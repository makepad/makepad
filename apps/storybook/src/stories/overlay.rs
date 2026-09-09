//! The overlay nesting story: the gate Phase 2 asks for.
//!
//! Every floating thing in the library takes the pointer while it is up, and
//! they have to nest: a menu raised from inside a popover must give the
//! popover its grab back when it closes, and Escape must unwind exactly one
//! level per press. This story is the one place all of that is put together,
//! because a lock stack is not something a unit test can prove.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.OverlayNesting = StoryPage{
        StoryNote{text: "Open the popover, then the one inside it, then the menu inside that. Each takes the pointer from the one below. Escape unwinds one level per press, and the level underneath is live again the moment the one above goes."}

        StoryHeading{text: "Three deep"}
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
        }

        StoryHeading{text: "What is up"}
        StoryRow{
            depth := Label{text: "nothing open"}
        }

        StoryNote{text: "The level under the pointer is the only one that answers it: pressing the button behind an open popover does nothing until that popover is gone."}
        StoryRow{
            behind := Button{text: "A button behind everything"}
            behind_note := Label{text: "pressed 0 times"}
        }

        menus := MenuLayer{}
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

fn overlay_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
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

pub const STORIES: &[Story] = &[Story {
    key: "overlay/nesting/overview",
    category: "Overlay",
    component: "Nesting",
    also: &["PopoverToggle"],
    name: "Overview",
    dsl: "OverlayNesting",
    added: "2026-09-05",
    tags: &["new"],
    doc: "# Nesting\n\nEvery floating thing in the library takes the pointer while it is up, so that a press on it cannot also reach whatever sits underneath. That grab has to nest: a menu raised from inside a popover gives the popover its grab back when the menu closes, and the popover gives it back to the page.\n\nThis story stacks three of them and shows the depth, because the nesting is not something a unit test can prove. Escape unwinds one level per press. The button at the bottom stays unreachable while anything is open, and answers again as soon as everything is closed.\n\nThe stack itself lives in the platform: `sweep_lock` and the scroll block are stacks of owners rather than single slots, so each level holds its own and releasing an inner one leaves the outer one in force.",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: Some(overlay_actions),
}];
