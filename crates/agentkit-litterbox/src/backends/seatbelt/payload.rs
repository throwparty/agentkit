//! The record a later process needs to address a sandbox.
//!
//! The payload contains no environment value and no profile text. It is stored
//! in `SandboxRecord.data`; the metadata store owns the file modes.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::BACKEND;
use super::host::{Host, PidState};
use super::mounts::CollapsedStep;
use super::policy;
use crate::sandbox::contract::{Error, SandboxRecord};
use crate::sandbox::environment::Provenance;

/// The payload edition this code writes.
pub(crate) const VERSION: u32 = 2;

/// What a later process needs to address one sandbox.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Payload {
    pub version: u32,
    pub helper: PathBuf,
    pub holder_pid: u32,
    pub holder_start_secs: i64,
    pub holder_start_micros: u32,
    pub socket: PathBuf,
    pub workspace_host: PathBuf,
    pub workspace_sandbox: PathBuf,
    pub reads: Vec<PathBuf>,
    pub collapsed: Vec<CollapsedStep>,
    pub provenance: Provenance,
    pub posture: String,
    pub tag: String,
}

/// The state a record describes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PayloadState {
    Live,
    Paused,
    Missing,
    Foreign,
    Invalid(String),
}

impl Payload {
    /// Build a payload from a record, refusing another edition.
    pub(crate) fn from_record(record: &SandboxRecord) -> Result<Self, Error> {
        let probe: VersionProbe = record
            .data
            .clone()
            .try_into()
            .map_err(|error| unavailable(format!("the payload is not readable: {error}")))?;
        if probe.version > VERSION {
            return Err(unavailable(format!(
                "the payload edition {} is newer than {VERSION}",
                probe.version
            )));
        }
        if probe.version < VERSION {
            return Err(unavailable(format!(
                "the payload edition {} is older than {VERSION}",
                probe.version
            )));
        }
        record
            .data
            .clone()
            .try_into()
            .map_err(|error| unavailable(format!("the payload is incomplete: {error}")))
    }

    /// Serialise the payload for `SandboxRecord.data`.
    pub(crate) fn to_value(&self) -> Result<toml::Value, Error> {
        toml::Value::try_from(self)
            .map_err(|error| Error::failed(format!("cannot serialise the payload: {error}")))
    }

    /// Check that the record still describes a live holder.
    pub(crate) fn state(&self, host: &dyn Host) -> PayloadState {
        if policy::validate_tag(&self.tag).is_err() {
            return PayloadState::Invalid(format!("the tag '{}' is not valid", self.tag));
        }
        if !matches!(self.posture.as_str(), "denied" | "host") {
            return PayloadState::Invalid(format!("the posture '{}' is not valid", self.posture));
        }
        if !self.socket.is_absolute()
            || !self.workspace_host.is_absolute()
            || !self.workspace_sandbox.is_absolute()
            || self.reads.iter().any(|path| !path.is_absolute())
        {
            return PayloadState::Invalid("a recorded path is not absolute".to_string());
        }

        match host.pid_state(self.holder_pid) {
            Ok(PidState::Running) => {}
            Ok(PidState::Stopped) => return PayloadState::Paused,
            Ok(PidState::Zombie | PidState::Gone) => return PayloadState::Missing,
            Err(error) => {
                return PayloadState::Invalid(format!("the holder cannot be read: {error}"));
            }
        }
        match host.process_start(self.holder_pid) {
            Ok(start)
                if start.secs == self.holder_start_secs
                    && start.micros == self.holder_start_micros => {}
            Ok(_) => return PayloadState::Foreign,
            Err(error) => {
                return PayloadState::Invalid(format!(
                    "the holder start time is unreadable: {error}"
                ));
            }
        }
        PayloadState::Live
    }

    /// The socket path as a path.
    pub(crate) fn socket_path(&self) -> &Path {
        &self.socket
    }
}

#[derive(Deserialize)]
struct VersionProbe {
    version: u32,
}

