# DSH Desktop — BUILD_PLAN

Tauri v2 + Vite 桌面壳，宿主 DeepSeek Harness (dsh) 内核。
本文件记录骨架阶段的**实际解析版本**、**目录结构**与**后续模块边界/建议签名**，
供后续 kernel / state / lifecycle 实现对齐。

---

## 1. 实际解析版本（骨架阶段实测）

### npm/pnpm 侧（pnpm 11.13.0，registry = https://registry.npmmirror.com）

| 包 | 解析版本 | 备注 |
| --- | --- | --- |
| @tauri-apps/api | 2.11.1 | |
| @tauri-apps/cli | 2.11.4 | |
| vite | 7.3.6 | 需 Node >= 22.12（本机 22.22.2 ✓） |
| typescript | 5.0.2 | ^5 被 npmmirror 解析到 5.0.2（镜像怪癖）；tsconfig 特性均 >=5.0，功能够用。要更新请显式 pin |

### crates.io 侧（稳定版上限，"2"/"1" caret 会解析到它们）

| crate | 最新稳定 | 说明 |
| --- | --- | --- |
| tauri | 2.11.5 | **default features 不含 tray-icon**（default=wry,compression,common-controls-v6,dynamic-acl,x11,dbus）→ 已显式开 tray-icon + image-png + image-ico |
| tauri-build | 2.6.3 | |
| tauri-plugin-single-instance | 2.4.3 | |
| tauri-plugin-autostart | 2.5.1 | |
| tauri-plugin-dialog | 2.7.2 | |
| tauri-plugin-opener | 2.5.4 | |
| tauri-plugin-clipboard-manager | 2.3.2 | |
| serde / serde_json | 1.x（最新） | serde 开了 derive |

### 工具链
- pnpm 11.13.0（**11 不再读 package.json 的 pnpm 字段**；构建脚本审批走 pnpm-workspace.yaml 的 allowBuilds map 语法）
- Node v22.22.2（volta 管理的 dsh 同源）

---

## 2. 目录结构（已建成）

    E:\Dev\Code\dhs-desktop\
    ├─ package.json                 # name=dsh-desktop, type=module; scripts: dev/build/preview/tauri
    ├─ pnpm-workspace.yaml          # pnpm 11 设置：allowBuilds.esbuild=true（esbuild postinstall 审批）
    ├─ index.html                   # Vite 入口，挂 #app
    ├─ vite.config.ts               # 固定端口 1420 + strictPort + clearScreen false
    ├─ tsconfig.json                # ES2022 / bundler resolution / noEmit（tsc 仅做类型检查）
    ├─ src\
    │  ├─ main.ts                   # 占位页（真实三态壳后续做）
    │  └─ styles.css
    ├─ src-tauri\
    │  ├─ Cargo.toml                # package=dsh-desktop, lib=dsh_desktop_lib, edition 2021
    │  ├─ build.rs                  # tauri_build::build()
    │  ├─ tauri.conf.json           # identifier=dev.dsh.desktop / productName=DSH Desktop / devUrl=1420
    │  ├─ capabilities\default.json # core/dialog/opener/clipboard-manager :default，windows=[main]
    │  ├─ icons\                    # app-icon.png(源) + tauri icon 生成的全套占位图标
    │  └─ src\
    │     ├─ main.rs                # windows_subsystem 属性 + 调 lib::run()
    │     └─ lib.rs                 # Builder：单实例/自启/dialog/opener/clipboard 插件 + 占位托盘（仅“退出”）

---

## 3. 模块边界与建议函数签名（下一步实现）

### src-tauri/src/kernel.rs — dsh 内核抽象（可替换、可扩展的核心）

    /// 内核启动参数；profile/port/cwd/patches 全部参数化，扩展时只改这里
    #[derive(Clone, Debug)]
    pub struct KernelConfig {
        pub profile: String,        // v1 固定 "web"；换 profile 只改此字段
        pub port: Option<u16>,      // Some(3080)=先试 3080；None=--port 0 让系统挑
        pub cwd: PathBuf,           // 工作目录（dsh 会话按 cwd 归档，必须显式设置）
        pub patches: Vec<PathBuf>,  // --patch 覆盖（扩展预留）
        pub home: Option<PathBuf>,  // DSH_HOME 覆盖（None => ~/.dsh 默认，复用现有 profile/session）
    }

    pub enum DshExec {
        WindowsCmd { shim: PathBuf }, // dsh.cmd（volta 镜像目录，与 node.exe 同目录）
        UnixSh { shim: PathBuf },     // dsh shell 脚本
    }

    /// 解析 dsh 可执行文件：Windows 找 volta 镜像目录里的 dsh.cmd；Linux 找 PATH 上的 dsh
    pub fn resolve_dsh() -> Result<DshExec, DshNotFound>;

    /// 内核状态机
    pub enum KernelState {
        Stopped,
        Starting,
        Ready { url: String, port: u16 },
        Crashed { restarts: u32, last_error: String }, // 10s 内 >=3 次崩溃转此态（止损）
    }

    pub struct Kernel {
        pub config: KernelConfig,
        pub state: KernelState,
        log_path: PathBuf,          // 滚动日志文件（stdout+stderr 都写）
    }

    impl Kernel {
        /// spawn：Windows 用 cmd /c "<shim> web --port N"，Linux 直接 exec shim；cwd=config.cwd
        pub fn spawn(&mut self) -> Result<()>;
        /// 优雅终止子进程（退出托盘“退出”时调用）
        pub fn kill(&mut self) -> Result<()>;
        /// 子进程退出回调：崩溃计数 + 限次重启决策（配合 std::time::Instant 窗口）
        pub fn on_child_exit(&mut self) -> KernelState;
        /// 从 stdout 的 URL 行解析实际端口（--port 0 时必需）；轮询 HTTP 200 由 readiness 做
        pub fn parse_url_line(&self, line: &str) -> Option<u16>;
        /// 诊断文本：dsh 路径 / node 版本 / cwd / port / 最近日志尾部
        pub fn diagnostics(&self) -> String;
    }

