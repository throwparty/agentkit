use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct ProviderQuotaState {
    pub quota: QuotaSource,
    pub degradation: Option<DegradationState>,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub last_validated_at: Instant,
}

impl ProviderQuotaState {
    pub fn new(quota: QuotaSource) -> Self {
        Self {
            quota,
            degradation: None,
            total_input_tokens: 0,
            total_output_tokens: 0,
            last_validated_at: Instant::now(),
        }
    }

    pub fn is_degraded(&self) -> bool {
        match &self.degradation {
            Some(d) => {
                match d.degraded_until {
                    Some(until) => Instant::now() < until,
                    None => true,
                }
            }
            None => false,
        }
    }

    pub fn check_expired(&mut self) {
        if let Some(ref d) = self.degradation {
            if let Some(until) = d.degraded_until {
                if Instant::now() >= until {
                    self.degradation = None;
                }
            }
        }
    }

    pub fn degrade(&mut self, reason: DegradationReason, duration: Option<Duration>) {
        let degraded_until = duration.map(|d| Instant::now() + d);
        let retry_count = match &self.degradation {
            Some(prev) if prev.reason == reason => prev.retry_count + 1,
            _ => 0,
        };
        self.degradation = Some(DegradationState {
            reason,
            degraded_until,
            retry_count,
            model_groups: None,
        });
    }

    pub fn clear_degradation(&mut self) {
        self.degradation = None;
    }

    pub fn update_from_headers(&mut self, headers: &[(String, String)]) {
        if let QuotaSource::PayAsYouGo(ref mut state) = self.quota {
            for (name, value) in headers {
                match (name.as_str(), value.parse::<u64>().ok()) {
                    ("x-ratelimit-remaining-requests", Some(v)) => state.requests_remaining = Some(v as u32),
                    ("x-ratelimit-remaining-tokens", Some(v)) => state.input_tokens_remaining = Some(v),
                    ("anthropic-ratelimit-requests-remaining", Some(v)) => state.requests_remaining = Some(v as u32),
                    ("anthropic-ratelimit-input-tokens-remaining", Some(v)) => state.input_tokens_remaining = Some(v),
                    ("anthropic-ratelimit-output-tokens-remaining", Some(v)) => state.output_tokens_remaining = Some(v),
                    _ => {}
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum QuotaSource {
    PayAsYouGo(PayAsYouGoState),
    Subscription(SubscriptionState),
    Free,
}

#[derive(Debug, Clone, Default)]
pub struct PayAsYouGoState {
    pub requests_remaining: Option<u32>,
    pub requests_limit: Option<u32>,
    pub requests_reset_after: Option<Duration>,
    pub input_tokens_remaining: Option<u64>,
    pub input_tokens_limit: Option<u64>,
    pub output_tokens_remaining: Option<u64>,
    pub output_tokens_limit: Option<u64>,
    pub spend_cap_exhausted: bool,
}

#[derive(Debug, Clone)]
pub struct SubscriptionState {
    pub exhausted: bool,
    pub exhausted_at: Option<Instant>,
    pub cooldown_duration: Duration,
    pub estimated_messages_per_window: Option<u32>,
    pub estimated_window_hours: Option<u32>,
}

impl Default for SubscriptionState {
    fn default() -> Self {
        Self {
            exhausted: false,
            exhausted_at: None,
            cooldown_duration: Duration::from_secs(5 * 3600),
            estimated_messages_per_window: None,
            estimated_window_hours: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum DegradationReason {
    RateLimitExceeded,
    QuotaExhausted,
    ProviderError,
    AuthenticationFailure,
    Timeout,
}

#[derive(Debug, Clone)]
pub struct DegradationState {
    pub reason: DegradationReason,
    pub degraded_until: Option<Instant>,
    pub retry_count: u32,
    pub model_groups: Option<Vec<String>>,
}

fn default_429_duration() -> Duration {
    Duration::from_secs(60)
}

fn backoff_duration(retry_count: u32, base_secs: u64, max_secs: u64) -> Duration {
    let secs = base_secs * 2u64.pow(retry_count.min(10));
    Duration::from_secs(secs.min(max_secs))
}

pub fn handle_response_status(
    state: &mut ProviderQuotaState,
    status: u16,
    headers: &[(String, String)],
    body: Option<&str>,
) {
    if status == 200 || status == 201 {
        state.clear_degradation();
        state.update_from_headers(headers);
        return;
    }

    match status {
        429 => handle_429(state, headers, body),
        401 | 403 => {
            state.degrade(DegradationReason::AuthenticationFailure, None);
        }
        s if (500..600).contains(&s) => {
            let duration = backoff_duration(
                state.degradation.as_ref().map_or(0, |d| d.retry_count),
                30,
                300,
            );
            state.degrade(DegradationReason::ProviderError, Some(duration));
        }
        _ => {}
    }
}

fn handle_429(state: &mut ProviderQuotaState, headers: &[(String, String)], body: Option<&str>) {
    let retry_after = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
        .and_then(|(_, value)| value.parse::<u64>().ok())
        .map(Duration::from_secs);

    let is_insufficient_quota = body.is_some_and(|b| b.contains("insufficient_quota"));

    match &state.quota {
        QuotaSource::Subscription(_) => {
            let duration = retry_after.unwrap_or(Duration::from_secs(5 * 3600));
            state.degrade(DegradationReason::QuotaExhausted, Some(duration));
        }
        QuotaSource::PayAsYouGo(_) => {
            if is_insufficient_quota {
                state.degrade(DegradationReason::QuotaExhausted, None);
                if let QuotaSource::PayAsYouGo(ref mut payg) = state.quota {
                    payg.spend_cap_exhausted = true;
                }
            } else {
                let duration = retry_after.unwrap_or_else(default_429_duration);
                state.degrade(DegradationReason::RateLimitExceeded, Some(duration));
            }
        }
        QuotaSource::Free => {}
    }
}
