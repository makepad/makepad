use {
    crate::{
        makepad_platform::*,
        makepad_component::{
            DesktopWindow,
        },
        makepad_studio_component::{
            shader_view::ShaderView,
            dock::{Dock, DockAction, DragPosition, PanelId},
            file_tree::{FileTreeAction, FileNodeId, FileTree},
            splitter::{SplitterAlign},
            tab_bar::TabId,
        },
        collab::{
            collab_client::CollabClient,
            collab_protocol::{
                FileTreeData,
                CollabRequest,
                CollabResponse,
                CollabClientAction
            },
        },
        builder::{
            builder_client::BuilderClient,
            builder_protocol::{
                BuilderCmd,
            }
        },
        app_state::{TabKind, AppState, SplitPanel, TabPanel, Panel, Tab},
        log_view::{LogView},
        editors::{Editors},
    },
    std::{
        path::{Path, PathBuf},
    },
};

live_register!{
    AppInner: {{AppInner}} {
        window: {caption: "Makepad Studio"}
    }
}

#[derive(Live, LiveHook)]
pub struct AppInner {
    
    window: DesktopWindow,
    dock: Dock,
    file_tree: FileTree,
    log_view: LogView,
    shader_view: ShaderView,
    editors: Editors,
    collab_client: CollabClient,
    builder_client: BuilderClient
}

impl AppInner {
    
    pub fn draw(&mut self, cx: &mut Cx2d, state: &AppState) {
        if self.window.begin(cx, None).is_ok() {
            if self.dock.begin(cx).is_ok() {
                self.draw_panel(cx, state, id!(root).into());
                self.dock.end(cx);
            }
            self.window.end(cx);
        }
    }
    
    fn draw_panel(&mut self, cx: &mut Cx2d, state: &AppState, panel_id: PanelId) {
        let panel = &state.panels[panel_id];
        match panel {
            Panel::Split(SplitPanel {child_panel_ids, axis, align}) => {
                self.dock.begin_split_panel(cx, panel_id, *axis, *align);
                self.draw_panel(cx, state, child_panel_ids[0]);
                self.dock.middle_split_panel(cx);
                self.draw_panel(cx, state, child_panel_ids[01]);
                self.dock.end_split_panel(cx);
            }
            Panel::Tab(TabPanel {tab_ids, selected_tab}) => {
                self.dock.begin_tab_panel(cx, panel_id);
                if self.dock.begin_tab_bar(cx, *selected_tab).is_ok() {
                    for tab_id in tab_ids {
                        let tab = &state.tabs[*tab_id];
                        self.dock.draw_tab(cx, *tab_id, &tab.name);
                    }
                    self.dock.end_tab_bar(cx);
                }
                if let Some(tab_id) = self.dock.selected_tab_id(cx, panel_id) {
                    let tab = &state.tabs[tab_id];
                    match tab.kind {
                        TabKind::ShaderView => {
                            self.shader_view.draw(cx)
                        }
                        TabKind::LogView => {
                            self.log_view.draw(cx, &state.editor_state)
                        }
                        TabKind::FileTree => {
                            if self.file_tree.begin(cx).is_ok() {
                                self.draw_file_node(cx, state, id!(root).into());
                                self.file_tree.end(cx);
                            }
                        }
                        TabKind::CodeEditor {..} => {
                            self.editors.draw(
                                cx,
                                &state.editor_state,
                                tab_id.into(),
                            );
                        }
                    }
                }
                self.dock.end_tab_panel(cx);
            }
        }
    }
    
