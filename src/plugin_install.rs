//! 插件安装浮窗面板：支持 npmjs 源（模糊搜索）与 GitHub 仓库源（精确匹配）。

use std::process::Command;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputEvent, InputState},
    label::Label,
    notification::NotificationType,
    v_flex,
};
use gpui_kit::base::Scrollbar;
use gpui_kit::{
    AnyElement, AsyncWindowContext, Context, Entity,
    Render, ScrollHandle, Subscription, WeakEntity, Window, div, px,
    prelude::*,
};

use crate::app::Managed;

/// 插件安装源类型
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallSourceTab {
    /// npmjs 官方源 / 镜像源（支持关键词模糊搜索）
    Npm,
    /// GitHub 仓库（精确匹配 owner/repo）
    GitHub,
}

/// npm 检索结果项
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NpmPkgResult {
    pub name: String,
    pub version: String,
    pub description: String,
    pub detail_url: String,
}

/// 解析后的 GitHub 仓库描述
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubSpec {
    pub owner: String,
    pub repo: String,
    pub git_ref: Option<String>,
}

impl GitHubSpec {
    /// 精确解析 GitHub 仓库地址。
    /// 支持：
    /// - `owner/repo`
    /// - `github:owner/repo`
    /// - `github:owner/repo#ref`
    /// - `https://github.com/owner/repo`
    /// - `https://github.com/owner/repo.git`
    /// - `https://github.com/owner/repo/tree/ref`
    /// - `git@github.com:owner/repo.git`
    pub fn parse(input: &str) -> Option<Self> {
        let s = input.trim();
        if s.is_empty() {
            return None;
        }

        let s = if let Some(stripped) = s.strip_prefix("git@github.com:") {
            stripped
        } else if let Some(stripped) = s.strip_prefix("https://github.com/") {
            stripped
        } else if let Some(stripped) = s.strip_prefix("http://github.com/") {
            stripped
        } else if let Some(stripped) = s.strip_prefix("github:") {
            stripped
        } else {
            s
        };

        let (path_part, hash_ref) = match s.split_once('#') {
            Some((p, r)) => (p, Some(r.trim().to_string())),
            None => (s, None),
        };

        let (clean_path, tree_ref) = if let Some((repo_part, branch_part)) = path_part.split_once("/tree/") {
            (repo_part, Some(branch_part.trim().to_string()))
        } else {
            (path_part, None)
        };

        let clean_path = clean_path.trim_end_matches(".git").trim_matches('/');
        let segments: Vec<&str> = clean_path.split('/').collect();
        if segments.len() != 2 {
            return None;
        }

        let owner = segments[0].trim();
        let repo = segments[1].trim();

        let is_valid_ident = |part: &str| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        };

        if !is_valid_ident(owner) || !is_valid_ident(repo) {
            return None;
        }

        let git_ref = hash_ref.or(tree_ref).filter(|r| !r.is_empty());

        Some(GitHubSpec {
            owner: owner.to_string(),
            repo: repo.to_string(),
            git_ref,
        })
    }

    /// 传给 `dsh plugin add` 的规范标识符（例如 `github:owner/repo` 或 `github:owner/repo#branch`）
    pub fn install_specifier(&self) -> String {
        match &self.git_ref {
            Some(r) => format!("github:{}/{}#{}", self.owner, self.repo, r),
            None => format!("github:{}/{}", self.owner, self.repo),
        }
    }

    /// 浏览器中查看 GitHub 仓库的 URL
    pub fn web_url(&self) -> String {
        match &self.git_ref {
            Some(r) => format!("https://github.com/{}/{}/tree/{}", self.owner, self.repo, r),
            None => format!("https://github.com/{}/{}", self.owner, self.repo),
        }
    }
}

