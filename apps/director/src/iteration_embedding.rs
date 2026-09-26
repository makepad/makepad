// Included in iteration_worker.rs. Only the worker starts and owns children;
// the UI owns the protocol presentation and acknowledges host registration.
// A pending launch keeps its role: a human evaluation build, or an AI test of
// an eligible retained artifact. Neither fabricates a build record.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbeddedRun {
    pub flow: String,
    pub run_id: String,
    pub artifact_id: String,
    pub client: u64,
    pub port: u16,
    pub pid: Option<u32>,
    pub pending: bool,
    /// AI-owned previews are read-only in the UI; human runs take input.
    pub role: iteration::RunRole,
    /// The test operation that asked for this launch, if any.
    pub operation: Option<String>,
    /// Display title of a demonstration test, if any.
    pub demo: Option<String>,
}

/// An AI test app the worker owns or is about to launch, for the lane's
/// watch-only item. It is a run of that lane, never one of its builds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveTestRun {
    pub flow: String,
    pub run_id: String,
    pub artifact_id: String,
    pub commit: Option<String>,
    /// (grant id, owner agent) when the executable is a parent's.
    pub grant: Option<(String, String)>,
    pub demo: Option<String>,
    /// Waiting for the host registration; no process exists yet.
    pub pending: bool,
    pub hosted: bool,
    pub first_frame: bool,
    /// Why automation let go of the app, once a person took it over.
    pub handed_off: Option<String>,
}

struct PendingEmbedded {
    flow: String,
    artifact: String,
    /// The build job whose launch this is; None for an AI test, which may use
    /// any eligible retained artifact.
    job: Option<String>,
    path: PathBuf,
    run: String,
    created: Instant,
    registration_sent: bool,
    role: iteration::RunRole,
    operation: Option<String>,
    demo: Option<String>,
    grant: Option<String>,
}

fn fresh_run_identity() -> (String, u64) {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let stamp = now();
    // Fits both the protocol's u64 and JSON's i64, without ever depending on
    // a user-chosen flow/job string to fit the durable 96-byte identifier cap.
    let client = stamp
        .saturating_mul(4096)
        .saturating_add(sequence % 4096)
        .max(1);
    (
        format!("run-{stamp}-p{}-{sequence}", std::process::id()),
        client,
    )
}

impl Host {
    /// One budget for every app process, running or pending launch, embedded
    /// or standalone. It is a GPU surface and process limit, separate from
    /// how many agents exist, and nothing launches around it.
    fn app_budget(&self) -> Result<(), String> {
        let running = self.apps.len();
        let pending = self.embedding_pending.len();
        if running + pending >= iteration::MAX_HOSTED_APPS {
            return Err(format!(
                "App budget is full: {running} running and {pending} pending of {} allowed across all lanes; close an app or stop a test first",
                iteration::MAX_HOSTED_APPS
            ));
        }
        Ok(())
    }

    fn host_port(&mut self, port: u16) -> Result<Value, String> {
        if port == 0 {
            return Err("Iteration host did not bind a real port".into());
        }
        if self.embedding_port.is_some_and(|existing| existing != port)
            && (!self.embedding_pending.is_empty()
                || self.apps.values().any(|run| run.embedding_client.is_some()))
        {
            return Err(
                "Cannot replace the embedding host while it owns pending or live app runs".into(),
            );
        }
        self.embedding_port = Some(port);
        self.changed = true;
        self.poll_embedding();
        Ok(json::obj(vec![("port", Value::Int(i64::from(port)))]))
    }

    fn fresh_pending_identity(&self) -> (String, u64) {
        loop {
            let (run, client) = fresh_run_identity();
            if !self.embedding_pending.contains_key(&client)
                && !self.embedding_unregister.contains(&client)
                && !self
                    .apps
                    .values()
                    .any(|run| run.embedding_client == Some(client))
            {
                break (run, client);
            }
        }
    }

