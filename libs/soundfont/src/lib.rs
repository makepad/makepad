//! SoundFont 2 and SFZ instrument loading plus a dependency-free sampler core.
//!
//! Loading and zone selection are control-thread operations and return owned
//! data. Rendering is deliberately separate: [`Sampler`] contains no heap
//! ownership, writes planar `f32` into caller-owned slices, and accesses PCM
//! only through [`SampleSource`]. A future streaming source can therefore
//! return [`SampleRead::Missing`] for a non-resident page; the audio thread
//! emits silence and reports the miss without waiting.
//!
//! The SFZ subset is documented on [`parse_sfz`]. SoundFont generator
//! inheritance and preset/instrument combination are resolved by
//! [`SoundFont::select`].

mod error;
mod model;
mod procedural;
mod queue;
mod sampler;
mod sf2;
mod sfz;

pub use error::{LoadError, SfzError};
pub use model::*;
pub use procedural::{metronome_click, piano_fallback};
pub use queue::{SpscConsumer, SpscProducer, SpscQueue};
pub use sampler::{
    EnvelopeRunner, EnvelopeStage, RenderReport, Sampler, SamplerEvent, ScheduledEvent,
    TimedEvent,
};
pub use sf2::{
    parse_sf2, parse_sf2_with_limits, Generator, GeneratorAmount, InfoEntry, Instrument,
    InstrumentZone, Modulator, ParseLimits, Preset, PresetZone, SampleHeader, SampleKind,
    SoundFont,
};
pub use sfz::{
    parse_sfz, parse_sfz_with_limits, SampleMetadata, SfzInstrument, SfzLimits, SfzSample,
};

#[cfg(test)]
mod tests;
