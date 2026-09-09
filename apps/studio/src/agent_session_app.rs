// Native terminal attachments are presentation state. Screen owns the agent's
// PTY across Studio restarts; only the explicit Stop action ends that session.

#[derive(Default)]
struct AgentSessionAppState {
    worker: Option<makepad_studio::agent_session::AgentSessionWorker>,
    started: bool,
    error: Option<String>,
    bindings: HashMap<u64, AgentTerminalBinding>,
    requests: HashMap<u64, (u64, AgentTerminalRequest)>,
    next_resume_probe: f64,
    terminal_busy: HashMap<u64, bool>,
    terminal_reservations: HashMap<String, (u64, AgentTerminalRequest)>,
    inventory: Vec<makepad_studio::agent_session::SessionInfo>,
    inventory_request: Option<u64>,
    inventory_state_dir: Option<PathBuf>,
    view_requests: HashMap<u64, u64>,
    mirrors: HashMap<u64, makepad_studio::agent_session::SessionInfo>,
    saved_views: HashMap<u64, String>,
    views_loaded: bool,
    next_inventory_probe: f64,
    reported_titles: HashMap<String, String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AgentTerminalRequest {
    Prepare,
    Attach,
    InspectExit,
    Stop,
    Restore,
    Refresh,
    Recover,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AgentTerminalPhase {
    Connecting,
    Attached,
    Detached,
    Ended,
    Stopping,
    Unavailable,
}

impl AgentTerminalPhase {
    fn as_str(self) -> &'static str {
        match self {
            Self::Connecting => "connecting",
            Self::Attached => "attached",
            Self::Detached => "detached",
            Self::Ended => "ended",
            Self::Stopping => "stopping",
            Self::Unavailable => "unavailable",
        }
    }
}

struct AgentTerminalBinding {
    session_id: String,
    cwd: PathBuf,
    initial_command: Option<String>,
    title: String,
    /// Changes only if Dock replaces the actual widget, not on tab switches.
    view_uid: Option<WidgetUid>,
    open: bool,
    phase: AgentTerminalPhase,
    info: Option<makepad_studio::agent_session::SessionInfo>,
    error: Option<String>,
    reconnect_attempts: u8,
    provider: makepad_studio::agent_session::AgentProvider,
    resume: Option<makepad_studio::agent_session::ResumeIdentity>,
    recovery: Option<makepad_studio::agent_session::RecoveryInfo>,
    /// User-supplied conversation id for the first Prepare of this tab.
    resume_conversation: Option<String>,
}

impl App {
    fn start_agent_sessions(&mut self, cx: &mut Cx) {
        if self.agent_sessions.started {
            return;
        }
        self.agent_sessions.started = true;
        match makepad_studio::agent_session::AgentSessionWorker::start(
            &cx.thread_spawner(),
            self.state_dir(),
        ) {
            Ok(worker) => {
                self.agent_sessions.worker = Some(worker);
                let _ = self.refresh_terminal_inventory();
            }
            Err(error) => {
                log!("studio agent sessions: {error}");
                self.agent_sessions.error = Some(error);
            }
        }
    }

    /// Must run before a newly created/restored terminal gets its first draw:
    /// MpTerm's default local shell is unloaded before it can spawn.
    fn bind_agent_terminals(&mut self, cx: &mut Cx) {
        let dock = self.ui.dock(cx, ids!(dock));
        if !self.iterations.snapshot_ready {
            // Restored Dock widgets do not yet know their repository or
            // provider. Hold them unloaded until the worker's first snapshot
            // lets bind_flow_terminals configure that identity first.
            for (id, item) in dock.clone_state().unwrap_or_default() {
                if let DockItem::Tab { kind, .. } = item {
                    if kind == id!(TerminalTab) {
                        if let Some(body) = dock.item_or_create(cx, id, kind) {
                            if let Some(mut term) =
                                body.widget(cx, ids!(term)).borrow_mut::<MpTerm>()
                            {
                                term.unload(cx);
                            }
                        }
                    }
                }
            }
            return;
        }
        self.start_agent_sessions(cx);
        let mut tabs: Vec<_> = dock
            .clone_state()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(id, item)| match item {
                DockItem::Tab { kind, name, .. } if kind == id!(TerminalTab) => {
                    Some((id, kind, name))
                }
                _ => None,
            })
            .collect();
        tabs.sort_by_key(|(id, _, _)| id.0);
        let open: std::collections::HashSet<_> = tabs.iter().map(|(id, _, _)| id.0).collect();
        for (tab, binding) in &mut self.agent_sessions.bindings {
            if !open.contains(tab) && binding.open {
                binding.open = false;
                binding.view_uid = None;
                if binding.phase == AgentTerminalPhase::Attached {
                    binding.phase = AgentTerminalPhase::Detached;
                }
                // No Stop command: closing a tab only drops the attach client.
            }
        }
        for (id, kind, title) in tabs {
            let Some(body) = dock.item_or_create(cx, id, kind) else {
                continue;
            };
            let terminal = body.widget(cx, ids!(term));
            let uid = terminal.widget_uid();
            if self
                .agent_sessions
                .bindings
                .get(&id.0)
                .is_some_and(|binding| binding.view_uid == Some(uid))
            {
                continue;
            }
            let Some(mut term) = terminal.borrow_mut::<MpTerm>() else {
                continue;
            };
            let cwd = term.cwd.clone().unwrap_or_else(|| self.project_dir());
            let initial_command = term.command.clone();
            term.unload(cx);
            drop(term);

            let existing = self.agent_sessions.bindings.contains_key(&id.0);
            if let Some(binding) = self.agent_sessions.bindings.get_mut(&id.0) {
                binding.open = true;
                binding.view_uid = Some(uid);
                binding.reconnect_attempts = 0;
            } else {
                let provider = self.provider_for_terminal(id.0);
                let resume_conversation = self.resume_conversation_for_terminal(id.0);
                self.agent_sessions.bindings.insert(
                    id.0,
                    AgentTerminalBinding {
                        session_id: format!("term-{:016x}", id.0),
                        cwd,
                        initial_command,
                        // A persisted Dock title can include our transient
                        // suffix when Studio closed mid-operation.
                        title: [
                            " · connecting",
                            " · detached",
                            " · ended",
                            " · stopping",
                            " · unavailable",
                        ]
                        .iter()
                        .find_map(|suffix| title.strip_suffix(*suffix))
                        .unwrap_or(&title)
                        .to_string(),
                        view_uid: Some(uid),
                        open: true,
                        phase: AgentTerminalPhase::Connecting,
                        info: None,
                        error: None,
                        reconnect_attempts: 0,
                        provider,
                        resume: None,
                        recovery: None,
                        resume_conversation,
                    },
                );
            }
            if let Some(info) = self.agent_sessions.mirrors.get(&id.0).cloned() {
                self.attach_shared_terminal_view(cx, id.0, info);
                continue;
            }
            let pending = self
                .agent_sessions
                .requests
                .values()
                .any(|(tab, _)| *tab == id.0);
            let ended = self
                .agent_sessions
                .bindings
                .get(&id.0)
                .is_some_and(|binding| {
                    matches!(
                        binding.phase,
                        AgentTerminalPhase::Ended | AgentTerminalPhase::Stopping
                    )
                });
            if !pending && !ended {
                let kind = if existing {
                    AgentTerminalRequest::Attach
                } else {
                    AgentTerminalRequest::Prepare
                };
                if let Err(error) = self.queue_agent_terminal(id.0, kind) {
                    self.set_agent_terminal_error(id.0, error);
                }
            }
            self.refresh_agent_terminal_status(cx, id.0);
        }
    }

