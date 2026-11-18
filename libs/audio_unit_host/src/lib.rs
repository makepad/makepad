//! Host a macOS Audio Unit instrument: an AUv2 music device (component type
//! `aumu`), whichever installed one the caller names.
//!
//! - [`instruments`] lists every music device the system has registered, by
//!   the name the system reports; [`find`] resolves a [`ComponentDescription`].
//! - [`Instrument::new`] instantiates and initialises one at a sample rate,
//!   with a non-interleaved stereo float output.
//! - [`Instrument::midi`] sends it a MIDI channel message
//!   (`MusicDeviceMIDIEvent`) at a frame offset into the next render, and
//!   [`Instrument::render`] pulls the next frames with `AudioUnitRender`.
//!
//! Offline, a caller drives one [`Instrument`] itself: events, then frames,
//! in order, on one thread, for a deterministic render (see
//! [`Instrument::set_offline`]). Live, [`live`] splits it into a
//! [`MidiSender`] for the UI and a [`LiveInstrument`] for the audio callback,
//! joined by a bounded lock-free ring: the callback drains the ring and
//! renders without allocating, locking or waiting, and a full ring hands the
//! message back to the sender to retry later.
//!
//! Building and dropping an [`Instrument`] loads and unloads the plug-in, and
//! may take a while: do both off the audio thread.
//!
//! Off macOS the same API exists and is empty: [`instruments`] lists
//! nothing and [`Instrument::new`] returns [`Error::unsupported`].

use std::fmt;

#[cfg(target_os = "macos")]
mod apple;
#[cfg(target_os = "macos")]
use apple as backend;

#[cfg(not(target_os = "macos"))]
mod other;
#[cfg(not(target_os = "macos"))]
use other as backend;

/// A four-character code as Core Audio packs it: big-endian ASCII.
pub const fn four_cc(code: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*code)
}

/// The component type of a music device (an instrument).
pub const MUSIC_DEVICE: u32 = four_cc(b"aumu");

/// Which component to load. A zero field matches anything, as in
/// `AudioComponentFindNext`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ComponentDescription {
    pub kind: u32,
    pub sub_kind: u32,
    pub manufacturer: u32,
}

impl ComponentDescription {
    /// Apple's General MIDI synth, which ships with macOS.
    pub const DLS_SYNTH: ComponentDescription = ComponentDescription {
        kind: MUSIC_DEVICE,
        sub_kind: four_cc(b"dls "),
        manufacturer: four_cc(b"appl"),
    };

    pub const fn new(kind: &[u8; 4], sub_kind: &[u8; 4], manufacturer: &[u8; 4]) -> Self {
        ComponentDescription { kind: four_cc(kind), sub_kind: four_cc(sub_kind), manufacturer: four_cc(manufacturer) }
    }

    /// `type/subtype/manufacturer` as three four-character codes, the form
    /// [`ComponentDescription::parse`] reads back (`aumu/dls /appl`).
    pub fn code(&self) -> String {
        format!("{}/{}/{}", code_text(self.kind), code_text(self.sub_kind), code_text(self.manufacturer))
    }

    /// The form [`ComponentDescription::code`] writes.
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.split('/');
        let mut next = || -> Option<u32> {
            let part = parts.next()?.as_bytes();
            let bytes: [u8; 4] = part.try_into().ok()?;
            Some(four_cc(&bytes))
        };
        let desc = ComponentDescription { kind: next()?, sub_kind: next()?, manufacturer: next()? };
        parts.next().is_none().then_some(desc)
    }
}

fn code_text(code: u32) -> String {
    code.to_be_bytes().iter().map(|&b| if b.is_ascii_graphic() || b == b' ' { b as char } else { '?' }).collect()
}

/// One installed component, as the system reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComponentInfo {
    pub description: ComponentDescription,
    /// The system's name for it, conventionally `Manufacturer: Name`.
    pub name: String,
    pub version: u32,
}

impl ComponentInfo {
    /// The part of [`ComponentInfo::name`] after the manufacturer.
    pub fn short_name(&self) -> &str {
        self.name.split_once(": ").map_or(self.name.as_str(), |(_, name)| name)
    }
}

/// Every installed music device, in the system's order.
pub fn instruments() -> Vec<ComponentInfo> {
    backend::components(&ComponentDescription { kind: MUSIC_DEVICE, sub_kind: 0, manufacturer: 0 })
}

/// The first installed component matching `description` (zero fields are
/// wildcards).
pub fn find(description: &ComponentDescription) -> Option<ComponentInfo> {
    backend::components(description).into_iter().next()
}

