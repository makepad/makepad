//! The `Pads` node's voices (Stage's SFX one-shot pads, donor `pads`,
//! deleted in ef48f3d9): a hit plays its pad's sample once from the start,
//! over whatever still sounds; a pad hit again plays again over itself.
//! The pool holds [`PAD_VOICES`]; a hit with every voice sounding takes the
//! oldest, which fades out over [`PAD_FADE_FRAMES`] first so nothing clicks.
//! A stop lets every voice of its pad fade.
//!
//! Runs on the audio thread: the pool's room was reserved when the node
//! was prepared, and nothing here allocates.

use crate::feed::PadHit;
use crate::schedule::{PadVoice, PadsState, PAD_FADE_FRAMES, PAD_VOICES};

/// One hit from the host.
pub fn take_hit(pads: &mut PadsState, hit: PadHit, rate: f32) {
    if hit.stop {
        for voice in pads.voices.iter_mut().filter(|voice| voice.pad == hit.pad) {
            voice.fading.get_or_insert(PAD_FADE_FRAMES);
        }
        return;
    }
    let Some(Some(sample)) = pads.samples.get(hit.pad as usize) else { return };
    if sample.left.is_empty() || !(rate > 0.0) {
        return;
    }
    let gain = pads.gains.get(hit.pad as usize).copied().unwrap_or(1.0) * if hit.gain.is_finite() { hit.gain.clamp(0.0, 4.0) } else { 1.0 };
    pads.serial += 1;
    let voice = PadVoice { sample: sample.clone(), pad: hit.pad, pos: 0.0, step: sample.rate as f64 / rate as f64, gain, fading: None, serial: pads.serial };
    if pads.voices.len() < PAD_VOICES {
        pads.voices.push(voice);
        return;
    }
    // Full: the oldest gives way (it is cut here; the newest is what the
    // hand asked for). A voice already fading goes first.
    let oldest = pads
        .voices
        .iter()
        .enumerate()
        .min_by_key(|(_, voice)| (voice.fading.is_none(), voice.serial))
        .map(|(index, _)| index);
    if let Some(index) = oldest {
        pads.voices[index] = voice;
    }
}

/// One pass: every voice into `left`/`right` at the node's level `target`
/// (smoothed across the pass), the voices that ended let go.
pub fn render(pads: &mut PadsState, left: &mut [f32], right: &mut [f32], target: f32) {
    left.fill(0.0);
    right.fill(0.0);
    let frames = left.len();
    for voice in pads.voices.iter_mut() {
        let sample = &voice.sample;
        let len = sample.left.len();
        for frame in 0..frames {
            let at = voice.pos as usize;
            if at + 1 >= len {
                voice.pos = len as f64;
                break;
            }
            let fade = match voice.fading.as_mut() {
                Some(0) => {
                    voice.pos = len as f64;
                    break;
                }
                Some(left) => {
                    *left -= 1;
                    *left as f32 / PAD_FADE_FRAMES as f32
                }
                None => 1.0,
            };
            let t = (voice.pos - at as f64) as f32;
            let l = sample.left[at] + (sample.left[at + 1] - sample.left[at]) * t;
            let r = match &sample.right {
                Some(right) if right.len() > at + 1 => right[at] + (right[at + 1] - right[at]) * t,
                _ => l,
            };
            let gain = voice.gain * fade;
            left[frame] += l * gain;
            right[frame] += r * gain;
            voice.pos += voice.step;
        }
    }
    pads.voices.retain(|voice| (voice.pos as usize) + 1 < voice.sample.left.len());
    let step = if frames > 0 { (target - pads.gain) / frames as f32 } else { 0.0 };
    pads.peak *= crate::run::PEAK_DECAY;
    for frame in 0..frames {
        pads.gain += step;
        left[frame] = if left[frame].is_finite() { left[frame] * pads.gain } else { 0.0 };
        right[frame] = if right[frame].is_finite() { right[frame] * pads.gain } else { 0.0 };
        pads.peak = pads.peak.max(left[frame].abs()).max(right[frame].abs());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_score_player::Sample;
    use std::sync::Arc;

    fn pads(len: usize) -> PadsState {
        let sample = Arc::new(Sample::new(vec![0.5; len].into(), None, 48_000.0));
        PadsState {
            samples: vec![Some(sample), None].into(),
            gains: vec![1.0, 1.0].into(),
            voices: Vec::with_capacity(PAD_VOICES),
            feed: None,
            serial: 0,
            gain: 1.0,
            peak: 0.0,
        }
    }

    #[test]
    fn a_hit_plays_its_sample_once_and_hits_overlap() {
        let mut state = pads(100);
        let (mut l, mut r) = (vec![0.0; 64], vec![0.0; 64]);
        take_hit(&mut state, PadHit { pad: 0, gain: 1.0, stop: false }, 48_000.0);
        take_hit(&mut state, PadHit { pad: 0, gain: 1.0, stop: false }, 48_000.0);
        take_hit(&mut state, PadHit { pad: 1, gain: 1.0, stop: false }, 48_000.0);
        assert_eq!(state.voices.len(), 2, "two voices of pad 0; the silent pad plays nothing");
        render(&mut state, &mut l, &mut r, 1.0);
        assert!((l[0] - 1.0).abs() < 1e-6, "two hits overlap: {}", l[0]);
        render(&mut state, &mut l, &mut r, 1.0);
        assert!(state.voices.is_empty(), "played out after its 100 frames");
        assert!(l[63].abs() < 1e-6);
    }

    #[test]
    fn a_full_pool_gives_the_oldest_up_and_a_stop_fades() {
        let mut state = pads(48_000);
        for _ in 0..PAD_VOICES + 3 {
            take_hit(&mut state, PadHit { pad: 0, gain: 1.0, stop: false }, 48_000.0);
        }
        assert_eq!(state.voices.len(), PAD_VOICES);
        assert_eq!(state.voices.iter().map(|voice| voice.serial).min(), Some(4), "the three oldest gave way");
        take_hit(&mut state, PadHit { pad: 0, gain: 1.0, stop: true }, 48_000.0);
        let (mut l, mut r) = (vec![0.0; PAD_FADE_FRAMES as usize + 8], vec![0.0; PAD_FADE_FRAMES as usize + 8]);
        render(&mut state, &mut l, &mut r, 1.0);
        assert!(state.voices.is_empty(), "faded and let go");
        assert!(l[0] > l[PAD_FADE_FRAMES as usize / 2], "a fade, not a cut");
    }
}
