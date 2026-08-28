//! dsh kernel abstraction — the replaceable core.
//!
//! Pure-std module (no tauri types): everything the desktop shell needs to
//! resolve, spawn, supervise, and diagnose the DeepSeek Harness (dsh)
//! process. KernelConfig parameterises profile / port / cwd / patches /
//! DSH_HOME so future extensions swap behaviour by changing config, not code.

use parking_lot::Mutex;
use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpListener;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
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
    #[allow(dead_code)] // constructed on windows targets only
    WindowsCmd { shim: PathBuf },
    /// dsh shell script on PATH (Linux/macOS). `interpreter` is the shebang
    /// parsed at detect time and resolved to a concrete program + fixed args
    /// (e.g. `["/usr/bin/zsh"]`, `["/home/u/.volta/bin/node"]`), so we never
    /// rely on bash/sh assumptions or on the interpreter being on the app's
    /// PATH at spawn time. `None` = no usable shebang (binary / plain script).
    #[allow(dead_code)] // constructed on unix targets only
    UnixSh {
        shim: PathBuf,
        interpreter: Option<Vec<String>>,
    },
}

/// Raised when no dsh launcher can be resolved (frontend shows the guide).
/// The string carries a human-readable reason when one is known (e.g. the
/// script exists but its shebang interpreter is missing).
#[derive(Debug)]
pub struct DshNotFound(pub String);

impl std::fmt::Display for DshNotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.is_empty() {
            write!(f, "未找到 dsh 可执行文件（dsh.cmd / dsh）")
        } else {
            write!(f, "{}", self.0)
        }
    }
}
impl std::error::Error for DshNotFound {}

/// Resolve the dsh executable.
/// Windows: scan PATH (semicolon-split) for dsh.cmd, then dsh, dsh.ps1, dsh.exe.
/// Linux: two-pass lookup.
///   Pass 1 — the app's own PATH (+ ~/.local/bin): fast, no subprocess.
///   Pass 2 — the user's login-shell PATH (`$SHELL -lic`, so .zshrc/.bashrc
///   volta/nvm/mise entries count): desktop-launched apps never source rc
///   files, so dsh may only be reachable there.
/// Each candidate is validated: the script must exist AND its shebang
/// interpreter (e.g. node/zsh/bash) must resolve to an executable — a script
/// whose interpreter is missing is reported as not-found with a reason.
pub fn resolve_dsh() -> Result<DshExec, DshNotFound> {
    let Some(path_var) = std::env::var_os("PATH") else {
        return Err(DshNotFound(String::new()));
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
        Err(DshNotFound(String::new()))
    }
    #[cfg(not(windows))]
    {
        let app_paths: Vec<PathBuf> = std::env::split_paths(&path_var)
            .map(PathBuf::from)
            .collect();
        // ~/.local/bin: XDG convention, typically in .profile — never in a
        // desktop-launched app's PATH.
        let mut paths = app_paths;
        if let Some(home) = std::env::var_os("HOME") {
            paths.push(PathBuf::from(home).join(".local/bin"));
        }
        let mut first_reason: Option<String> = None;

        // Pass 1: dsh on the app's own PATH.
        for dir in &paths {
            let cand = dir.join("dsh");
            if cand.is_file() {
                match validate_script(&cand, &paths) {
                    Ok(exec) => return Ok(exec),
                    Err(reason) => {
                        first_reason.get_or_insert(reason);
                    }
                }
            }
        }
        // Pass 2: the user's login shell may be the only place dsh (and its
        // interpreter) lives — volta / nvm / mise configured in .zshrc /
        // .bashrc. Probe the shell once, merge its PATH in, retry.
        if let Some(shell_path) = login_shell_path() {
            let mut merged = paths;
            merged.extend(
                std::env::split_paths(std::ffi::OsStr::new(&shell_path)).map(PathBuf::from),
            );
            for dir in &merged {
                let cand = dir.join("dsh");
                if cand.is_file() {
                    match validate_script(&cand, &merged) {
                        Ok(exec) => return Ok(exec),
                        Err(reason) => {
                            first_reason.get_or_insert(reason);
                        }
                    }
                }
            }
        }
        Err(DshNotFound(first_reason.unwrap_or_default()))
    }
}

