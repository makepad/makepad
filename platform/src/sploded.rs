//! The "sploded" view: a LIVE 2.5D exploded z-layer mode.
//!
//! Live, not frozen: the exploded stack is the running app seen from an
//! angle. Wheel scrolling still scrolls the list under the cursor, hover
//! still hovers, and the tweaker still picks — every pointer event is routed
//! through the inverse of the explode transform (`sploded_route`: screen
//! point -> the plane the ray lands on -> that plane's own 2D coordinates)
//! and then dispatched exactly as in flat 2D. The one gesture the mode keeps
//! for itself is press-and-drag, which orbits the stack; a press that never
//! moves is a click and flows through like everything else. Grabbing a
//! scrollbar thumb by dragging is therefore deliberately NOT possible while
//! exploded — a drag is the orbit — and that is the coexistence rule.
//!
//! F10 — once the dev overlays are switched on ([`crate::devtools`]) — tilts
//! the window into an isometric stack that renders **the component
//! nesting structure**: one plane per nesting level, siblings sharing a plane,
//! children lifting toward the viewer and their parents staying at the bottom
//! of the stack. The point is to see — and click — the fully-covered parent
//! containers that flat 2D picking can never reach.
//!
//! # The seam
//!
//! The 2D pass already carries everything needed:
//!
//! * `world.z` in every vertex shader is `<per-instance depth> +
//!   draw_call.zbias`, and `zbias` is a per-draw-call value the backend's
//!   draw walk hands out. While the mode is up, the walk hands out
//!   `CxDrawCall::turtle_depth` — the component nesting depth stamped when
//!   the call was created — instead of the paint-order counter.
//! * every 2D vertex function ends in
//!   `draw_pass.camera_projection * (draw_pass.camera_view * world)`, and
//!   `camera_view` is the identity for ordinary 2D passes.
//!
//! So the mode is one matrix plus one substituted z. `CxDrawPass::set_ortho_matrix`
//! writes an explode matrix into `camera_view` instead of the identity. No
//! shader is edited, no instance layout changes, and with the mode off
//! `camera_view` is the identity it always was and batching is untouched.
//!
//! Because a draw call carries ONE z, batches must stay depth-homogeneous
//! while the mode is up — `find_appendable_drawcall` takes a depth target that
//! is `Some` only then, which is what keeps mode-off rendering byte-identical.
//!
//! # The depth convention, derived once (do not flip signs by eye)
//!
//! This axis was flipped three times by trial before it was written down. The
//! chain below is the whole derivation; `sign_convention_is_stable` in the
//! tests at the bottom of this file asserts it, so it cannot regress silently.
//!
//! 1. **Nesting increases into the tree.** `Cx::nesting_depth` counts
//!    `WidgetRef` draw scopes, so a child's `turtle_depth` is strictly greater
//!    than its parent's.
//! 2. **Nesting depth becomes `world.z`.** While the mode is up the backend
//!    walk hands out `turtle_depth * SPLODED_DEPTH_UNIT` as `zbias`, and every
//!    2D shader computes `world.z = <per-instance depth> + zbias`. Deeper
//!    nesting is therefore a LARGER `world.z`.
//! 3. **Larger `world.z` is nearer the eye.** The 2D ortho is built with
//!    `near = 100, far = -100` (`set_ortho_matrix`), giving
//!    `z_clip = 0.5 - z_view/200`; the depth test is `LessEqual` against a
//!    1.0 clear. Larger z_view means smaller z_clip means it wins the depth
//!    test. So the eye sits at large +z looking down the -z direction, and
//!    "more nested" already means "in front" without any sign choice.
//! 4. **Screen y is down.** The ortho passes `top = offset.y` and
//!    `bottom = offset.y + size.y`, so the y row carries `-2/size.y`: a larger
//!    `world.y` is lower on the screen.
//! 5. **Ortho has no perspective, so step 3 is invisible on its own.** Moving
//!    along the view axis changes nothing in an orthographic projection. The
//!    stack is made legible by SHEARING the z axis onto a screen direction,
//!    and which direction that is, is the one real choice here.
//! 6. **The camera looks DOWN at the stack.** Picture sheets stacked on a
//!    desk with the most-nested sheet on top, viewed from above and slightly
//!    in front: the desk recedes toward the top of the image, so the sheet
//!    nearest the eye sits LOWER in the image and the base of the stack
//!    recedes upward and away.
//!
//! Conclusion, and the one sign this file sets: **a deeper layer projects
//! DOWNWARD on screen (larger `y`) and NEARER in depth (larger view z).**
//! Concretely `camera_view` applies `Rx(-pitch)`, not `Rx(+pitch)`: the raw
//! right-handed rotation in a (right, down, toward-viewer) frame puts the eye
//! BELOW the stack, which is the inside-out reading the user first rejected.
//!
//! Two independent things can make a correct matrix look wrong, and both have
//! done so here — check them before touching a sign:
//!
//! * the body pass composites through a texture, so a wrong `source_y_flip` in
//!   `SplodedStack::draw_resolve` mirrors the whole scene vertically, which
//!   inverts the APPARENT z direction while the matrix is fine. Readable text
//!   in a grab is the tell: if the text is mirrored, fix the flip, not the z.
//! * a stale build. Confirm the binary under test contains the fix.
//!
//! The matrix keeps its third row a pure `z -> z * scale` pass-through on
//! purpose. Instances inside one plane share a z, so paint order decides among
//! them exactly as in flat 2D. Parallel planes cannot intersect, so that is
//! also geometrically right.

