//! Players — the multiplayer refactor surface (game.md review finding 2).
//!
//! The audited engine had exactly one of everything a player owns: one input
//! set, one camera rig, one HUD, one `poll_gamepad` that discarded all but the
//! most active pad. Those are the singletons this module dissolves.
//!
//! **Player 0 is always the local device.** Its input still lives in the
//! world's original `held`/`pressed`/`pad`/`cam_yaw` fields, so the
//! single-player numeric path is untouched (input tapes replay bit-identically
//! — that is a gate, not an aspiration). Remote players carry their own
//! [`PlayerInput`], fed from the network; bots carry one filled by script.
//!
//! The knot the review flagged: `move_x`/`move_z` are camera-relative, so
//! per-player movement needs per-player camera yaw. The resolution is that
//! `cam_yaw` travels *inside the input frame* — a client's camera rig stays
//! pure presentation, and the host resolves movement for everyone with the
//! same expression it always used.

use makepad_live_id::*;
use std::collections::HashSet;

use crate::entity::{HudBar, HudSlot, PadState};

/// Stable identity of a participant. 0 is the local device's player, which
/// exists in every world including single-player ones.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PlayerId(pub u32);

impl PlayerId {
    pub const LOCAL: PlayerId = PlayerId(0);
    pub fn is_local_slot(self) -> bool {
        self.0 == 0
    }
}

/// How a player's intent reaches the host.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlayerSource {
    /// This device's keyboard/pad — always player 0.
    #[default]
    Local,
    /// A remote client's input packets.
    Remote,
    /// Host-side, no device: script or a block drives it.
    Bot,
}

/// One player's control state for the current tick.
///
/// For player 0 this is a *mirror* of the world's device fields, refreshed
/// once per tick; the authoritative copy stays where it always was so that
/// `action_held`, `build_input_object` and the tape path keep their exact
/// expressions. For everyone else this is the authoritative copy.
#[derive(Clone, Debug, Default)]
pub struct PlayerInput {
    pub held: HashSet<LiveId>,
    pub pressed: HashSet<LiveId>,
    pub pad: PadState,
    /// This player's camera yaw, carried in their input packet. Movement is
    /// resolved against it, so the camera rig itself never has to replicate.
    pub cam_yaw: f32,
    pub look_dx: f64,
    pub look_dy: f64,
}

impl PlayerInput {
    /// Keyboard OR gamepad, identical in shape to [`crate::GameWorld::action_held`]
    /// — the same expression, reading this player's devices.
    pub fn held(&self, action: LiveId) -> bool {
        if self.held.contains(&action) {
            return true;
        }
        match action {
            x if x == live_id!(jump) => self.pad.jump,
            x if x == live_id!(shoot) => self.pad.shoot,
            x if x == live_id!(grab) => self.pad.grab,
            x if x == live_id!(reset) => self.pad.reset,
            x if x == live_id!(left) => self.pad.axis_x < -0.5,
            x if x == live_id!(right) => self.pad.axis_x > 0.5,
            x if x == live_id!(up) => self.pad.axis_z < -0.5,
            x if x == live_id!(down) => self.pad.axis_z > 0.5,
            _ => false,
        }
    }

    pub fn pressed(&self, action: LiveId) -> bool {
        if self.pressed.contains(&action) {
            return true;
        }
        match action {
            x if x == live_id!(jump) => self.pad.jump_pressed,
            x if x == live_id!(shoot) => self.pad.shoot_pressed,
            x if x == live_id!(grab) => self.pad.grab_pressed,
            x if x == live_id!(reset) => self.pad.reset_pressed,
            _ => false,
        }
    }

    /// Digital keys and the analog stick, clamped — the expression the input
    /// object and the block driver have always used.
    pub fn axes(&self) -> (f64, f64) {
        let key = |name: LiveId| self.held.contains(&name);
        let axis_x = ((key(live_id!(right)) as i8 - key(live_id!(left)) as i8) as f64
            + self.pad.axis_x)
            .clamp(-1.0, 1.0);
        let axis_z = ((key(live_id!(down)) as i8 - key(live_id!(up)) as i8) as f64
            + self.pad.axis_z)
            .clamp(-1.0, 1.0);
        (axis_x, axis_z)
    }

    /// Buttons packed for the wire (bit order is protocol, keep it stable).
    pub fn buttons(&self) -> u32 {
        const ACTIONS: [LiveId; 8] = [
            live_id!(left),
            live_id!(right),
            live_id!(up),
            live_id!(down),
            live_id!(jump),
            live_id!(shoot),
            live_id!(grab),
            live_id!(reset),
        ];
        let mut bits = 0u32;
        for (i, action) in ACTIONS.iter().enumerate() {
            if self.held(*action) {
                bits |= 1 << i;
            }
            if self.pressed(*action) {
                bits |= 1 << (i + 16);
            }
        }
        bits
    }

