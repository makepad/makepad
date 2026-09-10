use makepad_director::iteration_host::{HostCommand, HostEvent};
use makepad_director::iteration_host_view::{IterationRunView, IterationRunViewAction};
use makepad_widgets::makepad_platform::studio::{AppToStudio, StudioToApp};

#[derive(Default)]
struct IterationHostAppState {
    port: Option<u16>,
    views: HashMap<u64, HostedIterationView>,
    outgoing: VecDeque<HostCommand>,
    deferred: VecDeque<HostEvent>,
    worker_requests: VecDeque<IterationRequest>,
}
struct HostedIterationView {
    flow: String,
    widget: WidgetRef,
    window: Option<usize>,
}

impl App {
    fn send_to_iteration_app(&mut self, cx: &mut Cx, command: HostCommand) {
        if self.iteration_host.outgoing.len() >= 512 {
            self.flow_note(cx, "Hosted app input queue is full; input was not accepted");
            return;
        }
        self.iteration_host.outgoing.push_back(command);
        self.flush_iteration_host(cx);
    }
    fn flush_iteration_host(&mut self, cx: &mut Cx) {
        let Some(worker) = &self.iterations.worker else {
            return;
        };
        while let Some(command) = self.iteration_host.outgoing.pop_front() {
            match worker.host_send(command) {
                Ok(()) => {}
                Err(std::sync::mpsc::TrySendError::Full(command)) => {
                    self.iteration_host.outgoing.push_front(command);
                    self.flow_note(cx, "Hosted app input is queued; waiting for transport");
                    break;
                }
                Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                    self.flow_note(cx, "Hosted app transport stopped");
                    self.iteration_host.outgoing.clear();
                    break;
                }
            }
        }
        while let Some(request) = self.iteration_host.worker_requests.front().cloned() {
            if self
                .submit_iteration(cx, request, "host_lifecycle")
                .is_err()
            {
                break;
            }
            self.iteration_host.worker_requests.pop_front();
        }
    }
    fn sync_iteration_app_views(&mut self, cx: &mut Cx) {
        let embedded = self.iterations.snapshot.embedded.clone();
        let gone: Vec<_> = self
            .iteration_host
            .views
            .keys()
            .filter(|client| !embedded.iter().any(|run| run.client == **client))
            .copied()
            .collect();
        for client in gone {
            if let Some(hosted) = self.iteration_host.views.remove(&client) {
                if let Some(mut view) = self
                    .ui
                    .widget(cx, ids!(flow_scene))
                    .borrow_mut::<StudioIterationView>()
                {
                    view.clear_app(cx, &hosted.flow);
                }
                if let Some(mut run) = hosted.widget.borrow_mut::<IterationRunView>() {
                    run.clear_run_target(cx);
                }
                self.ui
                    .dock(cx, ids!(artifact_dock))
                    .close_tab(cx, LiveId(client));
            }
        }
        for run in embedded {
            if !self.iteration_host.views.contains_key(&run.client) {
                let dock = self.ui.dock(cx, ids!(artifact_dock));
                let Some(widget) = dock.create_and_select_tab(
                    cx,
                    id!(root),
                    LiveId(run.client),
                    id!(ArtifactTab),
                    format!("{} / {}", run.flow, run.artifact_id),
                    id!(CloseableTab),
                    None,
                ) else {
                    continue;
                };
                if let Some(mut view) = widget.borrow_mut::<IterationRunView>() {
                    view.set_target_size(Some(dvec2(780.0, 450.0)));
                    view.set_read_only(cx, false);
                    view.set_run_target(cx, run.client, 0, run.port);
                }
                self.iteration_host.views.insert(
                    run.client,
                    HostedIterationView {
                        flow: run.flow.clone(),
                        widget,
                        window: None,
                    },
                );
            }
            if let Some(hosted) = self.iteration_host.views.get(&run.client) {
                if let Some(mut view) = self
                    .ui
                    .widget(cx, ids!(flow_scene))
                    .borrow_mut::<StudioIterationView>()
                {
                    view.set_app(cx, run.flow, hosted.widget.clone());
                }
            }
        }
    }
    fn drain_iteration_host(&mut self, cx: &mut Cx) {
        let Some(worker) = &self.iterations.worker else {
            return;
        };
        let events = worker.poll_host();
        self.sync_iteration_app_views(cx);
        self.iteration_host.deferred.extend(events);
        let count = self.iteration_host.deferred.len().min(256);
        for _ in 0..count {
            let Some(event) = self.iteration_host.deferred.pop_front() else {
                break;
            };
            match event {
                HostEvent::Listening { port } => {
                    self.iteration_host.port = Some(port);
                    self.iteration_host
                        .worker_requests
                        .push_back(IterationRequest::HostPort { port });
                }
                HostEvent::Registered { client, .. } => self
                    .iteration_host
                    .worker_requests
                    .push_back(IterationRequest::HostRegistered { client }),
                HostEvent::Connected { client } => self.send_to_iteration_app(
                    cx,
                    HostCommand::messages(client, vec![StudioToApp::Tick]),
                ),
                HostEvent::FromApp { client, messages } => {
                    let Some(hosted) = self.iteration_host.views.get_mut(&client) else {
                        if self.iteration_host.deferred.len() < 256 {
                            self.iteration_host
                                .deferred
                                .push_back(HostEvent::FromApp { client, messages });
                        }
                        continue;
                    };
                    for message in messages.iter().cloned() {
                        match message {
                            AppToStudio::CreateWindow { window_id, .. } => {
                                if hosted.window.is_none() {
                                    hosted.window = Some(window_id);
                                    if let Some(mut view) =
                                        hosted.widget.borrow_mut::<IterationRunView>()
                                    {
                                        view.app_ready(cx, client, window_id);
                                    }
                                }
                            }
                            AppToStudio::DrawCompleteAndFlip(frame) => {
                                if let Some(mut view) =
                                    hosted.widget.borrow_mut::<IterationRunView>()
                                {
                                    view.set_presentable_draw(cx, frame);
                                }
                            }
                            AppToStudio::SetCursor(cursor) => {
                                if let Some(mut view) =
                                    hosted.widget.borrow_mut::<IterationRunView>()
                                {
                                    view.set_remote_cursor(cx, cursor.into());
                                }
                            }
                            AppToStudio::SetClipboard(text) => cx.copy_to_clipboard(&text),
                            AppToStudio::LogItem(item) => {
                                log!("studio app {client}: {}", item.message)
                            }
                            _ => {}
                        }
                    }
                }
                HostEvent::Disconnected { client, reason } => {
                    if let Some(hosted) = self.iteration_host.views.get(&client) {
                        if let Some(mut view) = hosted.widget.borrow_mut::<IterationRunView>() {
                            view.set_status_line(cx, &format!("Transport disconnected: {reason}"));
                        }
                    }
                }
                HostEvent::Error(error) => self.flow_note(cx, &error),
            }
        }
        self.flush_iteration_host(cx);
    }
    fn handle_iteration_host_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for action in actions {
            let Some(action) = action.as_widget_action() else {
                continue;
            };
            match action.cast::<IterationRunViewAction>() {
                IterationRunViewAction::ForwardToApp { client, msg_bin } => self
                    .send_to_iteration_app(
                        cx,
                        HostCommand::Send {
                            client,
                            data: Arc::new(msg_bin),
                        },
                    ),
                IterationRunViewAction::Clicked { client } => {
                    if let Some(hosted) = self.iteration_host.views.get(&client) {
                        self.iterations.selected = Some(hosted.flow.clone());
                    }
                }
                IterationRunViewAction::None => {}
            }
        }
    }
}
