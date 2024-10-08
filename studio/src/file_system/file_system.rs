use {
    std::collections::{HashMap, hash_map},
    std::path::Path,
    crate::{
        makepad_code_editor::{Document, decoration::{Decoration, DecorationSet}, Session},
        makepad_platform::makepad_live_compiler::LiveFileChange,
        makepad_widgets::*,
        makepad_widgets::file_tree::*,
        file_system::FileClient,
        makepad_file_protocol::{
            FileRequest,
            FileError,
            FileResponse,
            FileClientMessage,
            FileNotification,
            FileNodeData,
            FileTreeData,
            SaveKind,
            SaveFileResponse
        },
    },
};

#[derive(Default)]
pub struct FileSystem {
    pub file_client: FileClient,
    pub root_path: String,
    pub file_nodes: LiveIdMap<LiveId, FileNode>,
    pub path_to_file_node_id: HashMap<String, LiveId>,
    pub tab_id_to_file_node_id: HashMap<LiveId, LiveId>,
    pub tab_id_to_session: HashMap<LiveId, Session>,
    pub open_documents: HashMap<LiveId, OpenDoc>
}

pub enum OpenDoc {
    Decorations(DecorationSet),
    Document(Document)
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
    None
}

impl FileSystem {
    pub fn init(&mut self, cx: &mut Cx, path:&Path) {
        self.file_client.init(cx, path);
        self.reload_file_tree();
    }
    
    pub fn reload_file_tree(&mut self) {
        self.file_client.send_request(FileRequest::LoadFileTree {with_data: false});
    }
    
    pub fn remove_tab(&mut self, tab_id: LiveId) {
        self.tab_id_to_file_node_id.remove(&tab_id);
        self.tab_id_to_session.remove(&tab_id);
    }
    
    pub fn path_to_file_node_id(&self, path: &str) -> Option<LiveId> {
        self.path_to_file_node_id.get(path).cloned()
    }
    
    pub fn file_node_id_to_tab_id(&self, file_node: LiveId) -> Option<LiveId> {
        for (tab, id) in &self.tab_id_to_file_node_id {
            if *id == file_node {
                return Some(*tab)
            }
        }
        None
    }
    