    /// Rebuild held/pressed from wire bits. Analog axes arrive separately, so
    /// they land on the pad — a remote player's stick and a local one read the
    /// same way through [`Self::axes`].
    pub fn apply_wire(&mut self, buttons: u32, axis_x: f32, axis_z: f32, cam_yaw: f32) {
        const ACTIONS: [LiveId; 8] = [
            live_id!(left),
            live_id!(right),
            live_id!(up),
            live_id!(down),
            live_id!(jump),
            live_id!(shoot),
            live_id!(grab),
            live_id!(reset),
        ];
        self.held.clear();
        self.pressed.clear();
        for (i, action) in ACTIONS.iter().enumerate() {
            if buttons & (1 << i) != 0 {
                self.held.insert(*action);
            }
            if buttons & (1 << (i + 16)) != 0 {
                self.pressed.insert(*action);
            }
        }
        self.pad = PadState::default();
        self.pad.axis_x = axis_x as f64;
        self.pad.axis_z = axis_z as f64;
        self.cam_yaw = cam_yaw;
    }
}

/// Per-player HUD. The world's own `hud_slots`/`hud_bars`/`crosshair` remain
/// the "everyone sees this" layer; these are the private overlays (your lap,
/// your score). A single-player world leaves this empty, so what the renderer
/// composites is byte-for-byte what it composited before.
#[derive(Clone, Debug, Default)]
pub struct PlayerHud {
    pub slots: Vec<(String, HudSlot)>,
    pub bars: Vec<HudBar>,
    pub crosshair: Option<bool>,
}

impl PlayerHud {
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty() && self.bars.is_empty() && self.crosshair.is_none()
    }

    pub fn set_text(&mut self, slot: &str, text: String, template: HudSlot) {
        if text.is_empty() {
            self.slots.retain(|(name, _)| name != slot);
            return;
        }
        let mut value = template;
        value.text = text;
        if let Some(existing) = self.slots.iter_mut().find(|(name, _)| name == slot) {
            existing.1 = value;
        } else {
            self.slots.push((slot.to_string(), value));
        }
    }

    pub fn set_bar(&mut self, bar: HudBar) {
        // Negative fraction removes, matching the global game.bar contract.
        if bar.fraction < 0.0 {
            self.bars.retain(|b| b.name != bar.name);
            return;
        }
        if let Some(existing) = self.bars.iter_mut().find(|b| b.name == bar.name) {
            *existing = bar;
        } else {
            self.bars.push(bar);
        }
    }
}

/// A camera command addressed to one player. Clients apply it to their own rig;
/// the host applies player 0's to the world camera directly. Camera state is
/// **Local tier** — this is a command, not replicated state.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PlayerCamera {
    pub follow: u64,
    pub third: u64,
    pub chase: u64,
    pub distance: Option<f32>,
    pub height: Option<f32>,
    pub boom: Option<f32>,
    pub fov: Option<f32>,
    /// Bumped on every write so a client can tell a repeat from a fresh order.
    pub revision: u32,
}

#[derive(Clone, Debug)]
pub struct Player {
    pub id: PlayerId,
    pub name: String,
    pub source: PlayerSource,
    /// The entity this player drives (car, character…). 0 = not embodied yet.
    pub entity: u64,
    pub input: PlayerInput,
    pub camera: PlayerCamera,
    pub hud: PlayerHud,
    pub score: i64,
    /// Set when the player joined and cleared on leave; a slot lingers for one
    /// tick so `on_leave` can still read the name.
    pub connected: bool,
}

impl Player {
    pub fn new(id: PlayerId, name: impl Into<String>, source: PlayerSource) -> Self {
        Self {
            id,
            name: name.into(),
            source,
            entity: 0,
            input: PlayerInput::default(),
            camera: PlayerCamera::default(),
            hud: PlayerHud::default(),
            score: 0,
            connected: true,
        }
    }

    pub fn is_bot(&self) -> bool {
        self.source == PlayerSource::Bot
    }
}

/// The player roster. Always non-empty: slot 0 is this device.
#[derive(Clone, Debug)]
pub struct Players {
    slots: Vec<Player>,
    next_id: u32,
}

impl Default for Players {
    fn default() -> Self {
        Self {
            slots: vec![Player::new(PlayerId::LOCAL, "player", PlayerSource::Local)],
            next_id: 1,
        }
    }
}

