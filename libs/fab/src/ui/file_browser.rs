//! Lane D. The in-app file browser (Fab ships its own for the same
//! reason): `platform/` exposes a *folder* picker only
//! (`cx_api.rs:1874`; `open_select_file_dialog` is a stub), and `platform/` is
//! off-limits to this project — so this browser *is* the open dialog.
//!
//! It is a real browser: bookmarks and the recent list on the left, a live
//! directory listing on the right, a path
//! field you can type into, and `..` to go up. Clicking a folder descends,
//! clicking a file opens it. Drag-and-drop onto the window (`shell.rs`) and
//! `--open PATH` reach the same `ShellAction::OpenFile`.

use crate::api::*;
use makepad_widgets::*;
use std::path::{Path, PathBuf};

/// One listed entry.
#[derive(Clone)]
struct Entry {
    path: PathBuf,
    label: String,
    is_dir: bool,
}

const RECENT_SLOTS: usize = 6;

fn read_dir(dir: &Path) -> Vec<Entry> {
    let mut dirs: Vec<Entry> = Vec::new();
    let mut files: Vec<Entry> = Vec::new();
    if let Some(parent) = dir.parent() {
        dirs.push(Entry {
            path: parent.to_path_buf(),
            label: "..".to_string(),
            is_dir: true,
        });
    }
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                dirs.push(Entry {
                    path: p,
                    label: name,
                    is_dir: true,
                });
            } else {
                let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                files.push(Entry {
                    path: p,
                    label: format!("{name}   ({:.1} MB)", size as f64 / 1e6),
                    is_dir: false,
                });
            }
        }
    }
    dirs.sort_by(|a, b| a.label.cmp(&b.label));
    files.sort_by(|a, b| a.label.cmp(&b.label));
    dirs.extend(files);
    dirs
}

fn default_dir() -> PathBuf {
    for c in ["local/models", "local", "."] {
        let p = PathBuf::from(c);
        if p.is_dir() {
            return p;
        }
    }
    PathBuf::from(".")
}