use crate::{
    cx::Cx,
    draw_list::{CxDrawKind, DrawListId},
    draw_pass::{CxDrawPassParent, CxDrawPassRect},
    event::{Event, KeyCode, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ScrollEvent},
    makepad_math::*,
};

/// Everything a pass needs to build its explode matrix. Copied onto the pass
/// so `set_ortho_matrix` — which has no `Cx` — can consume it.
#[derive(Clone, Copy, Debug)]
pub struct SplodedParams {
    /// Rotation about the vertical screen axis, radians.
    pub yaw: f32,
    /// Tilt about the horizontal screen axis, radians.
    pub pitch: f32,
    /// Pixels of pre-rotation displacement per world-z unit (= per draw call,
    /// because `zbias_step` is 1.0 while sploded).
    pub gain: f32,
    /// Uniform x/y scale that fits the tilted stack back inside the window.
    pub fit: f32,
    /// The world z the stack pivots on — the middle layer, so the fan opens
    /// both ways from the window centre instead of only forward.
    pub z_center: f32,
    /// z output scale, sized so the deepest layer stays inside the ortho
    /// clip range without touching `camera_projection`.
    pub z_scale: f32,
}

impl SplodedParams {
    /// Build `camera_view` for a pass covering `offset..offset+size`.
    ///
    /// Frame: `u` right, `v` down, `w` toward the viewer (larger `world.z` is
    /// nearer, because the ortho maps `z_clip = 0.5 - z/200` and the depth
    /// test is LessEqual). Yaw rotates about `v`, pitch about `u`, applied as
    /// `Ry(yaw) * Rx(-pitch)` — pitch first, so a horizontal row of text stays
    /// a horizontal row and the picture reads as a stack of layers rather
    /// than a sheared page. Then a uniform fit scale about the pass centre.
    ///
    /// The pitch is NEGATED against the raw right-handed convention on
    /// purpose: `Rx(+b)` in a (right, down, toward-viewer) frame puts the eye
    /// BELOW the stack, so the layers nearest you ride upward and the picture
    /// reads inside out. `Rx(-b)` is the same rotation seen from above —
    /// nested children, being nearer, come down and forward over their
    /// parents, which is what a stack of sheets on a desk looks like.
    pub fn camera_view(&self, offset: Vec2d, size: Vec2d) -> Mat4f {
        let cx = (offset.x + size.x * 0.5) as f32;
        let cy = (offset.y + size.y * 0.5) as f32;
        let (sa, ca) = (self.yaw.sin(), self.yaw.cos());
        // Rx(-pitch): looking down on the stack, not up at it.
        let (sb, cb) = (-self.pitch.sin(), self.pitch.cos());
        let f = self.fit;
        let g = self.gain;
        // The stack fans out of the z = z_center plane, so `w = (z - zc) * g`.
        let zc = self.z_center;

        // u' = u*ca + v*sa*sb + w*sa*cb        (w = (z - zc) * g)
        // v' = v*cb            - w*sb
        let m00 = f * ca;
        let m01 = f * sa * sb;
        let m02 = f * g * sa * cb;
        let m03 = cx - m00 * cx - m01 * cy - m02 * zc;
        let m10 = 0.0;
        let m11 = f * cb;
        let m12 = -f * g * sb;
        let m13 = cy - m11 * cy - m12 * zc;
        // row2: pure z pass-through — depth ordering identical to flat 2D.
        let m22 = self.z_scale;

        // Mat4f is column major: v[4 * col + row].
        Mat4f {
            v: [
                m00, m10, 0.0, 0.0, //
                m01, m11, 0.0, 0.0, //
                m02, m12, m22, 0.0, //
                m03, m13, 0.0, 1.0,
            ],
        }
    }
}

impl SplodedParams {
    /// Invert the projection for the plane at `level`: given a screen point,
    /// which layout point on that plane lands under it.
    ///
    /// This is what makes picking a PARENT possible. The transform is affine
    /// with a pure-z pass-through, so for a fixed plane the screen mapping is
    /// a 2x2 affine solve in closed form — no ray marching, no depth buffer
    /// readback. A caller walks planes from the deepest down, un-projects the
    /// cursor onto each, and hit-tests the widgets that drew at that depth
    /// with their ordinary layout rects: the first hit is what the eye sees
    /// under the cursor. A child only covers its own footprint, so everywhere
    /// else the ray reaches the parent's exposed plane or its hairline —
    /// which flat 2D picking can never offer.
    pub fn unproject(&self, offset: Vec2d, size: Vec2d, screen: Vec2d, level: f32) -> Vec2d {
        let cx = (offset.x + size.x * 0.5) as f32;
        let cy = (offset.y + size.y * 0.5) as f32;
        let (sa, ca) = (self.yaw.sin(), self.yaw.cos());
        let (sb, cb) = (-self.pitch.sin(), self.pitch.cos());
        let f = self.fit;
        let g = self.gain;
        let zc = self.z_center;
        let z = level * SPLODED_DEPTH_UNIT;

        // Same rows as `camera_view`, with z fixed so only x and y are unknown:
        //   sx = m00*x + m01*y + (m02*z + m03)
        //   sy =         m11*y + (m12*z + m13)
        let m00 = f * ca;
        let m01 = f * sa * sb;
        let m02 = f * g * sa * cb;
        let m03 = cx - m00 * cx - m01 * cy - m02 * zc;
        let m11 = f * cb;
        let m12 = -f * g * sb;
        let m13 = cy - m11 * cy - m12 * zc;

        // Row 1 has no x term, so y falls out directly and x follows.
        if m11.abs() < 1.0e-6 || m00.abs() < 1.0e-6 {
            return screen;
        }
        let y = (screen.y as f32 - (m12 * z + m13)) / m11;
        let x = (screen.x as f32 - m01 * y - (m02 * z + m03)) / m00;
        dvec2(x as f64, y as f64)
    }
}

