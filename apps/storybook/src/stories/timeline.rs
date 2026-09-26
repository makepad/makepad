//! The timeline story: a run of events down a line, the ways they can be
//! ranged against it, and the line lying flat.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.TimelineOverview = StoryPage{
        StoryNote{text: "A history a reader scans: each event a bullet, a time, a heading and a body. The line runs between the bullets rather than through them, so a bullet reads as a stop on the way and not as a bead someone has drawn a wire over."}

        StoryHeading{text: "Down the page"}
        StoryNote{text: "The default. Every event on one side, the line hugging the other edge. The third event is marked as where things have got to: its bullet is wider and in the accent colour, and so is the line behind it."}
        delivery := Timeline{
            width: 420.
            at: 3
            events: [
                "09:04 | Order placed | Paid for online and queued for the morning run."
                "11:20 | Packed | Everything in one box, weighed and labelled."
                "14:05 | Collected | Handed over to the courier and on the road."
                "Tomorrow | Delivered | Between eight and noon, and a signature is needed. | ring"
            ]
        }

        StoryHeading{text: "Which side"}
        StoryNote{text: "Trailing puts the events after the line, Leading mirrors the whole thing and ranges the text right against it, Alternating puts the line down the middle and swings the events either side. The first event of an alternating run stays where a one-sided run would have put it."}
        StoryRow{
            align: Align{x: 0. y: 0.}
            spacing: theme.space_4
            mirrored := Timeline{
                width: 260.
                side: TimelineSide.Leading
                events: [
                    "Mon | Drafted | Written and left overnight."
                    "Tue | Reviewed | Two people read it through."
                    "Wed | Sent | Out of the door before lunch."
                ]
            }
            both_sides := TimelineAlternating{
                width: 420.
                at: 2
                events: [
                    "Mon | Drafted | Written and left overnight."
                    "Tue | Reviewed | Two people read it through."
                    "Wed | Sent | Out of the door before lunch."
                ]
            }
        }

        StoryHeading{text: "Bullets"}
        StoryNote{text: "A filled disc is the ordinary event, a ring one of a lesser kind or one still to come, and mark: puts a glyph inside the ring. The glyph is drawn from the ordinary text chain, so it has to be one the chain carries \u{2014} a letter, a digit, or a mark like \u{25b2}."}
        bullets := Timeline{
            width: 420.
            events: [
                "Step | Filled | The ordinary bullet, and the default. | dot"
                "Step | Hollow | A ring, with the middle left clear so it is right on any surface. | ring"
                "Step | Numbered | A ring with a digit in it, for a run of steps. | mark:3"
                "Step | Marked | Any short glyph the text chain carries. | mark:\u{25b2}"
            ]
        }

        StoryHeading{text: "Lying flat"}
        StoryNote{text: "The same layout turned ninety degrees: the line runs across and the events hang off it. Each event gets an equal share of the width and its text is centred under its own bullet. Alternating puts half of them above the line."}
        across := TimelineAcross{
            width: 620.
            at: 2
            events: [
                "v1.0 | Released | The first one anybody used."
                "v1.4 | Rewritten | The parser, and nothing else."
                "v2.0 | Planned | Not before the spring. | ring"
            ]
        }
        across_both := TimelineAcross{
            width: 620.
            side: TimelineSide.Alternating
            events: [
                "v1.0 | Released | The first one anybody used."
                "v1.4 | Rewritten | The parser, and nothing else."
                "v2.0 | Planned | Not before the spring. | ring"
                "v2.1 | Wanted | Everything left over. | ring"
            ]
        }

        StoryHeading{text: "What it does not hold"}
        StoryNote{text: "An event is text the timeline measures and draws itself, which is what lets a bullet sit on the middle of its own first line and lets a segment stop short of it. An event that needs a button in it is a row of widgets, not a timeline. Nothing here scrolls either: a history taller than the room goes inside a scrolling parent."}

        StoryHeading{text: "Driven"}
        StoryNote{text: "The run below is live. Press an event and it says which one; step the marker and watch the accent colour follow it along the line."}
        StoryRow{
            step := Button{text: "Step the marker"}
            picked := Label{text: "nothing pressed yet"}
        }
        subject := Timeline{
            width: 460.
            at: 2
            events: [
                "Queued | Written | The change was made and pushed."
                "Running | Built | Every target, from a clean tree."
                "Waiting | Reviewed | One pair of eyes so far. | ring"
                "Then | Merged | Once the second pair agrees. | ring"
            ]
        }
    }
}

