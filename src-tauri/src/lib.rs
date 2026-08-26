//! WhaleNest — Tauri v2 shell hosting the DeepSeek Harness (dsh) kernel.
//!
//! Wires the kernel / readiness / state / lifecycle modules into the app:
//! spawns dsh on startup, supervises it (bounded auto-restart), probes
//! readiness, and exposes the IPC contract to the shell frontend.

mod kernel;
mod lifecycle;
mod readiness;
mod state;
mod updater;

use parking_lot::Mutex;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, OnceLock,
};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_clipboard_manager::ClipboardExt as _;
use tauri_plugin_dialog::DialogExt as _;

use kernel::{Kernel, KernelConfig, KernelState, KernelStatus};
use readiness::Readiness;
use state::AppState;
use updater::UpdateInfo;

/// Managed shared state handed to commands, tray handlers, and background tasks.
pub(crate) struct Managed {
    pub config: Arc<Mutex<AppState>>,
    pub kernel: Arc<Mutex<Kernel>>,
    /// Set before final app exit so supervisors do not relaunch dsh.
    pub shutting_down: Arc<AtomicBool>,
    /// Last update-check result; `None` until the first check completes.
    pub update: Arc<Mutex<Option<UpdateInfo>>>,
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

/// Send a desktop notification (best-effort; silently ignored on failure).
fn notify(app: &AppHandle, title: &str, body: &str) {
    use tauri_plugin_notification::NotificationExt as _;
    let _ = app.notification().builder().title(title).body(body).show();
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
    if window
        .url()
        .map(|u| u.as_str() == home.as_str())
        .unwrap_or(false)
    {
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
    let mut kernel = managed.kernel.lock();
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
///
/// Runs on the caller's thread only long enough to flip the UI into
/// "starting" and hand the kill+spawn work to a background thread: the tray
/// menu event callback lives on the app main thread, and `kill()` blocks on
/// `taskkill` + `wait()` for the whole dsh process tree, which would freeze
/// the UI (no animation, no response) for seconds.
pub(crate) fn restart_kernel_impl(app: &AppHandle) {
    let managed = app.state::<Managed>();
    // Re-resolve dsh on every restart attempt: the guide view's 重新检测 /
    // 一键安装 flows depend on picking up a newly installed dsh without an
    // app restart (Kernel::new resolved only once).
    {
        let mut kernel = managed.kernel.lock();
        if !kernel.redetect() {
            drop(kernel);
            emit_status(app, KernelStatus::Guide);
            return;
        }
    }
    // 1. Immediately tell the shell to show the loading view (and navigate
    //    the webview back to the shell page) while the restart is in flight.
    emit_and_reset_view(app, KernelStatus::Starting);

    // 2. Kill + respawn on a worker thread so the main thread (tray event
    //    dispatch) stays responsive; the watcher/readiness loops keep
    //    supervising from their own threads.
    let app = app.clone();
    std::thread::spawn(move || {
        let managed = app.state::<Managed>();
        let mut kernel = managed.kernel.lock();
        let _ = kernel.kill();
        match kernel.spawn() {
            Ok(()) => {
                // State is already Starting; readiness probing will emit Ready.
            }
            Err(e) => {
                let msg = format!("重启 dsh 内核失败: {e}");
                kernel.state = KernelState::Crashed {
                    restarts: 0,
                    last_error: msg.clone(),
                };
                drop(kernel);
                emit_and_reset_view(&app, KernelStatus::Error { message: msg });
            }
        }
    });
}

/// Switch the working directory: persist cwd + recent list, update the kernel
/// config, then restart the kernel. Shared by the folder picker and the tray's
/// recent-directory items.
pub(crate) fn switch_cwd(app: &AppHandle, new_cwd: std::path::PathBuf) {
    if new_cwd.as_os_str().is_empty() {
        return;
    }
    let managed = app.state::<Managed>();
    {
        let mut cfg = managed.config.lock();
        cfg.cwd = new_cwd.clone();
        cfg.push_recent_dir(new_cwd.clone());
        let _ = cfg.save();
    }
    {
        let mut k = managed.kernel.lock();
        k.config.cwd = new_cwd;
    }
    restart_kernel_impl(app);
}

/// Pop the native folder picker; on selection persist cwd and restart the kernel.
pub(crate) fn prompt_switch_dir(app: &AppHandle) -> Result<(), String> {
    let managed = app.state::<Managed>();
    let current = managed.config.lock().cwd.clone();
    let app2 = app.clone();
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
            switch_cwd(&app2, new_cwd);
        });
    Ok(())
}

// ── IPC commands (contract with the shell frontend) ─────────────────────────

/// Tool detection info
#[derive(Clone, Serialize)]
pub(crate) struct ToolInfo {
    pub name: String,
    pub found: bool,
    pub version: Option<String>,
    pub path: Option<String>,
}

/// Environment check results for node, npm, pnpm, dsh
#[derive(Clone, Serialize)]
pub(crate) struct EnvCheckResult {
    pub node: ToolInfo,
    pub npm: ToolInfo,
    pub pnpm: ToolInfo,
    pub dsh: ToolInfo,
    pub all_passed: bool,
}

/// Recommended plugin item from melon repo
#[derive(Clone, Serialize)]
pub(crate) struct RepoPluginItem {
    pub id: String,
    pub name: String,
    pub package_name: String,
    pub description: String,
    pub installed: bool,
    pub version: Option<String>,
    pub category: String,
}

/// get_state() -> { status, url?, message?, cwd, autostart, lock_port, initialized }
#[derive(Serialize)]
pub(crate) struct StateResponse {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub cwd: String,
    pub autostart: bool,
    pub lock_port: bool,
    pub initialized: bool,
}

fn resolve_binary_path_helper(name: &str) -> Option<String> {
    if let Some(path_var) = std::env::var_os("PATH") {
        #[cfg(windows)]
        {
            let exts = [".cmd", ".exe", ".bat", ".ps1", ""];
            for dir in std::env::split_paths(&path_var) {
                for ext in exts {
                    let full = format!("{}{}", name, ext);
                    let p = dir.join(&full);
                    if p.is_file() {
                        return Some(p.to_string_lossy().to_string());
                    }
                }
            }
        }
        #[cfg(not(windows))]
        {
            let mut paths: Vec<std::path::PathBuf> = std::env::split_paths(&path_var).collect();
            if let Some(home) = std::env::var_os("HOME") {
                let home_path = std::path::PathBuf::from(home);
                paths.push(home_path.join(".local/bin"));
                paths.push(home_path.join(".volta/bin"));
            }
            for dir in paths {
                let p = dir.join(name);
                if p.is_file() {
                    return Some(p.to_string_lossy().to_string());
                }
            }
        }
    }
    None
}

fn probe_command(bin: &str, args: &[&str]) -> (bool, Option<String>, Option<String>) {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let path = resolve_binary_path_helper(bin);
    let mut cmd = if let Some(ref p) = path {
        Command::new(p)
    } else {
        Command::new(bin)
    };
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return (false, None, path),
    };