/// One primary press in progress: where it started and the angles it started
/// from. It becomes an orbit only once the pointer travels past the click
/// threshold; until then it is a click in the making and flows to the app.
#[derive(Clone, Copy, Debug)]
struct SplodedDrag {
    start: Vec2d,
    yaw: f32,
    pitch: f32,
    orbiting: bool,
}

/// A rect to emphasise on its plane — the tweaker's hover and pinned
/// selection, rendered INSIDE the exploded pass at the widget's own nesting
/// level so the highlight sits on the sheet it belongs to. Outline only,
/// never a fill: the mode is an overdraw instrument.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SplodedMark {
    pub rect: Rect,
    pub level: f32,
}

/// Pixels of travel that turn a press into an orbit drag.
const ORBIT_THRESHOLD: f64 = 3.0;

/// The mode's whole state. Lives on `Cx`; inert (and free) while `active` is
/// false, which is the byte-identical-rendering gate.
pub struct SplodedView {
    active: bool,
    /// A toggle requested from WITHIN event dispatch (the tweaker's panel
    /// button): performed by the intercept at the next event, pre-dispatch —
    /// set_active clears hovers via nested dispatch and must never run
    /// re-entrantly (it hung the app when called from action handling).
    pending_toggle: bool,
    yaw: f32,
    pitch: f32,
    /// Fraction of the window's smaller dimension the whole stack spans.
    spread: f32,
    /// Deepest component nesting level in the last draw — the stack's height.
    /// Roughly 10-20, which is why the fan is calm by construction where v1's
    /// draw-call count (100s) made a staircase out of a row of buttons.
    layers: f32,
    drag: Option<SplodedDrag>,
    /// Draw a wireframe frame around every turtle scope. On by default: a
    /// container its children completely cover has no pixels of its own, and
    /// without a frame there is nothing to see or click — which is the whole
    /// point of the mode.
    hairlines: bool,
    /// A region the mode never takes the pointer in — the tweaker's panel
    /// band. See `sploded_set_flat_band`.
    flat_band: Option<Rect>,
    /// The plane the last routed pointer event landed on (`sploded_route`),
    /// `None` when the ray missed the stack or the pointer sat in the flat
    /// band. The tweaker's pick reads this to select ON that plane.
    hit_level: Option<usize>,
    /// The tweaker's hover and pinned outlines, drawn by the body pass owner.
    hover_mark: Option<SplodedMark>,
    pinned_mark: Option<SplodedMark>,
    /// The draw list the marks live in, so a mark change redraws only it.
    mark_list: Option<DrawListId>,
    /// Nesting depth per widget uid, stamped at the draw seam this frame.
    /// The widget tree is rebuilt mid-frame (sync_dirty), which wiped a
    /// per-node stamp, so the plane a widget sits on lives here instead —
    /// beside `nesting_depth_max`, which works for the same reason.
    depth_by_uid: std::collections::HashMap<u64, usize>,
}

/// World-z units per nesting level while the mode is up.
///
/// One level has to be far larger than the per-instance `draw_depth` offsets
/// widgets set for their own layering (Dock uses 10, tab bars 10, the map -50).
/// Those ride on the same `world.z = draw_depth + zbias` sum, so at one unit
/// per level a label with `draw_depth: 10` floated TEN planes in front of its
/// own background — the "why is that text so far in front" report. At 1000
/// units per level the same offset is a hundredth of a level: still a correct
/// tie-break inside the plane, invisible as separation.
pub const SPLODED_DEPTH_UNIT: f32 = 1000.0;
/// Headroom above the layer count for `draw_depth` (widgets use -50..20).
const DRAW_DEPTH_HEADROOM: f32 = 128.0;
/// Keep the clip z inside the ortho's +-100 range with room to spare.
const Z_CLIP_BUDGET: f32 = 90.0;
/// Leave a margin so the tilted stack does not touch the window edge.
const FIT_MARGIN: f32 = 0.92;

// Past roughly 65 degrees the stack goes edge-on and stops being readable,
// so the orbit stops there rather than letting a drag run off the useful range.
const YAW_LIMIT: f32 = 1.15;
const PITCH_LIMIT: f32 = 1.05;
const DRAG_RADIANS_PER_PX: f32 = 0.0035;

const DEFAULT_YAW: f32 = 0.50;
const DEFAULT_PITCH: f32 = 0.38;
/// Fraction of the window's smaller dimension the WHOLE stack spans.
///
/// Deliberately tight: the levels should read as one fanned deck you can count
/// at a glance, not as sheets scattered across an empty screen. At 0.12 a
/// 15-level tree in an 800pt-tall window steps about 6pt per level before the
/// fit scale. The panel binds a scrub field to `sploded_set_spread`.
const DEFAULT_SPREAD: f32 = 0.12;
/// The scrub range the panel knob should offer.
pub const SPLODED_SPREAD_MIN: f32 = 0.0;
pub const SPLODED_SPREAD_MAX: f32 = 2.0;
/// What the panel knob resets to on double-click.
pub const SPLODED_SPREAD_DEFAULT: f32 = DEFAULT_SPREAD;
const SPREAD_KEY_STEP: f32 = 0.05;

