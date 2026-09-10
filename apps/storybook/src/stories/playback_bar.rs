//! The playback bar story: a track you can press to move through something
//! timed, and the three things that separate it from a slider.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.PlaybackBarOverview = StoryPage{
        StoryNote{text: "Where you are in something timed, and a press that moves you there. The press target is the whole row below the clock, not the thin track; the host's reported position is ignored while a finger is down; and a drag reports one seek per interval at its newest target rather than one per pointer move."}

        StoryHeading{text: "A bar under the controls"}
        StoryNote{text: "Press anywhere on the row to jump there, or drag along it. This page plays the part of the host: it moves the position to wherever the bar asks, which is what a real player would do a moment later."}
        StoryRow{
            width: Fill
            subject := mod.widgets.PlaybackBar{
                width: Fill
                duration: 225.0
                position: 62.0
            }
        }
        StoryRow{
            seek_note := Label{text: "no seek yet"}
            scrub_note := Label{text: ""}
        }
        StoryRow{
            back := Button{text: "Back 15s"}
            on := Button{text: "On 15s"}
        }

        StoryHeading{text: "Counting down"}
        StoryNote{text: "`show_remaining` turns the right-hand clock into what is left. The two clocks always carry the same fields, so an hour-long piece prints 0:04:12 beside 1:12:30 rather than changing width on the way past the hour."}
        StoryRow{
            width: Fill
            mod.widgets.PlaybackBar{
                width: Fill
                duration: 4350.0
                position: 252.0
                show_remaining: true
            }
        }

        StoryHeading{text: "Without the clock"}
        StoryNote{text: "`show_clock: false` and `label_height: 0.` for a bar whose host already prints the times. The press band is then the whole widget, top to bottom, so give it a height worth aiming at even though the track stays thin."}
        StoryRow{
            width: Fill
            mod.widgets.PlaybackBar{
                width: Fill
                height: 22
                duration: 180.0
                position: 121.0
                show_clock: false
                label_height: 0.
            }
        }

        StoryHeading{text: "A length nobody has said"}
        StoryNote{text: "`duration: 0.` is unknown, not zero. The track draws empty, the right-hand clock is left off rather than printing a total that would be a lie, and every gesture is refused — a seek into something of unknown length has no destination to name."}
        StoryRow{
            width: Fill
            mod.widgets.PlaybackBar{
                width: Fill
                duration: 0.0
                position: 37.0
            }
        }

        StoryHeading{text: "Disabled"}
        StoryNote{text: "A disabled bar still says where the playhead is — the fill walks halfway to the track rather than vanishing — but it drops the knob and refuses the pointer and the keyboard."}
        StoryRow{
            width: Fill
            mod.widgets.PlaybackBar{
                width: Fill
                duration: 225.0
                position: 150.0
                animator +: {disabled: {default: @on}}
            }
        }

        StoryHeading{text: "The ladder"}
        StoryRow{
            width: Fill
            mod.widgets.PlaybackBarFlat{
                width: Fill
                duration: 225.0
                position: 90.0
            }
        }
        StoryRow{
            width: Fill
            mod.widgets.PlaybackBar{
                width: Fill
                duration: 225.0
                position: 90.0
            }
        }
    }
}

