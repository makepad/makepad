//! The dialog stories: the question that stops the work.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.DialogOverview = StoryPage{
        StoryNote{text: "A dialog stops the work and asks for an answer, so it takes the pointer and the keyboard and dims what is behind it. Return takes the default answer, Escape leaves without one, and the keyboard goes back where it came from."}

        StoryHeading{text: "Ask something"}
        StoryRow{
            ask_confirm := Button{text: "Confirm"}
            ask_alert := Button{text: "Just tell me"}
            ask_danger := Button{text: "Something destructive"}
            ask_full := Button{text: "Take the window"}
        }

        StoryHeading{text: "What came back"}
        StoryRow{
            answer := Label{text: "nothing asked yet"}
        }
        StoryRow{
            state := Label{text: "none open"}
        }

        StoryNote{text: "The button below is unreachable while a dialog is up, which is the whole difference between a dialog and a toast."}
        StoryRow{
            under := Button{text: "Behind the dialog"}
            under_note := Label{text: "pressed 0 times"}
        }

        confirm_dialog := ConfirmDialog{
            title: "Discard the take?"
            content +: {
                body +: {
                    P{text: "The last four bars have not been saved. Discarding them cannot be undone."}
                }
            }
        }

        alert_dialog := AlertDialog{
            title: "Export finished"
            content +: {
                body +: {
                    P{text: "Forty-two files were written to the folder you chose."}
                }
            }
        }

        danger_dialog := DangerDialog{
            title: "Delete this project?"
            content +: {
                body +: {
                    P{text: "Every take, every mix and every render goes with it. There is no undo for this one."}
                }
            }
        }

        full_dialog := FullScreenDialog{
            title: "The whole window"
            content +: {
                body +: {
                    P{text: "A full-screen dialog is for a task that has taken over: an import with choices to make, a first-run setup. It is still a dialog, so Escape still leaves it."}
                }
            }
        }
    }
}

fn dialog_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let confirm = root.dialog(cx, ids!(confirm_dialog));
    let alert = root.dialog(cx, ids!(alert_dialog));
    let danger = root.dialog(cx, ids!(danger_dialog));
    let full = root.dialog(cx, ids!(full_dialog));

    if root.button(cx, ids!(ask_confirm)).clicked(actions) {
        confirm.open(cx);
    }
    if root.button(cx, ids!(ask_alert)).clicked(actions) {
        alert.open(cx);
    }
    if root.button(cx, ids!(ask_danger)).clicked(actions) {
        danger.open(cx);
    }
    if root.button(cx, ids!(ask_full)).clicked(actions) {
        full.open(cx);
    }
    if root.button(cx, ids!(under)).clicked(actions) {
        let n = crate::stories::bump(live_id!(dialog_under));
        root.label(cx, ids!(under_note)).set_text(cx, &format!("pressed {n} times"));
    }

    // What is up, so the story says whether a press closed the dialog as
    // well as what it answered.
    let open: Vec<&str> = [
        ("confirm", &confirm),
        ("alert", &alert),
        ("danger", &danger),
        ("full", &full),
    ]
    .iter()
    .filter(|(_, d)| d.is_open())
    .map(|(name, _)| *name)
    .collect();
    let state = if open.is_empty() { "none open".to_string() } else { format!("{} open", open.join(", ")) };
    let state_label = root.label(cx, ids!(state));
    if state_label.text() != state {
        state_label.set_text(cx, &state);
    }

    for (name, dialog) in [
        ("confirm", &confirm),
        ("alert", &alert),
        ("danger", &danger),
        ("full", &full),
    ] {
        if let Some(answer) = dialog.answered(actions) {
            let said = match answer {
                DialogAction::Confirmed => "confirmed",
                DialogAction::Cancelled => "cancelled",
                DialogAction::Dismissed => "dismissed",
                DialogAction::None => continue,
            };
            root.label(cx, ids!(answer)).set_text(cx, &format!("{name}: {said}"));
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "overlay/dialog/overview",
    category: "Overlay",
    component: "Dialog",
    also: &["AlertDialog", "ConfirmDialog", "DangerDialog", "FullScreenDialog"],
    name: "Overview",
    dsl: "DialogOverview",
    added: "2026-09-05",
    tags: &["new"],
    doc: "# Dialog\n\nA dialog stops the work and asks for an answer. That is the opposite of a toast, and it is why a dialog takes the pointer, takes the keyboard, dims what is behind it and does not go away by itself.\n\nIt is a title, a body that scrolls, and a row of answers that does not: a long body that pushes its buttons off the screen leaves the reader unable to answer the question they were stopped for. `size` picks the width from `Xs` for a yes-or-no up to `Full` for a task that has taken over.\n\nReturn takes the default answer and Escape leaves without one, so a dialog can always be answered or left by keyboard alone; when it closes, the focus goes back where it came from. `destructive` puts the error role on the confirming answer, because \"delete everything\" drawn like \"cancel\" eventually deletes everything.\n\nA dialog nests like every other overlay: a menu or a popover opened inside it takes Escape first, one press closing one thing.",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: Some(dialog_actions),
}];
