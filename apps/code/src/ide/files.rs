//! The searchable file tree. A filter field on top; under it the lazy tree
//! (git-ignored entries are not listed), or, while the filter has text, a
//! flat list of the index's files ranked by a fuzzy match.
//!
//! Contract: `reveal(cx, rel)` opens the folders above a file, selects and
//! scrolls to it; `focus_filter(cx)` puts the keyboard in the filter field.
//! Rows emit `IdeAction::OpenFile`; a drag of a row starts
//! `DragItem::FilePath` with the absolute path; create / rename / delete
//! emit `Created` / `Renamed` / `Deleted`; "Add to chat" emits
//! `AddReference`.
//!
//! Keys while the tree has the keyboard: Up/Down select, Left folds or goes
//! to the parent, Right unfolds or goes into the folder, Enter opens a file
//! or toggles a folder, F2 renames, Delete or Cmd+Backspace deletes (after a
//! confirm), `a` or Cmd+N makes a new file in the selected folder (a name
//! ending in `/` makes a folder).

pub mod tree;

use self::tree::FileTree;
use makepad_code_agent::index::FileMatch;
use super::{IdeAction, Workspace};
use makepad_widgets::*;
use std::path::PathBuf;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.sdf.Finish

    mod.widgets.CodeHitTextBase = #(HitText::register_widget(vm))

    /// One line of mono text in runs of colour (a path with the characters
    /// a search matched lit), cut with an ellipsis at the right edge.
    mod.widgets.CodeHitText = set_type_default() do mod.widgets.CodeHitTextBase{
        width: Fill
        height: 20
        draw_text +: {
            color: mod.look.text
            text_style: mod.look.mono{font_size: 8.5}
        }
    }

    mod.widgets.FilesPanelBase = #(FilesPanel::register_widget(vm))

    let Mono = Label{
        width: Fit
        height: Fit
        padding: 0
        text: ""
        draw_text +: {
            color: mod.look.text
            text_style: mod.look.mono{font_size: 8.5}
        }
    }

    let Rule = View{
        width: Fill
        height: 1
        show_bg: true
        draw_bg +: {
            color: mod.look.hair_hi
            pixel: fn() {
                return vec4(self.color.rgb * self.color.a, self.color.a)
            }
        }
    }

    let Field = TextInput{
        draw_text +: {
            color: mod.look.text_hi
            color_hover: uniform(mod.look.text_hi)
            color_focus: uniform(mod.look.text_hi)
            color_down: uniform(mod.look.text_hi)
            color_empty: uniform(mod.look.dim)
            color_empty_hover: uniform(mod.look.dim)
            color_empty_focus: uniform(mod.look.dim)
            text_style: mod.look.mono{font_size: 9.0}
        }
        draw_cursor +: { color: mod.look.agent }
        draw_selection +: {
            color: mod.look.agent_select
            color_hover: mod.look.agent_select
            color_focus: mod.look.agent_select
        }
        draw_bg +: {
            color: #0000
            color_hover: #0000
            color_focus: #0000
            color_empty: #0000
            color_empty_hover: #0000
            color_empty_focus: #0000
            border_size: 0.0
        }
    }

    let KeyBtn = ButtonFlat{
        height: 20
        padding: Inset{left: 8 right: 8 top: 0 bottom: 0}
        margin: 0
        draw_text +: {
            color: mod.look.text
            color_hover: mod.look.text_hi
            color_down: uniform(mod.look.text_hi)
            color_focus: uniform(mod.look.text_hi)
            text_style: mod.look.mono{font_size: 8.5}
        }
        draw_bg +: {
            ink: uniform(mod.look.meta)
            pixel: fn() {
                let p = self.pos * self.rect_size
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                let c = self.rect_size * 0.5
                let d = Finish.sd_chamfer(p, c, c - vec2(0.5, 0.5), 4.0)
                let edge = Finish.band(d, 0.0, px, px)
                let fill = Finish.cover(d, px)
                let wash = 0.05 + 0.12 * self.hover + 0.22 * self.down
                let a = clamp(fill * wash + edge * (0.55 + 0.45 * max(self.hover, self.focus)), 0.0, 1.0)
                return vec4(self.ink.rgb * a, a)
            }
        }
    }

    /// A row's wash: the selected row carries a 2-point bar of the agent's
    /// colour on its left.
    let RowHead = View{
        width: Fill
        height: 20
        flow: Right
        spacing: 4
        align: Align{y: 0.5}
        padding: Inset{left: 4 right: 4}
        cursor: MouseCursor.Hand
        grab_key_focus: false
        show_bg: true
        draw_bg +: {
            color: #0000
            pixel: fn() {
                let p = self.pos * self.rect_size
                let bar = (1.0 - step(2.0, p.x)) * step(0.001, self.color.a)
                let c = mix(self.color, mod.look.agent, bar)
                return vec4(c.rgb * c.a, c.a)
            }
        }
    }

    let MenuRow = RowHead{
        width: Fill
        height: 22
        padding: Inset{left: 10 right: 10}
        menu_label := Mono{
            width: Fill
            draw_text +: { color: mod.look.text_hi }
        }
        menu_key := Mono{
            draw_text +: { color: mod.look.meta text_style: mod.look.mono{font_size: 8.0} }
        }
    }

    mod.widgets.FilesPanel = set_type_default() do mod.widgets.FilesPanelBase{
        width: Fill
        height: Fill
        flow: Down
        // The panel's own area holds the keyboard while the tree has it.
        show_bg: true
        draw_bg +: { color: #0000 }

        View{
            width: Fill
            height: 28
            flow: Right
            align: Align{y: 0.5}
            padding: Inset{left: 8 right: 6}
            Mono{
                width: 12
                text: "▸"
                draw_text +: { color: mod.look.agent text_style: mod.look.mono_bold{font_size: 9.0} }
            }
            filter := Field{
                width: Fill
                height: Fit
                empty_text: "filter files  ⌘P open"
            }
        }
        Rule{}

        // New file / folder or rename: a caption and the name field.
        name_bar := View{
            visible: false
            width: Fill
            height: Fit
            flow: Down
            padding: Inset{left: 8 right: 6 top: 4 bottom: 2}
            show_bg: true
            draw_bg +: { color: mod.look.agent_wash }
            name_caption := Mono{
                draw_text +: { color: mod.look.meta text_style: mod.look.mono{font_size: 7.5} }
            }
            name_input := Field{
                width: Fill
                height: Fit
            }
        }

        // Delete asks first.
        confirm_bar := View{
            visible: false
            width: Fill
            height: Fit
            flow: Down
            spacing: 4
            padding: Inset{left: 8 right: 6 top: 6 bottom: 6}
            show_bg: true
            draw_bg +: { color: mod.look.human_wash }
            confirm_text := Mono{
                width: Fill
                draw_text +: { color: mod.look.text_hi }
            }
            View{
                width: Fill
                height: Fit
                flow: Right
                spacing: 6
                confirm_yes := KeyBtn{ text: "Delete  enter" draw_bg +: { ink: mod.look.bad } }
                confirm_no := KeyBtn{ text: "Cancel  esc" }
            }
        }

        note := Mono{
            visible: false
            width: Fill
            padding: Inset{left: 8 right: 6 top: 3 bottom: 3}
            draw_text +: { color: mod.look.bad }
        }

        tree_wrap := View{
            width: Fill
            height: Fill
            tree_list := PortalList{
                width: Fill
                height: Fill
                FileRow := View{
                    width: Fill
                    height: 20
                    file_head := RowHead{
                        file_mark := Mono{
                            width: 12
                            draw_text +: { color: mod.look.meta text_style: mod.look.mono{font_size: 8.0} }
                        }
                        file_name := Mono{
                            max_lines: 1
                            text_overflow: Ellipsis
                            width: Fill
                        }
                    }
                }
            }
        }

        hit_wrap := View{
            visible: false
            width: Fill
            height: Fill
            hit_list := PortalList{
                width: Fill
                height: Fill
                HitRow := View{
                    width: Fill
                    height: 20
                    hit_head := RowHead{
                        padding: Inset{left: 20 right: 4}
                        hit_text := mod.widgets.CodeHitText{}
                    }
                }
            }
        }

        hit_empty := Mono{
            visible: false
            width: Fill
            padding: Inset{left: 20 top: 6}
            draw_text +: { color: mod.look.dim }
        }

        // Right-click menu: rows at the pointer over a clear scrim.
        menu := Modal{
            align: Align{x: 0.0 y: 0.0}
            bg_view +: { draw_bg.color: #0000 }
            content +: {
                menu_box := View{
                    width: 200
                    height: Fit
                    flow: Down
                    padding: Inset{left: 4 right: 4 top: 4 bottom: 4}
                    show_bg: true
                    draw_bg +: {
                        color: mod.look.rail
                        pixel: fn() {
                            let p = self.pos * self.rect_size
                            let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                            let c0 = self.rect_size * 0.5
                            let d = Finish.sd_chamfer(p, c0, c0 - vec2(0.5, 0.5), 6.0)
                            let inside = Finish.cover(d, px)
                            let edge = Finish.band(d, 0.0, px, px)
                            var c = vec4(self.color.rgb, 1.0) * inside
                            c = mix(c, vec4(mod.look.agent.rgb, 1.0), edge * 0.6)
                            return vec4(c.rgb * c.a, c.a)
                        }
                    }
                    m_open := MenuRow{ menu_label.text: "Open" menu_key.text: "enter" }
                    m_new_file := MenuRow{ menu_label.text: "New file" menu_key.text: "a" }
                    m_new_folder := MenuRow{ menu_label.text: "New folder" }
                    m_rename := MenuRow{ menu_label.text: "Rename" menu_key.text: "F2" }
                    m_delete := MenuRow{ menu_label.text: "Delete" menu_key.text: "del" }
                    m_copy := MenuRow{ menu_label.text: "Copy path" }
                    m_chat := MenuRow{ menu_label.text: "Add to chat" }
                }
            }
        }
    }
}

/// The menu's rows, in order, and what each does.
fn menu() -> [(&'static [LiveId; 1], MenuCmd); 7] {
    [
        (ids!(m_open), MenuCmd::Open),
        (ids!(m_new_file), MenuCmd::NewFile),
        (ids!(m_new_folder), MenuCmd::NewFolder),
        (ids!(m_rename), MenuCmd::Rename),
        (ids!(m_delete), MenuCmd::Delete),
        (ids!(m_copy), MenuCmd::CopyPath),
        (ids!(m_chat), MenuCmd::AddToChat),
    ]
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum MenuCmd {
    Open,
    NewFile,
    NewFolder,
    Rename,
    Delete,
    CopyPath,
    AddToChat,
}

/// What the panel is asking for, if anything.
#[derive(Clone, Debug, Default, PartialEq)]
enum Edit {
    #[default]
    None,
    /// A new file (or folder) in this folder ("" is the root).
    New { dir: String, folder: bool },
    Rename { from: String },
    ConfirmDelete { path: String },
}

/// A press on a row, until it lifts or becomes a drag.
#[derive(Clone, Copy)]
struct Press {
    index: usize,
    start: Vec2d,
    dragged: bool,
}

/// How many ranked files the filter lists.
const FILTER_LIMIT: usize = 400;

#[derive(Script, ScriptHook, Widget)]
pub struct FilesPanel {
    #[deref]
    view: View,
    #[rust]
    root: PathBuf,
    #[rust]
    tree: Option<FileTree>,
    /// The selected row's path (rows move when folders open).
    #[rust]
    sel: Option<String>,
    #[rust]
    query: String,
    #[rust]
    matches: Vec<FileMatch>,
    #[rust]
    msel: usize,
    #[rust]
    index_version: u64,
    #[rust]
    edit: Edit,
    #[rust]
    press: Option<Press>,
    #[rust]
    menu_target: Option<String>,
    #[rust]
    menu_hover: Option<usize>,
    #[rust]
    note: String,
    #[rust]
    want_filter_focus: bool,
    #[rust]
    want_name_focus: bool,
    #[rust]
    want_tree_focus: bool,
}

impl FilesPanel {
    /// Open the folders above `rel`, select it and scroll it into view.
    pub fn reveal(&mut self, cx: &mut Cx, rel: &str) {
        if !self.ensure_tree(cx) {
            return;
        }
        let Some(i) = self.tree.as_mut().and_then(|t| t.reveal(rel)) else { return };
        self.sel = Some(rel.to_string());
        self.view.portal_list(cx, ids!(tree_list)).keep_index_visible(cx, i);
        self.view.redraw(cx);
    }

    pub fn focus_filter(&mut self, cx: &mut Cx) {
        self.want_filter_focus = true;
        self.view.text_input(cx, ids!(filter)).set_key_focus(cx);
        self.view.redraw(cx);
    }

    fn ensure_tree(&mut self, cx: &Cx) -> bool {
        if self.tree.is_none() {
            let Some(ws) = Workspace::of(cx) else { return false };
            self.root = ws.root.clone();
            self.index_version = ws.index.version();
            self.tree = Some(FileTree::new(&ws.root));
        }
        true
    }

    fn nodes_len(&self) -> usize {
        self.tree.as_ref().map_or(0, |t| t.nodes().len())
    }

    fn sel_index(&self) -> Option<usize> {
        let tree = self.tree.as_ref()?;
        tree.position(self.sel.as_deref()?)
    }

    fn select_index(&mut self, cx: &mut Cx, i: usize) {
        let Some(node) = self.tree.as_ref().and_then(|t| t.nodes().get(i)) else { return };
        self.sel = Some(node.path.clone());
        self.view.portal_list(cx, ids!(tree_list)).keep_index_visible(cx, i);
        self.view.redraw(cx);
    }

    fn filtering(&self) -> bool {
        !self.query.trim().is_empty()
    }

    fn emit(&self, cx: &mut Cx, action: IdeAction) {
        cx.widget_action(self.view.widget_uid(), action);
    }

    fn open_path(&mut self, cx: &mut Cx, path: &str) {
        self.emit(cx, IdeAction::OpenFile { path: path.to_string(), line: None, col: None });
    }

    /// Enter on a row (or a click): open a file, fold or unfold a folder.
    fn activate(&mut self, cx: &mut Cx, i: usize) {
        let Some(node) = self.tree.as_ref().and_then(|t| t.nodes().get(i)).cloned() else { return };
        self.sel = Some(node.path.clone());
        if node.dir {
            if let Some(t) = self.tree.as_mut() {
                t.toggle(i);
            }
        } else {
            self.open_path(cx, &node.path);
        }
        self.view.redraw(cx);
    }

    fn focus_tree(&mut self, cx: &mut Cx) {
        cx.set_key_focus(self.view.area());
        self.want_tree_focus = true;
    }

    fn tree_has_focus(&self, cx: &Cx) -> bool {
        let area = self.view.area();
        !area.is_empty() && cx.has_key_focus(area)
    }

    // ---- the filter ------------------------------------------------------

    fn set_query(&mut self, cx: &mut Cx, text: &str) {
        self.query = text.to_string();
        self.msel = 0;
        self.rank(cx);
        self.view.portal_list(cx, ids!(hit_list)).set_first_id_and_scroll(0, 0.0);
        self.view.redraw(cx);
    }

    fn rank(&mut self, cx: &Cx) {
        self.matches = if self.filtering() {
            Workspace::of(cx).map(|ws| ws.index.search(&self.query, FILTER_LIMIT)).unwrap_or_default()
        } else {
            Vec::new()
        };
        self.msel = self.msel.min(self.matches.len().saturating_sub(1));
    }

    fn filter_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let input = self.view.text_input(cx, ids!(filter));
        if let Some(text) = input.changed(actions) {
            if text != self.query {
                self.set_query(cx, &text);
            }
        }
        if input.escaped(actions) {
            input.set_text(cx, "");
            self.set_query(cx, "");
            self.focus_tree(cx);
        }
        if let Some(key) = input.key_down_unhandled(actions) {
            if self.filtering() {
                let last = self.matches.len().saturating_sub(1);
                match key.key_code {
                    KeyCode::ArrowDown => self.msel = (self.msel + 1).min(last),
                    KeyCode::ArrowUp => self.msel = self.msel.saturating_sub(1),
                    KeyCode::PageDown => self.msel = (self.msel + 10).min(last),
                    KeyCode::PageUp => self.msel = self.msel.saturating_sub(10),
                    _ => {}
                }
                self.view.portal_list(cx, ids!(hit_list)).keep_index_visible(cx, self.msel);
                self.view.redraw(cx);
            } else if key.key_code == KeyCode::ArrowDown {
                if self.sel_index().is_none() && self.nodes_len() > 0 {
                    self.select_index(cx, 0);
                }
                self.focus_tree(cx);
            }
        }
        if input.returned(actions).is_some() && self.filtering() {
            if let Some(m) = self.matches.get(self.msel) {
                let path = m.path.clone();
                self.open_path(cx, &path);
            }
        }
    }

    // ---- the keyboard on the tree -----------------------------------------

    fn tree_key(&mut self, cx: &mut Cx, ke: &KeyEvent) {
        let m = ke.modifiers;
        if self.edit != Edit::None {
            if let Edit::ConfirmDelete { .. } = self.edit {
                match ke.key_code {
                    KeyCode::ReturnKey | KeyCode::KeyY => self.commit_delete(cx),
                    KeyCode::Escape | KeyCode::KeyN => self.cancel_edit(cx),
                    _ => {}
                }
            }
            return;
        }
        let len = self.nodes_len();
        if len == 0 {
            return;
        }
        let cur = self.sel_index();
        match ke.key_code {
            KeyCode::ArrowDown if !m.any() => {
                let i = cur.map_or(0, |i| (i + 1).min(len - 1));
                self.select_index(cx, i);
            }
            KeyCode::ArrowUp if !m.any() => {
                let i = cur.map_or(0, |i| i.saturating_sub(1));
                self.select_index(cx, i);
            }
            KeyCode::PageDown => {
                let i = cur.map_or(0, |i| (i + 20).min(len - 1));
                self.select_index(cx, i);
            }
            KeyCode::PageUp => {
                let i = cur.map_or(0, |i| i.saturating_sub(20));
                self.select_index(cx, i);
            }
            KeyCode::Home => self.select_index(cx, 0),
            KeyCode::End => self.select_index(cx, len - 1),
            KeyCode::ArrowLeft => {
                let Some(i) = cur else { return };
                let tree = self.tree.as_mut().unwrap();
                let node = &tree.nodes()[i];
                if node.dir && node.open {
                    tree.toggle(i);
                    self.view.redraw(cx);
                } else if let Some(p) = tree.parent(i) {
                    self.select_index(cx, p);
                }
            }
            KeyCode::ArrowRight => {
                let Some(i) = cur else { return };
                let tree = self.tree.as_mut().unwrap();
                let node = &tree.nodes()[i];
                if node.dir && !node.open {
                    tree.toggle(i);
                    self.view.redraw(cx);
                } else if node.dir && i + 1 < tree.nodes().len() && tree.nodes()[i + 1].depth > node.depth {
                    self.select_index(cx, i + 1);
                }
            }
            KeyCode::ReturnKey => {
                if let Some(i) = cur {
                    self.activate(cx, i);
                }
            }
            KeyCode::F2 => self.start_rename(cx),
            KeyCode::Delete => self.start_delete(cx),
            KeyCode::Backspace if m.is_primary() => self.start_delete(cx),
            KeyCode::KeyA if !m.any() => self.start_new(cx, false),
            KeyCode::KeyN if m.is_primary() && !m.shift && !m.alt => self.start_new(cx, false),
            KeyCode::KeyN if m.is_primary() && m.shift => self.start_new(cx, true),
            KeyCode::KeyF if m.is_primary() && !m.shift => self.focus_filter(cx),
            _ => {}
        }
    }

    // ---- new / rename / delete ---------------------------------------------

    /// The folder new things go in: the selected folder, else the selected
    /// file's folder, else the root.
    fn target_dir(&self) -> String {
        let Some(i) = self.sel_index() else { return String::new() };
        let node = &self.tree.as_ref().unwrap().nodes()[i];
        if node.dir {
            node.path.clone()
        } else {
            parent_of(&node.path).to_string()
        }
    }

    fn start_new(&mut self, cx: &mut Cx, folder: bool) {
        let dir = self.target_dir();
        // Show the folder's contents while naming the new entry.
        if let (Some(i), Some(tree)) = (self.sel_index(), self.tree.as_mut()) {
            if tree.nodes()[i].dir && !tree.nodes()[i].open {
                tree.toggle(i);
            }
        }
        let shown = if dir.is_empty() { "/".to_string() } else { format!("{dir}/") };
        let caption = if folder { format!("NEW FOLDER IN {shown}") } else { format!("NEW FILE IN {shown}  (name/ = folder)") };
        self.edit = Edit::New { dir, folder };
        self.begin_name(cx, &caption, "");
    }

    fn start_rename(&mut self, cx: &mut Cx) {
        let Some(path) = self.sel.clone() else { return };
        let name = path.rsplit('/').next().unwrap_or(&path).to_string();
        self.edit = Edit::Rename { from: path.clone() };
        self.begin_name(cx, &format!("RENAME {path}"), &name);
    }

    fn begin_name(&mut self, cx: &mut Cx, caption: &str, text: &str) {
        self.note.clear();
        self.view.label(cx, ids!(name_caption)).set_text(cx, caption);
        let input = self.view.text_input(cx, ids!(name_input));
        input.set_text(cx, text);
        if let Some(mut inner) = input.borrow_mut() {
            inner.select_all(cx);
        }
        self.want_name_focus = true;
        self.view.redraw(cx);
    }

    fn start_delete(&mut self, cx: &mut Cx) {
        let Some(path) = self.sel.clone() else { return };
        self.note.clear();
        let dir = self.tree.as_ref().and_then(|t| t.nodes().get(self.sel_index()?)).is_some_and(|n| n.dir);
        let what = if dir { format!("Delete {path}/ and everything in it?") } else { format!("Delete {path}?") };
        self.view.label(cx, ids!(confirm_text)).set_text(cx, &what);
        self.edit = Edit::ConfirmDelete { path };
        self.focus_tree(cx);
        self.view.redraw(cx);
    }

    fn cancel_edit(&mut self, cx: &mut Cx) {
        self.edit = Edit::None;
        self.focus_tree(cx);
        self.view.redraw(cx);
    }

    fn fail(&mut self, cx: &mut Cx, msg: String) {
        self.note = msg;
        self.view.redraw(cx);
    }

    fn commit_name(&mut self, cx: &mut Cx, text: &str) {
        let name = text.trim();
        let edit = std::mem::take(&mut self.edit);
        if name.is_empty() {
            self.cancel_edit(cx);
            return;
        }
        if !safe_relative(name) {
            self.edit = edit;
            self.fail(cx, format!("not a name inside the project: {name}"));
            return;
        }
        match edit {
            Edit::New { dir, folder } => {
                let folder = folder || name.ends_with('/');
                let rel = join(&dir, name.trim_end_matches('/'));
                let abs = self.root.join(&rel);
                let made = if folder {
                    if abs.exists() { Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "already exists")) } else { std::fs::create_dir_all(&abs) }
                } else {
                    abs.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::OpenOptions::new().write(true).create_new(true).open(&abs).map(|_| ()))
                };
                if let Err(e) = made {
                    self.edit = Edit::New { dir, folder };
                    self.fail(cx, format!("cannot create {rel}: {e}"));
                    return;
                }
                self.after_change(cx, &rel);
                self.emit(cx, IdeAction::Created(rel.clone()));
                if !folder {
                    self.open_path(cx, &rel);
                }
            }
            Edit::Rename { from } => {
                let to = join(parent_of(&from), name.trim_end_matches('/'));
                if to == from {
                    self.cancel_edit(cx);
                    return;
                }
                let (a, b) = (self.root.join(&from), self.root.join(&to));
                let moved = if b.exists() {
                    Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "already exists"))
                } else {
                    b.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::rename(&a, &b))
                };
                if let Err(e) = moved {
                    self.edit = Edit::Rename { from: from.clone() };
                    self.fail(cx, format!("cannot rename to {to}: {e}"));
                    return;
                }
                self.after_change(cx, &to);
                self.emit(cx, IdeAction::Renamed { from, to });
            }
            Edit::None | Edit::ConfirmDelete { .. } => {}
        }
        self.focus_tree(cx);
    }

    fn commit_delete(&mut self, cx: &mut Cx) {
        let Edit::ConfirmDelete { path } = std::mem::take(&mut self.edit) else { return };
        let abs = self.root.join(&path);
        let at = self.sel_index();
        let removed = if abs.is_dir() { std::fs::remove_dir_all(&abs) } else { std::fs::remove_file(&abs) };
        if let Err(e) = removed {
            self.fail(cx, format!("cannot delete {path}: {e}"));
            return;
        }
        if let Some(tree) = self.tree.as_mut() {
            tree.refresh();
        }
        self.sel = None;
        if let Some(i) = at {
            let len = self.nodes_len();
            if len > 0 {
                self.select_index(cx, i.min(len - 1));
            }
        }
        self.emit(cx, IdeAction::Deleted(path));
        self.focus_tree(cx);
        self.view.redraw(cx);
    }

    /// After a create or rename: list again and select the new path.
    fn after_change(&mut self, cx: &mut Cx, rel: &str) {
        self.note.clear();
        if let Some(tree) = self.tree.as_mut() {
            tree.refresh();
        }
        self.reveal(cx, rel);
    }

    fn name_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if !matches!(self.edit, Edit::New { .. } | Edit::Rename { .. }) {
            return;
        }
        let input = self.view.text_input(cx, ids!(name_input));
        if let Some((text, _)) = input.returned(actions) {
            self.commit_name(cx, &text);
        } else if input.escaped(actions) {
            self.cancel_edit(cx);
        }
    }

    // ---- the menu ----------------------------------------------------------

    fn open_menu(&mut self, cx: &mut Cx, i: usize, at: Vec2d) {
        let Some(node) = self.tree.as_ref().and_then(|t| t.nodes().get(i)).cloned() else { return };
        self.sel = Some(node.path.clone());
        self.menu_target = Some(node.path);
        self.menu_hover = None;
        let mut bx = self.view.view(cx, ids!(menu_box));
        let (x, y) = (at.x + 2.0, at.y + 2.0);
        script_apply_eval!(cx, bx, { margin: mod.turtle.Inset{left: #(x) top: #(y) right: 0.0 bottom: 0.0} });
        self.view.modal(cx, ids!(menu)).open(cx);
        self.view.redraw(cx);
    }

    fn menu_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let modal = self.view.modal(cx, ids!(menu));
        if !modal.is_open() {
            return;
        }
        for (slot, (path, cmd)) in menu().iter().enumerate() {
            let row = self.view.view(cx, *path);
            if row.finger_hover_in(actions).is_some() {
                self.menu_hover = Some(slot);
                self.view.redraw(cx);
            }
            if row.finger_up(actions).is_some_and(|f| f.is_over && f.device.is_primary_hit()) {
                modal.close(cx);
                self.run_menu(cx, *cmd);
                return;
            }
        }
    }

    fn run_menu(&mut self, cx: &mut Cx, cmd: MenuCmd) {
        let Some(path) = self.menu_target.take() else { return };
        self.sel = Some(path.clone());
        let dir = self.tree.as_ref().and_then(|t| t.nodes().get(t.position(&path)?)).is_some_and(|n| n.dir);
        match cmd {
            MenuCmd::Open => {
                if let Some(i) = self.sel_index() {
                    self.activate(cx, i);
                }
            }
            MenuCmd::NewFile => self.start_new(cx, false),
            MenuCmd::NewFolder => self.start_new(cx, true),
            MenuCmd::Rename => self.start_rename(cx),
            MenuCmd::Delete => self.start_delete(cx),
            MenuCmd::CopyPath => cx.copy_to_clipboard(&path),
            MenuCmd::AddToChat => {
                if !dir {
                    self.emit(cx, IdeAction::AddReference(path));
                }
            }
        }
        self.view.redraw(cx);
    }

    // ---- rows: press, drag, click, right-click -----------------------------

    fn row_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let list = self.view.portal_list(cx, ids!(tree_list));
        for (index, item) in list.items_with_actions(actions) {
            let head = item.view(cx, ids!(file_head));
            if let Some(fd) = head.finger_down(actions) {
                if fd.device.mouse_button().is_some_and(|b| b.is_secondary()) {
                    self.open_menu(cx, index, fd.abs);
                    continue;
                }
                if fd.device.is_primary_hit() {
                    self.press = Some(Press { index, start: fd.abs, dragged: false });
                    self.sel = self.tree.as_ref().and_then(|t| t.nodes().get(index)).map(|n| n.path.clone());
                    self.focus_tree(cx);
                    self.view.redraw(cx);
                }
            }
            if let Some(fm) = head.finger_move(actions) {
                if let Some(p) = self.press.as_mut().filter(|p| p.index == index && !p.dragged) {
                    if (fm.abs - p.start).length() > 4.0 {
                        p.dragged = true;
                        if let Some(node) = self.tree.as_ref().and_then(|t| t.nodes().get(index)) {
                            let abs = self.root.join(&node.path).to_string_lossy().to_string();
                            cx.start_dragging(vec![DragItem::FilePath { path: abs, internal_id: None }]);
                        }
                    }
                }
            }
            if let Some(fu) = head.finger_up(actions) {
                let press = self.press.take();
                if fu.is_over && fu.device.is_primary_hit() && press.is_some_and(|p| p.index == index && !p.dragged) {
                    self.activate(cx, index);
                }
            }
        }
        let hits = self.view.portal_list(cx, ids!(hit_list));
        for (index, item) in hits.items_with_actions(actions) {
            let head = item.view(cx, ids!(hit_head));
            if let Some(fd) = head.finger_down(actions) {
                if fd.device.is_primary_hit() {
                    self.msel = index;
                    self.press = Some(Press { index, start: fd.abs, dragged: false });
                    self.view.redraw(cx);
                }
            }
            if let Some(fm) = head.finger_move(actions) {
                if let Some(p) = self.press.as_mut().filter(|p| p.index == index && !p.dragged) {
                    if (fm.abs - p.start).length() > 4.0 {
                        p.dragged = true;
                        if let Some(m) = self.matches.get(index) {
                            let abs = self.root.join(&m.path).to_string_lossy().to_string();
                            cx.start_dragging(vec![DragItem::FilePath { path: abs, internal_id: None }]);
                        }
                    }
                }
            }
            if let Some(fu) = head.finger_up(actions) {
                let press = self.press.take();
                if fu.is_over && fu.device.is_primary_hit() && press.is_some_and(|p| p.index == index && !p.dragged) {
                    if let Some(path) = self.matches.get(index).map(|m| m.path.clone()) {
                        self.open_path(cx, &path);
                    }
                }
            }
        }
    }

    // ---- drawing -----------------------------------------------------------

    fn draw_tree(&mut self, cx: &mut Cx2d, list: &mut PortalList) {
        let Some(tree) = self.tree.as_ref() else {
            list.set_item_range(cx, 0, 0);
            while list.next_visible_item(cx).is_some() {}
            return;
        };
        let p = crate::ui::theme::palette();
        let total = tree.nodes().len();
        list.set_item_range(cx, 0, total);
        let focused = self.tree_has_focus(cx);
        while let Some(index) = list.next_visible_item(cx) {
            let Some(node) = tree.nodes().get(index) else { continue };
            let row = list.item(cx, index, id!(FileRow));
            let selected = self.sel.as_deref() == Some(node.path.as_str());
            let mark = if node.dir {
                if node.open { "▼" } else { "▶" }
            } else {
                ""
            };
            let indent = 4.0 + node.depth as f64 * 12.0;
            let tint = if selected { vec(p.primary, if focused { 0.22 } else { 0.12 }) } else { vec(0, 0.0) };
            let mut head = row.view(cx, ids!(file_head));
            script_apply_eval!(cx, head, { padding: mod.turtle.Inset{left: #(indent) right: 4.0 top: 0.0 bottom: 0.0} draw_bg +: { color: #(tint) } });
            row.label(cx, ids!(file_mark)).set_text(cx, mark);
            let mut name = row.label(cx, ids!(file_name));
            name.set_text(cx, &node.name);
            let ink = if selected { vec(p.text_hi, 1.0) } else if node.dir { vec(p.text, 1.0) } else { vec(p.meta, 1.0) };
            script_apply_eval!(cx, name, { draw_text +: { color: #(ink) } });
            row.draw_all(cx, &mut Scope::empty());
        }
    }

    fn draw_hits(&mut self, cx: &mut Cx2d, list: &mut PortalList) {
        let p = crate::ui::theme::palette();
        list.set_item_range(cx, 0, self.matches.len());
        while let Some(index) = list.next_visible_item(cx) {
            let Some(m) = self.matches.get(index) else { continue };
            let row = list.item(cx, index, id!(HitRow));
            let selected = index == self.msel;
            let tint = if selected { vec(p.primary, 0.22) } else { vec(0, 0.0) };
            let mut head = row.view(cx, ids!(hit_head));
            script_apply_eval!(cx, head, { draw_bg +: { color: #(tint) } });
            let runs = name_then_dir(&m.path, &m.hits, vec(p.text_hi, 1.0), vec(p.meta, 1.0), vec(p.primary, 1.0));
            if let Some(mut text) = row.widget(cx, ids!(hit_text)).borrow_mut::<HitText>() {
                text.set_runs(runs);
            }
            row.draw_all(cx, &mut Scope::empty());
        }
    }

    fn draw_bars(&mut self, cx: &mut Cx2d) {
        let naming = matches!(self.edit, Edit::New { .. } | Edit::Rename { .. });
        self.view.view(cx, ids!(name_bar)).set_visible(cx, naming);
        self.view.view(cx, ids!(confirm_bar)).set_visible(cx, matches!(self.edit, Edit::ConfirmDelete { .. }));
        let note = self.view.label(cx, ids!(note));
        note.set_visible(cx, !self.note.is_empty());
        note.set_text(cx, &self.note);
        let filtering = self.filtering();
        self.view.view(cx, ids!(tree_wrap)).set_visible(cx, !filtering);
        self.view.view(cx, ids!(hit_wrap)).set_visible(cx, filtering);
        let empty = self.view.label(cx, ids!(hit_empty));
        let no_hits = filtering && self.matches.is_empty();
        empty.set_visible(cx, no_hits);
        if no_hits {
            let ready = Workspace::of(cx).is_some_and(|ws| ws.index.ready());
            empty.set_text(cx, if ready { "no file matches" } else { "indexing files…" });
        }
        let p = crate::ui::theme::palette();
        for (slot, (path, _)) in menu().iter().enumerate() {
            let mut row = self.view.view(cx, *path);
            let tint = if self.menu_hover == Some(slot) { vec(p.primary, 0.22) } else { vec(0, 0.0) };
            script_apply_eval!(cx, row, { draw_bg +: { color: #(tint) } });
        }
    }
}

fn vec(rgb: u32, alpha: f32) -> Vec4f {
    crate::ui::theme::vec(rgb, alpha)
}

fn parent_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

/// A name typed into the tree stays inside the project.
fn safe_relative(name: &str) -> bool {
    !name.starts_with('/') && !name.contains('\\') && name.split('/').all(|part| part != ".." && part != ".")
}

/// Runs of `text[from..to]`, `ink` with the bytes in `hits` (offsets into
/// `text`) in `lit`; whole characters, so a hit inside one lights it.
pub fn lit_runs(text: &str, from: usize, to: usize, hits: &[usize], ink: Vec4f, lit: Vec4f, out: &mut Vec<(String, Vec4f)>) {
    let mut cur = String::new();
    let mut cur_lit = false;
    for (i, ch) in text[from..to].char_indices() {
        let at = from + i;
        let on = hits.iter().any(|&h| h >= at && h < at + ch.len_utf8());
        if on != cur_lit && !cur.is_empty() {
            out.push((std::mem::take(&mut cur), if cur_lit { lit } else { ink }));
        }
        cur_lit = on;
        cur.push(ch);
    }
    if !cur.is_empty() {
        out.push((cur, if cur_lit { lit } else { ink }));
    }
}

/// A path drawn as its file name, then its folder dimmer, matches lit.
pub fn name_then_dir(path: &str, hits: &[usize], name_ink: Vec4f, dir_ink: Vec4f, lit: Vec4f) -> Vec<(String, Vec4f)> {
    let mut runs = Vec::new();
    let split = path.rfind('/').map_or(0, |p| p + 1);
    lit_runs(path, split, path.len(), hits, name_ink, lit, &mut runs);
    if split > 0 {
        runs.push(("  ".to_string(), dir_ink));
        lit_runs(path, 0, split - 1, hits, dir_ink, lit, &mut runs);
    }
    runs
}

impl Widget for FilesPanel {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.ensure_tree(cx);
        if let Event::Signal = event {
            let v = Workspace::of(cx).map_or(0, |ws| ws.index.version());
            if v != self.index_version {
                self.index_version = v;
                if let Some(tree) = self.tree.as_mut() {
                    tree.refresh();
                }
                if self.filtering() {
                    self.rank(cx);
                }
                self.view.redraw(cx);
            }
        }
        if let Event::KeyDown(ke) = event {
            if self.tree_has_focus(cx) {
                self.tree_key(cx, ke);
            }
        }
        let actions = cx.capture_actions(|cx| self.view.handle_event(cx, event, scope));
        if actions.is_empty() {
            return;
        }
        self.filter_actions(cx, &actions);
        self.name_actions(cx, &actions);
        self.menu_actions(cx, &actions);
        self.row_actions(cx, &actions);
        if self.view.button(cx, ids!(confirm_yes)).clicked(&actions) {
            self.commit_delete(cx);
        }
        if self.view.button(cx, ids!(confirm_no)).clicked(&actions) {
            self.cancel_edit(cx);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ensure_tree(cx);
        self.draw_bars(cx);
        let tree_uid = self.view.portal_list(cx, ids!(tree_list)).widget_uid();
        let hit_uid = self.view.portal_list(cx, ids!(hit_list)).widget_uid();
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            let uid = item.widget_uid();
            if let Some(mut list) = item.borrow_mut::<PortalList>() {
                if uid == tree_uid {
                    self.draw_tree(cx, &mut list);
                } else if uid == hit_uid {
                    self.draw_hits(cx, &mut list);
                }
            }
        }
        if std::mem::take(&mut self.want_filter_focus) {
            self.view.text_input(cx, ids!(filter)).set_key_focus(cx);
        }
        if std::mem::take(&mut self.want_name_focus) {
            self.view.text_input(cx, ids!(name_input)).set_key_focus(cx);
        }
        if std::mem::take(&mut self.want_tree_focus) {
            cx.set_key_focus(self.view.area());
        }
        DrawStep::done()
    }
}

// ---- the lit text ----------------------------------------------------------

#[derive(Script, ScriptHook, Widget)]
pub struct HitText {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_text: DrawText,
    #[rust]
    runs: Vec<(String, Vec4f)>,
    /// One character's advance and the line's height (the face is mono).
    #[rust]
    metrics: Option<(f64, f64)>,
}

impl HitText {
    pub fn set_runs(&mut self, runs: Vec<(String, Vec4f)>) {
        self.runs = runs;
    }
}

impl Widget for HitText {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        let (cw, h) = *self.metrics.get_or_insert_with(|| {
            let laid = self.draw_text.layout(cx, 0.0, 0.0, None, false, Align::default(), "0000000000");
            (laid.size_in_lpxs.width as f64 / 10.0, laid.size_in_lpxs.height as f64)
        });
        let max = if cw > 0.0 { (rect.size.x / cw).floor() as usize } else { usize::MAX };
        let total: usize = self.runs.iter().map(|(t, _)| t.chars().count()).sum();
        let y = (rect.pos.y + (rect.size.y - h) * 0.5).round();
        let mut x = rect.pos.x;
        let mut left = if total > max { max.saturating_sub(1) } else { total };
        for (text, color) in &self.runs {
            if left == 0 {
                break;
            }
            let n = text.chars().count();
            let shown: String = if n > left { text.chars().take(left).collect() } else { text.clone() };
            let used = n.min(left);
            self.draw_text.color = *color;
            self.draw_text.draw_abs(cx, dvec2(x.round(), y), &shown);
            x += used as f64 * cw;
            left -= used;
        }
        if total > max && max > 0 {
            if let Some((_, color)) = self.runs.last() {
                self.draw_text.color = *color;
            }
            self.draw_text.draw_abs(cx, dvec2(x.round(), y), "…");
        }
        DrawStep::done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_light_the_hits_and_put_the_name_first() {
        let (a, b, l) = (vec(0x111111, 1.0), vec(0x222222, 1.0), vec(0x333333, 1.0));
        let path = "src/ide/shell.rs";
        let hits = [8, 9, 10];
        let runs = name_then_dir(path, &hits, a, b, l);
        let texts: Vec<&str> = runs.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(texts, ["she", "ll.rs", "  ", "src/ide"]);
        assert_eq!(runs[0].1, l);
        assert_eq!(runs[1].1, a);
        assert_eq!(runs[3].1, b);
    }

    #[test]
    fn names_stay_inside_the_project() {
        assert!(safe_relative("a/b.rs"));
        assert!(safe_relative("dir/"));
        assert!(!safe_relative("../x"));
        assert!(!safe_relative("/etc/x"));
        assert!(!safe_relative("a/./b"));
        assert_eq!(join("", "a"), "a");
        assert_eq!(join("x/y", "a"), "x/y/a");
        assert_eq!(parent_of("x/y/a"), "x/y");
        assert_eq!(parent_of("a"), "");
    }
}
