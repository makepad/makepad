//! The spinner family: faces, sizes, the delay, the status spinner, the
//! saving indicator and the loading overlay, plus one controlled spinner.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.SpinnerOverview = StoryPage{
        StoryNote{text: "The mark that says working. Three faces, the size ladder, a delayed one that never flashes for a quick load, the status spinner that ends in a tick or a cross, the saving indicator built from it, and the overlay that blocks a region while it loads."}

        StoryHeading{text: "Faces"}
        StoryRow{
            SpinnerFlat{}
            SpinnerDots{}
            SpinnerBars{}
            SpinnerContained{}
            SpinnerFlat{text: "Loading"}
            SpinnerFlat{draw_bg.color: theme.color_success}
        }

        StoryHeading{text: "Sizes"}
        StoryRow{
            SpinnerFlat{size: 12.}
            SpinnerFlat{size: 16.}
            SpinnerFlat{size: 24.}
            SpinnerFlat{size: 32.}
            SpinnerFlat{size: 48.}
            SpinnerDots{size: 12.}
            SpinnerDots{size: 24.}
            SpinnerBars{size: 16.}
            SpinnerBars{size: 32.}
        }

        StoryHeading{text: "Delayed"}
        StoryNote{text: "Both started with the page; the second waits a second and a half before it shows, so a load that finishes sooner never flashes."}
        StoryRow{
            SpinnerFlat{text: "at once"}
            delayed := SpinnerFlat{delay_secs: 1.5 text: "after 1.5 s"}
        }

        StoryHeading{text: "Status spinner"}
        StoryNote{text: "Start turns it, Finish morphs the arc into a tick, Fail into a cross; a mark goes quiet again by itself after two seconds."}
        StoryRow{
            status := StatusSpinner{text: "Working" text_finished: "Done" text_error: "Failed"}
            start := Button{text: "Start"}
            finish := Button{text: "Finish"}
            fail := Button{text: "Fail"}
        }
        StoryRow{
            StatusSpinner{status: SpinnerStatus.Active text: "Active"}
            StatusSpinner{status: SpinnerStatus.Finished auto_reset_secs: 0. text: "Finished"}
            StatusSpinner{status: SpinnerStatus.Error auto_reset_secs: 0. text: "Error"}
            StatusSpinner{status: SpinnerStatus.Inactive text: "Inactive keeps its space"}
        }

        StoryHeading{text: "Saving"}
        StoryNote{text: "Save begins a save; Saved lands it (the clock is UTC unless the host gives an offset); Fail shows the failure and a retry button, which begins another save."}
        StoryRow{
            saving := SavingIndicator{}
        }
        StoryRow{
            save := Button{text: "Save"}
            save_done := Button{text: "Saved"}
            save_fail := Button{text: "Fail"}
        }

        StoryHeading{text: "Loading overlay"}
        StoryNote{text: "While active the overlay dims its content, blocks the pointer over it and centres a spinner; the second one blurs the content through a glass pane instead."}
        StoryRow{
            overlay_toggle := Button{text: "Toggle overlay"}
            blur_toggle := Button{text: "Toggle blurred overlay"}
            blocked_note := Label{text: "the button under the overlay: not pressed"}
        }
        StoryRow{
            overlay := LoadingOverlay{
                width: 320.
                height: 120.
                text: "Refreshing"
                delay_secs: 0.2
                content := View{
                    width: Fill
                    height: Fill
                    show_bg: true
                    draw_bg.color: theme.color_surface_container_high
                    padding: theme.mspace_2
                    flow: Down
                    spacing: theme.space_2
                    Label{text: "Content underneath the overlay"}
                    under := Button{text: "A button the overlay blocks"}
                }
            }
            overlay_blur := LoadingOverlay{
                width: 320.
                height: 120.
                text: "Refreshing"
                blur: true
                content := View{
                    width: Fill
                    height: Fill
                    show_bg: true
                    draw_bg.color: theme.color_surface_container_high
                    padding: theme.mspace_2
                    flow: Down
                    spacing: theme.space_2
                    Label{text: "Content behind the glass"}
                    Button{text: "Another blocked button"}
                }
            }
        }
    }

    mod.stories.SpinnerLoading = StoryPage{
        StoryNote{text: "The older DSL-only view, unchanged: a ring with a turning gap, styled entirely through its shader."}
        StoryHeading{text: "Default"}
        StoryRow{
            LoadingSpinner{}
        }
    }

    mod.stories.SpinnerBasic = StoryPage{
        StoryNote{text: "One spinner; its face, size, delay, label and container come from the controls. Change the delay and press Reset to see it wait."}
        StoryRow{
            subject := SpinnerFlat{text: "Loading"}
        }
    }
}

