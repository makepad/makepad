//! Lane C owns this directory.
//!
//! `Navigator`: the camera controller behind every viewport (one instance per
//! `FabViewport`), implementing `api::NavController`. Fab's grammar,
//! Fab's feel:
//!
//! * **Orbit** — MMB drags, Shift+MMB pans, Ctrl+MMB and the wheel dolly *to
//!   the cursor*, Alt+LMB emulates MMB for one-button pointers. Turntable by
//!   default (up-locked, so twenty mixed drags cannot tilt the horizon),
//!   trackball on request. The pivot is the bounds centre until a double
//!   click puts it on the surface under the pointer. Releasing a flick keeps
//!   the view coasting for a moment.
//! * **Presets** — numpad 1/3/7 (+Ctrl for the opposite), 9 isometric, 5
//!   orthographic, 2/4/6/8 step 15°, Home frames the model, `.` frames the
//!   selection. Laptops have no numpad, so the top-row digits do the same.
//!   Every move is an eased 100–250 ms glide that lands exactly on its mark.
//! * **Fly / walk** — WASD + QE with mouse look, click to capture the pointer
//!   and **Escape always gives it back**; walk locks the eye 1.62 m over
//!   whatever floor the scene's own ray-down query finds, gliding up steps.
//!   Walk speed is physical (1.4 m/s); fly speed follows the size of the model.
//!   The handoff each way keeps the eye and the direction: orbit hands walk
//!   its eye, walk hands orbit a pivot on whatever it was looking at.
//! * **Tours** — plays lane G's `CameraTrack`s and scrubs them.
//!
//! Never: draws into the 3D pass, touches `SceneState`, or knows about any
//! widget other than its own gizmo.

pub mod gizmo;
pub mod orbit;
pub mod track;
pub mod walk;

use crate::api::*;
use makepad_widgets::*;
use orbit::{CameraAnim, WORLD_UP};
use walk::WalkState;

/// Mirrors of `fab.anim_fast` / `_normal` / `_slow` (`theme.rs`): the script
/// tokens are the source of truth for the UI, these are the same numbers for
/// the camera, which lives in Rust.
pub const ANIM_FAST: f32 = 0.10;
pub const ANIM_NORMAL: f32 = 0.15;
pub const ANIM_SLOW: f32 = 0.25;

/// A flick keeps coasting for about a fifth of a second, then stops. Long
/// enough to feel like momentum, short enough never to feel out of control.
const INERTIA_TAU: f32 = 0.11;
/// Below this the coast has visually stopped (layout points per second).
const INERTIA_MIN: f64 = 40.0;
const INERTIA_MAX: f64 = 6000.0;
/// A drag has to be a flick, not a nudge, to coast at all.
const INERTIA_START: f64 = 120.0;
/// Pointer travel (points) before a held primary/secondary button turns
/// from a click into a drag — under it the press stays a click for the
/// select tool or the context menu.
const DRAG_THRESHOLD_PT: f64 = 4.0;

