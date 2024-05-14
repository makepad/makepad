
use std::{cmp::Ordering, collections::HashMap, fs, path::{Path, PathBuf}};

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
#[derive(Default, Clone, Debug, SerBin, DeBin)]
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
#[derive(Default, Clone, Debug, SerBin, DeBin)]
pub enum FileNodeData {
   
    Directory { entries: Vec<DirectoryEntry> },
    File { data: Option<Vec<u8>> },
    #[default] Nothing

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
    pub file_node_id: LiveId,
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
    #[rust] pub file_nodes: LiveIdMap<LiveId, FileNode>,
    #[rust] pub root_path: String,
    #[rust] pub path_to_file_node_id:  HashMap<String, LiveId>
}

impl DemoFileTree{
    pub fn draw_file_node(cx: &mut Cx2d, file_node_id: LiveId, file_tree:&mut FileTree, file_nodes: &LiveIdMap<LiveId, FileNode>) {
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
                    _ => None
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

#[derive(Clone, Debug, SerBin, DeBin)]
pub enum FileError {
    Unknown(String),
    CannotOpen(String)
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
        match event{

            Event::Startup=>
            {
                fn get_directory_entries(path: &Path, with_data: bool) -> Result<Vec<DirectoryEntry>, FileError> {
                    let mut entries = Vec::new();
                    for entry in fs::read_dir(path).map_err( | error | FileError::Unknown(error.to_string())) ? {
                        // We can't get the entry for some unknown reason. Raise an error.
                        let entry = entry.map_err( | error | FileError::Unknown(error.to_string())) ?;
                        // Get the path for the entry.
                        let entry_path = entry.path();
                        // Get the file name for the entry.
                        let name = entry.file_name();
                        if let Ok(name_string) = name.into_string() {
                            if entry_path.is_dir() && name_string == "target"
                                || name_string.starts_with('.') {
                                // Skip over directories called "target". This is sort of a hack. The reason
                                // it's here is that the "target" directory for Rust projects is huge, and
                                // our current implementation of the file tree widget is not yet fast enough
                                // to display vast numbers of nodes. We paper over this by pretending the
                                // "target" directory does not exist.
                                continue;
                            }
                        }
                        else {
                            // Skip over entries with a non UTF-8 file name.
                            continue;
                        }
                        // Create a `DirectoryEntry` for this entry and add it to the list of entries.
                        entries.push(DirectoryEntry {
                            name: entry.file_name().to_string_lossy().to_string(),
                            node: if entry_path.is_dir() {
                                // If this entry is a subdirectory, recursively create `DirectoryEntry`'s
                                // for its entries as well.
                                FileNodeData::Directory {
                                    entries: get_directory_entries(&entry_path, with_data) ?,
                                }
                            } else if entry_path.is_file() {
                                if with_data {
                                    let bytes: Vec<u8> = fs::read(&entry_path).map_err(
                                        | error | FileError::Unknown(error.to_string())
                                    ) ?;
                                    FileNodeData::File {data: Some(bytes)}
                                }
                                else {
                                    FileNodeData::File {data: None}
                                }
                            }
                            else {
                                // If this entry is neither a directory or a file, skip it. This ignores
                                // things such as symlinks, for which we are not yet sure how we want to
                                // handle them.
                                continue
                            },
                        });
                    }
                    
                    // Sort all the entries by name, directories first, and files second.
                    entries.sort_by( | entry_0, entry_1 | {
                        match &entry_0.node {
                            FileNodeData::Directory {..} => match &entry_1.node {
                                FileNodeData::Directory {..} => entry_0.name.cmp(&entry_1.name),
                                FileNodeData::File {..} => Ordering::Less,
                                _ => Ordering::Less
                            }
                            FileNodeData::File {..} => match &entry_1.node {
                                FileNodeData::Directory {..} => Ordering::Greater,
                                FileNodeData::File {..} => entry_0.name.cmp(&entry_1.name),
                                _ => Ordering::Less
                            },
                            _ => Ordering::Less
                            
                        }
                    });
                    Ok(entries)
                }
    
            let root_path: PathBuf  = PathBuf::from(".");
                let root = FileNodeData::Directory {
                    entries: get_directory_entries(&root_path, false).unwrap(),
                };
                let file_tree_data=  FileTreeData { root_path: "".into(), root:root  };

                 self.load_file_tree(file_tree_data);
                }
            _ => {}
        }

        self.file_tree.handle_event(cx, event, scope);
    }
}