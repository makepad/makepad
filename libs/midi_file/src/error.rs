use std::fmt;

pub type MidiResult<T> = Result<T, MidiError>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MidiError {
    pub offset: usize,
    pub kind: MidiErrorKind,
}

impl MidiError {
    pub(crate) fn new(offset: usize, kind: MidiErrorKind) -> Self {
        Self { offset, kind }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MidiErrorKind {
    UnexpectedEof {
        context: &'static str,
        needed: usize,
        remaining: usize,
    },
    ExpectedHeaderChunk([u8; 4]),
    InvalidHeaderLength(u32),
    UnsupportedFormat(u16),
    InvalidTrackCount(u16),
    InvalidFormatZeroTrackCount(u16),
    TrackCountMismatch {
        declared: u16,
        found: usize,
    },
    InvalidTicksPerQuarter(u16),
    InvalidSmpteFramesPerSecond(i8),
    InvalidTicksPerFrame(u8),
    ChunkLengthExceedsInput {
        chunk: [u8; 4],
        length: u32,
        remaining: usize,
    },
    TrailingChunkHeader(usize),
    InvalidVariableLengthQuantity,
    TickOverflow,
    RunningStatusWithoutStatus(u8),
    InvalidStatus(u8),
    InvalidDataByte(u8),
    InvalidMetaLength {
        kind: u8,
        expected: usize,
        actual: usize,
    },
    InvalidTempo(u32),
    InvalidKeySignature {
        sharps_flats: i8,
        scale: u8,
    },
    MissingEndOfTrack {
        track: usize,
    },
    IndependentSequencesRequireIndex,
    SequenceOutOfRange {
        sequence: usize,
        count: usize,
    },
}

impl fmt::Display for MidiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MIDI error at byte {}: ", self.offset)?;
        match &self.kind {
            MidiErrorKind::UnexpectedEof {
                context,
                needed,
                remaining,
            } => write!(
                f,
                "unexpected end while reading {context} (needed {needed}, had {remaining})"
            ),
            MidiErrorKind::ExpectedHeaderChunk(id) => {
                write!(f, "expected MThd, found {:?}", String::from_utf8_lossy(id))
            }
            MidiErrorKind::InvalidHeaderLength(length) => {
                write!(f, "header length {length} is smaller than 6")
            }
            MidiErrorKind::UnsupportedFormat(format) => {
                write!(f, "unsupported SMF format {format}")
            }
            MidiErrorKind::InvalidTrackCount(count) => write!(f, "invalid track count {count}"),
            MidiErrorKind::InvalidFormatZeroTrackCount(count) => {
                write!(f, "format 0 must declare exactly one track, found {count}")
            }
            MidiErrorKind::TrackCountMismatch { declared, found } => write!(
                f,
                "header declares {declared} tracks but file contains {found}"
            ),
            MidiErrorKind::InvalidTicksPerQuarter(value) => {
                write!(f, "invalid ticks-per-quarter division {value}")
            }
            MidiErrorKind::InvalidSmpteFramesPerSecond(value) => {
                write!(f, "invalid SMPTE frame code {value}")
            }
            MidiErrorKind::InvalidTicksPerFrame(value) => {
                write!(f, "invalid SMPTE ticks-per-frame {value}")
            }
            MidiErrorKind::ChunkLengthExceedsInput {
                chunk,
                length,
                remaining,
            } => write!(
                f,
                "chunk {:?} declares {length} bytes but only {remaining} remain",
                String::from_utf8_lossy(chunk)
            ),
            MidiErrorKind::TrailingChunkHeader(length) => {
                write!(f, "{length} trailing bytes cannot form a chunk header")
            }
            MidiErrorKind::InvalidVariableLengthQuantity => {
                write!(f, "variable-length quantity exceeds four bytes")
            }
            MidiErrorKind::TickOverflow => write!(f, "absolute tick overflow"),
            MidiErrorKind::RunningStatusWithoutStatus(byte) => write!(
                f,
                "data byte {byte:#04x} appears before a channel status"
            ),
            MidiErrorKind::InvalidStatus(status) => {
                write!(f, "status {status:#04x} is not valid in an SMF track")
            }
            MidiErrorKind::InvalidDataByte(byte) => {
                write!(f, "channel data byte {byte:#04x} has its high bit set")
            }
            MidiErrorKind::InvalidMetaLength {
                kind,
                expected,
                actual,
            } => write!(
                f,
                "meta event {kind:#04x} has length {actual}, expected {expected}"
            ),
            MidiErrorKind::InvalidTempo(tempo) => {
                write!(f, "set-tempo value {tempo} is invalid")
            }
            MidiErrorKind::InvalidKeySignature {
                sharps_flats,
                scale,
            } => write!(
                f,
                "invalid key signature accidentals={sharps_flats}, scale={scale}"
            ),
            MidiErrorKind::MissingEndOfTrack { track } => {
                write!(f, "track {track} has no end-of-track event")
            }
            MidiErrorKind::IndependentSequencesRequireIndex => {
                write!(f, "format 2 has independent sequences; select a track index")
            }
            MidiErrorKind::SequenceOutOfRange { sequence, count } => {
                write!(f, "sequence {sequence} is out of range for {count} sequences")
            }
        }
    }
}

