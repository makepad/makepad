//! Lane E owns this directory (plus `../sheets/`).
//!
//! `ToolSet` — the active tool's input handler, one per `FabViewport`, after
//! the navigator in the input chain (`api::ToolController`). It turns pointer
//! and key input into `ShellAction`s; every result it produces lives in
//! `AppState` (`measurements`, `scene_state.section`, `scene_state.explode`,
//! `sun`). In-flight state — the half-placed measurement, a handle drag, the
//! day scrub — lives in [`session`], which the overlay and the panel read too.
//!
//! The parts:
//!
//! | file | what |
//! |---|---|
//! | `snap.rs` | vertex / midpoint / edge / face snapping (lane A's `query::snap` when it lands) |
//! | `measure.rs` | distance / area / angle, units, the 1 mm accuracy gate |
//! | `section.rs` | section planes + section box, handles, animate-in |
//! | `isolate.rs` | H / Shift+H / Alt+H, solo, per-storey and per-layer isolate |
//! | `explode.rs` | `element_offset` — the one explode rule, shared with lane B |
//! | `sun_study.rs` | date/time/latitude, day scrub, sun path |
//! | `info.rs` | the element info card |
//! | `overlay.rs` | `FabToolOverlay` — everything drawn over the viewport |
//! | `panel.rs` | `FabToolPanel` — the N sidebar's Tool tab |

pub mod explode;
pub mod info;
pub mod isolate;
pub mod measure;
pub mod overlay;
pub mod panel;
pub mod section;
pub mod session;
pub mod snap;
pub mod sun_study;

use crate::api::*;
use makepad_widgets::*;

/// Pointer travel (points) beyond which a click becomes a drag.
const DRAG_SLOP: f64 = 4.0;

struct Down {
    pos: DVec2,
    tap_count: u32,
}

#[derive(Default)]
pub struct ToolSet {
    down: Option<Down>,
}

impl ToolSet {
    /// Snap the cursor, remembering it for the overlay's glyph.
    fn snap_at(&self, state: &AppState, input: &ViewportInput, pos: DVec2) -> Option<SnapHit> {
        let proj = ViewProjector::new(state.view_at(input.view).camera, input.rect);
        let opts = state.snap;
        snap::snap(&state.scene, &proj, pos, &opts, &|id| state.is_visible(id))
    }

    /// Elements whose projected centre falls inside the rubber-band rect.
    fn box_select(&self, state: &AppState, input: &ViewportInput, a: DVec2, b: DVec2) -> Vec<ElementId> {
        let proj = ViewProjector::new(state.view_at(input.view).camera, input.rect);
        let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
        let (y0, y1) = (a.y.min(b.y), a.y.max(b.y));
        state
            .scene
            .elements
            .iter()
            .filter(|e| e.has_geometry() && state.is_visible(e.id))
            .filter(|e| {
                let c = aabb_center(&e.bounds) + explode::element_offset(&state.scene, &state.scene_state.explode, e.id);
                match proj.project(c) {
                    Some(s) => s.x >= x0 && s.x <= x1 && s.y >= y0 && s.y <= y1,
                    None => false,
                }
            })
            .map(|e| e.id)
            .collect()
    }

    /// A section handle within grab range of the pointer.
    fn handle_at(&self, state: &AppState, input: &ViewportInput, pos: DVec2) -> Option<session::SectionHandle> {
        let proj = ViewProjector::new(state.view_at(input.view).camera, input.rect);
        let bounds = state.scene.bounds;
        let mut best: Option<(f64, session::SectionHandle)> = None;
        for (handle, world, _) in section::handles(&state.scene_state.section, &bounds) {
            if let Some(s) = proj.project(world) {
                let d = (s - pos).length();
                if d < 14.0 && best.map_or(true, |b| d < b.0) {
                    best = Some((d, handle));
                }
            }
        }
        best.map(|b| b.1)
    }

    fn start_handle_drag(&self, state: &AppState, input: &ViewportInput, pos: DVec2, handle: session::SectionHandle) -> bool {
        let proj = ViewProjector::new(state.view_at(input.view).camera, input.rect);
        let bounds = state.scene.bounds;
        let Some((world, axis)) = section::handle_anchor(&state.scene_state.section, &bounds, handle) else {
            return false;
        };
        let Some(value) = section::handle_value(&state.scene_state.section, handle) else {
            return false;
        };
        // Screen direction of one metre along the handle axis.
        let (Some(a), Some(b)) = (proj.project(world), proj.project(world + axis)) else {
            return false;
        };
        let d = b - a;
        let len = d.length();
        if len < 1e-6 {
            return false;
        }
        session::with(|s| {
            s.section_drag = Some(session::SectionDrag {
                handle,
                start: pos,
                start_value: value,
                axis_screen: d / len,
                points_per_meter: len,
            });
        });
        true
    }