    fn draw_file_node(&mut self, cx: &mut Cx2d, state: &AppState, file_node_id: FileNodeId) {
        let file_node = &state.file_nodes[file_node_id];
        match &file_node.child_edges {
            Some(child_edges) => {
                if self.file_tree.begin_folder(cx, file_node_id, &file_node.name).is_ok()
                {
                    for child_edge in child_edges {
                        self.draw_file_node(cx, state, child_edge.file_node_id);
                    }
                    self.file_tree.end_folder();
                }
            }
            None => {
                self.file_tree.file(cx, file_node_id, &file_node.name);
            }
        }
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event, state: &mut AppState) {
        self.window.handle_event(cx, event);
        
        match event {
            Event::Construct => {
                self.collab_client.send_request(CollabRequest::LoadFileTree {with_data: false});
                self.create_code_editor_tab(
                    cx,
                    state,
                    id!(content).into(),
                    None,
                    state.file_path_join(&["studio/component/src/shader_view.rs"])
                );
                self.builder_client.send_cmd(BuilderCmd::CargoCheck);
            }
            Event::Draw(draw_event) => {
                self.draw(&mut Cx2d::new(cx, draw_event), state);
            }
            _ => ()
        }
        
        for action in self.dock.handle_event(cx, event) {
            match action {
                DockAction::SplitPanelChanged {panel_id, axis, align} => {
                    if let Panel::Split(panel) = &mut state.panels[panel_id] {
                        panel.axis = axis;
                        panel.align = align;
                    }
                    // do shit here
                    self.dock.redraw(cx);
                    self.redraw_panel(cx, state, panel_id);
                }
                DockAction::TabBarReceivedDraggedItem(panel_id, item) => {
                    for file_url in &item.file_urls {
                        let path = Path::new(&file_url[7..]).to_path_buf();
                        self.create_code_editor_tab(cx, state, panel_id, None, path);
                    }
                }
                DockAction::TabWasPressed(panel_id, tab_id) => {
                    self.select_tab(cx, state, panel_id, tab_id, Animate::Yes)
                }
                DockAction::TabCloseWasPressed(panel_id, tab_id) => {
                    let tab = &state.tabs[tab_id];
                    match tab.kind {
                        TabKind::CodeEditor {session_id} => {
                            let panel = state.panels[panel_id].as_tab_panel_mut();
                            self.editors.set_view_session_id(
                                cx,
                                &mut state.editor_state,
                                tab_id.into(),
                                None,
                            );
                            
                            state.editor_state.destroy_session(session_id, &mut self.collab_client.request_sender());
                            
                            panel.tab_ids.remove(panel.tab_position(tab_id));
                            state.tabs.remove(&tab_id);
                            
                            self.dock.set_next_selected_tab(cx, panel_id, tab_id, Animate::Yes);
                            self.dock.redraw_tab_bar(cx, panel_id);
                        }
                        _ => {}
                    }
                }
                DockAction::TabReceivedDraggedItem(panel_id, tab_id, item) => {
                    for file_url in &item.file_urls {
                        let path = Path::new(&file_url[7..]).to_path_buf();
                        self.create_code_editor_tab(cx, state, panel_id, Some(tab_id), path);
                    }
                }
                DockAction::ContentsReceivedDraggedItem(panel_id, position, item) => {
                    let panel_id = match position {
                        DragPosition::Center => panel_id,
                        _ => self.split_tab_panel(cx, state, panel_id, position),
                    };
                    for file_url in &item.file_urls {
                        let path = Path::new(&file_url[7..]).to_path_buf();
                        self.create_code_editor_tab(cx, state, panel_id, None, path);
                    }
                }
            }
        }
        
        for action in self.file_tree.handle_event(cx, event) {
            match action {
                FileTreeAction::WasClicked(file_node_id) => {
                    let node = &state.file_nodes[file_node_id];
                    if node.is_file() {
                        let path = state.file_node_path(file_node_id);
                        self.create_code_editor_tab(cx, state, state.selected_panel_id, None, path);
                    }
                }
                FileTreeAction::ShouldStartDragging(file_node_id) => {
                    let path = state.file_node_path(file_node_id);
                    self.file_tree.start_dragging_file_node(
                        cx,
                        file_node_id,
                        DraggedItem {
                            file_urls: vec![
                                String::from("file://") + &*path.into_os_string().to_string_lossy(),
                            ],
                        },
                    )
                }
            }
        }
        
        let mut panel_id_stack = vec![id!(root).into()];
        while let Some(panel_id) = panel_id_stack.pop() {
            let panel = &state.panels[panel_id];
            match panel {
                Panel::Split(SplitPanel {child_panel_ids, ..}) => {
                    for child_id in child_panel_ids {
                        panel_id_stack.push(*child_id);
                    }
                }
                Panel::Tab(tab_panel) => {
                    if let Some(tab_id) = tab_panel.selected_tab_id() {
                        if self.editors.has_editor(tab_id.into()) {
                            self.editors.handle_event(
                                cx,
                                &mut state.editor_state,
                                tab_id.into(),
                                event,
                                &mut self.collab_client.request_sender(),
                            );
                        }
                    }
                }
            }
        }
        
        for action in self.collab_client.handle_event(cx, event) {
            match action {
                CollabClientAction::Response(response) => match response {
                    CollabResponse::LoadFileTree(response) => {
                        self.load_file_tree(cx, state, response.unwrap());
                        self.select_tab(cx, state, id!(file_tree).into(), id!(file_tree).into(), Animate::No);
                    }
                    response => {
                        self.editors.handle_collab_response(cx, &mut state.editor_state, response, &mut self.collab_client.request_sender())
                    }
                },
                CollabClientAction::Notification(notification) => {
                    self.editors.handle_collab_notification(cx, &mut state.editor_state, notification)
                }
            }
        }
        
        let msgs = self.builder_client.handle_event(cx, event);
        if msgs.len()>0 {
            self.editors.handle_builder_messages(cx, &mut state.editor_state, msgs);
            // lets redraw the logview
            self.log_view.redraw(cx);
        }
        
        self.log_view.handle_event_with_fn(cx, event,&mut |_,_|{});
        self.shader_view.handle_event(cx, event);
    }
    
    
    fn load_file_tree(&mut self, cx: &mut Cx, state: &mut AppState, file_tree_data: FileTreeData) {
        self.file_tree.forget();
        state.load_file_tree(file_tree_data);
        self.file_tree.set_folder_is_open(cx, id!(root).into(), true, Animate::No);
        self.file_tree.redraw(cx);
    }
    
