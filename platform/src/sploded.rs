//! The "sploded" view: an inspection-only 2.5D exploded z-layer mode.
//!
//! F10 tilts the whole window into an isometric stack: every GPU draw call
//! becomes one layer, separated along z by its paint order, so overdraw and
//! layering are visible at a glance. While it is on the app receives no
//! input — this is a looking glass, not a picking surface.
//!
//! # The seam
//!
//! The 2D pass already carries everything needed:
//!
//! * `world.z` in every vertex shader is `draw_depth + draw_call.zbias`, and
//!   `zbias` is handed out by the backend's draw walk in *global paint order*
//!   (`render_view` in the OS backends). It IS the layer index.
//! * every 2D vertex function ends in
//!   `draw_pass.camera_projection * (draw_pass.camera_view * world)`, and
//!   `camera_view` is the identity for ordinary 2D passes.
//!
//! So the entire mode is one matrix. `CxDrawPass::set_ortho_matrix` writes an
//! explode matrix into `camera_view` instead of the identity, and the pass's
//! `zbias_step` is opened up from `0.001` to `1.0` so one draw call is one
//! z unit. Nothing else in the render path changes, no shader is edited, and
//! with the mode off `camera_view` is the identity it always was.
//!
//! The matrix keeps its third row a pure `z -> z * scale` pass-through on
//! purpose. Depth ordering therefore stays exactly what it is in normal 2D
//! (later draw call = nearer, instances inside one draw call all share a z so
//! paint order decides), while only x/y pick up the tilt. Parallel planes
//! cannot intersect, so that is also the geometrically correct answer.

use crate::{
    cx::Cx,
    draw_list::DrawListId,
    draw_pass::CxDrawPassParent,
    event::{Event, KeyCode},
    makepad_math::*,
};

/// Everything a pass needs to build its explode matrix. Copied onto the pass
/// so `set_ortho_matrix` — which has no `Cx` — can consume it.
#[derive(Clone, Copy, Debug)]
pub struct SplodedParams {
    /// +1: this pass displays directly. -1: this pass renders to a texture
    /// that the compositor samples Y-FLIPPED (the gauss scene pass on
    /// backends where `gauss_render_texture_y_flip_for_os` says so). The
    /// value comes from the pass REGISTRATION — one source of truth — and
    /// `camera_view` compensates numerically, so a pass restructure can
    /// never silently flip the explode direction again.
    pub v_flip: f32,
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
    /// `Ry(yaw) * Rx(pitch)` — pitch first, so a horizontal row of text stays
    /// a horizontal row and the picture reads as a stack of layers rather
    /// than a sheared page. Then a uniform fit scale about the pass centre.
    pub fn camera_view(&self, offset: Vec2d, size: Vec2d) -> Mat4f {
        let cx = (offset.x + size.x * 0.5) as f32;
        let cy = (offset.y + size.y * 0.5) as f32;
        let (sa, ca) = (self.yaw.sin(), self.yaw.cos());
        let (sb, cb) = (self.pitch.sin(), self.pitch.cos());
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
        let m = Mat4f {
            v: [
                m00, m10, 0.0, 0.0, //
                m01, m11, 0.0, 0.0, //
                m02, m12, m22, 0.0, //
                m03, m13, 0.0, 1.0,
            ],
        };
        if self.v_flip >= 0.0 {
            return m;
        }
        // The compositor will flip this pass vertically; pre-compose the
        // inverse flip (about the pass centre) so the DISPLAYED result is
        // identical to the direct-display case: F * M, F: v -> 2*cy - v.
        let mut v = m.v;
        v[1] = -v[1];
        v[5] = -v[5];
        v[9] = -v[9];
        v[13] = 2.0 * cy - v[13];
        Mat4f { v }
    }
}

#[cfg(test)]
mod sploded_convention_tests {
    use super::*;
    use crate::makepad_math::dvec2;

