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
        StoryNote{text: "SlidePanel animates in from whichever side it is given. It answers open, close and toggle, and is_open says where it stands. It also offers is_animating, and that one does not work: see the note in the docs before you build on it."}
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
                    panel: RoundedView{
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
    // is_open only. is_animating cannot be used here: it asks the animator
    // whether a track with this id EXISTS rather than whether it is running,
    // and the track outlives the animation - so once this panel has moved
    // once, is_animating answers true for ever. Driven: the panel settles
    // open (its rect stops changing) and closes again, and is_animating stays
    // true throughout both.
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

`is_open` says where it stands, and works.

**`is_animating` does not, and this page is where I found that out.** It asks the animator whether a track with the panel's id *exists*, not whether that track is running — and the track outlives the animation. Driven on this page: open the panel, wait until its rectangle stops changing, and `is_animating` still answers true; close it again and it answers true after that too. Once a panel has moved once, it reports itself as moving for ever. A host that gates on it will wait for something that never happens, so read `is_open` and, if you need to know when the slide finished, time it yourself until this is fixed.

## ExpandablePanel — the person moves it

Its `panel` sits over a background and is dragged up and down by hand. `initial_offset` is where it starts, `scrolled_at` reports the offset as it moves, `reset` puts it back and `get_current_offset` asks where it is.

**Nothing snaps and nothing settles by itself.** If you want it to spring to a detent, fade the background as it rises, or refuse to go past a point, that is your code reading `scrolled_at` — the widget hands you the number and takes no view about it. That is the opposite of the drawer's sheet detents, which do settle on their own.",
    subject: "sliding",
    feature: None,
    controls: &[],
    on_actions: Some(moving_panels_actions),
}];
