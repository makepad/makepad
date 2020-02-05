//use syn::Type;
use makepad_render::*;
use makepad_widget::*;
use makepad_tinyserde::*;
use std::collections::HashMap;

use crate::appstorage::*;
use crate::fileeditor::*;
use crate::filetree::*;
use crate::filepanel::*;
use crate::loglist::*;
use crate::itemdisplay::*;
use crate::keyboard::*;
use crate::buildmanager::*;
use crate::homepage::*;
use crate::searchresults::*;
use crate::rusteditor::*;
use crate::jseditor::*;
use crate::plaineditor::*;

#[derive(Debug, Clone, SerRon, DeRon)]
pub enum Panel {
    LogList,
    SearchResults,
    ItemDisplay,
    Keyboard,
    FileTree,
    FileEditorTarget,
    FileEditor {path: String, scroll_pos: Vec2, editor_id: u64}
}

#[derive(Clone)]
pub struct AppWindow {
    pub desktop_window: DesktopWindow,
    pub file_panel: FilePanel,
    pub home_page: HomePage,
    pub item_display: ItemDisplay,
    pub log_list: LogList,
    pub search_results: SearchResults,
    pub keyboard: Keyboard,
    pub file_editors: FileEditors,
    pub dock: Dock<Panel>,
}

#[derive(Clone, SerRon, DeRon)]
pub struct AppWindowState {
    pub open_folders: Vec<String>,
    pub window_position: Vec2,
    pub window_inner_size: Vec2,
    pub dock_items: DockItem<Panel>,
}

#[derive(Default, Clone, SerRon, DeRon)]
pub struct AppState {
    pub windows: Vec<AppWindowState>
}

