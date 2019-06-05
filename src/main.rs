//use syn::Type;
use render::*;
use widget::*;
use editor::*;
mod rustcompiler;
pub use crate::rustcompiler::*;
use std::collections::HashMap;
//use std::borrow::Cow;
use serde::*;

#[derive(Clone, Serialize, Deserialize)]

enum Panel {
    RustCompiler,
    Keyboard,
    FileTree,
    FileEditorTarget,
    FileEditor {path: String, editor_id: u64}
}

struct AppWindow {
    window_index: usize,
    desktop_window: DesktopWindow,
    file_tree: FileTree,
    keyboard: Keyboard,
    file_editors: Elements<u64, FileEditor, FileEditorTemplates>,
    dock: Dock<Panel>,
}

struct AppGlobal {
    file_tree_data: String,
    file_tree_reload_signal: Signal,
    text_buffers: TextBuffers,
    rust_compiler: RustCompiler,
    app_state: AppState,
    index_read_req: FileReadRequest,
    app_state_read_req: FileReadRequest,
}

struct App {
    app_global: AppGlobal,
    windows: Vec<AppWindow>,
}

#[derive(Clone, Serialize, Deserialize)]
struct AppStateWindow {
    window_position: Vec2,
    window_outer_size: Vec2,
    dock_items: DockItem<Panel>,
}

#[derive(Clone, Serialize, Deserialize)]
struct AppState {
    windows: Vec<AppStateWindow>
}

main_app!(App, "Makepad");

impl Style for AppWindow {
    fn style(cx: &mut Cx) -> Self {
        Self {
            desktop_window: DesktopWindow {
                ..Style::style(cx)
            },
            file_editors: Elements::new(FileEditorTemplates {
                rust_editor: RustEditor {
                    ..Style::style(cx)
                },
                js_editor: JSEditor {
                    ..Style::style(cx)
                }
            }),
            keyboard: Keyboard {
                ..Style::style(cx)
            },
            file_tree: FileTree {..Style::style(cx)},
            dock: Dock {
                ..Style::style(cx)
            },
            window_index: 0
        }
    }
}

impl Style for App {
    
    fn style(cx: &mut Cx) -> Self {
        set_dark_style(cx);
        Self {
            windows: vec![
                AppWindow {
                    window_index:0,
                    ..Style::style(cx)
                }
            ],
            app_global: AppGlobal {
                rust_compiler: RustCompiler {
                    ..Style::style(cx)
                },
                text_buffers: TextBuffers {
                    root_path: "./".to_string(),
                    storage: HashMap::new()
                },
                index_read_req: FileReadRequest::empty(),
                app_state_read_req: FileReadRequest::empty(),
                file_tree_data: String::new(),
                file_tree_reload_signal: cx.new_signal(),
                app_state: AppState {
                    windows: vec![
                        AppStateWindow {
                            window_outer_size: Vec2::zero(),
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
                                                closeable: false,
                                                title: "Edit".to_string(),
                                                item: Panel::FileEditorTarget
                                            },
                                            DockTab {
                                                closeable: true,
                                                title: "example.rs".to_string(),
                                                item: Panel::FileEditor {path: "example/src/example.rs".to_string(), editor_id: 1}
                                            }
                                        ],
                                    }),
                                    last: Box::new(DockItem::TabControl {
                                        current: 0,
                                        tabs: vec![
                                            DockTab {
                                                closeable: false,
                                                title: "Rust Compiler".to_string(),
                                                item: Panel::RustCompiler
                                            },
                                            DockTab {
                                                closeable: false,
                                                title: "Keyboard".to_string(),
                                                item: Panel::Keyboard
                                            }
                                        ]
                                    })
                                })
                            },
                        },
                    ]
                }
            }
        }
    }
}