/// One numpad-orbit step, in the same screen units the drag path uses.
fn orbit_step_points() -> f32 {
    (15.0f32).to_radians() / orbit::ORBIT_SENS
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drag {
    Orbit,
    Pan,
    Dolly,
}

pub struct Navigator {
    last_aspect: f32,
    last_rect: Rect,
    /// The mode this navigator has actually entered; compared against the
    /// view's every input so a mode set from the UI performs its handoff.
    mode: NavMode,
    drag: Option<Drag>,
    /// A primary/secondary button held but not yet dragged past
    /// `DRAG_THRESHOLD_PT`: the emulated three-button mouse (Fab's
    /// option, on by default here — Mac trackpads have no middle button).
    /// Bare LMB drag orbits, Alt+LMB and RMB drag pan, Ctrl+LMB dollies.
    pending: Option<(Drag, DVec2)>,
    /// Pointer travel waiting for the next frame. Camera transforms consume
    /// this once per frame; input callbacks only add physical pointer delta.
    move_accum: DVec2,
    /// Total travel from `drag_origin`. Re-evaluating from the press camera
    /// makes even trackball motion independent of how frames divide the path.
    drag_total: DVec2,
    drag_origin: Option<Camera>,
    drag_vel: DVec2,
    inertia: Option<(Drag, DVec2)>,
    anim: Option<CameraAnim>,
    walk: WalkState,
    /// Captured first-person look travel waiting for the next frame.
    walk_look_accum: DVec2,
    /// Last first-person HUD string, so we only emit `StatusMessage` on change.
    last_walk_hud: String,
    /// Fab's auto-perspective: orbiting away from an axis view by hand
    /// leaves orthographic behind.
    auto_persp: bool,
    /// Laptops have no numpad; the top-row digits drive the preset views too.
    emulate_numpad: bool,
}

impl Default for Navigator {
    fn default() -> Self {
        Navigator {
            last_aspect: 0.0,
            last_rect: Rect::default(),
            mode: NavMode::Orbit,
            drag: None,
            pending: None,
            move_accum: DVec2::default(),
            drag_total: DVec2::default(),
            drag_origin: None,
            drag_vel: DVec2::default(),
            inertia: None,
            anim: None,
            walk: WalkState::default(),
            walk_look_accum: DVec2::default(),
            last_walk_hud: String::new(),
            auto_persp: true,
            emulate_numpad: true,
        }
    }
}

impl Navigator {
    fn aspect(&self) -> f32 {
        if self.last_aspect > 0.0 {
            self.last_aspect
        } else {
            16.0 / 9.0
        }
    }

    fn rect_height(&self) -> f32 {
        let h = self.last_rect.size.y as f32;
        if h > 1.0 {
            h
        } else {
            1000.0
        }
    }

    fn model_radius(state: &AppState) -> f32 {
        Self::model_diagonal(state) * 0.5
    }

    /// AABB diagonal of the framed model, in meters. Fly speed is this / 60.
    fn model_diagonal(state: &AppState) -> f32 {
        let b = state
            .current_walk_analysis()
            .map(|analysis| analysis.building)
            .unwrap_or_else(|| framing_bounds(&state.scene));
        if aabb_is_empty(&b) {
            20.0
        } else {
            aabb_extent(&b).length().max(0.5)
        }
    }

    /// Write a camera and bump the revision, optionally keeping the preset
    /// badge (pan, dolly and recentre stay "Top Orthographic"; orbiting does
    /// not).
    fn commit(state: &mut AppState, view: usize, keep_preset: Option<PresetView>) {
        let vs = state.view_at_mut(view);
        vs.mark_camera_changed();
        vs.preset = keep_preset;
    }

    /// Start (or, with `dur <= 0`, immediately finish) a camera move.
    fn glide(
        &mut self,
        state: &mut AppState,
        view: usize,
        to: Camera,
        dur: f32,
        preset: Option<PresetView>,
    ) {
        self.inertia = None;
        let from = state.view_at(view).camera;
        if dur <= 1e-4 {
            self.anim = None;
            state.view_at_mut(view).camera = to;
            Self::commit(state, view, preset);
            return;
        }
        self.anim = Some(CameraAnim::new(from, to, dur, preset));
        // Show the first eased frame right away rather than a frame of
        // nothing: the move must feel like it started on the keypress.
        if let Some(anim) = &mut self.anim {
            let (cam, done) = anim.step(0.0);
            state.view_at_mut(view).camera = cam;
            if done {
                self.anim = None;
            }
        }
        Self::commit(state, view, preset);
    }

    fn apply_orbit(&self, cam: &mut Camera, style: OrbitStyle, dx: f32, dy: f32) {
        match style {
            OrbitStyle::Trackball => orbit::orbit_trackball(cam, dx, dy),
            OrbitStyle::Turntable => orbit::orbit_turntable(cam, dx, dy),
        }
    }

    /// Start a drag. An ORBIT additionally re-anchors the turntable on the
    /// surface under the pointer when there is one (`hit`), so the swing is
    /// about what the user is looking at rather than the model origin.
    fn begin_drag_at(&mut self, drag: Drag, mut camera: Camera, hit: Option<Vec3f>) {
        if drag == Drag::Orbit {
            if let Some(point) = hit {
                orbit::set_pivot(&mut camera, point);
            }
        }
        self.begin_drag(drag, camera);
    }

    fn begin_drag(&mut self, drag: Drag, camera: Camera) {
        self.drag = Some(drag);
        self.drag_origin = Some(camera);
        self.drag_total = DVec2::default();
        self.move_accum = DVec2::default();
        self.drag_vel = DVec2::default();
    }

    /// Apply one cumulative drag sample from the camera at pointer-down.
    /// No frame-time term belongs here: these are layout points, not a rate.
    fn apply_drag_total(&self, state: &mut AppState, view: usize, drag: Drag) {
        let mut cam = self
            .drag_origin
            .unwrap_or_else(|| state.view_at(view).camera);
        let dx = self.drag_total.x as f32;
        let dy = self.drag_total.y as f32;
        let preset = state.view_at(view).preset;
        let keep = match drag {
            Drag::Orbit => {
                self.break_auto_ortho(&mut cam, preset);
                self.apply_orbit(&mut cam, state.view_at(view).orbit_style, dx, dy);
                None
            }
            Drag::Pan => {
                orbit::pan(&mut cam, dx, dy, self.rect_height());
                preset
            }
            Drag::Dolly => {
                orbit::dolly(&mut cam, 1.01f32.powf(dy), None);
                preset
            }
        };
        state.view_at_mut(view).camera = cam;
        Self::commit(state, view, keep);
    }

    fn flush_drag_motion(&mut self, state: &mut AppState, view: usize) -> bool {
        let Some(drag) = self.drag else {
            return false;
        };
        let delta = std::mem::take(&mut self.move_accum);
        if delta.x == 0.0 && delta.y == 0.0 {
            return false;
        }
        self.drag_total += delta;
        self.apply_drag_total(state, view, drag);
        true
    }

    /// Orbiting by hand out of an axis view returns to perspective, exactly
    /// as Fab's auto-perspective preference does.
    fn break_auto_ortho(&self, cam: &mut Camera, preset: Option<PresetView>) {
        if self.auto_persp && preset.is_some() && cam.ortho {
            orbit::set_ortho(cam, false);
        }
    }

    // -----------------------------------------------------------------
    // mode handoff
    // -----------------------------------------------------------------

    /// Perform the handoff when the view's nav mode changed under us (the T
    /// toolbar's Walk button, `SetNavMode`, the keymap's Shift+` / W).
    fn sync_mode(&mut self, cx: &mut Cx, state: &mut AppState, view: usize) {
        let want = state.view_at(view).nav_mode;
        if want == self.mode {
            return;
        }
        if want == NavMode::Orbit {
            self.finish_first_person(cx, state, view, false);
        } else {
            self.enter_first_person(cx, state, view, want);
        }
        self.mode = want;
        cx.redraw_all();
    }

    /// Orbit → walk starts at the analysed main entrance; fly and the
    /// walk↔fly toggle preserve the current eye. Walk also switches the FOV
    /// to [`walk::WALK_FOV_DEG`] (restored on the way out).
    fn enter_first_person(
        &mut self,
        cx: &mut Cx,
        state: &mut AppState,
        view: usize,
        mode: NavMode,
    ) {
        self.anim = None;
        self.inertia = None;
        self.drag = None;
        self.drag_origin = None;
        self.move_accum = DVec2::default();
        self.walk_look_accum = DVec2::default();
        if state.tour.playing {
            state.tour.playing = false;
        }
        let diag = Self::model_diagonal(state);
        if mode == NavMode::Walk && self.mode == NavMode::Orbit {
            let pose = walk::walk_start_pose(state);
            let vs = state.view_at_mut(view);
            vs.camera.eye = pose.eye;
            vs.camera.target = pose.eye + pose.forward * 4.0;
            vs.camera.up = WORLD_UP;
            vs.camera.ortho = false;
        }
        let cam = state.view_at(view).camera;
        // Adopt after applying the entrance pose so yaw/pitch face inward.
        self.walk.adopt(&cam, mode, diag);
        if mode == NavMode::Walk {
            if self.walk.saved_fov.is_none() {
                self.walk.saved_fov = Some(cam.fov_y_deg);
            }
            state.view_at_mut(view).camera.fov_y_deg = walk::WALK_FOV_DEG;
        } else if let Some(fov) = self.walk.saved_fov.take() {
            state.view_at_mut(view).camera.fov_y_deg = fov;
        }
        walk::apply_look(&self.walk, state, view);
        Self::commit(state, view, None);
        self.push_walk_hud(cx, state, view);
    }

    /// Fly/walk → orbit: keep the eye and the aim, and put the pivot on
    /// whatever the camera is pointing at — so the very next MMB drag turns
    /// around the wall you were looking at, not around empty space.
    fn finish_first_person(
        &mut self,
        cx: &mut Cx,
        state: &mut AppState,
        view: usize,
        release_tool: bool,
    ) {
        self.walk.captured = false;
        self.walk.clear_keys();
        self.walk_look_accum = DVec2::default();
        let cam = state.view_at(view).camera;
        let dir = cam.forward();
        let radius = Self::model_radius(state);
        let fallback = (radius * 0.6).clamp(orbit::MIN_DISTANCE, orbit::MAX_DISTANCE);
        let dist = if state.scene.is_empty() || !dir.is_finite() {
            fallback
        } else {
            let ray = Ray::new(cam.eye, dir);
            walk::scene_raycast(state, &ray)
                .map(|h| h.t)
                .filter(|t| t.is_finite() && *t > 0.05)
                .unwrap_or(fallback)
                .clamp(orbit::MIN_DISTANCE, orbit::MAX_DISTANCE)
        };
        let restored_fov = self.walk.saved_fov.take();
        let vs = state.view_at_mut(view);
        vs.camera.target = vs.camera.eye + dir * dist;
        vs.camera.up = WORLD_UP;
        vs.camera.ortho = false;
        vs.camera.focus_distance = dist;
        if let Some(fov) = restored_fov {
            vs.camera.fov_y_deg = fov;
        }
        vs.nav_mode = NavMode::Orbit;
        vs.mark_camera_changed();
        self.mode = NavMode::Orbit;
        if release_tool && state.tool == Tool::Walk {
            cx.action(ShellAction::SetTool(Tool::Select));
        }
        if !self.last_walk_hud.is_empty() {
            self.last_walk_hud.clear();
            cx.action(ShellAction::StatusHint(String::new()));
        }
        cx.redraw_all();
    }

    /// Escape, from anywhere, at any time.
    fn escape(&mut self, cx: &mut Cx, state: &mut AppState, view: usize) -> InputResponse {
        let was_first_person = self.mode != NavMode::Orbit;
        self.walk.captured = false;
        self.anim = None;
        self.inertia = None;
        self.drag = None;
        self.drag_origin = None;
        self.move_accum = DVec2::default();
        self.walk_look_accum = DVec2::default();
        if was_first_person {
            self.finish_first_person(cx, state, view, true);
        }
        InputResponse {
            consumed: true,
            redraw: true,
            lock_pointer: Some(false),
            cursor: Some(MouseCursor::Default),
            wants_frames: false,
        }
    }

    fn toggle_first_person(&mut self, cx: &mut Cx, state: &mut AppState, view: usize) -> InputResponse {
        if self.mode == NavMode::Orbit {
            state.view_at_mut(view).nav_mode = NavMode::Fly;
            self.sync_mode(cx, state, view);
            InputResponse {
                consumed: true,
                redraw: true,
                cursor: Some(MouseCursor::Crosshair),
                wants_frames: true,
                ..Default::default()
            }
        } else {
            self.escape(cx, state, view)
        }
    }

    /// Walk-figure tool / W key: enter walk from orbit (or fly), leave with Esc.
    /// W is the walk toggle in orbit (`api.rs` `SetTool(Walk)` / `SetNavMode`);
    /// once walking, W is forward.
    fn enter_walk(&mut self, cx: &mut Cx, state: &mut AppState, view: usize) -> InputResponse {
        if self.mode == NavMode::Walk {
            return InputResponse::default();
        }
        state.view_at_mut(view).nav_mode = NavMode::Walk;
        if state.tool != Tool::Walk {
            cx.action(ShellAction::SetTool(Tool::Walk));
        }
        self.sync_mode(cx, state, view);
        InputResponse {
            consumed: true,
            redraw: true,
            cursor: Some(MouseCursor::Crosshair),
            wants_frames: true,
            ..Default::default()
        }
    }

    /// F: Walk ↔ Fly, keeping the eye and the look. From orbit, enter fly.
    fn toggle_fly_walk(&mut self, cx: &mut Cx, state: &mut AppState, view: usize) -> InputResponse {
        let next = match self.mode {
            NavMode::Orbit | NavMode::Walk => NavMode::Fly,
            NavMode::Fly => NavMode::Walk,
        };
        state.view_at_mut(view).nav_mode = next;
        if next == NavMode::Walk && state.tool != Tool::Walk {
            cx.action(ShellAction::SetTool(Tool::Walk));
        }
        self.sync_mode(cx, state, view);
        InputResponse {
            consumed: true,
            redraw: true,
            cursor: Some(if self.walk.captured {
                MouseCursor::Hidden
            } else {
                MouseCursor::Crosshair
            }),
            wants_frames: true,
            lock_pointer: if self.walk.captured && !walk::no_mouse_lock() {
                Some(true)
            } else {
                None
            },
            ..Default::default()
        }
    }

    fn push_walk_hud(&mut self, cx: &mut Cx, state: &AppState, view: usize) {
        let mode = state.view_at(view).nav_mode;
        if mode == NavMode::Orbit {
            return;
        }
        let line = walk::hud_line(&self.walk, mode);
        if line != self.last_walk_hud {
            self.last_walk_hud = line.clone();
            cx.action(ShellAction::StatusHint(line));
        }
    }

    /// Unlock the pointer without leaving the mode (window blur, hover-out).
    fn unlock_pointer(&mut self) -> InputResponse {
        self.walk.release_capture();
        InputResponse {
            consumed: false,
            redraw: true,
            lock_pointer: Some(false),
            cursor: Some(MouseCursor::Default),
            wants_frames: self.walk.active() || self.mode != NavMode::Orbit,
        }
    }

    // -----------------------------------------------------------------
    // orbit mode
    // -----------------------------------------------------------------

    fn handle_orbit(
        &mut self,
        cx: &mut Cx,
        input: &ViewportInput,
        state: &mut AppState,
    ) -> InputResponse {
        let view = input.view;
        match input.kind {
            ViewportInputKind::PointerDown {
                button,
                mods,
                tap_count,
                ..
            } => {
                self.inertia = None;
                self.pending = None;
                let middle = button == PointerButton::Middle;
                // Emulated three-button mouse: a bare LMB press may become an
                // orbit, Alt+LMB / RMB a pan, Ctrl+LMB a dolly — only once
                // the pointer travels; until then it is a click for the tool
                // or the context menu, so nothing is consumed here. Only in
                // the select tool: measure/section own their LMB.
                if !middle && tap_count < 2 && state.tool == Tool::Select {
                    let kind = if button == PointerButton::Secondary || mods.alt {
                        Drag::Pan
                    } else if mods.control {
                        Drag::Dolly
                    } else {
                        Drag::Orbit
                    };
                    self.pending = Some((kind, DVec2::default()));
                }
                if middle {
                    self.anim = None;
                    let drag = if mods.shift {
                        Drag::Pan
                    } else if mods.control {
                        Drag::Dolly
                    } else {
                        Drag::Orbit
                    };
                    self.begin_drag_at(
                        drag,
                        state.view_at(view).camera,
                        input.hit.map(|hit| hit.point),
                    );
                    return InputResponse {
                        consumed: true,
                        redraw: false,
                        cursor: Some(MouseCursor::Grabbing),
                        wants_frames: true,
                        ..Default::default()
                    };
                }
                // Double-click on a surface moves the pivot there without
                // turning the camera. Deliberately *not* consumed: the same
                // click also selects and reveals in the outliner.
                if button == PointerButton::Primary && tap_count >= 2 {
                    if let Some(hit) = input.hit {
                        let mut to = state.view_at(view).camera;
                        orbit::recenter(&mut to, hit.point);
                        to.focus_distance = to.distance();
                        let preset = state.view_at(view).preset;
                        self.glide(state, view, to, ANIM_SLOW, preset);
                        return InputResponse {
                            consumed: false,
                            redraw: true,
                            wants_frames: true,
                            ..Default::default()
                        };
                    }
                }
                InputResponse::default()
            }
            ViewportInputKind::PointerMove { delta, buttons, .. } => {
                let mut initial_delta = None;
                if self.drag.is_none() {
                    if let Some((kind, travel)) = self.pending.as_mut() {
                        if buttons == 0 {
                            // The release went elsewhere (a menu took it).
                            self.pending = None;
                            return InputResponse::default();
                        }
                        *travel += delta;
                        if travel.length() < DRAG_THRESHOLD_PT {
                            return InputResponse::default();
                        }
                        let kind = *kind;
                        initial_delta = Some(*travel);
                        self.pending = None;
                        self.anim = None;
                        self.begin_drag_at(
                            kind,
                            state.view_at(view).camera,
                            input.hit.map(|hit| hit.point),
                        );
                    }
                }
                if self.drag.is_none() {
                    return InputResponse::default();
                }
                // `initial_delta` already contains the threshold-crossing
                // event; established drags add this event here.
                self.move_accum += initial_delta.unwrap_or(delta);
                InputResponse {
                    consumed: true,
                    redraw: false,
                    wants_frames: true,
                    ..Default::default()
                }
            }
            ViewportInputKind::PointerUp { .. } => {
                self.pending = None;
                self.flush_drag_motion(state, view);
                let Some(drag) = self.drag.take() else {
                    return InputResponse::default();
                };
                let v = self.drag_vel;
                self.move_accum = DVec2::default();
                self.drag_total = DVec2::default();
                self.drag_origin = None;
                self.drag_vel = DVec2::default();
                let speed = v.length();
                if matches!(drag, Drag::Orbit | Drag::Pan) && speed > INERTIA_START {
                    let clamped = if speed > INERTIA_MAX {
                        v * (INERTIA_MAX / speed)
                    } else {
                        v
                    };
                    self.inertia = Some((drag, clamped));
                }
                InputResponse {
                    consumed: true,
                    redraw: true,
                    cursor: Some(MouseCursor::Default),
                    wants_frames: self.inertia.is_some(),
                    ..Default::default()
                }
            }
            ViewportInputKind::Scroll { delta, pos, mods } => {
                self.inertia = None;
                self.anim = None;
                let preset = state.view_at(view).preset;
                let rect_h = input.rect.size.y as f32;
                let mut cam = state.view_at(view).camera;
                if mods.shift {
                    orbit::pan(&mut cam, 0.0, -delta.y as f32, rect_h);
                } else if mods.control {
                    orbit::pan(&mut cam, -delta.y as f32, 0.0, rect_h);
                } else {
                    // Positive scroll is away from the model, everywhere in
                    // this repo. Dolly about a point on the cursor ray, which
                    // leaves that point on exactly the same pixel.
                    let steps = (delta.y as f32 / 80.0).clamp(-1.5, 1.5);
                    if steps.abs() < 1e-4 {
                        return InputResponse::default();
                    }
                    let anchor = match input.hit {
                        Some(hit) => hit.point,
                        None => {
                            let proj = ViewProjector::new(cam, input.rect);
                            orbit::anchor_on_pivot_plane(&cam, &proj.ray(pos))
                        }
                    };
                    orbit::dolly(&mut cam, 1.25f32.powf(steps), Some(anchor));
                }
                state.view_at_mut(view).camera = cam;
                Self::commit(state, view, preset);
                InputResponse::consumed()
            }
            ViewportInputKind::KeyDown { key, mods, repeat } => {
                self.handle_orbit_key(cx, state, view, key, mods, repeat)
            }
            _ => InputResponse::default(),
        }
    }

    fn handle_orbit_key(
        &mut self,
        cx: &mut Cx,
        state: &mut AppState,
        view: usize,
        key: KeyCode,
        mods: KeyModifiers,
        repeat: bool,
    ) -> InputResponse {
        if repeat {
            // Held preset keys would fight their own animation.
            return InputResponse::default();
        }
        if mods.logo {
            return InputResponse::default();
        }
        if let Some(preset) = self.preset_for_key(key, mods) {
            self.preset(cx, state, view, preset, true);
            return InputResponse {
                consumed: true,
                redraw: true,
                wants_frames: true,
                ..Default::default()
            };
        }
        if let Some((dx, dy)) = self.orbit_step_for_key(key) {
            let style = state.view_at(view).orbit_style;
            let preset = state.view_at(view).preset;
            let mut to = state.view_at(view).camera;
            self.break_auto_ortho(&mut to, preset);
            self.apply_orbit(&mut to, style, dx, dy);
            self.glide(state, view, to, ANIM_FAST, None);
            return InputResponse {
                consumed: true,
                redraw: true,
                wants_frames: true,
                ..Default::default()
            };
        }
        match key {
            KeyCode::Numpad5 | KeyCode::Key5 if self.numpad_key(key) => {
                let ortho = !state.view_at(view).camera.ortho;
                self.set_ortho(cx, state, view, ortho);
                InputResponse::consumed()
            }
            KeyCode::Home => {
                let b = framing_bounds(&state.scene);
                self.frame(cx, state, view, b, true);
                InputResponse {
                    consumed: true,
                    redraw: true,
                    wants_frames: true,
                    ..Default::default()
                }
            }
            KeyCode::Period | KeyCode::NumpadDecimal => {
                let b = state
                    .selection_bounds()
                    .unwrap_or_else(|| framing_bounds(&state.scene));
                self.frame(cx, state, view, b, true);
                InputResponse {
                    consumed: true,
                    redraw: true,
                    wants_frames: true,
                    ..Default::default()
                }
            }
            KeyCode::Backtick if mods.shift => self.toggle_first_person(cx, state, view),
            KeyCode::KeyW
                if !mods.control && !mods.alt && !mods.logo && !mods.shift =>
            {
                self.enter_walk(cx, state, view)
            }
            KeyCode::KeyF if !mods.control && !mods.alt && !mods.logo && !mods.shift => {
                self.toggle_fly_walk(cx, state, view)
            }
            _ => InputResponse::default(),
        }
    }

    /// True when this key counts as its numpad twin (top-row emulation).
    fn numpad_key(&self, key: KeyCode) -> bool {
        match key {
            KeyCode::Numpad0
            | KeyCode::Numpad1
            | KeyCode::Numpad2
            | KeyCode::Numpad3
            | KeyCode::Numpad4
            | KeyCode::Numpad5
            | KeyCode::Numpad6
            | KeyCode::Numpad7
            | KeyCode::Numpad8
            | KeyCode::Numpad9 => true,
            KeyCode::Key0
            | KeyCode::Key1
            | KeyCode::Key2
            | KeyCode::Key3
            | KeyCode::Key4
            | KeyCode::Key5
            | KeyCode::Key6
            | KeyCode::Key7
            | KeyCode::Key8
            | KeyCode::Key9 => self.emulate_numpad,
            _ => false,
        }
    }

    fn preset_for_key(&self, key: KeyCode, mods: KeyModifiers) -> Option<PresetView> {
        if !self.numpad_key(key) {
            return None;
        }
        let flip = mods.control;
        match key {
            KeyCode::Numpad1 | KeyCode::Key1 => Some(if flip {
                PresetView::Back
            } else {
                PresetView::Front
            }),
            KeyCode::Numpad3 | KeyCode::Key3 => Some(if flip {
                PresetView::Left
            } else {
                PresetView::Right
            }),
            KeyCode::Numpad7 | KeyCode::Key7 => Some(if flip {
                PresetView::Bottom
            } else {
                PresetView::Top
            }),
            KeyCode::Numpad9 | KeyCode::Key9 => Some(PresetView::Isometric),
            _ => None,
        }
    }

    fn orbit_step_for_key(&self, key: KeyCode) -> Option<(f32, f32)> {
        if !self.numpad_key(key) {
            return None;
        }
        let s = orbit_step_points();
        match key {
            KeyCode::Numpad4 | KeyCode::Key4 => Some((-s, 0.0)),
            KeyCode::Numpad6 | KeyCode::Key6 => Some((s, 0.0)),
            KeyCode::Numpad8 | KeyCode::Key8 => Some((0.0, -s)),
            KeyCode::Numpad2 | KeyCode::Key2 => Some((0.0, s)),
            _ => None,
        }
    }

    // -----------------------------------------------------------------
    // fly / walk mode
    // -----------------------------------------------------------------

    fn handle_first_person(
        &mut self,
        cx: &mut Cx,
        input: &ViewportInput,
        state: &mut AppState,
    ) -> InputResponse {
        let view = input.view;
        match input.kind {
            ViewportInputKind::PointerDown { button, mods, .. } => {
                self.walk.set_run(mods.shift);
                self.walk.set_slow(mods.control);
                if button == PointerButton::Secondary {
                    // Right-click releases capture without leaving the mode.
                    if self.walk.captured {
                        return self.unlock_pointer();
                    }
                    return InputResponse::default();
                }
                if button != PointerButton::Primary {
                    return InputResponse::default();
                }
                if !self.walk.captured {
                    self.walk.captured = true;
                    self.push_walk_hud(cx, state, view);
                    return InputResponse {
                        consumed: true,
                        redraw: true,
                        // A hidden window that grabs the OS pointer is
                        // unescapable; scripted sessions look by absolute
                        // motion instead. Viewport repins every MouseMove.
                        lock_pointer: if walk::no_mouse_lock() {
                            None
                        } else {
                            Some(true)
                        },
                        cursor: Some(MouseCursor::Hidden),
                        wants_frames: true,
                    };
                }
                InputResponse::consumed()
            }
            ViewportInputKind::PointerMove {
                delta,
                lock_delta,
                mods,
                ..
            } => {
                if !self.walk.captured {
                    // Uncaptured first person never turns the camera: hovering
                    // across the viewport must not swing the view.
                    return InputResponse::default();
                }
                self.walk.set_run(mods.shift);
                self.walk.set_slow(mods.control);
                // While the pointer is ours the event position is pinned and
                // the real motion arrives as `lock_delta`; without capture the
                // absolute delta is the truth.
                let d = if lock_delta.x != 0.0 || lock_delta.y != 0.0 {
                    lock_delta
                } else {
                    delta
                };
                if d.x == 0.0 && d.y == 0.0 {
                    return InputResponse::consumed();
                }
                self.walk_look_accum += d;
                InputResponse {
                    consumed: true,
                    redraw: false,
                    wants_frames: true,
                    ..Default::default()
                }
            }
            ViewportInputKind::PointerUp { button, .. } => {
                if button == PointerButton::Secondary && self.walk.captured {
                    return self.unlock_pointer();
                }
                if self.walk.captured {
                    InputResponse::consumed()
                } else {
                    InputResponse::default()
                }
            }
            ViewportInputKind::HoverOut => {
                // Window blur / leaving the pane must never leave the OS
                // pointer captured. Stay in walk; click recaptures.
                if self.walk.captured {
                    return self.unlock_pointer();
                }
                InputResponse::default()
            }
            ViewportInputKind::Scroll { delta, .. } => {
                // The walk wheel changes travel speed rather than distance.
                self.walk.nudge_speed(
                    (delta.y as f32 / -80.0).clamp(-1.5, 1.5),
                    state.view_at(view).nav_mode,
                );
                self.push_walk_hud(cx, state, view);
                InputResponse::consumed()
            }
            ViewportInputKind::KeyDown { key, mods, repeat } => {
                if key == KeyCode::Backtick && mods.shift && !repeat {
                    return self.toggle_first_person(cx, state, view);
                }
                if key == KeyCode::KeyF && !repeat && !mods.control && !mods.alt && !mods.logo {
                    return self.toggle_fly_walk(cx, state, view);
                }
                self.walk.set_run(mods.shift);
                self.walk.set_slow(mods.control);
                if self.walk.set_key(key, true) {
                    self.push_walk_hud(cx, state, view);
                    return InputResponse {
                        consumed: true,
                        redraw: true,
                        wants_frames: true,
                        ..Default::default()
                    };
                }
                InputResponse::default()
            }
            ViewportInputKind::KeyUp { key, mods } => {
                self.walk.set_run(mods.shift);
                self.walk.set_slow(mods.control);
                if self.walk.set_key(key, false) {
                    self.push_walk_hud(cx, state, view);
                    return InputResponse {
                        consumed: true,
                        redraw: true,
                        wants_frames: true,
                        ..Default::default()
                    };
                }
                InputResponse::default()
            }
            _ => InputResponse::default(),
        }
    }

    // -----------------------------------------------------------------
    // per-frame
    // -----------------------------------------------------------------

    fn tick(&mut self, cx: &mut Cx, state: &mut AppState, view: usize, dt: f32) -> InputResponse {
        let dt = dt.clamp(0.0, 0.1);
        let mut redraw = false;
        let mut wants = false;

        // Camera motion consumes physical pointer travel once. Frame time is
        // used only to estimate release velocity, never to scale the drag.
        if self.drag.is_some() {
            let accum = self.move_accum;
            if self.flush_drag_motion(state, view) {
                redraw = true;
            }
            if dt > 1e-4 {
                let inst = accum / dt as f64;
                // Exponential filtering is cadence-independent (0.5 blend
                // at 60 Hz, with the equivalent blend at other rates).
                let tau = (1.0f64 / 60.0) / std::f64::consts::LN_2;
                let blend = 1.0 - (-(dt as f64) / tau).exp();
                self.drag_vel += (inst - self.drag_vel) * blend;
            }
            wants = true;
        }

        if let Some(anim) = &mut self.anim {
            let (cam, done) = anim.step(dt);
            let preset = anim.preset;
            state.view_at_mut(view).camera = cam;
            Self::commit(state, view, preset);
            if done {
                self.anim = None;
            } else {
                wants = true;
            }
            redraw = true;
        } else if let Some((drag, vel)) = self.inertia {
            let step = vel * dt as f64;
            let style = state.view_at(view).orbit_style;
            let preset = state.view_at(view).preset;
            let rect_h = self.rect_height();
            let mut cam = state.view_at(view).camera;
            let keep = match drag {
                Drag::Orbit => {
                    self.break_auto_ortho(&mut cam, preset);
                    self.apply_orbit(&mut cam, style, step.x as f32, step.y as f32);
                    None
                }
                _ => {
                    orbit::pan(&mut cam, step.x as f32, step.y as f32, rect_h);
                    preset
                }
            };
            state.view_at_mut(view).camera = cam;
            Self::commit(state, view, keep);
            let decay = (-dt / INERTIA_TAU).exp() as f64;
            let next = vel * decay;
            if next.length() < INERTIA_MIN {
                self.inertia = None;
            } else {
                self.inertia = Some((drag, next));
                wants = true;
            }
            redraw = true;
        }

        if self.mode != NavMode::Orbit {
            let look = std::mem::take(&mut self.walk_look_accum);
            if look.x != 0.0 || look.y != 0.0 {
                self.walk.add_look_delta(look.x as f32, look.y as f32);
            }
            if walk::step(&mut self.walk, state, view, dt) {
                Self::commit(state, view, None);
                redraw = true;
            }
            self.push_walk_hud(cx, state, view);
            wants |= self.walk.active();
        }

        InputResponse {
            consumed: false,
            redraw,
            wants_frames: wants,
            ..Default::default()
        }
    }
}

impl NavController for Navigator {
    fn handle(&mut self, cx: &mut Cx, input: &ViewportInput, state: &mut AppState) -> InputResponse {
        self.last_rect = input.rect;
        if input.rect.size.y > 1.0 {
            self.last_aspect = (input.rect.size.x / input.rect.size.y) as f32;
        }
        let view = input.view;
        // Escape releases the pointer from anywhere, before any other rule
        // gets a chance to swallow it. This one is not negotiable.
        if let ViewportInputKind::KeyDown {
            key: KeyCode::Escape,
            ..
        } = input.kind
        {
            if self.mode != NavMode::Orbit
                || state.view_at(view).nav_mode != NavMode::Orbit
                || self.walk.captured
                || self.anim.is_some()
            {
                self.sync_mode(cx, state, view);
                return self.escape(cx, state, view);
            }
        }

        // Window-blur mailbox from the gizmo: unlock even if the viewport
        // did not get a HoverOut this frame.
        if walk::pending_capture_release(&mut self.walk.release_gen) && self.walk.captured {
            let mut resp = self.unlock_pointer();
            resp.consumed = matches!(
                input.kind,
                ViewportInputKind::PointerMove { .. } | ViewportInputKind::PointerDown { .. }
            );
            self.sync_mode(cx, state, view);
            if let ViewportInputKind::Frame { dt, .. } = input.kind {
                let tick = self.tick(cx, state, view, dt);
                resp.redraw |= tick.redraw;
                resp.wants_frames |= tick.wants_frames;
            }
            return resp;
        }

        let was_captured = self.walk.captured;
        self.sync_mode(cx, state, view);
        let released_by_mode = was_captured && !self.walk.captured;

        let mut resp = if let ViewportInputKind::Frame { dt, .. } = input.kind {
            self.tick(cx, state, view, dt)
        } else if self.mode != NavMode::Orbit {
            self.handle_first_person(cx, input, state)
        } else {
            self.handle_orbit(cx, input, state)
        };
        if released_by_mode {
            resp.lock_pointer = Some(false);
            resp.cursor = Some(MouseCursor::Default);
            resp.redraw = true;
        }
        resp
    }

    fn frame(
        &mut self,
        _cx: &mut Cx,
        state: &mut AppState,
        view: usize,
        bounds: Aabb,
        animate: bool,
    ) {
        if aabb_is_empty(&bounds) {
            return;
        }
        let aspect = self.aspect();
        let preset = state.view_at(view).preset;
        let mut to = state.view_at(view).camera;
        to.frame_bounds(&bounds, aspect);
        to.focus_distance = to.distance();
        self.glide(
            state,
            view,
            to,
            if animate { ANIM_SLOW } else { 0.0 },
            preset,
        );
    }

    fn preset(
        &mut self,
        _cx: &mut Cx,
        state: &mut AppState,
        view: usize,
        preset: PresetView,
        animate: bool,
    ) {
        let cam = state.view_at(view).camera;
        let was = state.view_at(view).preset;
        let mut to = orbit::preset_camera(&cam, preset, self.auto_persp);
        // Auto-perspective the other way round: leaving an axis view for a
        // free angle gives the perspective back, but only when it was the
        // axis view that took it away — a deliberate `5` is left alone.
        if self.auto_persp
            && preset == PresetView::Isometric
            && to.ortho
            && matches!(was, Some(p) if p != PresetView::Isometric)
        {
            orbit::set_ortho(&mut to, false);
        }
        self.glide(
            state,
            view,
            to,
            if animate { ANIM_SLOW } else { 0.0 },
            Some(preset),
        );
    }

    fn set_ortho(&mut self, _cx: &mut Cx, state: &mut AppState, view: usize, ortho: bool) {
        let preset = state.view_at(view).preset;
        let mut cam = state.view_at(view).camera;
        if cam.ortho == ortho {
            return;
        }
        orbit::set_ortho(&mut cam, ortho);
        state.view_at_mut(view).camera = cam;
        Self::commit(state, view, preset);
    }

    fn orbit_by(&mut self, _cx: &mut Cx, state: &mut AppState, view: usize, dx: f32, dy: f32) {
        self.anim = None;
        let style = state.view_at(view).orbit_style;
        let preset = state.view_at(view).preset;
        let mut cam = state.view_at(view).camera;
        self.break_auto_ortho(&mut cam, preset);
        self.apply_orbit(&mut cam, style, dx, dy);
        state.view_at_mut(view).camera = cam;
        Self::commit(state, view, None);
    }

    fn is_animating(&self) -> bool {
        self.anim.is_some() || self.inertia.is_some() || self.walk.active() || self.drag.is_some()
    }

    fn follow_track(
        &mut self,
        _cx: &mut Cx,
        state: &mut AppState,
        view: usize,
        track: &CameraTrack,
        t: f32,
    ) {
        // A user grab of the camera pauses the tour so we never fight the
        // playhead (lane G's contract: the camera moves only through us).
        if self.drag.is_some() || self.walk.captured || self.walk.keys != 0 {
            state.tour.playing = false;
            return;
        }
        self.anim = None;
        self.inertia = None;
        if let Some(key) = track::sample_c2(track, t) {
            let vs = state.view_at_mut(view);
            let focus = (key.look_at - key.pos).length();
            CameraTrack::apply(&key, &mut vs.camera);
            if focus > 0.01 {
                vs.camera.focus_distance = focus;
            }
            vs.mark_camera_changed();
        }
    }
}

script_mod! {
    use mod.prelude.fab.*
    #(gizmo::script_mod(vm))
}

/// Put the follow view's camera on the active track at the current playhead.
/// Scrubbing is exactly this, called from the action hook instead of a frame.
fn apply_tour_time(state: &mut AppState) -> bool {
    let Some(track) = state.tour.active_track().cloned() else {
        return false;
    };
    let t = state.tour.time;
    let Some(key) = track::sample_c2(&track, t) else {
        return false;
    };
    let view = state.tour.follow_view.min(state.views.len().saturating_sub(1));
    let vs = state.view_at_mut(view);
    let focus = (key.look_at - key.pos).length();
    CameraTrack::apply(&key, &mut vs.camera);
    if focus > 0.01 {
        vs.camera.focus_distance = focus;
    }
    vs.mark_camera_changed();
    state.sync_locked_cameras(view);
    true
}

/// Lane C's action hook, called from `App::dispatch`.
pub fn apply(_cx: &mut Cx, state: &mut AppState, action: &ShellAction) -> bool {
    match action {
        // Scrubbing and track selection must move the camera even while the
        // transport is stopped — that is what makes the timeline feel live.
        ShellAction::TourSeek(_) | ShellAction::TourSelect(_) | ShellAction::TourTracks(_) => {
            apply_tour_time(state)
        }
        ShellAction::TourPlay(false) => apply_tour_time(state),
        ShellAction::SetWorkspace(Workspace::Walkthrough) => {
            // The large realtime pane is always the walk driver; view 1 is
            // the locked rendered follower regardless of prior focus/state.
            state.active_view = 0;
            state.ui.lock_views = true;
            state.tool = Tool::Walk;
            state.view_at_mut(0).nav_mode = NavMode::Walk;
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_RECT: Rect = Rect {
        pos: DVec2 { x: 0.0, y: 0.0 },
        size: DVec2 { x: 960.0, y: 640.0 },
    };

    fn send(
        nav: &mut Navigator,
        cx: &mut Cx,
        state: &mut AppState,
        kind: ViewportInputKind,
    ) {
        nav.handle(
            cx,
            &ViewportInput {
                view: 0,
                rect: TEST_RECT,
                kind,
                hit: None,
            },
            state,
        );
    }

    fn camera_near(a: Camera, b: Camera) {
        let err = (a.eye - b.eye)
            .length()
            .max((a.target - b.target).length())
            .max((a.up - b.up).length());
        assert!(err < 2e-4, "camera mismatch {err}: {a:?} != {b:?}");
        assert_eq!(a.ortho, b.ortho);
        assert!((a.ortho_height - b.ortho_height).abs() < 2e-4);
    }

    fn drag_path(frame_stride: usize, drag: Drag, style: OrbitStyle) -> Camera {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut state = AppState::default();
        state.view_at_mut(0).orbit_style = style;
        let mut nav = Navigator::default();
        let mods = KeyModifiers {
            shift: drag == Drag::Pan,
            control: drag == Drag::Dolly,
            ..Default::default()
        };
        send(
            &mut nav,
            &mut cx,
            &mut state,
            ViewportInputKind::PointerDown {
                button: PointerButton::Middle,
                pos: dvec2(300.0, 240.0),
                mods,
                tap_count: 1,
            },
        );
        let dt = frame_stride as f32 / 120.0;
        for i in 0..120 {
            let delta = dvec2(0.45 + (i % 7) as f64 * 0.015, 0.18 - (i % 5) as f64 * 0.01);
            send(
                &mut nav,
                &mut cx,
                &mut state,
                ViewportInputKind::PointerMove {
                    pos: dvec2(300.0, 240.0),
                    delta,
                    lock_delta: DVec2::default(),
                    mods,
                    buttons: 4,
                },
            );
            if (i + 1) % frame_stride == 0 {
                send(
                    &mut nav,
                    &mut cx,
                    &mut state,
                    ViewportInputKind::Frame {
                        dt,
                        time: (i + 1) as f64 / 120.0,
                    },
                );
            }
        }
        state.view_at(0).camera
    }

    fn look_path(frame_stride: usize) -> Camera {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut state = AppState::default();
        state.view_at_mut(0).nav_mode = NavMode::Fly;
        let mut nav = Navigator::default();
        nav.mode = NavMode::Fly;
        nav.walk.adopt(&state.view_at(0).camera, NavMode::Fly, 20.0);
        nav.walk.captured = true;
        nav.walk.settling = false;
        let dt = frame_stride as f32 / 120.0;
        for i in 0..120 {
            let delta = dvec2(0.2 + (i % 3) as f64 * 0.01, -0.08);
            send(
                &mut nav,
                &mut cx,
                &mut state,
                ViewportInputKind::PointerMove {
                    pos: dvec2(480.0, 320.0),
                    delta: DVec2::default(),
                    lock_delta: delta,
                    mods: KeyModifiers::default(),
                    buttons: 1,
                },
            );
            if (i + 1) % frame_stride == 0 {
                send(
                    &mut nav,
                    &mut cx,
                    &mut state,
                    ViewportInputKind::Frame {
                        dt,
                        time: (i + 1) as f64 / 120.0,
                    },
                );
            }
        }
        // Compare the actual final aim, not a different point on the same
        // smoothing curve. One second is many LOOK_SMOOTH_TAU constants.
        for i in 0..(120 / frame_stride) {
            send(
                &mut nav,
                &mut cx,
                &mut state,
                ViewportInputKind::Frame {
                    dt,
                    time: 1.0 + (i + 1) as f64 * dt as f64,
                },
            );
        }
        state.view_at(0).camera
    }

    #[test]
    fn pointer_path_is_independent_of_frame_cadence() {
        for (drag, style) in [
            (Drag::Orbit, OrbitStyle::Turntable),
            (Drag::Orbit, OrbitStyle::Trackball),
            (Drag::Pan, OrbitStyle::Turntable),
            (Drag::Dolly, OrbitStyle::Turntable),
        ] {
            camera_near(drag_path(1, drag, style), drag_path(10, drag, style));
        }
        camera_near(look_path(1), look_path(10));
    }

    #[test]
    fn numpad_emulation_maps_the_top_row() {
        let nav = Navigator::default();
        let plain = KeyModifiers::default();
        let ctrl = KeyModifiers {
            control: true,
            ..Default::default()
        };
        assert_eq!(
            nav.preset_for_key(KeyCode::Key1, plain),
            Some(PresetView::Front)
        );
        assert_eq!(
            nav.preset_for_key(KeyCode::Numpad1, plain),
            Some(PresetView::Front)
        );
        assert_eq!(
            nav.preset_for_key(KeyCode::Key1, ctrl),
            Some(PresetView::Back)
        );
        assert_eq!(
            nav.preset_for_key(KeyCode::Key7, plain),
            Some(PresetView::Top)
        );
        assert_eq!(
            nav.preset_for_key(KeyCode::Key3, ctrl),
            Some(PresetView::Left)
        );
        assert_eq!(nav.preset_for_key(KeyCode::KeyW, plain), None);
        // W is the walk toggle in orbit, not a preset.
        // 2/4/6/8 step, they are not presets.
        assert_eq!(nav.preset_for_key(KeyCode::Key2, plain), None);
        assert!(nav.orbit_step_for_key(KeyCode::Key2).is_some());
        assert!(nav.orbit_step_for_key(KeyCode::Key1).is_none());
    }

    #[test]
    fn orbit_step_is_fifteen_degrees() {
        let mut cam = Camera::default();
        cam.target = vec3(0.0, 0.0, 0.0);
        cam.eye = vec3(10.0, 0.0, 0.0);
        cam.up = WORLD_UP;
        let before = cam.forward();
        let (dx, dy) = Navigator::default()
            .orbit_step_for_key(KeyCode::Numpad6)
            .unwrap();
        orbit::orbit_turntable(&mut cam, dx, dy);
        let angle = before.dot(cam.forward()).clamp(-1.0, 1.0).acos().to_degrees();
        assert!((angle - 15.0).abs() < 0.05, "stepped {angle} degrees");
    }

    #[test]
    fn tool_walk_action_reaches_the_navigator_mode() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut state = AppState::default();
        let mut nav = Navigator::default();
        let entry = makepad_fab_tour::WalkEntryPose {
            eye: vec3(2.0, -3.0, walk::EYE_HEIGHT),
            forward: vec3(0.0, 1.0, 0.0),
        };
        state.walk_analysis = Some(std::sync::Arc::new(WalkSceneAnalysis::for_nav_test(
            &state.scene,
            Aabb {
                min: vec3(-5.0, -5.0, 0.0),
                max: vec3(5.0, 5.0, 4.0),
            },
            entry,
        )));
        state.walk_analysis_revision = state.scene_revision;
        assert_eq!(nav.mode, NavMode::Orbit);
        assert!(state.apply_core(&ShellAction::SetTool(Tool::Walk)));
        assert_eq!(state.view().nav_mode, NavMode::Walk);

        let input = ViewportInput {
            view: 0,
            rect: Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(800.0, 600.0),
            },
            kind: ViewportInputKind::Frame { dt: 0.0, time: 0.0 },
            hit: None,
        };
        nav.handle(&mut cx, &input, &mut state);
        assert_eq!(nav.mode, NavMode::Walk);
        assert_eq!(state.view().nav_mode, NavMode::Walk);
        assert!((state.view().camera.eye - entry.eye).length() < 1e-5);
        assert!(state.view().camera.forward().dot(entry.forward) > 0.999);
    }

    #[test]
    fn blur_and_escape_unlock_the_pointer() {
        let mut nav = Navigator::default();
        nav.mode = NavMode::Walk;
        nav.walk.captured = true;
        nav.walk.keys = walk::K_FWD;
        let resp = nav.unlock_pointer();
        assert!(!nav.walk.captured);
        assert_eq!(resp.lock_pointer, Some(false));
        assert_eq!(nav.walk.keys, 0);

        nav.walk.captured = true;
        let mut seen = nav.walk.release_gen;
        walk::request_capture_release();
        assert!(walk::pending_capture_release(&mut seen));
        assert!(!walk::pending_capture_release(&mut seen));
    }
}
