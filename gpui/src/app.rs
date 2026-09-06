//! 应用级共享状态与事件总线（GPUI Global），以及从 Tauri 版 lib.rs 移植的
//! 内核启停 / 监督 / 环境检测 / 插件 / 更新逻辑。
//!
//! 架构映射：
//! - Tauri 的 `Event::emit("kernel-status", …)`  → `AppEvent::KernelStatus`
//!   经 flume 通道广播，Shell 视图订阅后驱动 UI。
//! - Tauri 的 IPC command（前端 invoke）         → 直接调本模块的纯函数
//!   （UI 回调与 tray 动作共用）。
//! - Tauri 的 tray 事件 + `on_window_event`      → `Control` 通道，由 main.rs
//!   里注册的应用级监听循环执行（需要 AsyncApp 能力，如重新开窗）。

use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant, SystemTime};

use gpui_kit::Global;
use parking_lot::Mutex;

use crate::kernel::{self, Kernel, KernelConfig, KernelState, KernelStatus};
use crate::readiness::Readiness;
use crate::state::{self, AppState, ProfileInfo, DEFAULT_PROFILE};
use crate::updater::{UpdateInfo, check_for_update};

/// 推荐插件清单的静态定义（与 Tauri 版一致）。
pub const INSTALL_COMMAND: &str = "npm i -g @deepseek-ai/dsh";

/// 广播给 UI 的应用事件。
#[derive(Clone, Debug)]
pub enum AppEvent {
    /// dsh 内核状态变化，携带当前 profile 名。
    KernelStatus {
        profile: String,
        status: KernelStatus,
    },
    Update(UpdateEventPayload),
    /// dsh 一键安装成功（guide / wizard 里的「一键安装」）
    DshInstalled,
}

/// 操作请求：tray / 设置对话框 / 向导共同发往应用级监听循环。
#[derive(Clone, Debug)]
pub enum Control {
    OpenMain,
    RestartKernel,
    OpenBrowser,
    CopyDiagnostics,
    OpenLog,
    OpenConfig,
    ToggleAutostart,
    SetAutostart(bool),
    CheckUpdate,
    InstallUpdate,
    InstallDsh,
    SetLockPort(bool),
    SwitchProfile(String),
    CreateProfile,
    DeleteProfile(String),
    ChangeCwd,
    Stop,
    Quit,
}

