//! Child-stderr relay: line-prefixed with the server name, under a rate cap. The child's stdout is the MCP protocol channel and is never touched.

use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

/// Stderr relay rate cap: at most this many lines per relay window.
pub const STDERR_RELAY_MAX_LINES: usize = 10;

/// The relay window the stderr rate cap applies to.
pub const STDERR_RELAY_WINDOW: Duration = Duration::from_secs(1);

/// A rate-cap admission decision for one stderr line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    Relay,
    Drop,
    /// The relay window rolled over after drops; report the count once.
    WindowReset {
        dropped: usize,
    },
}

/// The stderr relay rate cap: at most `max_per_window` lines per
/// `window`, dropping the rest. Time is injected so tests are
/// deterministic.
pub struct StderrRateCap {
    max_per_window: usize,
    window: Duration,
    window_start: std::time::Instant,
    relayed: usize,
    dropped: usize,
}

impl StderrRateCap {
    pub fn new(max_per_window: usize, window: Duration) -> Self {
        Self {
            max_per_window,
            window,
            window_start: std::time::Instant::now(),
            relayed: 0,
            dropped: 0,
        }
    }

    pub fn admit(&mut self, now: std::time::Instant) -> Admission {
        if now.duration_since(self.window_start) >= self.window {
            let dropped = std::mem::take(&mut self.dropped);
            self.window_start = now;
            self.relayed = 0;
            if dropped > 0 {
                return Admission::WindowReset { dropped };
            }
        }
        if self.relayed < self.max_per_window {
            self.relayed += 1;
            Admission::Relay
        } else {
            self.dropped += 1;
            Admission::Drop
        }
    }
}

/// Relays child stderr to `out` (tackle's stderr in production) with
/// each line prefixed by the server name under the rate cap. The
/// child's stdout is never seen here: it is the MCP protocol channel.
pub async fn relay_stderr<S, W>(
    server: &str,
    stderr: S,
    out: &mut W,
    cap: &mut StderrRateCap,
) -> std::io::Result<()>
where
    S: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut lines = BufReader::new(stderr).lines();
    while let Some(line) = lines.next_line().await? {
        match cap.admit(std::time::Instant::now()) {
            Admission::Relay => {
                out.write_all(format!("mcp[{server}] {line}\n").as_bytes())
                    .await?;
            }
            Admission::WindowReset { dropped } => {
                out.write_all(
                    format!("mcp[{server}] [rate cap: dropped {dropped} stderr lines]\n")
                        .as_bytes(),
                )
                .await?;
                out.write_all(format!("mcp[{server}] {line}\n").as_bytes())
                    .await?;
            }
            Admission::Drop => {}
        }
    }
    Ok(())
}
