use crate::store::{atomic, Store};
use anyhow::{ensure, Context, Result};
use reqwest::blocking::Client;
use serde_json::{json, Value};
use std::{
    fs,
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::LazyLock,
    thread,
    time::{Duration, Instant},
};
use tokio::{net::TcpListener as AsyncListener, runtime::Runtime, sync::watch};
static NET: LazyLock<Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
});

// Each browser keeps its own loopback endpoint while the verified upstream can change.
// Every state transition invalidates existing streams before accepting a new destination.
pub struct Gate {
    pub port: u16,
    state: watch::Sender<Option<u16>>,
    task: tokio::task::JoinHandle<()>,
}
impl Gate {
    pub fn new() -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let (state, rx) = watch::channel(None::<u16>);
        let task=NET.spawn(async move{
            let Ok(l)=AsyncListener::from_std(listener) else{return};
            while let Ok((mut front,_))=l.accept().await{
                let mut generation=rx.clone();let upstream=*generation.borrow_and_update();
                if let Some(port)=upstream {tokio::spawn(async move{
                    tokio::select! {biased; _=generation.changed()=>{}, _=async {
                        if let Ok(mut back)=tokio::net::TcpStream::connect(("127.0.0.1",port)).await{let _=tokio::io::copy_bidirectional(&mut front,&mut back).await;}
                    }=>{} }
                });}
            }
        });
        Ok(Self { port, state, task })
    }
    pub fn set(&self, port: Option<u16>) {
        self.state.send_replace(port);
    }
}
impl Drop for Gate {
    fn drop(&mut self) {
        self.set(None);
        self.task.abort();
    }
}
pub struct Core {
    guard: Option<Child>,
    pub child: Child,
    pub ports: Vec<u16>,
    client: Client,
    dir: PathBuf,
    pub idle: Instant,
}
impl Core {
    pub fn launch(store: &Store, exe: &Path) -> Result<Self> {
        let dir = store.root.join("engine");
        let sockets = (0..5)
            .map(|_| TcpListener::bind(("127.0.0.1", 0)))
            .collect::<std::io::Result<Vec<_>>>()?;
        let ports = sockets
            .iter()
            .map(|s| Ok(s.local_addr()?.port()))
            .collect::<std::io::Result<Vec<_>>>()?;
        let socket = dir.join("api.sock");
        let _ = fs::remove_file(&socket);
        let selectors=(0..5).map(|i|json!({"name":format!("SLOT_{i}"),"type":"select","proxies":["REJECT"],"use":["nodes"]})).collect::<Vec<_>>();
        let listeners=ports.iter().enumerate().map(|(i,p)|json!({"name":format!("slot-{i}"),"type":"mixed","listen":"127.0.0.1","port":p,"proxy":format!("SLOT_{i}")})).collect::<Vec<_>>();
        let config = json!({"external-controller-unix":socket,"mode":"rule","log-level":"silent","geo-auto-update":false,"tun":{"enable":false},"dns":{"enable":false},"sniffer":{"enable":false},"profile":{"store-selected":false},"proxy-providers":{"nodes":{"type":"file","path":"./nodes.yaml"}},"proxy-groups":selectors,"listeners":listeners,"rules":["MATCH,REJECT"]});
        atomic(
            &dir.join("nodes.yaml"),
            &serde_json::to_vec(&store.provider()?)?,
        )?;
        atomic(&dir.join("config.yaml"), &serde_json::to_vec(&config)?)?;
        let client = Client::builder()
            .unix_socket(socket)
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()?;
        drop(sockets);
        let child = Command::new(exe)
            .arg("-d")
            .arg(&dir)
            .arg("-f")
            .arg(dir.join("config.yaml"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("Mihomo 启动失败")?;
        let mut core = Self {
            guard: None,
            child,
            ports,
            client,
            dir,
            idle: Instant::now(),
        };
        let pid = core.child.id();
        let start = crate::supervisor::usage(pid as i32)
            .context("无法校验 Mihomo 的进程身份")?
            .ri_proc_start_abstime;
        let sidecar = exe.with_file_name("profile-harbor-guard");
        core.guard = Some(
            Command::new(sidecar)
                .arg(pid.to_string())
                .arg(start.to_string())
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .context("代理进程守护启动失败")?,
        );
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline && core.child.try_wait()?.is_none() {
            if core.api("GET", "/version", None).is_ok()
                && (0..5).all(|i| {
                    core.api("GET", &format!("/proxies/SLOT_{i}"), None)
                        .is_ok_and(|v| v["now"] == "REJECT")
                })
            {
                return Ok(core);
            }
            thread::sleep(Duration::from_millis(60));
        }
        anyhow::bail!("Mihomo 槽位初始化失败，请检查节点配置")
    }
    pub fn api(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
        let mut req = self
            .client
            .request(method.parse()?, format!("http://localhost{path}"));
        if let Some(v) = body {
            req = req.json(&v)
        }
        let res = req.send()?.error_for_status()?;
        let bytes = res.bytes()?;
        if bytes.is_empty() {
            Ok(Value::Null)
        } else {
            Ok(serde_json::from_slice(&bytes)?)
        }
    }
    pub fn select(&self, slot: usize, node: &str) -> Result<()> {
        let path = format!("/proxies/SLOT_{slot}");
        self.api("PUT", &path, Some(json!({"name":node})))?;
        ensure!(
            self.api("GET", &path, None)?["now"] == node,
            "槽位选择未生效"
        );
        Ok(())
    }
    pub fn reject(&self, slot: usize) -> Result<()> {
        self.select(slot, "REJECT")?;
        let tag = format!("slot-{slot}");
        if let Some(rows) = self.api("GET", "/connections", None)?["connections"].as_array() {
            for r in rows.iter().filter(|r| r["metadata"]["inboundName"] == tag) {
                if let Some(id) = r["id"].as_str() {
                    self.api("DELETE", &format!("/connections/{id}"), None)?;
                }
            }
        }
        Ok(())
    }
    pub fn refresh(&self, store: &Store) -> Result<()> {
        atomic(
            &self.dir.join("nodes.yaml"),
            &serde_json::to_vec(&store.provider()?)?,
        )?;
        self.api("PUT", "/providers/proxies/nodes", None)?;
        Ok(())
    }
}
impl Drop for Core {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(mut guard) = self.guard.take() {
            drop(guard.stdin.take());
            let _ = guard.wait();
        }
        for f in ["nodes.yaml", "config.yaml", "api.sock"] {
            let _ = fs::remove_file(self.dir.join(f));
        }
    }
}
