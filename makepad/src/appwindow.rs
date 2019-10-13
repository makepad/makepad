//use syn::Type;
use render::*;
use widget::*;
use serde::*;
use editor::*;
use terminal::*;
use hub::*;
use crate::app::*;
use crate::fileeditor::*;
use crate::filetree::*;
use crate::loglist::*;
use crate::logitem::*;
use crate::keyboard::*;
use crate::buildmanager::*;

#[derive(Clone, Serialize, Deserialize)]
pub enum Panel {
    LogList,
    LogItem,
    Keyboard,
    FileTree,
    FileEditorTarget,
    FileEditor {path: String, editor_id: u64},
    LocalTerminal {start_path: String, terminal_id: u64}
}

#[derive(Clone)]
pub struct AppWindow {
    pub desktop_window: DesktopWindow,
    pub file_tree: FileTree,
    pub log_item: LogItem,
    pub log_list: LogList,
    pub keyboard: Keyboard,
    pub file_editors: Elements<u64, FileEditor, FileEditorTemplates>,
    pub local_terminals: Elements<u64, LocalTerminal, LocalTerminal>,
    pub dock: Dock<Panel>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct AppWindowState {
    pub window_position: Vec2,
    pub window_inner_size: Vec2,
    pub open_folders: Vec<String>,
    pub dock_items: DockItem<Panel>,
}

impl AppWindow {
    pub fn style(cx: &mut Cx) -> Self {
        Self {
            desktop_window: DesktopWindow {
                caption: "Makepad".to_string(),
                window: Window {
                    create_inner_size: None,
                    ..Window::style(cx)
                },
                ..DesktopWindow::style(cx)
            },
            file_editors: Elements::new(FileEditorTemplates {
                rust_editor: RustEditor::style(cx),
                js_editor: JSEditor::style(cx),
                plain_editor: PlainEditor::style(cx)
            }),
            local_terminals: Elements::new(LocalTerminal::style(cx)),
            keyboard: Keyboard::style(cx),
            log_item: LogItem::style(cx),
            log_list: LogList::style(cx),
            file_tree: FileTree::style(cx),
            dock: Dock ::style(cx),
        }
    }
    
