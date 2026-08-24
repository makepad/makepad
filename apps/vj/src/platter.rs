//! THE PLATTER IN THE DECK — one [`Transport`] per slot, fed from the deck's
//! own state every display frame, anchored ONCE at the frame on screen.
//!
//! This is the presenter's clock for resident (cached) clips. It reads:
//! the play mode and REV flip (map + travel), the trim (range), the beats
//! chip and SYNC (beat lock), the shuttle (hand), pause, and the beat clock
//! at the frame's own stamp (`NextFrameEvent.time` — one stamp for both
//! decks, never the wall clock). It writes nothing back but a snapshot.
//!
//! Anchoring: a NEW clip is cued at the source position the deck last
//! presented (`SlotPlayer::position_secs`); a REBUILT cache of the same
//! clip (a trim epoch) rebinds with the phase preserved — rescale, never
//! teleport. A platter that was not stepped for a while (the deck was on
//! another presenter) re-cues at the presented position before it runs:
//! the handover is a cue, not a jump back to where it was left.

use std::sync::Arc;
use std::time::Instant;

use crate::media::{Frame, PlayMode};
use crate::transport::{BeatInput, Events, Mode, Timeline, Transport};
use crate::App;

/// A platter left unstepped longer than this re-cues at the presented
/// position when it resumes (another presenter owned the deck meanwhile).
const STALE_SECS: f64 = 0.25;

pub struct PlatterSlot {
    transport: Transport,
    /// The resident cache this platter's timeline came from.
    cache_ptr: usize,
    /// The player (clip) it belongs to — a different one is a new cue.
    player_id: usize,
    last_step_at: f64,
}

/// One frame of the deck's clock, as ONE snapshot.
#[derive(Clone, Copy, Debug)]
pub struct PlatterStep {
    /// Media seconds on the source timeline.
    pub pos_secs: f64,
    /// Fractional cache index: `pair + t` (the wrap pair is `last + t`).
    pub index: f64,
    pub pair: usize,
    pub ib: usize,
    pub t: f32,
    /// The single frame nearest to the position (the OFF tier's picture).
    pub nearest: usize,
    /// On-screen source frames per second, signed by the picture's travel.
    pub rate: f64,
    /// |rate|: what the AI-tier gate reads.
    pub pace: f64,
    /// The integration step used (clamped), seconds.
    pub dt: f64,
    pub events: Events,
    /// The direction the PICTURE is moving.
    pub screen_forward: bool,
    /// The platter was (re)anchored this frame.
    pub anchored: bool,
}

/// The shuttle's deflection → the hand's velocity in natural units (the
/// jog law: a square curve to 1× at 0.7 of the well, then to 2× at the
/// rim; floored so a held deflection always creeps).
pub fn hand_velocity(pos: f32) -> f64 {
    let t = pos.abs() as f64;
    let mag = if t <= 0.7 {
        let n = t / 0.7;
        (n * n).max(0.02)
    } else {
        1.0 + (t - 0.7) / 0.3
    };
    if pos < 0.0 {
        -mag
    } else {
        mag
    }
}

/// The deck's mode flags → the map and the travel sign.
pub fn map_of(mode: PlayMode, flip: bool) -> (Mode, bool) {
    match mode {
        PlayMode::Loop => (Mode::Loop, true),
        PlayMode::Reverse => (Mode::Loop, false),
        PlayMode::PingPong => (Mode::Bounce, !flip),
        PlayMode::Once => (Mode::Once, true),
    }
}

impl App {
    /// An `Instant` in the frame stamp's time base (the app-seconds base was
    /// captured once at startup).
    pub(crate) fn app_secs_of(&self, at: Instant) -> Option<f64> {
        let start = self.app_start_instant?;
        Some(if at >= start {
            (at - start).as_secs_f64()
        } else {
            -((start - at).as_secs_f64())
        })
    }

    /// The beat clock as the transport reads it, at the frame stamp: the
    /// published clock when it runs, else the free-running house tempo.
    pub(crate) fn platter_beat_input(&self, now: f64) -> Option<BeatInput> {
        if self.beat_clock.running() {
            let epoch = self.app_secs_of(self.clock_epoch?)?;
            Some(BeatInput {
                bpm: self.beat_clock.bpm(),
                beats: self.beat_clock.position_at(now - epoch),
                epoch: self.beat_clock.epoch(),
            })
        } else {
            let bpm = self.free_bpm.clamp(40.0, 300.0);
            let anchor = self.app_secs_of(self.free_anchor)?;
            Some(BeatInput { bpm, beats: (now - anchor) * bpm / 60.0, epoch: 0 })
        }
    }

