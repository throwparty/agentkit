//! Protocol-neutral orchestration shared by the version modules.
//!
//! Nothing here may depend on `acp::v1` or `acp::v2`; the version modules may
//! depend on it and never on each other.

pub(crate) mod dispatch;
pub mod messages;
pub mod session;
