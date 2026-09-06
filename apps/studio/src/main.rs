//! Studio: one live workspace, presented as structured tabs or a zoomable canvas.
//!
//! One window: a compact icon toolbar, a shared canvas/Dock and a status
//! strip. Everything inherits the stock
//! widget styles; standalone the OS-style picker in Settings chooses the
//! family, hosted under the WM its stylesheet wins.
//!
//! Command line:
//!   --state-dir <dir>   settings + dock layout (default ~/.makepad/studio)
//!   --cwd <dir>         working directory for new terminals

use makepad_ai_services::{
    port::{AiServicePort, PortEvent},
    wire::ToolResult,
};
use makepad_strict_json::{self as json, Value};
use makepad_studio::appearance::{self, StyleChoice};
use makepad_studio::state::{self, Args, Settings};
use makepad_studio::{
    activity::{ActivitySnapshot, ActivityWorker, FileState, ProcessState},
    canvas::{CanvasAction, StudioSurface},
    document::{DocumentHandle, DocumentRegistry, StudioCodeEditor, StudioCodeEditorAction},
    document_worker::DocumentWorker,
    project_tree::{ProjectTreeAction, StudioProjectTree},
    usage::{UsageProvider, UsageSnapshot, UsageWorker},
    workspace::{Card, CardKind, LayoutMode, Mode, Workspace},
};
use makepad_studio::{
    ai::{self, Action},
    disk::{self, DiskWorker, Snapshot},
    disk_graph::DiskGraph,
};
use makepad_terminal::widget::{MpTerm, MpTermAction};
pub use makepad_widgets;
use makepad_widgets::desktop_style;
use makepad_widgets::dock::{DockAction, DockItem};
use makepad_widgets::makepad_micro_serde::*;
use makepad_widgets::tip::TipWidgetRefExt;
use makepad_widgets::*;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

