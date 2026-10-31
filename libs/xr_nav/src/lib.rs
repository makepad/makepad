//! XR navigation for apps that show a world in a box standing in the room: the box
//! ([`ClipBox`]), where the world sits in it (`room = R_y(yaw) (scale * world) + origin`,
//! [`XrNavView`]) and how the headset's hands move both. One layer for every such app (the
//! Sandbox games' race box, Scope's code map), so the controls are the same everywhere.
//!
//! Inputs are two grabbers, one per hand: a controller's grip, or a tracked hand's middle pinch
//! while that hand holds no controller. Each has a position and a 2D stick (the controller's
//! stick, pushed up is +y; a pinching hand's drag from where it closed, [`PINCH_DRAG_FULL_M`]
//! being full deflection).
//! - Both grabbers: grab the box: move it, turn it about the vertical and scale it, the world
//!   with it, relative to when both closed (no jump on a press or a release).
//! - One grabber: its stick zooms (y, up is in; the box stays, the world scales about the box's
//!   anchor, exponential, eased) and turns the view about the anchor (x). A pinching hand's drag
//!   in [`XrNavMode::Free`] flies instead: across and along the floor it moves the world through
//!   the box (head-relative), up and down it zooms.
//! - [`XrNavMode::Follow`] (a game: the thing played stays at the box's anchor and the world
//!   turns with the game camera's heading, critically damped, the stick's turn on top): the app
//!   calls [`XrNav::follow`] each drawn frame.
//! - [`XrNavMode::Free`] (a map: nothing is followed): a controller's left stick held without
//!   its grip flies the world through the box, head-relative, at a rate in box sizes per second
//!   whatever the scale; its right stick's x turns the view.
//! - Everything else is [`XrNav::gamepad`]: the sticks, triggers and buttons of the hands not
//!   grabbing (a hand holding the box presses nothing), with press counts, for the app's own
//!   pad bindings.
use makepad_platform::event::game_input::{gamepad_button as gb, GamepadState};
use makepad_platform::event::xr::{XrController, XrHand, XrState};
use makepad_platform::makepad_math::{vec2f, vec3f, Mat4f, Vec2f, Vec3f};

/// The box a world shows in, in the room (tracking space): its centre, its turn about the
/// room's up axis and its half extents (metres).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipBox {
    pub center: Vec3f,
    pub yaw: f32,
    pub half: Vec3f,
}

impl ClipBox {
    /// Room -> the unit box [-1, 1]^3 (column-major).
    pub fn unit_from_room(&self) -> Mat4f {
        let (s, c) = (self.yaw.sin(), self.yaw.cos());
        let inv = vec3f(1.0 / self.half.x.max(1.0e-4), 1.0 / self.half.y.max(1.0e-4), 1.0 / self.half.z.max(1.0e-4));
        // local = R_y(-yaw) (room - center) / half: x' = c x - s z, z' = s x + c z
        let mut m = Mat4f::identity();
        m.v[0] = c * inv.x;
        m.v[2] = s * inv.z;
        m.v[5] = inv.y;
        m.v[8] = -s * inv.x;
        m.v[10] = c * inv.z;
        let p = self.center;
        m.v[12] = -(c * p.x - s * p.z) * inv.x;
        m.v[13] = -p.y * inv.y;
        m.v[14] = -(s * p.x + c * p.z) * inv.z;
        m
    }

    /// A point given in the box's own frame (metres from its centre, unturned) in the room.
    pub fn room_point(&self, local: Vec3f) -> Vec3f {
        let (s, c) = (self.yaw.sin(), self.yaw.cos());
        self.center + vec3f(local.x * c + local.z * s, local.y, -local.x * s + local.z * c)
    }
}

/// A pinching hand's drag that counts as a fully deflected stick (metres).
pub const PINCH_DRAG_FULL_M: f32 = 0.12;
/// Stick travel under this is no input; past it the travel is rescaled from 0.
pub const STICK_DEAD_ZONE: f32 = 0.15;

/// How the world moves in the box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XrNavMode {
    /// A subject stays at the box's anchor, the world turned with a followed heading.
    Follow,
    /// The world stays where the hands put it; sticks and pinch drags fly it through the box.
    Free,
}

