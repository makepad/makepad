use {
    crate::{
        makepad_studio_component::{
            dock::{PanelId},
            file_tree::{FileNodeId},
            splitter::{SplitterAlign},
            tab_bar::{TabId},
        },
        makepad_platform::*,
        editor_state::{EditorState, SessionId},
        collab::{
            collab_protocol::{FileNodeData, FileTreeData},
        },
    },
    std::{
        ffi::OsString,
        path::{ PathBuf},
    },
};

pub struct AppState {
    pub panels: LiveIdMap<PanelId, Panel>,
    pub tabs: LiveIdMap<TabId, Tab>,
    pub file_nodes: LiveIdMap<FileNodeId, FileNode>,
    
    pub selected_panel_id: PanelId,
    
    pub path: PathBuf,
    pub editor_state: EditorState,
}

impl AppState {
    pub fn new() -> AppState {
        let mut file_nodes = LiveIdMap::new();

        file_nodes.insert(
            id!(root),
            FileNode {
                parent_edge: None,
                name: String::from("root"),
                child_edges: Some(Vec::new()),
            },
        );
        
        let mut panels = LiveIdMap::new();
        let mut tabs = LiveIdMap::new();
        
        panels.insert(
            id!(log_view),
            Panel::Tab(TabPanel {
                tab_ids: vec![id!(log_view).into(), id!(shader_view).into()],
                selected_tab: Some(1)
            }),
        );
        
        tabs.insert(
            id!(log_view),
            Tab {
                name: String::from("Log"),
                kind: TabKind::LogView,
            },
        );
        tabs.insert(
            id!(shader_view),
            Tab {
                name: String::from("Shader"),
                kind: TabKind::ShaderView,
            },
        );
        
        panels.insert(
            id!(file_tree),
            Panel::Tab(TabPanel {
                tab_ids: vec![id!(file_tree).into()],
                selected_tab: Some(0)
            }),
        );
        
        tabs.insert(
            id!(file_tree),
            Tab {
                name: String::from("Files"),
                kind: TabKind::FileTree,
            },
        );
        
        panels.insert(
            id!(content),
            Panel::Tab(TabPanel {
                tab_ids: vec![],
                selected_tab: None
            }),
        );
        
        panels.insert(
            id!(root),
            Panel::Split(SplitPanel {
                axis: Axis::Vertical,
                align: SplitterAlign::FromEnd(250.0),
                child_panel_ids: [id!(above_log).into(), id!(log_view).into()],
            }),
        );
        
        panels.insert(
            id!(above_log),
            Panel::Split(SplitPanel {
                axis: Axis::Horizontal,
                align: SplitterAlign::FromStart(200.0),
                child_panel_ids: [id!(file_tree).into(), id!(content).into()],
            }),
        );
        
        AppState { 
            panels,
            tabs,
            selected_panel_id: id!(content).into(),
            file_nodes,
            path: PathBuf::new(),
            editor_state: EditorState::new(),
        }
    }
    
    pub fn find_parent_panel_id(&self, child_id: PanelId)->Option<PanelId>{
        for (panel_id, panel) in self.panels.iter(){
            if panel.is_child_of(child_id){
                return Some(*panel_id)
            }
        }
        None
    }
    
    pub fn file_node_path(&self, file_node_id: FileNodeId) -> PathBuf {
        let mut components = Vec::new();
        let mut file_node = &self.file_nodes[file_node_id];
        while let Some(edge) = &file_node.parent_edge {
            components.push(&edge.name);
            file_node = &self.file_nodes[edge.file_node_id];
        }
        self.path.join(components.into_iter().rev().collect::<PathBuf>())
    }
    
    pub fn file_path_join(&self, components: &[&str]) -> PathBuf {
        self.path.join(components.into_iter().rev().collect::<PathBuf>())
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
                    FileNodeData::File{..} => None,
                },
            };
            file_nodes.insert(file_node_id, node);
            file_node_id
        }
        
        self.path = tree_data.path;

        self.file_nodes.clear();

        create_file_node(
            Some(id!(root).into()),
            &mut self.file_nodes,
            None,
            tree_data.root,
        );
    }
}

#[derive(Debug)]
pub enum Panel{
    Split(SplitPanel),
    Tab(TabPanel),
}

impl Panel {
    pub fn as_split_panel_mut(&mut self) -> &mut SplitPanel {
        match self{
            Self::Split(panel) => panel,
            _ => panic!(),
        }
    }
    
    pub fn as_tab_panel(&self) -> &TabPanel {
        match self {
            Self::Tab(panel) => panel,
            _ => panic!(),
        }
    }
    
    pub fn as_tab_panel_mut(&mut self) -> &mut TabPanel {
        match self{
            Self::Tab(panel) => panel,
            _ => panic!(),
        }
    }

    pub fn is_child_of(&self, panel_id: PanelId) -> bool {
        match self {
            Self::Split(panel) => panel.child_panel_ids[0] == panel_id || panel.child_panel_ids[1] == panel_id,
            _ => false,
        }
    }
}

impl TabPanel{
    pub fn tab_position(&self, find_id:TabId)->usize{
        self.tab_ids.iter().position(|id| *id == find_id).unwrap()
    }
    
    pub fn selected_tab_id(&self)->Option<TabId>{
        if let Some(index) = self.selected_tab{
            if index < self.tab_ids.len(){
                return Some(self.tab_ids[index])
            }
        }
        None
    }
}

#[derive(Clone, Debug)]
pub struct SplitPanel {
    pub axis: Axis,
    pub align: SplitterAlign,
    pub child_panel_ids: [PanelId; 2],
}

impl SplitPanel{
    pub fn child_position(&self, find_id:PanelId)->usize{
        self.child_panel_ids.iter().position( | id | *id == find_id).unwrap()
    }
}

#[derive(Clone, Debug)]
pub struct TabPanel {
    pub tab_ids: Vec<TabId>,
    pub selected_tab: Option<usize>
}

pub struct Tab {
    pub name: String,
    pub kind: TabKind,
}

pub enum TabKind {
    LogView,
    ShaderView,
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

