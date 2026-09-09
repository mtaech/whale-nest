//! 设置对话框面板（对应 Tauri 版托盘「更多」里的维护项 + 工作目录/自启/
//! 端口锁定等设置）。
//!
//! 由标题栏齿轮（Shell::open_settings）打开；面板闭包从 `Entity<Shell>`
//! 取回 Shell 实体注册回调（App 上下文没有 cx.entity()，只能走句柄）。

use gpui_kit::{App, ClickEvent, Entity, Window, div, prelude::*, px};
use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputState},
    label::Label, switch::Switch, v_flex,
};

use crate::app::Managed;
use crate::shell::Shell;

/// 从 `Managed` 全局读一份设置快照（**不走 Shell 实体**，避免在对话框 build
/// 闭包内读取 Shell 造成的 entity 重入 panic）。
fn settings_snapshot_from_global(cx: &App) -> crate::shell::SettingsSnapshot {
    let managed = cx.global::<Managed>();
    let cfg = managed.config.lock();
    let update = managed.update.lock();
    crate::shell::SettingsSnapshot {
        autostart: cfg.autostart,
        lock_port: cfg.lock_port,
        font_family: cfg.font_family.clone(),
        update: match update.as_ref() {
            Some(u) => (u.current.clone(), u.latest.clone(), u.has_update),
            None => Default::default(),
        },
    }
}