app_main!(
    App,
    font_assets: [
        "makepad_widgets/resources/jetbrains_mono_variable.ttf",
        "makepad_widgets/resources/fa-solid-900.ttf",
    ]
);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let ToolbarIcon = ButtonIcon{
        width: 30 height: 30 padding: 0 margin: 0
        icon_walk: Walk{width: 17 height: 17}
        draw_icon +: {color: theme.color_label_inner}
    }
    let ToolbarMode = RadioButtonTab{
        width: 30 height: 30 padding: 0 margin: 0 spacing: 0 align: Center
        text: ""
        icon_walk: Walk{width: 17 height: 17 margin: 0}
        label_walk: Walk{width: 0 height: 0 margin: 0}
        draw_icon +: {color: theme.color_text}
        draw_bg +: {
            border_radius: 4.0
            color_active: mix(theme.color_bg_app, theme.color_focus, 0.2)
            border_color_active: theme.color_focus
        }
    }
    let ToolbarPicker = DropDown{
        height: 30 margin: 0
        align: Align{x: 0.0 y: 0.5}
        padding: Inset{left: 9 right: 22 top: 0 bottom: 0}
    }
    let ToolbarDivider = View{
        width: 1 height: 18 show_bg: true
        draw_bg +: {color: theme.color_bevel}
    }
    let StatusText = Label{
        padding: 0
        draw_text +: {
            color: theme.color_text_disabled
            text_style: theme.font_regular{font_size: 9.5}
        }
    }

    let StudioTab = Tab{
        height: 28
        padding: Inset{left: 10 right: 6 top: 0 bottom: 0}
        margin: Inset{top: 2 right: 2}
        draw_text +: {
            color: theme.color_label_inner_inactive
            color_hover: theme.color_text
            color_active: theme.color_text
        }
        draw_bg +: {
            border_radius: 3.0 border_size: 1.0 color_dither: 0.0
            color: mix(theme.color_bg_app, theme.color_text, 0.06)
            color_hover: mix(theme.color_bg_app, theme.color_text, 0.11)
            color_active: theme.color_bg_app
            border_color: mix(theme.color_bg_app, theme.color_text, 0.10)
            border_color_hover: mix(theme.color_bg_app, theme.color_text, 0.24)
            border_color_active: mix(theme.color_bg_app, theme.color_focus, 0.65)
        }
    }

    let StudioTabBar = TabBar{
        height: 30
        CloseableTab := StudioTab{closeable: true}
        PermanentTab := StudioTab{closeable: false}
        draw_bg +: {
            color: mix(theme.color_bg_app, theme.color_text, 0.075)
            border_color: uniform(mix(theme.color_bg_app, theme.color_text, 0.18))
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.clear(self.color)
                sdf.rect(0.0, self.rect_size.y - 1.0, self.rect_size.x, 1.0)
                return sdf.fill(self.border_color)
            }
        }
        draw_fill +: {pixel: fn(){return #0000}}
    }

    let StudioSplitter = Splitter{
        size: 8.0
        draw_bg +: {
            color_bg: mix(theme.color_bg_app, theme.color_text, 0.055)
            color: mix(theme.color_bg_app, theme.color_text, 0.25)
            color_hover: mix(theme.color_bg_app, theme.color_focus, 0.80)
            color_drag: theme.color_focus
            color_bg_hover: uniform(mix(theme.color_bg_app, theme.color_focus, 0.18))
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let emphasis = max(self.hover, self.drag)
                let thickness = mix(1.0, 2.0, emphasis)
                sdf.clear(mix(self.color_bg, self.color_bg_hover, emphasis))
                if self.is_vertical > 0.5 {
                    sdf.rect((self.rect_size.x - thickness) * 0.5, 0.0, thickness, self.rect_size.y)
                }
                else {
                    sdf.rect(0.0, (self.rect_size.y - thickness) * 0.5, self.rect_size.x, thickness)
                }
                return sdf.fill(mix(self.color, mix(self.color_hover, self.color_drag, self.drag), emphasis))
            }
        }
    }

    let StudioDock = Dock{
        width: Fill height: Fill
        tab_bar: StudioTabBar{}
        splitter: StudioSplitter{}
        round_corner +: {border_radius: 4.0}

        root := DockSplitter{
            axis: SplitterAxis.Horizontal align: SplitterAlign.FromA(216.0)
            a: @project_tabs b: @work_tabs
        }
        project_tabs := DockTabs{tabs: [@project_tree_tab] selected: 0 closable: true}
        project_tree_tab := DockTab{name: "Project" template: @PermanentTab kind: @ProjectTreeTab}
        work_tabs := DockTabs{
            tabs: [@terminal_1]
            selected: 0
            closable: true
        }
        terminal_1 := DockTab{
            name: "Terminal"
            template: @CloseableTab
            kind: @TerminalTab
        }

        TerminalTab := StudioTerminalTab{}
        SettingsTab := StudioSettings{}
        DiskTab := StudioDisk{}
        CodeTab := StudioCodeEditor{}
        ActivityTab := StudioActivity{}
        UsageTab := StudioUsage{}
        DesignTab := StudioDesign{}
        ProjectTreeTab := View{
            width: Fill height: Fill flow: Down spacing: 0
            show_bg: true
            draw_bg +: {color: mix(theme.color_bg_app, theme.color_text, 0.035)}
            View{width: Fill height: Fit flow: Right padding: 8 spacing: 4 align: Align{y: 0.5}
                show_bg: true
                draw_bg +: {color: mix(theme.color_bg_app, theme.color_text, 0.055)}
                project_name := Label{width: Fill text: "Project" draw_text +: {text_style: theme.font_bold{font_size: 10}}}
                Tip{text: "Reveal the active file in the project tree"
                    reveal_project_file := ToolbarIcon{width: 24 height: 24 draw_icon +: {svg: crate_resource("self:resources/icons/fit.svg")}}
                }
                Tip{text: "Refresh project files"
                    refresh_project_tree := ToolbarIcon{width: 24 height: 24 draw_icon +: {svg: crate_resource("self:resources/icons/refresh.svg")}}
                }
            }
            project_tree := StudioProjectTree{
                file_tree +: {
                    file_node +: {
                        draw_bg +: {
                            color_1: mix(theme.color_bg_app, theme.color_text, 0.035)
                            color_2: mix(theme.color_bg_app, theme.color_text, 0.035)
                            color_active: mix(theme.color_bg_app, theme.color_focus, 0.22)
                        }
                    }
                    folder_node +: {
                        draw_bg +: {
                            color_1: mix(theme.color_bg_app, theme.color_text, 0.035)
                            color_2: mix(theme.color_bg_app, theme.color_text, 0.035)
                            color_active: mix(theme.color_bg_app, theme.color_focus, 0.22)
                        }
                    }
                    filler +: {
                        color_1: uniform(mix(theme.color_bg_app, theme.color_text, 0.035))
                        pixel: fn(){return self.color_1}
                    }
                }
            }
        }
    }

    let UtilityDock = Dock{
        width: Fill height: Fill
        root := DockTabs{tabs: [] selected: 0 closable: false hide_tab_bar: true}
        SettingsTab := StudioSettings{}
        DiskTab := StudioDisk{}
        UsageTab := StudioUsage{}
        ActivityTab := StudioActivity{}
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Studio"
                window.inner_size: vec2(1180, 760)
                pass +: { clear_color: theme.color_bg_app }
                body +: {
                    flow: Overlay padding: 0 margin: 0 spacing: 0
                    main := View{
                        width: Fill height: Fill
                        flow: Down spacing: 0
                        work_area := View{
                            width: Fill height: Fill flow: Down spacing: 0
                            toolbar := View{
                                width: Fill height: Fit flow: Right spacing: 4 padding: 8
                                align: Align{y: 0.5}
                                Tip{text: "Structured · project files, tabs and docked panes"
                                    mode_structured := ToolbarMode{draw_icon +: {svg: crate_resource("self:resources/icons/structured.svg")}}
                                }
                                Tip{text: "Canvas · zoom and pan across agent work"
                                    mode_canvas := ToolbarMode{draw_icon +: {svg: crate_resource("self:resources/icons/canvas.svg")}}
                                }
                                canvas_controls := View{
                                    width: Fit height: Fit flow: Right spacing: 3 align: Align{y: 0.5}
                                    Tip{text: "Auto arranges new cards; Free lets you drag and resize"
                                        layout_picker := ToolbarPicker{width: 96 labels: ["Auto layout" "Free layout"] selected_item: 0}
                                    }
                                    Tip{text: "Fit all canvas cards"
                                        fit_canvas := ToolbarIcon{draw_icon +: {svg: crate_resource("self:resources/icons/fit.svg")}}
                                    }
                                    Tip{text: "Zoom out · scroll on the canvas to zoom"
                                        zoom_out := ToolbarIcon{draw_icon +: {svg: crate_resource("self:resources/icons/zoom-out.svg")}}
                                    }
                                    Tip{text: "Zoom in · scroll on the canvas to zoom"
                                        zoom_in := ToolbarIcon{draw_icon +: {svg: crate_resource("self:resources/icons/zoom-in.svg")}}
                                    }
                                }
                                ToolbarDivider{}
                                Tip{text: "New terminal"
                                    new_terminal := ToolbarIcon{draw_icon +: {svg: crate_resource("self:resources/icons/terminal.svg")}}
                                }
                                Tip{text: "Open a source file"
                                    toggle_code := ToolbarIcon{draw_icon +: {svg: crate_resource("self:resources/icons/code.svg")}}
                                }
                                save_code_tip := Tip{text: "Save the active code editor"
                                    save_code := ToolbarIcon{draw_icon +: {svg: crate_resource("self:resources/icons/save.svg")}}
                                }
                                discard_code_tip := Tip{text: "Discard local edits and reload the latest disk revision"
                                    discard_code := ToolbarIcon{draw_icon +: {svg: crate_resource("self:resources/icons/discard.svg")}}
                                }
                                Tip{text: "Activity · running processes and source changes"
                                    open_activity := ToolbarIcon{draw_icon +: {svg: crate_resource("self:resources/icons/activity.svg")}}
                                }
                                Tip{text: "Disk space and workspace inventory"
                                    open_disk := ToolbarIcon{draw_icon +: {svg: crate_resource("self:resources/icons/disk.svg")}}
                                }
                                canvas_summary := StatusText{text: "Live workspace"}
                                View{width: Fill height: 1}
                                Tip{text: "Studio settings · press F10 for the assistant"
                                    open_settings := ToolbarIcon{draw_icon +: {svg: crate_resource("self:resources/icons/settings.svg")}}
                                }
                            }
                            file_toolbar := View{
                                visible: false
                                width: Fill height: Fit flow: Right spacing: 4 padding: Inset{left: 8 right: 8 bottom: 8}
                                align: Align{y: 0.5}
                                file_path := TextInput{width: Fill height: 30 margin: 0 empty_text: "Source path · absolute or relative to this project"}
                                Tip{text: "Open source file · Enter"
                                    open_code := ToolbarIcon{draw_icon +: {svg: crate_resource("self:resources/icons/open-file.svg")}}
                                }
                                Tip{text: "Close the source path bar"
                                    close_file_toolbar := ToolbarIcon{draw_icon +: {svg: crate_resource("self:resources/icons/close.svg")}}
                                }
                            }
                            workspace := StudioSurface{dock: StudioDock{}}
                        }
                        status := View{
                            width: Fill height: Fit flow: Down spacing: 2
                            padding: Inset{left: 10 right: 10 top: 4 bottom: 4}
                            show_bg: true draw_bg +: {color: theme.color_bg_container}
                            View{width: Fill height: Fit flow: Right spacing: 8
                                status_mode := StatusText{text: "Standalone"}
                                status_style := StatusText{text: ""}
                                status_state := StatusText{width: Fill text: "" draw_text +: {wrap: Words}}
                            }
                            View{width: Fill height: Fit flow: Right spacing: 4 align: Align{y: 0.5}
                                Tip{text: "Fable session / week used · session reset time / weekly reset date · click for account and timezone"
                                    fable_usage := ButtonFlatter{height: 34 margin: 0 padding: Inset{left: 2 right: 2}
                                        text: "Fable  S — · W —" draw_text +: {text_style: theme.font_regular{font_size: 9.5}}
                                    }
                                }
                                Tip{text: "Astra weekly usage and reset date · click for account and reset details"
                                    astra_usage := ButtonFlatter{height: 34 margin: 0 padding: Inset{left: 2 right: 2}
                                        text: "Astra  W —" draw_text +: {text_style: theme.font_regular{font_size: 9.5}}
                                    }
                                }
                                Tip{text: "Refresh Fable and Astra limits now"
                                    refresh_usage_status := ToolbarIcon{
                                        width: 24 height: 24 icon_walk: Walk{width: 14 height: 14}
                                        draw_icon +: {svg: crate_resource("self:resources/icons/refresh.svg")}
                                    }
                                }
                                View{width: Fill height: 1}
                                disk_corner := View{
                                    width: Fit height: Fit flow: Right spacing: 8
                                    cursor: MouseCursor.Hand grab_key_focus: false
                                    align: Align{y: 0.5}
                                    disk_graph := StudioDiskGraph{}
                                    disk_open := Button{text: "Measuring disk…"}
                                }
                            }
                        }
                    }
                    utility_overlay := Modal{
                        content +: {
                            width: 640 height: 520
                            utility_root := RoundedView{
                                width: Fill height: Fill flow: Down
                                padding: 1 spacing: 0
                                draw_bg +: {
                                    color: theme.color_bg_app
                                    border_color: theme.color_bevel
                                    border_size: 1.0 border_radius: 5.0
                                }
                                View{
                                    width: Fill height: Fit flow: Right
                                    padding: 8 spacing: 8 align: Align{y: 0.5}
                                    utility_title := Label{
                                        width: Fill text: "Studio"
                                        draw_text +: {text_style: theme.font_bold{font_size: 12}}
                                    }
                                    Tip{text: "Close panel · Escape"
                                        close_utility := ToolbarIcon{draw_icon +: {svg: crate_resource("self:resources/icons/close.svg")}}
                                    }
                                }
                                utility_dock := UtilityDock{}
                            }
                        }
                    }
                    tip_layer := TipLayer{}
                }
            }
        }
    }
}

/// Tab kinds this build ships; a saved layout naming anything else is ignored.
const TAB_KINDS: [LiveId; 8] = [
    id!(TerminalTab),
    id!(SettingsTab),
    id!(DiskTab),
    id!(CodeTab),
    id!(ActivityTab),
    id!(UsageTab),
    id!(DesignTab),
    id!(ProjectTreeTab),
];

