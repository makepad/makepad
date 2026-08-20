//! Video program cue engine: two playback slots, latest-click-wins.
//!
//! Pure state machine — no clocks, sockets or decoders. The app feeds it
//! clicks and typed completions (each stamped with the generation that
//! issued it) and executes the commands it returns:
//!
//! click tile ──► FetchMedia(gen) ──media_ready──► OpenSlot(preroll)
//!        ──preroll_ready──► ArmFade ──start_armed──► BeginFade
//!        ──fade_complete──► HoldSlot(old)  (parked until a new cue claims it)
//!
//! Latest-click-wins: every click bumps the generation, so completions of a
//! superseded click are stale and ignored (or their slot is closed if it
//! already opened). During a fade both slots are busy; a ready cue then
//! waits and opens as soon as the fade releases a slot. Slot closes are
//! commands to the host, which reclaims decoders asynchronously — never a
//! join on the UI thread.

use makepad_asset_data::{AssetId, AssetKind, AssetRevisionId, BlobId, MediaType};
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

/// A second blob the cue's media is useless without. A grouped sprite actor
/// publishes ONE packed sheet plus the `stateful-billboard` manifest that
/// cuts it; the host fetches both and only reports `media_ready` when both
/// are local, so the engine stays a single-media state machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CueSidecar {
    pub blob: BlobId,
    pub len: u64,
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
    pub sidecar: Option<CueSidecar>,
    /// Catalog kind. The media type alone cannot separate a walkable level
    /// from a prop — both publish a `RenderGlb` — so the slot's presentation
    /// (walk-through vs turntable) keys on this.
    pub kind: Option<AssetKind>,
}

impl CueItem {
    /// A clicked catalog tile as a cue, or `None` while its manifest is
    /// still unresolved (no revision / no media blob yet). The caller must
    /// treat `None` as "defer this click until the manifest lands" — never
    /// as "drop it", or a freshly listed tile needs a second click.
    ///
    /// This is the whole of the click → cue conversion, kept pure so the
    /// grid → cue → armed-fade route stays pinned by tests.
    pub fn from_tile(tile: &crate::catalog::Tile) -> Option<CueItem> {
        let revision = tile.revision?;
        let media = tile.media.clone()?;
        Some(CueItem {
            asset: tile.asset,
            revision,
            title: tile.title.clone(),
            media_blob: media.blob,
            media_len: media.len,
            media: media.media,
            // Grouped sprite actor: the sheet alone is not playable.
            sidecar: tile
                .source
                .as_ref()
                .map(|s| CueSidecar { blob: s.blob, len: s.len }),
            kind: tile.kind,
        })
    }
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
    /// The faded-out slot stays on its last picture, paused and silent,
    /// until a newer cue claims the slot (that cue's `OpenSlot` replaces
    /// it). Nothing is reclaimed here.
    HoldSlot { slot: SlotId },
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
    /// The program fading out right now; becomes `held` when the fade lands.
    outgoing: Option<(SlotId, CueItem)>,
    /// The previous program, parked on its slot after the fade: visible but
    /// paused, and free for the next cue to replace.
    held: Option<(SlotId, CueItem)>,
    last_error: Option<String>,
    /// WHOSE load failed, so a grid can put the error back on the tile the
    /// operator clicked instead of only in the status bar. Cleared by the
    /// next click, exactly like `last_error`.
    last_error_asset: Option<AssetId>,
    /// B over A: both slots stay open; new cues replace the overlay slot.
    overlay: bool,
    /// Where the operator's crossfader stands (0 = A, 1 = B). The next cue
    /// loads into the slot FARTHEST from it, like preloading the other
    /// channel on a hardware mixer.
    fader: f32,
}

impl CueEngine {
    pub fn new() -> CueEngine {
        CueEngine::default()
    }

    pub fn live(&self) -> Option<&CueItem> {
        self.live.as_ref().map(|(_, item)| item)
    }