    fn queue_agent_terminal(
        &mut self,
        tab: u64,
        kind: AgentTerminalRequest,
    ) -> Result<u64, String> {
        if (self.agent_sessions.mirrors.contains_key(&tab)
            || self
                .agent_sessions
                .view_requests
                .values()
                .any(|pending| *pending == tab))
            && matches!(
                kind,
                AgentTerminalRequest::Stop
                    | AgentTerminalRequest::Restore
                    | AgentTerminalRequest::Recover
            )
        {
            return Err("Reconnect this lane’s own terminal before stopping, resuming or signing in. The displayed session belongs to another terminal.".into());
        }
        if self
            .agent_sessions
            .terminal_reservations
            .values()
            .any(|(id, _)| *id == tab)
        {
            return Err("An agent session operation is already pending for this tab".into());
        }
        if matches!(
            kind,
            AgentTerminalRequest::Stop
                | AgentTerminalRequest::Restore
                | AgentTerminalRequest::Recover
        ) {
            if self
                .agent_sessions
                .requests
                .values()
                .any(|(id, _)| *id == tab)
            {
                return Err("An agent session operation is already pending for this tab".into());
            }
            if let Some(flow) = self
                .iterations
                .snapshot
                .engine
                .flows
                .values()
                .find(|flow| flow.successor.is_none() && self.flow_terminal_id(&flow.id) == tab)
                .map(|flow| flow.id.clone())
            {
                self.iterations.sequence += 1;
                let id = format!("terminal-busy:{}", self.iterations.sequence);
                self.iterations
                    .worker
                    .as_ref()
                    .ok_or("Iteration host unavailable")?
                    .submit(
                        id.clone(),
                        IterationRequest::TerminalBusy { flow, busy: true },
                    )?;
                self.agent_sessions.terminal_busy.insert(tab, true);
                self.agent_sessions
                    .terminal_reservations
                    .insert(id, (tab, kind));
                // The process operation starts only after the iteration worker
                // acknowledges the reservation against callback-driven splits.
                if let Some(binding) = self.agent_sessions.bindings.get_mut(&tab) {
                    binding.phase = if kind == AgentTerminalRequest::Stop {
                        AgentTerminalPhase::Stopping
                    } else {
                        AgentTerminalPhase::Connecting
                    };
                    binding.error = None;
                }
                return Ok(self.iterations.sequence);
            }
        }
        self.queue_agent_terminal_reserved(tab, kind)
    }

