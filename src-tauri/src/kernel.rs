//! dsh kernel abstraction — the replaceable core.
//!
//! Pure-std module (no tauri types): everything the desktop shell needs to
//! resolve, spawn, supervise, and diagnose the DeepSeek Harness (dsh)
//! process. KernelConfig parameterises profile / port / cwd / patches /
//! DSH_HOME so future extensions swap behaviour by changing config, not code.

use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Crash-stop window: N abnormal exits inside this window trips the brake.
pub const CRASH_WINDOW: Duration = Duration::from_secs(10);
/// Abnormal exits allowed inside the crash window before stopping.
pub const MAX_CRASHES: usize = 3;
/// How long we wait for the dsh web server to accept connections.
pub const READY_TIMEOUT: Duration = Duration::from_secs(60);
/// Single log file cap before rotating to <name>.log.1
const MAX_LOG_SIZE: u64 = 1024 * 1024;
/// CREATE_NO_WINDOW — keep the spawned console from flashing on Windows.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Kernel launch parameters; profile / port / cwd / patches are all
/// parameterised so extensions only touch this struct.
#[derive(Clone, Debug)]
pub struct KernelConfig {
    /// v1 is fixed to "web"; booting another profile only changes this field.
    pub profile: String,
    /// Some(3080) = try 3080 first, fall back to a free port; None = pick a
    /// free port directly (the equivalent of dsh --port 0).
    pub port: Option<u16>,
    /// Working directory. dsh archives sessions by cwd, so this is mandatory.
    pub cwd: PathBuf,
    /// Extra --patch overlays (extension seam, empty in v1).
    #[allow(dead_code)]
    pub patches: Vec<PathBuf>,
    /// DSH_HOME override; None => default ~/.dsh (reuses existing profile).
    pub home: Option<PathBuf>,
    /// When true, the preferred port is mandatory: if it is taken at spawn
    /// time, startup fails with a clear error instead of drifting to a random
    /// port. Default false (drift allowed).
    #[allow(dead_code)]
    pub lock_port: bool,
}

/// Resolved dsh launcher, platform-specific.
#[derive(Clone, Debug)]
pub enum DshExec {
    /// dsh.cmd (volta image dir, sibling of node.exe); .ps1 handled too.
    WindowsCmd { shim: PathBuf },
    /// dsh shell script on PATH (Linux/macOS).
    #[allow(dead_code)] // constructed on unix targets only
    UnixSh { shim: PathBuf },
}

/// Raised when no dsh launcher can be resolved (frontend shows the guide).
#[derive(Debug)]
pub struct DshNotFound;

impl std::fmt::Display for DshNotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "未找到 dsh 可执行文件（dsh.cmd / dsh）")
    }
}
impl std::error::Error for DshNotFound {}

/// Resolve the dsh executable.
/// Windows: scan PATH (semicolon-split) for dsh.cmd, then dsh, dsh.ps1, dsh.exe.
/// Linux: scan PATH (colon-split) for the dsh shell script.
pub fn resolve_dsh() -> Result<DshExec, DshNotFound> {
    let Some(path_var) = std::env::var_os("PATH") else {
        return Err(DshNotFound);
    };
    #[cfg(windows)]
    {
        const NAMES: [&str; 4] = ["dsh.cmd", "dsh", "dsh.ps1", "dsh.exe"];
        for dir in std::env::split_paths(&path_var) {
            for name in NAMES {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Ok(DshExec::WindowsCmd { shim: candidate });
                }
            }
        }
        Err(DshNotFound)
    }
    #[cfg(not(windows))]
    {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join("dsh");
            if candidate.is_file() {
                return Ok(DshExec::UnixSh { shim: candidate });
            }
        }
        Err(DshNotFound)
    }
}

/// Kernel state machine.
#[derive(Clone, Debug)]
pub enum KernelState {
    /// Not running (initial, or after an intentional stop).
    Stopped,
    /// Spawned, waiting for the web server to accept connections.
    Starting,
    /// Web server is up; url is the actual reachable address.
    Ready {
        url: String,
        #[allow(dead_code)]
        port: u16,
    },
    /// >= MAX_CRASHES abnormal exits inside CRASH_WINDOW — brake engaged.
    Crashed {
        #[allow(dead_code)]
        restarts: u32,
        last_error: String,
    },
}

