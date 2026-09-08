//! The popover stories: where a panel hangs off its anchor, and what opens it.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let PopNote = Label{width: Fit draw_text.color: theme.color_text_meta}

    mod.stories.PopoverOverview = StoryPage{
        StoryNote{text: "A popover hangs a panel off a control. Twelve placements: a side, and how the panel lines up along it. Press one and the panel says where it thinks it went, so the label and the panel have to agree."}

        StoryHeading{text: "Twelve placements"}
        StoryRow{
            Grid{
                width: Fill height: Fit
                column_gap: 10. row_gap: 46.
                columns: ["repeat(3, minmax(150px, 1fr))"]
                // Tall enough for the side-placed controls: against a control
                // the panel's own height, Start, Center and End land in the
                // same place and the demo would be showing no difference.
                implicit_row_size: 80.
                // Cells stretch by default, and a popover anchors to its OWN
                // rect: stretched, every panel hung off the middle of a cell
                // instead of off the control, which makes a placement demo
                // say something untrue.
                justify_items: CellAlign.Start

                PopoverArrow{width: Fit height: Fit placement: TopStart    Button{text: "TopStart"}    content +: {PopNote{width: 170. text: "TopStart"}}}
                PopoverArrow{width: Fit height: Fit placement: TopCenter   Button{text: "TopCenter"}   content +: {PopNote{width: 170. text: "TopCenter"}}}
                PopoverArrow{width: Fit height: Fit placement: TopEnd      Button{text: "TopEnd"}      content +: {PopNote{width: 170. text: "TopEnd"}}}
                PopoverArrow{width: Fit height: Fit placement: BottomStart Button{text: "BottomStart"} content +: {PopNote{width: 170. text: "BottomStart"}}}
                PopoverArrow{width: Fit height: Fit placement: BottomCenter Button{text: "BottomCenter"} content +: {PopNote{width: 170. text: "BottomCenter"}}}
                PopoverArrow{width: Fit height: Fit placement: BottomEnd   Button{text: "BottomEnd"}   content +: {PopNote{width: 170. text: "BottomEnd"}}}
                PopoverArrow{width: Fit height: Fit placement: LeftStart   Button{height: 74. text: "LeftStart"}   content +: {PopNote{text: "LeftStart"}}}
                PopoverArrow{width: Fit height: Fit placement: LeftCenter  Button{height: 74. text: "LeftCenter"}  content +: {PopNote{text: "LeftCenter"}}}
                PopoverArrow{width: Fit height: Fit placement: LeftEnd     Button{height: 74. text: "LeftEnd"}     content +: {PopNote{text: "LeftEnd"}}}
                PopoverArrow{width: Fit height: Fit placement: RightStart  Button{height: 74. text: "RightStart"}  content +: {PopNote{text: "RightStart"}}}
                PopoverArrow{width: Fit height: Fit placement: RightCenter Button{height: 74. text: "RightCenter"} content +: {PopNote{text: "RightCenter"}}}
                PopoverArrow{width: Fit height: Fit placement: RightEnd    Button{height: 74. text: "RightEnd"}    content +: {PopNote{text: "RightEnd"}}}
            }
        }

        StoryHeading{text: "What opens it"}
        StoryNote{text: "A press, a dwell, or a secondary press which opens it AT THE POINTER rather than at the control."}
        StoryRow{
            PopoverToggle{
                placement: BottomStart
                Button{text: "On a press"}
                content +: {PopNote{text: "Escape or a press outside closes it."}}
            }
            PopoverHover{
                placement: BottomStart
                Button{text: "On a dwell"}
                content +: {PopNote{text: "It stays while the pointer travels into it."}}
            }
            PopoverArrow{
                trigger: Context
                placement: BottomStart
                Button{text: "On a secondary press"}
                content +: {PopNote{text: "Opened where the pointer was."}}
            }
        }

        StoryHeading{text: "A question with two answers"}
        StoryNote{text: "A confirm popover asks beside the control that raised it, rather than stopping the whole window the way a dialog does."}
        StoryRow{
            ask := ConfirmPopover{
                placement: BottomStart
                danger: true
                content +: {
                    title +: {text: "Discard the take?"}
                    buttons +: {confirm_danger +: {text: "Discard"}}
                }
                Button{text: "Discard"}
            }
            answer := Label{text: "nothing asked yet"}
        }
    }
}

fn popover_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let ask = root.confirm_popover(cx, ids!(ask));
    if ask.confirmed(actions) {
        root.label(cx, ids!(answer)).set_text(cx, "discarded");
    } else if ask.cancelled(actions) {
        root.label(cx, ids!(answer)).set_text(cx, "kept");
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "overlay/popover/overview",
    category: "Overlay",
    component: "Popover",
    also: &["ConfirmPopover"],
    name: "Overview",
    dsl: "PopoverOverview",
    added: "2026-09-05",
    tags: &["new"],
    doc: "# Popover\n\nA popover hangs a panel off a control, and unlike a dialog it does not stop the rest of the window.\n\n**Twelve placements**, and they are two decisions rather than one: which SIDE the panel takes, and how it LINES UP along that side. `RightStart` is to the right with the top edges level, `RightEnd` to the right with the bottom edges level, `RightCenter` centred on it. The shared placement helper flips a side that has no room and shifts the panel to stay in the window, so the placement is a preference, not a promise.\n\nEvery one of the twelve is on this page on purpose. Ten of them had never been drawn anywhere in this catalogue, and the last time a placement went unshown in a story its arrow turned out to be clipped away entirely.\n\n**What opens it** is `trigger`. `Click` is the toggletip, which takes the key focus so Tab cycles inside it. `Hover` is the hover card, which never takes the pointer and has a grace window so the pointer can travel from the control into the panel without it closing. `Context` opens on a secondary press, AT THE POINTER rather than at the control, because that is where the question was asked. `Manual` leaves it to the caller.\n\n`ConfirmPopover` is the question form: a title and two answers, reported as Confirmed or Cancelled, with `danger` colouring the confirm button for something that cannot be undone.",
    subject: "ask",
    feature: None,
    controls: &[],
    on_actions: Some(popover_actions),
}];