    /// THE ANTI-FLIP GATE: for any point and depth, the y-flipped-texture
    /// path composed with the compositor's flip must land on EXACTLY the
    /// same screen position as the direct path — the explode direction is
    /// invariant under pass restructures by construction. Runs as
    /// `cargo test -p makepad-platform sploded` and must pass before any
    /// user-instance refresh of this mode.
    #[test]
    fn explode_direction_invariant_under_texture_y_flip() {
        let offset = dvec2(0.0, 0.0);
        let size = dvec2(1200.0, 800.0);
        let base = SplodedParams {
            v_flip: 1.0,
            yaw: 0.35,
            pitch: 0.38,
            gain: 12.0,
            fit: 0.8,
            z_center: 5.0,
            z_scale: 0.01,
        };
        let flipped = SplodedParams {
            v_flip: -1.0,
            ..base
        };
        let md = base.camera_view(offset, size);
        let mf = flipped.camera_view(offset, size);
        let cy = (size.y * 0.5) as f32;
        let apply = |m: &Mat4f, p: [f32; 3]| -> [f32; 2] {
            let x = m.v[0] * p[0] + m.v[4] * p[1] + m.v[8] * p[2] + m.v[12];
            let y = m.v[1] * p[0] + m.v[5] * p[1] + m.v[9] * p[2] + m.v[13];
            [x, y]
        };
        for point in [
            [100.0f32, 100.0, 0.0],
            [600.0, 400.0, 5.0],
            [1100.0, 700.0, 12.0],
            [600.0, 100.0, 20.0],
        ] {
            let direct = apply(&md, point);
            let via_texture = {
                let p = apply(&mf, point);
                // the compositor's vertical flip about the pass centre
                [p[0], 2.0 * cy - p[1]]
            };
            assert!(
                (direct[0] - via_texture[0]).abs() < 0.01
                    && (direct[1] - via_texture[1]).abs() < 0.01,
                "convention flip leaked: {direct:?} vs {via_texture:?} at {point:?}"
            );
        }
        // And the direction law itself: with positive yaw, a DEEPER point
        // at the same screen position must displace toward positive u
        // (toward the camera side), never away.
        let shallow = apply(&md, [600.0, 400.0, 0.0]);
        let deep = apply(&md, [600.0, 400.0, 20.0]);
        assert!(
            deep[0] > shallow[0],
            "deeper must parallax toward the camera side: {shallow:?} vs {deep:?}"
        );
        // Rotation direction law: mirroring the yaw mirrors the parallax.
        let neg = SplodedParams {
            yaw: -base.yaw,
            ..base
        };
        let mn = neg.camera_view(offset, size);
        let deep_neg = apply(&mn, [600.0, 400.0, 20.0]);
        let shallow_neg = apply(&mn, [600.0, 400.0, 0.0]);
        assert!(
            deep_neg[0] < shallow_neg[0],
            "negative yaw must parallax the deep layer the other way"
        );
    }
}

/// One drag in progress: where it started and the angles it started from.
#[derive(Clone, Copy, Debug)]
struct SplodedDrag {
    start: Vec2d,
    yaw: f32,
    pitch: f32,
}

/// The mode's whole state. Lives on `Cx`; inert (and free) while `active` is
/// false, which is the byte-identical-rendering gate.
pub struct SplodedView {
    active: bool,
    /// The pass the exploded camera applies to, WITH its display
    /// y-convention (+1 direct, -1 sampled-flipped): registered each frame
    /// by the window widget from the one authority for that convention.
    /// Never the window pass itself, so panels/overlays stay flat.
    pub target_pass: Option<crate::draw_pass::DrawPassId>,
    /// The registered pass's display y-flip (see target_pass).
    pub target_v_flip: f32,
    /// A toggle requested from WITHIN event dispatch (the tweaker's panel
    /// button): performed by the intercept at the next event, pre-dispatch —
    /// set_active clears hovers via nested dispatch and must never run
    /// re-entrantly (it hung the app when called from action handling).
    pending_toggle: bool,
    yaw: f32,
    pitch: f32,
    /// Fraction of the window's smaller dimension the whole stack spans.
    spread: f32,
    /// Draw calls counted on the last sync — the stack's depth in layers.
    layers: f32,
    drag: Option<SplodedDrag>,
    /// The pass `zbias_step` that was in force before the mode opened it up.
    saved_zbias_step: f32,
}

/// Normal 2D `zbias_step`; while sploded one draw call is one whole z unit,
/// which is also `draw_depth`'s unit — so an element that asked to be drawn
/// `n` in front honestly floats `n` layers forward.
const SPLODED_ZBIAS_STEP: f32 = 1.0;
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
const DEFAULT_SPREAD: f32 = 0.85;