impl Default for SplodedView {
    fn default() -> Self {
        Self {
            active: false,
            pending_toggle: false,
            yaw: DEFAULT_YAW,
            pitch: DEFAULT_PITCH,
            spread: DEFAULT_SPREAD,
            layers: 1.0,
            drag: None,
            hairlines: true,
            flat_band: None,
            hit_level: None,
            hover_mark: None,
            pinned_mark: None,
            mark_list: None,
            depth_by_uid: std::collections::HashMap::new(),
        }
    }
}

impl SplodedView {
    pub fn active(&self) -> bool {
        self.active
    }

    /// Resolve the per-pass matrix inputs for a pass of this size.
    fn params(&self, size: Vec2d) -> SplodedParams {
        let w = (size.x as f32).max(1.0);
        let h = (size.y as f32).max(1.0);
        let layers = self.layers.max(1.0);

        // The stack's pre-rotation depth, in pixels, and the gain that gets
        // there one nesting level at a time.
        let depth_px = self.spread * w.min(h);
        let z_span = layers * SPLODED_DEPTH_UNIT;
        let gain = depth_px / z_span;

        let (sa, ca) = (self.yaw.sin().abs(), self.yaw.cos().abs());
        let (sb, cb) = (self.pitch.sin().abs(), self.pitch.cos().abs());
        // Bounding box of the rotated stack, before the fit scale.
        let ext_u = w * ca + h * sa * sb + depth_px * sa * cb;
        let ext_v = h * cb + depth_px * sb;
        let fit = (w / ext_u.max(1.0)).min(h / ext_v.max(1.0)) * FIT_MARGIN;

        SplodedParams {
            yaw: self.yaw,
            pitch: self.pitch,
            gain,
            fit,
            z_center: z_span * 0.5,
            z_scale: Z_CLIP_BUDGET / (z_span + DRAW_DEPTH_HEADROOM),
        }
    }
}

impl Cx {
    /// Is the exploded view up? Widgets may read this to hide chrome that
    /// makes no sense in the mode (the tweaker panel, for one).
    pub fn sploded_active(&self) -> bool {
        self.sploded.active
    }

    /// Enter one component draw scope. Called by every `WidgetRef` draw
    /// entry point, so the counter is the nesting depth as components see
    /// it — one plane per selectable node, with a widget's internal layout
    /// turtles collapsed onto its own plane.
    pub fn enter_nesting_depth(&mut self) {
        self.nesting_depth += 1;
        if self.nesting_depth > self.nesting_depth_max {
            self.nesting_depth_max = self.nesting_depth;
        }
    }

    /// Record which plane a widget drew on this frame (exploded view only),
    /// keyed by uid. Reset each time the mode syncs; the map is only ever
    /// read while the mode is up. The first stamp per uid per frame wins —
    /// a widget's own `draw_walk` seam, before its children deepen the
    /// counter.
    pub fn sploded_note_depth(&mut self, uid: u64, depth: usize) {
        if uid != 0 {
            // Last write per frame wins: a widget's own draw_walk seam
            // stamps it once, and a widget that moved planes (a tab switch)
            // overwrites its old value. Entries for widgets NOT drawn this
            // frame go stale, but the pick and the marks require a live
            // on-screen rect, so a stale depth is never acted on.
            self.sploded.depth_by_uid.insert(uid, depth);
        }
    }

    /// The plane a widget drew on this frame, if it drew at all.
    pub fn sploded_depth_of(&self, uid: u64) -> Option<usize> {
        self.sploded.depth_by_uid.get(&uid).copied()
    }

    pub fn exit_nesting_depth(&mut self) {
        self.nesting_depth = self.nesting_depth.saturating_sub(1);
    }

    /// The depth a draw call created right now belongs to, and the value the
    /// exploded view separates planes by. `None` while the mode is off — that
    /// is what keeps draw-call batching byte-identical.
    pub fn sploded_depth_target(&self) -> Option<f32> {
        if self.sploded.active {
            Some(self.nesting_depth as f32)
        } else {
            None
        }
    }

    /// Should turtle scopes get their wireframe frame this frame? Off unless
    /// the mode is up; `H` toggles it.
    pub fn sploded_hairlines_active(&self) -> bool {
        self.sploded.active && self.sploded.hairlines
    }

    /// How far apart the nesting levels sit, as a fraction of the window's
    /// smaller dimension spanned by the whole stack. Bind a panel scrub field
    /// to this pair; `SPLODED_SPREAD_MIN`/`MAX` are the sane range.
    pub fn sploded_spread(&self) -> f32 {
        self.sploded.spread
    }

    /// Set the level separation. Safe to call at any time — takes effect on
    /// the next frame whether or not the mode is currently up. The `+`/`-`
    /// keys drive this same value.
    pub fn sploded_set_spread(&mut self, spread: f32) {
        let spread = spread.clamp(SPLODED_SPREAD_MIN, SPLODED_SPREAD_MAX);
        if self.sploded.spread == spread {
            return;
        }
        self.sploded.spread = spread;
        if self.sploded.active {
            self.sploded_sync();
        }
    }

    /// Programmatic toggle (the tweaker's panel button). DEFERRED: safe to
    /// call from anywhere including action handling — the intercept
    /// performs it at the next event, pre-dispatch. Leave via Esc / F10, or
    /// the same button (a press inside a declared flat band always reaches
    /// the app, so the panel stays clickable while the mode is up).
    pub fn sploded_toggle(&mut self) {
        self.sploded.pending_toggle = true;
        self.redraw_all();
    }

    /// What `sploded_active` will read once a queued toggle has performed:
    /// the state a caller that just called `sploded_toggle` should show.
    pub fn sploded_will_be_active(&self) -> bool {
        self.sploded.active ^ self.sploded.pending_toggle
    }