#[derive(Clone, SerRon, DeRon)]
struct Design {
    id: u64,
    title: String,
    detail: String,
    parent: Option<u64>,
    path: Option<String>,
}
#[derive(Default, SerRon, DeRon)]
struct StoredItems {
    documents: Vec<(u64, String)>,
    designs: Vec<Design>,
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    args: Args,
    #[rust]
    settings: Settings,
    /// Numbering for terminal tab ids allocated at runtime.
    #[rust]
    terminal_seq: u64,
    #[rust]
    disk_worker: Option<DiskWorker>,
    #[rust]
    usage_worker: Option<UsageWorker>,
    #[rust]
    usage_snapshot: Arc<UsageSnapshot>,
    #[rust]
    usage_error: Option<String>,
    #[rust]
    disk_snapshot: Arc<Snapshot>,
    #[rust]
    disk_error: Option<String>,
    #[rust]
    disk_paths: Vec<PathBuf>,
    #[rust]
    ai_port: Option<AiServicePort>,
    #[rust]
    ai_context: String,
    #[rust]
    activity: VecDeque<String>,
    #[rust]
    quit_timer: Option<Timer>,
    #[rust]
    document_worker: Option<DocumentWorker>,
    #[rust]
    documents: DocumentRegistry,
    #[rust]
    code_tabs: HashMap<u64, PathBuf>,
    #[rust]
    code_errors: HashMap<u64, String>,
    #[rust]
    activity_worker: Option<ActivityWorker>,
    #[rust]
    activity_snapshot: Arc<ActivitySnapshot>,
    #[rust]
    seen_files: HashMap<PathBuf, u64>,
    #[rust]
    designs: Vec<Design>,
    #[rust]
    command_cards: VecDeque<Card>,
    #[rust]
    event_seq: u64,
    #[rust]
    active_item: Option<u64>,
    #[rust]
    workspace_timer: Option<Timer>,
    #[rust]
    workspace_dirty: bool,
    #[rust]
    file_toolbar_open: bool,
    #[rust]
    focus_file_path: bool,
    #[rust]
    utility_return_focus: Option<Area>,
    #[rust]
    project_tree_ref: WidgetRef,
}

impl App {
    fn hosted(cx: &Cx) -> bool {
        cx.in_makepad_studio()
    }

    fn state_dir(&self) -> PathBuf {
        self.args.state_dir()
    }

    /// Settings and monitoring are window utilities, never workspace cards.
    /// Their Dock owns separate widgets without changing the agent selection.
    fn show_utility(&mut self, cx: &mut Cx, tab_id: LiveId, kind: LiveId, title: &str) {
        self.close_utility_popups(cx);
        let dock = self.ui.dock(cx, ids!(utility_dock));
        if dock.item(tab_id).is_empty() {
            if dock
                .create_and_select_tab(
                    cx,
                    id!(root),
                    tab_id,
                    kind,
                    title.to_owned(),
                    id!(CloseableTab),
                    None,
                )
                .is_none()
            {
                log!("studio: could not open utility {title}");
                return;
            }
        } else {
            dock.select_tab(cx, tab_id);
        }
        self.ui.label(cx, ids!(utility_title)).set_text(cx, title);
        self.refresh_utility_layout(cx);
        let modal = self.ui.modal(cx, ids!(utility_overlay));
        if !modal.is_open() {
            self.utility_return_focus = Some(cx.key_focus());
            modal.open(cx);
        }
        self.focus_file_path = false;
    }

    fn close_utility_popups(&self, cx: &mut Cx) {
        // Hidden modal children stop receiving focus events, so release any
        // open popup's sweep lock before dismissing their owning panel.
        for id in [id!(style_picker), id!(disk_path)] {
            if let Some(mut picker) = self.ui.drop_down(cx, &[id]).borrow_mut() {
                picker.set_closed(cx);
            }
        }
    }

    fn close_utility(&mut self, cx: &mut Cx) {
        self.close_utility_popups(cx);
        self.ui.modal(cx, ids!(utility_overlay)).close(cx);
        if let Some(focus) = self.utility_return_focus.take() {
            cx.set_key_focus(focus);
        }
    }

    fn refresh_utility_layout(&self, cx: &mut Cx) {
        let size = self.ui.window(cx, ids!(main_window)).get_inner_size(cx);
        if size.x <= 0.0 || size.y <= 0.0 {
            return;
        }
        let width = Size::Fixed((size.x - 32.0).clamp(1.0, 640.0));
        let height = Size::Fixed((size.y - 72.0).clamp(1.0, 520.0));
        let content = self.ui.view(cx, ids!(utility_overlay.content));
        if let Some(mut content) = content.borrow_mut() {
            if content.walk.width != width || content.walk.height != height {
                content.walk.width = width;
                content.walk.height = height;
                content.redraw(cx);
            }
        };
    }

    /// Keep the compact toolbar honest after selection, document and style
    /// changes. The collapsed file row never consumes workspace height.
    fn refresh_toolbar(&self, cx: &mut Cx) {
        self.ui
            .view(cx, ids!(file_toolbar))
            .set_visible(cx, self.file_toolbar_open);
        let document = self
            .active_item
            .and_then(|id| self.code_document(cx, id).ok());
        let dirty = document.as_ref().is_some_and(|d| d.is_dirty());
        let conflict = document.as_ref().is_some_and(|d| d.has_conflict());
        let no_path = self
            .ui
            .text_input(cx, ids!(file_path))
            .text()
            .trim()
            .is_empty();
        for (id, disabled) in [
            (id!(save_code), !dirty || conflict),
            (id!(discard_code), !dirty && !conflict),
            (id!(open_code), no_path),
        ] {
            let button = self.ui.button(cx, &[id]);
            button.set_disabled(cx, disabled);
            if let Some(mut inner) = button.borrow_mut() {
                let opacity = if disabled { 0.35 } else { 1.0 };
                if inner.draw_icon.opacity != opacity {
                    inner.draw_icon.opacity = opacity;
                    inner.redraw(cx);
                }
            };
        }
        self.ui.tip(cx, ids!(save_code_tip)).set_text(if conflict {
            "Save unavailable: the file changed on disk while you were editing"
        } else if dirty {
            "Save the active code editor"
        } else {
            "Save · select a code editor with unsaved changes"
        });
    }

    fn set_file_toolbar_open(&mut self, cx: &mut Cx, open: bool) {
        self.file_toolbar_open = open;
        self.focus_file_path = open;
        if !open {
            let area = self.ui.text_input(cx, ids!(file_path)).area();
            if cx.key_focus() == area {
                cx.set_key_focus(Area::Empty);
            }
        }
        self.refresh_toolbar(cx);
    }

    /// New work belongs to the active work pane, with a separate pane restored
    /// if the user closed every terminal/editor and only Project remains.
    fn tabs_container(&self, cx: &mut Cx) -> Option<LiveId> {
        self.work_tabs_container(cx)
    }

    fn set_terminal_cwd(&self, cx: &mut Cx, tab: &WidgetRef) {
        let Some(cwd) = self.args.cwd.clone() else {
            return;
        };
        if let Some(mut term) = tab.widget(cx, ids!(term)).borrow_mut::<MpTerm>() {
            if term.cwd.is_none() {
                term.cwd = Some(cwd);
            }
        }
    }

    fn open_terminal(&mut self, cx: &mut Cx, cwd: Option<PathBuf>) -> Result<String, String> {
        let parent = self
            .tabs_container(cx)
            .ok_or("No tab container is available")?;
        let dock = self.ui.dock(cx, ids!(dock));
        self.terminal_seq += 1;
        let tab_id = dock.unique_id(LiveId::from_str_num("studio_terminal", self.terminal_seq).0);
        if let Some(tab) = dock.create_and_select_tab(
            cx,
            parent,
            tab_id,
            id!(TerminalTab),
            "Terminal".to_string(),
            id!(CloseableTab),
            None,
        ) {
            self.set_terminal_cwd(cx, &tab);
            if let Some(cwd) = cwd {
                if let Some(mut term) = tab.widget(cx, ids!(term)).borrow_mut::<MpTerm>() {
                    term.cwd = Some(cwd);
                }
            }
        } else {
            return Err("Could not create terminal tab".into());
        }
        self.save_dock(cx);
        self.active_item = Some(tab_id.0);
        self.refresh_workspace(cx);
        self.focus_canvas_item(cx, tab_id.0);
        Ok(format!("{:x}", tab_id.0))
    }

    fn open_settings(&mut self, cx: &mut Cx) {
        self.show_utility(cx, id!(settings_tab), id!(SettingsTab), "Settings");
        self.refresh_settings_panel(cx);
    }

    /// The dock tab whose terminal widget emitted an action.
    fn tab_of_terminal(&self, cx: &mut Cx, term_uid: WidgetUid) -> Option<LiveId> {
        let dock = self.ui.dock(cx, ids!(dock));
        let mut dock = dock.borrow_mut()?;
        let mut found = None;
        for (tab_id, (_, widget)) in dock.items().iter() {
            if widget.widget(cx, ids!(term)).widget_uid() == term_uid {
                found = Some(*tab_id);
                break;
            }
        }
        found
    }

    fn save_dock(&self, cx: &mut Cx) {
        let Some(items) = self.ui.dock(cx, ids!(dock)).clone_state() else {
            return;
        };
        if let Err(err) = state::save_dock(&self.state_dir(), &items) {
            log!("studio: could not save dock layout: {err}");
        }
    }