impl Players {
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        false
    }

    pub fn local(&self) -> &Player {
        &self.slots[0]
    }

    pub fn local_mut(&mut self) -> &mut Player {
        &mut self.slots[0]
    }

    pub fn get(&self, id: PlayerId) -> Option<&Player> {
        self.slots.iter().find(|p| p.id == id)
    }

    pub fn get_mut(&mut self, id: PlayerId) -> Option<&mut Player> {
        self.slots.iter_mut().find(|p| p.id == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Player> {
        self.slots.iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Player> {
        self.slots.iter_mut()
    }

    pub fn ids(&self) -> Vec<PlayerId> {
        self.slots.iter().map(|p| p.id).collect()
    }

    /// Add a remote or bot player. Ids are never reused within a session, so a
    /// stale reference resolves to `None` instead of somebody else.
    pub fn add(&mut self, name: impl Into<String>, source: PlayerSource) -> PlayerId {
        let id = PlayerId(self.next_id);
        self.next_id += 1;
        self.slots.push(Player::new(id, name, source));
        id
    }

    /// Re-seat a player under an id the transport already assigned (rejoin
    /// keeps the same id, which is why the net layer resets its sequence
    /// windows — see the audit's rejoin finding).
    pub fn add_with_id(&mut self, id: PlayerId, name: impl Into<String>, source: PlayerSource) {
        self.next_id = self.next_id.max(id.0 + 1);
        if let Some(existing) = self.get_mut(id) {
            existing.name = name.into();
            existing.source = source;
            existing.connected = true;
            existing.input = PlayerInput::default();
            return;
        }
        self.slots.push(Player::new(id, name, source));
    }

    pub fn remove(&mut self, id: PlayerId) -> Option<Player> {
        if id.is_local_slot() {
            return None; // the local slot is structural
        }
        let index = self.slots.iter().position(|p| p.id == id)?;
        Some(self.slots.remove(index))
    }

    /// Everything that is not this device — who the host replicates to.
    pub fn remotes(&self) -> impl Iterator<Item = &Player> {
        self.slots.iter().filter(|p| p.source == PlayerSource::Remote)
    }

    pub fn bots(&self) -> impl Iterator<Item = &Player> {
        self.slots.iter().filter(|p| p.is_bot())
    }

    /// Which player drives this entity, if any.
    pub fn owner_of(&self, entity: u64) -> Option<PlayerId> {
        self.slots
            .iter()
            .find(|p| p.entity != 0 && p.entity == entity)
            .map(|p| p.id)
    }

    /// Drop entity references to entities that no longer exist, so a removed
    /// car does not leave a player pointing at a dead id.
    pub fn reconcile(&mut self, alive: impl Fn(u64) -> bool) {
        for player in self.slots.iter_mut() {
            if player.entity != 0 && !alive(player.entity) {
                player.entity = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_player_always_exists_and_cannot_be_removed() {
        let mut players = Players::default();
        assert_eq!(players.len(), 1);
        assert_eq!(players.local().id, PlayerId::LOCAL);
        assert!(players.remove(PlayerId::LOCAL).is_none());
        assert_eq!(players.len(), 1);
    }

    #[test]
    fn ids_are_not_reused_after_leave() {
        let mut players = Players::default();
        let a = players.add("a", PlayerSource::Remote);
        players.remove(a);
        let b = players.add("b", PlayerSource::Remote);
        assert_ne!(a, b);
        assert!(players.get(a).is_none());
    }

    #[test]
    fn rejoin_with_same_id_resets_input_but_keeps_the_slot() {
        let mut players = Players::default();
        let id = PlayerId(7);
        players.add_with_id(id, "kid", PlayerSource::Remote);
        players.get_mut(id).unwrap().input.held.insert(live_id!(jump));
        players.add_with_id(id, "kid", PlayerSource::Remote);
        assert_eq!(players.len(), 2);
        assert!(players.get(id).unwrap().input.held.is_empty());
    }

    #[test]
    fn wire_roundtrip_preserves_actions_and_axes() {
        let mut a = PlayerInput::default();
        a.held.insert(live_id!(jump));
        a.held.insert(live_id!(right));
        a.pressed.insert(live_id!(shoot));
        a.pad.axis_x = 0.5;
        a.cam_yaw = 1.25;

        let mut b = PlayerInput::default();
        b.apply_wire(a.buttons(), 0.5, 0.0, 1.25);
        assert!(b.held(live_id!(jump)));
        assert!(b.held(live_id!(right)));
        assert!(b.pressed(live_id!(shoot)));
        assert!(!b.held(live_id!(left)));
        assert_eq!(b.cam_yaw, 1.25);
        assert_eq!(b.axes().0, a.axes().0);
    }

    #[test]
    fn owner_lookup_and_reconcile() {
        let mut players = Players::default();
        let id = players.add("kid", PlayerSource::Remote);
        players.get_mut(id).unwrap().entity = 42;
        assert_eq!(players.owner_of(42), Some(id));
        players.reconcile(|e| e != 42);
        assert_eq!(players.owner_of(42), None);
    }

    #[test]
    fn hud_set_and_clear() {
        let mut hud = PlayerHud::default();
        assert!(hud.is_empty());
        hud.set_text("lap", "Lap 2/3".into(), HudSlot::default());
        assert_eq!(hud.slots.len(), 1);
        hud.set_text("lap", String::new(), HudSlot::default());
        assert!(hud.is_empty());
    }
}
