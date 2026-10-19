//! Child-stderr relay: line-prefixed with the server name, under a rate cap. The child's stdout is the MCP protocol channel and is never touched.

use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

/// The most bytes of one stderr line retained for the relay. A child that
/// never emits a newline is drained, not buffered without bound.
pub(super) const STDERR_RELAY_MAX_LINE_BYTES: usize = 8 * 1024;

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
    let mut reader = BufReader::new(stderr);
    let mut line = Vec::new();
    loop {
        line.clear();
        let mut truncated = false;
        let mut saw_bytes = false;
        loop {
            let available = reader.fill_buf().await?;
            if available.is_empty() {
                break;
            }
            saw_bytes = true;
            let newline = available.iter().position(|&byte| byte == b'\n');
            let take = newline.unwrap_or(available.len());
            if !truncated {
                let remaining = STDERR_RELAY_MAX_LINE_BYTES.saturating_sub(line.len());
                line.extend_from_slice(&available[..take.min(remaining)]);
                truncated = take > remaining;
            }
            reader.consume(take + usize::from(newline.is_some()));
            if newline.is_some() {
                break;
            }
        }
        if !saw_bytes {
            return Ok(());
        }
        let text = String::from_utf8_lossy(&line);
        match cap.admit(std::time::Instant::now()) {
            Admission::Relay => write_relayed(out, server, &text, truncated).await?,
            Admission::WindowReset { dropped } => {
                out.write_all(
                    format!("mcp[{server}] [rate cap: dropped {dropped} stderr lines]\n")
                        .as_bytes(),
                )
                .await?;
                write_relayed(out, server, &text, truncated).await?;
            }
            Admission::Drop => {}
        }
    }
}

async fn write_relayed<W>(
    out: &mut W,
    server: &str,
    line: &str,
    truncated: bool,
) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let suffix = if truncated { " [truncated]" } else { "" };
    out.write_all(format!("mcp[{server}] {line}{suffix}\n").as_bytes())
        .await
}