/// The ranges and rates of one app's box.
#[derive(Clone, Copy, Debug)]
pub struct XrNavConfig {
    pub mode: XrNavMode,
    /// Where a session starts.
    pub box_start: ClipBox,
    /// World units to room metres at the start, and the range the gestures keep it in.
    pub scale_start: f32,
    pub scale_min: f32,
    pub scale_max: f32,
    /// The box's half extents' range (metres).
    pub half_min: f32,
    pub half_max: f32,
    /// The anchor (the followed subject, the zoom's and turn's centre) in the box's half
    /// extents.
    pub anchor: Vec3f,
    /// The zoom stick's rate (e-folds per second at full deflection), the turn stick's
    /// (rad/s), the fly's (box half widths per second) and how fast what is shown follows
    /// them (1/s).
    pub zoom_rate: f32,
    pub turn_rate: f32,
    pub fly_rate: f32,
    pub ease: f32,
    /// The heading follow's natural frequency (rad/s), critically damped.
    pub heading_omega: f32,
}

impl XrNavConfig {
    /// A game's race box: 1 m x 0.6 m x 1 m on a table top, 80 m of the world across, the
    /// subject a quarter down from the centre.
    pub const FOLLOW: XrNavConfig = XrNavConfig {
        mode: XrNavMode::Follow,
        box_start: ClipBox { center: Vec3f { x: 0.0, y: 0.85, z: -0.9 }, yaw: 0.0, half: Vec3f { x: 0.5, y: 0.3, z: 0.5 } },
        scale_start: 1.0 / 80.0,
        scale_min: 1.0 / 4000.0,
        scale_max: 2.0,
        half_min: 0.06,
        half_max: 6.0,
        anchor: Vec3f { x: 0.0, y: -0.5, z: 0.0 },
        zoom_rate: 1.2,
        turn_rate: 1.6,
        fly_rate: 0.8,
        ease: 10.0,
        heading_omega: 6.0,
    };
}

/// Where the world is in the room this frame: `room = R_y(yaw) (scale * world) + origin`,
/// clipped to `clip`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct XrNavView {
    pub clip: ClipBox,
    pub origin: Vec3f,
    pub yaw: f32,
    pub scale: f32,
}

impl XrNavView {
    /// A world point in the room.
    pub fn room_from_world(&self, w: Vec3f) -> Vec3f {
        let (s, c) = (self.yaw.sin(), self.yaw.cos());
        let k = self.scale;
        self.origin + vec3f((w.x * c + w.z * s) * k, w.y * k, (-w.x * s + w.z * c) * k)
    }

    /// A room point in the world.
    pub fn world_from_room(&self, r: Vec3f) -> Vec3f {
        let (s, c) = (self.yaw.sin(), self.yaw.cos());
        let d = (r - self.origin) * (1.0 / self.scale.max(1.0e-12));
        vec3f(d.x * c - d.z * s, d.y, d.x * s + d.z * c)
    }

    /// World -> room as one column-major matrix.
    pub fn matrix(&self) -> Mat4f {
        let (s, c) = (self.yaw.sin(), self.yaw.cos());
        let k = self.scale;
        let mut m = Mat4f::identity();
        m.v[0] = c * k;
        m.v[2] = -s * k;
        m.v[5] = k;
        m.v[8] = s * k;
        m.v[10] = c * k;
        m.v[12] = self.origin.x;
        m.v[13] = self.origin.y;
        m.v[14] = self.origin.z;
        m
    }
}

/// A stick value past the dead zone, rescaled so the first live degree is gentle.
pub fn dead_zoned(v: f32) -> f32 {
    if v.abs() < STICK_DEAD_ZONE {
        0.0
    } else {
        v.signum() * ((v.abs() - STICK_DEAD_ZONE) / (1.0 - STICK_DEAD_ZONE)).min(1.0)
    }
}

/// A controller's stick in the gamepad's convention (pushed up is +y; the platform's XR stick
/// has up at -y), dead-zoned. Zero for a controller not held.
pub fn pad_stick(c: &XrController) -> Vec2f {
    if !c.active() {
        return Vec2f::default();
    }
    vec2f(dead_zoned(c.stick.x), -dead_zoned(c.stick.y))
}

/// A controller's grip (or a tracked hand's middle pinch while no controller is held): where.
pub fn grip(c: &XrController, h: &XrHand) -> Option<Vec3f> {
    if c.active() {
        return (c.grip >= 0.5).then_some(c.grip_pose.position);
    }
    h.pinch_middle().then(|| h.tracking_pose().map(|p| p.position)).flatten()
}

