//! The moving panels story: one that slides in from an edge and one you
//! drag up from the bottom, and why they are not the same widget.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.MovingPanelsOverview = StoryPage{
        StoryNote{text: "Two panels that move, for two different reasons. One is opened and closed by the application and animates itself in from an edge. The other is dragged by the person, and its position is a number the host can read at any moment."}

        StoryHeading{text: "A panel the app opens"}
        StoryNote{text: "SlidePanel animates in from whichever side it is given. It answers open, close and toggle; is_open says where it stands and is_animating says whether it has got there yet. The label below shows only the first, for a reason the docs explain."}
        StoryRow{
            open_left := Button{text: "open"}
            close_left := Button{text: "close"}
            toggle_left := Button{text: "toggle"}
            slide_state := Label{text: "closed"}
        }
        StoryRow{
            View{
                width: Fill height: 200.
                flow: Overlay
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_lowest}
                Label{
                    text: "the page behind the panel"
                    margin: theme.mspace_3
                    draw_text +: {color: theme.color_text_meta}
                }
                sliding := SlidePanel{
                    side: SlideSide.Left
                    width: 220. height: Fill
                    View{
                        width: Fill height: Fill
                        flow: Down
                        spacing: theme.space_1
                        padding: theme.mspace_3
                        show_bg: true
                        draw_bg +: {color: theme.color_surface_container_high}
                        Label{text: "Slid in from the left"}
                        Label{text: "side: SlideSide.Left" draw_text +: {color: theme.color_text_meta}}
                    }
                }
            }
        }

        StoryHeading{text: "A panel the person drags"}
        StoryNote{text: "ExpandablePanel puts its panel over a background and lets you pull it up and down. It reports the offset as it moves, so a host can fade the background or snap it somewhere; nothing snaps on its own. initial_offset is how far down it starts."}
        StoryRow{
            reset_panel := Button{text: "reset"}
            offset_state := Label{text: "offset 0"}
        }
        StoryRow{
            View{
                width: Fill height: 260.
                expandable := ExpandablePanel{
                    width: Fill height: Fill
                    initial_offset: 120.0
                    SolidView{
                        width: Fill height: Fill
                        align: Align{x: 0.5, y: 0.5}
                        draw_bg +: {color: theme.color_surface_container_lowest}
                        Label{text: "behind the panel" draw_text +: {color: theme.color_text_meta}}
                    }
                    panel := RoundedView{
                        width: Fill height: Fill
                        flow: Down
                        spacing: theme.space_1
                        padding: theme.mspace_3
                        draw_bg +: {color: theme.color_surface_container_high}
                        Label{text: "Drag me up and down"}
                        Label{text: "the offset above follows" draw_text +: {color: theme.color_text_meta}}
                    }
                }
            }
        }
    }
}

fn moving_panels_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let panel = root.slide_panel(cx, ids!(sliding));
    if root.button(cx, ids!(open_left)).clicked(actions) {
        panel.open(cx);
    }
    if root.button(cx, ids!(close_left)).clicked(actions) {
        panel.close(cx);
    }
    if root.button(cx, ids!(toggle_left)).clicked(actions) {
        panel.toggle(cx);
    }
    // is_open only, and not because is_animating is broken - it is not. A
    // story's action handler runs when there ARE actions, so it cannot watch
    // a value that changes on a timer: the last time this runs during a slide
    // is the click that started it, and the label would sit on whatever it
    // said then. Showing "moving" here needs a widget that draws every frame,
    // not a handler that waits to be called.
    let text = if panel.is_open(cx) { "open" } else { "closed" };
    let label = root.label(cx, ids!(slide_state));
    if label.text() != text {
        label.set_text(cx, text);
    }

    let expandable = root.expandable_panel(cx, ids!(expandable));
    if root.button(cx, ids!(reset_panel)).clicked(actions) {
        expandable.reset(cx);
    }
    if let Some(offset) = expandable.scrolled_at(actions) {
        root.label(cx, ids!(offset_state))
            .set_text(cx, &format!("offset {offset:.0}"));
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "containers/movingpanels/overview",
    category: "Containers",
    component: "MovingPanels",
    also: &["SlidePanel", "ExpandablePanel"],
    name: "Overview",
    dsl: "MovingPanelsOverview",
    added: "2025-05-06",
    tags: &["layout"],
    doc: "# Panels that move

Two panels that move, for two different reasons. Reaching for the wrong one is the usual mistake, and the difference is **who moves it**.

## SlidePanel — the application moves it

It animates in from whichever `side` it is given and takes `open`, `close` and `toggle`. Use it for something the app decides to show: a settings drawer opened by a button, a bar that appears when a mode changes.

It answers two questions. `is_open` says where it stands; `is_animating` says whether it has got there yet, which matters because **mid-slide it is neither open nor closed**.

**This page shows only the first, and the reason is about the page rather than the widget.** A story's action handler runs when there *are* actions. During a slide there are none — the click that started it is the last one — so a label driven from that handler sits on whatever it said at the moment of the press. Watching a value that changes on a clock needs something that draws every frame, not a handler waiting to be called.

I had this page telling you `is_animating` was broken. It is not: driven with the animator instrumented, the track is created on the press, reports `true` while it runs, and is retired the frame it ends. What was stale was my label.

## ExpandablePanel — the person moves it

Its `panel` sits over a background and is dragged up and down by hand. `initial_offset` is where it starts, `scrolled_at` reports the offset as it moves, `reset` puts it back and `get_current_offset` asks where it is.

**Nothing snaps and nothing settles by itself.** If you want it to spring to a detent, fade the background as it rises, or refuse to go past a point, that is your code reading `scrolled_at` — the widget hands you the number and takes no view about it. That is the opposite of the drawer's sheet detents, which do settle on their own.",
    subject: "sliding",
    feature: None,
    controls: &[],
    on_actions: Some(moving_panels_actions),
}];