    fn commit_measure(&self, cx: &mut Cx, state: &mut AppState, points: Vec<Vec3f>, kind: MeasureKind) {
        let units = session::with(|s| s.units(&state.scene.units));
        if let Some(m) = measure::commit(kind, points, &units) {
            let hint = format!("{}: {}", measure::kind_label(kind), m.label);
            session::with(|s| s.hint = hint);
            cx.action(ShellAction::AddMeasurement(m));
        }
    }

    /// Place one measurement point; commit when the shape is complete.
    fn place_point(&self, cx: &mut Cx, state: &mut AppState, hit: SnapHit) {
        let kind = match state.tool {
            Tool::Measure(k) => k,
            _ => return,
        };
        let ready = session::with(|s| {
            if s.measure.kind != kind {
                s.measure.clear();
                s.measure.kind = kind;
            }
            // Closing an area loop by clicking the first point again.
            if kind == MeasureKind::Area && s.measure.points.len() >= 3 {
                if (s.measure.points[0] - hit.point).length() < 1e-3 {
                    return Some(std::mem::take(&mut s.measure.points));
                }
            }
            s.measure.points.push(hit.point);
            s.measure.snaps.push(hit.kind);
            s.hint = format!(
                "{} point {} · {}",
                measure::kind_label(kind),
                s.measure.points.len(),
                snap::label(hit.kind)
            );
            if s.measure.points.len() >= measure::needed(kind) {
                s.measure.snaps.clear();
                Some(std::mem::take(&mut s.measure.points))
            } else {
                None
            }
        });
        if let Some(points) = ready {
            self.commit_measure(cx, state, points, kind);
        }
    }

    /// Enter / right-click while an area loop is open.
    fn close_loop(&self, cx: &mut Cx, state: &mut AppState) -> bool {
        let kind = match state.tool {
            Tool::Measure(k) => k,
            _ => return false,
        };
        let points = session::with(|s| {
            if s.measure.points.len() >= 3 {
                s.measure.snaps.clear();
                Some(std::mem::take(&mut s.measure.points))
            } else {
                None
            }
        });
        match points {
            Some(p) => {
                self.commit_measure(cx, state, p, kind);
                true
            }
            None => false,
        }
    }

    fn place_section_from_hit(&self, cx: &mut Cx, state: &AppState, hit: &RayHit) {
        let target = section::single(section::plane_from_hit(hit));
        let bounds = state.scene.bounds;
        let (anim, first) = section::animate_to(&state.scene_state.section, target, &bounds);
        session::with(|s| {
            s.section_anim = Some(anim);
            s.hint = "Section from face — drag the handle to offset, F to flip".into();
        });
        cx.action(ShellAction::SetSection(first));
    }
}

