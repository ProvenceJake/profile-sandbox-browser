use crate::{
    runtime,
    store::{atomic, Environment, Profile},
};
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{BufRead, BufReader, Write},
    net::IpAddr,
    os::{
        fd::AsRawFd,
        unix::{net::UnixStream, process::CommandExt},
    },
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Sender},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

type Pending = Arc<Mutex<HashMap<u64, Sender<Value>>>>;
type EventRoutes = Arc<Mutex<HashMap<String, Sender<(String, Value)>>>>;
type Writer = Arc<Mutex<UnixStream>>;
fn transmit(writer: &Writer, msg: &Value) -> Result<()> {
    let mut bytes = serde_json::to_vec(msg)?;
    bytes.push(0);
    writer.lock().unwrap().write_all(&bytes)?;
    Ok(())
}
fn preferences(dir: &Path, p: &Profile) -> Result<()> {
    let path = dir.join("Default/Preferences");
    fs::create_dir_all(path.parent().unwrap())?;
    let mut prefs: Value = if path.exists() {
        serde_json::from_slice(&fs::read(&path)?).context("Preferences 损坏，已保留原文件")?
    } else {
        json!({})
    };
    let base = p.env.locale.split('-').next().unwrap();
    let langs = if base == p.env.locale {
        base.to_string()
    } else {
        format!("{},{}", p.env.locale, base)
    };
    prefs["webrtc"]["ip_handling_policy"] = json!("disable_non_proxied_udp");
    prefs["webrtc"]["ip_handling_url"] = json!([]);
    prefs["webrtc"]["local_ips_allowed_urls"] = json!([]);
    prefs["intl"]["accept_languages"] = json!(langs);
    prefs["intl"]["selected_languages"] = json!(langs);
    let seed = uuid::Uuid::parse_str(&p.id)?.as_bytes()[0] as usize;
    let palette = [
        0xff336e62u32,
        0xff596aa5,
        0xff864e76,
        0xff92643c,
        0xff486f92,
        0xff627546,
    ];
    prefs["browser"]["theme"]["user_color2"] = json!(palette[seed % palette.len()] as i32);
    prefs["browser"]["theme"]["follows_system_colors"] = json!(false);
    prefs["profile"]["name"] = json!(p.name);
    prefs["session"]["restore_on_startup"] = json!(5);
    prefs["credentials_enable_service"] = json!(false);
    prefs["profile"]["password_manager_enabled"] = json!(false);
    atomic(&path, &serde_json::to_vec(&prefs)?)
}
fn check_lock(dir: &Path) -> Result<()> {
    let path = dir.join("SingletonLock");
    if let Ok(link) = fs::read_link(&path) {
        let text = link.to_string_lossy();
        let pid = text
            .rsplit('-')
            .next()
            .and_then(|s| s.parse::<i32>().ok())
            .filter(|p| *p > 0)
            .context("浏览器目录锁无效")?;
        if unsafe { libc::kill(pid, 0) } == 0
            || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
        {
            bail!("此 Profile 的浏览器仍在运行");
        }
        fs::remove_file(path)?;
    }
    Ok(())
}
pub struct Browser {
    pub child: Child,
    writer: Writer,
    pending: Pending,
    seq: Arc<AtomicU64>,
    rules: Arc<Mutex<Environment>>,
    sessions: Arc<Mutex<HashSet<String>>>,
    events: EventRoutes,
    pub env: Environment,
}
impl Browser {
    pub fn launch(exe: &Path, dir: &Path, p: &Profile, port: u16) -> Result<Self> {
        check_lock(dir)?;
        preferences(dir, p)?;
        let (writer, child_in) = UnixStream::pair()?;
        let (reader, child_out) = UnixStream::pair()?;
        writer.set_write_timeout(Some(Duration::from_secs(2)))?;
        // Keep source descriptors above 4 so dup2 cannot overwrite the other source.
        let duplicate = |fd: i32| -> Result<std::os::fd::OwnedFd> {
            use std::os::fd::FromRawFd;
            let n = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 10) };
            ensure!(n >= 0, "无法分配 CDP 管道");
            Ok(unsafe { std::os::fd::OwnedFd::from_raw_fd(n) })
        };
        let input = duplicate(child_in.as_raw_fd())?;
        let output = duplicate(child_out.as_raw_fd())?;
        let (a, b) = (input.as_raw_fd(), output.as_raw_fd());
        let mut cmd = Command::new(exe);
        cmd.env("TZ", &p.env.zone)
            .args([
                "--remote-debugging-pipe",
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-background-networking",
                "--disable-component-update",
                "--disable-sync",
                "--disable-quic",
                "--disable-features=Translate,MediaRouter,OptimizationHints,DnsOverHttps",
                "--proxy-bypass-list=<-loopback>",
                "--force-webrtc-ip-handling-policy=disable_non_proxied_udp",
                "--disk-cache-size=524288000",
                "--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1",
            ])
            .arg(format!("--user-data-dir={}", dir.display()))
            .arg(format!("--proxy-server=http://127.0.0.1:{port}"))
            .arg(format!("--lang={}", p.env.locale))
            .arg(format!(
                "--accept-lang={},{}",
                p.env.locale,
                p.env.locale.split('-').next().unwrap()
            ));
        #[cfg(test)]
        cmd.arg("--headless=new");
        cmd.arg("about:blank")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        unsafe {
            cmd.pre_exec(move || {
                if libc::dup2(a, 3) < 0 || libc::dup2(b, 4) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = cmd.spawn().context("Chrome 启动失败")?;
        let writer = Arc::new(Mutex::new(writer));
        let pending: Pending = Default::default();
        let seq = Arc::new(AtomicU64::new(1));
        let rules = Arc::new(Mutex::new(p.env.clone()));
        let sessions = Arc::new(Mutex::new(HashSet::<String>::new()));
        let events: EventRoutes = Default::default();
        let title_script = include_str!("window-title.js")
            .replace("PROFILE_NAME", &serde_json::to_string(&p.name)?);
        let browser = Self {
            child,
            sessions: sessions.clone(),
            events: events.clone(),
            writer: writer.clone(),
            pending: pending.clone(),
            seq: seq.clone(),
            rules: rules.clone(),
            env: p.env.clone(),
        };
        thread::spawn(move || {
            let mut reader = BufReader::new(reader);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                if !matches!(reader.read_until(0,&mut buf),Ok(n) if n>1) {
                    break;
                }
                buf.pop();
                let Ok(msg) = serde_json::from_slice::<Value>(&buf) else {
                    break;
                };
                if let Some(id) = msg["id"].as_u64() {
                    if let Some(tx) = pending.lock().unwrap().remove(&id) {
                        let _ = tx.send(msg);
                    }
                    continue;
                }
                let method = msg["method"].as_str().unwrap_or_default();
                if method == "Target.detachedFromTarget" {
                    if let Some(s) = msg["params"]["sessionId"].as_str() {
                        sessions.lock().unwrap().remove(s);
                    }
                }
                if matches!(
                    method,
                    "Page.loadEventFired"
                        | "Network.requestWillBeSentExtraInfo"
                        | "Network.requestWillBeSent"
                ) {
                    if let Some(s) = msg["sessionId"].as_str() {
                        if let Some(tx) = events.lock().unwrap().get(s) {
                            let _ = tx.send((method.into(), msg["params"].clone()));
                        }
                    }
                }
                if msg["method"] == "Target.attachedToTarget" {
                    let params = &msg["params"];
                    let session = &params["sessionId"];
                    let kind = params["targetInfo"]["type"].as_str().unwrap_or("");
                    let send = |method: &str, data: Value| {
                        transmit(
                            &writer,
                            &json!({"id":seq.fetch_add(1,Ordering::Relaxed),"method":method,"params":data,"sessionId":session}),
                        )
                    };
                    // The macOS application language and ICU's formatting locale are
                    // independent. Queue the ICU override before resuming each target.
                    if matches!(
                        kind,
                        "page"
                            | "iframe"
                            | "webview"
                            | "worker"
                            | "shared_worker"
                            | "service_worker"
                    ) {
                        let locale = rules.lock().unwrap().locale.clone();
                        if send("Emulation.setLocaleOverride", json!({"locale":locale})).is_err() {
                            break;
                        }
                    }
                    if matches!(kind, "page" | "iframe" | "webview") {
                        if let Some(id) = session.as_str() {
                            sessions.lock().unwrap().insert(id.into());
                        }
                        if kind == "page" {
                            let _ = send(
                                "Page.addScriptToEvaluateOnNewDocument",
                                json!({"source":title_script,"worldName":"Profile Harbor title"}),
                            );
                        }
                        let e = rules.lock().unwrap().clone();
                        let _ = send(
                            "Emulation.setGeolocationOverride",
                            json!({"latitude":e.lat,"longitude":e.lng,"accuracy":100}),
                        );
                        let _ = send(
                            "Target.setAutoAttach",
                            json!({"autoAttach":true,"flatten":true,"waitForDebuggerOnStart":true}),
                        );
                    }
                    let _ = send("Runtime.runIfWaitingForDebugger", json!({}));
                }
            }
            pending.lock().unwrap().clear();
        });
        browser.call("Target.setDiscoverTargets", json!({"discover":true}), None)?;
        browser.call(
            "Target.setAutoAttach",
            json!({"autoAttach":true,"flatten":true,"waitForDebuggerOnStart":true}),
            None,
        )?;
        Ok(browser)
    }
    fn call(&self, method: &str, params: Value, session: Option<&str>) -> Result<Value> {
        let id = self.seq.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        let mut msg = json!({"id":id,"method":method,"params":params});
        if let Some(s) = session {
            msg["sessionId"] = json!(s)
        }
        self.pending.lock().unwrap().insert(id, tx);
        let result = transmit(&self.writer, &msg).and_then(|_| {
            Ok(rx
                .recv_timeout(Duration::from_secs(
                    if matches!(method, "Page.navigate" | "Runtime.evaluate") {
                        12
                    } else {
                        4
                    },
                ))
                .context("浏览器控制管道未响应")?)
        });
        self.pending.lock().unwrap().remove(&id);
        let answer = result?;
        ensure!(answer.get("error").is_none(), "浏览器指令 {method} 未成功");
        Ok(answer["result"].clone())
    }
    pub fn tabs(&self) -> Result<usize> {
        Ok(
            self.call("Target.getTargets", json!({}), None)?["targetInfos"]
                .as_array()
                .context("标签页不可用")?
                .iter()
                .filter(|t| t["type"] == "page")
                .count(),
        )
    }
    pub fn check(&self, url: &str, ip: IpAddr) -> Result<()> {
        let target = self.call(
            "Target.createTarget",
            json!({"url":"about:blank","background":true}),
            None,
        )?["targetId"]
            .as_str()
            .context("自检标签页创建失败")?
            .to_owned();
        let result = (|| -> Result<()> {
            let session = self.call(
                "Target.attachToTarget",
                json!({"targetId":target,"flatten":true}),
                None,
            )?["sessionId"]
                .as_str()
                .context("自检控制失败")?
                .to_owned();
            let (tx, rx) = mpsc::channel();
            self.events.lock().unwrap().insert(session.clone(), tx);
            let _route = Route {
                routes: self.events.clone(),
                session: session.clone(),
            };
            self.call("Network.enable", json!({}), Some(&session))?;
            self.call(
                "Network.setCacheDisabled",
                json!({"cacheDisabled":true}),
                Some(&session),
            )?;
            self.call("Page.enable", json!({}), Some(&session))?;
            let nav = self.call("Page.navigate", json!({"url":url}), Some(&session))?;
            ensure!(nav.get("errorText").is_none(), "浏览器无法访问出口回显");
            let deadline = Instant::now() + Duration::from_secs(8);
            let mut loaded = false;
            let mut doc = None;
            let mut headers = HashMap::new();
            while !loaded || !doc.as_ref().is_some_and(|id| headers.contains_key(id)) {
                let (event, data) = rx
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .context("等待页面加载或请求头超时")?;
                match event.as_str() {
                    "Page.loadEventFired" => loaded = true,
                    "Network.requestWillBeSent" if data["type"] == "Document" => {
                        doc = data["requestId"].as_str().map(str::to_owned)
                    }
                    "Network.requestWillBeSentExtraInfo" => {
                        if let (Some(id), Some(map)) =
                            (data["requestId"].as_str(), data["headers"].as_object())
                        {
                            if let Some((_, lang)) = map
                                .iter()
                                .find(|(k, _)| k.eq_ignore_ascii_case("accept-language"))
                            {
                                headers
                                    .insert(id.to_owned(), lang.as_str().unwrap_or("").to_owned());
                            }
                        }
                    }
                    _ => {}
                }
            }
            ensure!(
                headers[&doc.unwrap()]
                    .split(',')
                    .next()
                    .is_some_and(|v| v.eq_ignore_ascii_case(&self.env.locale)),
                "浏览器 Accept-Language 与配置不一致"
            );
            let body = loop {
                let res=self.call("Runtime.evaluate",json!({"expression":"document.readyState === 'complete' && document.body?.innerText","returnByValue":true}),Some(&session))?;
                if let Some(s) = res["result"]["value"].as_str().filter(|s| !s.is_empty()) {
                    break s.to_owned();
                }
                ensure!(Instant::now() < deadline, "浏览器出口自检超时");
                thread::sleep(Duration::from_millis(80));
            };
            ensure!(
                runtime::extract(&body)?.0 == ip,
                "浏览器出口与启动前校验不一致"
            );
            // An app-owned blank target avoids third-party CSP affecting the worker probe.
            self.call(
                "Page.navigate",
                json!({"url":"about:blank"}),
                Some(&session),
            )?;
            let script = include_str!("environment-check.js");
            let result = self.call(
                "Runtime.evaluate",
                json!({"expression":script,"awaitPromise":true,"returnByValue":true}),
                Some(&session),
            )?;
            let rows = result["result"]["value"]
                .as_array()
                .filter(|v| v.len() == 2)
                .context("页面或 Worker 环境自检失败")?;
            let canonical=self.call("Runtime.evaluate",json!({"expression":format!("new Intl.DateTimeFormat('en',{{timeZone:{}}}).resolvedOptions().timeZone",serde_json::to_string(&self.env.zone)?),"returnByValue":true}),Some(&session))?["result"]["value"].clone();
            for row in rows {
                ensure!(row["zone"] == canonical, "浏览器时区与配置不一致");
                for field in ["language", "dateLocale", "numberLocale"] {
                    ensure!(
                        row[field]
                            .as_str()
                            .is_some_and(|v| v.eq_ignore_ascii_case(&self.env.locale)),
                        "浏览器语言或默认格式与配置不一致（{field}）"
                    );
                }
            }
            Ok(())
        })();
        let _ = self.call("Target.closeTarget", json!({"targetId":target}), None);
        result
    }
    #[cfg(test)]
    pub fn audit_locales(&self, origin: &str) -> Result<()> {
        let target = self.call("Target.createTarget", json!({"url":"about:blank"}), None)?;
        let session = self.call(
            "Target.attachToTarget",
            json!({"targetId":target["targetId"],"flatten":true}),
            None,
        )?;
        let id = session["sessionId"].as_str();
        self.call("Page.navigate", json!({"url":origin}), id)?;
        let script = r#"(async()=>{if(document.readyState!=="complete"){await new Promise(resolve=>window.addEventListener("load",resolve,{once:true}));}return probe.readFormatContexts();})()"#;
        let result=self.call("Runtime.evaluate",json!({"expression":script,"awaitPromise":true,"returnByValue":true,"userGesture":true}),id)?;
        let rows = result["result"]["value"]
            .as_object()
            .context("网页上下文自检失败")?;
        for ctx in [
            "page",
            "frame",
            "crossSiteFrame",
            "popup",
            "worker",
            "shared",
            "service",
        ] {
            let row = rows.get(ctx).context("缺少测试结果")?;
            ensure!(row["zone"] == self.env.zone, "{ctx} 时区错误");
            for field in ["language", "dateLocale", "numberLocale"] {
                ensure!(row[field] == self.env.locale, "{ctx} {field} 不匹配");
            }
        }
        self.call(
            "Target.closeTarget",
            json!({"targetId":target["targetId"]}),
            None,
        )?;
        Ok(())
    }

    #[cfg(test)]
    pub fn audit_ice(&self, origin: &str) -> Result<()> {
        let udp = std::net::UdpSocket::bind(("127.0.0.1", 0))?;
        udp.set_nonblocking(true)?;
        let target = self.call(
            "Target.createTarget",
            json!({"url":"about:blank","background":true}),
            None,
        )?;
        let attached = self.call(
            "Target.attachToTarget",
            json!({"targetId":target["targetId"],"flatten":true}),
            None,
        )?;
        self.call(
            "Page.navigate",
            json!({"url":origin}),
            attached["sessionId"].as_str(),
        )?;
        let script = include_str!("../../tests/fixtures/ice.js")
            .replace("STUN_PORT", &udp.local_addr()?.port().to_string());
        let ans = self.call(
            "Runtime.evaluate",
            json!({"expression":script,"awaitPromise":true,"returnByValue":true}),
            attached["sessionId"].as_str(),
        )?;
        let rows = ans["result"]["value"].as_array().with_context(|| {
            format!(
                "ICE 测试未完成 {}",
                ans["exceptionDetails"]["exception"]["description"]
            )
        })?;
        ensure!(
            rows.iter()
                .all(|v| v.as_str().is_some_and(|s| s.contains(" typ relay "))),
            "发现未经中继的 ICE candidate"
        );
        let mut buf = [0; 2048];
        ensure!(
            udp.recv_from(&mut buf)
                .is_err_and(|e| e.kind() == std::io::ErrorKind::WouldBlock),
            "发现绕过代理的 UDP"
        );
        self.call(
            "Target.closeTarget",
            json!({"targetId":target["targetId"]}),
            None,
        )?;
        Ok(())
    }
    #[cfg(test)]
    pub fn navigation_blocked(&self, url: &str) -> Result<bool> {
        let t = self.call(
            "Target.createTarget",
            json!({"url":"about:blank","background":true}),
            None,
        )?;
        let session = self.call(
            "Target.attachToTarget",
            json!({"targetId":t["targetId"],"flatten":true}),
            None,
        )?;
        let r = self.call(
            "Page.navigate",
            json!({"url":url}),
            session["sessionId"].as_str(),
        )?;
        self.call(
            "Target.closeTarget",
            json!({"targetId":t["targetId"]}),
            None,
        )?;
        Ok(r["errorText"].is_string())
    }
    pub fn ready_page(&self, p: &Profile) -> Result<()> {
        let targets = self.call("Target.getTargets", json!({}), None)?;
        let target = targets["targetInfos"]
            .as_array()
            .and_then(|rows| {
                rows.iter()
                    .find(|v| v["type"] == "page" && v["url"] == "about:blank")
            })
            .context("找不到首个浏览器标签页")?;
        let attached = self.call(
            "Target.attachToTarget",
            json!({"targetId":target["targetId"],"flatten":true}),
            None,
        )?;
        let session = attached["sessionId"].as_str().context("启动页会话不可用")?;
        let (tx, rx) = mpsc::channel();
        self.events.lock().unwrap().insert(session.into(), tx);
        let _route = Route {
            routes: self.events.clone(),
            session: session.into(),
        };
        self.call("Page.enable", json!({}), Some(session))?;
        let frame = self.call("Page.getFrameTree", json!({}), Some(session))?["frameTree"]["frame"]
            ["id"]
            .clone();
        let label = p
            .name
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        let html = include_str!("ready.html").replace("PROFILE_LABEL", &label);
        self.call(
            "Page.setDocumentContent",
            json!({"frameId":frame,"html":html}),
            Some(session),
        )?;
        let (event, _) = rx
            .recv_timeout(Duration::from_secs(4))
            .context("首页加载超时")?;
        ensure!(event == "Page.loadEventFired", "首页尚未就绪");
        self.call(
            "Target.detachFromTarget",
            json!({"sessionId":session}),
            None,
        )?;
        Ok(())
    }
    pub fn set_location(&mut self, e: &Environment) -> Result<()> {
        *self.rules.lock().unwrap() = e.clone();
        let sessions = self.sessions.lock().unwrap().clone();
        for s in sessions {
            let res = self.call(
                "Emulation.setGeolocationOverride",
                json!({"latitude":e.lat,"longitude":e.lng,"accuracy":100}),
                Some(&s),
            );
            if self.sessions.lock().unwrap().contains(&s) {
                res?;
            }
        }
        self.env = e.clone();
        Ok(())
    }
}
struct Route {
    routes: EventRoutes,
    session: String,
}
impl Drop for Route {
    fn drop(&mut self) {
        self.routes.lock().unwrap().remove(&self.session);
    }
}
impl Drop for Browser {
    fn drop(&mut self) {
        let _ = transmit(
            &self.writer,
            &json!({"id":self.seq.fetch_add(1,Ordering::Relaxed),"method":"Browser.close"}),
        );
        let until = Instant::now() + Duration::from_secs(2);
        while Instant::now() < until {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            thread::sleep(Duration::from_millis(25));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
