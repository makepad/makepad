use {
    std::collections::{HashMap, hash_map},
    std::path::{Path,PathBuf},
    std::sync::Arc,
    std::cell::RefCell,
    crate::{
        makepad_code_editor::{CodeDocument, decoration::{Decoration, DecorationSet}, CodeSession},
        makepad_platform::makepad_live_compiler::LiveFileChange,
        makepad_widgets::*,
        makepad_widgets::file_tree::*,
        file_system::FileClient,
        makepad_file_server::FileSystemRoots,
        ai_chat::ai_chat_manager::AiChatDocument,
        makepad_file_protocol::{
            SearchResult,
            SearchItem,
            FileRequest,
            FileError,
            FileResponse,
            FileClientMessage,
            FileNotification,
            FileNodeData,
            FileTreeData,
            GitLog,
            SaveKind,
            SaveFileResponse
        },
    },
};

#[derive(Default)]
pub struct FileSystem {
    pub file_client: FileClient,
    pub file_nodes: LiveIdMap<LiveId, FileNode>,
    pub path_to_file_node_id: HashMap<String, LiveId>,
    pub tab_id_to_file_node_id: HashMap<LiveId, LiveId>,
    pub tab_id_to_session: HashMap<LiveId, EditSession>,
    pub open_documents: HashMap<LiveId, OpenDocument>,
    pub git_logs: Vec<GitLog>,
    pub snapshot_image_data: RefCell<HashMap<String, SnapshotImageData>>,
    pub search_results_id: u64,
    pub search_results: Vec<SearchResult>
}

pub enum SnapshotImageData{
    Loading,
    Error,
    Loaded{data:Arc<Vec<u8>>, path:PathBuf}
}


pub enum EditSession {
    Code(CodeSession),
    AiChat(LiveId)
}

pub enum OpenDocument {
    CodeLoading(DecorationSet),
    Code(CodeDocument),
    AiChatLoading,
    AiChat(AiChatDocument)
}

#[derive(Debug)]
pub struct FileNode {
    pub parent_edge: Option<FileEdge>,
    pub name: String,
    pub child_edges: Option<Vec<FileEdge >>,
}

impl FileNode {
    pub fn is_file(&self) -> bool {
        self.child_edges.is_none()
    }
}

#[derive(Debug)]
pub struct FileEdge {
    pub name: String,
    pub file_node_id: LiveId,
}

#[derive(DefaultNone, Debug, Clone)]
pub enum FileSystemAction {
    TreeLoaded,
    RecompileNeeded,
    LiveReloadNeeded(LiveFileChange),
    FileChangedOnDisk(SaveFileResponse),
    SnapshotImageLoaded,
    SearchResults,
    None
}

