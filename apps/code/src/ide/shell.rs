//! The window's dock: the file tree and search on the left, editor tabs in
//! the middle with terminals under them, the code-chat on the right. The
//! shell routes `IdeAction`s from the panels and owns the window keys
//! (quick-open, project search, terminal).

use super::editor::{FileEditor, FileEditorAction};
use super::files::FilesPanel;
use super::quick_open::QuickOpen;
use super::run::RunPanel;
use super::sessions::SessionsPanel;
use super::search::SearchPanel;
use super::terminal::TerminalPanel;
use super::{IdeAction, Workspace};
use crate::ui::view::AgentView;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.sdf.Finish


    // ---- the dock's chrome in the Code look --------------------------------

    /// A tab: mono caps-weight label, no frame; the selected one carries a
    /// two-point bar of the agent's ink along its top over a raised wash.
    let CodeTab = Tab{
        height: 26
        // Tight enough that CHATS FILES SEARCH RUN fit the 182 pt sidebar.
        padding: Inset{left: 7 right: 7 top: 0 bottom: 0}
        margin: 0
        draw_text +: {
            text_style: mod.look.mono{font_size: 8.0}
            color: mod.look.meta
            color_hover: uniform(mod.look.text)
            color_active: uniform(mod.look.text_hi)
        }
        draw_bg +: {
            pixel: fn() {
                let p = self.pos * self.rect_size
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                let bar = (1.0 - step(2.0, p.y)) * self.active
                let side = 1.0 - step(px, self.rect_size.x - p.x)
                var c = vec4(0.0, 0.0, 0.0, 0.0)
                c = mix(c, vec4(mod.look.raised.rgb, 0.55), self.hover * 0.6)
                c = mix(c, vec4(mod.look.surface.rgb, 1.0), self.active)
                c = mix(c, mod.look.hair, side)
                c = mix(c, vec4(mod.look.agent.rgb, 1.0), bar)
                return vec4(c.rgb * c.a, c.a)
            }
        }
    }

    let CodeTabBar = TabBar{
        height: 26
        CloseableTab := CodeTab{closeable: true}
        PermanentTab := CodeTab{closeable: false}
        draw_drag +: { color: mod.look.agent_wash }
        draw_fill +: {
            pixel: fn() {
                let p = self.pos * self.rect_size
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                let rule = step(self.rect_size.y - px, p.y)
                let c = mix(vec4(mod.look.rail.rgb, 1.0), mod.look.hair, rule)
                return vec4(c.rgb * c.a, c.a)
            }
        }
        draw_bg +: {
            pixel: fn() {
                let p = self.pos * self.rect_size
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                let rule = step(self.rect_size.y - px, p.y)
                let c = mix(vec4(mod.look.rail.rgb, 1.0), mod.look.hair, rule)
                return vec4(c.rgb * c.a, c.a)
            }
        }
    }

    /// The bar between panels: a hairline that lights the agent's ink while
    /// it is hovered or dragged.
    let CodeSplitter = Splitter{
        size: 3.0
        // The terminal strip (the one top/bottom split) folds down to its
        // tab bar: the floors of a vertical-axis split are `_horizontal`.
        min_horizontal: 26.0
        max_horizontal: 26.0
        draw_bg +: {
            pixel: fn() {
                let p = self.pos * self.rect_size
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                // A vertical bar parts left and right panes.
                var d = abs(p.y - self.rect_size.y * 0.5)
                if self.is_vertical > 0.5 {
                    d = abs(p.x - self.rect_size.x * 0.5)
                }
                let line = 1.0 - step(px, d)
                let lit = max(self.hover, self.drag)
                let c = mix(vec4(mod.look.bg.rgb, 1.0), mix(mod.look.hair_hi, vec4(mod.look.agent.rgb, 1.0), lit), line)
                return vec4(c.rgb * c.a, c.a)
            }
        }
    }


    let ShellMono = Label{
        width: Fit
        height: Fit
        padding: 0
        draw_text +: {
            color: mod.look.meta
            text_style: mod.look.mono{font_size: 8.5}
        }
    }

    /// A key button, as the chat draws them: a chamfered hairline frame.
    let ShellKey = ButtonFlat{
        height: 22
        padding: Inset{left: 10 right: 10 top: 0 bottom: 0}
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

    mod.widgets.IdeShellBase = #(IdeShell::register_widget(vm))

    mod.widgets.IdeShell = set_type_default() do mod.widgets.IdeShellBase{
        width: Fill
        height: Fill
        // Quick-open floats over the dock.
        flow: Overlay
        show_bg: true
        draw_bg +: { color: mod.look.bg }

        main := View{
        width: Fill
        height: Fill
        flow: Down
        dock := DockFlat{
            width: Fill
            height: Fill
            padding: 0
            tab_bar: CodeTabBar{}
            splitter: CodeSplitter{}
            round_corner +: { border_radius: 0.0 }

            // Two areas: the sidebar (chats, files, search, run) and the
            // centre's tabs (files and chats side by side), with the
            // terminal strip folded under the centre.
            root := DockSplitter{
                axis: SplitterAxis.Horizontal
                align: SplitterAlign.FromA(182.0)
                a: @side_tabs
                b: @centre_split
            }
            centre_split := DockSplitter{
                axis: SplitterAxis.Vertical
                align: SplitterAlign.FromB(26.0)
                a: @main_tabs
                b: @term_tabs
            }

            side_tabs := DockTabs{ tabs: [@chats_tab @files_tab @search_tab @run_tab] selected: 0 closable: false }
            main_tabs := DockTabs{ tabs: [@chat_1] selected: 0 closable: false }
            term_tabs := DockTabs{ tabs: [@term_fold @term_add @term_1] selected: 2 closable: false }

            chats_tab := DockTab{ name: "CHATS" template: @PermanentTab kind: @ChatsBody }
            files_tab := DockTab{ name: "FILES" template: @PermanentTab kind: @FilesBody }
            search_tab := DockTab{ name: "SEARCH" template: @PermanentTab kind: @SearchBody }
            run_tab := DockTab{ name: "RUN" template: @PermanentTab kind: @RunBody }
            chat_1 := DockTab{ name: "CHAT" template: @CloseableTab kind: @ChatBody }
            term_fold := DockTab{ name: "▸ TERMINAL" template: @PermanentTab kind: @FoldBody }
            term_add := DockTab{ name: "+" template: @PermanentTab kind: @FoldBody }
            term_1 := DockTab{ name: "zsh" template: @CloseableTab kind: @TermBody }

            ChatsBody := SessionsPanel{}
            FilesBody := FilesPanel{}
            SearchBody := SearchPanel{}
            RunBody := RunPanel{}
            EditorBody := FileEditor{}
            ChatBody := AgentView{}
            BuildBody := BuildOutput{}
            TermBody := TerminalPanel{}
            FoldBody := View{}
        }

        // The editor's line: the file, where the caret is, its state.
        ide_status := View{
            width: Fill
            height: 20
            flow: Right
            spacing: 14
            padding: Inset{left: 12 right: 12}
            align: Align{y: 0.5}
            show_bg: true
            draw_bg +: {
                color: mod.look.well
                pixel: fn() {
                    let p = self.pos * self.rect_size
                    let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                    let rule = 1.0 - step(px, p.y)
                    let c = mix(self.color, mod.look.hair_hi, rule)
                    return vec4(c.rgb * c.a, c.a)
                }
            }
            status_path := ShellMono{ width: Fill draw_text +: { color: mod.look.text text_style: mod.look.mono{font_size: 8.0} } }
            status_state := ShellMono{ draw_text +: { color: mod.look.change text_style: mod.look.mono{font_size: 8.0} } }
            status_caret := ShellMono{ draw_text +: { color: mod.look.meta text_style: mod.look.mono{font_size: 8.0} } }
        }
        }

        quick_open := QuickOpen{}

        // Closing a tab with unsaved edits, or quitting with unsaved edits
        // or apps still running from here, asks first.
        discard := Modal{
            align: Align{x: 0.5 y: 0.3}
            bg_view +: { draw_bg.color: #x020306b8 }
            content +: {
                View{
                    width: 460
                    height: Fit
                    flow: Down
                    spacing: 10
                    padding: Inset{left: 16 right: 16 top: 14 bottom: 14}
                    show_bg: true
                    draw_bg +: {
                        color: mod.look.rail
                        pixel: fn() {
                            let p = self.pos * self.rect_size
                            let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                            let c0 = self.rect_size * 0.5
                            let d = Finish.sd_chamfer(p, c0, c0 - vec2(0.5, 0.5), 8.0)
                            let inside = Finish.cover(d, px)
                            let edge = Finish.band(d, 0.0, px, px)
                            var c = vec4(self.color.rgb, 1.0) * inside
                            c = mix(c, vec4(mod.look.human.rgb, 1.0), edge * 0.7)
                            return vec4(c.rgb * c.a, c.a)
                        }
                    }
                    discard_title := ShellMono{ text: "◆ UNSAVED CHANGES" draw_text +: { color: mod.look.human text_style: mod.look.mono_bold{font_size: 7.5} } }
                    discard_text := ShellMono{ width: Fill draw_text +: { color: mod.look.text_hi text_style: mod.look.mono{font_size: 9.0} } }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        discard_save := ShellKey{ text: "Save  (s)" draw_bg +: { ink: mod.look.agent } }
                        discard_drop := ShellKey{ text: "Discard  (d)" draw_bg +: { ink: mod.look.human } }
                        discard_cancel := ShellKey{ text: "Cancel  (esc)" }
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Pending {
    Close(LiveId),
    Quit,
}

/// An open chat tab: its widget and the saved session it shows.
struct ChatTab {
    tab: LiveId,
    uid: WidgetUid,
    session: Option<String>,
}

/// Where a width the person dragged the sidebar to is kept.
fn sidebar_file() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".makepad/code/sidebar-width"))
}

/// The sidebar width the person last set, if any (a sane range only).
fn saved_sidebar_width() -> Option<f64> {
    let text = std::fs::read_to_string(sidebar_file()?).ok()?;
    text.trim().parse::<f64>().ok().filter(|w| (80.0..=1200.0).contains(w))
}

fn save_sidebar_width(width: f64) {
    let Some(path) = sidebar_file() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, format!("{width:.0}\n"));
}

/// Unfolded height of the terminal strip, and folded (its tab bar).
const STRIP_OPEN: f64 = 240.0;
const STRIP_FOLDED: f64 = 26.0;

/// An open editor tab.
struct EditorTab {
    tab: LiveId,
    path: String,
    uid: WidgetUid,
    dirty: bool,
}

#[derive(Script, ScriptHook, Widget)]
pub struct IdeShell {
    #[deref]
    view: View,
    #[rust]
    editors: Vec<EditorTab>,
    /// The editor tab that last had the keyboard.
    #[rust]
    active: Option<LiveId>,
    /// Open terminal tabs, oldest first (the declared one is `term_1`).
    #[rust(vec![id!(term_1)])]
    terminals: Vec<LiveId>,
    /// The terminal strip is unfolded.
    #[rust]
    strip_open: bool,
    /// The terminal the strip shows (the fold and + tabs are not ones).
    #[rust(Some(id!(term_1)))]
    strip_tab: Option<LiveId>,
    /// Open chat tabs (the declared one is `chat_1`).
    #[rust(vec![ChatTab{ tab: id!(chat_1), uid: WidgetUid::default(), session: None }])]
    chats: Vec<ChatTab>,
    /// The chat that last had the keyboard: references land there.
    #[rust(Some(id!(chat_1)))]
    active_chat: Option<LiveId>,
    #[rust]
    chat_count: u64,
    /// The remembered sidebar width was applied (once, on the first draw).
    #[rust]
    sidebar_restored: bool,
    /// What waits on the prompt: a dirty tab's close, or the quit.
    #[rust]
    closing: Option<Pending>,
    /// The quit was confirmed: the next close request goes through.
    #[rust]
    quitting: bool,
}

fn file_tab(path: &str) -> LiveId {
    LiveId::from_str(&format!("file:{path}"))
}

fn file_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

impl IdeShell {
    pub fn open_file(&mut self, cx: &mut Cx, path: &str, line: Option<u32>, col: Option<u32>) {
        let Some(root) = Workspace::of(cx).map(|w| w.root.clone()) else { return };
        if !root.join(path).is_file() {
            return;
        }
        let dock = self.view.dock(cx, ids!(dock));
        let tab = file_tab(path);
        let existing = dock.item(tab);
        let item = if existing.is_empty() {
            let Some(item) = dock.create_and_select_tab(cx, id!(main_tabs), tab, id!(EditorBody), file_name(path), id!(CloseableTab), None) else {
                log!("code: could not open a tab for {path}");
                return;
            };
            self.editors.push(EditorTab { tab, path: path.to_string(), uid: item.widget_uid(), dirty: false });
            item
        } else {
            dock.select_tab(cx, tab);
            existing
        };
        if let Some(mut editor) = item.borrow_mut::<FileEditor>() {
            if editor.path() != Some(path) {
                editor.open(cx, &root, path);
            }
            if let Some(line) = line {
                editor.goto(cx, line, col);
            }
            editor.focus(cx);
        }
        self.active = Some(tab);
        if let Some(mut quick) = self.view.widget(cx, ids!(quick_open)).borrow_mut::<QuickOpen>() {
            quick.note_opened(path);
        }
        self.reveal(cx, path);
        self.view.redraw(cx);
    }

    fn reveal(&mut self, cx: &mut Cx, path: &str) {
        let dock = self.view.dock(cx, ids!(dock));
        if let Some(mut files) = dock.item(id!(files_tab)).borrow_mut::<FilesPanel>() {
            files.reveal(cx, path);
        }
    }

    fn focus_search(&mut self, cx: &mut Cx, query: Option<&str>) {
        let dock = self.view.dock(cx, ids!(dock));
        dock.select_tab(cx, id!(search_tab));
        if let Some(mut search) = dock.item(id!(search_tab)).borrow_mut::<SearchPanel>() {
            search.focus(cx, query);
        }
    }

    /// Cmd+`: unfold the strip with the keyboard in its terminal (one
    /// opens when none is), or fold it when it is open.
    fn toggle_terminal(&mut self, cx: &mut Cx) {
        if self.strip_open {
            self.set_strip(cx, false);
            return;
        }
        let tab = self.strip_tab.filter(|t| self.terminals.contains(t)).or(self.terminals.last().copied());
        let Some(tab) = tab else {
            self.new_terminal(cx);
            return;
        };
        self.show_terminal(cx, tab);
    }

    fn show_terminal(&mut self, cx: &mut Cx, tab: LiveId) {
        self.strip_tab = Some(tab);
        self.set_strip(cx, true);
        let dock = self.view.dock(cx, ids!(dock));
        dock.select_tab(cx, tab);
        if let Some(mut term) = dock.item(tab).borrow_mut::<TerminalPanel>() {
            term.focus(cx);
        }
    }

    /// Fold the strip to its tab bar, or unfold it.
    fn set_strip(&mut self, cx: &mut Cx, open: bool) {
        self.strip_open = open;
        let dock = self.view.dock(cx, ids!(dock));
        let h = if open { STRIP_OPEN } else { STRIP_FOLDED };
        dock.set_splitter_align(cx, id!(centre_split), SplitterAlign::FromB(h), false);
        dock.set_tab_title(cx, id!(term_fold), if open { "▾ TERMINAL" } else { "▸ TERMINAL" }.to_string());
        if !open {
            // Folded, the strip shows no body: hand the keyboard back.
            if let Some(tab) = self.active.or(self.active_chat) {
                dock.select_tab(cx, tab);
            }
        }
        self.view.redraw(cx);
    }

    /// Cmd+Shift+B: the RUN tab with the keyboard (b builds, r runs).
    fn focus_run(&mut self, cx: &mut Cx) {
        let dock = self.view.dock(cx, ids!(dock));
        dock.select_tab(cx, id!(run_tab));
        if let Some(mut run) = dock.item(id!(run_tab)).borrow_mut::<RunPanel>() {
            run.focus(cx);
        }
    }

    fn new_terminal(&mut self, cx: &mut Cx) {
        let dock = self.view.dock(cx, ids!(dock));
        let tab = dock.unique_id(LiveId::from_str("term").0);
        self.terminals.push(tab);
        if dock.create_tab(cx, id!(term_tabs), tab, id!(TermBody), "zsh".to_string(), id!(CloseableTab), None).is_some() {
            self.show_terminal(cx, tab);
        }
    }

    // ---- chats ------------------------------------------------------------

    /// A chat as a centre tab: the one already showing that session, or a
    /// new tab resuming it (or starting a new conversation).
    fn open_chat(&mut self, cx: &mut Cx, session: Option<String>) {
        let dock = self.view.dock(cx, ids!(dock));
        if let Some(open) = session.as_ref().and_then(|s| self.chats.iter().find(|c| c.session.as_ref() == Some(s))) {
            let tab = open.tab;
            dock.select_tab(cx, tab);
            self.active_chat = Some(tab);
            return;
        }
        self.chat_count += 1;
        let tab = dock.unique_id(LiveId::from_str("chat").0 + self.chat_count);
        let Some(item) = dock.create_and_select_tab(cx, id!(main_tabs), tab, id!(ChatBody), "CHAT".to_string(), id!(CloseableTab), None) else {
            log!("code: could not open a chat tab");
            return;
        };
        if let Some(mut chat) = item.borrow_mut::<AgentView>() {
            chat.open_session(cx, session.clone());
        }
        self.chats.push(ChatTab { tab, uid: item.widget_uid(), session });
        self.active_chat = Some(tab);
        self.mark_open_sessions(cx);
    }

    fn mark_open_sessions(&mut self, cx: &mut Cx) {
        let open: Vec<String> = self.chats.iter().filter_map(|c| c.session.clone()).collect();
        let dock = self.view.dock(cx, ids!(dock));
        if let Some(mut sessions) = dock.item(id!(chats_tab)).borrow_mut::<SessionsPanel>() {
            sessions.set_open(cx, &open);
        }
    }

    /// The chat a reference goes to: the last one used, else a new one.
    fn reference_target(&mut self, cx: &mut Cx) -> WidgetRef {
        let tab = self.active_chat.filter(|t| self.chats.iter().any(|c| c.tab == *t));
        if tab.is_none() {
            self.open_chat(cx, None);
        }
        let tab = self.active_chat.unwrap_or(id!(chat_1));
        let dock = self.view.dock(cx, ids!(dock));
        dock.select_tab(cx, tab);
        dock.item(tab)
    }

    /// The BUILD tab in the centre (one; made on the first build).
    fn show_build(&mut self, cx: &mut Cx) {
        let dock = self.view.dock(cx, ids!(dock));
        if dock.item(id!(build_tab)).is_empty() {
            dock.create_and_select_tab(cx, id!(main_tabs), id!(build_tab), id!(BuildBody), "BUILD".to_string(), id!(CloseableTab), None);
        } else {
            dock.select_tab(cx, id!(build_tab));
        }
    }

    fn editor_at(&self, cx: &mut Cx, path: &str) -> Option<WidgetRef> {
        let tab = self.editors.iter().find(|e| e.path == path)?.tab;
        let item = self.view.dock(cx, ids!(dock)).item(tab);
        (!item.is_empty()).then_some(item)
    }

    fn refresh_index(&mut self, cx: &mut Cx) {
        if let Some(ws) = Workspace::of(cx) {
            ws.index.refresh();
        }
    }

    /// A file or folder moved: its open tabs follow (a clean one reloads
    /// from the new place, a dirty one keeps its edits and saves there).
    fn follow_rename(&mut self, cx: &mut Cx, from: &str, to: &str) {
        let Some(root) = Workspace::of(cx).map(|w| w.root.clone()) else { return };
        let moved = |p: &str| -> Option<String> {
            if p == from {
                Some(to.to_string())
            } else {
                p.strip_prefix(from).filter(|rest| rest.starts_with('/')).map(|rest| format!("{to}{rest}"))
            }
        };
        let dock = self.view.dock(cx, ids!(dock));
        for e in self.editors.iter_mut() {
            let Some(new) = moved(&e.path) else { continue };
            e.path = new.clone();
            let title = if e.dirty { format!("{} ●", file_name(&new)) } else { file_name(&new) };
            dock.set_tab_title(cx, e.tab, title);
            if let Some(mut editor) = dock.item(e.tab).borrow_mut::<FileEditor>() {
                editor.moved_to(cx, &root, &new);
            }
        }
    }

    /// A file or folder was deleted: clean tabs of it close; dirty ones stay
    /// (their edits can still be saved back).
    fn follow_delete(&mut self, cx: &mut Cx, path: &str) {
        let gone: Vec<LiveId> = self
            .editors
            .iter()
            .filter(|e| !e.dirty && (e.path == path || e.path.strip_prefix(path).is_some_and(|r| r.starts_with('/'))))
            .map(|e| e.tab)
            .collect();
        for tab in gone {
            self.close_tab(cx, tab);
        }
    }

    fn handle_ide_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for action in actions {
            let Some(wa) = action.as_widget_action() else { continue };
            match wa.cast::<IdeAction>() {
                IdeAction::OpenFile { path, line, col } => self.open_file(cx, &path, line, col),
                IdeAction::AgentEdited { path, lines } => {
                    if let Some(item) = self.editor_at(cx, &path) {
                        if let Some(mut editor) = item.borrow_mut::<FileEditor>() {
                            editor.agent_edited(cx, &lines);
                        }
                    }
                }
                IdeAction::Created(path) => {
                    self.refresh_index(cx);
                    self.open_file(cx, &path, None, None);
                }
                IdeAction::Renamed { from, to } => {
                    self.follow_rename(cx, &from, &to);
                    self.refresh_index(cx);
                }
                IdeAction::Deleted(path) => {
                    self.follow_delete(cx, &path);
                    self.refresh_index(cx);
                }
                IdeAction::AddReference(path) => {
                    let item = self.reference_target(cx);
                    if let Some(mut chat) = item.borrow_mut::<AgentView>() {
                        chat.add_reference(cx, &path);
                    };
                }
                IdeAction::OpenChat { session } => self.open_chat(cx, session),
                IdeAction::ChatTitle(title) => {
                    if let Some(c) = self.chats.iter().find(|c| c.uid == wa.widget_uid) {
                        let tab = c.tab;
                        self.view.dock(cx, ids!(dock)).set_tab_title(cx, tab, title);
                    }
                }
                IdeAction::ChatSession(id) => {
                    if let Some(c) = self.chats.iter_mut().find(|c| c.uid == wa.widget_uid) {
                        c.session = Some(id);
                    }
                    self.mark_open_sessions(cx);
                }
                IdeAction::SessionsChanged => {
                    let dock = self.view.dock(cx, ids!(dock));
                    if let Some(mut sessions) = dock.item(id!(chats_tab)).borrow_mut::<SessionsPanel>() {
                        sessions.refresh(cx);
                    }
                }
                IdeAction::ShowBuild => self.show_build(cx),
                IdeAction::None => {}
            }
            match wa.cast::<FileEditorAction>() {
                FileEditorAction::Dirty(dirty) => {
                    if let Some(e) = self.editors.iter_mut().find(|e| e.uid == wa.widget_uid) {
                        e.dirty = dirty;
                        let title = if dirty { format!("{} ●", file_name(&e.path)) } else { file_name(&e.path) };
                        let tab = e.tab;
                        self.view.dock(cx, ids!(dock)).set_tab_title(cx, tab, title);
                        // The status line shows the mark too.
                        self.view.redraw(cx);
                    }
                }
                FileEditorAction::Caret { line, col } => {
                    if self.editors.iter().any(|e| e.uid == wa.widget_uid) {
                        self.view.label(cx, ids!(status_caret)).set_text(cx, &format!("Ln {line}, Col {col}"));
                    }
                }
                FileEditorAction::Saved => {
                    // A save relaunches apps running under `makepad run` warm.
                    if let Some(path) = self.editors.iter().find(|e| e.uid == wa.widget_uid).map(|e| e.path.clone()) {
                        super::run::on_saved(cx, &path);
                    }
                }
                FileEditorAction::None => {}
            }
        }
    }

    fn handle_dock_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let dock = self.view.dock(cx, ids!(dock));
        for action in actions {
            let Some(wa) = action.as_widget_action() else { continue };
            match wa.cast::<DockAction>() {
                // A width the person dragged the sidebar to is remembered.
                DockAction::SplitPanelChanged { panel_id, align: SplitterAlign::FromA(width), .. } if panel_id == id!(root) => {
                    save_sidebar_width(width);
                }
                DockAction::ShouldTabStartDrag(tab) => {
                    dock.tab_start_drag(cx, tab, DragItem::FilePath { path: String::new(), internal_id: Some(tab) });
                }
                DockAction::Drag(drag) => {
                    if matches!(drag.items.first(), Some(DragItem::FilePath { internal_id: Some(_), .. })) {
                        dock.accept_drag(cx, drag, DragResponse::Move);
                    }
                }
                DockAction::Drop(drop) => {
                    if let Some(DragItem::FilePath { internal_id: Some(tab), .. }) = drop.items.first() {
                        dock.drop_move(cx, drop.abs, *tab);
                    }
                }
                DockAction::TabCloseWasPressed(tab) => {
                    match self.editors.iter().find(|e| e.tab == tab) {
                        Some(e) if e.dirty => {
                            let text = format!("{} has changes that are not saved.", e.path);
                            self.closing = Some(Pending::Close(tab));
                            self.prompt(cx, "◆ UNSAVED CHANGES", &text, Some("Save  (s)"), "Discard  (d)");
                        }
                        _ => {
                            if let Some(mut term) = dock.item(tab).borrow_mut::<TerminalPanel>() {
                                term.close(cx);
                            }
                            self.close_tab(cx, tab);
                        }
                    }
                }
                DockAction::TabWasPressed(tab) if tab == id!(term_fold) => {
                    let open = !self.strip_open;
                    match self.strip_tab.filter(|_| open) {
                        Some(t) => self.show_terminal(cx, t),
                        None if open => self.toggle_terminal(cx),
                        None => self.set_strip(cx, false),
                    }
                }
                DockAction::TabWasPressed(tab) if tab == id!(term_add) => self.new_terminal(cx),
                DockAction::TabWasPressed(tab) if self.terminals.contains(&tab) => self.show_terminal(cx, tab),
                DockAction::TabWasPressed(tab) if self.chats.iter().any(|c| c.tab == tab) => {
                    self.active_chat = Some(tab);
                    self.active = None;
                }
                DockAction::TabWasPressed(tab) => {
                    if let Some(path) = self.editors.iter().find(|e| e.tab == tab).map(|e| e.path.clone()) {
                        self.active = Some(tab);
                        self.reveal(cx, &path);
                    }
                }
                _ => {}
            }
        }
    }

    fn close_tab(&mut self, cx: &mut Cx, tab: LiveId) {
        self.view.dock(cx, ids!(dock)).close_tab(cx, tab);
        self.editors.retain(|e| e.tab != tab);
        self.terminals.retain(|t| *t != tab);
        if self.chats.iter().any(|c| c.tab == tab) {
            self.chats.retain(|c| c.tab != tab);
            if self.active_chat == Some(tab) {
                self.active_chat = self.chats.last().map(|c| c.tab);
            }
            self.mark_open_sessions(cx);
        }
        if self.strip_tab == Some(tab) {
            self.strip_tab = self.terminals.last().copied();
            if self.strip_tab.is_none() {
                self.set_strip(cx, false);
            }
        }
        if self.active == Some(tab) {
            self.active = None;
        }
    }

    fn prompt(&mut self, cx: &mut Cx, title: &str, text: &str, save: Option<&str>, drop: &str) {
        self.view.label(cx, ids!(discard_title)).set_text(cx, title);
        self.view.label(cx, ids!(discard_text)).set_text(cx, text);
        let save_button = self.view.button(cx, ids!(discard_save));
        save_button.set_visible(cx, save.is_some());
        if let Some(save) = save {
            save_button.set_text(cx, save);
        }
        self.view.button(cx, ids!(discard_drop)).set_text(cx, drop);
        self.view.modal(cx, ids!(discard)).open(cx);
    }

    /// The prompt's answer: Some(true) save first, Some(false) go on
    /// without saving, None keep everything as it is.
    fn answer_close(&mut self, cx: &mut Cx, save: Option<bool>) {
        self.view.modal(cx, ids!(discard)).close(cx);
        let Some(pending) = self.closing.take() else { return };
        let Some(save) = save else { return };
        match pending {
            Pending::Close(tab) => {
                if save && !self.save_tab(cx, tab) {
                    return;
                }
                self.close_tab(cx, tab);
            }
            Pending::Quit => {
                if save {
                    let dirty: Vec<LiveId> = self.editors.iter().filter(|e| e.dirty).map(|e| e.tab).collect();
                    for tab in dirty {
                        if !self.save_tab(cx, tab) {
                            return;
                        }
                    }
                }
                self.quitting = true;
                makepad_code_agent::runs::RunManager::global().shutdown();
                cx.quit();
            }
        }
    }

    fn save_tab(&mut self, cx: &mut Cx, tab: LiveId) -> bool {
        let item = self.view.dock(cx, ids!(dock)).item(tab);
        let saved = match item.borrow_mut::<FileEditor>() {
            Some(mut editor) => editor.save(cx).is_ok(),
            None => true,
        };
        saved
    }

    /// A close or quit request: let it through, or hold it and ask when
    /// files are unsaved or apps started here still run.
    fn hold_quit(&mut self, cx: &mut Cx) -> bool {
        if self.quitting {
            return false;
        }
        let dirty: Vec<String> = self.editors.iter().filter(|e| e.dirty).map(|e| e.path.clone()).collect();
        let live = makepad_code_agent::runs::RunManager::global().live_names();
        if dirty.is_empty() && live.is_empty() {
            return false;
        }
        if self.closing == Some(Pending::Quit) {
            return true;
        }
        let mut lines = Vec::new();
        if !dirty.is_empty() {
            lines.push(format!("Unsaved: {}.", dirty.join(", ")));
        }
        if !live.is_empty() {
            lines.push(format!("Still running from here (they will be stopped): {}.", live.join(", ")));
        }
        self.closing = Some(Pending::Quit);
        let save = (!dirty.is_empty()).then_some("Save all and quit  (s)");
        self.prompt(cx, "◆ QUIT MAKEPAD CODE", &lines.join("\n"), save, "Quit  (d)");
        true
    }

    fn discard_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.closing.is_none() {
            return;
        }
        if self.view.modal(cx, ids!(discard)).dismissed(actions) || self.view.button(cx, ids!(discard_cancel)).clicked(actions) {
            self.answer_close(cx, None);
        } else if self.view.button(cx, ids!(discard_save)).clicked(actions) {
            self.answer_close(cx, Some(true));
        } else if self.view.button(cx, ids!(discard_drop)).clicked(actions) {
            self.answer_close(cx, Some(false));
        }
    }

    fn window_keys(&mut self, cx: &mut Cx, ke: &KeyEvent) {
        if self.closing.is_some() {
            match ke.key_code {
                KeyCode::KeyS if self.view.button(cx, ids!(discard_save)).visible() => self.answer_close(cx, Some(true)),
                KeyCode::KeyD => self.answer_close(cx, Some(false)),
                KeyCode::Escape => self.answer_close(cx, None),
                _ => {}
            }
            return;
        }
        let m = ke.modifiers;
        if !m.is_primary() || m.alt {
            return;
        }
        match (ke.key_code, m.shift) {
            // On Linux Ctrl+P belongs to a focused terminal's shell (history).
            (KeyCode::KeyP, false) if cfg!(target_os = "macos") || !super::terminal::terminal_has_focus(cx) => {
                if let Some(mut quick) = self.view.widget(cx, ids!(quick_open)).borrow_mut::<QuickOpen>() {
                    quick.open(cx);
                }
            }
            (KeyCode::KeyF, true) => self.focus_search(cx, None),
            (KeyCode::Backtick, false) => self.toggle_terminal(cx),
            (KeyCode::KeyB, true) => self.focus_run(cx),
            (KeyCode::KeyT, false) => self.open_chat(cx, None),
            (KeyCode::Backtick, true) => self.new_terminal(cx),
            _ => {}
        }
    }
}

