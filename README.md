# WhaleNest

[![License](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)

WhaleNest 是一个基于 **Rust + GPUI** 的桌面应用，用作 DeepSeek Harness (dsh) 的管理器：管理 dsh profile、查看/启用插件、后台常驻守护 dsh 内核，并一键在系统浏览器里打开 dsh 的 Web UI。

> 由原先的 Tauri v2 + 内嵌 WebView 方案迁移而来：不再内嵌浏览器，界面走系统默认浏览器打开，应用本体只承担 profile / 插件 / 内核守护 / 浏览器打开这四件事。

## 特性

- **profile 管理**：扫描并卡片化展示每个 web 型 profile，支持创建 / 启动 / 切换 / 重启 / 更改目录 / 删除
- **插件管理**：复用 `dsh` 的 plugin-dashboard，在仪表盘里查看与启用插件
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
2. 主界面为 profile 仪表盘：每个 web 型 profile 一张卡片，含状态灯、端口、插件数、工作目录与最近会话
3. 打开某 profile = 启动内核（若未运行）+ 在系统浏览器打开；关闭窗口会最小化到托盘，dsh 继续后台运行
4. 托盘菜单提供：打开主界面 / 切换 profile / 在浏览器打开 / 重启内核 / 复制诊断信息 / 打开日志 / 开机自启 / 退出

## 项目结构

```
.
├── src/                  # GPUI 应用源码
│   ├── main.rs           # 入口 / 单实例锁 / 监督 / 托盘 / 主窗口
│   ├── app.rs            # 应用级状态与事件总线（Managed + Control）
│   ├── kernel.rs         # dsh 内核抽象（进程管理 / 就绪 / 自愈）
│   ├── shell.rs          # 仪表盘卡片 UI
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