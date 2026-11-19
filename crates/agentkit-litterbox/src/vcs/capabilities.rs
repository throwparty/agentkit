/// How a backend surfaces a conflict.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictModel {
    /// Conflicts live in commits and resolve natively.
    Native,
    /// The branch advance is held until the conflict is resolved.
    Blocking,
}

/// How a backend can present its workspace to a sandbox.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindingSupport {
    /// The workspace is copied into the sandbox.
    Copied,
    /// The workspace is bound into the sandbox.
    Bound,
    /// Both presentations are available.
    Both,
}

/// How a backend publishes its state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportSupport {
    /// There is nothing to export.
    None,
    /// The backend exports to a Git remote.
    GitExport,
}

/// What a constructed backend and its configuration can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    /// How conflicts surface.
    pub conflict_model: ConflictModel,
    /// Whether the backend keeps an operation log.
    pub op_log: bool,
    /// Which workspace presentations the configuration allows.
    pub bindings: BindingSupport,
    /// How the backend publishes its state.
    pub export: ExportSupport,
}
