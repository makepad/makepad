use crate::{
    makepad_code_editor::code_editor::*,
    makepad_code_editor::selection::Affinity,
    makepad_code_editor::session::SelectionMode,
    makepad_code_editor::history::NewGroup,
    makepad_widgets::*,
    makepad_micro_serde::*,
    makepad_widgets::file_tree::*,
    makepad_platform::os::cx_stdin::*,
    file_system::file_system::*,
    studio_editor::*,
    run_view::*,
    makepad_platform::studio::{JumpToFile,EditFile, PatchFile},
    run_list::*,
    log_list::*,
    makepad_code_editor::text::{Position},
    build_manager::{
        build_manager::{
            BuildManager,
            BuildManagerAction
        },
    }
};   
use std::fs::File;
use std::io::Write;
use std::env;
  
live_design!{
    import crate::app_ui::*;

    App = {{App}} {
        ui: <AppUI> {}
    }
}
 
#[derive(Live, LiveHook)]
pub struct App {
    #[live] ui: WidgetRef,
    #[rust] data: AppData,
}

impl LiveRegister for App{
    fn live_register(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
        crate::makepad_code_editor::live_design(cx);
        crate::run_list::live_design(cx);
        crate::log_list::live_design(cx);
        crate::profiler::live_design(cx);
        crate::run_view::live_design(cx);
        crate::studio_editor::live_design(cx);
        crate::studio_file_tree::live_design(cx);
        crate::app_ui::live_design(cx);
        // for macos
        cx.start_stdin_service();
    }
}

app_main!(App);

impl App {
    pub fn open_code_file_by_path(&mut self, cx: &mut Cx, path: &str) {
        if let Some(file_id) = self.data.file_system.path_to_file_node_id(&path) {
            let dock = self.ui.dock(id!(dock));            
            let tab_id = dock.unique_tab_id(file_id.0);
            self.data.file_system.request_open_file(tab_id, file_id);
            let (tab_bar, pos) = dock.find_tab_bar_of_tab(live_id!(edit_first)).unwrap();
            dock.create_and_select_tab(cx, tab_bar, tab_id, live_id!(CodeEditor), "".to_string(), live_id!(CloseableTab), Some(pos));
            self.data.file_system.ensure_unique_tab_names(cx, &dock)
        }
    }
}

#[derive(Default)]
pub struct AppData{ 
    pub build_manager: BuildManager,
    pub file_system: FileSystem,
}

// all global app commands coming in from keybindings, and UI components

#[derive(DefaultNone, Debug, Clone)]
pub enum AppAction{
    JumpTo(JumpToFile),
    RedrawLog,
    RedrawProfiler,
    RedrawFile(LiveId),
    FocusDesign(LiveId),
    EditFile(EditFile),
    PatchFile(PatchFile),
    StartRecompile,
    ReloadFileTree,
    RecompileStarted,
    ClearLog, 
    None
}

impl MatchEvent for App{
    fn handle_startup(&mut self, cx:&mut Cx){
        let mut root = "./".to_string();
        for arg in std::env::args(){
            if let Some(prefix) = arg.strip_prefix("--root="){
                root = prefix.to_string();
                break;
            }
        }
        let root_path = env::current_dir().unwrap().join(root);
                
        self.data.file_system.init(cx, &root_path);
        self.data.build_manager.init(cx, &root_path);
        //self.data.build_manager.discover_external_ip(cx);
        self.data.build_manager.start_http_server();
    }
    
