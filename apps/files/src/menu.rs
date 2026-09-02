//! The context menu's contents.
//!
//! The menu is data, not layout: this module says which rows exist for a given
//! situation and what each one does, and the shell draws exactly that. Keeping
//! it here is what lets the rule "every row does a real thing" be a test rather
//! than a promise — a row can only exist if it names a [`MenuAction`], and
//! every action is dispatched in one `match` the compiler checks.

use std::path::Path;

use crate::contents::ViewMode;

/// Everything the context menu can ask for. There is nothing here that the
/// shell does not do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    Open,
    /// Opens the submenu of apps rather than doing something itself.
    OpenWith,
    Preview,
    NewFolder,
    Rename,
    Duplicate,
    Copy,
    Cut,
    Paste,
    SelectAll,
    Trash,
    DeleteForever,
    RevealInTreemap,
    Properties,
    OpenInTerminal,
    ShowHidden,
    SetMode(ViewMode),
    /// One app from the Open With submenu, by its index in
    /// [`open_with_apps`]'s answer.
    OpenWithApp(usize),
}

/// One row of the menu.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuRow {
    pub action: MenuAction,
    pub label: String,
    /// The keyboard shortcut that does the same thing, shown greyed on the
    /// right. Empty when there is none.
    pub hint: &'static str,
    /// Drawn in the theme's warning color, because it cannot be undone.
    pub danger: bool,
    /// This row opens a submenu instead of acting.
    pub submenu: bool,
    /// A hairline above this row.
    pub separator: bool,
}

impl MenuRow {
    fn new(action: MenuAction, label: &str, hint: &'static str) -> Self {
        Self {
            action,
            label: label.to_string(),
            hint,
            danger: false,
            submenu: false,
            separator: false,
        }
    }

    fn sep(mut self) -> Self {
        self.separator = true;
        self
    }

    fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    fn submenu(mut self) -> Self {
        self.submenu = true;
        self
    }
}

/// The most rows any menu here has. The shell carries this many slots.
pub const MAX_ROWS: usize = 14;
/// The most apps the Open With submenu offers.
pub const MAX_APPS: usize = 4;

/// The menu for a selection. `count` is how many things are selected and
/// `folder` whether the one under the pointer is a directory.
pub fn entry_menu(count: usize, folder: bool) -> Vec<MenuRow> {
    let many = count > 1;
    let mut rows = vec![
        MenuRow::new(
            MenuAction::Open,
            if folder { "Open Folder" } else { "Open" },
            "Enter",
        ),
    ];
    // Only one file at a time can be handed to a chosen app, and only a file
    // has a viewer to choose.
    if !many && !folder {
        rows.push(MenuRow::new(MenuAction::OpenWith, "Open With", "").submenu());
        rows.push(MenuRow::new(MenuAction::Preview, "Preview", "Space"));
    }
    rows.push(MenuRow::new(MenuAction::NewFolder, "New Folder", "⇧⌘N").sep());
    rows.push(MenuRow::new(
        MenuAction::Rename,
        if many { "Rename…" } else { "Rename" },
        "F2",
    ));
    rows.push(MenuRow::new(MenuAction::Duplicate, "Duplicate", ""));
    rows.push(MenuRow::new(MenuAction::Copy, "Copy", "⌘C"));
    rows.push(MenuRow::new(MenuAction::Cut, "Cut", "⌘X"));
    rows.push(MenuRow::new(MenuAction::Trash, "Move to Trash", "⌘Del").sep());
    rows.push(
        MenuRow::new(MenuAction::DeleteForever, "Delete Permanently", "⇧Del")
            .danger(),
    );
    rows.push(MenuRow::new(MenuAction::RevealInTreemap, "Reveal in Treemap", "").sep());
    rows.push(MenuRow::new(MenuAction::Properties, "Properties", "⌘I"));
    rows.push(MenuRow::new(
        MenuAction::OpenInTerminal,
        "Open in Terminal",
        "",
    ));
    rows
}

/// The menu for the empty space of a folder.
pub fn empty_menu(mode: ViewMode, clipboard: usize, show_hidden: bool) -> Vec<MenuRow> {
    let mut rows = vec![MenuRow::new(MenuAction::NewFolder, "New Folder", "⇧⌘N")];
    // Paste is only a row when there is something to paste: a row that does
    // nothing is worse than no row.
    if clipboard > 0 {
        rows.push(MenuRow::new(
            MenuAction::Paste,
            &format!(
                "Paste {} item{}",
                clipboard,
                if clipboard == 1 { "" } else { "s" }
            ),
            "⌘V",
        ));
    }
    rows.push(MenuRow::new(MenuAction::SelectAll, "Select All", "⌘A"));
    for (index, view) in [
        ViewMode::Icons,
        ViewMode::List,
        ViewMode::Compact,
        ViewMode::Treemap,
    ]
    .into_iter()
    .enumerate()
    {
        let hint = ["⌘1", "⌘2", "⌘3", "⌘4"][index];
        let mark = if view == mode { "• " } else { "   " };
        let mut row = MenuRow::new(
            MenuAction::SetMode(view),
            &format!("{mark}{}", view.label()),
            hint,
        );
        if index == 0 {
            row = row.sep();
        }
        rows.push(row);
    }
    rows.push(
        MenuRow::new(
            MenuAction::ShowHidden,
            if show_hidden {
                "Hide Hidden Files"
            } else {
                "Show Hidden Files"
            },
            "⌃H",
        )
        .sep(),
    );
    rows
}

