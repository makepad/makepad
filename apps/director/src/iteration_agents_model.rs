// Included by iteration.rs. Agent genealogy: nodes keyed by stable terminal
// origin, delegation budgets, launch receipts, the parent/child inbox,
// observed terminal facts, queries and a JSON persistence envelope kept
// beside the legacy binary `Flow` snapshot.
fn flow_sequence(id: &str) -> u64 {
    id.strip_prefix("flow-")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

fn agent_message_json(message: &AgentMessage) -> Value {
    Value::Arr(vec![
        n(message.id),
        json::s(&message.from),
        json::s(message.kind.as_str()),
        json::s(&message.text),
        Value::Bool(message.read),
        message.delivered.map(n).unwrap_or(Value::Null),
    ])
}

fn parse_agent_message(value: &Value) -> Result<AgentMessage, String> {
    let tuple = value
        .as_arr()
        .filter(|tuple| matches!(tuple.len(), 5 | 6))
        .ok_or("agent message must be [id, from, kind, text, read, delivered?]")?;
    let text = tuple[3]
        .as_str()
        .ok_or("agent message text must be a string")?;
    if text.len() > 4096 {
        return Err("agent message exceeds 4096 bytes".into());
    }
    Ok(AgentMessage {
        id: tuple[0]
            .as_u64()
            .ok_or("agent message id must be an integer")?,
        from: identifier(&json::obj(vec![("from", tuple[1].clone())]), "from")?,
        kind: AgentMessageKind::parse(
            tuple[2]
                .as_str()
                .ok_or("agent message kind must be a string")?,
        )?,
        text: text.to_owned(),
        read: tuple[4]
            .as_bool()
            .ok_or("agent message read flag must be a boolean")?,
        delivered: match tuple.get(5) {
            None | Some(Value::Null) => None,
            Some(value) => Some(
                value
                    .as_u64()
                    .ok_or("agent message delivery must be a sequence")?,
            ),
        },
    })
}

impl Engine {
    /// The agent a flow belongs to: its stable terminal origin.
    pub fn agent_node(&self, flow: &str) -> Option<&AgentNode> {
        let origin = self.terminal_origin(flow).ok()?;
        self.agents.get(origin)
    }
    /// The flow currently owning an agent's terminal: the successor-less
    /// member of its history chain. None once every flow was deleted.
    pub fn agent_current_flow(&self, node: &str) -> Option<&Flow> {
        self.flows.values().find(|flow| {
            flow.successor.is_none() && self.terminal_origin(&flow.id).ok() == Some(node)
        })
    }
    fn agent_live(&self, node: &str) -> bool {
        self.agent_current_flow(node)
            .is_some_and(|flow| flow.lifecycle != FlowLifecycle::Archived)
    }
    /// Active or stopped agents directly under the Director root.
    pub fn root_agent_count(&self) -> usize {
        self.agents
            .values()
            .filter(|node| node.parent.is_none() && self.agent_live(&node.id))
            .count()
    }
    /// Active or stopped agents anywhere in the tree.
    pub fn active_agent_count(&self) -> usize {
        self.agents
            .values()
            .filter(|node| self.agent_live(&node.id))
            .count()
    }
    fn live_children(&self, node: &str) -> usize {
        self.agents
            .values()
            .filter(|child| child.parent.as_deref() == Some(node) && self.agent_live(&child.id))
            .count()
    }
    /// Every retained agent id, tombstones included.
    pub fn agent_ids(&self) -> Vec<String> {
        self.agents.keys().cloned().collect()
    }
    /// Direct children in stable sibling order; None lists the root agents.
    pub fn agent_children(&self, parent: Option<&str>) -> Vec<&AgentNode> {
        let mut children: Vec<_> = self
            .agents
            .values()
            .filter(|node| node.parent.as_deref() == parent)
            .collect();
        children.sort_by(|a, b| (a.order, &a.id).cmp(&(b.order, &b.id)));
        children
    }
    /// The lanes one tree level shows, in order. For an agent: its own lane
    /// first (its current flow, whatever its lifecycle; a deleted agent that
    /// only remains as a parent has none), then the lanes of its direct
    /// children in stable sibling order, archived children left out as
    /// everywhere else. For the Director root (`None`): the independent root
    /// lanes only; the root is no agent and has no lane. Never grandchildren,
    /// siblings or ancestors.
    pub fn agent_level_flows(&self, level: Option<&str>) -> Vec<&Flow> {
        level
            .and_then(|node| self.agent_current_flow(node))
            .into_iter()
            .chain(
                self.agent_children(level)
                    .into_iter()
                    .filter_map(|node| self.agent_current_flow(&node.id))
                    .filter(|flow| flow.lifecycle != FlowLifecycle::Archived),
            )
            .collect()
    }
    /// Root-first ancestor ids, excluding the node. Stops at a missing parent
    /// or a repeated id instead of guessing.
    pub fn agent_ancestors(&self, node: &str) -> Vec<String> {
        let mut chain = Vec::new();
        let mut current = self.agents.get(node).and_then(|node| node.parent.clone());
        for _ in 0..MAX_AGENT_NODES {
            let Some(id) = current else {
                break;
            };
            if chain.contains(&id) || id == node {
                break;
            }
            current = self.agents.get(&id).and_then(|node| node.parent.clone());
            chain.push(id);
        }
        chain.reverse();
        chain
    }
    pub fn agent_depth(&self, node: &str) -> usize {
        self.agent_ancestors(node).len() + 1
    }
    pub fn agent_unread(&self, node: &str) -> Vec<&AgentMessage> {
        self.agent_inbox
            .get(node)
            .map(|inbox| inbox.iter().filter(|message| !message.read).collect())
            .unwrap_or_default()
    }
    /// Messages the UI has not yet typed into the recipient's prompt.
    pub fn agent_undelivered(&self, node: &str) -> Vec<&AgentMessage> {
        self.agent_inbox
            .get(node)
            .map(|inbox| {
                inbox
                    .iter()
                    .filter(|message| message.delivered.is_none() && !message.read)
                    .collect()
            })
            .unwrap_or_default()
    }
    /// The latest result a child reported, retained independently of its
    /// parent's inbox pruning.
    pub fn agent_latest_result(&self, node: &str) -> Option<&AgentMessage> {
        self.agent_results.get(node)
    }
    pub fn agent_terminal(&self, node: &str) -> Option<&AgentTerminalFact> {
        self.agent_terminals.get(node)
    }
    /// The receipt a launch request already produced under a parent, if any.
    pub fn agent_launch(&self, parent: &str, request: &str) -> Option<&AgentLaunchReceipt> {
        self.agent_launches
            .get(parent)?
            .iter()
            .find(|receipt| receipt.request == request)
    }
    /// Shared-source delegates read a checkout another lane owns: no prepared
    /// report, checkpoint, build or Git mutation may come from them.
    pub fn source_mutation_allowed(&self, flow: &str) -> Result<(), String> {
        match self.agent_node(flow) {
            Some(node) if node.source == AgentSource::Shared => Err("this delegate shares another lane's checkout read-only; ask the owning lane to prepare, build or change source, or start the delegate with its own repo".into()),
            _ => Ok(()),
        }
    }
    /// Budget check before a flow becomes active or stopped again.
    fn agent_admission(&self, flow: &str) -> Result<(), String> {
        let Some(node) = self.agent_node(flow) else {
            return Ok(());
        };
        if self.agent_live(&node.id) {
            return Ok(());
        }
        if self.active_agent_count() >= MAX_ACTIVE_AGENTS {
            return Err(format!(
                "at most {MAX_ACTIVE_AGENTS} active or stopped agents may exist across the tree"
            ));
        }
        match &node.parent {
            None if self.root_agent_count() >= MAX_ROOT_AGENTS => {
                Err("at most four active or stopped lanes may be visible".into())
            }
            Some(parent) if self.live_children(parent) >= MAX_CHILDREN_PER_AGENT => Err(format!(
                "agent {parent} already has {MAX_CHILDREN_PER_AGENT} live children"
            )),
            _ => Ok(()),
        }
    }

    /// The parent's whole live context is inherited (roles, review rules,
    /// wherever they stand in it), the child's ownership, task and source
    /// rules are appended, and the caller's extra context comes last. The
    /// result is bounded like flow_context at 4096 bytes and nothing is ever
    /// cut: when the three parts do not fit, the launch is refused and the
    /// caller shortens its extra context or the lane's own context.
    fn child_delegation_context(
        parent_context: &str,
        parent_node: &str,
        parent_title: &str,
        source: AgentSource,
        extra: Option<&str>,
    ) -> Result<String, String> {
        let rules = format!(
            "\n\nDelegated by agent {parent_node} ({parent_title}). You own exactly the task recorded as your first requirement; read its full text from /brief q or GET /state if it is excerpted. Report progress with flow_todos and your outcome with flow_agent message kind result to {parent_node}; read follow-ups with flow_agent inbox and acknowledge them. Delegate further only with flow_agent start. Source: {}.",
            match source {
                AgentSource::Own => "your own checkout",
                AgentSource::Shared =>
                    "read-only shared checkout; no prepared report, build or Git mutation",
            }
        );
        let extra = extra
            .map(|extra| format!("\n\n{extra}"))
            .unwrap_or_default();
        let inherited = parent_context.trim();
        let total = inherited.len() + rules.len() + extra.len();
        if total > 4096 {
            return Err(format!(
                "the child's delegation context would be {total} bytes and the limit is 4096: this lane's context ({} bytes) and the delegation rules ({} bytes) are kept whole, which leaves {} bytes for context; shorten context, or this lane's own context with flow_context",
                inherited.len(),
                rules.len(),
                4096usize.saturating_sub(inherited.len() + rules.len() + 2)
            ));
        }
        Ok(format!("{inherited}{rules}{extra}"))
    }

    fn spawn_agent(
        &mut self,
        launch: AgentLaunch,
        sequence: u64,
    ) -> Result<(String, Vec<Effect>), String> {
        if launch.request.is_empty() {
            return Err("a durable launch request id is required".into());
        }
        let parent_flow = self
            .flows
            .get(&launch.parent)
            .ok_or("unknown parent lane")?;
        if parent_flow.successor.is_some() {
            return Err("delegate from the active successor lane, not an archived prefix".into());
        }
        if parent_flow.lifecycle != FlowLifecycle::Active {
            return Err("only an active lane can delegate".into());
        }
        let parent_node = self.terminal_origin(&launch.parent)?.to_owned();
        let parent_record = self
            .agents
            .get(&parent_node)
            .filter(|node| !node.tombstone)
            .ok_or("parent agent record is unavailable")?;
        let parent_source = parent_record.source;
        let signature = launch.signature();
        if let Some(receipt) = self.agent_launch(&parent_node, &launch.request) {
            return Err(if receipt.signature == signature {
                format!(
                    "launch request {} already started agent {}; inspect it instead of starting another",
                    launch.request, receipt.child
                )
            } else {
                format!(
                    "launch request {} already started agent {} with a different payload; use a new request id for a different launch",
                    launch.request, receipt.child
                )
            });
        }
        if self.agent_depth(&parent_node) >= MAX_AGENT_DEPTH {
            return Err(format!(
                "agent nesting is limited to {MAX_AGENT_DEPTH} levels"
            ));
        }
        if self.live_children(&parent_node) >= MAX_CHILDREN_PER_AGENT {
            return Err(format!(
                "this lane already has {MAX_CHILDREN_PER_AGENT} live children; archive one first"
            ));
        }
        if self.active_agent_count() >= MAX_ACTIVE_AGENTS {
            return Err(format!(
                "at most {MAX_ACTIVE_AGENTS} active or stopped agents may exist across the tree; archive one first"
            ));
        }
        if self.flows.len() >= MAX_STORED_FLOWS {
            return Err("stored iteration history has reached its 64-flow bound".into());
        }
        if self.agents.len() >= MAX_AGENT_NODES {
            return Err(
                "retained agent records have reached their bound; delete lanes first".into(),
            );
        }
        // Receipts are never expired, so a consumed request can never start
        // again. A parent that used its whole capacity is refused instead.
        if self
            .agent_launches
            .get(&parent_node)
            .is_some_and(|receipts| receipts.len() >= MAX_AGENT_LAUNCHES)
        {
            return Err(format!(
                "this lane has used all {MAX_AGENT_LAUNCHES} retained launch requests; delegate from another lane"
            ));
        }
        let parent_config = parent_flow.config.clone();
        let parent_title = parent_flow.title.clone();
        let AgentLaunch {
            parent,
            request,
            title,
            provider,
            task: _,
            repo,
            source,
            context,
            resume_token,
        } = launch;
        let (repo, source) = match (repo, source) {
            (None, AgentSource::Own) => {
                return Err(
                    "an own-source delegate needs an explicit repo; omit repo for read-only shared source"
                        .into(),
                )
            }
            (None, AgentSource::Shared) => (parent_config.repo.clone(), AgentSource::Shared),
            (Some(repo), AgentSource::Shared) => (repo, AgentSource::Shared),
            (Some(repo), AgentSource::Own) => {
                if parent_source == AgentSource::Shared && repo == parent_config.repo {
                    return Err("this lane only reads its checkout and cannot hand it to a delegate as own source".into());
                }
                if let Some(owner) = self
                    .agents
                    .values()
                    .filter(|node| node.source == AgentSource::Own && self.agent_live(&node.id))
                    .find(|node| {
                        self.agent_current_flow(&node.id)
                            .is_some_and(|flow| flow.config.repo == repo)
                    })
                {
                    return Err(format!(
                        "{} is already owned by agent {}; choose another worktree or pass source shared",
                        repo.display(),
                        owner.id
                    ));
                }
                (repo, AgentSource::Own)
            }
        };
        let manifest = if repo == parent_config.repo {
            parent_config.manifest.clone()
        } else {
            match parent_config.manifest.strip_prefix(&parent_config.repo) {
                Ok(relative) => repo.join(relative),
                Err(_) => repo.join("Cargo.toml"),
            }
        };
        let delegation_context = Self::child_delegation_context(
            &parent_config.delegation_context,
            &parent_node,
            &parent_title,
            source,
            context.as_deref(),
        )?;
        let id = format!("flow-{sequence}");
        self.flows.insert(
            id.clone(),
            Flow {
                id: id.clone(),
                title,
                package: parent_config.package.clone(),
                config: FlowConfig {
                    repo,
                    manifest,
                    package: parent_config.package,
                    binary: parent_config.binary,
                    check_targets: parent_config.check_targets,
                    test_scope: parent_config.test_scope,
                    agent_provider: Some(provider),
                    delegation_context,
                    resume_token,
                },
                lifecycle: FlowLifecycle::Active,
                predecessor: None,
                successor: None,
                source_revision: 1,
                requirements_revision: 0,
                requirements: vec![],
                prepared: None,
                todos_revision: 0,
                todos: vec![],
                job: None,
                artifacts: vec![],
                runs: vec![],
                captures: vec![],
                feedback: vec![],
            },
        );
        self.agents.insert(
            id.clone(),
            AgentNode {
                id: id.clone(),
                parent: Some(parent_node.clone()),
                order: sequence,
                spawned_by: Some(parent),
                request: Some(request.clone()),
                source,
                tombstone: false,
            },
        );
        let receipts = self.agent_launches.entry(parent_node).or_default();
        receipts.push(AgentLaunchReceipt {
            request,
            signature,
            child: id.clone(),
            at: sequence,
        });
        Ok((id, vec![]))
    }

    /// The agent a message's `to` names for this sender. The reserved
    /// `parent` is the sender's own immediate parent, read from its stable
    /// node, so it names the same agent in the mutation, in the reply and on
    /// replay; a root has none. Anything else is an agent id. This only
    /// names the recipient: whether the sender may write to it is decided by
    /// `agent_send`.
    fn agent_recipient<'a>(sender: &'a AgentNode, to: &'a str) -> Result<&'a str, String> {
        if to == "parent" {
            sender
                .parent
                .as_deref()
                .ok_or_else(|| "This agent has no parent".into())
        } else {
            Ok(to)
        }
    }

    /// `agent_recipient` for the agent that owns the lane `flow`.
    fn agent_message_recipient(&self, flow: &str, to: &str) -> Result<String, String> {
        let sender = self
            .agents
            .get(self.terminal_origin(flow)?)
            .ok_or("sender agent record is unavailable")?;
        Self::agent_recipient(sender, to).map(str::to_owned)
    }

    fn agent_send(
        &mut self,
        flow: &str,
        to: &str,
        kind: AgentMessageKind,
        text: String,
        sequence: u64,
    ) -> Result<(), String> {
        let sender_flow = self.flows.get(flow).ok_or("unknown lane")?;
        if sender_flow.successor.is_some() {
            return Err("send from the active successor lane, not an archived prefix".into());
        }
        let sender = self.terminal_origin(flow)?.to_owned();
        let sender_node = self
            .agents
            .get(&sender)
            .ok_or("sender agent record is unavailable")?;
        let to = Self::agent_recipient(sender_node, to)?;
        let target = self.agents.get(to).ok_or("unknown agent")?;
        if target.tombstone {
            return Err("that agent's lanes were deleted; nothing can read the message".into());
        }
        let to_parent = sender_node.parent.as_deref() == Some(to);
        let related = target.parent.as_deref() == Some(sender.as_str()) || to_parent;
        if !related {
            return Err(
                "messages travel only between a lane and its direct children or parent".into(),
            );
        }
        let message = AgentMessage {
            id: sequence,
            from: sender.clone(),
            kind,
            text,
            read: false,
            delivered: None,
        };
        if kind == AgentMessageKind::Result && to_parent {
            self.agent_results.insert(sender, message.clone());
        }
        let inbox = self.agent_inbox.entry(to.to_owned()).or_default();
        if inbox.len() >= MAX_AGENT_INBOX {
            match inbox.iter().position(|message| message.read) {
                Some(index) => {
                    inbox.remove(index);
                }
                None => {
                    return Err(format!(
                        "agent {to} has {MAX_AGENT_INBOX} unread messages; it must acknowledge its inbox first"
                    ))
                }
            }
        }
        inbox.push(message);
        Ok(())
    }

    fn agent_acknowledge(&mut self, flow: &str, ack: u64) -> Result<(), String> {
        self.flows.get(flow).ok_or("unknown lane")?;
        let node = self.terminal_origin(flow)?.to_owned();
        if !self.agents.contains_key(&node) {
            return Err("this lane has no agent record".into());
        }
        let inbox = self
            .agent_inbox
            .get_mut(&node)
            .ok_or("the inbox is empty; nothing to acknowledge")?;
        let mut changed = false;
        for message in inbox.iter_mut() {
            if message.id <= ack && !message.read {
                message.read = true;
                changed = true;
            }
        }
        if !changed {
            return Err("no unread message has an id at or below the acknowledged id".into());
        }
        Ok(())
    }

    fn agent_message_delivered(
        &mut self,
        flow: &str,
        id: u64,
        sequence: u64,
    ) -> Result<(), String> {
        self.flows.get(flow).ok_or("unknown lane")?;
        let node = self.terminal_origin(flow)?.to_owned();
        let message = self
            .agent_inbox
            .get_mut(&node)
            .and_then(|inbox| inbox.iter_mut().find(|message| message.id == id))
            .ok_or("no such inbox message to mark delivered")?;
        if message.delivered.is_some() {
            return Err("that message was already delivered".into());
        }
        message.delivered = Some(sequence);
        Ok(())
    }

    fn agent_terminal_observed(
        &mut self,
        flow: &str,
        fact: AgentTerminalFact,
    ) -> Result<(), String> {
        self.flows.get(flow).ok_or("unknown lane")?;
        let node = self.terminal_origin(flow)?.to_owned();
        if !self.agents.contains_key(&node) {
            return Err("this lane has no agent record".into());
        }
        self.agent_terminals.insert(node, fact);
        Ok(())
    }

    /// A worker that just started has observed nothing: stored terminal facts
    /// are last-run metadata (session, conversation) until seen again.
    pub fn invalidate_agent_terminals(&mut self) {
        for fact in self.agent_terminals.values_mut() {
            fact.live = false;
        }
    }

    /// The grant that lets `flow`'s agent test `artifact`, if its direct
    /// parent issued one.
    pub fn agent_grant_for(&self, flow: &str, artifact: &str) -> Option<&AgentGrant> {
        let node = self.terminal_origin(flow).ok()?;
        self.agent_grants
            .get(node)?
            .iter()
            .find(|grant| grant.artifact == artifact)
    }
    /// A grant by id, only for the lane it was issued to.
    pub fn agent_grant_by_id(&self, flow: &str, id: &str) -> Option<&AgentGrant> {
        let node = self.terminal_origin(flow).ok()?;
        self.agent_grants
            .get(node)?
            .iter()
            .find(|grant| grant.id == id)
    }
    pub fn agent_grants_of(&self, node: &str) -> &[AgentGrant] {
        self.agent_grants
            .get(node)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
    /// Every grant, for retention checks: a granted executable must outlive
    /// the deletion of the lane that built it.
    pub fn agent_grants_all(&self) -> impl Iterator<Item = &AgentGrant> {
        self.agent_grants.values().flatten()
    }

    fn agent_grant(
        &mut self,
        flow: &str,
        child: &str,
        artifact: &str,
        sequence: u64,
    ) -> Result<(), String> {
        let owner_flow = self.flows.get(flow).ok_or("unknown lane")?;
        if owner_flow.successor.is_some() {
            return Err("grant from the active successor lane, not an archived prefix".into());
        }
        let owner = self.terminal_origin(flow)?.to_owned();
        let record = self.agents.get(child).ok_or("unknown agent")?;
        if record.tombstone || record.parent.as_deref() != Some(owner.as_str()) {
            return Err(
                "an artifact can only be granted to a live direct child of this lane".into(),
            );
        }
        let retained = owner_flow
            .artifacts
            .iter()
            .find(|item| item.id == artifact)
            .ok_or("unknown retained artifact of this lane")?;
        let grants = self
            .agent_grants
            .get(child)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        if grants.iter().any(|grant| grant.artifact == artifact) {
            return Err("that artifact is already granted to this child".into());
        }
        if grants.len() >= MAX_AGENT_GRANTS {
            return Err(format!(
                "this child already holds {MAX_AGENT_GRANTS} artifact grants"
            ));
        }
        let grant = AgentGrant {
            id: format!("grant-{sequence}"),
            owner,
            // The directory that retains the executable is named after the
            // flow that built it, which may be an archived split prefix.
            origin_flow: self
                .evidence_origin("artifact", artifact)
                .unwrap_or(flow)
                .to_owned(),
            artifact: retained.id.clone(),
            commit: retained.commit.clone(),
            path: retained.path.clone(),
            child: child.to_owned(),
            at: sequence,
        };
        self.agent_grants
            .entry(child.to_owned())
            .or_default()
            .push(grant);
        Ok(())
    }

    /// After flows were removed: agents without any flow become tombstones,
    /// tombstones nobody references any more are dropped, and per-agent
    /// state follows its record. Launch receipts stay with the parent record
    /// so a deleted child never restarts under an old request.
    fn agent_flows_changed(&mut self) {
        let live: std::collections::BTreeSet<String> = self
            .flows
            .keys()
            .filter_map(|flow| self.terminal_origin(flow).ok().map(str::to_owned))
            .collect();
        for node in self.agents.values_mut() {
            node.tombstone = !live.contains(&node.id);
        }
        loop {
            let referenced: std::collections::BTreeSet<String> = self
                .agents
                .values()
                .filter_map(|node| node.parent.clone())
                .collect();
            let before = self.agents.len();
            self.agents
                .retain(|id, node| !node.tombstone || referenced.contains(id));
            if self.agents.len() == before {
                break;
            }
        }
        self.agent_inbox
            .retain(|id, _| self.agents.contains_key(id));
        self.agent_results
            .retain(|id, _| self.agents.contains_key(id));
        self.agent_terminals
            .retain(|id, _| self.agents.contains_key(id));
        self.agent_launches
            .retain(|id, _| self.agents.contains_key(id));
        // A grant follows the child it was issued to. It survives the
        // deletion of the granting lane: the record keeps the provenance and
        // the worker keeps the granted executable while a grant names it.
        self.agent_grants
            .retain(|id, _| self.agents.get(id).is_some_and(|node| !node.tombstone));
    }

    /// Validate the graph against the flows. With `legacy` (a compacted
    /// history from before delegation existed) every flow becomes a root
    /// agent; otherwise a flow without its record or a record with a missing
    /// parent is corrupt ancestry and refused rather than silently rooted.
    fn reconcile_agents(&mut self, legacy: bool) -> Result<(), String> {
        let mut origins = Vec::new();
        for flow in self.flows.keys() {
            origins.push((flow.clone(), self.terminal_origin(flow)?.to_owned()));
        }
        for (flow, origin) in origins {
            if !self.agents.contains_key(&origin) {
                if !legacy {
                    return Err(format!(
                        "agent graph is missing agent {origin} for lane {flow}"
                    ));
                }
                self.agents.insert(
                    origin.clone(),
                    AgentNode {
                        order: flow_sequence(&origin),
                        id: origin,
                        parent: None,
                        spawned_by: None,
                        request: None,
                        source: AgentSource::Own,
                        tombstone: false,
                    },
                );
            }
        }
        if self.agents.len() > MAX_AGENT_NODES {
            return Err("agent graph exceeds its retained record bound".into());
        }
        for node in self.agents.values() {
            if let Some(parent) = &node.parent {
                if !self.agents.contains_key(parent) {
                    return Err(format!(
                        "agent {} references missing parent {parent}",
                        node.id
                    ));
                }
            }
        }
        for id in self.agents.keys() {
            let mut seen = std::collections::BTreeSet::new();
            let mut current = Some(id.clone());
            while let Some(node) = current {
                if !seen.insert(node.clone()) {
                    return Err(format!("agent {id} has cyclic ancestry"));
                }
                if seen.len() > MAX_AGENT_DEPTH {
                    return Err(format!(
                        "agent {id} exceeds the depth bound of {MAX_AGENT_DEPTH}"
                    ));
                }
                current = self.agents.get(&node).and_then(|node| node.parent.clone());
            }
        }
        // Stored arrays beyond their admission bounds are corrupt state, not
        // something to trim: dropping a receipt would let its request restart.
        if self
            .agent_launches
            .values()
            .any(|receipts| receipts.len() > MAX_AGENT_LAUNCHES)
        {
            return Err("stored launch receipts exceed their bound".into());
        }
        if self
            .agent_inbox
            .values()
            .any(|inbox| inbox.len() > MAX_AGENT_INBOX)
        {
            return Err("a stored agent inbox exceeds its bound".into());
        }
        if self
            .agent_grants
            .values()
            .any(|grants| grants.len() > MAX_AGENT_GRANTS)
        {
            return Err("stored artifact grants exceed their bound".into());
        }
        self.agent_flows_changed();
        Ok(())
    }

    fn agents_json(&self) -> Value {
        let optional =
            |value: &Option<String>| value.as_deref().map(json::s).unwrap_or(Value::Null);
        json::obj(vec![
            (
                "nodes",
                Value::Arr(
                    self.agents
                        .values()
                        .map(|node| {
                            json::obj(vec![
                                ("id", json::s(&node.id)),
                                ("parent", optional(&node.parent)),
                                ("order", n(node.order)),
                                ("spawned_by", optional(&node.spawned_by)),
                                ("request", optional(&node.request)),
                                ("source", json::s(node.source.as_str())),
                                ("tombstone", Value::Bool(node.tombstone)),
                            ])
                        })
                        .collect(),
                ),
            ),
            (
                "inbox",
                Value::Obj(
                    self.agent_inbox
                        .iter()
                        .map(|(id, messages)| {
                            (
                                id.clone(),
                                Value::Arr(messages.iter().map(agent_message_json).collect()),
                            )
                        })
                        .collect(),
                ),
            ),
            (
                "launches",
                Value::Obj(
                    self.agent_launches
                        .iter()
                        .map(|(parent, receipts)| {
                            (
                                parent.clone(),
                                Value::Arr(
                                    receipts
                                        .iter()
                                        .map(|receipt| {
                                            Value::Arr(vec![
                                                json::s(&receipt.request),
                                                json::s(&receipt.signature),
                                                json::s(&receipt.child),
                                                n(receipt.at),
                                            ])
                                        })
                                        .collect(),
                                ),
                            )
                        })
                        .collect(),
                ),
            ),
            (
                "results",
                Value::Obj(
                    self.agent_results
                        .iter()
                        .map(|(id, message)| (id.clone(), agent_message_json(message)))
                        .collect(),
                ),
            ),
            (
                "terminals",
                Value::Obj(
                    self.agent_terminals
                        .iter()
                        .map(|(id, fact)| {
                            (
                                id.clone(),
                                json::obj(vec![
                                    ("state", json::s(fact.state.as_str())),
                                    ("provider", optional(&fact.provider)),
                                    ("session", optional(&fact.session)),
                                    ("conversation", optional(&fact.conversation)),
                                    (
                                        "pid",
                                        fact.pid.map(|pid| n(pid.into())).unwrap_or(Value::Null),
                                    ),
                                    ("error", optional(&fact.error)),
                                    ("at", n(fact.at)),
                                ]),
                            )
                        })
                        .collect(),
                ),
            ),
            (
                "grants",
                Value::Obj(
                    self.agent_grants
                        .iter()
                        .map(|(child, grants)| {
                            (
                                child.clone(),
                                Value::Arr(
                                    grants
                                        .iter()
                                        .map(|grant| {
                                            json::obj(vec![
                                                ("id", json::s(&grant.id)),
                                                ("owner", json::s(&grant.owner)),
                                                ("origin_flow", json::s(&grant.origin_flow)),
                                                ("artifact", json::s(&grant.artifact)),
                                                ("commit", json::s(&grant.commit)),
                                                ("path", path_json(&grant.path)),
                                                ("at", n(grant.at)),
                                            ])
                                        })
                                        .collect(),
                                ),
                            )
                        })
                        .collect(),
                ),
            ),
        ])
    }

    fn decode_agents(&mut self, value: Option<&Value>) -> Result<(), String> {
        let Some(value) = value else {
            return Ok(());
        };
        fields(
            value,
            &[
                "nodes",
                "inbox",
                "launches",
                "results",
                "terminals",
                "grants",
            ],
            &["nodes"],
        )?;
        for node in array(value, "nodes", MAX_AGENT_NODES)? {
            fields(
                node,
                &[
                    "id",
                    "parent",
                    "order",
                    "spawned_by",
                    "request",
                    "source",
                    "tombstone",
                ],
                &["id", "order", "source", "tombstone"],
            )?;
            let id = identifier(node, "id")?;
            let optional = |key: &str| -> Result<Option<String>, String> {
                match node.get(key) {
                    None | Some(Value::Null) => Ok(None),
                    Some(_) => identifier(node, key).map(Some),
                }
            };
            let parent = optional("parent")?;
            if parent.as_deref() == Some(id.as_str()) {
                return Err("an agent cannot be its own parent".into());
            }
            let record = AgentNode {
                id: id.clone(),
                parent,
                order: uint(node, "order")?,
                spawned_by: optional("spawned_by")?,
                request: optional("request")?,
                source: AgentSource::parse(&text(node, "source", 8)?)?,
                tombstone: node
                    .get("tombstone")
                    .and_then(Value::as_bool)
                    .ok_or("tombstone must be a boolean")?,
            };
            if self.agents.insert(id, record).is_some() {
                return Err("duplicate agent record".into());
            }
        }
        if let Some(inbox) = value.get("inbox") {
            let Value::Obj(entries) = inbox else {
                return Err("agent inbox must be an object".into());
            };
            for (id, messages) in entries {
                if !self.agents.contains_key(id) {
                    continue;
                }
                let messages = messages
                    .as_arr()
                    .ok_or("agent inbox entries must be arrays")?;
                if messages.len() > MAX_AGENT_INBOX {
                    return Err("a stored agent inbox exceeds its bound".into());
                }
                let mut list = Vec::new();
                for message in messages {
                    list.push(parse_agent_message(message)?);
                }
                self.agent_inbox.insert(id.clone(), list);
            }
        }
        if let Some(launches) = value.get("launches") {
            let Value::Obj(entries) = launches else {
                return Err("agent launches must be an object".into());
            };
            for (parent, receipts) in entries {
                if !self.agents.contains_key(parent) {
                    continue;
                }
                let receipts = receipts
                    .as_arr()
                    .ok_or("agent launch receipts must be arrays")?;
                if receipts.len() > MAX_AGENT_LAUNCHES {
                    return Err("stored launch receipts exceed their bound".into());
                }
                let mut list = Vec::new();
                for receipt in receipts {
                    let tuple = receipt
                        .as_arr()
                        .filter(|tuple| tuple.len() == 4)
                        .ok_or("agent launch receipt must be [request, signature, child, at]")?;
                    let signature = tuple[1]
                        .as_str()
                        .filter(|value| {
                            value.len() == 16 && value.bytes().all(|b| b.is_ascii_hexdigit())
                        })
                        .ok_or("agent launch signature must be 16 hex characters")?;
                    list.push(AgentLaunchReceipt {
                        request: identifier(
                            &json::obj(vec![("request", tuple[0].clone())]),
                            "request",
                        )?,
                        signature: signature.to_owned(),
                        child: identifier(&json::obj(vec![("child", tuple[2].clone())]), "child")?,
                        at: tuple[3]
                            .as_u64()
                            .ok_or("agent launch receipt sequence must be an integer")?,
                    });
                }
                self.agent_launches.insert(parent.clone(), list);
            }
        }
        if let Some(results) = value.get("results") {
            let Value::Obj(entries) = results else {
                return Err("agent results must be an object".into());
            };
            for (id, message) in entries {
                if !self.agents.contains_key(id) {
                    continue;
                }
                self.agent_results
                    .insert(id.clone(), parse_agent_message(message)?);
            }
        }
        if let Some(terminals) = value.get("terminals") {
            let Value::Obj(entries) = terminals else {
                return Err("agent terminals must be an object".into());
            };
            for (id, fact) in entries {
                if !self.agents.contains_key(id) {
                    continue;
                }
                fields(
                    fact,
                    &[
                        "state",
                        "provider",
                        "session",
                        "conversation",
                        "pid",
                        "error",
                        "at",
                    ],
                    &["state", "at"],
                )?;
                let optional = |key: &str, max: usize| -> Result<Option<String>, String> {
                    match fact.get(key) {
                        None | Some(Value::Null) => Ok(None),
                        Some(_) => text(fact, key, max).map(Some),
                    }
                };
                self.agent_terminals.insert(
                    id.clone(),
                    AgentTerminalFact {
                        state: AgentTerminalState::parse(&text(fact, "state", 32)?)?,
                        provider: optional("provider", 16)?,
                        session: optional("session", 48)?,
                        conversation: optional("conversation", 128)?,
                        pid: match fact.get("pid") {
                            None | Some(Value::Null) => None,
                            Some(_) => Some(
                                u32::try_from(uint(fact, "pid")?).map_err(|_| "pid exceeds u32")?,
                            ),
                        },
                        error: optional("error", 1024)?,
                        at: uint(fact, "at")?,
                        // Loaded state is last-run metadata until observed.
                        live: false,
                    },
                );
            }
        }
        if let Some(grants) = value.get("grants") {
            let Value::Obj(entries) = grants else {
                return Err("agent grants must be an object".into());
            };
            for (child, grants) in entries {
                if !self.agents.contains_key(child) {
                    continue;
                }
                let grants = grants.as_arr().ok_or("agent grants must be arrays")?;
                if grants.len() > MAX_AGENT_GRANTS {
                    return Err("stored artifact grants exceed their bound".into());
                }
                let mut list = Vec::new();
                for grant in grants {
                    fields(
                        grant,
                        &[
                            "id",
                            "owner",
                            "origin_flow",
                            "artifact",
                            "commit",
                            "path",
                            "at",
                        ],
                        &[
                            "id",
                            "owner",
                            "origin_flow",
                            "artifact",
                            "commit",
                            "path",
                            "at",
                        ],
                    )?;
                    list.push(AgentGrant {
                        id: identifier(grant, "id")?,
                        owner: identifier(grant, "owner")?,
                        origin_flow: identifier(grant, "origin_flow")?,
                        artifact: identifier(grant, "artifact")?,
                        commit: text(grant, "commit", 96)?,
                        path: path(grant, "path")?,
                        child: child.clone(),
                        at: uint(grant, "at")?,
                    });
                }
                self.agent_grants.insert(child.clone(), list);
            }
        }
        Ok(())
    }

    fn agent_terminal_json(fact: Option<&AgentTerminalFact>) -> Value {
        let Some(fact) = fact else {
            return Value::Null;
        };
        json::obj(vec![
            ("state", json::s(fact.state.as_str())),
            ("provider", string_option(&fact.provider)),
            ("session", string_option(&fact.session)),
            ("conversation", string_option(&fact.conversation)),
            (
                "pid",
                fact.pid.map(|pid| n(pid.into())).unwrap_or(Value::Null),
            ),
            ("error", string_option(&fact.error)),
            ("at", n(fact.at)),
            ("live", Value::Bool(fact.live)),
        ])
    }

    /// Launch evidence summarised from the observed terminal fact. An active
    /// flow without a fact is only intent, so it reports pending; a fact from
    /// before this worker started is unverified until observed again.
    fn agent_launch_state(&self, node: &str) -> &'static str {
        if self.agent_terminal(node).is_some_and(|fact| !fact.live) {
            return "unverified";
        }
        match self.agent_terminal(node).map(|fact| fact.state) {
            Some(AgentTerminalState::Attached) | Some(AgentTerminalState::Detached) => "running",
            Some(AgentTerminalState::Ended) => "ended",
            Some(AgentTerminalState::Unavailable) => "failed",
            Some(AgentTerminalState::Stopping) => "stopping",
            Some(AgentTerminalState::Connecting) => "starting",
            Some(AgentTerminalState::WaitingForBinding) | None => "pending",
        }
    }

    /// Compact status for one agent; None for an unknown id.
    pub fn agent_status(&self, node: &str, title_max: usize) -> Option<Value> {
        let record = self.agents.get(node)?;
        let flow = self.agent_current_flow(node);
        let optional =
            |value: &Option<String>| value.as_deref().map(json::s).unwrap_or(Value::Null);
        let count = |state: TodoState| {
            n(flow
                .map(|flow| flow.todos.iter().filter(|todo| todo.state == state).count())
                .unwrap_or(0) as u64)
        };
        Some(json::obj(vec![
            ("node", json::s(node)),
            ("parent", optional(&record.parent)),
            ("order", n(record.order)),
            ("depth", n(self.agent_depth(node) as u64)),
            (
                "flow",
                flow.map(|flow| json::s(&flow.id)).unwrap_or(Value::Null),
            ),
            (
                "title",
                flow.map(|flow| json::s(excerpt(&flow.title, title_max)))
                    .unwrap_or(Value::Null),
            ),
            (
                "lifecycle",
                flow.map(|flow| json::s(flow.lifecycle.as_str()))
                    .unwrap_or_else(|| json::s("deleted")),
            ),
            (
                "status",
                flow.map(|flow| json::s(flow_status(flow)))
                    .unwrap_or_else(|| json::s("deleted")),
            ),
            ("launch", json::s(self.agent_launch_state(node))),
            (
                "terminal",
                Self::agent_terminal_json(self.agent_terminal(node)),
            ),
            (
                "provider",
                flow.and_then(|flow| flow.config.agent_provider.as_deref())
                    .map(json::s)
                    .unwrap_or(Value::Null),
            ),
            ("source", json::s(record.source.as_str())),
            ("tombstone", Value::Bool(record.tombstone)),
            (
                "child_ids",
                Value::Arr(
                    self.agent_children(Some(node))
                        .iter()
                        .map(|child| json::s(&child.id))
                        .collect(),
                ),
            ),
            ("live_children", n(self.live_children(node) as u64)),
            ("unread", n(self.agent_unread(node).len() as u64)),
            ("queued", n(self.agent_undelivered(node).len() as u64)),
            (
                "grants",
                Value::Arr(
                    self.agent_grants_of(node)
                        .iter()
                        .map(|grant| {
                            Value::Arr(vec![
                                json::s(&grant.id),
                                json::s(&grant.artifact),
                                json::s(&grant.owner),
                                json::s(&grant.commit),
                            ])
                        })
                        .collect(),
                ),
            ),
            (
                "todos",
                json::obj(vec![
                    ("q", count(TodoState::Queued)),
                    ("w", count(TodoState::Working)),
                    ("d", count(TodoState::Implemented)),
                    ("b", count(TodoState::Blocked)),
                ]),
            ),
            (
                "requirements",
                n(flow.map(|flow| flow.requirements.len()).unwrap_or(0) as u64),
            ),
            (
                "latest_result",
                self.agent_latest_result(node)
                    .map(|message| json::s(excerpt(&message.text, 256)))
                    .unwrap_or(Value::Null),
            ),
            ("request", optional(&record.request)),
            ("spawned_by", optional(&record.spawned_by)),
        ]))
    }

    /// The lanes as the global status lists them: one compact row each with
    /// the stable ids and states a manager needs to choose the lane to
    /// inspect, and nothing that grows with a lane's history. Active lanes
    /// come first and archived ones last, so a bounded reader keeps the live
    /// tree. Counts are returned separately and are always complete.
    pub fn status_index(&self) -> (Value, Vec<Value>) {
        let mut flows: Vec<&Flow> = self.flows.values().collect();
        flows.sort_by_key(|flow| {
            (
                match flow.lifecycle {
                    FlowLifecycle::Active => 0,
                    FlowLifecycle::Stopped => 1,
                    FlowLifecycle::Archived => 2,
                },
                flow.successor.is_some(),
            )
        });
        let rows = flows
            .into_iter()
            .map(|flow| {
                let node = self.agent_node(&flow.id);
                let mut fields = vec![
                    ("id", json::s(&flow.id)),
                    ("title", json::s(excerpt(&flow.title, 48))),
                    ("lifecycle", json::s(flow.lifecycle.as_str())),
                    ("status", json::s(flow_status(flow))),
                    ("provider", string_option(&flow.config.agent_provider)),
                    (
                        "agent",
                        node.map(|node| json::s(&node.id)).unwrap_or(Value::Null),
                    ),
                    (
                        "parent",
                        node.and_then(|node| node.parent.as_deref())
                            .map(json::s)
                            .unwrap_or(Value::Null),
                    ),
                ];
                if let Some(node) = node {
                    // The head of an agent carries its live evidence; an
                    // archived prefix of the same agent only names it.
                    if self
                        .agent_current_flow(&node.id)
                        .is_some_and(|head| head.id == flow.id)
                    {
                        fields.push(("source", json::s(node.source.as_str())));
                        fields.push(("launch", json::s(self.agent_launch_state(&node.id))));
                        fields.push((
                            "children",
                            n(self.agent_children(Some(&node.id)).len() as u64),
                        ));
                        fields.push(("unread", n(self.agent_unread(&node.id).len() as u64)));
                    }
                }
                if let Some(successor) = &flow.successor {
                    fields.push(("successor", json::s(successor)));
                }
                json::obj(fields)
            })
            .collect();
        let counts = json::obj(vec![
            ("revision", n(self.revision)),
            ("lanes", n(self.flows.len() as u64)),
            ("visible_count", n(self.visible_flow_count() as u64)),
            ("root_count", n(self.root_agent_count() as u64)),
            ("active_agents", n(self.active_agent_count() as u64)),
            (
                "archived_count",
                n(self
                    .flows
                    .values()
                    .filter(|flow| flow.lifecycle == FlowLifecycle::Archived)
                    .count() as u64),
            ),
        ]);
        (counts, rows)
    }

    /// Every agent with its status and child ids, plus the root order. The
    /// UI indexes this once per snapshot; selection state stays with the UI.
    pub fn agent_tree(&self, title_max: usize) -> Value {
        json::obj(vec![
            (
                "roots",
                Value::Arr(
                    self.agent_children(None)
                        .iter()
                        .map(|node| json::s(&node.id))
                        .collect(),
                ),
            ),
            (
                "nodes",
                Value::Arr(
                    self.agents
                        .keys()
                        .filter_map(|id| self.agent_status(id, title_max))
                        .collect(),
                ),
            ),
        ])
    }

    /// Read-only queries scoped to the caller's agent: itself, its parent and
    /// its direct children. Visibility never grants mutation authority.
    pub fn agent_query(
        &self,
        flow: &str,
        action: AgentQuery,
        child: Option<&str>,
    ) -> Result<Value, String> {
        self.flows.get(flow).ok_or("Unknown iteration flow")?;
        let node = self.terminal_origin(flow)?.to_owned();
        let me = self
            .agents
            .get(&node)
            .ok_or("this lane has no agent record")?;
        let visible = |target: &str| -> Result<(), String> {
            let record = self.agents.get(target).ok_or("unknown agent")?;
            if target == node
                || record.parent.as_deref() == Some(node.as_str())
                || me.parent.as_deref() == Some(target)
            {
                Ok(())
            } else {
                Err("only this lane, its parent and its direct children are visible here".into())
            }
        };
        match action {
            AgentQuery::Status => {
                let target = child.unwrap_or(&node);
                visible(target)?;
                let mut value = self.agent_status(target, 120).ok_or("unknown agent")?;
                if let Value::Obj(fields) = &mut value {
                    fields.push(("caller".into(), json::s(&node)));
                    fields.push((
                        "ancestors".into(),
                        Value::Arr(self.agent_ancestors(target).iter().map(json::s).collect()),
                    ));
                }
                Ok(value)
            }
            AgentQuery::Result => {
                let target = child.ok_or("result requires child=<agent id>")?;
                let record = self.agents.get(target).ok_or("unknown agent")?;
                if record.parent.as_deref() != Some(node.as_str()) {
                    return Err("result is readable for direct children only".into());
                }
                let current = self.agent_current_flow(target);
                let latest = self.agent_latest_result(target);
                Ok(json::obj(vec![
                    ("child", json::s(target)),
                    (
                        "status",
                        self.agent_status(target, 120).unwrap_or(Value::Null),
                    ),
                    (
                        "result",
                        latest
                            .map(|message| json::s(&message.text))
                            .unwrap_or(Value::Null),
                    ),
                    (
                        "result_id",
                        latest.map(|message| n(message.id)).unwrap_or(Value::Null),
                    ),
                    (
                        "u",
                        Value::Arr(
                            current
                                .map(|flow| {
                                    flow.todos
                                        .iter()
                                        .filter(|todo| todo.state != TodoState::Implemented)
                                        .map(|todo| {
                                            Value::Arr(vec![
                                                json::s(&todo.id),
                                                json::s(todo.state.as_str()),
                                                json::s(excerpt(&todo.text, 160)),
                                            ])
                                        })
                                        .collect()
                                })
                                .unwrap_or_default(),
                        ),
                    ),
                    (
                        "d",
                        Value::Arr(
                            current
                                .map(|flow| {
                                    flow.todos
                                        .iter()
                                        .filter(|todo| todo.state == TodoState::Implemented)
                                        .map(|todo| json::s(&todo.id))
                                        .collect()
                                })
                                .unwrap_or_default(),
                        ),
                    ),
                    (
                        "q",
                        Value::Arr(
                            current
                                .map(|flow| {
                                    flow.requirements
                                        .iter()
                                        .map(|requirement| {
                                            Value::Arr(vec![
                                                json::s(&requirement.id),
                                                json::s(excerpt(&requirement.text, 256)),
                                            ])
                                        })
                                        .collect()
                                })
                                .unwrap_or_default(),
                        ),
                    ),
                ]))
            }
            AgentQuery::List => Ok(json::obj(vec![
                ("caller", json::s(&node)),
                (
                    "ancestors",
                    Value::Arr(self.agent_ancestors(&node).iter().map(json::s).collect()),
                ),
                (
                    "children",
                    Value::Arr(
                        self.agent_children(Some(&node))
                            .iter()
                            .filter_map(|child| self.agent_status(&child.id, 80))
                            .collect(),
                    ),
                ),
            ])),
            AgentQuery::Inbox => {
                let unread = self.agent_unread(&node);
                Ok(json::obj(vec![
                    ("node", json::s(&node)),
                    ("count", n(unread.len() as u64)),
                    (
                        "unread",
                        Value::Arr(
                            unread
                                .iter()
                                .take(16)
                                .map(|message| {
                                    Value::Arr(vec![
                                        n(message.id),
                                        json::s(&message.from),
                                        json::s(message.kind.as_str()),
                                        json::s(&message.text),
                                        json::s(if message.delivered.is_some() {
                                            "delivered"
                                        } else {
                                            "queued"
                                        }),
                                    ])
                                })
                                .collect(),
                        ),
                    ),
                ]))
            }
        }
    }
}