    fn queue_agent_terminal_reserved(
        &mut self,
        tab: u64,
        kind: AgentTerminalRequest,
    ) -> Result<u64, String> {
        let restore_environment = if matches!(
            kind,
            AgentTerminalRequest::Restore | AgentTerminalRequest::Recover
        ) {
            self.iterations.snapshot.engine.flows.values().find(|flow|
                flow.successor.is_none() && self.flow_terminal_id(&flow.id) == tab)
                .map(|flow| {
                    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
                    let cli = executable.with_file_name(if cfg!(windows) { "studio-flow.exe" } else { "studio-flow" });
                    let control = self.state_dir().join("iterations").join("control").join(&flow.id);
                    let quote = |value: &str| format!("'{}'", value.replace('\'', "'\\''"));
                    Ok::<_, String>(format!("export MAKEPAD_STUDIO_FLOW_ID={}; export MAKEPAD_STUDIO_CONTROL_DIR={}; export MAKEPAD_STUDIO_CLI={}; exec \"${{SHELL:-/bin/sh}}\" -l",
                        quote(&flow.id), quote(&control.to_string_lossy()), quote(&cli.to_string_lossy())))
                }).transpose()?
        } else {
            None
        };
        if self
            .agent_sessions
            .requests
            .values()
            .any(|(id, _)| *id == tab)
        {
            return Err("An agent session operation is already pending for this tab".into());
        }
        let binding = self
            .agent_sessions
            .bindings
            .get(&tab)
            .ok_or("Unknown agent terminal tab")?;
        let session_id = binding.session_id.clone();
        let spec = makepad_studio::agent_session::SessionSpec {
            session_id: session_id.clone(),
            cwd: binding.cwd.clone(),
            command: binding.initial_command.clone(),
            resume_conversation: binding.resume_conversation.clone(),
        };
        let worker = self.agent_sessions.worker.as_mut().ok_or_else(|| {
            self.agent_sessions.error.clone().unwrap_or_else(|| {
                "Persistent agent sessions are unavailable; a temporary shell was not started"
                    .into()
            })
        })?;
        let request = match kind {
            AgentTerminalRequest::Prepare => {
                if binding.provider == makepad_studio::agent_session::AgentProvider::Shell {
                    worker.prepare(spec)
                } else {
                    worker.prepare_provider(spec, binding.provider)
                }
            }
            AgentTerminalRequest::Attach => worker.attach(session_id),
            AgentTerminalRequest::InspectExit => worker.inspect(session_id),
            AgentTerminalRequest::Stop => worker.stop(session_id),
            AgentTerminalRequest::Restore => {
                if let Some(command) = restore_environment {
                    worker.restore_with_environment(session_id, command, binding.cwd.clone())
                } else {
                    worker.restore(session_id)
                }
            }
            AgentTerminalRequest::Refresh => worker.inspect(session_id),
            AgentTerminalRequest::Recover => worker.recover_with_environment(
                session_id,
                restore_environment.ok_or("Account recovery requires a known Studio flow")?,
                binding.cwd.clone(),
            ),
        }?;
        self.agent_sessions.requests.insert(request, (tab, kind));
        if let Some(binding) = self.agent_sessions.bindings.get_mut(&tab) {
            if kind != AgentTerminalRequest::Refresh {
                binding.phase = if kind == AgentTerminalRequest::Stop {
                    AgentTerminalPhase::Stopping
                } else {
                    AgentTerminalPhase::Connecting
                };
            }
            binding.error = None;
            if kind == AgentTerminalRequest::Recover {
                binding.recovery = Some(makepad_studio::agent_session::RecoveryInfo {
                    provider: binding.provider,
                    phase: makepad_studio::agent_session::RecoveryPhase::PreservingIdentity,
                    conversation_id: binding
                        .resume
                        .as_ref()
                        .map(|identity| identity.conversation_id.clone())
                        .unwrap_or_default(),
                    message: "Verifying the root conversation before account recovery…".into(),
                    started_at_ms: 0,
                });
            }
        }
        Ok(request)
    }

    fn set_flow_terminal_busy(&mut self, tab: u64, busy: bool) -> Result<(), String> {
        if self
            .agent_sessions
            .terminal_busy
            .get(&tab)
            .copied()
            .unwrap_or(false)
            == busy
        {
            return Ok(());
        }
        let Some(flow) = self
            .iterations
            .snapshot
            .engine
            .flows
            .values()
            .find(|flow| flow.successor.is_none() && self.flow_terminal_id(&flow.id) == tab)
            .map(|flow| flow.id.clone())
        else {
            return Ok(());
        };
        self.iterations.sequence += 1;
        self.iterations
            .worker
            .as_ref()
            .ok_or("Iteration host unavailable")?
            .submit(
                format!("terminal-busy:{}", self.iterations.sequence),
                IterationRequest::TerminalBusy { flow, busy },
            )?;
        self.agent_sessions.terminal_busy.insert(tab, busy);
        Ok(())
    }

    fn sync_terminal_busy(&mut self) {
        let tabs: std::collections::HashSet<_> = self
            .agent_sessions
            .terminal_busy
            .keys()
            .chain(self.agent_sessions.bindings.keys())
            .copied()
            .collect();
        for tab in tabs {
            let busy = self.agent_sessions.requests.values().any(|(id, request)| {
                *id == tab
                    && matches!(
                        request,
                        AgentTerminalRequest::Stop
                            | AgentTerminalRequest::Restore
                            | AgentTerminalRequest::Recover
                    )
            }) || self
                .agent_sessions
                .bindings
                .get(&tab)
                .is_some_and(|binding| {
                    binding
                        .recovery
                        .as_ref()
                        .is_some_and(|recovery| recovery.phase.active())
                })
                || self
                    .agent_sessions
                    .terminal_reservations
                    .values()
                    .any(|(id, _)| *id == tab)
                || self
                    .iterations
                    .pending_lifecycles
                    .keys()
                    .any(|flow| self.flow_terminal_id(flow) == tab);
            // A full queue leaves the old reservation in place and retries on
            // the next UI event; it can never accidentally admit a split.
            if let Err(error) = self.set_flow_terminal_busy(tab, busy) {
                log!("studio terminal handoff reservation: {error}");
            }
        }
    }

