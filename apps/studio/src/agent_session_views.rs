// A connection is a view of a session, never a transfer of its flow ownership.
impl App {
    fn refresh_terminal_inventory(&mut self) -> Result<(), String> {
        if self.agent_sessions.inventory_request.is_none() {
            let request = self
                .agent_sessions
                .worker
                .as_mut()
                .ok_or("Terminal host unavailable")?
                .list()?;
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
                    binding.open
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
        let labels = self
            .iterations
            .snapshot
            .engine
            .flows
            .keys()
            .filter_map(|flow| {
                self.agent_sessions
                    .mirrors
                    .get(&self.flow_terminal_id(flow))
                    .map(|info| {
                        (
                            flow.clone(),
                            format!(
                                "Viewing {}",
                                self.session_owner_flow(&info.session_id)
                                    .unwrap_or_else(|| info.session_id.clone())
                            ),
                        )
                    })
            })
            .collect();
        if let Some(mut view) = self
            .ui
            .widget(cx, ids!(flow_scene))
            .borrow_mut::<StudioIterationView>()
        {
            view.set_connection_labels(cx, labels);
        }
    }

    fn terminal_owner_for_tab(&self, tab: u64) -> Option<String> {
        let binding = self.agent_sessions.bindings.get(&tab)?;
        let session = self
            .agent_sessions
            .mirrors
            .get(&tab)
            .map(|info| info.session_id.as_str())
            .unwrap_or(&binding.session_id);
        self.session_owner_flow(session)
    }

    fn terminal_input_flow(&self, flow: &str) -> Result<String, String> {
        match self
            .agent_sessions
            .mirrors
            .get(&self.flow_terminal_id(flow))
        {
            Some(info) => self.session_owner_flow(&info.session_id).ok_or_else(|| {
                "This shared terminal has no Studio flow owner for image feedback".into()
            }),
            None => Ok(flow.to_owned()),
        }
    }

    fn open_terminal_connections(&mut self, cx: &mut Cx, tab: u64) {
        self.agent_sessions.picker_tab = Some(tab);
        self.show_utility(
            cx,
            id!(terminal_connections_tab),
            id!(TerminalConnectionsTab),
            "Connect terminal",
        );
        self.update_terminal_connections(cx);
        if let Err(error) = self.refresh_terminal_inventory() {
            self.ui
                .label(cx, ids!(terminal_connection_note))
                .set_text(cx, &error);
        }
    }

    fn update_terminal_connections(&mut self, cx: &mut Cx) {
        let Some(tab) = self.agent_sessions.picker_tab else {
            return;
        };
        let own = self
            .agent_sessions
            .bindings
            .get(&tab)
            .map(|binding| binding.session_id.as_str());
        let selected = self
            .agent_sessions
            .mirrors
            .get(&tab)
            .map(|info| info.session_id.as_str());
        let mut labels = vec!["This lane’s own terminal".to_string()];
        let mut ids = vec![None];
        let mut index = 0;
        for info in &self.agent_sessions.inventory {
            if Some(info.session_id.as_str()) == own {
                continue;
            }
            if Some(info.session_id.as_str()) == selected {
                index = ids.len();
            }
            let owner = self
                .session_owner_flow(&info.session_id)
                .unwrap_or_else(|| info.session_id.clone());
            labels.push(format!(
                "{} · {} · {} connected · {}",
                owner,
                info.provider.as_str(),
                info.clients,
                info.cwd.display()
            ));
            ids.push(Some(info.session_id.clone()));
        }
        self.agent_sessions.picker_ids = ids;
        let picker = self.ui.drop_down(cx, ids!(terminal_connection_picker));
        picker.set_labels(cx, labels);
        picker.set_selected_item(cx, index);
        self.ui.label(cx, ids!(terminal_connection_note)).set_text(cx,
            "Connect another view to a running Studio PTY. Other views stay connected. Agent updates go to the session’s original lane; the latest resize sets its size.");
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
        let request = self
            .agent_sessions
            .worker
            .as_mut()
            .ok_or("Terminal host unavailable")?
            .select_view(tab, session)?;
        self.agent_sessions.view_requests.insert(request, tab);
        Ok(())
    }

    fn attach_shared_terminal_view(
        &mut self,
        cx: &mut Cx,
        tab: u64,
        info: makepad_studio::agent_session::SessionInfo,
    ) {
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
                .find(|info| info.session_id == session)
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
        reply: &makepad_studio::agent_session::SessionReply,
    ) -> bool {
        use makepad_studio::agent_session::SessionOutcome;
        if self.agent_sessions.inventory_request == Some(reply.request_id) {
            self.agent_sessions.inventory_request = None;
            match &reply.result {
                Ok(SessionOutcome::Inventory { sessions, views }) => {
                    self.agent_sessions.inventory = sessions.clone();
                    if !self.agent_sessions.views_loaded {
                        self.agent_sessions.views_loaded = true;
                        self.agent_sessions.saved_views = views.iter().cloned().collect();
                    }
                    self.restore_terminal_views(cx);
                    self.update_terminal_connections(cx);
                }
                Err(error) => {
                    self.ui
                        .label(cx, ids!(terminal_connection_note))
                        .set_text(cx, error);
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
                self.close_utility(cx);
            }
            Err(error) => {
                self.ui
                    .label(cx, ids!(terminal_connection_note))
                    .set_text(cx, error);
                self.ui.label(cx, ids!(status_state)).set_text(cx, error);
            }
            _ => {}
        }
        true
    }

    fn handle_terminal_connection_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self
            .ui
            .button(cx, ids!(terminal_connection_cancel))
            .clicked(actions)
        {
            self.close_utility(cx);
        }
        if self
            .ui
            .button(cx, ids!(terminal_connection_refresh))
            .clicked(actions)
        {
            if let Err(error) = self.refresh_terminal_inventory() {
                self.ui
                    .label(cx, ids!(terminal_connection_note))
                    .set_text(cx, &error);
            }
        }
        if self
            .ui
            .button(cx, ids!(terminal_connection_apply))
            .clicked(actions)
        {
            if let Some(tab) = self.agent_sessions.picker_tab {
                let index = self
                    .ui
                    .drop_down(cx, ids!(terminal_connection_picker))
                    .selected_item();
                if let Some(session) = self.agent_sessions.picker_ids.get(index).cloned() {
                    if let Err(error) = self.select_terminal_view(tab, session) {
                        self.ui
                            .label(cx, ids!(terminal_connection_note))
                            .set_text(cx, &error);
                    }
                }
            }
        }
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
