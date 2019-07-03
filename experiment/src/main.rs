//use syn::Type;
use render::*;
use widget::*;
use editor::*;
mod style;

use std::collections::HashMap;
//use std::borrow::Cow;
use serde::*;

#[derive(Clone, Serialize, Deserialize)]
enum Panel {
    FileTree,
    FileEditor {path: String, editor_id: u64}
}

#[derive(Clone)]
struct AppWindow {
    desktop_window: DesktopWindow,
    file_tree: FileTree,
    file_editors: Elements<u64, FileEditor, FileEditorTemplates>,
    dock: Dock<Panel>,
    graph_nodes: Vec<GraphNode>,
    graph_view: View<NoScrollBar>,
}

struct AppGlobal {
    file_tree_data: String,
    file_tree_reload_signal: Signal,
    text_buffers: TextBuffers,
    state: AppState,
    index_file_read: FileRead,
    app_state_file_read: FileRead,
}

struct App {
    app_window_state_template: AppWindowState,
    app_window_template: AppWindow,
    app_global: AppGlobal,
    windows: Vec<AppWindow>,
}

#[derive(Clone, Serialize, Deserialize)]
struct AppWindowState {
    window_position: Vec2,
    window_inner_size: Vec2,
    dock_items: DockItem<Panel>,
}

#[derive(Default, Clone, Serialize, Deserialize)]
struct AppState {
    windows: Vec<AppWindowState>
}

main_app!(App, "HALLO WORLD");

impl Style for AppWindow {
    fn style(cx: &mut Cx) -> Self {
        Self {
            desktop_window: DesktopWindow::style(cx),
            file_editors: Elements::new(FileEditorTemplates {
                rust_editor: RustEditor::style(cx),
                js_editor: JSEditor::style(cx)
            }),
            file_tree: FileTree::style(cx),
            dock: Dock ::style(cx),
            graph_nodes: vec![
                GraphNode {
                    node_bg_layout: Layout {
                        abs_origin: Some(Vec2{x: 100.0, y: 100.0}),
                        width: Bounds::Fix(100.0),
                        height: Bounds::Fix(50.0),
                        ..Default::default()
                    },
                    ..Style::style(cx)
                },
                GraphNode {
                    node_bg_layout: Layout {
                        abs_origin: Some(Vec2{x: 100.0, y: 200.0}),
                        width: Bounds::Fix(100.0),
                        height: Bounds::Fix(50.0),
                        ..Layout::default()
                    },
                    ..Style::style(cx)
                },
                GraphNode {
                    node_bg_layout: Layout {
                        abs_origin: Some(Vec2{x: 100.0, y: 300.0}),
                        width: Bounds::Fix(100.0),
                        height: Bounds::Fix(50.0),
                        ..Layout::default()
                    },
                    ..Style::style(cx)
                },
            ],
            graph_view: View {
                // QUESTION: what is is_overlay for?
                is_overlay: true,
                ..Style::style(cx)
            },
        }
    }
}

impl Style for App {

    fn style(cx: &mut Cx) -> Self {
        style::set_experiment_style(cx);
        Self {
            app_window_template: AppWindow::style(cx),
            app_window_state_template: AppWindowState {
                window_inner_size: Vec2::zero(),
                window_position: Vec2::zero(),
                dock_items: DockItem::Splitter {
                    axis: Axis::Vertical,
                    align: SplitterAlign::First,
                    pos: 150.0,
                    first: Box::new(DockItem::TabControl {
                        current: 0,
                        tabs: vec![DockTab {
                            closeable: false,
                            title: "Files".to_string(),
                            item: Panel::FileTree
                        }]
                    }),
                    last: Box::new(DockItem::Splitter {
                        axis: Axis::Horizontal,
                        align: SplitterAlign::Last,
                        pos: 150.0,
                        first: Box::new(DockItem::TabControl {
                            current: 1,
                            tabs: vec![
                                DockTab {
                                    closeable: true,
                                    title: "example.rs".to_string(),
                                    item: Panel::FileEditor {path: "example/src/example.rs".to_string(), editor_id: 1}
                                }
                            ],
                        }),
                        last: Box::new(DockItem::TabControl {
                            current: 0,
                            tabs: vec![]
                        })
                    })
                },
            },
            windows: vec![],
            app_global: AppGlobal {
                text_buffers: TextBuffers {
                    root_path: "./".to_string(),
                    storage: HashMap::new()
                },
                index_file_read: FileRead::default(),
                app_state_file_read: FileRead::default(),
                file_tree_data: String::new(),
                file_tree_reload_signal: cx.new_signal(),
                state: AppState::default()
            }
        }
    }
}

