use std::{
    cmp::Ordering,
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use makepad_widgets::makepad_micro_serde::*;

use crate::{makepad_widgets::file_tree::*, makepad_widgets::*};

script_mod! {
    use mod.prelude.widgets.*

    mod.widgets.DemoFileTreeBase = #(DemoFileTree::register_widget(vm))

    mod.widgets.DemoFileTree = set_type_default() do mod.widgets.DemoFileTreeBase{
        file_tree: FileTree{}
    }
}

#[derive(Default, Clone, Debug, SerBin, DeBin)]
pub struct FileTreeData {
    pub root_path: String,
    pub root: FileNodeData,
}

#[derive(Default, Clone, Debug, SerBin, DeBin)]
pub enum FileNodeData {
    Directory {
        entries: Vec<DirectoryEntry>,
    },
    File {
        data: Option<Vec<u8>>,
    },
    #[default]
    Nothing,
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub struct DirectoryEntry {
    pub name: String,
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
    pub child_edges: Option<Vec<FileEdge>>,
}

impl FileNode {
    pub fn is_file(&self) -> bool {
        self.child_edges.is_none()
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct DemoFileTree {
    #[uid]
    uid: WidgetUid,
    #[redraw]
    #[live]
    pub file_tree: FileTree,
    #[rust]
    pub file_nodes: LiveIdMap<LiveId, FileNode>,
    #[rust]
    pub root_path: String,
    #[rust]
    pub path_to_file_node_id: HashMap<String, LiveId>,
}

impl DemoFileTree {
    pub fn draw_file_node(
        cx: &mut Cx2d,
        file_node_id: LiveId,
        file_tree: &mut FileTree,
        file_nodes: &LiveIdMap<LiveId, FileNode>,
    ) {
        if let Some(file_node) = file_nodes.get(&file_node_id) {
            match &file_node.child_edges {
                Some(child_edges) => {
                    if file_tree
                        .begin_folder(cx, file_node_id, &file_node.name)
                        .is_ok()
                    {
                        for child_edge in child_edges {
                            Self::draw_file_node(
                                cx,
                                child_edge.file_node_id,
                                file_tree,
                                file_nodes,
                            );
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
            let name = parent_edge
                .as_ref()
                .map_or_else(|| String::from("root"), |edge| edge.name.clone());
            let node = FileNode {
                parent_edge,
                name,
                child_edges: match node {
                    FileNodeData::Directory { entries } => Some(
                        entries
                            .into_iter()
                            .map(|entry| FileEdge {
                                name: entry.name.clone(),
                                file_node_id: create_file_node(
                                    None,
                                    if node_path.len() > 0 {
                                        format!("{}/{}", node_path, entry.name.clone())
                                    } else {
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
                            .collect::<Vec<_>>(),
                    ),
                    FileNodeData::File { .. } => None,
                    _ => None,
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
    CannotOpen(String),
}

impl Widget for DemoFileTree {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while self.file_tree.draw_walk(cx, scope, walk).is_step() {
            self.file_tree
                .set_folder_is_open(cx, live_id!(root).into(), true, Animate::No);
            Self::draw_file_node(
                cx,
                live_id!(root).into(),
                &mut self.file_tree,
                &self.file_nodes,
            );
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        match event {
            Event::Startup => {
                fn get_directory_entries(
                    path: &Path,
                    with_data: bool,
                ) -> Result<Vec<DirectoryEntry>, FileError> {
                    let mut entries = Vec::new();
                    for entry in
                        fs::read_dir(path).map_err(|error| FileError::Unknown(error.to_string()))?
                    {
                        let entry = entry.map_err(|error| FileError::Unknown(error.to_string()))?;
                        let entry_path = entry.path();
                        let name = entry.file_name();
                        if let Ok(name_string) = name.into_string() {
                            if entry_path.is_dir() && name_string == "target"
                                || name_string.starts_with('.')
                            {
                                continue;
                            }
                        } else {
                            continue;
                        }
                        entries.push(DirectoryEntry {
                            name: entry.file_name().to_string_lossy().to_string(),
                            node: if entry_path.is_dir() {
                                FileNodeData::Directory {
                                    entries: get_directory_entries(&entry_path, with_data)?,
                                }
                            } else if entry_path.is_file() {
                                if with_data {
                                    let bytes: Vec<u8> = fs::read(&entry_path)
                                        .map_err(|error| FileError::Unknown(error.to_string()))?;
                                    FileNodeData::File { data: Some(bytes) }
                                } else {
                                    FileNodeData::File { data: None }
                                }
                            } else {
                                continue;
                            },
                        });
                    }

                    entries.sort_by(|entry_0, entry_1| match &entry_0.node {
                        FileNodeData::Directory { .. } => match &entry_1.node {
                            FileNodeData::Directory { .. } => entry_0.name.cmp(&entry_1.name),
                            FileNodeData::File { .. } => Ordering::Less,
                            _ => Ordering::Less,
                        },
                        FileNodeData::File { .. } => match &entry_1.node {
                            FileNodeData::Directory { .. } => Ordering::Greater,
                            FileNodeData::File { .. } => entry_0.name.cmp(&entry_1.name),
                            _ => Ordering::Less,
                        },
                        _ => Ordering::Less,
                    });
                    Ok(entries)
                }

                #[cfg(target_arch = "wasm32")]
                {
                    let file_tree_data = FileTreeData {
                        root_path: "".into(),
                        root: FileNodeData::Directory {
                            entries: vec![
                                DirectoryEntry {
                                    name: "empty".to_string(),
                                    node: FileNodeData::Directory { entries: vec![] },
                                },
                                DirectoryEntry {
                                    name: "on".to_string(),
                                    node: FileNodeData::Directory {
                                        entries: vec![
                                            DirectoryEntry {
                                                name: "empty".to_string(),
                                                node: FileNodeData::Directory { entries: vec![] },
                                            },
                                            DirectoryEntry {
                                                name: "on".to_string(),
                                                node: FileNodeData::Directory { entries: vec![] },
                                            },
                                            DirectoryEntry {
                                                name: "web".to_string(),
                                                node: FileNodeData::Directory { entries: vec![] },
                                            },
                                        ],
                                    },
                                },
                                DirectoryEntry {
                                    name: "web".to_string(),
                                    node: FileNodeData::Directory { entries: vec![] },
                                },
                            ],
                        },
                    };
                    self.load_file_tree(file_tree_data);
                }
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let root_path: PathBuf = PathBuf::from(".");
                    let root = FileNodeData::Directory {
                        entries: get_directory_entries(&root_path, false).unwrap(),
                    };
                    let file_tree_data = FileTreeData {
                        root_path: "".into(),
                        root,
                    };
                    self.load_file_tree(file_tree_data);
                }
            }
            _ => {}
        }

        self.file_tree.handle_event(cx, event, scope);
    }
}