    fn set_agent_terminal_error(&mut self, tab: u64, error: String) {
        log!("studio agent terminal {tab:x}: {error}");
        if let Some(binding) = self.agent_sessions.bindings.get_mut(&tab) {
            binding.phase = AgentTerminalPhase::Unavailable;
            binding.error = Some(error);
        }
    }

    fn refresh_agent_terminal_status(&self, cx: &mut Cx, tab: u64) {
        let Some(binding) = self.agent_sessions.bindings.get(&tab) else {
            return;
        };
        if !binding.open {
            return;
        }
        let text = self.agent_terminal_status(tab).unwrap_or_default();
        let dock = self.ui.dock(cx, ids!(dock));
        dock.item(LiveId(tab))
            .label(cx, ids!(terminal_session_status))
            .set_text(cx, &text);
        let title = match binding.phase {
            AgentTerminalPhase::Attached => binding.title.clone(),
            AgentTerminalPhase::Connecting => format!("{} · connecting", binding.title),
            AgentTerminalPhase::Detached => format!("{} · detached", binding.title),
            AgentTerminalPhase::Ended => format!("{} · ended", binding.title),
            AgentTerminalPhase::Stopping => format!("{} · stopping", binding.title),
            AgentTerminalPhase::Unavailable => format!("{} · unavailable", binding.title),
        };
        dock.set_tab_title(cx, LiveId(tab), title);
        dock.redraw_tab(cx, LiveId(tab));
    }

    fn agent_terminal_status(&self, tab: u64) -> Option<String> {
        if let Some(info) = self.agent_sessions.mirrors.get(&tab) {
            return Some(format!(
                "Connected to {} · commands report to {} · closing this view detaches",
                info.title,
                self.info_owner_flow(info)
                    .unwrap_or_else(|| "its originating terminal".into())
            ));
        }
        let binding = self.agent_sessions.bindings.get(&tab)?;
        let text = if let Some(error) = &binding.error {
            format!("Agent session unavailable: {error}")
        } else if let Some(recovery) = &binding.recovery {
            recovery.message.clone()
        } else {
            match binding.phase {
                AgentTerminalPhase::Connecting => {
                    "Connecting to the persistent agent session…".into()
                }
                AgentTerminalPhase::Attached => format!(
                    "{} · PID {} · {} · closing Studio detaches",
                    binding.provider.as_str(),
                    binding
                        .info
                        .as_ref()
                        .map(|info| info.supervisor_pid)
                        .unwrap_or(0),
                    binding
                        .info
                        .as_ref()
                        .and_then(|info| info.resume_error.as_deref())
                        .map(|error| format!("resume identity pending: {error}"))
                        .unwrap_or_else(|| binding
                            .resume
                            .as_ref()
                            .map(|resume| format!("resume {}", resume.conversation_id))
                            .unwrap_or_else(|| "persistent terminal".into()))
                ),
                AgentTerminalPhase::Detached => {
                    "Agent remains running without a terminal attachment".into()
                }
                AgentTerminalPhase::Ended => binding
                    .resume
                    .as_ref()
                    .map(|resume| {
                        format!(
                            "{} stopped · resume {} is saved",
                            resume.provider.as_str(),
                            resume.conversation_id
                        )
                    })
                    .unwrap_or_else(|| {
                        "Terminal ended · explicit Restore checks its saved conversation".into()
                    }),
                AgentTerminalPhase::Stopping => "Stopping this agent session…".into(),
                AgentTerminalPhase::Unavailable => {
                    "Persistent agent session is unavailable; no replacement shell was started"
                        .into()
                }
            }
        };
        Some(text)
    }

    fn attach_agent_terminal_view(
        &mut self,
        cx: &mut Cx,
        tab: u64,
        info: makepad_studio::agent_session::SessionInfo,
    ) {
        let dock = self.ui.dock(cx, ids!(dock));
        let body = dock.item(LiveId(tab));
        let terminal = body.widget(cx, ids!(term));
        let Some(binding) = self.agent_sessions.bindings.get_mut(&tab) else {
            return;
        };
        binding.info = Some(info.clone());
        binding.provider = info.provider;
        if info.resume.is_some() {
            binding.resume = info.resume.clone();
        }
        if info.recovery.is_some() {
            binding.recovery = info.recovery.clone();
        }
        binding.initial_command = None;
        binding.error = None;
        if self.agent_sessions.mirrors.contains_key(&tab) {
            return;
        }
        if binding.open && binding.view_uid == Some(terminal.widget_uid()) {
            if let Some(mut term) = terminal.borrow_mut::<MpTerm>() {
                term.restart_with(cx, Some(binding.cwd.clone()), Some(info.attach_command));
                binding.phase = AgentTerminalPhase::Attached;
            } else {
                binding.phase = AgentTerminalPhase::Detached;
            }
        } else {
            // The user closed the tab while preparation was in flight. Keep
            // its new durable session detached; do not recreate a UI tab.
            binding.phase = AgentTerminalPhase::Detached;
        }
    }

