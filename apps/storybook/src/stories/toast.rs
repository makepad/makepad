//! The toast story: what the layer says, what it refuses to do, and the
//! corner panel that stays until it is closed.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.ToastOverview = StoryPage{
        StoryNote{text: "A toast reports a fact and expects to be ignored. It never takes the pointer, never blocks what is under it, and leaves on its own. Hovering the stack freezes every countdown, because a message that vanishes while it is being read has failed."}

        StoryHeading{text: "Say something"}
        StoryRow{
            say_saved := Button{text: "Saved"}
            say_undo := Button{text: "With an action"}
            say_error := Button{text: "Something went wrong"}
            say_long := Button{text: "With a second line"}
        }

        StoryHeading{text: "More than fits"}
        StoryNote{text: "Only three cards are on screen at once; the rest wait, and the layer says how many."}
        StoryRow{
            say_five := Button{text: "Say five things"}
            clear := Button{text: "Clear them all"}
        }

        StoryHeading{text: "What came back"}
        StoryRow{
            report := Label{text: "nothing yet"}
        }

        StoryNote{text: "The button below stays live while toasts are up: a toast that blocked the work would be a dialog wearing the wrong clothes."}
        StoryRow{
            under := Button{text: "Still reachable"}
            under_note := Label{text: "pressed 0 times"}
        }

        StoryHeading{text: "Which corner"}
        StoryNote{text: "Six corners. A card slides in from the edge its stack sits against, so a top stack drops and a bottom stack rises."}
        StoryRow{
            corner := SegmentedControl{
                options: ["Top left" "Top" "Top right" "Bottom left" "Bottom" "Bottom right"]
                selected: 4
            }
        }

        StoryHeading{text: "A notification that holds anything"}
        StoryNote{text: "PopupNotification is a container rather than a message. It opens in the top right corner of the window, over everything, holds whatever its content is, and stays until something closes it: no timer, no queue and no stack. A toast is for a fact the person may ignore; this is for a panel they have to close."}
        StoryRow{
            open_note := Button{text: "Open it"}
            note_state := Label{text: "closed"}
        }

        // Declared at the end with the toaster: it claims no room in the page,
        // and while it is open it lays its content out over the whole window.
        note := PopupNotification{
            content: SolidView{
                width: 260. height: Fit
                flow: Down
                spacing: theme.space_1
                padding: theme.mspace_3
                margin: theme.mspace_3
                cursor: MouseCursor.Default
                capture_overload: true
                draw_bg +: {color: theme.color_surface_container_high}
                Label{text: "Drawn on a layer of its own"}
                close_note := Button{text: "Close"}
            }
        }
        toasts := Toaster{}
    }
}

const CORNERS: [ToastPlace; 6] = [
    ToastPlace::TopStart,
    ToastPlace::TopCenter,
    ToastPlace::TopEnd,
    ToastPlace::BottomStart,
    ToastPlace::BottomCenter,
    ToastPlace::BottomEnd,
];

