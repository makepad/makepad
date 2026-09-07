// Included in iteration_worker.rs. Only the worker starts and owns children;
// the UI owns the protocol presentation and acknowledges host registration.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbeddedRun {
    pub flow: String,
    pub run_id: String,
    pub artifact_id: String,
    pub client: u64,
    pub port: u16,
    pub pid: Option<u32>,
    pub pending: bool,
}

struct PendingEmbedded {
    flow: String,
    artifact: String,
    job: String,
    path: PathBuf,
    run: String,
    created: Instant,
    registration_sent: bool,
}

fn fresh_run_identity() -> (String, u64) {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let stamp = now();
    // Fits both the protocol's u64 and JSON's i64, without ever depending on
    // a user-chosen flow/job string to fit the durable 96-byte identifier cap.
    let client = stamp.saturating_mul(4096).saturating_add(sequence % 4096).max(1);
    (format!("run-{stamp}-p{}-{sequence}", std::process::id()), client)
}

impl Host {
    fn host_port(&mut self, port: u16) -> Result<Value, String> {
        if port == 0 { return Err("Iteration host did not bind a real port".into()); }
        if self.embedding_port.is_some_and(|existing| existing != port)
            && (!self.embedding_pending.is_empty() || self.apps.values().any(|run| run.embedding_client.is_some())) {
            return Err("Cannot replace the embedding host while it owns pending or live app runs".into());
        }
        self.embedding_port = Some(port);
        self.changed = true;
        self.poll_embedding();
        Ok(json::obj(vec![("port", Value::Int(i64::from(port)))]))
    }

    fn queue_embedded(&mut self, flow: &str, artifact: &str, path: &Path) -> Result<(), String> {
        self.require_active_lane(flow)?;
        if self.apps.contains_key(flow) || self.embedding_pending.values().any(|pending| pending.flow == flow) {
            return Err("Close the current app before opening another revision".into());
        }
        if self.embedding_pending.len() >= 4 || self.embedding_unregister.len() >= 32 {
            return Err("The bounded embedding queue is full; wait for an owned app to finish".into());
        }
        let state = self.flow(flow)?;
        let selected = state.artifacts.iter().find(|selected| selected.id == artifact && selected.path == path)
            .ok_or("Unknown immutable artifact")?;
        if !state.job.as_ref().is_some_and(|job| job.id == selected.job_id && job.phase == iteration::BuildPhase::Succeeded) {
            return Err("Artifact no longer matches the build awaiting launch".into());
        }
        let job = selected.job_id.clone();
        let _ = self.owned(flow)?;
        let actual = fs::canonicalize(path).map_err(err)?;
        let root = fs::canonicalize(self.directory.join("artifacts").join(flow).join(artifact)).map_err(err)?;
        if !actual.starts_with(root) || !actual.is_file() { return Err("Artifact executable escapes its retained directory".into()); }
        let (run, client) = loop {
            let (run, client) = fresh_run_identity();
            if !self.embedding_pending.contains_key(&client) && !self.embedding_unregister.contains(&client)
                && !self.apps.values().any(|run| run.embedding_client == Some(client)) { break (run, client); }
        };
        self.embedding_pending.insert(client, PendingEmbedded {
            flow: flow.into(), artifact: artifact.into(), job, path: path.to_path_buf(), run,
            created: Instant::now(), registration_sent: false,
        });
        self.note = format!("{flow}: preparing the embedded surface for the retained artifact");
        self.changed = true;
        Ok(())
    }

    fn host_registered(&mut self, client: u64) -> Result<Value, String> {
        let Some(pending) = self.embedding_pending.remove(&client) else {
            if let Some(run) = self.apps.values().find(|run| run.embedding_client == Some(client)) {
                return Ok(json::obj(vec![("run", s(&run.run)), ("already_started", Value::Bool(true))]));
            }
            // Registration may finish just after the human canceled a launch.
            if self.embedding_unregister.len() >= 32 { return Err("Unknown embedding registration; cleanup queue is full".into()); }
            self.embedding_unregister.insert(client);
            return Ok(json::obj(vec![("canceled", Value::Bool(true))]));
        };
        if !pending.registration_sent {
            self.embedding_pending.insert(client, pending);
            return Err("The broker has not received this launch registration".into());
        }
        let outcome: Result<Value, String> = (|| {
            self.require_active_lane(&pending.flow)?;
            let port = self.embedding_port.ok_or("Embedding host is unavailable")?;
            let state = self.flow(&pending.flow)?;
            if !state.job.as_ref().is_some_and(|job| job.id == pending.job && job.phase == iteration::BuildPhase::Succeeded) {
                return Err("Embedded launch was superseded before its process started".into());
            }
            self.spawn_artifact(&pending.flow, &pending.artifact, &pending.path,
                iteration::LaunchMode::Embedded, false, Some((pending.run.clone(), client, port)))?;
            Ok(json::obj(vec![("flow", s(&pending.flow)), ("run", s(&pending.run)), ("client", Value::Int(client as i64))]))
        })();
        if let Err(error) = &outcome {
            self.embedding_unregister.insert(client);
            self.embedded_failure(&pending, error);
        }
        self.changed = true;
        outcome
    }

