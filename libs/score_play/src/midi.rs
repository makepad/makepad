//! Timestamped MIDI/UMP seam independent of the platform's legacy byte triplet.

/// Protocol carried by [`TimestampedMidiEvent`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MidiProtocol {
    Midi1Ump,
    Midi2Ump,
    System,
    Data,
}

/// Describes where an input timestamp was captured.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimestampQuality {
    /// Native timestamp from the MIDI backend or device protocol.
    Native,
    /// Stamped immediately in the backend input callback.
    ArrivalOnly,
    /// Estimated after crossing a less precise boundary.
    Estimated,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MidiPortCapabilities {
    pub midi1: bool,
    pub ump: bool,
    /// True only when the backend honors `host_time_ns` rather than sending immediately.
    pub scheduled_output: bool,
}

/// A timestamped, UMP-shaped MIDI event.
///
/// Backends normalize MIDI 1.0 byte streams into UMP and retain native SysEx7 chunks.
/// `host_time_ns` uses the same monotonic host clock as audio presentation anchors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimestampedMidiEvent {
    pub port: u32,
    pub group: u8,
    pub protocol: MidiProtocol,
    pub host_time_ns: u64,
    pub captured_sample: Option<u64>,
    pub timestamp_quality: TimestampQuality,
    pub word_count: u8,
    pub words: [u32; 4],
}

impl TimestampedMidiEvent {
    /// Constructs a packet when `word_count` and `group` fit UMP's representation.
    pub fn new(
        port: u32,
        group: u8,
        protocol: MidiProtocol,
        host_time_ns: u64,
        timestamp_quality: TimestampQuality,
        word_count: u8,
        words: [u32; 4],
    ) -> Option<Self> {
        if group > 0x0f || !(1..=4).contains(&word_count) {
            return None;
        }
        Some(Self {
            port,
            group,
            protocol,
            host_time_ns,
            captured_sample: None,
            timestamp_quality,
            word_count,
            words,
        })
    }

    /// Applies measured input latency while retaining the uncompensated host timestamp.
    pub fn with_audio_anchor(
        mut self,
        anchor_host_ns: u64,
        anchor_sample: u64,
        sample_rate: u32,
        input_latency_frames: u32,
    ) -> Self {
        let delta_ns = self.host_time_ns as i128 - anchor_host_ns as i128;
        let delta_frames = delta_ns * i128::from(sample_rate) / 1_000_000_000i128;
        let captured = i128::from(anchor_sample) + delta_frames - i128::from(input_latency_frames);
        self.captured_sample = Some(captured.max(0) as u64);
        self
    }
}

/// Platform adapter seam for planner-ahead external MIDI output.
///
/// A backend that cannot schedule may still implement this trait, but must advertise
/// `scheduled_output: false`; callers can then label the route best-effort while keeping
/// internal synthesis as the reference timing path.
pub trait TimedMidiOutput {
    type Error;

    fn capabilities(&self, port: u32) -> MidiPortCapabilities;
    fn send_at(&mut self, event: TimestampedMidiEvent) -> Result<(), Self::Error>;
}