impl Widget for IdeShell {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        match event {
            Event::WindowCloseRequested(e) => {
                if self.hold_quit(cx) {
                    e.accept_close.set(false);
                }
            }
            Event::QuitRequested(e) => {
                if self.hold_quit(cx) {
                    e.handled.set(true);
                }
            }
            _ => {}
        }
        if let Event::KeyDown(ke) = event {
            self.window_keys(cx, ke);
        }
        let actions = cx.capture_actions(|cx| self.view.handle_event(cx, event, scope));
        if !actions.is_empty() {
            self.discard_actions(cx, &actions);
            self.handle_dock_actions(cx, &actions);
            self.handle_ide_actions(cx, &actions);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.sidebar_restored {
            self.sidebar_restored = true;
            if let Some(width) = saved_sidebar_width() {
                self.view.dock(cx, ids!(dock)).set_splitter_align(cx, id!(root), SplitterAlign::FromA(width), false);
            }
        }
        // The declared chat tab's widget exists once the dock has made it.
        if let Some(first) = self.chats.iter_mut().find(|c| c.uid == WidgetUid::default()) {
            let uid = self.view.dock(cx, ids!(dock)).item(first.tab).widget_uid();
            first.uid = uid;
        }
        let active = self.active.and_then(|tab| self.editors.iter().find(|e| e.tab == tab));
        let (path, state) = match active {
            Some(e) => (e.path.clone(), if e.dirty { "● unsaved" } else { "" }),
            None => (String::new(), ""),
        };
        self.view.label(cx, ids!(status_path)).set_text(cx, &path);
        self.view.label(cx, ids!(status_state)).set_text(cx, state);
        // The line belongs to the editor: a chat tab has its own.
        self.view.view(cx, ids!(ide_status)).set_visible(cx, active.is_some());
        self.view.draw_walk(cx, scope, walk)
    }
}
