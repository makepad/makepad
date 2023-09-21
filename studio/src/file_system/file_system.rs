use {
    std::collections::{HashMap, hash_map},
    crate::{
        makepad_code_editor::{Document, decoration::{Decoration, DecorationSet}, Session},
        makepad_platform::*,
        makepad_platform::makepad_live_compiler::LiveFileChange,
        makepad_draw::*,
        makepad_widgets::*,
        makepad_widgets::file_tree::*,
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
    pub path_to_file_node_id: HashMap<String, FileNodeId>,
    pub tab_id_to_file_node_id: HashMap<LiveId, FileNodeId>,
    pub tab_id_to_session: HashMap<LiveId, Session>,
    pub open_documents: HashMap<FileNodeId, OpenDoc>
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
    pub file_node_id: FileNodeId,
}

pub enum FileSystemAction {
    RecompileNeeded,
    LiveReloadNeeded(LiveFileChange)
}

impl FileSystem {
    pub fn init(&mut self, cx: &mut Cx) {
        self.file_client.init(cx);
        self.file_client.send_request(FileRequest::LoadFileTree {with_data: false});
    }
    
    pub fn remove_tab(&mut self, tab_id:LiveId){
        self.tab_id_to_file_node_id.remove(&tab_id);
        self.tab_id_to_session.remove(&tab_id);
    }
    
    pub fn path_to_file_node_id(&self, path: &str) -> Option<FileNodeId> {
        self.path_to_file_node_id.get(path).cloned()
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
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, ui: &WidgetRef) -> Vec<FileSystemAction> {
        let mut actions = Vec::new();
        self.handle_event_with(cx, event, ui, &mut | _, action | actions.push(action));
        actions
    }
    
    pub fn handle_event_with(&mut self, cx: &mut Cx, event: &Event, ui: &WidgetRef, dispatch_action: &mut dyn FnMut(&mut Cx, FileSystemAction)) {
        for action in self.file_client.handle_event(cx, event) {
            match action {
                FileClientAction::Response(response) => match response {
                    FileResponse::LoadFileTree(response) => {
                        self.load_file_tree(response.unwrap());
                        ui.file_tree(id!(file_tree)).redraw(cx);
                        // dock.select_tab(cx, dock, state, live_id!(file_tree).into(), live_id!(file_tree).into(), Animate::No);
                    }
                    FileResponse::OpenFile(result) => match result {
                        Ok((_unix_path, data, id)) => {
                            let file_id = FileNodeId(LiveId(id));
                            let dock = ui.dock(id!(dock));
                            for (tab_id, file_id) in &self.tab_id_to_file_node_id {
                                if id == file_id.0.0 {
                                    dock.redraw_tab(cx, *tab_id);
                                }
                            }
                            if let Some(OpenDoc::Decorations(dec)) = self.open_documents.get(&file_id) {
                                let dec = dec.clone();
                                self.open_documents.insert(file_id, OpenDoc::Document(Document::new(data.into(), dec)));
                            }else {panic!()}
                            
                            ui.redraw(cx);
                        }
                        Err(FileError::CannotOpen(_unix_path)) => {
                        }
                        Err(FileError::Unknown(err)) => {
                            log!("File error unknown {}", err);
                            // ignore
                        }
                    }
                    FileResponse::SaveFile(result) => match result {
                        Ok((path, old, new, _id)) => {
                            // alright file has been saved
                            // now we need to check if a live_design!{} changed or something outside it
                            if old != new {
                                let mut old_neg = Vec::new();
                                let mut new_neg = Vec::new();
                                match LiveRegistry::tokenize_from_str_live_design(&old, Default::default(), Default::default(), Some(&mut old_neg)) {
                                    Err(e) => {
                                        log!("Cannot tokenize old file {}", e)
                                    }
                                    Ok(old_tokens) => match LiveRegistry::tokenize_from_str_live_design(&new, Default::default(), Default::default(), Some(&mut new_neg)) {
                                        Err(e) => {
                                            log!("Cannot tokenize new file {}", e);
                                        }
                                        Ok(new_tokens) => {
                                            // we need the space 'outside' of these tokens
                                            if old_neg != new_neg {
                                                dispatch_action(cx, FileSystemAction::RecompileNeeded)
                                            }
                                            if old_tokens != new_tokens {
                                                // design code changed, hotreload it
                                                dispatch_action(cx, FileSystemAction::LiveReloadNeeded(LiveFileChange {
                                                    file_name: path,
                                                    content: new
                                                }))
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Err(_) => {}
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
    
    pub fn request_open_file(&mut self, tab_id: LiveId, file_id: FileNodeId) {
        // ok lets see if we have a document
        // ifnot, we create a new one
        self.tab_id_to_file_node_id.insert(tab_id, file_id);
        
        if self.open_documents.get(&file_id).is_none() {
            self.open_documents.insert(file_id, OpenDoc::Decorations(DecorationSet::new()));
            let path = self.file_node_path(file_id);
            self.file_client.send_request(FileRequest::OpenFile(path, file_id.0.0));
        }
    }
    
    
    pub fn request_save_file(&mut self, tab_id: LiveId) {
        // ok lets see if we have a document
        // ifnot, we create a new one
        if let Some(file_id) = self.tab_id_to_file_node_id.get(&tab_id) {
            if let Some(OpenDoc::Document(doc)) = self.open_documents.get(&file_id) {
                let text = doc.as_text().to_string();
                let path = self.file_node_path(*file_id);
                self.file_client.send_request(FileRequest::SaveFile(path.clone(), text, file_id.0.0));
            }
        };
    }
    
    pub fn clear_decorations(&mut self, file_node_id: &FileNodeId) {
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
    
    pub fn redraw_view_by_file_id(&mut self, cx: &mut Cx, id: FileNodeId, dock: &DockRef) {
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
    
    pub fn add_decoration(&mut self, file_id: FileNodeId, dec: Decoration) {
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
            if file_node.parent_edge.is_some() {
                path.insert_str(0, "/");
            }
        }
        path
    }
    
    pub fn ensure_unique_tab_names(&self, cx: &mut Cx, dock: &DockRef) {
        // alright so. what do we do.
        // we first connect colliding names
        // and then add more path elements till they dont
        for (outer_tab_id, outer_id) in &self.tab_id_to_file_node_id {
            for (inner_tab_id, inner_id) in &self.tab_id_to_file_node_id {
                if outer_tab_id != inner_tab_id && outer_id == inner_id {
                    // here we have multiple views
                }
            }
        }
        
        let mut min_diff:HashMap<FileNodeId,usize> = HashMap::new();
        let mut outer_path = Vec::new();
        let mut inner_path = Vec::new();
        for (_outer_tab_id, outer_file_id) in &self.tab_id_to_file_node_id {
            let mut outer = &self.file_nodes[*outer_file_id];
            outer_path.clear();
            while let Some(edge) = &outer.parent_edge {
                outer_path.push(&edge.name);
                outer = &self.file_nodes[edge.file_node_id];
            }
            if min_diff.get(&outer_file_id).is_none(){
                min_diff.insert(*outer_file_id,0);
            }
            for (_inner_tab_id, inner_file_id) in &self.tab_id_to_file_node_id {
                let mut inner = &self.file_nodes[*inner_file_id];
                inner_path.clear();
                while let Some(edge) = &inner.parent_edge {
                    inner_path.push(&edge.name);
                    inner = &self.file_nodes[edge.file_node_id];
                }
                for i in 0..inner_path.len().min(outer_path.len()){
                    if inner_path[i] != outer_path[i]{
                        // store the min depth at which these ones are different
                        if let Some(min) = min_diff.get_mut(&inner_file_id){
                            *min = (*min).max(i);
                        }
                        else{
                            min_diff.insert(*inner_file_id, i);
                        }
                        if let Some(min) = min_diff.get_mut(&outer_file_id){
                            *min = (*min).max(i);
                        }
                        else{
                            min_diff.insert(*outer_file_id, i);
                        }
                    }
                }
            }
        }
        // now loop over the tabs
        for (tab_id, file_id) in &self.tab_id_to_file_node_id {
            if let Some(min) = min_diff.get(&file_id){
                let mut inner = &self.file_nodes[*file_id];
                inner_path.clear();
                while let Some(edge) = &inner.parent_edge {
                    inner_path.push(&edge.name);
                    inner = &self.file_nodes[edge.file_node_id];
                }
                let mut name = String::new();
                for i in (0..*min+1).rev(){
                    if name.len()>0{
                        name.push_str("/");
                    }
                    name.push_str(inner_path[i]);
                }
                dock.set_tab_title(cx, *tab_id, name);
            }
        }
    }
    
    
    pub fn load_file_tree(&mut self, tree_data: FileTreeData) {
        fn create_file_node(
            file_node_id: Option<FileNodeId>,
            node_path: String,
            path_to_file_id: &mut HashMap<String, FileNodeId>,
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
