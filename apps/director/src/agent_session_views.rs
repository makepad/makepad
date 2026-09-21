// A connection is a view of a session, never a transfer of its flow ownership.
impl App {
    fn refresh_terminal_inventory(&mut self) -> Result<(), String> {
        if self.agent_sessions.inventory_request.is_none() {
            let repo = self.project_dir();
            let request = self
                .agent_sessions
                .worker
                .as_mut()
                .ok_or("Terminal host unavailable")?
                .list(repo)?;
            self.agent_sessions.inventory_request = Some(request);
        }
        Ok(())
    }

    fn session_owner_flow(&self, session: &str) -> Option<String> {
        self.iterations
            .snapshot
            .engine
            .flows
            .values()
            .find(|flow| {
                flow.successor.is_none()
                    && format!("term-{:016x}", self.flow_terminal_id(&flow.id)) == session
            })
            .map(|flow| flow.id.clone())
    }

    fn terminal_view_for_flow(&self, flow: &str) -> Option<u64> {
        let own_tab = self.flow_terminal_id(flow);
        let session = format!("term-{own_tab:016x}");
        let is_view = |tab: u64| {
            self.agent_sessions
                .bindings
                .get(&tab)
                .is_some_and(|binding| {
                    !self.agent_sessions.mirrors.get(&tab).is_some_and(|info| {
                        self.agent_sessions.inventory_state_dir.as_ref() != Some(&info.state_dir)
                    }) && binding.open
                        && self
                            .agent_sessions
                            .mirrors
                            .get(&tab)
                            .map(|info| info.session_id.as_str())
                            .unwrap_or(&binding.session_id)
                            == session
                })
        };
        if is_view(own_tab) {
            return Some(own_tab);
        }
        self.agent_sessions
            .bindings
            .keys()
            .copied()
            .filter(|tab| is_view(*tab))
            .min()
    }

    fn refresh_terminal_view_labels(&mut self, cx: &mut Cx) {
        let mut labels = std::collections::BTreeMap::new();
        let flows: Vec<_> = self
            .iterations
            .snapshot
            .engine
            .flows
            .values()
            .filter(|flow| !self.iterations.deleting.contains(&flow.id))
            .map(|flow| (flow.id.clone(), flow.title.clone()))
            .collect();
        for (flow, old_title) in flows {
            let tab = self.flow_terminal_id(&flow);
            let info = self.agent_sessions.mirrors.get(&tab).or_else(|| {
                self.agent_sessions
                    .bindings
                    .get(&tab)
                    .and_then(|b| b.info.as_ref())
            });
            if let Some(info) = info {
                let title = info.title.clone();
                labels.insert(flow.clone(), title.clone());
                if title == old_title {
                    self.agent_sessions.reported_titles.remove(&flow);
                }
                if title != old_title
                    && self.agent_sessions.reported_titles.get(&flow) != Some(&title)
                {
                    if self
                        .submit_iteration(
                            cx,
                            IterationRequest::TerminalNamed {
                                flow: flow.clone(),
                                title: title.clone(),
                            },
                            "terminal_name",
                        )
                        .is_ok()
                    {
                        self.agent_sessions.reported_titles.insert(flow, title);
                    }
                }
            }
        }
        if let Some(mut view) = self
            .ui
            .widget(cx, ids!(flow_scene))
            .borrow_mut::<StudioIterationView>()
        {
            view.set_connection_labels(cx, labels);
        }
    }

    fn info_owner_flow(&self, info: &makepad_director::agent_session::SessionInfo) -> Option<String> {
        if self.agent_sessions.inventory_state_dir.as_ref() != Some(&info.state_dir) {
            return None;
        }
        self.session_owner_flow(&info.session_id)
    }

    fn terminal_owner_for_tab(&self, tab: u64) -> Option<String> {
        if let Some(info) = self.agent_sessions.mirrors.get(&tab) {
            return self.info_owner_flow(info);
        }
        self.session_owner_flow(&self.agent_sessions.bindings.get(&tab)?.session_id)
    }

    fn terminal_input_flow(&self, flow: &str) -> Result<String, String> {
        match self
            .agent_sessions
            .mirrors
            .get(&self.flow_terminal_id(flow))
        {
            Some(info) => self.info_owner_flow(info).ok_or_else(|| {
                "This shared terminal has no Studio flow owner for image feedback".into()
            }),
            None => Ok(flow.to_owned()),
        }
    }