    /// The picture's travel direction on a platter-driven slot.
    pub(crate) fn platter_screen_forward(&self, i: usize) -> Option<bool> {
        self.platter[i].as_ref().map(|p| p.transport.screen_forward())
    }

    /// Step the slot's platter to `now` against its resident cache.
    pub(crate) fn platter_step(
        &mut self,
        i: usize,
        now: f64,
        cache: &Arc<Vec<Frame>>,
    ) -> Option<PlatterStep> {
        let n = cache.len();
        if n < 2 {
            return None;
        }
        let player = self.players[i].as_ref()?;
        let player_id = player.identity();
        let duration = player.duration_secs;
        let playing = !player.is_paused();
        let presented = player.position_secs();
        let (t_in, t_out) = self.slot_trim[i];
        let (mode, fwd) = map_of(self.slot_play_mode(i), self.slot_flip[i]);
        let sync = if self.slot_beat_sync[i] && self.external_sync_enabled {
            Some(self.slot_beat_rate[i].round().clamp(1.0, 16.0) as f64)
        } else {
            None
        };
        let hand = self.slot_scratch[i].map(hand_velocity);
        let beat = self.platter_beat_input(now);

        let ptr = Arc::as_ptr(cache) as usize;
        let slot = self.platter[i].get_or_insert_with(|| PlatterSlot {
            transport: Transport::new(),
            cache_ptr: 0,
            player_id: 0,
            last_step_at: f64::NEG_INFINITY,
        });
        let new_clip = slot.player_id != player_id;
        let rebuilt = slot.cache_ptr != ptr;
        let stale = now - slot.last_step_at > STALE_SECS;
        if rebuilt {
            let tl = Timeline::from_pts_100ns(cache.iter().map(|f| f.pts_100ns))?;
            let (lo, hi) = tl.window(t_in * duration, t_out * duration);
            if new_clip {
                // A new clip has no phase to keep: bind fresh, cue below.
                slot.transport = Transport::new();
            }
            slot.transport.bind(tl, lo, hi);
            slot.cache_ptr = ptr;
            slot.player_id = player_id;
        }
        let t = &mut slot.transport;
        let (lo, hi) = t.timeline()?.window(t_in * duration, t_out * duration);
        t.set_range(lo, hi);
        t.set_mode(mode);
        t.set_travel(fwd);
        t.set_sync(sync);
        t.set_playing(playing);
        match hand {
            Some(v) => t.hand_hold(v),
            None => t.hand_release(),
        }
        let anchored = new_clip || stale;
        if anchored {
            // THE ONE CUE: the frame the deck last presented — never a
            // queue tail, never where this platter was left.
            t.seek(presented);
        }
        slot.last_step_at = now;
        let step = t.advance(now, beat);
        let loc = t.locate(step.pos)?;
        let fps = t.timeline()?.fps();
        Some(PlatterStep {
            pos_secs: step.pos,
            index: loc.a as f64 + loc.t,
            pair: loc.a,
            ib: loc.b,
            t: loc.t as f32,
            nearest: if loc.t < 0.5 { loc.a } else { loc.b },
            rate: step.screen_vel * fps,
            pace: step.screen_vel.abs() * fps,
            dt: step.dt,
            events: step.events,
            screen_forward: t.screen_forward(),
            anchored,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_jog_law_is_the_old_one() {
        // The wheel reports f32 (0.7f32 is 0.69999999): a 1e-6 tolerance.
        assert!((hand_velocity(0.0) - 0.02).abs() < 1e-6, "floored creep");
        assert!((hand_velocity(0.7) - 1.0).abs() < 1e-6, "1x at 0.7");
        assert!((hand_velocity(1.0) - 2.0).abs() < 1e-6, "2x at the rim");
        assert!((hand_velocity(-0.7) + 1.0).abs() < 1e-6, "signed");
        assert!(hand_velocity(0.35) < hand_velocity(0.5) && hand_velocity(0.5) < 1.0);
    }

    #[test]
    fn modes_map_to_the_platter() {
        assert_eq!(map_of(PlayMode::Loop, false), (Mode::Loop, true));
        assert_eq!(map_of(PlayMode::Reverse, false), (Mode::Loop, false));
        assert_eq!(map_of(PlayMode::PingPong, false), (Mode::Bounce, true));
        assert_eq!(map_of(PlayMode::PingPong, true), (Mode::Bounce, false));
        assert_eq!(map_of(PlayMode::Once, false), (Mode::Once, true));
    }
}
