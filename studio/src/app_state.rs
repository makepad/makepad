use {
    crate::{
        editor_state::{EditorState, SessionId},
        code_editor::{
            protocol::{FileNodeData, FileTreeData},
        },
        editors::{EditorViewId},
    },
    makepad_component::{
        dock::{PanelTag, PanelId},
        file_tree::{FileNodeTag, FileNodeId},
        splitter::{SplitterAlign},
        tab_bar::{TabTag, TabId},
    },
    makepad_component::makepad_render::*,
    std::{
        ffi::OsString,
        path::{ PathBuf},
    },
};

pub struct AppState {
    pub panel_id_allocator: GenIdAllocator<PanelTag>,
    pub panels_by_panel_id: GenIdMap<PanelTag, Panel>,
    pub root_panel_id: PanelId,
    pub side_bar_panel_id: PanelId,
    pub selected_panel_id: PanelId,
    pub content_panel_id: PanelId,
    pub tab_id_allocator: GenIdAllocator<TabTag>,
    pub tabs_by_tab_id: GenIdMap<TabTag, Tab>,
    pub file_tree_tab_id: TabId,
    pub file_node_id_allocator: GenIdAllocator<FileNodeTag>,
    pub file_nodes_by_file_node_id: GenIdMap<FileNodeTag, FileNode>,
    pub path: PathBuf,
    pub root_file_node_id: FileNodeId,
    pub editor_state: EditorState,
}

impl AppState {
    pub fn new() -> AppState {
        let mut file_node_id_allocator = GenIdAllocator::new();
        let mut file_nodes_by_file_node_id = GenIdMap::new();
        let root_file_node_id = file_node_id_allocator.allocate();
        file_nodes_by_file_node_id.insert(
            root_file_node_id,
            FileNode {
                parent_edge: None,
                name: String::from("root"),
                child_edges: Some(Vec::new()),
            },
        );
        
        let mut panel_id_allocator = GenIdAllocator::new();
        let mut panels_by_panel_id = GenIdMap::new();
        let mut tab_id_allocator = GenIdAllocator::new();
        let mut tabs_by_tab_id = GenIdMap::new();
        
        let root_panel_id = panel_id_allocator.allocate();
        let side_bar_panel_id = panel_id_allocator.allocate();
        let file_tree_tab_id = tab_id_allocator.allocate();
        
        panels_by_panel_id.insert(
            side_bar_panel_id,
            Panel {
                parent_panel_id: Some(root_panel_id),
                kind: PanelKind::Tab(TabPanel {
                    tab_ids: vec![file_tree_tab_id],
                    editor_view_id: None,
                }),
            },
        );
        
        tabs_by_tab_id.insert(
            file_tree_tab_id,
            Tab {
                name: String::from("File Tree"),
                kind: TabKind::FileTree,
            },
        );
        
        let content_panel_id = panel_id_allocator.allocate();
        panels_by_panel_id.insert(
            content_panel_id,
            Panel {
                parent_panel_id: Some(root_panel_id),
                kind: PanelKind::Tab(TabPanel {
                    tab_ids: vec![],
                    editor_view_id: None,
                }),
            },
        );
        
        panels_by_panel_id.insert(
            root_panel_id,
            Panel {
                parent_panel_id: None,
                kind: PanelKind::Split(SplitPanel {
                    axis: Axis::Horizontal,
                    align: SplitterAlign::FromStart(200.0),
                    child_panel_ids: [side_bar_panel_id, content_panel_id],
                }),
            },
        );
        
        AppState {
            content_panel_id,
            panel_id_allocator,
            panels_by_panel_id,
            root_panel_id,
            side_bar_panel_id,
            selected_panel_id: content_panel_id,
            tab_id_allocator,
            tabs_by_tab_id,
            file_tree_tab_id,
            file_node_id_allocator,
            file_nodes_by_file_node_id,
            path: PathBuf::new(),
            root_file_node_id,
            editor_state: EditorState::new(),
        }
    }
    
    pub fn file_node_path(&self, file_node_id: FileNodeId) -> PathBuf {
        let mut components = Vec::new();
        let mut file_node = &self.file_nodes_by_file_node_id[file_node_id];
        while let Some(edge) = &file_node.parent_edge {
            components.push(&edge.name);
            file_node = &self.file_nodes_by_file_node_id[edge.file_node_id];
        }
        self.path.join(components.into_iter().rev().collect::<PathBuf>())
    }
    
    pub fn file_path_join(&self, components: &[&str]) -> PathBuf {
        self.path.join(components.into_iter().rev().collect::<PathBuf>())
    }
    
    pub fn load_file_tree(&mut self, tree_data: FileTreeData) {
        fn create_file_node(
            file_node_id_allocator: &mut GenIdAllocator<FileNodeTag>,
            file_nodes_by_file_node_id: &mut GenIdMap<FileNodeTag, FileNode>,
            parent_edge: Option<FileEdge>,
            node: FileNodeData,
        ) -> FileNodeId {
            let file_node_id = file_node_id_allocator.allocate();
            let name = parent_edge.as_ref().map_or_else(
                || String::from("root"),
                | edge | edge.name.to_string_lossy().into_owned(),
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
                                file_node_id_allocator,
                                file_nodes_by_file_node_id,
                                Some(FileEdge {
                                    name: entry.name,
                                    file_node_id,
                                }),
                                entry.node,
                            ),
                        })
                            .collect::<Vec<_ >> (),
                    ),
                    FileNodeData::File => None,
                },
            };
            file_nodes_by_file_node_id.insert(file_node_id, node);
            file_node_id
        }
        
        self.path = tree_data.path;
        self.file_node_id_allocator.clear();
        self.file_nodes_by_file_node_id.clear();
        self.root_file_node_id = create_file_node(
            &mut self.file_node_id_allocator,
            &mut self.file_nodes_by_file_node_id,
            None,
            tree_data.root,
        );
    }
}

#[derive(Debug)]
pub struct Panel {
    pub parent_panel_id: Option<PanelId>,
    pub kind: PanelKind,
}

impl Panel {
    pub fn as_split_panel_mut(&mut self) -> &mut SplitPanel {
        match &mut self.kind {
            PanelKind::Split(panel) => panel,
            _ => panic!(),
        }
    }
    
    pub fn as_tab_panel(&self) -> &TabPanel {
        match &self.kind {
            PanelKind::Tab(panel) => panel,
            _ => panic!(),
        }
    }
    
    pub fn as_tab_panel_mut(&mut self) -> &mut TabPanel {
        match &mut self.kind {
            PanelKind::Tab(panel) => panel,
            _ => panic!(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum PanelKind {
    Split(SplitPanel),
    Tab(TabPanel),
}

#[derive(Clone, Debug)]
pub struct SplitPanel {
    pub axis: Axis,
    pub align: SplitterAlign,
    pub child_panel_ids: [PanelId; 2],
}

#[derive(Clone, Debug)]
pub struct TabPanel {
    pub tab_ids: Vec<TabId>,
    pub editor_view_id: Option<EditorViewId>,
}

pub struct Tab {
    pub name: String,
    pub kind: TabKind,
}

pub enum TabKind {
    FileTree,
    CodeEditor {session_id: SessionId},
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
    pub name: OsString,
    pub file_node_id: FileNodeId,
}

