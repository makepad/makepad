// Durable history ownership overlays leave original evidence and event identities intact.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryEntry {
    pub key: String,
    pub at: u64,
}

fn split_history_key(key: &str) -> bool {
    key.split_once('/').is_some_and(|(kind, id)| {
        matches!(
            kind,
            "artifact" | "feedback" | "attachment" | "recording" | "job"
        ) && !id.is_empty()
            && id.len() <= 220
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    })
}
fn parse_split_history(value: &Value) -> Result<Vec<HistoryEntry>, String> {
    let values = value.as_arr().ok_or("Split history must be an array")?;
    if values.is_empty() || values.len() > 1024 {
        return Err("Split history must contain 1..1024 entries".into());
    }
    let mut history = Vec::new();
    let mut keys = std::collections::BTreeSet::new();
    for value in values {
        let tuple = value
            .as_arr()
            .filter(|v| v.len() == 2)
            .ok_or("Invalid split history entry")?;
        let key = tuple[0]
            .as_str()
            .filter(|key| split_history_key(key))
            .ok_or("Invalid split history identity")?;
        let at = tuple[1]
            .as_u64()
            .filter(|at| *at <= i64::MAX as u64)
            .ok_or("Invalid split history timestamp")?;
        if !keys.insert(key) {
            return Err("Duplicate split history identity".into());
        }
        if history
            .last()
            .is_some_and(|previous: &HistoryEntry| previous.at > at)
        {
            return Err("Split history must be chronological".into());
        }
        history.push(HistoryEntry {
            key: key.into(),
            at,
        });
    }
    Ok(history)
}