/// A failed Core Audio call: which one, and the status it returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error {
    pub call: &'static str,
    pub status: i32,
}

impl Error {
    /// The status [`Instrument::new`] returns off macOS.
    pub const UNSUPPORTED: i32 = -4;

    pub const fn unsupported() -> Self {
        Error { call: "audio units are macOS only", status: Self::UNSUPPORTED }
    }

    /// No installed component matches the description.
    pub const fn not_found() -> Self {
        Error { call: "AudioComponentFindNext: no such component", status: -1 }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = self.status.to_be_bytes();
        if bytes.iter().all(|b| b.is_ascii_graphic()) {
            write!(f, "{} failed: '{}' ({})", self.call, String::from_utf8_lossy(&bytes), self.status)
        } else {
            write!(f, "{} failed: {}", self.call, self.status)
        }
    }
}

impl std::error::Error for Error {}

/// One of a plug-in's global parameters, as it describes itself.
#[derive(Clone, Debug, PartialEq)]
pub struct ParameterInfo {
    pub id: u32,
    pub name: String,
    pub min: f32,
    pub max: f32,
    pub default: f32,
}

/// One MIDI channel message: status byte (with channel) and two data bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiMessage {
    pub status: u8,
    pub data1: u8,
    pub data2: u8,
}

impl MidiMessage {
    pub const fn note_on(channel: u8, key: u8, velocity: u8) -> Self {
        MidiMessage { status: 0x90 | (channel & 15), data1: key & 127, data2: velocity & 127 }
    }

    pub const fn note_off(channel: u8, key: u8) -> Self {
        MidiMessage { status: 0x80 | (channel & 15), data1: key & 127, data2: 0 }
    }

    pub const fn control(channel: u8, controller: u8, value: u8) -> Self {
        MidiMessage { status: 0xB0 | (channel & 15), data1: controller & 127, data2: value & 127 }
    }

    /// Controller 123: every note on the channel off (pedals stay).
    pub const fn all_notes_off(channel: u8) -> Self {
        Self::control(channel, 123, 0)
    }
}

/// A loaded, initialised music device with a stereo float output.
///
/// Owned by one thread at a time; `Send` so it can be built on one thread
/// and rendered on another.
pub struct Instrument {
    unit: backend::Unit,
    info: ComponentInfo,
    sample_rate: f64,
    max_frames: u32,
}

impl Instrument {
    /// Load the first component matching `description`, set its output to
    /// non-interleaved stereo `f32` at `sample_rate`, let it render up to
    /// `max_frames` per call to the plug-in, and initialise it. Slow and
    /// allocating: never on the audio thread.
    pub fn new(description: &ComponentDescription, sample_rate: f64, max_frames: u32) -> Result<Self, Error> {
        let max_frames = max_frames.max(1);
        let (unit, info) = backend::Unit::new(description, sample_rate, max_frames)?;
        Ok(Instrument { unit, info, sample_rate, max_frames })
    }