/// 构建设置面板内容（在 open_dialog 的 build 闭包内调用）。
pub(crate) fn settings_panel(
    shell: &Entity<Shell>,
    font_input: &Entity<InputState>,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    let snapshot = settings_snapshot_from_global(cx);
    let autostart = snapshot.autostart;
    let lock_port = snapshot.lock_port;
    let (current, latest, has_update) = snapshot.update;

    let current_font = snapshot.font_family;
    let current_font_display = match &current_font {
        None => "系统默认".to_string(),
        Some(f) if f.is_empty() || f == ".SystemUIFont" => "系统默认".to_string(),
        Some(f) => f.clone(),
    };
    let preview_family = current_font.as_deref().unwrap_or(".SystemUIFont");

    let presets: [(&str, Option<&str>, &'static str); 5] = [
        ("系统默认", None, "whalenest-font-default"),
        ("思源黑体", Some("Noto Sans CJK SC"), "whalenest-font-noto"),
        ("微软雅黑", Some("Microsoft YaHei"), "whalenest-font-yahei"),
        ("苹方", Some("PingFang SC"), "whalenest-font-pingfang"),
        ("Maple Mono", Some("Maple Mono NF CN"), "whalenest-font-maple"),
    ];

    v_flex()
        .id("whalenest-settings")
        .w_full()
        .max_h(px(460.))
        .overflow_y_scroll()
        .gap_2p5()
        .pr_1p5()
        // ── 卡片 1：系统与守护 ──────────────────────────────────────────────
        .child(
            v_flex()
                .w_full()
                .p_3()
                .rounded_lg()
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().popover)
                .gap_2p5()
                .child(
                    Label::new("系统与常驻守护")
                        .text_xs()
                        .font_bold()
                        .text_color(cx.theme().muted_foreground),
                )
                .child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .justify_between()
                        .child(
                            v_flex()
                                .gap_0p5()
                                .child(Label::new("开机自启动").text_sm().font_medium())
                                .child(
                                    Label::new("登录系统时自动启动 WhaleNest 并恢复当前 Profile")
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground),
                                ),
                        )
                        .child(
                            Switch::new("whalenest-settings-autostart")
                                .checked(autostart)
                                .on_click(window.listener_for(
                                    shell,
                                    move |this, checked: &bool, window, cx| {
                                        this.set_autostart(*checked, window, cx);
                                    },
                                )),
                        ),
                )
                .child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .justify_between()
                        .child(
                            v_flex()
                                .gap_0p5()
                                .child(Label::new("固定端口模式").text_sm().font_medium())
                                .child(
                                    Label::new("固定使用 3080 端口；若被其他程序占用则直接告警而非自动漂移")
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground),
                                ),
                        )
                        .child(
                            Switch::new("whalenest-settings-lock-port")
                                .checked(lock_port)
                                .on_click(window.listener_for(
                                    shell,
                                    move |this, checked: &bool, window, cx| {
                                        this.set_lock_port(*checked, window, cx);
                                    },
                                )),
                        ),
                ),
        )
        // ── 卡片 2：界面字体与排版 ──────────────────────────────────────────
        .child(
            v_flex()
                .w_full()
                .p_3()
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
                            v_flex()
                                .gap_0p5()
                                .child(
                                    Label::new("界面字体")
                                        .text_xs()
                                        .font_bold()
                                        .text_color(cx.theme().muted_foreground),
                                )
                                .child(
                                    Label::new("选择或自定义全局显示字体（即时生效并保存）")
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground),
                                ),
                        )
                        .child(
                            h_flex()
                                .px_2()
                                .py_0p5()
                                .rounded_md()
                                .bg(cx.theme().primary.opacity(0.1))
                                .border_1()
                                .border_color(cx.theme().primary.opacity(0.2))
                                .child(
                                    Label::new(format!("当前: {current_font_display}"))
                                        .text_xs()
                                        .font_family("monospace")
                                        .font_semibold()
                                        .text_color(cx.theme().primary),
                                ),
                        ),
                )
                .child(
                    // 常用预设字体选择按钮组
                    h_flex()
                        .id("whalenest-font-presets")
                        .gap_2()
                        .flex_wrap()
                        .children(presets.into_iter().map(|(label, target_font, btn_id)| {
                            let is_active = match (&current_font, target_font) {
                                (None, None) => true,
                                (Some(curr), None) => curr.is_empty() || curr == ".SystemUIFont",
                                (Some(curr), Some(f)) => curr == f,
                                (None, Some(_)) => false,
                            };
                            let target_val = target_font.map(|s| s.to_string());

                            Button::new(btn_id)
                                .small()
                                .when(is_active, |this| this.primary().icon(IconName::Check))
                                .when(!is_active, |this| this.outline())
                                .label(label)
                                .on_click(window.listener_for(
                                    shell,
                                    move |this, _: &ClickEvent, window, cx| {
                                        this.set_font_family(target_val.clone(), window, cx);
                                    },
                                ))
                        })),
                )
                .child(
                    // 自定义字体输入框 + 应用按钮
                    h_flex()
                        .id("whalenest-font-custom-row")
                        .gap_2()
                        .items_center()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    Input::new(font_input)
                                        .small()
                                        .cleanable(true),
                                ),
                        )
                        .child(
                            Button::new("whalenest-font-apply-custom")
                                .small()
                                .outline()
                                .icon(IconName::Check)
                                .label("应用自定义")
                                .on_click(window.listener_for(
                                    shell,
                                    |this, _: &ClickEvent, window, cx| {
                                        this.apply_custom_font_input(window, cx);
                                    },
                                )),
                        ),
                )
                .child(
                    // 实时排版预览区域
                    v_flex()
                        .id("whalenest-font-preview-box")
                        .w_full()
                        .p_2()
                        .rounded_md()
                        .bg(cx.theme().muted.opacity(0.45))
                        .border_1()
                        .border_color(cx.theme().border)
                        .gap_1()
                        .child(
                            Label::new("字体实时预览效果")
                                .text_xs()
                                .font_medium()
                                .text_color(cx.theme().muted_foreground),
                        )
                        .child(
                            Label::new("WhaleNest 晨光灯塔 · 极速开发 · AaBbCc 123")
                                .text_xs()
                                .font_family(preview_family)
                                .text_color(cx.theme().foreground),
                        ),
                ),
        )
        // ── 卡片 3：dsh 内核版本 ────────────────────────────────────────────
        .child(
            v_flex()
                .w_full()
                .p_3()
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
                            Label::new("dsh 命令行版本")
                                .text_xs()
                                .font_bold()
                                .text_color(cx.theme().muted_foreground),
                        )
                        .child(
                            h_flex()
                                .id("whalenest-settings-update-actions")
                                .gap_2()
                                .items_center()
                                .child(
                                    Button::new("whalenest-settings-check-update")
                                        .xsmall()
                                        .outline()
                                        .icon(IconName::RotateCw)
                                        .label("检查更新")
                                        .on_click(window.listener_for(
                                            shell,
                                            |this, _: &ClickEvent, window, cx| {
                                                this.check_update(window, cx);
                                            },
                                        )),
                                )
                                .when(has_update, |this| {
                                    this.child(
                                        Button::new("whalenest-settings-install-update")
                                            .xsmall()
                                            .primary()
                                            .icon(IconName::RotateCw)
                                            .label(format!("升级至 v{latest}"))
                                            .on_click(window.listener_for(
                                                shell,
                                                move |this, _: &ClickEvent, window, cx| {
                                                    this.install_update(window, cx);
                                                    window.close_dialog(cx);
                                                },
                                            )),
                                    )
                                }),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            h_flex()
                                .px_2()
                                .py_0p5()
                                .rounded_md()
                                .bg(cx.theme().muted)
                                .child(
                                    Label::new(format!("当前版本: v{}", if current.is_empty() { "未知" } else { &current }))
                                        .text_xs()
                                        .font_family("monospace")
                                        .font_medium()
                                        .text_color(cx.theme().foreground),
                                ),
                        )
                        .when(has_update, |this| {
                            this.child(
                                h_flex()
                                    .px_2()
                                    .py_0p5()
                                    .rounded_md()
                                    .bg(cx.theme().warning.opacity(0.15))
                                    .child(
                                        Label::new(format!("发现新版 v{latest}"))
                                            .text_xs()
                                            .font_family("monospace")
                                            .font_semibold()
                                            .text_color(cx.theme().warning),
                                    ),
                            )
                        }),
                ),
        )
        // ── 卡片 4：诊断与维护 ──────────────────────────────────────────────
        .child(
            v_flex()
                .w_full()
                .p_3()
                .rounded_lg()
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().popover)
                .gap_2p5()
                .child(
                    Label::new("诊断与维护")
                        .text_xs()
                        .font_bold()
                        .text_color(cx.theme().muted_foreground),
                )
                .child(
                    h_flex()
                        .id("whalenest-settings-diag-actions")
                        .gap_2()
                        .flex_wrap()
                        .children(
                            [
                                ("复制诊断信息", IconName::Copy, "whalenest-settings-diag-copy", Shell::copy_diagnostics as fn(&Shell, &mut Window, &mut App)),
                                ("打开日志文件", IconName::File, "whalenest-settings-diag-log", Shell::open_log as fn(&Shell, &mut Window, &mut App)),
                                ("打开配置文件", IconName::Settings2, "whalenest-settings-diag-cfg", Shell::open_config_file as fn(&Shell, &mut Window, &mut App)),
                            ]
                            .into_iter()
                            .map(|(label, icon, btn_id, action)| {
                                Button::new(btn_id)
                                    .small()
                                    .outline()
                                    .icon(icon)
                                    .label(label)
                                    .on_click(window.listener_for(shell, move |this, _: &ClickEvent, window, cx| {
                                        action(this, window, cx);
                                    }))
                            }),
                        ),
                ),
        )
}