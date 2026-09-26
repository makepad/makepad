//! The carousel story: a strip of items that scrolls sideways and stops on
//! whole ones.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.CarouselOverview = StoryPage{
        StoryNote{text: "A row of items wider than the room it has. Drag it, press the arrows at its sides, press a dot beneath it, hold shift and use the wheel, or use the arrow keys. Wherever a gesture leaves it, it settles on whole items — nothing is left cut in half against an edge."}

        StoryHeading{text: "One at a time"}
        StoryRow{
            width: Fill
            subject := Carousel{
                width: Fill
                items: [
                    CarouselItem{text: "Harbour at dawn" detail: "1 of 5"}
                    CarouselItem{text: "The long bridge" detail: "2 of 5"}
                    CarouselItem{text: "Rain on the quay" detail: "3 of 5"}
                    CarouselItem{text: "Low tide" detail: "4 of 5"}
                    CarouselItem{text: "Last light" detail: "5 of 5"}
                ]
            }
        }
        StoryRow{
            where_note := Label{text: "1 of 5"}
            chosen_note := Label{text: "press a card to choose it"}
        }

        StoryHeading{text: "Several at a time"}
        StoryNote{text: "`per_view` is how many items share the viewport. The item width is whatever the arrows, the peeks and the gaps leave, divided between them, so the strip fits the room it is given rather than the room it wanted."}
        StoryRow{
            width: Fill
            Carousel{
                width: Fill
                per_view: 3
                gap: 10.0
                item_height: 92.0
                items: [
                    CarouselItem{text: "One"}
                    CarouselItem{text: "Two"}
                    CarouselItem{text: "Three"}
                    CarouselItem{text: "Four"}
                    CarouselItem{text: "Five"}
                    CarouselItem{text: "Six"}
                    CarouselItem{text: "Seven"}
                    CarouselItem{text: "Eight"}
                ]
            }
        }

        StoryHeading{text: "One large one, with the neighbours showing"}
        StoryNote{text: "`peek` is how much of the items either side stays in view. It is what says there is more to the strip without giving a whole card to saying it — and the tails at the edges are also where the finger reaches for the next one."}
        StoryRow{
            width: Fill
            Carousel{
                width: Fill
                peek: 56.0
                item_height: 132.0
                items: [
                    CarouselItem{text: "Wide" detail: "with a tail of each neighbour"}
                    CarouselItem{text: "Wider" detail: "and the same again"}
                    CarouselItem{text: "Widest" detail: "and once more"}
                ]
            }
        }

        StoryHeading{text: "Full width"}
        StoryNote{text: "One item, no peek and no gap: each item has the viewport to itself. The three layouts everyone names are these two numbers and nothing else."}
        StoryRow{
            width: Fill
            Carousel{
                width: Fill
                gap: 0.0
                item_height: 108.0
                items: [
                    CarouselItem{text: "Edge to edge" color: #x2E4057FF}
                    CarouselItem{text: "And the next one" color: #x3F5E4EFF}
                    CarouselItem{text: "And the one after" color: #x5A3E4EFF}
                ]
            }
        }

        StoryHeading{text: "It comes round, or it stops"}
        StoryNote{text: "`loop_items: true` puts the first item after the last one, and the arrows never go dim. With ends, the strip stops dead at them — no rubber band, because on a strip of a handful of cards the stretch would be most of the control and would read as the carousel having lost its place."}
        StoryRow{
            width: Fill
            Carousel{
                width: 300.
                item_height: 84.0
                loop_items: true
                items: [
                    CarouselItem{text: "Comes round"}
                    CarouselItem{text: "and round"}
                    CarouselItem{text: "and round again"}
                ]
            }
            Carousel{
                width: 300.
                item_height: 84.0
                items: [
                    CarouselItem{text: "Stops"}
                    CarouselItem{text: "at"}
                    CarouselItem{text: "the end"}
                ]
            }
        }

        StoryHeading{text: "The dots count stops, not items"}
        StoryNote{text: "Six items three at a time gives four places the strip can rest, so there are four dots. Six would be a lie: two of them could never be reached, and pressing one would light a different dot from the one pressed."}
        StoryRow{
            width: Fill
            stops := Carousel{
                width: Fill
                per_view: 3
                item_height: 80.0
                items: [
                    CarouselItem{text: "1"}
                    CarouselItem{text: "2"}
                    CarouselItem{text: "3"}
                    CarouselItem{text: "4"}
                    CarouselItem{text: "5"}
                    CarouselItem{text: "6"}
                ]
            }
        }
        StoryRow{
            stops_note := Label{text: "6 items, 3 at a time, 4 dots"}
        }

        StoryHeading{text: "Without the arrows, without the dots"}
        StoryNote{text: "Both are optional and neither is load-bearing: the strip still drags, still takes the keyboard and still snaps. Drop the arrows where the gesture is the whole point, and drop the dots where the strip is long enough that counting them tells nobody anything."}
        StoryRow{
            width: Fill
            Carousel{
                width: 300.
                item_height: 84.0
                show_arrows: false
                items: [
                    CarouselItem{text: "Dots only" detail: "drag it"}
                    CarouselItem{text: "Still snaps"}
                    CarouselItem{text: "Still takes keys"}
                ]
            }
            Carousel{
                width: 300.
                item_height: 84.0
                show_dots: false
                items: [
                    CarouselItem{text: "Arrows only" detail: "press the sides"}
                    CarouselItem{text: "Still snaps"}
                    CarouselItem{text: "Still takes keys"}
                ]
            }
        }

        StoryHeading{text: "Disabled"}
        StoryRow{
            width: Fill
            Carousel{
                width: Fill
                item_height: 80.0
                items: [
                    CarouselItem{text: "Off"}
                    CarouselItem{text: "Also off"}
                ]
                animator +: {disabled: {default: @on}}
            }
        }

        StoryHeading{text: "The ladder"}
        StoryRow{
            width: Fill
            CarouselFlat{
                width: Fill
                item_height: 80.0
                items: [
                    CarouselItem{text: "CarouselFlat"}
                    CarouselItem{text: "the same strip"}
                    CarouselItem{text: "without the bevel"}
                ]
            }
        }
    }
}

/// The captions of the `subject` strip, so the readout can name the card
/// that was pressed. The carousel reports an index; the words belong to
/// whoever wrote them.
const SHOTS: &[&str] =
    &["Harbour at dawn", "The long bridge", "Rain on the quay", "Low tide", "Last light"];

fn carousel_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let strip = root.carousel(cx, ids!(subject));
    if let Some(page) = strip.paged(actions) {
        root.label(cx, ids!(where_note))
            .set_text(cx, &format!("{} of {}", page + 1, strip.stops()));
    }
    if let Some(index) = strip.pressed(actions) {
        root.label(cx, ids!(chosen_note))
            .set_text(cx, SHOTS.get(index).copied().unwrap_or(""));
    }
    // The four-dot strip says which of its stops it is on, which is the
    // point of that section: the number never reaches five or six.
    let stops = root.carousel(cx, ids!(stops));
    if let Some(page) = stops.paged(actions) {
        root.label(cx, ids!(stops_note))
            .set_text(cx, &format!("6 items, 3 at a time, stop {} of 4", page + 1));
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "containers/carousel/overview",
    category: "Containers",
    component: "Carousel",
    also: &["CarouselFlat", "CarouselItem"],
    name: "Overview",
    dsl: "CarouselOverview",
    added: "2026-09-10",
    tags: &["new", "slider", "gallery", "strip", "slideshow", "swipe", "snap", "paging"],
    doc: "# Carousel\n\nA row of items wider than the room it has, moved by dragging it, by the arrows at its sides, by the dots beneath it, by the wheel or by the keyboard. Wherever a gesture leaves it, it settles on a whole number of items, so nothing is ever left cut in half against an edge. That one rule is the widget; the rest is settings.\n\n## The three layouts are two numbers\n\n`per_view` is how many items share the viewport and `peek` is how much of the neighbours shows at each side.\n\n| Layout | Settings |\n|---|---|\n| several at a time | `per_view: 3` |\n| one large one, neighbours peeking | `per_view: 1`, `peek: 56` |\n| full width | `per_view: 1`, no peek, no gap |\n\nThey are settings and not three named modes, because a mode would have to rule on what `per_view: 2` with a peek means, and there is nothing there to rule on. The item width falls out of them: whatever the arrows, the peeks and the gaps leave, divided between the items on show.\n\n## The dots count stops, not items\n\nWith six items three at a time there are four places the strip can come to rest, so there are four dots. Six would be a lie — two of them could never be reached, and pressing one would light a different dot from the one pressed. A strip that comes round can lead with any of its items, so there the two counts happen to agree. When there are more stops than dots that fit across the widget, the dots are dropped rather than crowded: thirty dots is not a control anyone can aim at, and the arrows still carry the strip.\n\n## Landing on an item\n\nA flick does not run its momentum out and then jerk to the nearest item. The release velocity predicts where a strip decaying at the library's scroll rate would stop, that prediction is snapped to an item, and the strip glides there opening at the speed the finger left it with — the same model the drum picker uses, so a flick feels the same wherever it is made.\n\nThe position is held in items rather than in pixels. A position in pixels would not survive a resize: the pitch changes with the width, and a strip resting on item four would come back from a resize standing between two of them.\n\n## The gestures\n\n| Gesture | Result |\n|---|---|\n| drag the strip | moves it |\n| let go while moving | carries on, and settles on whole items |\n| press a moving strip | stops it where it stands, then settles |\n| press a card in full view | reports it — the strip does not move |\n| press a card the edge is cutting | scrolls it in; it was being pointed at, not chosen |\n| an arrow, or a dot | one stop, or that stop |\n| sideways wheel, or shift and the wheel | one item per notch at most |\n\nA plain vertical wheel is left alone on purpose. It belongs to whatever the carousel is standing in, and a strip that swallowed it would trap the page under it — the reader scrolls, the page stops, and the only way out is to aim around the carousel.\n\n## Keyboard\n\nLeft and right move one stop. Home and End are the first and last item of the strip, not of the travel — on a strip that comes round, the last item may well be one step backwards.\n\n## Reading it\n\n`paged` reports the leading stop while the strip is still moving, which is what the dots and a readout beside them need. `settled` reports the stop the motion ended on: use it for anything expensive, since a flick passes every stop on the way. `pressed` reports an item in full view that was pressed; an item the edge is cutting reports nothing, because pressing it scrolls it in instead. `set_page` puts a stop at the leading edge with no motion to watch, because setting a position is not a gesture.\n\n## What it deliberately does not do\n\nIt does not advance by itself. A strip that moves while it is being read takes the reader's place away, and there is no setting for how fast somebody reads.\n\nIts items are a line of text, a quieter second line and a ground colour, not arbitrary children. What a carousel has to get right is the motion and the stopping; a strip that also hosts arbitrary widgets has to rule on the focus, the hit rects and the tab order of a widget that is half outside the viewport, and those are a different set of decisions.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Items at a time", target: "subject", kind: ControlKind::Number { prop: "per_view", min: 1., max: 6., step: 1., default: 1. } },
        Control { label: "Peek", target: "subject", kind: ControlKind::Number { prop: "peek", min: 0., max: 120., step: 4., default: 0. } },
        Control { label: "Gap", target: "subject", kind: ControlKind::Number { prop: "gap", min: 0., max: 40., step: 1., default: 8. } },
        Control { label: "Item height", target: "subject", kind: ControlKind::Number { prop: "item_height", min: 60., max: 280., step: 4., default: 120. } },
        Control { label: "Arrow room", target: "subject", kind: ControlKind::Number { prop: "arrow_width", min: 12., max: 80., step: 2., default: 26. } },
        Control { label: "Comes round", target: "subject", kind: ControlKind::Bool { prop: "loop_items", default: false } },
        Control { label: "Arrows", target: "subject", kind: ControlKind::Bool { prop: "show_arrows", default: true } },
        Control { label: "Dots", target: "subject", kind: ControlKind::Bool { prop: "show_dots", default: true } },
        Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
    ],
    on_actions: Some(carousel_actions),
}];
