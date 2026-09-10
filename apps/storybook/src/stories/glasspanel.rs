//! The glass panel stories: the default material, a custom tint and edge,
//! and the scene blur flag, ported from the widget zoo — and the floating
//! surface, which is that material with a frame you can take hold of.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.GlassPanelOverview = StoryPage{
        H4{text: "Default"}
        GlassPanel{
            width: 260
            height: 140
            padding: theme.mspace_3
            flow: Down
            spacing: theme.space_1
            Label{text: "GlassPanel"}
            Label{text: "Default material"}
        }

        Hr{}
        H4{text: "Custom tint + edge"}
        GlassPanel{
            width: 260
            height: 140
            padding: theme.mspace_3
            flow: Down
            spacing: theme.space_1
            draw_bg +: {
                tint_color: #6af
                tint_alpha: 0.23
                border_color: #9cf
                border_alpha: 0.6
                border_width: 1.5
                corner_radius: 18.0
                specular_strength: 0.5
                noise_strength: 0.025
            }
            Label{text: "Cool tint"}
            Label{text: "Rounded edge + stronger specular"}
        }

        Hr{}
        H4{text: "Scene blur path flag"}
        GlassPanel{
            width: 260
            height: 140
            padding: theme.mspace_3
            flow: Down
            spacing: theme.space_1
            draw_bg +: {
                tint_color: #fff
                tint_alpha: 0.18
                use_scene_blur: 1.0
                blur_amount: 0.75
                specular_strength: 0.4
                noise_strength: 0.04
            }
            Label{text: "use_scene_blur: true"}
            Label{text: "blur_amount: 0.75"}
        }
    }

    // The ground the lens has something to bend. A plain View cannot hand a
    // caller a slot, so the scene is written where it is used rather than
    // wrapped in a template with a hole in it.
    let Ground = View{
        width: Fill
        height: Fill
        show_bg: true
        draw_bg +: {
            pixel: fn() {
                let p = self.pos
                let a = vec3(0.05, 0.12, 0.38)
                let b = vec3(0.62, 0.16, 0.42)
                let c = vec3(0.05, 0.42, 0.45)
                let m = mix(a, b, p.x)
                let n = mix(c, b, p.y)
                return vec4(mix(m, n, 0.45 + 0.35 * sin(p.x * 6.0 + p.y * 3.0)), 1.0)
            }
        }
    }

    mod.stories.GlassFloatingSurfaceOverview = StoryPage{
        StoryNote{text: "The panel's material with a frame you can take hold of. Drag the body to move it; drag any edge or any corner to size it. It floats over the window in window points, so it can be dragged over anything on the page — including the rest of this catalogue."}

        StoryHeading{text: "Put one up"}
        StoryRow{
            show_surface := Button{text: "Show the surface"}
            hide_surface := Button{text: "Hide it"}
            surface_state := Label{text: "hidden"}
        }

        StoryHeading{text: "Something worth bending"}
        StoryNote{text: "Drag the surface across this band and back onto the plain page. The lens reads the scene BEHIND the surface, and it is re-read on every frame of a move or a resize — which is the whole difficulty of making a glass surface resizable, and the reason a naive one carries a picture of where the drag started."}
        StoryRow{
            View{
                width: Fill
                // Fixed, not Fit: the ground is height Fill, and a Fill child
                // of a Fit parent is given nothing at all.
                height: 200.
                flow: Overlay
                Ground{}
            }
        }

        StoryHeading{text: "What it reports"}
        StoryNote{text: "Every drag reports the frame it is passing through and the frame it settles on. The size is held between min_size and max_size, and the surface is pulled back inside the window every draw — one whose frame had gone past an edge could never be dragged back."}
        StoryRow{
            surface_frame := Label{text: "not moved yet"}
        }

        floater := mod.widgets.glass.FloatingSurface{
            pos: vec2(470., 300.)
            size: vec2(300., 200.)
            min_size: vec2(180., 120.)
            max_size: vec2(560., 460.)
            content +: {
                body +: {
                    mod.widgets.glass.H2{text: "A pane of glass"}
                    mod.widgets.glass.Body{text: "The body moves it. The edges and the corners size it, and the corner mark says where the surest grip is."}
                    mod.widgets.glass.Body{text: "The button below still takes its own press: the move is only claimed by what nothing inside wanted."}
                    inside := Button{text: "A control on the glass"}
                }
            }
        }
    }
}