/// IPC-facing status mirror of the kernel (event payload + get_state).
#[derive(Clone, Debug)]
pub enum KernelStatus {
    Guide,
    Starting,
    Ready { url: String },
    Error { message: String },
}

/// Small, always-constructible rolling log writer (stdout+stderr both land here).
struct LogWriter {
    file: Option<File>,
    path: PathBuf,
    size: u64,
}

impl LogWriter {
    fn open(path: &Path) -> Self {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let file = OpenOptions::new().create(true).append(true).open(path).ok();
        let size = file
            .as_ref()
            .and_then(|f| f.metadata().ok())
            .map(|m| m.len())
            .unwrap_or(0);
        Self {
            file,
            path: path.to_path_buf(),
            size,
        }
    }

    fn write_line(&mut self, line: &str) -> io::Result<()> {
        if self.size + line.len() as u64 + 1 > MAX_LOG_SIZE {
            self.rotate()?;
        }
        let Some(file) = self.file.as_mut() else {
            return Ok(());
        };
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        self.size += line.len() as u64 + 1;
        let _ = file.flush();
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        if let Some(file) = self.file.take() {
            drop(file); // must close before renaming on Windows
        }
        let backup = self.path.with_extension("log.1");
        let _ = fs::remove_file(&backup);
        let _ = fs::rename(&self.path, &backup);
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .ok();
        self.size = 0;
        Ok(())
    }
}

/// The kernel supervisor: owns the child process, its log, and crash bookkeeping.
pub struct Kernel {
    pub config: KernelConfig,
    pub state: KernelState,
    log_path: PathBuf,
    log_writer: Arc<Mutex<LogWriter>>,
    exec: Option<DshExec>,
    pub(crate) child: Option<Child>,
    /// Port actually handed to dsh (resolved at spawn time).
    pub(crate) current_port: Option<u16>,
    pub(crate) ready_emitted: bool,
    pub(crate) timeout_emitted: bool,
    pub(crate) ready_deadline: Option<Instant>,
    crash_times: VecDeque<Instant>,
    restarts: u32,
    pub(crate) last_exit_status: Option<ExitStatus>,
}

impl Kernel {
    pub fn new(config: KernelConfig, log_path: PathBuf) -> Self {
        let exec = resolve_dsh().ok();
        let log_writer = Arc::new(Mutex::new(LogWriter::open(&log_path)));
        Self {
            config,
            state: KernelState::Stopped,
            log_path,
            log_writer,
            exec,
            child: None,
            current_port: None,
            ready_emitted: false,
            timeout_emitted: false,
            ready_deadline: None,
            crash_times: VecDeque::new(),
            restarts: 0,
            last_exit_status: None,
        }
    }

    pub fn dsh_available(&self) -> bool {
        self.exec.is_some()
    }

    pub fn log_path(&self) -> &Path {
        &self.log_path
    }

    /// Current port actually bound (None while not starting).
    pub fn current_port(&self) -> Option<u16> {
        self.current_port
    }

    /// Remaining readiness timeout while Starting (None when not applicable).
    pub fn remaining_ready_timeout(&self) -> Option<Duration> {
        if self.ready_emitted
            || self.timeout_emitted
            || !matches!(self.state, KernelState::Starting)
        {
            return None;
        }
        self.ready_deadline
            .map(|d| d.saturating_duration_since(Instant::now()))
    }

    /// Spawn the dsh web process per platform, redirect stdout/stderr to the log.
    /// Windows: cmd /C "<shim> web --port N" with CREATE_NO_WINDOW (a .ps1 shim
    /// goes through powershell -File instead). Linux: exec the shell script.
    pub fn spawn(&mut self) -> Result<(), String> {
        let exec = self
            .exec
            .as_ref()
            .ok_or_else(|| "未找到 dsh 可执行文件".to_string())?;
        let port = self.resolve_port()?;
        self.current_port = Some(port);
        let port_str = port.to_string();

        #[cfg(windows)]
        let mut command = {
            use std::os::windows::process::CommandExt;
            let mut c = build_windows_command(exec, &port_str);
            c.creation_flags(CREATE_NO_WINDOW);
            c
        };
        #[cfg(not(windows))]
        let mut command = build_unix_command(exec, &port_str);

        command.current_dir(&self.config.cwd);
        if let Some(home) = &self.config.home {
            command.env("DSH_HOME", home);
        }
        command.stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = command
            .spawn()
            .map_err(|e| format!("spawn dsh 失败: {e}"))?;

        let writer = self.log_writer.clone();
        if let Some(out) = child.stdout.take() {
            spawn_log_reader(out, writer.clone(), "stdout");
        }
        if let Some(err) = child.stderr.take() {
            spawn_log_reader(err, writer.clone(), "stderr");
        }
        if let Ok(mut w) = self.log_writer.lock() {
            let _ = w.write_line(&format!(
                "=== dsh kernel start: profile={}, port={}, cwd={} ===",
                self.config.profile,
                port,
                self.config.cwd.display()
            ));
        }

        self.child = Some(child);
        self.state = KernelState::Starting;
        self.ready_emitted = false;
        self.timeout_emitted = false;
        self.ready_deadline = Some(Instant::now() + READY_TIMEOUT);
        Ok(())
    }