/// 解析 npm 搜索或 npm view 返回的 JSON 字符串。
pub fn parse_npm_search_json(json_str: &str) -> Vec<NpmPkgResult> {
    let mut results = Vec::new();
    let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) else {
        return results;
    };

    if let Some(arr) = val.as_array() {
        for item in arr {
            let Some(name) = item.get("name").and_then(|v| v.as_str()) else {
                continue;
            };
            let version = item
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or("latest");
            let description = item
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let detail_url = format!("https://www.npmjs.com/package/{name}");
            results.push(NpmPkgResult {
                name: name.to_string(),
                version: version.to_string(),
                description: description.to_string(),
                detail_url,
            });
        }
    } else if let Some(objects) = val.get("objects").and_then(|v| v.as_array()) {
        for obj in objects {
            let Some(pkg) = obj.get("package") else { continue };
            let Some(name) = pkg.get("name").and_then(|v| v.as_str()) else { continue };
            let version = pkg
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or("latest");
            let description = pkg
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let detail_url = pkg
                .get("links")
                .and_then(|l| l.get("npm"))
                .and_then(|u| u.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("https://www.npmjs.com/package/{name}"));
            results.push(NpmPkgResult {
                name: name.to_string(),
                version: version.to_string(),
                description: description.to_string(),
                detail_url,
            });
        }
    } else if let Some(name) = val.get("name").and_then(|v| v.as_str()) {
        let version = val
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("latest");
        let description = val
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let detail_url = format!("https://www.npmjs.com/package/{name}");
        results.push(NpmPkgResult {
            name: name.to_string(),
            version: version.to_string(),
            description: description.to_string(),
            detail_url,
        });
    }

    results
}

/// 执行 npm 模糊搜索（或精确回退查询）。
pub fn search_npm_packages(query: &str) -> Result<Vec<NpmPkgResult>, String> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }

    // 1. 优先调用 `npm search <query> --json --searchlimit 15`（继承用户的 npm 配置与镜像源）
    if let Ok(output) = Command::new("npm")
        .args(["search", q, "--json", "--searchlimit", "15"])
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let parsed = parse_npm_search_json(&stdout);
            if !parsed.is_empty() {
                return Ok(parsed);
            }
        }
    }

    // 2. 若模糊搜索未命中，尝试精确查询 `npm view <query> name version description --json`
    if let Ok(output) = Command::new("npm")
        .args(["view", q, "name", "version", "description", "--json"])
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let parsed = parse_npm_search_json(&stdout);
            if !parsed.is_empty() {
                return Ok(parsed);
            }
        }
    }

    // 3. Fallback: 尝试使用 curl 请求 npmjs 搜索 API
    if let Ok(output) = Command::new("curl")
        .args([
            "-s",
            "--max-time",
            "5",
            &format!("https://registry.npmjs.org/-/v1/search?text={}&size=15", q.replace(' ', "+")),
        ])
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let parsed = parse_npm_search_json(&stdout);
            if !parsed.is_empty() {
                return Ok(parsed);
            }
        }
    }

    Ok(Vec::new())
}

