//! Lane G owns this directory (plus `libs/fab_tour/`).
//!
//! Tours: automated cinematic fly/walk-throughs. `libs/fab_tour` analyses
//! the `SceneSnapshot` (storeys, free-space voxels with clearance, room
//! graph through doors, points of interest) and generates shots (exterior
//! drone reveal, approach, interior walkthrough, drone fly-through, orbit,
//! storey reveal) as C2-smooth constant-speed `api::CameraTrack`s, then
//! flies every track in QA and asserts no clipping / limit violations.
//!
//! This module is the app side: the Tours panel (generate, list, timeline
//! scrub/play, follow in the realtime viewport, "Render animation" → lane F
//! via `ShellAction::TourRenderAnimation`), the playback clock (advances
//! `state.tour.time` on the viewport's `Frame`, see `NavController::
//! follow_track`), and the worker that runs generation off the UI thread and
//! posts `ShellAction::TourTracks`.
//!
//! Skeleton state: placeholder panel with a Generate button that produces one
//! orbit track from the scene bounds so playback plumbing can be exercised.

use crate::api::*;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.fab.*
    use mod.widgets.*

    mod.widgets.FabToursPanelBase = #(FabToursPanel::register_widget(vm))
    mod.widgets.FabToursPanel = set_type_default() do mod.widgets.FabToursPanelBase{
        width: Fill
        height: Fill
        flow: Down
        show_bg: true
        draw_bg +: {
            color: fab.color_editor
        }
        header := View{
            width: Fill
            height: fab.header_height
            flow: Right
            align: Align{x: 0.0 y: 0.5}
            padding: Inset{left: 6 right: 6 top: 0 bottom: 0}
            spacing: 6
            show_bg: true
            draw_bg +: {
                color: fab.color_header
            }
            FabTip{ text: "Choose editor"
                editor_type := FabDropdownButton{ label +: { text: "Tours" } }
            }
            FabTip{ text: "Generate camera tours"
                generate := FabButton{ text: "Generate" }
            }
            FabTip{ text: "Play active tour"
                play := FabButton{ text: "Play" }
            }
            FabTip{ text: "Render tour animation"
                render := FabButton{ text: "Render animation" }
            }
            Filler{}
            status := FabLabelDim{ text: "" }
        }
        body := View{
            width: Fill
            height: Fill
            padding: fab.pad_3
            info := FabLabelDim{ text: "No tours yet — Generate analyses the model and proposes shots." }
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct FabToursPanel {
    #[deref]
    view: View,
}

impl Widget for FabToursPanel {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        if let Event::Actions(actions) = event {
            if self.view.button(cx, ids!(header.generate)).clicked(actions) {
                cx.action(ShellAction::TourGenerate);
            }
            if self.view.button(cx, ids!(header.play)).clicked(actions) {
                if let Some(state) = scope.data.get_mut::<AppState>() {
                    cx.action(ShellAction::TourPlay(!state.tour.playing));
                }
            }
            if self.view.button(cx, ids!(header.render)).clicked(actions) {
                cx.action(ShellAction::TourRenderAnimation);
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if let Some(state) = scope.data.get_mut::<AppState>() {
            let t = &state.tour;
            let status = match t.active_track() {
                Some(track) => format!(
                    "{} · {:.1} / {:.1} s{}",
                    track.name,
                    t.time,
                    track.duration(),
                    if t.playing { " (playing)" } else { "" }
                ),
                None => t.status.clone(),
            };
            self.view.label(cx, ids!(header.status)).set_text(cx, &status);
            self.view
                .button(cx, ids!(header.play))
                .set_text(cx, if t.playing { "Pause" } else { "Play" });
            let info = if t.tracks.is_empty() {
                "No tours yet — Generate analyses the model and proposes shots.".to_string()
            } else {
                t.tracks
                    .iter()
                    .enumerate()
                    .map(|(i, tr)| {
                        format!(
                            "{}{}  ({}, {:.0} s)",
                            if t.active == Some(i) { "[*] " } else { "[ ] " },
                            tr.name,
                            tr.kind,
                            tr.duration()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("    ")
            };
            self.view.label(cx, ids!(body.info)).set_text(cx, &info);
        }
        self.view.draw_walk(cx, scope, walk)
    }
}

/// Skeleton generator: one orbit around the scene bounds, 20 s, 30 keys/s.
/// Lane G replaces this with `libs/fab_tour` on a worker thread.
fn placeholder_orbit(state: &AppState) -> Vec<CameraTrack> {
    if aabb_is_empty(&state.scene.bounds) {
        return Vec::new();
    }
    let c = aabb_center(&state.scene.bounds);
    let r = aabb_radius(&state.scene.bounds).max(1.0);
    let n = 600;
    let dur = 20.0f32;
    let keys = (0..=n)
        .map(|i| {
            let f = i as f32 / n as f32;
            let a = f * std::f32::consts::TAU;
            CameraKey {
                t: f * dur,
                pos: vec3(c.x + a.cos() * r * 2.2, c.y + a.sin() * r * 2.2, c.z + r * 0.9),
                look_at: c,
                up: vec3(0.0, 0.0, 1.0),
                fov_y_deg: 40.0,
            }
        })
        .collect();
    vec![CameraTrack {
        name: "Orbit".into(),
        kind: "Drone orbit".into(),
        keys,
        fps: 30.0,
    }]
}

/// Lane G's action hook, called from `App::dispatch`.
pub fn apply(_cx: &mut Cx, state: &mut AppState, action: &ShellAction) -> bool {
    match action {
        ShellAction::TourGenerate => {
            let tracks = placeholder_orbit(state);
            state.tour.tracks = tracks;
            state.tour.active = if state.tour.tracks.is_empty() { None } else { Some(0) };
            state.tour.generating = false;
            state.tour.time = 0.0;
            state.tour.status = if state.tour.tracks.is_empty() {
                "Load a model first".into()
            } else {
                String::new()
            };
            true
        }
        ShellAction::TourRenderAnimation => {
            state.ui.status_message = "Render animation: lane F".into();
            true
        }
        _ => false,
    }
}
