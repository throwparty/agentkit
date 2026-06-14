use std::time::Duration;
use agentkit_switchboard::provider::quota::{
    handle_response_status, ProviderQuotaState, QuotaSource, PayAsYouGoState, SubscriptionState,
    DegradationReason,
};

fn make_payg() -> ProviderQuotaState {
    ProviderQuotaState::new(QuotaSource::PayAsYouGo(PayAsYouGoState::default()))
}

fn make_subscription() -> ProviderQuotaState {
    ProviderQuotaState::new(QuotaSource::Subscription(SubscriptionState::default()))
}

fn header(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

#[test]
fn quota_headers_openai() {
    let mut state = make_payg();
    let headers = vec![
        header("x-ratelimit-remaining-requests", "99"),
        header("x-ratelimit-remaining-tokens", "50000"),
    ];
    state.update_from_headers(&headers);
    if let QuotaSource::PayAsYouGo(ref payg) = state.quota {
        assert_eq!(payg.requests_remaining, Some(99));
        assert_eq!(payg.input_tokens_remaining, Some(50000));
    } else {
        panic!("expected PayAsYouGo");
    }
}

#[test]
fn quota_headers_anthropic() {
    let mut state = make_payg();
    let headers = vec![
        header("anthropic-ratelimit-requests-remaining", "50"),
        header("anthropic-ratelimit-input-tokens-remaining", "100000"),
        header("anthropic-ratelimit-output-tokens-remaining", "20000"),
    ];
    state.update_from_headers(&headers);
    if let QuotaSource::PayAsYouGo(ref payg) = state.quota {
        assert_eq!(payg.requests_remaining, Some(50));
        assert_eq!(payg.input_tokens_remaining, Some(100000));
        assert_eq!(payg.output_tokens_remaining, Some(20000));
    }
}

#[test]
fn quota_headers_missing() {
    let mut state = make_payg();
    state.update_from_headers(&[]);
    if let QuotaSource::PayAsYouGo(ref payg) = state.quota {
        assert!(payg.requests_remaining.is_none());
        assert!(payg.input_tokens_remaining.is_none());
    }
}

#[test]
fn quota_429_retry_after() {
    let mut state = make_payg();
    handle_response_status(&mut state, 429, &[header("retry-after", "30")], None);
    assert!(state.is_degraded());
    match &state.degradation {
        Some(d) => assert_eq!(d.reason, DegradationReason::RateLimitExceeded),
        None => panic!("expected degradation"),
    }
}

#[test]
fn quota_429_insufficient_quota() {
    let mut state = make_payg();
    handle_response_status(&mut state, 429, &[], Some("insufficient_quota"));
    assert!(state.is_degraded());
    assert!(state.degradation.as_ref().unwrap().degraded_until.is_none());
    if let QuotaSource::PayAsYouGo(ref payg) = state.quota {
        assert!(payg.spend_cap_exhausted);
    }
}

#[test]
fn quota_429_default() {
    let mut state = make_payg();
    handle_response_status(&mut state, 429, &[], None);
    assert!(state.is_degraded());
    match &state.degradation {
        Some(d) => assert_eq!(d.reason, DegradationReason::RateLimitExceeded),
        None => panic!("expected degradation"),
    }
}

#[test]
fn quota_401_permanent() {
    let mut state = make_payg();
    handle_response_status(&mut state, 401, &[], None);
    assert!(state.is_degraded());
    assert!(state.degradation.as_ref().unwrap().degraded_until.is_none());
    assert_eq!(
        state.degradation.as_ref().unwrap().reason,
        DegradationReason::AuthenticationFailure
    );
}

#[test]
fn quota_5xx_backoff() {
    let mut state = make_payg();
    handle_response_status(&mut state, 502, &[], None);
    assert!(state.is_degraded());
    assert_eq!(
        state.degradation.as_ref().unwrap().reason,
        DegradationReason::ProviderError
    );
}

#[test]
fn quota_success_clears() {
    let mut state = make_payg();
    handle_response_status(&mut state, 429, &[header("retry-after", "30")], None);
    assert!(state.is_degraded());
    handle_response_status(&mut state, 200, &[], None);
    assert!(!state.is_degraded());
}

#[test]
fn quota_subscription_429() {
    let mut state = make_subscription();
    handle_response_status(&mut state, 429, &[], None);
    assert!(state.is_degraded());
    assert_eq!(
        state.degradation.as_ref().unwrap().reason,
        DegradationReason::QuotaExhausted
    );
}

#[test]
fn quota_degradation_expired() {
    let mut state = make_payg();
    state.degrade(DegradationReason::RateLimitExceeded, Some(Duration::from_nanos(1)));
    std::thread::sleep(Duration::from_micros(10));
    state.check_expired();
    assert!(!state.is_degraded());
}
