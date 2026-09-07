//! WhaleNest — GPUI 版桌面壳（Tauri 版移植）。
//!
//! 启动顺序：
//! 1. gpui_kit::application() + gpui_kit::component::init（必须第一行）
//! 2. 单实例锁（config 目录 flock）
//! 3. Managed 全局状态（config/kernel/事件通道）
//! 4. 监督线程（watcher + readiness）+ 托盘 + 应用级控制监听
//! 5. 打开主窗口（Root + Shell）

mod app;
mod desktop_entry;
mod kernel;
mod lifecycle;
mod notify;
mod plugin_op;
mod readiness;
mod settings;
mod shell;
mod state;
mod tray;
mod updater;
mod wizard;

use std::path::Path;

use gpui_kit::{AnyWindowHandle, App, AppContext, WindowBounds, WindowOptions, px, size};
use gpui_kit::component::Root;

use app::Control;

fn main() {
    // 单实例：锁文件拿不到锁说明已有实例在跑
    let _lock = match acquire_single_instance_lock() {
        Ok(lock) => lock,
        Err(()) => {
            eprintln!("[whalenest] 已有 WhaleNest 实例在运行，退出。");
            return;
        }
    };

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);

            let (managed, controls_rx) = app::Managed::new();
            cx.set_global(managed.clone());

            // 0. Wayland / XDG 桌面入口（.desktop + 主题图标），供合成器按 app_id
            //    解析窗口 / 任务栏图标；失败静默，不影响主流程。
            if let Err(e) = desktop_entry::install() {
                eprintln!("[whalenest] 安装桌面入口失败（忽略）: {e}");
            }

            // 1. 内核首次启动（或引导页）
            app::start_kernel(&managed);

            // 2. 监督线程
            app::spawn_kernel_watcher(&managed);
            app::spawn_readiness(&managed);

            // 3. 托盘（失败静默 → tray_active = false，关闭窗口即退出）
            let managed_tray = managed.clone();
            std::thread::spawn(move || {
                tray::spawn_tray(managed_tray);
            });

            // 4. 应用级控制监听（tray / 无窗口时重开窗口等）
            let managed2 = managed.clone();
            cx.spawn(async move |cx| {
                while let Ok(control) = controls_rx.recv_async().await {
                    handle_control(&managed2, control, &cx);
                }
            })
            .detach();

            // 5. 延迟更新检查（5s 后，静默）
            let managed3 = managed.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(5));
                app::check_update_async(&managed3);
            });

            // 6. 主窗口
            open_main_window(cx);
        });
}

/// 打开（或重新打开）主窗口。窗口内容为 Root<Shell>；关闭后由
/// `Control::OpenMain` 重建（Shell 状态经 Managed 全局保留）。
fn open_main_window(cx: &mut App) {
    cx.open_window(window_options(), |window, cx| {
        let shell = cx.new(|cx| shell::Shell::new(window, cx));
        cx.new(|cx| Root::new(shell, window, cx))
    })
    .expect("Failed to open window");
}

fn window_options() -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Maximized(gpui_kit::Bounds {
            origin: gpui_kit::point(px(0.), px(0.)),
            size: size(px(1280.), px(960.)),
        })),
        window_min_size: Some(size(px(900.), px(600.))),
        titlebar: Some(gpui_kit::component::TitleBar::title_bar_options()),
        #[cfg(target_os = "linux")]
        window_decorations: Some(gpui_kit::WindowDecorations::Client),
        app_id: Some(desktop_entry::APP_ID.to_string()),
        focus: true,
        show: true,
        is_resizable: true,
        is_movable: true,
        is_minimizable: true,
        icon: load_window_icon(),
        ..Default::default()
    }
}

/// 窗口图标（X11；从仓库内图标 PNG 解码缩放）。
fn load_window_icon() -> Option<std::sync::Arc<image::RgbaImage>> {
    let bytes = include_bytes!("../public/whalenest-mark.png");
    let img = image::load_from_memory(bytes).ok()?;
    let small = img.resize(128, 128, image::imageops::FilterType::Triangle);
    Some(std::sync::Arc::new(small.to_rgba8()))
}

/// 应用级控制监听。cx 提供 AsyncApp；无窗口时 open_window 重建。
fn handle_control(managed: &app::Managed, control: Control, cx: &gpui_kit::AsyncApp) {
    match control {
        Control::OpenMain => {
            let _ = cx.update(|cx| {
                let windows: Vec<AnyWindowHandle> = cx.windows();
                if windows.is_empty() {
                    open_main_window(cx);
                } else if let Some(handle) = windows.first() {
                    let _ = handle.update(cx, |_, window, _| window.activate_window());
                }
            });
        }
        Control::RestartKernel => app::restart_kernel_impl(managed),
        Control::OpenBrowser => {
            let url = match &managed.kernel.lock().state {
                kernel::KernelState::Ready { url, .. } => url.clone(),
                _ => String::new(),
            };
            if url.is_empty() {
                eprintln!("[whalenest] dsh 内核尚未就绪，暂无可打开的地址");
            } else {
                let _ = lifecycle::open_url(&url);
            }
        }
        Control::CopyDiagnostics => {
            let _ = app::copy_diagnostics(managed);
        }
        Control::OpenLog => {
            let _ = app::open_log_file(managed);
        }
        Control::OpenConfig => {
            let _ = app::open_config_file();
        }
        Control::ToggleAutostart => {
            let new_val = !managed.config.lock().autostart;
            let _ = lifecycle::set_autostart(managed, new_val);
        }
        Control::SetAutostart(v) => {
            let _ = lifecycle::set_autostart(managed, v);
        }
        Control::CheckUpdate => app::check_update_async(managed),
        Control::InstallUpdate => app::install_update_async(managed),
        Control::InstallDsh => app::install_dsh(managed),
        Control::SwitchProfile(name) => app::switch_profile(managed, name),
        // 创建 profile 需要名称输入，由仪表盘对话框（Phase 3）直接调用
        // `app::create_profile`；此处字符串事件仅作占位（tray 不会触发它）。
        Control::CreateProfile => {}
        Control::DeleteProfile(name) => app::delete_profile(managed, name),
        Control::Stop => app::stop_kernel(managed),
        Control::SetLockPort(v) => {
            managed.config.lock().lock_port = v;
            managed.kernel.lock().config.lock_port = v;
            let _ = managed.config.lock().save();
            app::restart_kernel_impl(managed);
        }
        Control::Quit => {
            let _ = cx.update(|cx| lifecycle::exit_app(cx));
        }
    }
}

/// 单实例锁：对 config 目录的 lock 文件做 flock(LOCK_EX|LOCK_NB)。
fn acquire_single_instance_lock() -> Result<std::fs::File, ()> {
    let dir = state::init_config_dir();
    let lock_path = Path::new(&dir).join("whalenest.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|_| ())?;
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let fd = file.as_raw_fd();
        let rc = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            return Err(());
        }
    }
    Ok(file)
}