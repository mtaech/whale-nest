# WhaleNest — dsh 管理器 完整规格（已与用户确认）

> 本项目：`dsh-desktop`。是 dsh-desktop 仓库里 GPUI 版（已提升到仓库根目录），从 Tauri 版移植而来。
> 本规格是最终确认稿，所有 subagent 以本文档为需求基准。改动范围：**只动 `dsh-desktop/`**，禁止动 `src-tauri/`（Tauri 版）和 `prototype-egui-wry/`。

## 1. 定位
- 名称：WhaleNest（保留，图标/托盘/自启项沿用）
- 定位：dsh 管理器——profile 管理、插件管理（复用 plugin-dashboard）、后台常驻、浏览器打开
- 原则：零摩擦优先 / 内核稳定压倒一切 / 后台常驻随叫随到 / 配置可迁移 / 低调而精致

## 2. 技术栈（已定：升级 gpui-kit 0.6）
- Rust + GPUI，依赖升级为 `gpui-kit = "0.6"`（crates.io，默认 features 含 `component` + `assets`）
- **移除**依赖：`gpui`（git zed）、`gpui_platform`（git zed）、`gpui-component`（本地 path）、`gpui-component-assets`（本地 path）、`wry`/@wry、`base64`、`raw-window-handle`
- **保留**：serde/serde_json/toml/parking_lot/libc/flume/arboard/png/open/rfd/image/anyhow；Linux `gtk`/`ksni`/`notify-rust`；Windows `tray-icon`/`winreg`；macOS `tray-icon`
- **import 迁移映射**：`gpui::`→`gpui_kit::`；`gpui_component::`→`gpui_kit::component::`；`gpui_component_assets::Assets`→`gpui_kit::assets::Assets`；`gpui_platform::application()`→`gpui_kit::application()`；`gpui_component::init`→`gpui_kit::init`；`use gpui_component::{...}`→`use gpui_kit::component::{...}`
- 编译环境：cargo/rustc 在 `~/.cargo/bin`（PATH 不含，需 `export PATH=$HOME/.cargo/bin:$PATH`）。rustc 1.97.1（edition 2024 OK）
- 注意：gpui 被 zed 改名 `gpui-pre`，原 `#[derive(gpui::Action)]` 派生路径失效，改用 gpui-kit 的 `actions!` 宏（或 `gpui_kit::Action`）

## 3. 数据模型
- `AppState`（`~/.config/whale-nest/config.toml`）：
  - `active_profile: String`（默认 `web`）
  - `profile_cwds: HashMap<String, PathBuf>`（每 profile 记住 cwd；迁移：旧全局 `cwd` → `profile_cwds["web"]`）
  - `preferred_port` / `lock_port` / `autostart` / `initialized` 保留
  - **移除** `recent_dirs`
- `ProfileInfo`（运行时扫描，不持久化）：`name` / `path` / `is_web_type` / `plugin_count` / `cwd` / `session_count` / `last_session_time`

## 4. 内核
- 单实例 `Kernel`，`config.profile` 动态可改
- spawn 命令：`dsh --profile <name> --no-open --port <p>`
- 日志：每 profile 一个文件 `dsh-<profile>.log`
- **移除**：`detect_existing_dsh` / `attach_to_existing` / `is_attached` 及相关分支，不再对接外部 dsh
- 保留：崩溃自愈（10s/3 次止损）、端口自适应（preferred_port 3080 优先 + 漂移 + lock_port）、就绪探测（捕获带 token URL）、诊断

## 5. 交互规格
### 仪表盘（主窗口）
- **卡片列表**，每个 web 型 profile 一张卡（宽窗 2 列网格、窄窗单列）：
  - 头部：名称 + 状态灯（运行中绿/启动中黄/已停止灰/出错红）+ 端口 badge（运行时）
  - meta 三行：插件数 / 工作目录 / 最近会话（「N 个会话 · 3 分钟前」）
  - 操作按钮：**启动/切换**（运行中显示为「重启」）、**在浏览器打开**、**更改目录**、**删除**
- 顶部：标题 + 「创建 profile」+「刷新」
- 底部：日志区块（当前运行 profile 的 tail + 「打开完整日志文件」按钮）
- 空状态：无 web 型 profile 时显示「创建第一个 profile」引导

