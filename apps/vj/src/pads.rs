//! One-shot SFX pad bank: pure trigger/voice-allocation engine.
//!
//! Pads are performance triggers, not deck tracks: a click fires a voice
//! immediately (polyphonic, fast retrigger), never touches the music decks
//! or their crossfader, and has no timeline/transport. The engine owns the
//! voice allocation table; the mixer executes the returned commands. All
//! time is injected (`now_ms`), so every behavior is hermetically testable.
//!
//! Policies (all bounded, all deterministic):
//! - retrigger default: overlap — a new voice per click, old voices play on,
//! - optional choke group: starting a voice stops every live voice in the
//!   same group (across pads) first,
//! - optional hold: press starts, release stops that press's voice,
//! - optional loop: the voice loops until the pad is clicked again (toggle)
//!   or explicitly stopped,
//! - voice cap: at the cap the oldest voice is stolen (tie → quietest, then
//!   lowest id),
//! - clicking an unloaded pad requests its decode; the completion
//!   auto-fires only when it arrives within [`FRESH_TRIGGER_MS`] of the
//!   click — later completions just mark the pad loaded (stale-trigger
//!   suppression).

use makepad_asset_data::{AssetId, AssetRevisionId, BlobId, MediaType};
use std::collections::HashMap;

pub type PadGen = u64;
pub type VoiceId = u64;

/// Most simultaneous SFX voices; the steal policy keeps this exact.
pub const MAX_VOICES: usize = 24;
/// A decode completing within this window of the click that requested it
/// still fires the pad; later completions only mark it loaded.
pub const FRESH_TRIGGER_MS: u64 = 300;