/// Verify a found `dsh` script is actually runnable: its shebang parses and
/// the interpreter resolves against `lookup`. Scripts without a shebang
/// (binaries, plain scripts) pass through with `interpreter: None`.
#[cfg(not(windows))]
fn validate_script(shim: &Path, lookup: &[PathBuf]) -> Result<DshExec, String> {
    let interpreter = match parse_shebang(shim, lookup) {
        Some(Ok(v)) => Some(v),
        Some(Err(reason)) => return Err(reason),
        None => None,
    };
    Ok(DshExec::UnixSh {
        shim: shim.to_path_buf(),
        interpreter,
    })
}

/// Parse a script's shebang into a concrete interpreter invocation.
/// `lookup` resolves `/usr/bin/env NAME` (and bare relative names).
/// `None` = no shebang at all. `Some(Err)` = malformed / unresolvable.
#[cfg(not(windows))]
fn parse_shebang(shim: &Path, lookup: &[PathBuf]) -> Option<Result<Vec<String>, String>> {
    let mut f = File::open(shim).ok()?;
    let mut head = [0u8; 4096];
    let n = f.read(&mut head).ok()?;
    let Some(idx) = head[..n].iter().position(|&b| b == b'\n') else {
        return Some(Err("dsh 脚本首行无换行，无法解析解释器".into()));
    };
    let line = String::from_utf8_lossy(&head[..idx]);
    let rest = line.trim_start();
    if !rest.starts_with("#!") {
        return None; // binary / plain script
    }
    let mut toks = rest[2..].split_whitespace();
    let Some(prog) = toks.next() else {
        return Some(Err("dsh 脚本的 shebang 为空".into()));
    };
    let args: Vec<String> = toks.map(str::to_string).collect();
    let mut all: Vec<String> = Vec::new();
    if prog == "/usr/bin/env" || prog == "env" {
        // `env [-S] NAME [ARGS…]` — resolve NAME against the search paths.
        let mut it = args.into_iter();
        let first = it.next().unwrap_or_default();
        let (name, rest) = if first == "-S" {
            match it.next() {
                Some(n) => (n, it.collect::<Vec<_>>()),
                None => return Some(Err("dsh 的 shebang `env -S` 缺少命令名".into())),
            }
        } else {
            (first, it.collect::<Vec<_>>())
        };
        match lookup_in_paths(&name, lookup) {
            Some(path) => {
                all.push(path);
                all.extend(rest);
            }
            None => return Some(Err(format!("找到 dsh 脚本，但其解释器 `{name}` 不在 PATH"))),
        }
    } else if prog.contains('/') {
        let p = PathBuf::from(prog);
        if !is_executable(&p) {
            return Some(Err(format!("dsh 脚本的解释器 {prog} 不存在或不可执行")));
        }
        all.push(prog.to_string());
        all.extend(args);
    } else {
        match lookup_in_paths(prog, lookup) {
            Some(path) => {
                all.push(path);
                all.extend(args);
            }
            None => return Some(Err(format!("dsh 脚本的解释器 `{prog}` 不在 PATH"))),
        }
    }
    Some(Ok(all))
}

/// First executable `name` found in `paths`.
#[cfg(not(windows))]
fn lookup_in_paths(name: &str, paths: &[PathBuf]) -> Option<String> {
    for dir in paths {
        let c = dir.join(name);
        if c.is_file() && is_executable(&c) {
            return Some(c.to_string_lossy().into_owned());
        }
    }
    None
}

#[cfg(not(windows))]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(p)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// User's login shell (or a sane fallback).
#[cfg(not(windows))]
fn login_shell() -> Option<PathBuf> {
    std::env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .or_else(|| {
            ["/bin/bash", "/bin/zsh", "/bin/sh"]
                .into_iter()
                .map(PathBuf::from)
                .find(|p| p.is_file())
        })
}

