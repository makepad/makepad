//! The overlay messages story: four ways of saying something over a page,
//! and what each of them is actually for.
use crate::makepad_widgets::alert::AlertIntent;
use crate::makepad_widgets::callout_tooltip::{CalloutTooltipOptions, TooltipPosition};
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.OverlayMessagesOverview = StoryPage{
        StoryNote{text: "Four widgets for putting something over a page without moving what is under it. They look alike from a distance and are not interchangeable: what separates them is where the message is anchored and who decides when it goes away."}

        StoryHeading{text: "A tooltip you place"}
        StoryNote{text: "It takes a position and a string and appears there. Nothing points at anything: the caller knows where the pointer is and says so. Use it for a label that follows a cursor."}
        StoryRow{
            show_tip := Button{text: "show"}
            hide_tip := Button{text: "hide"}
        }

        StoryHeading{text: "A callout that points"}
        StoryNote{text: "Given a rectangle, it puts itself beside that rectangle and draws a triangle back at it. Use it when the message is ABOUT a particular control and the reader has to know which one."}
        StoryRow{
            anchor := Button{text: "explain this button"}
            hide_callout := Button{text: "hide"}
        }

        StoryHeading{text: "A notification in the corner"}
        StoryNote{text: "Opens in the top corner over everything, and stays until it is closed. It is a container rather than a message: what it shows is whatever you put in its content."}
        StoryRow{
            open_note := Button{text: "open"}
            note_state := Label{text: "closed"}
        }

        StoryHeading{text: "Only one of them is free to sit anywhere"}
        StoryNote{text: "A Modal pins the walk it reports to its parent at nothing, so it can be declared in the middle of a row and move none of it. Of these four only Tooltip does the same. Measured on this page: the tooltip reports a zero rect, the callout takes 91 points of this column, the banner 34, and the notification fills. So they are declared at the end of this page on purpose, and where you put yours is a layout decision rather than a free one."}

        StoryHeading{text: "A banner across the top"}
        StoryNote{text: "Full width, carries an intent, and holds a title and a line of detail. This is the one for something the reader must act on rather than glance at."}
        StoryRow{
            banner_info := Button{text: "info"}
            banner_warn := Button{text: "warning"}
            banner_off := Button{text: "dismiss"}
        }

        // Declared at the end deliberately. Only the tooltip pins its walk
        // away the way a Modal does; the other three take a real slot in this
        // column, so where they are written changes the page around them.
        tip := Tooltip{}
        callout := CalloutTooltip{}
        note := PopupNotification{
            content: View{
                width: 260. height: Fit
                flow: Down
                spacing: theme.space_1
                padding: theme.mspace_3
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_high}
                Label{text: "Rendered on its own layer"}
                close_note := Button{text: "close"}
            }
        }
        banner := BannerHost{}
    }
}

fn overlay_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    if root.button(cx, ids!(show_tip)).clicked(actions) {
        root.tooltip(cx, ids!(tip))
            .show_with_options(cx, Vec2d { x: 520.0, y: 210.0 }, "placed here, by hand");
    }
    if root.button(cx, ids!(hide_tip)).clicked(actions) {
        root.tooltip(cx, ids!(tip)).hide(cx);
    }

    if root.button(cx, ids!(anchor)).clicked(actions) {
        // The rect of the control it is about: the callout works out which
        // side to sit on and where to put the triangle from this alone.
        let rect = root.button(cx, ids!(anchor)).area().rect(cx);
        root.callout_tooltip(cx, ids!(callout)).show_with_options(
            cx,
            "this is the button the callout is about",
            rect,
            CalloutTooltipOptions {
                position: TooltipPosition::Bottom,
                ..Default::default()
            },
        );
    }
    if root.button(cx, ids!(hide_callout)).clicked(actions) {
        root.callout_tooltip(cx, ids!(callout)).hide(cx);
    }

    let note = root.popup_notification(cx, ids!(note));
    if root.button(cx, ids!(open_note)).clicked(actions) {
        note.open(cx);
    }
    if root.button(cx, ids!(close_note)).clicked(actions) {
        note.close(cx);
    }
    let note_text = if note.is_open() { "open" } else { "closed" };
    let label = root.label(cx, ids!(note_state));
    if label.text() != note_text {
        label.set_text(cx, note_text);
    }

    let banner = root.banner_host(cx, ids!(banner));
    if root.button(cx, ids!(banner_info)).clicked(actions) {
        banner.show(cx, AlertIntent::Info, "Rendering", "Three of nine frames done.");
    }
    if root.button(cx, ids!(banner_warn)).clicked(actions) {
        banner.show(cx, AlertIntent::Warning, "Disconnected", "The device stopped answering.");
    }
    if root.button(cx, ids!(banner_off)).clicked(actions) {
        banner.dismiss(cx);
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "overlay/messages/overview",
    category: "Overlay",
    component: "OverlayMessages",
    also: &["Tooltip", "CalloutTooltip", "PopupNotification", "BannerHost"],
    name: "Overview",
    dsl: "OverlayMessagesOverview",
    added: "2026-02-12",
    tags: &[],
    doc: "# Overlay messages

Four widgets for putting something over a page without moving what is under it. They look alike from a distance and are not interchangeable — what separates them is **where the message is anchored and who decides when it goes**.

`Tooltip` is placed. You give it a position and a string; it appears there and points at nothing. It is for a label that follows a cursor, and the caller is the one who knows where the cursor is.

`CalloutTooltip` is anchored. You give it the *rectangle of the control the message is about*, and it works out which side to sit on and draws a triangle back at it. Use it when the reader has to know which control you mean. Its `position` is a preference, not a demand — it will move rather than fall off the window.

`PopupNotification` is a container, not a message. It opens in a corner over everything and holds whatever you put in its `content`, and it stays until something closes it. If you want it to go away by itself, that is your timer, not its.

`BannerHost` is the loud one: full width, an `AlertIntent`, a title and a line of detail. Reach for it when the reader has to act, not glance.

**Only one of the four is free to sit anywhere.** A `Modal` pins the walk it reports to its parent at nothing, so it can be declared in the middle of a row and move none of it. Of these, only `Tooltip` does the same — measured on this page it reports a zero rect, while `CalloutTooltip` takes 91 points of the column it is declared in, `BannerHost` 34, and `PopupNotification` fills. They are declared at the end of this page for that reason. Where you put yours is a layout decision, not a free one, and that is worth knowing before you write one next to the control it describes.",
    subject: "tip",
    feature: None,
    controls: &[],
    on_actions: Some(overlay_actions),
}];