    /// The previous program still parked on a slot, if any.
    pub fn held(&self) -> Option<(SlotId, &CueItem)> {
        self.held.as_ref().map(|(slot, item)| (*slot, item))
    }

    /// A cue is about to own `slot` (open or close): whatever was parked
    /// there is gone.
    fn forget_held(&mut self, slot: SlotId) {
        if self.held.as_ref().is_some_and(|(held, _)| *held == slot) {
            self.held = None;
        }
    }

    fn close_slot(&mut self, slot: SlotId) -> CueCmd {
        self.forget_held(slot);
        CueCmd::CloseSlot { slot }
    }

    fn open_slot(&mut self, slot: SlotId, gen: CueGen, item: CueItem, path: PathBuf) -> CueCmd {
        self.forget_held(slot);
        CueCmd::OpenSlot { slot, gen, item, path }
    }

    pub fn live_slot(&self) -> Option<SlotId> {
        self.live.as_ref().map(|(slot, _)| *slot)
    }

    /// Crossfader position, 0 = A .. 1 = B. Fully on one side also makes
    /// that side the program as far as the next fade is concerned.
    pub fn set_fader(&mut self, fader: f32) {
        self.fader = fader.clamp(0.0, 1.0);
    }

    /// The slot the fader is (mostly) showing, and the other one.
    fn fader_sides(&self) -> (SlotId, SlotId) {
        if self.fader < 0.5 {
            (SlotId::A, SlotId::B)
        } else {
            (SlotId::B, SlotId::A)
        }
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

    /// The asset whose load produced [`Self::last_error`], if the failure
    /// belonged to one.
    pub fn failed_asset(&self) -> Option<AssetId> {
        self.last_error_asset
    }

    /// The cue being PREPARED right now — fetching, opening a decoder or
    /// pre-rolling — and therefore the one a grid should mark as busy. An
    /// armed cue is not busy: its media is ready and it is only waiting for
    /// the beat it was scheduled on.
    pub fn loading_asset(&self) -> Option<AssetId> {
        let pending = self.pending.as_ref()?;
        match pending.state {
            PendingState::Armed { .. } => None,
            _ => Some(pending.item.asset),
        }
    }

    pub fn armed(&self) -> Option<(CueGen, CueScheduleId, SlotId)> {
        let pending = self.pending.as_ref()?;
        let PendingState::Armed { slot, schedule } = pending.state else { return None };
        Some((pending.gen, schedule, slot))
    }

    /// The transition the engine believes is running right now. Until it is
    /// landed with `fade_complete_for`, BOTH its slots are spoken for (the
    /// outgoing one is "still fading", the incoming one is live), so every
    /// later click parks in `WaitingSlot`. A host that starts a fade must
    /// therefore always be able to land it again — see
    /// `a_started_fade_that_is_never_landed_wedges_every_later_click`.
    pub fn active_fade(&self) -> Option<CueScheduleId> {
        self.active_fade.map(|(schedule, _)| schedule)
    }

    /// The slot the next cue should load into. With a program playing, the
    /// slot FARTHEST from the fader first (preload the other channel); with
    /// nothing playing, the slot the fader is on (so the cue shows up where
    /// the hand is). Never a slot that is fading out or reserved. The live
    /// slot is only taken when the operator has faded hard away from it by
    /// hand and no fade is running — then the fader says it is off screen.
    fn free_slot(&self) -> Option<SlotId> {
        if self.overlay {
            return Some(self.overlay_slot());
        }
        let (near, far) = self.fader_sides();
        let order = if self.live.is_none() { [near, far] } else { [far, near] };
        for slot in order {
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
            if is_fading || is_reserved {
                continue;
            }
            if is_live && !self.faded_away_from(slot) {
                continue;
            }
            return Some(slot);
        }
        None
    }

    /// True when no fade is running and the fader sits hard on the other
    /// side of `slot` — the operator took it off screen by hand.
    fn faded_away_from(&self, slot: SlotId) -> bool {
        self.active_fade.is_none()
            && match slot {
                SlotId::A => self.fader > 0.9,
                SlotId::B => self.fader < 0.1,
            }
    }

    /// A tile was clicked. Supersedes any pending cue (latest click wins);
    /// a superseded preload's slot is closed immediately.
    pub fn click(&mut self, item: CueItem) -> Vec<CueCmd> {
        let mut cmds = Vec::new();
        self.gen += 1;
        self.last_error = None;
        self.last_error_asset = None;
        if self.overlay {
            let target = self.overlay_slot();
            if self.live.as_ref().is_some_and(|(slot, _)| *slot == target) {
                cmds.push(self.close_slot(target));
                self.live = self.bed.clone();
            }
        }
        if let Some(prev) = self.pending.take() {
            match prev.state {
                PendingState::Preloading { slot } => cmds.push(self.close_slot(slot)),
                PendingState::Armed { slot, schedule } => {
                    cmds.push(CueCmd::CancelArm { schedule });
                    cmds.push(self.close_slot(slot));
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
        if let Some(slot) = free {
            if self.live.as_ref().is_some_and(|(live, _)| *live == slot) {
                // Taking the far slot out from under the engine's "live":
                // whatever is parked on the near side is the program now.
                let (near, _) = self.fader_sides();
                self.live = match self.held.take() {
                    Some((held_slot, item)) if held_slot == near => Some((near, item)),
                    other => {
                        self.held = other;
                        None
                    }
                };
            }
        }
        let pending = self.pending.as_mut().expect("checked above");
        match free {
            Some(slot) => {
                pending.state = PendingState::Preloading { slot };
                let item = pending.item.clone();
                vec![self.open_slot(slot, gen, item, path)]
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
        if let Some(pending) = self.pending.take_if(|p| p.gen == gen) {
            self.last_error_asset = Some(pending.item.asset);
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
            // Keep the bed slot; fade_complete must not touch it.
            self.active_fade = Some((schedule, None));
            self.outgoing = None;
        } else {
            self.active_fade = Some((schedule, from));
            // Parked on its slot once the fade lands (see fade_complete_for).
            self.outgoing = previous.filter(|(old, _)| Some(*old) == from);
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
        vec![CueCmd::CancelArm { schedule }, self.close_slot(slot)]
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
        self.last_error_asset = Some(pending.item.asset);
        if let PendingState::Armed { schedule, .. } = pending.state {
            self.last_error = Some(error);
            return vec![CueCmd::CancelArm { schedule }, self.close_slot(slot)];
        }
        self.last_error = Some(error);
        vec![self.close_slot(slot)]
    }

    /// The timed crossfade finished: reclaim the faded-out slot, and open
    /// any cue that was waiting for a free slot.
    pub fn fade_complete_for(&mut self, schedule: CueScheduleId) -> Vec<CueCmd> {
        if !self.active_fade.is_some_and(|(active, _)| active == schedule) {
            return Vec::new();
        }
        let mut cmds = Vec::new();
        // A landed fade puts the fader on the new program's side (the host
        // mirrors the same value onto the physical fader).
        self.fader = match self.live_slot() {
            Some(SlotId::A) => 0.0,
            Some(SlotId::B) => 1.0,
            None => self.fader,
        };
        if let Some((_, Some(slot))) = self.active_fade.take() {
            // The outgoing program stays on screen (paused, silent) until a
            // newer cue claims this slot; the operator keeps seeing what
            // was just playing instead of a blank well.
            match self.outgoing.take() {
                Some((old, item)) if old == slot => {
                    self.held = Some((slot, item));
                    cmds.push(CueCmd::HoldSlot { slot });
                }
                _ => cmds.push(self.close_slot(slot)),
            }
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
                    let (gen, item) = (pending.gen, pending.item.clone());
                    cmds.push(self.open_slot(slot, gen, item, path));
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
            sidecar: None,
            kind: Some(AssetKind::Video),
        }
    }

    /// A catalog tile exactly as the grid holds one: `resolved` false is a
    /// hit whose manifest has not landed yet.
    fn tile(seed: u8, resolved: bool) -> crate::catalog::Tile {
        crate::catalog::Tile {
            asset: AssetId::from_bytes([seed; 16]),
            title: format!("clip {seed}"),
            alias: None,
            live: true,
            kind: Some(AssetKind::Video),
            revision: resolved.then(|| AssetRevisionId::from_bytes([seed; 32])),
            media: resolved.then(|| crate::catalog::TileMedia {
                blob: BlobId::from_bytes([seed ^ 0xff; 32]),
                len: 1000 + seed as u64,
                media: MediaType::Mp4,
            }),
            source: None,
            thumb: None,
            state: if resolved {
                crate::catalog::TileState::Ready
            } else {
                crate::catalog::TileState::Listed
            },
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

    /// The whole operator gesture, pinned end to end: a click on a grid
    /// tile becomes the NEXT cue ("standby"), pre-rolls on the standby
    /// slot, arms a fade off the live slot, and the fade parks the old
    /// program. A regression anywhere on this route (a tile that never
    /// becomes a `CueItem`, a click that never reaches `click`, an arm
    /// that never names the live slot as `from`) fails here.
    #[test]
    fn clicking_a_tile_cues_standby_and_arms_the_fade_off_the_live_slot() {
        let mut cue = CueEngine::new();
        // First click: nothing live, so the cue takes the fader's slot and
        // fades up from nothing.
        let first = CueItem::from_tile(&tile(1, true)).expect("a resolved tile is cueable");
        let gen1 = fetch_gen(&cue.click(first));
        let live_slot = open(&mut cue, gen1, 1);
        let schedule1 = arm(&mut cue, live_slot, gen1);
        start(&mut cue, gen1, schedule1);
        cue.fade_complete_for(schedule1);
        assert_eq!(cue.live().map(|i| i.title.as_str()), Some("clip 1"));
        assert!(cue.next().is_none(), "nothing is on standby once the cue is live");

        // Second click — the case the operator actually watches: the tile
        // must land in the standby label immediately, on the far slot, and
        // arm a fade whose `from` is the live program.
        let second = CueItem::from_tile(&tile(2, true)).expect("a resolved tile is cueable");
        let cmds = cue.click(second);
        assert!(
            matches!(cmds.as_slice(), [CueCmd::FetchMedia { .. }]),
            "a click on a resolved tile fetches its media at once: {cmds:?}"
        );
        assert_eq!(
            cue.next().map(|i| i.title.as_str()),
            Some("clip 2"),
            "the standby label reads the clicked tile from the click onwards"
        );
        let gen2 = fetch_gen(&cmds);
        let standby = open(&mut cue, gen2, 2);
        assert_eq!(standby, live_slot.other(), "standby is the far slot");
        assert_eq!(
            cue.live().map(|i| i.title.as_str()),
            Some("clip 1"),
            "the old program stays on air while the new one pre-rolls"
        );
        let schedule2 = arm(&mut cue, standby, gen2);
        assert_eq!(cue.armed(), Some((gen2, schedule2, standby)));
        assert_eq!(
            start(&mut cue, gen2, schedule2),
            CueCmd::BeginFade { schedule: schedule2, from: Some(live_slot), to: standby },
            "the armed fade runs off the live slot onto standby"
        );
        assert_eq!(cue.live().map(|i| i.title.as_str()), Some("clip 2"));
        assert_eq!(cue.fade_complete_for(schedule2), vec![CueCmd::HoldSlot { slot: live_slot }]);
    }

    /// A tile whose manifest has not landed is NOT cueable — the host must
    /// defer the click (resolve-first + `pending_click`) rather than build
    /// a half cue or drop the gesture.
    #[test]
    fn an_unresolved_tile_is_not_cueable_and_a_resolved_one_carries_its_sidecar() {
        assert!(CueItem::from_tile(&tile(7, false)).is_none());
        // Revision without media, and media without revision, are both
        // "not resolved yet".
        let mut half = tile(7, true);
        half.media = None;
        assert!(CueItem::from_tile(&half).is_none());
        let mut half = tile(7, true);
        half.revision = None;
        assert!(CueItem::from_tile(&half).is_none());

        let mut grouped = tile(7, true);
        grouped.kind = Some(AssetKind::Billboard);
        grouped.source = Some(crate::catalog::TileMedia {
            blob: BlobId::from_bytes([0x5a; 32]),
            len: 42,
            media: MediaType::Mp4,
        });
        let item = CueItem::from_tile(&grouped).expect("a resolved sheet is cueable");
        assert_eq!(
            item.sidecar,
            Some(CueSidecar { blob: BlobId::from_bytes([0x5a; 32]), len: 42 }),
            "a grouped sprite actor cues sheet + manifest, never the sheet alone"
        );
        assert_eq!(item.kind, Some(AssetKind::Billboard));
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
        assert_eq!(cue.fade_complete_for(schedule), vec![CueCmd::HoldSlot { slot: old }]);
        assert_eq!(cue.live().unwrap().title, "clip 2");
        // The outgoing program is parked, not blanked: still on its slot
        // until the next cue wants that slot.
        assert_eq!(cue.held().map(|(slot, item)| (slot, item.title.as_str())), Some((old, "clip 1")));
    }

    #[test]
    fn next_cue_loads_into_the_slot_farthest_from_the_fader() {
        let mut cue = CueEngine::new();
        // Nothing playing and the fader on B: the first cue shows up on B.
        cue.set_fader(1.0);
        let gen = fetch_gen(&cue.click(item(1)));
        assert_eq!(open(&mut cue, gen, 1), SlotId::B);
        let schedule = arm(&mut cue, SlotId::B, gen);
        start(&mut cue, gen, schedule);
        cue.fade_complete_for(schedule);
        // Program on B: the next cue preloads the far side, A.
        let gen2 = fetch_gen(&cue.click(item(2)));
        assert_eq!(open(&mut cue, gen2, 2), SlotId::A);
        let schedule2 = arm(&mut cue, SlotId::A, gen2);
        start(&mut cue, gen2, schedule2);
        cue.fade_complete_for(schedule2);
        assert_eq!(cue.live_slot(), Some(SlotId::A));
        // The operator drags the fader hard back to B by hand (B still holds
        // clip 1): A is "live" to the engine but off screen, so the next cue
        // takes A and the parked B clip becomes the program.
        cue.set_fader(1.0);
        let gen3 = fetch_gen(&cue.click(item(3)));
        assert_eq!(open(&mut cue, gen3, 3), SlotId::A);
        assert_eq!(cue.live().map(|i| i.title.as_str()), Some("clip 1"));
        assert!(cue.held().is_none());
    }

    #[test]
    fn held_program_stays_until_a_new_cue_claims_its_slot() {
        let mut cue = CueEngine::new();
        let (_, first_schedule, old) = make_live(&mut cue, 1);
        cue.fade_complete_for(first_schedule);
        let gen2 = fetch_gen(&cue.click(item(2)));
        let new = open(&mut cue, gen2, 2);
        let second = arm(&mut cue, new, gen2);
        start(&mut cue, gen2, second);
        assert_eq!(cue.fade_complete_for(second), vec![CueCmd::HoldSlot { slot: old }]);
        assert_eq!(cue.held().map(|(slot, _)| slot), Some(old));

        // A third cue takes the held slot: OpenSlot replaces the parked
        // program, and `held` is gone the moment the slot is claimed.
        let gen3 = fetch_gen(&cue.click(item(3)));
        assert!(cue.held().is_some(), "a click alone does not unpark");
        assert_eq!(open(&mut cue, gen3, 3), old);
        assert!(cue.held().is_none());
        let third = arm(&mut cue, old, gen3);
        start(&mut cue, gen3, third);
        // Now the other slot (clip 2) parks in turn.
        assert_eq!(cue.fade_complete_for(third), vec![CueCmd::HoldSlot { slot: new }]);
        assert_eq!(cue.held().map(|(_, item)| item.title.as_str()), Some("clip 2"));
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
        // Fade completes: the old program parks on its slot AND the waiting
        // cue immediately claims that same slot (so nothing stays parked).
        let commands = cue.fade_complete_for(second_schedule);
        assert_eq!(commands[0], CueCmd::HoldSlot { slot: old });
        assert!(cue.held().is_none(), "the waiting cue claimed the held slot");
        let CueCmd::OpenSlot { slot, gen, .. } = commands[1].clone() else {
            panic!("parked cue must open");
        };
        assert_eq!((slot, gen), (old, gen3));
    }

    /// The wedge behind "clicking a tile stopped cueing anything".
    ///
    /// A fade that STARTS but is never landed leaves both slots spoken for
    /// — the outgoing one is still fading, the incoming one is live — so
    /// `free_slot` runs out of answers and every later click parks in
    /// `WaitingSlot` forever: the standby label fills in, the program never
    /// moves, and the crossfade never happens again for the rest of the
    /// session. `fade_complete_for` is the only release, which is why the
    /// host must land EVERY fade it starts, including the ones the audio
    /// mixer refused and the host cut itself (`App::arm_fade`) and the ones
    /// whose `Completed` the single-slot phase mailbox dropped
    /// (`stale_fade_to_land`).
    #[test]
    fn a_started_fade_that_is_never_landed_wedges_every_later_click() {
        let mut cue = CueEngine::new();
        let (_, first, live) = make_live(&mut cue, 1);
        cue.fade_complete_for(first);
        assert_eq!(cue.active_fade(), None, "a landed fade holds no slots");

        // Second click: pre-rolls on the far slot and starts its fade…
        let gen2 = fetch_gen(&cue.click(item(2)));
        let standby = open(&mut cue, gen2, 2);
        let second = arm(&mut cue, standby, gen2);
        start(&mut cue, gen2, second);
        assert_eq!(cue.active_fade(), Some(second));

        // …but nothing lands it, so a third click has nowhere to go. This
        // is the exact failure the operator sees: the click registers, the
        // media is fetched, and no slot ever opens.
        let gen3 = fetch_gen(&cue.click(item(3)));
        assert!(
            cue.media_ready(gen3, "/m/3".into()).is_empty(),
            "an unlanded fade leaves no free slot"
        );
        assert_eq!(cue.next().map(|i| i.title.as_str()), Some("clip 3"));
        assert_eq!(cue.live().map(|i| i.title.as_str()), Some("clip 2"));

        // Landing it is the whole cure: the old program parks and the cue
        // that was waiting claims the slot at once.
        let commands = cue.fade_complete_for(second);
        assert_eq!(commands[0], CueCmd::HoldSlot { slot: live });
        let CueCmd::OpenSlot { slot, gen, .. } = commands[1].clone() else {
            panic!("the parked cue must open once the fade lands: {commands:?}");
        };
        assert_eq!((slot, gen), (live, gen3));
        assert_eq!(cue.active_fade(), None);

        // And the grid keeps working afterwards: click four still cues.
        let third = arm(&mut cue, slot, gen3);
        start(&mut cue, gen3, third);
        cue.fade_complete_for(third);
        let gen4 = fetch_gen(&cue.click(item(4)));
        assert_eq!(
            open(&mut cue, gen4, 4),
            standby,
            "the released slot is reused by the next click"
        );
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
