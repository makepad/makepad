//! `shell/plugins/menu/` — the omarchy menu AND the launcher.
//!
//! One surface, two doors: `Super+Space` opens it at the root of
//! `default/omarchy/omarchy-menu.jsonc`, `Super+Alt+Space` opens it on the
//! `apps` provider (the launcher). Geometry, type and behavior are read
//! from `Menu.qml` / `MenuModel.js`:
//!
//! * card 300 wide, `panelPadding` 18 all round, a 34px header, 6px to the
//!   rows, hard corners, a 2px `hyprland.active-border-foreground` border,
//!   over a full-screen scrim at `background` α .5;
//! * rows 50px (58 with a detail line), 3px apart, 28px of "peek" so a
//!   clipped list always ends mid-row, icon column 36 at left margin 8 with
//!   6 to the label (18 when a row has no icon), a 14px chevron column at
//!   right margin 8, and a 17px divider (1px hairline at fg α .2) between
//!   same-parent matches and deeper ones;
//! * height = 18*2 + 34 + 6 + rows, capped at 70% of the screen; centered
//!   horizontally, centered vertically on open, and the top FREEZES on the
//!   first keystroke so the card only grows downward after that;
//! * type: header and row label `heading` 16, detail `bodySmall` 11 at α
//!   .52, chevron at α .36, empty state `displayLarge` 28 + `title` 14;
//! * no animations anywhere.
//!
//! Mouse works exactly like the original: hover moves the cursor row
//! (through a pointer-move gate so a list moving under a still pointer does
//! not reselect), the pointing hand shows on enabled rows, a click
//! activates, a click outside the card cancels, and the wheel scrolls.

use makepad_widgets::*;

use crate::binds::{combo_text, keymap};
use crate::theme;

use super::launcher;
use super::ui::{contains, rect, DrawShellFill, Ico, ShellDraw};
use super::{alpha, fade, MenuTokens, ShellTokens};

// ======================================================================
// The model
// ======================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuKind {
    /// Opens a submenu.
    Menu,
    /// Runs something of ours.
    Action,
    /// An `apps` provider row.
    App,
    /// A link (omarchy opens a URL) — inert here.
    Link,
    /// Listed for shape, does nothing in nested mode.
    Inert,
}

#[derive(Clone, Debug)]
pub struct MenuItem {
    /// Dotted id — the tree IS the id space, like the jsonc.
    pub id: String,
    pub label: String,
    pub icon: Option<Ico>,
    pub kind: MenuKind,
    /// Appends " ✓" to the label.
    pub checked: bool,
    /// Drawn at α .4, skipped by the cursor, ignores clicks.
    pub disabled: bool,
    pub description: String,
    pub aliases: Vec<String>,
}

impl MenuItem {
    fn new(id: &str, label: &str, kind: MenuKind) -> Self {
        Self {
            id: id.to_string(),
            label: label.to_string(),
            icon: None,
            kind,
            checked: false,
            disabled: false,
            description: String::new(),
            aliases: Vec::new(),
        }
    }
    fn icon(mut self, ico: Ico) -> Self {
        self.icon = Some(ico);
        self
    }
    fn aliases(mut self, a: &[&str]) -> Self {
        self.aliases = a.iter().map(|s| s.to_string()).collect();
        self
    }
    fn describe(mut self, d: &str) -> Self {
        self.description = d.to_string();
        self
    }

    /// The parent id (`""` for a root entry).
    pub fn parent(&self) -> &str {
        match self.id.rfind('.') {
            Some(i) => &self.id[..i],
            None => "",
        }
    }

    /// The last dotted segment, `_`/`-` turned into spaces — what
    /// `MenuModel.js` searches as the leaf id.
    fn leaf(&self) -> String {
        self.id
            .rsplit('.')
            .next()
            .unwrap_or("")
            .replace(['_', '-'], " ")
            .to_lowercase()
    }

    pub fn depth(&self) -> usize {
        self.id.matches('.').count()
    }
}

