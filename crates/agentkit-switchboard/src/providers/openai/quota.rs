use std::time::Duration;
use crate::domain::quota::{DegradationReason, ProviderQuotaBehaviour};

#[derive(Debug, Clone, Default)]
pub struct OpenAiQuota {
    pub requests_remaining: Option<u32>,
    pub requests_limit: Option<u32>,
    pub requests_reset_after: Option<Duration>,
    pub input_tokens_remaining: Option<u64>,
    pub input_tokens_limit: Option<u64>,
    pub output_tokens_remaining: Option<u64>,
    pub output_tokens_limit: Option<u64>,
    pub spend_cap_exhausted: bool,
}

impl ProviderQuotaBehaviour for OpenAiQuota {
    fn update_from_headers(&mut self, headers: &[(String, String)]) {
        for (name, value) in headers {
            match (name.as_str(), value.parse::<u64>().ok()) {
                ("x-ratelimit-remaining-requests", Some(v)) => self.requests_remaining = Some(v as u32),
                ("x-ratelimit-remaining-tokens", Some(v)) => self.input_tokens_remaining = Some(v),
                _ => {}
            }
        }
    }

    fn handle_429(&mut self, headers: &[(String, String)], body: Option<&str>)
        -> (DegradationReason, Option<Duration>)
    {
        let retry_after = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
            .and_then(|(_, value)| value.parse::<u64>().ok())
            .map(Duration::from_secs);

        let is_insufficient_quota = body.is_some_and(|b| b.contains("insufficient_quota"));

        if is_insufficient_quota {
            self.spend_cap_exhausted = true;
            (DegradationReason::QuotaExhausted, None)
        } else {
            let duration = retry_after.unwrap_or(Duration::from_secs(60));
            (DegradationReason::RateLimitExceeded, Some(duration))
        }
    }

    fn clone_box(&self) -> Box<dyn ProviderQuotaBehaviour> {
        Box::new(self.clone())
    }
}