### 操作语义
- 启动/切换：未运行→启动；运行中→重启；其他在跑→先停旧的再起新的（激活即运行，无中间态）
- 在浏览器打开：未运行→自动启动+等就绪+打开；运行中→直接打开 token URL
- 停止：只停不切；全停后所有卡片「已停止」
- 创建：对话框输入 name → 校验（非空、不含路径分隔符、不重名、≠`web`）→ 生成最小模板（`dsh-base`+`dsh-web-app`）→ 自动切换并启动
- 删除：确认框（`web` 拒绝删除，按钮禁用+说明）；运行中→「`foo` 正在运行，将先停止再删除」→ 停内核→删目录→激活回退 `web`
- 更改目录：rfd 目录选择器 → 写入 `profile_cwds` → 若在跑则自动重启生效

### 托盘
打开主界面 / **切换 profile 子菜单**（列所有 web 型 profile，点击=切换+启动）/ 在浏览器打开 / 重启内核 / 更多（复制诊断、打开日志、打开配置、检查更新）/ 开机自启 / 退出

### 首次向导（一步）
环境检测（node/npm/pnpm/dsh）+ dsh 缺失时一键安装；**删除**推荐插件步。完成后进仪表盘。

### 设置对话框（只留全局）
开机自启 / 固定端口 / dsh 版本更新 / 诊断维护；**移除**工作目录相关（cwd 移到 profile 卡片内）。

## 6. 生命周期
- 关窗→托盘化（托盘可用时）；退出→kill 内核；开机自启→连带拉起 dsh

## 7. 实现细节（已定死）
- 插件数 = `dsh.profile.bundles` 里非 `@deepseek-ai/` 前缀的条数
- 最近会话 = 该 profile cwd 对应 `~/.dsh/sessions/--<cwd 编码>--/` 下的 session 目录数 + 目录 mtime
- 端口复用 `preferred_port`（单实例）
- `KernelStatus` 事件带 profile 名
- 刷新时机：启动扫一次 + 创建/删除/切换后重扫 + 手动刷新，不轮询
- 日志区块跟随「运行中 profile」，未运行时显示上次激活 profile 的日志文件
- 状态灯五态：运行中（绿）/ 启动中（黄）/ 已停止（灰）/ 出错（红）/ 待装 dsh（引导）
- 删除逻辑：当前激活或运行中的 profile 删除后，激活回退 `web`（若存在）

## 8. 文件改造映射
| 动作 | 文件 | 说明 |
|---|---|---|
| 改 | `kernel.rs` | spawn 用 `config.profile`；日志按 profile 分文件；删对接逻辑 |
| 改 | `state.rs` | `AppState` 换 `active_profile`+`profile_cwds`；删 `recent_dirs`；迁移旧 `cwd` |
| 改 | `app.rs` | `Control` 加 `SwitchProfile/CreateProfile/DeleteProfile/ChangeCwd/Stop`；删 `install_plugin`/`get_repo_plugins_list`/对接相关；新增 profile 扫描 |
| 改 | `tray.rs` | 加「切换 profile」子菜单 |
| 改 | `settings.rs` | 删工作目录区块 |
| 改 | `wizard.rs` | 砍推荐插件步 |
| 改 | `shell.rs` | 重写为卡片仪表盘 + 日志区块 |
| 改 | `main.rs` | 去 webview 相关环境变量/平台处理 |
| 删 | `webview.rs` `host.rs` `bridge.rs` `bridge.js` | webview 链路 |

## 9. 现有代码结构（改造前）
`dsh-desktop/src/`：
- `main.rs`：入口、单实例锁、全局状态、监督线程、托盘、控制监听、主窗口（Root<Shell>）
- `app.rs`：`Managed`（全局）、`Control` 枚举、内核启停/监督/环境检测/插件/更新、`AppEvent`
- `kernel.rs`：`KernelConfig`（已有 `profile` 字段）+ `Kernel`（resolve/spawn/supervise/diagnose）
- `state.rs`：`AppState`（cwd/preferred_port/autostart/lock_port/recent_dirs/initialized）
- `shell.rs`：`Shell`（标题栏+webview 三态+向导+设置），`ShellState`（Loading/Ready/Error/Guide），`host: Option<host::WebviewHost>`
- `tray.rs`：ksni（Linux）/ tray-icon（Win/mac）托盘，`Control` 发送
- `settings.rs`：设置对话框面板（工作目录/开关/版本/诊断）
- `wizard.rs`：首次向导（Step1 环境检测 / Step2 推荐插件）
- `lifecycle.rs`：自启、`open_url`/`open_path`、退出序列
- `readiness.rs`：就绪探测
- `updater.rs`：更新检查
- `notify.rs`：系统通知
- `webview.rs`/`host.rs`/`bridge.rs`/`bridge.js`：内嵌 webview 链路（要删）
