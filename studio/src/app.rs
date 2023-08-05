use {
    crate::{
        makepad_platform::*,
        makepad_draw::*,
        makepad_widgets::*,
        makepad_widgets::file_tree::*,
        makepad_widgets::dock::*,
        makepad_collab_protocol::{
            CollabRequest,
            CollabResponse,
            CollabClientAction,
            FileNodeData,
            FileTreeData,
            unix_str::UnixString,
            unix_path::UnixPathBuf,
        },
        collab_client::CollabClient,
        build::{
            build_manager::{
                BuildManager,
                //BuildManagerAction
            },
        },
        //app_inner::AppInner,
        //app_state::AppState,
    },
    //makepad_regex::regex::Regex
};

live_design!{
    import makepad_widgets::theme::*;
    import makepad_widgets::frame::*;
    import makepad_widgets::file_tree::FileTree;
    import makepad_widgets::log_list::LogList;
    import makepad_widgets::dock::*;
    import makepad_widgets::desktop_window::DesktopWindow;
    const FS_ROOT = ""
    App = {{App}} {
        ui: <DesktopWindow> {
            caption_bar = {visible: true, caption_label = {label = {label: "Makepad Studio"}}},
            dock = <Dock> {
                walk: {height: Fill, width: Fill}
                // alright so how would we do this thing
                // a dock has a serialised data rep
                root = Splitter {
                    axis: Horizontal,
                    align: FromA(200.0),
                    a: file_tree,
                    b: content1
                }
                
                content1 = Splitter {
                    axis: Vertical,
                    align: FromB(200.0),
                    a: content2,
                    b: log_list
                }
                
                content2 = Tabs {
                    tabs: [file1, file2, file3],
                    selected: 0
                }
                
                file1 = Tab {
                    name: "File1"
                    kind: Empty1
                }
                
                file2 = Tab {
                    name: "File2"
                    kind: Empty2
                }
                
                file3 = Tab {
                    name: "File3"
                    kind: Empty3
                }
                
                file_tree = Tab {
                    name: "FileTree",
                    kind: FileTree
                }
                
                log_list = Tab {
                    name: "LogList",
                    kind: Empty3
                }
                
                Empty1 = <Rect> {draw_bg: {color: #533}}
                Empty2 = <Rect> {draw_bg: {color: #353}}
                Empty3 = <Rect> {draw_bg: {color: #335}}
                Empty4 = <Rect> {draw_bg: {color: #535}}
                FileTree = <FileTree> {}
                //LogList = <LogList>{}
            }
        }
    }
}

#[derive(Live)]
pub struct App {
    #[live] ui: WidgetRef,
    #[live] collab_client: CollabClient,
    #[live] build_manager: BuildManager,
    #[rust] file_system: FileSystem
}

impl LiveHook for App {
    fn before_live_design(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
        crate::build::build_manager::live_design(cx);
        crate::collab_client::live_design(cx);
        crate::shader_view::live_design(cx);
        crate::run_view::live_design(cx);
    }
    
    fn after_new_from_doc(&mut self, _cx: &mut Cx) {
        self.collab_client.send_request(CollabRequest::LoadFileTree {with_data: false});
    }
}

app_main!(App);

impl App {
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        let dock = self.ui.get_dock(id!(dock));
        match event {
            Event::Draw(event) => {
                let cx = &mut Cx2d::new(cx, event);
                while let Some(next) = self.ui.draw_widget(cx).hook_widget() {
                    if let Some(mut file_tree) = next.into_file_tree().borrow_mut() {
                        file_tree.set_folder_is_open(cx, live_id!(root).into(),true, Animate::No);
                        self.file_system.draw_file_node(
                            cx,
                            live_id!(root).into(),
                            &mut *file_tree
                        );
                    }
                }
                return
            }
            _ => ()
        }
        for action in self.collab_client.handle_event(cx, event) {
            match action {
                CollabClientAction::Response(response) => match response {
                    CollabResponse::LoadFileTree(response) => {
                        self.file_system.load_file_tree(response.unwrap());
                        self.ui.get_file_tree(id!(file_tree)).redraw(cx);
                        // dock.select_tab(cx, dock, state, live_id!(file_tree).into(), live_id!(file_tree).into(), Animate::No);
                    }
                    _response => {
                        // self.build_manager.handle_collab_response(cx, state, &response);
                        // self.editors.handle_collab_response(cx, &mut state.editor_state, response, &mut self.collab_client.request_sender())
                    }
                },
                CollabClientAction::Notification(_notification) => {
                    //self.editors.handle_collab_notification(cx, &mut state.editor_state, notification)
                }
            }
        }
        let actions = self.ui.handle_widget_event(cx, event);
        
        if let Some(tab_id) = dock.clicked_tab_close(&actions) {
            dock.close_tab(cx, tab_id);
        }
        if let Some(tab_id) = dock.should_tab_start_drag(&actions) {
            dock.tab_start_drag(cx, tab_id, DragItem::FilePath {
                path: "".to_string(), //String::from("file://") + &*path.into_unix_string().to_string_lossy(),
                internal_id: Some(tab_id)
            });
        }
        // alright so drop validation
        if let Some(drag) = dock.should_accept_drag(&actions) {
            if drag.items.len() == 1 {
                dock.accept_drag(cx, drag);
            }
        }
        if let Some(drop) = dock.has_drop(&actions) {
            if let DragItem::FilePath {path: _, internal_id} = &drop.items[0] {
                if let Some(internal_id) = internal_id { // from inside the app
                    if cx.keyboard.modifiers().logo {
                        dock.drop_clone(cx, drop.abs, *internal_id, live_id!(drop));
                    }
                    else {
                        dock.drop_move(cx, drop.abs, *internal_id);
                    }
                }
                else { // external file, we have to create a new tab
                    dock.drop_create(cx, drop.abs, live_id!(newitem), live_id!(Empty4))
                }
            }
        }
        
        //self.inner.handle_event(cx, event, &mut *dock.borrow_mut().unwrap(), &mut self.app_state);
    }
}

#[derive(Default)]
struct FileSystem {
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
    fn draw_file_node(&self, cx: &mut Cx2d, file_node_id: FileNodeId, file_tree: &mut FileTree) {
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
