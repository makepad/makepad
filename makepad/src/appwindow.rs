//use syn::Type;
use render::*;
use widget::*;
use serde::*;
use editor::*;

use crate::appstorage::*;
use crate::fileeditor::*;
use crate::filetree::*;
use crate::filepanel::*;
use crate::loglist::*;
use crate::logitem::*;
use crate::keyboard::*;
use crate::buildmanager::*;
use crate::homepage::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Panel {
    LogList,
    LogItem,
    Keyboard,
    FileTree,
    FileEditorTarget,
    FileEditor {path: String, scroll_pos:Vec2, editor_id: u64}
}

#[derive(Clone)]
pub struct AppWindow {
    pub desktop_window: DesktopWindow,
    pub file_panel: FilePanel,
    pub home_page: HomePage,
    pub log_item: LogItem,
    pub log_list: LogList,
    pub keyboard: Keyboard,
    pub file_editors: Elements<u64, FileEditor, FileEditorTemplates>,
    pub dock: Dock<Panel>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct AppWindowState {
    pub open_folders: Vec<String>,
    pub window_position: Vec2,
    pub window_inner_size: Vec2,
    pub dock_items: DockItem<Panel>,
}

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct AppState {
    pub windows: Vec<AppWindowState>
}

impl AppWindow {
    pub fn style(cx: &mut Cx) -> Self {
        Self {
            desktop_window: DesktopWindow {
                caption: "Makepad".to_string(),
                window: Window::proto(cx),
                ..DesktopWindow::proto(cx)
            },
            file_editors: Elements::new(FileEditorTemplates {
                rust_editor: RustEditor::proto(cx),
                js_editor: JSEditor::proto(cx),
                plain_editor: PlainEditor::proto(cx)
            }),
            home_page: HomePage::proto(cx),
            keyboard: Keyboard::proto(cx),
            log_item: LogItem::proto(cx),
            log_list: LogList::proto(cx),
            file_panel: FilePanel::proto(cx),
            dock: Dock ::proto(cx),
        }
    }
    
    pub fn handle_app_window(&mut self, cx: &mut Cx, event: &mut Event, window_index: usize, state: &mut AppState, storage: &mut AppStorage, build_manager: &mut BuildManager) {
        
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
                    match self.log_list.handle_log_list(cx, event, storage, build_manager) {
                         LogListEvent::SelectLocMessage {loc_message} => {
                            // just make it open an editor
                            if loc_message.path.len()>0 {
                                file_tree_event = FileTreeEvent::SelectFile {path: loc_message.path.clone()};
                            }
                            self.log_item.load_loc_message(cx, &loc_message);
                        },
                        LogListEvent::SelectMessages {items} => {
                            self.log_item.load_plain_text(cx, &items);
                        }
                        _ => ()
                    }
                },
                Panel::LogItem => {
                    self.log_item.handle_log_item(cx, event);
                },
                Panel::Keyboard => {
                    self.keyboard.handle_keyboard(cx, event, storage);
                },
                Panel::FileEditorTarget => {
                    self.home_page.handle_home_page(cx, event);
                },
                Panel::FileTree => {
                    file_tree_event = self.file_panel.handle_file_panel(cx, event);
                },
                Panel::FileEditor {path, scroll_pos, editor_id} => {
                    if let Some(file_editor) = &mut self.file_editors.get(*editor_id) {
                        
                        let text_buffer = storage.text_buffer_from_path(cx, path);
                        
                        match file_editor.handle_file_editor(cx, event, text_buffer) {
                            FileEditorEvent::LagChange => {
                                
                                // HERE WE SAVE
                                storage.text_buffer_file_write(cx, path);
                                
                                // lets re-trigger the rust compiler
                                if storage.settings.build_on_save {
                                    build_manager.restart_build(cx, storage);
                                    self.log_item.clear_msg(cx);
                                }
                                
                                //app_global.rust_compiler.restart_rust_checker(cx, &mut app_global.text_buffers);
                            },
                            _ => ()
                        }
                        *scroll_pos = file_editor.get_scroll_pos(cx);
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
                    tabs.push(self.new_file_editor_tab(/*window_index, state,*/ &path));
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
                state.windows[window_index].open_folders = self.file_panel.file_tree.save_open_folders();
                storage.save_state(cx, state);
            },
            _ => {}
        }
        
        let dock_items = &mut state.windows[window_index].dock_items;
        match self.dock.handle_dock(cx, event, dock_items) {
            DockEvent::DockChanged => { // thats a bit bland event. lets let the thing know which file closed
                storage.save_state(cx, state);
            },
            DockEvent::DockTabClosed => {
                storage.save_state(cx, state);
            },
            DockEvent::DockTabCloned {tab_control_id, tab_id} => {
                // lets change up our editor_id
                let max_id = self.highest_file_editor_id();
                let mut dock_walker = self.dock.walker(dock_items);
                while let Some((ctrl_id,dock_item)) = dock_walker.walk_dock_item() {
                    match dock_item {
                        DockItem::TabControl {tabs, ..} => if ctrl_id == tab_control_id {
                            if let Some(tab) = tabs.get_mut(tab_id){
                                match &mut tab.item {
                                    Panel::FileEditor {editor_id,..} => {
                                        // we need to make a new editor_id here.
                                        *editor_id = max_id + 1;
                                        break;
                                        // and now it needs to scroll the new one....
                                    },
                                    _ => ()
                                }
                            }
                        },
                        _ => ()
                    }
                }
                storage.save_state(cx, state);
            },
            _ => ()
        }
    }
    
