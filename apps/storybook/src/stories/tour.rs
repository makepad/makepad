//! The tour story: a run of steps, each pointing at something on the page.
use crate::makepad_widgets::tour::TourWidgetRefExt;
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.TourOverview = StoryPage{
        StoryNote{text: "A tour dims the page, cuts a hole around the one thing the step is about, and puts a card beside the hole. Press Start. Next and Back walk the run, Skip leaves it, and Escape does the same from the keyboard."}

        StoryHeading{text: "A run over this page"}
        StoryRow{
            start := Button{text: "Start the tour"}
            state := Label{text: "not running"}
        }

        StoryHeading{text: "The things it points at"}
        StoryNote{text: "Each step names its target by id, and the target is looked up when the step is reached — so the same tour can be written beside a page whose controls come and go."}
        StoryRow{
            pick := Button{text: "Pick a colour"}
            amount := Slider{width: 200. text: "Amount"}
        }

        StoryHeading{text: "A target that is not there"}
        StoryNote{text: "The second step names a button that is declared but never drawn. Rather than cut a hole around nothing, the tour steps over it: the run is three steps long and the card says so."}
        StoryRow{
            // Never drawn, so the step naming it has nothing to point at.
            ghost := Button{visible: false text: "Gone"}
            Label{width: Fit text: "the step for this one is skipped" draw_text.color: theme.color_text_meta}
        }

        StoryHeading{text: "Whichever side has room"}
        StoryNote{text: "The card is placed against the hole, not in a fixed corner: below the target where there is room below, above it near the foot of the window, and beside it when the target is tall enough to leave neither. Move the window's edges and step through again to see it change."}
        StoryRow{
            send := Button{text: "Send"}
        }

        // Last on the page, and claiming no room in it: the tour paints on
        // its own overlay over the whole window.
        subject := Tour{
            steps: [
                TourStep{
                    target: "pick"
                    heading: "Start here"
                    body: "This is the control the first step is about. The ring is drawn round its own rect, grown a little, so it is never touching the dim."
                }
                TourStep{
                    target: "ghost"
                    heading: "You will not see this"
                    body: "Its target is never drawn, so this step is stepped over on the way past."
                }
                TourStep{
                    target: "amount"
                    heading: "Then this one"
                    body: "The card takes whichever side of the hole has room for it. Near the top of the window it sits below; near the bottom, above; beside a full-height panel, to one side."
                }
                TourStep{
                    target: "send"
                    heading: "And it ends here"
                    body: "Last step, so the onward button says Done rather than Next. Escape leaves at any point."
                }
            ]
        }
    }
}

fn tour_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let tour = root.tour(cx, ids!(subject));
    if root.button(cx, ids!(start)).clicked(actions) {
        tour.start(cx);
    }
    let state = root.label(cx, ids!(state));
    if let Some(i) = tour.stepped(actions) {
        // The index into the steps as written, which is not the number the
        // card shows: the card counts the ones with a target.
        state.set_text(cx, &format!("on the step written {}", i + 1));
    }
    if tour.finished(actions) {
        state.set_text(cx, "ran to the end");
    }
    if tour.skipped(actions) {
        state.set_text(cx, "left early");
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "overlay/tour/overview",
    category: "Overlay",
    component: "Tour",
    also: &["TourCard", "TourStep"],
    name: "Overview",
    dsl: "TourOverview",
    added: "2026-09-10",
    tags: &[
        "new",
        "tour",
        "walkthrough",
        "onboarding",
        "coach marks",
        "spotlight",
        "highlight",
        "steps",
        "guide",
        "first run",
    ],
    doc: "# Tour\n\nA run of steps, each pointing at something already on the screen. Everything else is dimmed, a hole is cut around the step's target, and the card sits beside the hole.\n\n## Why the hole, and not an arrow\n\nThe thing a tour has to get right is the pointing: the reader must be sure, without being told twice, which control the words are about. A card parked in a corner with a line drawn to the target fails the moment the target is near that corner. A hole in a dim screen cannot be misread, and it also answers \"what else can I press\" — nothing.\n\n## Writing one\n\n```\nTour{\n    steps: [\n        TourStep{target: \"send\" heading: \"Send it\" body: \"...\"}\n        TourStep{target: \"panel.filters\" heading: \"Narrow it down\" body: \"...\"}\n    ]\n}\n```\n\n`target` is a dotted id path, searched for from the tour outward, so a tour declared beside the page it describes finds that page's own ids. Declare the tour last among its siblings: it claims no room in the layout and paints on its own overlay.\n\n## A target that is not there is stepped over\n\nA tour outlives the page it was written for. A control gets hidden behind a setting, a row is empty today, a panel is off in this build. A step whose target is not in the tree, or is in the tree and has never been drawn, is skipped — and the count on the card counts the steps the reader will actually see, so a run of five with two targets missing says \"1 of 3\", not \"1 of 5\". A tour with no target at all does not start.\n\n## Advancing scrolls first\n\nA target inside a scrolling panel may be nowhere near the viewport, so every scrolling ancestor is asked to bring it in before the card is placed. The hole follows the target while that scroll settles. A panel the tour is declared *inside* is the exception — it is mid-dispatch the whole time the tour is running code and cannot be asked anything, which is the other reason to declare a tour beside the panels it points into rather than within one.\n\n## Driving it\n\n`start`, `stop`, `next` and `back` from Rust or from the script layer; `started`, `stepped`, `finished` and `skipped` report back. Next, Back and Skip are on the card; Return and the right arrow advance, the left arrow goes back, and Escape or the back gesture leaves.\n\n## What it does not do\n\nIt does not let the reader operate the thing it is pointing at — while a tour runs, its own card holds the pointer. A sequence whose steps can be answered out of order is a checklist, and wants a different widget. It also does not remember whether it has been shown; that belongs to whatever knows who the reader is.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Card width", target: "subject", kind: ControlKind::Number { prop: "card_width", min: 200., max: 520., step: 10., default: 300. } },
        Control { label: "Gap to the hole", target: "subject", kind: ControlKind::Number { prop: "gap", min: 0., max: 48., step: 1., default: 12. } },
        Control { label: "Room round the target", target: "subject", kind: ControlKind::Number { prop: "pad", min: 0., max: 24., step: 1., default: 6. } },
        Control { label: "Room at the edges", target: "subject", kind: ControlKind::Number { prop: "edge", min: 0., max: 48., step: 1., default: 12. } },
    ],
    on_actions: Some(tour_actions),
}];