    let mut child = child;
    let deadline = Instant::now() + Duration::from_millis(3000);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    if let Some(mut out) = child.stdout.take() {
                        let mut buf = String::new();
                        let _ = std::io::Read::read_to_string(&mut out, &mut buf);
                        let trimmed = buf.trim().to_string();
                        let first_line = trimmed.lines().next().unwrap_or("").trim().to_string();
                        return (true, Some(first_line), path);
                    }
                    return (true, None, path);
                } else {
                    return (true, None, path);
                }
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    return (true, None, path);
                }
                std::thread::sleep(Duration::from_millis(30));
            }
            Err(_) => return (false, None, path),
        }
    }
}

fn read_installed_dsh_plugins() -> Vec<String> {
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE").map(std::path::PathBuf::from);
    #[cfg(not(windows))]
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);

    let Some(home_path) = home else {
        return Vec::new();
    };
    let pkg_path = home_path
        .join(".dsh")
        .join("profiles")
        .join("web")
        .join("package.json");

    if let Ok(text) = std::fs::read_to_string(&pkg_path) {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
            let mut installed = Vec::new();
            if let Some(deps) = json.get("dependencies").and_then(|d| d.as_object()) {
                for key in deps.keys() {
                    installed.push(key.clone());
                }
            }
            if let Some(bundles) = json
                .pointer("/dsh/profile/bundles")
                .and_then(|b| b.as_array())
            {
                for b in bundles {
                    if let Some(s) = b.as_str() {
                        if !installed.contains(&s.to_string()) {
                            installed.push(s.to_string());
                        }
                    }
                }
            }
            return installed;
        }
    }
    Vec::new()
}