fn playback_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let bar = root.playback_bar(cx, ids!(subject));
    if let Some(seconds) = bar.seek(actions) {
        // What a player does: go there, then report back. The bar ignores
        // the report while the finger is still down and takes it the moment
        // the finger lifts, which is the whole point of the hold.
        bar.set_position(cx, seconds);
        root.label(cx, ids!(seek_note))
            .set_text(cx, &format!("seek to {}", format_clock(seconds)));
    }
    if let Some(seconds) = bar.scrubbed(actions) {
        root.label(cx, ids!(scrub_note))
            .set_text(cx, &format!("showing {}", format_clock(seconds)));
    }
    if root.button(cx, ids!(back)).clicked(actions) {
        bar.set_position(cx, (bar.shown_seconds() - 15.0).max(0.0));
    }
    if root.button(cx, ids!(on)).clicked(actions) {
        bar.set_position(cx, (bar.shown_seconds() + 15.0).min(bar.duration()));
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "media/playbackbar/overview",
    category: "Media",
    component: "PlaybackBar",
    also: &["PlaybackBarFlat"],
    name: "Overview",
    dsl: "PlaybackBarOverview",
    added: "2026-09-10",
    tags: &["new", "controls", "playback", "position", "clock", "duration", "media"],
    doc: "# PlaybackBar\n\nHow far into something timed you are, and a press that moves you. `duration` is the whole length in seconds and `position` is where the playhead is; both are plain seconds, so nothing has to be converted on the way in.\n\n## Why not a slider\n\nA slider's drag is *relative* — a press at seven tenths of the track moves by seven tenths of nothing, it does not go to seven tenths — and its double tap resets to whatever the source said, which on a playhead means jumping wherever the script happened to write. A playhead is absolute in both gestures. The progress bar is the other half of the family and refuses input by charter; this is the one that takes it.\n\n## The three things it does that a hand-written bar keeps forgetting\n\n| | |\n|---|---|\n| the press band is the whole row below the clock | a four-pixel track cannot be hit by a moving pointer, and never by a finger |\n| the host is ignored while a finger is down | a player keeps reporting where it is really playing, which is behind the finger; letting that through makes the playhead flick between the two |\n| a seek is coalesced to its newest target | a drag makes a target per pointer move, and every one but the last is already overtaken by the time a decoder could act on it |\n\nThe hold outlasts the release by `settle_secs`, because a seek takes time to land and until it does the host still reports the old position. It ends early the moment the host reports within `settle_tolerance` of where it was sent, and it always ends — a stream that cannot seek must not be able to freeze the playhead for the rest of the session.\n\n## Reading it\n\n`seek` is the one to act on: already coalesced, so a player may take every one at face value. `scrubbed` fires on every pointer move and is for cheap things — a caption, a preview. `grabbed` and `released` bracket a drag, for a host that pauses while the playhead is being moved. A keyboard seek raises `scrubbed` and `seek` but neither `grabbed` nor `released`, since a key press is one whole intent rather than a gesture with two ends.\n\n## Free functions\n\n`format_clock(seconds)` is the clock this library prints: `M:SS`, growing an hours field at the hour, floored rather than rounded so it never reads a second that has not played. `fraction_at(x, track_x, track_w)` is where a press falls along a track — the line every hand-written copy wrote out again, including the divide-by-zero it forgot.\n\n## Traps\n\n- `duration: 0.` means *unknown*, not empty: the bar draws a bare track, leaves the total off the clock rather than printing a zero, and refuses every gesture.\n- The clock row is not part of the press band, so pressing the total does not throw the playhead to the end.\n- It does not answer the wheel. A bar inside a scrolling page that seeked on scroll would fling the playhead every time the page moved.\n- It has no auto-hide. A bar that fades over a picture is the host's business, and a separate widget.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Length", target: "subject", kind: ControlKind::Number { prop: "duration", min: 0., max: 7200., step: 15., default: 225. } },
        Control { label: "Position", target: "subject", kind: ControlKind::Number { prop: "position", min: 0., max: 7200., step: 1., default: 62. } },
        Control { label: "Clock", target: "subject", kind: ControlKind::Bool { prop: "show_clock", default: true } },
        Control { label: "Count down", target: "subject", kind: ControlKind::Bool { prop: "show_remaining", default: false } },
        Control { label: "Track thickness", target: "subject", kind: ControlKind::Number { prop: "draw_bg.thickness", min: 1., max: 24., step: 0.5, default: 4. } },
        Control { label: "Knob", target: "subject", kind: ControlKind::Number { prop: "draw_bg.knob_size", min: 0., max: 32., step: 0.5, default: 11. } },
        Control { label: "Seek interval", target: "subject", kind: ControlKind::Number { prop: "seek_interval", min: 0., max: 1., step: 0.01, default: 0.12 } },
        Control { label: "Arrow key step", target: "subject", kind: ControlKind::Number { prop: "step_secs", min: 0.5, max: 60., step: 0.5, default: 5. } },
        Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
    ],
    on_actions: Some(playback_actions),
}];