    fn restore_dock(&mut self, cx: &mut Cx) {
        let Some(mut items) = state::load_dock(&self.state_dir()) else {
            return;
        };
        if !state::dock_state_is_usable(&items, &TAB_KINDS) {
            if let Some(rooted) = state::recover_rooted_dock(&items, &TAB_KINDS) {
                log!(
                    "studio: recovered saved Dock layout without {} detached entries",
                    items.len() - rooted.len()
                );
                items = rooted;
            } else {
                log!("studio: saved dock layout is not usable by this build; using the default");
                return;
            }
        }
        let items: HashMap<LiveId, DockItem> = items;
        self.ui.dock(cx, ids!(dock)).load_state(cx, items);
    }

    /// Standalone: install the chosen stylesheet and reapply Splash; the
    /// terminal sessions, dock layout and picker state ride through
    /// `Apply::ScriptReapply`. Hosted: the WM's sheet is authoritative.
    fn apply_style_choice(&mut self, cx: &mut Cx) {
        if Self::hosted(cx) {
            return;
        }
        let host = appearance::host_family(cx);
        let choice = StyleChoice::from_settings(&self.settings, host, appearance::host_dark());
        let changed = cx.with_vm(|vm| {
            if desktop_style::current_name(vm).as_deref() == Some(choice.name().as_str()) {
                return false;
            }
            desktop_style::install(vm, choice.sheet());
            true
        });
        if changed {
            log!("studio: appearance resolved to {}", choice.name());
            cx.request_style_reload();
        }
    }

    fn current_style_name(cx: &mut Cx) -> Option<String> {
        cx.with_vm(|vm| desktop_style::current_name(vm))
    }

    fn refresh_status(&self, cx: &mut Cx) {
        let mode = if Self::hosted(cx) {
            "Hosted by the window manager"
        } else {
            "Standalone"
        };
        self.ui.label(cx, ids!(status_mode)).set_text(cx, mode);
        let style = Self::current_style_name(cx)
            .map(|n| appearance::describe(&n))
            .unwrap_or_else(|| "stock theme".to_string());
        self.ui
            .label(cx, ids!(status_style))
            .set_text(cx, &format!("Style: {style}"));
        self.ui
            .label(cx, ids!(status_state))
            .set_text(cx, self.activity.back().map(String::as_str).unwrap_or(""));
    }

    /// Push settings into the Settings tab (when it exists).
    fn refresh_settings_panel(&self, cx: &mut Cx) {
        let picker = self.ui.drop_down(cx, ids!(style_picker));
        if picker.is_empty() {
            return;
        }
        let hosted = Self::hosted(cx);
        picker.set_selected_item(cx, appearance::picker_index(&self.settings));
        picker.set_disabled(cx, hosted);
        let toggle = self.ui.check_box(cx, ids!(dark_toggle));
        let host_dark = appearance::host_dark();
        let follows_appearance =
            appearance::picker_index(&self.settings) == 0 && host_dark.is_some();
        let choice =
            StyleChoice::from_settings(&self.settings, appearance::host_family(cx), host_dark);
        let family = choice.family;
        toggle.set_active(cx, choice.dark, Animate::No);
        toggle.set_disabled(cx, hosted || follows_appearance || !family.supports_dark());
        let note = if hosted {
            "Hosted: the window manager's style picker rules; this one is read-only.".to_string()
        } else if follows_appearance {
            format!(
                "Following {} appearance. Choose a style to set light or dark manually.",
                family.label()
            )
        } else if family.supports_dark() {
            format!(
                "Standalone. {} has light and dark variants.",
                family.label()
            )
        } else {
            format!("Standalone. {} has one appearance.", family.label())
        };
        self.ui.label(cx, ids!(style_note)).set_text(cx, &note);
        self.ui
            .label(cx, ids!(state_dir_label))
            .set_text(cx, &self.state_dir().display().to_string());
    }
}

impl App {
    fn begin_quit(&mut self, cx: &mut Cx) {
        if let Some((id, _)) = self.code_tabs.iter().find(|(_, path)| {
            self.documents
                .get(path)
                .is_some_and(|doc| doc.is_dirty() || doc.has_conflict())
        }) {
            let id = *id;
            self.close_utility(cx);
            self.active_item = Some(id);
            self.ui.dock(cx, ids!(dock)).select_tab(cx, LiveId(id));
            self.focus_canvas_item(cx, id);
            self.ui.label(cx, ids!(status_state)).set_text(
                cx,
                "Save or discard the open code edits before closing Studio",
            );
            log!("studio: close deferred because an editor has unsaved changes");
            return;
        }
        self.save_workspace(cx);
        if let Some(tree) = self.project_tree_ref.borrow::<StudioProjectTree>() {
            tree.request_stop();
        }
        if let Some(worker) = &self.disk_worker {
            worker.request_stop();
        }
        if let Some(worker) = &self.document_worker {
            worker.request_stop();
        }
        if let Some(worker) = &self.activity_worker {
            worker.request_stop();
        }
        if let Some(worker) = &self.usage_worker {
            worker.request_stop();
        }
        if !self.workers_finished() {
            if self.quit_timer.is_none() {
                self.quit_timer = Some(cx.start_interval(0.05));
            }
        } else {
            self.disk_worker = None;
            self.document_worker = None;
            self.activity_worker = None;
            self.usage_worker = None;
            cx.quit();
        }
    }

    fn start_services(&mut self, cx: &mut Cx) {
        self.start_project_tree(cx);
        if self.ai_port.is_none() {
            self.ai_port = AiServicePort::open(cx, ai::manifest());
        }
        if self.disk_worker.is_none() && self.disk_error.is_none() {
            let cwd =
                self.args.cwd.clone().unwrap_or_else(|| {
                    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                });
            match DiskWorker::start(&cx.thread_spawner(), cwd) {
                Ok(worker) => self.disk_worker = Some(worker),
                Err(error) => {
                    log!("studio disk worker: {error}");
                    self.disk_error = Some(error);
                }
            }
        }
        if self.document_worker.is_none() {
            match DocumentWorker::start(&cx.thread_spawner()) {
                Ok(mut worker) => {
                    for path in self.code_tabs.values() {
                        let _ = worker.watch(path.clone());
                    }
                    self.document_worker = Some(worker);
                }
                Err(error) => log!("studio documents: {error}"),
            }
        }
        if self.activity_worker.is_none() {
            let cwd = self.project_dir();
            match ActivityWorker::start(&cx.thread_spawner(), cwd, std::process::id()) {
                Ok(worker) => self.activity_worker = Some(worker),
                Err(error) => log!("studio activity: {error}"),
            }
        }
        if self.usage_worker.is_none() && self.usage_error.is_none() {
            match UsageWorker::start(&cx.thread_spawner()) {
                Ok(worker) => self.usage_worker = Some(worker),
                Err(error) => self.usage_error = Some(error),
            }
        }
        self.refresh_usage_panel(cx);
        self.refresh_ai_context(cx);
    }

    fn open_disk(&mut self, cx: &mut Cx) {
        self.show_utility(cx, id!(disk_tab), id!(DiskTab), "Disk & workspaces");
        self.refresh_disk_panel(cx);
    }

    fn refresh_disk_panel(&mut self, cx: &mut Cx) {
        let summary = if let Some(error) = &self.disk_error {
            format!("Disk unavailable: {error}")
        } else if self.disk_snapshot.volume_at == 0 {
            "Measuring disk…".into()
        } else {
            self.disk_snapshot.summary()
        };
        let width = self.ui.window(cx, ids!(main_window)).get_inner_size(cx).x;
        let summary = if width < 900.0 {
            self.disk_snapshot
                .volume
                .as_ref()
                .map(|v| {
                    format!(
                        "Disk {:.0}% · {:.0}G free",
                        100.0 * v.used as f64 / v.total.max(1) as f64,
                        v.available as f64 / 1_073_741_824.0
                    )
                })
                .unwrap_or(summary)
        } else {
            summary
        };
        self.ui.button(cx, ids!(disk_open)).set_text(cx, &summary);
        if let Some(mut graph) = self
            .ui
            .widget(cx, ids!(disk_graph))
            .borrow_mut::<DiskGraph>()
        {
            graph.update(cx, self.disk_snapshot.clone());
        }
        self.ui
            .label(cx, ids!(disk_report))
            .set_text(cx, &self.disk_snapshot.report());
        let status = if self.disk_snapshot.scanning {
            "Scanning…".into()
        } else if self.disk_snapshot.scanned_at == 0 {
            "Waiting for inventory".into()
        } else {
            format!(
                "Updated {}s ago",
                disk::now().saturating_sub(self.disk_snapshot.scanned_at)
            )
        };
        self.ui
            .label(cx, ids!(disk_scan_status))
            .set_text(cx, &status);
        let picker = self.ui.drop_down(cx, ids!(disk_path));
        let selected = self.disk_paths.get(picker.selected_item()).cloned();
        let paths: Vec<_> = self
            .disk_snapshot
            .entries
            .iter()
            .map(|e| e.path.clone())
            .collect();
        if !picker.is_empty() {
            picker.set_labels(
                cx,
                self.disk_snapshot
                    .entries
                    .iter()
                    .map(|e| format!("{} · {}", disk::size(e.usage.apparent), e.path.display()))
                    .collect(),
            );
            picker.set_selected_item(
                cx,
                selected
                    .and_then(|p| paths.iter().position(|q| *q == p))
                    .unwrap_or(0),
            );
        }
        self.disk_paths = paths;
    }

