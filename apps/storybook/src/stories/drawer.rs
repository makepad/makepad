//! The drawer stories: the panel that comes in from an edge.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.DrawerOverview = StoryPage{
        StoryNote{text: "A drawer is a dialog that has chosen a side. It stops the work the same way, but it arrives from an edge and is shaped by it: a left or right drawer is a column with a width, a top or bottom one is a row with a height."}

        StoryHeading{text: "From each edge"}
        StoryRow{
            open_left := Button{text: "From the left"}
            open_right := Button{text: "From the right"}
            open_top := Button{text: "From the top"}
            open_bottom := Button{text: "From the bottom"}
        }

        StoryHeading{text: "What is up"}
        StoryRow{
            state := Label{text: "none open"}
        }

        StoryHeading{text: "Where a sheet rests"}
        StoryNote{text: "A sheet's grabber is a handle. Drag it to move the panel between a peek, half the room and the whole of what its size asks for, and drag it below the peek to send it back. These open it at a rung, so the three can be seen without dragging. The page is behind the scrim while a sheet is out, so send it back before choosing another."}
        StoryRow{
            sheet_peek := Button{text: "Peek"}
            sheet_half := Button{text: "Half"}
            sheet_full := Button{text: "Full"}
            sheet_rung := Label{text: "shut"}
        }

        StoryNote{text: "The page stays unreachable while a drawer is out, and Escape or a press on the scrim sends it back."}
        StoryRow{
            under := Button{text: "Behind the drawer"}
            under_note := Label{text: "pressed 0 times"}
        }

        nav := Drawer{
            title: "Navigation"
            side: PanelEdge.Left
            content +: {
                body +: {
                    P{text: "A drawer is where navigation belongs: it is a place rather than a question."}
                    P{text: "A left drawer is a column, so its size is a width."}
                }
            }
        }

        details := SideSheet{
            title: "Details"
            size: Lg
            content +: {
                body +: {
                    P{text: "A side sheet comes from the right, for the detail of whatever is selected."}
                }
            }
        }

        banner := Drawer{
            title: "From the top"
            side: PanelEdge.Top
            size: Sm
            content +: {
                body +: {
                    P{text: "A top drawer is a row, so its size is a height."}
                }
            }
        }

        sheet := BottomSheet{
            title: "Choices"
            // Full, so the three rungs are three places. A sheet whose size
            // is smaller than half the window has no room for a half rung —
            // it clamps into the full one, correctly, and then a page with
            // three buttons on it would be showing two.
            size: Full
            content +: {
                body +: {
                    P{text: "A sheet is a drawer from the bottom with a grabber. Drag it: the panel follows, settles at the nearest rung, and goes back if you pull it below the lowest one."}
                }
            }
        }
    }
}

fn drawer_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let drawers = [
        ("navigation", ids!(nav), ids!(open_left)),
        ("details", ids!(details), ids!(open_right)),
        ("top", ids!(banner), ids!(open_top)),
        ("sheet", ids!(sheet), ids!(open_bottom)),
    ];
    for (_, drawer_id, button_id) in drawers {
        if root.button(cx, button_id).clicked(actions) {
            root.drawer(cx, drawer_id).open(cx);
        }
    }
    if root.button(cx, ids!(under)).clicked(actions) {
        let n = crate::stories::bump(live_id!(drawer_under));
        root.label(cx, ids!(under_note)).set_text(cx, &format!("pressed {n} times"));
    }

    // The rungs, reachable without a drag: a page that can only be read
    // shows one of the three, and the point is that there are three.
    let sheet = root.drawer(cx, ids!(sheet));
    for (button_id, rung) in [
        (ids!(sheet_peek), SheetDetent::Collapsed),
        (ids!(sheet_half), SheetDetent::Half),
        (ids!(sheet_full), SheetDetent::Expanded),
    ] {
        if root.button(cx, button_id).clicked(actions) {
            if !sheet.is_open() {
                sheet.open(cx);
            }
            sheet.set_detent(cx, rung);
        }
    }
    let rung = if sheet.is_open() {
        match sheet.detent() {
            SheetDetent::Collapsed => "resting at the peek",
            SheetDetent::Half => "resting at half the room",
            SheetDetent::Expanded => "resting at its full size",
        }
    } else {
        "shut"
    };
    let rung_label = root.label(cx, ids!(sheet_rung));
    if rung_label.text() != rung {
        rung_label.set_text(cx, rung);
    }

    let open: Vec<&str> = drawers
        .iter()
        .filter(|(_, id, _)| root.drawer(cx, *id).is_open())
        .map(|(name, _, _)| *name)
        .collect();
    let text = if open.is_empty() {
        "none open".to_string()
    } else {
        format!("{} open", open.join(", "))
    };
    let label = root.label(cx, ids!(state));
    if label.text() != text {
        label.set_text(cx, &text);
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "overlay/drawer/overview",
    category: "Overlay",
    component: "Drawer",
    also: &["BottomSheet", "SideSheet"],
    name: "Overview",
    dsl: "DrawerOverview",
    added: "2026-09-05",
    tags: &["new"],
    doc: "# Drawer\n\nA drawer is a dialog that has chosen a side. It stops the work the same way — scrim, pointer taken, keyboard taken — but it arrives from an edge and is shaped by that edge. Navigation, filters and a long list of settings belong here rather than in a centred card, because they are places rather than questions.\n\n`side` picks the edge and decides the shape: a left or right drawer is a column whose `size` is a width, a top or bottom one is a row whose `size` is a height, which is why the rungs are named for how much room they take rather than for a number.\n\n`SideSheet` is a drawer from the right, for the detail of whatever is selected. `BottomSheet` is one from the bottom with a grabber, and that grabber is the only difference the library makes, because it is the only one that matters: it is the handle. Dragging it moves the panel between three rungs — `SheetDetent.Collapsed` for a peek, `Half` for half the room, `Expanded` for the whole of what `size` asks for — and a drag below the lowest rung sends the sheet back the way it came. `detent` picks the rung it opens at, and a sheet that has been dragged reports where it settled. A drawer with no grabber has no rungs: it is open or it is shut, and it should not draw a handle for a thing it cannot do.\n\nThe panel slides in from its edge rather than appearing, because on a panel this large the movement is what says where it came from. Escape and a press on the scrim both send it back, through the same claim every other overlay uses.\n\n`SlidePanel` and `ExpandablePanel`, on Containers > MovingPanels, are panels that move without stopping the work: no scrim, and nothing taken from the page. `SlidePanel` comes in from an edge when the app says so. `ExpandablePanel` is dragged by hand like a sheet, but it only reports its offset and settles nowhere by itself, where a sheet here settles on its rungs.",
    subject: "nav",
    feature: None,
    controls: &[],
    on_actions: Some(drawer_actions),
}];