/// 插件安装悬浮对话框主体组件
pub struct InstallPluginModal {
    pub managed: Managed,
    pub profile_name: String,
    pub tab: InstallSourceTab,
    pub npm_input: Entity<InputState>,
    pub github_input: Entity<InputState>,
    pub is_searching: bool,
    pub search_error: Option<String>,
    pub search_results: Vec<NpmPkgResult>,
    pub has_searched: bool,
    results_scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl InstallPluginModal {
    pub fn new(
        managed: Managed,
        profile_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let npm_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("输入 npm 包名关键词搜索，如 dsh-ast-edit-tool")
        });
        let github_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("输入 GitHub 仓库地址，例如 deepseek-ai/deepseek-harness")
        });

        // 仅在 GitHub 输入变化时刷新 UI 以展示实时匹配解析卡片
        let sub_gh = cx.observe(&github_input, |_, _, cx| {
            cx.notify();
        });

        // 监听 npm 输入的 Enter 键事件，直接触发搜索（输入按键时不全量重渲染列表，消除打字输入卡顿）
        let sub_npm = cx.subscribe_in(&npm_input, window, |this, _, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                this.trigger_search(window, cx);
            }
        });

        Self {
            managed,
            profile_name,
            tab: InstallSourceTab::Npm,
            npm_input,
            github_input,
            is_searching: false,
            search_error: None,
            search_results: Vec::new(),
            has_searched: false,
            results_scroll: ScrollHandle::new(),
            _subscriptions: vec![sub_npm, sub_gh],
        }
    }

    /// 执行 npm 异步搜索
    pub fn trigger_search(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.is_searching {
            return;
        }
        let q = self.npm_input.read(cx).value().trim().to_string();
        if q.is_empty() {
            return;
        }

        self.is_searching = true;
        self.search_error = None;
        cx.notify();

        cx.spawn_in(window, async move |this: WeakEntity<Self>, cx: &mut AsyncWindowContext| {
            let result = cx
                .background_executor()
                .spawn(async move { search_npm_packages(&q) })
                .await;

            let _ = this.update_in(&mut *cx, |modal, _window, cx| {
                modal.is_searching = false;
                modal.has_searched = true;
                match result {
                    Ok(pkgs) => {
                        modal.search_results = pkgs;
                        modal.search_error = None;
                    }
                    Err(e) => {
                        modal.search_results.clear();
                        modal.search_error = Some(e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for InstallPluginModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_npm = matches!(self.tab, InstallSourceTab::Npm);
        let is_gh = matches!(self.tab, InstallSourceTab::GitHub);

        v_flex()
            .id("whalenest-install-plugin-modal")
            .w_full()
            .gap_4()
            .pt_1()
            // ── 顶部选项卡：选择安装源 ──────────────────────────────────────────
            .child(
                h_flex()
                    .w_full()
                    .p_1()
                    .rounded_lg()
                    .bg(cx.theme().secondary.opacity(0.35))
                    .gap_1p5()
                    .child(
                        Button::new("whalenest-install-tab-npm")
                            .flex_1()
                            .when(is_npm, |b| b.primary())
                            .when(!is_npm, |b| b.ghost())
                            .label("📦 npmjs 官方源 (支持模糊搜索)")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.tab = InstallSourceTab::Npm;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("whalenest-install-tab-gh")
                            .flex_1()
                            .when(is_gh, |b| b.primary())
                            .when(!is_gh, |b| b.ghost())
                            .label("🐙 GitHub 仓库 (精确匹配)")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.tab = InstallSourceTab::GitHub;
                                cx.notify();
                            })),
                    ),
            )
            // ── 选项卡内容 ──────────────────────────────────────────────────────
            .when(is_npm, |this| this.child(self.render_npm_tab(window, cx)))
            .when(is_gh, |this| this.child(self.render_github_tab(window, cx)))
    }
}

impl InstallPluginModal {
    /// 渲染 npmjs 源面板（模糊搜索与结果列表）
    fn render_npm_tab(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current_input = self.npm_input.read(cx).value().trim().to_string();
        let is_searching = self.is_searching;
        let has_searched = self.has_searched;
        let results = self.search_results.clone();
        let results_len = results.len();
        let search_error = self.search_error.clone();
        let managed = self.managed.clone();
        let profile = self.profile_name.clone();

        let theme = cx.theme();
        let border_color = theme.border;
        let danger_color = theme.danger;
        let primary_color = theme.primary;
        let muted_foreground = theme.muted_foreground;
        let foreground_color = theme.foreground;
        let bg_color = theme.background;
        let secondary_color = theme.secondary;

        let results_view: AnyElement = if is_searching {
            // 搜索中态
            h_flex()
                .w_full()
                .py_8()
                .justify_center()
                .items_center()
                .gap_2p5()
                .child(Icon::new(IconName::RotateCw).small().text_color(primary_color))
                .child(
                    Label::new("正在从 npm 仓库检索匹配插件，请稍候…")
                        .text_sm()
                        .text_color(muted_foreground),
                )
                .into_any_element()
        } else if has_searched && results_len > 0 {
            // 搜索完成且有结果
            v_flex()
                .w_full()
                .gap_2()
                .child(
                    h_flex()
                        .w_full()
                        .justify_between()
                        .items_center()
                        .child(
                            Label::new(format!("共检索到 {results_len} 个相关 npm 包："))
                                .text_xs()
                                .font_bold()
                                .text_color(muted_foreground),
                        ),
                )
                .child(
                    div()
                        .id("whalenest-npm-results-scroll-wrap")
                        .relative()
                        .w_full()
                        .when(results_len >= 4, |d| d.h(px(330.)))
                        .when(results_len < 4, |d| d.max_h(px(330.)))
                        .child(
                            v_flex()
                                .id("whalenest-npm-results-scroll")
                                .size_full()
                                .overflow_y_scroll()
                                .track_scroll(&self.results_scroll)
                                .gap_2()
                                .pr_3()
                                .children(results.into_iter().map(move |pkg| {
                            let pkg_name = pkg.name.clone();
                            let pkg_version = pkg.version.clone();
                            let detail_url = pkg.detail_url.clone();
                            let managed_add = managed.clone();
                            let profile_add = profile.clone();

                            h_flex()
                                .w_full()
                                .p_3()
                                .rounded_lg()
                                .border_1()
                                .border_color(border_color)
                                .bg(bg_color)
                                .justify_between()
                                .items_center()
                                .gap_3()
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .gap_1()
                                        .child(
                                            h_flex()
                                                .gap_2()
                                                .items_center()
                                                .child(
                                                    Label::new(pkg_name.clone())
                                                        .text_sm()
                                                        .font_bold()
                                                        .text_color(foreground_color)
                                                        .truncate(),
                                                )
                                                .child(
                                                    h_flex()
                                                        .flex_shrink_0()
                                                        .px_1p5()
                                                        .py_0p5()
                                                        .rounded_sm()
                                                        .bg(secondary_color.opacity(0.5))
                                                        .child(
                                                            Label::new(format!("v{}", pkg_version))
                                                                .text_xs()
                                                                .font_family("monospace")
                                                                .text_color(muted_foreground),
                                                        ),
                                                ),
                                        )
                                        .when(!pkg.description.is_empty(), move |row| {
                                            row.child(
                                                Label::new(pkg.description)
                                                    .text_xs()
                                                    .text_color(muted_foreground)
                                                    .line_clamp(2),
                                            )
                                        }),
                                )
                                .child(
                                    h_flex()
                                        .flex_shrink_0()
                                        .gap_2()
                                        .items_center()
                                        .child({
                                            let url = detail_url.clone();
                                            Button::new(format!("npm-detail-{}", pkg_name))
                                                .xsmall()
                                                .outline()
                                                .icon(IconName::ExternalLink)
                                                .label("查看详情")
                                                .on_click(move |_, window, cx| {
                                                    if let Err(e) = crate::lifecycle::open_url(&url) {
                                                        window.push_notification(
                                                            (NotificationType::Error, e),
                                                            cx,
                                                        );
                                                    }
                                                })
                                        })
                                        .child({
                                            let pkg_for_add = pkg_name.clone();
                                            Button::new(format!("npm-install-{}", pkg_name))
                                                .xsmall()
                                                .primary()
                                                .icon(IconName::Plus)
                                                .label("安装")
                                                .on_click(move |_, window, cx| {
                                                    crate::plugin_op::add(
                                                        &managed_add,
                                                        profile_add.clone(),
                                                        pkg_for_add.clone(),
                                                    );
                                                    window.push_notification(
                                                        (
                                                            NotificationType::Info,
                                                            format!("已提交安装任务: {pkg_for_add}"),
                                                        ),
                                                        cx,
                                                    );
                                                    window.close_dialog(cx);
                                                })
                                        }),
                                )
                        })),
                        )
                        .child(Scrollbar::vertical(&self.results_scroll)),
                )
                .into_any_element()
        } else if has_searched && results_len == 0 {
            // 搜索完成但无结果
            let direct_pkg = current_input.clone();
            let managed_dir = managed.clone();
            let profile_dir = profile.clone();

            v_flex()
                .w_full()
                .p_4()
                .rounded_lg()
                .border_1()
                .border_color(border_color)
                .bg(secondary_color.opacity(0.15))
                .gap_3()
                .items_center()
                .child(
                    Label::new("未在 npm 公开检索中找到匹配的包。")
                        .text_sm()
                        .text_color(muted_foreground),
                )
                .when(!direct_pkg.is_empty(), move |box_view| {
                    let direct_click = direct_pkg.clone();
                    box_view.child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                Label::new("若该包确已发布（如特定私有包或新包），您仍可：")
                                    .text_xs()
                                    .text_color(muted_foreground),
                            )
                            .child(
                                Button::new("npm-direct-install")
                                    .small()
                                    .primary()
                                    .icon(IconName::Plus)
                                    .label(format!("直接安装「{direct_pkg}」"))
                                    .on_click(move |_, window, cx| {
                                        crate::plugin_op::add(
                                            &managed_dir,
                                            profile_dir.clone(),
                                            direct_click.clone(),
                                        );
                                        window.push_notification(
                                            (
                                                NotificationType::Info,
                                                format!("已提交安装任务: {direct_click}"),
                                            ),
                                            cx,
                                        );
                                        window.close_dialog(cx);
                                    }),
                            ),
                    )
                })
                .into_any_element()
        } else {
            // 尚未搜索时的初始引导卡片
            let direct_pkg = current_input.clone();
            let managed_init = managed.clone();
            let profile_init = profile.clone();

            v_flex()
                .w_full()
                .p_4()
                .rounded_lg()
                .border_1()
                .border_color(border_color)
                .bg(secondary_color.opacity(0.15))
                .gap_2()
                .child(
                    Label::new("💡 模糊搜索提示：")
                        .text_xs()
                        .font_bold()
                        .text_color(foreground_color),
                )
                .child(
                    Label::new("在上方输入插件关键词（如 dsh-ast-edit-tool）后点击「搜索」，即可模糊检索 npm 社区插件。")
                        .text_xs()
                        .text_color(muted_foreground),
                )
                .when(!direct_pkg.is_empty(), move |box_view| {
                    let direct_click = direct_pkg.clone();
                    box_view.child(
                        h_flex()
                            .pt_1()
                            .gap_2()
                            .items_center()
                            .child(
                                Label::new("或者直接安装当前输入包：")
                                    .text_xs()
                                    .text_color(muted_foreground),
                            )
                            .child(
                                Button::new("npm-initial-direct-install")
                                    .small()
                                    .primary()
                                    .icon(IconName::Plus)
                                    .label(format!("安装「{direct_pkg}」"))
                                    .on_click(move |_, window, cx| {
                                        crate::plugin_op::add(
                                            &managed_init,
                                            profile_init.clone(),
                                            direct_click.clone(),
                                        );
                                        window.push_notification(
                                            (
                                                NotificationType::Info,
                                                format!("已提交安装任务: {direct_click}"),
                                            ),
                                            cx,
                                        );
                                        window.close_dialog(cx);
                                    }),
                            ),
                    )
                })
                .into_any_element()
        };

        v_flex()
            .w_full()
            .gap_3()
            // 搜索输入栏
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .child(
                        h_flex()
                            .flex_1()
                            .child(Input::new(&self.npm_input).cleanable(true).w_full()),
                    )
                    .child(
                        Button::new("whalenest-npm-search-btn")
                            .primary()
                            .icon(IconName::Search)
                            .label(if is_searching { "搜索中…" } else { "搜索" })
                            .disabled(is_searching)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.trigger_search(window, cx);
                            })),
                    ),
            )
            // 错误提示
            .when_some(search_error, move |this, err| {
                this.child(
                    h_flex()
                        .w_full()
                        .p_2p5()
                        .rounded_lg()
                        .bg(danger_color.opacity(0.1))
                        .border_1()
                        .border_color(danger_color.opacity(0.25))
                        .gap_2()
                        .items_center()
                        .child(Icon::new(IconName::TriangleAlert).small().text_color(danger_color))
                        .child(Label::new(err).text_xs().text_color(danger_color)),
                )
            })
            // 动态呈现内容区
            .child(results_view)
    }

    /// 渲染 GitHub 仓库源面板（精确匹配、格式解析、实时详情与安装）
    fn render_github_tab(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let input_val = self.github_input.read(cx).value().trim().to_string();
        let parsed_spec = GitHubSpec::parse(&input_val);
        let managed = self.managed.clone();
        let profile = self.profile_name.clone();

        let theme = cx.theme();
        let border_color = theme.border;
        let primary_color = theme.primary;
        let muted_foreground = theme.muted_foreground;
        let foreground_color = theme.foreground;
        let secondary_color = theme.secondary;
        let success_color = theme.success;
        let warning_color = theme.warning;

        v_flex()
            .w_full()
            .gap_3()
            // 提示横幅：强调 GitHub 源仅限精确匹配
            .child(
                h_flex()
                    .w_full()
                    .p_2p5()
                    .rounded_lg()
                    .bg(primary_color.opacity(0.06))
                    .border_1()
                    .border_color(primary_color.opacity(0.18))
                    .gap_2()
                    .items_center()
                    .child(Icon::new(IconName::Info).small().text_color(primary_color))
                    .child(
                        Label::new("GitHub 仓库源仅支持精确匹配，支持 owner/repo、github:owner/repo#branch 或完整仓库 URL。")
                            .text_xs()
                            .text_color(foreground_color.opacity(0.9)),
                    ),
            )
            // GitHub 坐标输入栏
            .child(
                h_flex()
                    .w_full()
                    .child(Input::new(&self.github_input).cleanable(true).w_full()),
            )
            // 匹配结果预览卡片
            .child({
                if input_val.is_empty() {
                    // 输入为空：等待输入提示
                    v_flex()
                        .w_full()
                        .p_4()
                        .rounded_lg()
                        .border_1()
                        .border_color(border_color)
                        .bg(secondary_color.opacity(0.15))
                        .gap_1p5()
                        .child(
                            Label::new("等待输入 GitHub 仓库坐标…")
                                .text_sm()
                                .font_bold()
                                .text_color(muted_foreground),
                        )
                        .child(
                            Label::new("输入后将自动精确校验仓库坐标，并展示「查看详情」与「安装」按钮。")
                                .text_xs()
                                .text_color(muted_foreground),
                        )
                } else if let Some(spec) = parsed_spec {
                    // 精确匹配成功：展示仓库信息卡片 + 查看详情 + 安装按钮
                    let web_url = spec.web_url();
                    let specifier = spec.install_specifier();
                    let owner_repo = format!("{}/{}", spec.owner, spec.repo);
                    let git_ref_label = spec.git_ref.as_deref().unwrap_or("默认分支 (HEAD)");

                    v_flex()
                        .w_full()
                        .p_3p5()
                        .rounded_lg()
                        .border_1()
                        .border_color(success_color.opacity(0.35))
                        .bg(success_color.opacity(0.06))
                        .gap_3()
                        .child(
                            h_flex()
                                .w_full()
                                .justify_between()
                                .items_center()
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .items_center()
                                        .child(
                                            Icon::new(IconName::CircleCheck)
                                                .small()
                                                .text_color(success_color),
                                        )
                                        .child(
                                            Label::new(owner_repo)
                                                .text_base()
                                                .font_bold()
                                                .text_color(foreground_color),
                                        ),
                                )
                                .child(
                                    h_flex()
                                        .px_2()
                                        .py_0p5()
                                        .rounded_sm()
                                        .bg(success_color.opacity(0.15))
                                        .child(
                                            Label::new(format!("分支/Tag: {git_ref_label}"))
                                                .text_xs()
                                                .font_family("monospace")
                                                .text_color(success_color),
                                        ),
                                ),
                        )
                        .child(
                            h_flex()
                                .gap_1p5()
                                .items_center()
                                .child(
                                    Label::new("安装标识:")
                                        .text_xs()
                                        .text_color(muted_foreground),
                                )
                                .child(
                                    Label::new(specifier.clone())
                                        .text_xs()
                                        .font_family("monospace")
                                        .text_color(foreground_color),
                                ),
                        )
                        .child(
                            h_flex()
                                .w_full()
                                .justify_end()
                                .gap_2p5()
                                .child({
                                    let url = web_url.clone();
                                    Button::new("gh-preview-detail")
                                        .small()
                                        .outline()
                                        .icon(IconName::ExternalLink)
                                        .label("在 GitHub 查看详情 ↗")
                                        .on_click(move |_, window, cx| {
                                            if let Err(e) = crate::lifecycle::open_url(&url) {
                                                window.push_notification(
                                                    (NotificationType::Error, e),
                                                    cx,
                                                );
                                            }
                                        })
                                })
                                .child({
                                    let spec_for_add = specifier.clone();
                                    Button::new("gh-preview-install")
                                        .small()
                                        .primary()
                                        .icon(IconName::Plus)
                                        .label("安装此仓库插件")
                                        .on_click(move |_, window, cx| {
                                            crate::plugin_op::add(
                                                &managed,
                                                profile.clone(),
                                                spec_for_add.clone(),
                                            );
                                            window.push_notification(
                                                (
                                                    NotificationType::Info,
                                                    format!("已提交安装任务: {spec_for_add}"),
                                                ),
                                                cx,
                                            );
                                            window.close_dialog(cx);
                                        })
                                }),
                        )
                } else {
                    // 输入无法解析为合法的 GitHub 坐标
                    v_flex()
                        .w_full()
                        .p_3p5()
                        .rounded_lg()
                        .border_1()
                        .border_color(warning_color.opacity(0.35))
                        .bg(warning_color.opacity(0.08))
                        .gap_2()
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(
                                    Icon::new(IconName::TriangleAlert)
                                        .small()
                                        .text_color(warning_color),
                                )
                                .child(
                                    Label::new("无法精确匹配为合法的 GitHub 仓库坐标")
                                        .text_sm()
                                        .font_bold()
                                        .text_color(warning_color),
                                ),
                        )
                        .child(
                            Label::new("GitHub 仓库源仅支持精确匹配。请确认格式为 `owner/repo`、`github:owner/repo#branch` 或合法的 GitHub 仓库链接。")
                                .text_xs()
                                .text_color(foreground_color.opacity(0.85)),
                        )
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_standard_github_repo() {
        let spec = GitHubSpec::parse("deepseek-ai/deepseek-harness").unwrap();
        assert_eq!(spec.owner, "deepseek-ai");
        assert_eq!(spec.repo, "deepseek-harness");
        assert_eq!(spec.git_ref, None);
        assert_eq!(spec.install_specifier(), "github:deepseek-ai/deepseek-harness");
        assert_eq!(spec.web_url(), "https://github.com/deepseek-ai/deepseek-harness");
    }

    #[test]
    fn parses_github_prefix_with_ref() {
        let spec = GitHubSpec::parse("github:foo/bar#v1.2.0").unwrap();
        assert_eq!(spec.owner, "foo");
        assert_eq!(spec.repo, "bar");
        assert_eq!(spec.git_ref, Some("v1.2.0".to_string()));
        assert_eq!(spec.install_specifier(), "github:foo/bar#v1.2.0");
        assert_eq!(spec.web_url(), "https://github.com/foo/bar/tree/v1.2.0");
    }

    #[test]
    fn parses_https_github_url() {
        let spec = GitHubSpec::parse("https://github.com/torvalds/linux.git").unwrap();
        assert_eq!(spec.owner, "torvalds");
        assert_eq!(spec.repo, "linux");
        assert_eq!(spec.git_ref, None);
    }

    #[test]
    fn parses_https_github_url_with_tree_branch() {
        let spec = GitHubSpec::parse("https://github.com/owner/repo/tree/feat-modal").unwrap();
        assert_eq!(spec.owner, "owner");
        assert_eq!(spec.repo, "repo");
        assert_eq!(spec.git_ref, Some("feat-modal".to_string()));
        assert_eq!(spec.install_specifier(), "github:owner/repo#feat-modal");
        assert_eq!(spec.web_url(), "https://github.com/owner/repo/tree/feat-modal");
    }

    #[test]
    fn parses_git_ssh_url() {
        let spec = GitHubSpec::parse("git@github.com:facebook/react.git").unwrap();
        assert_eq!(spec.owner, "facebook");
        assert_eq!(spec.repo, "react");
        assert_eq!(spec.git_ref, None);
    }

    #[test]
    fn rejects_invalid_github_inputs() {
        assert_eq!(GitHubSpec::parse(""), None);
        assert_eq!(GitHubSpec::parse("   "), None);
        assert_eq!(GitHubSpec::parse("only-one-part"), None);
        assert_eq!(GitHubSpec::parse("too/many/parts/here"), None);
        assert_eq!(GitHubSpec::parse("invalid space/repo"), None);
    }

    #[test]
    fn parses_npm_search_json_array() {
        let json = r#"[
            {
                "name": "dsh-ast-edit-tool",
                "version": "0.2.0",
                "description": "AST edit tool for dsh"
            },
            {
                "name": "@scope/plugin",
                "version": "1.0.0",
                "description": "Scoped plugin"
            }
        ]"#;
        let results = parse_npm_search_json(json);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].name, "dsh-ast-edit-tool");
        assert_eq!(results[0].version, "0.2.0");
        assert_eq!(results[0].description, "AST edit tool for dsh");
        assert_eq!(results[0].detail_url, "https://www.npmjs.com/package/dsh-ast-edit-tool");
        assert_eq!(results[1].name, "@scope/plugin");
    }

    #[test]
    fn parses_npm_registry_objects_json() {
        let json = r#"{
            "objects": [
                {
                    "package": {
                        "name": "dsh",
                        "version": "1.0.1",
                        "description": "A shell written in JavaScript",
                        "links": {
                            "npm": "https://www.npmjs.com/package/dsh"
                        }
                    }
                }
            ]
        }"#;
        let results = parse_npm_search_json(json);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "dsh");
        assert_eq!(results[0].version, "1.0.1");
        assert_eq!(results[0].detail_url, "https://www.npmjs.com/package/dsh");
    }

    #[test]
    fn parses_empty_or_malformed_npm_json() {
        assert!(parse_npm_search_json("{}").is_empty());
        assert!(parse_npm_search_json("[]").is_empty());
        assert!(parse_npm_search_json("not json").is_empty());
    }
}