/// Run one command in the user's shell; returns the last stdout line.
/// `flags` like "-lic" (login + interactive, so .zshrc/.bashrc run and
/// contribute PATH); stdin is null so prompt-y rc code hits EOF instead of
/// hanging. Killed after a 1.5s budget.
#[cfg(not(windows))]
fn shell_probe(shell: &Path, flags: &str, cmd: &str) -> Option<String> {
    let mut child = Command::new(shell)
        .args([flags, cmd])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_millis(1500);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    // rc files may print noise first; our command's output is last and has no
    // trailing newline (printf %s), so the last line is exactly what we asked.
    let last = out.rsplit('\n').next().unwrap_or("").trim().to_string();
    if last.is_empty() {
        None
    } else {
        Some(last)
    }
}

/// PATH as seen by the user's login shell (rc-file contributions included).
#[cfg(not(windows))]
fn login_shell_path() -> Option<String> {
    let shell = login_shell()?;
    shell_probe(&shell, "-lic", "printf %s \"$PATH\"")
        .or_else(|| shell_probe(&shell, "-lc", "printf %s \"$PATH\""))
        .or_else(|| shell_probe(&shell, "-c", "printf %s \"$PATH\""))
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
    /// The authenticated dsh web URL captured from the `dsh web:` startup line
    /// (carrying the per-process launch token), when the managed dsh kernel has
    /// printed one. Written by the log readers, read by the readiness loop.
    pub(crate) auth_url: Arc<Mutex<Option<String>>>,
    exec: Option<DshExec>,
    /// Why detection failed (DshNotFound reason), for diagnostics; None when
    /// dsh is available or detection has not run.
    pub(crate) exec_error: Option<String>,
    pub(crate) child: Option<Child>,
    /// Port actually handed to dsh (resolved at spawn time).
    pub(crate) current_port: Option<u16>,
    /// Whether this kernel is attached to an externally running dsh process
    pub is_attached: bool,
    pub(crate) ready_emitted: bool,
    pub(crate) timeout_emitted: bool,
    pub(crate) ready_deadline: Option<Instant>,
    crash_times: VecDeque<Instant>,
    restarts: u32,
    pub(crate) last_exit_status: Option<ExitStatus>,
}

impl Kernel {
    pub fn new(config: KernelConfig, log_path: PathBuf) -> Self {
        let (exec, exec_error) = match resolve_dsh() {
            Ok(e) => (Some(e), None),
            Err(e) => (None, Some(e.to_string())),
        };
        let log_writer = Arc::new(Mutex::new(LogWriter::open(&log_path)));
        Self {
            config,
            state: KernelState::Stopped,
            log_path,
            log_writer,
            auth_url: Arc::new(Mutex::new(None)),
            exec,
            exec_error,
            child: None,
            current_port: None,
            is_attached: false,
            ready_emitted: false,
            timeout_emitted: false,
            ready_deadline: None,
            crash_times: VecDeque::new(),
            restarts: 0,
            last_exit_status: None,
        }
    }

    /// Attach to an existing running dsh process (external kernel).
    pub fn attach_to_existing(&mut self, port: u16, url: String) {
        self.is_attached = true;
        self.current_port = Some(port);
        self.state = KernelState::Ready {
            url: url.clone(),
            port,
        };
        self.ready_emitted = true;
        self.timeout_emitted = false;
        let mut w = self.log_writer.lock();
        let _ = w.write_line(&format!(
            "=== WhaleNest attached to existing dsh process at {} (external kernel) ===",
            url
        ));
    }

    pub fn dsh_available(&self) -> bool {
        self.exec.is_some()
    }

    /// Re-run PATH resolution and refresh the cached executable.
    /// `Kernel::new` resolves only once; the guide view's 重新检测 / 一键安装
    /// flows call this (via restart_kernel_impl) to pick up a newly installed
    /// dsh without an app restart. Returns true when a launcher is available.
    pub fn redetect(&mut self) -> bool {
        match resolve_dsh() {
            Ok(e) => {
                self.exec = Some(e);
                self.exec_error = None;
                true
            }
            Err(e) => {
                self.exec = None;
                self.exec_error = Some(e.to_string());
                false
            }
        }
    }

    pub fn log_path(&self) -> &Path {
        &self.log_path
    }

