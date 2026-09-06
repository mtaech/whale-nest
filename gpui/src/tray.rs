//! 系统托盘。
//!
//! - Linux：ksni（纯 Rust StatusNotifierItem，不需要 GTK 主循环；在
//!   KDE/GNOME-扩展等支持 SNI 的面板上工作）。菜单在每次打开时由 DBusMenu
//!   重新取（GetLayout），因此无需手动刷新菜单项状态。
//! - Windows / macOS：tray-icon crate（自带事件泵线程；本机未验证运行行为，
//!   仅保证编译与 API 正确）。
//!
//! 托盘不可用时静默降级：`tray_active() == false` 时「关闭窗口 = 退出」，
//! 见 shell.rs 的 on_window_should_close。

use std::sync::atomic::{AtomicBool, Ordering};

use crate::app::{Control, Managed};

/// 托盘是否成功启动（与「关闭窗口 = 退出」的策略联动）。
static TRAY_ACTIVE: AtomicBool = AtomicBool::new(false);

/// 托盘是否可用。
pub fn tray_active() -> bool {
    TRAY_ACTIVE.load(Ordering::Acquire)
}

/// 解码仓库内图标并缩到托盘尺寸。
fn tray_icon_rgba() -> Option<(u32, u32, Vec<u8>)> {
    let bytes = include_bytes!("../../public/whalenest-mark.png");
    let img = image::load_from_memory(bytes).ok()?;
    let small = img.resize(32, 32, image::imageops::FilterType::Triangle);
    let rgba = small.to_rgba8();
    Some((rgba.width(), rgba.height(), rgba.as_raw().clone()))
}

