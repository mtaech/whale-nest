# WhaleNest — dsh 管理器 · 交接文档（HANDOFF）

> 交给接手 agent 的完整任务书。全文自包含，不依赖任何历史对话。
> 接手第一步：完整读一遍本文 + `docs/dsh-manager-spec.md`。

---

## 0. 通用安全约定（所有执行环境适用）

本 app 是 dsh 管理器：它会 spawn 自己的 dsh 内核、并读写用户 dsh home（默认 `~/.dsh`）。开发/测试时遵守以下约定，避免干扰用户真实在用的 dsh。

**不要**：
- 不要 kill / 重启 / 停止任何**不是本 app 自己 spawn 的**、正在运行的 dsh 进程（用户可能正在另一个终端或别的 profile 里用着它）
- 不要修改用户真实在用的配置：`~/.dsh/profiles/*/package.json`、`cordis.patch.yml`、`~/.dsh/settings.yaml`
- 在删除内核对 `detect_existing_dsh`（探测已运行 dsh 并 attach）这段逻辑之前，**不要运行 WhaleNest 连真实 `~/.dsh`**——否则它会 attach 到任意端口上已运行的 dsh，可能抢走用户正在用的那个

**测试用隔离环境（推荐）**：
- 运行时/集成测试用 `export DSH_HOME=/tmp/whalenest-test-home`（独立 home，不指真实 `~/.dsh`）+ 独立端口（如 `--port 39123`）
- 验证完成 kill 掉**自己 spawn 的**测试进程（记下 PID）
- 静态验证用 `cargo check` / `cargo test`；GPUI 是图形框架，headless 环境可能起不了窗口 → 此时以编译通过 + 单测 + 静态逻辑审查为准，并在总结注明「GUI 未在真实显示环境验证」

---

## 1. 项目背景

- 仓库：`/home/huang/Personal/Dev/Code/melon`（melon，DSH 插件/皮肤单仓库）
- 目标项目：`dsh-desktop/`（独立 git 仓库，名为 whalenest，恰好在 melon 目录下）
- 工作目录：`/home/huang/Personal/Dev/Code/melon/dsh-desktop`（**GPUI 版**，已提升到仓库根目录）
- 改动范围：只动 `dsh-desktop/`。禁止动 `../src-tauri/`（Tauri 版旧主线）、`../prototype-egui-wry/`（废弃原型）、melon 的 `packages/*/`（插件，不归本项目）

### 一句话目标
把 WhaleNest 从「Tauri 壳 + 内嵌 webview 显示 dsh UI」改造成「**GPUI 原生 dsh 管理器**」：dsh 仍由 dsh 自身提供并后台常驻，界面走系统默认浏览器打开，管理器只做 4 件事——**profile 管理、插件管理（复用现成 dsh-plugin-dashboard）、后台常驻守护、浏览器打开**。同时把 UI 依赖从旧的 `gpui-component 0.5.2`（本地 path）升级到 `gpui-kit 0.6`（crates.io）。

---

## 2. 完整规格

> 已与需求方逐条确认。此文件是权威稿，改它前先向需求方说明。

### 2.1 定位
- 名称：**WhaleNest**（保留，图标/托盘/自启项沿用）
- 定位：dsh 管理器——profile 管理、插件管理（复用 plugin-dashboard）、后台常驻、浏览器打开
- 原则：零摩擦优先 / 内核稳定压倒一切 / 后台常驻随叫随到 / 配置可迁移 / 低调精致

### 2.2 技术栈（升级）
- Rust + GPUI，依赖改为 `gpui-kit = "0.6"`（crates.io，默认 features 含 `component`+`assets`）
- 移除依赖：`gpui`（git zed）、`gpui_platform`（git zed）、`gpui-component`（本地 path）、`gpui-component-assets`（本地 path）、`wry`/`lb-wry`、`base64`、`raw-window-handle`
- 保留：serde/serde_json/toml/parking_lot/libc/flume/arboard/png/open/rfd/image/anyhow；Linux `gtk`/`ksni`/`notify-rust`；Windows `tray-icon`/`winreg`；macOS `tray-icon`
- 保留：托盘（ksni/tray-icon）、`open`（浏览器/文件）、`rfd`（目录选择）、`arboard`（剪贴板）