impl ToolController for ToolSet {
    fn handle(&mut self, cx: &mut Cx, input: &ViewportInput, state: &mut AppState) -> InputResponse {
        match input.kind {
            ViewportInputKind::PointerDown {
                button: PointerButton::Primary,
                pos,
                mods,
                tap_count,
            } => {
                let _ = mods;
                self.down = Some(Down { pos, tap_count });
                if state.tool == Tool::Section {
                    if let Some(handle) = self.handle_at(state, input, pos) {
                        if self.start_handle_drag(state, input, pos, handle) {
                            return InputResponse::consumed();
                        }
                    }
                }
                InputResponse {
                    consumed: true,
                    ..Default::default()
                }
            }

            ViewportInputKind::PointerDown {
                button: PointerButton::Secondary,
                ..
            } => {
                // Right-click closes an open area loop; otherwise it belongs
                // to whoever owns context menus.
                if self.close_loop(cx, state) {
                    InputResponse::consumed()
                } else {
                    InputResponse::default()
                }
            }

            ViewportInputKind::PointerMove { pos, buttons, mods, .. } => {
                let mut resp = InputResponse::default();
                // 1. a section handle drag
                let drag = session::with(|s| s.section_drag);
                if let Some(drag) = drag {
                    if buttons & 1 != 0 {
                        let d = pos - drag.start;
                        let along = (d.x * drag.axis_screen.x + d.y * drag.axis_screen.y) / drag.points_per_meter.max(1e-6);
                        let mut value = drag.start_value + along as f32;
                        if mods.control {
                            value = (value * 10.0).round() * 0.1; // 100 mm snap
                        }
                        let mut section = state.scene_state.section.clone();
                        let limits = state.scene.bounds;
                        section::apply_handle(&mut section, drag.handle, value, &limits);
                        session::with(|s| {
                            s.hint = format!("Section offset {:.3} m", value);
                            s.section_anim = None;
                        });
                        cx.action(ShellAction::SetSection(section));
                        return InputResponse::consumed();
                    }
                    session::with(|s| s.section_drag = None);
                }

                // 2. box select rubber band
                if state.tool == Tool::Select && buttons & 1 != 0 {
                    if let Some(down) = &self.down {
                        if (pos - down.pos).length() > DRAG_SLOP {
                            let start = down.pos;
                            session::with(|s| s.box_select = Some((start, pos)));
                            return InputResponse::consumed();
                        }
                    }
                }

                // 3. snapping preview under the cursor
                match state.tool {
                    Tool::Measure(kind) => {
                        let hit = self.snap_at(state, input, pos);
                        session::with(|s| {
                            s.snap_cursor = hit;
                            s.measure.kind = kind;
                            s.measure.preview = hit;
                        });
                        resp.redraw = true;
                    }
                    Tool::Section => {
                        let hover = self.handle_at(state, input, pos);
                        let changed = session::with(|s| {
                            let c = s.section_hover != hover;
                            s.section_hover = hover;
                            c
                        });
                        if changed {
                            resp.redraw = true;
                            if hover.is_some() {
                                resp.cursor = Some(MouseCursor::Grab);
                            }
                        }
                    }
                    _ => {
                        let had = session::with(|s| {
                            let had = s.snap_cursor.is_some() || s.measure.preview.is_some();
                            s.snap_cursor = None;
                            s.measure.preview = None;
                            had
                        });
                        resp.redraw = had;
                    }
                }
                resp
            }

            ViewportInputKind::PointerUp {
                button: PointerButton::Primary,
                pos,
                mods,
            } => {
                let dragging_handle = session::with(|s| s.section_drag.take()).is_some();
                if dragging_handle {
                    return InputResponse::consumed();
                }
                let band = session::with(|s| s.box_select.take());
                if let Some((a, _)) = band {
                    let ids = self.box_select(state, input, a, pos);
                    self.down = None;
                    if ids.is_empty() && !mods.shift {
                        cx.action(ShellAction::ClearSelection);
                    } else if mods.shift {
                        for id in ids {
                            cx.action(ShellAction::SelectAdd(id));
                        }
                    } else {
                        cx.action(ShellAction::SelectSet(ids));
                    }
                    return InputResponse::consumed();
                }

                let Some(down) = self.down.take() else {
                    return InputResponse::default();
                };
                if (pos - down.pos).length() > DRAG_SLOP {
                    return InputResponse::default();
                }

                match state.tool {
                    Tool::Select | Tool::Walk => {
                        match input.hit {
                            Some(hit) => {
                                if mods.shift {
                                    cx.action(ShellAction::SelectToggle(hit.element));
                                } else {
                                    cx.action(ShellAction::SelectOnly(hit.element));
                                }
                                if down.tap_count >= 2 || session::with(|s| s.info_card) {
                                    info::focus_properties(cx, hit.element);
                                }
                            }
                            None => {
                                if !mods.shift {
                                    cx.action(ShellAction::ClearSelection);
                                }
                            }
                        }
                        InputResponse::consumed()
                    }
                    Tool::Measure(_) => {
                        if let Some(hit) = self.snap_at(state, input, pos) {
                            self.place_point(cx, state, hit);
                        }
                        InputResponse::consumed()
                    }
                    Tool::Section => {
                        match input.hit {
                            Some(hit) => self.place_section_from_hit(cx, state, &hit),
                            None => session::with(|s| {
                                s.hint = "Click a face to cut, or pick an axis in the N panel".into()
                            }),
                        }
                        InputResponse::consumed()
                    }
                }
            }

            ViewportInputKind::HoverOut => {
                let had = session::with(|s| {
                    let had = s.snap_cursor.is_some() || s.measure.preview.is_some();
                    s.snap_cursor = None;
                    s.measure.preview = None;
                    had
                });
                InputResponse {
                    redraw: had,
                    ..Default::default()
                }
            }

            ViewportInputKind::KeyDown { key, mods, repeat } if !repeat => {
                match key {
                    KeyCode::KeyH if mods.alt => isolate::unhide_all(cx),
                    KeyCode::KeyH if mods.shift => isolate::isolate_selected(cx),
                    KeyCode::KeyH => isolate::hide_selected(cx),
                    KeyCode::Slash => {
                        let hint = isolate::toggle_solo(cx, state);
                        session::with(|s| s.hint = hint.into());
                    }
                    KeyCode::KeyI => {
                        let on = session::with(|s| {
                            s.info_card = !s.info_card;
                            s.info_card
                        });
                        if on {
                            if let Some(id) = state.scene_state.selection.active {
                                info::focus_properties(cx, id);
                            }
                        }
                        session::with(|s| {
                            s.hint = if on { "Element info on" } else { "Element info off" }.into()
                        });
                    }
                    KeyCode::KeyM => {
                        // Cycle the measure kinds; from any other tool, start
                        // with distance.
                        let next = match state.tool {
                            Tool::Measure(MeasureKind::Distance) => MeasureKind::Area,
                            Tool::Measure(MeasureKind::Area) => MeasureKind::Angle,
                            Tool::Measure(MeasureKind::Angle) => MeasureKind::Distance,
                            _ => MeasureKind::Distance,
                        };
                        session::with(|s| s.measure.clear());
                        cx.action(ShellAction::SetTool(Tool::Measure(next)));
                    }
                    KeyCode::KeyC if mods.alt => {
                        session::with(|s| s.section_anim = None);
                        cx.action(ShellAction::SetSection(SectionState::default()));
                    }
                    KeyCode::KeyC if mods.shift => {
                        let b = section::box_from_bounds(&state.scene.bounds, 0.12);
                        let target = section::boxed(b);
                        let bounds = state.scene.bounds;
                        let (anim, first) = section::animate_to(&state.scene_state.section, target, &bounds);
                        session::with(|s| s.section_anim = Some(anim));
                        cx.action(ShellAction::SetSection(first));
                        cx.action(ShellAction::SetTool(Tool::Section));
                    }
                    KeyCode::KeyC => cx.action(ShellAction::SetTool(Tool::Section)),
                    KeyCode::KeyF if state.tool == Tool::Section => {
                        let mut section = state.scene_state.section.clone();
                        for p in &mut section.planes {
                            *p = section::flip(p);
                        }
                        cx.action(ShellAction::SetSection(section));
                    }
                    KeyCode::Backspace | KeyCode::Delete => {
                        let had = session::with(|s| {
                            let had = !s.measure.is_empty();
                            s.measure.undo();
                            had
                        });
                        if !had {
                            return InputResponse::default();
                        }
                    }
                    KeyCode::ReturnKey | KeyCode::NumpadEnter => {
                        if !self.close_loop(cx, state) {
                            return InputResponse::default();
                        }
                    }
                    KeyCode::Escape => {
                        let had = session::with(|s| {
                            let had = !s.measure.is_empty();
                            s.measure.clear();
                            s.box_select = None;
                            s.section_drag = None;
                            had
                        });
                        if !had {
                            cx.action(ShellAction::SetTool(Tool::Select));
                        }
                    }
                    _ => return InputResponse::default(),
                }
                InputResponse::consumed()
            }

            _ => InputResponse::default(),
        }
    }
}