fn toast_story_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let toaster = root.toaster(cx, ids!(toasts));

    if let Some(index) = root.segmented_control(cx, ids!(corner)).selected(actions) {
        if let Some(place) = CORNERS.get(index) {
            toaster.set_place(cx, *place);
        }
    }

    if root.button(cx, ids!(say_saved)).clicked(actions) {
        toaster.show(cx, Toast::new(live_id!(saved), "Saved").intent(BadgeIntent::Success));
    }
    if root.button(cx, ids!(say_undo)).clicked(actions) {
        toaster.show(
            cx,
            Toast::new(live_id!(moved), "Moved to Archive")
                .action("Undo")
                .life(ToastLife::Long),
        );
    }
    if root.button(cx, ids!(say_error)).clicked(actions) {
        toaster.show(
            cx,
            Toast::new(live_id!(failed), "Could not reach the server")
                .body("The last change is still on this machine.")
                .intent(BadgeIntent::Error)
                .action("Retry"),
        );
    }
    if root.button(cx, ids!(say_long)).clicked(actions) {
        toaster.show(
            cx,
            Toast::new(live_id!(export), "Export finished")
                .body("42 files written to the chosen folder.")
                .intent(BadgeIntent::Info)
                .life(ToastLife::Long),
        );
    }
    if root.button(cx, ids!(say_five)).clicked(actions) {
        for (i, id) in [
            live_id!(one),
            live_id!(two),
            live_id!(three),
            live_id!(four),
            live_id!(five),
        ]
        .into_iter()
        .enumerate()
        {
            toaster.show(cx, Toast::new(id, &format!("Message {}", i + 1)).life(ToastLife::Long));
        }
    }
    if root.button(cx, ids!(clear)).clicked(actions) {
        toaster.dismiss_all(cx);
    }
    if root.button(cx, ids!(under)).clicked(actions) {
        let n = crate::stories::bump(live_id!(toast_under));
        root.label(cx, ids!(under_note)).set_text(cx, &format!("pressed {n} times"));
    }

    for action in toast_actions(actions) {
        let text = match action {
            ToastAction::ActionPressed(id) => Some(format!(
                "action pressed on {}",
                id.as_string(|s| s.map(str::to_string).unwrap_or_default())
            )),
            ToastAction::Closed(id) => Some(format!(
                "closed {}",
                id.as_string(|s| s.map(str::to_string).unwrap_or_default())
            )),
            _ => None,
        };
        if let Some(text) = text {
            let showing = toaster.showing();
            let waiting = toaster.waiting();
            root.label(cx, ids!(report))
                .set_text(cx, &format!("{text} · {showing} up, {waiting} waiting"));
        }
    }
}

fn popup_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let note = root.popup_notification(cx, ids!(note));
    if root.button(cx, ids!(open_note)).clicked(actions) {
        note.open(cx);
    }
    if root.button(cx, ids!(close_note)).clicked(actions) {
        note.close(cx);
    }
    // Asked every pass rather than tracked, so the label follows the panel
    // whoever closed it.
    let text = if note.is_open() { "open" } else { "closed" };
    let label = root.label(cx, ids!(note_state));
    if label.text() != text {
        label.set_text(cx, text);
    }
}

/// The page's one handler: the toasts, then the corner notification.
fn toast_page_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    toast_story_actions(cx, root, actions);
    popup_actions(cx, root, actions);
}

pub const STORIES: &[Story] = &[Story {
    key: "feedback/toast/overview",
    category: "Feedback",
    component: "Toast",
    also: &["Toaster", "PopupNotification"],
    name: "Overview",
    dsl: "ToastOverview",
    added: "2026-09-05",
    tags: &["new"],
    doc: "# Toast\n\nA toast is the opposite of a dialog. A dialog stops the work and demands an answer; a toast reports a fact and expects to be ignored. So it never takes the pointer, never takes the keyboard, never blocks what is under it, and leaves on its own.\n\nOne `Toaster` per app owns the queue, the lifetimes, the stacking and the chrome, so any code with a `Cx` can say something without owning a widget. `place` puts the stack in one of six corners; `max_visible` caps what is on screen and the rest wait, with the layer saying how many.\n\nLifetimes are a promise about attention: `Short` for a fact that needs no thought, `Long` for one carrying an action worth reaching, `Indefinite` for something unfinished or wrong. An `Error` is indefinite unless the caller insists otherwise, because an error that disappears on its own is an error nobody read.\n\nThe remaining time is a hairline along the card's bottom edge, and hovering the stack freezes every countdown.\n\n## PopupNotification\n\n`PopupNotification` is a container, not a message. `open` lays its `content` out over the whole window, in the top right corner by default through its `align`, and `close` takes it away; `is_open` asks. It claims no room in the page it is declared in. It has no lifetime, no queue and no stack, and while it is open its content takes the pointer like any other widget. Reach for it when the person has to close the panel themselves, and for a toast when they may ignore it.",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: Some(toast_page_actions),
}];
