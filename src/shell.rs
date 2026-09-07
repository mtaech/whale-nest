//! Shell 根视图：标题栏 + 仪表盘（profile 卡片）/ 向导 / 引导 / 日志区块 + 设置对话框。
//!
//! 仪表盘是主视图（dsh 缺失时退化为引导；首次运行时为单步向导）。每个 web 型
//! profile 一张卡，呈现运行状态、meta 信息与操作按钮；底部展示当前 profile 的日志 tail。

use gpui_kit::{
    App, AsyncWindowContext, ClickEvent, Context, Entity, Hsla, Render, ScrollHandle, WeakEntity,
    Window, div,
    prelude::*, px,
};
use gpui_kit::base::Scrollbar;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Root, Sizable as _, StyledExt as _,
    TitleBar, WindowExt as _, button::{Button, ButtonVariants}, h_flex,
    input::{Input, InputState}, label::Label, notification::NotificationType, switch::Switch,
    tab::{Tab, TabBar},
    v_flex,
};

use crate::app::{self, AppEvent, Managed};
use crate::kernel::KernelState;
use crate::state::ProfileInfo;

/// 内核状态镜像（用于标题栏 / 状态灯）。
#[derive(Clone, Debug)]
pub enum ShellState {
    Loading,
    Ready { url: String },
    Error(String),
    Guide,
    Stopped,
}

/// 一张 profile 卡片的状态灯。
#[derive(Clone, Copy, PartialEq)]
enum CardStatus {
    Running,
    Starting,
    Stopped,
    Error,
}

pub struct Shell {
    managed: Managed,
    state: ShellState,
    /// 未完成首次向导时强制展示向导
    show_wizard: bool,
    /// 更新横幅
    update: Option<app::UpdateEventPayload>,
    pub(crate) updating: bool,
    /// dsh profile 快照（运行时扫描，不持久化）
    profiles: Vec<ProfileInfo>,
    /// 当前选中的 profile 名称（用于右侧工作区展示）。
    selected_profile: Option<String>,
    /// 底部日志区块展示的 tail。
    log_tail: String,
    /// 日志滚动区句柄（跟随尾部 + 滚动条）。
    log_scroll: ScrollHandle,
    /// 待前台打开（点「浏览器打开」但内核未就绪时置位）。
    pending_open: bool,
    /// 创建 profile 对话框的输入状态。
    create_input: Option<Entity<InputState>>,
    /// 插件面板「安装」输入框状态。
    install_input: Option<Entity<InputState>>,
    /// 当前 profile 可升级插件缓存（包名 -> 最新版本）。
    outdated: std::collections::HashMap<String, String>,
    /// 已查询过 outdated 的 profile 名（避免每次渲染重复查询）。
    outdated_profile: Option<String>,
    /// 详情区当前激活的 Tab：0=日志（默认），1=插件管理。
    active_tab: usize,
    /// 事件订阅
    events_rx: flume::Receiver<AppEvent>,
    /// 向导状态
    pub(crate) env_check: Option<crate::wizard::EnvCheckUi>,
    pub(crate) env_check_triggered: bool,
    pub(crate) wizard_running: bool,
}

/// Shell 实体的窗口内弱引用（设置对话框构建闭包里取用）。
pub struct ShellHandle(pub WeakEntity<Shell>);
impl gpui_kit::Global for ShellHandle {}

/// 设置面板读取的只读快照。
#[derive(Clone, Default)]
pub struct SettingsSnapshot {
    pub autostart: bool,
    pub lock_port: bool,
    pub update: (String, String, bool),
}