/// The omarchy menu tree (`default/omarchy/omarchy-menu.jsonc`), root order
/// and children as declared. Entries with nothing behind them in nested
/// mode are listed and DISABLED rather than dropped, so the menu keeps its
/// real shape and tells the truth about what it can do.
pub fn omarchy_tree() -> Vec<MenuItem> {
    let mut v: Vec<MenuItem> = Vec::new();
    let mut menu = |id: &str, label: &str, ico: Ico| {
        v.push(MenuItem::new(id, label, MenuKind::Menu).icon(ico));
    };
    menu("apps", "Apps", Ico::Menu);
    menu("learn", "Learn", Ico::Search);
    menu("trigger", "Trigger", Ico::Record);
    menu("style", "Style", Ico::Moon);
    menu("setup", "Setup", Ico::Keyboard);
    menu("install", "Install", Ico::Refresh);
    menu("remove", "Remove", Ico::Close);
    menu("update", "Update", Ico::Refresh);
    menu("about", "About", Ico::Dot);
    menu("system", "System", Ico::Power);

    // Aliases the jsonc carries so typing "settings" finds Setup, etc.
    for (id, aliases) in [
        ("apps", &["app", "applications"][..]),
        ("setup", &["settings"][..]),
        ("remove", &["uninstall"][..]),
        ("system", &["power-menu", "power"][..]),
    ] {
        if let Some(item) = v.iter_mut().find(|i| i.id == id) {
            item.aliases = aliases.iter().map(|s| s.to_string()).collect();
        }
    }

    let mut child = |parent: &str, leaf: &str, label: &str, kind: MenuKind| {
        v.push(MenuItem::new(&format!("{}.{}", parent, leaf), label, kind));
    };
    for (leaf, label) in [
        ("keybindings", "Keybindings"),
        ("omarchy", "Omarchy"),
        ("hyprland", "Hyprland"),
        ("neovim", "Neovim"),
        ("bash", "Bash"),
        ("tmux-keybindings", "Tmux"),
        ("herdr-keybindings", "Herdr"),
        ("community", "Community"),
    ] {
        child("learn", leaf, label, MenuKind::Menu);
    }
    for (leaf, label) in [
        ("emoji", "Emoji"),
        ("reminder", "Reminder"),
        ("capture", "Capture"),
        ("transcode", "Transcode"),
        ("share", "Share"),
        ("toggle", "Toggle"),
        ("hardware", "Hardware"),
        ("tests", "Tests"),
    ] {
        child("trigger", leaf, label, MenuKind::Action);
    }
    for (leaf, label) in [
        ("theme", "Theme"),
        ("background", "Background"),
        ("unlock", "Unlock"),
        ("font", "Font"),
        ("bar", "Bar"),
        ("hyprland", "Hyprland"),
        ("screensaver", "Screensaver"),
        ("about", "About"),
    ] {
        child("style", leaf, label, MenuKind::Action);
    }
    for (leaf, label) in [
        ("monitors", "Monitors"),
        ("keybindings", "Keybindings"),
        ("input", "Input"),
        ("network", "Network"),
        ("default", "Defaults"),
        ("plugin", "Plugins"),
        ("security", "Security"),
        ("config", "Config"),
        ("direct-boot", "Direct Boot"),
        ("reset", "Reset"),
    ] {
        child("setup", leaf, label, MenuKind::Menu);
    }
    for (leaf, label) in [
        ("package", "Package"),
        ("aur", "AUR"),
        ("ai", "AI"),
        ("service", "Service"),
        ("development", "Development"),
        ("editor", "Editor"),
        ("style", "Style"),
        ("gaming", "Gaming"),
        ("browser", "Browser"),
        ("webapp", "Web App"),
        ("terminal", "Terminal"),
        ("tui", "TUI"),
        ("windows", "Windows"),
        ("preinstalls", "Preinstalls"),
    ] {
        child("install", leaf, label, MenuKind::Menu);
    }
    for (leaf, label) in [
        ("package", "Package"),
        ("ai", "AI"),
        ("service", "Service"),
        ("development", "Development"),
        ("theme", "Theme"),
        ("gaming", "Gaming"),
        ("browser", "Browser"),
        ("webapp", "Web App"),
        ("tui", "TUI"),
        ("windows", "Windows"),
        ("preinstalls", "Preinstalls"),
        ("security", "Security"),
    ] {
        child("remove", leaf, label, MenuKind::Menu);
    }
    for (leaf, label) in [
        ("omarchy", "Omarchy"),
        ("channel", "Channel"),
        ("config", "Config"),
        ("themes", "Themes"),
        ("process", "Process"),
        ("hardware", "Hardware"),
        ("firmware", "Firmware"),
        ("password", "Password"),
        ("timezone", "Timezone"),
        ("time", "Time"),
    ] {
        child("update", leaf, label, MenuKind::Action);
    }
    for (leaf, label) in [
        ("screensaver", "Screensaver"),
        ("lock", "Lock"),
        ("suspend", "Suspend"),
        ("hibernate", "Hibernate"),
        ("logout", "Logout"),
        ("reboot", "Reboot"),
        ("shutdown", "Shutdown"),
    ] {
        child("system", leaf, label, MenuKind::Action);
    }

    // What this desktop can really do. Everything else stays listed and
    // disabled — the menu never offers an action it cannot perform.
    let live: &[(&str, MenuKind, Option<Ico>, &[&str], &str)] = &[
        (
            "learn.keybindings",
            MenuKind::Menu,
            Some(Ico::Keyboard),
            &["keys", "shortcuts"],
            "Every binding this desktop answers to",
        ),
        (
            "style.theme",
            MenuKind::Menu,
            Some(Ico::Moon),
            &["theme", "themes"],
            "Switch or import a theme",
        ),
        (
            "style.background",
            MenuKind::Action,
            Some(Ico::Monitor),
            &["wallpaper"],
            "Next background of this theme",
        ),
        (
            "style.bar",
            MenuKind::Action,
            Some(Ico::Menu),
            &["bar", "top bar"],
            "Show or hide the top bar",
        ),
        ("apps", MenuKind::Menu, Some(Ico::Menu), &[], ""),
    ];
    for (id, kind, ico, aliases, desc) in live {
        if let Some(item) = v.iter_mut().find(|i| &i.id == id) {
            item.kind = *kind;
            if let Some(ico) = ico {
                item.icon = Some(*ico);
            }
            if !aliases.is_empty() {
                item.aliases = aliases.iter().map(|s| s.to_string()).collect();
            }
            if !desc.is_empty() {
                item.description = desc.to_string();
            }
        }
    }
    let alive: &[&str] = &[
        "apps",
        "learn",
        "learn.keybindings",
        "style",
        "style.theme",
        "style.background",
        "style.bar",
        "system",
    ];
    for item in v.iter_mut() {
        if !alive.contains(&item.id.as_str()) {
            item.disabled = true;
        }
    }
    v
}

/// The `style.theme` submenu: every installed theme, then the import list.
fn theme_items() -> Vec<MenuItem> {
    let current = std::fs::read_to_string(theme::themes_dir().join("../current-theme"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| theme::DEFAULT_THEME.to_string());
    let mut v: Vec<MenuItem> = theme::installed_themes()
        .into_iter()
        .map(|name| {
            let mut item = MenuItem::new(
                &format!("style.theme.{}", name),
                &name.replace('-', " "),
                MenuKind::Action,
            )
            .icon(Ico::Moon)
            .aliases(&[name.as_str()]);
            item.checked = name == current;
            item
        })
        .collect();
    v.push(
        MenuItem::new("style.theme.import", "Import from omarchy…", MenuKind::Menu)
            .icon(Ico::Refresh)
            .aliases(&["import", "omarchy"]),
    );
    for name in theme::OMARCHY_THEMES {
        v.push(
            MenuItem::new(
                &format!("style.theme.import.{}", name),
                &name.replace('-', " "),
                MenuKind::Action,
            )
            .icon(Ico::Moon)
            .aliases(&[name]),
        );
    }
    v
}

/// `learn.keybindings` — `omarchy-menu-keybindings`, showing OUR binds with
/// the combos this OS actually answers to.
fn key_items() -> Vec<MenuItem> {
    keymap()
        .iter()
        .enumerate()
        .map(|(i, bind)| {
            MenuItem::new(
                &format!("learn.keybindings.{}", i),
                bind.help,
                MenuKind::Inert,
            )
            .describe(&combo_text(bind))
            .icon(Ico::Keyboard)
        })
        .collect()
}

/// One drawn row.
#[derive(Clone, Debug)]
pub struct MenuRow {
    pub label: String,
    pub detail: String,
    pub icon: Option<Ico>,
    pub target: String,
    pub kind: MenuKind,
    pub disabled: bool,
    pub has_children: bool,
    /// A 17px divider sits above this row (same-parent matches above,
    /// deeper matches below).
    pub divider: bool,
}

/// Which token set the surface wears — the menu's, or the launcher's
/// translucent card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuSkin {
    Menu,
    Launcher,
}

