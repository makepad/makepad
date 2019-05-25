//use syn::Type;
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

struct App {
    view: View<ScrollBar>,
    dock: Dock<Panel>,
    app_state: AppState,
    file_tree: FileTree,
    file_editors: Elements<u64, FileEditor, FileEditorTemplates>,
    rust_compiler: RustCompiler,
    keyboard: Keyboard,
    text_buffers: TextBuffers,
    index_read_id: u64,
    app_state_read_id: u64
}

#[derive(Clone, Serialize, Deserialize)]
struct AppState {
    window_outer_size: Vec2,
    window_position: Vec2,
    dock_items: DockItem<Panel>,
}

main_app!(App, "Makepad");

impl Style for App {
    
    fn style(cx: &mut Cx) -> Self {
        set_dark_style(cx);
        Self {
            
            view: View {
                ..Style::style(cx)
            },
            text_buffers: TextBuffers {
                root_path: "./".to_string(),
                storage: HashMap::new()
            },
            file_tree: FileTree {..Style::style(cx)},
            index_read_id: 0,
            app_state_read_id: 0,
            file_editors: Elements::new(FileEditorTemplates {
                rust_editor: RustEditor {
                    ..Style::style(cx)
                }
            }),
            rust_compiler: RustCompiler {
                ..Style::style(cx)
            },
            keyboard: Keyboard {
                ..Style::style(cx)
            },
            app_state: AppState {
                window_outer_size: cx.window_geom.outer_size,
                window_position: cx.window_geom.position,
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
                                    item: Panel::FileEditor {path: "src/example.rs".to_string(), editor_id: 1}
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
            dock: Dock {
                ..Style::style(cx)
            }
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

impl App {
    fn handle_app(&mut self, cx: &mut Cx, event: &mut Event) {
        match event {
            Event::Construct => {
                
                if cx.feature == "mtl" {
                    self.text_buffers.root_path = "./edit_repo/".to_string();
                }
                
                self.index_read_id = cx.read_file(&format!("{}index.json", self.text_buffers.root_path));
                self.app_state_read_id = cx.read_file(&format!("{}makepad_state.json", self.text_buffers.root_path));
                
                self.rust_compiler.init(cx, &mut self.text_buffers);
                
            },
            Event::FileRead(fr) => {
                // lets see which file we loaded
                if fr.read_id == self.index_read_id {
                    if let Ok(str_data) = &fr.data {
                        if let Ok(utf8_data) = std::str::from_utf8(&str_data) {
                            self.file_tree.load_from_json(cx, utf8_data);
                        }
                    }
                }
                else if fr.read_id == self.app_state_read_id {
                    self.app_state_read_id = 0;
                    if let Ok(str_data) = &fr.data {
                        if let Ok(utf8_data) = std::str::from_utf8(&str_data) {
                            if let Ok(app_state) = serde_json::from_str(&utf8_data) {
                                self.app_state = app_state;
                                // update window pos and size
                                println!("READ {:?}", self.app_state.window_position);
                                cx.set_window_position(self.app_state.window_position);
                                cx.set_window_outer_size(self.app_state.window_outer_size)
                            }
                            self.file_tree.load_from_json(cx, utf8_data);
                        }
                    }
                }
                else if self.text_buffers.handle_file_read(&fr) {
                    cx.redraw_area(Area::All);
                }
            },
            Event::WindowChange(wc)=>{
                if self.app_state_read_id == 0{
                    self.app_state.window_position = wc.new_geom.position;
                    println!("HERE {:?}", self.app_state.window_position);
                    self.app_state.window_outer_size = wc.new_geom.outer_size;
                    self.save_app_state(cx);
                }
            },
            _ => ()
        }
        
        self.view.handle_scroll_bars(cx, event);
        
        let mut dock_walker = self.dock.walker(&mut self.app_state.dock_items);
        let mut file_tree_event = FileTreeEvent::None;
        while let Some(item) = dock_walker.walk_handle_dock(cx, event) {
            match item {
                Panel::RustCompiler => {
                    match self.rust_compiler.handle_rust_compiler(cx, event, &mut self.text_buffers) {
                        RustCompilerEvent::SelectMessage {path} => {
                            // just make it open an editor
                            file_tree_event = FileTreeEvent::SelectFile {path: path};
                            
                        },
                        _ => ()
                    }
                },
                Panel::Keyboard => {
                    self.keyboard.handle_keyboard(cx, event, &mut self.text_buffers);
                },
                Panel::FileEditorTarget => {
                    
                },
                Panel::FileTree => {
                    file_tree_event = self.file_tree.handle_file_tree(cx, event);
                },
                Panel::FileEditor {path, editor_id} => {
                    if let Some(file_editor) = &mut self.file_editors.get(*editor_id) {
                        let text_buffer = self.text_buffers.from_path(cx, path);
                        match file_editor.handle_file_editor(cx, event, text_buffer) {
                            FileEditorEvent::LagChange => {
                                self.text_buffers.save_file(cx, path);
                                // lets save the textbuffer to disk
                                // lets re-trigger the rust compiler
                                self.rust_compiler.restart_rust_checker(cx, &mut self.text_buffers);
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
                    tabs.push(self.new_file_editor_tab(&path));
                }
                self.dock.dock_drag_end(cx, fe, tabs);
            },
            FileTreeEvent::SelectFile {path} => {
                // search for the tabcontrol with the maximum amount of editors
                if self.focus_or_new_editor(cx, &path) {
                    self.save_app_state(cx);
                }
            },
            _ => {}
        }
        
        // handle the dock events
        match self.dock.handle_dock(cx, event, &mut self.app_state.dock_items) {
            DockEvent::DockChanged => { // thats a bit bland event. lets let the thing know which file closed
                self.save_app_state(cx);
            },
            _ => ()
        }
    }
    
    fn save_app_state(&mut self, cx: &mut Cx) {
        let json = serde_json::to_string(&self.app_state).unwrap();
        cx.write_file(&format!("{}makepad_state.json", self.text_buffers.root_path), json.as_bytes());
    }
    
    fn draw_app(&mut self, cx: &mut Cx) {
        if let Err(()) = self.view.begin_view(cx, &Layout {..Default::default()}) {
            return
        }
        
        self.dock.draw_dock(cx);
        
        let mut dock_walker = self.dock.walker(&mut self.app_state.dock_items);
        while let Some(item) = dock_walker.walk_draw_dock(cx) {
            match item {
                Panel::RustCompiler => {
                    self.rust_compiler.draw_rust_compiler(cx);
                },
                Panel::Keyboard => {
                    self.keyboard.draw_keyboard(cx);
                },
                Panel::FileEditorTarget => {},
                Panel::FileTree => {
                    self.file_tree.draw_file_tree(cx);
                },
                Panel::FileEditor {path, editor_id} => {
                    let text_buffer = self.text_buffers.from_path(cx, path);
                    self.file_editors.get_draw(cx, *editor_id, | _cx, tmpl | {
                        FileEditor::create_file_editor_for_path(path, tmpl)
                    }).draw_file_editor(cx, text_buffer);
                }
            }
        }
        self.view.end_view(cx);
    }
    
    fn new_file_editor_tab(&mut self, path: &str) -> DockTab<Panel> {
        let mut max_id = 0;
        let mut dock_walker = self.dock.walker(&mut self.app_state.dock_items);
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
    
    fn focus_or_new_editor(&mut self, cx: &mut Cx, file_path: &str) -> bool {
        let mut target_ctrl_id = 0;
        let mut only_focus_editor = false;
        let mut dock_walker = self.dock.walker(&mut self.app_state.dock_items);
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
                                    cx.redraw_area(Area::All);
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
            let new_tab = self.new_file_editor_tab(file_path);
            let mut dock_walker = self.dock.walker(&mut self.app_state.dock_items);
            let mut ctrl_id = 1;
            while let Some(dock_item) = dock_walker.walk_dock_item() {
                if ctrl_id == target_ctrl_id {
                    if let DockItem::TabControl {current, tabs} = dock_item {
                        tabs.insert(*current + 1, new_tab);
                        *current = *current + 1;
                        cx.redraw_area(Area::All);
                        return true;
                    }
                }
                ctrl_id += 1;
            }
        }
        return false
    }
}

struct FileEditorTemplates {
    rust_editor: RustEditor,
}

#[derive(Clone)]
enum FileEditor {
    Rust(RustEditor)
}

impl ElementLife for FileEditor {
    fn construct(&mut self, cx: &mut Cx) {
        match self {
            FileEditor::Rust(re) => re.construct(cx),
        }
    }
    fn destruct(&mut self, cx: &mut Cx) {
        match self {
            FileEditor::Rust(re) => re.destruct(cx),
        }
    }
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
        }
    }
    
    fn set_key_focus(&mut self, cx: &mut Cx) {
        match self {
            FileEditor::Rust(re) => re.code_editor.set_key_focus(cx),
        }
    }
    
    fn draw_file_editor(&mut self, cx: &mut Cx, text_buffer: &mut TextBuffer) {
        match self {
            FileEditor::Rust(re) => re.draw_rust_editor(cx, text_buffer),
        }
    }
    
    fn create_file_editor_for_path(path: &str, template: &FileEditorTemplates) -> FileEditor {
        // check which file extension we have to spawn a new editor
        FileEditor::Rust(RustEditor {
            path: path.to_string(),
            set_key_focus_on_draw: true,
            ..template.rust_editor.clone()
        })
    }
}