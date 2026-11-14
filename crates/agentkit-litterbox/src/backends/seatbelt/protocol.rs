//! The holder's wire protocol.
//!
//! A frame is a 12-byte header and a payload. The encoding is deliberately
//! simple: the helper has no serde and must stay dependency-free.

use std::fmt;

const MAGIC: &[u8; 4] = b"LBX1";
const VERSION: u16 = 1;
const HEADER_LEN: usize = 12;
pub(crate) const MAX_FRAME: usize = 1024 * 1024;
const MAX_STREAM: usize = 64 * 1024;
const MAX_STRING: usize = 64 * 1024;
const MAX_COUNT: usize = 4096;

/// A frame kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Config = 1,
    Ready = 2,
    Exec = 3,
    Stdout = 4,
    Stderr = 5,
    Exit = 6,
    Error = 7,
    Status = 8,
    StatusReply = 9,
}

impl Kind {
    fn from_byte(value: u8) -> Result<Self, ProtocolError> {
        match value {
            1 => Ok(Self::Config),
            2 => Ok(Self::Ready),
            3 => Ok(Self::Exec),
            4 => Ok(Self::Stdout),
            5 => Ok(Self::Stderr),
            6 => Ok(Self::Exit),
            7 => Ok(Self::Error),
            8 => Ok(Self::Status),
            9 => Ok(Self::StatusReply),
            other => Err(ProtocolError::UnknownKind(other)),
        }
    }
}

/// How the sandbox reaches the network, on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WirePosture {
    Denied,
    Host,
    Loopback,
}

impl WirePosture {
    fn to_byte(self) -> u8 {
        match self {
            Self::Denied => 0,
            Self::Host => 1,
            Self::Loopback => 2,
        }
    }

    fn from_byte(value: u8) -> Result<Self, ProtocolError> {
        match value {
            0 => Ok(Self::Denied),
            1 => Ok(Self::Host),
            2 => Ok(Self::Loopback),
            other => Err(ProtocolError::InvalidPosture(other)),
        }
    }
}

/// Why a child failed, on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ErrorCode {
    Apply = 1,
    Policy = 2,
    Exec = 3,
    Protocol = 4,
    Internal = 5,
    Busy = 6,
}

impl ErrorCode {
    fn to_byte(self) -> u8 {
        match self {
            Self::Apply => 1,
            Self::Policy => 2,
            Self::Exec => 3,
            Self::Protocol => 4,
            Self::Internal => 5,
            Self::Busy => 6,
        }
    }

    fn from_byte(value: u8) -> Result<Self, ProtocolError> {
        match value {
            1 => Ok(Self::Apply),
            2 => Ok(Self::Policy),
            3 => Ok(Self::Exec),
            4 => Ok(Self::Protocol),
            5 => Ok(Self::Internal),
            6 => Ok(Self::Busy),
            other => Err(ProtocolError::InvalidErrorCode(other)),
        }
    }
}

/// Which streams a command overflowed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ExitFlags {
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

impl ExitFlags {
    fn to_byte(self) -> u8 {
        u8::from(self.stdout_truncated) | (u8::from(self.stderr_truncated) << 1)
    }

    fn from_byte(value: u8) -> Self {
        Self {
            stdout_truncated: value & 1 != 0,
            stderr_truncated: value & 2 != 0,
        }
    }
}

/// An environment as name and value bytes.
pub(crate) type Environment = Vec<(Vec<u8>, Vec<u8>)>;

/// Everything the holder needs to render a profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ConfigPayload {
    pub workspace: Vec<u8>,
    pub reads: Vec<Vec<u8>>,
    pub writes: Vec<Vec<u8>>,
    pub posture: WirePosture,
    pub endpoints: Vec<Vec<u8>>,
    pub tag: Vec<u8>,
    pub environment: Environment,
    pub baseline_reads: Vec<Vec<u8>>,
    pub mach_services: Vec<Vec<u8>>,
}

/// One protocol message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Message {
    Config(Box<ConfigPayload>),
    Ready,
    Exec(Vec<Vec<u8>>),
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    Exit { code: i32, flags: ExitFlags },
    Error { code: ErrorCode, message: String },
    Status,
    StatusReply,
}

impl Message {
    pub(crate) fn kind(&self) -> Kind {
        match self {
            Self::Config(_) => Kind::Config,
            Self::Ready => Kind::Ready,
            Self::Exec(_) => Kind::Exec,
            Self::Stdout(_) => Kind::Stdout,
            Self::Stderr(_) => Kind::Stderr,
            Self::Exit { .. } => Kind::Exit,
            Self::Error { .. } => Kind::Error,
            Self::Status => Kind::Status,
            Self::StatusReply => Kind::StatusReply,
        }
    }