    /// The authenticated dsh web URL (with launch token) captured from dsh's
    /// startup line, if one has been announced for the current process.
    pub fn auth_url(&self) -> Option<String> {
        self.auth_url.lock().clone()
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
        // The dsh shim may spawn node and other descendants. Put the entire
        // launch chain in its own process group so Unix shutdown can reap the
        // whole tree instead of leaving the web server bound to its old port.
        #[cfg(unix)]
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        if let Some(home) = &self.config.home {
            command.env("DSH_HOME", home);
        }
        command.stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = command
            .spawn()
            .map_err(|e| format!("spawn dsh 失败: {e}"))?;

        let writer = self.log_writer.clone();
        let auth_url = self.auth_url.clone();
        // Reset the captured URL on every spawn: a restart mints a fresh
        // launch token, so a stale authenticated URL must not be reused.
        *auth_url.lock() = None;
        if let Some(out) = child.stdout.take() {
            spawn_log_reader(out, writer.clone(), auth_url.clone(), "stdout");
        }
        if let Some(err) = child.stderr.take() {
            spawn_log_reader(err, writer.clone(), auth_url.clone(), "stderr");
        }
        let mut w = self.log_writer.lock();
        let _ = w.write_line(&format!(
            "=== dsh kernel start: profile={}, port={}, cwd={} ===",
            self.config.profile,
            port,
            self.config.cwd.display()
        ));

        self.child = Some(child);
        self.state = KernelState::Starting;
        self.ready_emitted = false;
        self.timeout_emitted = false;
        self.ready_deadline = Some(Instant::now() + READY_TIMEOUT);
        Ok(())
    }

    /// Graceful-ish stop: kill the child and reap it. Called from restart/quit.
    pub fn kill(&mut self) -> Result<(), String> {
        if self.is_attached {
            let mut w = self.log_writer.lock();
            let _ = w.write_line(
                "=== WhaleNest detached from external dsh kernel (kernel left running) ===",
            );
            self.state = KernelState::Stopped;
            return Ok(());
        }
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
            #[cfg(unix)]
            {
                // Negative PID targets the process group created in spawn().
                // Ignore ESRCH: the group may already have exited naturally.
                let _ = unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) };
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        let mut w = self.log_writer.lock();
        let _ = w.write_line("=== dsh kernel stopped ===");
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
                DshExec::WindowsCmd { shim } | DshExec::UnixSh { shim, .. } => {
                    shim.display().to_string()
                }
            })
            .unwrap_or_else(|| "未找到".to_string());
        s.push_str(&format!("dsh 可执行文件: {dsh}\n"));
        if let Some(err) = &self.exec_error {
            s.push_str(&format!("dsh 探测详情: {err}\n"));
        }
        if self.is_attached {
            s.push_str("运行模式: 对接现有 dsh 进程 (外部内核 - 重启/退出保护生效)\n");
        } else {
            s.push_str("运行模式: 独立托管内核 (子进程)\n");
        }
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
            DshExec::WindowsCmd { shim } | DshExec::UnixSh { shim, .. } => {
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
            "--no-open",
            "--port",
            port,
        ]);
        c
    } else {
        let mut c = Command::new("cmd");
        c.args(["/C", shim_str.as_str(), "web", "--no-open", "--port", port]);
        c
    }
}

#[cfg(not(windows))]
fn build_unix_command(exec: &DshExec, port: &str) -> Command {
    let DshExec::UnixSh { shim, interpreter } = exec else {
        unreachable!("unix resolve_dsh always returns UnixSh")
    };
    if let Some(v) = interpreter {
        // Run through the script's declared interpreter, resolved to a
        // concrete path at detect time. Deterministic and correct whether the
        // script is bash/zsh/node flavoured, and independent of the
        // interpreter being on the app's PATH at spawn time.
        let mut c = Command::new(&v[0]);
        if v.len() > 1 {
            c.args(&v[1..]);
        }
        c.arg(shim).args(["web", "--no-open", "--port", port]);
        return c;
    }
    // No shebang: a real binary → exec directly; otherwise sh fallback.
    if is_executable(shim) {
        let mut c = Command::new(shim);
        c.args(["web", "--no-open", "--port", port]);
        c
    } else {
        let mut c = Command::new("sh");
        c.arg(shim).args(["web", "--no-open", "--port", port]);
        c
    }
}

