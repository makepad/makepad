// Included by iteration_worker.rs. A split transfers source ownership, never a process.
pub fn parse_clear_history_request(args: &Value) -> Result<Request, String> {
    let Value::Obj(fields) = args else {
        return Err("History clear arguments must be an object".into());
    };
    if fields.len() != 2
        || fields
            .iter()
            .any(|(key, _)| !["flow", "state"].contains(&key.as_str()))
        || args.get("state").and_then(Value::as_str) != Some("clear_history")
    {
        return Err("History clear accepts only flow and state=clear_history".into());
    }
    let flow = args
        .get("flow")
        .and_then(Value::as_str)
        .filter(|flow| cli_identifier(flow))
        .ok_or("Invalid flow identity")?;
    Ok(Request::ClearHistory { flow: flow.into() })
}

pub fn parse_split_request(args: &Value) -> Result<Request, String> {
    let Value::Obj(fields) = args else {
        return Err("Split arguments must be an object".into());
    };
    if fields
        .iter()
        .any(|(key, _)| !["flow", "item", "title", "state"].contains(&key.as_str()))
    {
        return Err("Split accepts flow, item and title".into());
    }
    if args
        .get("state")
        .is_some_and(|state| state.as_str() != Some("split"))
    {
        return Err("Split state must be split".into());
    }
    let text = |key: &str, max: usize| -> Result<String, String> {
        let value = args
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("Missing {key}"))?;
        if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
            return Err(format!("Invalid split {key}"));
        }
        Ok(value.into())
    };
    let flow = text("flow", 96)?;
    if !cli_identifier(&flow) {
        return Err("Invalid split flow".into());
    }
    Ok(Request::SplitLane {
        flow,
        item: text("item", 256)?,
        title: text("title", 240)?,
    })
}
impl Host {
    fn split_lane(&mut self, flow: &str, item: &str, title: &str) -> Result<Value, String> {
        parse_split_request(&json::obj(vec![
            ("flow", s(flow)),
            ("item", s(item)),
            ("title", s(title)),
        ]))?;
        self.require_active_lane(flow)?;
        if self.terminal_busy.contains(flow) {
            return Err(
                "Wait for the terminal stop/recovery/resume operation before splitting this lane"
                    .into(),
            );
        }
        if self.builds.contains_key(flow)
            || self.apps.contains_key(flow)
            || self.test_operations.contains_key(flow)
            || self
                .embedding_pending
                .values()
                .any(|pending| pending.flow == flow)
            || self
                .effects
                .iter()
                .any(|effect| effect_flow(effect) == flow)
        {
            return Err("Finish the lane's build/test/launch work and close its app before splitting history".into());
        }
        let owned = self.owned(flow)?;
        let history = self.split_history(flow)?;
        let previous = self.engine.clone();
        let transition = self.engine.observe(
            Observation::LaneSplit {
                flow: flow.into(),
                title: title.into(),
                item: item.into(),
                history,
            },
            now(),
        )?;
        let next = self.engine.resolve_active_flow(flow)?.to_owned();
        if let Err(error) = cli_control_dir(&self.directory, &next) {
            self.engine = previous;
            return Err(error);
        }
        let previous_heights = self.terminal_heights.clone();
        let previous_widths = self.lane_widths.clone();
        if let Some(height) = self.terminal_heights.remove(flow) {
            self.terminal_heights.insert(next.clone(), height);
        }
        if let Some(width) = self.lane_widths.remove(flow) {
            self.lane_widths.insert(next.clone(), width);
        }
        // Source, history and dimensions share one durable handoff boundary.
        if let Err(error) = self.transition(previous, transition) {
            self.terminal_heights = previous_heights;
            self.lane_widths = previous_widths;
            return Err(error);
        }
        if let Some(fingerprint) = self.fingerprints.remove(flow) {
            self.fingerprints.insert(next.clone(), fingerprint);
        }
        let current = &self.engine.flows[&next];
        self.note = format!(
            "{}: earlier history archived; the same terminal and source continue in {}",
            flow, next
        );
        self.changed = true;
        Ok(json::obj(vec![
            ("flow", s(&next)),
            ("predecessor", s(flow)),
            ("item", s(item)),
            ("terminal_origin", s(self.engine.terminal_origin(&next)?)),
            ("cwd", s(owned.path.to_string_lossy())),
            ("v", Value::Int(current.todos_revision as i64)),
            ("r", Value::Int(current.requirements_revision as i64)),
        ]))
    }
    fn clear_lane_history(&mut self, flow: &str) -> Result<Value, String> {
        self.require_active_lane(flow)?;
        let state = self.flow(flow)?;
        let live_artifacts: BTreeSet<_> = state
            .artifacts
            .iter()
            .filter(|artifact| Engine::artifact_in_use(state, artifact))
            .map(|artifact| format!("artifact/{}", artifact.id))
            .collect();
        let mut history = self.lane_history(flow, true)?;
        history.retain(|entry| !live_artifacts.contains(&entry.key));
        if let Some(job) = &state.job {
            let key = format!("job/{}", job.id);
            if matches!(
                job.phase,
                iteration::BuildPhase::Failed
                    | iteration::BuildPhase::LaunchFailed
                    | iteration::BuildPhase::Interrupted
                    | iteration::BuildPhase::Superseded
            ) && self.engine.history_visible(flow, &key)
            {
                history.push(iteration::HistoryEntry { key, at: now() });
            }
        }
        if history.is_empty() {
            return Ok(json::obj(vec![("cleared", Value::Int(0))]));
        }
        let count = history.len();
        let previous = self.engine.clone();
        let transition = self.engine.observe(
            Observation::HistoryCleared {
                flow: flow.into(),
                history,
            },
            now(),
        )?;
        self.transition(previous, transition)?;
        self.note = format!("{flow}: history cleared; terminal and current work continue");
        self.changed = true;
        Ok(json::obj(vec![("cleared", Value::Int(count as i64))]))
    }
    fn split_history(&self, flow: &str) -> Result<Vec<iteration::HistoryEntry>, String> {
        self.lane_history(flow, false)
    }
    fn lane_history(
        &self,
        flow: &str,
        clearing: bool,
    ) -> Result<Vec<iteration::HistoryEntry>, String> {
        let state = self.flow(flow)?;
        let mut history = Vec::<(u64, u8, usize, String)>::new();
        for (index, artifact) in state.artifacts.iter().enumerate() {
            let key = format!("artifact/{}", artifact.id);
            if !self.engine.history_visible(flow, &key) {
                continue;
            }
            let at = self
                .engine
                .events(flow)
                .find(|event| {
                    event.operation.get("observation").is_some_and(|o| {
                        o.get("kind").and_then(Value::as_str) == Some("build_succeeded")
                            && o.get("artifact_id").and_then(Value::as_str) == Some(&artifact.id)
                    })
                })
                .map(|event| event.at)
                .ok_or("Artifact has no durable creation event")?;
            history.push((at, 0, index, key));
        }
        for (index, feedback) in state.feedback.iter().enumerate() {
            let key = format!("feedback/{}", feedback.id);
            if !self.engine.history_visible(flow, &key) {
                continue;
            }
            let sequence = feedback
                .id
                .strip_prefix("feedback-")
                .and_then(|id| id.parse().ok())
                .ok_or("Invalid feedback identity")?;
            let at = self
                .engine
                .events(flow)
                .find(|event| event.sequence == sequence)
                .map(|event| event.at)
                .ok_or("Feedback has no durable creation event")?;
            history.push((at, 1, index, key));
        }
        for (index, attachment) in self
            .projected_attachments()
            .iter()
            .filter(|attachment| attachment.flow == flow)
            .enumerate()
        {
            if state
                .captures
                .iter()
                .any(|capture| capture.id == attachment.id)
                || (clearing && !attachment.submitted)
            {
                continue;
            }
            let at = attachment
                .id
                .split('-')
                .nth(1)
                .and_then(|at| at.parse().ok())
                .ok_or("Attachment has no creation timestamp")?;
            history.push((at, 2, index, format!("attachment/{}", attachment.id)));
        }
        // Metadata-only scan includes finalized evidence not currently expanded
        // or paged into the bounded thumbnail cache. No files are moved.
        let mut recordings = BTreeMap::new();
        for run in self.known_recording_runs() {
            if !self.engine.has_history_ancestor(flow, &run.flow) {
                continue;
            }
            for sidecar in recording_sidecars(&self.directory, &run)? {
                let tile = recording_tile(&self.directory, &run, &sidecar)?;
                let key = format!("recording/{}", tile.id);
                if self.engine.history_owner(&tile.flow, &key) != flow {
                    continue;
                }
                if !self.engine.history_visible(flow, &key) {
                    continue;
                }
                if tile.active {
                    if clearing {
                        continue;
                    }
                    return Err(
                        "Wait for the app recording to finalize before splitting its history"
                            .into(),
                    );
                }
                recordings.insert(tile.id.clone(), tile);
                if recordings.len() > RECORDING_MAX_TILES {
                    return Err("Recording history exceeds the split bound".into());
                }
            }
        }
        let mut recordings: Vec<_> = recordings.into_values().collect();
        recordings.sort_by(|a, b| b.sidecar_path.cmp(&a.sidecar_path));
        for (index, tile) in recordings.iter().enumerate() {
            let at = tile
                .run
                .split('-')
                .nth(1)
                .and_then(|at| at.parse().ok())
                .unwrap_or(0);
            history.push((at, 3, index, format!("recording/{}", tile.id)));
        }
        history.sort_by_key(|(at, kind, index, _)| (*at, *kind, *index));
        Ok(history
            .into_iter()
            .map(|(at, _, _, key)| iteration::HistoryEntry { key, at })
            .collect())
    }
    fn attachment_owner<'a>(&'a self, attachment: &'a Attachment) -> &'a str {
        if let Some((_, origin, key)) = self
            .engine
            .flows
            .values()
            .flat_map(|flow| {
                flow.feedback.iter().filter_map(move |feedback| {
                    feedback
                        .feedback
                        .evidence
                        .iter()
                        .any(|evidence| evidence.capture_id == attachment.id)
                        .then(|| {
                            (
                                feedback
                                    .id
                                    .strip_prefix("feedback-")
                                    .and_then(|id| id.parse::<u64>().ok())
                                    .unwrap_or(0),
                                flow.id.as_str(),
                                format!("feedback/{}", feedback.id),
                            )
                        })
                })
            })
            .max_by_key(|(sequence, _, _)| *sequence)
        {
            return self.engine.history_owner(origin, &key);
        }
        self.engine
            .history_owner(&attachment.flow, &format!("attachment/{}", attachment.id))
    }
    fn recording_owner<'a>(&'a self, tile: &'a RecordingTile) -> &'a str {
        self.engine
            .history_owner(&tile.flow, &format!("recording/{}", tile.id))
    }
    fn projected_attachments(&self) -> Vec<Attachment> {
        self.attachments
            .iter()
            .filter(|original| {
                self.engine
                    .flows
                    .contains_key(self.attachment_owner(original))
                    && self.engine.history_visible(
                        self.attachment_owner(original),
                        &format!("attachment/{}", original.id),
                    )
            })
            .map(|original| {
                let mut attachment = original.clone();
                attachment.flow = self.attachment_owner(original).to_owned();
                attachment
            })
            .collect()
    }
    fn projected_recordings(&self) -> Vec<RecordingTile> {
        self.recordings
            .snapshot()
            .into_iter()
            .filter(|tile| {
                self.engine.flows.contains_key(self.recording_owner(tile))
                    && self.engine.history_visible(
                        self.recording_owner(tile),
                        &format!("recording/{}", tile.id),
                    )
            })
            .map(|mut tile| {
                tile.flow = self
                    .engine
                    .history_owner(&tile.flow, &format!("recording/{}", tile.id))
                    .to_owned();
                tile
            })
            .collect()
    }
    fn projected_reports(&self) -> Vec<Value> {
        self.reports
            .values()
            .cloned()
            .map(|mut report| {
                let owner = report
                    .get("flow")
                    .and_then(Value::as_str)
                    .zip(report.get("artifact").and_then(Value::as_str))
                    .map(|(flow, artifact)| {
                        self.engine
                            .history_owner(flow, &format!("artifact/{artifact}"))
                            .to_owned()
                    });
                if let (Some(owner), Value::Obj(fields)) = (owner, &mut report) {
                    if let Some((_, flow)) = fields.iter_mut().find(|(key, _)| key == "flow") {
                        *flow = s(owner);
                    }
                }
                report
            })
            .collect()
    }
}

fn flow_split_tool_def() -> makepad_ai_services::wire::ToolDef {
    makepad_ai_services::wire::ToolDef::new("flow_lane", "Split at a history item: move it and newer activity to a new titled lane, retaining the same live terminal/source and archiving the prefix. Close apps and finish builds first. Use state=clear_history to clear previous timeline entries while retaining the live terminal, tasks, media files and checkpoints. Requires user authorization.", r#"{"type":"object","properties":{"flow":{"type":"string","maxLength":96},"state":{"type":"string","enum":["split","clear_history"]},"item":{"type":"string","maxLength":256},"title":{"type":"string","maxLength":240}},"required":["flow","state"],"additionalProperties":false}"#, makepad_ai_services::wire::Risk::Act)
}