    fn drain_agent_sessions(&mut self, cx: &mut Cx) {
        self.sync_terminal_busy();
        if cx.seconds_since_app_start() >= self.agent_sessions.next_inventory_probe {
            // Inventory invokes the helper. Explicit connect/refresh actions
            // remain immediate; background discovery needs no one-second poll.
            self.agent_sessions.next_inventory_probe = cx.seconds_since_app_start() + 30.0;
            let _ = self.refresh_terminal_inventory();
        }
        self.restore_terminal_views(cx);
        let now = cx.seconds_since_app_start();
        if now >= self.agent_sessions.next_resume_probe {
            self.agent_sessions.next_resume_probe = now
                + if self.agent_sessions.bindings.values().any(|binding| {
                    binding
                        .recovery
                        .as_ref()
                        .is_some_and(|recovery| recovery.phase.active())
                }) {
                    1.0
                } else {
                    10.0
                };
            let tabs: Vec<_> = self
                .agent_sessions
                .bindings
                .iter()
                .filter(|(_, binding)| {
                    matches!(
                        binding.phase,
                        AgentTerminalPhase::Attached | AgentTerminalPhase::Detached
                    )
                })
                .map(|(tab, _)| *tab)
                .collect();
            for tab in tabs {
                let _ = self.queue_agent_terminal(tab, AgentTerminalRequest::Refresh);
            }
        }
        let replies = self
            .agent_sessions
            .worker
            .as_mut()
            .map(|worker| worker.poll())
            .unwrap_or_default();
        if replies.is_empty() {
            return;
        }
        for reply in replies {
            if self.handle_terminal_view_reply(cx, &reply) {
                continue;
            }
            let Some((tab, request)) = self.agent_sessions.requests.remove(&reply.request_id)
            else {
                continue;
            };
            if !self
                .agent_sessions
                .bindings
                .get(&tab)
                .is_some_and(|binding| binding.session_id == reply.session_id)
            {
                continue;
            }
            match reply.result {
                Ok(
                    makepad_studio::agent_session::SessionOutcome::Inventory { .. }
                    | makepad_studio::agent_session::SessionOutcome::ViewReady { .. },
                ) => {}
                Ok(makepad_studio::agent_session::SessionOutcome::Ready(info)) => {
                    self.attach_agent_terminal_view(cx, tab, info);
                }
                Ok(makepad_studio::agent_session::SessionOutcome::Status(Some(info))) => {
                    let reconnect = request == AgentTerminalRequest::InspectExit
                        && self
                            .agent_sessions
                            .bindings
                            .get(&tab)
                            .is_some_and(|binding| binding.open && binding.reconnect_attempts < 1);
                    if reconnect {
                        if let Some(binding) = self.agent_sessions.bindings.get_mut(&tab) {
                            binding.reconnect_attempts += 1;
                        }
                        self.attach_agent_terminal_view(cx, tab, info);
                    } else if let Some(binding) = self.agent_sessions.bindings.get_mut(&tab) {
                        binding.provider = info.provider;
                        if info.resume.is_some() {
                            binding.resume = info.resume.clone();
                        }
                        if info.recovery.is_some() {
                            binding.recovery = info.recovery.clone();
                        }
                        binding.info = Some(info);
                        if request != AgentTerminalRequest::Refresh {
                            binding.phase = AgentTerminalPhase::Detached;
                        }
                        binding.error = None;
                    }
                }
                Ok(makepad_studio::agent_session::SessionOutcome::StoppedWithResume(resume)) => {
                    if let Some(binding) = self.agent_sessions.bindings.get_mut(&tab) {
                        binding.provider = resume.provider;
                        binding.resume = Some(resume);
                        binding.info = None;
                        binding.phase = AgentTerminalPhase::Ended;
                        binding.error = None;
                    }
                }
                Ok(makepad_studio::agent_session::SessionOutcome::ResumeCaptured(resume)) => {
                    if let Some(binding) = self.agent_sessions.bindings.get_mut(&tab) {
                        binding.provider = resume.provider;
                        binding.resume = Some(resume);
                    }
                }
                Ok(makepad_studio::agent_session::SessionOutcome::FableLoginReady(info)) => {
                    let result = self.submit_fable_login(cx, tab, &info);
                    if let Some(binding) = self.agent_sessions.bindings.get_mut(&tab) {
                        binding.provider = info.provider;
                        binding.resume = info.resume.clone();
                        binding.info = Some(info.clone());
                        binding.phase = if binding.open {
                            AgentTerminalPhase::Attached
                        } else {
                            AgentTerminalPhase::Detached
                        };
                        binding.recovery = Some(makepad_studio::agent_session::RecoveryInfo {
                            provider: info.provider,
                            phase: if result.is_ok() { makepad_studio::agent_session::RecoveryPhase::LoginRequested } else { makepad_studio::agent_session::RecoveryPhase::Failed },
                            conversation_id: info.resume.as_ref().map(|identity| identity.conversation_id.clone()).unwrap_or_default(),
                            message: result.as_ref().map(|_| "Fable /login submitted; finish sign-in in its existing terminal".to_string()).unwrap_or_else(|error| error.clone()),
                            started_at_ms: info.resume.as_ref().map(|identity| identity.verified_at_ms).unwrap_or_default(),
                        });
                        binding.error = result.err();
                    }
                }
                Ok(makepad_studio::agent_session::SessionOutcome::RecoveryStatus(
                    info,
                    recovery,
                )) => {
                    if let Some(info) = info {
                        self.attach_agent_terminal_view(cx, tab, info);
                    } else if let Some(binding) = self.agent_sessions.bindings.get_mut(&tab) {
                        binding.info = None;
                        binding.phase = AgentTerminalPhase::Ended;
                    }
                    if let Some(binding) = self.agent_sessions.bindings.get_mut(&tab) {
                        binding.recovery = Some(recovery);
                    }
                }
                Ok(makepad_studio::agent_session::SessionOutcome::Status(None))
                | Ok(makepad_studio::agent_session::SessionOutcome::Stopped) => {
                    if let Some(binding) = self.agent_sessions.bindings.get_mut(&tab) {
                        binding.info = None;
                        binding.phase = AgentTerminalPhase::Ended;
                        binding.error = None;
                    }
                }
                Err(error) => {
                    if matches!(
                        request,
                        AgentTerminalRequest::Stop | AgentTerminalRequest::Recover
                    ) {
                        if let Some(binding) = self.agent_sessions.bindings.get_mut(&tab) {
                            binding.phase = if binding.open {
                                AgentTerminalPhase::Attached
                            } else {
                                AgentTerminalPhase::Detached
                            };
                            binding.error = Some(error.clone());
                            if request == AgentTerminalRequest::Recover {
                                if let Some(recovery) = &mut binding.recovery {
                                    recovery.phase =
                                        makepad_studio::agent_session::RecoveryPhase::Failed;
                                    recovery.message = error.clone();
                                }
                            }
                        }
                        log!("studio stop agent refused: {error}");
                    } else {
                        self.set_agent_terminal_error(tab, error);
                    }
                }
            }
            self.refresh_agent_terminal_status(cx, tab);
        }
        self.sync_terminal_busy();
        self.refresh_workspace(cx);
        self.refresh_ai_context(cx);
    }

