//! The default tables a Seatbelt profile is built from.
//!
//! These are data, not configuration. A machine may add to the read and
//! service lists, and may add credential denies, but cannot remove a default.

/// OS paths a command needs to start. No developer path and no Nix store.
pub(crate) const BASELINE_READS: &[&str] = &[
    "/System/Library",
    "/System/Applications",
    "/Library/Frameworks",
    "/usr/bin",
    "/usr/lib",
    "/usr/share",
    "/usr/libexec",
    "/bin",
    "/sbin",
    "/private/etc",
    "/private/var/db/dyld",
    "/private/var/select",
    "/dev",
];

/// Mach services the workloads measured so far need. Each was added by a run.
pub(crate) const MACH_SERVICES: &[&str] = &[
    "com.apple.system.logger",
    "com.apple.system.notification_center",
    "com.apple.distributed_notifications@Uv3",
    "com.apple.logd",
    "com.apple.lsd.mapdb",
    "com.apple.fonts",
    "com.apple.FontObjectsServer",
    "com.apple.PowerManagement.control",
    "com.apple.system.opendirectoryd.libinfo",
    "com.apple.system.opendirectoryd.membership",
    "com.apple.bsd.dirhelper",
    "com.apple.SystemConfiguration.configd",
];

/// Paths no contribution may expose. Denied for read, write and unlink.
pub(crate) const CREDENTIAL_DENIES: &[&str] = &[
    "~/.ssh",
    "~/.aws",
    "~/.gnupg",
    "~/.docker",
    "~/.kube",
    "~/.netrc",
    "~/.git-credentials",
    "~/.npmrc",
    "~/.cargo/credentials.toml",
    "~/.config/gh",
    "~/.config/gcloud",
    "~/.config/containers",
    "~/.config/pip",
    "~/.pypirc",
    "~/.terraform.d",
    "~/.vault-token",
    "~/.password-store",
    "~/Library/Keychains",
    "~/Library/Application Support/Google/Chrome",
    "~/Library/Application Support/Firefox",
    "~/Library/Safari",
    "~/Library/Cookies",
];
