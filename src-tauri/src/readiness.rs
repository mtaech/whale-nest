//! Readiness probing: poll the dsh web server until it accepts TCP connections.

use std::io;
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use tokio::time::sleep;

/// Why readiness probing failed.
#[derive(Debug)]
pub enum ReadinessError {
    /// No connection within the given timeout.
    Timeout(Duration),
    /// A probe failed for an I/O reason other than "not ready yet".
    #[allow(dead_code)]
    Io(io::Error),
}

impl std::fmt::Display for ReadinessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadinessError::Timeout(d) => write!(f, "等待 dsh 就绪超时（{}s）", d.as_secs()),
            ReadinessError::Io(e) => write!(f, "探测 dsh 端口失败: {e}"),
        }
    }
}
impl std::error::Error for ReadinessError {}

/// Probes `http://127.0.0.1:<port>` until the port accepts connections.
pub struct Readiness {
    /// e.g. "http://127.0.0.1:3080" — the url returned on success.
    pub base_url: String,
}

impl Readiness {
    pub fn new(base_url: String) -> Self {
        Self { base_url }
    }

    fn port(&self) -> u16 {
        self.base_url
            .rsplit(':')
            .next()
            .and_then(|s| s.parse::<u16>().ok())
            .filter(|p| *p > 0)
            .unwrap_or(3080)
    }

    /// Poll with TcpStream::connect every ~300ms until connected or timeout.
    /// Returns the actual reachable URL on success.
    pub async fn wait_until_ready(&self, timeout: Duration) -> Result<String, ReadinessError> {
        let started = Instant::now();
        let addr = SocketAddr::from(([127, 0, 0, 1], self.port()));
        loop {
            match TcpStream::connect_timeout(&addr, Duration::from_millis(500)) {
                Ok(_) => return Ok(self.base_url.clone()),
                Err(e) => {
                    if started.elapsed() >= timeout {
                        return Err(ReadinessError::Timeout(timeout));
                    }
                    // ECONNREFUSED while the server is still booting: keep polling.
                    let _ = e;
                }
            }
            sleep(Duration::from_millis(300)).await;
        }
    }
}