    fn queue_embedded(&mut self, flow: &str, artifact: &str, path: &Path) -> Result<(), String> {
        self.require_active_lane(flow)?;
        if self.apps.contains_key(flow)
            || self
                .embedding_pending
                .values()
                .any(|pending| pending.flow == flow)
        {
            return Err("Close the current app before opening another revision".into());
        }
        self.app_budget()?;
        if self.embedding_unregister.len() >= 32 {
            return Err(
                "The bounded embedding queue is full; wait for an owned app to finish".into(),
            );
        }
        let state = self.flow(flow)?;
        let selected = state
            .artifacts
            .iter()
            .find(|selected| selected.id == artifact && selected.path == path)
            .ok_or("Unknown immutable artifact")?;
        if !state.job.as_ref().is_some_and(|job| {
            job.id == selected.job_id && job.phase == iteration::BuildPhase::Succeeded
        }) {
            return Err("Artifact no longer matches the build awaiting launch".into());
        }
        let job = selected.job_id.clone();
        let _ = self.owned(flow)?;
        let actual = fs::canonicalize(path).map_err(err)?;
        let root = fs::canonicalize(self.directory.join("artifacts").join(flow).join(artifact))
            .map_err(err)?;
        if !actual.starts_with(root) || !actual.is_file() {
            return Err("Artifact executable escapes its retained directory".into());
        }
        let (run, client) = self.fresh_pending_identity();
        self.embedding_pending.insert(
            client,
            PendingEmbedded {
                flow: flow.into(),
                artifact: artifact.into(),
                job: Some(job),
                path: path.to_path_buf(),
                run,
                created: Instant::now(),
                registration_sent: false,
                role: iteration::RunRole::Human,
                operation: None,
                demo: None,
                grant: None,
            },
        );
        self.note = format!("{flow}: preparing the embedded surface for the retained artifact");
        self.changed = true;
        Ok(())
    }

    /// Queue a hosted AI test of an eligible retained artifact (the lane's
    /// own, or one its parent granted). The run identity exists from here on
    /// and is inspectable while the broker registration is still pending.
    fn queue_embedded_test(
        &mut self,
        flow: &str,
        retained: &RetainedArtifact,
        operation: &str,
        demo: Option<&str>,
    ) -> Result<String, String> {
        self.require_active_lane(flow)?;
        if self.apps.contains_key(flow)
            || self
                .embedding_pending
                .values()
                .any(|pending| pending.flow == flow)
        {
            return Err("Close the current app before opening another revision".into());
        }
        self.app_budget()?;
        if self.embedding_unregister.len() >= 32 {
            return Err(
                "The bounded embedding queue is full; wait for an owned app to finish".into(),
            );
        }
        let (run, client) = self.fresh_pending_identity();
        self.embedding_pending.insert(
            client,
            PendingEmbedded {
                flow: flow.into(),
                artifact: retained.id.clone(),
                job: None,
                path: retained.path.clone(),
                run: run.clone(),
                created: Instant::now(),
                registration_sent: false,
                role: iteration::RunRole::AiTest,
                operation: Some(operation.into()),
                demo: demo.map(str::to_owned),
                grant: retained.grant.as_ref().map(|grant| grant.id.clone()),
            },
        );
        self.note = format!("{flow}: preparing the hosted surface for an AI test");
        self.changed = true;
        Ok(run)
    }

    fn pending_expected(&self, pending: &PendingEmbedded) -> bool {
        if self.require_active_lane(&pending.flow).is_err() {
            return false;
        }
        match &pending.job {
            Some(job) => self
                .engine
                .flows
                .get(&pending.flow)
                .and_then(|flow| flow.job.as_ref())
                .is_some_and(|current| {
                    &current.id == job && current.phase == iteration::BuildPhase::Succeeded
                }),
            // An AI test needs only its lane still active and no other run.
            None => self
                .engine
                .flows
                .get(&pending.flow)
                .is_some_and(|flow| flow.runs.iter().all(|run| run.closed)),
        }
    }

    fn host_registered(&mut self, client: u64) -> Result<Value, String> {
        let Some(pending) = self.embedding_pending.remove(&client) else {
            if let Some(run) = self
                .apps
                .values()
                .find(|run| run.embedding_client == Some(client))
            {
                return Ok(json::obj(vec![
                    ("run", s(&run.run)),
                    ("already_started", Value::Bool(true)),
                ]));
            }
            // Registration may finish just after the human canceled a launch.
            if self.embedding_unregister.len() >= 32 {
                return Err("Unknown embedding registration; cleanup queue is full".into());
            }
            self.embedding_unregister.insert(client);
            return Ok(json::obj(vec![("canceled", Value::Bool(true))]));
        };
        if !pending.registration_sent {
            self.embedding_pending.insert(client, pending);
            return Err("The broker has not received this launch registration".into());
        }
        let outcome: Result<Value, String> = (|| {
            let port = self.embedding_port.ok_or("Embedding host is unavailable")?;
            if !self.pending_expected(&pending) {
                return Err("Embedded launch was superseded before its process started".into());
            }
            self.spawn_app(AppLaunch {
                flow: &pending.flow,
                artifact: &pending.artifact,
                path: &pending.path,
                mode: iteration::LaunchMode::Embedded,
                requested: iteration::LaunchMode::Embedded,
                reopening: false,
                embedded: Some((pending.run.clone(), client, port)),
                role: pending.role,
                grant: pending.grant.as_deref(),
                demo: pending.demo.as_deref(),
            })?;
            Ok(json::obj(vec![
                ("flow", s(&pending.flow)),
                ("run", s(&pending.run)),
                ("client", Value::Int(client as i64)),
            ]))
        })();
        if let Err(error) = &outcome {
            self.embedding_unregister.insert(client);
            self.embedded_failure(&pending, error);
        }
        self.changed = true;
        outcome
    }