    fn handle_action(&mut self, cx:&mut Cx, action:&Action){
        let dock = self.ui.dock(id!(dock));
        let file_tree = self.ui.view(id!(file_tree));
        let log_list = self.ui.log_list(id!(log_list));
        let run_list = self.ui.view(id!(run_list));
        let profiler = self.ui.view(id!(profiler));
        match action.cast(){
            AppAction::JumpTo(jt)=>{
                let pos = Position{line_index: jt.line as usize, byte_index:jt.column as usize};
                if let Some(file_id) = self.data.file_system.path_to_file_node_id(&jt.file_name) {
                    if let Some(tab_id) = self.data.file_system.file_node_id_to_tab_id(file_id){
                        dock.select_tab(cx, tab_id);
                        // ok lets scroll into view
                        if let Some(mut editor) = dock.item(tab_id).studio_editor(id!(editor)).borrow_mut() {
                            if let Some(session) = self.data.file_system.get_session_mut(tab_id) {
                                editor.editor.set_cursor_and_scroll(cx, pos, session);
                                editor.editor.set_key_focus(cx);
                            }
                        }
                    }
                    else{
                        // lets open the editor
                        let tab_id = dock.unique_tab_id(file_id.0);
                        self.data.file_system.request_open_file(tab_id, file_id);
                        // lets add a file tab 'somewhere'
                        let (tab_bar, pos) = dock.find_tab_bar_of_tab(live_id!(edit_first)).unwrap();
                        dock.create_and_select_tab(cx, tab_bar, tab_id, live_id!(StudioEditor), "".to_string(), live_id!(CloseableTab), Some(pos));
                        // lets scan the entire doc for duplicates
                        self.data.file_system.ensure_unique_tab_names(cx, &dock)
                    }
                }
            }
            AppAction::PatchFile(ef)=>{
                let start = Position{line_index: ef.line as usize, byte_index:ef.column_start as usize};
                let end = Position{line_index: ef.line as usize, byte_index:ef.column_end as usize};
                if let Some(file_id) = self.data.file_system.path_to_file_node_id(&ef.file_name) {
                    if let Some(tab_id) = self.data.file_system.file_node_id_to_tab_id(file_id){
                        //dock.select_tab(cx, tab_id);
                        // ok lets scroll into view
                        if let Some(mut editor) = dock.item(tab_id).studio_editor(id!(editor)).borrow_mut() {
                            if let Some(session) = self.data.file_system.get_session_mut(tab_id) {
                                // alright lets do 
                                session.set_selection(
                                    start,
                                    Affinity::After,
                                    SelectionMode::Simple,
                                    NewGroup::No
                                );
                                session.move_to(
                                    end,
                                    Affinity::Before,
                                    NewGroup::No 
                                );
                                session.paste_grouped(ef.replace.into(), ef.undo_group);
                            }
                            self.data.file_system.handle_sessions();
                            editor.redraw(cx);
                            self.data.file_system.request_save_file_for_file_node_id(file_id, true)
                        }
                    }
                }
            }
            AppAction::EditFile(ef)=>{
                let start = Position{line_index: ef.line_start as usize, byte_index:ef.column_start as usize};
                let end = Position{line_index: ef.line_end as usize, byte_index:ef.column_end as usize};
                if let Some(file_id) = self.data.file_system.path_to_file_node_id(&ef.file_name) {
                    if let Some(tab_id) = self.data.file_system.file_node_id_to_tab_id(file_id){
                        dock.select_tab(cx, tab_id);
                        // ok lets scroll into view
                        if let Some(mut editor) = dock.item(tab_id).studio_editor(id!(editor)).borrow_mut() {
                            if let Some(session) = self.data.file_system.get_session_mut(tab_id) {
                                // alright lets do 
                                session.set_selection(
                                    start,
                                    Affinity::After,
                                    SelectionMode::Simple,
                                    NewGroup::Yes
                                );
                                session.move_to(
                                    end,
                                    Affinity::Before,
                                    NewGroup::Yes
                                );
                                session.paste(ef.replace.into());
                                // lets serialise the session
                            }
                            self.data.file_system.handle_sessions();
                            editor.redraw(cx);
                            self.data.file_system.request_save_file_for_file_node_id(file_id, false)
                        }
                    }
                }
            }
            AppAction::RedrawFile(file_id)=>{
                self.data.file_system.redraw_view_by_file_id(cx, file_id, &dock);
            }
            AppAction::ClearLog=>{
                self.data.build_manager.clear_log(cx, &dock, &mut self.data.file_system);
                log_list.reset_scroll(cx);
                log_list.redraw(cx);
                profiler.redraw(cx);
            }
            AppAction::ReloadFileTree=>{
                self.data.file_system.reload_file_tree();
            }
            AppAction::RedrawProfiler=>{
                profiler.redraw(cx);
            }
            AppAction::RedrawLog=>{
                log_list.redraw(cx);
            }
            AppAction::StartRecompile=>{
                self.data.build_manager.start_recompile(cx);
            }
            AppAction::FocusDesign(build_id)=>{
                let mut id = None;
                if let Some(mut dock) = dock.borrow_mut() {
                    for (tab_id, (_, item)) in dock.items().iter() {
                        if let Some(run_view) = item.as_run_view().borrow_mut() {
                            if run_view.build_id == build_id {
                                if let WindowKindId::Design = run_view.kind_id{
                                    // lets focus this tab
                                    id = Some(*tab_id);
                                    break;
                                }
                            }
                        }
                    }
                }
                if let Some(id) = id{
                    dock.select_tab(cx, id);
                }
            }
            AppAction::RecompileStarted=>{
                if let Some(mut dock) = dock.borrow_mut() {
                    for (_id, (_, item)) in dock.items().iter() {
                        if let Some(mut run_view) = item.as_run_view().borrow_mut() {
                            run_view.recompile_started(cx);
                            run_view.resend_framebuffer(cx);
                        }
                    }
                }
            }
            AppAction::None=>()
        }
                
        match action.cast(){
            BuildManagerAction::StdinToHost {build_id, msg} => {
                match msg{
                    StdinToHost::CreateWindow{window_id, kind_id}=>{
                        let panel_id = build_id.add(window_id as u64);
                        if let Some(name) = self.data.build_manager.process_name(build_id){
                            
                            let (tab_bar_id, pos) = if kind_id == 0{
                                dock.find_tab_bar_of_tab(live_id!(run_first)).unwrap()
                            }
                            else if kind_id == 1{ 
                                dock.find_tab_bar_of_tab(live_id!(design_first)).unwrap()
                            }
                            else{
                                dock.find_tab_bar_of_tab(live_id!(outline_first)).unwrap()
                            };
                            
                            
                            // we might already have it
                            
                            let item = dock.create_and_select_tab(cx, tab_bar_id, panel_id, live_id!(RunView), name.clone(), live_id!(CloseableTab), Some(pos)).unwrap();
                            
                            if let Some(mut item) = item.as_run_view().borrow_mut(){
                                item.window_id = window_id;
                                item.build_id = build_id;
                                item.kind_id = WindowKindId::from_usize(kind_id);
                            }
                            
                            dock.redraw(cx);
                            log_list.redraw(cx);                        
                        }
                    }
                    StdinToHost::SetCursor(cursor) => {
                        cx.set_cursor(cursor)
                    }
                    StdinToHost::ReadyToStart => { 
                        // lets fetch all our runviews
                        if let Some(mut dock) = dock.borrow_mut() {
                            for (_, (_, item)) in dock.items().iter() {
                                if let Some(mut run_view) = item.as_run_view().borrow_mut() {
                                    if run_view.build_id == build_id{
                                        run_view.ready_to_start(cx);
                                    }
                                }
                            }
                        }
                    }
                    StdinToHost::DrawCompleteAndFlip(presentable_draw) => {
                        if let Some(mut dock) = dock.borrow_mut() {
                            for (_, (_, item)) in dock.items().iter() {
                                if let Some(mut run_view) = item.as_run_view().borrow_mut() {
                                    if run_view.build_id == build_id && run_view.window_id == presentable_draw.window_id{
                                        run_view.draw_complete_and_flip(cx, &presentable_draw, &mut self.data.build_manager);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            BuildManagerAction::None=>()
        }
                
        match action.cast(){
            FileSystemAction::TreeLoaded => {
                file_tree.redraw(cx);
                //self.open_code_file_by_path(cx, "examples/slides/src/app.rs");
            }
            FileSystemAction::RecompileNeeded => {
                self.data.build_manager.start_recompile_timer(cx);
            }
            FileSystemAction::LiveReloadNeeded(live_file_change) => {
                self.data.build_manager.live_reload_needed(live_file_change);
                //self.data.build_manager.clear_log(cx, &dock, &mut self.data.file_system);
                log_list.redraw(cx);
            }
            FileSystemAction::None=>()
        }
                
        match action.cast(){
            RunListAction::Create(..) => {
                
            }
            RunListAction::Destroy(run_view_id) => {
                dock.close_tab(cx, run_view_id);
                dock.close_tab(cx, run_view_id.add(1));
                dock.close_tab(cx, run_view_id.add(2));
                dock.redraw(cx);
                log_list.redraw(cx);
            }
            RunListAction::None=>{}
        }
        
        if let Some(action) = action.as_widget_action(){
            match action.cast(){
                CodeEditorAction::TextDidChange => {
                    // lets write the file
                    self.data.file_system.request_save_file_for_tab_id(action.path.from_end(1), false)
                }
                CodeEditorAction::None=>{}
            }
            
            match action.cast(){
                DockAction::TabCloseWasPressed(tab_id)=>{
                    dock.close_tab(cx, tab_id);
                    if self.data.build_manager.handle_tab_close(tab_id) {
                        log_list.redraw(cx);
                        run_list.redraw(cx);
                    }
                    self.data.file_system.remove_tab(tab_id);
                    self.data.file_system.ensure_unique_tab_names(cx, &dock);
                }
                DockAction::ShouldTabStartDrag(tab_id)=>{
                    dock.tab_start_drag(cx, tab_id, DragItem::FilePath {
                        path: "".to_string(), //String::from("file://") + &*path.into_unix_string().to_string_lossy(),
                        internal_id: Some(tab_id)
                    });
                }
                DockAction::Drag(drag_event)=>{
                    if drag_event.items.len() == 1 {
                        if drag_event.modifiers.logo {
                            dock.accept_drag(cx, drag_event, DragResponse::Copy);
                        }
                        else {
                            dock.accept_drag(cx, drag_event, DragResponse::Move);
                        }
                    }
                }
                DockAction::Drop(drop_event)=>{
                    if let DragItem::FilePath {path, internal_id} = &drop_event.items[0] {
                        if let Some(internal_id) = internal_id { // from inside the dock
                            if drop_event.modifiers.logo {
                                let tab_id = dock.unique_tab_id(internal_id.0);
                                dock.drop_clone(cx, drop_event.abs, *internal_id, tab_id, live_id!(CloseableTab));
                            }
                            else {
                                dock.drop_move(cx, drop_event.abs, *internal_id);
                            }
                            self.data.file_system.ensure_unique_tab_names(cx, &dock);
                        }
                        else { // external file, we have to create a new tab
                            if let Some(file_id) = self.data.file_system.path_to_file_node_id(&path) {
                                let tab_id = dock.unique_tab_id(file_id.0);
                                self.data.file_system.request_open_file(tab_id, file_id);
                                dock.drop_create(cx, drop_event.abs, tab_id, live_id!(StudioEditor), "".to_string(), live_id!(CloseableTab));
                                self.data.file_system.ensure_unique_tab_names(cx, &dock)
                            }
                        }
                    }
                },
                _=>()
            }
        }
    }        
        
    fn handle_key_down(&mut self, cx: &mut Cx, event: &KeyEvent){
        let KeyEvent {
            key_code,
            modifiers: KeyModifiers {logo, control, ..},
            ..
        } = event;
        if *control || *logo {
            if let KeyCode::Backtick = key_code {
                cx.action(AppAction::StartRecompile)
            }
            else if let KeyCode::KeyK = key_code {
                cx.action(AppAction::ClearLog)
            }
            else if let KeyCode::KeyR = key_code{
                cx.action(AppAction::ReloadFileTree)
            }
        }
    }
    
    fn handle_actions(&mut self, cx: &mut Cx, actions:&Actions){
        let file_tree = self.ui.file_tree(id!(file_tree));
        let dock = self.ui.dock(id!(dock));
        for action in actions{
            self.handle_action(cx, action);
        }
        if let Some(file_id) = file_tree.should_file_start_drag(&actions) {
            let path = self.data.file_system.file_node_path(file_id);
            file_tree.file_start_drag(cx, file_id, DragItem::FilePath {
                path,
                internal_id: None
            }); 
        }
                            
        if let Some(file_id) = file_tree.file_clicked(&actions) {
            // ok lets open the file
            if let Some(tab_id) = self.data.file_system.file_node_id_to_tab_id(file_id) {
                // If the tab is already open, focus it
                dock.select_tab(cx, tab_id);
            } else {
                let tab_id = dock.unique_tab_id(file_id.0);
                self.data.file_system.request_open_file(tab_id, file_id);
                // lets add a file tab 'some
                let (tab_bar, pos) = dock.find_tab_bar_of_tab(live_id!(edit_first)).unwrap();
                dock.create_and_select_tab(cx, tab_bar, tab_id, live_id!(StudioEditor), "".to_string(), live_id!(CloseableTab), Some(pos));
                                            
                // lets scan the entire doc for duplicates
                self.data.file_system.ensure_unique_tab_names(cx, &dock)
            }
        }
    }
    
    fn handle_shutdown(&mut self, _cx:&mut Cx){
        self.data.build_manager.clear_active_builds();
    }
}

impl AppMain for App {
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::with_data(&mut self.data));
        
        self.data.file_system.handle_event(cx, event, &self.ui);
        self.data.build_manager.handle_event(cx, event, &mut self.data.file_system); 

        // process events on all run_views
        let dock = self.ui.dock(id!(dock));
        /*
        if let Some(mut dock) = dock.borrow_mut() {
            for (id, (_, item)) in dock.items().iter() {
                if let Some(mut run_view) = item.as_run_view().borrow_mut() {
                    run_view.pump_event_loop(cx, event, *id, &mut self.data.build_manager);
                }
            }
        }*/
         
        if let Some(mut dock_items) = dock.needs_save(){
            dock_items.retain(|di| {
                if let DockItemStore::Tab{kind,..} = di{
                    if kind.0 == live_id!(RunView){
                        return false
                    }
                }
                true 
            }); 
            let state = PersistentState{
                dock_items
            };
            // alright lets save it to disk
            let saved = state.serialize_ron();
            let mut f = File::create("makepad_state.ron").expect("Unable to create file");
            f.write_all(saved.as_bytes()).expect("Unable to write data");
        }
    }
}

#[derive(Clone, Debug, SerRon, DeRon)]
struct PersistentState{
    dock_items: Vec<DockItemStore>
}