/// The angle in (-pi, pi].
pub fn wrap(a: f32) -> f32 {
    let t = std::f32::consts::TAU;
    let mut d = a % t;
    if d > std::f32::consts::PI {
        d -= t;
    }
    if d <= -std::f32::consts::PI {
        d += t;
    }
    d
}

/// Counts a gamepad's presses (what the platform keeps for a tap shorter than a frame): feed
/// each frame's pad, its `presses` are filled in.
#[derive(Clone, Debug, Default)]
pub struct PadPresses {
    presses: [u16; gb::COUNT],
    down: u32,
}

impl PadPresses {
    pub fn count(&mut self, pad: &mut GamepadState) {
        let mut down = 0u32;
        for (i, v) in [(gb::A, pad.a), (gb::B, pad.b), (gb::X, pad.x), (gb::Y, pad.y), (gb::LEFT_SHOULDER, pad.left_shoulder), (gb::RIGHT_SHOULDER, pad.right_shoulder), (gb::START, pad.start), (gb::LEFT_THUMB, pad.left_thumb), (gb::RIGHT_THUMB, pad.right_thumb)] {
            if v > 0.5 {
                down |= 1 << i;
                if self.down & (1 << i) == 0 {
                    self.presses[i] = self.presses[i].wrapping_add(1);
                }
            }
        }
        self.down = down;
        pad.presses = self.presses;
    }
}

/// One headset's box and the world in it.
pub struct XrNav {
    pub config: XrNavConfig,
    pub clip: ClipBox,
    /// The scale shown, and the zoom's target it eases to.
    pub scale: f32,
    zoom_target: f32,
    /// The view turn shown (radians), and its target. Follow: on top of the heading follow.
    turn: f32,
    turn_target: f32,
    /// Free: the world's place (`room = R_y(yaw) (scale * world) + origin`).
    origin: Vec3f,
    yaw: f32,
    /// Follow: the followed heading (a heading h faces (-sin h, -cos h)) and its rate.
    heading: Option<f32>,
    heading_vel: f32,
    heading_at: Option<f64>,
    /// Both grabbers: the box, scale, turn and origin when they closed and the hands then.
    grab: Option<(ClipBox, f32, f32, Vec3f, [Vec3f; 2])>, // (box, scale, world yaw, origin, hands)
    /// A pinching hand's position when its pinch closed (its drag is the stick).
    pinch_from: [Option<Vec3f>; 2],
    grips: [Option<Vec3f>; 2],
    /// The stick the box took this frame (one grabber's), for logs.
    pub nav_stick: Vec2f,
    pad: GamepadState,
    presses: PadPresses,
}

impl XrNav {
    pub fn new(config: XrNavConfig) -> XrNav {
        let scale = config.scale_start.clamp(config.scale_min, config.scale_max);
        let mut nav = XrNav {
            config,
            clip: config.box_start,
            scale,
            zoom_target: scale,
            turn: 0.0,
            turn_target: 0.0,
            origin: Vec3f::default(),
            yaw: 0.0,
            heading: None,
            heading_vel: 0.0,
            heading_at: None,
            grab: None,
            pinch_from: [None; 2],
            grips: [None; 2],
            nav_stick: Vec2f::default(),
            pad: GamepadState::default(),
            presses: PadPresses::default(),
        };
        nav.origin = nav.anchor();
        nav
    }

    /// Free: put the world point `world` at the box's anchor.
    pub fn center_on(&mut self, world: Vec3f) {
        let v = XrNavView { clip: self.clip, origin: Vec3f::default(), yaw: self.yaw, scale: self.scale };
        self.origin = self.anchor() - v.room_from_world(world);
    }

    /// Free: show the world at `scale` (clamped to the range), the world point at the anchor
    /// staying there; any zoom in progress ends there.
    pub fn set_scale(&mut self, scale: f32) {
        let scale = scale.clamp(self.config.scale_min, self.config.scale_max);
        let a = self.anchor();
        let at = self.view().world_from_room(a);
        self.scale = scale;
        self.zoom_target = scale;
        self.center_on(at);
    }

    /// The anchor in the room.
    pub fn anchor(&self) -> Vec3f {
        let a = self.config.anchor;
        self.clip.room_point(vec3f(a.x * self.clip.half.x, a.y * self.clip.half.y, a.z * self.clip.half.z))
    }

