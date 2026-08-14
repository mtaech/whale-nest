# WhaleNest

[![License](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)

WhaleNest 是一个基于 Tauri v2 的桌面客户端：后台常驻运行 DeepSeek Harness (dsh)，前端展示 dsh 的 Web UI。告别「开终端 → cd 到目录 → 运行 dsh web → 手动开浏览器」的繁琐流程，双击图标即可使用。

## 特性

- **一键启动**：应用启动时自动拉起 dsh 内核，无需手动开终端
- **托盘常驻**：关闭窗口最小化到托盘，dsh 继续后台运行，随时唤回
- **端口自适应**：优先使用 3080，被占用时自动切换空闲端口
- **崩溃自愈**：dsh 异常退出后自动重启，短时间连续崩溃则止损停止
- **工作目录**：记住上次工作目录，托盘菜单一键切换（会话按目录归档）
- **诊断日志**：捕获 dsh 输出到滚动日志，托盘一键复制诊断信息

## 快速开始

### 环境要求

- Windows 10/11（需 WebView2）或 Linux
- Node.js >= 22，且已全局安装 dsh
- Rust >= 1.77
- pnpm

### 安装 dsh

WhaleNest 依赖已安装的 DeepSeek Harness (dsh) 命令行工具：

```bash
npm install -g @deepseek-ai/dsh
```

### 开发与构建

```bash
# 安装前端依赖
pnpm install

# 开发模式
pnpm tauri dev

# 打包发布
pnpm tauri build
```

构建产物位于 `src-tauri/target/release/`：

- `WhaleNest.exe`（Windows 可执行文件）
- `bundle/nsis/WhaleNest_0.1.0_x64-setup.exe`（NSIS 安装包）
- `bundle/msi/WhaleNest_0.1.0_x64_en-US.msi`（MSI 安装包）

## 使用说明

1. 双击 `WhaleNest.exe` 启动
2. 首次启动以用户主目录作为工作目录，可在托盘菜单「切换工作目录」修改
3. 关闭窗口会最小化到托盘，dsh 继续后台运行
4. 托盘菜单提供：打开主界面 / 切换工作目录 / 重启 dsh 内核 / 复制诊断信息 / 打开日志 / 开机自启 / 退出

## 项目结构

```
.
├── src/                 # 前端薄壳（Vite + TypeScript）
├── src-tauri/           # Tauri 后端（Rust）
│   └── src/
│       ├── kernel.rs    # dsh 内核抽象（进程管理）
│       ├── readiness.rs # 就绪探测
│       ├── state.rs     # 配置持久化
│       └── lifecycle.rs # 托盘 / 窗口生命周期
├── package.json
└── tauri.conf.json
```

## 许可证

[MIT](./LICENSE)