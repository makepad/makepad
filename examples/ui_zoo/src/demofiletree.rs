
use std::collections::HashMap;

use makepad_widgets::makepad_micro_serde::*;

use crate::{
        makepad_widgets::*,
        makepad_widgets::file_tree::*,
    };

live_design!{
    import makepad_widgets::theme_desktop_dark::*;
        
    DemoFileTree = {{DemoFileTree}}{
        file_tree: <FileTree>{}
    }
} 

/// A type for representing data about a file tree.
#[derive(Clone, Debug, SerBin, DeBin)]
pub struct FileTreeData {
    /// The path to the root of this file tree.
    pub root_path: String,
    /// Data about the root of this file tree.
    pub root: FileNodeData,
}

/// A type for representing data about a node in a file tree.
/// 
/// Each node is either a directory a file. Directories form the internal nodes of the file tree.
/// They consist of one or more named entries, each of which is another node. Files form the leaves
/// of the file tree, and do not contain any further nodes.
#[derive(Clone, Debug, SerBin, DeBin)]
pub enum FileNodeData {
    Directory { entries: Vec<DirectoryEntry> },
    File { data: Option<Vec<u8>> },
}

/// A type for representing an entry in a directory.
#[derive(Clone, Debug, SerBin, DeBin)]
pub struct DirectoryEntry {
    /// The name of this entry.
    pub name: String,
    /// The node for this entry.
    pub node: FileNodeData,
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
    #[rust] pub file_nodes: LiveIdMap<FileNodeId, FileNode>,
    #[rust] pub root_path: String,
    #[rust] pub path_to_file_node_id:  HashMap<String, FileNodeId>
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


    pub fn load_file_tree(&mut self, tree_data: FileTreeData) {
        fn create_file_node(
            file_node_id: Option<FileNodeId>,
            node_path: String,
            path_to_file_id: &mut HashMap<String, FileNodeId>,
            file_nodes: &mut LiveIdMap<FileNodeId, FileNode>,
            parent_edge: Option<FileEdge>,
            node: FileNodeData,
        ) -> FileNodeId {
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