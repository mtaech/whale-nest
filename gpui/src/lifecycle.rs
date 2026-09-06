//! 生命周期：开机自启、外链/文件打开、退出序列。
//!
//! 原 Tauri 版用 tauri-plugin-autostart / tauri-plugin-opener；GPUI 版手写：
//! - 自启：Linux `~/.config/autostart/*.desktop`；Windows 注册表 Run 键；
//!   macOS 未实现（返回错误，由调用方静默）。
//! - 打开：`open` crate（xdg-open / 系统默认处理程序）。
//! - 退出：置位 shutting_down → kill 内核 → 退出事件循环。

use std::path::Path;

use crate::app::Managed;

/// 打开 URL（http/https/mailto），走系统默认浏览器。
pub fn open_url(url: &str) -> Result<(), String> {
    open::that_detached(url).map_err(|e| format!("打开链接失败: {e}"))
}

/// 用系统默认程序打开文件/目录/日志。
pub fn open_path(path: &Path) -> Result<(), String> {
    open::that_detached(path).map_err(|e| format!("打开 {} 失败: {e}", path.display()))
}

// ── 开机自启 ────────────────────────────────────────────────────────────────

pub fn set_autostart(managed: &Managed, enabled: bool) -> Result<(), String> {
    if enabled {
        enable_autostart()?;
    } else {
        disable_autostart()?;
    }
    managed.config.lock().autostart = enabled;
    let _ = managed.config.lock().save();
    Ok(())
}

#[cfg(target_os = "linux")]
fn autostart_desktop_path() -> std::path::PathBuf {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let base = home
        .map(|h| h.join(".config"))
        .unwrap_or_else(std::env::temp_dir);
    base.join("autostart").join("whalenest.desktop")
}

#[cfg(target_os = "linux")]
fn enable_autostart() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let path = autostart_desktop_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let content = format!(
        "[Desktop Entry]\nType=Application\nName=WhaleNest\nComment=DeepSeek Harness desktop shell\nExec=\"{}\" --silent\nX-GNOME-Autostart-enabled=true\n",
        exe.display()
    );
    std::fs::write(&path, content).map_err(|e| format!("写入自启文件失败: {e}"))
}

#[cfg(target_os = "linux")]
fn disable_autostart() -> Result<(), String> {
    let path = autostart_desktop_path();
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("删除自启文件失败: {e}")),
    }
}

#[cfg(target_os = "windows")]
fn enable_autostart() -> Result<(), String> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _disp) = hkcu
        .create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")
        .map_err(|e| e.to_string())?;
    key.set_value(
        "WhaleNest",
        &format!("\"{}\" --silent", exe.display()),
    )
    .map_err(|e| e.to_string())
}

#[cfg(target_os = "windows")]
fn disable_autostart() -> Result<(), String> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _disp) = hkcu
        .create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")
        .map_err(|e| e.to_string())?;
    match key.delete_value("WhaleNest") {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("删除自启注册表项失败: {e}")),
    }
}

#[cfg(target_os = "macos")]
fn enable_autostart() -> Result<(), String> {
    Err("macOS 开机自启尚未实现（GPUI 版暂缺 LaunchAgent 安装）".into())
}

#[cfg(target_os = "macos")]
fn disable_autostart() -> Result<(), String> {
    Err("macOS 开机自启尚未实现（GPUI 版暂缺 LaunchAgent 卸载）".into())
}

// ── 退出 ────────────────────────────────────────────────────────────────────

/// 退出：先 kill dsh 内核，再退出应用。置位 shutting_down 阻止监督线程把
/// 有意终止误判成崩溃而重启。
pub fn exit_app(cx: &mut gpui_kit::App) {
    if let Some(managed) = gpui_kit::App::try_global::<Managed>(cx) {
        managed.shutting_down.store(true, std::sync::atomic::Ordering::Release);
        let mut kernel = managed.kernel.lock();
        let _ = kernel.kill();
    }
    cx.quit();
}