use crate::{
    browser::Browser,
    proxy::{Core, Gate},
    runtime,
    store::{self, Profile, Store},
    supervisor::{self, Sampler},
};
use anyhow::{anyhow, bail, ensure, Context, Result};
use rusqlite::params;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::mpsc::Sender,
    time::{Duration, Instant},
};
struct Live {
    browser: Browser,
    gate: Gate,
    slot: usize,
    launch: String,
    sample: Sampler,
    stats: Value,
    paused: bool,
}
pub struct Manager {
    pub store: Store,
    core: Option<Core>,
    live: HashMap<String, Live>,
    wake: Sender<()>,
    health: Instant,
    failed: u8,
    sampled: Instant,
    maintenance: Instant,
    engine: PathBuf,
    pub phase: Box<dyn Fn(&str, &str) + Send + Sync>,
}
pub fn field<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    v[k].as_str().with_context(|| format!("缺少字段 {k}"))
}
impl Manager {
    pub fn new(store: Store, wake: Sender<()>, engine: PathBuf) -> Result<Self> {
        supervisor::recover(&store.root.join("children"))?;
        for f in ["config.yaml", "nodes.yaml", "api.sock"] {
            let _ = fs::remove_file(store.root.join("engine").join(f));
        }
        store.db.execute(
            "UPDATE launches SET end_reason='管理器异常退出' WHERE end_reason IS NULL",
            [],
        )?;
        Ok(Self {
            store,
            core: None,
            live: HashMap::new(),
            wake,
            health: Instant::now(),
            failed: 0,
            sampled: Instant::now(),
            maintenance: Instant::now(),
            engine,
            phase: Box::new(|_, _| {}),
        })
    }
    fn track(&self, pid: u32, marker: &Path) -> Result<()> {
        supervisor::register(
            &self.store.root.join("children"),
            pid,
            &marker.to_string_lossy(),
            self.wake.clone(),
        )
    }
    fn forget(&self, pid: u32) {
        let _ = fs::remove_file(self.store.root.join("children").join(pid.to_string()));
    }
    fn ensure_core(&mut self) -> Result<()> {
        if self.core.is_none() {
            let core = Core::launch(&self.store, &self.engine)?;
            self.track(core.child.id(), &self.store.root.join("engine"))?;
            self.core = Some(core);
            self.health = Instant::now();
            self.failed = 0;
        }
        Ok(())
    }
    fn close_core(&mut self) {
        if let Some(core) = self.core.take() {
            let pid = core.child.id();
            drop(core);
            self.forget(pid)
        }
    }
    fn end(&mut self, key: &str, reason: &str) {
        if let Some(l) = self.live.remove(key) {
            l.gate.set(None);
            if let Some(c) = self.core.as_mut() {
                let _ = c.reject(l.slot);
                c.idle = Instant::now();
            }
            let pid = l.browser.child.id();
            let _ = self.store.db.execute(
                "UPDATE launches SET end_reason=? WHERE id=?",
                params![reason, l.launch],
            );
            drop(l);
            self.forget(pid);
            self.store.event(key, reason);
        }
    }
    pub fn shutdown(&mut self) {
        for k in self.live.keys().cloned().collect::<Vec<_>>() {
            self.end(&k, "管理器退出");
        }
        self.close_core();
    }
    fn idle(&self, k: &str) -> Result<()> {
        ensure!(!self.live.contains_key(k), "请先停止此 Profile");
        Ok(())
    }
    fn record_error(&self, k: &str, e: &anyhow::Error) {
        if let Ok(mut p) = self.store.profile(k) {
            p.error = e.to_string();
            let _ = self.store.save(&p);
        }
        self.store.event(k, &e.to_string());
    }
    pub fn snapshot(&self) -> Result<Value> {
        let mut profiles = vec![];
        for p in self.store.profiles()? {
            let mut v = json!(p);
            v["state"] = json!(if let Some(l) = self.live.get(&p.id) {
                if l.paused {
                    "Paused"
                } else {
                    "Running"
                }
            } else if p.error.is_empty() {
                "Stopped"
            } else {
                "Error"
            });
            if let Some(l) = self.live.get(&p.id) {
                v["metrics"] = l.stats.clone();
                v["slot"] = json!(l.slot + 1);
            }
            profiles.push(v);
        }
        Ok(
            json!({"profiles":profiles,"trash":self.store.records("SELECT id,json_extract(data,'$.name') AS name,deleted FROM profiles WHERE deleted IS NOT NULL ORDER BY deleted DESC")?,"nodes":self.store.nodes()?,"sources":self.store.sources()?,"chrome":runtime::chrome(&self.store).is_file(),"chromePath":runtime::chrome(&self.store),"managed":self.store.setting("browser","system")=="managed","geo":self.store.root.join("runtime/city.mmdb").is_file(),"core":self.core.is_some(),"echo":self.store.setting("echo",runtime::TRACE),"fallback":self.store.setting("fallback",runtime::IPIFY),"events":self.store.records("SELECT ts,profile,detail FROM events ORDER BY rowid DESC LIMIT 80")?,"launches":self.store.records("SELECT profile,started,ready_ms,verify_ms,ip,country,slot,end_reason FROM launches ORDER BY rowid DESC LIMIT 80")?}),
        )
    }
    pub fn act(&mut self, op: &str, v: Value) -> Result<Value> {
        let key = v["id"].as_str().unwrap_or("");
        match op {
            "snapshot" => return self.snapshot(),
            "metrics" => {
                if !key.is_empty() {
                    store::valid_id(key)?;
                }
                let samples=self.store.records_with("SELECT m.ts,m.memory_mb,m.cpu,m.tabs,l.profile FROM metrics m JOIN launches l ON m.launch=l.id WHERE (?1='' OR l.profile=?1) ORDER BY m.ts DESC LIMIT 200",[key])?;
                let mut q=self.store.db.prepare("SELECT ready_ms FROM launches WHERE ready_ms IS NOT NULL AND (?1='' OR profile=?1) AND started>=?2 ORDER BY ready_ms")?;
                let times = q
                    .query_map(params![key, store::now() - 30 * 86400], |r| {
                        r.get::<_, i64>(0)
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let rank = |percent: usize| {
                    times
                        .get((times.len() * percent).div_ceil(100).saturating_sub(1))
                        .copied()
                };
                return Ok(
                    json!({"samples":samples,"n":times.len(),"p50":rank(50),"p95":rank(95)}),
                );
            }
            "save" => {
                let node = field(&v, "node")?;
                ensure!(self.store.has_node(node)?, "请先添加并选择一个节点");
                let mut p = if key.is_empty() {
                    Profile {
                        id: store::id(),
                        name: String::new(),
                        node: String::new(),
                        auto: true,
                        env: Default::default(),
                        ip: String::new(),
                        country: String::new(),
                        version: String::new(),
                        error: String::new(),
                        used: 0,
                    }
                } else {
                    self.idle(key)?;
                    self.store.profile(key)?
                };
                p.name = store::name(field(&v, "name")?)?;
                p.node = node.into();
                p.auto = v["auto"].as_bool().context("请选择环境模式")?;
                if !p.auto {
                    p.env = serde_json::from_value(v["env"].clone())?;
                    p.env.validate()?;
                }
                p.error.clear();
                self.store.save(&p)?;
            }
            "clone" => {
                let mut p = self.store.profile(key)?;
                p.id = store::id();
                p.name = store::name(&format!(
                    "{} 副本",
                    p.name.chars().take(95).collect::<String>()
                ))?;
                p.ip.clear();
                p.country.clear();
                p.version.clear();
                p.error.clear();
                p.used = 0;
                self.store.save(&p)?;
            }
            "delete" => {
                self.idle(key)?;
                self.store.profile(key)?;
                self.store.db.execute(
                    "UPDATE profiles SET deleted=? WHERE id=?",
                    params![store::now(), key],
                )?;
                self.store.maintenance()?;
            }
            "restore" | "purge" => {
                store::valid_id(key)?;
                let exists: bool = self.store.db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM profiles WHERE id=? AND deleted IS NOT NULL)",
                    [key],
                    |r| r.get(0),
                )?;
                ensure!(exists, "回收目录中找不到此 Profile");
                self.store.maintenance()?;
                if op == "purge" {
                    self.store.db.execute(
                        "UPDATE profiles SET deleted=0 WHERE id=? AND deleted IS NOT NULL",
                        [key],
                    )?;
                    self.store.maintenance()?;
                } else {
                    let from = self.store.root.join("trash").join(key);
                    if from.exists() {
                        fs::rename(&from, self.store.root.join("profiles").join(key))?;
                    }
                    ensure!(
                        self.store.db.execute(
                            "UPDATE profiles SET deleted=NULL WHERE id=? AND deleted IS NOT NULL",
                            [key]
                        )? == 1,
                        "保留期限已过"
                    );
                }
            }
            "source" | "refresh" | "removeSource" => {
                let source = if key.is_empty() {
                    store::id()
                } else {
                    store::valid_id(key)?.into()
                };
                let affected = self
                    .store
                    .nodes()?
                    .into_iter()
                    .filter(|n| n["source"] == source)
                    .map(|n| n["id"].as_str().unwrap().to_owned())
                    .collect::<Vec<_>>();
                let keys = self
                    .live
                    .keys()
                    .filter(|k| {
                        self.store
                            .profile(k)
                            .is_ok_and(|p| affected.contains(&p.node))
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                match op {
                    "source" => {
                        let url = v["url"].as_str().filter(|s| !s.trim().is_empty());
                        let body = if let Some(url) = url {
                            store::fetch_source(url)?
                        } else {
                            field(&v, "yaml")?.to_owned()
                        };
                        self.store.import(&source, field(&v, "name")?, url, &body)?;
                    }
                    "refresh" => self.store.refresh(&source)?,
                    _ => self.store.remove_source(&source)?,
                }
                for k in &keys {
                    if let Some(l) = self.live.get_mut(k) {
                        l.gate.set(None);
                        l.paused = true;
                    }
                }
                if self.store.nodes()?.is_empty() {
                    self.shutdown();
                }
                if let Some(c) = &self.core {
                    if let Err(e) = c.refresh(&self.store) {
                        self.shutdown();
                        bail!("节点已保存。内核刷新失败，已关闭会话：{e}")
                    }
                }
                for k in keys {
                    let p = self.store.profile(&k)?;
                    if !self.store.has_node(&p.node)? {
                        self.end(&k, "绑定节点已从来源移除");
                        self.record_error(&k, &anyhow!("绑定节点已移除，请重新选择"));
                    } else if let Err(e) = self.transition(&k, &p.node, "", "") {
                        self.end(&k, "更新后校验失败");
                        self.record_error(&k, &e);
                    }
                }
            }
            "start" | "switch" => {
                let p = self.store.profile(key)?;
                if op == "start" {
                    self.idle(key)?;
                } else {
                    ensure!(self.live.contains_key(key), "Profile 已停止");
                }
                let node = if op == "switch" {
                    field(&v, "node")?
                } else {
                    &p.node
                };
                let result = self.transition(
                    key,
                    node,
                    v["decision"].as_str().unwrap_or(""),
                    v["confirmed"].as_str().unwrap_or(""),
                );
                match result {
                    Ok(value) => return Ok(value),
                    Err(e) => {
                        self.end(key, "校验失败");
                        self.record_error(key, &e);
                        return Err(e);
                    }
                }
            }
            "stop" => {
                self.end(key, "用户停止");
            }
            "systemChrome" => {
                self.store.setting_set("browser", "system")?;
            }
            "settings" => {
                let echo = field(&v, "echo")?;
                let fallback = field(&v, "fallback")?;
                store::checked_url(echo)?;
                if !fallback.is_empty() {
                    store::checked_url(fallback)?;
                }
                self.store.setting_set("echo", echo)?;
                self.store.setting_set("fallback", fallback)?;
            }
            "geo" => runtime::install_geo(&self.store.root)?,
            "chrome" => runtime::install_chrome(&self.store.root)?,
            _ => bail!("未知操作"),
        }
        Ok(json!({"ok":true}))
    }
    fn transition(
        &mut self,
        key: &str,
        node: &str,
        decision: &str,
        confirmed: &str,
    ) -> Result<Value> {
        let started = Instant::now();
        (self.phase)(key, "正在校验代理出口");
        let mut p = self.store.profile(key)?;
        ensure!(self.store.has_node(node)?, "绑定节点不可用");
        if (p.auto || decision == "update") && !self.store.root.join("runtime/city.mmdb").is_file()
        {
            runtime::install_geo(&self.store.root)?
        }
        let slot = if let Some(l) = self.live.get_mut(key) {
            l.gate.set(None);
            l.paused = true;
            l.slot
        } else {
            (0..5)
                .find(|i| !self.live.values().any(|l| l.slot == *i))
                .context("最多可同时运行 5 个 Profile")?
        };
        self.ensure_core()?;
        let c = self.core.as_ref().unwrap();
        c.reject(slot)?;
        c.select(slot, node)?;
        let port = c.ports[slot];
        let result = (|| -> Result<Value> {
            let (ip, hint, url) = runtime::verify_exit(&self.store, port)?;
            let geo = runtime::locate(&self.store, ip);
            let (inferred, country) = if p.auto || decision == "update" {
                geo?
            } else {
                match geo {
                    Ok(pair) => pair,
                    Err(_) => (p.env.clone(), hint),
                }
            };
            let country_changed =
                !p.country.is_empty() && !country.is_empty() && country != p.country;
            let zone_changed = p.auto && !p.ip.is_empty() && inferred.zone != p.env.zone;
            let changed = country_changed || zone_changed;
            let fingerprint = if zone_changed {
                format!("{country}/{}", inferred.zone)
            } else {
                country.clone()
            };
            let confirmed_now = confirmed == fingerprint && matches!(decision, "update" | "keep");
            if changed && !confirmed_now {
                self.store.event(key, "出口地区变化，等待确认");
                return Ok(
                    json!({"confirm":true,"previous":p.country,"previousZone":p.env.zone,"newZone":inferred.zone,"confirmed":fingerprint,"country":country,"ip":ip.to_string(),"node":node}),
                );
            }
            if (p.auto && decision != "keep") || (changed && decision == "update" && confirmed_now)
            {
                p.auto = true;
                p.env = inferred;
            }
            if changed && decision == "keep" {
                p.auto = false;
            }
            p.env.validate()?;
            let verify_ms = started.elapsed().as_millis() as i64;
            let exe = runtime::chrome(&self.store);
            if !exe.is_file() {
                runtime::install_chrome(&self.store.root)?;
            }
            (self.phase)(key, "正在验证浏览器签名及版本");
            let exe = runtime::chrome(&self.store);
            let ver = runtime::verify_chrome(&exe)?;
            if !p.version.is_empty() {
                ensure!(
                    runtime::version(&ver)? >= runtime::version(&p.version)?,
                    "此 Profile 曾使用较新的 Chrome，禁止降级"
                );
            }
            p.version = ver;
            p.node = node.into();
            p.ip = ip.to_string();
            p.country = country;
            p.error.clear();
            p.used = store::now();
            // Persist the opened version before Chrome can migrate its data directory.
            let mut guard = self.store.profile(key)?;
            guard.version = p.version.clone();
            self.store.save(&guard)?;
            let needs_restart = self.live.get(key).is_some_and(|l| {
                l.browser.env.zone != p.env.zone || l.browser.env.locale != p.env.locale
            });
            if needs_restart {
                self.end(key, "地区变更，重新启动浏览器");
                self.core.as_ref().unwrap().select(slot, node)?;
            }
            (self.phase)(key, "正在检查浏览器出口、时区和语言");
            if let Some(l) = self.live.get_mut(key) {
                l.browser.set_location(&p.env)?;
                l.gate.set(Some(port));
                l.browser.check(&url, ip)?;
                l.paused = false;
                self.store.db.execute(
                    "UPDATE launches SET ip=?,country=? WHERE id=?",
                    params![p.ip, p.country, l.launch],
                )?;
            } else {
                let gate = Gate::new()?;
                gate.set(Some(port));
                let dir = self.store.root.join("profiles").join(key);
                let browser = Browser::launch(&exe, &dir, &p, gate.port)?;
                let pid = browser.child.id();
                self.track(pid, &dir)?;
                if let Err(e) = browser.check(&url, ip).and_then(|_| browser.ready_page(&p)) {
                    drop(browser);
                    self.forget(pid);
                    return Err(e);
                }
                let launch = store::id();
                let ms = started.elapsed().as_millis() as i64;
                self.store.db.execute("INSERT INTO launches(id,profile,started,ready_ms,verify_ms,ip,country,slot) VALUES(?,?,?,?,?,?,?,?)",params![launch,key,p.used,ms,verify_ms,p.ip,p.country,slot+1])?;
                let mut sample = Sampler::new();
                let (memory, _) = sample.sample(pid as i32);
                self.live.insert(
                    key.into(),
                    Live {
                        browser,
                        gate,
                        slot,
                        launch,
                        sample,
                        stats: json!({"memory":memory,"cpu":0,"tabs":1,"ready":ms}),
                        paused: false,
                    },
                );
            }
            self.store.save(&p)?;
            self.store.event(key, "出口与浏览器环境自检通过");
            Ok(json!({"ok":true}))
        })();
        if result.as_ref().map_or(true, |v| v["confirm"] == true) {
            if let Some(c) = self.core.as_mut() {
                let _ = c.reject(slot);
                c.idle = Instant::now();
            }
        }
        result
    }
    pub fn tick(&mut self) -> Result<bool> {
        let mut changed = false;
        let ended = self
            .live
            .iter_mut()
            .filter_map(|(k, l)| {
                l.browser
                    .child
                    .try_wait()
                    .ok()
                    .flatten()
                    .map(|s| (k.clone(), s.success()))
            })
            .collect::<Vec<_>>();
        for (k, normal) in ended {
            let reason = if normal {
                "浏览器已关闭"
            } else {
                "浏览器异常退出"
            };
            self.end(&k, reason);
            if !normal {
                self.record_error(&k, &anyhow!(reason));
            }
            changed = true;
        }
        if self.maintenance.elapsed() > Duration::from_secs(3600) {
            self.store.maintenance()?;
            self.maintenance = Instant::now();
        }
        let Some(c) = self.core.as_mut() else {
            return Ok(changed);
        };
        if self.live.is_empty() && c.idle.elapsed() > Duration::from_secs(300) {
            self.close_core();
            return Ok(true);
        }
        let dead = c.child.try_wait()?.is_some();
        if self.health.elapsed() > Duration::from_secs(30) {
            self.health = Instant::now();
            if c.api("GET", "/version", None).is_err() {
                self.failed += 1
            } else {
                self.failed = 0
            }
        }
        if dead || self.failed >= 2 {
            let keys = self
                .live
                .iter()
                .filter(|(_, l)| !l.paused)
                .map(|(k, _)| k.clone())
                .collect::<Vec<_>>();
            for l in self.live.values_mut() {
                l.gate.set(None);
                l.paused = true;
            }
            self.close_core();
            self.store
                .event("", "代理内核中断，重新验证运行中的 Profile");
            for k in keys {
                let p = self.store.profile(&k)?;
                if let Err(e) = self.transition(&k, &p.node, "", "") {
                    self.end(&k, "代理恢复失败");
                    self.record_error(&k, &e);
                }
            }
            return Ok(true);
        }
        if self.sampled.elapsed() > Duration::from_secs(10) {
            self.sampled = Instant::now();
            for l in self.live.values_mut() {
                let (memory, cpu) = l.sample.sample(l.browser.child.id() as i32);
                let tabs = l.browser.tabs().unwrap_or(0);
                l.stats["memory"] = json!(memory);
                l.stats["cpu"] = json!(cpu);
                l.stats["tabs"] = json!(tabs);
                self.store.db.execute(
                    "INSERT INTO metrics VALUES(?,?,?,?,?)",
                    params![l.launch, store::now(), memory, cpu, tabs],
                )?;
                changed = true;
            }
        }
        Ok(changed)
    }
}
impl Drop for Manager {
    fn drop(&mut self) {
        self.shutdown()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    #[test]
    #[ignore = "requires macOS Chrome, Keychain, local fixture nodes and bundled Mihomo; run tests/desktop.mjs"]
    fn desktop_flow() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::open(temp.path().to_path_buf())?;
        let (tx, _rx) = mpsc::channel();
        let engine = std::env::var("HARBOR_TEST_ENGINE").context("set HARBOR_TEST_ENGINE")?;
        let mut m = Manager::new(store, tx, engine.into())?;
        let source = store::id();
        let result = (|| -> Result<()> {
            let ports = std::env::var("HARBOR_TEST_PORTS")?
                .split(',')
                .map(|s| s.parse::<u16>())
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let echoes = format!("http://localhost:{}/trace", ports[0]);
            m.act("settings", json!({"echo":echoes,"fallback":""}))?;
            let mut proxies=ports.iter().enumerate().map(|(i,p)|json!({"name":format!("Node {i}"),"server":"127.0.0.1","port":p,"type":"http"})).collect::<Vec<_>>();
            m.act("source",json!({"id":source,"name":"Test subscription","url":std::env::var("HARBOR_TEST_SUB")?}))?;
            let first = m.store.nodes()?;
            m.act("refresh", json!({"id":source}))?;
            assert_eq!(first, m.store.nodes()?);
            let nodes = m.store.nodes()?;
            let node = |i: usize| nodes[i]["id"].as_str().unwrap();
            let mut ids = vec![];
            let environments = [
                ("Asia/Tokyo", "ja-JP"),
                ("America/Toronto", "fr-CA"),
                ("Europe/Berlin", "de-DE"),
                ("America/Sao_Paulo", "pt-BR"),
                ("America/New_York", "en-US"),
            ];
            for (i, (zone, language)) in environments.into_iter().enumerate() {
                m.act("save",json!({"name":format!("Browser {i}"),"node":node(i),"auto":false,"env":{"zone":zone,"locale":language,"lat":35.68,"lng":139.69}}))?;
                let id = m.store.profiles()?.last().unwrap().id.clone();
                m.act("start", json!({"id":id}))?;
                assert!(m.live.contains_key(&id));
                ids.push(id);
            }
            for id in &ids {
                m.live[id]
                    .browser
                    .audit_locales(&format!("http://localhost:{}/", ports[0]))?;
            }
            let k = &ids[0];
            m.act("clone", json!({"id":k}))?;
            let sixth = m.store.profiles()?.last().unwrap().id.clone();
            assert!(m.act("start", json!({"id":sixth})).is_err());
            assert_eq!(m.live.len(), 5);
            m.live[k]
                .browser
                .audit_ice(&format!("http://localhost:{}/", ports[0]))?;
            let hostname = format!("{}.harbor-probe.invalid", store::id());
            m.live[k]
                .browser
                .check(&format!("http://{hostname}/trace"), "9.9.9.1".parse()?)?;
            let original = m.live[k].browser.child.id();
            m.act("switch", json!({"id":k,"node":node(1)}))?;
            assert_eq!(original, m.live[k].browser.child.id());
            let confirm = m.act("switch", json!({"id":k,"node":node(2)}))?;
            assert_eq!(confirm["confirm"], true);
            assert!(m.live[k].paused);
            m.act(
                "switch",
                json!({"id":k,"node":node(2),"decision":"keep","confirmed":"FR"}),
            )?;
            assert!(!m.live[k].paused);
            m.sampled = Instant::now() - Duration::from_secs(11);
            m.tick()?;
            assert!(m.live[k].stats["memory"].as_f64().unwrap() > 10.);
            m.core.as_mut().unwrap().child.kill()?;
            m.core.as_mut().unwrap().child.wait()?;
            let v6 = std::env::var("HARBOR_TEST_V6")?;
            let direct = reqwest::blocking::Client::builder()
                .no_proxy()
                .build()?
                .get(format!("http://[::1]:{v6}/"))
                .send()?
                .text()?;
            assert_eq!(direct, "reachable direct IPv6 origin");
            assert!(m.live[k]
                .browser
                .navigation_blocked(&format!("http://[::1]:{v6}/"))?);
            assert!(m.live[k].browser.navigation_blocked(&echoes)?);
            m.tick()?;
            assert_eq!(m.live.len(), 5);
            assert!(!m.live[k].paused);
            proxies.retain(|v| v["name"] != "Node 2");
            m.act("source",json!({"id":source,"name":"Updated","yaml":serde_json::to_string(&json!({"proxies":proxies}))?}))?;
            assert!(!m.live.contains_key(k));
            assert!(m.live.contains_key(&ids[1]));
            m.shutdown();
            m.act("delete", json!({"id":k}))?;
            assert!(temp.path().join("trash").join(k).is_dir());
            m.act("restore", json!({"id":k}))?;
            assert!(temp.path().join("profiles").join(k).is_dir());
            m.act("delete", json!({"id":k}))?;
            m.act("purge", json!({"id":k}))?;
            assert!(!temp.path().join("trash").join(k).exists());
            assert!(m.snapshot()?["launches"].as_array().unwrap().len() >= 5);
            Ok(())
        })();
        m.shutdown();
        m.store.remove_source(&source)?;
        result
    }
    #[test]
    #[ignore = "invoked as an isolated child by desktop_flow_crash"]
    fn sacrificial_manager() -> Result<()> {
        let root = PathBuf::from(std::env::var("HARBOR_CRASH_ROOT")?);
        let (tx, _rx) = mpsc::channel();
        let mut m = Manager::new(
            Store::open(root.clone())?,
            tx,
            std::env::var("HARBOR_TEST_ENGINE")?.into(),
        )?;
        let p = m.store.profiles()?.remove(0);
        m.act("start", json!({"id":p.id}))?;
        let pids = [
            m.core.as_ref().unwrap().child.id(),
            m.live[&p.id].browser.child.id(),
        ];
        store::atomic(&root.join("ready.json"), &serde_json::to_vec(&pids)?)?;
        std::thread::sleep(Duration::from_secs(60));
        bail!("fixture timed out")
    }
    #[test]
    #[ignore = "requires the local desktop fixtures"]
    fn desktop_flow_crash() -> Result<()> {
        use std::{
            process::{Command, Stdio},
            thread,
        };
        let tmp = tempfile::tempdir()?;
        let root = tmp.path();
        let source = store::id();
        let (tx, _rx) = mpsc::channel();
        let mut m = Manager::new(
            Store::open(root.into())?,
            tx,
            std::env::var("HARBOR_TEST_ENGINE")?.into(),
        )?;
        let port = std::env::var("HARBOR_TEST_PORTS")?
            .split(',')
            .next()
            .unwrap()
            .parse::<u16>()?;
        m.act("source",json!({"id":source,"name":"Guard test","yaml":serde_json::to_string(&json!({"name":"local","type":"http","server":"127.0.0.1","port":port}))?}))?;
        let result = (|| -> Result<()> {
            m.act(
                "settings",
                json!({"echo":format!("http://localhost:{port}/trace"),"fallback":""}),
            )?;
            m.act("save",json!({"name":"Crash fixture","node":m.store.nodes()?[0]["id"],"auto":false,"env":{"zone":"UTC","locale":"en-US","lat":0,"lng":0}}))?;
            let mut child = Command::new(std::env::current_exe()?)
                .args([
                    "--exact",
                    "manager::tests::sacrificial_manager",
                    "--ignored",
                ])
                .env("HARBOR_CRASH_ROOT", root)
                .stdout(Stdio::null())
                .spawn()?;
            let deadline = Instant::now() + Duration::from_secs(25);
            while !root.join("ready.json").is_file() {
                if child.try_wait()?.is_some() {
                    bail!("临时管理器未启动")
                }
                if Instant::now() > deadline {
                    child.kill()?;
                    child.wait()?;
                    bail!("临时管理器启动超时")
                }
                thread::sleep(Duration::from_millis(50));
            }
            let pids: Vec<i32> = serde_json::from_slice(&fs::read(root.join("ready.json"))?)?;
            child.kill()?;
            child.wait()?;
            for _ in 0..100 {
                if pids.iter().all(|p| supervisor::usage(*p).is_none()) {
                    return Ok(());
                }
                thread::sleep(Duration::from_millis(50));
            }
            bail!("管理器被杀后仍有子进程")
        })();
        m.store.remove_source(&source)?;
        result
    }
}
