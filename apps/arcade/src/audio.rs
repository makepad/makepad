//! The bridge from the script's audio queue to the synth.
//!
//! `game_script` never touches a synth — it queues [`AudioRequest`]s and the
//! host decides what they sound like (that is the hook that lets the crate
//! stay free of an audio dependency). This module is Arcade's half: drain the
//! queue each frame, resolve positional requests against the local listener,
//! and push voices.
//!
//! Positional audio is Local tier by construction (game.md): the listener is
//! *this* device's camera, so two players in the same room hear the same game
//! differently and none of it reaches the wire.

use crate::synth;
use makepad_game_script::audio3d::{place, Listener};
use makepad_game_script::dispatch::{AudioRequest, ToneWave};

fn wave(w: ToneWave) -> synth::Wave {
    match w {
        ToneWave::Sine => synth::Wave::Sine,
        ToneWave::Square => synth::Wave::Square,
        ToneWave::Saw => synth::Wave::Saw,
        ToneWave::Triangle => synth::Wave::Triangle,
        ToneWave::Noise => synth::Wave::Noise,
    }
}

/// Play one request. Returns false when a named sound was not in the bank —
/// the caller logs it, because a silent typo costs an agent a whole test cycle.
pub fn play(request: &AudioRequest, listener: &Listener) -> bool {
    match request {
        AudioRequest::Sfx { name, pitch } => synth::play_named(name, *pitch),
        AudioRequest::SfxAt {
            name,
            pitch,
            at,
            range,
        } => {
            let placement = place(listener, *at, *range);
            // Out of range: nothing is queued at all, so a busy world does not
            // burn its 24 voices on sounds the player cannot hear.
            if placement.gain <= 0.0 {
                return true;
            }
            synth::play_named_at(name, *pitch, placement.gain, placement.pan)
        }
        AudioRequest::Beep {
            freq,
            to,
            ms,
            wave: w,
            gain,
        } => {
            synth::beep(*freq, *to, *ms / 1000.0, wave(*w), *gain, 0.0);
            true
        }
        AudioRequest::Jingle { notes, ms } => {
            synth::jingle(notes, *ms / 1000.0, synth::Wave::Triangle, 0.22);
            true
        }
        AudioRequest::Tone {
            id,
            freq,
            wave: w,
            gain,
        } => {
            synth::tone(*id, *freq, wave(*w), *gain);
            true
        }
        AudioRequest::ToneSet { id, freq, gain } => {
            synth::tone_set(*id, *freq, *gain);
            true
        }
        AudioRequest::ToneStop { id } => {
            synth::tone_stop(*id);
            true
        }
        AudioRequest::StopAllTones => {
            synth::stop_all_tones();
            true
        }
    }
}

/// Drain a frame's worth of requests. Unknown names are collected rather than
/// logged here, so the caller can route them wherever it routes diagnostics.
pub fn play_all(requests: &[AudioRequest], listener: &Listener) -> Vec<String> {
    let mut unknown = Vec::new();
    for request in requests {
        if !play(request, listener) {
            let name = match request {
                AudioRequest::Sfx { name, .. } | AudioRequest::SfxAt { name, .. } => name.clone(),
                _ => continue,
            };
            unknown.push(name);
        }
    }
    unknown
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_widgets::makepad_platform::audio::AudioBuffer;
    use makepad_widgets::*;

    fn v(x: f32, y: f32, z: f32) -> Vec3f {
        Vec3f { x, y, z }
    }

    fn buffer() -> AudioBuffer {
        let mut b = AudioBuffer::new_with_size(256, 2);
        b.zero();
        b
    }

    fn peaks(buf: &AudioBuffer) -> (f32, f32) {
        let p = |c: usize| {
            buf.channel(c)
                .iter()
                .fold(0.0f32, |acc, s| acc.max(s.abs()))
        };
        (p(0), p(1))
    }

    /// Facing -z at the origin, so +x is to the listener's right.
    fn listener() -> Listener {
        Listener::from_yaw(v(0.0, 0.0, 0.0), 0.0)
    }

    fn render(requests: &[AudioRequest]) -> (f32, f32) {
        synth::reset();
        play_all(requests, &listener());
        let mut buf = buffer();
        synth::mix_into(&mut buf, 44100.0);
        let out = peaks(&buf);
        synth::reset();
        out
    }

    #[test]
    fn a_queued_sfx_reaches_the_mixer() {
        let _guard = synth::SYNTH_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (l, r) = render(&[AudioRequest::Sfx {
            name: "jump".into(),
            pitch: 1.0,
        }]);
        assert!(l > 0.0 && r > 0.0, "2D sfx is audible in both channels");
        assert_eq!(l, r, "and centred");
    }

    #[test]
    fn sfx_at_pans_by_direction_and_fades_with_distance() {
        let _guard = synth::SYNTH_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let at = |x: f32, z: f32| AudioRequest::SfxAt {
            name: "jump".into(),
            pitch: 1.0,
            at: v(x, 0.0, z),
            range: 40.0,
        };

        let (l, r) = render(&[at(10.0, 0.0)]);
        assert!(r > l, "a sound to the right is louder on the right");

        let (l, r) = render(&[at(-10.0, 0.0)]);
        assert!(l > r, "and mirrored on the left");

        // Straight ahead is centred; further away is quieter.
        let (near_l, near_r) = render(&[at(0.0, -4.0)]);
        assert_eq!(near_l, near_r, "dead ahead is centred");
        let (far_l, _) = render(&[at(0.0, -30.0)]);
        assert!(far_l < near_l, "far {far_l} must be quieter than near {near_l}");
    }

    #[test]
    fn a_sound_past_its_range_queues_no_voice_at_all() {
        let _guard = synth::SYNTH_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        synth::reset();
        play_all(
            &[AudioRequest::SfxAt {
                name: "jump".into(),
                pitch: 1.0,
                at: v(0.0, 0.0, -500.0),
                range: 40.0,
            }],
            &listener(),
        );
        assert_eq!(
            synth::live_counts().0,
            0,
            "out of range must not spend a voice slot"
        );
        synth::reset();
    }

    #[test]
    fn an_unknown_name_is_reported_to_the_caller() {
        let _guard = synth::SYNTH_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        synth::reset();
        let unknown = play_all(
            &[
                AudioRequest::Sfx {
                    name: "jump".into(),
                    pitch: 1.0,
                },
                AudioRequest::Sfx {
                    name: "nonsense".into(),
                    pitch: 1.0,
                },
            ],
            &listener(),
        );
        assert_eq!(unknown, vec!["nonsense".to_string()]);
        synth::reset();
    }

    #[test]
    fn tone_lifecycle_runs_through_the_queue() {
        let _guard = synth::SYNTH_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        synth::reset();
        play_all(
            &[AudioRequest::Tone {
                id: 3,
                freq: 220.0,
                wave: ToneWave::Saw,
                gain: 0.4,
            }],
            &listener(),
        );
        assert_eq!(synth::live_counts().1, 1);

        // Retune, then stop: the tone releases rather than vanishing.
        play_all(
            &[
                AudioRequest::ToneSet {
                    id: 3,
                    freq: Some(440.0),
                    gain: None,
                },
                AudioRequest::ToneStop { id: 3 },
            ],
            &listener(),
        );
        for _ in 0..40 {
            let mut buf = buffer();
            synth::mix_into(&mut buf, 44100.0);
        }
        assert_eq!(synth::live_counts().1, 0);
        synth::reset();
    }
}