/// dsh 版本更新事件负载（对应 Tauri 版 UpdateEventPayload）。
#[derive(Clone, Debug)]
pub struct UpdateEventPayload {
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

/// 环境检测工具信息。
#[derive(Clone, Debug)]
pub struct ToolInfo {
    pub name: String,
    pub found: bool,
    pub version: Option<String>,
    pub path: Option<String>,
}

/// 环境检测结果（node / npm / pnpm / dsh）。
#[derive(Clone, Debug)]
pub struct EnvCheckResult {
    pub node: ToolInfo,
    pub npm: ToolInfo,
    pub pnpm: ToolInfo,
    pub dsh: ToolInfo,
    pub all_passed: bool,
}

/// 应用级共享状态（GPUI Global，窗口关闭后仍存活）。
///
/// 所有字段都是可跨线程共享的句柄（Arc / flume 通道），因此可以廉价 Clone：
/// App 侧持一份作 Global，std 监督线程、tray 线程各持一份克隆。
#[derive(Clone)]
pub struct Managed {
    pub config: Arc<Mutex<AppState>>,
    pub kernel: Arc<Mutex<Kernel>>,
    /// 置位后监督线程不再重启 dsh（退出前设）。
    pub shutting_down: Arc<AtomicBool>,
    /// 最近一次更新检查结果。
    pub update: Arc<Mutex<Option<UpdateInfo>>>,
    /// 事件广播（Shell 视图订阅；可多个接收者）。
    pub events: flume::Sender<AppEvent>,
    pub events_rx: flume::Receiver<AppEvent>,
    /// 操作通道（tray / UI → 应用监听循环）。
    pub controls: flume::Sender<Control>,
    pub controls_rx: flume::Receiver<Control>,
}

impl Global for Managed {}

impl Managed {
    pub fn new() -> (Self, flume::Receiver<Control>) {
        let (events_tx, events_rx) = flume::unbounded();
        let (controls_tx, controls_rx) = flume::unbounded();
        let state = AppState::load();
        let active_profile = state.active_profile.clone();
        let cwd = state.profile_cwd(&active_profile);
        let port = state.preferred_port;
        let lock_port = state.lock_port;
        (
            Self {
                config: Arc::new(Mutex::new(state)),
                kernel: Arc::new(Mutex::new(Kernel::new(
                    KernelConfig {
                        profile: active_profile,
                        port: Some(port),
                        cwd,
                        patches: Vec::new(),
                        home: None,
                        lock_port,
                    },
                    crate::state::init_config_dir(),
                ))),
                shutting_down: Arc::new(AtomicBool::new(false)),
                update: Arc::new(Mutex::new(None)),
                events: events_tx,
                events_rx: events_rx.clone(),
                controls: controls_tx,
                controls_rx: controls_rx.clone(),
            },
            controls_rx,
        )
    }
}

/// Broadcast a kernel status. The profile name is injected from the kernel's
/// current profile so the dashboard can attribute the event to a card.
pub fn emit_status(managed: &Managed, status: KernelStatus) {
    let profile = managed.kernel.lock().config.profile.clone();
    let _ = managed.events.send(AppEvent::KernelStatus { profile, status });
}

fn notify(_managed: &Managed, title: &str, body: &str) {
    crate::notify::notify(title, body);
}

// ── 内核启停（从 lib.rs 移植）──────────────────────────────────────────────

/// 初始内核启动：spawn 当前激活 profile，dsh 缺失则引导。
pub fn start_kernel(managed: &Managed) {
    let mut kernel = managed.kernel.lock();
    if !kernel.dsh_available() {
        drop(kernel);
        emit_status(managed, KernelStatus::Guide);
        return;
    }
    match kernel.spawn() {
        Ok(()) => {
            drop(kernel);
            emit_status(managed, KernelStatus::Starting);
        }
        Err(e) => {
            let msg = format!("启动 dsh 内核失败: {e}");
            kernel.state = KernelState::Crashed {
                restarts: 0,
                last_error: msg.clone(),
            };
            drop(kernel);
            emit_status(managed, KernelStatus::Error { message: msg });
        }
    }
}

/// 重启内核（重启命令 / tray / 切换 profile / 更改目录共用）。
pub fn restart_kernel_impl(managed: &Managed) {
    // 重新解析 dsh（guide「重新检测」/ 一键安装后依赖它）
    {
        let mut kernel = managed.kernel.lock();
        if !kernel.redetect() {
            drop(kernel);
            emit_status(managed, KernelStatus::Guide);
            return;
        }
    }
    // 1. 立即让壳页回到 loading
    emit_status(managed, KernelStatus::Starting);

    // 2. 工作线程 kill + respawn（不阻塞应用主线程）
    let managed = managed.clone();
    std::thread::spawn(move || {
        let mut kernel = managed.kernel.lock();
        let _ = kernel.kill();
        match kernel.spawn() {
            Ok(()) => {}
            Err(e) => {
                let msg = format!("重启 dsh 内核失败: {e}");
                kernel.state = KernelState::Crashed {
                    restarts: 0,
                    last_error: msg.clone(),
                };
                drop(kernel);
                emit_status(&managed, KernelStatus::Error { message: msg });
            }
        }
    });
}

/// 更改指定 profile 的工作目录：写入 profile_cwds；该 profile 正在运行时重启生效。
pub fn change_cwd(managed: &Managed, profile: String, new_cwd: PathBuf) {
    if new_cwd.as_os_str().is_empty() {
        return;
    }
    {
        let mut cfg = managed.config.lock();
        cfg.profile_cwds.insert(profile.clone(), new_cwd.clone());
        let _ = cfg.save();
    }
    {
        let mut k = managed.kernel.lock();
        if k.config.profile == profile {
            k.config.cwd = new_cwd;
        }
    }
    // 仅当该 profile 正在运行（即内核当前 profile）时重启生效
    let is_running = {
        let k = managed.kernel.lock();
        k.config.profile == profile && !matches!(k.state, KernelState::Stopped)
    };
    if is_running {
        restart_kernel_impl(managed);
    }
}

/// 弹出系统目录选择器（rfd），选中后写入指定 profile 并（若正在运行）重启。
pub fn prompt_change_cwd(managed: &Managed, profile: String) {
    let current = managed.config.lock().profile_cwd(&profile);
    let managed = managed.clone();
    std::thread::spawn(move || {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("选择 dsh 工作目录")
            .set_directory(&current)
            .pick_folder()
        {
            change_cwd(&managed, profile, path);
        }
    });
}

// ── 环境检测 / 插件（从 lib.rs 移植）─────────────────────────────────────

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
    use std::io::Read;
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
                        let _ = out.read_to_string(&mut buf);
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

pub fn check_env() -> EnvCheckResult {
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

/// 完成首次初始化向导：置位 initialized 并启动内核。
pub fn complete_setup(managed: &Managed) -> Result<(), String> {
    {
        let mut cfg = managed.config.lock();
        cfg.initialized = true;
        let _ = cfg.save();
    }
    start_kernel(managed);
    Ok(())
}

// ── 更新与安装（从 lib.rs 移植）───────────────────────────────────────────

/// 后台跑一次更新检查并广播结果（离线 / dsh 缺失时静默）。
pub fn check_update_async(managed: &Managed) {
    let managed = managed.clone();
    std::thread::spawn(move || {
        let result = check_for_update();
        match result {
            Some(info) => {
                *managed.update.lock() = Some(info.clone());
                let _ = managed
                    .events
                    .send(AppEvent::Update(UpdateEventPayload::from(info)));
            }
            None => {
                *managed.update.lock() = None;
            }
        }
        // 托盘「发现新版本」项随结果刷新
        crate::tray::refresh_update_item();
    });
}

/// 后台 `npm i -g @deepseek-ai/dsh`；完成后重新检查。
pub fn install_update_async(managed: &Managed) {
    let managed = managed.clone();
    std::thread::spawn(move || {
        let _ = install_update_inner();
        check_update_async(&managed);
    });
}

fn install_update_inner() -> Result<(), String> {
    use std::process::{Command, Stdio};
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

/// 一键安装 dsh（guide / 向导）；成功后广播 DshInstalled 并重启内核。
pub fn install_dsh(managed: &Managed) {
    let managed = managed.clone();
    std::thread::spawn(move || {
        let ok = install_update_inner().is_ok();
        if ok {
            let _ = managed.events.send(AppEvent::DshInstalled);
            // 给 npm 一点时间让 dsh 进入 PATH，再重新检测
            std::thread::sleep(Duration::from_millis(800));
            restart_kernel_impl(&managed);
        }
    });
}

// ── 后台监督（从 lib.rs 移植：watcher + readiness 两个独立线程）────────────

/// 监视子进程：异常退出 → 崩溃计数 + 限次自动重启；应用退出前置位
/// `shutting_down` 阻止把有意终止误判为崩溃。
pub fn spawn_kernel_watcher(managed: &Managed) {
    let managed = managed.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(300));
            if managed.shutting_down.load(Ordering::Acquire) {
                return;
            }
            {
                let mut k = managed.kernel.lock();
                let exited = k
                    .child
                    .as_mut()
                    .and_then(|child| child.try_wait().ok().flatten());
                if let Some(status) = exited {
                    k.child = None;
                    k.last_exit_status = Some(status);
                    let new_state = k.on_child_exit();
                    match &new_state {
                        KernelState::Starting => {
                            notify(&managed, "dsh 内核异常退出", "正在自动重启，请稍候…");
                            drop(k);
                            emit_status(&managed, KernelStatus::Starting);
                        }
                        KernelState::Crashed { last_error, .. } => {
                            let msg = last_error.clone();
                            drop(k);
                            notify(&managed, "dsh 内核已停止", &msg);
                            emit_status(&managed, KernelStatus::Error { message: msg });
                        }
                        _ => {}
                    }
                }
            }
        }
    });
}