    /// After a panic was contained at a platform callback boundary the mode
    /// is the first suspect: leave it, so a mode-specific fault cannot
    /// wound every following frame, and let the flat app draw again.
    #[allow(dead_code)] // wired where panics are contained at the callback boundary (macos today)
    pub(crate) fn sploded_recover_after_panic(&mut self) {
        if self.sploded.active {
            crate::log!("sploded view OFF — a panic was contained while it was up");
            self.sploded_set_active(false);
        }
    }

    /// Declare a screen region the exploded mode must keep its hands off.
    ///
    /// The mode owns the pointer so a drag can orbit the stack, but the
    /// tweaker's panel occupies a fixed band and has to stay clickable. A
    /// press that lands inside this rect is never consumed — it flows to the
    /// app exactly as it would with the mode off. Call it with the panel's
    /// band each time the panel draws, and `None` when it closes.
    pub fn sploded_set_flat_band(&mut self, band: Option<Rect>) {
        self.sploded.flat_band = band;
    }

    /// Map a screen point back onto the plane at nesting level `level`, for a
    /// window of `size`. `None` while the mode is off, in which case the
    /// caller's ordinary 2D point is already correct.
    ///
    /// The ray pick: walk levels from `sploded_max_level()` down to 0, call
    /// this for each, and hit-test the widgets whose nesting depth equals that
    /// level against their normal `Area::rect()`s. First hit wins — clicking a
    /// covered parent's exposed frame selects the PARENT.
    pub fn sploded_unproject(&self, size: Vec2d, screen: Vec2d, level: f32) -> Option<Vec2d> {
        if !self.sploded.active {
            return None;
        }
        Some(
            self.sploded
                .params(size)
                .unproject(dvec2(0.0, 0.0), size, screen, level),
        )
    }

    /// Deepest nesting level in the last draw — where a ray pick starts.
    pub fn sploded_max_level(&self) -> usize {
        self.nesting_depth_max
    }

    /// The plane the last routed pointer event landed on. `None` when the
    /// mode is off, the ray missed every plane, or the pointer is in the flat
    /// band. A pick made while this is `Some(level)` must not select anything
    /// nested deeper than `level`: the cursor is on that sheet, and a deeper
    /// child whose 2D rect happens to contain the un-projected point sits on
    /// another sheet entirely.
    pub fn sploded_hit_level(&self) -> Option<usize> {
        if !self.sploded.active {
            return None;
        }
        self.sploded.hit_level
    }

    /// Set the hover / pinned outlines the body pass renders on their own
    /// planes. Cheap to call every overlay redraw: only a change redraws, and
    /// then only the mark draw list.
    pub fn sploded_set_marks(&mut self, hover: Option<SplodedMark>, pinned: Option<SplodedMark>) {
        if self.sploded.hover_mark == hover && self.sploded.pinned_mark == pinned {
            return;
        }
        self.sploded.hover_mark = hover;
        self.sploded.pinned_mark = pinned;
        if let Some(list) = self.sploded.mark_list {
            self.redraw_list(list);
        }
    }

    pub fn sploded_marks(&self) -> (Option<SplodedMark>, Option<SplodedMark>) {
        (self.sploded.hover_mark, self.sploded.pinned_mark)
    }

    /// The body-pass owner registers the draw list its marks go in.
    pub fn sploded_set_mark_list(&mut self, list: DrawListId) {
        self.sploded.mark_list = Some(list);
    }

    /// The exploded pass and its logical size, while one is armed.
    fn sploded_pass(&self) -> Option<(Vec2d, DrawListId)> {
        for pass_id in self.passes.id_iter() {
            let pass = &self.passes[pass_id];
            if pass.sploded.is_none() {
                continue;
            }
            if let (Some(CxDrawPassRect::Size(size)), Some(list)) =
                (&pass.pass_rect, pass.main_draw_list_id)
            {
                return Some((*size, list));
            }
        }
        None
    }