    /// Call before the ordinary MpTermAction::Exited close-tab branch. A
    /// persistent terminal's attachment exit is not proof the agent stopped.
    fn agent_terminal_exited(&mut self, cx: &mut Cx, tab: u64) -> bool {
        if self.agent_sessions.mirrors.contains_key(&tab) {
            self.ui.label(cx, ids!(status_state)).set_text(
                cx,
                "Shared terminal detached. Use Connect to reconnect; its process was not stopped.",
            );
            return true;
        }
        let Some(binding) = self.agent_sessions.bindings.get(&tab) else {
            return false;
        };
        if !binding.open || binding.phase != AgentTerminalPhase::Attached {
            return true;
        }
        if let Err(error) = self.queue_agent_terminal(tab, AgentTerminalRequest::InspectExit) {
            self.set_agent_terminal_error(tab, error);
        }
        self.refresh_agent_terminal_status(cx, tab);
        true
    }

    fn stop_agent_session(&mut self, cx: &mut Cx, tab: u64) -> Result<String, String> {
        let binding = self
            .agent_sessions
            .bindings
            .get(&tab)
            .ok_or("This tab has no persistent agent session")?;
        if binding.phase == AgentTerminalPhase::Ended {
            return Ok("Agent session has already ended".into());
        }
        let request = self.queue_agent_terminal(tab, AgentTerminalRequest::Stop)?;
        self.refresh_agent_terminal_status(cx, tab);
        self.refresh_ai_context(cx);
        Ok(format!("Stop agent queued for terminal {tab:x} (request {request}); inspect agent sessions for its acknowledgement"))
    }

    fn resume_conversation_for_terminal(&self, tab: u64) -> Option<String> {
        self.iterations
            .snapshot
            .engine
            .flows
            .values()
            .find(|flow| flow.successor.is_none() && self.flow_terminal_id(&flow.id) == tab)
            .and_then(|flow| flow.config.resume_token.clone())
    }

    fn provider_for_terminal(&self, tab: u64) -> makepad_studio::agent_session::AgentProvider {
        use makepad_studio::agent_session::AgentProvider;
        self.iterations
            .snapshot
            .engine
            .flows
            .values()
            .find(|flow| flow.successor.is_none() && self.flow_terminal_id(&flow.id) == tab)
            .and_then(|flow| flow.config.agent_provider.as_deref())
            .map(|provider| match provider {
                "claude" => AgentProvider::Fable,
                "codex" => AgentProvider::Codex,
                "grok" => AgentProvider::Grok,
                _ => AgentProvider::Unknown,
            })
            .unwrap_or(AgentProvider::Shell)
    }

    fn flow_terminal_id(&self, flow: &str) -> u64 {
        self.iterations
            .terminals
            .get(flow)
            .copied()
            .unwrap_or_else(|| {
                let origin = self
                    .iterations
                    .snapshot
                    .engine
                    .terminal_origin(flow)
                    .unwrap_or(flow);
                LiveId::from_str(&format!("studio-flow-terminal:{origin}")).0
            })
    }

    fn stop_flow_terminal(&mut self, cx: &mut Cx, flow: &str) -> Result<(), String> {
        let tab = self.flow_terminal_id(flow);
        if !self.agent_sessions.bindings.contains_key(&tab) {
            return Ok(());
        }
        self.stop_agent_session(cx, tab).map(|_| ())
    }

    fn flow_terminal_stopped(&self, flow: &str) -> bool {
        self.agent_sessions
            .bindings
            .get(&self.flow_terminal_id(flow))
            .is_none_or(|binding| binding.phase == AgentTerminalPhase::Ended)
    }

    fn flow_terminal_stop_error(&self, flow: &str) -> Option<String> {
        self.agent_sessions
            .bindings
            .get(&self.flow_terminal_id(flow))
            .and_then(|binding| binding.error.clone())
    }

    fn recover_flow_terminal(&mut self, cx: &mut Cx, flow: &str) -> Result<(), String> {
        if !self
            .iterations
            .snapshot
            .engine
            .flows
            .get(flow)
            .is_some_and(|lane| {
                lane.lifecycle == iteration::FlowLifecycle::Active && lane.successor.is_none()
            })
        {
            return Err("Account recovery requires the active lane that owns this terminal".into());
        }
        let tab = self.flow_terminal_id(flow);
        let binding = self
            .agent_sessions
            .bindings
            .get(&tab)
            .ok_or("This lane has no attached agent identity yet")?;
        if !matches!(
            binding.provider,
            makepad_studio::agent_session::AgentProvider::Codex
                | makepad_studio::agent_session::AgentProvider::Fable
        ) {
            return Err("Account recovery is available for Fable and Codex lanes".into());
        }
        self.queue_agent_terminal(tab, AgentTerminalRequest::Recover)?;
        self.refresh_agent_terminal_status(cx, tab);
        self.refresh_ai_context(cx);
        Ok(())
    }

