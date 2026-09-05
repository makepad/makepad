//! The menu stories: one layer, raised from three different controls.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.MenuOverview = StoryPage{
        StoryNote{text: "One layer draws every menu. A control raises one by asking for it with the rows it wants and the rect it measured from its own drawn area, then reads the pick back. Try the button, the right-click area and the keyboard."}

        StoryHeading{text: "From a button"}
        StoryRow{
            file_menu := Button{text: "File"}
            edit_menu := Button{text: "Edit"}
        }

        StoryHeading{text: "From a right-click"}
        StoryNote{text: "Secondary press anywhere in the panel opens the menu at the pointer."}
        StoryRow{
            context := RoundedView{
                width: 320.
                height: 90.
                align: Align{x: 0.5 y: 0.5}
                show_bg: true
                draw_bg +: {color: theme.color_surface_container}
                Label{text: "right-click here"}
            }
        }

        StoryHeading{text: "What came back"}
        StoryRow{
            picked := Label{text: "nothing picked yet"}
        }

        StoryNote{text: "Escape closes, the arrows walk the rows, Right opens a flyout, a letter jumps to the row that starts with it. A disabled row and a heading are skipped by every one of those."}

        // The layer is declared once, last, so it draws over everything in
        // the story above it.
        menus := MenuLayer{}
    }
}

fn menu_rows() -> Vec<MenuRow> {
    vec![
        MenuRow::section("File"),
        MenuRow::new(live_id!(new_file), "New").key("Ctrl+N"),
        MenuRow::new(live_id!(open_file), "Open…").key("Ctrl+O"),
        MenuRow::new(live_id!(recent), "Open recent").submenu(vec![
            MenuRow::new(live_id!(recent_a), "notes.txt"),
            MenuRow::new(live_id!(recent_b), "budget.csv"),
            MenuRow::new(live_id!(recent_c), "shader.frag"),
        ]),
        MenuRow::separator(),
        MenuRow::new(live_id!(save), "Save").key("Ctrl+S"),
        MenuRow::new(live_id!(save_as), "Save as…").enabled(false),
        MenuRow::separator(),
        MenuRow::new(live_id!(wrap), "Wrap lines").checked(true),
        MenuRow::new(live_id!(minimap), "Show minimap").checked(false),
        MenuRow::separator(),
        MenuRow::new(live_id!(delete_file), "Delete").danger(true),
    ]
}

fn edit_rows() -> Vec<MenuRow> {
    vec![
        MenuRow::new(live_id!(undo), "Undo").key("Ctrl+Z"),
        MenuRow::new(live_id!(redo), "Redo").key("Ctrl+Shift+Z").enabled(false),
        MenuRow::separator(),
        MenuRow::new(live_id!(cut), "Cut").key("Ctrl+X"),
        MenuRow::new(live_id!(copy), "Copy").key("Ctrl+C"),
        MenuRow::new(live_id!(paste), "Paste").key("Ctrl+V"),
        MenuRow::separator(),
        MenuRow::section("Line endings"),
        MenuRow::new(live_id!(lf), "Unix").radio(true),
        MenuRow::new(live_id!(crlf), "Windows").radio(false),
    ]
}

fn menu_actions_handler(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let layer = root.menu_layer(cx, ids!(menus));

    let file = root.button(cx, ids!(file_menu));
    if file.clicked(actions) {
        let anchor = file.area().rect(cx);
        layer.open(cx, live_id!(file_menu), menu_rows(), anchor, MenuPlace::Below);
    }
    let edit = root.button(cx, ids!(edit_menu));
    if edit.clicked(actions) {
        let anchor = edit.area().rect(cx);
        layer.open(cx, live_id!(edit_menu), edit_rows(), anchor, MenuPlace::Below);
    }
    // A secondary press in the panel opens the same rows at the pointer.
    let context = root.view(cx, ids!(context));
    if let Some(fe) = context.finger_down(actions) {
        if !fe.is_primary_hit() {
            let at = Rect { pos: fe.abs, size: dvec2(0.0, 0.0) };
            layer.open(cx, live_id!(context), menu_rows(), at, MenuPlace::At);
        }
    }

    for action in menu_actions(actions) {
        if let MenuAction::Picked { owner, id } = action {
            let text = format!("picked {} from {}", id.as_string(|s| s.map(str::to_string).unwrap_or_default()), owner.as_string(|s| s.map(str::to_string).unwrap_or_default()));
            root.label(cx, ids!(picked)).set_text(cx, &text);
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "navigation/menu/overview",
    category: "Navigation",
    component: "Menu",
    name: "Overview",
    dsl: "MenuOverview",
    added: "2026-09-05",
    tags: &["new"],
    doc: "# Menu\n\nOne `MenuLayer` draws every menu in an app: dropdowns, context menus, flyouts. Declare it once, last in the window body or inside an `OverlayLayers` host, and no other widget has to own a menu.\n\nA control raises one by calling `open` on the layer (or by broadcasting `MenuAction::Open`) with a list of `MenuRow` and the rect it measured from its own drawn area. The pick comes back as `MenuAction::Picked`, carrying the owner id the caller chose, so a control can tell its own menus apart.\n\nA row can be a command, a heading, a rule, a checked or radio item, a dangerous action, or a submenu. While a menu is up the layer holds the sweep lock, so the press that picks a row cannot also reach whatever sits under it; the lock nests, so a menu raised inside a dialog hands the lock back when it closes.\n\nKeyboard: the arrows walk the rows and skip anything that cannot be chosen, Right opens a flyout and Left leaves it, Home and End jump to the ends, a letter jumps to the next row starting with it, Return chooses and Escape closes.",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: Some(menu_actions_handler),
}];