impl AppWindow {
    fn handle_app_window(&mut self, cx: &mut Cx, event: &mut Event, window_index: usize, app_global: &mut AppGlobal) {

        match self.desktop_window.handle_desktop_window(cx, event) {
            DesktopWindowEvent::EventForOtherWindow => {
                return
            },
            DesktopWindowEvent::WindowClosed => {
                return
            },
            DesktopWindowEvent::WindowGeomChange(wc) => {
                if !app_global.app_state_file_read.is_pending() {
                    // store our new window geom
                    app_global.state.windows[window_index].window_position = wc.new_geom.position;
                    app_global.state.windows[window_index].window_inner_size = wc.new_geom.inner_size;
                    app_global.save_state(cx);
                }
            },
            _ => ()
        }

        for node in &mut self.graph_nodes {
            match node.handle_graph_node(cx, event) {
                GraphNodeEvent::DragMove {..} => {
                    self.graph_view.redraw_view_area(cx);
                },
                _ => ()
            }

        }

        match event {
            Event::Signal(se) => if app_global.file_tree_reload_signal.is_signal(se) {
                self.file_tree.load_from_json(cx, &app_global.file_tree_data);
            },
            _ => ()
        }

        let dock_items = &mut app_global.state.windows[window_index].dock_items;
        let mut dock_walker = self.dock.walker(dock_items);
        let mut file_tree_event = FileTreeEvent::None;
        while let Some(item) = dock_walker.walk_handle_dock(cx, event) {
            match item {
                Panel::FileTree => {
                    file_tree_event = self.file_tree.handle_file_tree(cx, event);
                },
                Panel::FileEditor {path, editor_id} => {
                    if let Some(file_editor) = &mut self.file_editors.get(*editor_id) {
                        let text_buffer = app_global.text_buffers.from_path(cx, path);
                        match file_editor.handle_file_editor(cx, event, text_buffer) {
                            FileEditorEvent::LagChange => {
                                app_global.text_buffers.save_file(cx, path);
                            },
                            _ => ()
                        }
                    }
                }
            }
        }
        match file_tree_event {
            FileTreeEvent::DragMove {fe, ..} => {
                self.dock.dock_drag_move(cx, fe);
            },
            FileTreeEvent::DragOut => {
                self.dock.dock_drag_out(cx);
            },
            FileTreeEvent::DragEnd {fe, paths} => {
                let mut tabs = Vec::new();
                for path in paths {
                    // find a free editor id
                    tabs.push(self.new_file_editor_tab(window_index, app_global, &path));
                }
                self.dock.dock_drag_end(cx, fe, tabs);
            },
            FileTreeEvent::SelectFile {path} => {
                // search for the tabcontrol with the maximum amount of editors
                if self.focus_or_new_editor(cx, window_index, app_global, &path) {
                    app_global.save_state(cx);
                }
            },
            _ => {}
        }

        let dock_items = &mut app_global.state.windows[window_index].dock_items;
        match self.dock.handle_dock(cx, event, dock_items) {
            DockEvent::DockChanged => { // thats a bit bland event. lets let the thing know which file closed
                app_global.save_state(cx);
            },
            _ => ()
        }
    }

    fn draw_app_window(&mut self, cx: &mut Cx, window_index: usize, app_global: &mut AppGlobal) {
        if let Err(()) = self.desktop_window.begin_desktop_window(cx) {
            return
        }

        // self.dock.draw_dock(cx);
        if let Err(()) = self.graph_view.begin_view(cx, Layout::default()){
            return
        }

        for node in &mut self.graph_nodes {
            node.draw_graph_node(cx);
        }

        self.graph_view.end_view(cx);
        self.desktop_window.end_desktop_window(cx);
    }