    fn terminal(&self, cx: &mut Cx, id: u64) -> Result<WidgetRef, String> {
        let dock = self.ui.dock(cx, ids!(dock));
        let items = dock.clone_state().ok_or("Dock unavailable")?;
        if !matches!(items.get(&LiveId(id)), Some(DockItem::Tab { kind, .. }) if *kind == id!(TerminalTab))
        {
            return Err("No terminal with this tab ID".into());
        }
        Ok(dock.item(LiveId(id)).widget(cx, ids!(term)))
    }

    fn status_json(&self, cx: &mut Cx) -> Value {
        let items = self
            .ui
            .dock(cx, ids!(dock))
            .clone_state()
            .unwrap_or_default();
        let mut tabs: Vec<_> = items.iter().filter_map(|(id, item)| {
            let DockItem::Tab { name, kind, .. } = item else { return None; };
            let active = items.values().any(|item| matches!(item, DockItem::Tabs { tabs, selected, .. } if tabs.get(*selected) == Some(id)));
            let kind = if *kind == id!(TerminalTab) { "terminal" } else if *kind == id!(SettingsTab) { "settings" } else if *kind == id!(CodeTab) { "code" } else if *kind == id!(DesignTab) { "design" } else if *kind == id!(ActivityTab) { "activity" } else if *kind == id!(UsageTab) { "usage" } else if *kind == id!(ProjectTreeTab) { "project" } else { "disk" };
            Some((*id, json::obj(vec![("id", json::s(format!("{:x}", id.0))), ("name", json::s(name)), ("kind", json::s(kind)), ("selected", Value::Bool(active))])))
        }).collect();
        tabs.sort_by_key(|(id, _)| id.0);
        json::obj(vec![
            (
                "tabs",
                Value::Arr(tabs.into_iter().map(|(_, t)| t).collect()),
            ),
            (
                "style",
                json::s(Self::current_style_name(cx).unwrap_or_default()),
            ),
            ("hosted", Value::Bool(Self::hosted(cx))),
            ("layout", json::s(state::encode_dock(&items))),
            (
                "project_tree",
                json::s(
                    self.project_tree_ref
                        .borrow::<StudioProjectTree>()
                        .map(|tree| tree.status())
                        .unwrap_or_default(),
                ),
            ),
            ("disk", json::s(self.disk_snapshot.summary())),
            ("usage", self.usage_json()),
            (
                "recent_activity",
                Value::Arr(self.activity.iter().map(json::s).collect()),
            ),
            ("workspace", self.workspace_json(cx)),
        ])
    }

    fn ui_action(&mut self, cx: &mut Cx, action: Action) {
        if let Err(error) = self.dispatch(cx, action, "UI") {
            log!("studio action: {error}");
            self.ui.label(cx, ids!(status_state)).set_text(cx, &error);
        }
    }

    fn dispatch(&mut self, cx: &mut Cx, action: Action, actor: &str) -> Result<String, String> {
        // Keep activity labels bounded and avoid copying commands or paths
        // into the status strip. Their contents remain in the live terminal.
        let label = match &action {
            Action::Usage => "Open usage",
            Action::RefreshUsage => "Refresh usage",
            Action::InspectUsage => "Inspect usage",
            Action::Status => "Read status",
            Action::NewTerminal { .. } => "Open terminal",
            Action::SelectTab(_) => "Select tab",
            Action::CloseTab(_) => "Close tab",
            Action::Settings => "Open settings",
            Action::Activity => "Open activity",
            Action::Disk => "Inspect disk",
            Action::Appearance { .. } => "Change appearance",
            Action::ReadTerminal { .. } => "Read terminal",
            Action::Input { .. } => "Type in terminal",
            Action::Run { .. } => "Run terminal command",
            Action::RefreshDisk => "Refresh disk",
            Action::InspectDisk => "Read disk inventory",
            Action::CleanupPreview(_) => "Preview cleanup",
            Action::Layout(_) => "Arrange tabs",
            Action::DockTab { .. } => "Dock tab",
            Action::RefreshProject => "Refresh project files",
            Action::RevealProject(_) => "Reveal project file",
            Action::Mode(_) => "Switch workspace mode",
            Action::CanvasLayout(_) => "Change canvas layout",
            Action::CanvasFit => "Fit canvas",
            Action::CanvasZoom(_) => "Zoom canvas",
            Action::MoveCard { .. } => "Move canvas card",
            Action::ResizeCard { .. } => "Resize canvas card",
            Action::FocusCard(_) => "Focus canvas card",
            Action::OpenCode(_) => "Open code",
            Action::ReadCode { .. } => "Read code",
            Action::SaveCode { .. } => "Save code",
            Action::ReloadCode { .. } => "Discard local edits and reload",
            Action::AddDesign { .. } => "Add system design",
        };
        let run = if let Action::Run { tab, command } = &action {
            Some((*tab, command.clone()))
        } else {
            None
        };
        let result = self.execute(cx, action);
        if let Some((tab, command)) = run {
            self.event_seq += 1;
            let mut card = Card::new(
                LiveId::from_str_num("studio-command", self.event_seq).0,
                CardKind::Run,
                if result.is_ok() {
                    "Command submitted"
                } else {
                    "Command refused"
                },
            );
            card.parent = Some(tab);
            card.detail = format!(
                "{actor}\n{command}\n\n{}",
                result
                    .as_ref()
                    .map(String::as_str)
                    .unwrap_or_else(|e| e.as_str())
            );
            self.command_cards.push_back(card);
            if self.command_cards.len() > 24 {
                self.command_cards.pop_front();
            }
        }
        self.activity.push_back(format!(
            "{actor}: {label}{}",
            if result.is_err() { " (failed)" } else { "" }
        ));
        if self.activity.len() > 32 {
            self.activity.pop_front();
        }
        self.refresh_status(cx);
        self.refresh_workspace(cx);
        self.refresh_ai_context(cx);
        result
    }