    pub fn get_session_mut(&mut self, tab_id: LiveId) -> Option<&mut Session> {
        // lets see if we have a document yet
        if let Some(file_id) = self.tab_id_to_file_node_id.get(&tab_id) {
            if let Some(OpenDoc::Document(document)) = self.open_documents.get(file_id) {
                return Some(match self.tab_id_to_session.entry(tab_id) {
                    hash_map::Entry::Occupied(o) => o.into_mut(),
                    hash_map::Entry::Vacant(v) => v.insert(Session::new(document.clone()))
                })
            }
        }
        None
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, ui: &WidgetRef) {
        
        if let Event::Signal = event{
            while let Ok(message) = self.file_client.inner.as_mut().unwrap().message_receiver.try_recv() {
                match message {
                    FileClientMessage::Response(response) => match response {
                        FileResponse::LoadFileTree(response) => {
                            self.load_file_tree(response.unwrap());
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
                                    if let Some(OpenDoc::Decorations(dec)) = self.open_documents.get(&file_id) {
                                        let dec = dec.clone();
                                        self.open_documents.insert(file_id, OpenDoc::Document(Document::new(response.data.into(), dec)));
                                    }else {panic!()}
                                    dock.redraw(cx);
                                }
                                Err(FileError::CannotOpen(_unix_path)) => {
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
                            FileNotification::FileChangedOnDisk(response)=>{
                               
                                if let Some(file_id) = self.path_to_file_node_id.get(&response.path){
                                    
                                    if let Some(OpenDoc::Document(doc)) = self.open_documents.get_mut(&file_id){
                                        if let Some(tab_id) = 
                                        // lets grab a session id for our first session
                                        self.tab_id_to_file_node_id.iter()
                                        .find_map(|(k, v)| if v == file_id { Some(k) } else { None }){
                                            let session = self.tab_id_to_session.get(&tab_id).unwrap();
                                            doc.replace(session.id(), response.new_data.clone().into());
                                        }
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
    
    pub fn process_save_response(&mut self, cx:&mut Cx, response:SaveFileResponse){
        // alright file has been saved
        // now we need to check if a live_design!{} changed or something outside it
        
        if response.old_data != response.new_data && response.kind != SaveKind::Patch {
            let mut old_neg = Vec::new();
            let mut new_neg = Vec::new();
            match LiveRegistry::tokenize_from_str_live_design(&response.old_data, Default::default(), Default::default(), Some(&mut old_neg)) {
                Err(e) => {
                    log!("Cannot tokenize old file {}", e)
                }
                Ok(old_tokens) => match LiveRegistry::tokenize_from_str_live_design(&response.new_data, Default::default(), Default::default(), Some(&mut new_neg)) {
                    Err(e) => {
                        log!("Cannot tokenize new file {}", e);
                    }
                    Ok(new_tokens) => {
                        // we need the space 'outside' of these tokens
                        if old_neg != new_neg {
                            cx.action(FileSystemAction::RecompileNeeded)
                        }
                        if old_tokens != new_tokens{
                            // design code changed, hotreload it
                            cx.action( FileSystemAction::LiveReloadNeeded(LiveFileChange {
                                file_name: response.path,
                                content: response.new_data
                            }));
                        }
                    }
                }
            }
        }
    }
    
    pub fn handle_sessions(&mut self) {
        for session in self.tab_id_to_session.values_mut() {
            session.handle_changes();
        }
    }
    
    pub fn request_open_file(&mut self, tab_id: LiveId, file_id: LiveId) {
        // ok lets see if we have a document
        // ifnot, we create a new one
        self.tab_id_to_file_node_id.insert(tab_id, file_id);
        // move decorations to doc
        let dec = match self.open_documents.get(&file_id){
            Some(OpenDoc::Decorations(_))=> if let Some(OpenDoc::Decorations(dec)) = self.open_documents.remove(&file_id){
                dec
            }
            else{
                panic!()
            },
            Some(OpenDoc::Document(_))=>{
                return
            }
            None=>DecorationSet::new()
        };
        self.open_documents.insert(file_id, OpenDoc::Decorations(dec));
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
    
    pub fn request_save_file_for_file_node_id(&mut self, file_id: LiveId, patch:bool) {
        if let Some(OpenDoc::Document(doc)) = self.open_documents.get(&file_id) {
            let text = doc.as_text().to_string();
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
            Some(OpenDoc::Decorations(dec)) => dec.clear(),
            Some(OpenDoc::Document(doc)) => doc.clear_decorations(),
            None => ()
        };
    }
    
    pub fn clear_all_decorations(&mut self) {
        // ok lets see if we have a document
        // ifnot, we create a new one
        for document in self.open_documents.values_mut() {
            match document {
                OpenDoc::Decorations(dec) => dec.clear(),
                OpenDoc::Document(doc) => doc.clear_decorations(),
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
            Some(OpenDoc::Decorations(decs)) => decs.add_decoration(dec),
            Some(OpenDoc::Document(doc)) => {
                doc.add_decoration(dec);
            }
            None => {
                let mut set = DecorationSet::new();
                set.add_decoration(dec);
                self.open_documents.insert(file_id, OpenDoc::Decorations(set));
            }
        };
    }
    
    
    pub fn draw_file_node(&self, cx: &mut Cx2d, file_node_id: LiveId, file_tree: &mut FileTree) {
        if let Some(file_node) = self.file_nodes.get(&file_node_id) {
            match &file_node.child_edges {
                Some(child_edges) => {
                    if file_tree.begin_folder(cx, file_node_id, &file_node.name).is_ok() {
                        for child_edge in child_edges {
                            self.draw_file_node(cx, child_edge.file_node_id, file_tree);
                        }
                        file_tree.end_folder();
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
        let mut path = self.root_path.clone();
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
                
        fn longest_common_suffix(a: &[&str], b: &[&str]) -> usize {
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
            count
        }
        // Collect the path components for each open tab
        let mut tabs: Vec<(LiveId, Vec<&str>)> = Vec::new();
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
            
            tabs.push((tab_id, path_components));
        }
        
        // Sort the tabs by their path components
        tabs.sort_by(|a, b| a.1.cmp(&b.1));
        
        // Determine the minimal unique suffix for each tab
        for i in 0..tabs.len() {
            let (tab_id, ref path) = tabs[i];
            let mut min_suffix_len = 1;
            
            // Compare with previous tab
            if i > 0 {
                let (_, ref prev_path) = tabs[i - 1];
                let common = longest_common_suffix(path, prev_path);
                min_suffix_len = min_suffix_len.max(common + 1);
            }
            // Compare with next tab
            if i + 1 < tabs.len() {
                let (_, ref next_path) = tabs[i + 1];
                let common = longest_common_suffix(path, next_path);
                min_suffix_len = min_suffix_len.max(common + 1);
            }
            
            // Build the tab title using the minimal unique suffix
            let start = path.len().saturating_sub(min_suffix_len);
            let title = path[start..].join("/");
            dock.set_tab_title(cx, tab_id, title);
        }
    }
    
    pub fn load_file_tree(&mut self, tree_data: FileTreeData) {
        fn create_file_node(
            file_node_id: Option<LiveId>,
            node_path: String,
            path_to_file_id: &mut HashMap<String, LiveId>,
            file_nodes: &mut LiveIdMap<LiveId, FileNode>,
            parent_edge: Option<FileEdge>,
            node: FileNodeData,
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
                    FileNodeData::Directory {entries} => Some(
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
                            ),
                        })
                            .collect::<Vec<_ >> (),
                    ),
                    FileNodeData::File {..} => None,
                },
            };
            path_to_file_id.insert(node_path, file_node_id);
            file_nodes.insert(file_node_id, node);
            file_node_id
        }
        
        self.root_path = tree_data.root_path;
        
        
        self.file_nodes.clear();
        
        create_file_node(
            Some(live_id!(root).into()),
            "".to_string(),
            &mut self.path_to_file_node_id,
            &mut self.file_nodes,
            None,
            tree_data.root,
        );
    }
}