    /// The UI hosts one view per app. Windows beyond it exist but are not
    /// shown or recorded, which test results must say rather than hide.
    fn host_windows(&mut self, client: u64, extra: usize) -> Result<Value, String> {
        let run = self
            .apps
            .values_mut()
            .find(|run| run.embedding_client == Some(client))
            .ok_or("Unknown hosted app")?;
        if run.extra_windows != extra {
            run.extra_windows = extra.min(64);
            self.changed = true;
        }
        Ok(Value::Bool(true))
    }

    /// The host transport presented this app's first valid frame. A remote
    /// port alone does not prove a connected, rendering hosted surface.
    fn host_frame(&mut self, client: u64) -> Result<Value, String> {
        let run = self
            .apps
            .values_mut()
            .find(|run| run.embedding_client == Some(client))
            .ok_or("Unknown hosted app")?;
        if !run.first_frame {
            run.first_frame = true;
            self.changed = true;
        }
        Ok(Value::Bool(true))
    }

    fn test_summary(&self) -> Vec<LiveTestRun> {
        let provenance = |flow: &str, artifact: &str, grant: Option<&str>| {
            let grant = grant.and_then(|id| self.engine.agent_grant_by_id(flow, id));
            let commit = grant.map(|grant| grant.commit.clone()).or_else(|| {
                self.engine
                    .flows
                    .get(flow)?
                    .artifacts
                    .iter()
                    .find(|item| item.id == artifact)
                    .map(|item| item.commit.clone())
            });
            (
                commit,
                grant.map(|grant| (grant.id.clone(), grant.owner.clone())),
            )
        };
        let mut tests = Vec::new();
        for pending in self
            .embedding_pending
            .values()
            .filter(|pending| pending.role == iteration::RunRole::AiTest)
        {
            let (commit, grant) =
                provenance(&pending.flow, &pending.artifact, pending.grant.as_deref());
            tests.push(LiveTestRun {
                flow: pending.flow.clone(),
                run_id: pending.run.clone(),
                artifact_id: pending.artifact.clone(),
                commit,
                grant,
                demo: pending.demo.clone(),
                pending: true,
                hosted: true,
                first_frame: false,
                handed_off: None,
            });
        }
        for (flow, run) in self
            .apps
            .iter()
            .filter(|(_, run)| run.role == iteration::RunRole::AiTest)
        {
            let (commit, grant) = provenance(flow, &run.artifact, run.grant.as_deref());
            tests.push(LiveTestRun {
                flow: flow.clone(),
                run_id: run.run.clone(),
                artifact_id: run.artifact.clone(),
                commit,
                grant,
                demo: run.demo.clone(),
                pending: false,
                hosted: run.mode == iteration::LaunchMode::Embedded,
                first_frame: run.first_frame,
                handed_off: run.handed_off.then(|| {
                    run.handoff_reason
                        .clone()
                        .unwrap_or_else(|| "a person is using it".into())
                }),
            });
        }
        tests.sort_by(|a, b| a.flow.cmp(&b.flow));
        tests
    }

