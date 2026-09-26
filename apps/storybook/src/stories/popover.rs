//! The popover story: what opens a panel off its anchor, the twelve places it
//! hangs, the question form, and popovers and a menu nested three deep.
//!
//! Every floating thing in the library takes the pointer while it is up, and
//! they have to nest: a menu raised from inside a popover must give the
//! popover its grab back when it closes, and Escape must unwind exactly one
//! level per press. The last section is the one place all of that is put
//! together, because a lock stack is not something a unit test can prove.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let PopNote = Label{width: Fit draw_text.color: theme.color_text_meta}

    mod.stories.PopoverOverview = StoryPage{
        StoryNote{text: "A popover hangs a panel off a control and, unlike a dialog, leaves the rest of the window working. What opens it is one choice; where it hangs is two more, a side and how the panel lines up along it."}

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

        StoryHeading{text: "Twelve placements"}
        StoryNote{text: "Press one and the panel says where it thinks it went, so the label and the panel have to agree. A tip takes the same twelve."}
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

        StoryHeading{text: "Nested three deep"}
        StoryNote{text: "Open the popover, then the one inside it, then the menu inside that. Each takes the pointer from the one below. Escape unwinds one level per press, and the level underneath is live again the moment the one above goes."}
        StoryRow{
            outer := PopoverToggle{
                trigger: Click
                arrow: true
                open_btn := Button{text: "Open a popover"}
                content := View{
                    width: Fit
                    height: Fit
                    flow: Down
                    spacing: theme.space_2
                    Label{text: "The first level."}
                    inner := PopoverToggle{
                        trigger: Click
                        arrow: true
                        placement: RightStart
                        inner_btn := Button{text: "One inside it"}
                        content := View{
                            width: Fit
                            height: Fit
                            flow: Down
                            spacing: theme.space_2
                            Label{text: "The second level."}
                            menu_btn := Button{text: "And a menu"}
                        }
                    }
                }
            }
            // Beside the button rather than under it, where the first
            // popover would cover the count it is there to show.
            depth := Label{text: "nothing open"}
        }

        StoryNote{text: "The level under the pointer is the only one that answers it: pressing the button behind an open popover does nothing until that popover is gone."}
        StoryRow{
            behind := Button{text: "A button behind everything"}
            behind_note := Label{text: "pressed 0 times"}
        }

        // The layer is declared once, last, so the menu it raises draws over
        // everything above it on the page.
        menus := MenuLayer{}
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

fn nesting_rows() -> Vec<MenuRow> {
    vec![
        MenuRow::new(live_id!(one), "The third level"),
        MenuRow::new(live_id!(two), "Also the third level"),
        MenuRow::separator(),
        MenuRow::new(live_id!(deeper), "Deeper still").submenu(vec![
            MenuRow::new(live_id!(fourth), "The fourth"),
        ]),
    ]
}

fn nesting_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let outer = root.popover(cx, ids!(outer));
    let inner = root.popover(cx, ids!(inner));
    let layer = root.menu_layer(cx, ids!(menus));

    let menu_btn = root.button(cx, ids!(menu_btn));
    if menu_btn.clicked(actions) {
        let anchor = menu_btn.area().rect(cx);
        layer.open(cx, live_id!(nesting), nesting_rows(), anchor, MenuPlace::Below);
    }
    if root.button(cx, ids!(behind)).clicked(actions) {
        // A count, not a flag: the question is whether a press that
        // dismissed an overlay ALSO reached the button, and a flag that is
        // already set cannot answer it.
        let n = crate::stories::bump(live_id!(overlay_behind));
        root.label(cx, ids!(behind_note)).set_text(cx, &format!("pressed {n} times"));
    }

    // What is up, counted from the outside in, so a test can read the depth
    // rather than guess it from pixels.
    let mut open: Vec<&str> = Vec::new();
    if outer.is_open() {
        open.push("popover");
    }
    if inner.is_open() {
        open.push("popover in popover");
    }
    if layer.is_open() {
        open.push("menu");
    }
    let text = if open.is_empty() {
        "nothing open".to_string()
    } else {
        format!("{} deep: {}", open.len(), open.join(", "))
    };
    let label = root.label(cx, ids!(depth));
    if label.text() != text {
        label.set_text(cx, &text);
    }
}

/// The popover's own answers first, then the nesting section's depth count.
fn popover_page_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    popover_actions(cx, root, actions);
    nesting_actions(cx, root, actions);
}

pub const STORIES: &[Story] = &[Story {
    key: "overlay/popover/overview",
    category: "Overlay",
    component: "Popover",
    also: &["ConfirmPopover", "PopoverArrow", "PopoverHover", "PopoverToggle"],
    name: "Overview",
    dsl: "PopoverOverview",
    added: "2026-09-05",
    tags: &["new"],
    doc: "# Popover\n\nA popover hangs a panel off a control, and unlike a dialog it does not stop the rest of the window.\n\n**What opens it** is `trigger`. `Click` is the toggletip, which takes the key focus so Tab cycles inside it. `Hover` is the hover card, which never takes the pointer and has a grace window so the pointer can travel from the control into the panel without it closing. `Context` opens on a secondary press, AT THE POINTER rather than at the control, because that is where the question was asked. `Manual` leaves it to the caller.\n\n**Twelve placements**, and they are two decisions rather than one: which SIDE the panel takes, and how it LINES UP along that side. `RightStart` is to the right with the top edges level, `RightEnd` to the right with the bottom edges level, `RightCenter` centred on it. The shared placement helper flips a side that has no room and shifts the panel to stay in the window, so the placement is a preference, not a promise. All twelve are drawn on this page, because a placement nobody has seen drawn is one whose arrow nobody has checked. A `Tip` takes the same twelve.\n\n`ConfirmPopover` is the question form: a title and two answers, reported as Confirmed or Cancelled, with `danger` colouring the confirm button for something that cannot be undone.\n\n## Nesting\n\nEvery floating thing in the library takes the pointer while it is up, so that a press on it cannot also reach whatever sits underneath. That grab has to nest: a menu raised from inside a popover gives the popover its grab back when the menu closes, and the popover gives it back to the page.\n\nThe last section stacks three of them and shows the depth, because the nesting is not something a unit test can prove. Escape unwinds one level per press. The button at the bottom stays unreachable while anything is open, and answers again as soon as everything is closed.\n\nThe stack itself lives in the platform: `sweep_lock` and the scroll block are stacks of owners rather than single slots, so each level holds its own and releasing an inner one leaves the outer one in force.",
    subject: "ask",
    feature: None,
    controls: &[],
    on_actions: Some(popover_page_actions),
}];
