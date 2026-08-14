//! WhaleNest — Tauri v2 shell hosting the DeepSeek Harness (dsh) kernel.
//!
//! Wires the kernel / readiness / state / lifecycle modules into the app:
//! spawns dsh on startup, supervises it (bounded auto-restart), probes
//! readiness, and exposes the IPC contract to the shell frontend.

mod kernel;
mod lifecycle;
mod readiness;
mod state;

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_clipboard_manager::ClipboardExt as _;
use tauri_plugin_dialog::DialogExt as _;

use kernel::{Kernel, KernelConfig, KernelState, KernelStatus};
use readiness::Readiness;
use state::AppState;

/// Managed shared state handed to commands, tray handlers, and background tasks.
pub(crate) struct Managed {
    pub config: Arc<Mutex<AppState>>,
    pub kernel: Arc<Mutex<Kernel>>,
}

/// Event payload for the "kernel-status" channel.
#[derive(Clone, Serialize)]
pub(crate) struct KernelEventPayload {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl KernelStatus {
    pub(crate) fn payload(&self) -> KernelEventPayload {
        match self {
            KernelStatus::Guide => KernelEventPayload {
                status: "guide".into(),
                url: None,
                message: None,
            },
            KernelStatus::Starting => KernelEventPayload {
                status: "starting".into(),
                url: None,
                message: None,
            },
            KernelStatus::Ready { url } => KernelEventPayload {
                status: "ready".into(),
                url: Some(url.clone()),
                message: None,
            },
            KernelStatus::Error { message } => KernelEventPayload {
                status: "error".into(),
                url: None,
                message: Some(message.clone()),
            },
        }
    }
}

/// Broadcast a kernel status change to the frontend.
pub(crate) fn emit_status(app: &AppHandle, status: KernelStatus) {
    let _ = app.emit("kernel-status", status.payload());
}

/// The shell page URL (the window's initial address), recorded at setup so the
/// webview can be navigated back home whenever the kernel stops serving.
static HOME_URL: OnceLock<String> = OnceLock::new();

/// Navigate the main webview back to the shell page (no-op when already there).
fn navigate_home(app: &AppHandle) {
    let Some(home) = HOME_URL.get() else {
        return;
    };
    let Ok(url) = home.parse::<tauri::Url>() else {
        return;
    };
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if window.url().map(|u| u.as_str() == home.as_str()).unwrap_or(false) {
        return; // already on the shell page
    }
    let _ = window.navigate(url);
}

/// Emit a status; whenever the kernel is no longer serving (Starting after a
/// crash/restart, or Error), pull the webview back to the shell page so the
/// frontend keeps listening. Callers must set the kernel state BEFORE calling.
fn emit_and_reset_view(app: &AppHandle, status: KernelStatus) {
    let go_home = matches!(status, KernelStatus::Starting | KernelStatus::Error { .. });
    emit_status(app, status);
    if go_home {
        navigate_home(app);
    }
}

/// Initial kernel launch (or guide state when dsh is missing).
pub(crate) fn start_kernel(app: &AppHandle) {
    let managed = app.state::<Managed>();
    let mut kernel = managed.kernel.lock().unwrap();
    if !kernel.dsh_available() {
        drop(kernel);
        emit_status(app, KernelStatus::Guide);
        return;
    }
    match kernel.spawn() {
        Ok(()) => {
            drop(kernel);
            emit_and_reset_view(app, KernelStatus::Starting);
        }
        Err(e) => {
            let msg = format!("启动 dsh 内核失败: {e}");
            kernel.state = KernelState::Crashed {
                restarts: 0,
                last_error: msg.clone(),
            };
            drop(kernel);
            emit_and_reset_view(app, KernelStatus::Error { message: msg });
        }
    }
}

/// Restart the kernel (used by restart command, tray item, and dir switch).
pub(crate) fn restart_kernel_impl(app: &AppHandle) {
    let managed = app.state::<Managed>();
    let mut kernel = managed.kernel.lock().unwrap();
    if !kernel.dsh_available() {
        drop(kernel);
        emit_status(app, KernelStatus::Guide);
        return;
    }
    let _ = kernel.kill();
    match kernel.spawn() {
        Ok(()) => {
            drop(kernel);
            emit_and_reset_view(app, KernelStatus::Starting);
        }
        Err(e) => {
            let msg = format!("重启 dsh 内核失败: {e}");
            kernel.state = KernelState::Crashed {
                restarts: 0,
                last_error: msg.clone(),
            };
            drop(kernel);
            emit_and_reset_view(app, KernelStatus::Error { message: msg });
        }
    }
}

/// Pop the native folder picker; on selection persist cwd and restart the kernel.
pub(crate) fn prompt_switch_dir(app: &AppHandle) -> Result<(), String> {
    let managed = app.state::<Managed>();
    let current = managed.config.lock().unwrap().cwd.clone();
    let app2 = app.clone();
    let config = managed.config.clone();
    let kernel = managed.kernel.clone();
    app.dialog()
        .file()
        .set_directory(current)
        .pick_folder(move |picked| {
            let Some(fp) = picked else {
                return;
            };
            let Ok(new_cwd) = fp.into_path() else {
                return;
            };
            if new_cwd.as_os_str().is_empty() {
                return;
            }
            {
                let mut cfg = config.lock().unwrap();
                cfg.cwd = new_cwd.clone();
                let _ = cfg.save();
            }
            {
                let mut k = kernel.lock().unwrap();
                k.config.cwd = new_cwd;
            }
            restart_kernel_impl(&app2);
        });
    Ok(())
}

// ── IPC commands (contract with the shell frontend) ─────────────────────────

/// get_state() -> { status, url?, message?, cwd, autostart }
#[derive(Serialize)]
pub(crate) struct StateResponse {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub cwd: String,
    pub autostart: bool,
}

#[tauri::command]
fn get_state(state: tauri::State<'_, Managed>) -> StateResponse {
    let cfg = state.config.lock().unwrap();
    let kernel = state.kernel.lock().unwrap();
    let (status, url, message) = if !kernel.dsh_available() {
        ("guide".to_string(), None, None)
    } else {
        match &kernel.state {
            KernelState::Ready { url, .. } => ("ready".to_string(), Some(url.clone()), None),
            KernelState::Crashed { last_error, .. } => {
                ("error".to_string(), None, Some(last_error.clone()))
            }
            _ => ("starting".to_string(), None, None),
        }
    };
    StateResponse {
        status,
        url,
        message,
        cwd: cfg.cwd.to_string_lossy().into_owned(),
        autostart: cfg.autostart,
    }
}

#[tauri::command]
fn set_working_dir(app: AppHandle) -> Result<(), String> {
    prompt_switch_dir(&app)
}

#[tauri::command]
fn restart_kernel(app: AppHandle) -> Result<(), String> {
    restart_kernel_impl(&app);
    Ok(())
}

#[tauri::command]
fn get_diagnostics(state: tauri::State<'_, Managed>) -> String {
    state.kernel.lock().unwrap().diagnostics()
}

#[tauri::command]
fn open_log_file(_app: AppHandle, state: tauri::State<'_, Managed>) -> Result<(), String> {
    let path = state.kernel.lock().unwrap().log_path().to_path_buf();
    tauri_plugin_opener::open_path(path, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
fn copy_diagnostics(app: AppHandle, state: tauri::State<'_, Managed>) -> Result<(), String> {
    let diag = state.kernel.lock().unwrap().diagnostics();
    app.clipboard().write_text(diag).map_err(|e| e.to_string())
}

#[tauri::command]
fn quit(app: AppHandle) -> Result<(), String> {
    lifecycle::exit_app(&app);
    Ok(())
}

// ── background supervision ──────────────────────────────────────────────────

/// Watch the child process; on spontaneous exit do crash accounting and
/// bounded auto-restart (intentional kills never reach this path — kill()
/// detaches the child while holding the lock).
fn spawn_kernel_watcher(app: AppHandle, kernel: Arc<Mutex<Kernel>>) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(300));
        let mut exited = None;
        {
            let mut k = kernel.lock().unwrap();
            if let Some(child) = k.child.as_mut() {
                if let Ok(Some(status)) = child.try_wait() {
                    exited = Some(status);
                }
            }
            if let Some(status) = exited {
                k.child = None;
                k.last_exit_status = Some(status);
                let new_state = k.on_child_exit();
                match &new_state {
                    KernelState::Starting => {
                        drop(k);
                        emit_and_reset_view(&app, KernelStatus::Starting);
                    }
                    KernelState::Crashed { last_error, .. } => {
                        drop(k);
                        emit_and_reset_view(&app, KernelStatus::Error {
                            message: last_error.clone(),
                        });
                    }
                    _ => {}
                }
            }
        }
    });
}