impl AppWindow {
    fn handle_app_window(&mut self, cx: &mut Cx, event: &mut Event, app_global: &mut AppGlobal) {
        
        match self.desktop_window.handle_desktop_window(cx, event) {
            DesktopWindowEvent::EventForOtherWindow => {
                return
            },
            DesktopWindowEvent::WindowClosed=>{
                return
            },
            _ => ()
        }
        
        match event {
            Event::Signal(se) => if app_global.file_tree_reload_signal.is_signal(se) {
                self.file_tree.load_from_json(cx, &app_global.file_tree_data);
            },
            _ => ()
        }
        
        let dock_items = &mut app_global.app_state.windows[self.window_index].dock_items;
        let mut dock_walker = self.dock.walker(dock_items);
        let mut file_tree_event = FileTreeEvent::None;
        while let Some(item) = dock_walker.walk_handle_dock(cx, event) {
            match item {
                Panel::RustCompiler => {
                    match app_global.rust_compiler.handle_rust_compiler(cx, event, &mut app_global.text_buffers) {
                        RustCompilerEvent::SelectMessage {path} => {
                            // just make it open an editor
                            file_tree_event = FileTreeEvent::SelectFile {path: path};
                            
                        },
                        _ => ()
                    }
                },
                Panel::Keyboard => {
                    self.keyboard.handle_keyboard(cx, event, &mut app_global.text_buffers);
                },
                Panel::FileEditorTarget => {
                    
                },
                Panel::FileTree => {
                    file_tree_event = self.file_tree.handle_file_tree(cx, event);
                },
                Panel::FileEditor {path, editor_id} => {
                    if let Some(file_editor) = &mut self.file_editors.get(*editor_id) {
                        let text_buffer = app_global.text_buffers.from_path(cx, path);
                        match file_editor.handle_file_editor(cx, event, text_buffer) {
                            FileEditorEvent::LagChange => {
                                app_global.text_buffers.save_file(cx, path);
                                // lets save the textbuffer to disk
                                // lets re-trigger the rust compiler
                                app_global.rust_compiler.restart_rust_checker(cx, &mut app_global.text_buffers);
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
                    tabs.push(self.new_file_editor_tab(app_global, &path));
                }
                self.dock.dock_drag_end(cx, fe, tabs);
            },
            FileTreeEvent::SelectFile {path} => {
                // search for the tabcontrol with the maximum amount of editors
                if self.focus_or_new_editor(cx, app_global, &path) {
                    app_global.save_app_state(cx);
                }
            },
            _ => {}
        }
        
        let dock_items = &mut app_global.app_state.windows[self.window_index].dock_items;
        match self.dock.handle_dock(cx, event, dock_items) {
            DockEvent::DockChanged => { // thats a bit bland event. lets let the thing know which file closed
                app_global.save_app_state(cx);
            },
            _ => ()
        }
    }
    
    fn draw_app_window(&mut self, cx: &mut Cx, app_global: &mut AppGlobal) {
        if let Err(()) = self.desktop_window.begin_desktop_window(cx) {
            return
        }
        self.dock.draw_dock(cx);
        
        let dock_items = &mut app_global.app_state.windows[self.window_index].dock_items;
        let mut dock_walker = self.dock.walker(dock_items);
        while let Some(item) = dock_walker.walk_draw_dock(cx) {
            match item {
                Panel::RustCompiler => {
                    app_global.rust_compiler.draw_rust_compiler(cx);
                },
                Panel::Keyboard => {
                    self.keyboard.draw_keyboard(cx);
                },
                Panel::FileEditorTarget => {},
                Panel::FileTree => {
                    self.file_tree.draw_file_tree(cx);
                },
                Panel::FileEditor {path, editor_id} => {
                    let text_buffer = app_global.text_buffers.from_path(cx, path);
                    let mut set_key_focus = false;
                    let file_editor = self.file_editors.get_draw(cx, *editor_id, | _cx, tmpl | {
                        set_key_focus = true;
                        FileEditor::create_file_editor_for_path(path, tmpl)
                    });
                    file_editor.draw_file_editor(cx, text_buffer);
                    if set_key_focus {
                        file_editor.set_key_focus(cx);
                    }
                }
            }
        }
        self.desktop_window.end_desktop_window(cx);
    }
    
    fn new_file_editor_tab(&mut self, app_global: &mut AppGlobal, path: &str) -> DockTab<Panel> {
        let mut max_id = 0;
        let dock_items = &mut app_global.app_state.windows[self.window_index].dock_items;
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
    
    fn focus_or_new_editor(&mut self, cx: &mut Cx, app_global: &mut AppGlobal, file_path: &str) -> bool {
        let mut target_ctrl_id = 0;
        let mut only_focus_editor = false;
        let dock_items = &mut app_global.app_state.windows[self.window_index].dock_items;
        let mut dock_walker = self.dock.walker(dock_items);
        let mut ctrl_id = 1;
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
                            Panel::FileEditorTarget => { // found the editor target
                                target_ctrl_id = ctrl_id;
                            },
                            _ => ()
                        }
                    }
                },
                _ => ()
            }
            ctrl_id += 1;
        }
        if target_ctrl_id != 0 && !only_focus_editor { // open a new one
            let new_tab = self.new_file_editor_tab(app_global, file_path);
            let dock_items = &mut app_global.app_state.windows[self.window_index].dock_items;
            let mut dock_walker = self.dock.walker(dock_items);
            let mut ctrl_id = 1;
            while let Some(dock_item) = dock_walker.walk_dock_item() {
                if ctrl_id == target_ctrl_id {
                    if let DockItem::TabControl {current, tabs} = dock_item {
                        tabs.insert(*current + 1, new_tab);
                        *current = *current + 1;
                        cx.redraw_child_area(Area::All);
                        return true;
                    }
                }
                ctrl_id += 1;
            }
        }
        return false
    }
}