    /// Graceful-ish stop: kill the child and reap it. Called from restart/quit.
    pub fn kill(&mut self) -> Result<(), String> {
        if let Some(mut child) = self.child.take() {
            // Windows: the spawn chain is cmd /C <dsh.cmd> web --port N, and
            // dsh.cmd launches node.exe as its own child. child.kill() only
            // terminates the cmd.exe wrapper, orphaning node.exe which keeps
            // the port bound (port drift + orphan accumulation on restart/quit).
            // Kill the whole tree with taskkill first; child.kill()/wait()
            // below stay as a harmless fallback once the tree is gone.
            #[cfg(windows)]
            {
                let pid = child.id();
                use std::os::windows::process::CommandExt;
                let _ = Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/T", "/F"])
                    .creation_flags(CREATE_NO_WINDOW)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Ok(mut w) = self.log_writer.lock() {
            let _ = w.write_line("=== dsh kernel stopped ===");
        }
        self.state = KernelState::Stopped;
        Ok(())
    }

    /// Child-exit callback: crash accounting + bounded auto-restart.
    /// Caller sets last_exit_status before invoking.
    pub fn on_child_exit(&mut self) -> KernelState {
        let now = Instant::now();
        while self
            .crash_times
            .front()
            .is_some_and(|t| now.duration_since(*t) > CRASH_WINDOW)
        {
            self.crash_times.pop_front();
        }
        self.crash_times.push_back(now);
        self.restarts = self.crash_times.len() as u32;

        if self.restarts >= MAX_CRASHES as u32 {
            let last = self
                .last_exit_status
                .as_ref()
                .map(|s| format!("{s:?}"))
                .unwrap_or_else(|| "未知".to_string());
            let msg = format!(
                "dsh 内核在 {CRASH_WINDOW:?} 内异常退出 {} 次（最近退出状态: {last}），已停止自动重启",
                self.restarts
            );
            self.state = KernelState::Crashed {
                restarts: self.restarts,
                last_error: msg,
            };
        } else if let Err(e) = self.spawn() {
            let msg = format!("dsh 内核自动重启失败: {e}");
            self.state = KernelState::Crashed {
                restarts: self.restarts,
                last_error: msg,
            };
        } else {
            self.state = KernelState::Starting;
        }
        self.state.clone()
    }

    /// Parse the actual port from dsh's printed URL line
    /// (http://127.0.0.1:PORT / http://localhost:PORT). Reserved for the
    /// --port 0 path; v1 always pre-picks the port, so this stays unused.
    #[allow(dead_code)]
    pub fn parse_url_line(&self, line: &str) -> Option<u16> {
        for needle in ["127.0.0.1:", "localhost:"] {
            if let Some(idx) = line.find(needle) {
                let rest = &line[idx + needle.len()..];
                let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                if let Ok(p) = digits.parse::<u16>() {
                    if p > 0 {
                        return Some(p);
                    }
                }
            }
        }
        None
    }

    /// Diagnostic dump: dsh path, node version, cwd, ports, log tail.
    pub fn diagnostics(&self) -> String {
        let mut s = String::new();
        s.push_str("WhaleNest 诊断信息\n");
        s.push_str("====================\n");
        let dsh = self
            .exec
            .as_ref()
            .map(|e| match e {
                DshExec::WindowsCmd { shim } | DshExec::UnixSh { shim } => {
                    shim.display().to_string()
                }
            })
            .unwrap_or_else(|| "未找到".to_string());
        s.push_str(&format!("dsh 可执行文件: {dsh}\n"));
        s.push_str(&format!("内核状态: {:?}\n", self.state));
        s.push_str(&format!("profile: {}\n", self.config.profile));
        s.push_str(&format!("工作目录: {}\n", self.config.cwd.display()));
        s.push_str(&format!("配置端口: {:?}\n", self.config.port));
        s.push_str(&format!("当前端口: {:?}\n", self.current_port));
        s.push_str(&format!("日志文件: {}\n", self.log_path.display()));
        if let Some(node) = self.node_version() {
            s.push_str(&format!("node 版本: {node}\n"));
        }
        s.push_str("\n--- 最近日志 ---\n");
        s.push_str(&read_log_tail(&self.log_path, 40));
        s
    }

    fn node_version(&self) -> Option<String> {
        let dir = match self.exec.as_ref()? {
            DshExec::WindowsCmd { shim } | DshExec::UnixSh { shim } => {
                shim.parent().map(|p| p.to_path_buf())?
            }
        };
        #[cfg(windows)]
        let node = dir.join("node.exe");
        #[cfg(not(windows))]
        let node = dir.join("node");
        if !node.is_file() {
            return None;
        }
        let mut child = Command::new(&node)
            .arg("--version")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        return None;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(_) => return None,
            }
        }
        let mut out = child.stdout.take()?;
        let mut buf = String::new();
        let _ = out.read_to_string(&mut buf);
        let v = buf.trim().to_string();
        if v.is_empty() {
            None
        } else {
            Some(v)
        }
    }

