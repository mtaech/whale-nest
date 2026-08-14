# Product

<!-- impeccable:product-schema 1 -->

## Platform

adaptive — 原生桌面应用（Tauri v2），以 Windows 为主（当前开发/构建环境），Linux/macOS 保持可编译可用。

## Stack

Tauri v2 + Vite + TypeScript（前端壳）+ Rust（内核监督/托盘/生命周期）。前端薄壳页（`index.html` + `src/main.ts` + `src/styles.css`）只在内核未就绪/出错/缺 dsh 时展示；就绪后整窗导航到 dsh web UI（`http://127.0.0.1:<port>`）。

## Users

开发者。不想经历「开终端 → cd 到目录 → 运行 dsh web → 手动开浏览器」的繁琐流程，期望双击图标即用，后台自动拉起 DeepSeek Harness (dsh) 内核并在窗口内呈现其 Web UI。

## Product Purpose

让 dsh（DeepSeek Harness 命令行工具）有一个零摩擦的桌面入口：应用启动即自动拉起 dsh 内核、托盘常驻后台运行、窗口即 dsh 的 Web UI，关闭窗口最小化到托盘而不是退出。

## Positioning

不是 dsh 的替代品，而是 dsh 的桌面宿主壳：把「命令行拉起 + 浏览器打开」压缩成「双击图标」。内核仍由 dsh 提供，壳负责解析、拉起、监督（崩溃自愈）、端口自适应、托盘与工作目录管理。

## Operating Context

- 用户从托盘菜单可：打开主界面 / 切换工作目录（dsh 会话按 cwd 归档）/ 重启 dsh 内核 / 复制诊断信息 / 打开日志 / 开关开机自启 / 退出
- 应用记住上次工作目录，会话按目录归档（dsh 的 `~/.dsh/sessions/--<cwd 编码>--`）
- 端口优先 3080，被占自动切换空闲端口
- 诊断日志滚动写入 `%APPDATA%/DSH Desktop/dsh.log`（1MB 轮转）
- dsh 异常退出自动重启，10s 窗口内 ≥3 次崩溃止损进入错误页
- 未检测到 dsh 时展示引导页（安装命令 + 复制 + 重新检测）

## Capabilities and Constraints

- 后端模块：`kernel.rs`（解析/spawn/监督/诊断）、`readiness.rs`（就绪探测）、`state.rs`（配置持久化）、`lifecycle.rs`（托盘/关窗/自启/退出）
- IPC 契约（前端勿改名）：事件 `kernel-status`；命令 `get_state` / `set_working_dir` / `restart_kernel` / `get_diagnostics` / `open_log_file` / `copy_diagnostics` / `quit`
- 依赖系统已安装的 dsh（`npm i -g @deepseek-ai/dsh`），不内置打包 dsh
- 前端壳三态：loading（启动中）/ error（启动失败，可重启）/ guide（未检测到 dsh）
- 技术约束：Tauri v2 default features 不含 tray-icon（已显式开启）；Windows 上以 dsh.ps1/cmd shim 经 `cmd /C` 或 `powershell -File` 拉起，`CREATE_NO_WINDOW` 防黑窗
- 待定事实：无（未发现需记录的重大未决项）

## Brand Commitments

- 名称 **WhaleNest**（鲸巢），与「波浪/鲸鱼」logo 意象绑定，不可替换
- 用户明确选择的 **「极简高级」** 视觉方向为品牌约束（克制排版、暗色、精致动效、细线图标），后续界面延续此方向
- 深色主题优先（用户已确认暗色环境光 + 深色背景的启动页风格）

## Evidence on Hand

- `README.md`：产品定位、特性、使用说明
- `BUILD_PLAN.md`：架构解析版本、模块边界与函数签名、关键机制备忘
- `src-tauri/icons/`：已生成的透明图标套件（`app-icon.png` 源 + `app-icon-transparent.png` 透明版 + 全套尺寸）
- 启动壳页三态实现（`index.html` / `src/main.ts` / `src/styles.css`）
- 缺失：无用户测试/评价数据，无真实使用录像；未来工作不得虚构这些

## Product Principles

1. **零摩擦优先**：任何流程，若能用「双击/托盘单击」代替手动步骤，就替。壳存在的唯一理由是消除摩擦。
2. **内核稳定压倒一切**：dsh 是心脏，壳只做监督（崩溃自愈、止损、诊断），不做替代；任何改动不得削弱内核隔离。
3. **后台常驻、随叫随到**：关窗不退出，托盘唤回；退出才真正杀内核。
4. **配置可迁移**：工作目录、端口、自启等状态持久化，重启后原样恢复。
5. **低调而精致**：界面克制、暗色、细节考究（动效、焦点、选中态），不与 dsh 内容抢注意力。

## Accessibility & Inclusion

- 前端壳页支持 `prefers-reduced-motion`（关闭动画）
- 关键文本对比度 ≥ 4.5:1；键盘焦点环（`:focus-visible`）已实现
- 未建立 WCAG 等级承诺；托盘/原生菜单为系统组件，随系统无障碍
