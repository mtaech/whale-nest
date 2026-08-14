//! Tray menu, window-close-to-tray, autostart, and app exit.

use tauri::menu::{CheckMenuItem, Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, CloseRequestApi, Manager, Runtime, Window};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_clipboard_manager::ClipboardExt as _;

use crate::{prompt_switch_dir, restart_kernel_impl, Managed};

/// Handle to the autostart checkbox, kept so the toggle can sync its state.
static AUTOSTART_ITEM: std::sync::OnceLock<tauri::menu::CheckMenuItem<tauri::Wry>> =
    std::sync::OnceLock::new();

/// Full 7-item tray menu:
/// 打开主界面 / 切换工作目录 / 重启 dsh 内核 / 复制诊断信息 / 打开日志 / 开机自启(勾选) / 退出
pub fn build_tray_menu(app: &AppHandle) -> tauri::Result<()> {
    let autostart = app
        .state::<Managed>()
        .config
        .lock()
        .unwrap()
        .autostart;

    let open_main = MenuItem::with_id(app, "open-main", "打开主界面", true, None::<&str>)?;
    let switch_cwd = MenuItem::with_id(app, "switch-cwd", "切换工作目录", true, None::<&str>)?;
    let restart = MenuItem::with_id(app, "restart-kernel", "重启 dsh 内核", true, None::<&str>)?;
    let copy_diag = MenuItem::with_id(app, "copy-diagnostics", "复制诊断信息", true, None::<&str>)?;
    let open_log = MenuItem::with_id(app, "open-log", "打开日志", true, None::<&str>)?;
    let autostart_item = CheckMenuItem::with_id(
        app,
        "toggle-autostart",
        "开机自启",
        true,
        autostart,
        None::<&str>,
    )?;
    let _ = AUTOSTART_ITEM.set(autostart_item.clone());
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[&open_main, &switch_cwd, &restart, &copy_diag, &open_log, &autostart_item, &quit],
    )?;

    TrayIconBuilder::with_id("main-tray")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            let _ = handle_tray_event(app, event.id.as_ref());
        })
        .on_tray_icon_event(|tray, event| {
            // Left-click the tray icon: show and focus the main window.
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                if let Some(window) = tray.app_handle().get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
        })
        .build(app)?;
    Ok(())
}

/// Tray menu event dispatch.
pub fn handle_tray_event(app: &AppHandle, id: &str) -> Result<(), String> {
    match id {
        "open-main" => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }
        "switch-cwd" => prompt_switch_dir(app)?,
        "restart-kernel" => restart_kernel_impl(app),
        "copy-diagnostics" => {
            let diag = app.state::<Managed>().kernel.lock().unwrap().diagnostics();
            app.clipboard().write_text(diag).map_err(|e| e.to_string())?;
        }
        "open-log" => {
            let path = app
                .state::<Managed>()
                .kernel
                .lock()
                .unwrap()
                .log_path()
                .to_path_buf();
            tauri_plugin_opener::open_path(path, None::<&str>).map_err(|e| e.to_string())?;
        }
        "toggle-autostart" => {
            let new_val = !app.state::<Managed>().config.lock().unwrap().autostart;
            set_autostart(app, new_val)?;
            // Keep the checkbox in sync with the new value.
            if let Some(check) = AUTOSTART_ITEM.get() {
                let _ = check.set_checked(new_val);
            }
        }
        "quit" => exit_app(app),
        _ => {}
    }
    Ok(())
}

/// Close request = hide to tray (dsh keeps running). Consistent across
/// platforms; on Linux without a tray the window is still hidden and can be
/// recalled from the app icon (single-instance focuses it).
pub fn on_window_close_request<R: Runtime>(window: &Window<R>, api: &CloseRequestApi) {
    api.prevent_close();
    let _ = window.hide();
}

/// Autostart wrapper: Win = registry Run key; Lin = ~/.config/autostart
/// (tauri-plugin-autostart). Config.autostart stays the source of truth.
pub fn set_autostart(app: &AppHandle, enabled: bool) -> Result<(), String> {
    let manager = app.autolaunch();
    if enabled {
        manager.enable().map_err(|e| e.to_string())?;
    } else {
        manager.disable().map_err(|e| e.to_string())?;
    }
    let managed = app.state::<Managed>();
    let mut cfg = managed.config.lock().unwrap();
    cfg.autostart = enabled;
    let _ = cfg.save();
    Ok(())
}

/// Quit: kill the dsh kernel first, then exit the app for real.
pub fn exit_app(app: &AppHandle) {
    let managed = app.state::<Managed>();
    if let Ok(mut kernel) = managed.kernel.lock() {
        let _ = kernel.kill();
    }
    app.exit(0);
}