    /// Encode a complete frame.
    pub(crate) fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        let payload = self.encode_payload()?;
        let limit = match self {
            Self::Stdout(_) | Self::Stderr(_) => MAX_STREAM,
            _ => MAX_FRAME,
        };
        if payload.len() > limit {
            return Err(ProtocolError::TooLarge {
                length: payload.len(),
            });
        }

        let mut frame = Vec::with_capacity(HEADER_LEN + payload.len());
        frame.extend_from_slice(MAGIC);
        frame.extend_from_slice(&VERSION.to_le_bytes());
        frame.push(self.kind() as u8);
        frame.push(0);
        let length = u32::try_from(payload.len()).map_err(|_| ProtocolError::TooLarge {
            length: payload.len(),
        })?;
        frame.extend_from_slice(&length.to_le_bytes());
        frame.extend_from_slice(&payload);
        Ok(frame)
    }

    /// Decode one complete frame.
    pub(crate) fn decode(frame: &[u8]) -> Result<Self, ProtocolError> {
        let header = Header::decode(frame)?;
        let end = HEADER_LEN + header.length as usize;
        if frame.len() < end {
            return Err(ProtocolError::Truncated {
                expected: end,
                found: frame.len(),
            });
        }
        if frame.len() > end {
            return Err(ProtocolError::TrailingBytes);
        }
        Self::decode_payload(header.kind, &frame[HEADER_LEN..end])
    }

    /// Decode a payload once its header has been read.
    pub(crate) fn decode_payload(kind: Kind, payload: &[u8]) -> Result<Self, ProtocolError> {
        let mut reader = Reader::new(payload);
        let message = match kind {
            Kind::Config => {
                let workspace = reader.string()?;
                let reads = reader.strings_u16()?;
                let writes = reader.strings_u16()?;
                let posture = WirePosture::from_byte(reader.u8()?)?;
                let endpoints = match posture {
                    WirePosture::Loopback => reader.strings_u16()?,
                    _ => Vec::new(),
                };
                let tag = reader.string()?;
                let environment = reader.environment()?;
                let baseline_reads = reader.strings_u16()?;
                let mach_services = reader.strings_u16()?;
                Self::Config(Box::new(ConfigPayload {
                    workspace,
                    reads,
                    writes,
                    posture,
                    endpoints,
                    tag,
                    environment,
                    baseline_reads,
                    mach_services,
                }))
            }
            Kind::Ready => Self::Ready,
            Kind::Exec => Self::Exec(reader.strings_u16()?),
            Kind::Stdout => Self::Stdout(reader.raw(MAX_STREAM)?),
            Kind::Stderr => Self::Stderr(reader.raw(MAX_STREAM)?),
            Kind::Exit => Self::Exit {
                code: i32::from_le_bytes(reader.array::<4>()?),
                flags: ExitFlags::from_byte(reader.u8()?),
            },
            Kind::Error => Self::Error {
                code: ErrorCode::from_byte(reader.u8()?)?,
                message: String::from_utf8(reader.string()?)
                    .map_err(|_| ProtocolError::InvalidText)?,
            },
            Kind::Status => Self::Status,
            Kind::StatusReply => Self::StatusReply,
        };
        reader.finish()?;
        Ok(message)
    }

    fn encode_payload(&self) -> Result<Vec<u8>, ProtocolError> {
        let mut writer = Writer::default();
        match self {
            Self::Config(config) => {
                writer.string(&config.workspace)?;
                writer.strings_u16(&config.reads)?;
                writer.strings_u16(&config.writes)?;
                writer.u8(config.posture.to_byte());
                if config.posture == WirePosture::Loopback {
                    writer.strings_u16(&config.endpoints)?;
                }
                writer.string(&config.tag)?;
                writer.environment(&config.environment)?;
                writer.strings_u16(&config.baseline_reads)?;
                writer.strings_u16(&config.mach_services)?;
            }
            Self::Ready | Self::Status | Self::StatusReply => {}
            Self::Exec(argv) => writer.strings_u16(argv)?,
            Self::Stdout(bytes) | Self::Stderr(bytes) => {
                if bytes.len() > MAX_STREAM {
                    return Err(ProtocolError::TooLarge {
                        length: bytes.len(),
                    });
                }
                writer.bytes(bytes);
            }
            Self::Exit { code, flags } => {
                writer.bytes(&code.to_le_bytes());
                writer.u8(flags.to_byte());
            }
            Self::Error { code, message } => {
                writer.u8(code.to_byte());
                writer.string(message.as_bytes())?;
            }
        }
        Ok(writer.finish())
    }
}