    /// Which hands grab (left, right) this frame.
    pub fn grips(&self) -> [bool; 2] {
        [self.grips[0].is_some(), self.grips[1].is_some()]
    }

    /// The controls the box left (the hands not grabbing), with press counts.
    pub fn gamepad(&self) -> &GamepadState {
        &self.pad
    }

    /// One headset frame (`dt` seconds since the last, the caller's clock): the grabbers, the
    /// sticks, the fly and the gamepad.
    pub fn update(&mut self, state: &XrState, dt: f32) {
        let dt = dt.clamp(0.0, 0.1);
        let (l, r) = (&state.left_controller, &state.right_controller);
        let hands = [(l, &state.left_hand), (r, &state.right_hand)];
        let grips = [grip(l, &state.left_hand), grip(r, &state.right_hand)];
        self.grips = grips;
        self.hold(grips);
        // a pinching hand's drag (a hand with no controller), head-relative
        let head = state.head_pose.orientation;
        let fwd = head.rotate_vec3(&vec3f(0.0, 0.0, -1.0));
        let flat_fwd = {
            let f = vec3f(fwd.x, 0.0, fwd.z);
            let n = f.length();
            if n > 1.0e-4 { f * (1.0 / n) } else { vec3f(0.0, 0.0, -1.0) }
        };
        let flat_right = vec3f(-flat_fwd.z, 0.0, flat_fwd.x);
        let mut drag = [None; 2];
        for i in 0..2 {
            let (c, _) = hands[i];
            match (grips[i], c.active()) {
                (Some(p), false) => {
                    let from = *self.pinch_from[i].get_or_insert(p);
                    let d = (p - from) * (1.0 / PINCH_DRAG_FULL_M);
                    let clamp = |v: f32| dead_zoned(v.clamp(-1.0, 1.0));
                    drag[i] = Some(vec3f(clamp(d.dot(flat_right)), clamp(d.y), clamp(d.dot(flat_fwd))));
                }
                _ => self.pinch_from[i] = None,
            }
        }
        let (ls, rs) = (pad_stick(l), pad_stick(r));
        let sticks = [ls, rs];
        // one grabber: that hand's stick (or its pinch drag) is the box's
        let one = match grips {
            [Some(_), None] => Some(0),
            [None, Some(_)] => Some(1),
            _ => None,
        };
        let (mut zoom, mut turn, mut fly) = (0.0, 0.0, Vec2f::default());
        self.nav_stick = Vec2f::default();
        if let Some(i) = one {
            match drag[i] {
                Some(d) if self.config.mode == XrNavMode::Free => {
                    zoom = d.y;
                    fly = vec2f(d.x, d.z);
                    self.nav_stick = vec2f(d.x, d.z);
                }
                Some(d) => {
                    zoom = d.y;
                    turn = d.x;
                    self.nav_stick = vec2f(d.x, d.y);
                }
                None => {
                    zoom = sticks[i].y;
                    turn = sticks[i].x;
                    self.nav_stick = sticks[i];
                }
            }
        }
        // Free: the free sticks fly (left) and turn (right)
        let free = self.config.mode == XrNavMode::Free;
        if free && grips[0].is_none() && l.active() {
            fly = fly + ls;
        }
        if free && grips[1].is_none() && r.active() {
            turn += rs.x;
        }
        self.sticks(zoom, turn, dt);
        if free && self.grab.is_none() && (fly.x != 0.0 || fly.y != 0.0) {
            // the world flows past: a stick pushed forward moves the view forward, so the
            // world moves back toward the head
            let rate = self.config.fly_rate * self.clip.half.x.max(self.clip.half.z) * dt;
            let d = flat_right * (fly.x * rate) + flat_fwd * (fly.y * rate);
            self.origin = self.origin - d;
        }
        // the gamepad: the hands not grabbing
        let (lf, rf) = (grips[0].is_none() && l.active(), grips[1].is_none() && r.active());
        let on = |c: &XrController, free: bool, bit: u16| free && c.buttons & bit != 0;
        let f = |v: bool| if v { 1.0 } else { 0.0 };
        let mut pad = GamepadState::default();
        if lf && !free {
            pad.left_stick.x = ls.x as _;
            pad.left_stick.y = ls.y as _;
        }
        if rf && !free {
            pad.right_stick.x = rs.x as _;
            pad.right_stick.y = rs.y as _;
        }
        pad.left_trigger = if lf { l.trigger } else { 0.0 };
        pad.right_trigger = if rf { r.trigger } else { 0.0 };
        pad.a = f(on(r, rf, XrController::CLICK_A));
        pad.b = f(on(r, rf, XrController::CLICK_B));
        pad.x = f(on(l, lf, XrController::CLICK_X));
        pad.y = f(on(l, lf, XrController::CLICK_Y));
        pad.start = f(on(l, lf, XrController::CLICK_MENU));
        pad.left_thumb = f(on(l, l.active(), XrController::CLICK_THUMBSTICK));
        pad.right_thumb = f(on(r, r.active(), XrController::CLICK_THUMBSTICK));
        self.presses.count(&mut pad);
        self.pad = pad;
    }