fn get_repo_plugins_list() -> Vec<RepoPluginItem> {
    let installed = read_installed_dsh_plugins();

    let defs = [
        (
            "dsh-skin-material-you",
            "Material You 主题皮肤",
            "dsh-skin-material-you",
            "Material You (Material 3) 浅深色质感皮肤，优雅动效与精致色彩。",
            "外观主题",
        ),
        (
            "dsh-plugin-dashboard",
            "插件版本仪表盘",
            "dsh-plugin-dashboard",
            "在设置面板中集中展示已安装插件、检测最新版本并提供一键升级与管理。",
            "扩展增强",
        ),
        (
            "dsh-browser-tool",
            "浏览器驱动工具",
            "dsh-browser-tool",
            "支持无头 Chromium、CDP 连接与真实 Chrome Relay 控制，网页分析与截图。",
            "核心工具",
        ),
        (
            "dsh-ast-edit-tool",
            "AST 结构化代码编辑",
            "dsh-ast-edit-tool",
            "基于 ast-grep 的代码结构重构与预览工具，支持高精度代码变换。",
            "核心工具",
        ),
    ];

    defs.iter()
        .map(|(id, name, pkg, desc, cat)| {
            let is_inst = installed.iter().any(|item| item == *pkg);
            RepoPluginItem {
                id: id.to_string(),
                name: name.to_string(),
                package_name: pkg.to_string(),
                description: desc.to_string(),
                installed: is_inst,
                version: None,
                category: cat.to_string(),
            }
        })
        .collect()
}

#[tauri::command]
fn check_env() -> EnvCheckResult {
    let (node_found, node_ver, node_path) = probe_command("node", &["-v"]);
    let (npm_found, npm_ver, npm_path) = probe_command("npm", &["-v"]);
    let (pnpm_found, pnpm_ver, pnpm_path) = probe_command("pnpm", &["-v"]);
    let (dsh_found, dsh_ver, dsh_path) = probe_command("dsh", &["--version"]);

    let all_passed = node_found && npm_found && dsh_found;

    EnvCheckResult {
        node: ToolInfo {
            name: "Node.js".into(),
            found: node_found,
            version: node_ver,
            path: node_path,
        },
        npm: ToolInfo {
            name: "npm".into(),
            found: npm_found,
            version: npm_ver,
            path: npm_path,
        },
        pnpm: ToolInfo {
            name: "pnpm".into(),
            found: pnpm_found,
            version: pnpm_ver,
            path: pnpm_path,
        },
        dsh: ToolInfo {
            name: "DeepSeek Harness (dsh)".into(),
            found: dsh_found,
            version: dsh_ver,
            path: dsh_path,
        },
        all_passed,
    }
}

#[tauri::command]
fn get_recommended_plugins() -> Vec<RepoPluginItem> {
    get_repo_plugins_list()
}