#[derive(Clone, Debug)]
pub struct MenuModel {
    pub open: bool,
    pub skin: MenuSkin,
    /// The parent id whose children are showing.
    pub path: String,
    /// Visited parents, for Backspace/Left.
    pub stack: Vec<(String, usize)>,
    pub filter: String,
    pub sel: usize,
    pub rows: Vec<MenuRow>,
    pub scroll: usize,
    items: Vec<MenuItem>,
    /// The card top, frozen on the first keystroke or descent.
    pub frozen_top: Option<f64>,
}

impl Default for MenuModel {
    fn default() -> Self {
        Self {
            open: false,
            skin: MenuSkin::Menu,
            path: String::new(),
            stack: Vec::new(),
            filter: String::new(),
            sel: 0,
            rows: Vec::new(),
            scroll: 0,
            items: Vec::new(),
            frozen_top: None,
        }
    }
}

impl MenuModel {
    /// Every item of the tree plus the providers that are live under the
    /// current path (`style.theme`, `learn.keybindings`).
    ///
    /// The `apps` provider is ALWAYS in the index, whatever the path:
    /// `Menu.qml` searches every descendant of the open menu, so typing
    /// "vj" at the root has to find Apps > VJ. Which rows are LISTED is
    /// still the path's business — the search filter does that.
    fn all_items(path: &str) -> Vec<MenuItem> {
        let mut items = omarchy_tree();
        items.extend(launcher::apps());
        if path.starts_with("style.theme") {
            items.extend(theme_items());
        }
        if path.starts_with("learn.keybindings") {
            items.extend(key_items());
        }
        items
    }

    pub fn open_at(&mut self, path: &str, skin: MenuSkin) {
        self.open = true;
        self.skin = skin;
        self.path = path.to_string();
        self.stack.clear();
        self.filter.clear();
        self.sel = 0;
        self.scroll = 0;
        self.frozen_top = None;
        self.items = Self::all_items(path);
        self.rebuild();
    }

    pub fn close(&mut self) {
        self.open = false;
        self.rows.clear();
        self.stack.clear();
        self.filter.clear();
        self.frozen_top = None;
    }

    fn descend(&mut self, id: &str) {
        self.stack.push((self.path.clone(), self.sel));
        self.path = id.to_string();
        self.filter.clear();
        self.sel = 0;
        self.scroll = 0;
        self.items = Self::all_items(&self.path);
        self.rebuild();
    }

    /// `goBack`: pop the visited stack, else the parent, else nothing.
    pub fn back(&mut self) -> bool {
        if let Some((path, sel)) = self.stack.pop() {
            self.path = path;
            self.filter.clear();
            self.items = Self::all_items(&self.path);
            self.rebuild();
            self.sel = sel.min(self.rows.len().saturating_sub(1));
            return true;
        }
        if self.path.is_empty() {
            return false;
        }
        let parent = match self.path.rfind('.') {
            Some(i) => self.path[..i].to_string(),
            None => String::new(),
        };
        self.path = parent;
        self.filter.clear();
        self.sel = 0;
        self.items = Self::all_items(&self.path);
        self.rebuild();
        true
    }

    /// The card header: the filter while typing, else the current title
    /// with the ellipsis prompt.
    pub fn header(&self) -> (String, bool) {
        if !self.filter.is_empty() {
            return (self.filter.clone(), false);
        }
        let title = if self.path.is_empty() {
            "Menu".to_string()
        } else {
            self.items
                .iter()
                .find(|i| i.id == self.path)
                .map(|i| i.label.clone())
                .unwrap_or_else(|| self.path.clone())
        };
        (format!("{}\u{2026}", title), true)
    }

    /// The breadcrumb of an item, " › " joined — the detail line of a
    /// deeper match.
    fn breadcrumb(&self, item: &MenuItem) -> String {
        let mut parts: Vec<String> = Vec::new();
        let mut id = item.parent().to_string();
        while !id.is_empty() {
            if let Some(p) = self.items.iter().find(|i| i.id == id) {
                parts.push(p.label.clone());
            }
            id = match id.rfind('.') {
                Some(i) => id[..i].to_string(),
                None => String::new(),
            };
        }
        parts.reverse();
        parts.join(" \u{203a} ")
    }

