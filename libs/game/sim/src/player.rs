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
//! `cam_yaw` and `cam_pitch` travel *inside the input frame* — a client's
//! camera rig stays pure presentation, and the host resolves movement and aim
//! for everyone from the latest complete camera pose.

use makepad_live_id::*;
use std::collections::HashSet;

use crate::entity::{HudBar, HudSlot, PadState};

/// Actions carried in the wire button mask, in bit order: bit `i` is held,
/// bit `i + 16` is pressed-this-tick. **Bit order is protocol** — append,
/// never reorder. A v1 peer knows the first eight; punch/kick/guard joined
/// for the fighting archetype (mix.md §3.3) and claim bits 8-10 / 24-26.
/// Throw deliberately reuses `grab`, so it needs no bit of its own.
pub const WIRE_ACTIONS: [LiveId; 11] = [
    live_id!(left),
    live_id!(right),
    live_id!(up),
    live_id!(down),
    live_id!(jump),
    live_id!(shoot),
    live_id!(grab),
    live_id!(reset),
    live_id!(punch),
    live_id!(kick),
    live_id!(guard),
];

/// View preference presence and value. Two reserved HELD bits avoid treating
/// a legacy client's all-zero mask as an explicit third-person choice: bit 11
/// says this peer speaks the preference extension, bit 12 selects first person.
/// Neither has a pressed-edge twin because the current mode is persistent.
pub const WIRE_VIEW_PREFERENCE_BIT: u32 = 1 << 11;
pub const WIRE_FIRST_PERSON_BIT: u32 = 1 << 12;

// ── analog wire quantization ────────────────────────────────────────────
//
// The quantized value is the truth (mix.md §3.3): the sim only ever consumes
// floats derived from the u8/i16 that crossed the wire, through these exact
// functions, on host and client alike. A raw device float never reaches
// gameplay math, so the two ends cannot disagree by a rounding path.