fn timeline_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let run = root.timeline(cx, ids!(subject));
    if let Some(index) = run.picked(actions) {
        root.label(cx, ids!(picked))
            .set_text(cx, &format!("event {} pressed", index + 1));
    }
    if root.button(cx, ids!(step)).clicked(actions) {
        // Round through 0, which is the value that marks nothing: a run
        // that is just a history has to be able to say so.
        let next = (run.at() + 1) % (run.count() + 1);
        run.set_at(cx, next);
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "data-display/timeline/overview",
    category: "Data display",
    component: "Timeline",
    also: &["TimelineAcross", "TimelineAlternating"],
    name: "Overview",
    dsl: "TimelineOverview",
    added: "2026-09-10",
    tags: &["new"],
    doc: "# Timeline

A run of events in the order they happened: a delivery's progress, a release's milestones, an audit trail. Each event carries a time, a heading, a body and a bullet.

**The line runs between the bullets, not through them.** Every segment is a separate quad that starts clear of the bullet above it and stops clear of the one below, so a bullet reads as a stop on the way rather than a bead threaded onto a wire drawn over it. Two bullets too close together to leave room get no segment at all instead of one drawn backwards through both.

Events are written one to a string: `\"time | heading | body | bullet\"`. Everything after the time is optional. The bullet word is `dot` for a filled disc, `ring` for a hollow one, or `mark:X` for a ring with the glyph `X` in it; an unknown word falls back to a disc rather than drawing a mystery. A `|` cannot appear in the text, which is the price of a list a caller can write in one markup literal instead of four parallel ones. A host with real data calls `set_events` instead and skips the parsing.

`side` is `Trailing`, `Leading` or `Alternating`. `axis` is `Down` or `Across`; `Across` is the same layout turned ninety degrees, with each event taking an equal share of the width and its text centred under its own bullet.

`at` marks where things have got to, **counting from one**, and 0 marks nothing. One-based because with a 0-based index there is no value left to mean \"nothing is current\", and most timelines are a plain history that has to be able to say that. The marked event gets a wider bullet in the accent colour, and the line behind it is drawn in that colour too.

What it deliberately does not do: it holds no widgets, it does not scroll or recycle rows, it does not parse or format times, and it does not break a word wider than the room \u{2014} that word overruns on a line of its own, which reads better than the same word cut in half.

`parse_event`, `wrap_lines`, `link_spans` and `marked_index` are free functions with their own tests; the widget is only their drawing.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Marked", target: "subject", kind: ControlKind::Number { prop: "at", min: 0., max: 4., step: 1., default: 2. } },
        Control {
            label: "Axis",
            target: "subject",
            kind: ControlKind::Choice {
                prop: "axis",
                options: &["TimelineAxis.Down", "TimelineAxis.Across"],
                default: 0,
            },
        },
        Control {
            label: "Side",
            target: "subject",
            kind: ControlKind::Choice {
                prop: "side",
                options: &["TimelineSide.Trailing", "TimelineSide.Leading", "TimelineSide.Alternating"],
                default: 0,
            },
        },
        Control { label: "Bullet size", target: "subject", kind: ControlKind::Number { prop: "bullet_size", min: 6., max: 32., step: 1., default: 13. } },
        Control { label: "Line width", target: "subject", kind: ControlKind::Number { prop: "line_width", min: 1., max: 6., step: 0.5, default: 2. } },
        Control { label: "Gap", target: "subject", kind: ControlKind::Number { prop: "gap", min: 4., max: 48., step: 1., default: 12. } },
        Control { label: "Event gap", target: "subject", kind: ControlKind::Number { prop: "event_gap", min: 0., max: 64., step: 1., default: 14. } },
    ],
    on_actions: Some(timeline_actions),
}];