/// 就绪探测：轮询 dsh web 端口直到可用，然后广播 Ready（带鉴权 URL）。
pub fn spawn_readiness(managed: &Managed) {
    let managed = managed.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(300));
        let (port, timeout) = {
            let k = managed.kernel.lock();
            (k.current_port(), k.remaining_ready_timeout())
        };
        let Some(port) = port else { continue };
        let Some(timeout) = timeout else { continue };

        let base_url = format!("http://127.0.0.1:{port}");
        let readiness = Readiness::new(base_url.clone());
        match readiness.wait_until_ready(timeout) {
            Ok(_) => {
                // dsh 会为每次进程 mint 一个带 token 的启动 URL（`dsh web:` 行）。
                // 裸 `/` 会 401，WebView 必须用带 token 的地址。
                let auth_url = {
                    let k = managed.kernel.lock();
                    if k.ready_emitted || k.current_port != Some(port) {
                        continue;
                    }
                    match k.auth_url() {
                        Some(url) => Some(url),
                        None if k
                            .remaining_ready_timeout()
                            .is_some_and(|d| !d.is_zero()) =>
                        {
                            continue
                        }
                        None => None,
                    }
                };
                let url = auth_url.unwrap_or(base_url);
                let mut k = managed.kernel.lock();
                if k.ready_emitted || k.current_port != Some(port) {
                    continue;
                }
                k.state = KernelState::Ready {
                    url: url.clone(),
                    port,
                };
                // 仅在重启（崩溃恢复 / 手动重启）后通知，首次启动不打扰
                let notify_ready = k.last_exit_status.is_some();
                k.ready_emitted = true;
                drop(k);
                if notify_ready {
                    notify(&managed, "dsh 内核已就绪", &format!("访问地址：{url}"));
                }
                emit_status(&managed, KernelStatus::Ready { url });
            }
            Err(_) => {
                let mut k = managed.kernel.lock();
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
                emit_status(&managed, KernelStatus::Error { message: msg });
            }
        }
    });
}