### src-tauri/src/readiness.rs — 就绪探测

    pub struct Readiness { pub base_url: String } // http://127.0.0.1:<port>

    impl Readiness {
        /// 轮询 GET base_url 直至 HTTP 200 或超时（默认 ~60s）；返回实际可用 URL
        pub async fn wait_until_ready(&self, timeout: Duration) -> Result<String, ReadinessError>;
    }

### src-tauri/src/state.rs — 持久化应用配置

    #[derive(Serialize, Deserialize)]
    pub struct AppState {
        pub cwd: PathBuf,        // 上次工作目录
        pub preferred_port: u16, // 默认 3080；0=自动
        pub autostart: bool,     // 默认 false
    }

    impl AppState {
        pub fn load() -> Self;                       // 不存在则默认值
        pub fn save(&self) -> Result<()>;
        pub fn config_path() -> PathBuf;             // Win: %APPDATA%\DSH Desktop\config.json
                                                     // Lin: ~/.config/dsh-desktop/config.json
    }

### src-tauri/src/lifecycle.rs — 托盘 / 关窗 / 单实例 / 退出

    /// 完整 7 项托盘：打开主界面 / 切换工作目录 / 重启 dsh 内核 / 复制诊断信息 / 打开日志 / 开机自启(勾选) / 退出
    pub fn build_tray_menu(app: &AppHandle) -> tauri::Result<()>;
    /// 托盘菜单事件分发（含 autostart 勾选态同步）
    pub fn handle_tray_event(app: &AppHandle, id: &str) -> Result<(), String>;
    /// 关窗 = 隐藏到托盘（dsh 继续跑）；Linux 无托盘时同样隐藏、靠图标唤回
    pub fn on_window_close_request(window: &Window, api: CloseRequestApi);
    /// 自启开关包装：Win=注册表 Run 键；Lin=~/.config/autostart/*.desktop（autostart 插件）
    pub fn set_autostart(enabled: bool) -> Result<()>;
    /// 退出：杀 dsh 内核 -> app.exit(0)
    pub fn exit_app(app: &AppHandle);

### IPC（typed commands，给前端壳页面用）

    #[tauri::command] fn get_kernel_status() -> KernelStatus;          // 启动中/就绪/错误+URL
    #[tauri::command] fn set_working_dir(path: String) -> Result<(), String>; // 持久化+重启内核
    #[tauri::command] fn restart_kernel() -> Result<(), String>;
    #[tauri::command] fn get_diagnostics() -> String;
    #[tauri::command] fn open_log_file() -> Result<(), String>;

---

## 4. 关键机制备忘（来自调研）

- **spawn**：dsh web 等价 dsh --profile web。默认 host=127.0.0.1 port=3080（--host 0.0.0.0 被 dsh 官方禁止）。
- **会话按 cwd 归档**：~/.dsh/sessions/--<cwd 编码>--，所以 cwd 必须显式传，不能继承壳的进程目录。
- **DSH_HOME**：默认 ~/.dsh（现有 web profile + 自定义插件 + 会话原样复用），不覆盖。
- **就绪判定**：轮询 http://127.0.0.1:<port> 到 HTTP 200（比解析日志可靠）；--port 0 时先解析 stdout URL 行拿实际端口。
- **崩溃限次**：10s 窗口内 >=3 次崩溃 -> 转错误页（止损，防 崩→起→崩 死循环）。
- **端口冲突**：先试 3080，被占则 --port 0 让系统挑，托盘显示实际地址。
- **不复用外部 dsh 实例**：始终自己 spawn 一个，保证退出能杀干净。

---

## 5. 风险提示（骨架阶段已踩/已规避）

1. **tauri features**：tray-icon 不在 default 里，已显式开启（含 image-png/ico）。若后续去掉托盘改用其他方案，需同步改 Cargo.toml。
2. **pnpm 11 构建脚本审批**：esbuild postinstall 被默认拦截（ERR_PNPM_IGNORED_BUILDS），已通过 pnpm-workspace.yaml allowBuilds 放行；package.json 的 pnpm 字段在 pnpm 11 下会被忽略（勿再往那里写）。
3. **npmmirror 怪癖**：typescript ^5 被解析到 5.0.2（镜像索引问题），当前可用；后续需要新 TS 特性时显式 pin 版本。
4. **首次 cargo build 未跑**（按规格留给集成步骤）；crate 版本为 crates.io 上限预测，实际以 Cargo.lock 为准。
5. **图标为占位纯色方块**（app-icon.png 源文件保留在 src-tauri/icons/），后续替换源图后重跑 pnpm tauri icon 即可。
6. **pnpm dev 依赖 esbuild 二进制**：安装已完成（postinstall Done），vite 可用。
