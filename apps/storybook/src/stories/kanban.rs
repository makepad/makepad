//! The kanban board story: columns of cards, a drag that crosses between
//! them, and a column that will take no more.
use crate::makepad_widgets::kanban::KanbanBoardWidgetRefExt;
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.KanbanBoardOverview = StoryPage{
        StoryNote{text: "Columns of cards. Drag a card up or down its own column to put it in order, or across to move it on: the card stays where it lies, washed over, and a marker shows the gap it would land in. The board moves the card itself when you let go — it has to, because a column can refuse one."}

        StoryHeading{text: "A board"}
        StoryNote{text: "Every column is one entry in the board: a heading, a count, and the cards it starts with. The count is the column's own, drawn beside the heading, so a column that is filling up says so before anything is dragged into it."}
        StoryRow{
            width: Fill
            subject := KanbanBoard{
                width: Fill
                // A board takes the height it is given, and a story page
                // sizes to fit: without a stated height this one would be
                // laid out and never painted.
                height: 320.
                backlog := KanbanColumn{
                    heading: "Backlog"
                    cards: [
                        "Rename the export control",
                        "Audit the empty states",
                        "Tokens for the light theme",
                        "Drop the old date parser",
                        "Write up the drag rules"
                    ]
                }
                doing := KanbanColumn{
                    heading: "Doing"
                    cap: 3
                    cards: ["Wire the new list", "Second pass on the icons"]
                }
                review := KanbanColumn{
                    heading: "Review"
                    cap: 2
                    cards: ["Keyboard traversal", "Scroll bounce"]
                }
                done := KanbanColumn{
                    heading: "Done"
                    cards: ["Split the layout pass"]
                }
            }
        }
        StoryRow{
            moved_note := Label{text: "Take hold of a card and move it."}
        }

        StoryHeading{text: "A column that is full"}
        StoryNote{text: "Review holds two and will take two. Carry a third one over and the marker turns to the refusal colour, the column's line lights, and letting go leaves the card where it was — the board reports the refusal rather than pretending. Review can still be put in order: a cap is about how much work is in flight, not about whether the column may be tidied."}

        StoryHeading{text: "Columns scroll on their own"}
        StoryNote{text: "Each column scrolls under the wheel, and only the one under the pointer moves. During a drag the wheel is ignored — cards sliding away under the finger would be the one thing worse than a slow drag — and holding the card near a column's top or bottom edge crawls that column instead."}
        StoryRow{
            width: Fill
            deep := KanbanBoard{
                width: Fill
                height: 220.
                column_width: 240.
                inbox := KanbanColumn{
                    heading: "Inbox"
                    cards: [
                        "One", "Two", "Three", "Four", "Five", "Six",
                        "Seven", "Eight", "Nine", "Ten", "Eleven", "Twelve"
                    ]
                }
                later := KanbanColumn{
                    heading: "Later"
                    cards: ["Something for next week"]
                }
            }
        }

        StoryHeading{text: "Cards of your own"}
        StoryNote{text: "The board draws its cards from the `card` template on the instance. Write your own and it can be any card at all, as long as something inside it is called `title`: that is where the board writes each card's text."}
        StoryRow{
            width: Fill
            dressed := KanbanBoard{
                width: Fill
                height: 200.
                column_width: 220.
                card := OutlinedCard{
                    width: Fill
                    height: Fit
                    radius: 4.
                    padding: theme.mspace_1{left: theme.space_2, right: theme.space_2}
                    body: CardBody{
                        title := Label{width: Fill, text: ""}
                        Label{
                            text: "two days ago"
                            draw_text +: {color: theme.color_text_meta}
                        }
                    }
                }
                open := KanbanColumn{
                    heading: "Open"
                    cards: ["Cannot paste into the field", "Wrong month on the picker"]
                }
                closed := KanbanColumn{
                    heading: "Closed"
                    cards: ["Crash on an empty list"]
                }
            }
        }
    }
}