    pub fn rebuild(&mut self) {
        let needle = self.filter.trim().to_lowercase();
        let path = self.path.clone();
        let items = self.items.clone();
        let mut rows: Vec<MenuRow> = Vec::new();

        if needle.is_empty() {
            // Browse: direct children, in declared order.
            for item in items.iter().filter(|i| i.parent() == path) {
                rows.push(self.row_for(item, String::new(), false));
            }
        } else {
            // Live search over the open submenu's SUBTREE, scored.
            let prefix = if path.is_empty() {
                String::new()
            } else {
                format!("{}.", path)
            };
            let mut scored: Vec<(i64, String, usize)> = Vec::new();
            for (order, item) in items.iter().enumerate() {
                if item.id.is_empty() || item.disabled {
                    continue;
                }
                if !prefix.is_empty() && !item.id.starts_with(&prefix) {
                    continue;
                }
                let Some(score) = search_score(item, &needle, item.depth(), order) else {
                    continue;
                };
                scored.push((score, item.id.clone(), order));
            }
            scored.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
            let same_parent = |id: &str| -> bool {
                match id.rfind('.') {
                    Some(i) => id[..i] == path,
                    None => path.is_empty(),
                }
            };
            let mut seen_deep = false;
            for (_, id, _) in scored {
                let Some(item) = items.iter().find(|i| i.id == id) else {
                    continue;
                };
                let deep = !same_parent(&item.id);
                let divider = deep && !seen_deep && !rows.is_empty();
                if deep {
                    seen_deep = true;
                }
                let detail = if deep { self.breadcrumb(item) } else { String::new() };
                rows.push(self.row_for(item, detail, divider));
            }
        }

        self.rows = rows;
        if self.sel >= self.rows.len() {
            self.sel = self.rows.len().saturating_sub(1);
        }
        // The cursor never lands on a disabled row.
        if self
            .rows
            .get(self.sel)
            .map(|r| r.disabled)
            .unwrap_or(false)
        {
            if let Some(next) = self.rows.iter().position(|r| !r.disabled) {
                self.sel = next;
            }
        }
    }

    fn row_for(&self, item: &MenuItem, detail: String, divider: bool) -> MenuRow {
        let label = if item.checked {
            format!("{} \u{2713}", item.label)
        } else {
            item.label.clone()
        };
        let detail = if detail.is_empty() {
            item.description.clone()
        } else {
            detail
        };
        MenuRow {
            label,
            detail,
            icon: item.icon,
            target: item.id.clone(),
            kind: item.kind,
            disabled: item.disabled,
            has_children: item.kind == MenuKind::Menu,
            divider,
        }
    }

    pub fn move_sel(&mut self, by: i64) {
        if self.rows.is_empty() {
            return;
        }
        let len = self.rows.len() as i64;
        let mut next = (self.sel as i64 + by).clamp(0, len - 1);
        // Skip disabled rows in the direction of travel.
        let step = if by >= 0 { 1 } else { -1 };
        while next >= 0 && next < len && self.rows[next as usize].disabled {
            next += step;
        }
        if next < 0 || next >= len {
            return;
        }
        self.sel = next as usize;
    }

    /// What activating the current row means. `None` when the row is a
    /// submenu (already descended) or dead.
    pub fn activate(&mut self) -> Option<String> {
        let row = self.rows.get(self.sel)?.clone();
        if row.disabled {
            return None;
        }
        match row.kind {
            MenuKind::Menu => {
                self.descend(&row.target);
                None
            }
            MenuKind::Inert | MenuKind::Link => None,
            MenuKind::Action | MenuKind::App => Some(row.target),
        }
    }
}

/// `MenuModel.js` `matchesQuery` + `searchScore`, lower is better.
///
/// Every whitespace term has to match the label, the leaf id or an alias by
/// substring, or the description by whole word. The tiers are then: label
/// equal to the query, an app whose label contains the query as a whole
/// word, label prefix, label substring, the searchable-name text, a
/// description whole word, else 80 — with a −2 nudge for menus and links
/// and −5 for apps, and `tier * 1000 + depth * 25 + order` as the sort key.
pub fn search_score(item: &MenuItem, needle: &str, depth: usize, order: usize) -> Option<i64> {
    let label = item.label.to_lowercase();
    let leaf = item.leaf();
    let aliases = item
        .aliases
        .iter()
        .map(|a| a.replace(['.', '_', '-'], " "))
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let name_text = format!("{} {} {}", label, leaf, aliases);
    let description = item.description.to_lowercase();
    let whole_word = |hay: &str, term: &str| hay.split_whitespace().any(|w| w == term);

    for term in needle.split_whitespace() {
        if !(name_text.contains(term) || whole_word(&description, term)) {
            return None;
        }
    }

    let mut tier: i64 = 80;
    if label == needle {
        tier = if depth == 0 { 0 } else { 2 };
    } else if item.kind == MenuKind::App && whole_word(&label, needle) {
        tier = 0;
    } else if label.starts_with(needle) {
        tier = 10;
    } else if label.contains(needle) {
        tier = 30;
    } else if name_text.contains(needle) {
        tier = 40;
    } else if whole_word(&description, needle) {
        tier = 60;
    }
    if matches!(item.kind, MenuKind::Menu | MenuKind::Link) {
        tier -= 2;
    }
    if item.kind == MenuKind::App {
        tier -= 5;
    }
    Some(tier * 1000 + depth as i64 * 25 + order as i64)
}

// ======================================================================
// The surface
// ======================================================================

/// Geometry, `Menu.qml`.
const CARD_WIDTH: f64 = 300.0;
const HEADER_HEIGHT: f64 = 34.0;
const HEADER_GAP: f64 = 6.0;
const ROW_HEIGHT: f64 = 50.0;
const ROW_HEIGHT_DETAIL: f64 = 58.0;
const ROW_SPACING: f64 = 3.0;
const ROW_PEEK: f64 = 28.0;
const DIVIDER_HEIGHT: f64 = 17.0;
const ICON_COLUMN: f64 = 36.0;
const ICON_MARGIN: f64 = 8.0;
const ICON_GAP: f64 = 6.0;
const LABEL_INSET_NO_ICON: f64 = 18.0;
const CHEVRON_COLUMN: f64 = 14.0;
const CHEVRON_MARGIN: f64 = 8.0;
/// The card never grows past 70% of the screen.
const MAX_HEIGHT_FRACTION: f64 = 0.7;
/// Empty state: gap between the accent glyph and the message.
const EMPTY_BLOCK_GAP: f64 = 8.0;