impl Default for SplodedView {
    fn default() -> Self {
        Self {
            active: false,
            pending_toggle: false,
            target_pass: None,
            target_v_flip: 1.0,
            yaw: DEFAULT_YAW,
            pitch: DEFAULT_PITCH,
            spread: DEFAULT_SPREAD,
            layers: 1.0,
            drag: None,
            saved_zbias_step: 0.001,
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
        // there one draw call at a time.
        let depth_px = self.spread * w.min(h);
        let gain = depth_px / layers;

        let (sa, ca) = (self.yaw.sin().abs(), self.yaw.cos().abs());
        let (sb, cb) = (self.pitch.sin().abs(), self.pitch.cos().abs());
        // Bounding box of the rotated stack, before the fit scale.
        let ext_u = w * ca + h * sa * sb + depth_px * sa * cb;
        let ext_v = h * cb + depth_px * sb;
        let fit = (w / ext_u.max(1.0)).min(h / ext_v.max(1.0)) * FIT_MARGIN;

        SplodedParams {
            v_flip: 1.0,
            yaw: self.yaw,
            pitch: self.pitch,
            gain,
            fit,
            z_center: layers * 0.5,
            z_scale: Z_CLIP_BUDGET / (layers + DRAW_DEPTH_HEADROOM),
        }
    }
}

impl Cx {
    /// Is the exploded view up? Widgets may read this to hide chrome that
    /// makes no sense in the mode (the tweaker panel, for one).
    pub fn sploded_active(&self) -> bool {
        self.sploded.active
    }

    /// Programmatic toggle (the tweaker's panel button). DEFERRED: safe to
    /// call from anywhere including action handling — the intercept
    /// performs it at the next event, pre-dispatch. Entering hands input
    /// to the mode; leave via Esc / F10 (the panel is hidden and cannot be
    /// clicked while the mode is up).
    pub fn sploded_toggle(&mut self) {
        self.sploded.pending_toggle = true;
        self.redraw_all();
    }