    fn submit_fable_login(
        &self,
        cx: &mut Cx,
        tab: u64,
        info: &makepad_studio::agent_session::SessionInfo,
    ) -> Result<(), String> {
        if self.agent_sessions.mirrors.contains_key(&tab)
            || self
                .agent_sessions
                .view_requests
                .values()
                .any(|pending| *pending == tab)
        {
            return Err("Fable login requires this lane’s own terminal connection".into());
        }
        let binding = self
            .agent_sessions
            .bindings
            .get(&tab)
            .ok_or("Fable terminal is unavailable")?;
        if info.provider != makepad_studio::agent_session::AgentProvider::Fable
            || !binding.open
            || binding
                .info
                .as_ref()
                .is_none_or(|current| current.supervisor_pid != info.supervisor_pid)
        {
            return Err("Open this exact Fable terminal before requesting /login".into());
        }
        let terminal = self
            .ui
            .dock(cx, ids!(dock))
            .item(LiveId(tab))
            .widget(cx, ids!(term));
        let mut term = terminal
            .borrow_mut::<MpTerm>()
            .ok_or("Fable terminal is unavailable")?;
        let visible = term
            .ai_screen_rows(None)
            .ok_or("Fable terminal is not connected")?
            .0
            .len();
        let (rows, cursor_y, cursor_x) = term
            .ai_screen_rows(Some(visible))
            .ok_or("Fable terminal is not connected")?;
        let row = rows.get(cursor_y).map(|row| row.trim()).unwrap_or("");
        if cursor_x > 4 || !matches!(row, ">" | "❯" | "›") {
            return Err("Fable's input is not visibly empty. Preserve or submit its draft, return to an empty prompt, then retry /login".into());
        }
        if !term.ai_type_bytes(b"/login\r") {
            return Err("The Fable terminal did not accept /login".into());
        }
        terminal.redraw(cx);
        Ok(())
    }

    fn resume_flow_terminal(&mut self, cx: &mut Cx, flow: &str) -> Result<(), String> {
        if let Some(next) = self
            .iterations
            .snapshot
            .engine
            .flows
            .get(flow)
            .and_then(|lane| lane.successor.as_ref())
        {
            return Err(format!(
                "This archived history continues in {next}; resume that lane instead"
            ));
        }
        let tab = self.flow_terminal_id(flow);
        if !self.agent_sessions.bindings.contains_key(&tab) {
            let state = self
                .iterations
                .snapshot
                .engine
                .flows
                .get(flow)
                .ok_or("Unknown flow")?;
            let cwd = state.config.repo.clone();
            let provider = self.provider_for_terminal(tab);
            self.agent_sessions.bindings.insert(
                tab,
                AgentTerminalBinding {
                    session_id: format!("term-{tab:016x}"),
                    cwd,
                    initial_command: None,
                    title: state.title.clone(),
                    view_uid: None,
                    open: false,
                    phase: AgentTerminalPhase::Ended,
                    info: None,
                    error: None,
                    reconnect_attempts: 0,
                    provider,
                    resume: None,
                    recovery: None,
                    resume_conversation: None,
                },
            );
        }
        if self.ui.dock(cx, ids!(dock)).item(LiveId(tab)).is_empty() {
            self.iterations.terminals.remove(flow);
        }
        // The retained Ended binding prevents the normal first-draw path from
        // starting a new process while its presentation is reconstructed.
        self.bind_flow_terminals(cx);
        self.queue_agent_terminal(tab, AgentTerminalRequest::Restore)?;
        self.refresh_agent_terminal_status(cx, tab);
        self.refresh_ai_context(cx);
        Ok(())
    }

    fn flow_terminal_info(&self, flow: &str) -> Value {
        let Some(binding) = self
            .agent_sessions
            .bindings
            .get(&self.flow_terminal_id(flow))
        else {
            return json::obj(vec![
                ("status", json::s("not_started")),
                ("resume", Value::Null),
            ]);
        };
        json::obj(vec![
            ("session_id", json::s(&binding.session_id)),
            ("status", json::s(binding.phase.as_str())),
            ("provider", json::s(binding.provider.as_str())),
            (
                "transport_program",
                binding
                    .info
                    .as_ref()
                    .map(|info| json::s(&info.transport_program))
                    .unwrap_or(Value::Null),
            ),
            (
                "transport_version",
                binding
                    .info
                    .as_ref()
                    .map(|info| json::s(&info.transport_version))
                    .unwrap_or(Value::Null),
            ),
            (
                "transport_warning",
                binding
                    .info
                    .as_ref()
                    .and_then(|info| info.transport_warning.as_ref())
                    .map(json::s)
                    .unwrap_or(Value::Null),
            ),
            (
                "recovery",
                binding
                    .recovery
                    .as_ref()
                    .map(|recovery| {
                        json::obj(vec![
                            ("phase", json::s(recovery.phase.as_str())),
                            ("provider", json::s(recovery.provider.as_str())),
                            ("conversation_id", json::s(&recovery.conversation_id)),
                            ("message", json::s(&recovery.message)),
                            ("started_at_ms", Value::Int(recovery.started_at_ms as i64)),
                            (
                                "affects_shared_account",
                                Value::Bool(
                                    recovery.provider
                                        == makepad_studio::agent_session::AgentProvider::Codex,
                                ),
                            ),
                            ("quota_recovery_verified", Value::Bool(false)),
                        ])
                    })
                    .unwrap_or(Value::Null),
            ),
            (
                "resume",
                binding
                    .resume
                    .as_ref()
                    .map(|identity| {
                        json::obj(vec![
                            ("provider", json::s(identity.provider.as_str())),
                            ("conversation_id", json::s(&identity.conversation_id)),
                            ("cwd", json::s(&identity.cwd)),
                            ("evidence_path", json::s(&identity.evidence_path)),
                            ("verified_at_ms", Value::Int(identity.verified_at_ms as i64)),
                        ])
                    })
                    .unwrap_or(Value::Null),
            ),
            (
                "error",
                binding.error.as_ref().map(json::s).unwrap_or(Value::Null),
            ),
        ])
    }