impl Engine {
    pub fn resolve_active_flow<'a>(&'a self, flow: &'a str) -> Result<&'a str, String> {
        let mut id = flow;
        for _ in 0..MAX_STORED_FLOWS {
            let state = self.flows.get(id).ok_or("Unknown iteration flow")?;
            match state.successor.as_deref() {
                Some(next) => id = next,
                None => return Ok(id),
            }
        }
        Err("Lane successor chain exceeds its bound".into())
    }
    pub fn terminal_origin<'a>(&'a self, flow: &'a str) -> Result<&'a str, String> {
        let mut id = flow;
        for _ in 0..MAX_STORED_FLOWS {
            let state = self.flows.get(id).ok_or("Unknown iteration flow")?;
            match state.predecessor.as_deref() {
                Some(previous) => id = previous,
                None => return Ok(id),
            }
        }
        Err("Lane predecessor chain exceeds its bound".into())
    }
    fn has_history_ancestor(&self, flow: &str, ancestor: &str) -> bool {
        let mut id = flow;
        for _ in 0..MAX_STORED_FLOWS {
            if id == ancestor {
                return true;
            }
            let Some(previous) = self
                .flows
                .get(id)
                .and_then(|flow| flow.predecessor.as_deref())
            else {
                return false;
            };
            id = previous;
        }
        false
    }
    /// Membership is separate from immutable dependencies. A moved feedback
    /// item can still resolve an artifact/capture owned by the archived prefix.
    pub fn history_visible(&self, flow: &str, key: &str) -> bool {
        !self.cleared_history.contains(key)
            && self
                .history_owners
                .get(key)
                .is_none_or(|owner| owner == flow)
    }
    pub fn history_owner<'a>(&'a self, origin: &'a str, key: &str) -> &'a str {
        self.history_owners
            .get(key)
            .map(String::as_str)
            .unwrap_or(origin)
    }
    pub fn artifact_in_use(flow: &Flow, artifact: &Artifact) -> bool {
        flow.runs
            .iter()
            .any(|run| run.artifact_id == artifact.id && !run.closed)
            || (flow
                .job
                .as_ref()
                .is_some_and(|job| job.id == artifact.job_id && job.phase == BuildPhase::Succeeded)
                && !flow
                    .runs
                    .iter()
                    .any(|run| run.artifact_id == artifact.id && run.role == RunRole::Human))
    }
    pub fn history_projection(&self, id: &str) -> Result<Flow, String> {
        let mut flow = self.flows.get(id).ok_or("Unknown lane")?.clone();
        let in_use: std::collections::BTreeSet<_> = flow
            .artifacts
            .iter()
            .filter(|artifact| Self::artifact_in_use(&flow, artifact))
            .map(|artifact| artifact.id.clone())
            .collect();
        flow.artifacts.retain(|artifact| {
            self.history_visible(id, &format!("artifact/{}", artifact.id))
                || in_use.contains(&artifact.id)
        });
        flow.feedback
            .retain(|feedback| self.history_visible(id, &format!("feedback/{}", feedback.id)));
        flow.runs.retain(|run| {
            flow.artifacts
                .iter()
                .any(|artifact| artifact.id == run.artifact_id)
        });
        Ok(flow)
    }
    /// Original creation identity, used for retained paths and recorder checks.
    pub fn evidence_origin(&self, kind: &str, id: &str) -> Option<&str> {
        let (event_kind, field) = match kind {
            "artifact" => ("build_succeeded", "artifact_id"),
            "run" => ("", "run_id"),
            _ => return None,
        };
        self.events
            .iter()
            .find(|event| {
                event.operation.get("observation").is_some_and(|o| {
                    let observed = o.get("kind").and_then(Value::as_str).unwrap_or("");
                    (if kind == "run" {
                        matches!(
                            observed,
                            "run_started" | "run_reopened" | "test_run_started"
                        )
                    } else {
                        observed == event_kind
                    }) && o.get(field).and_then(Value::as_str) == Some(id)
                })
            })
            .map(|event| event.flow.as_str())
    }
    fn split_lane_model(
        &mut self,
        source: &str,
        title: String,
        item: &str,
        history: Vec<HistoryEntry>,
    ) -> Result<(String, Vec<Effect>), String> {
        if self.flows.len() >= MAX_STORED_FLOWS {
            return Err("Stored history has reached its 64-lane bound".into());
        }
        let old = self.flows.get(source).ok_or("Unknown lane")?;
        if old.successor.is_some()
            || old.lifecycle != FlowLifecycle::Active
            || old.worktree.is_none()
        {
            return Err("Split requires the active lane that owns this source and terminal".into());
        }
        if old.runs.iter().any(|run| !run.closed) {
            return Err("Close the lane app and stop AI tests before splitting its history".into());
        }
        if old.job.as_ref().is_some_and(|job| {
            matches!(
                job.phase,
                BuildPhase::WaitingForClose
                    | BuildPhase::CheckpointRequested
                    | BuildPhase::Ready
                    | BuildPhase::Building
            ) || (job.phase == BuildPhase::Succeeded
                && old.artifacts.iter().any(|artifact| {
                    artifact.job_id == job.id
                        && !old
                            .runs
                            .iter()
                            .any(|run| run.role == RunRole::Human && run.artifact_id == artifact.id)
                }))
        }) {
            return Err(
                "Finish or cancel the pending build/launch before splitting this lane".into(),
            );
        }
        if title.trim().is_empty() || title.chars().any(char::is_control) {
            return Err("The new lane needs a nonempty title without control characters".into());
        }
        let position = history
            .iter()
            .position(|entry| entry.key == item)
            .ok_or("Selected history item is no longer in this lane")?;
        if history
            .iter()
            .any(|entry| !self.history_visible(source, &entry.key))
        {
            return Err("Split contains history owned by another lane".into());
        }
        let cutoff = history[position].at;
        let id = format!("flow-{}", self.revision + 1);
        let mut next = old.clone();
        let mut prefix = old.clone();
        let (
            prefix_requirements,
            prefix_todos,
            prefix_r,
            prefix_v,
            suffix_requirements,
            suffix_todos,
        ) = self.split_checklists(source, cutoff, item);
        prefix.requirements = prefix_requirements;
        prefix.todos = prefix_todos;
        prefix.requirements_revision = prefix_r;
        prefix.todos_revision = prefix_v;
        prefix.prepared = None;
        prefix.job = None;
        prefix.lifecycle = FlowLifecycle::Archived;
        prefix.successor = Some(id.clone());
        prefix.worktree = None;
        next.id = id.clone();
        next.title = title;
        next.predecessor = Some(source.into());
        next.successor = None;
        next.lifecycle = FlowLifecycle::Active;
        next.requirements
            .retain(|requirement| suffix_requirements.contains(&requirement.id));
        next.todos.retain(|todo| suffix_todos.contains(&todo.id));
        // Keep optimistic counters monotonic across a live process handoff.
        next.prepared = None;
        next.job = None;
        for (index, entry) in history.iter().enumerate() {
            self.history_owners.insert(
                entry.key.clone(),
                if index < position {
                    source.into()
                } else {
                    id.clone()
                },
            );
        }
        self.flows.insert(source.into(), prefix);
        self.flows.insert(id.clone(), next);
        Ok((id, vec![]))
    }
    fn split_checklists(
        &self,
        flow: &str,
        cutoff: u64,
        selected: &str,
    ) -> (
        Vec<Requirement>,
        Vec<Todo>,
        u64,
        u64,
        std::collections::BTreeSet<String>,
        std::collections::BTreeSet<String>,
    ) {
        let current = &self.flows[flow];
        let mut requirements = BTreeMap::<String, Requirement>::new();
        let mut todos = BTreeMap::<String, Todo>::new();
        let mut suffix_requirements = std::collections::BTreeSet::new();
        let selected_feedback = selected.strip_prefix("feedback/").and_then(|id| {
            current
                .feedback
                .iter()
                .find(|feedback| feedback.id == id && feedback.from_human)
        });
        if let Some(feedback) = selected_feedback {
            if let Some(sequence) = feedback
                .id
                .strip_prefix("feedback-")
                .and_then(|id| id.parse::<u64>().ok())
            {
                if let Some(event) = self
                    .events(flow)
                    .find(|event| event.sequence + 1 == sequence)
                {
                    if let Some(command) = event
                        .operation
                        .get("command")
                        .and_then(|value| parse_command(value).ok())
                    {
                        if let Command::Requirement { id, .. } = command {
                            suffix_requirements
                                .insert(id.unwrap_or_else(|| format!("req-{}", event.sequence)));
                        }
                    }
                }
            }
        }
        let artifact_scope = selected
            .strip_prefix("artifact/")
            .and_then(|id| current.artifacts.iter().find(|artifact| artifact.id == id))
            .map(|artifact| artifact.requirements_revision);
        let mut scope_revision = 0;
        if let Some(wanted) = artifact_scope {
            for event in self.events(flow) {
                if let Some(Command::Requirement { id, .. }) = event
                    .operation
                    .get("command")
                    .and_then(|value| parse_command(value).ok())
                {
                    scope_revision += 1;
                    if scope_revision == wanted {
                        suffix_requirements
                            .insert(id.unwrap_or_else(|| format!("req-{}", event.sequence)));
                        break;
                    }
                }
            }
        }
        let selected_scope = suffix_requirements.clone();
        let mut suffix_todos = std::collections::BTreeSet::new();
        let mut revision_r = 0;
        let mut revision_v = 0;
        let mut observed_r = 0;
        for event in self.events(flow) {
            let Some(command) = event.operation.get("command") else {
                continue;
            };
            let Ok(command) = parse_command(command) else {
                continue;
            };
            match command {
                Command::Requirement { id, text, .. } => {
                    observed_r += 1;
                    if event.at < cutoff {
                        revision_r = observed_r;
                    }
                    let id = id.unwrap_or_else(|| format!("req-{}", event.sequence));
                    if !current.requirements.iter().any(|r| r.id == id) {
                        continue;
                    }
                    if event.at >= cutoff {
                        suffix_requirements.insert(id);
                    } else {
                        // Immutable event order supplies scope; archived values
                        // remain explanatory and can never authorize a build.
                        requirements.insert(
                            id.clone(),
                            Requirement {
                                id,
                                text,
                                revision: observed_r,
                            },
                        );
                    }
                }
                Command::Todos {
                    expected_revision,
                    updates,
                    ..
                } => {
                    if event.at < cutoff {
                        revision_v = expected_revision + 1;
                    }
                    for update in updates {
                        let Some(known) = current.todos.iter().find(|todo| todo.id == update.id)
                        else {
                            continue;
                        };
                        if event.at >= cutoff {
                            suffix_todos.insert(update.id);
                            continue;
                        }
                        let previous = todos.get(&update.id);
                        let text = update
                            .text
                            .or_else(|| previous.map(|todo| todo.text.clone()))
                            .unwrap_or_else(|| known.text.clone());
                        todos.insert(
                            update.id.clone(),
                            Todo {
                                id: update.id,
                                state: update.state,
                                text,
                                revision: expected_revision + 1,
                                source_revision: known.source_revision,
                                requirements_revision: revision_r,
                            },
                        );
                    }
                }
                _ => {}
            }
        }
        requirements.retain(|id, _| !selected_scope.contains(id));
        (
            requirements.into_values().collect(),
            todos.into_values().collect(),
            revision_r,
            revision_v,
            suffix_requirements,
            suffix_todos,
        )
    }
}