### 2.3 数据模型
- `AppState`（`~/.config/whale-nest/config.toml`）：
  - `active_profile: String`（默认 `web`）
  - `profile_cwds: HashMap<String, PathBuf>`（每 profile 记住 cwd；迁移：旧全局 `cwd` → `profile_cwds["web"]`）
  - 保留 `preferred_port`/`lock_port`/`autostart`/`initialized`；移除 `recent_dirs`
- `ProfileInfo`（运行时扫描，不持久化）：`name`/`path`/`is_web_type`/`plugin_count`/`cwd`/`session_count`/`last_session_time`

### 2.4 内核
- 单实例 `Kernel`，`config.profile` 动态可改
- spawn 命令：`dsh --profile <name> --no-open --port <p>`
- 日志：每 profile 一个文件 `dsh-<profile>.log`
- 移除：`detect_existing_dsh`/`attach_to_existing`/`is_attached` 及相关分支（不再对接外部 dsh）
- 保留：崩溃自愈（10s/3 次止损）、端口自适应（`preferred_port` 3080 优先 + 漂移 + `lock_port`）、就绪探测（捕获带 token URL）、诊断

### 2.5 交互（仪表盘）
- 卡片列表，每个 web 型 profile 一张卡（宽窗 2 列、窄窗单列）：
  - 头部：名称 + 状态灯（运行中绿/启动中黄/已停止灰/出错红）+ 端口 badge（运行时）
  - meta 三行：插件数 / 工作目录 / 最近会话（「N 个会话 · 3 分钟前」）
  - 操作按钮：启动/切换（运行中显示「重启」）、在浏览器打开、更改目录、删除
- 顶部：标题 + 「创建 profile」+「刷新」
- 底部：日志区块（当前运行 profile 的 tail + 「打开完整日志文件」按钮）
- 空状态：无 web 型 profile 时显示「创建第一个 profile」引导

### 2.6 交互（操作语义）
- 启动/切换：未运行→启动；运行中→重启；其他在跑→先停旧的再起新的（激活即运行，无中间态）
- 在浏览器打开：未运行→自动启动+等就绪+打开；运行中→直接打开 token URL
- 停止：只停不切；全停后所有卡片「已停止」
- 创建：对话框输入 name → 校验（非空、不含路径分隔符、不重名、≠`web`）→ 生成最小模板（`dsh-base`+`dsh-web-app`）→ 自动切换并启动
- 删除：确认框（`web` 拒绝删除，按钮禁用+说明）；运行中→「`foo` 正在运行，将先停止再删除」→ 停内核→删目录→激活回退 `web`
- 更改目录：rfd 目录选择器 → 写入 `profile_cwds` → 若在跑则自动重启生效

### 2.7 托盘 / 向导 / 设置 / 生命周期
- 托盘：打开主界面 / 切换 profile 子菜单 / 在浏览器打开 / 重启内核 / 更多（复制诊断、打开日志、打开配置、检查更新）/ 开机自启 / 退出
- 首次向导（一步）：环境检测（node/npm/pnpm/dsh）+ dsh 缺失时一键安装；删除推荐插件步
- 设置对话框（只留全局）：开机自启 / 固定端口 / dsh 版本更新 / 诊断维护；移除工作目录相关
- 生命周期：关窗→托盘化；退出→kill 内核；开机自启→连带拉起 dsh

### 2.8 实现细节（已定死）
- 插件数 = `dsh.profile.bundles` 里非 `@deepseek-ai/` 前缀的条数
- 最近会话 = 该 profile cwd 对应 `~/.dsh/sessions/--<cwd 编码>--/` 下 session 目录数 + 目录 mtime
- 端口复用 `preferred_port`（单实例）
- `KernelStatus` 事件带 profile 名
- 刷新时机：启动扫一次 + 创建/删除/切换后重扫 + 手动刷新，不轮询
- 日志区块跟随「运行中 profile」；未运行时显示上次激活 profile 的日志文件
- 状态灯五态：运行中（绿）/ 启动中（黄）/ 已停止（灰）/ 出错（红）/ 待装 dsh（引导）

