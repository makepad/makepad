use {
    crate::{
        app_io::AppIO,
        app_state::{PanelKind, TabKind, AppState, SplitPanel, TabPanel, Panel, Tab},
        editor_state::{SessionId},
        code_editor::{
            protocol::{FileTreeData,Notification, Request, Response, ResponseOrNotification},
        },
        editors::{Editors},
    },
    makepad_render::*,
    makepad_widget::{
        DesktopWindow,
        dock::{Dock, DockAction, DragPosition, PanelId},
        file_tree::{FileTreeAction, FileNodeId, FileTree},
        splitter::{SplitterAlign},
        tab_bar::TabId,
    },
    std::{
        path::{Path, PathBuf},
        sync::mpsc::{TryRecvError},
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
    editors: Editors,
    
    #[rust(AppIO::new(cx))] io: AppIO
}

impl AppInner {
    
    pub fn draw(&mut self, cx: &mut Cx, state: &AppState) {
        if self.window.begin(cx, None).is_ok() {
            if self.dock.begin(cx).is_ok() {
                self.draw_panel(cx, state, state.root_panel_id);
                self.dock.end(cx);
            }
            self.window.end(cx);
        }
        //cx.redraw_all();
    }
    
    fn draw_panel(&mut self, cx: &mut Cx, state: &AppState, panel_id: PanelId) {
        let panel = &state.panels_by_panel_id[panel_id];
        match &panel.kind {
            PanelKind::Split(SplitPanel {child_panel_ids, axis, align}) => {
                
                self.dock.begin_split_panel(cx, panel_id, *axis, *align);
                self.draw_panel(cx, state, child_panel_ids[0]);
                self.dock.middle_split_panel(cx);
                self.draw_panel(cx, state, child_panel_ids[01]);
                self.dock.end_split_panel(cx);
            }
            PanelKind::Tab(TabPanel {tab_ids, ..}) => {
                self.dock.begin_tab_panel(cx, panel_id);
                if self.dock.begin_tab_bar(cx).is_ok() {
                    for tab_id in tab_ids {
                        let tab = &state.tabs_by_tab_id[*tab_id];
                        self.dock.draw_tab(cx, *tab_id, &tab.name);
                    }
                    self.dock.end_tab_bar(cx);
                }
                if let Some(tab_id) = self.dock.selected_tab_id(cx, panel_id) {
                    let tab = &state.tabs_by_tab_id[tab_id];
                    match tab.kind {
                        TabKind::FileTree => {
                            if self.file_tree.begin(cx).is_ok() {
                                self.draw_file_node(cx, state, state.root_file_node_id);
                                self.file_tree.end(cx);
                            }
                        }
                        TabKind::CodeEditor {..} => {
                            let panel = state.panels_by_panel_id[panel_id].as_tab_panel();
                            self.editors.draw(
                                cx,
                                &state.editor_state,
                                panel.editor_view_id.unwrap(),
                            );
                        }
                    }
                }
                self.dock.end_tab_panel(cx);
            }
        }
    }
    
    fn draw_file_node(&mut self, cx: &mut Cx, state: &AppState, file_node_id: FileNodeId) {
        let file_node = &state.file_nodes_by_file_node_id[file_node_id];
       // println!("DRAWING NODE {}", &file_node.name);
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
        
        if let Event::Construct = event {
            self.send_request(Request::LoadFileTree());
            self.create_code_editor_tab(
                cx,
                state,
                state.content_panel_id,
                None,
                state.file_path_join(&["widget/src/button.rs"])
            );
        }
        
        let mut actions = Vec::new();
        self.dock.handle_event(cx, event, &mut | _, action | actions.push(action));
        for action in actions {
            match action {
                DockAction::SplitPanelChanged {panel_id, axis, align} => {
                    if let PanelKind::Split(panel) = &mut state.panels_by_panel_id[panel_id].kind {
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
                    self.select_tab(cx, state, panel_id, tab_id, true)
                }
                DockAction::TabCloseWasPressed(panel_id, tab_id) => {
                    let tab = &state.tabs_by_tab_id[tab_id];
                    match tab.kind {
                        TabKind::CodeEditor {session_id} => {
                            let panel = state
                                .panels_by_panel_id
                                .get_mut(panel_id)
                                .unwrap()
                                .as_tab_panel_mut();
                            if let Some(editor_view_id) = panel.editor_view_id {
                                self.editors.set_view_session_id(
                                    cx,
                                    &mut state.editor_state,
                                    editor_view_id,
                                    None,
                                );
                            }
                            state.editor_state.destroy_session(session_id, &mut {
                                let request_sender = &self.io.request_sender;
                                move | request | request_sender.send(request).unwrap()
                            });
                            panel.tab_ids.remove(
                                panel
                                    .tab_ids
                                    .iter()
                                    .position( | existing_tab_id | *existing_tab_id == tab_id)
                                    .unwrap(),
                            );
                            state.tabs_by_tab_id.remove(tab_id);
                            state.tab_id_allocator.deallocate(tab_id.0);
                            self.dock.set_next_selected_tab(cx, panel_id, tab_id, true);
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
        
        let mut actions = Vec::new();
        self.file_tree.handle_event(cx, event, &mut | _cx, action | actions.push(action));
        for action in actions {
            match action {
                FileTreeAction::WasClicked(file_node_id) => {
                    let node = &state.file_nodes_by_file_node_id[file_node_id];
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
        
        let mut panel_id_stack = vec![state.root_panel_id];
        while let Some(panel_id) = panel_id_stack.pop() {
            let panel = &state.panels_by_panel_id[panel_id];
            match &panel.kind {
                PanelKind::Split(SplitPanel {child_panel_ids, ..}) => {
                    for child_id in child_panel_ids {
                        panel_id_stack.push(*child_id);
                    }
                }
                PanelKind::Tab(TabPanel {
                    editor_view_id,
                    ..
                }) => {
                    if let Some(code_editor_view_id) = editor_view_id {
                        let request_sender = &self.io.request_sender;
                        self.editors.handle_event(
                            cx,
                            &mut state.editor_state,
                            *code_editor_view_id,
                            event,
                            &mut | request | request_sender.send(request).unwrap(),
                        );
                    }
                }
            }
        }
        
        match event {
            Event::Signal(event)
            if event
                .signals
                .contains_key(&self.io.response_or_notification_signal) =>
            {
                loop {
                    match self.io.response_or_notification_receiver.try_recv() {
                        Ok(ResponseOrNotification::Response(response)) => {
                            self.handle_response(cx, state, response)
                        }
                        Ok(ResponseOrNotification::Notification(notification)) => {
                            self.handle_notification(cx, state, notification)
                        }
                        Err(TryRecvError::Empty) => break,
                        _ => panic!(),
                    }
                }
            }
            _ => {}
        }
    }
    
    fn handle_response(&mut self, cx: &mut Cx, state: &mut AppState, response: Response) {
        match response {
            Response::LoadFileTree(response) => {
                self.load_file_tree(cx, state, response.unwrap());
                self.select_tab(cx, state, state.side_bar_panel_id, state.file_tree_tab_id, false);
            }
            response => {
                self.editors.handle_response(cx, &mut state.editor_state, response, &mut {
                    let request_sender = &self.io.request_sender;
                    move | request | request_sender.send(request).unwrap()
                })
            }
        };
    }
    
    fn handle_notification(&mut self, cx: &mut Cx, state: &mut AppState, notification: Notification) {
        match notification {
            notification => {
                self.editors
                    .handle_notification(cx, &mut state.editor_state, notification)
            }
        }
    }
    
    fn load_file_tree(&mut self, cx: &mut Cx, state: &mut AppState, file_tree_data: FileTreeData) {
        self.file_tree.forget();
        state.load_file_tree(file_tree_data);
        self.file_tree.set_folder_is_open(cx, state.root_file_node_id, true, false);
        self.file_tree.redraw(cx);
    }
    
    fn split_tab_panel(
        &mut self,
        cx: &mut Cx,
        state: &mut AppState,
        panel_id: PanelId,
        position: DragPosition,
    ) -> PanelId {
        let panel = &state.panels_by_panel_id[panel_id];
        let parent_panel_id = panel.parent_panel_id;
        let new_parent_panel_id = PanelId(state.panel_id_allocator.allocate());
        let new_panel_id = PanelId(state.panel_id_allocator.allocate());
        
        let panel = &mut state.panels_by_panel_id[panel_id];
        panel.parent_panel_id = Some(new_parent_panel_id);
        
        state.panels_by_panel_id.insert(
            new_panel_id,
            Panel {
                parent_panel_id: Some(new_parent_panel_id),
                kind: PanelKind::Tab(TabPanel {
                    tab_ids: Vec::new(),
                    editor_view_id: None,
                }),
            },
        );
        
        state.panels_by_panel_id.insert(
            new_parent_panel_id,
            Panel {
                parent_panel_id,
                kind: PanelKind::Split(SplitPanel {
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
            },
        );
        
        if let Some(parent_panel_id) = parent_panel_id {
            let parent_panel = &mut state.panels_by_panel_id[parent_panel_id].as_split_panel_mut();
            let position = parent_panel
                .child_panel_ids
                .iter()
                .position( | child_panel_id | *child_panel_id == panel_id)
                .unwrap();
            parent_panel.child_panel_ids[position] = new_parent_panel_id;
        }
        /*
        self.dock.set_split_panel_axis(
            cx,
            new_parent_panel_id,
            match position {
                DragPosition::Left | DragPosition::Right => Axis::Horizontal,
                DragPosition::Top | DragPosition::Bottom => Axis::Vertical,
                _ => panic!(),
            },
        );*/
        
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
        let tab_id = TabId(state.tab_id_allocator.allocate());
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let session_id = state.editor_state.create_session(path, &mut {
            let request_sender = &self.io.request_sender;
            move | request | request_sender.send(request).unwrap()
        });
        state.tabs_by_tab_id.insert(
            tab_id,
            Tab {
                name,
                kind: TabKind::CodeEditor {session_id},
            },
        );
        let panel = state
            .panels_by_panel_id
            .get_mut(panel_id)
            .unwrap()
            .as_tab_panel_mut();
        match next_tab_id {
            Some(next_tab_id) => {
                panel.tab_ids.insert(
                    panel
                        .tab_ids
                        .iter()
                        .position( | existing_tab_id | *existing_tab_id == next_tab_id)
                        .unwrap(),
                    tab_id,
                );
            }
            None => panel.tab_ids.push(tab_id),
        }
        self.select_tab(cx, state, panel_id, tab_id, false);
    }
    
    fn select_tab(&mut self, cx: &mut Cx, state: &mut AppState, panel_id: PanelId, tab_id: TabId, should_animate:bool) {
        let tab = &state.tabs_by_tab_id[tab_id];
        self.dock.set_selected_tab_id(cx, panel_id, Some(tab_id), should_animate);
        self.dock.redraw_tab_bar(cx, panel_id);
        match tab.kind {
            TabKind::CodeEditor {session_id} => {
                self.set_code_editor_view_session_id(cx, state, panel_id, session_id);
            }
            _ => {}
        }
    }
    
    fn set_code_editor_view_session_id(
        &mut self,
        cx: &mut Cx,
        state: &mut AppState,
        panel_id: PanelId,
        session_id: SessionId,
    ) {
        let panel = state
            .panels_by_panel_id
            .get_mut(panel_id)
            .unwrap()
            .as_tab_panel_mut();
        match panel.editor_view_id {
            Some(view_id) => {
                self.editors.set_view_session_id(
                    cx,
                    &mut state.editor_state,
                    view_id,
                    Some(session_id),
                );
            }
            None => {
                panel.editor_view_id = Some(self.editors.create_view(
                    cx,
                    &mut state.editor_state,
                    Some(session_id),
                ));
            }
        }
    }
    
    fn send_request(&mut self, request: Request) {
        self.io.request_sender.send(request).unwrap();
    }
    
    fn redraw_panel(&mut self, cx: &mut Cx, state: &AppState, panel_id: PanelId) {
        match &state.panels_by_panel_id[panel_id].kind {
            PanelKind::Split(panel) => {
                for child_panel_id in panel.child_panel_ids {
                    self.redraw_panel(cx, state, child_panel_id);
                }
            }
            PanelKind::Tab(panel) => {
                self.dock.redraw_tab_bar(cx, panel_id);
                if let Some(code_editor_view_id) = panel.editor_view_id {
                    self.editors.redraw_view(cx, code_editor_view_id);
                }
            }
        }
    }
}