    pub fn draw_app_window(&mut self, cx: &mut Cx, menu:&Menu, window_index: usize, state: &mut AppState, storage: &mut AppStorage, build_manager: &mut BuildManager) {
        if self.desktop_window.begin_desktop_window(cx, Some(menu)).is_err() {return}

        self.dock.draw_dock(cx);
        
        let dock_items = &mut state.windows[window_index].dock_items;
        let mut dock_walker = self.dock.walker(dock_items);
        let file_panel = &mut self.file_panel;
        while let Some(item) = dock_walker.walk_draw_dock(cx, |cx, tab_control, tab, selected|{
            // this draws the tabs, so we can customimze it
            match tab.item{
                Panel::FileTree=>{
                    // we can now draw our own things 
                    let tab = tab_control.get_draw_tab(cx, &tab.title, selected, tab.closeable);
                    // lets open up a draw API in the tab 
                    if tab.begin_tab(cx).is_ok(){
                        file_panel.draw_tab_buttons(cx);
                        tab.end_tab(cx);
                    };
                },
                _=>tab_control.draw_tab(cx, &tab.title, selected, tab.closeable)
            }
        }) {
            match item {
                Panel::LogList => {
                    self.log_list.draw_log_list(cx, build_manager);
                },
                Panel::LogItem => {
                    self.log_item.draw_log_item(cx);
                },
                Panel::Keyboard => {
                    self.keyboard.draw_keyboard(cx);
                },
                Panel::FileEditorTarget => {
                    self.home_page.draw_home_page(cx);
                },
                Panel::FileTree => {
                    file_panel.draw_file_panel(cx);
                },
                Panel::FileEditor {path, scroll_pos, editor_id} => {
                    let text_buffer = storage.text_buffer_from_path(cx, path);
                    let mut set_key_focus = false;
                    let file_editor = self.file_editors.get_draw(cx, *editor_id, | _cx, tmpl | {
                        set_key_focus = true;
                        let mut editor = FileEditor::create_file_editor_for_path(path, tmpl);
                        editor.set_scroll_pos_on_load(*scroll_pos);
                        editor
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
    
    pub fn highest_file_editor_id(&self) -> u64 {
        let mut max_id = 0;
        for id in &self.file_editors.element_list {
            if *id > max_id {
                max_id = *id;
            }
        }
        max_id
    }
    
    pub fn new_file_editor_tab(&self, path: &str) -> DockTab<Panel> {
        DockTab {
            closeable: true,
            title: path_file_name(&path),
            item: Panel::FileEditor {
                path: path.to_string(),
                scroll_pos:Vec2::zero(),
                editor_id: self.highest_file_editor_id() + 1
            }
        }
    }
    
    pub fn focus_or_new_editor(&mut self, cx: &mut Cx, window_index: usize, state: &mut AppState, file_path: &str) -> bool {
        let mut target_ctrl_id = None;
        let dock_items = &mut state.windows[window_index].dock_items;
        let mut dock_walker = self.dock.walker(dock_items);
        // first find existing editor, or 
        while let Some((ctrl_id, dock_item)) = dock_walker.walk_dock_item() {
            match dock_item {
                DockItem::TabControl {current, previous:_, tabs} => {
                    for (id, tab) in tabs.iter().enumerate() {
                        match &tab.item {
                            Panel::FileEditor {path, scroll_pos:_, editor_id} => {
                                if *path == file_path {
                                    *current = id;
                                    if let Some(file_editor) = &mut self.file_editors.get(*editor_id) {
                                        file_editor.set_key_focus(cx);
                                    }
                                    cx.redraw_child_area(Area::All);
                                    return false
                                }
                            },
                            Panel::FileEditorTarget => { // found the editor target
                                target_ctrl_id = Some(ctrl_id);
                            },
                            _ => ()
                        }
                    }
                },
                _ => ()
            }
        }
        if let Some(target_ctrl_id) = target_ctrl_id{ // open a new one
            let new_tab = self.new_file_editor_tab(file_path);
            let dock_items = &mut state.windows[window_index].dock_items;
            let mut dock_walker = self.dock.walker(dock_items);
            while let Some((ctrl_id, dock_item)) = dock_walker.walk_dock_item() {
                if ctrl_id == target_ctrl_id { 
                    if let DockItem::TabControl {current, previous:_, tabs} = dock_item {
                        tabs.insert(*current + 1, new_tab);
                        *current = *current + 1;
                        cx.redraw_child_area(Area::All);
                        return true;
                    }
                }
            }
        }
        return false
    }
}