fn spinner_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let status = root.status_spinner(cx, ids!(status));
    if root.button(cx, ids!(start)).clicked(actions) {
        status.start(cx);
    }
    if root.button(cx, ids!(finish)).clicked(actions) {
        status.finish(cx);
    }
    if root.button(cx, ids!(fail)).clicked(actions) {
        status.fail(cx);
    }
    let saving = root.saving_indicator(cx, ids!(saving));
    if root.button(cx, ids!(save)).clicked(actions) || saving.retry(actions) {
        saving.saving(cx);
    }
    if root.button(cx, ids!(save_done)).clicked(actions) {
        saving.saved(cx);
    }
    if root.button(cx, ids!(save_fail)).clicked(actions) {
        saving.failed(cx);
    }
    if root.button(cx, ids!(overlay_toggle)).clicked(actions) {
        let overlay = root.loading_overlay(cx, ids!(overlay));
        let on = !overlay.is_active();
        overlay.set_active(cx, on);
    }
    if root.button(cx, ids!(blur_toggle)).clicked(actions) {
        let overlay = root.loading_overlay(cx, ids!(overlay_blur));
        let on = !overlay.is_active();
        overlay.set_active(cx, on);
    }
    if root.button(cx, ids!(under)).clicked(actions) {
        root.label(cx, ids!(blocked_note)).set_text(cx, "the button under the overlay: PRESSED");
    }
}

pub const STORIES: &[Story] = &[
    Story {
        key: "feedback/spinner/overview",
        category: "Feedback",
        component: "Spinner",
        also: &["LoadingOverlay", "SavingIndicator", "StatusSpinner"],
        name: "Overview",
        dsl: "SpinnerOverview",
        added: "2026-09-05",
        tags: &["new"],
        doc: "# Spinner\n\nThe mark that says \"working, no idea how long\". `SpinnerFlat` is a turning arc; `SpinnerDots` and `SpinnerBars` are the other faces, `SpinnerContained` puts a rounded container behind the arc. `size` is the side of the mark (12, 16, 24, 32, 48 make the xs..xl ladder), `text` a word beside it, and `delay_secs` keeps it invisible until a load has taken that long, so a quick one never flashes.\n\n`StatusSpinner` has an ending: `start`, `finish` and `fail` turn it, morph it into a tick or a cross, and after `auto_reset_secs` it goes quiet by itself, raising `Finished`, `Failed` and `Reset`.\n\n`SavingIndicator` is the status spinner with a label and a retry button: `saving` shows \"Saving\" only after `debounce_secs`, `saved` prints the clock, `failed` shows the retry button, which raises `Retry`.\n\n`LoadingOverlay` wraps content: while `active` it dims the content, claims every pointer event over it and centres a spinner on it, fading in after `delay_secs`; `blur` swaps the scrim for a glass pane.\n\n`LoadingSpinner`, the older DSL-only view, is unchanged.",
        subject: "status",
        feature: None,
        controls: &[],
        on_actions: Some(spinner_actions),
    },
    Story {
        key: "feedback/spinner/basic",
        category: "Feedback",
        component: "Spinner",
        also: &[],
        name: "Basic",
        dsl: "SpinnerBasic",
        added: "2026-09-05",
        tags: &["new", "controls"],
        doc: "# Spinner\n\nOne spinner under the controls: the face, the size of the mark, the delay before it shows, the word beside it and whether it sits in a container.",
        subject: "subject",
        feature: None,
        controls: &[
            Control { label: "Face", target: "subject", kind: ControlKind::Choice { prop: "face", options: &["SpinnerFace.Arc", "SpinnerFace.Dots", "SpinnerFace.Bars"], default: 0 } },
            Control { label: "Size", target: "subject", kind: ControlKind::Number { prop: "size", min: 8., max: 96., step: 1., default: 24. } },
            Control { label: "Delay (s)", target: "subject", kind: ControlKind::Number { prop: "delay_secs", min: 0., max: 3., step: 0.05, default: 0. } },
            Control { label: "Label", target: "subject", kind: ControlKind::Text { prop: "text", default: "Loading" } },
            Control { label: "Contained", target: "subject", kind: ControlKind::Bool { prop: "contained", default: false } },
            Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
        ],
        on_actions: None,
    },
    Story {
        key: "feedback/spinner/loading",
        category: "Feedback",
        component: "Spinner",
        also: &[],
        name: "Loading spinner",
        dsl: "SpinnerLoading",
        added: "2026-02-16",
        tags: &["ported"],
        doc: "# Loading spinner

The older `LoadingSpinner`: a DSL-only view whose ring, gap and turning speed are shader properties. Several apps override those properties, so it stays exactly as it was; new work uses the spinner family instead.",
        subject: "",
        feature: None,
        controls: &[],
        on_actions: None,
    },
];
