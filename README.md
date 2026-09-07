# WhaleNest

[![License](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)

WhaleNest 是一个基于 **Rust + GPUI** 的桌面应用，用作 DeepSeek Harness (dsh) 的管理器：管理 dsh profile、查看/启用插件、后台常驻守护 dsh 内核，并一键在系统浏览器里打开 dsh 的 Web UI。

> 由原先的 Tauri v2 + 内嵌 WebView 方案迁移而来：不再内嵌浏览器，界面走系统默认浏览器打开，应用本体只承担 profile / 插件 / 内核守护 / 浏览器打开这四件事。

## 特性

- **profile 管理**：扫描并卡片化展示每个 web 型 profile，支持创建 / 启动 / 切换 / 重启 / 删除
- **插件管理**：在仪表盘里原生列出每个 profile 已安装的插件，带启用/禁用开关（读写 `dsh.profile.bundles`，官方核心不可动；切换后正在运行的 profile 自动重启内核生效）
- **后台常驻**：关窗最小化到托盘，dsh 内核继续后台运行，随叫随到
- **崩溃自愈**：dsh 异常退出后自动重启，短时间连续崩溃则止损停止
- **端口自适应**：优先使用 3080，被占用时自动漂移到空闲端口
- **浏览器打开**：无需手动开终端，一键在系统默认浏览器打开 dsh Web UI
- **诊断日志**：每 profile 一份日志，托盘一键复制诊断信息

## 快速开始

### 环境要求

- Windows 10/11 或 Linux 或 macOS
- Rust（cargo / rustc）
- 已全局安装 dsh（`npm i -g @deepseek-ai/dsh`）
- Linux 需额外安装 GPUI 运行所需的系统库（见下文）

### 安装 dsh

WhaleNest 依赖已安装的 DeepSeek Harness (dsh) 命令行工具：

```bash
npm install -g @deepseek-ai/dsh
```

### 构建与运行

```bash
cargo run       # 开发运行（仓库根目录即 GPUI 工程根）
cargo build     # 生成可执行文件 target/debug/whalenest
```

### Linux 系统依赖

```bash
sudo apt-get install -y \
  build-essential pkg-config \
  libx11-dev libxkbcommon-dev \
  libxcb1-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
  libxkbcommon-x11-dev libwayland-dev \
  libgtk-3-dev libssl-dev \
  libfontconfig1-dev libfreetype6-dev
```

## 使用说明

1. 首次启动进入环境检测向导（node / pnpm / dsh），dsh 缺失时可一键安装
2. 主界面为 profile 工作台：左侧 Profiles 列表，右侧为选中 profile 的详情（Hero 操作栏、运行指标、[Tab] 实时日志 / 插件管理，默认显示实时日志）
3. 打开某 profile = 启动内核（若未运行）+ 在系统浏览器打开；关闭窗口会最小化到托盘，dsh 继续后台运行
4. 托盘菜单提供：打开主界面 / 切换 profile / 在浏览器打开 / 重启内核 / 复制诊断信息 / 打开日志 / 开机自启 / 退出

### 插件管理

WhaleNest 的插件管理**完整对齐 dsh 的插件机制**——所有写操作都通过 dsh plugin --profile <name> <args> 执行，与 dsh 命令行行为一致（含 dsh.profile.bundles 的自动对账）：

- **列插件**：右侧「插件管理 · N 个已启用」面板列出当前 profile 安装的每个用户插件：状态灯（绿=启用 / 灰=禁用）、插件名、版本号；面板会**异步查询 outdated**，有新版插件显示「可升级 ↑」徽标并高亮其「升级」按钮。
- **启用/禁用**：每个插件右侧有一个开关，开启把包名写入 dsh.profile.bundles，关闭从数组移除；官方核心（@deepseek-ai/ 前缀）不可动，界面不显示开关。
- **安装**：面板顶部输入框填 npm 包名（如 dsh-skin-material-you）+「安装」按钮，走 dsh plugin ... add <pkg>。
- **升级**：每个插件右侧「升级」按钮，**先校验**该插件确有新版（走 dsh plugin ... outdated），有新版才执行 dsh plugin ... update <pkg>；已是最新则明确提示「无需升级」，不会无脑硬升。
- **卸载**：每个插件右侧「卸载」按钮，走 dsh plugin ... remove <pkg>。
- 上述操作均为**后台异步执行**，不阻塞界面；进行中弹「进行中」通知，完成后弹成功/失败通知并自动刷新列表。
- 若某插件正在运行，操作成功后 WhaleNest 会自动重启内核使变更生效。
- 依赖 pnpm（dsh 插件管理靠它转发），需先安装 pnpm。

## 项目结构

```
.
├── src/                  # GPUI 应用源码
│   ├── main.rs           # 入口 / 单实例锁 / 监督 / 托盘 / 主窗口
│   ├── app.rs            # 应用级状态与事件总线 + profile 扫描 / 插件启用禁用（Managed + Control）
│   ├── desktop_entry.rs  # Wayland / XDG 桌面入口（.desktop + 主题图标安装）
│   ├── kernel.rs         # dsh 内核抽象（进程管理 / 就绪 / 自愈）
│   ├── shell.rs          # 仪表盘工作台（profile 列表 + 插件管理 + 终端日志）
│   ├── state.rs          # 配置持久化（active_profile + profile_cwds）
│   ├── tray.rs           # 托盘（ksni / tray-icon）
│   └── ...               # settings / wizard / updater / lifecycle / notify
├── docs/                 # 需求规格与设计文档
├── Cargo.toml            # GPUI 工程（Rust）
├── public/               # 应用图标（whalenest-mark.png，编译期嵌入二进制）
├── assets/               # 品牌 / 启动图资源
└── tools/                # 图标处理等辅助脚本
```

## 开发约定

- 运行/集成测试请用隔离环境，避免干扰用户真实在用的 dsh：`export DSH_HOME=/tmp/whalenest-test-home` + 独立端口
- 编译验收：`cargo check`（仓库根目录直接编译；首次编译 gpui-kit 依赖树较慢，约 10–30 分钟）
- 提交信息用中文

## 许可证

[MIT](./LICENSE)