/// Widget registration for lane E: the panel first (the overlay places it).
pub fn script_mod(vm: &mut ScriptVm) {
    panel::script_mod(vm);
    overlay::script_mod(vm);
}

/// Lane E's action hook, called from `App::dispatch`. This is the action
/// application path, so writing `ui.status_hint` here is the app doing it, not
/// a widget reaching around the contract.
pub fn apply(_cx: &mut Cx, state: &mut AppState, action: &ShellAction) -> bool {
    match action {
        ShellAction::Loaded(_) => {
            session::with(|s| s.reset_for_scene());
            true
        }
        ShellAction::SetTool(tool) => {
            session::with(|s| {
                s.measure.clear();
                s.box_select = None;
                s.section_drag = None;
                if let Tool::Measure(k) = tool {
                    s.measure.kind = *k;
                }
                s.hint.clear();
            });
            state.ui.status_hint = match tool {
                Tool::Select => {
                    "LMB Select · Shift+LMB Extend · LMB drag Box select · MMB Orbit".into()
                }
                Tool::Measure(k) => measure::hint(*k).to_string(),
                Tool::Section => {
                    "LMB Face to cut · Drag handle Offset · F Flip · Shift+C Box · Alt+C Clear".into()
                }
                Tool::Walk => "WASD Walk · Mouse Look · Esc Release".into(),
            };
            if let Tool::Section = tool {
                state.ui.sidebar_open = true;
            }
            true
        }
        ShellAction::ClearMeasurements => {
            session::with(|s| s.measure.clear());
            true
        }
        _ => false,
    }
}
