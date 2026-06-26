use std::collections::HashMap;
use crate::config::BillingModel;
use crate::provider::ProviderState;
use crate::session::SessionAffinity;

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderSelection {
    pub identity: String,
    pub reason: SelectionReason,
    pub switch_count: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SelectionReason {
    Affinity,
    Cost,
    Fallback,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RoutingError {
    ModelNotFound,
    NoProvider,
}

fn score_billing(billing: &BillingModel) -> u8 {
    match billing {
        BillingModel::Subscription => 0,
        BillingModel::PayAsYouGo => 1,
        BillingModel::Free => 2,
    }
}

pub fn select_provider(
    model: &str,
    session: Option<&SessionAffinity>,
    providers: &HashMap<String, ProviderState>,
) -> Result<ProviderSelection, RoutingError> {
    let mut candidates: Vec<&ProviderState> = providers
        .values()
        .filter(|p| p.is_available() && p.serves_model(model))
        .collect();

    if candidates.is_empty() {
        let any_serves = providers.values().any(|p| p.serves_model(model));
        if !any_serves {
            return Err(RoutingError::ModelNotFound);
        }
        return Err(RoutingError::NoProvider);
    }

    if let Some(sa) = session {
        if let Some(assigned) = candidates.iter().find(|p| p.identity == sa.provider_identity) {
            return Ok(ProviderSelection {
                identity: assigned.identity.clone(),
                reason: SelectionReason::Affinity,
                switch_count: 0,
            });
        }
    }

    candidates.sort_by(|a, b| {
        let billing_cmp = score_billing(&a.billing).cmp(&score_billing(&b.billing));
        if billing_cmp != std::cmp::Ordering::Equal {
            return billing_cmp;
        }
        let cost_cmp = a
            .pricing
            .input_per_mtok
            .partial_cmp(&b.pricing.input_per_mtok)
            .unwrap_or(std::cmp::Ordering::Equal);
        if cost_cmp != std::cmp::Ordering::Equal {
            return cost_cmp;
        }
        a.identity.cmp(&b.identity)
    });

    let switch_count = session.map_or(0, |_| 1);

    Ok(ProviderSelection {
        identity: candidates[0].identity.clone(),
        reason: SelectionReason::Cost,
        switch_count,
    })
}