    /// Both grabbers: move, turn (about the vertical) and scale the box and the world with it.
    fn hold(&mut self, hands: [Option<Vec3f>; 2]) {
        let [Some(l), Some(r)] = hands else {
            self.grab = None;
            return;
        };
        let Some((start, scale0, yaw0, origin0, [l0, r0])) = self.grab else {
            self.grab = Some((self.clip, self.scale, self.yaw, self.origin, [l, r]));
            return;
        };
        let cfg = self.config;
        let (mid0, mid) = ((l0 + r0) * 0.5, (l + r) * 0.5);
        let (v0, v) = (r0 - l0, r - l);
        let (len0, len) = (v0.length(), v.length());
        if len0 < 0.02 || len < 0.02 {
            return;
        }
        // the scale, kept inside both ranges
        let lo = (cfg.scale_min / scale0).max(cfg.half_min / start.half.x.min(start.half.y).min(start.half.z));
        let hi = (cfg.scale_max / scale0).min(cfg.half_max / start.half.x.max(start.half.y).max(start.half.z));
        let k = (len / len0).clamp(lo.min(1.0), hi.max(1.0));
        // the turn about the vertical: R_y(turn) takes v0's floor direction to v's
        let turn = -(v.z.atan2(v.x) - v0.z.atan2(v0.x));
        let (s, c) = (turn.sin(), turn.cos());
        let moved = |p: Vec3f| {
            let d = (p - mid0) * k;
            mid + vec3f(d.x * c + d.z * s, d.y, -d.x * s + d.z * c)
        };
        self.clip.center = moved(start.center);
        self.clip.yaw = start.yaw + turn;
        self.clip.half = start.half * k;
        self.scale = scale0 * k;
        self.zoom_target = self.scale;
        if self.config.mode == XrNavMode::Free {
            // the world rides with the box
            self.origin = moved(origin0);
            self.yaw = yaw0 + turn;
        }
    }

    /// The box's stick: zoom (y, up is in) and the view turn (x); what is shown eases there,
    /// about the anchor.
    fn sticks(&mut self, zoom: f32, turn: f32, dt: f32) {
        let cfg = self.config;
        self.zoom_target = (self.zoom_target * (zoom * cfg.zoom_rate * dt).exp()).clamp(cfg.scale_min, cfg.scale_max);
        self.turn_target += turn * cfg.turn_rate * dt;
        let ease = 1.0 - (-cfg.ease * dt).exp();
        let (scale0, turn0) = (self.scale, self.turn);
        if self.grab.is_none() {
            self.scale = (self.scale.ln() + (self.zoom_target.ln() - self.scale.ln()) * ease).exp();
        }
        self.turn += (self.turn_target - self.turn) * ease;
        if cfg.mode == XrNavMode::Free && self.grab.is_none() {
            // about the anchor: the world point there stays there (a turn to the right turns
            // the world the way Follow's does)
            let a = self.anchor();
            let dyaw = -(self.turn - turn0);
            self.yaw = wrap(self.yaw + dyaw);
            let (s, c) = (dyaw.sin(), dyaw.cos());
            let d = (self.origin - a) * (self.scale / scale0);
            self.origin = a + vec3f(d.x * c + d.z * s, d.y, -d.x * s + d.z * c);
        }
    }

