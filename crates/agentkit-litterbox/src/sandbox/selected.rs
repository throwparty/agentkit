//! One mechanism, viewed through the contract and the facets it supports.
//!
//! A backend constructs this value itself: the contract owns the shape, so a
//! mechanism can describe what it provides without reaching into the registry.

use std::sync::Arc;

use super::contract::{Backend, Error};
use super::facets::{Forwarding, Lifecycle, Limits};
use super::report::{Capability, State};

/// One backend, viewed through the contract and the facets it supports.
///
/// Every field points at the same object; the facets are `None` when the
/// mechanism does not implement them. Use the builder so the name and the
/// implementation cannot drift apart.
#[derive(Clone)]
pub struct Selected {
    name: String,
    core: Arc<dyn Backend>,
    lifecycle: Option<Arc<dyn Lifecycle>>,
    limits: Option<Arc<dyn Limits>>,
    forwarding: Option<Arc<dyn Forwarding>>,
}

impl Selected {
    /// A backend with no optional facets.
    pub fn new(name: impl Into<String>, core: Arc<dyn Backend>) -> Self {
        Self {
            name: name.into(),
            core,
            lifecycle: None,
            limits: None,
            forwarding: None,
        }
    }

    /// Add the lifecycle facet.
    pub fn with_lifecycle(mut self, facet: Arc<dyn Lifecycle>) -> Self {
        self.lifecycle = Some(facet);
        self
    }

    /// Add the limits facet.
    pub fn with_limits(mut self, facet: Arc<dyn Limits>) -> Self {
        self.limits = Some(facet);
        self
    }

    /// Add the forwarding facet.
    pub fn with_forwarding(mut self, facet: Arc<dyn Forwarding>) -> Self {
        self.forwarding = Some(facet);
        self
    }

    /// The mechanism's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The contract every caller uses.
    pub fn core(&self) -> &Arc<dyn Backend> {
        &self.core
    }

    /// The standard capabilities implied by the facets this backend exposes.
    ///
    /// Backend-specific extras, such as an egress posture, stay in
    /// `Backend::capabilities`.
    pub fn standard_capabilities(&self) -> Vec<Capability> {
        let mut capabilities = Vec::new();
        if self.lifecycle.is_some() {
            capabilities.push(Capability {
                operation: "pause and resume".to_string(),
                state: State::Available,
                reason: None,
                remedy: None,
            });
        }
        if self.limits.is_some() {
            capabilities.push(Capability {
                operation: "resource limits".to_string(),
                state: State::Available,
                reason: None,
                remedy: None,
            });
        }
        if self.forwarding.is_some() {
            capabilities.push(Capability {
                operation: "port forwarding".to_string(),
                state: State::Available,
                reason: None,
                remedy: None,
            });
        }
        capabilities
    }

    /// Merge a backend's own capability entries with the facet-derived ones.
    ///
    /// Provisioning kinds come from the contract's support report; a backend's
    /// own entry replaces any standard entry for the same operation.
    pub fn merge_capabilities(&self, extras: Vec<Capability>) -> Vec<Capability> {
        let mut capabilities = self.standard_capabilities();
        for support in self.core().provision_support() {
            capabilities.push(Capability {
                operation: format!("provision:{}", support.kind.as_str()),
                state: support.state,
                reason: support.reason,
                remedy: support.remedy,
            });
        }
        for extra in extras {
            if let Some(existing) = capabilities
                .iter_mut()
                .find(|capability| capability.operation == extra.operation)
            {
                *existing = extra;
            } else {
                capabilities.push(extra);
            }
        }
        capabilities
    }

    /// The lifecycle facet, or a structured refusal naming what is missing.
    pub fn lifecycle_or_unsupported(&self, operation: &str) -> Result<&Arc<dyn Lifecycle>, Error> {
        self.lifecycle.as_ref().ok_or_else(|| Error::Unsupported {
            operation: operation.to_string(),
            backend: self.name.clone(),
            primitive: "a sandbox-level lifecycle".to_string(),
            reason: "the mechanism has no sandbox-level state".to_string(),
            remedy: "stop or restart the command instead".to_string(),
        })
    }

    /// The limits facet, or a structured refusal naming what is missing.
    pub fn limits_or_unsupported(&self, operation: &str) -> Result<&Arc<dyn Limits>, Error> {
        self.limits.as_ref().ok_or_else(|| Error::Unsupported {
            operation: operation.to_string(),
            backend: self.name.clone(),
            primitive: "a post-create resource bound".to_string(),
            reason: "the mechanism applies its bounds only when it creates the sandbox".to_string(),
            remedy: "set the bounds in the backend's configuration before creating".to_string(),
        })
    }

    /// The forwarding facet, or a structured refusal naming what is missing.
    pub fn forwarding_or_unsupported(
        &self,
        operation: &str,
    ) -> Result<&Arc<dyn Forwarding>, Error> {
        self.forwarding.as_ref().ok_or_else(|| Error::Unsupported {
            operation: operation.to_string(),
            backend: self.name.clone(),
            primitive: "a host port namespace".to_string(),
            reason: "the mechanism does not forward ports to the host".to_string(),
            remedy: "use a mechanism that forwards ports".to_string(),
        })
    }
}
