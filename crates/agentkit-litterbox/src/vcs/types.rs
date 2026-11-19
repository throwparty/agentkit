use std::fmt;

/// The name of a branch a backend owns.
///
/// Opaque to the orchestrator beyond display and equality.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BranchName(String);

impl BranchName {
    /// Wrap a branch name.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BranchName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// An opaque revision identity.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Revision(String);

impl Revision {
    /// Wrap a revision identity.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The identity as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Revision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// An opaque tree identity.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TreeId(String);

impl TreeId {
    /// Wrap a tree identity.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The identity as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TreeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A sandbox's branch, as a backend handle.
///
/// The orchestrator can address a record's branch without holding a backend:
/// a bound workspace is a property of the backend, not of this value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    branch: BranchName,
}

impl Workspace {
    /// Address an existing branch.
    pub fn from_branch(branch: impl Into<String>) -> Self {
        Self {
            branch: BranchName::new(branch),
        }
    }

    /// The branch this workspace belongs to.
    pub fn branch(&self) -> &BranchName {
        &self.branch
    }
}

/// What [`crate::vcs::Vcs::prepare`] produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    /// The branch and its handle.
    pub workspace: Workspace,
    /// The revision the sandbox starts from.
    pub base: Revision,
}