    /// Follow, each drawn frame: the view that keeps the world point `pos` at the anchor, the
    /// world turned so `target_heading` (critically damped, the stick's turn on top) points into
    /// the box. `time`: the frame's time (seconds).
    pub fn follow(&mut self, pos: Vec3f, target_heading: f32, time: f64) -> XrNavView {
        let dt = self.heading_at.map_or(0.0, |t| (time - t).clamp(0.0, 0.1)) as f32;
        self.heading_at = Some(time);
        let heading = self.heading.get_or_insert(target_heading);
        let err = wrap(target_heading - *heading);
        if err.abs() > 2.8 {
            // a respawn or a camera cut: no long swing round
            *heading = target_heading;
            self.heading_vel = 0.0;
        } else {
            let w = self.config.heading_omega;
            self.heading_vel += (w * w * err - 2.0 * w * self.heading_vel) * dt;
            *heading = wrap(*heading + self.heading_vel * dt);
        }
        // a heading h's forward (-sin h, -cos h) turned by `yaw` is the box's forward
        // (-sin b, -cos b) when yaw = b - h
        let yaw = self.clip.yaw - (self.heading.unwrap_or(0.0) + self.turn);
        let v = XrNavView { clip: self.clip, origin: Vec3f::default(), yaw, scale: self.scale };
        XrNavView { origin: self.anchor() - v.room_from_world(pos), ..v }
    }

    /// The followed heading and the view turn on top of it (Follow), for logs.
    pub fn heading_turn(&self) -> (Option<f32>, f32) {
        (self.heading, self.turn)
    }

    /// Free: the world's place in the room now.
    pub fn view(&self) -> XrNavView {
        XrNavView { clip: self.clip, origin: self.origin, yaw: self.yaw, scale: self.scale }
    }
}