    fn resolve_port(&self) -> Result<u16, String> {
        match self.config.port {
            Some(p) => {
                if TcpListener::bind(("127.0.0.1", p)).is_ok() {
                    Ok(p)
                } else if self.config.lock_port {
                    Err(format!(
                        "端口 {p} 已被占用（已锁定固定端口），请先释放该端口或关闭「固定端口」后重试"
                    ))
                } else {
                    Ok(Self::pick_free_port())
                }
            }
            None => Ok(Self::pick_free_port()),
        }
    }

    fn pick_free_port() -> u16 {
        TcpListener::bind(("127.0.0.1", 0))
            .and_then(|l| l.local_addr())
            .map(|a| a.port())
            .unwrap_or(3080)
    }
}

#[cfg(windows)]
fn build_windows_command(exec: &DshExec, port: &str) -> Command {
    let DshExec::WindowsCmd { shim } = exec else {
        unreachable!("windows resolve_dsh always returns WindowsCmd")
    };
    let shim_str = shim.to_string_lossy().into_owned();
    if shim.extension().and_then(|e| e.to_str()) == Some("ps1") {
        let mut c = Command::new("powershell");
        c.args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            shim_str.as_str(),
            "web",
            "--port",
            port,
        ]);
        c
    } else {
        let mut c = Command::new("cmd");
        c.args(["/C", shim_str.as_str(), "web", "--port", port]);
        c
    }
}

#[cfg(not(windows))]
fn build_unix_command(exec: &DshExec, port: &str) -> Command {
    let DshExec::UnixSh { shim } = exec else {
        unreachable!("unix resolve_dsh always returns UnixSh")
    };
    use std::os::unix::fs::PermissionsExt;
    let is_exec = fs::metadata(shim)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false);
    if is_exec {
        let mut c = Command::new(shim);
        c.args(["web", "--port", port]);
        c
    } else {
        let mut c = Command::new("sh");
        c.arg(shim).args(["web", "--port", port]);
        c
    }
}

/// Stream a child pipe (stdout/stderr) into the shared rolling log.
fn spawn_log_reader<R: Read + Send + 'static>(
    reader: R,
    writer: Arc<Mutex<LogWriter>>,
    tag: &'static str,
) {
    std::thread::spawn(move || {
        let mut buf = BufReader::new(reader);
        let mut line = String::new();
        loop {
            line.clear();
            match buf.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let trimmed = line.trim_end();
                    if !trimmed.is_empty() {
                        if let Ok(mut w) = writer.lock() {
                            let _ = w.write_line(&format!("[{tag}] {trimmed}"));
                        }
                    }
                }
            }
        }
    });
}

fn read_log_tail(path: &Path, n: usize) -> String {
    let Ok(data) = fs::read(path) else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&data);
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}