/// Splits the header row into an icon slot (square, `icon_size` wide, at
/// the left edge) and the text rect that follows it — so the search glyph
/// is always exactly `icon_size` px, inline with the filter/title text,
/// never the oversized, header-filling glyph this used to draw. Both rects
/// stay inside `header_rect` (never past the card).
fn header_icon_and_text_rects(header_rect: Rect, icon_size: f64) -> (Rect, Rect) {
    let icon_slot = rect(header_rect.pos.x, header_rect.pos.y, icon_size, header_rect.size.y);
    let text_rect = rect(
        header_rect.pos.x + icon_size + ICON_GAP,
        header_rect.pos.y,
        (header_rect.size.x - icon_size - ICON_GAP).max(0.0),
        header_rect.size.y,
    );
    (icon_slot, text_rect)
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ShellMenuBase = #(ShellMenu::register_widget(vm))
    mod.widgets.ShellMenu = set_type_default() do mod.widgets.ShellMenuBase {
        width: Fill
        height: Fill
        draw_bg +: {}
        d +: {}
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum ShellMenuAction {
    /// A row was activated: its dotted id.
    Activate(String),
    /// Escape, or a click outside the card.
    Cancel,
    #[default]
    None,
}

/// `Ui/PointerMoveGate.qml`: hover only re-selects after real pointer
/// motion, so a list moving under a still pointer never steals the cursor.
#[derive(Clone, Copy, Debug, Default)]
pub struct PointerGate {
    last: Vec2d,
    armed: bool,
}

impl PointerGate {
    pub fn reset(&mut self) {
        self.armed = false;
    }
    /// True when this move is real pointer motion (≥ 1px).
    pub fn moved(&mut self, p: Vec2d) -> bool {
        let moved = !self.armed || (p.x - self.last.x).abs() >= 1.0 || (p.y - self.last.y).abs() >= 1.0;
        self.last = p;
        self.armed = true;
        moved
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct ShellMenu {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawShellFill,
    #[live]
    d: ShellDraw,
    #[live]
    tokens: ShellTokens,
    #[rust]
    pub model: MenuModel,
    #[rust]
    area: Area,
    #[rust]
    screen: Rect,
    #[rust]
    card: Rect,
    #[rust]
    row_rects: Vec<Rect>,
    #[rust]
    gate: PointerGate,
    /// Fixture mode: the gallery draws the surface without owning input.
    #[rust]
    pub inert: bool,
}

impl ShellMenu {
    fn skin(&self) -> MenuTokens {
        match self.model.skin {
            MenuSkin::Menu => self.tokens.menu,
            MenuSkin::Launcher => self.tokens.launcher,
        }
    }

    pub fn open_at(&mut self, cx: &mut Cx, path: &str, skin: MenuSkin) {
        self.model.open_at(path, skin);
        self.gate.reset();
        self.redraw(cx);
    }

    pub fn close(&mut self, cx: &mut Cx) {
        self.model.close();
        self.redraw(cx);
    }

    /// A live theme switch: new tokens, redraw.
    pub fn set_tokens(&mut self, cx: &mut Cx, tokens: ShellTokens) {
        self.tokens = tokens;
        self.redraw(cx);
    }

    fn row_height(&self) -> f64 {
        if self.model.rows.iter().any(|r| !r.detail.is_empty()) {
            ROW_HEIGHT_DETAIL
        } else {
            ROW_HEIGHT
        }
    }

    /// The empty state's own height (accent glyph + gap + message line) —
    /// what the "rows area" collapses to when nothing matches.
    fn empty_block_height(&self) -> f64 {
        let tok = &self.tokens;
        tok.font.display_large + EMPTY_BLOCK_GAP + tok.font.title * 1.6
    }

    /// Total height of `n` rows including their spacing and any dividers.
    fn rows_height(&self, n: usize) -> f64 {
        let rh = self.row_height();
        let dividers = self
            .model
            .rows
            .iter()
            .take(n)
            .filter(|r| r.divider)
            .count() as f64;
        if n == 0 {
            0.0
        } else {
            n as f64 * rh + (n as f64 - 1.0) * ROW_SPACING + dividers * DIVIDER_HEIGHT
        }
    }

    /// The card rect for a screen, and how many rows fit in it.
    fn layout_card(&self, screen: Rect) -> (Rect, usize) {
        let tok = &self.tokens;
        let pad = tok.spacing.panel_padding;
        let gaps_out = tok.spacing.gaps_out;
        let chrome = pad * 2.0 + HEADER_HEIGHT + HEADER_GAP;
        let max_h = (screen.size.y * MAX_HEIGHT_FRACTION).min(screen.size.y - gaps_out * 2.0);
        let avail = (max_h - chrome).max(self.row_height());
        let rh = self.row_height();
        let full = (((avail + ROW_SPACING) / (rh + ROW_SPACING)).floor() as usize).max(1);
        let visible = full.min(self.model.rows.len().max(1));
        let clipped = self.model.rows.len() > visible;
        let list_h = if self.model.rows.is_empty() {
            // The rows area collapses to exactly the empty-state block —
            // never the zero it fell out to before, never a full page.
            self.empty_block_height()
        } else if clipped {
            self.rows_height(visible) + ROW_PEEK
        } else {
            self.rows_height(self.model.rows.len())
        };
        let height = (chrome + list_h).min(max_h);
        let x = (screen.pos.x + (screen.size.x - CARD_WIDTH) * 0.5).floor();
        let y = match self.model.frozen_top {
            Some(top) => top,
            None => (screen.pos.y + (screen.size.y - height) * 0.5)
                .max(screen.pos.y + gaps_out)
                .floor(),
        };
        (rect(x, y, CARD_WIDTH, height), visible)
    }

    /// Draw the whole surface into `screen`. Under glass it draws in its
    /// own overlay list, claimed every frame, open or not.
    pub fn draw_surface(&mut self, cx: &mut Cx2d, screen: Rect) {
        let tokens = self.tokens;
        self.d.begin_surface(cx, &tokens);
        self.draw_surface_inner(cx, screen);
        self.d.end_surface(cx);
    }

    /// The body, scrim included.
    fn draw_surface_inner(&mut self, cx: &mut Cx2d, screen: Rect) {
        self.screen = screen;
        if !self.model.open {
            self.card = Rect::default();
            self.row_rects.clear();
            return;
        }
        let tok = self.tokens;
        let skin = self.skin();
        let pad = tok.spacing.panel_padding;
        self.screen = screen;

        // Full-screen scrim, then the card.
        self.draw_bg.color = skin.scrim_color();
        self.draw_bg.draw_abs(cx, screen);

        let (card, visible) = self.layout_card(screen);
        self.card = card;
        self.d.card(cx, card, &skin.surface);

        // Header: the filter, or the title with its ellipsis prompt at α .58.
        let (header, placeholder) = self.model.header();
        let header_rect = rect(
            card.pos.x + pad,
            card.pos.y + pad,
            card.size.x - pad * 2.0,
            HEADER_HEIGHT,
        );
        let header_color = if placeholder {
            fade(skin.surface.text, 0.58)
        } else {
            skin.surface.text
        };
        // An inline search glyph, `iconLarge` (18px) like every row icon —
        // never the whole header row — sitting left of the filter/title
        // text and vertically centered in the 34px header.
        let (header_icon_slot, header_text_rect) =
            header_icon_and_text_rects(header_rect, tok.font.icon_large);
        self.d.icon_centered(
            cx,
            Ico::Search,
            header_icon_slot,
            tok.font.icon_large,
            header_color,
        );
        self.d.label_elided(
            cx,
            header_text_rect,
            false,
            tok.font.heading,
            header_color,
            super::ui::HAlign::Left,
            &header,
        );

        let list = rect(
            card.pos.x + pad,
            card.pos.y + pad + HEADER_HEIGHT + HEADER_GAP,
            card.size.x - pad * 2.0,
            (card.size.y - pad * 2.0 - HEADER_HEIGHT - HEADER_GAP).max(0.0),
        );

        self.row_rects.clear();
        if self.model.rows.is_empty() {
            // Empty state: the accent glyph over the message, centered as
            // one block inside the rows area — clipped to `list` so it can
            // never draw past the card even if the block is taller than
            // the space `layout_card` gave it.
            cx.push_clip_rect(list);
            let block_h = self.empty_block_height();
            let top = list.pos.y + ((list.size.y - block_h) * 0.5).max(0.0);
            let icon = rect(list.pos.x, top, list.size.x, tok.font.display_large);
            self.d.icon_centered(
                cx,
                Ico::Search,
                icon,
                tok.font.display_large,
                skin.selected_text,
            );
            let msg = rect(
                list.pos.x,
                icon.pos.y + icon.size.y + EMPTY_BLOCK_GAP,
                list.size.x,
                tok.font.title * 1.6,
            );
            let text = format!("No matches for \"{}\"", self.model.filter);
            self.d.label_elided(
                cx,
                msg,
                false,
                tok.font.title,
                fade(skin.surface.text, 0.7),
                super::ui::HAlign::Center,
                &text,
            );
            cx.pop_clip_rect();
            return;
        }

        // Keep the cursor row inside the visible window.
        let first = self.model.scroll.min(self.model.rows.len().saturating_sub(1));
        let rh = self.row_height();

        cx.push_clip_rect(list);
        let mut y = list.pos.y;
        for (i, row) in self.model.rows.iter().enumerate().skip(first) {
            if y > list.pos.y + list.size.y {
                break;
            }
            if row.divider {
                let line = rect(
                    list.pos.x,
                    (y + DIVIDER_HEIGHT * 0.5).floor(),
                    list.size.x,
                    1.0,
                );
                self.d
                    .solid(cx, line, alpha(skin.surface.text, 0.2));
                y += DIVIDER_HEIGHT;
            }
            let row_rect = rect(list.pos.x, y, list.size.x, rh);
            self.row_rects.push(row_rect);
            let selected = i == self.model.sel;
            let dim = if row.disabled { 0.4 } else { 1.0 };
            if selected {
                self.d.solid(cx, row_rect, skin.selected_bg());
            }
            let text_color = fade(
                if selected {
                    skin.selected_text
                } else {
                    skin.surface.text
                },
                dim,
            );
            // Menu.qml pulls an icon-less label left to LABEL_INSET_NO_ICON;
            // the user wants one label column, so an icon-less row keeps an
            // EMPTY icon slot and every label starts at the same x.
            let label_x = ICON_MARGIN + ICON_COLUMN + ICON_GAP;
            if let Some(ico) = row.icon {
                self.d.icon_centered(
                    cx,
                    ico,
                    rect(row_rect.pos.x + ICON_MARGIN, y, ICON_COLUMN, rh),
                    tok.font.icon_large,
                    text_color,
                );
            }
            let label_w = row_rect.size.x - label_x - CHEVRON_COLUMN - CHEVRON_MARGIN;
            if row.detail.is_empty() {
                self.d.label_elided(
                    cx,
                    rect(row_rect.pos.x + label_x, y, label_w, rh),
                    false,
                    tok.font.heading,
                    text_color,
                    super::ui::HAlign::Left,
                    &row.label,
                );
            } else {
                let half = rh * 0.5;
                self.d.label_elided(
                    cx,
                    rect(row_rect.pos.x + label_x, y + 4.0, label_w, half),
                    false,
                    tok.font.heading,
                    text_color,
                    super::ui::HAlign::Left,
                    &row.label,
                );
                self.d.label_elided(
                    cx,
                    rect(row_rect.pos.x + label_x, y + half, label_w, half - 4.0),
                    false,
                    tok.font.body_small,
                    fade(text_color, 0.52),
                    super::ui::HAlign::Left,
                    &row.detail,
                );
            }
            if row.has_children {
                self.d.icon_centered(
                    cx,
                    Ico::ChevronRight,
                    rect(
                        row_rect.pos.x + row_rect.size.x - CHEVRON_MARGIN - CHEVRON_COLUMN,
                        y,
                        CHEVRON_COLUMN,
                        rh,
                    ),
                    tok.font.heading * 0.8,
                    fade(text_color, 0.36),
                );
            }
            y += rh + ROW_SPACING;
        }
        cx.pop_clip_rect();

        let _ = visible;
    }

    /// Scroll so the cursor row is on screen (the list is the only thing
    /// that scrolls; the card top is frozen).
    fn follow_cursor(&mut self, screen: Rect) {
        let (card, visible) = self.layout_card(screen);
        let _ = card;
        if self.model.sel < self.model.scroll {
            self.model.scroll = self.model.sel;
        } else if self.model.sel >= self.model.scroll + visible {
            self.model.scroll = self.model.sel + 1 - visible;
        }
    }

    /// As `handle_key`, against the rect the surface was last drawn into —
    /// what the WM calls, since it does not track the overlay's geometry.
    pub fn key(&mut self, cx: &mut Cx, e: &KeyEvent) -> bool {
        let screen = self.screen;
        self.handle_key(cx, e, screen)
    }

    /// As `handle_pointer`, against the last-drawn screen — the WM's own
    /// modal intercept calls this directly (see `App::handle_event`'s
    /// pointer-event block) instead of letting the point reach this widget
    /// through the ordinary tree dispatch, so the click a row consumes can
    /// never ALSO fall through to whatever the scrim is dimming. `key()`
    /// is this same shape for the keyboard, which the menu already grabs
    /// exclusively while open.
    pub fn pointer(&mut self, cx: &mut Cx, event: &Event) -> bool {
        let screen = self.screen;
        self.handle_pointer(cx, event, screen)
    }

    pub fn is_open(&self) -> bool {
        self.model.open
    }

    /// The menu's key table, in order (`Menu.qml` `Keys.onPressed`).
    /// Returns true when the key was consumed.
    pub fn handle_key(&mut self, cx: &mut Cx, e: &KeyEvent, screen: Rect) -> bool {
        if !self.model.open {
            return false;
        }
        let has_filter = !self.model.filter.is_empty();
        match e.key_code {
            KeyCode::Escape => {
                if has_filter {
                    self.model.filter.clear();
                    self.model.sel = 0;
                    self.model.rebuild();
                } else {
                    self.model.close();
                    cx.widget_action(self.uid, ShellMenuAction::Cancel);
                }
            }
            KeyCode::Backspace => {
                if has_filter {
                    if e.modifiers.control {
                        // Ctrl+Backspace deletes a word, Ctrl+U the lot.
                        let trimmed = self.model.filter.trim_end();
                        let cut = trimmed.rfind(' ').map(|i| i + 1).unwrap_or(0);
                        self.model.filter.truncate(cut);
                    } else {
                        self.model.filter.pop();
                    }
                    self.model.sel = 0;
                    self.model.rebuild();
                } else if !self.model.back() {
                    self.model.close();
                    cx.widget_action(self.uid, ShellMenuAction::Cancel);
                }
            }
            KeyCode::KeyU if e.modifiers.control => {
                self.model.filter.clear();
                self.model.sel = 0;
                self.model.rebuild();
            }
            KeyCode::ArrowLeft => {
                if !has_filter && !self.model.back() {
                    self.model.close();
                    cx.widget_action(self.uid, ShellMenuAction::Cancel);
                }
            }
            KeyCode::ArrowDown => self.model.move_sel(1),
            KeyCode::ArrowUp => self.model.move_sel(-1),
            KeyCode::PageDown => self.model.move_sel(6),
            KeyCode::PageUp => self.model.move_sel(-6),
            KeyCode::ReturnKey | KeyCode::NumpadEnter | KeyCode::ArrowRight => {
                self.activate_selected(cx);
            }
            other => {
                if e.modifiers.control || e.modifiers.logo || e.modifiers.alt {
                    return false;
                }
                let Some(ch) = other.to_char(e.modifiers.shift) else {
                    return false;
                };
                if (ch as u32) < 32 || ch as u32 == 127 {
                    return false;
                }
                // The first keystroke freezes the card top.
                if self.model.frozen_top.is_none() {
                    self.model.frozen_top = Some(self.layout_card(screen).0.pos.y);
                }
                self.model.filter.push(ch);
                self.model.sel = 0;
                self.model.scroll = 0;
                self.model.rebuild();
            }
        }
        self.gate.reset();
        self.follow_cursor(screen);
        self.redraw(cx);
        true
    }

    fn activate_selected(&mut self, cx: &mut Cx) {
        // A descent freezes the card top too.
        if self.model.frozen_top.is_none() {
            self.model.frozen_top = Some(self.card.pos.y);
        }
        if let Some(target) = self.model.activate() {
            self.model.close();
            cx.widget_action(self.uid, ShellMenuAction::Activate(target));
        }
        self.gate.reset();
        self.redraw(cx);
    }

    /// Mouse, exactly like the original: hover moves the cursor through the
    /// gate, a click on an enabled row activates, a click outside cancels,
    /// and the wheel scrolls.
    pub fn handle_pointer(&mut self, cx: &mut Cx, event: &Event, screen: Rect) -> bool {
        if !self.model.open {
            return false;
        }
        match event {
            Event::MouseMove(e) => {
                if !self.gate.moved(e.abs) {
                    return true;
                }
                if let Some(i) = self.row_at(e.abs) {
                    if !self.model.rows[i].disabled && self.model.sel != i {
                        self.model.sel = i;
                        self.redraw(cx);
                    }
                }
                true
            }
            Event::MouseDown(e) => {
                if !contains(self.card, e.abs) {
                    self.model.close();
                    cx.widget_action(self.uid, ShellMenuAction::Cancel);
                    self.redraw(cx);
                    return true;
                }
                if let Some(i) = self.row_at(e.abs) {
                    if !self.model.rows[i].disabled {
                        self.model.sel = i;
                        self.activate_selected(cx);
                    }
                }
                true
            }
            Event::MouseUp(_) => true,
            Event::Scroll(e) => {
                // Only the list under the pointer scrolls — the card
                // swallows its own wheel, nothing else.
                if !contains(self.card, e.abs) {
                    return false;
                }
                if e.scroll.y.abs() > 0.5 {
                    let max = self.model.rows.len().saturating_sub(1);
                    if e.scroll.y > 0.0 {
                        self.model.scroll = (self.model.scroll + 1).min(max);
                    } else {
                        self.model.scroll = self.model.scroll.saturating_sub(1);
                    }
                    self.gate.reset();
                    self.redraw(cx);
                }
                let _ = screen;
                true
            }
            _ => false,
        }
    }

    fn row_at(&self, p: Vec2d) -> Option<usize> {
        let first = self.model.scroll;
        self.row_rects
            .iter()
            .position(|r| contains(*r, p))
            .map(|i| i + first)
            .filter(|i| *i < self.model.rows.len())
    }
}

impl Widget for ShellMenu {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        let screen = cx.turtle().rect();
        self.draw_surface(cx, screen);
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.inert || !self.model.open {
            return;
        }
        let screen = self.screen;
        self.handle_pointer(cx, event, screen);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tree_has_the_omarchy_root_in_order() {
        let tree = omarchy_tree();
        let roots: Vec<&str> = tree
            .iter()
            .filter(|i| i.parent().is_empty())
            .map(|i| i.id.as_str())
            .collect();
        assert_eq!(
            roots,
            vec![
                "apps", "learn", "trigger", "style", "setup", "install", "remove", "update",
                "about", "system"
            ]
        );
        // The children the jsonc declares are there too.
        assert!(tree.iter().any(|i| i.id == "system.shutdown"));
        assert!(tree.iter().any(|i| i.id == "install.preinstalls"));
        assert!(tree.iter().any(|i| i.id == "learn.keybindings"));
        // What we cannot do is listed and disabled, never hidden.
        assert!(tree.iter().find(|i| i.id == "system.reboot").unwrap().disabled);
        assert!(!tree.iter().find(|i| i.id == "style.theme").unwrap().disabled);
    }

    #[test]
    fn browsing_shows_direct_children_in_order() {
        let mut m = MenuModel::default();
        m.open_at("", MenuSkin::Menu);
        assert_eq!(m.rows.len(), 10);
        assert_eq!(m.rows[0].label, "Apps");
        assert_eq!(m.rows[9].label, "System");
        // Submenu rows carry the chevron.
        assert!(m.rows[0].has_children);
    }

    #[test]
    fn search_scores_the_way_menumodel_does() {
        let apps = MenuItem::new("apps", "Apps", MenuKind::Menu);
        let style = MenuItem::new("style", "Style", MenuKind::Menu);
        // An exact top-level label match is tier 0, minus the menu nudge.
        let exact = search_score(&apps, "apps", 0, 0).unwrap();
        let prefix = search_score(&style, "st", 0, 3).unwrap();
        assert!(exact < prefix);
        // A non-matching term drops the row entirely.
        assert!(search_score(&apps, "zzz", 0, 0).is_none());
        // Depth and declaration order break ties, in that order.
        let deep = MenuItem::new("style.theme", "Apps", MenuKind::Menu);
        assert!(search_score(&apps, "apps", 0, 0).unwrap() < search_score(&deep, "apps", 1, 0).unwrap());
        // Apps outrank menus at the same tier.
        let app = MenuItem::new("apps.terminal", "Terminal", MenuKind::App);
        let menu = MenuItem::new("terminal", "Terminal", MenuKind::Menu);
        assert!(search_score(&app, "term", 1, 0).unwrap() < search_score(&menu, "term", 1, 0).unwrap());
    }

    #[test]
    fn back_walks_the_visited_stack_then_the_parents() {
        let mut m = MenuModel::default();
        m.open_at("", MenuSkin::Menu);
        m.sel = 3; // Style
        assert_eq!(m.rows[3].label, "Style");
        m.activate();
        assert_eq!(m.path, "style");
        assert!(m.rows.iter().any(|r| r.label == "Theme"));
        assert!(m.back());
        assert_eq!(m.path, "");
        assert_eq!(m.sel, 3);
        // At the root there is nowhere left to go.
        assert!(!m.back());
    }

    #[test]
    fn checked_rows_get_the_tick_and_disabled_rows_are_skipped() {
        let mut m = MenuModel::default();
        m.open_at("system", MenuSkin::Menu);
        assert!(m.rows.iter().all(|r| r.disabled));
        // Every row disabled: the cursor stays put rather than landing on one.
        let before = m.sel;
        m.move_sel(1);
        assert_eq!(m.sel, before);

        let mut item = MenuItem::new("style.theme.nord", "nord", MenuKind::Action);
        item.checked = true;
        let row = m.row_for(&item, String::new(), false);
        assert_eq!(row.label, "nord \u{2713}");
    }

    #[test]
    fn the_pointer_gate_ignores_a_still_pointer() {
        let mut gate = PointerGate::default();
        assert!(gate.moved(dvec2(10.0, 10.0)));
        // The list moved, not the pointer.
        assert!(!gate.moved(dvec2(10.0, 10.0)));
        assert!(gate.moved(dvec2(10.0, 12.0)));
        gate.reset();
        assert!(gate.moved(dvec2(10.0, 12.0)));
    }

    /// The header search glyph is `iconLarge` (18px), inline with the
    /// filter text inside the 34px header row — never the oversized glyph
    /// centered over the whole card this used to draw.
    #[test]
    fn header_icon_is_inline_and_never_bigger_than_the_row() {
        let header_rect = rect(18.0, 18.0, CARD_WIDTH - 36.0, HEADER_HEIGHT);
        let icon_size = 18.0;
        let (icon_slot, text_rect) = header_icon_and_text_rects(header_rect, icon_size);
        // The glyph is exactly icon_size square, not the header's height or
        // the card's width, and it never grows past the header row.
        assert_eq!(icon_slot.size.x, icon_size);
        assert_eq!(icon_slot.size.y, header_rect.size.y);
        assert!(icon_slot.size.x <= HEADER_HEIGHT);
        // The text starts after the icon plus its gap, and both rects stay
        // fully inside the header row — nothing overlaps, nothing spills.
        assert_eq!(text_rect.pos.x, header_rect.pos.x + icon_size + ICON_GAP);
        assert_eq!(icon_slot.pos.x + icon_slot.size.x, header_rect.pos.x + icon_size);
        assert!(text_rect.pos.x + text_rect.size.x <= header_rect.pos.x + header_rect.size.x);
        assert_eq!(icon_slot.pos.y, header_rect.pos.y);
        assert_eq!(text_rect.pos.y, header_rect.pos.y);
    }
}