    fn execute(&mut self, cx: &mut Cx, action: Action) -> Result<String, String> {
        if matches!(
            &action,
            Action::NewTerminal { .. }
                | Action::OpenCode(_)
                | Action::SelectTab(_)
                | Action::FocusCard(_)
                | Action::Mode(_)
                | Action::DockTab { .. }
                | Action::RevealProject(_)
        ) {
            self.close_utility(cx);
        }
        match action {
            Action::Status => Ok(self.status_json(cx).to_json()),
            Action::Usage => {
                self.open_usage(cx);
                Ok(self.usage_json().to_json())
            }
            Action::InspectUsage => Ok(self.usage_json().to_json()),
            Action::RefreshUsage => {
                self.usage_worker
                    .as_mut()
                    .ok_or_else(|| {
                        self.usage_error
                            .clone()
                            .unwrap_or_else(|| "Usage worker unavailable".into())
                    })?
                    .refresh();
                Ok("Background usage refresh queued".into())
            }
            Action::Mode(mode) => {
                if let Some(mut surface) = self
                    .ui
                    .widget(cx, ids!(workspace))
                    .borrow_mut::<StudioSurface>()
                {
                    surface.set_mode(cx, mode);
                }
                Ok(format!("{} mode", mode.as_str()))
            }
            Action::CanvasLayout(layout) => {
                if let Some(mut surface) = self
                    .ui
                    .widget(cx, ids!(workspace))
                    .borrow_mut::<StudioSurface>()
                {
                    surface.set_layout(cx, layout);
                }
                Ok(format!(
                    "{} layout; both arrangements retained",
                    layout.as_str()
                ))
            }
            Action::CanvasFit => {
                if let Some(mut surface) = self
                    .ui
                    .widget(cx, ids!(workspace))
                    .borrow_mut::<StudioSurface>()
                {
                    surface.fit(cx);
                }
                Ok("Canvas fitted".into())
            }
            Action::CanvasZoom(factor) => {
                if let Some(mut surface) = self
                    .ui
                    .widget(cx, ids!(workspace))
                    .borrow_mut::<StudioSurface>()
                {
                    surface.zoom_by(cx, factor);
                }
                Ok("Canvas zoom updated".into())
            }
            Action::MoveCard { id, x, y } => {
                self.ui
                    .widget(cx, ids!(workspace))
                    .borrow_mut::<StudioSurface>()
                    .ok_or("Canvas unavailable")?
                    .move_card(cx, id, x, y)?;
                Ok("Card moved in Free layout".into())
            }
            Action::ResizeCard { id, width, height } => {
                self.ui
                    .widget(cx, ids!(workspace))
                    .borrow_mut::<StudioSurface>()
                    .ok_or("Canvas unavailable")?
                    .resize_card(cx, id, width, height)?;
                Ok("Card resized; live content retained".into())
            }
            Action::FocusCard(id) => {
                if !self
                    .ui
                    .widget(cx, ids!(workspace))
                    .borrow::<StudioSurface>()
                    .is_some_and(|s| s.workspace.geometry(id).is_some())
                {
                    return Err("Unknown card ID".into());
                }
                self.active_item = Some(id);
                self.focus_canvas_item(cx, id);
                Ok("Card focused".into())
            }
            Action::OpenCode(path) => self.open_code(cx, path, true),
            Action::ReadCode { tab } => {
                let doc = self.code_document(cx, tab)?;
                Ok(json::obj(vec![
                    ("path", json::s(doc.path().display().to_string())),
                    ("text", json::s(doc.current_text())),
                    ("revision", Value::Int(doc.revision() as i64)),
                    ("status", json::s(doc.status())),
                    ("dirty", Value::Bool(doc.is_dirty())),
                    ("conflict", Value::Bool(doc.has_conflict())),
                ])
                .to_json())
            }
            Action::SaveCode { tab } => self.save_code(cx, tab),
            Action::ReloadCode { tab } => {
                let doc = self.code_document(cx, tab)?;
                doc.discard_local_and_reload()?;
                self.ui.dock(cx, ids!(dock)).item(LiveId(tab)).redraw(cx);
                Ok("Local edits discarded; latest observed disk revision loaded".into())
            }
            Action::AddDesign {
                title,
                detail,
                parent,
                path,
            } => {
                if self.designs.len() >= 64 {
                    return Err("At most 64 design cards".into());
                }
                if parent.is_some_and(|p| {
                    !self
                        .ui
                        .widget(cx, ids!(workspace))
                        .borrow::<StudioSurface>()
                        .is_some_and(|s| s.workspace.geometry(p).is_some())
                }) {
                    return Err("Unknown parent card".into());
                }
                self.event_seq += 1;
                let dock = self.ui.dock(cx, ids!(dock));
                let id = dock.unique_id(LiveId::from_str_num("studio-design", self.event_seq).0);
                let container = self.tabs_container(cx).ok_or("No tab container")?;
                dock.create_and_select_tab(
                    cx,
                    container,
                    id,
                    id!(DesignTab),
                    title.clone(),
                    id!(CloseableTab),
                    None,
                )
                .ok_or("Could not create design card")?;
                self.designs.push(Design {
                    id: id.0,
                    title,
                    detail,
                    parent,
                    path: path.map(|p| p.display().to_string()),
                });
                self.active_item = Some(id.0);
                self.workspace_dirty = true;
                self.save_dock(cx);
                self.refresh_workspace(cx);
                self.focus_canvas_item(cx, id.0);
                Ok(format!("{:x}", id.0))
            }
            Action::NewTerminal { cwd } => self.open_terminal(cx, cwd),
            Action::SelectTab(id) | Action::CloseTab(id) => {
                // The variant is preserved below using the action value.
                let dock = self.ui.dock(cx, ids!(dock));
                if !matches!(
                    dock.clone_state()
                        .as_ref()
                        .and_then(|items| items.get(&LiveId(id))),
                    Some(DockItem::Tab { .. })
                ) {
                    return Err("No tab with this ID".into());
                }
                if matches!(action, Action::CloseTab(_)) {
                    self.close_item(cx, id)?;
                } else {
                    dock.select_tab(cx, LiveId(id));
                    self.active_item = Some(id);
                    self.focus_canvas_item(cx, id);
                }
                self.save_dock(cx);
                Ok("Tab updated".into())
            }
            Action::DockTab {
                tab,
                target,
                position,
            } => {
                let dock = self.ui.dock(cx, ids!(dock));
                if !dock.move_tab(cx, LiveId(tab), LiveId(target), position) {
                    return Err(
                        "Dock move requires two different existing tabs and a changed placement"
                            .into(),
                    );
                }
                if tab != id!(project_tree_tab).0 {
                    self.active_item = Some(tab);
                }
                self.save_dock(cx);
                Ok("Structured dock arrangement updated; live views retained".into())
            }
            Action::RefreshProject => {
                self.project_tree_ref
                    .borrow_mut::<StudioProjectTree>()
                    .ok_or("Project tree unavailable")?
                    .refresh(cx);
                Ok("Project refresh queued".into())
            }
            Action::RevealProject(path) => {
                self.project_tree_ref
                    .borrow_mut::<StudioProjectTree>()
                    .ok_or("Project tree unavailable")?
                    .reveal(cx, &path)?;
                if let Some(mut surface) = self
                    .ui
                    .widget(cx, ids!(workspace))
                    .borrow_mut::<StudioSurface>()
                {
                    surface.set_mode(cx, Mode::Structured);
                }
                self.ui
                    .dock(cx, ids!(dock))
                    .select_tab(cx, id!(project_tree_tab));
                Ok("Project tree reveal queued".into())
            }
            Action::Settings => {
                self.open_settings(cx);
                Ok("Settings opened".into())
            }
            Action::Activity => {
                self.open_activity(cx);
                Ok("Activity opened".into())
            }
            Action::Disk => {
                self.open_disk(cx);
                Ok(self.disk_snapshot.report())
            }
            Action::Appearance { style, dark } => {
                if Self::hosted(cx) {
                    return Err("The window manager controls hosted appearance".into());
                }
                self.settings.style = style;
                self.settings.dark = dark;
                self.apply_style_choice(cx);
                if let Err(err) = self.settings.save(&self.state_dir()) {
                    log!("studio: could not save settings: {err}");
                }
                self.refresh_settings_panel(cx);
                Ok("Appearance applied".into())
            }
            Action::ReadTerminal { tab, lines } => {
                let widget = self.terminal(cx, tab)?;
                let term = widget.borrow::<MpTerm>().ok_or("Terminal unavailable")?;
                let (rows, row, col) = term
                    .ai_screen_rows(lines)
                    .ok_or("Terminal session is not ready")?;
                Ok(json::obj(vec![
                    ("text", json::s(rows.join("\n"))),
                    ("cursor_row", Value::Int(row as i64)),
                    ("cursor_column", Value::Int(col as i64)),
                    (
                        "cwd",
                        term.cwd
                            .as_ref()
                            .map(|p| json::s(p.display().to_string()))
                            .unwrap_or(Value::Null),
                    ),
                ])
                .to_json())
            }
            Action::Input { tab, text } => self.type_terminal(cx, tab, text.as_bytes()),
            Action::Run { tab, command } => {
                self.type_terminal(cx, tab, format!("{command}\n").as_bytes())
            }
            Action::RefreshDisk => {
                self.disk_worker
                    .as_mut()
                    .ok_or("Disk worker unavailable")?
                    .refresh();
                self.ui
                    .label(cx, ids!(disk_scan_status))
                    .set_text(cx, "Refresh queued");
                Ok(
                    "Background refresh queued; inspect_disk reports completion and scan age"
                        .into(),
                )
            }
            Action::InspectDisk => Ok(json::obj(vec![
                ("inventory", json::s(self.disk_snapshot.report())),
                (
                    "history",
                    Value::Arr(
                        self.disk_snapshot
                            .history
                            .iter()
                            .map(|(time, used)| {
                                json::obj(vec![
                                    ("unix_seconds", Value::Int(*time as i64)),
                                    ("used_bytes", Value::Int(*used as i64)),
                                ])
                            })
                            .collect(),
                    ),
                ),
            ])
            .to_json()),
            Action::CleanupPreview(path) => self.disk_snapshot.cleanup_preview(&path),
            Action::Layout(text) => {
                let items = state::decode_dock(&text).ok_or("Invalid layout document")?;
                if !state::dock_state_is_usable(&items, &TAB_KINDS) {
                    return Err(
                        "Layout must be a rooted tree of supported tabs with valid selections"
                            .into(),
                    );
                }
                let dock = self.ui.dock(cx, ids!(dock));
                let current = dock.clone_state().ok_or("Dock unavailable")?;
                let tabs = |items: &HashMap<LiveId, DockItem>| {
                    let mut tabs: Vec<_> = items
                        .iter()
                        .filter_map(|(id, item)| match item {
                            DockItem::Tab { .. } => Some((id.0, item.serialize_ron())),
                            _ => None,
                        })
                        .collect();
                    tabs.sort();
                    tabs
                };
                if tabs(&items) != tabs(&current) {
                    return Err(
                        "Layout changes must retain every current tab and its properties".into(),
                    );
                }
                dock.load_state_preserving_items(cx, items);
                self.save_dock(cx);
                Ok("Layout updated; terminal sessions retained".into())
            }
        }
    }

