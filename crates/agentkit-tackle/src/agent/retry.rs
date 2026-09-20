//! Retry policy and failure classification for provider errors.
//!
//! Fixed internal limits, not configuration: a retry policy the user can
//! tune is a surface to misconfigure (the same posture as the script
//! budgets).

/// The class of a provider failure, from the error's text. rig's typed
/// errors flatten to strings at the ModelError seam, so classification
/// sniffs the message — pragmatic and monotonic across providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    /// Transient (rate limit, network, 5xx): worth retrying.
    Retryable,
    /// The conversation no longer fits the model's window: retrying can
    /// never succeed — the caller translates to an actionable message.
    ContextLength,
    /// Everything else: fail the turn.
    Fatal,
}

pub fn classify(error_text: &str) -> FailureClass {
    let lower = error_text.to_lowercase();
    if lower.contains("context_length")
        || lower.contains("context_length_exceeded")
        || lower.contains("context window")
        || lower.contains("too many tokens")
        || lower.contains("maximum context")
        || lower.contains("prompt is too long")
    {
        FailureClass::ContextLength
    } else if lower.contains("429")
        || lower.contains("rate limit")
        || lower.contains("overloaded")
        || lower.contains("timeout")
        || lower.contains("timed out")
        || lower.contains("connection")
        || lower.contains("temporarily")
        || lower.contains("502")
        || lower.contains("503")
        || lower.contains("504")
        || lower.contains("service unavailable")
        || lower.contains("internal server error")
    {
        FailureClass::Retryable
    } else {
        FailureClass::Fatal
    }
}

/// Retry attempts per model request.
pub const MAX_ATTEMPTS: u32 = 3;

/// The backoff before `attempt` (1-based) — 500ms doubling, capped.
pub fn backoff(attempt: u32) -> std::time::Duration {
    let millis = 500u64.saturating_mul(1 << (attempt - 1).min(5));
    std::time::Duration::from_millis(millis.min(8_000))
}

/// Extracts a retry-after delay (seconds) from an error's text, when the
/// provider embedded one. Capped at 30s.
pub fn retry_after_secs(error_text: &str) -> Option<std::time::Duration> {
    let lower = error_text.to_lowercase();
    let index = lower.find("retry-after")?;
    let after = &lower[index + "retry-after".len()..];
    let digits: String = after
        .chars()
        .skip_while(|ch| !ch.is_ascii_digit())
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    digits
        .parse::<u64>()
        .ok()
        .map(|secs| std::time::Duration::from_secs(secs.min(30)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_length_never_retries() {
        assert_eq!(
            classify("prompt is too long: 300000 tokens > 200000 maximum"),
            FailureClass::ContextLength
        );
        assert_eq!(
            classify("context_length_exceeded"),
            FailureClass::ContextLength
        );
    }

    #[test]
    fn transient_failures_are_retryable() {
        for text in [
            "HTTP 429: rate limit exceeded",
            "connection reset by peer",
            "503 service unavailable",
            "request timeout",
            "provider overloaded",
        ] {
            assert_eq!(classify(text), FailureClass::Retryable, "{text}");
        }
    }

    #[test]
    fn auth_failures_are_fatal() {
        assert_eq!(classify("invalid api key"), FailureClass::Fatal);
        assert_eq!(classify("model not found"), FailureClass::Fatal);
    }

    #[test]
    fn backoff_doubles_and_caps() {
        assert_eq!(backoff(1), std::time::Duration::from_millis(500));
        assert_eq!(backoff(2), std::time::Duration::from_millis(1000));
        assert_eq!(backoff(6), std::time::Duration::from_millis(8_000));
    }

    #[test]
    fn retry_after_is_parsed_and_capped() {
        assert_eq!(
            retry_after_secs("HTTP 429 rate limited; retry-after: 12"),
            Some(std::time::Duration::from_secs(12))
        );
        assert_eq!(
            retry_after_secs("retry-after: 900"),
            Some(std::time::Duration::from_secs(30))
        );
        assert_eq!(retry_after_secs("no hint here"), None);
    }
}
