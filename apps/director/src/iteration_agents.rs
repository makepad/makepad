// Included by iteration_worker.rs. A delegate launch records the durable
// graph edge and the child's task first; the child's callback binding is
// published before its provider is started by the UI-driven session path.
// Repository paths are canonicalized here, at the worker boundary, so path
// aliases cannot evade the own-source policy.

/// Presentation state of the agent tree and its levels, owned by the worker
/// so the UI persists it without blocking I/O.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TreePresentation {
    /// Selected level: None is the synthetic Director root.
    pub level: Option<String>,
    pub expanded: BTreeSet<String>,
    /// Sidebar width in points; 0 means the default.
    pub width: f64,
    /// Per-level canvas camera as (pan_x, zoom), keyed by level id ("" = root).
    pub cameras: BTreeMap<String, (f64, f64)>,
}

impl TreePresentation {
    pub fn json(&self) -> Value {
        json::obj(vec![
            ("level", self.level.as_deref().map(s).unwrap_or(Value::Null)),
            (
                "expanded",
                Value::Arr(self.expanded.iter().map(s).collect()),
            ),
            ("width", Value::F64(self.width)),
            (
                "cameras",
                Value::Obj(
                    self.cameras
                        .iter()
                        .map(|(level, (pan, zoom))| {
                            (
                                level.clone(),
                                Value::Arr(vec![Value::F64(*pan), Value::F64(*zoom)]),
                            )
                        })
                        .collect(),
                ),
            ),
        ])
    }

    pub fn parse(value: &Value) -> Result<Self, String> {
        let number = |value: &Value| match value {
            Value::F64(value) => Some(*value),
            Value::Int(value) => Some(*value as f64),
            _ => None,
        };
        let mut tree = Self::default();
        if let Some(level) = value.get("level").and_then(Value::as_str) {
            if !cli_identifier(level) {
                return Err("Invalid tree level".into());
            }
            tree.level = Some(level.to_owned());
        }
        if let Some(Value::Arr(expanded)) = value.get("expanded") {
            for id in expanded.iter().take(iteration::MAX_AGENT_NODES) {
                if let Some(id) = id.as_str().filter(|id| cli_identifier(id)) {
                    tree.expanded.insert(id.to_owned());
                }
            }
        }
        if let Some(width) = value.get("width").and_then(number) {
            if width.is_finite() {
                tree.width = width.clamp(0.0, 800.0);
            }
        }
        if let Some(Value::Obj(cameras)) = value.get("cameras") {
            for (level, camera) in cameras.iter().take(iteration::MAX_AGENT_NODES + 1) {
                if !(level.is_empty() || cli_identifier(level)) {
                    continue;
                }
                let Some(pair) = camera.as_arr().filter(|pair| pair.len() == 2) else {
                    continue;
                };
                if let (Some(pan), Some(zoom)) = (number(&pair[0]), number(&pair[1])) {
                    if pan.is_finite() && zoom.is_finite() && zoom > 0.0 {
                        tree.cameras.insert(
                            level.clone(),
                            (pan.clamp(-1.0e6, 1.0e6), zoom.clamp(0.1, 10.0)),
                        );
                    }
                }
            }
        }
        Ok(tree)
    }
}