impl Shell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        gpui_kit::component::theme::Theme::change(
            gpui_kit::component::theme::ThemeMode::Light,
            Some(window),
            cx,
        );

        let managed = cx.global::<Managed>().clone();
        let events_rx = managed.events_rx.clone();
        let initialized = managed.config.lock().initialized;
        let snapshot = Self::snapshot_from_kernel(&managed);

        let active_profile = managed.kernel.lock().config.profile.clone();
        let mut this = Self {
            managed: managed.clone(),
            state: snapshot.state,
            show_wizard: !initialized,
            update: managed.update.lock().clone().map(Into::into),
            updating: false,
            profiles: Vec::new(),
            selected_profile: Some(active_profile),
            log_tail: String::new(),
            log_scroll: ScrollHandle::new(),
            pending_open: false,
            create_input: None,
            install_input: None,
            outdated: std::collections::HashMap::new(),
            outdated_profile: None,
            active_tab: 0,
            events_rx,
            env_check: None,
            env_check_triggered: false,
            wizard_running: false,
        };
        this.refresh(cx);

        // 供设置对话框（App 上下文）取回 Shell 实体
        cx.set_global(ShellHandle(cx.entity().downgrade()));

        window.on_window_should_close(cx, |_window, cx| {
            if crate::tray::tray_active() {
                true // 窗口移除，应用与 dsh 内核继续后台运行
            } else {
                crate::lifecycle::exit_app(cx);
                true
            }
        });

        this.spawn_event_loop(window, cx);
        this
    }

    /// 从内核共享状态恢复界面快照（窗口重开 / 初始载入共用）。
    fn snapshot_from_kernel(managed: &Managed) -> KernelSnapshot {
        let kernel = managed.kernel.lock();
        let state = match &kernel.state {
            KernelState::Stopped => ShellState::Stopped,
            KernelState::Starting => ShellState::Loading,
            KernelState::Ready { url, .. } => ShellState::Ready { url: url.clone() },
            KernelState::Crashed { last_error, .. } => {
                if kernel.dsh_available() {
                    ShellState::Error(last_error.clone())
                } else {
                    ShellState::Guide
                }
            }
        };
        KernelSnapshot { state }
    }

    /// 重新扫描 profile 列表并刷新日志 tail。
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        self.profiles = app::scan_profiles(&self.managed);
        self.log_tail = self.managed.kernel.lock().log_tail(120);
        // 日志更新后滚动到底部跟随最新；若以后要保留用户上滚位置，需先判 offset。
        self.log_scroll.scroll_to_bottom();
        cx.notify();
    }

    /// 事件订阅：监督线程 / 后台任务的结果经 flume 通道推到这里。
    fn spawn_event_loop(&mut self, window: &Window, cx: &mut Context<Self>) {
        let rx = self.events_rx.clone();
        cx.spawn_in(window, async move |this: WeakEntity<Self>, cx: &mut AsyncWindowContext| {
            while let Ok(event) = rx.recv_async().await {
                let _ = this.update_in(&mut *cx, |this, window, cx| {
                    this.on_event(event, window, cx);
                });
            }
        })
        .detach();
    }

    fn on_event(&mut self, event: AppEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            AppEvent::KernelStatus { profile: _, status } => match status {
                crate::kernel::KernelStatus::Ready { url } => {
                    self.set_ready(&url, window, cx);
                    if self.pending_open {
                        self.pending_open = false;
                        let _ = app::open_external_url(url);
                    }
                }
                crate::kernel::KernelStatus::Starting => {
                    self.state = ShellState::Loading;
                }
                crate::kernel::KernelStatus::Error { message } => {
                    self.state = ShellState::Error(message);
                }
                crate::kernel::KernelStatus::Guide => {
                    self.state = ShellState::Guide;
                }
                crate::kernel::KernelStatus::Stopped => {
                    self.state = ShellState::Stopped;
                }
            },
            AppEvent::Update(payload) => {
                self.update = Some(payload);
                self.updating = false;
            }
            AppEvent::DshInstalled => {
                self.wizard_running = false;
                window.push_notification(
                    (NotificationType::Success, "dsh 已安装，正在重新检测…"),
                    cx,
                );
                if self.show_wizard {
                    self.recheck_env(window, cx);
                }
            }
            AppEvent::PluginOpStarted { message, .. } => {
                window.push_notification((NotificationType::Info, message), cx);
            }
            AppEvent::PluginOpResult { ok, message, .. } => {
                let notif = if ok {
                    NotificationType::Success
                } else {
                    NotificationType::Error
                };
                window.push_notification((notif, message), cx);
                self.refresh(cx);
            }
            AppEvent::PluginOutdated { updates, .. } => {
                self.outdated.clear();
                for (name, latest) in updates {
                    self.outdated.insert(name, latest);
                }
                cx.notify();
            }
        }
        cx.notify();
    }

    fn set_ready(&mut self, url: &str, _window: &mut Window, cx: &mut Context<Self>) {
        self.state = ShellState::Ready { url: url.into() };
        cx.notify();
    }

    // ── 用户操作 ────────────────────────────────────────────────────────────

    pub(crate) fn restart_kernel(&self) {
        app::restart_kernel_impl(&self.managed);
    }

    pub(crate) fn copy_diagnostics(&self, window: &mut Window, cx: &mut App) {
        match app::copy_diagnostics(&self.managed) {
            Ok(()) => window.push_notification(
                (NotificationType::Success, "诊断信息已复制到剪贴板"),
                cx,
            ),
            Err(e) => window.push_notification((NotificationType::Error, e), cx),
        }
    }

    pub(crate) fn open_log(&self, window: &mut Window, cx: &mut App) {
        match app::open_log_file(&self.managed) {
            Ok(()) => {}
            Err(e) => window.push_notification((NotificationType::Error, e), cx),
        }
    }

    pub(crate) fn open_config_file(&self, window: &mut Window, cx: &mut App) {
        match app::open_config_file() {
            Ok(()) => {}
            Err(e) => window.push_notification((NotificationType::Error, e), cx),
        }
    }

    pub(crate) fn check_update(&mut self) {
        self.updating = true;
        app::check_update_async(&self.managed);
    }

    pub(crate) fn install_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.updating = true;
        app::install_update_async(&self.managed);
        window.push_notification((NotificationType::Info, "后台安装 dsh 新版本中…"), cx);
    }

    pub(crate) fn set_autostart(&mut self, enabled: bool, window: &mut Window, cx: &mut Context<Self>) {
        let result = crate::lifecycle::set_autostart(&self.managed, enabled);
        match result {
            Ok(()) => {
                window.push_notification(
                    if enabled {
                        (NotificationType::Success, "已开启开机自启")
                    } else {
                        (NotificationType::Info, "已关闭开机自启")
                    },
                    cx,
                );
            }
            Err(e) => {
                self.managed.config.lock().autostart = !enabled;
                window.push_notification((NotificationType::Error, e), cx);
            }
        }
        cx.notify();
    }

    pub(crate) fn set_lock_port(&mut self, enabled: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.managed.config.lock().lock_port = enabled;
        self.managed.kernel.lock().config.lock_port = enabled;
        let _ = self.managed.config.lock().save();
        window.push_notification(
            if enabled {
                (NotificationType::Info, "已锁定端口，重启内核后生效")
            } else {
                (NotificationType::Info, "已解除端口锁定，重启内核后生效")
            },
            cx,
        );
        self.restart_kernel();
        cx.notify();
    }

    pub(crate) fn install_dsh(&mut self) {
        self.wizard_running = true;
        app::install_dsh(&self.managed);
    }

    /// 后台跑环境检测并把结果写回。
    pub(crate) fn recheck_env(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.env_check = Some(crate::wizard::EnvCheckUi::Checking);
        cx.spawn_in(window, async move |this: WeakEntity<Self>, cx: &mut AsyncWindowContext| {
            let result = cx
                .background_executor()
                .spawn(async move { app::check_env() })
                .await;
            let _ = this.update_in(&mut *cx, |shell, _window, cx| {
                shell.env_check = Some(crate::wizard::EnvCheckUi::Result(result));
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn finish_wizard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.wizard_running = true;
        match app::complete_setup(&self.managed) {
            Ok(()) => {
                self.show_wizard = false;
                self.state = ShellState::Loading;
            }
            Err(e) => {
                window.push_notification((NotificationType::Error, e), cx);
            }
        }
        cx.notify();
    }

    /// 卡片「启动/切换/重启」。
    pub(crate) fn start_profile(&self, name: String) {
        let running = {
            let k = self.managed.kernel.lock();
            k.config.profile == name && matches!(k.state, KernelState::Ready { .. })
        };
        if running {
            app::restart_kernel_impl(&self.managed);
        } else {
            app::switch_profile(&self.managed, name);
        }
    }

    /// 卡片「在浏览器打开」：运行中直接开；否则切换启动并等就绪后开。
    pub(crate) fn open_browser_for(&mut self, name: String) {
        let url = {
            let k = self.managed.kernel.lock();
            if k.config.profile == name {
                match &k.state {
                    KernelState::Ready { url, .. } => Some(url.clone()),
                    _ => None,
                }
            } else {
                None
            }
        };
        match url {
            Some(url) => {
                let _ = app::open_external_url(url);
            }
            None => {
                self.pending_open = true;
                app::switch_profile(&self.managed, name);
            }
        }
    }

    /// 卡片「更改目录」。
    pub(crate) fn change_cwd_for(&self, name: String) {
        app::prompt_change_cwd(&self.managed, name);
    }

    /// 卡片「删除」：弹确认框，`web` 禁删。
    pub(crate) fn confirm_delete_profile(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        if name == crate::state::DEFAULT_PROFILE {
            window.push_notification(
                (NotificationType::Error, "`web` 是保留 profile，不能删除"),
                cx,
            );
            return;
        }
        let running = self.card_status(&name) == CardStatus::Running;
        let shell = cx.entity().clone();
        window.open_dialog(cx, move |dialog, _window, _cx| {
            let msg = if running {
                format!("「{name}」正在运行，将先停止再删除该 profile。")
            } else {
                format!("确定删除 profile「{name}」吗？")
            };
            dialog
                .title("删除 profile")
                .w(px(420.))
                .child(
                    v_flex()
                        .id("whalenest-delete-body")
                        .gap_2()
                        .child(Label::new(msg).text_sm()),
                )
                .footer(
                    h_flex()
                        .id("whalenest-delete-footer")
                        .gap_3()
                        .justify_end()
                        .child(
                            Button::new("whalenest-delete-cancel")
                                .outline()
                                .label("取消")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child({
                            let shell = shell.clone();
                            let name = name.clone();
                            Button::new("whalenest-delete-confirm")
                                .primary()
                                .label("删除")
                                .on_click(move |_, window, cx| {
                                    let _ = shell.update(cx, |this, inner_cx| {
                                        app::delete_profile(&this.managed, name.clone());
                                        this.refresh(inner_cx);
                                        window.close_dialog(inner_cx);
                                    });
                                })
                        }),
                )
        });
    }

    /// 打开「创建 profile」对话框（名称输入）。
    pub(crate) fn open_create_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("profile 名称（如 work、lab）"));
        self.create_input = Some(input.clone());
        let shell = cx.entity().clone();
        window.open_dialog(cx, move |dialog, _window, cx| {
            dialog
                .title("创建 profile")
                .w(px(420.))
                .child(
                    v_flex()
                        .id("whalenest-create-body")
                        .gap_2()
                        .child(
                            Label::new("输入新 profile 名称：不含路径分隔符，不能是保留名 `web`。")
                                .text_sm()
                                .text_color(cx.theme().muted_foreground),
                        )
                        .child(Input::new(&input).w_full()),
                )
                .footer(
                    h_flex()
                        .id("whalenest-create-footer")
                        .gap_3()
                        .justify_end()
                        .child(
                            Button::new("whalenest-create-cancel")
                                .outline()
                                .label("取消")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child({
                            let shell = shell.clone();
                            let input = input.clone();
                            Button::new("whalenest-create-confirm")
                                .primary()
                                .label("创建")
                                .on_click(move |_, window, cx| {
                                    let name = input.read(cx).value().to_string();
                                    let _ = shell.update(cx, |this, inner_cx| {
                                        match app::create_profile(&this.managed, name) {
                                            Ok(()) => {
                                                this.refresh(inner_cx);
                                                window.close_dialog(inner_cx);
                                            }
                                            Err(e) => window.push_notification(
                                                (NotificationType::Error, e),
                                                inner_cx,
                                            ),
                                        }
                                    });
                                })
                        }),
                )
        });
    }

    // ── 渲染 ────────────────────────────────────────────────────────────────

    fn render_titlebar(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status_label = self.titlebar_status();
        let status_color = self.titlebar_status_color(cx);

        TitleBar::new()
            .child(
                h_flex()
                    .id("whalenest-titlebar-brand")
                    .gap_2()
                    .items_center()
                    .child(
                        h_flex()
                            .id("whalenest-titlebar-logo")
                            .size_2()
                            .rounded(px(5.))
                            .bg(cx.theme().primary)
                            .items_center()
                            .justify_center()
                            .child(
                                Icon::new(IconName::Bot)
                                    .text_color(cx.theme().primary_foreground)
                                    .xsmall(),
                            ),
                    )
                    .child(Label::new("WhaleNest").text_sm().font_semibold())
                    .child(
                        h_flex()
                            .id("whalenest-titlebar-status")
                            .gap_1()
                            .items_center()
                            .child(
                                div()
                                    .id("whalenest-status-dot")
                                    .size_2()
                                    .rounded_full()
                                    .bg(status_color),
                            )
                            .child(
                                Label::new(status_label)
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .id("whalenest-titlebar-actions")
                    .gap_1()
                    .items_center()
                    .child(
                        Button::new("whalenest-settings")
                            .ghost()
                            .icon(IconName::Settings2)
                            .xsmall()
                            .tooltip("设置")
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.open_settings(window, cx);
                            })),
                    ),
            )
    }

    fn titlebar_status(&self) -> &'static str {
        match &self.state {
            ShellState::Ready { .. } => "已就绪",
            ShellState::Loading => "启动中",
            ShellState::Error(_) => "出错",
            ShellState::Guide => "待安装 dsh",
            ShellState::Stopped => "已停止",
        }
    }

    fn titlebar_status_color(&self, cx: &App) -> Hsla {
        match &self.state {
            ShellState::Ready { .. } => cx.theme().success,
            ShellState::Loading => cx.theme().warning,
            ShellState::Error(_) => cx.theme().danger,
            ShellState::Guide => cx.theme().muted_foreground,
            ShellState::Stopped => cx.theme().muted_foreground,
        }
    }

    fn render_main(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content: gpui_kit::AnyElement = if self.show_wizard {
            self.render_wizard(window, cx).into_any_element()
        } else if !self.managed.kernel.lock().dsh_available() {
            self.render_guide(window, cx).into_any_element()
        } else {
            self.render_dashboard(window, cx).into_any_element()
        };

        v_flex()
            .id("whalenest-shell-main")
            .size_full()
            .child(self.render_update_banner(window, cx))
            .child(
                div()
                    .id("whalenest-shell-content")
                    .flex_1()
                    .size_full()
                    .child(content),
            )
    }

    /// 仪表盘主视图：双栏工作台布局（左侧 Profiles 列表 + 右侧工作区与终端日志）。
    fn render_dashboard(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let web_profiles: Vec<ProfileInfo> =
            self.profiles.iter().filter(|p| p.is_web()).cloned().collect();

        if web_profiles.is_empty() {
            return self.render_empty_dashboard(window, cx).into_any_element();
        }

        let selected_name = self.selected_profile.clone().unwrap_or_else(|| {
            web_profiles.first().map(|p| p.name.clone()).unwrap_or_else(|| "web".to_string())
        });
        let active_profile = web_profiles
            .iter()
            .find(|p| p.name == selected_name)
            .cloned()
            .unwrap_or_else(|| web_profiles[0].clone());

        h_flex()
            .id("whalenest-dashboard-workbench")
            .size_full()
            // ── 左侧：Profile 侧边栏 ──
            .child(
                v_flex()
                    .id("whalenest-dash-sidebar")
                    .w(px(260.))
                    .h_full()
                    .border_r_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().muted.opacity(0.25))
                    .p_3()
                    .gap_3()
                    // 侧边栏头部
                    .child(
                        h_flex()
                            .id("whalenest-dash-sidebar-header")
                            .w_full()
                            .items_center()
                            .justify_between()
                            .child(
                                Label::new("PROFILES")
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(cx.theme().muted_foreground),
                            )
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(
                                        Button::new("whalenest-dash-sidebar-refresh")
                                            .ghost()
                                            .icon(IconName::RotateCw)
                                            .xsmall()
                                            .tooltip("刷新")
                                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                                this.refresh(cx);
                                            })),
                                    )
                                    .child(
                                        Button::new("whalenest-dash-sidebar-create")
                                            .primary()
                                            .icon(IconName::Plus)
                                            .xsmall()
                                            .label("新建")
                                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                                this.open_create_dialog(window, cx);
                                            })),
                                    ),
                            ),
                    )
                    // Profile 垂直列表
                    .child(
                        v_flex()
                            .id("whalenest-dash-sidebar-list")
                            .flex_1()
                            .w_full()
                            .overflow_y_scroll()
                            .gap_1p5()
                            .children(web_profiles.iter().map(|p| {
                                self.render_sidebar_profile_item(p, &selected_name, cx)
                            })),
                    ),
            )
            // ── 右侧：主详情与实时终端工作区 ──
            .child(
                v_flex()
                    .id("whalenest-dash-detail")
                    .flex_1()
                    .h_full()
                    .p_5()
                    .gap_4()
                    .child(self.render_detail_hero(&active_profile, cx))
                    .child(self.render_detail_stats(&active_profile, cx))
                    .child(self.render_detail_tabs(&active_profile, window, cx))
            )
            .into_any_element()
    }

    /// 详情区 Tab 容器：实时日志 + 插件管理（默认日志）。
    fn render_detail_tabs(&mut self, profile: &ProfileInfo, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("whalenest-detail-tabs")
            .w_full()
            .flex_1()
            .gap_2()
            .child(
                TabBar::new("whalenest-detail-tabbar")
                    .selected_index(self.active_tab)
                    .on_click(cx.listener(|this, idx: &usize, _, cx| {
                        this.active_tab = *idx;
                        cx.notify();
                    }))
                    .child(Tab::new().label("实时日志"))
                    .child(Tab::new().label("插件管理")),
            )
            .child(
                match self.active_tab {
                    0 => self.render_detail_console(profile, window, cx).into_any_element(),
                    _ => self.render_detail_plugins(profile, window, cx).into_any_element(),
                },
            )
    }

    /// 详情区：已加载插件列表面板
    fn render_detail_plugins(&mut self, profile: &ProfileInfo, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entries = app::plugin_entries(profile);
        let enabled_count = entries.iter().filter(|e| e.enabled).count();
        let profile_name = profile.name.clone();

        // 首次进入（或切换 profile 后）异步查询可升级插件，驱动「可升级」徽标。
        if self.outdated_profile.as_deref() != Some(profile.name.as_str()) {
            self.outdated_profile = Some(profile.name.clone());
            crate::plugin_op::query_outdated(&self.managed, profile.name.clone());
        }

        // 安装输入框（复用实体，避免每次重渲染重建焦点状态）。
        if self.install_input.is_none() {
            self.install_input = Some(cx.new(|cx| InputState::new(_window, cx).placeholder("包名，如 dsh-skin-material-you")));
        }
        let install_input = self.install_input.clone().unwrap();
        let input_for_click = install_input.clone();

        v_flex()
            .id("whalenest-detail-plugins")
            .w_full()
            .p_4()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
            .gap_2p5()
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(Icon::new(IconName::Bot).small().text_color(cx.theme().primary))
                            .child(
                                Label::new(format!("插件管理 · {enabled_count} 个已启用"))
                                    .text_sm()
                                    .font_semibold(),
                            ),
                    )
                    .child(
                        Label::new("dsh.profile.bundles")
                            .text_xs()
                            .text_color(cx.theme().muted_foreground),
                    ),
            )
            // 「安装插件」输入行
            .child(
                h_flex()
                    .id("whalenest-plugin-install-row")
                    .w_full()
                    .gap_2()
                    .items_center()
                    .child(Input::new(&install_input).flex_1())
                    .child({
                        let profile_name_for_add = profile_name.clone();
                        Button::new("whalenest-plugin-install")
                            .primary()
                            .icon(IconName::Plus)
                            .label("安装")
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                let pkg = input_for_click.read(cx).value().trim().to_string();
                                if pkg.is_empty() {
                                    window.push_notification(
                                        (NotificationType::Error, "请输入要安装的插件包名"),
                                        cx,
                                    );
                                    return;
                                }
                                crate::plugin_op::add(&this.managed, profile_name_for_add.clone(), pkg);
                            }))
                    }),
            )
            .child(
                v_flex()
                    .w_full()
                    .gap_1()
                    .children(entries.iter().map(|e| {
                        let plugin = e.name.clone();
                        let version = e.version.clone();
                        let enabled = e.enabled;
                        let latest = self.outdated.get(&plugin).cloned();
                        let prof_update = profile_name.clone();
                        let prof_remove = profile_name.clone();
                        let prof_switch = profile_name.clone();
                        let plugin_update = plugin.clone();
                        let plugin_remove = plugin.clone();
                        h_flex()
                            .id(format!("whalenest-plugin-row-{}", plugin))
                            .w_full()
                            .p_2()
                            .rounded_md()
                            .border_1()
                            .border_color(if enabled { cx.theme().primary.opacity(0.3) } else { cx.theme().border })
                            .bg(if enabled { cx.theme().primary.opacity(0.04) } else { cx.theme().background })
                            .items_center()
                            .justify_between()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        h_flex()
                                            .size_2()
                                            .rounded_full()
                                            .bg(if enabled { cx.theme().success } else { cx.theme().muted_foreground }),
                                    )
                                    .child(
                                        v_flex()
                                            .gap_0p5()
                                            .child(
                                                Label::new(plugin.clone())
                                                    .text_sm()
                                                    .font_family("monospace")
                                                    .font_medium()
                                                    .text_color(cx.theme().foreground),
                                            )
                                            .child(
                                                h_flex()
                                                    .gap_1p5()
                                                    .items_center()
                                                    .child(
                                                        Label::new(format!("版本 {version}"))
                                                            .text_xs()
                                                            .text_color(cx.theme().muted_foreground),
                                                    )
                                                    .when(latest.is_some(), |this| {
                                                        this.child(
                                                            h_flex()
                                                                .gap_1()
                                                                .px_1p5()
                                                                .py_0p5()
                                                                .rounded_full()
                                                                .bg(cx.theme().warning.opacity(0.14))
                                                                .child(
                                                                    Label::new(format!("可升级 ↑ {latest}", latest = latest.clone().unwrap_or_default()))
                                                                        .text_xs()
                                                                        .text_color(cx.theme().warning),
                                                                ),
                                                        )
                                                    }),
                                    ),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        Button::new(format!("whalenest-plugin-update-{}", plugin))
                                            .xsmall()
                                            .when(latest.is_some(), |b| b.primary())
                                            .when(latest.is_none(), |b| b.outline())
                                            .label("升级")
                                            .on_click(cx.listener(move |this, _: &ClickEvent, _, _| {
                                                crate::plugin_op::update(&this.managed, prof_update.clone(), plugin_update.clone());
                                            })),
                                    )
                                    .child(
                                        Button::new(format!("whalenest-plugin-remove-{}", plugin))
                                            .xsmall()
                                            .ghost()
                                            .label("卸载")
                                            .on_click(cx.listener(move |this, _: &ClickEvent, _, _| {
                                                crate::plugin_op::remove(&this.managed, prof_remove.clone(), plugin_remove.clone());
                                            })),
                                    )
                                    .child(
                                        Switch::new(format!("whalenest-plugin-sw-{}", plugin))
                                            .checked(enabled)
                                            .on_click(cx.listener(move |this, checked: &bool, window, cx| {
                                                match app::set_plugin_enabled(&this.managed, &prof_switch, &plugin, *checked) {
                                                    Ok(()) => {
                                                        window.push_notification(
                                                            (NotificationType::Success, format!("「{plugin}」已{}", if *checked { "启用" } else { "禁用" })),
                                                            cx,
                                                        );
                                                    }
                                                    Err(e) => {
                                                        window.push_notification((NotificationType::Error, e), cx);
                                                    }
                                                }
                                                this.refresh(cx);
                                            })),
                                    ),
                            )
                    })),
            )
    }

    /// 侧边栏单个 Profile 条目
    fn render_sidebar_profile_item(
        &self,
        profile: &ProfileInfo,
        selected_name: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_selected = profile.name == selected_name;
        let status = self.card_status(&profile.name);
        let (dot_color, status_label) = match status {
            CardStatus::Running => (cx.theme().success, "运行中"),
            CardStatus::Starting => (cx.theme().warning, "启动中"),
            CardStatus::Stopped => (cx.theme().muted_foreground, "已停止"),
            CardStatus::Error => (cx.theme().danger, "出错"),
        };
        let port = self.card_port(&profile.name);
        let name_select = profile.name.clone();

        h_flex()
            .id(format!("whalenest-sidebar-item-{}", profile.name))
            .w_full()
            .p_2p5()
            .rounded_md()
            .border_1()
            .border_color(if is_selected { cx.theme().primary.opacity(0.5) } else { Hsla::transparent_black() })
            .bg(if is_selected { cx.theme().popover } else { Hsla::transparent_black() })
            .items_center()
            .justify_between()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.selected_profile = Some(name_select.clone());
                cx.notify();
            }))
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        Label::new(profile.name.clone())
                            .text_sm()
                            .font_medium()
                            .text_color(if is_selected { cx.theme().foreground } else { cx.theme().muted_foreground }),
                    )
                    .child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .child(
                                div()
                                    .size_1p5()
                                    .rounded_full()
                                    .bg(dot_color),
                            )
                            .child(
                                Label::new(status_label)
                                    .text_xs()
                                    .text_color(dot_color),
                            )
                            .child(
                                Label::new(format!("· {} 插件", profile.plugin_count))
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground),
                            ),
                    ),
            )
            .when(port.is_some(), |this| {
                this.child(
                    h_flex()
                        .px_1p5()
                        .py_0p5()
                        .rounded_sm()
                        .bg(cx.theme().primary.opacity(0.12))
                        .child(
                            Label::new(format!("{}", port.unwrap()))
                                .text_xs()
                                .font_family("monospace")
                                .text_color(cx.theme().primary),
                        ),
                )
            })
    }

    /// 详情区 Hero 标题与主操作栏
    fn render_detail_hero(&self, profile: &ProfileInfo, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self.card_status(&profile.name);
        let (dot_color, status_label) = match status {
            CardStatus::Running => (cx.theme().success, "运行中"),
            CardStatus::Starting => (cx.theme().warning, "启动中"),
            CardStatus::Stopped => (cx.theme().muted_foreground, "已停止"),
            CardStatus::Error => (cx.theme().danger, "出错"),
        };
        let port = self.card_port(&profile.name);

        let name_start = profile.name.clone();
        let name_open = profile.name.clone();
        let name_cwd = profile.name.clone();
        let name_del = profile.name.clone();

        h_flex()
            .id("whalenest-detail-hero")
            .w_full()
            .items_center()
            .justify_between()
            .p_4()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
            .child(
                v_flex()
                    .gap_1p5()
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(
                                Label::new(profile.name.clone())
                                    .text_2xl()
                                    .font_bold(),
                            )
                            .child(
                                h_flex()
                                    .gap_1p5()
                                    .px_2p5()
                                    .py_1()
                                    .rounded_full()
                                    .bg(dot_color.opacity(0.14))
                                    .child(
                                        div()
                                            .size_2()
                                            .rounded_full()
                                            .bg(dot_color),
                                    )
                                    .child(
                                        Label::new(status_label)
                                            .text_xs()
                                            .font_medium()
                                            .text_color(dot_color),
                                    ),
                            )
                            .when(port.is_some(), |this| {
                                this.child(
                                    h_flex()
                                        .px_2()
                                        .py_0p5()
                                        .rounded_md()
                                        .bg(cx.theme().primary.opacity(0.12))
                                        .child(
                                            Label::new(format!("端口 {}", port.unwrap()))
                                                .text_xs()
                                                .font_family("monospace")
                                                .font_semibold()
                                                .text_color(cx.theme().primary),
                                        ),
                                )
                            }),
                    ),
            )
            .child(
                h_flex()
                    .id("whalenest-hero-actions")
                    .gap_2p5()
                    .items_center()
                    // 核心高频主按钮：在浏览器打开 Web UI (Primary)
                    .child(
                        Button::new(format!("whalenest-hero-open-{}", profile.name))
                            .primary()
                            .icon(IconName::ExternalLink)
                            .label("在浏览器打开 Web UI")
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, _| {
                                this.open_browser_for(name_open.clone());
                            })),
                    )
                    // 重启 / 启动
                    .child(
                        Button::new(format!("whalenest-hero-start-{}", profile.name))
                            .outline()
                            .icon(IconName::RotateCw)
                            .label(if status == CardStatus::Running { "重启内核" } else { "启动 / 切换" })
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, _| {
                                this.start_profile(name_start.clone());
                            })),
                    )
                    // 更改目录
                    .child(
                        Button::new(format!("whalenest-hero-cwd-{}", profile.name))
                            .ghost()
                            .icon(IconName::Folder)
                            .label("更改目录")
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, _| {
                                this.change_cwd_for(name_cwd.clone());
                            })),
                    )
                    // 删除 profile
                    .child(
                        Button::new(format!("whalenest-hero-del-{}", profile.name))
                            .ghost()
                            .icon(IconName::Delete)
                            .label("删除")
                            .disabled(profile.name == crate::state::DEFAULT_PROFILE)
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                this.confirm_delete_profile(name_del.clone(), window, cx);
                            })),
                    ),
            )
    }

    /// 详情区 运行指标卡片 (Stats / Info)
    fn render_detail_stats(&self, profile: &ProfileInfo, cx: &App) -> impl IntoElement {
        let cwd_text = profile.cwd.to_string_lossy().into_owned();
        let session_text = session_text(profile.session_count, profile.last_session_time);

        h_flex()
            .id("whalenest-detail-stats")
            .w_full()
            .gap_3()
            .child(
                h_flex()
                    .flex_1()
                    .p_3()
                    .rounded_md()
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().popover)
                    .gap_2p5()
                    .items_center()
                    .child(Icon::new(IconName::Folder).small().text_color(cx.theme().muted_foreground))
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(Label::new("工作目录").text_xs().text_color(cx.theme().muted_foreground))
                            .child(Label::new(shorten_path(&cwd_text, 40)).text_sm().font_family("monospace")),
                    ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .p_3()
                    .rounded_md()
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().popover)
                    .gap_2p5()
                    .items_center()
                    .child(Icon::new(IconName::Bot).small().text_color(cx.theme().muted_foreground))
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(Label::new("插件配置").text_xs().text_color(cx.theme().muted_foreground))
                            .child(Label::new(format!("{} 个插件已启用", profile.plugin_count)).text_sm()),
                    ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .p_3()
                    .rounded_md()
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().popover)
                    .gap_2p5()
                    .items_center()
                    .child(Icon::new(IconName::RotateCw).small().text_color(cx.theme().muted_foreground))
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(Label::new("会话活跃").text_xs().text_color(cx.theme().muted_foreground))
                            .child(Label::new(session_text).text_sm()),
                    ),
            )
    }

    /// 详情区 现代化终端日志面板 (撑满垂直剩余空间)
    fn render_detail_console(
        &self,
        profile: &ProfileInfo,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // 终端式日志：旧在上、新在下，全部展示以允许上滚查看历史。
        let lines = self.log_tail.lines().collect::<Vec<_>>();
        let profile_name = profile.name.clone();

        v_flex()
            .id("whalenest-detail-console")
            .w_full()
            .flex_1()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
            .child(
                // 终端标题栏
                h_flex()
                    .id("whalenest-console-header")
                    .w_full()
                    .px_4()
                    .py_2p5()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(Icon::new(IconName::File).small().text_color(cx.theme().muted_foreground))
                            .child(
                                Label::new(format!("实时终端输出 · {}", profile_name))
                                    .text_sm()
                                    .font_semibold(),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                Button::new("whalenest-console-open-log")
                                    .xsmall()
                                    .outline()
                                    .icon(IconName::File)
                                    .label("打开完整日志文件")
                                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                        this.open_log(window, cx);
                                    })),
                            ),
                    ),
            )
            .child(
                // 日志输出滚动区域（包裹层 relative，滚动条只覆盖日志区，不含标题栏）
                div()
                    .id("whalenest-console-body-wrap")
                    .relative()
                    .flex_1()
                    .w_full()
                    .min_h_0()
                    .child(
                        v_flex()
                            .id("whalenest-console-body")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.log_scroll)
                            .p_3()
                            .gap_1()
                            .children(lines.into_iter().map(|l| {
                        let is_err = l.contains("err") || l.contains("Error") || l.contains("ERR");
                        let is_url = l.contains("http://") || l.contains("https://");
                        let color = if is_err {
                            cx.theme().danger
                        } else if is_url {
                            cx.theme().primary
                        } else {
                            cx.theme().muted_foreground
                        };

                        h_flex()
                            .w_full()
                            .child(
                                Label::new(format!("  {l}"))
                                    .text_xs()
                                    .font_family("monospace")
                                    .text_color(color),
                            )
                    })),
                        )
                        .child(Scrollbar::vertical(&self.log_scroll)),
            )
    }

    fn render_empty_dashboard(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        v_flex()
            .id("whalenest-dash-empty")
            .flex_1()
            .w_full()
            .items_center()
            .justify_center()
            .gap_3()
            .child(
                Icon::new(IconName::Bot)
                    .large()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(
                Label::new("还没有可用的 web 型 profile")
                    .text_sm()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(
                Button::new("whalenest-dash-empty-create")
                    .primary()
                    .label("创建第一个 profile")
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.open_create_dialog(window, cx);
                    })),
            )
    }

    /// 卡片状态灯：内核当前 profile + KernelState 推导。
    fn card_status(&self, profile_name: &str) -> CardStatus {
        let kernel = self.managed.kernel.lock();
        if kernel.config.profile != profile_name {
            return CardStatus::Stopped;
        }
        match &kernel.state {
            KernelState::Ready { .. } => CardStatus::Running,
            KernelState::Starting => CardStatus::Starting,
            KernelState::Crashed { .. } => CardStatus::Error,
            KernelState::Stopped => CardStatus::Stopped,
        }
    }

    /// 运行中 profile 的端口 badge。
    fn card_port(&self, profile_name: &str) -> Option<u16> {
        let kernel = self.managed.kernel.lock();
        if kernel.config.profile == profile_name {
            kernel.current_port()
        } else {
            None
        }
    }

    fn render_guide(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("whalenest-guide")
            .size_full()
            .px_8()
            .items_center()
            .justify_center()
            .child(
                v_flex()
                    .id("whalenest-guide-card")
                    .w(px(520.))
                    .gap_4()
                    .child(Label::new("未检测到 dsh").text_2xl().font_semibold())
                    .child(
                        Label::new(
                            "WhaleNest 需要 DeepSeek Harness (dsh) 内核。请先安装，然后点击重新检测。",
                        )
                        .text_sm()
                        .text_color(cx.theme().muted_foreground),
                    )
                    .child(command_box(app::INSTALL_COMMAND, cx))
                    .child(
                        h_flex()
                            .id("whalenest-guide-actions")
                            .gap_3()
                            .items_center()
                            .child(
                                Button::new("whalenest-guide-copy")
                                    .ghost()
                                    .icon(IconName::Copy)
                                    .label("复制命令")
                                    .on_click({
                                        move |event, window, cx| {
                                            let _ = event;
                                            copy_text(app::INSTALL_COMMAND, window, cx);
                                        }
                                    }),
                            )
                            .child(
                                Button::new("whalenest-guide-install")
                                    .primary()
                                    .label(if self.wizard_running { "安装中…" } else { "一键安装" })
                                    .disabled(self.wizard_running)
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, _| {
                                        this.install_dsh();
                                    })),
                            )
                            .child(
                                Button::new("whalenest-guide-redetect")
                                    .outline()
                                    .icon(IconName::Redo2)
                                    .label("重新检测")
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, _| {
                                        this.restart_kernel();
                                    })),
                            ),
                    ),
            )
    }

    fn render_update_banner(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(update) = &self.update else {
            return v_flex().id("whalenest-update-none").h(px(0.));
        };
        if !update.has_update {
            return v_flex().id("whalenest-update-none").h(px(0.));
        }
        v_flex()
            .id("whalenest-update-banner")
            .w_full()
            .px_4()
            .py_2()
            .bg(cx.theme().info.opacity(0.12))
            .border_b_1()
            .border_color(cx.theme().border)
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                h_flex()
                    .id("whalenest-update-text")
                    .gap_2()
                    .items_center()
                    .child(Icon::new(IconName::Info).small().text_color(cx.theme().info))
                    .child(
                        Label::new(format!(
                            "发现新版本 dsh v{}（当前 v{}）",
                            update.latest, update.current
                        ))
                        .text_sm(),
                    ),
            )
            .child(
                Button::new("whalenest-update-install")
                    .small()
                    .primary()
                    .label(if self.updating { "更新中…" } else { "一键更新" })
                    .disabled(self.updating)
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.install_update(window, cx);
                    })),
            )
    }

    /// 打开设置对话框。
    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let shell = cx.entity().clone();
        window.open_dialog(cx, move |dialog, window, cx| {
            let panel = crate::settings::settings_panel(&shell, window, cx);
            dialog
                .title("WhaleNest 设置")
                .w(px(600.))
                .child(panel)
                .footer(
                    h_flex()
                        .id("whalenest-settings-footer")
                        .gap_3()
                        .justify_end()
                        .child(
                            Button::new("whalenest-settings-close")
                                .outline()
                                .label("关闭")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        ),
                )
        });
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_client_decorated = matches!(window.window_decorations(), gpui_kit::Decorations::Client { .. });

        // 首次渲染触发器：向导首步自动开始环境检测
        if self.show_wizard && !self.env_check_triggered {
            self.env_check_triggered = true;
            self.recheck_env(window, cx);
        }

        v_flex()
            .id("whalenest-shell")
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .when(
                is_client_decorated || cfg!(any(target_os = "windows", target_os = "macos")),
                |this| this.child(self.render_titlebar(window, cx)),
            )
            .child(self.render_main(window, cx))
            .children(Root::render_dialog_layer(window, cx))
            .children(Root::render_sheet_layer(window, cx))
            .children(Root::render_notification_layer(window, cx))
    }
}