/// Probe the dsh web server until it accepts connections, then emit ready.
fn spawn_readiness(app: AppHandle, kernel: Arc<Mutex<Kernel>>) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(300)).await;
            let (port, timeout) = {
                let k = kernel.lock().unwrap();
                (k.current_port(), k.remaining_ready_timeout())
            };
            let Some(port) = port else { continue };
            let Some(timeout) = timeout else { continue };

            let base_url = format!("http://127.0.0.1:{port}");
            let readiness = Readiness::new(base_url);
            match readiness.wait_until_ready(timeout).await {
                Ok(url) => {
                    let mut k = kernel.lock().unwrap();
                    if k.ready_emitted || k.current_port != Some(port) {
                        continue;
                    }
                    k.state = KernelState::Ready {
                        url: url.clone(),
                        port,
                    };
                    k.ready_emitted = true;
                    drop(k);
                    emit_status(&app, KernelStatus::Ready { url });
                }
                Err(_) => {
                    let mut k = kernel.lock().unwrap();
                    if k.ready_emitted
                        || k.timeout_emitted
                        || k.current_port != Some(port)
                        || k.ready_deadline.is_none()
                        || Instant::now() < k.ready_deadline.unwrap()
                    {
                        continue;
                    }
                    let msg = format!(
                        "dsh 内核 {} 秒内未就绪，请查看日志",
                        kernel::READY_TIMEOUT.as_secs()
                    );
                    k.timeout_emitted = true;
                    k.state = KernelState::Crashed {
                        restarts: 0,
                        last_error: msg.clone(),
                    };
                    drop(k);
                    emit_and_reset_view(&app, KernelStatus::Error { message: msg });
                }
            }
        }
    });
}

