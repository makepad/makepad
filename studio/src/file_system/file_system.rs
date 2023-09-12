use {
    std::collections::{HashMap,hash_map},
    crate::{
        makepad_code_editor::{Document, Session},
        makepad_platform::*,
        makepad_draw::*,
        makepad_widgets::*,
        makepad_widgets::file_tree::*,
        makepad_widgets::dock::*,
        file_system::FileClient,
        makepad_file_protocol::{
            FileRequest,
            FileError,
            FileResponse,
            FileClientAction,
            FileNodeData,
            FileTreeData,
        },
    },
};

#[derive(Default)]
pub struct FileSystem {
    pub file_client: FileClient,
    pub root_path: String,
    pub file_nodes: LiveIdMap<FileNodeId, FileNode>,
    pub tab_id_to_path: HashMap<LiveId, String>,
    pub tab_id_to_session: HashMap<LiveId, Session>,
    pub open_documents: HashMap<String, Option<Document>>
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
    pub file_node_id: FileNodeId,
}

pub enum FileSystemAction{
    RecompileNeeded,
    LiveReloadNeeded
}

impl FileSystem {
    pub fn init(&mut self, cx:&mut Cx){
        self.file_client.init(cx);
        self.file_client.send_request(FileRequest::LoadFileTree {with_data: false});
    }
    
    pub fn get_session_mut(&mut self, tab_id:LiveId)->Option<&mut Session>{
        // lets see if we have a document yet
        if let Some(path) = self.tab_id_to_path.get(&tab_id){
            if let Some(Some(document)) = self.open_documents.get(path){
                return Some(match self.tab_id_to_session.entry(tab_id){
                    hash_map::Entry::Occupied(o) => o.into_mut(),
                    hash_map::Entry::Vacant(v) => v.insert(Session::new(document.clone()))
                })
            }
        }
        None
    }

    pub fn handle_event(&mut self, cx:&mut Cx, event:&Event, ui:&WidgetRef)->Vec<FileSystemAction>{
        let mut actions = Vec::new();
        self.handle_event_with(cx, event, ui, &mut |_,action| actions.push(action));
        actions
    }
    
    pub fn handle_event_with(&mut self, cx: &mut Cx, event: &Event, ui:&WidgetRef, dispatch_action: &mut dyn FnMut(&mut Cx, FileSystemAction)) {
        for action in self.file_client.handle_event(cx, event) {
            match action {
                FileClientAction::Response(response) => match response {
                    FileResponse::LoadFileTree(response) => {
                        self.load_file_tree(response.unwrap());
                        ui.file_tree(id!(file_tree)).redraw(cx);
                        // dock.select_tab(cx, dock, state, live_id!(file_tree).into(), live_id!(file_tree).into(), Animate::No);
                    }
                    FileResponse::OpenFile(result)=>match result{
                        Ok((unix_path, data))=>{
                            let dock = ui.dock(id!(dock));
                            for (tab_id, path) in &self.tab_id_to_path{
                                if unix_path == *path{
                                    dock.redraw_tab(cx, *tab_id);
                                }
                            }
                            self.open_documents.insert(unix_path, Some(Document::new(data.into())));
                            ui.redraw(cx);
                        }
                        Err(FileError::CannotOpen(_unix_path))=>{
                        }
                        Err(FileError::Unknown(err))=>{
                            log!("File error unknown {}", err);
                            // ignore
                        }
                    }
                    FileResponse::SaveFile(result)=>match result{
                        Ok((_path, old, new))=>{
                            // alright file has been saved
                            // now we need to check if a live_design!{} changed or something outside it
                            if old != new{
                                let mut old_neg = Vec::new();
                                let mut new_neg = Vec::new();
                                match  LiveRegistry::tokenize_from_str_live_design(&old, Default::default(), Default::default(), Some(&mut old_neg)){
                                    Err(e)=>{
                                        log!("Cannot tokenize old file {}", e)
                                    }
                                    Ok(old_tokens)=> match LiveRegistry::tokenize_from_str_live_design(&new, Default::default(), Default::default(), Some(&mut new_neg)){
                                        Err(e)=>{
                                            log!("Cannot tokenize new file {}", e);
                                        }
                                        Ok(new_tokens)=>{
                                            // we need the space 'outside' of these tokens
                                            if old_neg != new_neg{
                                                dispatch_action(cx, FileSystemAction::RecompileNeeded)
                                            }
                                            if old_tokens != new_tokens{
                                                // design code changed, hotreload it
                                                dispatch_action(cx, FileSystemAction::LiveReloadNeeded)
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Err(_)=>{}
                        // ok we saved a file, we should check however what changed
                        // to see if we need a recompile
                        
                    }
                },
                FileClientAction::Notification(_notification) => {
                    //self.editors.handle_collab_notification(cx, &mut state.editor_state, notification)
                }
            }
        }
    }
    
    pub fn request_open_file(&mut self, tab_id:LiveId, path:String){
        // ok lets see if we have a document
        // ifnot, we create a new one
        self.tab_id_to_path.insert(tab_id, path.clone());
        if self.open_documents.get(&path).is_none(){
            self.open_documents.insert(path.clone(), None);
            self.file_client.send_request(FileRequest::OpenFile(path));
        }
    }
    
    
    pub fn request_save_file(&mut self, tab_id:LiveId){
        // ok lets see if we have a document
        // ifnot, we create a new one
        if let Some(path) = self.tab_id_to_path.get(&tab_id){
            if let Some(Some(doc)) = self.open_documents.get(path){
                let text = doc.text().to_string();
                self.file_client.send_request(FileRequest::SaveFile(path.clone(), text));
            }
        };
    }
    
    pub fn draw_file_node(&self, cx: &mut Cx2d, file_node_id: FileNodeId, file_tree: &mut FileTree) {
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
    
    pub fn file_node_name(&self, file_node_id: FileNodeId) -> String {
        self.file_nodes.get(&file_node_id).unwrap().name.clone()
    }
    
    pub fn file_node_path(&self, file_node_id: FileNodeId) -> String {
        let mut path = self.root_path.clone();
        let mut file_node = &self.file_nodes[file_node_id];
        while let Some(edge) = &file_node.parent_edge {
            path.insert_str(0, &edge.name);
            file_node = &self.file_nodes[edge.file_node_id];
            if file_node.parent_edge.is_some(){
                path.insert_str(0, "/");
            }
        }
        path
    }
    
    pub fn load_file_tree(&mut self, tree_data: FileTreeData) {
        fn create_file_node(
            file_node_id: Option<FileNodeId>,
            file_nodes: &mut LiveIdMap<FileNodeId, FileNode>,
            parent_edge: Option<FileEdge>,
            node: FileNodeData,
        ) -> FileNodeId {
            let file_node_id = file_node_id.unwrap_or(LiveId::unique().into());
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
            file_nodes.insert(file_node_id, node);
            file_node_id
        }
        
        self.root_path = tree_data.root_path;
        
        self.file_nodes.clear();
        
        create_file_node(
            Some(live_id!(root).into()),
            &mut self.file_nodes,
            None,
            tree_data.root,
        );
    }
}