script_mod! {
    use mod.prelude.fab.*
    use mod.widgets.*

    let SideButton = mod.widgets.FabFlatButton{
        width: Fill
        align: Align{x: 0.0 y: 0.5}
        visible: false
        text: ""
    }

    // One row body, two templates: `PortalList` keys its recycling pool by
    // template, and a template cannot derive from a sibling template, so the
    // shared shape lives in a `let` binding.
    let FileRow = View{
        width: Fill
        height: fab.row_height
        flow: Right
        align: Align{x: 0.0 y: 0.5}
        padding: Inset{left: 6 right: 6 top: 0 bottom: 0}
        spacing: 6
        cursor: MouseCursor.Hand
        show_bg: true
        draw_bg +: {
            hover: instance(0.0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(1.0, 0.0, self.rect_size.x - 2.0, self.rect_size.y, fab.radius)
                sdf.fill(vec4(fab.color_menu_row_hover.xyz, self.hover * 0.7))
                return sdf.result
            }
        }
        animator: Animator{
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: fab.anim_fast}}
                    apply: { draw_bg: {hover: 0.0} }
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: { draw_bg: {hover: 1.0} }
                }
            }
        }
        ico := mod.widgets.FabIconSmall{
            width: 14
            height: 14
            icon_walk: Walk{ width: 14 height: 14 }
            draw_icon +: { svg: crate_resource("self://resources/icons/folder.svg") }
        }
        name := mod.widgets.FabLabel{ width: Fill text: "" }
    }

    mod.widgets.FabFileBrowserBase = #(FabFileBrowser::register_widget(vm))
    mod.widgets.FabFileBrowser = set_type_default() do mod.widgets.FabFileBrowserBase{
        width: Fill
        height: Fill
        modal := Modal{
            content +: {
                width: 760
                height: Fit
                View{
                    width: Fill
                    height: Fit
                    flow: Down
                    padding: 12
                    spacing: 8
                    show_bg: true
                    draw_bg +: {
                        pixel: fn() {
                            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                            sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, fab.radius_lg)
                            sdf.fill_keep(fab.color_popover)
                            sdf.stroke(fab.color_popover_border, 1.0)
                            return sdf.result
                        }
                    }
                    head := View{
                        width: Fill
                        height: Fit
                        flow: Right
                        align: Align{x: 0.0 y: 0.5}
                        spacing: 8
                        mod.widgets.FabTitleLabel{ text: "Open Model" }
                        Filler{}
                        up := mod.widgets.FabButton{ text: "Up" }
                    }
                    path := TextInput{
                        width: Fill
                        height: 24
                        empty_text: "/path/to/model.glb"
                        draw_bg +: {
                            color: fab.color_input
                            border_radius: fab.radius
                        }
                        draw_text +: {
                            color: fab.color_text
                            ink_centered: true
                            text_style: theme.font_code{ font_size: fab.font_size_ui }
                        }
                    }
                    cols := View{
                        width: Fill
                        height: 300
                        flow: Right
                        spacing: 8
                        side := View{
                            width: 220
                            height: Fill
                            flow: Down
                            spacing: 2
                            padding: 6
                            show_bg: true
                            draw_bg +: {
                                pixel: fn() {
                                    let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                                    sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, fab.radius)
                                    sdf.fill_keep(fab.color_editor)
                                    sdf.stroke(fab.color_border, 1.0)
                                    return sdf.result
                                }
                            }
                            mod.widgets.FabLabelMuted{ text: "PLACES" }
                            bm_samples := SideButton{ visible: true text: "Models" }
                            bm_local := SideButton{ visible: true text: "local" }
                            bm_cwd := SideButton{ visible: true text: "Working folder" }
                            mod.widgets.FabLabelMuted{ margin: Inset{top: 6} text: "RECENT" }
                            rc_0 := SideButton{}
                            rc_1 := SideButton{}
                            rc_2 := SideButton{}
                            rc_3 := SideButton{}
                            rc_4 := SideButton{}
                            rc_5 := SideButton{}
                        }
                        listbox := View{
                            width: Fill
                            height: Fill
                            flow: Down
                            padding: 3
                            show_bg: true
                            draw_bg +: {
                                pixel: fn() {
                                    let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                                    sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, fab.radius)
                                    sdf.fill_keep(fab.color_editor)
                                    sdf.stroke(fab.color_border, 1.0)
                                    return sdf.result
                                }
                            }
                            list := PortalList{
                                width: Fill
                                height: Fill
                                flow: Down
                                auto_tail: false
                                RowDir := FileRow{
                                    ico +: {
                                        draw_icon +: { svg: crate_resource("self://resources/icons/folder.svg") }
                                    }
                                }
                                RowFile := FileRow{
                                    ico +: {
                                        draw_icon +: { svg: crate_resource("self://resources/icons/file.svg") }
                                    }
                                }
                            }
                        }
                    }
                    mod.widgets.FabHr{}
                    foot := View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0 y: 0.5}
                        mod.widgets.FabLabelMuted{ text: "Model files (*.glb, *.gltf)" }
                        open_demo := mod.widgets.FabButton{ text: "Open demo house" }
                        Filler{}
                        cancel := mod.widgets.FabButton{ text: "Cancel" }
                        open := mod.widgets.FabButtonAccent{ text: "Open" }
                    }
                }
            }
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct FabFileBrowser {
    #[deref]
    view: View,
    #[rust]
    shown: bool,
    #[rust]
    dir: Option<PathBuf>,
    #[rust]
    entries: Vec<Entry>,
    #[rust]
    selected: Option<PathBuf>,
}

impl FabFileBrowser {
    fn goto(&mut self, cx: &mut Cx, dir: PathBuf) {
        self.entries = read_dir(&dir);
        let shown = dir.display().to_string();
        self.dir = Some(dir);
        self.view
            .text_input(cx, ids!(modal.path))
            .set_text(cx, &shown);
        self.view.redraw(cx);
    }

    fn open(&mut self, cx: &mut Cx, path: PathBuf) {
        cx.action(ShellAction::ShowFileBrowser(false));
        cx.action(ShellAction::OpenFile(path));
    }

    fn recent_ids() -> [&'static [LiveId]; RECENT_SLOTS] {
        [
            ids!(modal.cols.side.rc_0),
            ids!(modal.cols.side.rc_1),
            ids!(modal.cols.side.rc_2),
            ids!(modal.cols.side.rc_3),
            ids!(modal.cols.side.rc_4),
            ids!(modal.cols.side.rc_5),
        ]
    }
}