impl AppGlobal {
    fn handle_construct(&mut self, cx: &mut Cx) {
        if cx.feature == "mtl" {
            self.text_buffers.root_path = "./edit_repo/".to_string();
        }
        
        self.index_read_req = cx.read_file(&format!("{}index.json", self.text_buffers.root_path));
        self.app_state_read_req = cx.read_file(&format!("{}makepad_state.json", self.text_buffers.root_path));
        
        self.rust_compiler.init(cx, &mut self.text_buffers);
    }
    
    fn handle_file_read(&mut self, cx: &mut Cx, fr: &FileReadEvent) {
        // lets see which file we loaded
        if let Some(utf8_data) = self.index_read_req.as_utf8(fr) {
            self.file_tree_data = utf8_data.to_string();
            cx.send_signal_before_draw(self.file_tree_reload_signal, 0);
        }
        else if let Some(_utf8_data) = self.app_state_read_req.as_utf8(fr) {
            //if let Ok(app_state) = serde_json::from_str(&utf8_data) {
                //self.app_state = app_state;
                // update window pos and size
                //cx.set_window_position(self.app_state.window_position);
                //cx.set_window_outer_size(self.app_state.window_outer_size)
            //}
        }
        else if self.text_buffers.handle_file_read(&fr) {
            cx.redraw_child_area(Area::All);
        }
    }
    
    fn handle_window_change(&mut self, cx: &mut Cx, _wc: &WindowGeomChangeEvent) {
        if !self.app_state_read_req.is_loading() {
            //self.app_state.window_position = wc.new_geom.position;
            //self.app_state.window_outer_size = wc.new_geom.outer_size;
            self.save_app_state(cx);
        }
    }
    
    fn save_app_state(&mut self, cx: &mut Cx) {
        let json = serde_json::to_string(&self.app_state).unwrap();
        cx.write_file(&format!("{}makepad_state.json", self.text_buffers.root_path), json.as_bytes());
    }
}

impl App {
    fn handle_app(&mut self, cx: &mut Cx, event: &mut Event) {
        match event {
            Event::Construct => {
                self.app_global.handle_construct(cx);
            },
            Event::FileRead(fr) => {
                self.app_global.handle_file_read(cx, fr);
                
            },
            Event::WindowGeomChange(wc) => {
                self.app_global.handle_window_change(cx, wc);
            },
            _ => ()
        }
        for window in &mut self.windows {
            window.handle_app_window(cx, event, &mut self.app_global);
           // break;
        }
    }
    
    fn draw_app(&mut self, cx: &mut Cx) {
        //return;
        for window in &mut self.windows {
            window.draw_app_window(cx, &mut self.app_global);
           // break;
        }
    }
}

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