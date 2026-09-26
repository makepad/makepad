//! The tree component's Files page: the working directory read into a
//! file tree, ported from the widget zoo. The component's overview, the
//! general tree, is `tree.rs`.
use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::{
    cmp::Ordering,
    fs,
    path::{Path, PathBuf},
};

use crate::makepad_widgets::makepad_micro_serde::*;

use crate::makepad_widgets::file_tree::*;
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.storybook.StoryFileTreeBase = #(StoryFileTree::register_widget(vm))

    mod.storybook.StoryFileTree = set_type_default() do mod.storybook.StoryFileTreeBase{
        file_tree: FileTree{}
    }

    mod.stories.TreeFiles = StoryPage{
        StoryNote{text: "The working directory read into a tree. The status dots are cycled for the demonstration rather than read from the repository, so all five kinds are on screen: none, new, modified, deleted and mixed."}
        files := mod.storybook.StoryFileTree{file_tree +: {width: Fill height: Fill}}
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

/// The zoo's file tree demo: the working directory read once and drawn as
/// a tree, skipping `target` and dot-prefixed entries.
#[derive(Script, ScriptHook, Widget)]
pub struct StoryFileTree {
    #[uid]
    uid: WidgetUid,
    #[redraw]
    #[find]
    #[live]
    pub file_tree: FileTree,
    #[rust]
    pub file_nodes: LiveIdMap<LiveId, FileNode>,
    #[rust]
    pub root_path: String,
    #[rust]
    pub path_to_file_node_id: HashMap<String, LiveId>,
    /// The story is built when it is opened, long after startup, so the
    /// directory is read on the first draw rather than on `Event::Startup`.
    #[rust]
    loaded: bool,
}

impl StoryFileTree {
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
                    // The five status kinds, cycled by node id so every one
                    // of them is on screen. They are a DEMONSTRATION, not a
                    // reading of the working tree: four of the five had never
                    // been drawn anywhere, which is how one of them came to
                    // be drawn fully transparent without anyone noticing.
                    let status = match file_node_id.0 % 5 {
                        0 => StatusDotKind::None,
                        1 => StatusDotKind::New,
                        2 => StatusDotKind::Modified,
                        3 => StatusDotKind::Deleted,
                        _ => StatusDotKind::Mixed,
                    };
                    file_tree.file_with_status(cx, file_node_id, &file_node.name, status);
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

    /// Read the working directory into the tree, or a fixed sample where
    /// there is no file system.
    fn load_working_directory(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
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
            let root = match get_directory_entries(&root_path, false) {
                Ok(entries) => FileNodeData::Directory { entries },
                Err(err) => {
                    let message = match err {
                        FileError::Unknown(s) | FileError::CannotOpen(s) => s,
                    };
                    log!(
                        "StoryFileTree: cannot read `{}`: {}",
                        root_path.display(),
                        message
                    );
                    FileNodeData::Directory {
                        entries: vec![DirectoryEntry {
                            name: format!("<cannot access filesystem: {}>", message),
                            node: FileNodeData::File { data: None },
                        }],
                    }
                }
            };
            let file_tree_data = FileTreeData {
                root_path: "".into(),
                root,
            };
            self.load_file_tree(file_tree_data);
        }
    }
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub enum FileError {
    Unknown(String),
    CannotOpen(String),
}

impl Widget for StoryFileTree {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.loaded {
            self.loaded = true;
            self.load_working_directory();
        }
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
        self.file_tree.handle_event(cx, event, scope);
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "collections/tree/files",
    category: "Collections",
    component: "Tree",
    also: &["FileTree", "FileTreeNode"],
    name: "Files",
    dsl: "TreeFiles",
    added: "2026-02-16",
    tags: &["ported"],
    doc: "# FileTree

The file browser: folders that open and shut, files with a status dot, and the hover, selection and drag-start the navigator on the left is made of.

**The tree is drawn, not declared.** A host walks its own model every draw pass, calling `begin_folder` and `end_folder` for a directory and `file` or `file_with_status` for a leaf. The widget keeps only what has to outlive a pass, keyed by the node id the host handed it: which folders are open, what is selected, and the scroll. What it reports back is a `FileTreeAction`: a file or folder clicked, a row hovered or left, and a file that should start a drag.

**What the demo does.** It reads the working directory on the first draw rather than at startup, because a story is built long after startup; skips `target` and dot-prefixed entries; sorts directories before files and each alphabetically; and shows one error row if the directory cannot be read. Where there is no file system it draws a fixed three-folder sample instead.

**The status dots are cycled by node id** so that all five kinds are on screen: none, new, modified, deleted and mixed. They are a demonstration, not a reading of the repository.

The Overview page beside this one is the general tree, TreeView: an outline given as text, with fold marks, indent guides, checkboxes and a keyboard walk. Use that for a hierarchy of anything; use this one for files.",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: None,
}];

#[cfg(test)]
mod tests {
    use super::*;

    /// The page is built from the DSL, which the Rust compiler never reads,
    /// and the widget on it is this file's own, registered here. The page
    /// moved under a new component and its template was renamed with it, so
    /// the record's template name is asked for, and the widget is checked
    /// to be the demo and not yet to have read anything: the directory is
    /// read on the first draw, and building a page draws nothing.
    #[test]
    fn the_page_builds_and_holds_the_demo_unread() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::theme::widgets_script_mod(vm);
            crate::shell::script_mod(vm);
            self::script_mod(vm);
            let _ = makepad_platform::shader_error::take();
        });
        for story in STORIES {
            let page = cx.with_vm(|vm| {
                let stories = vm.module(id!(stories));
                let value = vm.bx.heap.value(stories, LiveId::from_str(story.dsl).into(), NoTrap);
                assert!(value.as_object().is_some(), "no template {}", story.dsl);
                WidgetRef::script_from_value(vm, value)
            });
            assert!(!page.is_empty(), "{} built no widget", story.key);
            assert_eq!(
                makepad_platform::shader_error::take(),
                None,
                "{}: a draw shader failed to compile",
                story.key
            );
            let files = page.widget(&cx, &[live_id!(files)]);
            let demo = files.borrow::<StoryFileTree>();
            let demo = demo.as_deref().expect("files is the story's file tree demo");
            assert!(!demo.loaded, "{}: the directory was read while building", story.key);
            assert!(demo.file_nodes.is_empty(), "{}: nodes exist before the first draw", story.key);
        }
    }
}