impl Widget for FabFileBrowser {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        let Event::Actions(actions) = event else {
            return;
        };
        let modal = self.view.modal(cx, ids!(modal));
        if modal.dismissed(actions) || self.view.button(cx, ids!(modal.cancel)).clicked(actions) {
            cx.action(ShellAction::ShowFileBrowser(false));
            return;
        }
        if self.view.button(cx, ids!(modal.open_demo)).clicked(actions) {
            cx.action(ShellAction::ShowFileBrowser(false));
            cx.action(ShellAction::OpenDemo);
            return;
        }
        // Places.
        for (id, dir) in [
            (ids!(modal.cols.side.bm_samples), default_dir()),
            (ids!(modal.cols.side.bm_local), PathBuf::from("local")),
            (ids!(modal.cols.side.bm_cwd), PathBuf::from(".")),
        ] {
            if self.view.button(cx, id).clicked(actions) {
                self.goto(cx, dir);
                return;
            }
        }
        if self.view.button(cx, ids!(modal.head.up)).clicked(actions) {
            if let Some(p) = self.dir.clone().and_then(|d| d.parent().map(|p| p.to_path_buf())) {
                self.goto(cx, p);
            }
            return;
        }
        // Recent.
        if let Some(state) = scope.data.get_mut::<AppState>() {
            let recent = state.recent.clone();
            for (i, id) in Self::recent_ids().iter().enumerate() {
                if self.view.button(cx, id).clicked(actions) {
                    if let Some(p) = recent.get(i) {
                        self.open(cx, p.clone());
                        return;
                    }
                }
            }
        }
        // Typed path: a directory navigates, a file opens.
        let input = self.view.text_input(cx, ids!(modal.path));
        let typed = input.returned(actions).is_some()
            || self.view.button(cx, ids!(modal.open)).clicked(actions);
        if typed {
            let text = input.text();
            let p = PathBuf::from(text.trim());
            if p.is_dir() {
                self.goto(cx, p);
            } else if !text.trim().is_empty() {
                self.open(cx, p);
            } else if let Some(sel) = self.selected.clone() {
                self.open(cx, sel);
            }
            return;
        }
        // Rows.
        let list = self.view.portal_list(cx, ids!(modal.cols.listbox.list));
        let entries = self.entries.clone();
        for (i, e) in entries.iter().enumerate() {
            if let Some((_, item)) = list.get_item(i) {
                if item.as_view().finger_up(actions).is_some() {
                    if e.is_dir {
                        self.goto(cx, e.path.clone());
                    } else {
                        self.open(cx, e.path.clone());
                    }
                    return;
                }
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if let Some(state) = scope.data.get_mut::<AppState>() {
            let modal = self.view.modal(cx, ids!(modal));
            if state.ui.file_browser_open != self.shown {
                self.shown = state.ui.file_browser_open;
                if self.shown {
                    modal.open(cx);
                    if self.dir.is_none() {
                        let d = state
                            .recent
                            .first()
                            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
                            .unwrap_or_else(default_dir);
                        self.goto(cx, d);
                    }
                    let recent = state.recent.clone();
                    for (i, id) in Self::recent_ids().iter().enumerate() {
                        let b = self.view.button(cx, id);
                        match recent.get(i) {
                            Some(p) => {
                                let name = p
                                    .file_name()
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_else(|| p.display().to_string());
                                b.set_text(cx, &name);
                                b.set_visible(cx, true);
                            }
                            None => b.set_visible(cx, false),
                        }
                    }
                } else {
                    modal.close(cx);
                }
            }
        }
        let entries = self.entries.clone();
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            let list_ref = step.as_portal_list();
            let Some(mut list) = list_ref.borrow_mut() else {
                continue;
            };
            list.set_item_range(cx, 0, entries.len());
            while let Some(i) = list.next_visible_item(cx) {
                let Some(e) = entries.get(i) else { continue };
                let item = list.item(
                    cx,
                    i,
                    if e.is_dir {
                        live_id!(RowDir)
                    } else {
                        live_id!(RowFile)
                    },
                );
                item.label(cx, ids!(name)).set_text(cx, &e.label);
                item.draw_all(cx, &mut Scope::empty());
            }
        }
        DrawStep::done()
    }
}
