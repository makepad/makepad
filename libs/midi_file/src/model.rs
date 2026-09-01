use crate::{parse, parse_with_options, write, write_with_options};
use crate::{MidiResult, ParseOptions, WriteError, WriteOptions};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    SingleTrack,
    Parallel,
    Sequential,
}

impl Format {
    pub fn as_u16(self) -> u16 {
        match self {
            Self::SingleTrack => 0,
            Self::Parallel => 1,
            Self::Sequential => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SmpteFramesPerSecond {
    Fps24,
    Fps25,
    /// The SMF -29 code: 30 drop-frame, timed as 30000/1001 fps.
    Fps29Drop,
    Fps30,
}

impl SmpteFramesPerSecond {
    pub fn smf_code(self) -> i8 {
        match self {
            Self::Fps24 => -24,
            Self::Fps25 => -25,
            Self::Fps29Drop => -29,
            Self::Fps30 => -30,
        }
    }

    pub fn ratio(self) -> (u32, u32) {
        match self {
            Self::Fps24 => (24, 1),
            Self::Fps25 => (25, 1),
            Self::Fps29Drop => (30_000, 1_001),
            Self::Fps30 => (30, 1),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Division {
    TicksPerQuarter(u16),
    Smpte {
        frames_per_second: SmpteFramesPerSecond,
        ticks_per_frame: u8,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub format: Format,
    pub track_count: u16,
    pub division: Division,
    /// Bytes following the standard six-byte header payload, if any.
    pub extra_data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MidiFile {
    pub header: Header,
    pub tracks: Vec<Track>,
    /// Non-MTrk chunks are skipped by musical views but retained for writing.
    pub unknown_chunks: Vec<UnknownChunk>,
}

impl MidiFile {
    pub fn parse(bytes: &[u8]) -> MidiResult<Self> {
        parse(bytes)
    }

    pub fn parse_with_options(bytes: &[u8], options: ParseOptions) -> MidiResult<Self> {
        parse_with_options(bytes, options)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, WriteError> {
        write(self)
    }

    pub fn to_bytes_with_options(&self, options: WriteOptions) -> Result<Vec<u8>, WriteError> {
        write_with_options(self, options)
    }

    /// Format 0/1 are one sequence; each format-2 track is an independent sequence.
    pub fn sequence_count(&self) -> usize {
        match self.header.format {
            Format::Sequential => self.tracks.len(),
            Format::SingleTrack | Format::Parallel => usize::from(!self.tracks.is_empty()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownChunk {
    pub id: [u8; 4],
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Track {
    pub events: Vec<TrackEvent>,
    /// Bytes after the first end-of-track event, commonly zero padding.
    pub trailing_data: Vec<u8>,
}

impl Track {
    pub fn has_end_of_track(&self) -> bool {
        self.events
            .iter()
            .any(|event| matches!(event.kind, EventKind::Meta(MetaEvent::EndOfTrack)))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackEvent {
    pub tick: u64,
    pub kind: EventKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventKind {
    Channel(ChannelEvent),
    Meta(MetaEvent),
    SysEx(SysExEvent),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelEvent {
    pub channel: u8,
    pub message: ChannelMessage,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChannelMessage {
    NoteOff { key: u8, velocity: u8 },
    NoteOn { key: u8, velocity: u8 },
    PolyphonicKeyPressure { key: u8, pressure: u8 },
    ControlChange { controller: u8, value: u8 },
    ProgramChange { program: u8 },
    ChannelPressure { pressure: u8 },
    /// Unsigned 14-bit value: 0..=16383, with center at 8192.
    PitchBend { value: u16 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MetaEvent {
    SequenceNumber(u16),
    Text(Vec<u8>),
    Copyright(Vec<u8>),
    SequenceOrTrackName(Vec<u8>),
    InstrumentName(Vec<u8>),
    Lyric(Vec<u8>),
    Marker(Vec<u8>),
    CuePoint(Vec<u8>),
    MidiChannelPrefix(u8),
    MidiPort(u8),
    EndOfTrack,
    SetTempo(u32),
    SmpteOffset([u8; 5]),
    TimeSignature(TimeSignature),
    KeySignature(KeySignature),
    SequencerSpecific(Vec<u8>),
    Unknown { kind: u8, data: Vec<u8> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeSignature {
    pub numerator: u8,
    /// The denominator is `2.pow(denominator_power)`.
    pub denominator_power: u8,
    pub midi_clocks_per_metronome_click: u8,
    pub thirty_second_notes_per_quarter: u8,
}

impl TimeSignature {
    pub fn denominator(self) -> Option<u32> {
        1_u32.checked_shl(self.denominator_power.into())
    }
}

impl Default for TimeSignature {
    fn default() -> Self {
        Self {
            numerator: 4,
            denominator_power: 2,
            midi_clocks_per_metronome_click: 24,
            thirty_second_notes_per_quarter: 8,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeySignature {
    pub sharps_flats: i8,
    pub is_minor: bool,
}

impl Default for KeySignature {
    fn default() -> Self {
        Self {
            sharps_flats: 0,
            is_minor: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SysExEvent {
    pub kind: SysExKind,
    /// Payload exactly as stored after the event's VLQ length.
    pub data: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SysExKind {
    F0,
    /// F7 escape or continuation form. Kept distinct so split SysEx is lossless.
    F7,
}