/// Stream a child pipe (stdout/stderr) into the shared rolling log.
/// When a line carries dsh's own `dsh web: <url>` announcement, the
/// authenticated URL (with its per-process launch token) is captured into
/// `auth_url` so the readiness loop can emit the token-bearing Ready URL.
fn spawn_log_reader<R: Read + Send + 'static>(
    reader: R,
    writer: Arc<Mutex<LogWriter>>,
    auth_url: Arc<Mutex<Option<String>>>,
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
                        // Capture dsh's own startup URL line. It always embeds
                        // the process launch token, which the WebView needs to
                        // authenticate on first load. Note the `dsh web:` prefix
                        // is printed even with `--no-open` (printUrl defaults on).
                        if let Some(url) = extract_dsh_web_url(trimmed) {
                            *auth_url.lock() = Some(url);
                        }
                        let mut w = writer.lock();
                        let _ = w.write_line(&format!("[{tag}] {trimmed}"));
                    }
                }
            }
        }
    });
}

/// Pull the authenticated dsh web URL out of a `dsh web: http://…?token=…`
/// startup line. Returns `None` for any other line (or a parenthesised LAN
/// suffix, which is intentionally ignored: the loopback URL is the one the
/// local WebView must use).
fn extract_dsh_web_url(line: &str) -> Option<String> {
    let idx = line.find("dsh web:")?;
    let rest = &line[idx + "dsh web:".len()..];
    let url = rest.trim().split_whitespace().next()?;
    if url.starts_with("http://") || url.starts_with("https://") {
        Some(url.to_string())
    } else {
        None
    }
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

/// Probe a specific port to check if an active DeepSeek Harness (dsh) web server is running.
pub fn probe_dsh_at_port(port: u16) -> Option<String> {
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpStream};
    use std::time::Duration;

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(300)).ok()?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(600)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(300)));

    let req = format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).ok()?;

    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf[..n]);

    if text.contains("HTTP/1.1 200")
        || text.contains("HTTP/1.0 200")
        || text.contains("HTTP/1.1 304")
    {
        let lower = text.to_ascii_lowercase();
        if lower.contains("__moduleloader__")
            || lower.contains("__dsh_boot__")
            || lower.contains("deepseek")
            || lower.contains("dsh")
            || (lower.contains("content-type: text/html") && lower.contains("<!doctype html>"))
        {
            return Some(format!("http://127.0.0.1:{port}"));
        }
    }
    None
}

/// Detect if any existing dsh server is already running on candidate ports.
/// Priority: preferred_port first, then standard range [3080, 3081, 3082, 3083, 3084, 3085].
pub fn detect_existing_dsh(preferred_port: Option<u16>) -> Option<(u16, String)> {
    let mut candidate_ports = Vec::new();
    if let Some(p) = preferred_port {
        if p > 0 {
            candidate_ports.push(p);
        }
    }
    for p in [3080, 3081, 3082, 3083, 3084, 3085] {
        if !candidate_ports.contains(&p) {
            candidate_ports.push(p);
        }
    }

    for port in candidate_ports {
        if let Some(url) = probe_dsh_at_port(port) {
            return Some((port, url));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_plain_loopback_url() {
        let line = "dsh web: http://127.0.0.1:3987/?token=abcDEF123_-";
        assert_eq!(
            extract_dsh_web_url(line).as_deref(),
            Some("http://127.0.0.1:3987/?token=abcDEF123_-")
        );
    }

    #[test]
    fn extracts_url_from_line_with_lan_suffix() {
        let line =
            "dsh web: http://127.0.0.1:3080/?token=xyz (LAN: http://192.168.1.5:3080/?token=xyz)";
        assert_eq!(
            extract_dsh_web_url(line).as_deref(),
            Some("http://127.0.0.1:3080/?token=xyz")
        );
    }

    #[test]
    fn ignores_other_lines() {
        assert_eq!(extract_dsh_web_url("=== dsh kernel start ==="), None);
        assert_eq!(extract_dsh_web_url("some unrelated stdout line"), None);
        assert_eq!(extract_dsh_web_url(""), None);
    }

    #[test]
    fn ignores_malformed_or_unknown_scheme() {
        assert_eq!(extract_dsh_web_url("dsh web: ftp://127.0.0.1/x"), None);
        assert_eq!(extract_dsh_web_url("dsh web:"), None);
    }
}
