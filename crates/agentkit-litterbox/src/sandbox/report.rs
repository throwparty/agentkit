//! What a backend can and cannot do here, in structured form.
//!
//! Reports are fields rather than prose so a caller, a test and the CLI can
//! each read them without parsing a message.

/// Whether an operation or primitive is usable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum State {
    /// Usable now.
    Available,
    /// Usable, but with a limitation the caller must know about.
    Degraded,
    /// Not usable on this host or with this backend.
    Unavailable,
}

impl State {
    /// The worse of two states.
    pub fn worst(self, other: Self) -> Self {
        match (self, other) {
            (Self::Unavailable, _) | (_, Self::Unavailable) => Self::Unavailable,
            (Self::Degraded, _) | (_, Self::Degraded) => Self::Degraded,
            (Self::Available, Self::Available) => Self::Available,
        }
    }
}

/// What a fallback cannot do, when a degraded operation is used.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Fallback {
    /// The fallback's name, in the backend's own vocabulary.
    pub name: String,
    /// What it does not bound or guarantee.
    pub does_not_bound: String,
}

/// One primitive a backend needs, and whether this host provides it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Check {
    /// The primitive, named by the mechanism that needs it.
    pub primitive: String,
    /// Whether it is usable.
    pub state: State,
    /// Why it is in this state.
    pub reason: Option<String>,
    /// What the user can do about it.
    pub remedy: Option<String>,
    /// The fallback, when the operation degrades rather than fails.
    pub fallback: Option<Fallback>,
}

/// A backend's readiness on this host.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Readiness {
    /// The backend that was asked.
    pub backend: String,
    /// One entry per primitive.
    pub checks: Vec<Check>,
}

impl Readiness {
    /// The worst state across every check.
    pub fn state(&self) -> State {
        self.checks
            .iter()
            .map(|check| check.state)
            .fold(State::Available, State::worst)
    }

    /// Whether the backend can run here.
    pub fn is_available(&self) -> bool {
        self.state() == State::Available
    }
}

/// What a backend says about one optional operation.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Capability {
    /// The operation, in the backend's own vocabulary.
    pub operation: String,
    /// Whether it is usable.
    pub state: State,
    /// Why it is in this state.
    pub reason: Option<String>,
    /// What the user can do about it.
    pub remedy: Option<String>,
}

/// How a sandbox reaches the network.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub enum EgressPosture {
    /// No route beyond the sandbox's own loopback.
    Denied,
    /// The sandbox shares the host network.
    Direct,
    /// Reachable through a proxy, at the endpoint given.
    Proxied {
        /// The endpoint a caller configures a proxy with.
        endpoint: String,
    },
}

#[cfg(test)]
mod tests {
    use super::{Capability, Check, EgressPosture, Fallback, Readiness, State};

    fn check(state: State) -> Check {
        Check {
            primitive: "unprivileged user namespaces".to_string(),
            state,
            reason: None,
            remedy: None,
            fallback: None,
        }
    }

    #[test]
    fn readiness_reports_the_worst_check() {
        let readiness = Readiness {
            backend: "bubblewrap".to_string(),
            checks: vec![check(State::Available), check(State::Unavailable)],
        };
        assert_eq!(readiness.state(), State::Unavailable);
        assert!(!readiness.is_available());
    }

    #[test]
    fn a_degraded_check_carries_its_fallback_and_its_limits() {
        let readiness = Readiness {
            backend: "bubblewrap".to_string(),
            checks: vec![Check {
                primitive: "cgroup delegation".to_string(),
                state: State::Degraded,
                reason: Some("the user manager does not delegate cgroups".to_string()),
                remedy: Some("enable cgroup delegation for the user manager".to_string()),
                fallback: Some(Fallback {
                    name: "stop and rescan the pid namespace".to_string(),
                    does_not_bound: "memory and task count".to_string(),
                }),
            }],
        };
        assert_eq!(readiness.state(), State::Degraded);
        let fallback = readiness.checks[0]
            .fallback
            .as_ref()
            .expect("fallback recorded");
        assert_eq!(fallback.does_not_bound, "memory and task count");
    }

    #[test]
    fn egress_serialises_as_three_distinct_states_and_no_boolean() {
        let rendered: Vec<String> = [
            EgressPosture::Denied,
            EgressPosture::Direct,
            EgressPosture::Proxied {
                endpoint: "unix:/run/litterbox/proxy.sock".to_string(),
            },
        ]
        .iter()
        .map(|posture| serde_json::to_string(posture).expect("serialises"))
        .collect();

        assert_eq!(rendered.len(), 3);
        assert_ne!(rendered[0], rendered[1]);
        assert_ne!(rendered[1], rendered[2]);
        assert!(
            rendered.iter().all(|value| !value.contains("true")),
            "egress is not a boolean: {rendered:?}"
        );
    }

    #[test]
    fn capability_is_structured() {
        let capability = Capability {
            operation: "pause".to_string(),
            state: State::Unavailable,
            reason: Some("the sandbox has no sandbox-level state".to_string()),
            remedy: Some("stop the command instead".to_string()),
        };
        assert_eq!(capability.state, State::Unavailable);
        assert!(capability.reason.is_some());
    }
}
