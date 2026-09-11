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
        StoryNote{text: "`show_remaining` turns the right-hand clock from the whole length into what is left of it, counting down as the piece plays. Both clocks carry the same fields whatever their own values are, so this one prints 0:04:12 beside -1:08:18 rather than gaining a field and changing width the moment the elapsed time crosses the hour."}
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
        StoryNote{text: "`show_clock: false` for a bar whose host already prints the times. It gives the clock's strip back rather than leaving a dead band along the top, so the press target is then the whole widget, top to bottom — give it a height worth aiming at even though the track stays thin."}
        StoryRow{
            width: Fill
            mod.widgets.PlaybackBar{
                width: Fill
                height: 22
                duration: 180.0
                position: 121.0
                show_clock: false
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
        StoryNote{text: "A disabled bar still says where the playhead is. The knob stays, shrunk to the track's own thickness — a mark rather than a grip — because knob against ring is the one pair on this bar that clears the contrast a graphical object needs in all three themes, and the fill boundary alone does not. The fill walks halfway to the track and the clock goes to the muted label colour; the pointer and the keyboard are refused."}
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
        StoryNote{text: "Two rungs, one number apart: `PlaybackBar` is the flat face with the theme's inset bevel stroked around the track. Both are drawn at the same 1:30 of 3:45, so the difference on the screen is the whole difference between them."}
        StoryRow{
            width: Fill
            Label{width: 130. text: "PlaybackBarFlat"}
            mod.widgets.PlaybackBarFlat{
                width: Fill
                duration: 225.0
                position: 90.0
            }
        }
        StoryRow{
            width: Fill
            Label{width: 130. text: "PlaybackBar"}
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
    doc: "# PlaybackBar\n\nHow far into something timed you are, and a press that moves you. `duration` is the whole length in seconds and `position` is where the playhead is; both are plain seconds, so nothing has to be converted on the way in.\n\n## Why not a slider\n\nA slider's drag is *relative* — a press at seven tenths of the track moves by seven tenths of nothing, it does not go to seven tenths — and its double tap resets to whatever the source said, which on a playhead means jumping wherever the script happened to write. A playhead is absolute in both gestures. The progress bar is the other half of the family and refuses input by charter; this is the one that takes it.\n\n## The three things it does that a hand-written bar keeps forgetting\n\n| What it does | Why |\n|---|---|\n| the press band is the whole row below the clock | a four-pixel track cannot be hit by a moving pointer, and never by a finger |\n| the host is ignored while a finger is down | a player keeps reporting where it is really playing, which is behind the finger; letting that through makes the playhead flick between the two |\n| a seek is coalesced to its newest target | a drag makes a target per pointer move, and every one but the last is already overtaken by the time a decoder could act on it |\n\nThe hold outlasts the release by `settle_secs`, because a seek takes time to land and until it does the host still reports the old position. It ends early the moment the host reports within `settle_tolerance` of where it was sent, and it always ends — a stream that cannot seek must not be able to freeze the playhead for the rest of the session.\n\n## Reading it\n\n`seek` is the one to act on: already coalesced, so a player may take every one at face value. `scrubbed` fires on every pointer move and is for cheap things — a caption, a preview. `grabbed` and `released` bracket a drag, for a host that pauses while the playhead is being moved. A keyboard seek raises `scrubbed` and `seek` but neither `grabbed` nor `released`, since a key press is one whole intent rather than a gesture with two ends.\n\nOne press lands THREE of them in a single pass — `grabbed`, then `scrubbed`, then `seek` — so every reader here scans the pass rather than taking its first action. Asking two of them in the same handler works, and the order they are asked in does not matter.\n\n## Free functions\n\n`format_clock(seconds)` is the clock this library prints: `M:SS`, growing an hours field at the hour, floored rather than rounded so it never reads a second that has not played. `fraction_at(x, track_x, track_w)` is where a press falls along a track — the line every hand-written copy wrote out again, including the divide-by-zero it forgot.\n\n## Traps\n\n- `duration: 0.` means *unknown*, not empty: the bar draws a bare track, leaves the total off the clock rather than printing a zero, and refuses every gesture.\n- The clock row is not part of the press band, so pressing the total does not throw the playhead to the end.\n- `show_clock: false` hands that row back as well as hiding the text — there is no second property to set, and no dead strip left refusing presses along the top.\n- The knob's centre is the very x a press there maps back to, so what is drawn is what is grabbable. `track_inset` is the room that buys: the track is held in from the widget's edges far enough for the knob's ring to clear them at both ends of the travel. Widen the knob past that room and the ring is what the edge cuts, so widen the inset with it.\n- Disabled, the knob shrinks to a mark but does not go: the elapsed fill against the track is barely over 1.7:1 in the dark theme even at full strength, so the fill was never the thing carrying the answer.\n- It does not answer the wheel. A bar inside a scrolling page that seeked on scroll would fling the playhead every time the page moved.\n- It has no auto-hide. A bar that fades over a picture is the host's business, and a separate widget.",
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