    fn open_terminal_connections(&mut self, cx: &mut Cx, tab: u64) {
        let flow = self
            .iterations
            .snapshot
            .engine
            .flows
            .keys()
            .find(|flow| self.flow_terminal_id(flow) == tab)
            .cloned();
        self.update_terminal_connections(cx);
        if let Some(flow) = flow {
            if let Some(mut view) = self
                .ui
                .widget(cx, ids!(flow_scene))
                .borrow_mut::<StudioIterationView>()
            {
                view.open_agent_menu(cx, flow);
            }
        }
        if let Err(error) = self.refresh_terminal_inventory() {
            self.flow_note(cx, &error);
        }
    }

    fn update_terminal_connections(&mut self, cx: &mut Cx) {
        let choices = makepad_director::agent_session::terminal_menu(&self.agent_sessions.inventory);
        if let Some(mut view) = self
            .ui
            .widget(cx, ids!(flow_scene))
            .borrow_mut::<StudioIterationView>()
        {
            view.set_agent_menu(cx, choices);
        }
    }

    fn select_terminal_view(&mut self, tab: u64, session: Option<String>) -> Result<(), String> {
        if self
            .agent_sessions
            .view_requests
            .values()
            .any(|id| *id == tab)
        {
            return Err("A terminal connection is already pending".into());
        }
        if self.agent_sessions.requests.values().any(|(id, kind)| {
            *id == tab
                && matches!(
                    kind,
                    AgentTerminalRequest::Stop
                        | AgentTerminalRequest::Recover
                        | AgentTerminalRequest::Restore
                )
        }) || self
            .agent_sessions
            .terminal_reservations
            .values()
            .any(|(id, _)| *id == tab)
        {
            return Err("Wait for this terminal’s current process operation to finish".into());
        }
        let binding = self
            .agent_sessions
            .bindings
            .get(&tab)
            .ok_or("Lane terminal is unavailable")?;
        let cwd = binding.cwd.clone();
        let command = binding.initial_command.clone();
        let worker = self
            .agent_sessions
            .worker
            .as_mut()
            .ok_or("Terminal host unavailable")?;
        let request = if session.is_some() {
            worker.select_view(tab, session)?
        } else {
            worker.new_view(tab, cwd, command)?
        };
        self.agent_sessions.view_requests.insert(request, tab);
        Ok(())
    }

    fn attach_shared_terminal_view(
        &mut self,
        cx: &mut Cx,
        tab: u64,
        info: makepad_director::agent_session::SessionInfo,
    ) {
        if self.agent_sessions.inventory_state_dir.as_ref() == Some(&info.state_dir)
            && self
                .agent_sessions
                .bindings
                .get(&tab)
                .is_some_and(|binding| binding.session_id == info.session_id)
        {
            self.agent_sessions.mirrors.remove(&tab);
            self.attach_agent_terminal_view(cx, tab, info);
            self.refresh_agent_terminal_status(cx, tab);
            self.refresh_terminal_view_labels(cx);
            return;
        }
        let body = self.ui.dock(cx, ids!(dock)).item(LiveId(tab));
        let terminal = body.widget(cx, ids!(term));
        self.agent_sessions.mirrors.insert(tab, info.clone());
        if let Some(binding) = self.agent_sessions.bindings.get(&tab) {
            if binding.open && binding.view_uid == Some(terminal.widget_uid()) {
                if let Some(mut term) = terminal.borrow_mut::<MpTerm>() {
                    term.restart_with(cx, Some(info.cwd), Some(info.attach_command));
                }
            }
        }
        self.refresh_agent_terminal_status(cx, tab);
        self.refresh_terminal_view_labels(cx);
    }

    fn restore_terminal_views(&mut self, cx: &mut Cx) {
        let views: Vec<_> = self
            .agent_sessions
            .saved_views
            .iter()
            .filter(|(tab, _)| {
                self.agent_sessions
                    .bindings
                    .get(tab)
                    .is_some_and(|binding| binding.open && binding.view_uid.is_some())
            })
            .map(|(tab, session)| (*tab, session.clone()))
            .collect();
        for (tab, session) in views {
            self.agent_sessions.saved_views.remove(&tab);
            if let Some(info) = self
                .agent_sessions
                .inventory
                .iter()
                .find(|info| info.key() == session || info.session_id == session)
                .cloned()
            {
                self.attach_shared_terminal_view(cx, tab, info);
            } else {
                self.ui.label(cx, ids!(status_state)).set_text(
                    cx,
                    "A saved shared PTY has ended. Use Connect to choose a running terminal.",
                );
            }
        }
    }