// ── entry ───────────────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Single instance: a second launch focuses the existing main window.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![]),
        ))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .invoke_handler(tauri::generate_handler![
            get_state,
            set_working_dir,
            restart_kernel,
            get_diagnostics,
            open_log_file,
            copy_diagnostics,
            quit
        ])
        // Close = hide to tray; dsh keeps running in the background.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                lifecycle::on_window_close_request(window, api);
            }
        })
        .setup(|app| {
            // 1. config dir + persisted state
            let config_dir = state::init_config_dir(app.handle());
            let app_state = AppState::load();
            let config_arc = Arc::new(Mutex::new(app_state.clone()));

            // 2. kernel (log file lives in the config dir)
            let kernel_config = KernelConfig {
                profile: "web".into(),
                port: Some(app_state.preferred_port),
                cwd: app_state.cwd.clone(),
                patches: Vec::new(),
                home: None,
            };
            let kernel = Kernel::new(kernel_config, config_dir.join("dsh.log"));
            let kernel_arc = Arc::new(Mutex::new(kernel));

            // 3. managed state
            app.manage(Managed {
                config: config_arc.clone(),
                kernel: kernel_arc.clone(),
            });

            // 4. autostart sync (config is the source of truth)
            if app_state.autostart {
                let _ = app.autolaunch().enable();
            }

            // 5. tray
            lifecycle::build_tray_menu(app.handle())?;

            // 6. remember the shell page URL so we can navigate back on crash
            if let Some(window) = app.get_webview_window("main") {
                if let Ok(url) = window.url() {
                    let _ = HOME_URL.set(url.to_string());
                }
            }

            // 7. start the kernel (or emit guide when dsh is missing)
            start_kernel(app.handle());

            // 8. background supervision
            spawn_kernel_watcher(app.handle().clone(), kernel_arc.clone());
            spawn_readiness(app.handle().clone(), kernel_arc.clone());

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running whalenest");
}