fn kanban_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let board = root.kanban_board(cx, ids!(subject));
    if let Some(moved) = board.moved(actions) {
        let heading = board.heading(moved.to_column);
        root.label(cx, ids!(moved_note)).set_text(
            cx,
            &format!("moved into {heading}, position {}", moved.to + 1),
        );
    }
    if let Some(column) = board.refused(actions) {
        let heading = board.heading(column);
        root.label(cx, ids!(moved_note))
            .set_text(cx, &format!("{heading} is full — the card stayed where it was"));
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "containers/kanbanboard/overview",
    category: "Containers",
    component: "KanbanBoard",
    also: &["KanbanColumn", "KanbanCard"],
    name: "Overview",
    dsl: "KanbanBoardOverview",
    added: "2026-09-10",
    tags: &["new", "board", "columns", "cards", "drag", "drop", "reorder", "backlog", "work in progress"],
    doc: "# KanbanBoard\n\nColumns of cards. A card is dragged up and down its own column to put it in order, and across to move it on.\n\n## Who owns the cards\n\nThe board does, and this is the one place it parts company with `ReorderList`. The reorder list moves nothing: it reports where a row was dropped and the host applies it, because the host is the only thing that knows what a row *is*. A board is different in one respect that decides the whole design — a column may cap how many cards it will hold, so the board is the thing that knows whether a drop can happen at all. Something that has to answer \"no\" has to be the thing that moves the card when the answer is yes; a widget that drew a refusal and then reported the move anyway would be lying to its host.\n\nSo the board moves its own cards and says what it did: `moved` carries where the card came from and where it went, and `refused` carries the column that would not take it.\n\n## The gesture\n\n| Gesture | Result |\n|---|---|\n| drag a card | it lifts; a marker shows the gap it would land in |\n| drag it across | the column under the pointer becomes the target |\n| let go | the card goes there, unless the column is full |\n| Escape | the gesture is dropped and nothing moves |\n| wheel | scrolls the column under the pointer |\n| hold at a column's edge | that column crawls, so a long one is reachable in one gesture |\n\nThe gap is decided by the midpoints of the cards drawn in the target column — the same rule the reorder list uses, and the same code: `ReorderDrag` does that part here too, so a card and a row land in the same place for the same reason. What this widget adds is the column, and waking on **sideways** travel: a card carried straight across never moves up or down at all, and a gesture that only watched y would never start.\n\n## Columns\n\nA column is one entry on the board — `heading`, `cap`, and the `cards` it starts with. They are told apart from the card template by type, so they can be called anything:\n\n```\nKanbanBoard {\n    height: 320.\n    todo := KanbanColumn { heading: \"To do\" cards: [\"One\", \"Two\"] }\n    doing := KanbanColumn { heading: \"Doing\" cap: 3 cards: [] }\n}\n```\n\n`cap` is the most cards a column will hold; 0 is no limit. A full column says so beside its count, turns the marker to the refusal colour, and leaves the card where it was — but it still reorders its own, because a cap is about how much work is in flight and not about whether the column may be tidied.\n\n## Cards\n\nEvery card is drawn from the `card` template on the instance, and the board writes the card's text into whatever inside it is called `title`. A template without one draws the same words on every card, so the board says so in the log rather than leaving you to wonder.\n\n## What it does not do\n\nIt does not virtualise: every card is a live widget, which is right for the tens of cards a board is read at a glance and wrong for thousands. It does not scroll sideways — columns are squeezed towards `min_column_width` to fit the width they are given, and a board with more columns than that will hold clips them. It does not add, remove or rename columns, edit a card, or move one by keyboard.\n\nAnd it takes the height it is given: on a page that sizes to fit, `height: Fill` resolves to nothing and the board is laid out and never painted. State a height.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Column width", target: "subject", kind: ControlKind::Number { prop: "column_width", min: 120., max: 320., step: 10., default: 200. } },
        Control { label: "Least column width", target: "subject", kind: ControlKind::Number { prop: "min_column_width", min: 80., max: 240., step: 10., default: 120. } },
        Control { label: "Room between columns", target: "subject", kind: ControlKind::Number { prop: "column_gap", min: 0., max: 24., step: 1., default: 6. } },
        Control { label: "Heading height", target: "subject", kind: ControlKind::Number { prop: "header_height", min: 16., max: 48., step: 1., default: 26. } },
        Control { label: "Room between cards", target: "subject", kind: ControlKind::Number { prop: "card_spacing", min: 0., max: 20., step: 1., default: 6. } },
        Control { label: "Room around the cards", target: "subject", kind: ControlKind::Number { prop: "card_inset", min: 0., max: 20., step: 1., default: 6. } },
        Control { label: "Drag threshold", target: "subject", kind: ControlKind::Number { prop: "drag_threshold", min: 0., max: 20., step: 1., default: 4. } },
        Control { label: "Marker thickness", target: "subject", kind: ControlKind::Number { prop: "marker_size", min: 1., max: 8., step: 0.5, default: 2. } },
        Control { label: "What a full column says", target: "subject", kind: ControlKind::Text { prop: "full_text", default: "Full" } },
    ],
    on_actions: Some(kanban_actions),
}];
