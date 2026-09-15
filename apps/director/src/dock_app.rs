// Structured presentation: use the stock Dock for project files and live work.
impl App {
    fn ensure_project_tree(&self, cx: &mut Cx) {
        let dock = self.ui.dock(cx, ids!(dock));
        let mut state = dock.clone_state().unwrap_or_default();
        if matches!(state.get(&id!(project_tree_tab)), Some(DockItem::Tab { kind, .. }) if *kind == id!(ProjectTreeTab))
        {
            return;
        }
        let Some(old_root) = state.remove(&id!(root)) else {
            return;
        };
        let work = dock.unique_id(id!(studio_work_root).0);
        let project = dock.unique_id(id!(project_tabs).0);
        state.insert(work, old_root);
        state.insert(
            project,
            DockItem::Tabs {
                tabs: vec![id!(project_tree_tab)],
                selected: 0,
                closable: true,
                hide_tab_bar: false,
            },
        );
        state.insert(
            id!(project_tree_tab),
            DockItem::Tab {
                name: "Project".into(),
                template: id!(PermanentTab),
                kind: id!(ProjectTreeTab),
            },
        );
        state.insert(
            id!(root),
            DockItem::Splitter {
                axis: SplitterAxis::Horizontal,
                align: SplitterAlign::FromA(216.0),
                a: project,
                b: work,
            },
        );
        dock.load_state_preserving_items(cx, state);
        self.save_dock(cx);
    }

    fn work_tabs_container(&self, cx: &mut Cx) -> Option<LiveId> {
        let dock = self.ui.dock(cx, ids!(dock));
        let mut state = dock.clone_state()?;
        if let Some(active) = self.active_item.filter(|id| *id != id!(project_tree_tab).0) {
            if let Some((container, _)) = dock.find_tab_bar_of_tab(LiveId(active)) {
                return Some(container);
            }
        }
        let mut containers: Vec<_> = state.iter().filter_map(|(id, item)| {
            matches!(item, DockItem::Tabs { tabs, .. } if !tabs.contains(&id!(project_tree_tab))).then_some(*id)
        }).collect();
        containers.sort_by_key(|id| id.0);
        if let Some(container) = containers.first() {
            return Some(*container);
        }
        // Closing the last work pane can leave Project as the root. Restore a
        // right work pane without replacing the project widget or its worker.
        let previous = state.remove(&id!(root))?;
        let left = dock.unique_id(id!(studio_project_root).0);
        let right = dock.unique_id(id!(studio_new_work).0);
        state.insert(left, previous);
        state.insert(
            right,
            DockItem::Tabs {
                tabs: vec![],
                selected: 0,
                closable: true,
                hide_tab_bar: false,
            },
        );
        state.insert(
            id!(root),
            DockItem::Splitter {
                axis: SplitterAxis::Horizontal,
                align: SplitterAlign::FromA(216.0),
                a: left,
                b: right,
            },
        );
        dock.load_state_preserving_items(cx, state);
        Some(right)
    }

    fn start_project_tree(&mut self, cx: &mut Cx) {
        self.project_tree_ref = self
            .ui
            .dock(cx, ids!(dock))
            .item(id!(project_tree_tab))
            .widget(cx, ids!(project_tree));
        let project = self.project_dir();
        self.ui.label(cx, ids!(project_name)).set_text(
            cx,
            &project.file_name().unwrap_or_default().to_string_lossy(),
        );
        if let Some(mut tree) = self.project_tree_ref.borrow_mut::<StudioProjectTree>() {
            if let Err(error) = tree.set_project(cx, project) {
                log!("studio project tree: {error}");
            }
        };
    }

    fn reveal_active_project_file(&mut self, cx: &mut Cx) {
        let path = self
            .active_item
            .and_then(|id| self.code_tabs.get(&id))
            .cloned();
        if let Some(path) = path {
            if let Some(mut tree) = self.project_tree_ref.borrow_mut::<StudioProjectTree>() {
                if let Err(error) = tree.reveal(cx, &path) {
                    self.ui.label(cx, ids!(status_state)).set_text(cx, &error);
                }
            }
        }
    }

    fn internal_drag_tab(&self, cx: &mut Cx, item: &DragItem) -> Option<LiveId> {
        let DragItem::String {
            value,
            internal_id: Some(id),
        } = item
        else {
            return None;
        };
        if value != &format!("studio-tab:{}", std::process::id()) {
            return None;
        }
        self.ui
            .dock(cx, ids!(dock))
            .clone_state()
            .is_some_and(|state| matches!(state.get(id), Some(DockItem::Tab { .. })))
            .then_some(*id)
    }

    fn dragged_source_path(item: &DragItem) -> Option<PathBuf> {
        let DragItem::FilePath {
            path,
            internal_id: None,
        } = item
        else {
            return None;
        };
        let path_buf = PathBuf::from(path);
        (path_buf.is_absolute() && path.len() <= 4096 && !path.contains('\0')).then_some(path_buf)
    }

    fn handle_work_dock_action(&mut self, cx: &mut Cx, action: DockAction) {
        let dock = self.ui.dock(cx, ids!(dock));
        match action {
            DockAction::TabWasPressed(id) => {
                if id != id!(project_tree_tab) {
                    self.active_item = Some(id.0);
                }
            }
            DockAction::TabCloseWasPressed(id) => self.ui_action(cx, Action::CloseTab(id.0)),
            DockAction::ShouldTabStartDrag(id) => {
                dock.tab_start_drag(
                    cx,
                    id,
                    DragItem::String {
                        value: format!("studio-tab:{}", std::process::id()),
                        internal_id: Some(id),
                    },
                );
            }
            DockAction::Drag(event) => {
                let response = if event
                    .items
                    .iter()
                    .any(|item| self.internal_drag_tab(cx, item).is_some())
                {
                    DragResponse::Move
                } else if event
                    .items
                    .iter()
                    .any(|item| Self::dragged_source_path(item).is_some())
                {
                    DragResponse::Copy
                } else {
                    return;
                };
                dock.accept_drag(cx, event, response);
            }
            DockAction::Drop(event) => {
                for item in event.items.iter().take(32) {
                    if let Some(id) = self.internal_drag_tab(cx, item) {
                        dock.drop_move(cx, event.abs, id);
                        dock.select_tab(cx, id);
                        if id != id!(project_tree_tab) {
                            self.active_item = Some(id.0);
                        }
                    } else if let Some(path) = Self::dragged_source_path(item) {
                        match self.open_code(cx, path, true) {
                            Ok(id) => {
                                let id = LiveId(u64::from_str_radix(&id, 16).unwrap());
                                dock.drop_move(cx, event.abs, id);
                                dock.select_tab(cx, id);
                            }
                            Err(error) => {
                                self.ui.label(cx, ids!(status_state)).set_text(cx, &error)
                            }
                        }
                    }
                }
                dock.redraw(cx);
                self.save_dock(cx);
                self.refresh_workspace(cx);
            }
            DockAction::SplitPanelChanged { .. } => self.save_dock(cx),
            DockAction::None => {}
        }
    }
}
