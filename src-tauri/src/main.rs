mod browser;
mod manager;
mod proxy;
mod runtime;
mod store;
mod supervisor;

use fs2::FileExt;
use manager::Manager as Harbor;
use serde_json::Value;
use std::{
    fs::{self, File},
    sync::{mpsc, Arc, Mutex},
    time::Duration,
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder,
};
type Shared = Arc<Mutex<Harbor>>;
struct Downloads(Mutex<()>);
struct Instance {
    _file: File,
}

#[tauri::command]
async fn operate(
    app: AppHandle,
    window: tauri::WebviewWindow,
    op: String,
    payload: Value,
) -> Result<Value, String> {
    if window.label() != "main" {
        return Err("窗口无权限".into());
    }
    let state = app.state::<Shared>().inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        // Keep supervision and stopping other sessions responsive during large downloads.
        let result = (|| -> anyhow::Result<Value> {
            if matches!(op.as_str(), "start" | "switch" | "geo" | "chrome") {
                let (root, auto, missing_chrome) = {
                    let m = state.lock().map_err(|_| anyhow::anyhow!("管理器不可用"))?;
                    let auto = if matches!(op.as_str(), "start" | "switch") {
                        let p = m.store.profile(manager::field(&payload, "id")?)?;
                        p.auto || payload["decision"] == "update"
                    } else {
                        false
                    };
                    (
                        m.store.root.clone(),
                        auto,
                        !runtime::chrome(&m.store).is_file(),
                    )
                };
                let downloads = app.state::<Downloads>();
                let _serial = downloads
                    .0
                    .lock()
                    .map_err(|_| anyhow::anyhow!("运行时安装器不可用"))?;
                if op == "geo" || (auto && !root.join("runtime/city.mmdb").is_file()) {
                    let _ = app.emit("harbor-phase", "正在下载并校验离线城市库");
                    runtime::install_geo(&root)?;
                }
                if op == "chrome" || (op != "geo" && missing_chrome) {
                    let _ = app.emit("harbor-phase", "正在安装并验证 Google Chrome");
                    runtime::install_chrome(&root)?;
                }
                if op == "chrome" {
                    state
                        .lock()
                        .map_err(|_| anyhow::anyhow!("管理器不可用"))?
                        .store
                        .setting_set("browser", "managed")?;
                }
                if matches!(op.as_str(), "geo" | "chrome") {
                    return Ok(serde_json::json!({"ok":true}));
                }
            }
            state
                .lock()
                .map_err(|_| anyhow::anyhow!("管理器不可用"))?
                .act(&op, payload)
        })()
        .map_err(|e| e.to_string());
        if !matches!(op.as_str(), "snapshot" | "metrics") {
            let _ = app.emit("harbor-updated", ());
        }
        result
    })
    .await
    .map_err(|e| e.to_string())?
}
fn show(app: &AppHandle) -> tauri::Result<()> {
    if let Some(w) = app.get_webview_window("main") {
        w.unminimize()?;
        w.show()?;
        w.set_focus()?;
        return Ok(());
    }
    WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title("Profile Harbor")
        .inner_size(1040., 710.)
        .min_inner_size(770., 530.)
        .on_navigation(|u| {
            u.scheme() == "tauri" && u.host_str() == Some("localhost")
                || u.as_str() == "about:blank"
        })
        .build()?;
    Ok(())
}
fn main() {
    unsafe {
        libc::umask(0o077);
    }
    let app = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![operate])
        .setup(|app| {
            let root = app.path().app_data_dir()?;
            fs::create_dir_all(&root)?;
            let lock = File::options()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(root.join("manager.lock"))?;
            lock.try_lock_exclusive()
                .map_err(|_| std::io::Error::other("应用已运行，请从菜单栏打开"))?;
            app.manage(Instance { _file: lock });
            let (tx, rx) = mpsc::channel();
            let state = Arc::new(Mutex::new(Harbor::new(
                store::Store::open(root)?,
                tx,
                runtime::engine()?,
            )?));
            let phase_handle = app.handle().clone();
            state.lock().unwrap().phase = Box::new(move |_, message| {
                let _ = phase_handle.emit("harbor-phase", message);
            });
            app.manage(Downloads(Mutex::new(())));
            app.manage(state.clone());
            let handle = app.handle().clone();
            let weak = Arc::downgrade(&state);
            std::thread::spawn(move || loop {
                let _ = rx.recv_timeout(Duration::from_secs(10));
                let Some(shared) = weak.upgrade() else { break };
                if let Ok(mut m) = shared.lock() {
                    if m.tick().unwrap_or(false) {
                        let _ = handle.emit("harbor-updated", ());
                    }
                };
            });
            let open = MenuItem::with_id(app, "open", "打开 Profile Harbor", true, None::<&str>)?;
            let quit =
                MenuItem::with_id(app, "quit", "停止所有 Profile 并退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            TrayIconBuilder::new()
                .icon(app.default_window_icon().expect("app icon").clone())
                .tooltip("Profile Harbor")
                .menu(&menu)
                .on_menu_event(|app, e| match e.id.as_ref() {
                    "open" => {
                        let _ = show(app);
                    }
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .build(app)?;
            show(app.handle())?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("无法启动 Profile Harbor");
    app.run(|handle, event| match event {
        RunEvent::ExitRequested {
            code: None, api, ..
        } => {
            if handle.webview_windows().is_empty() {
                api.prevent_exit();
            }
        }
        RunEvent::Reopen { .. } => {
            let _ = show(handle);
        }
        RunEvent::Exit => {
            if let Some(s) = handle.try_state::<Shared>() {
                if let Ok(mut manager) = s.lock() {
                    manager.shutdown();
                }
            }
        }
        _ => {}
    });
}