    pub fn handle_app_window(&mut self, cx: &mut Cx, event: &mut Event, window_index: usize, state: &mut AppState, storage:&mut AppStorage, build_manager:&mut BuildManager) {
        
        match self.desktop_window.handle_desktop_window(cx, event) {      
            DesktopWindowEvent::EventForOtherWindow => {
                return
            },
            DesktopWindowEvent::WindowClosed => {
                return
            },
            DesktopWindowEvent::WindowGeomChange(wc) => {
                if !storage.app_state_file_read.is_pending() {
                    // store our new window geom
                    state.windows[window_index].window_position = wc.new_geom.position;
                    state.windows[window_index].window_inner_size = wc.new_geom.inner_size;
                    storage.save_state(cx, state);
                }
            },
            _ => ()
        }
       
        let dock_items = &mut state.windows[window_index].dock_items;
        let mut dock_walker = self.dock.walker(dock_items);
        let mut file_tree_event = FileTreeEvent::None;
        while let Some(item) = dock_walker.walk_handle_dock(cx, event) {
            match item {
                Panel::LogList => {
                    match self.log_list.handle_log_list(cx, event, storage, build_manager){
                        LogListEvent::SelectLogItem {path, item, level} => {
                            // just make it open an editor
                            if let Some(path) = path{
                                file_tree_event = FileTreeEvent::SelectFile {path:path};
                            }
                            if let Some(item) = item{
                                self.log_item.load_item(cx, &item, level);
                            }
                        },
                        LogListEvent::SelectLogRange{items}=>{
                            self.log_item.load_item(cx, &items, HubLogItemLevel::Log);
                        }
                        _ => ()
                    }
                },
                Panel::LogItem=>{
                    self.log_item.handle_log_item(cx, event);
                },
                Panel::Keyboard => {
                    self.keyboard.handle_keyboard(cx, event, storage);
                },
                Panel::FileEditorTarget => {
                },
                Panel::LocalTerminal {terminal_id, ..} => {
                    if let Some(local_terminal) = &mut self.local_terminals.get(*terminal_id) {
                        local_terminal.handle_local_terminal(cx, event);
                    }
                },
                Panel::FileTree => {
                    file_tree_event = self.file_tree.handle_file_tree(cx, event);
                },
                Panel::FileEditor {path, editor_id} => {
                    if let Some(file_editor) = &mut self.file_editors.get(*editor_id) {

                        let text_buffer = storage.text_buffer_from_path(cx, path);

                        match file_editor.handle_file_editor(cx, event, text_buffer) {
                            FileEditorEvent::LagChange => {
                                
                                // HERE WE SAVE
                                storage.text_buffer_file_write(cx, path);
                                
                                // lets re-trigger the rust compiler
                                build_manager.restart_cargo(cx, storage);
                                self.log_item.clear_msg(cx);
                                
                                //app_global.rust_compiler.restart_rust_checker(cx, &mut app_global.text_buffers);
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
            FileTreeEvent::DragCancel => {
                self.dock.dock_drag_cancel(cx);
            },
            FileTreeEvent::DragOut => {
                self.dock.dock_drag_out(cx);
            },
            FileTreeEvent::DragEnd {fe, paths} => {
                let mut tabs = Vec::new();
                for path in paths {
                    // find a free editor id
                    tabs.push(self.new_file_editor_tab(window_index, state, &path));
                }
                self.dock.dock_drag_end(cx, fe, tabs);
            },
            FileTreeEvent::SelectFile {path} => {
                // search for the tabcontrol with the maximum amount of editors
                if self.focus_or_new_editor(cx, window_index, state, &path) {
                    storage.save_state(cx, state);
                }
            },
            FileTreeEvent::SelectFolder {..} => {
                state.windows[window_index].open_folders = self.file_tree.save_open_folders();
                storage.save_state(cx, state);
            },
            _ => {}
        }
        
        let dock_items = &mut state.windows[window_index].dock_items;
        match self.dock.handle_dock(cx, event, dock_items) {
            DockEvent::DockChanged => { // thats a bit bland event. lets let the thing know which file closed
                storage.save_state(cx, state);
            },
            _ => ()
        }
    }
    
    pub fn draw_app_window(&mut self, cx: &mut Cx, window_index: usize, state: &mut AppState, storage:&mut AppStorage, build_manager:&mut BuildManager) {
        if let Err(()) = self.desktop_window.begin_desktop_window(cx) {
            return
        }
        self.dock.draw_dock(cx);
        
        let dock_items = &mut state.windows[window_index].dock_items;
        let mut dock_walker = self.dock.walker(dock_items);
        while let Some(item) = dock_walker.walk_draw_dock(cx) {
            match item {
                Panel::LogList => {
                    self.log_list.draw_log_list(cx, build_manager);
                },
                Panel::LogItem=>{
                    self.log_item.draw_log_item(cx);
                },
                Panel::Keyboard => {
                    self.keyboard.draw_keyboard(cx);
                },
                Panel::FileEditorTarget => {
                },
                Panel::FileTree => {
                    self.file_tree.draw_file_tree(cx);
                },
                Panel::LocalTerminal {terminal_id, ..} => {
                    let local_terminal = self.local_terminals.get_draw(cx, *terminal_id, | cx, tmpl | {
                        let mut new_terminal = tmpl.clone();
                        new_terminal.start_terminal(cx);
                        new_terminal
                    });
                    local_terminal.draw_local_terminal(cx);
                },
                Panel::FileEditor {path, editor_id} => {
                    let text_buffer = storage.text_buffer_from_path(cx, path);
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
    
    pub fn new_file_editor_tab(&mut self, window_index: usize, state: &mut AppState, path: &str) -> DockTab<Panel> {
        let mut max_id = 0;
        let dock_items = &mut state.windows[window_index].dock_items;
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
    
    pub fn focus_or_new_editor(&mut self, cx: &mut Cx, window_index: usize, state: &mut AppState, file_path: &str) -> bool {
        let mut target_ctrl_id = 0;
        let mut only_focus_editor = false;
        let dock_items = &mut state.windows[window_index].dock_items;
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
            let new_tab = self.new_file_editor_tab(window_index, state, file_path);
            let dock_items = &mut state.windows[window_index].dock_items;
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