/// 内核状态快照（Shell::new 用）。
pub struct KernelSnapshot {
    pub state: ShellState,
}

/// 命令展示框（guide / wizard 共用）。
pub(crate) fn command_box(command: &str, cx: &App) -> impl IntoElement {
    h_flex()
        .id("whalenest-command-box")
        .w_full()
        .px_3()
        .py_2()
        .rounded_md()
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().popover)
        .items_center()
        .justify_between()
        .gap_2()
        .child(
            Label::new(command.to_string())
                .text_sm()
                .font_family("monospace")
                .text_color(cx.theme().foreground),
        )
}

/// 复制文本到剪贴板并弹提示（guide / wizard 共用）。
pub(crate) fn copy_text(text: &str, window: &mut Window, cx: &mut App) {
    let result = arboard::Clipboard::new().and_then(|mut c| c.set_text(text.to_string()));
    match result {
        Ok(()) => window.push_notification((NotificationType::Success, "已复制到剪贴板"), cx),
        Err(_) => window.push_notification((NotificationType::Error, "复制失败"), cx),
    }
}


/// 过长路径的省略显示。
fn shorten_path(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        s.to_string()
    } else {
        let head: String = chars[..max / 2].iter().collect();
        let tail: String = chars[chars.len() - max / 2..].iter().collect();
        format!("{head}…{tail}")
    }
}

/// 「N 个会话 · M 分钟前」显示文本。
fn session_text(count: usize, last: Option<std::time::SystemTime>) -> String {
    let ago = last
        .and_then(|t| t.elapsed().ok())
        .map(|d| {
            let s = d.as_secs();
            if s < 60 {
                format!("{s} 秒前")
            } else if s < 3600 {
                format!("{} 分钟前", s / 60)
            } else {
                format!("{} 小时前", s / 3600)
            }
        })
        .unwrap_or_else(|| "暂无".to_string());
    format!("{count} 个会话 · {ago}")
}
