use crate::{runtime, store::Profile};
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    net::IpAddr,
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

pub struct Interactive {
    pub child: Child,
}

impl Interactive {
    pub fn launch(
        helper_dir: &Path,
        root: &Path,
        p: &Profile,
        gate: u16,
        echo: &str,
        ip: IpAddr,
    ) -> Result<Self> {
        let dir = root.join("profiles").join(&p.id).join("obscura");
        fs::create_dir_all(&dir)?;
        let binary = runtime::verified_obscura(root)?;
        let mut viewer = Self {
            child: Command::new(helper_dir.join("profile-harbor-viewer"))
                .arg(&dir)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .context("无法打开 Obscura 浏览窗口")?,
        };
        let cfg = json!({"name":p.name,"zone":p.env.zone,"locale":p.env.locale,"echo":echo,"ip":ip.to_string(),"directory":dir,
            "binary":binary,"guardBinary":helper_dir.join("profile-harbor-guard"),"port":gate});
        let input = viewer.child.stdin.as_mut().context("浏览窗口输入不可用")?;
        writeln!(input, "{cfg}")?;
        let output = viewer.child.stdout.take().context("浏览窗口输出不可用")?;
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut line = String::new();
            let res = BufReader::new(output).read_line(&mut line).map(|_| line);
            let _ = tx.send(res);
        });
        let reply = rx
            .recv_timeout(Duration::from_secs(35))
            .context("Obscura 出口或页面校验超时")??;
        let report: Value = serde_json::from_str(&reply).context("Obscura 窗口提前退出")?;
        if let Some(error) = report["error"].as_str() {
            bail!("Obscura 启动失败：{error}")
        }
        let body = report["body"].as_str().context("缺少 Obscura 出口校验")?;
        ensure!(runtime::extract(body)?.0 == ip, "Obscura 出口与节点不一致");
        writeln!(input, "show")?;
        Ok(viewer)
    }
}
impl Drop for Interactive {
    fn drop(&mut self) {
        drop(self.child.stdin.take());
        for _ in 0..35 {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            thread::sleep(Duration::from_millis(40));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
