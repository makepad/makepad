//! The surfaces story: a container that eases to its content's height, and
//! one that blurs whatever is behind it.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let Row = View{
        width: Fill height: Fit
        padding: theme.mspace_1
        show_bg: true
        draw_bg +: {color: theme.color_surface_container_high}
        Label{text: "a row"}
    }

    mod.stories.SurfacesOverview = StoryPage{
        StoryNote{text: "Two containers that do something to themselves rather than to their children. One changes its own height gradually instead of at once; the other blurs what is behind it."}

        StoryHeading{text: "A height that eases"}
        StoryNote{text: "Add and remove rows. The plain view on the left jumps to its new height on the next frame; the rubber view on the right eases to it. Nothing about the rows differs — the smoothing is the container's."}
        StoryRow{
            add_row := Button{text: "add a row"}
            drop_row := Button{text: "remove one"}
            row_count := Label{text: "3 rows"}
        }
        StoryRow{
            View{
                width: 300. height: Fit flow: Down spacing: theme.space_1
                Label{text: "plain View" draw_text +: {color: theme.color_text_meta}}
                plain := View{
                    width: Fill height: Fit flow: Down spacing: 2.
                    padding: theme.mspace_1
                    show_bg: true
                    draw_bg +: {color: theme.color_surface_container_low}
                    Row{} Row{} Row{}
                    extra_a := Row{visible: false}
                    extra_b := Row{visible: false}
                    extra_c := Row{visible: false}
                }
            }
            View{
                width: 300. height: Fit flow: Down spacing: theme.space_1
                Label{text: "RubberView" draw_text +: {color: theme.color_text_meta}}
                rubber := RubberView{
                    width: Fill height: Fit
                    smoothing: 0.25
                    View{
                        width: Fill height: Fit flow: Down spacing: 2.
                        padding: theme.mspace_1
                        show_bg: true
                        draw_bg +: {color: theme.color_surface_container_low}
                        Row{} Row{} Row{}
                        rextra_a := Row{visible: false}
                        rextra_b := Row{visible: false}
                        rextra_c := Row{visible: false}
                    }
                }
            }
        }

        StoryHeading{text: "A surface that blurs what is behind it"}
        StoryNote{text: "GaussRoundedView samples the scene behind itself through a chain of mip textures and blurs it. It opens its own overlay and asks the window for that capture on every draw, and announces itself when it is built, so an ordinary window captures for it — there is nothing a host has to set up."}
        StoryRow{
            View{
                width: Fill height: 140.
                flow: Overlay
                Label{
                    text: "content behind the surface"
                    margin: theme.mspace_3
                    draw_text +: {color: theme.color_text_meta}
                }
                blurred := GaussRoundedView{
                    width: 240. height: 90.
                    margin: theme.mspace_3
                }
            }
        }
    }
}

fn surfaces_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let extras = [
        (ids!(extra_a), ids!(rextra_a)),
        (ids!(extra_b), ids!(rextra_b)),
        (ids!(extra_c), ids!(rextra_c)),
    ];
    let shown = extras
        .iter()
        .filter(|(a, _)| root.view(cx, *a).visible())
        .count();

    let mut want = shown;
    if root.button(cx, ids!(add_row)).clicked(actions) && shown < extras.len() {
        want = shown + 1;
    }
    if root.button(cx, ids!(drop_row)).clicked(actions) && shown > 0 {
        want = shown - 1;
    }
    if want == shown {
        return;
    }
    // Both columns get exactly the same change, so the only difference on
    // screen is how each container answers it.
    for (i, (plain, rubber)) in extras.iter().enumerate() {
        let on = i < want;
        root.view(cx, *plain).set_visible(cx, on);
        root.view(cx, *rubber).set_visible(cx, on);
    }
    root.label(cx, ids!(row_count))
        .set_text(cx, &format!("{} rows", 3 + want));
    root.view(cx, ids!(plain)).redraw(cx);
    root.widget(cx, ids!(rubber)).redraw(cx);
}

pub const STORIES: &[Story] = &[Story {
    key: "containers/surfaces/overview",
    category: "Containers",
    component: "Surfaces",
    also: &["RubberView", "GaussRoundedView"],
    name: "Overview",
    dsl: "SurfacesOverview",
    added: "2025-05-06",
    tags: &["layout"],
    doc: "# RubberView and GaussRoundedView

Two containers that do something to *themselves* rather than to their children.

## RubberView

It measures its content on every draw and eases its own height toward that number instead of snapping to it — exponential smoothing, frame-rate corrected, with `smoothing` as the rate. The first draw snaps, so nothing animates in from nowhere; after that a change of content is a movement rather than a jump.

Put one around anything whose height changes while a person is looking at it: a list that gains a row, a panel that reveals a detail, a message that grows a second line. The two columns on this page hold identical rows and differ only in the container.

It is worth being clear about what it is not. It does not animate its children, it does not fade anything, and it has no notion of open or closed — it is one number, eased. If you want a panel that comes in from an edge, that is `SlidePanel`; if you want one a person drags, that is `ExpandablePanel`.

## GaussRoundedView

A rounded surface that samples the scene behind it through a chain of mip textures and blurs it — the backing the glass family is built on.

**It arranges its own capture.** In normal flow it opens an overlay draw list, asks the window for the blurred scene on every draw, and announces itself from its apply hook so the window captures on the frame it first paints in rather than the one after. An ordinary window is all it needs. The exception is `MAKEPAD_NO_GAUSS=1`, which switches capture off everywhere and leaves every surface on its `fallback_color` face — a slab mixed from the theme's background and text colours, which is what the second row here shows when that flag is set.",
    subject: "rubber",
    feature: None,
    controls: &[],
    on_actions: Some(surfaces_actions),
}];
