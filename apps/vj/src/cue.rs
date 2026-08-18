//! Video program cue engine: two playback slots, latest-click-wins.
//!
//! Pure state machine — no clocks, sockets or decoders. The app feeds it
//! clicks and typed completions (each stamped with the generation that
//! issued it) and executes the commands it returns:
//!
//! click tile ──► FetchMedia(gen) ──media_ready──► OpenSlot(preroll)
//!        ──preroll_ready──► ArmFade ──start_armed──► BeginFade
//!        ──fade_complete──► CloseSlot(old)
//!
//! Latest-click-wins: every click bumps the generation, so completions of a
//! superseded click are stale and ignored (or their slot is closed if it
//! already opened). During a fade both slots are busy; a ready cue then
//! waits and opens as soon as the fade releases a slot. Slot closes are
//! commands to the host, which reclaims decoders asynchronously — never a
//! join on the UI thread.

use makepad_asset_data::{AssetId, AssetRevisionId, BlobId, MediaType};
use std::path::PathBuf;

pub type CueGen = u64;
/// Identity of one armed program transition. Unlike a cue generation this
/// names the actual scheduling attempt, so a late callback can never start a
/// newer cue that happens to reuse the same physical slot.
pub type CueScheduleId = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotId {
    A,
    B,
}

impl SlotId {
    pub fn index(self) -> usize {
        match self {
            SlotId::A => 0,
            SlotId::B => 1,
        }
    }
}

#[cfg(test)]
impl SlotId {
    fn other(self) -> SlotId {
        match self {
            SlotId::A => SlotId::B,
            SlotId::B => SlotId::A,
        }
    }
}

