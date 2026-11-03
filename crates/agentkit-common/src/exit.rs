//! Process exit codes shared by AgentKit components.
//!
//! A component that classifies a failure the same way exits with the same
//! number, so a script can distinguish "not found" from "denied" without
//! knowing which binary produced it.

/// The class a process reports when it exits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitClass {
    /// The operation completed.
    Success,
    /// The operation ran and failed.
    Failed,
    /// The invocation was malformed or its configuration unusable.
    Usage,
    /// The thing addressed does not exist.
    NotFound,
    /// The thing addressed already exists, or changed underneath the caller.
    Conflict,
    /// The backend does not provide the operation.
    Unsupported,
    /// Permission was refused.
    Denied,
    /// The mechanism this host needs is not available.
    Unavailable,
}

impl ExitClass {
    /// The process exit number for this class.
    pub const fn code(self) -> u8 {
        match self {
            Self::Success => 0,
            Self::Failed => 1,
            Self::Usage => 2,
            Self::NotFound => 3,
            Self::Conflict => 4,
            Self::Unsupported => 5,
            Self::Denied => 6,
            Self::Unavailable => 7,
        }
    }
}

impl From<ExitClass> for std::process::ExitCode {
    fn from(class: ExitClass) -> Self {
        std::process::ExitCode::from(class.code())
    }
}

#[cfg(test)]
mod tests {
    use super::ExitClass;

    #[test]
    fn codes_are_stable() {
        assert_eq!(ExitClass::Success.code(), 0);
        assert_eq!(ExitClass::Failed.code(), 1);
        assert_eq!(ExitClass::Usage.code(), 2);
        assert_eq!(ExitClass::NotFound.code(), 3);
        assert_eq!(ExitClass::Conflict.code(), 4);
        assert_eq!(ExitClass::Unsupported.code(), 5);
        assert_eq!(ExitClass::Denied.code(), 6);
        assert_eq!(ExitClass::Unavailable.code(), 7);
    }
}