/// The apps offered for one file, in the order the submenu lists them: the
/// association first, then the terminal's pager, then the desktop's own
/// opener. `available` decides whether a sibling binary is actually there —
/// an app that cannot run is not offered.
pub fn open_with_apps(path: &Path, available: &dyn Fn(&str) -> bool) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let primary = makepad_wm_api::viewer_for(path);
    if available(primary) {
        out.push((primary.to_string(), format!("Open with {primary}")));
    }
    if primary != "terminal" && available("terminal") {
        out.push(("terminal".to_string(), "Open in the terminal pager".to_string()));
    }
    // The desktop's own opener always exists; it is the honest last resort.
    out.push((
        String::new(),
        "Open with the desktop default".to_string(),
    ));
    out.truncate(MAX_APPS);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_menu_fits_the_slots_the_shell_carries() {
        assert!(entry_menu(1, false).len() <= MAX_ROWS);
        assert!(entry_menu(1, true).len() <= MAX_ROWS);
        assert!(entry_menu(9, false).len() <= MAX_ROWS);
        assert!(empty_menu(ViewMode::Icons, 3, false).len() <= MAX_ROWS);
    }

    #[test]
    fn a_folder_is_never_offered_a_viewer() {
        let rows = entry_menu(1, true);
        assert!(!rows.iter().any(|r| r.action == MenuAction::OpenWith));
        assert!(!rows.iter().any(|r| r.action == MenuAction::Preview));
        assert_eq!(rows[0].label, "Open Folder");
    }

    #[test]
    fn a_multiple_selection_drops_the_single_file_rows() {
        let rows = entry_menu(4, false);
        assert!(!rows.iter().any(|r| r.action == MenuAction::OpenWith));
        // …but keeps everything that works on a set.
        for action in [
            MenuAction::Copy,
            MenuAction::Cut,
            MenuAction::Trash,
            MenuAction::DeleteForever,
            MenuAction::Duplicate,
        ] {
            assert!(rows.iter().any(|r| r.action == action), "{action:?}");
        }
        assert_eq!(
            rows.iter().find(|r| r.action == MenuAction::Rename).unwrap().label,
            "Rename…"
        );
    }

    #[test]
    fn only_the_permanent_delete_is_dangerous() {
        let dangerous: Vec<MenuAction> = entry_menu(1, false)
            .into_iter()
            .filter(|r| r.danger)
            .map(|r| r.action)
            .collect();
        assert_eq!(dangerous, [MenuAction::DeleteForever]);
    }

    #[test]
    fn paste_appears_only_when_there_is_something_to_paste() {
        let empty = empty_menu(ViewMode::Icons, 0, false);
        assert!(!empty.iter().any(|r| r.action == MenuAction::Paste));
        let full = empty_menu(ViewMode::Icons, 2, false);
        let paste = full.iter().find(|r| r.action == MenuAction::Paste).unwrap();
        assert_eq!(paste.label, "Paste 2 items");
    }

    #[test]
    fn the_empty_menu_marks_the_view_it_is_in() {
        let rows = empty_menu(ViewMode::Treemap, 0, false);
        let marked: Vec<&str> = rows
            .iter()
            .filter(|r| r.label.starts_with('•'))
            .map(|r| r.label.as_str())
            .collect();
        assert_eq!(marked, ["• Treemap"]);
        // And the hidden-files row says what pressing it will do.
        assert!(rows.iter().any(|r| r.label == "Show Hidden Files"));
        assert!(empty_menu(ViewMode::Icons, 0, true)
            .iter()
            .any(|r| r.label == "Hide Hidden Files"));
    }

    #[test]
    fn open_with_offers_only_apps_that_exist() {
        let none = |_: &str| false;
        let all = |_: &str| true;
        let picture = Path::new("/a/x.png");
        let offered = open_with_apps(picture, &all);
        assert_eq!(offered[0].0, "image");
        assert_eq!(offered[1].0, "terminal");
        // The desktop opener is the last resort and has no binary of its own.
        assert!(offered.last().unwrap().0.is_empty());
        // With nothing built, only the desktop opener is left.
        assert_eq!(open_with_apps(picture, &none).len(), 1);
        // A text file's association *is* the pager, so it is not listed twice.
        let ids: Vec<String> = open_with_apps(Path::new("/a/n.txt"), &all)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(ids, ["terminal", ""]);
        assert!(offered.len() <= MAX_APPS);
    }

    #[test]
    fn separators_never_start_a_menu() {
        for rows in [
            entry_menu(1, false),
            entry_menu(1, true),
            empty_menu(ViewMode::List, 1, false),
        ] {
            assert!(!rows[0].separator);
            // …and no row is a separator with nothing to separate it from.
            assert!(rows.iter().skip(1).any(|r| r.separator));
        }
    }
}
