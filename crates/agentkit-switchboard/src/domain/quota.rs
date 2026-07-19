use std::time::{Duration, Instant};
use std::fmt::Debug;

pub trait ProviderQuotaBehaviour: Debug + Send + Sync {
    fn update_from_headers(&mut self, headers: &[(String, String)]);
    fn handle_429(&mut self, headers: &[(String, String)], body: Option<&str>)
        -> (DegradationReason, Option<Duration>);
    fn clone_box(&self) -> Box<dyn ProviderQuotaBehaviour>;
}

#[derive(Debug, Clone)]
pub struct ProviderQuotaState {
    pub inner: QuotaBehaviourBox,
    pub degradation: Option<DegradationState>,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub last_validated_at: Instant,
}

/// Workaround: boxed trait object with manual clone via factory.
/// Each provider implementation provides a clone_box() method.
pub struct QuotaBehaviourBox(pub Box<dyn ProviderQuotaBehaviour>);

impl QuotaBehaviourBox {
    pub fn new(inner: Box<dyn ProviderQuotaBehaviour>) -> Self {
        Self(inner)
    }
}

impl std::ops::Deref for QuotaBehaviourBox {
    type Target = dyn ProviderQuotaBehaviour;
    fn deref(&self) -> &Self::Target {
        &*self.0
    }
}

impl std::ops::DerefMut for QuotaBehaviourBox {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut *self.0
    }
}

impl Clone for QuotaBehaviourBox {
    fn clone(&self) -> Self {
        Self(self.0.clone_box())
    }
}

impl Debug for QuotaBehaviourBox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "QuotaBehaviourBox({:?})", self.0)
    }
}

impl ProviderQuotaState {
    pub fn new(inner: Box<dyn ProviderQuotaBehaviour>) -> Self {
        Self {
            inner: QuotaBehaviourBox::new(inner),
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

pub fn backoff_duration(retry_count: u32, base_secs: u64, max_secs: u64) -> Duration {
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
        state.inner.update_from_headers(headers);
        return;
    }

    match status {
        429 => {
            let (reason, duration) = state.inner.handle_429(headers, body);
            state.degrade(reason, duration);
        }
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
