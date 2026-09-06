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
            content +: {
                body +: {
                    P{text: "A sheet is a drawer from the bottom with a grabber, which says the panel can be dragged."}
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
    name: "Overview",
    dsl: "DrawerOverview",
    added: "2026-09-05",
    tags: &["new"],
    doc: "# Drawer\n\nA drawer is a dialog that has chosen a side. It stops the work the same way — scrim, pointer taken, keyboard taken — but it arrives from an edge and is shaped by that edge. Navigation, filters and a long list of settings belong here rather than in a centred card, because they are places rather than questions.\n\n`side` picks the edge and decides the shape: a left or right drawer is a column whose `size` is a width, a top or bottom one is a row whose `size` is a height, which is why the rungs are named for how much room they take rather than for a number.\n\n`SideSheet` is a drawer from the right, for the detail of whatever is selected. `BottomSheet` is one from the bottom with a grabber, and that grabber is the only difference the library makes, because it is the only one that matters: it says the panel can be dragged, and a drawer that cannot be dragged should not draw one.\n\nThe panel slides in from its edge rather than appearing, because on a panel this large the movement is what says where it came from. Escape and a press on the scrim both send it back, through the same claim every other overlay uses.",
    subject: "nav",
    feature: None,
    controls: &[],
    on_actions: Some(drawer_actions),
}];
