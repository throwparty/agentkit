//! The structured failure every host operation returns.
//!
//! The classes and the exit numbers are the ones `agentkit-common` shares, so a
//! script can branch on the class without reading prose.

use std::fmt;

/// A provisioning failure class, as CONTEXT.md names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Unavailable,
    Misconfigured,
    Denied,
    Unsupported,
    Failed,
}

impl Class {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::Misconfigured => "misconfigured",
            Self::Denied => "denied",
            Self::Unsupported => "unsupported",
            Self::Failed => "failed",
        }
    }

    /// The process exit number, from `agentkit-common`'s shared numbering.
    pub fn exit_code(self) -> i32 {
        match self {
            Self::Failed => 1,
            Self::Misconfigured => 2,
            Self::Unsupported => 5,
            Self::Denied => 6,
            Self::Unavailable => 7,
        }
    }
}

/// A refusal or failure, with everything a reader needs to act on it.
#[derive(Debug, Clone)]
pub struct Failure {
    pub class: Class,
    pub reason: String,
    pub step: Option<String>,
    pub kind: Option<&'static str>,
    pub primitive: Option<String>,
    pub remedy: Option<String>,
    pub exit: Option<i32>,
    pub output: Option<String>,
}

impl Failure {
    pub fn new(class: Class, reason: impl Into<String>) -> Self {
        Self {
            class,
            reason: reason.into(),
            step: None,
            kind: None,
            primitive: None,
            remedy: None,
            exit: None,
            output: None,
        }
    }

    pub fn unavailable(primitive: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::new(Class::Unavailable, reason).with_primitive(primitive)
    }

    pub fn misconfigured(reason: impl Into<String>) -> Self {
        Self::new(Class::Misconfigured, reason)
    }

    pub fn denied(reason: impl Into<String>) -> Self {
        Self::new(Class::Denied, reason)
    }

    pub fn unsupported(reason: impl Into<String>) -> Self {
        Self::new(Class::Unsupported, reason)
    }

    pub fn failed(reason: impl Into<String>) -> Self {
        Self::new(Class::Failed, reason)
    }

    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = reason.into();
        self
    }

    pub fn with_step(mut self, identity: impl Into<String>, kind: &'static str) -> Self {
        self.step = Some(identity.into());
        self.kind = Some(kind);
        self
    }

    pub fn with_primitive(mut self, primitive: impl Into<String>) -> Self {
        self.primitive = Some(primitive.into());
        self
    }

    pub fn with_remedy(mut self, remedy: impl Into<String>) -> Self {
        self.remedy = Some(remedy.into());
        self
    }

    pub fn with_exit(mut self, exit: i32) -> Self {
        self.exit = Some(exit);
        self
    }

    pub fn with_output(mut self, output: impl Into<String>) -> Self {
        self.output = Some(output.into());
        self
    }

    pub fn is(&self, class: Class) -> bool {
        self.class == class
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.class.as_str(), self.reason)?;
        if let Some(step) = &self.step {
            write!(formatter, "\n  step: {step}")?;
            if let Some(kind) = self.kind {
                write!(formatter, " ({kind})")?;
            }
        }
        if let Some(primitive) = &self.primitive {
            write!(formatter, "\n  primitive: {primitive}")?;
        }
        if let Some(exit) = self.exit {
            write!(formatter, "\n  exit: {exit}")?;
        }
        if let Some(output) = &self.output {
            write!(formatter, "\n  output: {output}")?;
        }
        if let Some(remedy) = &self.remedy {
            write!(formatter, "\n  remedy: {remedy}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Failure {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_class_renders_its_name() {
        assert_eq!(Class::Unavailable.as_str(), "unavailable");
        assert_eq!(Class::Misconfigured.as_str(), "misconfigured");
        assert_eq!(Class::Denied.as_str(), "denied");
        assert_eq!(Class::Unsupported.as_str(), "unsupported");
        assert_eq!(Class::Failed.as_str(), "failed");
    }

    #[test]
    fn exit_codes_come_from_the_shared_numbering() {
        assert_eq!(Class::Failed.exit_code(), 1);
        assert_eq!(Class::Misconfigured.exit_code(), 2);
        assert_eq!(Class::Unsupported.exit_code(), 5);
        assert_eq!(Class::Denied.exit_code(), 6);
        assert_eq!(Class::Unavailable.exit_code(), 7);
    }

    #[test]
    fn a_failure_renders_every_field_it_was_given() {
        let failure = Failure::unavailable("nix", "no nix on PATH")
            .with_step("dev", "devshell")
            .with_remedy("install nix")
            .with_exit(127)
            .with_output("sh: nix: not found");
        let rendered = failure.to_string();
        assert!(rendered.starts_with("unavailable: no nix on PATH"), "{rendered}");
        assert!(rendered.contains("step: dev (devshell)"), "{rendered}");
        assert!(rendered.contains("primitive: nix"), "{rendered}");
        assert!(rendered.contains("exit: 127"), "{rendered}");
        assert!(rendered.contains("output: sh: nix: not found"), "{rendered}");
        assert!(rendered.contains("remedy: install nix"), "{rendered}");
    }
}