/// Stable pad identity: the asset. PCM identity is the revision.
pub type PadKey = AssetId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PadItem {
    pub asset: AssetId,
    pub revision: AssetRevisionId,
    pub title: String,
    pub media_blob: BlobId,
    pub media_len: u64,
    pub media: MediaType,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum PadLoad {
    #[default]
    Idle,
    Loading {
        gen: PadGen,
    },
    Ready,
    Failed {
        error: String,
    },
}

#[derive(Clone, Debug)]
pub struct PadState {
    pub item: PadItem,
    pub load: PadLoad,
    pub gain: f32,
    /// 0 = no choke group; 1..=4 are the group buttons in the UI.
    pub choke_group: u8,
    pub hold: bool,
    pub loop_on: bool,
    /// Wall-clock of the most recent unserviced trigger (fresh-fire window).
    last_trigger_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoiceAlloc {
    pub id: VoiceId,
    pub pad: PadKey,
    pub choke_group: u8,
    pub loop_on: bool,
    pub gain: f32,
    pub started_ms: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PadCmd {
    /// Fetch + decode this pad's sample under `gen` (stale results drop).
    LoadPad { pad: PadKey, gen: PadGen, item: PadItem },
    /// Start a voice on the mixer playing the pad's decoded PCM.
    StartVoice { voice: VoiceAlloc, revision: AssetRevisionId },
    StopVoice { id: VoiceId },
    /// Live-update the gain of every playing voice of this pad.
    SetPadVoicesGain { pad: PadKey, gain: f32 },
}

#[derive(Default)]
pub struct PadEngine {
    pads: HashMap<PadKey, PadState>,
    /// Stable UI order (insertion order of `upsert_pad`).
    order: Vec<PadKey>,
    voices: Vec<VoiceAlloc>,
    next_voice: VoiceId,
    next_gen: PadGen,
}

impl PadEngine {
    pub fn new() -> PadEngine {
        PadEngine::default()
    }

    pub fn pad(&self, key: &PadKey) -> Option<&PadState> {
        self.pads.get(key)
    }

    pub fn voice_count(&self) -> usize {
        self.voices.len()
    }

    pub fn playing_voices(&self, key: &PadKey) -> usize {
        self.voices.iter().filter(|v| v.pad == *key).count()
    }

    /// Install or refresh a pad from catalog data. A changed revision resets
    /// the load state (the old PCM no longer represents this pad); per-pad
    /// performance settings (gain/choke/hold/loop) survive.
    pub fn upsert_pad(&mut self, item: PadItem) {
        match self.pads.get_mut(&item.asset) {
            Some(pad) => {
                if pad.item.revision != item.revision {
                    pad.load = PadLoad::Idle;
                    pad.last_trigger_ms = None;
                }
                pad.item = item;
            }
            None => {
                self.order.push(item.asset);
                self.pads.insert(
                    item.asset,
                    PadState {
                        item,
                        load: PadLoad::Idle,
                        gain: 1.0,
                        choke_group: 0,
                        hold: false,
                        loop_on: false,
                        last_trigger_ms: None,
                    },
                );
            }
        }
    }

    /// Drop pads no longer present in the catalog view, stopping their
    /// voices. Live settings of surviving pads are untouched.
    pub fn retain_pads(&mut self, keep: &[PadKey]) -> Vec<PadCmd> {
        let mut cmds = Vec::new();
        self.order.retain(|k| keep.contains(k));
        let dropped: Vec<PadKey> =
            self.pads.keys().filter(|k| !keep.contains(k)).copied().collect();
        for key in dropped {
            self.pads.remove(&key);
            let stopped: Vec<VoiceId> =
                self.voices.iter().filter(|v| v.pad == key).map(|v| v.id).collect();
            self.voices.retain(|v| v.pad != key);
            cmds.extend(stopped.into_iter().map(|id| PadCmd::StopVoice { id }));
        }
        cmds
    }

    fn fire(&mut self, key: PadKey, now_ms: u64) -> Vec<PadCmd> {
        let Some(pad) = self.pads.get(&key) else { return Vec::new() };
        let (gain, choke, loop_on, revision) =
            (pad.gain, pad.choke_group, pad.loop_on, pad.item.revision);
        let mut cmds = Vec::new();

        // Loop pads toggle: a second click while looping stops the loop.
        if loop_on {
            let live: Vec<VoiceId> =
                self.voices.iter().filter(|v| v.pad == key && v.loop_on).map(|v| v.id).collect();
            if !live.is_empty() {
                self.voices.retain(|v| !(v.pad == key && v.loop_on));
                return live.into_iter().map(|id| PadCmd::StopVoice { id }).collect();
            }
        }

        // Choke group: silence the group before the new voice starts.
        if choke != 0 {
            let choked: Vec<VoiceId> =
                self.voices.iter().filter(|v| v.choke_group == choke).map(|v| v.id).collect();
            self.voices.retain(|v| v.choke_group != choke);
            cmds.extend(choked.into_iter().map(|id| PadCmd::StopVoice { id }));
        }

        // Voice cap: steal the oldest (tie → quietest, then lowest id).
        if self.voices.len() >= MAX_VOICES {
            let victim = self
                .voices
                .iter()
                .min_by(|a, b| {
                    a.started_ms
                        .cmp(&b.started_ms)
                        .then(a.gain.total_cmp(&b.gain))
                        .then(a.id.cmp(&b.id))
                })
                .map(|v| v.id)
                .expect("cap reached implies a voice exists");
            self.voices.retain(|v| v.id != victim);
            cmds.push(PadCmd::StopVoice { id: victim });
        }

        self.next_voice += 1;
        let voice = VoiceAlloc {
            id: self.next_voice,
            pad: key,
            choke_group: choke,
            loop_on,
            gain,
            started_ms: now_ms,
        };
        self.voices.push(voice);
        cmds.push(PadCmd::StartVoice { voice, revision });
        cmds
    }

    /// Pad pressed. Ready pads fire immediately; unloaded pads start their
    /// decode and remember the click for the fresh-fire window.
    pub fn press(&mut self, key: PadKey, now_ms: u64) -> Vec<PadCmd> {
        let Some(pad) = self.pads.get_mut(&key) else { return Vec::new() };
        match pad.load.clone() {
            PadLoad::Ready => self.fire(key, now_ms),
            PadLoad::Idle | PadLoad::Failed { .. } => {
                self.next_gen += 1;
                let gen = self.next_gen;
                pad.load = PadLoad::Loading { gen };
                pad.last_trigger_ms = Some(now_ms);
                let item = pad.item.clone();
                vec![PadCmd::LoadPad { pad: key, gen, item }]
            }
            PadLoad::Loading { .. } => {
                // Decode already in flight; refresh the fresh-fire window.
                pad.last_trigger_ms = Some(now_ms);
                Vec::new()
            }
        }
    }

    /// Pad released — only meaningful for hold pads: the press's newest
    /// voice stops.
    pub fn release(&mut self, key: PadKey) -> Vec<PadCmd> {
        let Some(pad) = self.pads.get(&key) else { return Vec::new() };
        if !pad.hold {
            return Vec::new();
        }
        let newest = self
            .voices
            .iter()
            .filter(|v| v.pad == key)
            .max_by_key(|v| v.id)
            .map(|v| v.id);
        match newest {
            Some(id) => {
                self.voices.retain(|v| v.id != id);
                vec![PadCmd::StopVoice { id }]
            }
            None => Vec::new(),
        }
    }

    /// Explicit stop/choke button: silence every voice of this pad.
    pub fn stop_pad(&mut self, key: PadKey) -> Vec<PadCmd> {
        let stopped: Vec<VoiceId> =
            self.voices.iter().filter(|v| v.pad == key).map(|v| v.id).collect();
        self.voices.retain(|v| v.pad != key);
        stopped.into_iter().map(|id| PadCmd::StopVoice { id }).collect()
    }

    /// Decode finished. Stale generations (a newer revision reset the pad,
    /// or the pad vanished) are dropped. Fires only when the requesting
    /// click is still fresh.
    pub fn load_ready(&mut self, key: PadKey, gen: PadGen, now_ms: u64) -> Vec<PadCmd> {
        let Some(pad) = self.pads.get_mut(&key) else { return Vec::new() };
        if pad.load != (PadLoad::Loading { gen }) {
            return Vec::new();
        }
        pad.load = PadLoad::Ready;
        let fresh = pad
            .last_trigger_ms
            .take()
            .is_some_and(|t| now_ms.saturating_sub(t) <= FRESH_TRIGGER_MS);
        if fresh {
            self.fire(key, now_ms)
        } else {
            Vec::new()
        }
    }

    pub fn load_failed(&mut self, key: PadKey, gen: PadGen, error: String) -> Vec<PadCmd> {
        if let Some(pad) = self.pads.get_mut(&key) {
            if pad.load == (PadLoad::Loading { gen }) {
                pad.load = PadLoad::Failed { error };
                pad.last_trigger_ms = None;
            }
        }
        Vec::new()
    }

    /// The mixer reports a (non-looping) voice ran off its end.
    pub fn voice_ended(&mut self, id: VoiceId) {
        self.voices.retain(|v| v.id != id);
    }

    pub fn set_gain(&mut self, key: PadKey, gain: f32) -> Vec<PadCmd> {
        let gain = gain.clamp(0.0, 1.5);
        let Some(pad) = self.pads.get_mut(&key) else { return Vec::new() };
        pad.gain = gain;
        for v in self.voices.iter_mut().filter(|v| v.pad == key) {
            v.gain = gain;
        }
        vec![PadCmd::SetPadVoicesGain { pad: key, gain }]
    }

    pub fn set_choke_group(&mut self, key: PadKey, group: u8) {
        if let Some(pad) = self.pads.get_mut(&key) {
            pad.choke_group = group.min(4);
        }
    }

    /// Absolute setters for the UI checkboxes (idempotent).
    pub fn set_hold(&mut self, key: PadKey, hold: bool) {
        if let Some(pad) = self.pads.get_mut(&key) {
            pad.hold = hold;
        }
    }

    pub fn set_loop_on(&mut self, key: PadKey, loop_on: bool) {
        if let Some(pad) = self.pads.get_mut(&key) {
            pad.loop_on = loop_on;
        }
    }

    pub fn pad_keys(&self) -> Vec<PadKey> {
        self.order.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(seed: u8) -> PadKey {
        AssetId::from_bytes([seed; 16])
    }

    fn item(seed: u8) -> PadItem {
        PadItem {
            asset: key(seed),
            revision: AssetRevisionId::from_bytes([seed; 32]),
            title: format!("sfx {seed}"),
            media_blob: BlobId::from_bytes([seed ^ 0xff; 32]),
            media_len: 100 + seed as u64,
            media: MediaType::Wav,
        }
    }

    fn ready_engine(seeds: &[u8]) -> PadEngine {
        let mut e = PadEngine::new();
        for &s in seeds {
            e.upsert_pad(item(s));
            let cmds = e.press(key(s), 0);
            let PadCmd::LoadPad { gen, .. } = cmds[0] else { panic!() };
            // Complete outside the fresh window so setup never auto-fires.
            e.load_ready(key(s), gen, FRESH_TRIGGER_MS + 1000);
        }
        e
    }

    fn started(cmds: &[PadCmd]) -> Vec<VoiceId> {
        cmds.iter()
            .filter_map(|c| match c {
                PadCmd::StartVoice { voice, .. } => Some(voice.id),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn ready_pad_fires_immediately_and_overlaps_polyphonically() {
        let mut e = ready_engine(&[1, 2]);
        let t = 10_000;
        // Same pad twice + another pad: three live voices, zero stops.
        let c1 = e.press(key(1), t);
        let c2 = e.press(key(1), t + 5);
        let c3 = e.press(key(2), t + 9);
        assert_eq!(started(&c1).len(), 1);
        assert_eq!(started(&c2).len(), 1);
        assert_eq!(started(&c3).len(), 1);
        assert!(c1.iter().chain(&c2).chain(&c3).all(|c| !matches!(c, PadCmd::StopVoice { .. })));
        assert_eq!(e.voice_count(), 3);
        assert_eq!(e.playing_voices(&key(1)), 2);
        // One-shot ends: mixer reports, table frees.
        let id = started(&c1)[0];
        e.voice_ended(id);
        assert_eq!(e.playing_voices(&key(1)), 1);
    }

    #[test]
    fn one_shot_triggering_never_emits_deck_or_fader_commands() {
        // The command vocabulary itself proves isolation: PadCmd has no deck
        // or crossfader variants. This test pins the runtime behavior side:
        // firing pads only starts/stops voices and loads.
        let mut e = ready_engine(&[1]);
        for c in e.press(key(1), 50) {
            match c {
                PadCmd::StartVoice { .. } | PadCmd::StopVoice { .. } => {}
                other => panic!("unexpected non-voice command {other:?}"),
            }
        }
    }

    #[test]
    fn choke_group_stops_group_voices_across_pads() {
        let mut e = ready_engine(&[1, 2, 3]);
        e.set_choke_group(key(1), 2);
        e.set_choke_group(key(2), 2);
        // Pad 3 stays outside the group.
        let c3 = e.press(key(3), 100);
        let v3 = started(&c3)[0];
        let c1 = e.press(key(1), 110);
        let v1 = started(&c1)[0];
        // Firing pad 2 chokes pad 1's voice (same group), leaves pad 3.
        let c2 = e.press(key(2), 120);
        assert!(c2.contains(&PadCmd::StopVoice { id: v1 }));
        assert_eq!(started(&c2).len(), 1);
        assert_eq!(e.playing_voices(&key(1)), 0);
        assert_eq!(e.playing_voices(&key(3)), 1);
        let _ = v3;
        // Same-pad retrigger inside a choke group chokes its own prior voice.
        let v2 = started(&c2)[0];
        let c2b = e.press(key(2), 130);
        assert!(c2b.contains(&PadCmd::StopVoice { id: v2 }));
        assert_eq!(e.playing_voices(&key(2)), 1);
    }

    #[test]
    fn voice_cap_steals_oldest_then_quietest_then_lowest_id() {
        let mut e = ready_engine(&[1, 2]);
        // Fill the pool: MAX_VOICES voices, pad 1, increasing start times.
        let mut first_id = None;
        for i in 0..MAX_VOICES {
            let c = e.press(key(1), 1000 + i as u64);
            first_id.get_or_insert(started(&c)[0]);
        }
        assert_eq!(e.voice_count(), MAX_VOICES);
        // The next trigger steals the oldest voice.
        let c = e.press(key(2), 5000);
        assert_eq!(c[0], PadCmd::StopVoice { id: first_id.unwrap() });
        assert_eq!(started(&c).len(), 1);
        assert_eq!(e.voice_count(), MAX_VOICES);

        // Tie on start time → the quietest goes first.
        let mut e = ready_engine(&[1, 2, 3]);
        e.set_gain(key(1), 1.0);
        e.set_gain(key(2), 0.2);
        let mut quiet_id = None;
        for i in 0..MAX_VOICES {
            let pad = if i == 3 { key(2) } else { key(1) };
            let c = e.press(pad, 7000); // identical start time
            if i == 3 {
                quiet_id = Some(started(&c)[0]);
            }
        }
        let c = e.press(key(3), 7000);
        assert_eq!(c[0], PadCmd::StopVoice { id: quiet_id.unwrap() });
    }

    #[test]
    fn hold_release_stops_that_press_and_only_that_press() {
        let mut e = ready_engine(&[1]);
        e.set_hold(key(1), true);
        let c1 = e.press(key(1), 100);
        let c2 = e.press(key(1), 110);
        let (v1, v2) = (started(&c1)[0], started(&c2)[0]);
        // Release stops the newest press first.
        assert_eq!(e.release(key(1)), vec![PadCmd::StopVoice { id: v2 }]);
        assert_eq!(e.release(key(1)), vec![PadCmd::StopVoice { id: v1 }]);
        assert!(e.release(key(1)).is_empty());
        // Non-hold pads ignore release.
        e.set_hold(key(1), false);
        let c3 = e.press(key(1), 200);
        assert!(e.release(key(1)).is_empty());
        assert_eq!(e.playing_voices(&key(1)), 1);
        let _ = c3;
    }

    #[test]
    fn loop_pad_toggles_and_explicit_stop_chokes_everything() {
        let mut e = ready_engine(&[1]);
        e.set_loop_on(key(1), true);
        let c1 = e.press(key(1), 100);
        let v1 = started(&c1)[0];
        assert_eq!(e.playing_voices(&key(1)), 1);
        // Second click: the loop toggles OFF (stop, no new voice).
        let c2 = e.press(key(1), 200);
        assert_eq!(c2, vec![PadCmd::StopVoice { id: v1 }]);
        assert_eq!(e.playing_voices(&key(1)), 0);
        // Loop off again → normal one-shots; stop_pad silences all.
        e.set_loop_on(key(1), false);
        e.press(key(1), 300);
        e.press(key(1), 310);
        let stops = e.stop_pad(key(1));
        assert_eq!(stops.len(), 2);
        assert_eq!(e.voice_count(), 0);
    }

    #[test]
    fn unloaded_pad_loads_then_fires_only_within_fresh_window() {
        let mut e = PadEngine::new();
        e.upsert_pad(item(1));
        // Click requests the load.
        let cmds = e.press(key(1), 1000);
        let PadCmd::LoadPad { gen, .. } = cmds[0] else { panic!() };
        assert_eq!(cmds.len(), 1);
        // Completion inside the fresh window auto-fires.
        let cmds = e.load_ready(key(1), gen, 1000 + FRESH_TRIGGER_MS);
        assert_eq!(started(&cmds).len(), 1);

        // Same flow, late completion: loaded but silent.
        let mut e = PadEngine::new();
        e.upsert_pad(item(2));
        let cmds = e.press(key(2), 1000);
        let PadCmd::LoadPad { gen, .. } = cmds[0] else { panic!() };
        let cmds = e.load_ready(key(2), gen, 1000 + FRESH_TRIGGER_MS + 1);
        assert!(cmds.is_empty(), "stale decode completion must not fire");
        assert_eq!(e.pad(&key(2)).unwrap().load, PadLoad::Ready);
        // Next click fires instantly.
        assert_eq!(started(&e.press(key(2), 9000)).len(), 1);
    }

    #[test]
    fn revision_change_resets_load_and_stale_gen_results_drop() {
        let mut e = PadEngine::new();
        e.upsert_pad(item(1));
        let cmds = e.press(key(1), 100);
        let PadCmd::LoadPad { gen: g1, .. } = cmds[0] else { panic!() };
        // Catalog event replaces the revision mid-load.
        let mut newer = item(1);
        newer.revision = AssetRevisionId::from_bytes([0x77; 32]);
        e.upsert_pad(newer.clone());
        assert_eq!(e.pad(&key(1)).unwrap().load, PadLoad::Idle);
        // The old decode completes: dropped (its PCM is the old revision).
        assert!(e.load_ready(key(1), g1, 150).is_empty());
        assert_eq!(e.pad(&key(1)).unwrap().load, PadLoad::Idle);
        // Reload under a fresh generation works.
        let cmds = e.press(key(1), 200);
        let PadCmd::LoadPad { gen: g2, item: it, .. } = cmds[0].clone() else { panic!() };
        assert!(g2 > g1);
        assert_eq!(it.revision, newer.revision);
    }

    #[test]
    fn per_pad_gain_is_independent_and_updates_live_voices() {
        let mut e = ready_engine(&[1, 2]);
        e.press(key(1), 10);
        e.press(key(2), 11);
        let cmds = e.set_gain(key(1), 0.4);
        assert_eq!(cmds, vec![PadCmd::SetPadVoicesGain { pad: key(1), gain: 0.4 }]);
        // Pad 2's voice keeps its own gain.
        let pad2_voice = e.voices.iter().find(|v| v.pad == key(2)).unwrap();
        assert!((pad2_voice.gain - 1.0).abs() < 1e-6);
        let pad1_voice = e.voices.iter().find(|v| v.pad == key(1)).unwrap();
        assert!((pad1_voice.gain - 0.4).abs() < 1e-6);
        // New voices of pad 1 inherit the new gain.
        let c = e.press(key(1), 50);
        let PadCmd::StartVoice { voice, .. } = c.last().unwrap() else { panic!() };
        assert!((voice.gain - 0.4).abs() < 1e-6);
    }

    #[test]
    fn retained_pads_keep_settings_dropped_pads_stop_voices() {
        let mut e = ready_engine(&[1, 2]);
        e.set_gain(key(1), 0.6);
        e.press(key(2), 10);
        let cmds = e.retain_pads(&[key(1)]);
        assert_eq!(cmds.len(), 1, "dropped pad's voice must stop");
        assert!(e.pad(&key(2)).is_none());
        assert!((e.pad(&key(1)).unwrap().gain - 0.6).abs() < 1e-6);
        assert_eq!(e.pad_keys().len(), 1);
    }
}