/// Synthetic controllers for a test without a headset's (the XR remote selftest supplying the
/// head), `t` seconds since the first synthetic frame: both held, the right trigger at 0.7, the
/// left stick steering a slow slalom, A pressed once at 20 s, both grips closed from 30 s to
/// 33 s while the controllers part (the box grows by half), the right grip with the right stick
/// up from 36 s to 38 s (zoom in ~11x; the platform's XR stick: up is -y) and then right from
/// 40 s to 41 s (the view turns ~1.6 rad), the left stick pushed up from 44 s to 47 s and the
/// right stick right from 48 s to 50 s.
pub fn synth_controllers(state: &mut XrState, t: f32) {
    let (l, r) = (&mut state.left_controller, &mut state.right_controller);
    for c in [&mut *l, &mut *r] {
        c.buttons = XrController::ACTIVE;
        c.grip = 0.0;
        c.trigger = 0.0;
        c.stick = Vec2f::default();
    }
    r.trigger = 0.7;
    l.stick.x = (t * 0.35).sin() * 0.5;
    if (20.0..20.2).contains(&t) {
        r.buttons |= XrController::CLICK_A;
    }
    let apart = 0.3 + ((t - 30.0) / 3.0).clamp(0.0, 1.0) * 0.15;
    l.grip_pose.position = vec3f(-apart * 0.5, 1.0, -0.5);
    r.grip_pose.position = vec3f(apart * 0.5, 1.0, -0.5);
    if (30.0..33.0).contains(&t) {
        l.grip = 1.0;
        r.grip = 1.0;
    }
    if (36.0..38.0).contains(&t) {
        r.grip = 1.0;
        r.stick.y = -1.0;
    }
    if (40.0..41.0).contains(&t) {
        r.grip = 1.0;
        r.stick.x = 1.0;
    }
    if (44.0..47.0).contains(&t) {
        l.stick = vec2f(0.0, -1.0);
    }
    if (48.0..50.0).contains(&t) {
        r.stick.x = 0.6;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(x: f32, grip: bool) -> XrController {
        let mut c = XrController { buttons: XrController::ACTIVE, ..Default::default() };
        c.grip_pose.position = vec3f(x, 1.0, -0.5);
        c.grip = if grip { 1.0 } else { 0.0 };
        c
    }

    fn state(l: XrController, r: XrController) -> XrState {
        XrState { left_controller: l, right_controller: r, ..Default::default() }
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() <= 1.0e-3 * a.abs().max(b.abs()).max(1.0)
    }

    #[test]
    fn both_grips_scale_and_turn_the_box_and_the_world_with_it_without_a_jump() {
        let mut nav = XrNav::new(XrNavConfig { mode: XrNavMode::Free, ..XrNavConfig::FOLLOW });
        let (half0, scale0) = (nav.clip.half.x, nav.scale);
        nav.update(&state(held(-0.15, true), held(0.15, true)), 0.01);
        assert_eq!(nav.clip.half.x, half0, "closing both grips moves nothing");
        // the hands twice as far apart: box and world twice the size
        nav.update(&state(held(-0.3, true), held(0.3, true)), 0.01);
        assert!(near(nav.clip.half.x, half0 * 2.0) && near(nav.scale, scale0 * 2.0), "{} {}", nav.clip.half.x, nav.scale);
        // the right hand pulled back: the box turns about the vertical
        let mut r = held(0.3, true);
        r.grip_pose.position.z = -0.2;
        nav.update(&state(held(-0.3, true), r), 0.01);
        assert!(nav.clip.yaw.abs() > 0.3, "turned {}", nav.clip.yaw);
        // letting go keeps it where the hands left it
        let (yaw, scale) = (nav.clip.yaw, nav.scale);
        nav.update(&state(held(-0.3, false), held(0.3, false)), 0.01);
        assert!(nav.clip.yaw == yaw && near(nav.scale, scale));
    }

    #[test]
    fn one_grip_and_its_stick_zoom_in_eased() {
        let mut nav = XrNav::new(XrNavConfig::FOLLOW);
        let scale0 = nav.scale;
        let mut r = held(0.2, true);
        r.stick.y = -1.0; // the platform's XR stick: up is -y
        for _ in 0..100 {
            nav.update(&state(held(-0.2, false), r.clone()), 0.01);
        }
        let want = scale0 * XrNavConfig::FOLLOW.zoom_rate.exp();
        assert!(nav.zoom_target > scale0 * 3.0 && near(nav.zoom_target, want), "target {} want {want}", nav.zoom_target);
        assert!(nav.scale > scale0 * 2.0 && nav.scale <= nav.zoom_target, "eased {}", nav.scale);
    }

    #[test]
    fn free_left_stick_flies_the_world_toward_the_head() {
        let mut nav = XrNav::new(XrNavConfig { mode: XrNavMode::Free, ..XrNavConfig::FOLLOW });
        let z0 = nav.view().origin.z;
        let mut l = held(-0.2, false);
        l.stick.y = -1.0;
        for _ in 0..10 {
            nav.update(&state(l.clone(), held(0.2, false)), 0.01);
        }
        assert!(nav.view().origin.z > z0, "the world came closer: {} -> {}", z0, nav.view().origin.z);
        assert_eq!(nav.gamepad().left_stick.y, 0.0, "Free: a flying stick is no gamepad stick");
    }

    #[test]
    fn a_grabbing_hand_presses_nothing_and_presses_are_counted() {
        let mut nav = XrNav::new(XrNavConfig::FOLLOW);
        let mut r = held(0.2, true);
        r.buttons |= XrController::CLICK_A;
        nav.update(&state(held(-0.2, false), r.clone()), 0.01);
        assert_eq!(nav.gamepad().a, 0.0);
        r.grip = 0.0;
        nav.update(&state(held(-0.2, false), r.clone()), 0.01);
        assert_eq!(nav.gamepad().a, 1.0);
        assert_eq!(nav.gamepad().presses[gb::A], 1);
        nav.update(&state(held(-0.2, false), r), 0.01);
        assert_eq!(nav.gamepad().presses[gb::A], 1, "held is one press");
    }

    #[test]
    fn follow_keeps_the_subject_at_the_anchor_facing_into_the_box() {
        let mut nav = XrNav::new(XrNavConfig::FOLLOW);
        let pos = vec3f(120.0, 4.0, -35.0);
        let v = nav.follow(pos, 0.7, 1.0);
        let at = v.room_from_world(pos);
        let a = nav.anchor();
        assert!(near(at.x, a.x) && near(at.y, a.y) && near(at.z, a.z), "{at:?} vs {a:?}");
        // the subject's forward (-sin h, -cos h) is the box's forward
        let fwd = v.room_from_world(pos + vec3f(-(0.7f32).sin(), 0.0, -(0.7f32).cos())) - at;
        assert!(fwd.x.abs() < 1.0e-3 && fwd.z < 0.0, "{fwd:?}");
        // a cut (a respawn facing back) snaps instead of swinging round
        let _ = nav.follow(pos, 0.7 + 3.1, 1.01);
        assert_eq!(nav.heading_turn().0, Some(0.7 + 3.1));
    }
}