impl AppWindow {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            desktop_window: DesktopWindow {
                caption: "Makepad".to_string(),
                window: Window::new(cx),
                ..DesktopWindow::new(cx)
            },
            file_editors: FileEditors {
                rust_editor: RustEditor::new(cx),
                js_editor: JSEditor::new(cx),
                plain_editor: PlainEditor::new(cx),
                editors: HashMap::new(),
            },
            home_page: HomePage::new(cx),
            keyboard: Keyboard::new(cx),
            item_display: ItemDisplay::new(cx),
            log_list: LogList::new(cx),
            search_results: SearchResults::new(cx),
            file_panel: FilePanel::new(cx),
            dock: Dock ::new(cx),
        }
    }
    
    pub fn handle_app_window(&mut self, cx: &mut Cx, event: &mut Event, window_index: usize, state: &mut AppState, storage: &mut AppStorage, build_manager: &mut BuildManager) {
        
        match self.desktop_window.handle_desktop_window(cx, event) {
            DesktopWindowEvent::EventForOtherWindow => {
                return
            }
            DesktopWindowEvent::WindowClosed => {
                return
            }
            DesktopWindowEvent::WindowGeomChange(wc) => {
                if !storage.app_state_file_read.is_pending() {
                    // store our new window geom
                    state.windows[window_index].window_position = wc.new_geom.position;
                    state.windows[window_index].window_inner_size = wc.new_geom.inner_size;
                    storage.save_state(cx, state);
                }
            }
            _ => ()
        }
        
        match event {
            Event::KeyDown(ke) => match ke.key_code {
                KeyCode::Backtick => if ke.modifiers.logo || ke.modifiers.control {
                    if build_manager.active_builds.len() == 0 {
                        build_manager.restart_build(cx, storage);
                    }
                    let mut clear = true;
                    for ab in &build_manager.active_builds {
                        if !ab.build_uid.is_none() {
                            clear = false;
                        }
                    }
                    if clear {
                        build_manager.tail_log_items = true;
                        build_manager.log_items.truncate(0);
                    }
                    build_manager.artifact_run(storage);
                    self.show_log_tab(cx, window_index, state);
                }
                _ => ()
            }
            _ => ()
        }
        
        if self.search_results.handle_search_input(cx, event, &mut build_manager.search_index, storage) {
            self.show_search_tab(cx, window_index, state);
        }
        
        let dock_items = &mut state.windows[window_index].dock_items;
        let mut dock_walker = self.dock.walker(dock_items);
        let mut file_tree_event = FileTreeEvent::None;
        //let mut text_editor_event = TextEditorEvent::None;
        let mut set_last_cursor = None;
        let mut do_search = None;
        let mut show_item_display_tab = false;
        let mut do_display_rust_file = None;
        
        while let Some(item) = dock_walker.walk_handle_dock(cx, event) {
            match item {
                Panel::LogList => {
                    match self.log_list.handle_log_list(cx, event, storage, build_manager) {
                        LogListEvent::SelectLocMessage {loc_message, jump_to_offset} => {
                            // just make it open an editor
                            if loc_message.path.len()>0 {
                                // ok so. lets lookup the path in our remap list
                                //println!("TRYING TO SELECT FILE {} ")
                                file_tree_event = FileTreeEvent::SelectFile {path: storage.remap_sync_path(&loc_message.path)};
                            }
                            self.item_display.display_message(cx, &loc_message);
                            set_last_cursor = Some((jump_to_offset,jump_to_offset));
                            show_item_display_tab = true;
                        },
                        LogListEvent::SelectMessages {items} => {
                            self.item_display.display_plain_text(cx, &items);
                            show_item_display_tab = true;
                        }
                        _ => ()
                    }
                }
                Panel::ItemDisplay => {
                    self.item_display.handle_item_display(cx, event);
                }
                Panel::SearchResults => {
                    match self.search_results.handle_search_results(cx, event, &mut build_manager.search_index, storage) {
                        SearchResultEvent::DisplayFile {text_buffer_id, cursor} => {
                            set_last_cursor = Some(cursor);
                            do_display_rust_file = Some(text_buffer_id);
                        },
                        SearchResultEvent::OpenFile {text_buffer_id, cursor} => {
                            let path = storage.text_buffer_id_to_path.get(&text_buffer_id).expect("Path not found").clone();
                            file_tree_event = FileTreeEvent::SelectFile {path: path};
                            set_last_cursor = Some(cursor);
                        },
                        _ => ()
                    }
                }
                Panel::Keyboard => {
                    self.keyboard.handle_keyboard(cx, event, storage);
                }
                Panel::FileEditorTarget => {
                    self.home_page.handle_home_page(cx, event);
                }
                Panel::FileTree => {
                    file_tree_event = self.file_panel.handle_file_panel(cx, event);
                }
                Panel::FileEditor {path, scroll_pos, editor_id} => {
                    if let Some(file_editor) = &mut self.file_editors.editors.get_mut(editor_id) {
                        
                        let atb = storage.text_buffer_from_path(cx, path);
                        
                        match file_editor.handle_file_editor(cx, event, atb, Some(&mut build_manager.search_index)) {
                            TextEditorEvent::Search(search) => {
                                do_search = Some((Some(search), atb.text_buffer_id, true, false));
                            }
                            TextEditorEvent::Decl(search) => {
                                do_search = Some((Some(search), atb.text_buffer_id, false, false));
                            }
                            TextEditorEvent::Escape => {
                                do_search = Some((Some("".to_string()), atb.text_buffer_id, false, true));
                            }
                            TextEditorEvent::Change => {
                                do_search = Some((None, AppTextBufferId(0), false, false));
                            }
                            TextEditorEvent::LagChange => {
                                storage.text_buffer_file_write(cx, path);
                                if storage.settings.build_on_save {
                                    build_manager.restart_build(cx, storage);
                                }
                            },
                            _ => ()
                        }
                        *scroll_pos = file_editor.get_scroll_pos(cx);
                    }
                }
            }
        }
        
        if let Some((search, first_tbid, focus, escape)) = do_search {

            if let Some(search) = search {
                self.search_results.set_search_input_value(cx, &search, first_tbid, focus);
            }
            let first_result = self.search_results.do_search(cx, &mut build_manager.search_index, storage);
            if escape {
                self.show_files_tab(cx, window_index, state);
            }
            if focus {
                self.show_search_tab(cx, window_index, state);
            }
            else{
                if let Some((tbid, cursor)) = first_result{
                    set_last_cursor = Some(cursor);
                    do_display_rust_file = Some(tbid);
                }
            }            
        }
        
        if show_item_display_tab {
            self.show_item_display_tab(cx, window_index, state);
        }
        
        if let Some(tbid) = do_display_rust_file{
            let path = storage.text_buffer_id_to_path.get(&tbid).unwrap();
            if self.open_preview_editor_tab(cx, window_index, state, &path, set_last_cursor) {
                storage.save_state(cx, state);
                self.ensure_unique_tab_title_for_file_editors(cx, window_index, state);
            }
        }
        
        match file_tree_event {
            FileTreeEvent::DragMove {fe, ..} => {
                self.dock.dock_drag_move(cx, fe);
            }
            FileTreeEvent::DragCancel => {
                self.dock.dock_drag_cancel(cx);
            }
            FileTreeEvent::DragOut => {
                self.dock.dock_drag_out(cx);
            }
            FileTreeEvent::DragEnd {fe, paths} => {
                let mut tabs = Vec::new();
                for path in paths {
                    // find a free editor id
                    tabs.push(self.new_file_editor_tab(cx, &path, None, false, true));
                }
                self.dock.dock_drag_end(cx, fe, tabs);
                self.ensure_unique_tab_title_for_file_editors(cx, window_index, state);
            }
            FileTreeEvent::SelectFile {path} => {
                // search for the tabcontrol with the maximum amount of editors
                if self.focus_or_new_editor(cx, window_index, state, &path, set_last_cursor) {
                    storage.save_state(cx, state);
                    self.ensure_unique_tab_title_for_file_editors(cx, window_index, state);
                }
            }
            FileTreeEvent::SelectFolder {..} => {
                state.windows[window_index].open_folders = self.file_panel.file_tree.save_open_folders();
                storage.save_state(cx, state);
            }
            _ => {}
        }
        
        let dock_items = &mut state.windows[window_index].dock_items;
        match self.dock.handle_dock(cx, event, dock_items) {
            DockEvent::DockChanged => { // thats a bit bland event. lets let the thing know which file closed
                storage.save_state(cx, state);
            }
            DockEvent::DockTabClosed => {
                self.ensure_unique_tab_title_for_file_editors(cx, window_index, state);
                storage.save_state(cx, state);
            }
            DockEvent::DockTabCloned {tab_control_id, tab_id} => {
                // lets change up our editor_id
                let max_id = self.file_editors.highest_file_editor_id();
                let mut dock_walker = self.dock.walker(dock_items);
                while let Some((ctrl_id, dock_item)) = dock_walker.walk_dock_item() {
                    match dock_item {
                        DockItem::TabControl {tabs, ..} => if ctrl_id == tab_control_id {
                            if let Some(tab) = tabs.get_mut(tab_id) {
                                match &mut tab.item {
                                    Panel::FileEditor {editor_id, ..} => {
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
    
    pub fn draw_app_window(
        &mut self,
        cx: &mut Cx,
        menu: &Menu,
        window_index: usize,
        state: &mut AppState,
        storage: &mut AppStorage,
        build_manager: &mut BuildManager
    ) {
        if self.desktop_window.begin_desktop_window(cx, Some(menu)).is_err() {return}
        
        self.dock.draw_dock(cx);
        
        let dock_items = &mut state.windows[window_index].dock_items;
        let mut dock_walker = self.dock.walker(dock_items);
        let file_panel = &mut self.file_panel;
        let search_results = &mut self.search_results;
        let item_display = &mut self.item_display;
        while let Some(item) = dock_walker.walk_draw_dock(cx, | cx, tab_control, tab, selected | {
            // this draws the tabs, so we can customimze it
            match tab.item {
                Panel::FileTree => {
                    let tab = tab_control.get_draw_tab(cx, &tab.title, selected, tab.closeable);
                    if tab.begin_tab(cx).is_ok() {
                        file_panel.draw_file_panel_tab(cx);
                        tab.end_tab(cx);
                    };
                }
                Panel::SearchResults => {
                    let tab = tab_control.get_draw_tab(cx, &tab.title, selected, tab.closeable);
                    if tab.begin_tab(cx).is_ok() {
                        search_results.draw_search_result_tab(cx, &build_manager.search_index);
                        tab.end_tab(cx);
                    };
                }
                _ => tab_control.draw_tab(cx, &tab.title, selected, tab.closeable)
            }
        }) {
            match item {
                Panel::LogList => {
                    self.log_list.draw_log_list(cx, build_manager);
                }
                Panel::SearchResults => {
                    search_results.draw_search_results(cx, storage);
                }
                Panel::ItemDisplay => {
                    item_display.draw_item_display(cx);
                }
                Panel::Keyboard => {
                    self.keyboard.draw_keyboard(cx);
                }
                Panel::FileEditorTarget => {
                    self.home_page.draw_home_page(cx);
                }
                Panel::FileTree => {
                    file_panel.draw_file_panel(cx);
                }
                Panel::FileEditor {path, scroll_pos, editor_id} => {
                    let text_buffer = storage.text_buffer_from_path(cx, path);
                    let (file_editor, is_new) = self.file_editors.get_file_editor_for_path(path, *editor_id);
                    if is_new {
                        file_editor.set_scroll_pos_on_load(*scroll_pos);
                    }
                    file_editor.draw_file_editor(cx, text_buffer, &mut build_manager.search_index);
                }
            }
        }
        self.desktop_window.end_desktop_window(cx);
    }
    
    pub fn ensure_unique_tab_title_for_file_editors(&mut self, cx:&mut Cx, window_index: usize, state: &mut AppState) {
        // we walk through the dock collecting tab titles, if we run into a collision
        // we need to find the shortest uniqueness
        let mut collisions: HashMap<String, Vec<(String, usize, usize)>> = HashMap::new();
        let dock_items = &mut state.windows[window_index].dock_items;
        // enumerate dockspace and collect all names
        let mut dock_walker = self.dock.walker(dock_items);
        while let Some((ctrl_id, dock_item)) = dock_walker.walk_dock_item() {
            if let DockItem::TabControl {tabs, ..} = dock_item {
                for (id, tab) in tabs.iter_mut().enumerate() {
                    if let Panel::FileEditor {path, ..} = &tab.item {
                        let title = path_file_name(&path);
                        tab.title = title.clone(); // set the title here once
                        if let Some(items) = collisions.get_mut(&title) {
                            items.push((path.clone(), ctrl_id, id));
                        }
                        else {
                            collisions.insert(title, vec![(path.clone(), ctrl_id, id)]);
                        }
                    }
                }
            }
        }
        
        // walk through hashmap and update collisions with new title
        for (_, values) in &mut collisions {
            if values.len() > 1 {
                let mut splits = Vec::new();
                for (path, _, _) in values.iter_mut() {
                    // we have to find the shortest unique path combo
                    let item: Vec<String> = path.split("/").map( | v | v.to_string()).collect();
                    splits.push(item);
                }
                // compare each pair
                let mut max_equal = 0;
                for i in 0..splits.len() - 1 {
                    let a = &splits[i];
                    let b = &splits[i + 1];
                    for i in 0..a.len().min(b.len()) {
                        if a[a.len() - i - 1] != b[b.len() - i - 1] {
                            if i > max_equal {
                                max_equal = i;
                            }
                            break;
                        }
                    }
                }
                if max_equal == 0 {
                    continue;
                }
                for (index, (_, scan_ctrl_id, tab_id)) in values.iter_mut().enumerate() {
                    let split = &splits[index];
                    let mut dock_walker = self.dock.walker(dock_items);
                    while let Some((ctrl_id, dock_item)) = dock_walker.walk_dock_item() {
                        if ctrl_id != *scan_ctrl_id {
                            continue;
                        }
                        if let DockItem::TabControl {tabs, ..} = dock_item {
                            let tab = &mut tabs[*tab_id];
                            // ok lets set the tab title
                            let new_title = if max_equal == 0 {
                                split[split.len() - 1].clone()
                            }
                            else {
                                split[(split.len() - max_equal - 1)..].join("/")
                            };
                            if new_title != tab.title {
                                tab.title = new_title;
                            }
                        }
                    }
                }
            }
        }
        cx.redraw_child_area(Area::All);
    }
    
    pub fn new_file_editor_tab(&mut self, cx: &mut Cx, path: &str, set_last_cursor: Option<(usize, usize)>, at_top:bool, focus:bool) -> DockTab<Panel> {
        let editor_id = self.file_editors.highest_file_editor_id() + 1;
        let (file_editor, is_new) = self.file_editors.get_file_editor_for_path(path, editor_id);
        if is_new && focus {
            file_editor.set_key_focus(cx);
        }
        if let Some(cursor) = set_last_cursor {
            file_editor.set_last_cursor(cx, cursor,at_top);
        }
        DockTab {
            closeable: true,
            title: path_file_name(&path),
            item: Panel::FileEditor {
                path: path.to_string(),
                scroll_pos: Vec2::default(),
                editor_id: editor_id
            }
        }
    }
    
    pub fn show_log_tab(&mut self, cx: &mut Cx, window_index: usize, state: &mut AppState) {
        let mut dock_walker = self.dock.walker(&mut state.windows[window_index].dock_items);
        while let Some((_ctrl_id, dock_item)) = dock_walker.walk_dock_item() {
            if let DockItem::TabControl {current, tabs, ..} = dock_item {
                for (id, tab) in tabs.iter().enumerate() {
                    if let Panel::LogList = &tab.item {
                        if *current != id {
                            *current = id;
                            cx.redraw_child_area(Area::All);
                        }
                    }
                }
            }
        }
    }
    
    pub fn show_files_tab(&mut self, cx: &mut Cx, window_index: usize, state: &mut AppState) {
        let mut dock_walker = self.dock.walker(&mut state.windows[window_index].dock_items);
        while let Some((_ctrl_id, dock_item)) = dock_walker.walk_dock_item() {
            if let DockItem::TabControl {current, tabs, ..} = dock_item {
                for (id, tab) in tabs.iter().enumerate() {
                    if let Panel::FileTree = &tab.item {
                        if *current != id {
                            *current = id;
                            cx.redraw_child_area(Area::All);
                        }
                    }
                }
            }
        }
    }
    
    pub fn show_search_tab(&mut self, cx: &mut Cx, window_index: usize, state: &mut AppState) {
        let mut dock_walker = self.dock.walker(&mut state.windows[window_index].dock_items);
        while let Some((_ctrl_id, dock_item)) = dock_walker.walk_dock_item() {
            if let DockItem::TabControl {current, tabs, ..} = dock_item {
                for (id, tab) in tabs.iter().enumerate() {
                    if let Panel::SearchResults = &tab.item {
                        if *current != id {
                            *current = id;
                            cx.redraw_child_area(Area::All);
                        }
                    }
                }
            }
        }
    }
    
    pub fn show_item_display_tab(&mut self, cx: &mut Cx, window_index: usize, state: &mut AppState) {
        let mut dock_walker = self.dock.walker(&mut state.windows[window_index].dock_items);
        while let Some((_ctrl_id, dock_item)) = dock_walker.walk_dock_item() {
            if let DockItem::TabControl {current, tabs, ..} = dock_item {
                for (id, tab) in tabs.iter().enumerate() {
                    if let Panel::ItemDisplay = &tab.item {
                        if *current != id {
                            *current = id;
                            cx.redraw_child_area(Area::All);
                        }
                    }
                }
            }
        }
    }
    
    pub fn focus_or_new_editor(&mut self, cx: &mut Cx, window_index: usize, state: &mut AppState, file_path: &str, set_last_cursor: Option<(usize, usize)>) -> bool {
        let mut target_ctrl_id = None;
        let mut dock_walker = self.dock.walker(&mut state.windows[window_index].dock_items);
        while let Some((ctrl_id, dock_item)) = dock_walker.walk_dock_item() {
            if let DockItem::TabControl {current, tabs, ..} = dock_item {
                let mut item_ctrl_id = None;
                for (id, tab) in tabs.iter().enumerate() {
                    match &tab.item {
                        Panel::ItemDisplay => { // found the editor target
                            item_ctrl_id = Some((ctrl_id,id));
                        },
                        Panel::FileEditor {path, scroll_pos: _, editor_id} => {
                            if *path == file_path {
                                // check if we aren't the preview..
                                if let Some((item_ctrl_id, tab_id)) = item_ctrl_id{
                                    if item_ctrl_id == ctrl_id && tab_id == id - 1{
                                        continue
                                    }
                                }
                                let (file_editor, _is_new) = self.file_editors.get_file_editor_for_path(path, *editor_id);
                                file_editor.set_key_focus(cx);
                                if let Some(cursor) = set_last_cursor {
                                    file_editor.set_last_cursor(cx, cursor, false);
                                }
                                if *current != id {
                                    *current = id;
                                    cx.redraw_child_area(Area::All);
                                }
                                return false
                            }
                        },
                        Panel::FileEditorTarget => { // found the editor target
                            target_ctrl_id = Some(ctrl_id);
                        },
                        _ => ()
                    }
                }
            }
        }
        if let Some(target_ctrl_id) = target_ctrl_id { // open a new one
            let new_tab = self.new_file_editor_tab(cx, file_path, set_last_cursor, false, true);
            let mut dock_walker = self.dock.walker(&mut state.windows[window_index].dock_items);
            while let Some((ctrl_id, dock_item)) = dock_walker.walk_dock_item() {
                if ctrl_id == target_ctrl_id {
                    if let DockItem::TabControl {current, tabs, ..} = dock_item {
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
    
    pub fn open_preview_editor_tab(&mut self, cx: &mut Cx, window_index: usize, state: &mut AppState, file_path: &str, set_last_cursor: Option<(usize, usize)>) -> bool {
        
        let mut target_ctrl_id = None;
        let mut target_tab_after = 0;
        let mut dock_walker = self.dock.walker(&mut state.windows[window_index].dock_items);
        while let Some((ctrl_id, dock_item)) = dock_walker.walk_dock_item() {
            if let DockItem::TabControl {tabs, ..} = dock_item {
                for (id, tab) in tabs.iter().enumerate() {
                    match &tab.item {
                        Panel::ItemDisplay => { // found the editor target
                            target_ctrl_id = Some(ctrl_id);
                            target_tab_after = id;
                        },
                        _ => ()
                    }
                }
            }
        }
        if let Some(target_ctrl_id) = target_ctrl_id { // open a new one
            let mut dock_walker = self.dock.walker(&mut state.windows[window_index].dock_items);
            while let Some((ctrl_id, dock_item)) = dock_walker.walk_dock_item() {
                if ctrl_id == target_ctrl_id {
                    if let DockItem::TabControl {current, tabs, ..} = dock_item {
                        // already contains the editor we need, or if we need a new one
                        // check what tab is right next to ItemDisplay
                        if target_tab_after + 1 < tabs.len(){
                            match &mut tabs[target_tab_after + 1].item {
                                Panel::FileEditor {path, scroll_pos: _, editor_id} => {
                                    if self.file_editors.does_path_match_editor_type(file_path, *editor_id){
                                        *path = file_path.to_string();
                                        let (file_editor, _is_new) = self.file_editors.get_file_editor_for_path(path, *editor_id);
                                        if let Some(cursor) = set_last_cursor {
                                            file_editor.set_last_cursor(cx, cursor, true);
                                        }
                                        *current = target_tab_after + 1;
                                        cx.redraw_child_area(Area::All);
                                        return true
                                    }
                                },
                                _ => ()
                            }
                        }
                    }
                }
            }
        }
        
        if let Some(target_ctrl_id) = target_ctrl_id { // open a new one
            let new_tab = self.new_file_editor_tab(cx, file_path, set_last_cursor, true, false);
            let mut dock_walker = self.dock.walker(&mut state.windows[window_index].dock_items);
            while let Some((ctrl_id, dock_item)) = dock_walker.walk_dock_item() {
                if ctrl_id == target_ctrl_id {
                    if let DockItem::TabControl {current, tabs, ..} = dock_item {
                        tabs.insert(target_tab_after + 1, new_tab);
                        *current = target_tab_after + 1;
                        cx.redraw_child_area(Area::All);
                        return true;
                    }
                }
            }
        }

        return false
    }
}