    /// The ray pick: which plane does a screen point land on, and where on
    /// that plane. Walks the exploded pass's draw lists once, collecting every
    /// instance's clipped rect per nesting level (the same rect
    /// `Area::clipped_rect` reports, so it agrees with 2D hit-testing), then
    /// un-projects the cursor onto each plane from the deepest down. The
    /// first plane with an instance under the un-projected point wins —
    /// clicking a covered parent's exposed frame lands on the PARENT, because
    /// its child's footprint on the deeper plane does not contain the ray.
    /// Hairline frames are instances too, so drawless containers are hit.
    fn sploded_ray_hit(&self, screen: Vec2d) -> Option<(usize, Vec2d)> {
        let (size, root) = self.sploded_pass()?;
        let params = self.sploded.params(size);
        let max = self.nesting_depth_max;
        let mut planes: Vec<Vec<Rect>> = vec![Vec::new(); max + 1];
        let mut stack = vec![root];
        while let Some(list_id) = stack.pop() {
            let draw_list = &self.draw_lists[list_id];
            let u = &draw_list.draw_list_uniforms;
            let has_clip = draw_list.draw_list_has_clip;
            let shift = dvec2(u.view_shift.x as f64, u.view_shift.y as f64);
            let view_clip = (
                dvec2(u.view_clip.x as f64, u.view_clip.y as f64),
                dvec2(u.view_clip.z as f64, u.view_clip.w as f64),
            );
            for order_index in 0..draw_list.draw_item_order_len() {
                let Some(item_id) = draw_list.draw_item_id_at_order_index(order_index) else {
                    continue;
                };
                let item = &draw_list.draw_items[item_id];
                match &item.kind {
                    CxDrawKind::SubList(sub) => stack.push(*sub),
                    CxDrawKind::DrawCall(dc) => {
                        let level = dc.turtle_depth.max(0.0) as usize;
                        if level > max {
                            continue;
                        }
                        let Some(buf) = item.instances.as_ref() else {
                            continue;
                        };
                        let sh = &self.draw_shaders[dc.draw_shader_id.index];
                        let (Some(rp), Some(rs)) = (sh.mapping.rect_pos, sh.mapping.rect_size)
                        else {
                            continue;
                        };
                        let stride = sh.mapping.instances.total_slots;
                        if stride == 0 {
                            continue;
                        }
                        for i in 0..buf.len() / stride {
                            let o = i * stride;
                            let mut rect = Rect {
                                pos: dvec2(buf[o + rp] as f64, buf[o + rp + 1] as f64),
                                size: dvec2(buf[o + rs] as f64, buf[o + rs + 1] as f64),
                            };
                            if let Some(dcl) = sh.mapping.draw_clip {
                                rect = rect.clip((
                                    dvec2(buf[o + dcl] as f64, buf[o + dcl + 1] as f64),
                                    dvec2(buf[o + dcl + 2] as f64, buf[o + dcl + 3] as f64),
                                ));
                            }
                            if has_clip {
                                rect = rect.translate(shift).clip(view_clip);
                            }
                            if rect.size.x > 0.0 && rect.size.y > 0.0 {
                                planes[level].push(rect);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        for level in (0..=max).rev() {
            if planes[level].is_empty() {
                continue;
            }
            let p = params.unproject(dvec2(0.0, 0.0), size, screen, level as f32);
            if planes[level].iter().any(|r| r.contains(p)) {
                return Some((level, p));
            }
        }
        None
    }

    /// The live-view router. Runs AFTER `sploded_intercept` on every event:
    /// a pointer event over the exploded body comes back re-addressed to the
    /// plane its ray lands on, in that plane's own 2D coordinates, for the
    /// ordinary dispatch that follows. `None` leaves the event untouched —
    /// mode off, not a pointer event, pointer in the flat band, or an orbit
    /// drag in progress (the intercept eats those).
    pub(crate) fn sploded_route(&mut self, event: &Event) -> Option<Event> {
        if !self.sploded.active {
            return None;
        }
        if self.sploded.drag.is_some_and(|d| d.orbiting) {
            return None;
        }
        let abs = match event {
            Event::MouseMove(e) => e.abs,
            Event::MouseDown(e) => e.abs,
            Event::MouseUp(e) => e.abs,
            Event::Scroll(e) => e.abs,
            _ => return None,
        };
        if self.sploded_in_flat_band(abs) {
            self.sploded.hit_level = None;
            return None;
        }
        let (level, p) = match self.sploded_ray_hit(abs) {
            Some((level, p)) => (Some(level), p),
            // Off the deck entirely: address the base plane so the app sees
            // a coherent point (a hover leaving a widget must still arrive).
            None => {
                let p = self
                    .sploded_pass()
                    .map(|(size, _)| {
                        self.sploded
                            .params(size)
                            .unproject(dvec2(0.0, 0.0), size, abs, 0.0)
                    })
                    .unwrap_or(abs);
                (None, p)
            }
        };
        self.sploded.hit_level = level;
        Some(match event {
            Event::MouseMove(e) => Event::MouseMove(MouseMoveEvent { abs: p, ..e.clone() }),
            Event::MouseDown(e) => Event::MouseDown(MouseDownEvent { abs: p, ..e.clone() }),
            Event::MouseUp(e) => Event::MouseUp(MouseUpEvent { abs: p, ..e.clone() }),
            Event::Scroll(e) => Event::Scroll(ScrollEvent { abs: p, ..e.clone() }),
            _ => unreachable!(),
        })
    }

    fn sploded_in_flat_band(&self, abs: Vec2d) -> bool {
        self.sploded
            .flat_band
            .map(|b| b.contains(abs))
            .unwrap_or(false)
    }

    /// First stop for every event. Returns true when the event was consumed
    /// by the mode and must not reach the app.
    ///
    /// Off, this only ever looks at one key. On, it claims the mode's own
    /// keys and the orbit drag — nothing else. Every other pointer event
    /// flows on through `sploded_route` to the app: the view is live.
    pub(crate) fn sploded_intercept(&mut self, event: &Event) -> bool {
        if self.sploded.pending_toggle {
            self.sploded.pending_toggle = false;
            let on = !self.sploded.active;
            self.sploded_set_active(on);
        }
        match event {
            Event::KeyDown(e) => {
                // F10 is only ours when the app opted into the dev overlays
                // (`--devtools` / `MAKEPAD_DEVTOOLS=1` / `--remote`). Otherwise
                // it is the app's key like any other. `sploded_toggle` still
                // works either way, so an app can put the mode on a key of its
                // own choosing.
                if e.key_code == KeyCode::F10 && crate::devtools::enabled() {
                    if e.is_repeat {
                        return true;
                    }
                    let on = !self.sploded.active;
                    self.sploded_set_active(on);
                    return true;
                }
                if !self.sploded.active {
                    return false;
                }
                // Only the mode's own keys are claimed; the rest reach the app
                // so the panel stays usable while the stack is exploded.
                match e.key_code {
                    KeyCode::Escape => self.sploded_set_active(false),
                    KeyCode::Equals | KeyCode::NumpadAdd => {
                        self.sploded_set_spread(self.sploded.spread + SPREAD_KEY_STEP);
                    }
                    KeyCode::Minus | KeyCode::NumpadSubtract => {
                        self.sploded_set_spread(self.sploded.spread - SPREAD_KEY_STEP);
                    }
                    KeyCode::ArrowLeft => {
                        self.sploded.yaw = (self.sploded.yaw - 0.06).max(-YAW_LIMIT);
                        self.sploded_sync();
                    }
                    KeyCode::ArrowRight => {
                        self.sploded.yaw = (self.sploded.yaw + 0.06).min(YAW_LIMIT);
                        self.sploded_sync();
                    }
                    KeyCode::ArrowUp => {
                        self.sploded.pitch = (self.sploded.pitch + 0.06).min(PITCH_LIMIT);
                        self.sploded_sync();
                    }
                    KeyCode::ArrowDown => {
                        self.sploded.pitch = (self.sploded.pitch - 0.06).max(-PITCH_LIMIT);
                        self.sploded_sync();
                    }
                    KeyCode::Key0 => {
                        self.sploded.yaw = DEFAULT_YAW;
                        self.sploded.pitch = DEFAULT_PITCH;
                        self.sploded.spread = DEFAULT_SPREAD;
                        self.sploded_sync();
                    }
                    // True isometric: 45 degrees of yaw, the classic
                    // 35.264-degree pitch (atan(1/sqrt 2)), so the three
                    // axes foreshorten equally.
                    KeyCode::KeyI => {
                        self.sploded.yaw = std::f32::consts::FRAC_PI_4;
                        self.sploded.pitch = 0.6154797;
                        self.sploded_sync();
                    }
                    // Cycle the hairline frames: all scopes / none.
                    KeyCode::KeyH => {
                        self.sploded.hairlines = !self.sploded.hairlines;
                        self.sploded_sync();
                    }
                    _ => return false,
                }
                true
            }
            // Keys the mode does not claim reach the app: the panel's fields
            // stay typeable while the stack is exploded.
            Event::KeyUp(_) | Event::TextInput(_) => false,
            Event::MouseDown(e) => {
                if !self.sploded.active
                    || !e.button.is_primary()
                    || self.sploded_in_flat_band(e.abs)
                {
                    return false;
                }
                // Arm a possible orbit, but let the press itself flow: the
                // tweaker picks on the down, and a press that never travels
                // is a click, not a drag.
                self.sploded.drag = Some(SplodedDrag {
                    start: e.abs,
                    yaw: self.sploded.yaw,
                    pitch: self.sploded.pitch,
                    orbiting: false,
                });
                false
            }
            Event::MouseMove(e) => {
                // Only an orbit in progress belongs to the mode. Plain motion
                // flows on (re-addressed to its plane), so hover works.
                let Some(mut drag) = self.sploded.drag else {
                    return false;
                };
                let d = e.abs - drag.start;
                if !drag.orbiting {
                    if d.x.abs() < ORBIT_THRESHOLD && d.y.abs() < ORBIT_THRESHOLD {
                        return false;
                    }
                    drag.orbiting = true;
                    self.sploded.drag = Some(drag);
                }
                self.sploded.yaw =
                    (drag.yaw + d.x as f32 * DRAG_RADIANS_PER_PX).clamp(-YAW_LIMIT, YAW_LIMIT);
                self.sploded.pitch =
                    (drag.pitch + d.y as f32 * DRAG_RADIANS_PER_PX).clamp(-PITCH_LIMIT, PITCH_LIMIT);
                self.sploded_sync();
                true
            }
            Event::MouseUp(_) => {
                // The up always pairs with the down the app received, orbit
                // or not — an eaten up wedges the app's own capture.
                self.sploded.drag = None;
                false
            }
            // The wheel belongs to the list under the ray: `sploded_route`
            // re-addresses it and ordinary dispatch scrolls. Spread moved to
            // the +/- keys for exactly this reason.
            Event::Scroll(_) => false,
            _ => false,
        }
    }

    fn sploded_set_active(&mut self, active: bool) {
        if self.sploded.active == active {
            return;
        }
        self.sploded.active = active;
        self.sploded.drag = None;
        self.sploded.hit_level = None;
        self.sploded.hover_mark = None;
        self.sploded.pinned_mark = None;
        self.sploded.depth_by_uid.clear();
        if active {
            // Drop hover/pressed visuals the app was showing when the mode
            // opened, so the frozen picture is not stuck mid-hover.
            self.clear_all_hovers();
            self.handle_pending_clear_hover();
        }
        self.sploded_sync();
        if active {
            crate::log!(
                "sploded view ON — {} nesting levels, drag to orbit, wheel scrolls the app, +/- to explode, I = isometric, 0 resets, esc/F10 exits",
                self.sploded.layers as u32
            );
        } else {
            crate::log!("sploded view OFF");
        }
    }

    /// The explode camera inputs for a pass of this size, or `None` while the
    /// mode is off. `Window::begin` calls this to arm its BODY pass — the
    /// explode is a per-pass camera, and putting it on the window pass would
    /// tilt the tweaker's panel along with the app.
    pub fn sploded_params(&self, size: Vec2d) -> Option<SplodedParams> {
        if !self.sploded.active {
            return None;
        }
        Some(self.sploded.params(size))
    }

    /// Recompute and get a frame out. Cheap enough to run on each drag step.
    ///
    /// The params themselves are armed by whoever owns the body pass; all this
    /// does is refresh the measured layer count, disarm every pass when the
    /// mode goes off, and force the redraw.
    ///
    /// `zbias_step` is deliberately left alone: while the mode is up the
    /// backend walk ignores its running counter entirely (`resolve_zbias`), so
    /// there is nothing to open up and nothing to restore.
    fn sploded_sync(&mut self) {
        if self.sploded.active {
            // Measured by `enter_nesting_depth` during the last draw.
            self.sploded.layers = (self.nesting_depth_max as f32).max(1.0);
        } else {
            for draw_pass_id in self.passes.id_iter() {
                self.passes[draw_pass_id].sploded = None;
            }
        }
        for draw_pass_id in self.passes.id_iter() {
            if matches!(self.passes[draw_pass_id].parent, CxDrawPassParent::Window(_)) {
                self.passes[draw_pass_id].paint_dirty = true;
            }
        }
        // Text's slug matrix is CPU-side, and the depth-homogeneous batching
        // split only happens on emission — both need the content re-emitted.
        self.redraw_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Params equivalent to a real window: 1200x800 at the default angles,
    /// with a 10-level deep tree.
    fn probe() -> (SplodedParams, Vec2d, Vec2d) {
        let mut view = SplodedView::default();
        view.layers = 10.0;
        let size = dvec2(1200.0, 800.0);
        (view.params(size), dvec2(0.0, 0.0), size)
    }

    /// Project a point sitting at the centre of the window on plane `level`.
    fn project(level: f32) -> Vec4f {
        let (params, offset, size) = probe();
        let m = params.camera_view(offset, size);
        let x = (offset.x + size.x * 0.5) as f32;
        let y = (offset.y + size.y * 0.5) as f32;
        m.transform_vec4(vec4(x, y, level * SPLODED_DEPTH_UNIT, 1.0))
    }

    /// THE ANTI-FLIP CHECK. This axis was flipped three times by eye before it
    /// was derived; the derivation lives in this module's docs and this test is
    /// what holds it. If you are here because the picture looks inverted, read
    /// those docs first — a mirrored `source_y_flip` on the body-pass composite
    /// and a stale build both invert the APPARENT direction while this stays
    /// correct.
    #[test]
    fn sign_convention_is_stable() {
        let shallow = project(0.0);
        let deep = project(9.0);

        // Deeper nesting is NEARER the eye: the ortho maps a larger view z to
        // a smaller z_clip and the depth test is LessEqual, so the deeper
        // plane wins. (Step 3 of the derivation.)
        assert!(
            deep.z > shallow.z,
            "a deeper layer must project nearer the eye: deep z {} !> shallow z {}",
            deep.z,
            shallow.z
        );

        // ...and it projects DOWNWARD on screen, because the camera looks down
        // at the stack. Screen y is down (step 4), so "down" is a larger y.
        // (Step 6 of the derivation.)
        assert!(
            deep.y > shallow.y,
            "a deeper layer must project downward on screen: deep y {} !> shallow y {}",
            deep.y,
            shallow.y
        );
    }

    /// Every level is the same size step — the user must be able to count
    /// levels by eye rather than guess whether something is deeply nested or
    /// the stepping is uneven.
    #[test]
    fn levels_are_evenly_spaced() {
        let steps: Vec<f32> = (0..9)
            .map(|i| project((i + 1) as f32).y - project(i as f32).y)
            .collect();
        let first = steps[0];
        assert!(first > 0.5, "level step too small to see: {first}");
        for (i, s) in steps.iter().enumerate() {
            assert!(
                (s - first).abs() < 1.0e-3,
                "level {i} steps by {s}, expected the uniform {first}"
            );
        }
    }

    /// The pick's inverse must land back exactly where the projection took a
    /// point from, on every plane — that round trip IS the ray pick.
    #[test]
    fn unproject_inverts_the_projection() {
        let (params, offset, size) = probe();
        let m = params.camera_view(offset, size);
        for level in [0.0f32, 1.0, 4.0, 9.0] {
            for p in [dvec2(10.0, 20.0), dvec2(600.0, 400.0), dvec2(1190.0, 790.0)] {
                let projected = m.transform_vec4(vec4(
                    p.x as f32,
                    p.y as f32,
                    level * SPLODED_DEPTH_UNIT,
                    1.0,
                ));
                let back = params.unproject(
                    offset,
                    size,
                    dvec2(projected.x as f64, projected.y as f64),
                    level,
                );
                assert!(
                    (back.x - p.x).abs() < 0.01 && (back.y - p.y).abs() < 0.01,
                    "level {level}: {p:?} projected then un-projected to {back:?}"
                );
            }
        }
    }

    /// A widget's own `draw_depth` (Dock uses 10, the map -50) rides the same
    /// `world.z` sum as the nesting depth. It must stay a tie-break INSIDE a
    /// plane and never read as separation — the "why is that text so far in
    /// front of its background" report.
    #[test]
    fn draw_depth_stays_inside_its_plane() {
        let (params, offset, size) = probe();
        let m = params.camera_view(offset, size);
        let x = (offset.x + size.x * 0.5) as f32;
        let y = (offset.y + size.y * 0.5) as f32;
        let plane = |z: f32| m.transform_vec4(vec4(x, y, z, 1.0)).y;

        let one_level = plane(SPLODED_DEPTH_UNIT) - plane(0.0);
        // The largest draw_depth any widget in the tree uses.
        let worst_offset = plane(20.0) - plane(0.0);
        assert!(
            worst_offset.abs() < one_level.abs() * 0.05,
            "draw_depth 20 moves {worst_offset}, more than 5% of a {one_level} level step"
        );
    }
}