---

## 3. gpui-kit 0.6 迁移映射（已核实，直接照做）

gpui-kit 0.6.0 是聚合 crate，一个依赖打包整个 GPUI 栈（含 zed 官方发布的 gpui-pre 0.3.1 / gpui-pre-platform 0.3.1 / gpui-base 0.6 / gpui-component 0.6 / gpui-kit-assets 0.6）。

**Cargo.toml**：删 4 加 1（见 2.2），其余保留。

**import 替换**（机械，sed/精确编辑，勿伤字符串注释）：
- `use gpui::` → `use gpui_kit::`；`gpui::`（路径）→ `gpui_kit::`
- `use gpui_component::` → `use gpui_kit::component::`；`gpui_component::` → `gpui_kit::component::`
- `gpui_component_assets::Assets` → `gpui_kit::assets::Assets`
- `gpui_platform::application()` → `gpui_kit::application()`
- `gpui_component::init(cx)` → `gpui_kit::init(cx)`
- 其它 `gpui_platform::` → `gpui_kit::platform::`
- `gpui_kit::*` 已 re-export 所有 gpui 类型，故 `use gpui::*` 可换 `use gpui_kit::*`

**注意**：gpui 现发布为 `gpui-pre`，原 `#[derive(gpui::Action)]` 派生路径失效 → 改用 gpui-kit 的 `actions!` 宏，或前缀改 `gpui_kit::`。其余逐个编译错误修。rustc 1.97.1 已装（edition 2024 OK）。本机 cargo/rustc 在 `~/.cargo/bin`，当前 PATH 不含 → 每次编译先 `export PATH=$HOME/.cargo/bin:$PATH`。首次编译 gpui-kit 依赖树很慢（10-30 分钟），用 `cargo check` 做验收并耐心等待。

---

## 4. 分阶段执行计划

> 三段串行。每段验收通过再进下一段。每段只改 `gpui/`。

### Phase 1 — 依赖迁移 + 删 webview 链路（编译地基）
- 改 `Cargo.toml`：按 2.2 换依赖；profile.dev 优化段里对 `gpui`/`gpui_platform` 的 opt-level 段删掉
- 全局 import 按 §3 替换所有 `src/*.rs`
- 删 `src/webview.rs`、`src/host.rs`、`src/bridge.rs`、`src/bridge.js`
- `main.rs`：删 `mod webview; mod bridge; mod host;`；删 Windows `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` 段；删 Linux `WEBKIT_FORCE_COMPOSITING_MODE` 和「Wayland→X11」段
- `shell.rs`：删 `host` 字段、`ensure_host`、`hide_webview`、`spawn_glib_pump`、所有 `self.host`/`host::` 引用、`webview_error`；`ShellState::Ready{url}` 渲染分支直接调 `render_ready_browser(&url, window, cx)`；`set_ready` 删除建 webview 逻辑
- 其它文件若引用 `crate::host`/`crate::webview`/`crate::bridge` 一并清
- 本阶段不改功能逻辑（kernel 的 profile 用法、state 数据模型留 Phase 2）。gpui 0.2→0.3 若有 breaking change，做最小必要修改修到编译通过、保持语义
- 验收：`cargo check` exit 0（warning 可接受）

### Phase 2 — 内核与数据层（profile 中心化）
- `state.rs`：`AppState` 换 `active_profile`+`profile_cwds`（HashMap），删 `recent_dirs`，加载时迁 `cwd`→`profile_cwds["web"]`；加 `ProfileInfo` 结构
- `kernel.rs`：`spawn` 用 `self.config.profile`（`dsh --profile <name> --no-open --port <p>`）；日志路径按 `dsh-<profile>.log`；删 `detect_existing_dsh`/`attach_to_existing`/`is_attached` 及相关分支（`KernelState::Ready` 保留）
- `app.rs`：`Control` 新增 `SwitchProfile(String)`/`CreateProfile`/`DeleteProfile(String)`/`ChangeCwd`/`Stop`；新增 `scan_profiles()`（列 `$DSH_HOME/profiles/*` 中 web 型）；`switch_profile`（持久化 + 更新 kernel.config.profile + 重启）；删 `install_plugin`/`get_repo_plugins_list`/`read_installed_dsh_plugins`（推荐插件向导已砍）；`Managed::new` 用 `active_profile` 初始化 kernel.config；`install_dsh` 等保留；`KernelStatus` 事件携带 profile 名
- `wizard.rs`：删推荐插件步（Step 2 相关 UI/代码）
- 验收：`cargo check` exit 0 + `cargo test` 通过（含 state 迁移/序列化单测）