/// 0..1 analog (trigger, pedal) to one wire byte. Non-finite input is a
/// hostile or broken device and reads as released.
pub fn quantize_unit(v: f32) -> u8 {
    if !v.is_finite() {
        return 0;
    }
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// The one inverse of [`quantize_unit`] — every consumer goes through here.
pub fn dequantize_unit(q: u8) -> f32 {
    q as f32 / 255.0
}

/// Signed −1..1 analog (the flight rudder, A1) to one wire byte on the SPARE
/// analog channel. Byte 0 is reserved to mean "axis absent" — exactly what a
/// v1 frame or a non-flight device sends — so silence decodes to a centred
/// rudder, never full deflection. 1..=255 maps −1..1 with 128 = centre.
pub fn quantize_rudder(v: f32) -> u8 {
    if !v.is_finite() {
        return 128;
    }
    let t = (v.clamp(-1.0, 1.0) + 1.0) * 0.5;
    1 + (t * 254.0).round() as u8
}

/// The one inverse of [`quantize_rudder`].
pub fn dequantize_rudder(q: u8) -> f32 {
    if q == 0 {
        return 0.0;
    }
    (q - 1) as f32 / 254.0 * 2.0 - 1.0
}

/// Radians per LSB of the wire look delta: ±16 rad of range per tick and
/// ~0.5 mrad of resolution — finer than any mouse moves in one 60 Hz tick.
pub const LOOK_QUANTUM: f32 = 1.0 / 2048.0;

/// Look delta (radians this tick) to the wire. Non-finite reads as no motion.
pub fn quantize_look(v: f32) -> i16 {
    if !v.is_finite() {
        return 0;
    }
    (v / LOOK_QUANTUM)
        .round()
        .clamp(i16::MIN as f32, i16::MAX as f32) as i16
}

pub fn dequantize_look(q: i16) -> f32 {
    q as f32 * LOOK_QUANTUM
}

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
    /// This player's absolute camera angles in renderer/view convention,
    /// carried in every input packet. Movement resolves against yaw; gameplay
    /// aim resolves against both. Repeating the pose makes a packet after loss
    /// self-healing instead of requiring every intervening look delta.
    pub cam_yaw: f32,
    pub cam_pitch: f32,
    pub look_dx: f64,
    pub look_dy: f64,
    /// This device wants its on-foot/cockpit camera in first person. The
    /// camera itself stays local; the host consumes this bit only to keep the
    /// player's authoritative aim-facing and fire ray on the same rig.
    pub first_person: bool,
    /// True after a device/input frame has actually stated its preference.
    /// This distinguishes an explicit third-person bit-clear from a freshly
    /// joined player's default-zero input, where authored game presentation
    /// should remain in force until their first frame arrives.
    pub view_preference_received: bool,
    /// Analog wire axes `[throttle, brake, handbrake, spare]`, stored exactly
    /// as quantized for the wire (mix.md §3.3). The byte is the truth: every
    /// consumer derives its float through [`dequantize_unit`], so the host
    /// applies precisely the number the client's device layer committed to.
    pub analog: [u8; 4],
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
            x if x == live_id!(punch) => self.pad.punch,
            x if x == live_id!(kick) => self.pad.kick,
            x if x == live_id!(guard) => self.pad.guard,
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
            x if x == live_id!(punch) => self.pad.punch_pressed,
            x if x == live_id!(kick) => self.pad.kick_pressed,
            x if x == live_id!(guard) => self.pad.guard_pressed,
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

    /// Buttons packed for the wire ([`WIRE_ACTIONS`] bit order is protocol,
    /// keep it stable).
    pub fn buttons(&self) -> u32 {
        let mut bits = 0u32;
        for (i, action) in WIRE_ACTIONS.iter().enumerate() {
            if self.held(*action) {
                bits |= 1 << i;
            }
            if self.pressed(*action) {
                bits |= 1 << (i + 16);
            }
        }
        if self.view_preference_received {
            bits |= WIRE_VIEW_PREFERENCE_BIT;
            if self.first_person {
                bits |= WIRE_FIRST_PERSON_BIT;
            }
        }
        bits
    }

    /// Device-side entry for analog triggers/pedals: quantized immediately,
    /// because only the quantized value may ever influence the sim.
    pub fn set_analog(&mut self, throttle: f32, brake: f32, handbrake: f32, spare: f32) {
        self.analog = [
            quantize_unit(throttle),
            quantize_unit(brake),
            quantize_unit(handbrake),
            quantize_unit(spare),
        ];
    }

    /// Device-side entry for the flight rudder (A1): the spare analog byte
    /// carries a signed −1..1 axis through its own quantizer. Call after
    /// [`Self::set_analog`], which writes the raw spare byte.
    pub fn set_rudder(&mut self, yaw: f32) {
        self.analog[3] = quantize_rudder(yaw);
    }

    /// Rebuild held/pressed from v1 wire bits. A v1 frame carries no analog
    /// axes or look deltas, so those read as released/still — which is exactly
    /// what a v1 client meant.
    pub fn apply_wire(&mut self, buttons: u32, axis_x: f32, axis_z: f32, cam_yaw: f32) {
        self.apply_wire_v2(buttons, axis_x, axis_z, cam_yaw, 0.0, [0; 4], 0, 0);
    }

    /// Rebuild this input from a v2 wire frame. Stick axes arrive separately
    /// from buttons, so they land on the pad — a remote player's stick and a
    /// local one read the same way through [`Self::axes`]. Analog axes stay
    /// quantized ([`Self::analog`] is the truth); absolute yaw/pitch are copied
    /// exactly, while look deltas are dequantized here through the one
    /// canonical function so host and client derive the identical float.
    pub fn apply_wire_v2(
        &mut self,
        buttons: u32,
        axis_x: f32,
        axis_z: f32,
        cam_yaw: f32,
        cam_pitch: f32,
        analog: [u8; 4],
        look_dx: i16,
        look_dy: i16,
    ) {
        self.held.clear();
        self.pressed.clear();
        for (i, action) in WIRE_ACTIONS.iter().enumerate() {
            if buttons & (1 << i) != 0 {
                self.held.insert(*action);
            }
            if buttons & (1 << (i + 16)) != 0 {
                self.pressed.insert(*action);
            }
        }
        // An older peer sends neither reserved bit. Preserve the game's
        // authored/default presentation until a preference-aware frame says
        // otherwise; zero must never mean an implicit third-person override.
        if buttons & WIRE_VIEW_PREFERENCE_BIT != 0 {
            self.first_person = buttons & WIRE_FIRST_PERSON_BIT != 0;
            self.view_preference_received = true;
        }
        self.pad = PadState::default();
        self.pad.axis_x = axis_x as f64;
        self.pad.axis_z = axis_z as f64;
        self.cam_yaw = cam_yaw;
        self.cam_pitch = cam_pitch;
        self.analog = analog;
        self.look_dx = dequantize_look(look_dx) as f64;
        self.look_dy = dequantize_look(look_dy) as f64;
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
        a.cam_pitch = -0.4;
        a.first_person = true;
        a.view_preference_received = true;

        let mut b = PlayerInput::default();
        b.apply_wire(a.buttons(), 0.5, 0.0, 1.25);
        assert!(b.held(live_id!(jump)));
        assert!(b.held(live_id!(right)));
        assert!(b.pressed(live_id!(shoot)));
        assert!(!b.held(live_id!(left)));
        assert_eq!(b.cam_yaw, 1.25);
        // Legacy payloads have no absolute pitch and therefore level it.
        assert_eq!(b.cam_pitch, 0.0);
        assert_eq!(b.axes().0, a.axes().0);
        assert!(b.first_person);
        assert!(b.view_preference_received);
        assert_eq!(
            a.buttons() & WIRE_VIEW_PREFERENCE_BIT,
            WIRE_VIEW_PREFERENCE_BIT
        );
        assert_eq!(a.buttons() & WIRE_FIRST_PERSON_BIT, WIRE_FIRST_PERSON_BIT);

        b.apply_wire(WIRE_VIEW_PREFERENCE_BIT, 0.0, 0.0, 0.0);
        assert!(!b.first_person, "a later third-person frame clears the preference");
        assert!(b.view_preference_received);
    }

    #[test]
    fn legacy_zero_mask_does_not_invent_a_third_person_preference() {
        let mut input = PlayerInput::default();
        input.first_person = true; // authored FPS presentation, not wire intent
        input.apply_wire(0, 0.0, 0.0, 0.0);
        assert!(input.first_person, "legacy input must preserve authored view");
        assert!(!input.view_preference_received);
    }

    #[test]
    fn wire_roundtrip_preserves_fighting_actions() {
        let mut a = PlayerInput::default();
        a.held.insert(live_id!(punch));
        a.held.insert(live_id!(guard));
        a.pressed.insert(live_id!(kick));
        a.pressed.insert(live_id!(punch));

        let mut b = PlayerInput::default();
        b.apply_wire_v2(a.buttons(), 0.0, 0.0, 0.0, -0.35, [0; 4], 0, 0);
        assert!(b.held(live_id!(punch)));
        assert!(b.held(live_id!(guard)));
        assert!(!b.held(live_id!(kick)));
        assert!(b.pressed(live_id!(kick)));
        assert!(b.pressed(live_id!(punch)));
        assert!(!b.pressed(live_id!(guard)));
        assert_eq!(b.cam_pitch, -0.35);
        // The new bits sit exactly where the layout promises: held 8-10,
        // pressed 24-26. A v1 peer never sets them, so they read released.
        assert_eq!(a.buttons() & 0x0000_ff00, 0b0101 << 8);
        assert_eq!(a.buttons() & 0xff00_0000, 0b0011 << 24);
    }

    #[test]
    fn v1_wire_frames_clear_v2_analog_state() {
        let mut input = PlayerInput::default();
        input.apply_wire_v2(
            0,
            0.0,
            0.0,
            0.0,
            -0.6,
            [255, 128, 64, 32],
            100,
            -100,
        );
        assert_eq!(input.analog, [255, 128, 64, 32]);
        assert!(input.look_dx > 0.0 && input.look_dy < 0.0);
        assert_eq!(input.cam_pitch, -0.6);

        // A v1 frame from an old peer means "no analog, no look" — stale v2
        // state must not linger under it.
        input.apply_wire(1 << 4, 0.25, -0.5, 0.75);
        assert_eq!(input.analog, [0; 4]);
        assert_eq!(input.look_dx, 0.0);
        assert_eq!(input.look_dy, 0.0);
        assert_eq!(input.cam_pitch, 0.0);
        assert!(input.held(live_id!(jump)));
        assert_eq!(input.cam_yaw, 0.75);
    }

    #[test]
    fn quantization_clamps_hostile_values_and_is_exact_at_endpoints() {
        assert_eq!(quantize_unit(0.0), 0);
        assert_eq!(quantize_unit(1.0), 255);
        assert_eq!(dequantize_unit(0), 0.0);
        assert_eq!(dequantize_unit(255), 1.0);
        // Out-of-range and non-finite device floats can never mint an
        // out-of-range wire value.
        assert_eq!(quantize_unit(-5.0), 0);
        assert_eq!(quantize_unit(7.0), 255);
        // Non-finite is a broken or hostile device, not a pressed trigger:
        // both NaN and infinity read as released.
        assert_eq!(quantize_unit(f32::NAN), 0);
        assert_eq!(quantize_unit(f32::INFINITY), 0);
        assert_eq!(quantize_unit(f32::NEG_INFINITY), 0);

        assert_eq!(quantize_look(0.0), 0);
        assert_eq!(quantize_look(f32::NAN), 0);
        assert_eq!(quantize_look(1.0e9), i16::MAX);
        assert_eq!(quantize_look(-1.0e9), i16::MIN);
        // Round trip within one quantum across the useful range.
        for v in [-3.0f32, -0.31, 0.0, 0.0007, 2.5] {
            let back = dequantize_look(quantize_look(v));
            assert!((back - v).abs() <= LOOK_QUANTUM, "{v} -> {back}");
        }
    }

    #[test]
    fn set_analog_quantizes_at_the_device_edge() {
        let mut input = PlayerInput::default();
        input.set_analog(0.5, 1.0, f32::NAN, -1.0);
        assert_eq!(input.analog, [128, 255, 0, 0]);
    }

    #[test]
    fn rudder_byte_zero_means_absent_never_full_deflection() {
        // The trap this encoding exists to avoid: a v1 frame (or any device
        // that never touched the rudder) zeroes the analog block, and a naive
        // signed mapping would read that as FULL LEFT RUDDER on every old
        // client. Byte 0 must decode to centred.
        assert_eq!(dequantize_rudder(0), 0.0);
        // Endpoints and centre are exact.
        assert_eq!(quantize_rudder(-1.0), 1);
        assert_eq!(quantize_rudder(1.0), 255);
        assert_eq!(quantize_rudder(0.0), 128);
        assert_eq!(dequantize_rudder(128), 0.0);
        assert_eq!(dequantize_rudder(1), -1.0);
        assert_eq!(dequantize_rudder(255), 1.0);
        // Hostile floats centre rather than deflect.
        assert_eq!(quantize_rudder(f32::NAN), 128);
        assert_eq!(quantize_rudder(9.0), 255);
        assert_eq!(quantize_rudder(-9.0), 1);
        // Round trip within one quantum.
        for v in [-0.85f32, -0.2, 0.33, 0.99] {
            let back = dequantize_rudder(quantize_rudder(v));
            assert!((back - v).abs() < 1.5 / 254.0, "{v} -> {back}");
        }
        // The device edge writes the spare byte through the rudder quantizer.
        let mut input = PlayerInput::default();
        input.set_analog(0.5, 0.0, 0.0, 0.0);
        input.set_rudder(-0.5);
        assert_eq!(input.analog[3], quantize_rudder(-0.5));
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
