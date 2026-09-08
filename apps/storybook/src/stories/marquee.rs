//! The marquee story: a line that travels, and the cases a two-copy loop
//! gets wrong.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.MarqueeOverview = StoryPage{
        StoryNote{text: "A strip too narrow for its words can cut them, wrap them, or move them. This moves them, and the whole difficulty is the coming round: the line is drawn as many times as it takes to cover the strip, each copy a period further along, so nothing ever restarts where the eye can see it."}

        StoryHeading{text: "A line longer than its strip"}
        StoryNote{text: "The ordinary case: the words do not fit, so they travel. Rest the pointer on it and it holds still to be read."}
        View{
            width: 420. height: Fit
            subject := Marquee{
                text: "Every strip that cannot hold its words has to choose what to do about it — this one moves them."
            }
        }

        StoryHeading{text: "A line shorter than its strip"}
        StoryNote{text: "The case a two-copy marquee gets wrong. Two copies is only enough when the line is at least as wide as the strip; a short line in a wide one needs as many as fit and one more, or a hole crosses the strip once every cycle."}
        View{
            width: Fill height: Fit
            short := Marquee{
                text: "SHORT LINE"
                gap: 40.
            }
        }

        StoryHeading{text: "Which way, and how fast"}
        StoryRow{
            View{
                width: 300. height: Fit
                Marquee{text: "travelling left, the way reading goes"}
            }
            View{
                width: 300. height: Fit
                Marquee{text: "travelling right, against it" direction: MarqueeDirection.Right}
            }
        }
        StoryRow{
            View{
                width: 300. height: Fit
                Marquee{text: "slow, at fifteen points a second" speed: 15.}
            }
            View{
                width: 300. height: Fit
                Marquee{text: "quick, at a hundred and twenty" speed: 120.}
            }
        }

        StoryHeading{text: "Held still"}
        StoryNote{text: "Speed zero is a marquee that is not going anywhere: the same widget, so a host can stop one without swapping it for a label."}
        View{
            width: 420. height: Fit
            Marquee{text: "This one is not going anywhere until something sets its speed." speed: 0.}
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "containers/marquee/overview",
    category: "Containers",
    component: "Marquee",
    name: "Overview",
    dsl: "MarqueeOverview",
    added: "2026-09-08",
    tags: &["new"],
    doc: "# Marquee

A line of text that travels across its strip and comes round again without a seam: for a name too long for the room it has, or a status line with more to say than it can show at once.

**The loop has no seam because there is no single copy.** The line is drawn as many times as it takes to cover the strip at every phase of the travel, each copy one period further along, where a period is the line plus the `gap` after it. Nothing restarts — the offset walks from zero to one period and wraps, and a copy is always already in place where the previous one is leaving.

**How many copies is arithmetic, not two.** Two is enough only when the line is at least as wide as the strip. A short line in a wide strip needs as many as fit and one more; with two, a hole crosses the strip once every cycle. `copies_needed` is a free function with its own tests, because that is the whole difference between a marquee and a glitch.

`speed` is points per second, so the travel is the same on any machine; zero holds it still. `direction` picks the way it goes. `pause_on_hover` stops it under the pointer, since a line that cannot be read is not saying anything.

It carries text rather than arbitrary children on purpose: drawing the same child widget several times in one frame would give every copy but the last a hit rect that lies, and a moving target is not something to click at.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Text", target: "subject", kind: ControlKind::Text { prop: "text", default: "Every strip that cannot hold its words has to choose what to do about it — this one moves them." } },
        Control { label: "Speed", target: "subject", kind: ControlKind::Number { prop: "speed", min: 0., max: 300., step: 5., default: 40. } },
        Control { label: "Gap", target: "subject", kind: ControlKind::Number { prop: "gap", min: 0., max: 300., step: 4., default: 64. } },
        Control { label: "Pause on hover", target: "subject", kind: ControlKind::Bool { prop: "pause_on_hover", default: true } },
    ],
    on_actions: None,
}];