    /// First stop for every event. Returns true when the event was consumed
    /// by the mode and must not reach the app.
    ///
    /// Off, this only ever looks at one key. On, it eats all pointer and
    /// keyboard input — the mode is inspection-only.
    pub(crate) fn sploded_intercept(&mut self, event: &Event) -> bool {
        if self.sploded.pending_toggle {
            self.sploded.pending_toggle = false;
            let on = !self.sploded.active;
            self.sploded_set_active(on);
        }
        match event {
            Event::KeyDown(e) => {
                if e.key_code == KeyCode::F10 {
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
                match e.key_code {
                    KeyCode::Escape => self.sploded_set_active(false),
                    KeyCode::Equals | KeyCode::NumpadAdd => {
                        self.sploded.spread = (self.sploded.spread + 0.1).min(4.0);
                        self.sploded_sync();
                    }
                    KeyCode::Minus | KeyCode::NumpadSubtract => {
                        self.sploded.spread = (self.sploded.spread - 0.1).max(0.0);
                        self.sploded_sync();
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
                    _ => {}
                }
                true
            }
            Event::KeyUp(_) | Event::TextInput(_) => self.sploded.active,
            Event::MouseDown(e) => {
                if !self.sploded.active {
                    return false;
                }
                self.sploded.drag = Some(SplodedDrag {
                    start: e.abs,
                    yaw: self.sploded.yaw,
                    pitch: self.sploded.pitch,
                });
                true
            }
            Event::MouseMove(e) => {
                if !self.sploded.active {
                    return false;
                }
                if let Some(drag) = self.sploded.drag {
                    let d = e.abs - drag.start;
                    self.sploded.yaw = (drag.yaw + d.x as f32 * DRAG_RADIANS_PER_PX)
                        .clamp(-YAW_LIMIT, YAW_LIMIT);
                    self.sploded.pitch = (drag.pitch + d.y as f32 * DRAG_RADIANS_PER_PX)
                        .clamp(-PITCH_LIMIT, PITCH_LIMIT);
                    self.sploded_sync();
                }
                true
            }
            Event::MouseUp(_) => {
                if !self.sploded.active {
                    return false;
                }
                self.sploded.drag = None;
                true
            }
            Event::Scroll(e) => {
                if !self.sploded.active {
                    return false;
                }
                self.sploded.spread =
                    (self.sploded.spread - e.scroll.y as f32 * 0.004).clamp(0.0, 4.0);
                self.sploded_sync();
                true
            }
            _ => false,
        }
    }

    fn sploded_set_active(&mut self, active: bool) {
        if self.sploded.active == active {
            return;
        }
        self.sploded.active = active;
        self.sploded.drag = None;
        if active {
            // Drop hover/pressed visuals the app was showing when the mode
            // opened, so the frozen picture is not stuck mid-hover.
            self.clear_all_hovers();
            self.handle_pending_clear_hover();
        }
        self.sploded_sync();
        if active {
            crate::log!(
                "sploded view ON — {} draw-call layers, drag to orbit, +/- to explode, 0 resets, esc/F10 exits",
                self.sploded.layers as u32
            );
        } else {
            crate::log!("sploded view OFF");
        }
    }

    /// Push the current state onto every window pass and get a frame out of
    /// it. Cheap enough to run on each drag step.
    /// The window widget registers its body-content (scene) pass here each
    /// frame while the mode is on; the exploded camera then applies to
    /// THAT pass only, and everything at window level (the tweak panel,
    /// tooltips, modals) composites flat.
    pub fn sploded_set_target_pass(
        &mut self,
        pass: Option<crate::draw_pass::DrawPassId>,
        v_flip: f32,
    ) {
        if self.sploded.target_pass != pass || self.sploded.target_v_flip != v_flip {
            self.sploded.target_pass = pass;
            self.sploded.target_v_flip = v_flip;
            self.sploded_sync();
        }
    }

    fn sploded_sync(&mut self) {
        let active = self.sploded.active;
        if active {
            self.sploded.layers = self.sploded_count_layers();
        }
        let target = self.sploded.target_pass;
        for draw_pass_id in self.passes.id_iter() {
            let is_target = target == Some(draw_pass_id);
            let is_window_parent = matches!(
                self.passes[draw_pass_id].parent,
                CxDrawPassParent::Window(_)
            );
            // With a registered target, ONLY that pass explodes; without
            // one (no window began its scene pass yet this toggle), fall
            // back to window passes so the mode is never a silent no-op.
            if target.is_some() {
                if !is_target {
                    // ensure anything previously marked (e.g. the window
                    // pass from the fallback frame) is cleared
                    if self.passes[draw_pass_id].sploded.is_some() {
                        let saved = self.sploded.saved_zbias_step;
                        let pass = &mut self.passes[draw_pass_id];
                        pass.sploded = None;
                        pass.zbias_step = saved;
                    }
                    continue;
                }
            } else if !is_window_parent {
                continue;
            }
            if active {
                let size = self
                    .get_pass_rect(draw_pass_id, 1.0)
                    .map(|r| r.size)
                    .unwrap_or(dvec2(1.0, 1.0));
                let mut params = self.sploded.params(size);
                if Some(draw_pass_id) == self.sploded.target_pass {
                    params.v_flip = self.sploded.target_v_flip;
                }
                let pass = &mut self.passes[draw_pass_id];
                if pass.sploded.is_none() {
                    self.sploded.saved_zbias_step = pass.zbias_step;
                }
                pass.sploded = Some(params);
                pass.zbias_step = SPLODED_ZBIAS_STEP;
                // The slug-text path bakes camera_view into a CPU matrix at
                // DRAW time, so the matrix has to already be current when the
                // redraw below runs — not only when the backend renders.
                let offset = self
                    .get_pass_rect(draw_pass_id, 1.0)
                    .map(|r| r.pos)
                    .unwrap_or_default();
                self.passes[draw_pass_id].set_ortho_matrix(offset, size);
            } else {
                let saved = self.sploded.saved_zbias_step;
                let pass = &mut self.passes[draw_pass_id];
                if pass.sploded.take().is_some() {
                    pass.zbias_step = saved;
                }
            }
            self.passes[draw_pass_id].paint_dirty = true;
        }
        // Text's slug matrix is CPU-side, so the content has to be re-emitted
        // for it to follow the tilt.
        self.redraw_all();
    }

    /// Count the draw calls a window pass will emit, the same walk the
    /// backends do when they hand out zbias — that count is the stack depth.
    fn sploded_count_layers(&self) -> f32 {
        let mut max = 0usize;
        for draw_pass_id in self.passes.id_iter() {
            if !matches!(self.passes[draw_pass_id].parent, CxDrawPassParent::Window(_)) {
                continue;
            }
            if let Some(list_id) = self.passes[draw_pass_id].main_draw_list_id {
                let mut running = 0usize;
                let mut seen = 0usize;
                self.sploded_scan(list_id, &mut running, &mut seen, 0);
                max = max.max(seen);
            }
        }
        (max as f32).max(1.0)
    }

    fn sploded_scan(
        &self,
        draw_list_id: DrawListId,
        running: &mut usize,
        seen: &mut usize,
        depth: usize,
    ) {
        if depth > 64 {
            return;
        }
        let len = self.draw_lists[draw_list_id].draw_item_order_len();
        for order_index in 0..len {
            let Some(draw_item_id) =
                self.draw_lists[draw_list_id].draw_item_id_at_order_index(order_index)
            else {
                continue;
            };
            let kind = &self.draw_lists[draw_list_id].draw_items[draw_item_id].kind;
            if let Some(sub_list_id) = kind.sub_list() {
                if self.draw_lists[sub_list_id].reset_zbias {
                    let mut child = 0usize;
                    self.sploded_scan(sub_list_id, &mut child, seen, depth + 1);
                } else {
                    self.sploded_scan(sub_list_id, running, seen, depth + 1);
                }
            } else if kind.draw_call().is_some() {
                *running += 1;
                *seen = (*seen).max(*running);
            }
        }
    }
}