fn canonical_repo(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

impl Host {
    fn launch_receipt_status(&self, child: &str) -> Value {
        let mut value = self.engine.agent_status(child, 120).unwrap_or_else(|| {
            json::obj(vec![
                ("node", s(child)),
                ("status", s("deleted")),
                ("launch", s("ended")),
            ])
        });
        if let Value::Obj(fields) = &mut value {
            fields.push(("already_started".into(), Value::Bool(true)));
            if let Ok(session) = cli_screen_session_id(child) {
                fields.push(("session".into(), s(session)));
            }
        }
        value
    }

    fn spawn_agent(&mut self, command: FlowCommand) -> Result<Value, String> {
        let FlowCommand::Spawn(mut launch) = command else {
            return Err("Not a delegate launch".into());
        };
        self.require_active_lane(&launch.parent)?;
        let parent_node = self.engine.terminal_origin(&launch.parent)?.to_owned();
        if let Some(repo) = &launch.repo {
            launch.repo = Some(
                fs::canonicalize(repo)
                    .map_err(|error| format!("repo {} is unavailable: {error}", repo.display()))?,
            );
        }
        let signature = launch.signature();
        if let Some(receipt) = self.engine.agent_launch(&parent_node, &launch.request) {
            if receipt.signature != signature {
                return Err(format!(
                    "Launch request {} already started agent {} with a different payload; use a new request id for a different launch",
                    launch.request, receipt.child
                ));
            }
            // A retried or recovered launch reports the child it already made
            // (or that it was deleted) instead of starting a second agent.
            return Ok(self.launch_receipt_status(&receipt.child.clone()));
        }
        if launch.source == iteration::AgentSource::Own {
            if let Some(repo) = &launch.repo {
                let parent = self.flow(&launch.parent)?;
                let parent_shared = self
                    .engine
                    .agent_node(&launch.parent)
                    .is_some_and(|node| node.source == iteration::AgentSource::Shared);
                if parent_shared && canonical_repo(&parent.config.repo) == *repo {
                    return Err("This lane only reads its checkout and cannot hand it to a delegate as own source".into());
                }
                for other in self.engine.flows.values() {
                    if other.successor.is_some()
                        || other.lifecycle == iteration::FlowLifecycle::Archived
                    {
                        continue;
                    }
                    let owned = self
                        .engine
                        .agent_node(&other.id)
                        .is_some_and(|node| node.source == iteration::AgentSource::Own);
                    if owned && canonical_repo(&other.config.repo) == *repo {
                        return Err(format!(
                            "{} is already owned by lane {} ({}); choose another worktree or pass source shared",
                            repo.display(),
                            other.id,
                            other.title
                        ));
                    }
                }
            }
        }
        let task = launch.task.clone();
        let title = launch.title.clone();
        let parent = launch.parent.clone();
        let previous = self.engine.clone();
        let mut next = previous.clone();
        let timestamp = now();
        let spawn = next.apply(FlowCommand::Spawn(launch), timestamp)?;
        let child = spawn
            .events
            .first()
            .map(|event| event.flow.clone())
            .ok_or("Delegate launch produced no durable event")?;
        // The task is the child's first requirement, written in the same
        // durable transition as the graph edge and the launch receipt.
        let requirement = next.apply(
            FlowCommand::Requirement {
                flow: child.clone(),
                id: None,
                text: task,
            },
            timestamp,
        )?;
        // The child's control directory exists before its flow is published:
        // the loopback capability and screen binding are written ahead of the
        // provider start that the UI drives from the next snapshot.
        cli_control_dir(&self.directory, &child)?;
        let mut events = spawn.events;
        events.extend(requirement.events);
        let mut effects = spawn.effects;
        effects.extend(requirement.effects);
        self.engine = next;
        self.transition(
            previous,
            Transition {
                result: Value::Null,
                effects,
                events,
            },
        )?;
        let mut result = self
            .engine
            .agent_query(&child, iteration::AgentQuery::Status, None)?;
        if let Value::Obj(fields) = &mut result {
            fields.push(("session".into(), s(cli_screen_session_id(&child)?)));
            fields.push(("launch_note".into(), s("The child's terminal starts once its callback binding is published; launch reports pending until the session owner observes it. Admission is not completion. Check agent status at most once to confirm the launch, and never poll it in a loop: do other work or end the turn. The child's result arrives in this lane's durable inbox; read it with agent inbox and acknowledge it with ack.")));
        }
        self.note = format!("{parent}: delegated {child} ({title})");
        self.changed = true;
        Ok(result)
    }

    /// Two lanes on one canonical checkout must not checkpoint or build over
    /// each other's live evaluation. The Cargo lease serializes compilation;
    /// this refuses source mutation while another lane's human app is open or
    /// its build is queued or running.
    fn checkout_conflict(&self, flow: &str) -> Result<(), String> {
        let Some(state) = self.engine.flows.get(flow) else {
            return Ok(());
        };
        let repo = canonical_repo(&state.config.repo);
        for other in self.engine.flows.values() {
            if other.id == flow
                || other.successor.is_some()
                || other.lifecycle == iteration::FlowLifecycle::Archived
                || canonical_repo(&other.config.repo) != repo
            {
                continue;
            }
            let busy = other
                .runs
                .iter()
                .any(|run| run.role == iteration::RunRole::Human && !run.closed)
                // An AI test runs an immutable retained executable, so it
                // does not hold the checkout; a human evaluation app does.
                || self
                    .apps
                    .get(&other.id)
                    .is_some_and(|app| app.role == iteration::RunRole::Human)
                || self.builds.contains_key(&other.id)
                || other.job.as_ref().is_some_and(|job| {
                    matches!(
                        job.phase,
                        iteration::BuildPhase::WaitingForClose
                            | iteration::BuildPhase::CheckpointRequested
                            | iteration::BuildPhase::Ready
                            | iteration::BuildPhase::Building
                    )
                });
            if busy {
                return Err(format!(
                    "Checkout {} is in use by lane {} ({}); wait for its app to close and its build to finish before changing that source",
                    repo.display(),
                    other.id,
                    other.title
                ));
            }
        }
        Ok(())
    }

    /// The session owner's observation for a lane's terminal. Unchanged facts
    /// append no event; a changed state, identity or error is durable.
    fn terminal_observed(
        &mut self,
        flow: &str,
        fact: iteration::AgentTerminalFact,
    ) -> Result<Value, String> {
        let owner = self.engine.resolve_active_flow(flow)?.to_owned();
        let node = self.engine.terminal_origin(&owner)?.to_owned();
        if let Some(current) = self.engine.agent_terminal(&node) {
            let mut same = current.clone();
            same.at = fact.at;
            if same == fact {
                return Ok(Value::Bool(false));
            }
        }
        self.observe(Observation::TerminalObserved { flow: owner, fact })?;
        Ok(Value::Bool(true))
    }

    fn agent_delivered(&mut self, flow: &str, id: u64) -> Result<Value, String> {
        let owner = self.engine.resolve_active_flow(flow)?.to_owned();
        self.observe(Observation::AgentDelivered { flow: owner, id })
    }

    fn set_tree(&mut self, tree: TreePresentation) -> Result<Value, String> {
        if self.tree == tree {
            return Ok(Value::Bool(true));
        }
        let previous = std::mem::replace(&mut self.tree, tree);
        if let Err(error) = self.persist() {
            self.tree = previous;
            return Err(error);
        }
        self.changed = true;
        Ok(Value::Bool(true))
    }
}