/// The fixed part of a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Header {
    pub kind: Kind,
    pub length: u32,
}

impl Header {
    pub(crate) fn decode(frame: &[u8]) -> Result<Self, ProtocolError> {
        if frame.len() < HEADER_LEN {
            return Err(ProtocolError::ShortHeader { found: frame.len() });
        }
        if !frame.starts_with(MAGIC) {
            return Err(ProtocolError::BadMagic);
        }
        let version = u16::from_le_bytes([frame[4], frame[5]]);
        if version != VERSION {
            return Err(ProtocolError::BadVersion(version));
        }
        let kind = Kind::from_byte(frame[6])?;
        if frame[7] != 0 {
            return Err(ProtocolError::BadFlags(frame[7]));
        }
        let length = u32::from_le_bytes([frame[8], frame[9], frame[10], frame[11]]);
        if length as usize > MAX_FRAME {
            return Err(ProtocolError::TooLarge {
                length: length as usize,
            });
        }
        Ok(Self { kind, length })
    }
}

/// Why a frame was rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ProtocolError {
    ShortHeader { found: usize },
    BadMagic,
    BadVersion(u16),
    BadFlags(u8),
    UnknownKind(u8),
    InvalidPosture(u8),
    InvalidErrorCode(u8),
    InvalidText,
    TooLarge { length: usize },
    TooMany { field: &'static str, found: usize },
    Truncated { expected: usize, found: usize },
    TrailingBytes,
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ShortHeader { found } => {
                write!(f, "the header is {found} bytes, expected {HEADER_LEN}")
            }
            Self::BadMagic => write!(f, "the frame magic is not LBX1"),
            Self::BadVersion(version) => {
                write!(f, "the protocol version {version} is not {VERSION}")
            }
            Self::BadFlags(flags) => write!(f, "the header flags are {flags}, expected 0"),
            Self::UnknownKind(kind) => write!(f, "the frame kind {kind} is unknown"),
            Self::InvalidPosture(posture) => write!(f, "the posture {posture} is unknown"),
            Self::InvalidErrorCode(code) => write!(f, "the error code {code} is unknown"),
            Self::InvalidText => write!(f, "a text field is not UTF-8"),
            Self::TooLarge { length } => {
                write!(f, "the payload is {length} bytes, above the limit")
            }
            Self::TooMany { field, found } => {
                write!(f, "{field} holds {found} entries, above the limit")
            }
            Self::Truncated { expected, found } => {
                write!(f, "the frame is {found} bytes, expected {expected}")
            }
            Self::TrailingBytes => write!(f, "the frame has bytes after its payload"),
        }
    }
}

impl std::error::Error for ProtocolError {}