    fn split_tab_panel(
        &mut self,
        cx: &mut Cx,
        state: &mut AppState,
        panel_id: PanelId,
        position: DragPosition,
    ) -> PanelId {
        let parent_panel_id = state.find_parent_panel_id(panel_id);
        
        let new_panel_id = state.panels.insert_unique(
            Panel::Tab(TabPanel {
                tab_ids: Vec::new(),
                selected_tab: None
            }),
        );
        
        let new_parent_panel_id = state.panels.insert_unique(
            Panel::Split(SplitPanel {
                axis: match position {
                    DragPosition::Left | DragPosition::Right => Axis::Horizontal,
                    DragPosition::Top | DragPosition::Bottom => Axis::Vertical,
                    _ => panic!(),
                },
                align: SplitterAlign::Weighted(0.5),
                child_panel_ids: match position {
                    DragPosition::Left | DragPosition::Top => [new_panel_id, panel_id],
                    DragPosition::Right | DragPosition::Bottom => [panel_id, new_panel_id],
                    _ => panic!(),
                },
            }),
        );
        
        if let Some(parent_panel_id) = parent_panel_id {
            let parent_panel = &mut state.panels[parent_panel_id].as_split_panel_mut();
            let position = parent_panel.child_position(panel_id);
            parent_panel.child_panel_ids[position] = new_parent_panel_id;
        }
        
        self.dock.redraw(cx);
        self.redraw_panel(cx, state, panel_id);
        
        new_panel_id
    }
    
    fn create_code_editor_tab(
        &mut self,
        cx: &mut Cx,
        state: &mut AppState,
        panel_id: PanelId,
        next_tab_id: Option<TabId>,
        path: PathBuf,
    ) {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        
        let session_id = state.editor_state.create_session(path, &mut self.collab_client.request_sender());
        
        let tab_id = state.tabs.insert_unique(Tab {
            name,
            kind: TabKind::CodeEditor {session_id},
        },);
        
        let panel = state.panels[panel_id].as_tab_panel_mut();
        
        match next_tab_id {
            Some(next_tab_id) => {
                panel.tab_ids.insert(panel.tab_position(next_tab_id), tab_id);
            }
            None => panel.tab_ids.push(tab_id),
        }
        self.select_tab(cx, state, panel_id, tab_id, Animate::No);
    }
    
    fn select_tab(&mut self, cx: &mut Cx, state: &mut AppState, panel_id: PanelId, tab_id: TabId, animate: Animate) {
        let tab_panel = state.panels[panel_id].as_tab_panel_mut();
        let tab = &state.tabs[tab_id];
        tab_panel.selected_tab = Some(tab_panel.tab_position(tab_id));
        self.dock.set_selected_tab_id(cx, panel_id, Some(tab_id), animate);
        self.dock.redraw_tab_bar(cx, panel_id);
        match tab.kind {
            TabKind::CodeEditor {session_id} => {
                self.editors.set_view_session_id(
                    cx,
                    &mut state.editor_state,
                    tab_id.into(),
                    Some(session_id),
                );
            }
            _ => {}
        }
    }
    
    fn redraw_panel(&mut self, cx: &mut Cx, state: &AppState, panel_id: PanelId) {
        match &state.panels[panel_id] {
            Panel::Split(panel) => {
                for child_panel_id in panel.child_panel_ids {
                    self.redraw_panel(cx, state, child_panel_id);
                }
            }
            Panel::Tab(_) => {
                self.dock.redraw_tab_bar(cx, panel_id);
                
                if let Some(tab_id) = self.dock.selected_tab_id(cx, panel_id) {
                    let tab = &state.tabs[tab_id];
                    match tab.kind {
                        TabKind::ShaderView => {
                            self.shader_view.redraw(cx);
                        }
                        TabKind::LogView => {
                            self.log_view.redraw(cx);
                        }
                        TabKind::FileTree => {
                            self.file_tree.redraw(cx);
                        }
                        TabKind::CodeEditor {..} => {
                            self.editors.redraw_view(cx, tab_id.into());
                        }
                    }
                }
            }
        }
    }
}

