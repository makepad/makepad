//! The pagination story: the numbered strip, and the things that go beside
//! it rather than inside it.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.PaginationOverview = StoryPage{
        StoryNote{text: "A list too long to show at once needs the reader's place kept. A strip of every page number stops being readable past a handful; prev and next alone lose the place entirely. This shows the pages around the one being read, the two ends, and one mark for everything folded."}

        StoryHeading{text: "Where the fold moves"}
        StoryNote{text: "The same twenty pages, read at the start, the middle and the end. The fold is on the far side of wherever you are, and both folds appear only once you are away from both ends."}
        StoryRow{
            at_start := Pagination{total: 20 page: 1}
        }
        StoryRow{
            at_middle := Pagination{total: 20 page: 10}
        }
        StoryRow{
            at_end := Pagination{total: 20 page: 20}
        }

        StoryHeading{text: "Short enough not to fold"}
        StoryNote{text: "Five pages fit, so nothing is hidden and no mark is drawn. A strip that folds when it does not need to is just noise."}
        StoryRow{
            short := Pagination{total: 5 page: 3}
        }

        StoryHeading{text: "How much of the middle to keep"}
        StoryNote{text: "siblings is how many neighbours flank the current page, boundaries how many stay pinned at each end. Both cost width, which is why they are the caller's choice."}
        StoryRow{
            wide := Pagination{total: 40 page: 20 siblings: 3 boundaries: 2}
        }
        StoryRow{
            narrow := Pagination{total: 40 page: 20 siblings: 0 boundaries: 1}
        }

        StoryHeading{text: "Driven"}
        StoryNote{text: "The strip below is live: the ends step, a number jumps, and the fold moves with you. Everything beside it — a page size, a total, a jumper — is a control of its own next to the strip rather than a part of it."}
        StoryRow{
            subject := Pagination{total: 12 page: 1}
            where_label := Label{text: "page 1 of 12"}
        }

        StoryHeading{text: "Without the step marks"}
        StoryRow{
            plain := Pagination{total: 9 page: 4 can_step: false}
        }
    }
}

fn pagination_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let strip = root.pagination(cx, ids!(subject));
    if let Some(page) = strip.changed(actions) {
        root.label(cx, ids!(where_label))
            .set_text(cx, &format!("page {page} of 12"));
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "navigation/pagination/overview",
    category: "Navigation",
    component: "Pagination",
    name: "Overview",
    dsl: "PaginationOverview",
    added: "2026-09-08",
    tags: &["new"],
    doc: "# Pagination

A numbered strip for a list too long to show at once.

Two failures it exists to avoid: a strip of every page number, which stops being readable past a handful, and a bare prev/next, which loses the reader's place in a list they were scanning. It shows the pages around the one being read, keeps the two ends, and folds everything else into one mark.

`page_window(total, page, siblings, boundaries)` is a free function with its own tests, and the strip is only its drawing. One rule in it is worth knowing: **a gap of exactly one page is filled in rather than folded**, because an ellipsis standing in for a single page costs the room that page would have taken and hides a real number for nothing.

`siblings` is how many neighbours flank the current page and `boundaries` how many stay pinned at each end; both cost width, so both are the caller's choice. Pages are 1-indexed, and a `total` of zero or a `page` past the end are clamped rather than treated as errors — a caller mid-load has nothing better to hand it.

What is deliberately NOT in the widget: a page-size chooser, a total, and a jump-to box are a `Select`, a `Label` and a `TextInput` standing next to the strip. They are compositions, not parts, and the page above shows them as such.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Total", target: "subject", kind: ControlKind::Number { prop: "total", min: 1., max: 100., step: 1., default: 12. } },
        Control { label: "Page", target: "subject", kind: ControlKind::Number { prop: "page", min: 1., max: 100., step: 1., default: 1. } },
        Control { label: "Siblings", target: "subject", kind: ControlKind::Number { prop: "siblings", min: 0., max: 4., step: 1., default: 1. } },
        Control { label: "Boundaries", target: "subject", kind: ControlKind::Number { prop: "boundaries", min: 0., max: 3., step: 1., default: 1. } },
        Control { label: "Step marks", target: "subject", kind: ControlKind::Bool { prop: "can_step", default: true } },
    ],
    on_actions: Some(pagination_actions),
}];