fn glass_floating_surface_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let surface = root.glass_floating_surface(cx, ids!(floater));
    if root.button(cx, ids!(show_surface)).clicked(actions) {
        surface.open(cx);
    }
    if root.button(cx, ids!(hide_surface)).clicked(actions) {
        surface.close(cx);
    }
    if root.button(cx, ids!(inside)).clicked(actions) {
        let n = crate::stories::bump(live_id!(glass_surface_inside));
        root.button(cx, ids!(inside))
            .set_text(cx, &format!("pressed {n} times"));
    }
    // `framed` answers whichever kind of drag reported this pass, which is
    // what a readout wants: the page cares about the frame, not about which
    // handle is doing it.
    if let Some((pos, size)) = surface.framed(actions) {
        root.label(cx, ids!(surface_frame)).set_text(
            cx,
            &format!("at {:.0},{:.0} sized {:.0}x{:.0}", pos.x, pos.y, size.x, size.y),
        );
    }
    let state = if surface.is_open() { "shown" } else { "hidden" };
    let label = root.label(cx, ids!(surface_state));
    if label.text() != state {
        label.set_text(cx, state);
    }
}

pub const STORIES: &[Story] = &[
    Story {
        key: "containers/glasspanel/overview",
        category: "Containers",
        component: "GlassPanel",
        also: &[],
        name: "Overview",
        dsl: "GlassPanelOverview",
        added: "2026-03-12",
        tags: &["ported"],
        doc: "# GlassPanel\n\nReusable glass-style panel with tint, border, specular and noise controls. `use_scene_blur` and `blur_amount` are exposed for M2 visuals.",
        subject: "",
        feature: None,
        controls: &[],
        on_actions: None,
    },
    Story {
        key: "containers/glassfloatingsurface/overview",
        category: "Containers",
        component: "GlassFloatingSurface",
        also: &["FloatingSurface"],
        name: "Floating surface",
        dsl: "GlassFloatingSurfaceOverview",
        added: "2026-09-10",
        tags: &["new", "layout"],
        doc: "# GlassFloatingSurface

The panel's material with a frame you can take hold of: a sheet of glass that floats over the page, **moved by its body and sized by its edges and its corners**.

## The two gestures are claimed at opposite ends

The frame claims a press *before* the surface's own contents see it. The grab band is a few points wide and lies over whatever was put against the edge, so a resize that begins by dropping a caret into a field is a resize you then have to undo. `grab_margin` sets its width, and it reaches both ways from the edge — the surface is a rounded rectangle, and a band that stopped at the boundary would ask for a press on glass that is not there.

The move is claimed the other way round, *after* the contents have had their turn. The press's handled mark is read once before the contents run and once after, and the only handler between those two reads is the surface's own subtree — so a press a button on the glass took is told apart from a press on a control sitting **behind** the glass, which had marked the event handled long before the surface was reached at all. The first leaves the surface where it is; the second still moves it. Only what nothing inside wanted moves the surface, which is why there is no title bar: a strip of chrome across the top is exactly what this family exists not to draw.

## Why a resizable glass surface is harder than a resizable panel

The lens reads the scene **behind** the surface, from a capture the window takes only when a draw asks for one — and the ask happens inside the surface's own draw. A repaint that reuses the drawn content asks for nothing, the window stops capturing, and the glass goes on showing the page as it was when the drag began. So every frame of a move or a resize redraws the surface's whole subtree. Drag it across the coloured band on this page and the band bends through it as it crosses; that is the capture being re-taken, not a still picture being carried around.

## The frame

`min_size` and `max_size` hold the size; a zero side of `max_size` means the window is the only ceiling. Dragging a near edge past the floor pins **that** edge and leaves the far one where it was — clamping the position instead would shove the far edge along, quietly moving a surface you were only trying to make smaller. Every draw pulls the whole frame back inside the window, because a surface whose frame had gone past an edge could never be dragged back.

`Sizing` and `Moving` arrive on every frame of a drag and `Placed` once, when the hand comes off; `framed` answers whichever of the three came this pass. Position is reported, never stored — a caller that wants it back next run keeps the value itself.",
        subject: "floater",
        feature: None,
        controls: &[
            Control {
                label: "Grab margin",
                target: "floater",
                kind: ControlKind::Number { prop: "grab_margin", min: 2.0, max: 24.0, step: 1.0, default: 8.0 },
            },
            Control {
                label: "Corner mark",
                target: "floater",
                kind: ControlKind::Number { prop: "grip_size", min: 0.0, max: 48.0, step: 1.0, default: 24.0 },
            },
            Control { label: "Movable", target: "floater", kind: ControlKind::Bool { prop: "movable", default: true } },
            Control { label: "Resizable", target: "floater", kind: ControlKind::Bool { prop: "resizable", default: true } },
        ],
        on_actions: Some(glass_floating_surface_actions),
    },
];