impl std::error::Error for MidiError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteError {
    UnsupportedFormat(u16),
    InvalidTrackCount(usize),
    InvalidFormatZeroTrackCount(usize),
    TooManyTracks(usize),
    HeaderTrackCountMismatch { header: u16, actual: usize },
    EventsNotSorted { track: usize, previous: u64, next: u64 },
    DeltaTooLarge { track: usize, delta: u64 },
    VariableLengthValueTooLarge(u64),
    ChunkTooLarge(usize),
    InvalidChannel(u8),
    InvalidDataByte(u8),
    InvalidPitchBend(u16),
    InvalidTempo(u32),
    InvalidTicksPerQuarter(u16),
    InvalidTicksPerFrame(u8),
    InvalidKeySignature(i8),
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WriteError::UnsupportedFormat(format) => {
                write!(f, "writing SMF format {format} is not supported")
            }
            WriteError::InvalidTrackCount(count) => write!(f, "invalid track count {count}"),
            WriteError::InvalidFormatZeroTrackCount(count) => {
                write!(f, "format 0 must have exactly one track, found {count}")
            }
            WriteError::TooManyTracks(count) => write!(f, "{count} tracks do not fit in SMF"),
            WriteError::HeaderTrackCountMismatch { header, actual } => write!(
                f,
                "header track count {header} does not match {actual} tracks"
            ),
            WriteError::EventsNotSorted {
                track,
                previous,
                next,
            } => write!(
                f,
                "track {track} events are not sorted: tick {next} follows {previous}"
            ),
            WriteError::DeltaTooLarge { track, delta } => {
                write!(f, "track {track} delta {delta} exceeds the SMF VLQ limit")
            }
            WriteError::VariableLengthValueTooLarge(value) => {
                write!(f, "value {value} exceeds the SMF VLQ limit")
            }
            WriteError::ChunkTooLarge(length) => {
                write!(f, "chunk length {length} does not fit in u32")
            }
            WriteError::InvalidChannel(channel) => write!(f, "invalid MIDI channel {channel}"),
            WriteError::InvalidDataByte(byte) => write!(f, "invalid MIDI data byte {byte}"),
            WriteError::InvalidPitchBend(value) => {
                write!(f, "pitch bend {value} exceeds 14 bits")
            }
            WriteError::InvalidTempo(tempo) => write!(f, "invalid tempo {tempo}"),
            WriteError::InvalidTicksPerQuarter(value) => {
                write!(f, "invalid ticks-per-quarter division {value}")
            }
            WriteError::InvalidTicksPerFrame(value) => {
                write!(f, "invalid SMPTE ticks-per-frame {value}")
            }
            WriteError::InvalidKeySignature(value) => {
                write!(f, "invalid key-signature accidentals {value}")
            }
        }
    }
}

impl std::error::Error for WriteError {}
