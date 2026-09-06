//! 设置对话框面板（对应 Tauri 版托盘「更多」里的维护项 + 工作目录/自启/
//! 端口锁定等设置）。
//!
//! 由标题栏齿轮（Shell::open_settings）打开；面板闭包从 `Entity<Shell>`
//! 取回 Shell 实体注册回调（App 上下文没有 cx.entity()，只能走句柄）。

use gpui_kit::{App, ClickEvent, Entity, Window, prelude::*};
use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonVariants}, h_flex, label::Label, switch::Switch, v_flex,
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
        update: match update.as_ref() {
            Some(u) => (u.current.clone(), u.latest.clone(), u.has_update),
            None => Default::default(),
        },
    }
}

/// 构建设置面板内容（在 open_dialog 的 build 闭包内调用）。
pub(crate) fn settings_panel(
    shell: &Entity<Shell>,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    let snapshot = settings_snapshot_from_global(cx);
    let autostart = snapshot.autostart;
    let lock_port = snapshot.lock_port;
    let (current, latest, has_update) = snapshot.update;

    v_flex()
        .id("whalenest-settings")
        .w_full()
        .gap_4()
        .pt_2()
        // ── 开关行 ────────────────────────────────────────────────────────────
        .child(
            h_flex()
                .id("whalenest-settings-toggles")
                .gap_6()
                .child(
                    Switch::new("whalenest-settings-autostart")
                        .label("开机自启")
                        .checked(autostart)
                        .on_click(window.listener_for(
                            shell,
                            move |this, checked: &bool, window, cx| {
                                this.set_autostart(*checked, window, cx);
                            },
                        )),
                )
                .child(
                    Switch::new("whalenest-settings-lock-port")
                        .label("固定端口（3080 被占时报错而非漂移）")
                        .checked(lock_port)
                        .on_click(window.listener_for(
                            shell,
                            move |this, checked: &bool, window, cx| {
                                this.set_lock_port(*checked, window, cx);
                            },
                        )),
                ),
        )
        // ── 版本更新 ──────────────────────────────────────────────────────────
        .child(
            v_flex()
                .id("whalenest-settings-update")
                .gap_2()
                .child(
                    h_flex()
                        .id("whalenest-settings-update-row")
                        .items_center()
                        .justify_between()
                        .child(Label::new("dsh 版本更新").text_sm().font_semibold())
                        .child(
                            h_flex()
                                .id("whalenest-settings-update-actions")
                                .gap_2()
                                .items_center()
                                .child(
                                    Button::new("whalenest-settings-check-update")
                                        .small()
                                        .outline()
                                        .icon(IconName::Redo2)
                                        .label("检查更新")
                                        .on_click(window.listener_for(
                                            shell,
                                            |this, _: &ClickEvent, _, _| this.check_update(),
                                        )),
                                )
                                .when(has_update, |this| {
                                    this.child(
                                        Button::new("whalenest-settings-install-update")
                                            .small()
                                            .primary()
                                            .label(format!("更新到 v{latest}"))
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
                .when(!current.is_empty(), |this| {
                    this.child(
                        Label::new(format!(
                            "当前 v{current}{}",
                            if has_update {
                                format!(" → 最新 v{latest}")
                            } else {
                                String::new()
                            }
                        ))
                        .text_xs()
                        .text_color(cx.theme().muted_foreground),
                    )
                }),
        )
        // ── 诊断与维护 ────────────────────────────────────────────────────────
        .child(
            v_flex()
                .id("whalenest-settings-diag")
                .gap_2()
                .child(Label::new("诊断与维护").text_sm().font_semibold())
                .child(
                    h_flex().id("whalenest-settings-diag-actions").gap_2().flex_wrap().children(
                        [
                            ("复制诊断信息", IconName::Copy, Shell::copy_diagnostics as fn(&Shell, &mut Window, &mut App)),
                            ("打开日志", IconName::File, Shell::open_log as fn(&Shell, &mut Window, &mut App)),
                            ("打开配置文件", IconName::Settings2, Shell::open_config_file as fn(&Shell, &mut Window, &mut App)),
                        ]
                        .into_iter()
                        .map(|(label, icon, action)| {
                            Button::new(format!("whalenest-settings-{label}"))
                                .small()
                                .ghost()
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