    fn agent_pid_for_tab(&self, tab: u64) -> Option<u32> {
        if let Some(info) = self.agent_sessions.mirrors.get(&tab) {
            return Some(info.supervisor_pid);
        }
        let binding = self.agent_sessions.bindings.get(&tab)?;
        if !matches!(
            binding.phase,
            AgentTerminalPhase::Attached
                | AgentTerminalPhase::Detached
                | AgentTerminalPhase::Stopping
        ) {
            return None;
        }
        binding.info.as_ref().map(|info| info.supervisor_pid)
    }

    fn agent_sessions_json(&self) -> Value {
        let mut bindings: Vec<_> = self.agent_sessions.bindings.iter().collect();
        bindings.sort_by_key(|(tab, _)| **tab);
        json::obj(vec![
            ("ui_close", json::s("detach_only")),
            (
                "flow_terminals",
                Value::Obj(
                    self.iterations
                        .snapshot
                        .engine
                        .flows
                        .keys()
                        .map(|flow| (flow.clone(), self.flow_terminal_info(flow)))
                        .collect(),
                ),
            ),
            ("tab_close", json::s("detach_only")),
            ("explicit_stop_required", Value::Bool(true)),
            (
                "manager_error",
                self.agent_sessions
                    .error
                    .as_ref()
                    .map(json::s)
                    .unwrap_or(Value::Null),
            ),
            (
                "sessions",
                Value::Arr(
                    bindings
                        .into_iter()
                        .map(|(tab, binding)| {
                            json::obj(vec![
                                ("tab", json::s(format!("{tab:x}"))),
                                ("session_id", json::s(&binding.session_id)),
                                (
                                    "display_session",
                                    json::s(
                                        self.agent_sessions
                                            .mirrors
                                            .get(tab)
                                            .map(|info| info.session_id.as_str())
                                            .unwrap_or(&binding.session_id),
                                    ),
                                ),
                                (
                                    "shared_view",
                                    Value::Bool(self.agent_sessions.mirrors.contains_key(tab)),
                                ),
                                ("status", json::s(binding.phase.as_str())),
                                ("provider", json::s(binding.provider.as_str())),
                                (
                                    "transport_program",
                                    binding
                                        .info
                                        .as_ref()
                                        .map(|info| json::s(&info.transport_program))
                                        .unwrap_or(Value::Null),
                                ),
                                (
                                    "transport_version",
                                    binding
                                        .info
                                        .as_ref()
                                        .map(|info| json::s(&info.transport_version))
                                        .unwrap_or(Value::Null),
                                ),
                                (
                                    "transport_warning",
                                    binding
                                        .info
                                        .as_ref()
                                        .and_then(|info| info.transport_warning.as_ref())
                                        .map(json::s)
                                        .unwrap_or(Value::Null),
                                ),
                                (
                                    "resume_id",
                                    binding
                                        .resume
                                        .as_ref()
                                        .map(|identity| json::s(&identity.conversation_id))
                                        .unwrap_or(Value::Null),
                                ),
                                (
                                    "resume_evidence",
                                    binding
                                        .resume
                                        .as_ref()
                                        .map(|identity| json::s(&identity.evidence_path))
                                        .unwrap_or(Value::Null),
                                ),
                                (
                                    "resume_error",
                                    binding
                                        .info
                                        .as_ref()
                                        .and_then(|info| info.resume_error.as_ref())
                                        .map(json::s)
                                        .unwrap_or(Value::Null),
                                ),
                                ("tab_open", Value::Bool(binding.open)),
                                (
                                    "supervisor_pid",
                                    binding
                                        .info
                                        .as_ref()
                                        .map(|info| Value::Int(info.supervisor_pid as i64))
                                        .unwrap_or(Value::Null),
                                ),
                                ("cwd", json::s(binding.cwd.to_string_lossy())),
                                (
                                    "backend",
                                    binding
                                        .info
                                        .as_ref()
                                        .map(|info| json::s(info.backend))
                                        .unwrap_or(Value::Null),
                                ),
                                (
                                    "error",
                                    binding.error.as_ref().map(json::s).unwrap_or(Value::Null),
                                ),
                                (
                                    "pending",
                                    Value::Bool(
                                        self.agent_sessions
                                            .requests
                                            .values()
                                            .any(|(pending, _)| pending == tab),
                                    ),
                                ),
                            ])
                        })
                        .collect(),
                ),
            ),
        ])
    }
}