    fn handle_terminal_view_reply(
        &mut self,
        cx: &mut Cx,
        reply: &makepad_director::agent_session::SessionReply,
    ) -> bool {
        use makepad_director::agent_session::SessionOutcome;
        if self.agent_sessions.inventory_request == Some(reply.request_id) {
            self.agent_sessions.inventory_request = None;
            match &reply.result {
                Ok(SessionOutcome::Inventory {
                    state_dir,
                    sessions,
                    views,
                }) => {
                    self.agent_sessions.inventory_state_dir = Some(state_dir.clone());
                    self.agent_sessions.inventory = sessions.clone();
                    for info in sessions {
                        for mirror in self.agent_sessions.mirrors.values_mut() {
                            if mirror.key() == info.key() {
                                *mirror = info.clone();
                            }
                        }
                        for binding in self.agent_sessions.bindings.values_mut() {
                            if let Some(current) = &mut binding.info {
                                if current.key() == info.key() {
                                    *current = info.clone();
                                }
                            }
                        }
                    }
                    if !self.agent_sessions.views_loaded {
                        self.agent_sessions.views_loaded = true;
                        self.agent_sessions.saved_views = views.iter().cloned().collect();
                    }
                    self.restore_terminal_views(cx);
                    self.update_terminal_connections(cx);
                    self.refresh_terminal_view_labels(cx);
                }
                Err(error) => {
                    self.flow_note(cx, error);
                    log!("studio terminal inventory: {error}");
                }
                _ => {}
            }
            return true;
        }
        let Some(tab) = self.agent_sessions.view_requests.remove(&reply.request_id) else {
            return false;
        };
        match &reply.result {
            Ok(SessionOutcome::ViewReady {
                tab: actual,
                session,
            }) if tab == *actual => {
                if let Some(info) = session {
                    self.attach_shared_terminal_view(cx, tab, info.clone());
                } else {
                    self.agent_sessions.mirrors.remove(&tab);
                    // Drop only the local attach client, never the remote PTY.
                    if let Some(mut term) = self
                        .ui
                        .dock(cx, ids!(dock))
                        .item(LiveId(tab))
                        .widget(cx, ids!(term))
                        .borrow_mut::<MpTerm>()
                    {
                        term.unload(cx);
                    }
                    if let Err(error) = self.queue_agent_terminal(tab, AgentTerminalRequest::Attach)
                    {
                        self.set_agent_terminal_error(tab, error);
                    }
                }
                self.refresh_agent_terminal_status(cx, tab);
                self.refresh_ai_context(cx);
                self.refresh_terminal_view_labels(cx);
            }
            Err(error) => {
                self.flow_note(cx, error);
                self.ui.label(cx, ids!(status_state)).set_text(cx, error);
            }
            _ => {}
        }
        true
    }

    fn handle_terminal_connection_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for action in actions {
            let Some(wa) = action.as_widget_action() else {
                continue;
            };
            if let MpTermAction::TitleChanged(title) = wa.cast::<MpTermAction>() {
                if let Some(tab) = self.tab_of_terminal(cx, wa.widget_uid) {
                    if let Some(info) = self.agent_sessions.mirrors.get_mut(&tab.0) {
                        info.title = title.clone();
                    } else if let Some(info) = self
                        .agent_sessions
                        .bindings
                        .get_mut(&tab.0)
                        .and_then(|b| b.info.as_mut())
                    {
                        info.title = title;
                    }
                }
            }
        }
        self.refresh_terminal_view_labels(cx);
        let tabs: Vec<_> = self.agent_sessions.bindings.keys().copied().collect();
        for tab in tabs {
            if self
                .ui
                .dock(cx, ids!(dock))
                .item(LiveId(tab))
                .button(cx, ids!(connect_terminal))
                .clicked(actions)
            {
                self.open_terminal_connections(cx, tab);
            }
        }
    }
}