    fn new_file_editor_tab(&mut self, window_index: usize, app_global: &mut AppGlobal, path: &str) -> DockTab<Panel> {
        let mut max_id = 0;
        let dock_items = &mut app_global.state.windows[window_index].dock_items;
        let mut dock_walker = self.dock.walker(dock_items);
        while let Some(dock_item) = dock_walker.walk_dock_item() {
            match dock_item {
                DockItem::TabControl {tabs, ..} => {
                    for (_, tab) in tabs.iter().enumerate() {
                        match &tab.item {
                            Panel::FileEditor {editor_id, ..} => {
                                if *editor_id > max_id {
                                    max_id = *editor_id;
                                }
                            },
                            _ => ()
                        }
                    }
                },
                _ => ()
            }
        }
        let editor_id = max_id + 1;
        DockTab {
            closeable: true,
            title: path_file_name(&path),
            item: Panel::FileEditor {path: path.to_string(), editor_id: editor_id}
        }
    }

    fn focus_or_new_editor(&mut self, cx: &mut Cx, window_index: usize, app_global: &mut AppGlobal, file_path: &str) -> bool {
        let mut only_focus_editor = false;
        let dock_items = &mut app_global.state.windows[window_index].dock_items;
        let mut dock_walker = self.dock.walker(dock_items);
        while let Some(dock_item) = dock_walker.walk_dock_item() {
            match dock_item {
                DockItem::TabControl {current, tabs} => {
                    for (id, tab) in tabs.iter().enumerate() {
                        match &tab.item {
                            Panel::FileEditor {path, editor_id} => {
                                if *path == file_path {
                                    *current = id;
                                    // focus this one
                                    // set the keyboard focus
                                    if let Some(file_editor) = &mut self.file_editors.get(*editor_id) {
                                        file_editor.set_key_focus(cx);
                                    }
                                    only_focus_editor = true;
                                    cx.redraw_child_area(Area::All);
                                }
                            },
                            _ => ()
                        }
                    }
                },
                _ => ()
            }
        }
        return false
    }
}

impl AppGlobal {
    fn handle_construct(&mut self, cx: &mut Cx) {
        if cx.platform_type.is_desktop() {
            self.text_buffers.root_path = "./edit_repo/".to_string();
        }
    }

    fn save_state(&mut self, cx: &mut Cx) {
        let json = serde_json::to_string(&self.state).unwrap();
        cx.file_write(&format!("{}makepad_state.json", self.text_buffers.root_path), json.as_bytes());
    }
}

impl App {
    fn handle_app(&mut self, cx: &mut Cx, event: &mut Event) {
        match event {
            Event::Construct => {
                self.app_global.handle_construct(cx);
                self.app_global.index_file_read = cx.file_read(&format!("{}index.json", self.app_global.text_buffers.root_path));
                self.app_global.app_state_file_read = cx.file_read(&format!("{}makepad_state.json", self.app_global.text_buffers.root_path));
            },
            Event::FileRead(fr) => {
                // lets see which file we loaded
                if let Some(utf8_data) = self.app_global.index_file_read.resolve_utf8(fr) {
                    if let Ok(utf8_data) = utf8_data {
                        self.app_global.file_tree_data = utf8_data.to_string();
                        cx.send_signal_before_draw(self.app_global.file_tree_reload_signal, 0);
                    }
                }
                else if let Some(utf8_data) = self.app_global.app_state_file_read.resolve_utf8(fr) {
                    if let Ok(utf8_data) = utf8_data {
                        if let Ok(state) = serde_json::from_str(&utf8_data) {
                            self.app_global.state = state;

                            // create our windows with the serialized positions/size
                            for window_state in &self.app_global.state.windows {
                                let mut size = window_state.window_inner_size;

                                if size.x <= 10. {
                                    size.x = 800.;
                                }
                                if size.y <= 10. {
                                    size.y = 600.;
                                }
                                let last_pos = window_state.window_position;
                                let create_pos;
                                if last_pos.x < -1000. || last_pos.y < -1000. {
                                    create_pos = None;
                                }
                                else {
                                    create_pos = Some(last_pos);
                                }
                                self.windows.push(AppWindow {
                                    desktop_window: DesktopWindow {window: Window {
                                        create_inner_size: Some(size),
                                        create_position: create_pos,
                                        ..Style::style(cx)
                                    }, ..Style::style(cx)},
                                    ..self.app_window_template.clone()
                                })
                            }
                            cx.send_signal_before_draw(self.app_global.file_tree_reload_signal, 0);
                            cx.redraw_child_area(Area::All);
                        }
                    }
                    else { // load default window
                        println!("DOING DEFAULT");
                        self.app_global.state.windows = vec![self.app_window_state_template.clone()];
                        self.windows = vec![self.app_window_template.clone()];
                        cx.send_signal_before_draw(self.app_global.file_tree_reload_signal, 0);
                        cx.redraw_child_area(Area::All);
                    }
                }
                else if self.app_global.text_buffers.handle_file_read(cx, &fr) {
                    // this should work already
                    //cx.redraw_child_area(Area::All);
                }
            },

            _ => ()
        }
        for (window_index, window) in self.windows.iter_mut().enumerate() {
            window.handle_app_window(cx, event, window_index, &mut self.app_global);
            // break;
        }
    }

