//! The ACP server surface: version negotiation, the protocol-neutral common
//! layer, and one module per protocol version.
//!
//! `common` holds orchestration shared by every version. `v1` and `v2` hold
//! the per-version wire surfaces and must not depend on each other (enforced
//! by arch-lint). `transport` carries whichever version a connection
//! negotiates.

pub mod common;
pub mod http;
pub mod router;
pub mod transport;
pub mod v1;
pub mod v2;

pub use transport::run_stdio;
pub(crate) use router::run_acp_over;
pub use v1::{PendingProject, TackleState};