fn unavailable(reason: String) -> Error {
    Error::Unavailable {
        backend: BACKEND.to_string(),
        remedy: "delete the sandbox and create it again".to_string(),
        reason,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{Payload, PayloadState, VERSION};
    use crate::backends::seatbelt::host::{FakeHost, ProcessStart};
    use crate::backends::seatbelt::mounts::CollapsedStep;
    use crate::sandbox::contract::{Error, SandboxRecord};
    use crate::sandbox::environment::{Origin, Provenance, Variable};

    fn sample() -> Payload {
        Payload {
            version: VERSION,
            helper: PathBuf::from("/usr/local/bin/litterbox-seatbelt-helper"),
            holder_pid: 42,
            holder_start_secs: 100,
            holder_start_micros: 7,
            socket: PathBuf::from("/private/tmp/lbx/one.sock"),
            workspace_host: PathBuf::from("/state/workspace"),
            workspace_sandbox: PathBuf::from("/state/workspace"),
            reads: vec![PathBuf::from("/nix/store/abc")],
            collapsed: vec![CollapsedStep {
                identity: "dev".to_string(),
                contributed: 238,
                granted: PathBuf::from("/nix/store"),
            }],
            provenance: Provenance {
                origin: Origin::Captured {
                    from: "print-dev-env".to_string(),
                },
                variables: vec![Variable {
                    name: "RUSTFLAGS".to_string(),
                    change: crate::sandbox::environment::Change::Unchanged,
                }],
                host_paths: Vec::new(),
                steps: Vec::new(),
            },
            posture: "denied".to_string(),
            tag: "demo".to_string(),
        }
    }

    fn as_record(payload: &Payload) -> SandboxRecord {
        SandboxRecord {
            name: "demo".to_string(),
            backend: "seatbelt".to_string(),
            project_slug: "project".to_string(),
            data: payload.to_value().expect("value"),
        }
    }

    fn host() -> FakeHost {
        let host = FakeHost::new("/Users/tester");
        host.insert_process(
            42,
            ProcessStart {
                secs: 100,
                micros: 7,
            },
        );
        host
    }

    #[test]
    fn a_payload_round_trips_through_a_record() {
        let payload = sample();
        let record = as_record(&payload);
        assert_eq!(Payload::from_record(&record).expect("payload"), payload);
    }

    #[test]
    fn a_newer_older_or_incomplete_edition_is_refused() {
        let mut newer = sample();
        newer.version = VERSION + 1;
        let record = as_record(&newer);
        assert!(matches!(
            Payload::from_record(&record),
            Err(Error::Unavailable { .. })
        ));

        let mut older = sample();
        older.version = VERSION - 1;
        let record = as_record(&older);
        assert!(matches!(
            Payload::from_record(&record),
            Err(Error::Unavailable { .. })
        ));

        let mut value = sample().to_value().expect("value");
        let table = value.as_table_mut().expect("table");
        table.remove("socket");
        let record = SandboxRecord {
            name: "demo".to_string(),
            backend: "seatbelt".to_string(),
            project_slug: "project".to_string(),
            data: value,
        };
        assert!(matches!(
            Payload::from_record(&record),
            Err(Error::Unavailable { .. })
        ));
    }

    #[test]
    fn the_state_follows_the_holder() {
        let payload = sample();
        let host = host();
        assert_eq!(payload.state(&host), PayloadState::Live);

        host.set_state(42, crate::backends::seatbelt::host::PidState::Stopped);
        assert_eq!(payload.state(&host), PayloadState::Paused);
        host.set_state(42, crate::backends::seatbelt::host::PidState::Running);

        host.set_state(42, crate::backends::seatbelt::host::PidState::Zombie);
        assert_eq!(payload.state(&host), PayloadState::Missing);

        host.insert_process(
            42,
            ProcessStart {
                secs: 101,
                micros: 7,
            },
        );
        assert_eq!(payload.state(&host), PayloadState::Foreign);

        let missing = FakeHost::new("/Users/tester");
        assert_eq!(payload.state(&missing), PayloadState::Missing);
    }

    #[test]
    fn an_invalid_tag_posture_or_path_is_reported() {
        let host = host();
        let mut payload = sample();
        payload.tag = "with space".to_string();
        assert!(matches!(payload.state(&host), PayloadState::Invalid(_)));

        let mut payload = sample();
        payload.posture = "sometimes".to_string();
        assert!(matches!(payload.state(&host), PayloadState::Invalid(_)));

        let mut payload = sample();
        payload.socket = PathBuf::from("relative");
        assert!(matches!(payload.state(&host), PayloadState::Invalid(_)));
    }

    #[test]
    fn the_payload_holds_no_environment_value() {
        let text = toml::to_string(&sample().to_value().expect("value")).expect("toml");
        assert!(text.contains("RUSTFLAGS"));
        assert!(!text.contains("debuginfo"));
    }
}