    fn poll_embedding(&mut self) {
        let retiring = self
            .embedding_unregister
            .iter()
            .copied()
            .collect::<Vec<_>>();
        for client in retiring {
            match self
                .embedding_commands
                .try_send(crate::iteration_host::HostCommand::Unregister { client })
            {
                Ok(()) => {
                    self.embedding_unregister.remove(&client);
                }
                Err(TrySendError::Full(_)) => return,
                Err(TrySendError::Disconnected(_)) => {
                    self.embedding_unregister.clear();
                    break;
                }
            }
        }
        let clients = self.embedding_pending.keys().copied().collect::<Vec<_>>();
        for client in clients {
            let Some(mut pending) = self.embedding_pending.remove(&client) else {
                continue;
            };
            let error = if !self.pending_expected(&pending) {
                Some("Embedded launch was superseded before its process started".to_owned())
            } else if pending.created.elapsed() > Duration::from_secs(20) {
                Some(
                    "Embedded launch timed out waiting for the Studio host registration".to_owned(),
                )
            } else if self.embedding_port.is_some() && !pending.registration_sent {
                match self.embedding_commands.try_send(
                    crate::iteration_host::HostCommand::Register {
                        client,
                        run_id: pending.run.clone(),
                    },
                ) {
                    Ok(()) => {
                        pending.registration_sent = true;
                        None
                    }
                    Err(TrySendError::Full(_)) => None,
                    Err(TrySendError::Disconnected(_)) => {
                        Some("Embedding broker stopped before the app could launch".into())
                    }
                }
            } else {
                None
            };
            if let Some(error) = error {
                if pending.registration_sent {
                    self.embedding_unregister.insert(client);
                }
                self.embedded_failure(&pending, &error);
                self.changed = true;
            } else {
                self.embedding_pending.insert(client, pending);
            }
        }
    }

    fn embedded_failure(&mut self, pending: &PendingEmbedded, error: &str) {
        self.note = format!("{}: {error}", pending.flow);
        // A hosted AI test that never launched is resolved by its own test
        // operation (bounded fallback or failure); it has no build job.
        if let Some(operation) = &pending.operation {
            self.reports.insert(
                format!("test-launch:{operation}"),
                json::obj(vec![
                    ("kind", s("ai_test_launch")),
                    ("flow", s(&pending.flow)),
                    ("run_id", s(&pending.run)),
                    ("operation_id", s(operation)),
                    ("error", s(error)),
                ]),
            );
        }
        let awaiting = pending.job.as_ref().is_some_and(|job| {
            self.engine.flows.get(&pending.flow).is_some_and(|flow| {
                flow.job.as_ref().is_some_and(|current| {
                    &current.id == job && current.phase == iteration::BuildPhase::Succeeded
                }) && !flow
                    .runs
                    .iter()
                    .any(|run| run.artifact_id == pending.artifact)
            })
        });
        if awaiting {
            if let Err(persistence) = self.observe(Observation::LaunchFailed {
                flow: pending.flow.clone(),
                job_id: pending.job.clone().unwrap_or_default(),
                artifact_id: pending.artifact.clone(),
                error: error.into(),
            }) {
                self.note = format!(
                    "{}: {error}; launch evidence could not be saved: {persistence}",
                    pending.flow
                );
            }
        }
        self.changed = true;
    }

    fn cancel_pending_embedding(&mut self, flow: &str, reason: &str) -> bool {
        let client = self
            .embedding_pending
            .iter()
            .find_map(|(client, pending)| (pending.flow == flow).then_some(*client));
        let Some(client) = client else {
            return false;
        };
        let pending = self.embedding_pending.remove(&client).unwrap();
        if pending.registration_sent {
            self.embedding_unregister.insert(client);
        }
        self.embedded_failure(&pending, reason);
        true
    }

    fn stop_embedding(&mut self) {
        let flows = self
            .embedding_pending
            .values()
            .map(|pending| pending.flow.clone())
            .collect::<Vec<_>>();
        for flow in flows {
            self.cancel_pending_embedding(&flow, "Studio stopped before this app launched");
        }
        self.poll_embedding();
    }

    fn embed_summary(&self) -> Vec<EmbeddedRun> {
        let mut summaries = Vec::new();
        if let Some(port) = self.embedding_port {
            for (client, pending) in &self.embedding_pending {
                summaries.push(EmbeddedRun {
                    flow: pending.flow.clone(),
                    run_id: pending.run.clone(),
                    artifact_id: pending.artifact.clone(),
                    client: *client,
                    port,
                    pid: None,
                    pending: true,
                    role: pending.role,
                    operation: pending.operation.clone(),
                    demo: pending.demo.clone(),
                });
            }
        }
        for (flow, run) in &self.apps {
            if let (Some(client), Some(port)) = (run.embedding_client, run.embedding_port) {
                summaries.push(EmbeddedRun {
                    flow: flow.clone(),
                    run_id: run.run.clone(),
                    artifact_id: run.artifact.clone(),
                    client,
                    port,
                    pid: Some(run.child.id()),
                    pending: false,
                    role: run.role,
                    operation: None,
                    demo: None,
                });
            }
        }
        summaries.sort_by(|a, b| a.flow.cmp(&b.flow));
        summaries
    }
}
