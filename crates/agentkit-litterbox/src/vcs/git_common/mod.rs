//! Git concepts that anything using the Git storage backend reuses.
//!
//! This module is Git-specific, not VCS-neutral: it knows about objects, trees,
//! ignore files and branch naming. The Git backend and, later, the jj bridge use
//! it; neither backend imports the other. It may use `git2`.

pub mod archive;
pub mod filter;
pub mod ignore;
pub mod naming;

pub use filter::SnapshotFilter;
pub use ignore::IgnoreFiles;
pub use naming::branch_name_for;
