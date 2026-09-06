//! 首次初始化向导（单步）：环境检测（node / npm / pnpm / dsh），dsh 缺失时
//! 给出安装指引。推荐插件步已移除（见 HANDOFF Phase 2）。
//!
//! 流程：环境检测在后台线程执行（cx.spawn + background_executor），结果写回
//! Shell 状态并 notify；检测通过后点「完成」进入仪表盘。

use gpui_kit::{App, ClickEvent, Context, Hsla, Window, div, prelude::*, px};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants}, h_flex, v_flex,
};

use crate::app::{self, EnvCheckResult};
use crate::shell::{Shell, command_box, copy_text};

/// 环境检测的 UI 状态。
pub enum EnvCheckUi {
    Checking,
    Result(EnvCheckResult),
    #[allow(dead_code)]
    Failed(String),
}

impl Shell {
    /// 渲染向导主区域（单步：环境检测）。
    pub(crate) fn render_wizard(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let env_items = wizard_env_items(&self.env_check, cx);
        let dsh_found = matches!(
            &self.env_check,
            Some(EnvCheckUi::Result(res)) if res.dsh.found
        );

        let dsh_guide = if !dsh_found {
            v_flex()
                .id("whalenest-wizard-dsh-guide")
                .gap_3()
                .child(
                    gpui_kit::component::label::Label::new("尚未检测到 dsh，请先安装：")
                        .text_sm()
                        .text_color(cx.theme().muted_foreground),
                )
                .child(command_box(app::INSTALL_COMMAND, cx))
                .child(
                    h_flex()
                        .id("whalenest-wizard-dsh-actions")
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new("whalenest-wizard-copy-cmd")
                                .small()
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
                            Button::new("whalenest-wizard-install-dsh")
                                .small()
                                .primary()
                                .label(if self.wizard_running { "安装中…" } else { "一键安装" })
                                .disabled(self.wizard_running)
                                .on_click(cx.listener(|this, _: &ClickEvent, _, _| {
                                    this.install_dsh();
                                })),
                        )
                        .child(
                            Button::new("whalenest-wizard-recheck")
                                .small()
                                .outline()
                                .icon(IconName::Redo2)
                                .label("重新检测")
                                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                    this.recheck_env(window, cx);
                                })),
                        ),
                )
                .into_any_element()
        } else {
            v_flex().id("whalenest-wizard-dsh-guide-none").into_any_element()
        };

        v_flex()
            .id("whalenest-wizard")
            .size_full()
            .items_center()
            .py_10()
            .child(
                v_flex()
                    .id("whalenest-wizard-inner")
                    .w(px(680.))
                    .gap_6()
                    .child(
                        v_flex()
                            .id("whalenest-wizard-header")
                            .gap_1()
                            .items_center()
                            .child(
                                gpui_kit::component::label::Label::new("欢迎使用 WhaleNest")
                                    .text_2xl()
                                    .font_semibold(),
                            )
                            .child(
                                gpui_kit::component::label::Label::new(
                                    "几步完成环境准备，即可以桌面应用的方式使用 DeepSeek Harness",
                                )
                                .text_sm()
                                .text_color(cx.theme().muted_foreground),
                            ),
                    )
                    .child(env_items)
                    .child(dsh_guide)
                    .child(
                        h_flex()
                            .id("whalenest-wizard-actions")
                            .justify_end()
                            .gap_3()
                            .items_center()
                            .child(
                                Button::new("whalenest-wizard-recheck-all")
                                    .outline()
                                    .icon(IconName::Redo2)
                                    .label("重新检测")
                                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                        this.recheck_env(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("whalenest-wizard-finish")
                                    .primary()
                                    .label(if self.wizard_running {
                                        "正在启动内核…"
                                    } else {
                                        "完成设置"
                                    })
                                    .disabled(self.wizard_running || !dsh_found)
                                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                        this.finish_wizard(window, cx);
                                    })),
                            ),
                    ),
            )
    }
}

fn wizard_env_items(env: &Option<EnvCheckUi>, cx: &App) -> impl IntoElement {
    match env {
        None | Some(EnvCheckUi::Checking) => h_flex()
            .id("whalenest-wizard-env-checking")
            .gap_2()
            .items_center()
            .child(
                gpui_kit::component::spinner::Spinner::new()
                    .small()
                    .color(cx.theme().primary),
            )
            .child(
                gpui_kit::component::label::Label::new("正在检测系统环境（Node.js、npm、pnpm、dsh）…")
                    .text_sm()
                    .text_color(cx.theme().muted_foreground),
            ),
        Some(EnvCheckUi::Failed(msg)) => h_flex()
            .id("whalenest-wizard-env-failed")
            .gap_2()
            .items_center()
            .child(
                Icon::new(IconName::CircleX)
                    .small()
                    .text_color(cx.theme().danger),
            )
            .child(
                gpui_kit::component::label::Label::new(format!("环境检测异常: {msg}"))
                    .text_sm()
                    .text_color(cx.theme().danger),
            ),
        Some(EnvCheckUi::Result(res)) => {
            let items = [
                ("Node.js 运行环境", &res.node, true),
                ("npm 包管理器", &res.npm, true),
                ("pnpm (推荐)", &res.pnpm, false),
                ("DeepSeek Harness (dsh 内核)", &res.dsh, true),
            ];
            v_flex()
                .id("whalenest-wizard-env-list")
                .gap_2()
                .children(items.iter().map(|(label, tool, required)| {
                    env_item_row(label, tool.found, tool.version.as_deref(), tool.path.as_deref(), *required, cx)
                }))
        }
    }
}

fn env_item_row(
    label: &str,
    found: bool,
    version: Option<&str>,
    path: Option<&str>,
    required: bool,
    cx: &App,
) -> impl IntoElement {
    let (badge_text, badge_color): (&str, Hsla) = if found {
        (
            version.map(|v| v.trim_start_matches('v')).unwrap_or("已就绪"),
            cx.theme().success,
        )
    } else if !required {
        ("未检测到", cx.theme().warning)
    } else {
        ("未安装", cx.theme().danger)
    };
    let desc = if found {
        path.unwrap_or("已在环境变量 PATH 中").to_string()
    } else if !required {
        "可选工具（安装后加速依赖解析）".to_string()
    } else {
        "未检测到可执行文件".to_string()
    };

    h_flex()
        .id(format!("whalenest-wizard-env-{label}"))
        .gap_3()
        .px_4()
        .py_2p5()
        .rounded_md()
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().popover)
        .items_center()
        .justify_between()
        .child(
            v_flex()
                .id(format!("whalenest-wizard-env-info-{label}"))
                .gap_0p5()
                .flex_1()
                .child(
                    gpui_kit::component::label::Label::new(label.to_string())
                        .text_sm()
                        .font_semibold(),
                )
                .child(
                    gpui_kit::component::label::Label::new(desc)
                        .text_xs()
                        .text_color(cx.theme().muted_foreground),
                ),
        )
        .child(
            h_flex()
                .id(format!("whalenest-wizard-env-badge-{label}"))
                .gap_1p5()
                .items_center()
                .child(
                    div()
                        .id(format!("whalenest-wizard-env-dot-{label}"))
                        .size_1p5()
                        .rounded_full()
                        .bg(badge_color),
                )
                .child(
                    gpui_kit::component::label::Label::new(badge_text.to_string())
                        .text_xs()
                        .text_color(badge_color),
                ),
        )
}
