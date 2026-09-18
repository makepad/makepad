//! An overlay that can say the load STOPPED, with the comet face beside it.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.OverlayStopped = StoryPage{
        StoryNote{text: "A busy overlay that can only say \"working\" leaves a host whose load fails with one move: tear the overlay down and put something else in the hole. Meanwhile a scrim that stays up with an arc turning on it says the machine is working when it has stopped. `failed` morphs the arc into a cross and leaves it there."}

        StoryHeading{text: "Working, then stopped"}
        StoryNote{text: "Load puts the overlay up. Fail leaves it up and turns the arc into a cross. Clear takes the whole thing down, and the failure goes with it — a failure belongs to the load that failed."}
        StoryRow{
            load := Button{text: "Load"}
            fail := Button{text: "Fail"}
            clear := Button{text: "Clear"}
            under := Button{text: "The button underneath"}
            blocked := Label{text: "the button under the overlay: not pressed"}
        }
        StoryRow{
            subject := LoadingOverlay{
                width: 320.
                height: 140.
                text: "Loading"
                text_failed: "Could not load"
                spinner: SpinnerComet{
                    size: 30.
                }
                content := View{
                    width: Fill
                    height: Fill
                    show_bg: true
                    draw_bg.color: theme.color_surface_container_high
                    padding: theme.mspace_2
                    flow: Down
                    spacing: theme.space_2
                    Label{text: "Twelve rows of something"}
                    Button{text: "A blocked button"}
                }
            }
            glassy := LoadingOverlay{
                width: 320.
                height: 140.
                blur: true
                text: "Loading"
                text_failed: "Could not load"
                View{
                    width: Fill
                    height: Fill
                    show_bg: true
                    draw_bg.color: theme.color_surface_container_high
                    padding: theme.mspace_2
                    flow: Down
                    spacing: theme.space_2
                    Label{text: "The same, behind glass"}
                    Button{text: "Also blocked"}
                }
            }
        }

        StoryHeading{text: "The comet"}
        StoryNote{text: "A fourth face: one bright head dragging a tail that fades out behind it. The head LEADS — the sweep is worked out as phase minus angle, and the other way round the tail sits in front of the head and the whole mark reads as turning backwards."}
        StoryRow{
            SpinnerComet{size: 16.}
            SpinnerComet{size: 24.}
            SpinnerComet{size: 32.}
            SpinnerComet{size: 48.}
            SpinnerComet{size: 32. text: "Loading"}
            SpinnerComet{size: 32. draw_bg.color: theme.color_success}
            SpinnerComet{size: 32. draw_bg.track_alpha: 0.0}
        }

        StoryHeading{text: "Beside the other three"}
        StoryRow{
            SpinnerFlat{size: 32.}
            SpinnerDots{size: 32.}
            SpinnerBars{size: 32.}
            SpinnerComet{size: 32.}
        }
    }
}

fn overlay_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let plain = root.loading_overlay(cx, ids!(subject));
    let glassy = root.loading_overlay(cx, ids!(glassy));
    if root.button(cx, ids!(load)).clicked(actions) {
        plain.set_failed(cx, false);
        plain.set_active(cx, true);
        glassy.set_failed(cx, false);
        glassy.set_active(cx, true);
    }
    if root.button(cx, ids!(fail)).clicked(actions) {
        plain.fail(cx);
        glassy.fail(cx);
    }
    if root.button(cx, ids!(clear)).clicked(actions) {
        plain.set_active(cx, false);
        glassy.set_active(cx, false);
    }
    if root.button(cx, ids!(under)).clicked(actions) {
        root.label(cx, ids!(blocked)).set_text(cx, "the button under the overlay: PRESSED");
    }
}


pub const STORIES: &[Story] = &[
    Story {
        key: "feedback/spinner/stopped-and-comet",
        category: "Feedback",
        component: "Spinner",
        also: &["LoadingOverlay", "SpinnerComet"],
        name: "Stopped, and the comet",
        dsl: "OverlayStopped",
        added: "2026-09-18",
        tags: &["new", "loading", "failed", "error", "overlay", "comet"],
        doc: "# A loading overlay that can fail\n\n`LoadingOverlay` could say \"working\" and nothing else. A host whose load failed had one move — tear the overlay down and put something else in the hole — and until it did, a scrim sitting over the content with an arc still turning on it said the machine was working when it had stopped.\n\n`fail(cx)` puts the overlay up if it is not already, closes the arc into a ring and fades a cross into it over the status spinner's own morph. `text_failed` replaces the word under the mark, and falls back to `text` when it is empty, so a host that only wants one word writes one. `set_active(cx, false)` takes the overlay down and clears the failure with it: a failure belongs to the load that failed, and the next load starts clean.\n\nThe failure face is a whole `StatusSpinner` rather than a second shader — the arc-to-cross morph, its easing and its ink all already lived on that widget, and this is the thing it was written for. The blurred pane keeps its own pair, because the glass composites above anything its parent draws after it.\n\n**A stopped mark must never be mistaken for a slow one.** That is the rule the whole state exists for, and the library already follows it elsewhere: the media widget gives its missing-picture slot a STILL placeholder, commented \"still, because nothing is coming\".\n\n## Recycled rows\n\n`set_active(cx, true)` on an overlay that is already active is deliberately not a flip — otherwise a host calling it every draw would restart the delay every frame and the overlay would never appear. A recycled list row is the case that costs: its previous occupant left the overlay up, the new row is loading too, and nothing about the flip says the row changed hands, so the new row silently inherits the old one's finished fade and its own delay never runs. Call `restart(cx)` when a row is re-seated.\n\n# The comet\n\nA fourth face beside the arc, the dots and the bars: one bright head dragging a tail that fades out behind it, worked out per pixel rather than stamped as a row of dots so it stays smooth at any size. Wanted where the mark is the only thing moving on the screen — a splash, a full-window overlay — and the arc's breathing gap reads as a stutter.\n\nIts one hard-won detail: the sweep runs **phase minus angle**. The bright end has to be the LEADING one; swap the two and the tail sits in front of the head and the whole mark reads as a spinner turning backwards. `comet_tail` carries the same rule in Rust so it can be checked without a window.",
        subject: "subject",
        feature: None,
        controls: &[
            Control { label: "Active", target: "subject", kind: ControlKind::Bool { prop: "active", default: false } },
            Control { label: "Failed", target: "subject", kind: ControlKind::Bool { prop: "failed", default: false } },
            Control { label: "Word", target: "subject", kind: ControlKind::Text { prop: "text", default: "Loading" } },
            Control { label: "Word once failed", target: "subject", kind: ControlKind::Text { prop: "text_failed", default: "Could not load" } },
            Control { label: "Blur the content", target: "subject", kind: ControlKind::Bool { prop: "blur", default: false } },
            Control { label: "Delay (s)", target: "subject", kind: ControlKind::Number { prop: "delay_secs", min: 0., max: 3., step: 0.05, default: 0. } },
        ],
        on_actions: Some(overlay_actions),
    },
];
