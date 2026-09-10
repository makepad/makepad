//! The glass panel stories: the material over something worth bending, and
//! the floating surface, which is that material with a frame you can take
//! hold of.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.GlassPanelOverview = StoryPage{
        StoryNote{text: "A sheet of glass: it samples the window behind itself, blurs it, and bends that through its own rounded edge. Everything it looks like comes from what is behind it, which is why it is shown here over a ground and not over the page."}

        StoryHeading{text: "Default"}
        StoryNote{text: "Drive the controls panel and watch this one. Every number the material takes is a shader input on draw_bg, so the panel opposite edits it the same way a page would."}
        GlassStage{
            height: 210.
            body +: {
                subject := GlassPanel{
                    width: 260
                    height: 140
                    padding: theme.mspace_3
                    flow: Down
                    spacing: theme.space_1
                    Label{text: "GlassPanel"}
                    Label{text: "Default material"}
                }
            }
        }

        StoryHeading{text: "Custom tint + edge"}
        StoryNote{text: "The same widget with seven values merged into draw_bg. Note the corner: the SDF box draws a visual radius of twice the number, so corner_radius 18 is a 36pt corner."}
        GlassStage{
            height: 210.
            body +: {
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
            }
        }

        StoryHeading{text: "The same panel over one colour"}
        StoryNote{text: "Identical declaration, nothing behind it to bend. This is what the family looks like on a plain page, and it is worth seeing before choosing it for one: the refraction has nothing to work with and the panel is left as a border, a specular rim and a shadow."}
        flat := FlatStage{
            height: 210.
            body +: {
                GlassPanel{
                    width: 260
                    height: 140
                    padding: theme.mspace_3
                    flow: Down
                    spacing: theme.space_1
                    Label{text: "GlassPanel"}
                    Label{text: "nothing behind it"}
                }
            }
        }
    }

    mod.stories.GlassFloatingSurfaceOverview = StoryPage{
        StoryNote{text: "The panel's material with a frame you can take hold of. Drag the body to move it; drag any edge or any corner to size it. It floats over the window in window points, so it is already up when this page opens."}

        StoryHeading{text: "Take it down and put it back"}
        StoryNote{text: "The page declares shown: true, so the surface is here on arrival; these two buttons are open() and close(). A rebuild - a theme switch, a reload - brings back what the page declared, not what you last pressed."}
        StoryRow{
            show_surface := Button{text: "Show it"}
            hide_surface := Button{text: "Hide it"}
            surface_state := Label{text: "shown"}
        }

        StoryHeading{text: "Something worth bending"}
        StoryNote{text: "Drag the surface across this band and back onto the plain page. The lens reads the scene BEHIND the surface, and it is re-read on every frame of a move or a resize - which is the whole difficulty of making a glass surface resizable, and the reason a naive one carries a picture of where the drag started."}
        GlassStage{height: 200.}

        StoryHeading{text: "Whose press it is"}
        StoryNote{text: "The surface can be dragged over the navigator and the splitter bars, but a drag cannot be STARTED on the part of it that lies over them: those panes are asked about a press before the pane this page lives in, and a press belongs to whoever answered it first. Park the sheet over the file tree and the cost is plain - every attempt to pick it back up opens a different story instead, and takes this page down with it. Start the drag over the page and it carries on anywhere."}

        StoryHeading{text: "What it reports"}
        StoryNote{text: "Every drag reports the frame it is passing through and the frame it settles on. The size is held between min_size and max_size, and the surface is pulled back inside the window every draw - one whose frame had gone past an edge could never be dragged back."}
        StoryRow{
            surface_frame := Label{text: "not moved yet"}
        }

        floater := mod.widgets.glass.FloatingSurface{
            shown: true
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
        doc: "# GlassPanel

A sheet of glass: it samples the window behind itself through a chain of mip textures, blurs it, and bends that through its own rounded edge.

**It draws what is behind it, so something has to be behind it.** Over a flat page the refraction has nothing to work with and the panel is left as a border, a specular rim and a shadow — which is why every demo here stands on a coloured ground, and why the last one deliberately does not. The ground also has to reach PAST the glass: the rim samples along the outward normal by up to `lensing_strength` points, so a ground that stops at the edge hands the bend the page background instead.

## The knobs

`blur_level` is a level in that pyramid, 0 to 6, not a radius in pixels. Level 0 is the captured scene at full size and level N reads a texture 1/2^N of the window; values between two levels blend. The deep levels are carried back up to a 1/8-resolution floor before the glass reads them, so no sample ever comes from a texture coarser than eight device pixels per texel.

`lensing_effect` (0..1) is how much of the edge bend to apply, `lensing_strength` how far it bends, and `lensing_width` how wide the band at the edge is — capped in the shader at about a third of the shorter side, so a small surface does not become all edge. `diffraction_strength` splits red, green and blue to slightly different lookups, which is the colour fringe at the rim.

`corner_radius` is **half the visual radius**: the SDF box draws a corner twice the number you give it.

The rest are surface: `tint_color` with `tint_alpha`, `border_color`/`border_alpha`/`border_width`, `specular_strength` for the rim light, `noise_strength` for the de-banding grain, and `shadow_color`/`shadow_alpha`/`shadow_radius`/`shadow_offset` for what it casts.

`fallback_color` is the face it shows when there is no scene to sample — before the first capture, or with `MAKEPAD_NO_GAUSS=1` set. It is mixed from the theme's background and text colours, so it stays visible as a slab whichever theme is running.

## What it costs

A window with any glass on it renders its own body to a texture and builds a blur pyramid from it every frame: thirteen extra passes — one capture, six downsamples, six that carry the deep levels back up — plus a full-screen composite, and up to twenty-four texture fetches per glass pixel. Full-window supersampling is switched off for that window while it runs. A window with no glass in it never captures at all. Reach for this family where it earns that, not by default.

`MAKEPAD_NO_GAUSS=1` switches the capture off and shows every surface's fallback face, which is the way to check that face. `MAKEPAD_GAUSS_FAST=1` is a pass-count probe and **not** a like-for-like picture above `blur_level` 3: it stops the chain early, so the levels this family uses are either un-smoothed or never rendered at all.",
        subject: "subject",
        feature: None,
        controls: &[
            Control { label: "Blur level",    target: "subject", kind: ControlKind::Number { prop: "draw_bg.blur_level",           min: 0.,  max: 6.,   step: 0.1,   default: 5.2 } },
            Control { label: "Refraction",    target: "subject", kind: ControlKind::Number { prop: "draw_bg.lensing_effect",       min: 0.,  max: 1.,   step: 0.02,  default: 0.94 } },
            Control { label: "Bend",          target: "subject", kind: ControlKind::Number { prop: "draw_bg.lensing_strength",     min: 0.,  max: 60.,  step: 1.,    default: 28. } },
            Control { label: "Edge band",     target: "subject", kind: ControlKind::Number { prop: "draw_bg.lensing_width",        min: 1.,  max: 48.,  step: 1.,    default: 20. } },
            Control { label: "Colour split",  target: "subject", kind: ControlKind::Number { prop: "draw_bg.diffraction_strength", min: 0.,  max: 12.,  step: 0.2,   default: 4.4 } },
            Control { label: "Corner radius", target: "subject", kind: ControlKind::Number { prop: "draw_bg.corner_radius",        min: 0.,  max: 32.,  step: 0.5,   default: 10. } },
            Control { label: "Tint",          target: "subject", kind: ControlKind::Color  { prop: "draw_bg.tint_color",           default: 0xF8FBFFFF } },
            Control { label: "Tint amount",   target: "subject", kind: ControlKind::Number { prop: "draw_bg.tint_alpha",           min: 0.,  max: 0.30, step: 0.002, default: 0.006 } },
            Control { label: "Border",        target: "subject", kind: ControlKind::Number { prop: "draw_bg.border_alpha",         min: 0.,  max: 1.,   step: 0.02,  default: 0.72 } },
            Control { label: "Border width",  target: "subject", kind: ControlKind::Number { prop: "draw_bg.border_width",         min: 0.,  max: 4.,   step: 0.25,  default: 1. } },
            Control { label: "Specular",      target: "subject", kind: ControlKind::Number { prop: "draw_bg.specular_strength",    min: 0.,  max: 1.,   step: 0.02,  default: 0.22 } },
            Control { label: "Grain",         target: "subject", kind: ControlKind::Number { prop: "draw_bg.noise_strength",       min: 0.,  max: 0.08, step: 0.002, default: 0.004 } },
            Control { label: "Shadow",        target: "subject", kind: ControlKind::Number { prop: "draw_bg.shadow_alpha",         min: 0.,  max: 1.,   step: 0.05,  default: 1.0 } },
            Control { label: "Shadow radius", target: "subject", kind: ControlKind::Number { prop: "draw_bg.shadow_radius",        min: 0.,  max: 32.,  step: 1.,    default: 13. } },
        ],
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

**A press another widget already answered is not the surface's.** It floats in window points, so it can lie over panes that are asked about a press before the pane it lives in — a navigator, a splitter bar. The press belongs to whoever took it first, so a drag cannot be *started* on the part of the sheet that overlaps one; once a drag has begun it carries on anywhere, because nothing else holds the pointer. On this shell the cost is not just a drag that fails to start: the navigator answers that press by opening a different story, which tears down the page the surface is standing on. A sheet that let a press through would be answering with a widget nobody can see, so it claims a press on itself whether or not `movable` and `resizable` are on, and whichever button made it. Every press it answers also takes the key focus, so a search box elsewhere stops eating keys the moment the sheet is worked. Closing asks where the caret *is*, not who put it there: one anywhere the surface draws goes back to whatever the surface took it from, or is simply dropped where the surface never took it; one that has since moved off the surface is left where it is, which on this page is the button that just asked it to close.

**The pointer and the wheel go with the press.** A hover over the sheet is claimed at the same two places a press is, so a field under the glass does not light up and offer a caret for a click it will never get; and a wheel is stopped once the surface's own body has had it, so the page underneath does not slide out from under a sheet that stays put.

**The grab band costs a ring of page.** `grab_margin` reaches outward as well as inward, so presses that far outside the painted glass belong to the surface, with nothing drawn there to explain it. The outward half is not optional — a rounded rectangle's corners are unpainted, and a band that stopped at the boundary would ask for a press on glass that is not there — but it is a reason to keep the number small. And the gesture is mouse only: it is written against the mouse events rather than `Event::TouchUpdate`, so this widget is desktop only until a touch path is written.

## Why a resizable glass surface is harder than a resizable panel

The lens reads the scene **behind** the surface, from a capture the window takes only when a draw asks for one — and the ask happens inside the surface's own draw. A repaint that reuses the drawn content asks for nothing, the window stops capturing, and the glass goes on showing the page as it was when the drag began. So every frame of a move or a resize redraws the surface's whole subtree. Drag it across the coloured band on this page and the band bends through it as it crosses; that is the capture being re-taken, not a still picture being carried around.

The same reasoning is why coming up is not just a redraw. The window decides whether to capture before any widget draws, so a surface that appears — `open()`, a `place()` while it is up, or a page writing `shown: true` — announces itself first; without that its first painted frame carries the flat fallback face and the window then redraws the whole UI to correct itself.

## The frame

`shown` is a live property, so a page can put one up by declaring it — and a theme switch or a live edit brings back what the page declared, not what you last pressed. `min_size` and `max_size` hold the size; a zero side of `max_size` means the window is the only ceiling. Dragging a near edge past the floor pins **that** edge and leaves the far one where it was — clamping the position instead would shove the far edge along, quietly moving a surface you were only trying to make smaller. Every draw pulls the whole frame back inside the window, because a surface whose frame had gone past an edge could never be dragged back.

`Sizing` and `Moving` arrive on every frame of a drag and `Placed` once, when the hand comes off; `framed` answers whichever of the three came this pass. Position is reported, never stored — a caller that wants it back next run keeps the value itself.",
        subject: "floater",
        feature: None,
        controls: &[
            // `grab_margin` caps at 16, which is also the library's own
            // exposed maximum: the band reaches that far OUTSIDE the glass
            // as well as inside, and a ring of page that wide answering to a
            // surface, with nothing drawn there to say so, is more than a
            // reader will forgive.
            Control { label: "Grab margin", target: "floater", kind: ControlKind::Number { prop: "grab_margin", min: 2.,  max: 16., step: 1., default: 8. } },
            Control { label: "Corner mark", target: "floater", kind: ControlKind::Number { prop: "grip_size",   min: 0.,  max: 48., step: 1., default: 24. } },
            Control { label: "Movable",     target: "floater", kind: ControlKind::Bool   { prop: "movable",   default: true } },
            Control { label: "Resizable",   target: "floater", kind: ControlKind::Bool   { prop: "resizable", default: true } },
            // The material, reached on the surface's own glass panel.
            // `content` is one id segment, found by the same subtree search
            // that already reaches `inside` three levels deeper.
            Control { label: "Blur level",  target: "content", kind: ControlKind::Number { prop: "draw_bg.blur_level",   min: 0., max: 6.,   step: 0.1,   default: 5.2 } },
            Control { label: "Tint amount", target: "content", kind: ControlKind::Number { prop: "draw_bg.tint_alpha",   min: 0., max: 0.30, step: 0.002, default: 0.006 } },
            Control { label: "Shadow",      target: "content", kind: ControlKind::Number { prop: "draw_bg.shadow_alpha", min: 0., max: 1.,   step: 0.05,  default: 1.0 } },
        ],
        on_actions: Some(glass_floating_surface_actions),
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// A `Cx` with this crate's theme, the shared page templates and this
    /// file's own two stories registered, and nothing else: a failure here
    /// is this page's, not some other story's.
    fn shell() -> Cx {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::theme::widgets_script_mod(vm);
            crate::shell::script_mod(vm);
            self::script_mod(vm);
            // Registering a template compiles nothing; making an instance
            // out of one does. Clearing here keeps any other module's
            // complaint out of these tests' answers.
            let _ = makepad_platform::shader_error::take();
        });
        cx
    }

    fn build(cx: &mut Cx, dsl: &str) -> WidgetRef {
        cx.with_vm(|vm| {
            let stories = vm.module(id!(stories));
            let value = vm.bx.heap.value(stories, LiveId::from_str(dsl).into(), NoTrap);
            assert!(value.as_object().is_some(), "no template {dsl}");
            WidgetRef::script_from_value(vm, value)
        })
    }

    /// Both pages are built from the DSL, which the Rust compiler never
    /// reads: a mistake in a `script_mod!` block shows up only in a running
    /// app's log, and a shader that fails to compile is not an error
    /// anywhere — the draw is skipped and the widget paints nothing. These
    /// two pages stand on a shared stage that did not exist before, and
    /// they reach for a widget three levels inside another one, so building
    /// both and asking for every id the controls panel addresses is what
    /// turns either mistake into a failed build.
    #[test]
    fn both_pages_build_and_their_subjects_can_be_reached() {
        let mut cx = shell();
        for story in STORIES {
            let page = build(&mut cx, story.dsl);
            assert!(!page.is_empty(), "{} built no widget", story.key);
            assert_eq!(
                makepad_platform::shader_error::take(),
                None,
                "{}: a draw shader failed to compile",
                story.key
            );
            // Every id the controls panel writes to has to be findable, or
            // the row moves a slider that reaches nothing. `content` is the
            // one worth the trouble: it is three levels inside another
            // widget, reached by the same single-segment subtree search.
            for target in std::iter::once(story.subject)
                .chain(story.controls.iter().map(|c| c.target))
                .filter(|t| !t.is_empty())
            {
                assert!(
                    !page.widget(&cx, &[LiveId::from_str(target)]).is_empty(),
                    "{}: no widget at {}",
                    story.key,
                    target
                );
            }
        }
    }

    /// The flat stage REPLACES the coloured stage's two layers rather than
    /// adding two more.
    ///
    /// This is the trap the shared stage exists inside: a derived object
    /// copies its prototype's children and then appends its own, so an
    /// anonymous child in `FlatStage` would leave the coloured ground
    /// switched on under a fourth layer and paint an opaque fifth one over
    /// the demo — in an Overlay flow the last layer is on top. Nothing warns
    /// about it and it compiles perfectly, so the layers are counted here.
    #[test]
    fn the_flat_stage_overrides_its_layers_instead_of_adding_more() {
        let mut cx = shell();
        let page = build(&mut cx, "GlassPanelOverview");
        let stage = page.widget(&cx, &[live_id!(flat)]);
        assert!(!stage.is_empty(), "the flat stage is named on the page");
        let view = stage.borrow::<View>().expect("a stage is a View");
        let layers: Vec<LiveId> = view.children.iter().map(|(id, _)| *id).collect();
        assert_eq!(
            layers,
            vec![live_id!(ground), live_id!(scrim), live_id!(body)],
            "three layers, in paint order, and the demo slot last"
        );
        let ground = view
            .children
            .iter()
            .find(|(id, _)| *id == live_id!(ground))
            .map(|(_, w)| w.clone())
            .unwrap();
        assert!(
            !ground.borrow::<View>().expect("the ground is a View").visible,
            "the flat stage switched the coloured ground off"
        );
    }

    /// A number control's default has to be a value its own slider can
    /// hold. A default outside the range is silently clamped on the first
    /// drag, so the row jumps the moment it is touched — and the panel
    /// fills each row exactly once, so nothing ever puts it back.
    ///
    /// Scoped to this file's own stories. The same invariant is worth
    /// pinning across the whole catalogue, but that test belongs next to
    /// the registry these records are declared against.
    #[test]
    fn number_controls_can_hold_their_own_defaults() {
        for story in STORIES {
            for control in story.controls {
                if let ControlKind::Number { prop, min, max, step, default } = &control.kind {
                    assert!(min < max, "{} {}: min {} is not below max {}", story.key, prop, min, max);
                    assert!(*step > 0., "{} {}: step {} does not move", story.key, prop, step);
                    assert!(
                        min <= default && default <= max,
                        "{} {}: default {} is outside {}..{}",
                        story.key, prop, default, min, max
                    );
                }
            }
        }
    }
}