/// What a video tile resolves to once its manifest is known.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CueItem {
    pub asset: AssetId,
    pub revision: AssetRevisionId,
    pub title: String,
    pub media_blob: BlobId,
    pub media_len: u64,
    pub media: MediaType,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CueCmd {
    /// Download the cue's media blob (media runtime; verified cache).
    FetchMedia { gen: CueGen, item: CueItem },
    /// Open a decoder on `slot`, paused, pre-rolling the first frame.
    OpenSlot { slot: SlotId, gen: CueGen, item: CueItem, path: PathBuf },
    /// The decoder is ready, but must remain paused until the host schedules
    /// this exact identity on the audio device clock.
    ArmFade {
        gen: CueGen,
        schedule: CueScheduleId,
        from: Option<SlotId>,
        to: SlotId,
    },
    /// Cancel a previously armed device-clock transition. This always
    /// precedes `CloseSlot` when latest-click-wins supersedes an armed cue.
    CancelArm { schedule: CueScheduleId },
    /// The device clock has started the armed transition. The host uses this
    /// command to start picture pacing; audio was already released at the
    /// exact scheduled sample.
    BeginFade { schedule: CueScheduleId, from: Option<SlotId>, to: SlotId },
    /// Reclaim a slot's decoder asynchronously.
    CloseSlot { slot: SlotId },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PendingState {
    Fetching,
    /// Media is local but both slots were busy (mid-fade); waiting for one.
    WaitingSlot { path: PathBuf },
    /// Decoder opening/pre-rolling on this slot.
    Preloading { slot: SlotId },
    /// Decoder and bounded A/V preroll are ready. The old live slot remains
    /// live until `start_armed` validates this identity.
    Armed { slot: SlotId, schedule: CueScheduleId },
}

#[derive(Clone, Debug)]
struct Pending {
    gen: CueGen,
    item: CueItem,
    state: PendingState,
}

#[derive(Default)]
pub struct CueEngine {
    gen: CueGen,
    next_schedule: CueScheduleId,
    live: Option<(SlotId, CueItem)>,
    /// Overlay bed: the slot that stays on screen under the live overlay.
    bed: Option<(SlotId, CueItem)>,
    pending: Option<Pending>,
    /// Transition currently running and its old, still-audible slot.
    active_fade: Option<(CueScheduleId, Option<SlotId>)>,
    last_error: Option<String>,
    /// B over A: both slots stay open; new cues replace the overlay slot.
    overlay: bool,
}

impl CueEngine {
    pub fn new() -> CueEngine {
        CueEngine::default()
    }

    pub fn live(&self) -> Option<&CueItem> {
        self.live.as_ref().map(|(_, item)| item)
    }

    pub fn live_slot(&self) -> Option<SlotId> {
        self.live.as_ref().map(|(slot, _)| *slot)
    }

    pub fn set_overlay(&mut self, overlay: bool) {
        self.overlay = overlay;
        if !overlay {
            self.bed = None;
        }
    }

    fn overlay_slot(&self) -> SlotId {
        match self.bed.as_ref().map(|(slot, _)| *slot).or_else(|| self.live_slot()) {
            Some(SlotId::A) => SlotId::B,
            Some(SlotId::B) => SlotId::A,
            None => SlotId::A,
        }
    }

    /// The cue currently being prepared ("next"), if any.
    pub fn next(&self) -> Option<&CueItem> {
        self.pending.as_ref().map(|p| &p.item)
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn armed(&self) -> Option<(CueGen, CueScheduleId, SlotId)> {
        let pending = self.pending.as_ref()?;
        let PendingState::Armed { slot, schedule } = pending.state else { return None };
        Some((pending.gen, schedule, slot))
    }

    /// A slot that is neither live nor fading out, if one exists.
    fn free_slot(&self) -> Option<SlotId> {
        if self.overlay {
            return Some(self.overlay_slot());
        }
        for slot in [SlotId::A, SlotId::B] {
            let is_live = self.live.as_ref().is_some_and(|(s, _)| *s == slot);
            let is_fading = self
                .active_fade
                .is_some_and(|(_, fading_out)| fading_out == Some(slot));
            let is_reserved = self
                .pending
                .as_ref()
                .is_some_and(|p| {
                    matches!(
                        p.state,
                        PendingState::Preloading { slot: reserved }
                            | PendingState::Armed { slot: reserved, .. }
                            if reserved == slot
                    )
                });
            if !is_live && !is_fading && !is_reserved {
                return Some(slot);
            }
        }
        None
    }

    /// A tile was clicked. Supersedes any pending cue (latest click wins);
    /// a superseded preload's slot is closed immediately.
    pub fn click(&mut self, item: CueItem) -> Vec<CueCmd> {
        let mut cmds = Vec::new();
        self.gen += 1;
        self.last_error = None;
        if self.overlay {
            let target = self.overlay_slot();
            if self.live.as_ref().is_some_and(|(slot, _)| *slot == target) {
                cmds.push(CueCmd::CloseSlot { slot: target });
                self.live = self.bed.clone();
            }
        }
        if let Some(prev) = self.pending.take() {
            match prev.state {
                PendingState::Preloading { slot } => cmds.push(CueCmd::CloseSlot { slot }),
                PendingState::Armed { slot, schedule } => {
                    cmds.push(CueCmd::CancelArm { schedule });
                    cmds.push(CueCmd::CloseSlot { slot });
                }
                PendingState::Fetching | PendingState::WaitingSlot { .. } => {}
            }
        }
        cmds.push(CueCmd::FetchMedia { gen: self.gen, item: item.clone() });
        self.pending = Some(Pending { gen: self.gen, item, state: PendingState::Fetching });
        cmds
    }

    /// The media blob for `gen` is verified-local at `path`.
    pub fn media_ready(&mut self, gen: CueGen, path: PathBuf) -> Vec<CueCmd> {
        let current = self
            .pending
            .as_ref()
            .is_some_and(|p| p.gen == gen && p.state == PendingState::Fetching);
        if !current {
            return Vec::new(); // stale completion of a superseded click
        }
        let free = self.free_slot();
        let pending = self.pending.as_mut().expect("checked above");
        match free {
            Some(slot) => {
                pending.state = PendingState::Preloading { slot };
                vec![CueCmd::OpenSlot {
                    slot,
                    gen,
                    item: pending.item.clone(),
                    path,
                }]
            }
            None => {
                pending.state = PendingState::WaitingSlot { path };
                Vec::new()
            }
        }
    }

    /// The media fetch for `gen` failed; surfaces an honest error unless a
    /// newer click already superseded it.
    pub fn media_failed(&mut self, gen: CueGen, error: String) -> Vec<CueCmd> {
        if self.pending.as_ref().is_some_and(|p| p.gen == gen) {
            self.pending = None;
            self.last_error = Some(error);
        }
        Vec::new()
    }

    /// The slot holds a video frame and its bounded audio lead. It is only
    /// armed here: the current program remains live until the device clock
    /// starts this exact schedule.
    /// True while `gen` is still the preloading generation for `slot`.
    /// Decode completions must check this before touching slot state, so a
    /// late decode can't stomp media a newer click already owns.
    pub fn preroll_current(&self, slot: SlotId, gen: CueGen) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|p| p.gen == gen && p.state == PendingState::Preloading { slot })
    }

    pub fn preroll_ready(&mut self, slot: SlotId, gen: CueGen) -> Vec<CueCmd> {
        let matches = self
            .pending
            .as_ref()
            .is_some_and(|p| p.gen == gen && p.state == PendingState::Preloading { slot });
        if !matches {
            // The superseding click already reclaimed the old generation.
            // A late completion must not close a newer decoder that reused
            // the same physical slot.
            return Vec::new();
        }
        self.next_schedule = self.next_schedule.wrapping_add(1).max(1);
        let schedule = self.next_schedule;
        let from = self.live_slot();
        self.pending.as_mut().expect("checked above").state =
            PendingState::Armed { slot, schedule };
        vec![CueCmd::ArmFade { gen, schedule, from, to: slot }]
    }

    /// Confirm that the audio device clock started an armed cue. A stale or
    /// superseded identity is rejected; importantly it cannot mutate `live`.
    /// The superseding click/cancel path has already reclaimed its slot.
    pub fn start_armed(&mut self, gen: CueGen, schedule: CueScheduleId) -> Vec<CueCmd> {
        let matches = self.pending.as_ref().is_some_and(|pending| {
            pending.gen == gen
                && matches!(pending.state, PendingState::Armed { schedule: armed, .. } if armed == schedule)
        });
        if !matches {
            return Vec::new();
        }
        let pending = self.pending.take().expect("checked above");
        let PendingState::Armed { slot, .. } = pending.state else { unreachable!() };
        let from = self.live_slot();
        let previous = self.live.take();
        self.live = Some((slot, pending.item));
        if self.overlay {
            if let Some(previous) = previous {
                if previous.0 != slot {
                    self.bed = Some(previous);
                }
            }
            // Keep the bed slot; fade_complete must not CloseSlot it.
            self.active_fade = Some((schedule, None));
        } else {
            self.active_fade = Some((schedule, from));
        }
        vec![CueCmd::BeginFade { schedule, from, to: slot }]
    }

    /// Convenience for an unsynchronised host: arm and start immediately.
    /// Keeping this explicit avoids silently turning a missed beat into a
    /// late transition in the synchronised path.
    pub fn preroll_ready_immediate(&mut self, slot: SlotId, gen: CueGen) -> Vec<CueCmd> {
        let commands = self.preroll_ready(slot, gen);
        let Some(CueCmd::ArmFade { schedule, .. }) = commands.first().cloned() else {
            return commands;
        };
        self.start_armed(gen, schedule)
    }

    /// Cancel a still-armed cue, for example when the host decides a missed
    /// beat should be rescheduled by preparing a fresh cue. Merely missing a
    /// beat does not call `start_armed` late.
    pub fn cancel_armed(&mut self, gen: CueGen, schedule: CueScheduleId) -> Vec<CueCmd> {
        let matches = self.pending.as_ref().is_some_and(|pending| {
            pending.gen == gen
                && matches!(pending.state, PendingState::Armed { schedule: armed, .. } if armed == schedule)
        });
        if !matches {
            return Vec::new();
        }
        let pending = self.pending.take().expect("checked above");
        let PendingState::Armed { slot, .. } = pending.state else { unreachable!() };
        vec![CueCmd::CancelArm { schedule }, CueCmd::CloseSlot { slot }]
    }

    /// The decoder open failed (unsupported codec, torn file…).
    pub fn preroll_failed(&mut self, slot: SlotId, gen: CueGen, error: String) -> Vec<CueCmd> {
        let matches = self
            .pending
            .as_ref()
            .is_some_and(|p| {
                p.gen == gen
                    && matches!(
                        p.state,
                        PendingState::Preloading { slot: pending_slot }
                            | PendingState::Armed { slot: pending_slot, .. }
                            if pending_slot == slot
                    )
            });
        if !matches {
            return Vec::new();
        }
        let pending = self.pending.take().expect("checked above");
        if let PendingState::Armed { schedule, .. } = pending.state {
            self.last_error = Some(error);
            return vec![CueCmd::CancelArm { schedule }, CueCmd::CloseSlot { slot }];
        }
        self.last_error = Some(error);
        vec![CueCmd::CloseSlot { slot }]
    }

    /// The timed crossfade finished: reclaim the faded-out slot, and open
    /// any cue that was waiting for a free slot.
    pub fn fade_complete_for(&mut self, schedule: CueScheduleId) -> Vec<CueCmd> {
        if !self.active_fade.is_some_and(|(active, _)| active == schedule) {
            return Vec::new();
        }
        let mut cmds = Vec::new();
        if let Some((_, Some(slot))) = self.active_fade.take() {
            cmds.push(CueCmd::CloseSlot { slot });
        }
        let waiting = matches!(
            self.pending.as_ref().map(|p| &p.state),
            Some(PendingState::WaitingSlot { .. })
        );
        if waiting {
            if let Some(slot) = self.free_slot() {
                let pending = self.pending.as_mut().expect("checked above");
                if let PendingState::WaitingSlot { path } = pending.state.clone() {
                    pending.state = PendingState::Preloading { slot };
                    cmds.push(CueCmd::OpenSlot {
                        slot,
                        gen: pending.gen,
                        item: pending.item.clone(),
                        path,
                    });
                }
            }
        }
        cmds
    }

    /// Legacy/immediate host helper. Synchronised hosts should always use
    /// `fade_complete_for` with the device-clock snapshot's identity.
    pub fn fade_complete(&mut self) -> Vec<CueCmd> {
        let Some((schedule, _)) = self.active_fade else { return Vec::new() };
        self.fade_complete_for(schedule)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(seed: u8) -> CueItem {
        CueItem {
            asset: AssetId::from_bytes([seed; 16]),
            revision: AssetRevisionId::from_bytes([seed; 32]),
            title: format!("clip {seed}"),
            media_blob: BlobId::from_bytes([seed ^ 0xff; 32]),
            media_len: 1000 + seed as u64,
            media: MediaType::Mp4,
        }
    }

    fn fetch_gen(cmds: &[CueCmd]) -> CueGen {
        cmds.iter()
            .find_map(|c| match c {
                CueCmd::FetchMedia { gen, .. } => Some(*gen),
                _ => None,
            })
            .expect("fetch command")
    }

    fn open(cue: &mut CueEngine, gen: CueGen, seed: u8) -> SlotId {
        let commands = cue.media_ready(gen, format!("/m/{seed}").into());
        let CueCmd::OpenSlot { slot, gen: command_gen, .. } = commands[0].clone() else {
            panic!("expected open slot")
        };
        assert_eq!(command_gen, gen);
        slot
    }

    fn arm(cue: &mut CueEngine, slot: SlotId, gen: CueGen) -> CueScheduleId {
        let commands = cue.preroll_ready(slot, gen);
        let CueCmd::ArmFade { gen: command_gen, schedule, to, .. } = commands[0] else {
            panic!("expected armed fade")
        };
        assert_eq!((command_gen, to), (gen, slot));
        schedule
    }

    fn start(cue: &mut CueEngine, gen: CueGen, schedule: CueScheduleId) -> CueCmd {
        let commands = cue.start_armed(gen, schedule);
        assert_eq!(commands.len(), 1);
        commands.into_iter().next().unwrap()
    }

    fn make_live(cue: &mut CueEngine, seed: u8) -> (CueGen, CueScheduleId, SlotId) {
        let gen = fetch_gen(&cue.click(item(seed)));
        let slot = open(cue, gen, seed);
        let schedule = arm(cue, slot, gen);
        start(cue, gen, schedule);
        (gen, schedule, slot)
    }

    #[test]
    fn preroll_arms_without_claiming_live_then_start_promotes() {
        let mut cue = CueEngine::new();
        let gen = fetch_gen(&cue.click(item(1)));
        let slot = open(&mut cue, gen, 1);
        let schedule = arm(&mut cue, slot, gen);
        assert_eq!(cue.armed(), Some((gen, schedule, slot)));
        assert!(cue.live().is_none(), "armed is not live before the device clock starts");
        assert_eq!(cue.next().unwrap().title, "clip 1");

        assert_eq!(
            start(&mut cue, gen, schedule),
            CueCmd::BeginFade { schedule, from: None, to: slot }
        );
        assert_eq!(cue.live().unwrap().title, "clip 1");
        assert!(cue.next().is_none());
        assert!(cue.fade_complete_for(schedule).is_empty());
    }

    #[test]
    fn immediate_helper_does_not_leak_an_arm_command_to_unsynced_host() {
        let mut cue = CueEngine::new();
        let gen = fetch_gen(&cue.click(item(1)));
        let slot = open(&mut cue, gen, 1);
        let commands = cue.preroll_ready_immediate(slot, gen);
        assert_eq!(commands.len(), 1);
        assert!(matches!(commands[0], CueCmd::BeginFade { to, .. } if to == slot));
        assert_eq!(cue.live_slot(), Some(slot));
    }

    #[test]
    fn old_slot_is_retained_until_actual_start_and_fade_completion() {
        let mut cue = CueEngine::new();
        let (_, first_schedule, old) = make_live(&mut cue, 1);
        cue.fade_complete_for(first_schedule);

        let gen = fetch_gen(&cue.click(item(2)));
        assert_eq!(cue.next().unwrap().title, "clip 2");
        let new = open(&mut cue, gen, 2);
        assert_eq!(new, old.other());
        let schedule = arm(&mut cue, new, gen);
        assert_eq!(cue.live_slot(), Some(old), "arming must retain the old program");
        assert!(cue.fade_complete_for(schedule).is_empty(), "an unstarted arm has no fade");

        assert_eq!(
            start(&mut cue, gen, schedule),
            CueCmd::BeginFade { schedule, from: Some(old), to: new }
        );
        assert_eq!(cue.live_slot(), Some(new));
        assert_eq!(cue.fade_complete_for(schedule - 1), Vec::<CueCmd>::new());
        assert_eq!(cue.fade_complete_for(schedule), vec![CueCmd::CloseSlot { slot: old }]);
        assert_eq!(cue.live().unwrap().title, "clip 2");
    }

    #[test]
    fn superseding_an_armed_cue_cancels_schedule_and_reclaims_slot() {
        let mut cue = CueEngine::new();
        let gen1 = fetch_gen(&cue.click(item(1)));
        let slot = open(&mut cue, gen1, 1);
        let schedule = arm(&mut cue, slot, gen1);

        let commands = cue.click(item(2));
        assert_eq!(commands[0], CueCmd::CancelArm { schedule });
        assert_eq!(commands[1], CueCmd::CloseSlot { slot });
        let gen2 = fetch_gen(&commands);
        assert!(gen2 > gen1);
        assert!(cue.start_armed(gen1, schedule).is_empty());
        assert!(cue.live().is_none());
    }

    #[test]
    fn stale_preroll_completion_cannot_close_a_reused_physical_slot() {
        let mut cue = CueEngine::new();
        let old_gen = fetch_gen(&cue.click(item(1)));
        let slot = open(&mut cue, old_gen, 1);
        let new_gen = fetch_gen(&cue.click(item(2)));
        let reused = open(&mut cue, new_gen, 2);
        assert_eq!(reused, slot);
        assert!(cue.preroll_ready(slot, old_gen).is_empty());
        assert!(cue
            .preroll_failed(slot, old_gen, "late decoder error".into())
            .is_empty());
        let schedule = arm(&mut cue, reused, new_gen);
        start(&mut cue, new_gen, schedule);
        assert_eq!(cue.live().unwrap().title, "clip 2");
    }

    #[test]
    fn stale_schedule_start_is_rejected_without_touching_current_arm() {
        let mut cue = CueEngine::new();
        let gen = fetch_gen(&cue.click(item(1)));
        let slot = open(&mut cue, gen, 1);
        let schedule = arm(&mut cue, slot, gen);
        assert!(cue.start_armed(gen, schedule.wrapping_add(1)).is_empty());
        assert!(cue.start_armed(gen.wrapping_add(1), schedule).is_empty());
        assert_eq!(cue.armed(), Some((gen, schedule, slot)));
        assert_eq!(cue.live_slot(), None);
        start(&mut cue, gen, schedule);
        assert_eq!(cue.live_slot(), Some(slot));
    }

    #[test]
    fn click_mid_fade_waits_and_opens_after_matching_completion() {
        let mut cue = CueEngine::new();
        let (_, first_schedule, old) = make_live(&mut cue, 1);
        cue.fade_complete_for(first_schedule);
        let gen2 = fetch_gen(&cue.click(item(2)));
        let new = open(&mut cue, gen2, 2);
        let second_schedule = arm(&mut cue, new, gen2);
        start(&mut cue, gen2, second_schedule);

        // Click clip 3 mid-fade: both slots busy, so media-ready parks.
        let gen3 = fetch_gen(&cue.click(item(3)));
        assert!(cue.media_ready(gen3, "/m/3".into()).is_empty());
        assert!(cue.fade_complete_for(first_schedule).is_empty());
        // Fade completes: old slot closes AND the parked cue opens on it.
        let commands = cue.fade_complete_for(second_schedule);
        assert_eq!(commands[0], CueCmd::CloseSlot { slot: old });
        let CueCmd::OpenSlot { slot, gen, .. } = commands[1].clone() else {
            panic!("parked cue must open");
        };
        assert_eq!((slot, gen), (old, gen3));
    }

    #[test]
    fn cancel_and_rearm_gets_a_fresh_identity() {
        let mut cue = CueEngine::new();
        let gen1 = fetch_gen(&cue.click(item(1)));
        let slot1 = open(&mut cue, gen1, 1);
        let schedule1 = arm(&mut cue, slot1, gen1);
        assert_eq!(
            cue.cancel_armed(gen1, schedule1),
            vec![CueCmd::CancelArm { schedule: schedule1 }, CueCmd::CloseSlot { slot: slot1 }]
        );
        assert!(cue.start_armed(gen1, schedule1).is_empty());

        let gen2 = fetch_gen(&cue.click(item(1)));
        let slot2 = open(&mut cue, gen2, 1);
        let schedule2 = arm(&mut cue, slot2, gen2);
        assert_ne!(schedule1, schedule2);
        start(&mut cue, gen2, schedule2);
    }

    #[test]
    fn failures_surface_unless_superseded() {
        let mut cue = CueEngine::new();
        let gen = fetch_gen(&cue.click(item(1)));
        cue.media_failed(gen, "digest mismatch".into());
        assert_eq!(cue.last_error(), Some("digest mismatch"));
        assert!(cue.next().is_none());

        let old = fetch_gen(&cue.click(item(2)));
        let current = fetch_gen(&cue.click(item(3)));
        cue.media_failed(old, "late loser".into());
        assert!(cue.last_error().is_none());
        let slot = open(&mut cue, current, 3);
        assert_eq!(
            cue.preroll_failed(slot, current, "codec unsupported".into()),
            vec![CueCmd::CloseSlot { slot }]
        );
        assert_eq!(cue.last_error(), Some("codec unsupported"));
    }

    #[test]
    fn overlay_keeps_the_bed_slot_and_replaces_the_overlay() {
        let mut cue = CueEngine::new();
        cue.set_overlay(true);
        let (_, first_schedule, bed) = make_live(&mut cue, 1);
        assert_eq!(bed, SlotId::A);
        assert!(cue.fade_complete_for(first_schedule).is_empty());

        let gen2 = fetch_gen(&cue.click(item(2)));
        let overlay = open(&mut cue, gen2, 2);
        assert_eq!(overlay, SlotId::B);
        let second = arm(&mut cue, overlay, gen2);
        start(&mut cue, gen2, second);
        assert!(
            cue.fade_complete_for(second).is_empty(),
            "overlay fade must not close the bed"
        );
        assert_eq!(cue.live_slot(), Some(SlotId::B));

        let cmds = cue.click(item(3));
        assert!(cmds.iter().any(|c| matches!(c, CueCmd::CloseSlot { slot } if *slot == SlotId::B)));
        assert!(!cmds.iter().any(|c| matches!(c, CueCmd::CloseSlot { slot } if *slot == SlotId::A)));
        let gen3 = fetch_gen(&cmds);
        assert_eq!(open(&mut cue, gen3, 3), SlotId::B);
    }
}