// ── Linux（ksni）────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use ksni::{Category, Handle, Icon, MenuItem, TrayService, menu::*};
    use std::sync::Mutex;

    pub struct WhalenestTray {
        pub managed: Managed,
        icon: Vec<Icon>,
    }

    /// 生效中的托盘句柄（spawn 后由 TrayService 提供）；重建时 drop 旧的。
    static TRAY: Mutex<Option<Handle<WhalenestTray>>> = Mutex::new(None);

    impl WhalenestTray {
        pub fn new(managed: Managed) -> Self {
            // ksni 需要 ARGB32（大端字节序：A R G B）
            let icon = tray_icon_rgba()
                .map(|(w, h, raw)| {
                    let mut argb = Vec::with_capacity(raw.len());
                    for px in raw.chunks_exact(4) {
                        argb.push(px[3]);
                        argb.push(px[0]);
                        argb.push(px[1]);
                        argb.push(px[2]);
                    }
                    vec![Icon {
                        width: w as i32,
                        height: h as i32,
                        data: argb,
                    }]
                })
                .unwrap_or_default();
            Self { managed, icon }
        }
    }

    impl ksni::Tray for WhalenestTray {
        fn id(&self) -> String {
            "dev.whalenest.desktop".into()
        }
        fn title(&self) -> String {
            "WhaleNest".into()
        }
        fn category(&self) -> Category {
            Category::ApplicationStatus
        }
        fn icon_pixmap(&self) -> Vec<Icon> {
            self.icon.clone()
        }
        /// 托盘图标左键单击 = 打开主界面
        fn activate(&mut self, _x: i32, _y: i32) {
            let _ = self.managed.controls.send(Control::OpenMain);
        }

        fn menu(&self) -> Vec<MenuItem<Self>> {
            let autostart = self.managed.config.lock().autostart;
            let update = self.managed.update.lock().clone();

            let send = |ctl: Control| {
                let tx = self.managed.controls.clone();
                Box::new(move |_: &mut Self| {
                    let _ = tx.send(ctl.clone());
                })
            };

            let more_submenu = SubMenu {
                label: "更多".into(),
                submenu: vec![
                    StandardItem {
                        label: "复制诊断信息".into(),
                        activate: send(Control::CopyDiagnostics),
                        ..Default::default()
                    }
                    .into(),
                    StandardItem {
                        label: "打开日志".into(),
                        activate: send(Control::OpenLog),
                        ..Default::default()
                    }
                    .into(),
                    StandardItem {
                        label: "打开配置文件".into(),
                        activate: send(Control::OpenConfig),
                        ..Default::default()
                    }
                    .into(),
                    MenuItem::Separator,
                    StandardItem {
                        label: "检查更新".into(),
                        activate: send(Control::CheckUpdate),
                        ..Default::default()
                    }
                    .into(),
                    StandardItem {
                        label: update_label(&update),
                        enabled: update.as_ref().map(|u| u.has_update).unwrap_or(false),
                        activate: send(Control::InstallUpdate),
                        ..Default::default()
                    }
                    .into(),
                ],
                ..Default::default()
            }
            .into();

            let switch_items: Vec<MenuItem<Self>> = crate::app::scan_profiles(&self.managed)
                .into_iter()
                .filter(|p| p.is_web())
                .map(|p| {
                    let name = p.name.clone();
                    StandardItem {
                        label: format!("切换到 {name}").into(),
                        activate: send(Control::SwitchProfile(name)),
                        ..Default::default()
                    }
                    .into()
                })
                .collect();
            let switch_submenu = SubMenu {
                label: "切换 profile".into(),
                submenu: switch_items,
                ..Default::default()
            }
            .into();

            vec![
                StandardItem {
                    label: "打开主界面".into(),
                    activate: send(Control::OpenMain),
                    ..Default::default()
                }
                .into(),
                switch_submenu,
                MenuItem::Separator,
                StandardItem {
                    label: "重启 dsh 内核".into(),
                    activate: send(Control::RestartKernel),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: "在浏览器打开".into(),
                    activate: send(Control::OpenBrowser),
                    ..Default::default()
                }
                .into(),
                MenuItem::Separator,
                more_submenu,
                MenuItem::Separator,
                CheckmarkItem {
                    label: "开机自启".into(),
                    checked: autostart,
                    activate: send(Control::ToggleAutostart),
                    ..Default::default()
                }
                .into(),
                MenuItem::Separator,
                StandardItem {
                    label: "退出".into(),
                    activate: send(Control::Quit),
                    ..Default::default()
                }
                .into(),
            ]
        }
    }

    fn update_label(update: &Option<crate::updater::UpdateInfo>) -> String {
        match update {
            Some(u) if u.has_update => format!("发现新版本 v{} → 更新", u.latest),
            _ => "发现新版本".into(),
        }
    }

    /// 启动托盘（SNI）。无 DBus / 无 SNI 宿主时静默失败；调用后置位
    /// `TRAY_ACTIVE` 供「关闭窗口 = 托盘化」策略判断。
    pub fn spawn_tray(managed: Managed) -> bool {
        let service = TrayService::new(WhalenestTray::new(managed));
        let handle = service.handle();
        if let Ok(mut guard) = TRAY.lock() {
            *guard = Some(handle);
        }
        service.spawn();
        TRAY_ACTIVE.store(true, Ordering::Release);
        true
    }

    /// 更新检查完成后的托盘刷新（菜单是打开时实时取，这里只触发一次属性推送）。
    pub fn refresh_tray() {
        if let Ok(guard) = TRAY.lock() {
            if let Some(h) = guard.as_ref() {
                h.update(|_| {});
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub use imp::{refresh_tray as refresh_update_item, spawn_tray};

// ── Windows / macOS（tray-icon）────────────────────────────────────────────

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::*;
    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
    use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};
    use std::sync::Mutex;

    static TRAY: Mutex<Option<TrayIcon>> = Mutex::new(None);

    fn menu_item(
        id: &str,
        label: &'static str,
        enabled: bool,
    ) -> Result<tray_icon::menu::MenuItem, Box<dyn std::error::Error>> {
        MenuItem::with_id(id, label, enabled, None).map_err(|e| e.into())
    }

    pub fn spawn_tray(managed: Managed) -> bool {
        let _ = TRAY_ACTIVE.fetch_or(true, Ordering::Release);

        let open_main = match menu_item("open-main", "打开主界面", true) {
            Ok(m) => m,
            Err(_) => return false,
        };
        let restart = match menu_item("restart-kernel", "重启 dsh 内核", true) {
            Ok(m) => m,
            Err(_) => return false,
        };
        let open_browser = match menu_item("open-browser", "在浏览器打开", true) {
            Ok(m) => m,
            Err(_) => return false,
        };
        let copy_diag = match menu_item("copy-diagnostics", "复制诊断信息", true) {
            Ok(m) => m,
            Err(_) => return false,
        };
        let open_log = match menu_item("open-log", "打开日志", true) {
            Ok(m) => m,
            Err(_) => return false,
        };
        let open_config = match menu_item("open-config", "打开配置文件", true) {
            Ok(m) => m,
            Err(_) => return false,
        };
        let check_update = match menu_item("check-update", "检查更新", true) {
            Ok(m) => m,
            Err(_) => return false,
        };
        let has_update = managed.update.lock().as_ref().map(|u| u.has_update).unwrap_or(false);
        let label = match managed.update.lock().as_ref() {
            Some(u) if u.has_update => format!("发现新版本 v{} → 更新", u.latest),
            _ => "发现新版本".to_string(),
        };
        let install_update = match MenuItem::with_id("install-update", label, has_update, None) {
            Ok(m) => m,
            Err(_) => return false,
        };
        let autostart_item =
            match CheckMenuItem::with_id("toggle-autostart", "开机自启", true, managed.config.lock().autostart, None) {
                Ok(m) => m,
                Err(_) => return false,
            };
        let quit = match menu_item("quit", "退出", true) {
            Ok(m) => m,
            Err(_) => return false,
        };

        // 切换 profile 子菜单（菜单构建时扫描；新建 profile 后需重启生效，已知限制）
        let mut switch_items: Vec<MenuItem> = Vec::new();
        for p in crate::app::scan_profiles(&managed) {
            if !p.is_web() {
                continue;
            }
            if let Ok(item) = MenuItem::with_id(
                &format!("switch-{}", p.name),
                &format!("切换到 {}", p.name),
                true,
                None,
            ) {
                switch_items.push(item);
            }
        }
        let switch_refs: Vec<&dyn tray_icon::menu::IsMenuItem> =
            switch_items.iter().map(|i| i as &dyn tray_icon::menu::IsMenuItem).collect();
        let switch_submenu = match Submenu::with_id_and_items(
            "switch-profile",
            "切换 profile",
            true,
            &switch_refs,
        ) {
            Ok(m) => m,
            Err(_) => return false,
        };

        let sep = PredefinedMenuItem::separator();
        let more_submenu = match Submenu::with_id_and_items(
            "more",
            "更多",
            true,
            &[
                &copy_diag as &dyn tray_icon::menu::IsMenuItem,
                &open_log,
                &open_config,
                &sep,
                &check_update,
                &install_update,
            ],
        ) {
            Ok(m) => m,
            Err(_) => return false,
        };

        let menu = match Menu::with_items(&[
            &open_main,
            &switch_submenu,
            &sep,
            &restart,
            &open_browser,
            &sep,
            &more_submenu,
            &sep,
            &autostart_item,
            &quit,
        ]) {
            Ok(m) => m,
            Err(_) => return false,
        };

        let icon = tray_icon_rgba()
            .ok()
            .and_then(|(w, h, rgba)| Icon::from_rgba(rgba, w, h).ok());

        let mut builder = TrayIconBuilder::new()
            .with_id("main-tray")
            .with_tooltip("WhaleNest")
            .with_menu(Box::new(menu));
        if let Some(icon) = icon {
            builder = builder.with_icon(icon);
        }
        let tray = match builder.build() {
            Ok(t) => t,
            Err(_) => return false,
        };

        let controls = managed.controls.clone();
        let menu_events = MenuEvent::receiver();
        let tray_events = TrayIconEvent::receiver();
        std::thread::spawn(move || loop {
            while let Ok(event) = menu_events.try_recv() {
                dispatch(&controls, &event.id.0);
            }
            while let Ok(event) = tray_events.try_recv() {
                if let TrayIconEvent::Click { .. } = event {
                    let _ = controls.send(Control::OpenMain);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        });

        if let Ok(mut guard) = TRAY.lock() {
            *guard = Some(tray);
        }
        true
    }

    fn dispatch(controls: &flume::Sender<Control>, id: &str) {
        if let Some(name) = id.strip_prefix("switch-") {
            let _ = controls.send(Control::SwitchProfile(name.to_string()));
            return;
        }
        let ctl = match id {
            "open-main" => Control::OpenMain,
            "restart-kernel" => Control::RestartKernel,
            "open-browser" => Control::OpenBrowser,
            "copy-diagnostics" => Control::CopyDiagnostics,
            "open-log" => Control::OpenLog,
            "open-config" => Control::OpenConfig,
            "check-update" => Control::CheckUpdate,
            "install-update" => Control::InstallUpdate,
            "toggle-autostart" => Control::ToggleAutostart,
            "quit" => Control::Quit,
            _ => return,
        };
        let _ = controls.send(ctl);
    }

    /// 更新检查完成后的托盘刷新（非 Linux 平台：菜单文本在下次打开时由
    /// 平台重建读取为准——本版不动态改文本，属已知限制）。
    pub fn refresh_tray() {}
}

#[cfg(not(target_os = "linux"))]
pub use imp::{refresh_tray as refresh_update_item, spawn_tray};