impl FileSystem {
    pub fn get_editor_template_from_path(path:&str)->LiveId{
        let p = Path::new(path);
        match p.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext_str| ext_str.to_lowercase()) {
            Some(ext) => match ext.as_str() {
                "mpai"=>live_id!(AiChat),
                _=>live_id!(CodeEditor)
            }
            _=>{
                live_id!(CodeEditor)
            }
        }
    }
    
    pub fn get_tab_after_from_path(path:&str)->LiveId{
        match Self::get_editor_template_from_path(path){
            live_id!(AiChat)=>live_id!(ai_first),
            _=>live_id!(edit_first),
        }
    }
         
            
    pub fn load_file_tree(&self) {
        self.file_client.send_request(FileRequest::LoadFileTree {with_data: false});
    }
            
    pub fn load_snapshot_image(&self, root:&str, hash:&str) {
        let mut image_data = self.snapshot_image_data.borrow_mut();
        if image_data.get(hash).is_none(){
            image_data.insert(root.to_string(), SnapshotImageData::Loading);
            self.file_client.send_request(FileRequest::LoadSnapshotImage {root:root.to_string(), hash:hash.to_string()});
        }
        
    }
            
    pub fn get_editor_template_from_file_id(&self, file_id:LiveId)->Option<LiveId>{
        if let Some(path) = self.file_node_id_to_path(file_id){
            Some(Self::get_editor_template_from_path(path))
        }
        else{
            None
        }
    }
    
    pub fn init(&mut self, cx: &mut Cx, roots:FileSystemRoots) {
        self.file_client.init(cx, roots);
        self.file_client.load_file_tree();
    }
    
    pub fn search_string(&mut self, _cx:&mut Cx, set:Vec<SearchItem>){
        self.search_results_id += 1;
        self.search_results.clear();
        self.file_client.send_request(FileRequest::Search{
            id: self.search_results_id,
            set
        });
        //cx.action( FileSystemAction::SearchResults );
    }
    
    pub fn remove_tab(&mut self, tab_id: LiveId) {
        self.tab_id_to_file_node_id.remove(&tab_id);
        self.tab_id_to_session.remove(&tab_id);
    }
    
    pub fn path_to_file_node_id(&self, path: &str) -> Option<LiveId> {
        self.path_to_file_node_id.get(path).cloned()
    }
    
    pub fn file_node_id_to_path(&self, file_id:LiveId) -> Option<&str> {
        for (path, id) in &self.path_to_file_node_id{
            if *id == file_id{
                return Some(path)
            }
        }
        None
    }
    
    pub fn file_node_id_to_tab_id(&self, file_node: LiveId) -> Option<LiveId> {
        for (tab, id) in &self.tab_id_to_file_node_id {
            if *id == file_node {
                return Some(*tab)
            }
        }
        None
    }
    
    pub fn get_word_under_cursor_for_session(&mut self, tab_id: LiveId)->Option<String> {
        if let Some(EditSession::Code(session)) = self.tab_id_to_session.get(&tab_id){
            return session.word_at_cursor();
        }
        None
    }
    
    pub fn get_session_mut(&mut self, tab_id: LiveId) -> Option<&mut EditSession> {
        // lets see if we have a document yet
        if let Some(file_id) = self.tab_id_to_file_node_id.get(&tab_id) {
            match self.open_documents.get(file_id){
                Some(OpenDocument::Code(document))=>{
                    return Some(match self.tab_id_to_session.entry(tab_id) {
                        hash_map::Entry::Occupied(o) => o.into_mut(),
                        hash_map::Entry::Vacant(v) => {
                            v.insert(EditSession::Code(CodeSession::new(document.clone())))            
                        }
                    })
                }
                Some(OpenDocument::AiChat(_document))=>{
                    return Some(match self.tab_id_to_session.entry(tab_id) {
                        hash_map::Entry::Occupied(o) => o.into_mut(),
                        hash_map::Entry::Vacant(v) => {
                            v.insert(EditSession::AiChat(*file_id))
                        }
                    })
                }
                Some(_)| None=>()
            }
        }
        None
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, ui: &WidgetRef) {
        
        if let Event::Signal = event{
            while let Ok(message) = self.file_client.inner.as_mut().unwrap().message_receiver.try_recv() {
                match message {
                    FileClientMessage::Response(response) => match response {
                        FileResponse::SearchInProgress(_)=>{
                        }
                        FileResponse::SaveSnapshotImage(_)=>{
                        }
                        FileResponse::LoadSnapshotImage(response)=>{
                            // lets store this in our snapshot cache
                            match response{
                                Ok(res)=>{
                                    let path = Path::new(&res.hash).to_path_buf().with_extension(".png");
                                    self.snapshot_image_data.borrow_mut().insert(res.hash, 
                                        SnapshotImageData::Loaded{
                                            data:Arc::new(res.data),
                                            path
                                        });
                                    cx.action( FileSystemAction::SnapshotImageLoaded);
                                }
                                Err(res)=>{
                                    self.snapshot_image_data.borrow_mut().insert(res.hash, SnapshotImageData::Error);
                                    cx.action( FileSystemAction::SnapshotImageLoaded);
                                }
                            }
                        }
                        FileResponse::LoadFileTree(response) => {
                            self.process_load_file_tree(response.unwrap());
                            cx.action(FileSystemAction::TreeLoaded)
                            // dock.select_tab(cx, dock, state, live_id!(file_tree).into(), live_id!(file_tree).into(), Animate::No);
                        }
                        FileResponse::OpenFile(result) => {
                            match result {
                                Ok(response) => {
                                    let file_id = LiveId(response.id);
                                    let dock = ui.dock(id!(dock));
                                    for (tab_id, file_id) in &self.tab_id_to_file_node_id {
                                        if response.id == file_id.0 {
                                            dock.redraw_tab(cx, *tab_id);
                                        }
                                    }
                                    match self.open_documents.get(&file_id){
                                        Some(OpenDocument::CodeLoading(dec))=>{
                                            let dec = dec.clone();
                                            self.open_documents.insert(file_id, OpenDocument::Code(CodeDocument::new(response.data.into(), dec)));
                                        }
                                        Some(OpenDocument::Code(_))=>{
                                        }
                                        Some(OpenDocument::AiChatLoading)=>{
                                             self.open_documents.insert(file_id, OpenDocument::AiChat(AiChatDocument::load_or_empty(&response.data)));
                                        }
                                        Some(OpenDocument::AiChat(_))=>{
                                        }
                                        _=>panic!()
                                    }
                                    
                                    dock.redraw(cx);
                                }
                                Err(FileError::CannotOpen(_unix_path)) => {
                                }
                                Err(FileError::RootNotFound(_unix_path)) => {
                                }
                                Err(FileError::Unknown(err)) => {
                                    log!("File error unknown {}", err);
                                    // ignore
                                }
                            }
                        }
                        FileResponse::SaveFile(result) => match result {
                            Ok(response) => {
                                self.process_save_response(cx, response);
                            }
                            Err(_) => {}
                            // ok we saved a file, we should check however what changed
                            // to see if we need a recompile
                            
                        }
                    },
                    FileClientMessage::Notification(notification) => {
                        match notification{
                            FileNotification::SearchResults{id, results} =>{
                                if self.search_results_id == id{
                                    self.search_results.extend(results);
                                }
                                cx.action( FileSystemAction::SearchResults );
                            }
                            FileNotification::FileChangedOnDisk(response)=>{
                               //println!("FILE CHANGED ON DISK {}", response.path);
                                if let Some(file_id) = self.path_to_file_node_id.get(&response.path){
                                    
                                    if let Some(OpenDocument::Code(doc)) = self.open_documents.get_mut(&file_id){
                                        doc.replace(response.new_data.clone().into());
                                    }
                                    ui.redraw(cx);
                                }
                                self.process_save_response(cx, response.clone());
                                // alright now what.
                                // we should chuck this into the load comparison
                                cx.action( FileSystemAction::FileChangedOnDisk(response));
                            }
                        }
                        //self.editors.handle_collab_notification(cx, &mut state.editor_state, notification)
                    }
                }
            }
        }
    }
    
    pub fn replace_live_design(&self, cx:&mut Cx, file_id:LiveId, new_data:&str){
        let mut old_neg = Vec::new();
        let mut new_neg = Vec::new();
        
        match self.open_documents.get(&file_id){
            Some(OpenDocument::Code(doc))=>{
                let old_data = doc.as_text().to_string();
                match LiveRegistry::tokenize_from_str_live_design(&old_data, Default::default(), Default::default(), Some(&mut old_neg)) {
                    Err(e) => {
                        log!("Cannot tokenize old file {}", e)
                    }
                    Ok(old_tokens) if old_tokens.len()>2  => match LiveRegistry::tokenize_from_str_live_design(new_data, Default::default(), Default::default(), Some(&mut new_neg)) {
                        Err(e) => {
                            log!("Cannot tokenize new file {}", e);
                        }
                        Ok(new_tokens) if new_tokens.len()>2 => {
                            let old_start = old_tokens[0].span.start.to_byte_offset(&old_data);
                            let old_end = old_tokens.iter().rev().nth(1).unwrap().span.end.to_byte_offset(&old_data);
                            let new_start = new_tokens[0].span.start.to_byte_offset(&new_data);
                            let new_end = new_tokens.iter().rev().nth(1).unwrap().span.end.to_byte_offset(&new_data);
                            if old_start.is_none() || old_end.is_none() || new_start.is_none() || new_end.is_none(){
                                log!("Cannot find range correctly {:?} {:?} {:?} {:?}", old_start, old_end, new_start, new_end);
                            }
                            else{
                                let mut combined_data = old_data.to_string();
                                combined_data.replace_range(old_start.unwrap()..old_end.unwrap(), &new_data[new_start.unwrap()..new_end.unwrap()]);
                                cx.action( FileSystemAction::LiveReloadNeeded(LiveFileChange {
                                    file_name: self.file_node_id_to_path(file_id).unwrap().to_string(),
                                    content: combined_data.to_string(),
                                }));
                                doc.replace(combined_data.into());
                            }
                        }
                        _ => {
                            log!("Cannot tokenize new file");
                        }
                    }
                    _ => {
                        log!("Cannot tokenize new file");
                    }
                }
            }
            _=>()
        }
                
    }
    
    
    pub fn process_possible_live_reload(&mut self, cx:&mut Cx, path:&str, old_data:&str, new_data:&str, recompile:bool){
        let mut old_neg = Vec::new();
        let mut new_neg = Vec::new();
        match LiveRegistry::tokenize_from_str_live_design(old_data, Default::default(), Default::default(), Some(&mut old_neg)) {
            Err(e) => {
                log!("Cannot tokenize old file {}", e)
            }
            Ok(old_tokens) => match LiveRegistry::tokenize_from_str_live_design(new_data, Default::default(), Default::default(), Some(&mut new_neg)) {
                Err(e) => {
                    log!("Cannot tokenize new file {}", e);
                }
                Ok(new_tokens) => {
                    // we need the space 'outside' of these tokens
                    if recompile && old_neg != new_neg {
                        cx.action(FileSystemAction::RecompileNeeded)
                    }
                    if old_tokens != new_tokens{
                        // design code changed, hotreload it
                        cx.action( FileSystemAction::LiveReloadNeeded(LiveFileChange {
                            file_name: path.to_string(),
                            content: new_data.to_string(),
                        }));
                    }
                }
            }
        }
    }
    
    
    pub fn process_save_response(&mut self, cx:&mut Cx, response:SaveFileResponse){
        // alright file has been saved
        // now we need to check if a live_design!{} changed or something outside it
        if Self::get_editor_template_from_path(&response.path) != live_id!(CodeEditor){
            return
        }
        
        if response.old_data != response.new_data && response.kind != SaveKind::Patch {
            self.process_possible_live_reload(cx, &response.path, &response.old_data, &response.new_data, true);
        }
    }
    
    pub fn handle_sessions(&mut self) {
        for session in self.tab_id_to_session.values_mut() {
            match session{
                EditSession::Code(session)=>{
                    session.handle_changes();
                }
                EditSession::AiChat(_id)=>{
                }
            }
        }
    }
    
    pub fn request_open_file(&mut self, tab_id: LiveId, file_id: LiveId) {
        // ok lets see if we have a document
        // ifnot, we create a new one
        if tab_id != LiveId(0){
            self.tab_id_to_file_node_id.insert(tab_id, file_id);
        }
            
        // fetch decoration set
        let dec = match self.open_documents.get(&file_id){
            Some(OpenDocument::CodeLoading(_))=> if let Some(OpenDocument::CodeLoading(dec)) = self.open_documents.remove(&file_id){
                dec
            }
            else{
                panic!()
            },
            Some(OpenDocument::Code(_))=>{
                return
            }
            Some(_) | None=>DecorationSet::new()
        };
        
        let template = self.get_editor_template_from_file_id(file_id);
        
        match template{
            Some(live_id!(CodeEditor))=>{
                self.open_documents.insert(file_id, OpenDocument::CodeLoading(dec));
            }
            Some(live_id!(AiChat))=>{
                self.open_documents.insert(file_id, OpenDocument::AiChatLoading);
            }
            None=>{
                error!("File id {:?} does not have a template", file_id);
                return
            }
            _=>panic!()
        }
        
        let path = self.file_node_path(file_id);
        self.file_client.send_request(FileRequest::OpenFile{path, id: file_id.0});
    }
    
    pub fn request_save_file_for_tab_id(&mut self, tab_id: LiveId, was_patch:bool) {
        // ok lets see if we have a document
        // ifnot, we create a new one
        if let Some(file_id) = self.tab_id_to_file_node_id.get(&tab_id) {
            self.request_save_file_for_file_node_id(*file_id, was_patch)
        };
    }
    
    pub fn replace_code_document(&self, file_id:LiveId, text:&str){
        match self.open_documents.get(&file_id){
            Some(OpenDocument::Code(doc))=>{
                doc.replace(text.into());
            }
            _=>()
        }
        
    }
    
    pub fn file_path_as_string(&self, path:&str)->Option<String>{
        if let Some(file_id) = self.path_to_file_node_id(&path){
            self.file_id_as_string(file_id)
        }
        else{
            None
        }
    }
    
    pub fn file_id_as_string(&self, file_id: LiveId)->Option<String>{
        match self.open_documents.get(&file_id){
            Some(OpenDocument::Code(doc))=>{
                Some(doc.as_text().to_string())
            }
            Some(OpenDocument::CodeLoading(_))=>{
                None
            }
            Some(OpenDocument::AiChat(doc))=>{
                Some(doc.file.to_string())
            }
            _=>None
        }
    }
    
    pub fn request_save_file_for_file_node_id(&mut self, file_id: LiveId, patch:bool) {
        if let Some(text) = self.file_id_as_string(file_id){
            let path = self.file_node_path(file_id);
            self.file_client.send_request(FileRequest::SaveFile{
                path: path.clone(), 
                data: text, 
                id: file_id.0,
                patch
            });
        }
    }
    
    pub fn clear_decorations(&mut self, file_node_id: &LiveId) {
        // ok lets see if we have a document
        // ifnot, we create a new one
        match self.open_documents.get_mut(file_node_id) {
            Some(OpenDocument::CodeLoading(dec)) => dec.clear(),
            Some(OpenDocument::Code(doc)) => doc.clear_decorations(),
            Some(_) | None=>()
        };
    }
    
    pub fn clear_all_decorations(&mut self) {
        // ok lets see if we have a document
        // ifnot, we create a new one
        for document in self.open_documents.values_mut() {
            match document {
                OpenDocument::CodeLoading(dec) => dec.clear(),
                OpenDocument::Code(doc) => doc.clear_decorations(),
                _=>()
            }
        }
    }
    
    pub fn redraw_view_by_file_id(&mut self, cx: &mut Cx, id: LiveId, dock: &DockRef) {
        for (tab_id, file_id) in &self.tab_id_to_file_node_id {
            if id == *file_id {
                dock.item(*tab_id).redraw(cx)
            }
        }
    }
    
    pub fn redraw_all_views(&mut self, cx: &mut Cx, dock: &DockRef) {
        for (tab_id, _) in &self.tab_id_to_file_node_id {
            dock.item(*tab_id).redraw(cx)
        }
    }
    
    pub fn add_decoration(&mut self, file_id: LiveId, dec: Decoration) {
        // ok lets see if we have a document
        // ifnot, we create a new one
        match self.open_documents.get_mut(&file_id) {
            Some(OpenDocument::CodeLoading(decs)) => decs.add_decoration(dec),
            Some(OpenDocument::Code(doc)) => {
                doc.add_decoration(dec);
            }
            Some(_) =>{}
            None => {
                let mut set = DecorationSet::new();
                set.add_decoration(dec);
                self.open_documents.insert(file_id, OpenDocument::CodeLoading(set));
            }
        };
    }
    
    pub fn draw_file_node(&self, cx: &mut Cx2d, file_node_id: LiveId, level: usize, file_tree: &mut FileTree) {
        if let Some(file_node) = self.file_nodes.get(&file_node_id) {
            match &file_node.child_edges {
                Some(child_edges) => {
                    if level == 0{
                        for child_edge in child_edges {
                            self.draw_file_node(cx, child_edge.file_node_id, level + 1, file_tree);
                        }
                    }
                    else{
                        if file_tree.begin_folder(cx, file_node_id, &file_node.name).is_ok() {
                            for child_edge in child_edges {
                                self.draw_file_node(cx, child_edge.file_node_id, level + 1, file_tree);
                            }
                            file_tree.end_folder();
                        }
                    }
                }
                None => {
                    file_tree.file(cx, file_node_id, &file_node.name);
                }
            }
        }
    }
    
    pub fn file_node_name(&self, file_node_id: LiveId) -> String {
        self.file_nodes.get(&file_node_id).unwrap().name.clone()
    }
    
    pub fn file_node_path(&self, file_node_id: LiveId) -> String {
        let mut path = String::new();
        let mut file_node = &self.file_nodes[file_node_id];
        while let Some(edge) = &file_node.parent_edge {
            path.insert_str(0, &edge.name);
            file_node = &self.file_nodes[edge.file_node_id];
            if file_node.parent_edge.is_some() {
                path.insert_str(0, "/");
            }
        }
        path
    }
    pub fn ensure_unique_tab_names(&self, cx: &mut Cx, dock: &DockRef) {
                
        fn longest_common_suffix(a: &[&str], b: &[&str]) -> Option<usize> {
            if a == b{
                return None // same file
            }
            let mut ai = a.len();
            let mut bi = b.len();
            let mut count = 0;
            while ai > 0 && bi > 0 {
                ai -= 1;
                bi -= 1;
                if a[ai] == b[bi] {
                    count += 1;
                } else {
                    break;
                }
            }
            Some(count)
        }
        // Collect the path components for each open tab
        let mut tabs: Vec<(LiveId, Vec<&str>, usize)> = Vec::new();
        for (&tab_id, &file_id) in &self.tab_id_to_file_node_id {
            let mut path_components = Vec::new();
            let mut file_node = &self.file_nodes[file_id];
            
            while let Some(edge) = &file_node.parent_edge {
                // Collect references to the file node names without cloning
                path_components.push(edge.name.as_str());
                file_node = &self.file_nodes[edge.file_node_id];
            }
            // Reverse the components so they go from root to leaf
            path_components.reverse();
            
            tabs.push((tab_id, path_components, 1));
        }
        
        // Sort the tabs by their path components
        tabs.sort_by(|a, b| a.1.cmp(&b.1));
        
        // Determine the minimal unique suffix for each tab
        let mut changing = true;
        while changing{
            changing = false;
            for i in 0..tabs.len() {
                let (_, ref path, minsfx) = tabs[i];
                let mut min_suffix_len = minsfx;
                // Compare with previous tab
                if i > 0 {
                    let (_, ref prev_path, _) = tabs[i - 1];
                    if let Some(common)= longest_common_suffix(path, prev_path){
                        min_suffix_len = min_suffix_len.max(common + 1)
                    }
                }
                // Compare with next tab
                if i + 1 < tabs.len() {
                    let (_, ref next_path, minsfx) = tabs[i + 1];
                    if let Some(common) = longest_common_suffix(path, next_path){
                        min_suffix_len = min_suffix_len.max(common + 1).max(minsfx);
                    }
                    else{
                        min_suffix_len = minsfx;
                    }
                }
                // lets store this one 
                let (_,_, ref mut minsfx) = tabs[i];
                if *minsfx != min_suffix_len{
                    changing = true;
                    *minsfx = min_suffix_len;
                }
            }
        }
        for i in 0..tabs.len() {
            let (tab_id, ref path, minsfx) = tabs[i];
            let start = path.len().saturating_sub(minsfx);
            let title = path[start..].join("/");
            dock.set_tab_title(cx, tab_id, title);
        }
        
    }
    
    pub fn process_load_file_tree(&mut self, tree_data: FileTreeData) {
        fn create_file_node(
            file_node_id: Option<LiveId>,
            node_path: String,
            path_to_file_id: &mut HashMap<String, LiveId>,
            file_nodes: &mut LiveIdMap<LiveId, FileNode>,
            parent_edge: Option<FileEdge>,
            node: FileNodeData,
            git_logs: &mut Vec<GitLog>
        ) -> LiveId {
            let file_node_id = file_node_id.unwrap_or(LiveId::from_str(&node_path).into());
            let name = parent_edge.as_ref().map_or_else(
                || String::from("root"),
                | edge | edge.name.clone(),
            );
            let node = FileNode {
                parent_edge,
                name,
                child_edges: match node {
                    FileNodeData::Directory {entries, git_log} => Some({
                        if let Some(git_log) = git_log{
                            git_logs.push(git_log);
                        }
                        entries
                            .into_iter()
                            .map( | entry | FileEdge {
                            name: entry.name.clone(),
                            file_node_id: create_file_node(
                                None,
                                if node_path.len()>0 {
                                    format!("{}/{}", node_path, entry.name.clone())
                                }
                                else {
                                    format!("{}", entry.name.clone())
                                },
                                path_to_file_id,
                                file_nodes,
                                Some(FileEdge {
                                    name: entry.name,
                                    file_node_id,
                                }),
                                entry.node,
                                git_logs,
                            ),
                        })
                            .collect::<Vec<_ >> ()
                    }),
                    FileNodeData::File {..} => None,
                },
            };
            path_to_file_id.insert(node_path, file_node_id);
            file_nodes.insert(file_node_id, node);
            file_node_id
        }
        
        self.file_nodes.clear();
        
        create_file_node(
            Some(live_id!(root).into()),
            "".to_string(),
            &mut self.path_to_file_node_id,
            &mut self.file_nodes,
            None,
            tree_data.root,
            &mut self.git_logs
        );
    }
}