    pub fn info(&self) -> &ComponentInfo {
        &self.info
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// The most frames one call into the plug-in renders; [`Instrument::render`]
    /// splits longer spans.
    pub fn max_frames(&self) -> u32 {
        self.max_frames
    }

    /// Tell the plug-in it renders offline (not against a device clock), so it
    /// may trade time for quality and need not drop voices to keep up. Not
    /// every plug-in supports it; a refusal is returned and changes nothing.
    pub fn set_offline(&mut self, offline: bool) -> Result<(), Error> {
        self.unit.set_offline(offline)
    }

    /// The plug-in's reported processing latency, in seconds (0 when it
    /// reports none).
    pub fn latency_seconds(&self) -> f64 {
        self.unit.latency_seconds()
    }

    /// Send one MIDI channel message, `offset` frames into the next
    /// [`Instrument::render`] (keep it below that render's first slice,
    /// [`Instrument::max_frames`]). Allocation-free.
    pub fn midi(&mut self, message: MidiMessage, offset: u32) -> Result<(), Error> {
        self.unit.midi(message, offset)
    }

    pub fn note_on(&mut self, channel: u8, key: u8, velocity: u8) -> Result<(), Error> {
        self.midi(MidiMessage::note_on(channel, key, velocity), 0)
    }

    pub fn note_off(&mut self, channel: u8, key: u8) -> Result<(), Error> {
        self.midi(MidiMessage::note_off(channel, key), 0)
    }

    pub fn control(&mut self, channel: u8, controller: u8, value: u8) -> Result<(), Error> {
        self.midi(MidiMessage::control(channel, controller, value), 0)
    }

    /// Render the next `left.len()` frames into both slices (overwriting
    /// them). Allocation-free; spans longer than [`Instrument::max_frames`]
    /// go to the plug-in in slices. Both slices must be the same length.
    pub fn render(&mut self, left: &mut [f32], right: &mut [f32]) -> Result<(), Error> {
        let frames = left.len().min(right.len());
        let mut at = 0;
        while at < frames {
            let n = (frames - at).min(self.max_frames as usize);
            self.unit.render(&mut left[at..at + n], &mut right[at..at + n])?;
            at += n;
        }
        Ok(())
    }

    /// The plug-in's global parameters, by its own names. Allocates: not on
    /// the audio thread.
    pub fn parameters(&self) -> Vec<ParameterInfo> {
        self.unit.parameters()
    }

    pub fn parameter(&self, id: u32) -> Result<f32, Error> {
        self.unit.parameter(id)
    }

    pub fn set_parameter(&mut self, id: u32, value: f32) -> Result<(), Error> {
        self.unit.set_parameter(id, value)
    }

    /// How the plug-in itself writes `value` of parameter `id` (with its
    /// unit), when it says.
    pub fn parameter_text(&self, id: u32, value: f32) -> Option<String> {
        self.unit.parameter_text(id, value)
    }

    /// The plug-in's own presets: number and name.
    pub fn factory_presets(&self) -> Vec<(i32, String)> {
        self.unit.factory_presets()
    }

    /// Load one of [`Instrument::factory_presets`] by its number.
    pub fn set_factory_preset(&mut self, number: i32) -> Result<(), Error> {
        self.unit.set_factory_preset(number)
    }

    /// Silence every voice and clear the plug-in's state (`AudioUnitReset`).
    pub fn reset(&mut self) -> Result<(), Error> {
        self.unit.reset()
    }
}

/// The UI side of a [`live`] instrument.
pub struct MidiSender {
    ring: makepad_lock_free::Producer<MidiMessage>,
}

impl MidiSender {
    /// Queue a message for the next rendered block. Never blocks: a full ring
    /// hands the message back, and the caller keeps it and tries again on a
    /// later frame.
    pub fn send(&mut self, message: MidiMessage) -> Result<(), MidiMessage> {
        self.ring.push(message)
    }

    /// Room left in the ring.
    pub fn free(&self) -> usize {
        self.ring.free()
    }
}

/// The audio-callback side of a [`live`] instrument.
pub struct LiveInstrument {
    instrument: Instrument,
    ring: makepad_lock_free::Consumer<MidiMessage>,
}

impl LiveInstrument {
    /// Deliver every queued message at the start of this block, then render
    /// it. Allocates nothing, takes no lock and never waits.
    pub fn render(&mut self, left: &mut [f32], right: &mut [f32]) -> Result<(), Error> {
        while let Some(message) = self.ring.pop() {
            self.instrument.midi(message, 0)?;
        }
        self.instrument.render(left, right)
    }

    pub fn instrument(&mut self) -> &mut Instrument {
        &mut self.instrument
    }

    /// Back to a plain instrument (to drop it off the audio thread).
    pub fn into_inner(self) -> Instrument {
        self.instrument
    }
}

/// Split an instrument for live play: the sender for the thread that makes
/// the music, the renderer for the audio callback, joined by a ring of
/// `capacity` messages.
pub fn live(instrument: Instrument, capacity: usize) -> (MidiSender, LiveInstrument) {
    let (producer, consumer) = makepad_lock_free::ring(capacity.max(1));
    (MidiSender { ring: producer }, LiveInstrument { instrument, ring: consumer })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptions_round_trip_through_their_code() {
        let dls = ComponentDescription::DLS_SYNTH;
        assert_eq!(dls.code(), "aumu/dls /appl");
        assert_eq!(ComponentDescription::parse(&dls.code()), Some(dls));
        assert_eq!(ComponentDescription::parse("aumu/dls"), None, "too few codes");
        assert_eq!(ComponentDescription::parse("aumu/dls /appl/x"), None, "too many codes");
        assert_eq!(ComponentDescription::parse("aumu/dl/appl"), None, "a short code");
        let info = ComponentInfo { description: dls, name: "Apple: DLSMusicDevice".into(), version: 0 };
        assert_eq!(info.short_name(), "DLSMusicDevice");
        assert_eq!(Error { call: "x", status: four_cc(b"fmt?") as i32 }.to_string(), "x failed: 'fmt?' (1718449215)");
    }
}