    fn type_terminal(&self, cx: &mut Cx, tab: u64, bytes: &[u8]) -> Result<String, String> {
        let widget = self.terminal(cx, tab)?;
        let mut term = widget
            .borrow_mut::<MpTerm>()
            .ok_or("Terminal unavailable")?;
        if term.ai_type_bytes(bytes) {
            Ok("Input accepted by the live terminal; execution may still be running".into())
        } else {
            Err("Terminal input is not ready; nothing accepted".into())
        }
    }

    fn refresh_ai_context(&mut self, cx: &mut Cx) {
        let count = self
            .ui
            .dock(cx, ids!(dock))
            .clone_state()
            .map(|s| {
                s.values()
                    .filter(|i| matches!(i, DockItem::Tab { .. }))
                    .count()
            })
            .unwrap_or(0);
        let context = format!(
            "{count} tabs; {}; {}",
            self.disk_snapshot.summary(),
            self.activity.back().map(String::as_str).unwrap_or("Ready")
        );
        if context != self.ai_context {
            if let Some(port) = &self.ai_port {
                port.set_context(&context);
            }
            self.ai_context = context;
        }
    }

    fn drain_services(&mut self, cx: &mut Cx, event: &Event) {
        if let Some(snapshot) = self.disk_worker.as_mut().and_then(DiskWorker::poll) {
            self.disk_snapshot = snapshot;
            self.refresh_disk_panel(cx);
            self.refresh_usage_panel(cx);
            self.refresh_ai_context(cx);
        }
        let events = self
            .ai_port
            .as_mut()
            .map(|p| p.handle_event(cx, event))
            .unwrap_or_default();
        for event in events {
            match event {
                PortEvent::Registered(endpoint) => {
                    log!("studio: AI service registered as {}", endpoint.as_str());
                    self.ai_context.clear();
                    self.refresh_ai_context(cx);
                }
                PortEvent::Call(call) => {
                    let result = match ai::parse(&call)
                        .and_then(|action| self.dispatch(cx, action, "Assistant"))
                    {
                        Ok(text) => ToolResult::ok(&call.call_id, text, "Studio action completed"),
                        Err(error) => ToolResult::refused(&call.call_id, error),
                    };
                    if let Some(port) = &self.ai_port {
                        port.reply(result);
                    }
                }
                _ => {}
            }
        }
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.args = Args::from_env();
        if let Some((w, h)) = self.args.window_size {
            self.ui
                .window(cx, ids!(main_window))
                .resize(cx, dvec2(w as f64, h as f64));
        }
        self.settings = Settings::load(&self.state_dir());
        makepad_wm_api::set_title(cx, "Studio");
        self.restore_dock(cx);
        self.restore_items(cx);
        self.ensure_project_tree(cx);
        if let Some(mut surface) = self
            .ui
            .widget(cx, ids!(workspace))
            .borrow_mut::<StudioSurface>()
        {
            surface.set_workspace(cx, Workspace::load(&self.state_dir()));
        }
        self.materialize_tabs(cx);
        self.workspace_timer = Some(cx.start_interval(0.5));
        // Every terminal tab that exists now (default or restored) opens in
        // the requested directory.
        let tabs: Vec<WidgetRef> = self
            .ui
            .dock(cx, ids!(dock))
            .borrow_mut()
            .map(|mut dock| dock.items().iter().map(|(_, (_, w))| w.clone()).collect())
            .unwrap_or_default();
        for tab in tabs {
            self.set_terminal_cwd(cx, &tab);
        }
        self.refresh_settings_panel(cx);
        self.refresh_status(cx);
        if !makepad_wm_api::warm_start() {
            self.start_services(cx);
        }
        self.refresh_workspace(cx);
        if !self.state_dir().join("canvas.ron").exists() {
            let terminal = self
                .ui
                .dock(cx, ids!(dock))
                .clone_state()
                .unwrap_or_default()
                .into_iter()
                .find_map(|(id, item)| {
                    matches!(item,DockItem::Tab{kind,..}if kind==id!(TerminalTab)).then_some(id.0)
                });
            if let Some(id) = terminal {
                self.active_item = Some(id);
                self.focus_canvas_item(cx, id);
            }
        }
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(close_utility)).clicked(actions)
            || self.ui.modal(cx, ids!(utility_overlay)).dismissed(actions)
        {
            self.close_utility(cx);
        }
        if self
            .ui
            .button(cx, ids!(refresh_project_tree))
            .clicked(actions)
        {
            if let Some(mut tree) = self.project_tree_ref.borrow_mut::<StudioProjectTree>() {
                tree.refresh(cx);
            }
        }
        if self
            .ui
            .button(cx, ids!(reveal_project_file))
            .clicked(actions)
        {
            self.reveal_active_project_file(cx);
        }
        if self.ui.button(cx, ids!(new_terminal)).clicked(actions) {
            self.ui_action(cx, Action::NewTerminal { cwd: None });
        }
        if self.ui.button(cx, ids!(open_settings)).clicked(actions) {
            self.ui_action(cx, Action::Settings);
        }
        if let Some(index) = self.ui.drop_down(cx, ids!(style_picker)).changed(actions) {
            if !Self::hosted(cx) {
                self.ui_action(
                    cx,
                    Action::Appearance {
                        style: appearance::family_at(index).map(|f| f.id().to_string()),
                        dark: self.settings.dark,
                    },
                );
            }
        }
        if let Some(dark) = self.ui.check_box(cx, ids!(dark_toggle)).changed(actions) {
            if !Self::hosted(cx) {
                self.ui_action(
                    cx,
                    Action::Appearance {
                        style: self.settings.style.clone(),
                        dark,
                    },
                );
            }
        }
        if self.ui.button(cx, ids!(open_disk)).clicked(actions)
            || self.ui.button(cx, ids!(disk_open)).clicked(actions)
            || self
                .ui
                .view(cx, ids!(disk_corner))
                .finger_up(actions)
                .is_some_and(|e| e.is_over)
        {
            self.ui_action(cx, Action::Disk);
        }
        if self.ui.button(cx, ids!(fable_usage)).clicked(actions)
            || self.ui.button(cx, ids!(astra_usage)).clicked(actions)
        {
            self.ui_action(cx, Action::Usage);
        }
        if self.ui.button(cx, ids!(refresh_usage)).clicked(actions)
            || self
                .ui
                .button(cx, ids!(refresh_usage_status))
                .clicked(actions)
        {
            self.ui_action(cx, Action::RefreshUsage);
        }
        if self.ui.button(cx, ids!(refresh_disk)).clicked(actions) {
            self.ui_action(cx, Action::RefreshDisk);
        }
        if self.ui.button(cx, ids!(preview_cleanup)).clicked(actions) {
            let index = self.ui.drop_down(cx, ids!(disk_path)).selected_item();
            if let Some(path) = self.disk_paths.get(index).cloned() {
                let result = self.dispatch(cx, Action::CleanupPreview(path), "UI");
                self.ui
                    .label(cx, ids!(cleanup_note))
                    .set_text(cx, &result.unwrap_or_else(|e| e));
            }
        }
        if self
            .ui
            .radio_button(cx, ids!(mode_structured))
            .clicked(actions)
        {
            self.ui_action(cx, Action::Mode(Mode::Structured));
        }
        if self.ui.radio_button(cx, ids!(mode_canvas)).clicked(actions) {
            self.ui_action(cx, Action::Mode(Mode::Canvas));
        }
        if let Some(index) = self.ui.drop_down(cx, ids!(layout_picker)).changed(actions) {
            self.ui_action(
                cx,
                Action::CanvasLayout(if index == 0 {
                    LayoutMode::Auto
                } else {
                    LayoutMode::Free
                }),
            );
        }
        if self.ui.button(cx, ids!(fit_canvas)).clicked(actions) {
            self.ui_action(cx, Action::CanvasFit);
        }
        if self.ui.button(cx, ids!(zoom_out)).clicked(actions) {
            self.ui_action(cx, Action::CanvasZoom(0.8));
        }
        if self.ui.button(cx, ids!(zoom_in)).clicked(actions) {
            self.ui_action(cx, Action::CanvasZoom(1.25));
        }
        if self.ui.button(cx, ids!(toggle_code)).clicked(actions) {
            self.set_file_toolbar_open(cx, !self.file_toolbar_open);
        }
        if self
            .ui
            .button(cx, ids!(close_file_toolbar))
            .clicked(actions)
        {
            self.set_file_toolbar_open(cx, false);
        }
        if self.ui.button(cx, ids!(open_code)).clicked(actions)
            || self
                .ui
                .text_input(cx, ids!(file_path))
                .returned(actions)
                .is_some()
        {
            let input = self.ui.text_input(cx, ids!(file_path)).text();
            if !input.trim().is_empty() {
                let path = PathBuf::from(input);
                let path = if path.is_absolute() {
                    path
                } else {
                    self.project_dir().join(path)
                };
                match self.dispatch(cx, Action::OpenCode(path), "UI") {
                    Ok(_) => self.set_file_toolbar_open(cx, false),
                    Err(error) => {
                        log!("studio action: {error}");
                        self.ui.label(cx, ids!(status_state)).set_text(cx, &error);
                    }
                }
            }
        }
        if self.ui.button(cx, ids!(save_code)).clicked(actions) {
            if let Some(tab) = self.active_item {
                self.ui_action(cx, Action::SaveCode { tab });
            }
        }
        if self.ui.button(cx, ids!(discard_code)).clicked(actions) {
            if let Some(tab) = self.active_item {
                self.ui_action(cx, Action::ReloadCode { tab });
            }
        }
        if self.ui.button(cx, ids!(open_activity)).clicked(actions) {
            self.ui_action(cx, Action::Activity);
        }
        for action in actions {
            let Some(wa) = action.as_widget_action() else {
                continue;
            };
            // A utility tab selection never changes the active agent/editor.
            let utility_dock = self.ui.dock(cx, ids!(utility_dock));
            if wa.widget_uid == utility_dock.widget_uid() {
                match wa.cast::<makepad_widgets::dock::DockAction>() {
                    makepad_widgets::dock::DockAction::TabWasPressed(id) => {
                        if let Some(DockItem::Tab { name, .. }) = utility_dock
                            .clone_state()
                            .as_ref()
                            .and_then(|items| items.get(&id))
                        {
                            self.ui.label(cx, ids!(utility_title)).set_text(cx, name);
                        }
                    }
                    makepad_widgets::dock::DockAction::TabCloseWasPressed(id) => {
                        utility_dock.close_tab(cx, id);
                        let state = utility_dock.clone_state().unwrap_or_default();
                        if !state
                            .values()
                            .any(|item| matches!(item, DockItem::Tab { .. }))
                        {
                            self.close_utility(cx);
                        } else if let Some(DockItem::Tabs { tabs, selected, .. }) =
                            state.get(&id!(root))
                        {
                            if let Some(DockItem::Tab { name, .. }) =
                                tabs.get(*selected).and_then(|id| state.get(id))
                            {
                                self.ui.label(cx, ids!(utility_title)).set_text(cx, name);
                            }
                        }
                    }
                    _ => {}
                }
                continue;
            }
            match wa.cast::<CanvasAction>() {
                CanvasAction::Changed => {
                    self.workspace_dirty = true;
                    self.refresh_canvas_controls(cx);
                }
                CanvasAction::Select(id) => {
                    self.active_item = Some(id);
                }
                CanvasAction::Open(id) => {
                    self.active_item = Some(id);
                    if let Some(path) = self
                        .designs
                        .iter()
                        .find(|d| d.id == id)
                        .and_then(|d| d.path.clone())
                    {
                        self.ui_action(cx, Action::OpenCode(PathBuf::from(path)));
                    }
                }
                CanvasAction::Close(id) => self.ui_action(cx, Action::CloseTab(id)),
                CanvasAction::None => {}
            }
            if let StudioCodeEditorAction::Changed(_) = wa.cast::<StudioCodeEditorAction>() {
                self.refresh_workspace(cx);
            }
            if wa.widget_uid == self.ui.dock(cx, ids!(dock)).widget_uid() {
                self.handle_work_dock_action(cx, wa.cast::<DockAction>());
            }
            if let ProjectTreeAction::Open(path) = wa.cast::<ProjectTreeAction>() {
                self.ui_action(cx, Action::OpenCode(path));
            }
            // The shell exited (`exit`, Ctrl-D): its tab goes with it.
            if let MpTermAction::Exited = wa.cast::<MpTermAction>() {
                if let Some(tab_id) = self.tab_of_terminal(cx, wa.widget_uid) {
                    self.ui.dock(cx, ids!(dock)).close_tab(cx, tab_id);
                    self.save_dock(cx);
                }
            }
        }
        self.refresh_toolbar(cx);
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        // Standalone first start: the persisted family (or the host's) is
        // installed before the widgets register so theme roles resolve to it.
        let args = Args::from_env();
        let settings = Settings::load(&args.state_dir());
        appearance::install_initial(vm, &settings);
        makepad_widgets::script_mod(vm);
        makepad_wm_theme::apply(vm);
        makepad_terminal::widget::script_mod(vm);
        makepad_aichat::script_mod(vm);
        makepad_code_editor::script_mod(vm);
        makepad_flowgraph::script_mod(vm);
        makepad_studio::document::script_mod(vm);
        makepad_studio::project_tree::script_mod(vm);
        makepad_studio::canvas::script_mod(vm);
        makepad_studio::disk_graph::script_mod(vm);
        makepad_studio::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if matches!(event, Event::KeyUp(key) if key.key_code == KeyCode::Escape)
            && self.ui.modal(cx, ids!(utility_overlay)).is_open()
        {
            self.close_utility(cx);
            return;
        }
        if let Event::WindowCloseRequested(request) = event {
            request.accept_close.set(false);
            log!("studio: user closed the window; finishing disk inspection shutdown");
            self.begin_quit(cx);
            return;
        }
        if let Event::QuitRequested(request) = event {
            request.handled.set(true);
            self.begin_quit(cx);
            return;
        }
        if self
            .quit_timer
            .as_ref()
            .is_some_and(|t| t.is_event(event).is_some())
        {
            if self.workers_finished() {
                if let Some(timer) = self.quit_timer.take() {
                    cx.stop_timer(timer);
                }
                self.disk_worker = None;
                self.document_worker = None;
                self.activity_worker = None;
                self.usage_worker = None;
                cx.quit();
            }
            return;
        }
        if self.quit_timer.is_some() {
            return;
        }
        if let Event::Custom(json) = event {
            match makepad_wm_api::WmEvent::parse(json) {
                Some(makepad_wm_api::WmEvent::CloseRequested) => {
                    self.begin_quit(cx);
                    return;
                }
                Some(makepad_wm_api::WmEvent::Adopted) => self.start_services(cx),
                _ => {}
            }
        }
        if let Some(mut tree) = self.project_tree_ref.borrow_mut::<StudioProjectTree>() {
            tree.poll(cx);
        }
        self.drain_documents(cx);
        self.drain_activity(cx);
        self.drain_usage(cx);
        self.drain_services(cx, event);
        if self
            .workspace_timer
            .as_ref()
            .is_some_and(|t| t.is_event(event).is_some())
        {
            // Reuse the existing timer to follow host changes while open.
            // Resolving an unchanged choice does not reload or write settings.
            if !Self::hosted(cx) && appearance::picker_index(&self.settings) == 0 {
                self.apply_style_choice(cx);
            }
            self.refresh_usage_panel(cx);
            self.refresh_workspace(cx);
            if self.workspace_dirty {
                self.save_workspace(cx);
                self.workspace_dirty = false;
            }
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        if matches!(event, Event::Draw(_)) && self.focus_file_path {
            let input = self.ui.text_input(cx, ids!(file_path));
            if !input.area().is_empty() {
                self.focus_file_path = false;
                input.take_key_focus(cx);
            }
        }
        if matches!(event, Event::WindowGeomChange(_)) {
            self.refresh_utility_layout(cx);
            self.refresh_canvas_controls(cx);
            self.refresh_toolbar(cx);
        }
        if matches!(event, Event::LiveEdit | Event::ScriptReapply) {
            self.refresh_utility_layout(cx);
            // A stylesheet reapply (ours or the WM's) just ran: the strip
            // and the picker's note describe the sheet now in force.
            self.refresh_status(cx);
            self.refresh_settings_panel(cx);
            self.refresh_disk_panel(cx);
            self.refresh_usage_panel(cx);
            self.refresh_toolbar(cx);
        }
        if self.ui.dock(cx, ids!(dock)).check_and_clear_need_save() {
            self.save_dock(cx);
            self.refresh_workspace(cx);
        }
        if matches!(
            event,
            Event::Actions(_) | Event::Signal | Event::ScriptReapply
        ) {
            self.refresh_ai_context(cx);
        }
        if matches!(event, Event::Shutdown) {
            self.disk_worker = None;
            self.document_worker = None;
            self.activity_worker = None;
            self.usage_worker = None;
        }
    }
}

include!("workspace_app.rs");
include!("dock_app.rs");
include!("usage_app.rs");

#[cfg(test)]
mod desktop_style_tests {
    include!("../../../widgets/tests/support/app_style.rs");
}