### Phase 3 — UI 层（仪表盘 + 托盘 + 设置 + main）
- `shell.rs`：重写为卡片仪表盘（§2.5）+ 日志区块（§2.8）；操作按 §2.6 语义；空状态；移除 webview 残留
- `tray.rs`：加「切换 profile」子菜单（列 web 型 profile，点击=切换+启动）
- `settings.rs`：删工作目录区块，只留全局项（§2.7）
- `main.rs`：清理 webview 相关初始化；按新 `Control` 处理新变体
- `lifecycle.rs`/`notify.rs`/`updater.rs` 基本不动
- 验收：`cargo check` + `cargo build` 通过；`cargo test` 通过

### Phase 4 — 运行时验证（可选，需隔离）
- 严格按 §0 隔离：`DSH_HOME=/tmp/whalenest-test-home` + 端口 39123+，绝不连用户真实在用的 dsh home `~/.dsh`
- 验证：profile 列表扫描、创建/删除/切换、启动→浏览器打开、停止、常驻自愈
- headless 起不了 GUI 则跳过，改为代码审查 + 编译 + 单测，并在总结注明

---

## 5. 编译环境与命令

export PATH=$HOME/.cargo/bin:$PATH
cd /home/huang/Personal/Dev/Code/melon/dsh-desktop
cargo check 2>&1 | tail -60     # 快速验收（推荐）
cargo test                      # 单测（需显示环境则跳过 GUI 相关）
cargo build                     # 最终构建

---

## 6. 当前状态
- `gpui/`：原始未动（所有文件停在 9 月 2 日，Cargo.toml 仍是旧 git/path 依赖）。可直接从 Phase 1 开始。若要保险，先 `cp -r gpui gpui.original-backup`。
- 规格文档已写好：`/home/huang/Personal/Dev/Code/melon/dsh-desktop/docs/dsh-manager-spec.md`（与本文 §2 一致）。
- 上游依赖：`gpui-kit 0.6` 需要网络从 crates.io 拉取（首次编译慢）。
- 交接产物：本 HANDOFF.md（+ spec 文档）。接手 agent 从这里独立执行。

---

## 7. 成功判据
- `cargo check`（无 error）+ `cargo test` 通过
- 代码满足 §2.5/§2.6/§2.7 交互规格；`webview`/`host`/`bridge` 链路已删
- 运行时不碰用户真实在用的 dsh home（隔离测试，红线全程守住）
- 提交信息用中文，参考 dsh-desktop 仓库历史风格（git log 是中文 commit）

## 8. 附：现有代码结构（改造前）
`gpui/src/`：`main.rs`（入口/单实例锁/监督/托盘/控制监听/主窗口 Root<Shell>）、`app.rs`（Managed 全局 + Control 枚举 + 内核启停/监督/环境检测/插件/更新 + AppEvent）、`kernel.rs`（KernelConfig 已有 profile 字段 + Kernel 监督/诊断）、`state.rs`（AppState）、`shell.rs`（Shell 标题栏+webview 三态+向导+设置，ShellState 四态）、`tray.rs`（ksni/tray-icon 托盘）、`settings.rs`（设置对话框）、`wizard.rs`（两步向导）、`lifecycle.rs`（自启/open_url/退出）、`readiness.rs`（就绪探测）、`updater.rs`（更新）、`notify.rs`（通知）、`webview.rs`/`host.rs`/`bridge.rs`/`bridge.js`（内嵌 webview，要删）。