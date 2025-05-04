use crate::{
    makepad_code_editor::code_editor::*,
    makepad_code_editor::selection::Affinity,
    makepad_code_editor::session::SelectionMode,
    makepad_code_editor::history::NewGroup,
    makepad_widgets::*,
    makepad_micro_serde::*,
    makepad_widgets::file_tree::*,
    makepad_platform::os::cx_stdin::*,
    makepad_file_protocol::SearchItem,
    makepad_file_server::FileSystemRoots,
    file_system::file_system::*,
    studio_editor::*,
    run_view::*,
    makepad_platform::studio::{JumpToFile,EditFile, SelectInFile, PatchFile, SwapSelection},
    log_list::*,
    makepad_code_editor::{CodeSession,text::{Position}},
    ai_chat::ai_chat_manager::AiChatManager,
    build_manager::{
        build_protocol::BuildProcess,
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
    use crate::app_ui::*;
    use link::widgets::*;
    
    App = {{App}} {
        ui: <Root>{
            <AppUI> {}
        }
    }
}
 
#[derive(Live, LiveHook)]
pub struct App {
    #[live] pub ui: WidgetRef,
    #[rust] pub data: AppData,
}
 
impl LiveRegister for App{
    fn live_register(cx: &mut Cx) {
        crate::live_design(cx);
        // for macos
        cx.start_stdin_service();
    }
}

app_main!(App);

impl App {
     
    pub fn open_code_file_by_path(&mut self, cx: &mut Cx, path: &str) {
        if let Some(file_id) = self.data.file_system.path_to_file_node_id(&path) {
            let dock = self.ui.dock(id!(dock));            
            let tab_id = dock.unique_id(file_id.0);
            self.data.file_system.request_open_file(tab_id, file_id);
            let (tab_bar, pos) = dock.find_tab_bar_of_tab(live_id!(edit_first)).unwrap();
            // lets pick the template
            let template = FileSystem::get_editor_template_from_path(path);
            dock.create_and_select_tab(cx, tab_bar, tab_id, template, "".to_string(), live_id!(CloseableTab), Some(pos));
            self.data.file_system.ensure_unique_tab_names(cx, &dock)
        }
    }
    
    pub fn load_state(&mut self, cx:&mut Cx, slot:usize){
        
        if let Ok(contents) = std::fs::read_to_string(format!("makepad_state{}.ron", slot)) {
            match AppStateRon::deserialize_ron(&contents) {
                Ok(state)=>{
                    // lets kill all running processes
                    self.data.build_manager.stop_all_active_builds(cx);
                    // Now we need to apply the saved state
                    let dock = self.ui.dock(id!(dock));
                    if let Some(mut dock) = dock.borrow_mut() {
                        dock.load_state(cx, state.dock_items);
                                                
                        self.data.file_system.tab_id_to_file_node_id = state.tab_id_to_file_node_id.clone();
                        for (tab_id, file_node_id) in state.tab_id_to_file_node_id.iter() {
                            self.data.file_system.request_open_file(*tab_id, *file_node_id);
                        }
                        // ok lets run the processes
                                                                       
                        for process in state.processes{
                            if let Some(binary_id) = self.data.build_manager.binary_name_to_id(&process.binary){
                                self.data.build_manager.start_active_build(cx, binary_id, process.target);
                            }       
                        }
                    };
                    self.ui.clear_query_cache();
                    return;
                    //self.ui.redraw(cx);
                    // cx.redraw_all();
                    //self.data.build_manager.designer_selected_files = 
                     //   state.designer_selected_files;
                }
                Err(e)=>{
                    println!("ERR {:?}",e);
                }
            }
        }
    }
    
    fn save_state(&self, slot:usize){
        let dock = self.ui.dock(id!(dock));
        let dock_items = dock.clone_state().unwrap();
        
        // lets store the active build ids so we can fire them up again
        let mut processes = Vec::new();
        for build in self.data.build_manager.active.builds.values(){
            processes.push(build.process.clone());
        }
                
        //do we keep the runviews? we should eh.
        // alright lets save it to disk
        let state = AppStateRon{
            dock_items,
            processes,
            tab_id_to_file_node_id: self.data.file_system.tab_id_to_file_node_id.clone()
        };
        let saved = state.serialize_ron();
        let mut f = File::create(format!("makepad_state{}.ron", slot)).expect("Unable to create file");
        f.write_all(saved.as_bytes()).expect("Unable to write data");
    }
}

#[derive(Default)]
pub struct AppData{ 
    pub build_manager: BuildManager,
    pub file_system: FileSystem,
    pub ai_chat_manager: AiChatManager,
}

// all global app commands coming in from keybindings, and UI components

#[derive(DefaultNone, Debug, Clone)]
pub enum AppAction{
    JumpTo(JumpToFile),
    SelectInFile(SelectInFile),
    SwapSelection(SwapSelection),
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
    SendAiChatToBackend{chat_id:LiveId, history_slot:usize},
    CancelAiGeneration{chat_id:LiveId},
    SaveAiChat{chat_id:LiveId},
    RedrawAiChat{chat_id:LiveId},
    RunAiChat{chat_id:LiveId, history_slot:usize, item_id:usize},
    DestroyRunViews{run_view_id:LiveId},
    None
}

impl MatchEvent for App{
    fn handle_startup(&mut self, cx:&mut Cx){
        let mut roots = Vec::new();
        let current_dir = env::current_dir().unwrap();
        
        for arg in std::env::args(){
            if let Some(prefix) = arg.strip_prefix("--root="){
                for root in prefix.split(","){
                    let mut parts = root.splitn(2,":");
                    let base = parts.next().expect("name:path expected");
                    let path = parts.next().expect("name:path expected");
                    let dir = current_dir.clone();
                    roots.push((base.to_string(), dir.join(path).canonicalize().unwrap()));
                }
            }
            else{
            }
        }
        if roots.is_empty(){
            let dir1 = current_dir.join("./").canonicalize().unwrap();
            //roots.push(("ai_snake".to_string(),current_dir.join("../snapshots/ai_snake").canonicalize().unwrap()));
            roots.push(("makepad".to_string(),dir1));
            //roots.push(("experiments".to_string(),current_dir.join("../experiments").canonicalize().unwrap()));
        }
        let roots = FileSystemRoots{roots};
        self.data.file_system.init(cx, roots.clone());
        self.data.build_manager.init(cx, roots);
                
        //self.data.build_manager.discover_external_ip(cx);
        self.data.build_manager.start_http_server();
        // lets load the tabs
    }
    
    fn handle_action(&mut self, cx:&mut Cx, action:&Action){
        let dock = self.ui.dock(id!(dock));
        let file_tree = self.ui.view(id!(file_tree));
        let log_list = self.ui.log_list(id!(log_list));
        let run_list = self.ui.view(id!(run_list_tab));
        let profiler = self.ui.view(id!(profiler));
        let search = self.ui.view(id!(search));
        let snapshot = self.ui.view(id!(snapshot_tab));
        
        match action.cast(){
            AppAction::SwapSelection(ss)=>{
                let s1_start = Position{line_index: ss.s1_line_start as usize, byte_index:ss.s1_column_start as usize};
                let s1_end = Position{line_index: ss.s1_line_end as usize, byte_index:ss.s1_column_end as usize};
                let s2_start = Position{line_index: ss.s2_line_start as usize, byte_index:ss.s2_column_start as usize};
                let s2_end = Position{line_index: ss.s2_line_end as usize, byte_index:ss.s2_column_end as usize};
                if let Some(s1_file_id) = self.data.file_system.path_to_file_node_id(&ss.s1_file_name) {
                    if let Some(s2_file_id) = self.data.file_system.path_to_file_node_id(&ss.s1_file_name) {
                        if let Some(OpenDocument::Code(doc1)) = self.data.file_system.open_documents.get(&s1_file_id) {
                            if let Some(OpenDocument::Code(doc2)) = self.data.file_system.open_documents.get(&s2_file_id) {
                                // Create sessions
                                let mut session1 = CodeSession::new(doc1.clone());
                                let mut session2 = CodeSession::new(doc2.clone());
            
                                // Set selections in both sessions
                                session1.set_selection(session1.clamp_position(s1_start), Affinity::After, SelectionMode::Simple, NewGroup::Yes);
                                session1.move_to(session1.clamp_position(s1_end), Affinity::Before, NewGroup::Yes);
                                
                                session2.set_selection(session2.clamp_position(s2_start), Affinity::After, SelectionMode::Simple, NewGroup::Yes);
                                session2.move_to(session2.clamp_position(s2_end), Affinity::Before, NewGroup::Yes);
            
                                // Get the selected text from both sessions
                                let text1 = session1.copy();
                                let text2 = session2.copy();
                                                                            
                                // Swap the text by pasting into each session
                                session1.paste(text2.into());
                                session1.handle_changes();
                                session2.handle_changes();
                                self.data.file_system.handle_sessions();
                                
                                session2.paste(text1.into());
                                session1.handle_changes();
                                session2.handle_changes();
                                self.data.file_system.handle_sessions();
                                
                                // lets draw any views file file 1 and 2
                                cx.action(AppAction::RedrawFile(s1_file_id));
                                cx.action(AppAction::RedrawFile(s2_file_id));
                                // Handle any pending changes and save the files
                                self.data.file_system.request_save_file_for_file_node_id(s1_file_id, false);
                                //self.data.file_system.request_save_file_for_file_node_id(s2_file_id, false);
                            }
                        }
                    }
                }
            }
            AppAction::SelectInFile(sf)=>{
                let start = Position{line_index: sf.line_start as usize, byte_index:sf.column_start as usize};
                let end = Position{line_index: sf.line_end as usize, byte_index:sf.column_end as usize};
                if let Some(file_id) = self.data.file_system.path_to_file_node_id(&sf.file_name) {
                    if let Some(tab_id) = self.data.file_system.file_node_id_to_tab_id(file_id){
                        dock.select_tab(cx, tab_id);
                        // ok lets scroll into view
                        if let Some(mut editor) = dock.item(tab_id).studio_code_editor(id!(editor)).borrow_mut() {
                            if let Some(EditSession::Code(session)) = self.data.file_system.get_session_mut(tab_id) {
                                editor.editor.set_selection_and_scroll(cx, start, end, session);
                                editor.editor.set_key_focus(cx);
                            }
                        }
                    }
                }
            }
            AppAction::JumpTo(jt)=>{
                let pos = Position{line_index: jt.line as usize, byte_index:jt.column as usize};
                if let Some(file_id) = self.data.file_system.path_to_file_node_id(&jt.file_name) {
                    if let Some(tab_id) = self.data.file_system.file_node_id_to_tab_id(file_id){
                        dock.select_tab(cx, tab_id);
                        // ok lets scroll into view
                        if let Some(mut editor) = dock.item(tab_id).studio_code_editor(id!(editor)).borrow_mut() {
                            if let Some(EditSession::Code(session)) = self.data.file_system.get_session_mut(tab_id) {
                                editor.editor.set_cursor_and_scroll(cx, pos, session);
                                editor.editor.set_key_focus(cx);
                            }
                        }
                    }
                    else{
                        // lets open the editor
                        let tab_id = dock.unique_id(file_id.0);
                        self.data.file_system.request_open_file(tab_id, file_id);
                        // lets add a file tab 'somewhere'
                        let (tab_bar, pos) = dock.find_tab_bar_of_tab(live_id!(edit_first)).unwrap();
                        let template = FileSystem::get_editor_template_from_path(&jt.file_name);
                        dock.create_and_select_tab(cx, tab_bar, tab_id, template, "".to_string(), live_id!(CloseableTab), Some(pos));
                        // lets scan the entire doc for duplicates
                        self.data.file_system.ensure_unique_tab_names(cx, &dock)
                    }
                }
            }
            AppAction::PatchFile(_ef)=>{
                panic!()
                /*let start = Position{line_index: ef.line as usize, byte_index:ef.column_start as usize};
                let end = Position{line_index: ef.line as usize, byte_index:ef.column_end as usize};
                if let Some(file_id) = self.data.file_system.path_to_file_node_id(&ef.file_name) {
                    if let Some(tab_id) = self.data.file_system.file_node_id_to_tab_id(file_id){
                        //dock.select_tab(cx, tab_id);
                        // ok lets scroll into view
                        if let Some(mut editor) = dock.item(tab_id).studio_code_editor(id!(editor)).borrow_mut() {
                            if let Some(EditSession::Code(session)) = self.data.file_system.get_session_mut(tab_id) {
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
                }*/
            }
            AppAction::EditFile(ef)=>{
                let start = Position{line_index: ef.line_start as usize, byte_index:ef.column_start as usize};
                let end = Position{line_index: ef.line_end as usize, byte_index:ef.column_end as usize};
                if let Some(file_id) = self.data.file_system.path_to_file_node_id(&ef.file_name) {
                    if let Some(tab_id) = self.data.file_system.file_node_id_to_tab_id(file_id){
                        dock.select_tab(cx, tab_id);
                        // ok lets scroll into view
                        if let Some(mut editor) = dock.item(tab_id).studio_code_editor(id!(editor)).borrow_mut() {
                            if let Some(EditSession::Code(session)) = self.data.file_system.get_session_mut(tab_id) {
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
                self.data.file_system.file_client.load_file_tree();
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
                            if run_view.build_id == Some(build_id) {
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
            AppAction::None=>(),
            AppAction::SendAiChatToBackend{chat_id, history_slot}=>{
                self.data.ai_chat_manager.send_chat_to_backend(cx, chat_id, history_slot, &mut self.data.file_system);
            }
            AppAction::CancelAiGeneration{chat_id}=>{
                self.data.ai_chat_manager.cancel_chat_generation(cx, &self.ui, chat_id,  &mut self.data.file_system);
            }
            AppAction::SaveAiChat{chat_id}=>{
                self.data.file_system.request_save_file_for_file_node_id(chat_id, false);
            }
            AppAction::RedrawAiChat{chat_id}=>{
                self.data.ai_chat_manager.redraw_ai_chat_by_id(cx, chat_id,&self.ui,  &mut self.data.file_system);
            }
            AppAction::RunAiChat{chat_id, history_slot, item_id}=>{
                self.data.ai_chat_manager.run_ai_chat(cx, chat_id, history_slot, item_id, &mut self.data.file_system);
            }
            AppAction::DestroyRunViews{run_view_id} => {
                dock.close_tab(cx, run_view_id);
                dock.close_tab(cx, run_view_id.add(1));
                dock.close_tab(cx, run_view_id.add(2));
                dock.redraw(cx);
                log_list.redraw(cx);
            }
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
                                item.build_id = Some(build_id);
                                item.kind_id = WindowKindId::from_usize(kind_id);
                            }
                            else{
                                println!("WHIT");
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
                                    if run_view.build_id == Some(build_id){
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
                                    if run_view.build_id == Some(build_id) && run_view.window_id == presentable_draw.window_id{
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
                self.load_state(cx, 0);
                self.data.ai_chat_manager.init(&mut self.data.file_system);
                //self.open_code_file_by_path(cx, "examples/slides/src/app.rs");
            }
            FileSystemAction::SnapshotImageLoaded => {
                snapshot.redraw(cx);
            }
            FileSystemAction::RecompileNeeded => {
                self.data.build_manager.start_recompile_timer(cx);
            }
            FileSystemAction::LiveReloadNeeded(live_file_change) => {
                self.data.build_manager.live_reload_needed(live_file_change);
                self.data.build_manager.clear_log(cx, &dock, &mut self.data.file_system);
                log_list.redraw(cx);
            }
            FileSystemAction::FileChangedOnDisk(_res)=>{
                
            }
            FileSystemAction::SearchResults=>{
                search.redraw(cx);
            }
            FileSystemAction::None=>()
        }
        
        if let Some(action) = action.as_widget_action(){
            match action.cast(){
                CodeEditorAction::UnhandledKeyDown(ke) if ke.key_code == KeyCode::F12 && !ke.modifiers.shift =>{
                    if let Some(word) = self.data.file_system.get_word_under_cursor_for_session(action.path.from_end(1)){
                        dock.select_tab(cx, live_id!(search));
                        let set = vec![SearchItem{
                            needle:word.clone(), 
                            prefixes: Some(vec![
                                format!("struct "),
                                format!("enum "),
                                format!("fn "),
                                format!("type "),
                                format!("trait "),
                                format!("pub ")
                            ]),
                            pre_word_boundary:true,
                            post_word_boundary:true
                        }];
                        search.text_input(id!(search_input)).set_text(cx, &word);
                        self.data.file_system.search_string(cx, set);
                    } 
                },
                CodeEditorAction::UnhandledKeyDown(ke) if ke.key_code == KeyCode::F12 && ke.modifiers.shift =>{
                    if let Some(word) = self.data.file_system.get_word_under_cursor_for_session(action.path.from_end(1)){
                        dock.select_tab(cx, live_id!(search));
                        let set = vec![SearchItem{
                            needle:word.clone(), 
                            prefixes: None,
                            pre_word_boundary:ke.modifiers.control,
                            post_word_boundary:ke.modifiers.control
                        }];
                        search.text_input(id!(search_input)).set_text(cx, &word);
                        self.data.file_system.search_string(cx, set);
                    } 
                },
                CodeEditorAction::TextDidChange => {
                    // lets write the file
                    self.data.file_system.request_save_file_for_tab_id(action.path.from_end(1), false)
                }
                CodeEditorAction::UnhandledKeyDown(_)=>{}
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
                                let tab_id = dock.unique_id(internal_id.0);
                                dock.drop_clone(cx, drop_event.abs, *internal_id, tab_id, live_id!(CloseableTab));
                            }
                            else {
                                dock.drop_move(cx, drop_event.abs, *internal_id);
                            }
                            self.data.file_system.ensure_unique_tab_names(cx, &dock);
                        }
                        else { // external file, we have to create a new tab
                            if let Some(file_id) = self.data.file_system.path_to_file_node_id(&path) {
                                let tab_id = dock.unique_id(file_id.0);
                                self.data.file_system.request_open_file(tab_id, file_id);
                                let template = FileSystem::get_editor_template_from_path(&path);
                                dock.drop_create(cx, drop_event.abs, tab_id, template, "".to_string(), live_id!(CloseableTab));
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
                cx.action(AppAction::ClearLog);
                cx.action(AppAction::RecompileStarted);
                cx.action(AppAction::StartRecompile);
            }
            else if let KeyCode::KeyK = key_code {
                cx.action(AppAction::ClearLog);
            }
            else if let KeyCode::KeyR = key_code{
                cx.action(AppAction::ReloadFileTree);
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
        
        for (i,id) in [*id!(preset_1),*id!(preset_2),*id!(preset_3),*id!(preset_4)].iter().enumerate(){
            if let Some(km) = self.ui.button(id).pressed_modifiers(actions){
                if km.control{
                    self.save_state(i+1)
                }
                else{
                    self.load_state(cx, i+1);
                    cx.redraw_all();
                }
            }
        }
            
        if let Some(file_id) = file_tree.file_clicked(&actions) {
            // ok lets open the file
            if let Some(tab_id) = self.data.file_system.file_node_id_to_tab_id(file_id) {
                // If the tab is already open, focus it
                dock.select_tab(cx, tab_id);
            } else {
                let tab_id = dock.unique_id(file_id.0);
                self.data.file_system.request_open_file(tab_id, file_id);
                self.data.file_system.request_open_file(tab_id, file_id);
                                
                // lets add a file tab 'some
                let path = self.data.file_system.file_node_id_to_path(file_id).unwrap();
                let tab_after = FileSystem::get_tab_after_from_path(path);
                let (tab_bar, pos) = dock.find_tab_bar_of_tab(tab_after).unwrap();
                let template = FileSystem::get_editor_template_from_path(path);
                dock.create_and_select_tab(cx, tab_bar, tab_id, template, "".to_string(), live_id!(CloseableTab), Some(pos));
                
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
        self.data.ai_chat_manager.handle_event(cx, event, &mut self.data.file_system);
        if self.ui.dock(id!(dock)).check_and_clear_need_save(){
            self.save_state(0);
        }
    }
}

// we should store probably also scroll position / which chat slot we're visiting
use std::collections::HashMap;
#[derive(SerRon, DeRon)]
pub struct AppStateRon{
    dock_items: HashMap<LiveId, DockItem>,
    processes: Vec<BuildProcess>,
    tab_id_to_file_node_id: HashMap<LiveId, LiveId>,
}
