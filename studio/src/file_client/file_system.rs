use {
    crate::{
        makepad_platform::*,
        makepad_draw::*,
        makepad_widgets::*,
        makepad_widgets::file_tree::*,
        file_client::FileClient,
        makepad_file_protocol::{
            FileRequest,
            FileResponse,
            FileClientAction,
            FileNodeData,
            FileTreeData,
            unix_str::UnixString,
            unix_path::UnixPathBuf,
        },
    },
};

#[derive(Default)]
pub struct FileSystem {
    pub file_client: FileClient,
    pub path: UnixPathBuf,
    pub file_nodes: LiveIdMap<FileNodeId, FileNode>,
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
    pub name: UnixString,
    pub file_node_id: FileNodeId,
}

impl FileSystem {
    pub fn init(&mut self, cx:&mut Cx){
        self.file_client.init(cx);
        self.file_client.send_request(FileRequest::LoadFileTree {with_data: false});
    }
    
    pub fn handle_event(&mut self, cx:&mut Cx, event:&Event, ui:&WidgetRef){
        for action in self.file_client.handle_event(cx, event) {
            match action {
                FileClientAction::Response(response) => match response {
                    FileResponse::LoadFileTree(response) => {
                        self.load_file_tree(response.unwrap());
                        ui.get_file_tree(id!(file_tree)).redraw(cx);
                        // dock.select_tab(cx, dock, state, live_id!(file_tree).into(), live_id!(file_tree).into(), Animate::No);
                    }
                    _response => {
                        //self.build_manager.handle_file_response(cx, &response);
                        // self.editors.handle_collab_response(cx, &mut state.editor_state, response, &mut self.collab_client.request_sender())
                    }
                },
                FileClientAction::Notification(_notification) => {
                    //self.editors.handle_collab_notification(cx, &mut state.editor_state, notification)
                }
            }
        }
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
    
    pub fn _file_node_path(&self, file_node_id: FileNodeId) -> UnixPathBuf {
        let mut components = Vec::new();
        let mut file_node = &self.file_nodes[file_node_id];
        while let Some(edge) = &file_node.parent_edge {
            components.push(&edge.name);
            file_node = &self.file_nodes[edge.file_node_id];
        }
        self.path.join(components.into_iter().rev().collect::<UnixPathBuf>())
    }
    
    pub fn _file_path_join(&self, components: &[&str]) -> UnixPathBuf {
        self.path.join(components.into_iter().rev().collect::<UnixPathBuf>())
    }
    
    pub fn load_file_tree(&mut self, tree_data: FileTreeData) {
        fn create_file_node(
            file_node_id: Option<FileNodeId>,
            file_nodes: &mut LiveIdMap<FileNodeId, FileNode>,
            parent_edge: Option<FileEdge>,
            node: FileNodeData,
        ) -> FileNodeId {
            let file_node_id = file_node_id.unwrap_or(file_nodes.alloc_key());
            let name = parent_edge.as_ref().map_or_else(
                || String::from("root"),
                | edge | edge.name.to_string_lossy().to_string(),
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
        
        self.path = tree_data.path;
        
        self.file_nodes.clear();
        
        create_file_node(
            Some(live_id!(root).into()),
            &mut self.file_nodes,
            None,
            tree_data.root,
        );
    }
}
