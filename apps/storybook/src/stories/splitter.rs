//! The splitter story: two panes, a bar between them, and the one rule that
//! decides who wins when a floor and a host disagree.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let Pane = RectView{
        width: Fill
        height: Fill
        align: Align{x: 0.5, y: 0.5}
        draw_bg +: {color: theme.color_surface_container_low}
    }

    mod.stories.SplitterOverview = StoryPage{
        StoryNote{text: "Two panes and a bar you can drag. Six places in this repository split a window this way, and the storybook you are reading is one of them."}

        StoryHeading{text: "Drag the bar"}
        StoryNote{text: "The bar reports where it was put, as an align. Weighted keeps a fraction as the window resizes; FromA and FromB keep a fixed number of points against one edge."}
        StoryRow{
            View{
                width: Fill height: 160.
                dragged := Splitter{
                    axis: Horizontal
                    align: Weighted(0.5)
                    a: Pane{Label{text: "A"}}
                    b: Pane{Label{text: "B"}}
                }
            }
        }
        StoryRow{
            where_ := Label{text: "align: Weighted(0.50)"}
        }

        StoryHeading{text: "Each pane keeps a floor"}
        StoryNote{text: "Drag this bar all the way to either edge. It stops with 80 points between the bar and the edge, on both sides, because the floor is applied to the HAND: the drag handler clamps what you asked for. Nothing clamps the layout, which is what lets the next section work at all. The floors in use here are min_vertical and max_vertical, because those are named for the bar and this bar is vertical."}
        StoryRow{
            View{
                width: Fill height: 160.
                floored := Splitter{
                    axis: Horizontal
                    align: Weighted(0.5)
                    min_vertical: 80.
                    max_vertical: 80.
                    a: Pane{Label{text: "80 minimum"}}
                    b: Pane{Label{text: "80 minimum"}}
                }
            }
        }

        StoryHeading{text: "A floor governs the hand, not the host"}
        StoryNote{text: "The same splitter, with the same 80 point floors. Fold a pane away and it goes to nothing — the floor does not fight it. A floor is what a person may drag to; a host asking for a closed panel is not a person dragging, and a splitter that clamped its layout would silently reopen every panel an app tried to close. Either way the bar stays: it is the only way back."}
        StoryRow{
            fold_a := Button{text: "fold A"}
            fold_b := Button{text: "fold B"}
            fold_none := Button{text: "open both"}
            folded := Label{text: "collapse: None"}
        }
        StoryRow{
            View{
                width: Fill height: 160.
                collapsing := Splitter{
                    axis: Horizontal
                    align: Weighted(0.5)
                    min_vertical: 80.
                    max_vertical: 80.
                    a: Pane{Label{text: "A"}}
                    b: Pane{Label{text: "B"}}
                }
            }
        }

        StoryHeading{text: "One widget, either way round"}
        StoryNote{text: "Side by side or stacked is the axis, not a different control. Horizontal splits horizontally, so the panes sit left and right; Vertical stacks them."}
        StoryRow{
            View{
                width: 300. height: 160.
                Splitter{
                    axis: Horizontal
                    align: Weighted(0.5)
                    a: Pane{Label{text: "left"}}
                    b: Pane{Label{text: "right"}}
                }
            }
            View{
                width: 300. height: 160.
                Splitter{
                    axis: Vertical
                    align: Weighted(0.5)
                    a: Pane{Label{text: "above"}}
                    b: Pane{Label{text: "below"}}
                }
            }
        }
    }
}

fn splitter_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    if root.splitter(cx, ids!(dragged)).changed(actions).is_some() {
        let text = match root.splitter(cx, ids!(dragged)).align() {
            Some(SplitterAlign::Weighted(f)) => format!("align: Weighted({f:.2})"),
            Some(SplitterAlign::FromA(p)) => format!("align: FromA({p:.0})"),
            Some(SplitterAlign::FromB(p)) => format!("align: FromB({p:.0})"),
            None => "align: none".to_string(),
        };
        root.label(cx, ids!(where_)).set_text(cx, &text);
    }

    let collapsing = root.splitter(cx, ids!(collapsing));
    let mut set = None;
    if root.button(cx, ids!(fold_a)).clicked(actions) {
        set = Some(SplitterCollapse::A);
    }
    if root.button(cx, ids!(fold_b)).clicked(actions) {
        set = Some(SplitterCollapse::B);
    }
    if root.button(cx, ids!(fold_none)).clicked(actions) {
        set = Some(SplitterCollapse::None);
    }
    if let Some(c) = set {
        collapsing.set_collapse(cx, c);
        let text = match c {
            SplitterCollapse::A => "collapse: A",
            SplitterCollapse::B => "collapse: B",
            SplitterCollapse::None => "collapse: None",
        };
        root.label(cx, ids!(folded)).set_text(cx, text);
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "containers/splitter/overview",
    category: "Containers",
    component: "Splitter",
    also: &[],
    name: "Overview",
    dsl: "SplitterOverview",
    added: "2026-09-08",
    tags: &["new", "layout"],
    doc: "# Splitter

Two panes with a bar between them that a person can drag.

`a` and `b` are the panes, `axis` says whether they sit side by side or stacked, and `align` says where the bar is: `Weighted(f)` keeps a fraction as the window resizes, `FromA(points)` and `FromB(points)` keep a fixed distance against one edge. It reports the axis and the align together whenever the bar is let go.

**The floor fields are named for the bar, not for the axis, and the two words are opposites.** A `Horizontal` axis splits horizontally, so the panes sit left and right and the bar between them is *vertical* — and it is `min_vertical` and `max_vertical` that bound it. A `Vertical` axis stacks the panes and uses the horizontal pair. `min_` is pane A's floor, `max_` is pane B's; the name is `max` because it was once read as a distance from the far edge, and every caller in the repository relies on the pairing as it stands.

I wrote this page with the axis the wrong way round and only found out by dragging it.

**A floor governs the hand, not the host.** This is the rule worth knowing, and it is not the obvious one. The floors are applied where the drag is handled, so they bound what a person can drag to. They are *not* applied when the splitter lays itself out. That looks like an oversight until an app asks for a panel to be closed: `collapse` folds a pane to nothing, and a splitter that clamped its own layout would quietly reopen it to eighty points and go on doing that forever. The layout clamps only into the room there actually is.

I know because I wrote that clamp into the layout, and it took a gutter that two applications close on purpose and wedged it open.

**Folding leaves the bar behind.** Writing this page turned up the other half of that: folding the second pane used to hand the first one the whole room, and the bar is laid out after the first pane and takes its width from what is left — so it got nothing and was not drawn. The panel closed and the handle that opens it went with it. Folding the first pane never had the problem, because a pane of no width leaves the bar its own.",
    subject: "dragged",
    feature: None,
    controls: &[],
    on_actions: Some(splitter_actions),
}];