// ── 诊断 / 剪贴板 / 外链（从 lib.rs 移植）─────────────────────────────────

pub fn copy_diagnostics(managed: &Managed) -> Result<(), String> {
    let diag = managed.kernel.lock().diagnostics();
    let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    clipboard
        .set_text(diag)
        .map_err(|e| format!("写入剪贴板失败: {e}"))
}

pub fn open_log_file(managed: &Managed) -> Result<(), String> {
    let path = managed.kernel.lock().log_path().to_path_buf();
    crate::lifecycle::open_path(&path)
}

pub fn open_config_file() -> Result<(), String> {
    use crate::state::AppState;
    let path = crate::state::config_path();
    if !path.exists() {
        let _ = AppState::default().save();
    }
    crate::lifecycle::open_path(&path)
}

pub fn open_external_url(url: String) -> Result<(), String> {
    let trimmed = url.trim();
    if trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("mailto:")
    {
        crate::lifecycle::open_url(trimmed)
    } else {
        Err("仅支持打开 http / https / mailto 协议的外部链接".into())
    }
}

// ── profile 管理（从 Tauri 版 lib.rs 扩展：profile 中心化）──────────────

/// 扫描 `$DSH_HOME/profiles/*` 下的所有 profile，填充运行时摘要。
pub fn scan_profiles(managed: &Managed) -> Vec<ProfileInfo> {
    let config = managed.config.lock();
    let home = state::dsh_home();
    let profiles_dir = home.join("profiles");
    let sessions_dir = home.join("sessions");
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&profiles_dir) {
        for entry in rd.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == "node_modules" || name.starts_with('.') {
                continue;
            }
            let (bundles, is_web_type) = read_profile_bundles(&path.join("package.json"));
            let plugin_count = bundles
                .iter()
                .filter(|b| !b.starts_with("@deepseek-ai/"))
                .count();
            let cwd = config.profile_cwd(&name);
            let (session_count, last_session_time) = session_stats(&sessions_dir, &cwd);
            out.push(ProfileInfo {
                name: name.clone(),
                path,
                is_web_type,
                plugin_count,
                cwd,
                session_count,
                last_session_time,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// 读取一个 profile 的 `dsh.profile.bundles`；返回（bundles，是否 web 型）。
fn read_profile_bundles(pkg_path: &Path) -> (Vec<String>, bool) {
    let Ok(text) = std::fs::read_to_string(pkg_path) else {
        return (Vec::new(), false);
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return (Vec::new(), false);
    };
    let mut bundles = Vec::new();
    if let Some(arr) = json.pointer("/dsh/profile/bundles").and_then(|b| b.as_array()) {
        for b in arr {
            if let Some(s) = b.as_str() {
                bundles.push(s.to_string());
            }
        }
    }
    let is_web_type = bundles
        .iter()
        .any(|b| b == "dsh-web-app" || b.ends_with("/dsh-web-app"));
    (bundles, is_web_type)
}

/// 该 cwd 对应会话目录的（子目录数，目录 mtime）。
fn session_stats(sessions_dir: &Path, cwd: &Path) -> (usize, Option<SystemTime>) {
    let dir = sessions_dir.join(state::encode_session_dir_name(cwd));
    let Ok(meta) = std::fs::metadata(&dir) else {
        return (0, None);
    };
    let last_session_time = meta.modified().ok();
    let mut count = 0;
    if let Ok(rd) = std::fs::read_dir(&dir) {
        count = rd.flatten().filter(|e| e.path().is_dir()).count();
    }
    (count, last_session_time)
}

/// 切换激活 profile：持久化 active_profile + 更新内核 profile/cwd 并重启。
pub fn switch_profile(managed: &Managed, name: String) {
    let cwd = {
        let mut cfg = managed.config.lock();
        cfg.active_profile = name.clone();
        let _ = cfg.save();
        cfg.profile_cwd(&name)
    };
    {
        let mut k = managed.kernel.lock();
        k.set_profile(&name);
        k.config.cwd = cwd;
    }
    restart_kernel_impl(managed);
}

/// 停止内核（不切换 profile）。
pub fn stop_kernel(managed: &Managed) {
    {
        let mut k = managed.kernel.lock();
        let _ = k.kill();
    }
    emit_status(managed, KernelStatus::Stopped);
}

/// 删除一个 profile：先停内核（若正在运行该 profile）→ 删目录 → 激活回退 web。
pub fn delete_profile(managed: &Managed, name: String) {
    if name == DEFAULT_PROFILE {
        return; // `web` 是保留 profile，拒绝删除（UI 也已禁用按钮）
    }
    // 若正在运行该 profile，先停止
    {
        let mut k = managed.kernel.lock();
        if k.config.profile == name {
            let _ = k.kill();
        }
    }
    // 删目录
    let profile_dir = state::dsh_home().join("profiles").join(&name);
    if profile_dir.exists() {
        let _ = std::fs::remove_dir_all(&profile_dir);
    }
    // 持久化：移除 profile_cwds；若删除的是激活 profile 则激活回退 web
    let fallback_cwd = {
        let mut cfg = managed.config.lock();
        cfg.profile_cwds.remove(&name);
        if cfg.active_profile == name {
            cfg.active_profile = DEFAULT_PROFILE.to_string();
        }
        let _ = cfg.save();
        cfg.profile_cwd(DEFAULT_PROFILE)
    };
    // 内核配置回退到 web（不自动重启，由仪表盘卡片启动）
    {
        let mut k = managed.kernel.lock();
        k.set_profile(DEFAULT_PROFILE);
        k.config.cwd = fallback_cwd;
    }
    emit_status(managed, KernelStatus::Stopped);
}

/// 用一个最小模板（dsh-base + dsh-web-app）创建新 profile 并自动切换启动。
pub fn create_profile(managed: &Managed, name: String) -> Result<(), String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("名称不能为空".into());
    }
    if name.contains('/') || name.contains('\\') {
        return Err("名称不能包含路径分隔符".into());
    }
    if name == DEFAULT_PROFILE {
        return Err(format!("`{DEFAULT_PROFILE}` 是保留名称，不能用作新 profile"));
    }
    let home = state::dsh_home();
    let profile_dir = home.join("profiles").join(&name);
    if profile_dir.exists() {
        return Err(format!("profile `{name}` 已存在"));
    }
    std::fs::create_dir_all(&profile_dir)
        .map_err(|e| format!("创建 profile 目录失败: {e}"))?;
    let pkg = serde_json::json!({
        "name": format!("dsh-profile-{name}"),
        "private": true,
        "dependencies": {},
        "dsh": { "profile": { "bundles": ["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-web-app"] } }
    });
    let text = serde_json::to_string_pretty(&pkg).map_err(|e| e.to_string())?;
    std::fs::write(profile_dir.join("package.json"), text)
        .map_err(|e| format!("写入 package.json 失败: {e}"))?;
    // 持久化激活 + 工作目录
    {
        let mut cfg = managed.config.lock();
        cfg.active_profile = name.clone();
        cfg.profile_cwds.insert(name.clone(), state::default_cwd());
        let _ = cfg.save();
    }
    switch_profile(managed, name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 建一个隔离的临时目录（绝不碰 `~/.dsh`），并带清理钩子。
    fn temp_ctx(name: &str) -> (PathBuf, impl FnOnce()) {
        let dir = std::env::temp_dir().join(format!(
            "whalenest-test-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let cleanup_dir = dir.clone();
        (dir, move || {
            let _ = std::fs::remove_dir_all(&cleanup_dir);
        })
    }

    #[test]
    fn reads_web_profile_bundles() {
        let (dir, clean) = temp_ctx("bundles-web");
        let pkg = dir.join("package.json");
        std::fs::write(
            &pkg,
            r#"{
  "name": "dsh-profile-web",
  "dependencies": { "dsh-x": "^0.1" },
  "dsh": { "profile": { "bundles": ["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-web-app", "dsh-skin-material-you"] } }
}"#,
        )
        .unwrap();
        let (bundles, is_web) = read_profile_bundles(&pkg);
        assert!(is_web);
        assert!(bundles.contains(&"@deepseek-ai/dsh-web-app".to_string()));
        assert_eq!(
            bundles.iter().filter(|b| !b.starts_with("@deepseek-ai/")).count(),
            1 // dsh-skin-material-you
        );
        clean();
    }

    #[test]
    fn non_web_profile_is_not_web_type() {
        let (dir, clean) = temp_ctx("bundles-tui");
        let pkg = dir.join("package.json");
        std::fs::write(
            &pkg,
            r#"{
  "dependencies": {},
  "dsh": { "profile": { "bundles": ["@deepseek-ai/dsh-base", "dsh-skin-material-you"] } }
}"#,
        )
        .unwrap();
        let (_, is_web) = read_profile_bundles(&pkg);
        assert!(!is_web);
        clean();
    }

    #[test]
    fn session_stats_counts_dirs_by_encoded_cwd() {
        let (dir, clean) = temp_ctx("sessions");
        let encoded = state::encode_session_dir_name(Path::new("/home/huang/proj"));
        let session_dir = dir.join(&encoded);
        std::fs::create_dir_all(session_dir.join("a")).unwrap();
        std::fs::create_dir_all(session_dir.join("b")).unwrap();
        std::fs::create_dir_all(session_dir.join("c")).unwrap();
        std::fs::write(session_dir.join("mtime-marker"), b"x").unwrap();

        let (count, last) = session_stats(&dir, Path::new("/home/huang/proj"));
        assert_eq!(count, 3);
        assert!(last.is_some());
        clean();
    }

    #[test]
    fn session_stats_empty_for_unknown_cwd() {
        let (dir, clean) = temp_ctx("sessions-empty");
        let (count, last) = session_stats(&dir, Path::new("/no/such/cwd"));
        assert_eq!(count, 0);
        assert!(last.is_none());
        clean();
    }
}