#[tauri::command]
fn install_plugin(app: AppHandle, package_name: String) -> Result<(), String> {
    let app2 = app.clone();
    std::thread::spawn(move || {
        use std::process::Command;
        use std::process::Stdio;

        // Try dsh plugin --profile web add <pkg>
        let status = Command::new("dsh")
            .args(["plugin", "--profile", "web", "add", &package_name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();

        let ok = match status {
            Ok(s) => s.success(),
            Err(_) => false,
        };

        let _ = app2.emit(
            "plugin-installed",
            serde_json::json!({
                "package_name": package_name,
                "success": ok
            }),
        );
    });
    Ok(())
}

#[tauri::command]
fn complete_setup(app: AppHandle, state: tauri::State<'_, Managed>) -> Result<(), String> {
    {
        let mut cfg = state.config.lock();
        cfg.initialized = true;
        let _ = cfg.save();
    }
    // Start kernel now if dsh is available
    start_kernel(&app);
    Ok(())
}

#[tauri::command]
fn get_state(state: tauri::State<'_, Managed>) -> StateResponse {
    let cfg = state.config.lock();
    let kernel = state.kernel.lock();
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
        lock_port: cfg.lock_port,
        initialized: cfg.initialized,
    }
}

#[tauri::command]
fn set_working_dir(app: AppHandle) -> Result<(), String> {
    prompt_switch_dir(&app)
}

/// Toggle fixed-port mode: persist + apply to the kernel config, then restart
/// the kernel so the new setting takes effect.
#[tauri::command]
fn set_lock_port(app: AppHandle, enabled: bool) -> Result<(), String> {
    let managed = app.state::<Managed>();
    {
        let mut cfg = managed.config.lock();
        cfg.lock_port = enabled;
        let _ = cfg.save();
    }
    {
        let mut k = managed.kernel.lock();
        k.config.lock_port = enabled;
    }
    restart_kernel_impl(&app);
    Ok(())
}

#[tauri::command]
fn restart_kernel(app: AppHandle) -> Result<(), String> {
    restart_kernel_impl(&app);
    Ok(())
}

#[tauri::command]
fn get_diagnostics(state: tauri::State<'_, Managed>) -> String {
    state.kernel.lock().diagnostics()
}

#[tauri::command]
fn open_log_file(_app: AppHandle, state: tauri::State<'_, Managed>) -> Result<(), String> {
    let path = state.kernel.lock().log_path().to_path_buf();
    tauri_plugin_opener::open_path(path, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
fn copy_diagnostics(app: AppHandle, state: tauri::State<'_, Managed>) -> Result<(), String> {
    let diag = state.kernel.lock().diagnostics();
    app.clipboard().write_text(diag).map_err(|e| e.to_string())
}

#[tauri::command]
fn quit(app: AppHandle) -> Result<(), String> {
    lifecycle::exit_app(&app);
    Ok(())
}

// ── update checking ──────────────────────────────────────────────────────────

/// Event payload for the "dsh-update" channel.
#[derive(Clone, Serialize)]
pub(crate) struct UpdateEventPayload {
    pub current: String,
    pub latest: String,
    pub has_update: bool,
}

impl From<UpdateInfo> for UpdateEventPayload {
    fn from(info: UpdateInfo) -> Self {
        Self {
            current: info.current,
            latest: info.latest,
            has_update: info.has_update,
        }
    }
}

/// Broadcast the stored update-check result (or a fresh one) to the frontend.
pub(crate) fn emit_update(app: &AppHandle, info: &UpdateInfo) {
    let _ = app.emit("dsh-update", UpdateEventPayload::from(info.clone()));
}

/// Run one update check off the main thread and store + broadcast the result.
/// Callers may pass the last known result to avoid re-checking; `None` forces
/// a fresh registry query.
pub(crate) fn check_update_async(app: AppHandle) {
    std::thread::spawn(move || {
        let result = updater::check_for_update();
        let app2 = app.clone();
        let managed = app.state::<Managed>();
        match result {
            Some(info) => {
                *managed.update.lock() = Some(info.clone());
                emit_update(&app2, &info);
            }
            None => {
                // Offline / dsh missing: stay silent — never nag.
                *managed.update.lock() = None;
            }
        }
        // Refresh the tray item regardless of outcome.
        lifecycle::refresh_update_item(&app2);
    });
}

/// Run `npm i -g @deepseek-ai/dsh` in the background; on success store the new
/// version (read from `dsh --version` again) and broadcast.
pub(crate) fn install_update_async(app: AppHandle) {
    std::thread::spawn(move || {
        let _ = install_update_inner();
        let app2 = app.clone();
        // Re-check so the UI reflects the new state (update or no-op).
        check_update_async(app2);
    });
}

fn install_update_inner() -> Result<(), String> {
    use std::process::Command;
    use std::process::Stdio;
    let status = Command::new("npm")
        .args(["install", "-g", "@deepseek-ai/dsh"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("执行 npm 安装失败: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("npm 安装失败，退出码 {:?}", status.code()))
    }
}

#[tauri::command]
fn check_update(app: AppHandle) {
    check_update_async(app);
}

#[tauri::command]
fn install_update(app: AppHandle) -> Result<(), String> {
    install_update_async(app);
    Ok(())
}

/// One-click dsh install from the guide view (dsh missing). Runs
/// `npm i -g @deepseek-ai/dsh` in the background; on success emits
/// "dsh-installed" so the shell re-detects and boots the kernel.
#[tauri::command]
fn install_dsh(app: AppHandle) -> Result<(), String> {
    std::thread::spawn(move || {
        let ok = install_update_inner().is_ok();
        let app2 = app.clone();
        if ok {
            let _ = app2.emit("dsh-installed", ());
            // Give the kernel a moment to pick up the new dsh on PATH, then
            // re-detect by restarting the kernel.
            std::thread::sleep(Duration::from_millis(800));
            restart_kernel_impl(&app2);
        }
    });
    Ok(())
}

// ── background supervision ──────────────────────────────────────────────────

/// Watch the child process; on spontaneous exit do crash accounting and
/// bounded auto-restart. Final application shutdown stops this loop before it
/// can interpret the intentional tree kill as a crash.
fn spawn_kernel_watcher(
    app: AppHandle,
    kernel: Arc<Mutex<Kernel>>,
    shutting_down: Arc<AtomicBool>,
) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(300));
        if shutting_down.load(Ordering::Acquire) {
            return;
        }
        let mut exited = None;
        {
            let mut k = kernel.lock();
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
                        let app2 = app.clone();
                        let _ = std::thread::spawn(move || {
                            notify(&app2, "dsh 内核异常退出", "正在自动重启，请稍候…");
                        });
                        drop(k);
                        emit_and_reset_view(&app, KernelStatus::Starting);
                    }
                    KernelState::Crashed { last_error, .. } => {
                        let app2 = app.clone();
                        let msg = last_error.clone();
                        let _ = std::thread::spawn(move || {
                            notify(&app2, "dsh 内核已停止", &msg);
                        });
                        drop(k);
                        emit_and_reset_view(
                            &app,
                            KernelStatus::Error {
                                message: last_error.clone(),
                            },
                        );
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
                let k = kernel.lock();
                (k.current_port(), k.remaining_ready_timeout())
            };
            let Some(port) = port else { continue };
            let Some(timeout) = timeout else { continue };

            let base_url = format!("http://127.0.0.1:{port}");
            let readiness = Readiness::new(base_url);
            match readiness.wait_until_ready(timeout).await {
                Ok(url) => {
                    let mut k = kernel.lock();
                    if k.ready_emitted || k.current_port != Some(port) {
                        continue;
                    }
                    k.state = KernelState::Ready {
                        url: url.clone(),
                        port,
                    };
                    // Only notify on restarts (crash recovery / manual restart),
                    // not on the very first launch.
                    let notify_ready = k.last_exit_status.is_some();
                    k.ready_emitted = true;
                    drop(k);
                    if notify_ready {
                        let app2 = app.clone();
                        let url2 = url.clone();
                        let _ = std::thread::spawn(move || {
                            notify(&app2, "dsh 内核已就绪", &format!("访问地址：{url2}"));
                        });
                    }
                    emit_status(&app, KernelStatus::Ready { url });
                }
                Err(_) => {
                    let mut k = kernel.lock();
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
    // ── GPU acceleration & WebView rendering optimizations ──────────────────
    #[cfg(target_os = "windows")]
    {
        // Enable Chromium GPU rasterization, zero-copy tile transfer, and bypass conservative GPU blocklists
        if std::env::var_os("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS").is_none() {
            std::env::set_var(
                "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
                "--enable-gpu-rasterization --enable-zero-copy --ignore-gpu-blocklist --enable-features=VaapiVideoDecoder,CanvasOopif,RawDraw",
            );
        }
    }
    #[cfg(target_os = "linux")]
    {
        // Force hardware compositing mode on Linux WebKitGTK to avoid software rasterizer fallback
        if std::env::var_os("WEBKIT_FORCE_COMPOSITING_MODE").is_none() {
            std::env::set_var("WEBKIT_FORCE_COMPOSITING_MODE", "1");
        }
    }

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
            // Pass --silent when launched via autostart so the window can be
            // hidden (the app keeps running in the tray).
            Some(vec!["--silent".into()]),
        ))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            get_state,
            set_working_dir,
            set_lock_port,
            restart_kernel,
            get_diagnostics,
            open_log_file,
            copy_diagnostics,
            check_update,
            install_update,
            install_dsh,
            check_env,
            get_recommended_plugins,
            install_plugin,
            complete_setup,
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
                lock_port: app_state.lock_port,
            };
            let kernel = Kernel::new(kernel_config, config_dir.join("dsh.log"));
            let kernel_arc = Arc::new(Mutex::new(kernel));

            // 3. managed state
            app.manage(Managed {
                config: config_arc.clone(),
                kernel: kernel_arc.clone(),
                shutting_down: Arc::new(AtomicBool::new(false)),
                update: Arc::new(Mutex::new(None)),
            });

            // 4. autostart sync (config is the source of truth)
            if app_state.autostart {
                let _ = app.autolaunch().enable();
            }

            // 5. tray
            lifecycle::build_tray_menu(app.handle())?;

            // 5b. global hotkey (Ctrl+Alt+W summons the window). Registered at
            //     runtime so a conflict with another app's shortcut only
            //     disables the hotkey — never crashes startup.
            {
                use tauri_plugin_global_shortcut::GlobalShortcutExt as _;
                if let Err(e) = app.global_shortcut().on_shortcut(
                    "Ctrl+Alt+W",
                    move |app, _shortcut, _event| {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.unminimize();
                            let _ = window.set_focus();
                        }
                    },
                ) {
                    eprintln!(
                        "[whalenest] 全局快捷键 Ctrl+Alt+W 注册失败（可能已被其他程序占用）: {e}"
                    );
                }
            }

            // 6. silent autostart: hide the window when launched via the
            //    autostart entry (args carry --silent), keeping the kernel
            //    and tray running.
            let silent = std::env::args().any(|a| a == "--silent");
            if silent {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.hide();
                }
            }

            // 7. remember the shell page URL so we can navigate back on crash
            if let Some(window) = app.get_webview_window("main") {
                if let Ok(url) = window.url() {
                    let _ = HOME_URL.set(url.to_string());
                }
            }

            // 8. start the kernel (or emit guide when dsh is missing, unless in step init wizard)
            if app_state.initialized {
                start_kernel(app.handle());
            }

            // 9. background supervision
            spawn_kernel_watcher(
                app.handle().clone(),
                kernel_arc.clone(),
                app.state::<Managed>().shutting_down.clone(),
            );
            spawn_readiness(app.handle().clone(), kernel_arc.clone());

            // 10. delayed update check (off the critical startup path, silent on failure)
            {
                let app = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_secs(5));
                    check_update_async(app);
                });
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running whalenest");
}