    fn poll_embedding(&mut self) {
        let retiring = self.embedding_unregister.iter().copied().collect::<Vec<_>>();
        for client in retiring {
            match self.embedding_commands.try_send(crate::iteration_host::HostCommand::Unregister { client }) {
                Ok(()) => { self.embedding_unregister.remove(&client); },
                Err(TrySendError::Full(_)) => return,
                Err(TrySendError::Disconnected(_)) => {
                    self.embedding_unregister.clear();
                    break;
                }
            }
        }
        let clients = self.embedding_pending.keys().copied().collect::<Vec<_>>();
        for client in clients {
            let Some(mut pending) = self.embedding_pending.remove(&client) else { continue; };
            let expected = self.require_active_lane(&pending.flow).is_ok()
                && self.engine.flows.get(&pending.flow).and_then(|flow| flow.job.as_ref())
                    .is_some_and(|job| job.id == pending.job && job.phase == iteration::BuildPhase::Succeeded);
            let error = if !expected {
                Some("Embedded launch was superseded before its process started".to_owned())
            } else if pending.created.elapsed() > Duration::from_secs(20) {
                Some("Embedded launch timed out waiting for the Studio host registration".to_owned())
            } else if self.embedding_port.is_some() && !pending.registration_sent {
                match self.embedding_commands.try_send(crate::iteration_host::HostCommand::Register { client, run_id: pending.run.clone() }) {
                    Ok(()) => { pending.registration_sent = true; None },
                    Err(TrySendError::Full(_)) => None,
                    Err(TrySendError::Disconnected(_)) => Some("Embedding broker stopped before the app could launch".into()),
                }
            } else { None };
            if let Some(error) = error {
                if pending.registration_sent { self.embedding_unregister.insert(client); }
                self.embedded_failure(&pending, &error);
                self.changed = true;
            } else {
                self.embedding_pending.insert(client, pending);
            }
        }
    }

    fn embedded_failure(&mut self, pending: &PendingEmbedded, error: &str) {
        self.note = format!("{}: {error}", pending.flow);
        let awaiting = self.engine.flows.get(&pending.flow).is_some_and(|flow| {
            flow.job.as_ref().is_some_and(|job| job.id == pending.job && job.phase == iteration::BuildPhase::Succeeded)
                && !flow.runs.iter().any(|run| run.artifact_id == pending.artifact)
        });
        if awaiting {
            if let Err(persistence) = self.observe(Observation::LaunchFailed {
                flow: pending.flow.clone(), job_id: pending.job.clone(), artifact_id: pending.artifact.clone(), error: error.into(),
            }) {
                self.note = format!("{}: {error}; launch evidence could not be saved: {persistence}", pending.flow);
            }
        }
        self.changed = true;
    }

    fn cancel_pending_embedding(&mut self, flow: &str, reason: &str) -> bool {
        let client = self.embedding_pending.iter().find_map(|(client, pending)| (pending.flow == flow).then_some(*client));
        let Some(client) = client else { return false; };
        let pending = self.embedding_pending.remove(&client).unwrap();
        if pending.registration_sent { self.embedding_unregister.insert(client); }
        self.embedded_failure(&pending, reason);
        true
    }

    fn stop_embedding(&mut self) {
        let flows = self.embedding_pending.values().map(|pending| pending.flow.clone()).collect::<Vec<_>>();
        for flow in flows { self.cancel_pending_embedding(&flow, "Studio stopped before this app launched"); }
        self.poll_embedding();
    }

    fn embed_summary(&self) -> Vec<EmbeddedRun> {
        let mut summaries = Vec::new();
        if let Some(port) = self.embedding_port {
            for (client, pending) in &self.embedding_pending {
                summaries.push(EmbeddedRun {
                    flow: pending.flow.clone(), run_id: pending.run.clone(), artifact_id: pending.artifact.clone(),
                    client: *client, port, pid: None, pending: true,
                });
            }
        }
        for (flow, run) in &self.apps {
            if let (Some(client), Some(port)) = (run.embedding_client, run.embedding_port) {
                summaries.push(EmbeddedRun {
                    flow: flow.clone(), run_id: run.run.clone(), artifact_id: run.artifact.clone(),
                    client, port, pid: Some(run.child.id()), pending: false,
                });
            }
        }
        summaries.sort_by(|a, b| a.flow.cmp(&b.flow));
        summaries
    }
}