    fn draw_app(&mut self, cx: &mut Cx) {
        //return;
        for (window_index, window) in self.windows.iter_mut().enumerate() {
            window.draw_app_window(cx, window_index, &mut self.app_global);
            // break;
        }
    }
}

#[derive(Clone)]
struct FileEditorTemplates {
    rust_editor: RustEditor,
    js_editor: JSEditor,
    //text_editor: TextEditor
}

#[derive(Clone)]
enum FileEditor {
    Rust(RustEditor),
    JS(JSEditor),
    //Text(TextEditor)
}

#[derive(Clone)]
enum FileEditorEvent {
    None,
    LagChange,
    Change
}

impl FileEditor {
    fn handle_file_editor(&mut self, cx: &mut Cx, event: &mut Event, text_buffer: &mut TextBuffer) -> FileEditorEvent {
        match self {
            FileEditor::Rust(re) => {
                match re.handle_rust_editor(cx, event, text_buffer) {
                    CodeEditorEvent::Change => FileEditorEvent::Change,
                    CodeEditorEvent::LagChange => FileEditorEvent::LagChange,
                    _ => FileEditorEvent::None
                }
            },
            FileEditor::JS(re) => {
                match re.handle_js_editor(cx, event, text_buffer) {
                    CodeEditorEvent::Change => FileEditorEvent::Change,
                    CodeEditorEvent::LagChange => FileEditorEvent::LagChange,
                    _ => FileEditorEvent::None
                }
            },
        }
    }

    fn set_key_focus(&mut self, cx: &mut Cx) {
        match self {
            FileEditor::Rust(re) => re.code_editor.set_key_focus(cx),
            FileEditor::JS(re) => re.code_editor.set_key_focus(cx),
        }
    }

    fn draw_file_editor(&mut self, cx: &mut Cx, text_buffer: &mut TextBuffer) {
        match self {
            FileEditor::Rust(re) => re.draw_rust_editor(cx, text_buffer),
            FileEditor::JS(re) => re.draw_js_editor(cx, text_buffer),
        }
    }

    fn create_file_editor_for_path(path: &str, template: &FileEditorTemplates) -> FileEditor {
        // check which file extension we have to spawn a new editor
        if path.ends_with(".rs") {
            FileEditor::Rust(RustEditor {
                ..template.rust_editor.clone()
            })
        }
        else if path.ends_with(".js") {
            FileEditor::JS(JSEditor {
                ..template.js_editor.clone()
            })
        }
        else {
            FileEditor::Rust(RustEditor {
                ..template.rust_editor.clone()
            })
        }
    }
}


fn path_file_name(path: &str) -> String {
    if let Some(pos) = path.rfind('/') {
        path[pos + 1..path.len()].to_string()
    }
    else {
        path.to_string()
    }
}