#[derive(Default)]
struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    fn finish(self) -> Vec<u8> {
        self.bytes
    }

    fn bytes(&mut self, value: &[u8]) {
        self.bytes.extend_from_slice(value);
    }

    fn u8(&mut self, value: u8) {
        self.bytes.push(value);
    }

    fn u16(&mut self, value: u16) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn u32(&mut self, value: usize, field: &'static str) -> Result<(), ProtocolError> {
        let value = u32::try_from(value).map_err(|_| ProtocolError::TooMany {
            field,
            found: value,
        })?;
        self.bytes.extend_from_slice(&value.to_le_bytes());
        Ok(())
    }

    fn string(&mut self, value: &[u8]) -> Result<(), ProtocolError> {
        if value.len() > MAX_STRING {
            return Err(ProtocolError::TooLarge {
                length: value.len(),
            });
        }
        self.u32(value.len(), "string")?;
        self.bytes(value);
        Ok(())
    }

    fn strings_u16(&mut self, values: &[Vec<u8>]) -> Result<(), ProtocolError> {
        if values.len() > MAX_COUNT {
            return Err(ProtocolError::TooMany {
                field: "list",
                found: values.len(),
            });
        }
        let count = u16::try_from(values.len()).map_err(|_| ProtocolError::TooMany {
            field: "list",
            found: values.len(),
        })?;
        self.u16(count);
        for value in values {
            self.string(value)?;
        }
        Ok(())
    }

    fn environment(&mut self, values: &Environment) -> Result<(), ProtocolError> {
        if values.len() > MAX_COUNT {
            return Err(ProtocolError::TooMany {
                field: "environment",
                found: values.len(),
            });
        }
        self.u32(values.len(), "environment")?;
        for (name, value) in values {
            if name.len() > u16::MAX as usize {
                return Err(ProtocolError::TooLarge { length: name.len() });
            }
            let length = u16::try_from(name.len())
                .map_err(|_| ProtocolError::TooLarge { length: name.len() })?;
            self.u16(length);
            self.bytes(name);
            self.string(value)?;
        }
        Ok(())
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn finish(&self) -> Result<(), ProtocolError> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(ProtocolError::TrailingBytes)
        }
    }

    fn raw(&mut self, limit: usize) -> Result<Vec<u8>, ProtocolError> {
        if self.bytes.len() - self.offset > limit {
            return Err(ProtocolError::TooLarge {
                length: self.bytes.len() - self.offset,
            });
        }
        let value = self.bytes[self.offset..].to_vec();
        self.offset = self.bytes.len();
        Ok(value)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], ProtocolError> {
        let end = self.offset + N;
        if end > self.bytes.len() {
            return Err(ProtocolError::Truncated {
                expected: end,
                found: self.bytes.len(),
            });
        }
        let mut value = [0u8; N];
        value.copy_from_slice(&self.bytes[self.offset..end]);
        self.offset = end;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8, ProtocolError> {
        Ok(self.array::<1>()?[0])
    }

    fn u16(&mut self) -> Result<u16, ProtocolError> {
        Ok(u16::from_le_bytes(self.array::<2>()?))
    }

    fn u32(&mut self) -> Result<u32, ProtocolError> {
        Ok(u32::from_le_bytes(self.array::<4>()?))
    }

    fn string(&mut self) -> Result<Vec<u8>, ProtocolError> {
        let length = self.u32()? as usize;
        if length > MAX_STRING {
            return Err(ProtocolError::TooLarge { length });
        }
        let end = self.offset + length;
        if end > self.bytes.len() {
            return Err(ProtocolError::Truncated {
                expected: end,
                found: self.bytes.len(),
            });
        }
        let value = self.bytes[self.offset..end].to_vec();
        self.offset = end;
        Ok(value)
    }

    fn strings_u16(&mut self) -> Result<Vec<Vec<u8>>, ProtocolError> {
        let count = self.u16()? as usize;
        if count > MAX_COUNT {
            return Err(ProtocolError::TooMany {
                field: "list",
                found: count,
            });
        }
        let mut values = Vec::with_capacity(count);
        for _ in 0..count {
            values.push(self.string()?);
        }
        Ok(values)
    }

    fn environment(&mut self) -> Result<Environment, ProtocolError> {
        let count = self.u32()? as usize;
        if count > MAX_COUNT {
            return Err(ProtocolError::TooMany {
                field: "environment",
                found: count,
            });
        }
        let mut values = Vec::with_capacity(count);
        for _ in 0..count {
            let length = self.u16()? as usize;
            let end = self.offset + length;
            if end > self.bytes.len() {
                return Err(ProtocolError::Truncated {
                    expected: end,
                    found: self.bytes.len(),
                });
            }
            let name = self.bytes[self.offset..end].to_vec();
            self.offset = end;
            let value = self.string()?;
            values.push((name, value));
        }
        Ok(values)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ConfigPayload, ErrorCode, ExitFlags, Header, Kind, MAX_COUNT, MAX_FRAME, MAX_STREAM,
        MAX_STRING, Message, ProtocolError, WirePosture,
    };

    fn config() -> ConfigPayload {
        ConfigPayload {
            workspace: b"/private/tmp/ws".to_vec(),
            reads: vec![b"/private/tmp/read".to_vec()],
            writes: vec![b"/private/tmp/ws".to_vec()],
            posture: WirePosture::Loopback,
            endpoints: vec![b"localhost:8765".to_vec()],
            tag: b"demo".to_vec(),
            environment: vec![(b"PATH".to_vec(), b"/usr/bin".to_vec())],
            baseline_reads: vec![b"/usr/bin".to_vec()],
            mach_services: vec![b"com.apple.example".to_vec()],
        }
    }

    fn messages() -> Vec<Message> {
        vec![
            Message::Config(Box::new(config())),
            Message::Ready,
            Message::Exec(vec![b"/bin/sh".to_vec(), b"-c".to_vec(), b"true".to_vec()]),
            Message::Stdout(b"out".to_vec()),
            Message::Stderr(b"err".to_vec()),
            Message::Exit {
                code: 7,
                flags: ExitFlags {
                    stdout_truncated: true,
                    stderr_truncated: false,
                },
            },
            Message::Error {
                code: ErrorCode::Apply,
                message: "sandbox_init_with_parameters: bad profile".to_string(),
            },
            Message::Status,
            Message::StatusReply,
        ]
    }

    #[test]
    fn every_message_round_trips() {
        for message in messages() {
            let frame = message.encode().expect("encodes");
            assert_eq!(Message::decode(&frame).expect("decodes"), message);
        }
    }

    #[test]
    fn the_header_names_the_kind_and_length() {
        let frame = Message::Status.encode().expect("encodes");
        let header = Header::decode(&frame).expect("header");
        assert_eq!(header.kind, Kind::Status);
        assert_eq!(header.length, 0);
    }

    #[test]
    fn a_short_header_is_refused() {
        assert!(matches!(
            Message::decode(b"LBX1"),
            Err(ProtocolError::ShortHeader { .. })
        ));
    }

    #[test]
    fn bad_magic_version_flags_and_kind_are_refused() {
        let mut frame = Message::Status.encode().expect("encodes");
        frame[0] = b'X';
        assert!(matches!(
            Message::decode(&frame),
            Err(ProtocolError::BadMagic)
        ));

        let mut frame = Message::Status.encode().expect("encodes");
        frame[4] = 2;
        assert!(matches!(
            Message::decode(&frame),
            Err(ProtocolError::BadVersion(2))
        ));

        let mut frame = Message::Status.encode().expect("encodes");
        frame[7] = 1;
        assert!(matches!(
            Message::decode(&frame),
            Err(ProtocolError::BadFlags(1))
        ));

        let mut frame = Message::Status.encode().expect("encodes");
        frame[6] = 99;
        assert!(matches!(
            Message::decode(&frame),
            Err(ProtocolError::UnknownKind(99))
        ));
    }

    #[test]
    fn an_over_length_frame_is_refused_before_allocation() {
        let mut frame = Message::Status.encode().expect("encodes");
        frame[8..12].copy_from_slice(&(MAX_FRAME as u32 + 1).to_le_bytes());
        assert!(matches!(
            Message::decode(&frame),
            Err(ProtocolError::TooLarge { .. })
        ));
    }

    #[test]
    fn trailing_and_missing_bytes_are_refused() {
        let mut frame = Message::Status.encode().expect("encodes");
        frame.push(0);
        assert!(matches!(
            Message::decode(&frame),
            Err(ProtocolError::TrailingBytes)
        ));

        let frame = Message::Stdout(b"out".to_vec()).encode().expect("encodes");
        assert!(matches!(
            Message::decode(&frame[..frame.len() - 1]),
            Err(ProtocolError::Truncated { .. })
        ));
    }

    #[test]
    fn stream_frames_are_bounded() {
        let error = Message::Stdout(vec![0; MAX_STREAM + 1])
            .encode()
            .expect_err("too large");
        assert!(matches!(error, ProtocolError::TooLarge { .. }));
    }

    #[test]
    fn counts_and_strings_are_bounded() {
        let too_many = vec![b"x".to_vec(); MAX_COUNT + 1];
        let error = Message::Exec(too_many).encode().expect_err("too many");
        assert!(matches!(error, ProtocolError::TooMany { .. }));

        let long = vec![b'x'; MAX_STRING + 1];
        let error = Message::Config(Box::new(ConfigPayload {
            workspace: long,
            ..config()
        }))
        .encode()
        .expect_err("too long");
        assert!(matches!(error, ProtocolError::TooLarge { .. }));
    }

    #[test]
    fn an_unknown_posture_or_error_code_is_refused() {
        let mut posture_config = config();
        posture_config.reads.clear();
        posture_config.writes.clear();
        posture_config.endpoints.clear();
        posture_config.posture = WirePosture::Denied;
        let posture_offset = 12 + 4 + posture_config.workspace.len() + 2 + 2;
        let mut frame = Message::Config(Box::new(posture_config))
            .encode()
            .expect("encodes");
        frame[posture_offset] = 9;
        assert!(matches!(
            Message::decode(&frame),
            Err(ProtocolError::InvalidPosture(9))
        ));

        let mut frame = Message::Error {
            code: ErrorCode::Apply,
            message: "x".to_string(),
        }
        .encode()
        .expect("encodes");
        frame[12] = 99;
        assert!(matches!(
            Message::decode(&frame),
            Err(ProtocolError::InvalidErrorCode(99))
        ));
    }
}
