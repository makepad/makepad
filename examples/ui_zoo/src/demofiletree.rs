
use {
    crate::{
        makepad_widgets::*,
        makepad_widgets::file_tree::*,
    },
};

live_design!{
    import makepad_widgets::theme_desktop_dark::*;
        
    DemoFileTree = {{DemoFileTree}}{
        file_tree: <FileTree>{}
    }
} 
#[derive(Debug)]
pub struct FileEdge {
    pub name: String,
    pub file_node_id: FileNodeId,
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

#[derive(Live, LiveHook, Widget)] 
pub struct DemoFileTree{
    #[wrap] #[live] pub file_tree: FileTree,
    #[rust] pub file_nodes: LiveIdMap<FileNodeId, FileNode>
}

impl DemoFileTree{
    pub fn draw_file_node(cx: &mut Cx2d, file_node_id: FileNodeId, file_tree:&mut FileTree, file_nodes: &LiveIdMap<FileNodeId, FileNode>) {
        if let Some(file_node) = file_nodes.get(&file_node_id) {
            match &file_node.child_edges {
                Some(child_edges) => {
                    if file_tree.begin_folder(cx, file_node_id, &file_node.name).is_ok() {
                        for child_edge in child_edges {
                            Self::draw_file_node(cx, child_edge.file_node_id, file_tree, file_nodes);
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
}

impl Widget for DemoFileTree {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut Scope, walk:Walk)->DrawStep{
        while self.file_tree.draw_walk(cx, scope, walk).is_step() {
            self.file_tree.set_folder_is_open(cx, live_id!(root).into(), true, Animate::No);
             Self::draw_file_node(
                cx,
                live_id!(root).into(),
                &mut self.file_tree,
                &self.file_nodes
            );
        }
        DrawStep::done()
    }
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope){
        self.file_tree.handle_event(cx, event, scope);
    }
}