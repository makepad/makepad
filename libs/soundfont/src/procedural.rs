use crate::model::{Envelope, LoopMode, VoiceParameters, VoiceSource};

/// A cheap deterministic piano-like fallback: three struck harmonics plus a
/// short seeded hammer transient, shaped by this envelope in the sampler.
pub fn piano_fallback(key: u8, velocity: u8) -> VoiceParameters {
    VoiceParameters {
        source: VoiceSource::ProceduralPiano,
        key,
        velocity,
        root_key: key as f32,
        tune_cents: 0.0,
        scale_tuning: 100.0,
        sample_rate: 44_100,
        start_frame: 0,
        end_frame: i64::MAX,
        loop_start: 0,
        loop_end: 0,
        loop_mode: LoopMode::NoLoop,
        release_on_note_off: true,
        envelope: Envelope {
            delay: 0.0,
            attack: 0.003,
            hold: 0.0,
            decay: 0.8,
            sustain: 0.16,
            release: 0.7,
        },
        gain: (velocity.max(1) as f32 / 127.0).sqrt() * 0.34,
        pan: 0.0,
        filter_cutoff_hz: 8_500.0,
        filter_resonance_db: 0.0,
        exclusive_class: 0,
    }
}

/// Short, bright deterministic click. Accents are louder and lower-pitched;
/// both contain a seeded noise transient so they cut through dense playback.
pub fn metronome_click(accent: bool) -> VoiceParameters {
    VoiceParameters {
        source: VoiceSource::Metronome { accent },
        key: if accent { 84 } else { 91 },
        velocity: 127,
        root_key: 60.0,
        tune_cents: 0.0,
        scale_tuning: 100.0,
        sample_rate: 44_100,
        start_frame: 0,
        end_frame: i64::MAX,
        loop_start: 0,
        loop_end: 0,
        loop_mode: LoopMode::NoLoop,
        release_on_note_off: false,
        envelope: Envelope {
            delay: 0.0,
            attack: 0.0,
            hold: 0.0,
            decay: 0.0,
            sustain: 1.0,
            release: 0.0,
        },
        gain: if accent { 0.9 } else { 0.65 },
        pan: 0.0,
        filter_cutoff_hz: 20_000.0,
        filter_resonance_db: 0.0,
        exclusive_class: 0,
    }
}
