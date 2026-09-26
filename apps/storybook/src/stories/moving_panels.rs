//! The moving panels story: one that slides in from an edge, one you drag
//! up from the bottom, one whose height follows its content, and why they
//! are not the same widget.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    /** One row of the growing columns. */
    let EasedRow = SolidView{
        width: Fill height: Fit
        padding: theme.mspace_1
        draw_bg +: {color: theme.color_surface_container_high}
        Label{text: "a row"}
    }

    mod.stories.MovingPanelsOverview = StoryPage{
        StoryNote{text: "Three containers that move, for three different reasons. One is opened and closed by the application and animates itself in from an edge. One is dragged by the person, and its position is a number the host can read at any moment. The last is moved by its content: its height follows what it holds."}

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
        StoryNote{text: "ExpandablePanel puts its panel over a background and lets you pull it up and down. It reports the offset as it moves, so a host can fade the background or snap it somewhere; nothing snaps on its own. initial_offset is how far down it starts and how far up a drag may take it. The panel is the child named panel, and it is declared last because this is an overlay."}
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

        StoryHeading{text: "A height that eases"}
        StoryNote{text: "RubberView measures its content on every draw and grows toward that height over a few frames instead of taking it at once. Add rows: the plain column on the left jumps to its new height on the next frame, and the RubberView on the right grows into it. Remove rows and both shrink at once, because it eases a growing height only. The rows are the same in both columns; the easing is the container's."}
        StoryRow{
            add_row := Button{text: "Add a row"}
            drop_row := Button{text: "Remove one"}
            row_count := Label{text: "3 rows"}
        }
        StoryRow{
            align: Align{y: 0.}
            View{
                width: 300. height: Fit
                flow: Down
                spacing: theme.space_1
                Label{text: "View" draw_text +: {color: theme.color_text_meta}}
                plain_rows := SolidView{
                    width: Fill height: Fit
                    flow: Down
                    spacing: 2.
                    padding: theme.mspace_1
                    draw_bg +: {color: theme.color_surface_container_low}
                    EasedRow{} EasedRow{} EasedRow{}
                    extra_a := EasedRow{visible: false}
                    extra_b := EasedRow{visible: false}
                    extra_c := EasedRow{visible: false}
                }
            }
            View{
                width: 300. height: Fit
                flow: Down
                spacing: theme.space_1
                Label{text: "RubberView" draw_text +: {color: theme.color_text_meta}}
                rubber := RubberView{
                    width: Fill height: Fit
                    smoothing: 0.25
                    SolidView{
                        width: Fill height: Fit
                        flow: Down
                        spacing: 2.
                        padding: theme.mspace_1
                        draw_bg +: {color: theme.color_surface_container_low}
                        EasedRow{} EasedRow{} EasedRow{}
                        rextra_a := EasedRow{visible: false}
                        rextra_b := EasedRow{visible: false}
                        rextra_c := EasedRow{visible: false}
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

fn rubber_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
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
    root.view(cx, ids!(plain_rows)).redraw(cx);
    root.widget(cx, ids!(rubber)).redraw(cx);
}

/// The page's one handler: the two panels, then the easing height.
fn moving_panels_page_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    moving_panels_actions(cx, root, actions);
    rubber_actions(cx, root, actions);
}

pub const STORIES: &[Story] = &[Story {
    key: "containers/movingpanels/overview",
    category: "Containers",
    component: "MovingPanels",
    also: &["SlidePanel", "ExpandablePanel", "RubberView"],
    name: "Overview",
    dsl: "MovingPanelsOverview",
    added: "2025-05-06",
    tags: &["layout"],
    doc: "# Panels that move

Three containers that move, for three different reasons. Reaching for the wrong one is the usual mistake, and the difference is **who moves it**: the application moves a `SlidePanel`, the person moves an `ExpandablePanel`, and its own content moves a `RubberView`.

## SlidePanel — the application moves it

It animates in from whichever `side` it is given and takes `open`, `close` and `toggle`. Use it for something the app decides to show: a settings drawer opened by a button, a bar that appears when a mode changes.

It answers two questions. `is_open` says where it stands; `is_animating` says whether it has got there yet, which matters because **mid-slide it is neither open nor closed**.

**This page shows only the first, and the reason is about the page rather than the widget.** A story's action handler runs when there *are* actions. During a slide there are none — the click that started it is the last one — so a label driven from that handler sits on whatever it said at the moment of the press. Watching a value that changes on a clock needs something that draws every frame, not a handler waiting to be called.

`is_animating` itself is sound: it turns true on the press that starts a slide and false on the frame the slide ends. A label that reads it has to be redrawn on that clock to show it.

## ExpandablePanel — the person moves it

Its `panel` sits over a background and is dragged up and down by hand. `scrolled_at` reports the offset as it moves, `reset` puts it back and `get_current_offset` asks where it is.

**The panel is a child you name `panel`, and it goes last.** The widget is an overlay, so the child written last is the one painted on top — the background first, the panel after it. The widget does not hold that slot open itself, because a child declared on a preset is copied into every instance ahead of the instance's own children: a panel held there would always be the first child, and so always underneath the background it is meant to sit above.

`initial_offset` is how far down the panel rests and the whole of the travel: pulled all the way up it meets the top of the area and stops there, and it does not go below where it started.

**Nothing snaps and nothing settles by itself.** If you want it to spring to a detent, fade the background as it rises, or refuse to go past a point, that is your code reading `scrolled_at` — the widget hands you the number and takes no view about it. That is the opposite of the drawer's sheet detents, which do settle on their own.

## RubberView — its content moves it

It measures its content on every draw and, when the content has grown, reports a height that eases toward the new one instead of taking it at once. `smoothing` runs from 0 to 1, higher being faster. Each frame covers `smoothing` of the distance left at 60 frames a second, and proportionally less on a faster display; a frame that arrives late still covers only `smoothing`. Once the height has arrived it stops asking for frames. The first draw takes the content's height outright, so nothing grows in from nothing.

**It eases a growing height only.** When the content shrinks, the view takes the smaller height on the next frame. And while it grows, the content is drawn at full size and the height it reports to its parent is the eased one, so something laid out directly under it can be overlapped for those few frames.

Put one around content whose height grows while a person is looking at it: a list that gains a row, a panel that reveals a detail, a message that grows a second line. It does not animate its children, fade anything or know about open and closed: it is one number, eased. A panel that comes in from an edge is a `SlidePanel`, and one a person drags is an `ExpandablePanel`.",
    subject: "sliding",
    feature: None,
    controls: &[],
    on_actions: Some(moving_panels_page_actions),
}];
