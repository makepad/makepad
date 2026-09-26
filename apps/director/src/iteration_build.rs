// Included in iteration_worker.rs: all process ownership stays on its worker.

const BUILD_LOG_LIMIT: u64 = 32 * 1024 * 1024;
const APP_LOG_LIMIT: u64 = 32 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum PhaseKind {
    Check,
    Format,
    Binary,
    Test,
}
#[derive(Clone)]
struct BuildPhase {
    kind: PhaseKind,
    label: String,
    program: String,
    args: Vec<String>,
}
struct Build {
    job: String,
    commit: String,
    child: Option<Child>,
    phases: VecDeque<BuildPhase>,
    active: Option<BuildPhase>,
    started: Instant,
    phase_started: Instant,
    directory: PathBuf,
    log: PathBuf,
    manifest: PathBuf,
    owned: RepoCheckout,
    config: iteration::FlowConfig,
    host_target: String,
    target_directory: PathBuf,
    artifact_id: String,
    executable: Option<PathBuf>,
    expected_source: String,
    tainted: Option<String>,
    failed_exit: Option<i32>,
    preflight: bool,
    formatted_changed: bool,
    rechecked: bool,
    results: Vec<Value>,
    lease: Option<CargoLease>,
}
struct AppRun {
    child: Child,
    role: iteration::RunRole,
    run: String,
    artifact: String,
    directory: PathBuf,
    log: PathBuf,
    closing: Option<Instant>,
    port: Option<u16>,
    grab_directory: Option<PathBuf>,
    user_closed: bool,
    log_offset: u64,
    log_partial: String,
    close_request: Option<Child>,
    close_stage: u8,
    exit: Option<std::process::ExitStatus>,
    mode: iteration::LaunchMode,
    pop_out: bool,
    embedding_client: Option<u64>,
    embedding_port: Option<u16>,
    /// The mode the caller asked for; `mode` is what actually launched.
    requested: iteration::LaunchMode,
    /// Parent-artifact grant this run executes under, if any.
    grant: Option<String>,
    /// The person's input counter read once at test start. Every automated
    /// mutation carries it; a changed counter or a 409 hands the app over.
    user_seq: Option<u64>,
    /// The human interacted during an AI test: automation stopped and the
    /// instance stays running until they close it.
    handed_off: bool,
    /// Windows the app opened beyond the one hosted view; not observed.
    extra_windows: usize,
    /// Why automation let go of this app, when it did.
    handoff_reason: Option<String>,
    /// The person asked Director to close this app. Only that request closes
    /// a handed-off app; automatic cleanup never sets it.
    operator_close: bool,
    /// A guarded close request ended with the unchanged input counter, so the
    /// app was still the test's when it was asked to quit.
    close_verified: bool,
    /// The host transport presented this hosted app's first valid frame.
    first_frame: bool,
    /// Display title of a demonstration test, if any.
    demo: Option<String>,
}

/// One app launch. `requested` is what the caller asked for and `mode` what
/// this launch actually is; they differ only for a recorded fallback.
struct AppLaunch<'a> {
    flow: &'a str,
    artifact: &'a str,
    path: &'a Path,
    mode: iteration::LaunchMode,
    requested: iteration::LaunchMode,
    reopening: bool,
    embedded: Option<(String, u64, u16)>,
    role: iteration::RunRole,
    grant: Option<&'a str>,
    demo: Option<&'a str>,
}

/// An immutable executable a lane may run: its own retained artifact, or one
/// its direct parent granted. The provenance stays with the original owner.
#[derive(Clone)]
struct RetainedArtifact {
    id: String,
    commit: String,
    path: PathBuf,
    /// Flow directory under `artifacts/` that retains the executable.
    origin_flow: String,
    grant: Option<iteration::AgentGrant>,
}
struct CargoLease(PathBuf);
impl Drop for CargoLease {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

impl Host {
    fn start_preflight(&mut self, flow: &str, job: &str) -> Result<(), String> {
        self.cancel_pending_embedding(flow, "Superseded by a newer build request");
        if self.builds.contains_key(flow) || self.apps.contains_key(flow) {
            return Err("Close the previous app before preparing another build".into());
        }
        let mut build = self.pipeline(flow, job, "", true)?;
        build.phases = check_phases(&build);
        build.phases.extend(format_phases(&build)?);
        self.fingerprints
            .insert(flow.into(), build.expected_source.clone());
        self.publish_build(flow, &build, "waiting for build slot", false, None)?;
        self.builds.insert(flow.into(), build);
        Ok(())
    }

    fn prepare_build(&mut self, flow: &str, job: &str, commit: &str) -> Result<Build, String> {
        if self.apps.contains_key(flow) {
            return Err("The previous app is still running".into());
        }
        let mut build = self.pipeline(flow, job, commit, false)?;
        git::verify_checkpoint_for_build(&build.owned, commit)?;
        let prior = build.directory.join("preflight.json");
        let prior =
            json::parse(bounded_read(&prior, 1024 * 1024)?.as_bytes()).map_err(str::to_owned)?;
        if prior.get("commit").and_then(Value::as_str) != Some(commit)
            || prior.get("checks_passed") != Some(&Value::Bool(true))
        {
            return Err("This checkpoint lacks completed check/format evidence".into());
        }
        if let Some(Value::Arr(results)) = prior.get("results") {
            build.results = results.clone();
        }
        let mut binary_args = cargo_base(&build, "build");
        binary_args.extend([
            "--package".into(),
            build.config.package.clone(),
            "--bin".into(),
            build.config.binary.clone(),
            "--target".into(),
            build.host_target.clone(),
            "--message-format=json-render-diagnostics".into(),
        ]);
        build.phases.push_back(BuildPhase {
            kind: PhaseKind::Binary,
            label: "release binary build".into(),
            program: "cargo".into(),
            args: binary_args,
        });
        let mut test_args = cargo_base(&build, "test");
        match build.config.test_scope {
            iteration::TestScope::Workspace => test_args.push("--workspace".into()),
            iteration::TestScope::Package => {
                test_args.extend(["--package".into(), build.config.package.clone()])
            }
        }
        test_args.extend(["--target".into(), build.host_target.clone()]);
        build.phases.push_back(BuildPhase {
            kind: PhaseKind::Test,
            label: match build.config.test_scope {
                iteration::TestScope::Workspace => "existing native workspace tests".into(),
                iteration::TestScope::Package => {
                    "existing native package tests (partial scope)".into()
                }
            },
            program: "cargo".into(),
            args: test_args,
        });
        self.fingerprints
            .insert(flow.into(), build.expected_source.clone());
        self.publish_build(flow, &build, "waiting for release build", false, None)?;
        Ok(build)
    }

    fn pipeline(
        &self,
        flow: &str,
        job: &str,
        commit: &str,
        preflight: bool,
    ) -> Result<Build, String> {
        let config = self.flow(flow)?.config.clone();
        let owned = self.owned(flow)?;
        let original = fs::canonicalize(&config.repo).map_err(err)?;
        let original_manifest = fs::canonicalize(&config.manifest).map_err(err)?;
        let relative = original_manifest
            .strip_prefix(&original)
            .map_err(|_| "Manifest is outside its declared repository")?;
        let manifest = fs::canonicalize(owned.path.join(relative)).map_err(err)?;
        if !manifest.starts_with(&owned.path) {
            return Err("Manifest escapes the open repository".into());
        }
        let directory = self.directory.join("builds").join(flow).join(job);
        fs::create_dir_all(&directory).map_err(err)?;
        let host_target = rust_host()?;
        let expected_source = source_fingerprint(&owned.path)?;
        let target_directory = owned.common_dir.join("studio-target");
        fs::create_dir_all(&target_directory).map_err(err)?;
        if !rustc_guard()?.is_file() {
            return Err("Build director-rustc-guard before running iteration checks".into());
        }
        Ok(Build {
            job: job.into(),
            commit: commit.into(),
            child: None,
            phases: VecDeque::new(),
            active: None,
            started: Instant::now(),
            phase_started: Instant::now(),
            directory: directory.clone(),
            log: directory.join("pending.log"),
            manifest,
            owned,
            config,
            host_target,
            target_directory,
            artifact_id: format!("artifact-{flow}-{job}"),
            executable: None,
            expected_source,
            tainted: None,
            failed_exit: None,
            preflight,
            formatted_changed: false,
            rechecked: false,
            results: vec![],
            lease: None,
        })
    }

    fn poll_builds(&mut self) {
        let flows: Vec<_> = self.builds.keys().cloned().collect();
        for flow in flows {
            let Some(mut build) = self.builds.remove(&flow) else {
                continue;
            };
            let current = self
                .engine
                .flows
                .get(&flow)
                .and_then(|flow| flow.job.as_ref())
                .is_some_and(|job| {
                    job.id == build.job
                        && matches!(
                            job.phase,
                            iteration::BuildPhase::CheckpointRequested
                                | iteration::BuildPhase::Ready
                                | iteration::BuildPhase::Building
                        )
                });
            if !current {
                build.tainted = Some("Build superseded by newer requirements or source".into());
            }
            let outcome = self.poll_build(&flow, &mut build);
            match outcome {
                Ok(false) => {
                    self.builds.insert(flow, build);
                }
                Ok(true) => {}
                Err(error) => {
                    if let Some(child) = &mut build.child {
                        stop_build_process(child);
                        build.tainted = Some(error);
                        self.builds.insert(flow, build);
                        continue;
                    }
                    if let Some(path) = &build.executable {
                        // Failed candidates were never published as artifacts.
                        let _ = fs::remove_file(path);
                    }
                    let _ = self.publish_build(&flow, &build, "failed", false, Some(&error));
                    if current {
                        if let Err(observe) = self.observe(Observation::BuildFailed {
                            flow: flow.clone(),
                            job_id: build.job.clone(),
                            exit_code: build.failed_exit,
                            error: error.clone(),
                        }) {
                            self.note =
                                format!("{error}; could not persist build failure: {observe}");
                            self.changed = true;
                        }
                    }
                    if let Ok(fingerprint) = source_fingerprint(&build.owned.path) {
                        let changed = fingerprint != build.expected_source;
                        self.fingerprints.insert(flow.clone(), fingerprint);
                        if changed {
                            let _ = self.observe(Observation::SourceChanged { flow });
                        }
                    }
                }
            }
        }
    }

    fn poll_build(&mut self, flow: &str, build: &mut Build) -> Result<bool, String> {
        if let Some(child) = &mut build.child {
            if build.tainted.is_some() {
                stop_build_process(child);
            }
            if fs::metadata(&build.log).map(|m| m.len()).unwrap_or(0) > BUILD_LOG_LIMIT {
                build.tainted =
                    Some("Build log exceeded 32 MiB; process stopped to bound storage".into());
                stop_build_process(child);
            }
            let Some(status) = child.try_wait().map_err(err)? else {
                return Ok(false);
            };
            build.child = None;
            let phase = build.active.take().ok_or("Build process had no phase")?;
            build.results.push(json::obj(vec![
                ("phase", s(&phase.label)),
                (
                    "success",
                    Value::Bool(status.success() && build.tainted.is_none()),
                ),
                (
                    "exit_code",
                    status
                        .code()
                        .map(|code| Value::Int(code.into()))
                        .unwrap_or(Value::Null),
                ),
                (
                    "elapsed_ms",
                    Value::Int(
                        build
                            .phase_started
                            .elapsed()
                            .as_millis()
                            .min(i64::MAX as u128) as i64,
                    ),
                ),
                ("log", s(build.log.to_string_lossy())),
            ]));
            if let Some(error) = build.tainted.take() {
                return Err(error);
            }
            if !status.success() {
                build.failed_exit = status.code();
                return Err(format!(
                    "{} failed with {status}. {}",
                    phase.label,
                    log_tail(&build.log, 8192).unwrap_or_default()
                ));
            }
            if log_has_warning(&build.log)? {
                return Err(format!(
                    "{} emitted a warning; the validation gate requires no warnings",
                    phase.label
                ));
            }
            let source = source_fingerprint(&build.owned.path)?;
            if phase.kind == PhaseKind::Format {
                build.formatted_changed |= source != build.expected_source;
                build.expected_source = source;
            } else if source != build.expected_source {
                return Err("Source changed while a validation/build process was running; this output cannot be assigned to its checkpoint".into());
            }
            if !build.commit.is_empty() {
                git::verify_checkpoint_for_build(&build.owned, &build.commit)?;
            }
            if phase.kind == PhaseKind::Binary {
                let executable = find_executable(&build.log, &build.config.binary)?;
                let executable = fs::canonicalize(executable).map_err(err)?;
                if !executable.starts_with(fs::canonicalize(&build.target_directory).map_err(err)?)
                {
                    return Err("Cargo executable is outside the owned build cache".into());
                }
                let destination = self
                    .directory
                    .join("artifacts")
                    .join(flow)
                    .join(&build.artifact_id)
                    .join(executable.file_name().ok_or("Executable has no filename")?);
                copy_immutable(&executable, &destination)?;
                let binary_hash = retained_binary_hash(&destination)?;
                atomic_write(
                    &destination
                        .parent()
                        .ok_or("Retained artifact has no directory")?
                        .join("binary.json"),
                    json::obj(vec![
                        ("commit", s(&build.commit)),
                        ("artifact_id", s(&build.artifact_id)),
                        ("content_hash", s(binary_hash)),
                        ("algorithm", s("git_blob")),
                    ])
                    .to_json()
                    .as_bytes(),
                )?;
                build.executable = Some(destination);
            }
            self.publish_build(flow, build, "phase complete", false, None)?;
        }
        if let Some(error) = build.tainted.take() {
            return Err(error);
        }
        if build.lease.is_none() {
            let lease = build.owned.common_dir.join("studio-build.lock");
            match OpenOptions::new().write(true).create_new(true).open(&lease) {
                Ok(mut file) => {
                    writeln!(
                        file,
                        "pid={} flow={flow} job={}",
                        std::process::id(),
                        build.job
                    )
                    .map_err(err)?;
                    build.lease = Some(CargoLease(lease));
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    return Ok(false)
                }
                Err(error) => return Err(err(error)),
            }
        }
        if source_fingerprint(&build.owned.path)? != build.expected_source {
            return Err("Prepared source changed while waiting for the build slot; prepare the new revision".into());
        }
        if let Some(phase) = build.phases.pop_front() {
            let index = build.results.len();
            build.log = build.directory.join(format!("phase-{index:02}.log"));
            let mut log = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&build.log)
                .map_err(err)?;
            writeln!(
                log,
                "Studio phase: {}\nCommand: {} {:?}\n",
                phase.label, phase.program, phase.args
            )
            .map_err(err)?;
            let mut command = Command::new(&phase.program);
            command
                .args(&phase.args)
                .current_dir(&build.owned.path)
                .stdin(Stdio::null())
                .stdout(log.try_clone().map_err(err)?)
                .stderr(log);
            if phase.kind != PhaseKind::Format {
                command
                    .env("CARGO_TARGET_DIR", &build.target_directory)
                    .env("CARGO_TERM_COLOR", "never");
                command.env_remove("MAKEPAD_STUDIO_PREVIOUS_RUSTC_WRAPPER");
                let wrapper = rustc_guard()?;
                if let Some(previous) =
                    std::env::var_os("RUSTC_WRAPPER").filter(|value| !value.is_empty())
                {
                    if Path::new(&previous) != wrapper {
                        command.env("MAKEPAD_STUDIO_PREVIOUS_RUSTC_WRAPPER", previous);
                    }
                }
                command.env("RUSTC_WRAPPER", wrapper);
            }
            if phase.kind == PhaseKind::Test {
                let run = format!("test-{flow}-{}", build.job);
                let run_directory = self.directory.join("runs").join(&run);
                fs::create_dir_all(run_directory.join("video")).map_err(err)?;
                fs::create_dir_all(run_directory.join("feedback")).map_err(err)?;
                command
                    .env("MAKEPAD_HIDE_WINDOWS", "1")
                    .env_remove("MAKEPAD_FOCUS")
                    .env("MAKEPAD_STUDIO_RECORDING_DIR", run_directory.join("video"))
                    .env(
                        "MAKEPAD_STUDIO_FEEDBACK_DIR",
                        run_directory.join("feedback"),
                    )
                    .env("MAKEPAD_STUDIO_FLOW_ID", flow)
                    .env("MAKEPAD_STUDIO_ARTIFACT_ID", &build.artifact_id)
                    .env("MAKEPAD_STUDIO_RUN_ID", run)
                    .env("MAKEPAD_STUDIO_REVISION", &build.commit);
            }
            configure_process_group(&mut command);
            build.child = Some(
                command
                    .spawn()
                    .map_err(|error| format!("Cannot start {}: {error}", phase.label))?,
            );
            build.phase_started = Instant::now();
            let label = phase.label.clone();
            build.active = Some(phase);
            self.publish_build(flow, build, &label, false, None)?;
            return Ok(false);
        }
        if build.preflight {
            if build.formatted_changed && !build.rechecked {
                build.phases = check_phases(build);
                build.rechecked = true;
                self.publish_build(
                    flow,
                    build,
                    "format changed source; repeating platform checks",
                    false,
                    None,
                )?;
                return Ok(false);
            }
            let state = git::inspect(&build.owned.path)?;
            let paths: Vec<_> = state.changes.iter().map(|path| path.path.clone()).collect();
            let checkpoint = git::checkpoint(
                &build.owned,
                &state.head,
                &paths,
                &["AGENTS.md".into(), "CLAUDE.md".into()],
                &format!("local: {flow} {}", build.job),
            )?;
            build.commit = checkpoint.commit.clone();
            build.expected_source = source_fingerprint(&build.owned.path)?;
            self.fingerprints
                .insert(flow.into(), build.expected_source.clone());
            let evidence = json::obj(vec![
                ("commit", s(&build.commit)),
                ("tree", s(&checkpoint.tree)),
                ("checks_passed", Value::Bool(true)),
                ("results", Value::Arr(build.results.clone())),
            ]);
            atomic_write(
                &build.directory.join("preflight.json"),
                evidence.to_json().as_bytes(),
            )?;
            self.publish_build(flow, build, "checked, formatted, checkpointed", false, None)?;
            self.observe(Observation::Checkpointed {
                flow: flow.into(),
                job_id: build.job.clone(),
                commit: checkpoint.commit,
            })?;
            return Ok(true);
        }
        git::verify_checkpoint_for_build(&build.owned, &build.commit)?;
        let executable = build
            .executable
            .clone()
            .ok_or("Release build produced no immutable executable")?;
        self.publish_build(flow, build, "validated release ready", true, None)?;
        self.observe(Observation::BuildSucceeded {
            flow: flow.into(),
            job_id: build.job.clone(),
            artifact_id: build.artifact_id.clone(),
            path: executable,
        })?;
        Ok(true)
    }

    fn publish_build(
        &mut self,
        flow: &str,
        build: &Build,
        phase: &str,
        passed: bool,
        error: Option<&str>,
    ) -> Result<(), String> {
        let full_scope = build.config.test_scope == iteration::TestScope::Workspace;
        let tree = if passed {
            git::commit_tree(&build.owned.path, &build.commit).ok()
        } else {
            None
        };
        let report = json::obj(vec![
            ("kind", s("build")),
            ("flow", s(flow)),
            ("job", s(&build.job)),
            ("phase", s(phase)),
            ("preflight", Value::Bool(build.preflight)),
            ("commit", s(&build.commit)),
            ("artifact", s(&build.artifact_id)),
            (
                "complete",
                Value::Bool(passed && full_scope && tree.is_some()),
            ),
            ("selected_checks_passed", Value::Bool(passed)),
            (
                "test_scope",
                s(if full_scope {
                    "workspace"
                } else {
                    "package_partial"
                }),
            ),
            (
                "validated_tree",
                tree.as_deref().map(s).unwrap_or(Value::Null),
            ),
            (
                "check_targets",
                Value::Arr(build.config.check_targets.iter().map(s).collect()),
            ),
            ("host_target", s(&build.host_target)),
            ("results", Value::Arr(build.results.clone())),
            ("log", s(build.log.to_string_lossy())),
            ("error", error.map(s).unwrap_or(Value::Null)),
            (
                "elapsed_ms",
                Value::Int(build.started.elapsed().as_millis().min(i64::MAX as u128) as i64),
            ),
            ("test_run", s(format!("test-{flow}-{}", build.job))),
            (
                "test_video",
                s(self
                    .directory
                    .join("runs")
                    .join(format!("test-{flow}-{}", build.job))
                    .join("video")
                    .to_string_lossy()),
            ),
        ]);
        if let Err(error) = atomic_write(
            &build.directory.join("report.json"),
            report.to_json().as_bytes(),
        ) {
            self.note = format!("Build evidence could not be saved: {error}");
            self.storage_error = Some(self.note.clone());
            self.changed = true;
            return Err(self.note.clone());
        }
        self.reports
            .insert(format!("build:{flow}:{}", build.job), report);
        self.changed = true;
        Ok(())
    }

    fn launch(
        &mut self,
        flow: &str,
        artifact: &str,
        path: &Path,
        mode: iteration::LaunchMode,
    ) -> Result<(), String> {
        self.launch_artifact(flow, artifact, path, mode, false)
    }

    fn launch_artifact(
        &mut self,
        flow: &str,
        artifact: &str,
        path: &Path,
        mode: iteration::LaunchMode,
        reopening: bool,
    ) -> Result<(), String> {
        if mode == iteration::LaunchMode::Embedded {
            if reopening {
                return Err("Pop-out must reopen standalone".into());
            }
            return self.queue_embedded(flow, artifact, path);
        }
        self.spawn_artifact(flow, artifact, path, mode, reopening, None)
    }

    fn spawn_artifact(
        &mut self,
        flow: &str,
        artifact: &str,
        path: &Path,
        mode: iteration::LaunchMode,
        reopening: bool,
        embedded: Option<(String, u64, u16)>,
    ) -> Result<(), String> {
        self.spawn_app(AppLaunch {
            flow,
            artifact,
            path,
            mode,
            requested: mode,
            reopening,
            embedded,
            role: iteration::RunRole::Human,
            grant: None,
            demo: None,
        })
    }

    /// The lane's own retained artifact, or one named by a grant its direct
    /// parent issued to exactly this lane. Anything else is refused: no other
    /// lane's artifact, no unrelated grant and no external executable.
    fn retained_artifact(
        &self,
        flow: &str,
        artifact: &str,
        grant: Option<&str>,
    ) -> Result<RetainedArtifact, String> {
        let state = self.flow(flow)?;
        if let Some(id) = grant {
            let grant = self
                .engine
                .agent_grant_by_id(flow, id)
                .ok_or("Unknown artifact grant for this lane")?;
            if grant.artifact != artifact {
                return Err("That grant names a different artifact".into());
            }
            return Ok(RetainedArtifact {
                id: grant.artifact.clone(),
                commit: grant.commit.clone(),
                path: grant.path.clone(),
                origin_flow: grant.origin_flow.clone(),
                grant: Some(grant.clone()),
            });
        }
        let selected = state
            .artifacts
            .iter()
            .find(|item| item.id == artifact)
            .ok_or("Unknown retained artifact; a parent's artifact needs grant=<grant id>")?;
        Ok(RetainedArtifact {
            id: selected.id.clone(),
            commit: selected.commit.clone(),
            path: selected.path.clone(),
            origin_flow: self
                .engine
                .evidence_origin("artifact", artifact)
                .unwrap_or(flow)
                .to_owned(),
            grant: None,
        })
    }

    fn spawn_app(&mut self, launch: AppLaunch) -> Result<(), String> {
        let AppLaunch {
            flow,
            artifact,
            path,
            mode,
            requested,
            reopening,
            embedded,
            role,
            grant,
            demo,
        } = launch;
        if self.apps.contains_key(flow) {
            return Err("Close the current app before opening another revision".into());
        }
        self.app_budget()?;
        let state = self.flow(flow)?;
        let selected = self.retained_artifact(flow, artifact, grant)?;
        if selected.path != path {
            return Err("Unknown immutable artifact".into());
        }
        // A granted artifact runs against the checkout that built it when that
        // lane still exists; the child's run directory and evidence stay its own.
        let owned = match selected
            .grant
            .as_ref()
            .and_then(|grant| self.engine.agent_current_flow(&grant.owner))
        {
            Some(owner) => self.owned(&owner.id)?,
            None => self.owned(flow)?,
        };
        let actual = fs::canonicalize(path).map_err(err)?;
        let artifact_root = fs::canonicalize(
            self.directory
                .join("artifacts")
                .join(&selected.origin_flow)
                .join(artifact),
        )
        .map_err(err)?;
        if !actual.starts_with(&artifact_root) || !actual.is_file() {
            return Err("Artifact executable escapes its retained directory".into());
        }
        let identity_path = artifact_root.join("binary.json");
        let binary_hash = if identity_path.exists() || role == iteration::RunRole::AiTest {
            let identity = json::parse(bounded_read(&identity_path, 4096).map_err(|_| "This artifact has no build-time executable identity; rebuild it before AI testing".to_owned())?.as_bytes()).map_err(str::to_owned)?;
            let actual_hash = retained_binary_hash(&actual)?;
            if identity.get("commit").and_then(Value::as_str) != Some(selected.commit.as_str())
                || identity.get("artifact_id").and_then(Value::as_str) != Some(artifact)
                || identity.get("content_hash").and_then(Value::as_str)
                    != Some(actual_hash.as_str())
            {
                return Err("Retained executable changed since this artifact was built".into());
            }
            Some(actual_hash)
        } else {
            None
        };
        let (run, embedding_client, embedding_port) = match embedded {
            Some((run, client, port)) => (run, Some(client), Some(port)),
            None => {
                let (run, _) = fresh_run_identity();
                (run, None, None)
            }
        };
        let directory = self.directory.join("runs").join(&run);
        fs::create_dir(&directory)
            .or_else(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    fs::create_dir_all(directory.parent().unwrap())?;
                    fs::create_dir(&directory)
                } else {
                    Err(error)
                }
            })
            .map_err(err)?;
        fs::create_dir(directory.join("feedback")).map_err(err)?;
        fs::create_dir(directory.join("video")).map_err(err)?;
        let log = directory.join("app.log");
        let output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&log)
            .map_err(err)?;
        let mut command = Command::new(&actual);
        command
            .arg("--remote")
            .arg(format!("--remote-title-tag={flow}"))
            .current_dir(&owned.path)
            .stdin(Stdio::null())
            .stdout(output.try_clone().map_err(err)?)
            .stderr(output)
            .env_remove("MAKEPAD_HIDE_WINDOWS")
            .env_remove("MAKEPAD_FOCUS")
            .env_remove("STUDIO_HOST")
            .env_remove("STUDIO_BUILD")
            .env_remove("STUDIO_CRATE")
            .env_remove("STUDIO")
            .env("MAKEPAD_STUDIO_FEEDBACK_DIR", directory.join("feedback"))
            .env("MAKEPAD_STUDIO_RECORDING_DIR", directory.join("video"))
            .env("MAKEPAD_STUDIO_FLOW_ID", flow)
            .env("MAKEPAD_STUDIO_ARTIFACT_ID", artifact)
            .env("MAKEPAD_STUDIO_RUN_ID", &run)
            .env("MAKEPAD_STUDIO_REVISION", &selected.commit);
        if role == iteration::RunRole::AiTest {
            command.env("MAKEPAD_HIDE_WINDOWS", "1");
        }
        if let (Some(client), Some(port)) = (embedding_client, embedding_port) {
            command
                .arg("--stdin-loop")
                .env("STUDIO_HOST", format!("http://127.0.0.1:{port}"))
                .env("STUDIO_BUILD", client.to_string())
                .env("STUDIO_CRATE", &state.config.binary);
        }
        configure_process_group(&mut command);
        let mut child = command.spawn().map_err(err)?;
        let pid = child.id();
        let metadata = atomic_write(
            &directory.join("run.json"),
            json::obj(vec![
                ("flow", s(flow)),
                ("run", s(&run)),
                ("artifact", s(artifact)),
                ("commit", s(&selected.commit)),
                ("pid", Value::Int(pid.into())),
                ("executable", s(actual.to_string_lossy())),
                ("cwd", s(owned.path.to_string_lossy())),
                (
                    "mode",
                    s(if mode == iteration::LaunchMode::Embedded {
                        "embedded"
                    } else {
                        "standalone"
                    }),
                ),
                (
                    "host_client",
                    embedding_client
                        .map(|client| Value::Int(client as i64))
                        .unwrap_or(Value::Null),
                ),
                (
                    "host_port",
                    embedding_port
                        .map(|port| Value::Int(i64::from(port)))
                        .unwrap_or(Value::Null),
                ),
                ("role", s(role.as_str())),
                (
                    "binary_content_hash",
                    binary_hash.as_deref().map(s).unwrap_or(Value::Null),
                ),
                ("resource_snapshot", Value::Bool(false)),
                ("requested_mode", s(requested.as_str())),
                (
                    "node",
                    self.engine
                        .agent_node(flow)
                        .map(|node| s(&node.id))
                        .unwrap_or(Value::Null),
                ),
                // Provenance of a granted artifact stays with its owner; the
                // run, input, results and video belong to this lane.
                (
                    "grant",
                    selected
                        .grant
                        .as_ref()
                        .map(|grant| {
                            json::obj(vec![
                                ("id", s(&grant.id)),
                                ("owner", s(&grant.owner)),
                                ("origin_flow", s(&grant.origin_flow)),
                                ("artifact", s(&grant.artifact)),
                                ("commit", s(&grant.commit)),
                            ])
                        })
                        .unwrap_or(Value::Null),
                ),
            ])
            .to_json()
            .as_bytes(),
        );
        let observation = if role == iteration::RunRole::AiTest {
            Observation::TestRunStarted {
                flow: flow.into(),
                artifact_id: artifact.into(),
                run_id: run.clone(),
                pid: Some(pid),
                mode,
            }
        } else if reopening {
            Observation::RunReopened {
                flow: flow.into(),
                artifact_id: artifact.into(),
                run_id: run.clone(),
                pid: Some(pid),
            }
        } else {
            Observation::RunStarted {
                flow: flow.into(),
                artifact_id: artifact.into(),
                run_id: run.clone(),
                pid: Some(pid),
            }
        };
        let observed = metadata.and_then(|_| self.observe(observation).map(|_| ()));
        if let Err(error) = observed {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "Owned launch was stopped because process identity could not be persisted: {error}"
            ));
        }
        self.apps.insert(
            flow.into(),
            AppRun {
                child,
                role,
                run,
                artifact: artifact.into(),
                directory,
                log,
                closing: None,
                port: None,
                grab_directory: None,
                user_closed: false,
                log_offset: 0,
                log_partial: String::new(),
                close_request: None,
                close_stage: 0,
                exit: None,
                mode,
                pop_out: false,
                embedding_client,
                embedding_port,
                requested,
                grant: selected.grant.as_ref().map(|grant| grant.id.clone()),
                user_seq: None,
                handed_off: false,
                handoff_reason: None,
                operator_close: false,
                close_verified: false,
                first_frame: false,
                demo: demo.map(str::to_owned),
                extra_windows: 0,
            },
        );
        self.changed = true;
        Ok(())
    }

    /// Automatic cleanup: test stop, a failed start, fallback retirement,
    /// lane lifecycle and shutdown. It never closes an app a person took over.
    fn close(&mut self, flow: &str) -> Result<(), String> {
        self.close_app(flow, false)
    }

    /// The person's explicit Close in Director. It is the only way a
    /// handed-off app is closed, and it is sent without the test's guard.
    fn close_by_operator(&mut self, flow: &str) -> Result<(), String> {
        self.close_app(flow, true)
    }

    fn close_app(&mut self, flow: &str, operator: bool) -> Result<(), String> {
        if !operator && self.apps.get(flow).is_some_and(|run| run.handed_off) {
            return Err("A person is using this test app; automation does not close it. Close it from Director".into());
        }
        self.cancel_test_operation(flow, "Owned app close requested");
        if self.cancel_pending_embedding(flow, "Launch canceled before the child started") {
            return Ok(());
        }
        if let Some(run) = self.apps.get_mut(flow) {
            run.pop_out = false;
            if operator && !run.operator_close {
                // Their request starts its own close sequence.
                run.operator_close = true;
                run.close_stage = 0;
                run.closing = Some(Instant::now());
            }
            run.closing.get_or_insert_with(Instant::now);
            self.note = format!("{flow}: capturing the final frame and closing the owned app");
            self.changed = true;
            return Ok(());
        }
        let run = self
            .flow(flow)?
            .runs
            .last()
            .ok_or("This flow has no app to close")?
            .clone();
        if run.role == iteration::RunRole::AiTest {
            return Err(
                "This AI test has already closed; it cannot acknowledge a human app close".into(),
            );
        }
        if run.closed && !run.human_requested {
            self.observe(Observation::RunClosed {
                flow: flow.into(),
                run_id: run.id,
                human_requested: true,
                exit_code: run.exit_code,
            })?;
            return Ok(());
        }
        // Explicit human closure can reconcile a vanished process after a UI
        // restart. An existing/reused PID is never stopped or assumed ours.
        #[cfg(unix)]
        if run.observation_lost && !run.closed {
            if let Some(pid) = run.pid.filter(|pid| *pid > 1 && *pid <= i32::MAX as u32) {
                unsafe extern "C" {
                    fn kill(pid: i32, signal: i32) -> i32;
                }
                if unsafe { kill(pid as i32, 0) } != 0
                    && std::io::Error::last_os_error().raw_os_error() == Some(3)
                {
                    self.observe(Observation::RunClosed {
                        flow: flow.into(),
                        run_id: run.id,
                        human_requested: true,
                        exit_code: None,
                    })?;
                    self.note = "App exit confirmed; its last saved frame is retained".into();
                    self.changed = true;
                    return Ok(());
                }
            }
        }
        Err(
            "No live process owned by this host; an interrupted run needs ownership reconciliation"
                .into(),
        )
    }

    fn pop_out(&mut self, flow: &str, run_id: &str) -> Result<(), String> {
        let run = self
            .apps
            .get_mut(flow)
            .ok_or("No live owned app is available to pop out")?;
        if run.run != run_id {
            return Err(
                "The selected app run changed; inspect the current run before popping it out"
                    .into(),
            );
        }
        if run.role != iteration::RunRole::Human {
            // Popping out closes and reopens the app, which would rerun an
            // active AI test behind its agent's back.
            return Err(
                "An AI test preview is watch-only and is not popped out; its agent owns that run"
                    .into(),
            );
        }
        if run.mode == iteration::LaunchMode::Standalone {
            return Err("This app is already running standalone".into());
        }
        if run.closing.is_some() {
            return Err("This app is already closing; wait for its observed exit".into());
        }
        run.pop_out = true;
        run.closing = Some(Instant::now());
        self.note = format!("{flow}: closing the embedded app, then reopening the same artifact standalone without recompiling");
        self.changed = true;
        Ok(())
    }

    fn poll_apps(&mut self) {
        let flows: Vec<_> = self.apps.keys().cloned().collect();
        for flow in flows {
            let Some(mut run) = self.apps.remove(&flow) else {
                continue;
            };
            if let Err(error) = read_app_log(&mut run) {
                self.note = format!("{flow}: {error}");
                self.changed = true;
            }
            if run.exit.is_none() {
                match run.child.try_wait() {
                    Ok(Some(status)) => run.exit = Some(status),
                    Ok(None) => {}
                    Err(error) => {
                        self.note = format!("{flow}: cannot observe app exit: {error}");
                        self.changed = true;
                    }
                }
            }
            // An automatic close of an AI test is guarded; the person's own
            // close request from Director is not.
            let guarded = run.role == iteration::RunRole::AiTest && !run.operator_close;
            if let Some(request) = &mut run.close_request {
                match request.try_wait() {
                    Ok(Some(_)) => {
                        run.close_request = None;
                        // Ownership evidence is read first, whatever curl's
                        // own exit status was: a partial reply can still
                        // carry the 409 or the moved input counter.
                        let evidence = if guarded {
                            close_evidence(&run)
                        } else {
                            CloseEvidence::Verified
                        };
                        match evidence {
                            CloseEvidence::Refused(reason) => {
                                self.note = format!(
                                    "{flow}: {reason}; the app was not closed and stays running"
                                );
                                hand_off(&mut run, reason);
                                self.changed = true;
                            }
                            CloseEvidence::Verified => {
                                run.close_verified = true;
                                if let Err(error) = retain_close_capture(&run) {
                                    self.note = format!("{flow}: {error}");
                                    self.changed = true;
                                }
                            }
                            // An established guard without the ending counter
                            // of this very reply proves nothing: no further
                            // request is sent and nothing is killed. If the
                            // process still runs at the deadline below it is
                            // left to the person; one that exits meanwhile
                            // simply finishes its cleanup.
                            CloseEvidence::Unknown => {
                                run.close_verified = false;
                                run.close_stage = 2;
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        self.note = format!("{flow}: close request failed: {error}");
                        run.close_request = None;
                        self.changed = true;
                    }
                }
            }
            if let Some(since) = run.closing {
                if run.exit.is_none() && run.close_request.is_none() && run.close_stage < 2 {
                    if let Some(port) = run.port {
                        let route = if run.close_stage == 0 { "gq" } else { "quit" };
                        run.close_stage += 1;
                        let mut command = Command::new("curl");
                        command.args([
                            "--silent",
                            "--show-error",
                            "--max-time",
                            "2",
                            "--connect-timeout",
                            "1",
                        ]);
                        let mut url = format!("http://127.0.0.1:{port}/{route}");
                        // The status and the ending input counter must be
                        // recordable before a guarded close is sent at all.
                        let status = if guarded {
                            let code = run.directory.join("close.code");
                            let headers = run.directory.join("close.headers");
                            if let Some(sequence) = run.user_seq {
                                url.push_str(&format!("?if_user_seq={sequence}"));
                            }
                            command
                                .args(["--write-out", "%{http_code}", "--dump-header"])
                                .arg(&headers);
                            // Both files are created here, emptied of any
                            // earlier stage, before curl may write to them.
                            File::create(&headers)
                                .and_then(|_| File::create(&code))
                                .map(Stdio::from)
                                .map_err(|error| {
                                    format!("the close reply could not be recorded ({error}), so the app was not asked to close")
                                })
                        } else {
                            command.arg("--fail");
                            Ok(Stdio::null())
                        };
                        // Each request proves ownership for itself only.
                        run.close_verified = false;
                        match status {
                            Ok(status) => match command
                                .arg("--output")
                                .arg(run.directory.join("close.json"))
                                .arg(url)
                                .stdin(Stdio::null())
                                .stdout(status)
                                .stderr(Stdio::null())
                                .spawn()
                            {
                                Ok(child) => run.close_request = Some(child),
                                Err(error) => {
                                    self.note = format!(
                                        "{flow}: graceful {route} could not start: {error}"
                                    );
                                    self.changed = true;
                                }
                            },
                            Err(reason) => {
                                self.note = format!("{flow}: {reason}; it stays running");
                                hand_off(&mut run, reason);
                                self.changed = true;
                            }
                        }
                    }
                }
                if run.closing.is_some()
                    && run.exit.is_none()
                    && run.close_request.is_none()
                    && since.elapsed() > Duration::from_secs(6)
                {
                    // A test with an established guard is only killed after its
                    // latest close reply proved the counter unchanged. Without
                    // that proof the app is left to the person.
                    if may_force_kill(&run) {
                        if let Err(error) = run.child.kill() {
                            self.note = format!("{flow}: owned app did not close: {error}");
                            self.changed = true;
                        }
                    } else {
                        let reason = "no close reply carried the person's input counter, so ownership could not be verified".to_owned();
                        self.note =
                            format!("{flow}: {reason}; the app was not killed and stays running");
                        hand_off(&mut run, reason);
                        self.changed = true;
                    }
                }
            }
            if let Some(status) = run.exit.filter(|_| run.close_request.is_none()) {
                if let Some(client) = run.embedding_client {
                    self.embedding_unregister.insert(client);
                }

                let human_requested = run.role == iteration::RunRole::Human
                    && !run.pop_out
                    && (run.closing.is_some() || run.user_closed);
                let report = json::obj(vec![
                    ("kind", s("run")),
                    ("flow", s(&flow)),
                    ("run", s(&run.run)),
                    ("artifact", s(&run.artifact)),
                    ("role", s(run.role.as_str())),
                    ("requested_mode", s(run.requested.as_str())),
                    ("mode", s(run.mode.as_str())),
                    ("grant", run.grant.as_deref().map(s).unwrap_or(Value::Null)),
                    ("handed_off", Value::Bool(run.handed_off)),
                    (
                        "handoff_reason",
                        run.handoff_reason.as_deref().map(s).unwrap_or(Value::Null),
                    ),
                    ("operator_close", Value::Bool(run.operator_close)),
                    (
                        "first_frame",
                        if run.mode == iteration::LaunchMode::Embedded {
                            Value::Bool(run.first_frame)
                        } else {
                            Value::Null
                        },
                    ),
                    ("unobserved_windows", Value::Int(run.extra_windows as i64)),
                    ("closed", Value::Bool(true)),
                    ("human_requested", Value::Bool(human_requested)),
                    (
                        "exit_code",
                        status
                            .code()
                            .map(|code| Value::Int(code.into()))
                            .unwrap_or(Value::Null),
                    ),
                    ("log", s(run.log.to_string_lossy())),
                    ("video", s(run.directory.join("video").to_string_lossy())),
                ]);
                self.reports
                    .insert(format!("run:{}", run.run), report.clone());
                let _ = atomic_write(
                    &run.directory.join("closed.json"),
                    report.to_json().as_bytes(),
                );
                let result = self.observe(Observation::RunClosed {
                    flow: flow.clone(),
                    run_id: run.run.clone(),
                    human_requested,
                    exit_code: status.code(),
                });
                if let Err(error) = result {
                    self.note =
                        format!("Process exited; closure evidence could not be persisted: {error}");
                    self.changed = true;
                } else if run.pop_out {
                    let path = self
                        .engine
                        .flows
                        .get(&flow)
                        .and_then(|state| {
                            state
                                .artifacts
                                .iter()
                                .find(|artifact| artifact.id == run.artifact)
                        })
                        .map(|artifact| artifact.path.clone());
                    let result = path
                        .ok_or_else(|| "Retained artifact disappeared before pop-out".to_owned())
                        .and_then(|path| {
                            self.launch_artifact(
                                &flow,
                                &run.artifact,
                                &path,
                                iteration::LaunchMode::Standalone,
                                true,
                            )
                        });
                    if let Err(error) = result {
                        self.note =
                            format!("{flow}: pop-out could not reopen the artifact: {error}");
                        self.changed = true;
                    }
                }
            } else {
                self.apps.insert(flow, run);
            }
        }
    }

    fn observe_sources(&mut self) {
        let paths: Vec<_> = self
            .engine
            .flows
            .iter()
            .map(|(id, flow)| (id.clone(), flow.config.repo.clone()))
            .collect();
        for (flow, path) in paths {
            let fingerprint = match source_fingerprint(&path) {
                Ok(value) => value,
                Err(error) => {
                    self.note = format!("{flow}: source observation failed: {error}");
                    if let Some(build) = self.builds.get_mut(&flow) {
                        build.tainted = Some(self.note.clone());
                    }
                    self.changed = true;
                    continue;
                }
            };
            if let Some(build) = self.builds.get_mut(&flow) {
                if !build
                    .active
                    .as_ref()
                    .is_some_and(|phase| phase.kind == PhaseKind::Format)
                    && fingerprint != build.expected_source
                {
                    build.tainted = Some("Source changed during the build/check pipeline".into());
                }
                continue;
            }
            let previous = self.fingerprints.insert(flow.clone(), fingerprint.clone());
            if previous.is_some_and(|previous| previous != fingerprint) {
                if let Err(error) = self.observe(Observation::SourceChanged { flow }) {
                    self.note = error;
                    self.changed = true;
                }
            }
        }
    }
}

fn cargo_base(build: &Build, command: &str) -> Vec<String> {
    vec![
        command.into(),
        "--release".into(),
        "--locked".into(),
        "--manifest-path".into(),
        build.manifest.to_string_lossy().into_owned(),
    ]
}
fn check_phases(build: &Build) -> VecDeque<BuildPhase> {
    let mut targets = build.config.check_targets.clone();
    if !targets.contains(&build.host_target) {
        targets.push(build.host_target.clone());
    }
    targets
        .into_iter()
        .map(|target| {
            let mut args = cargo_base(build, "check");
            args.extend([
                "--workspace".into(),
                "--all-targets".into(),
                "--target".into(),
                target.clone(),
            ]);
            BuildPhase {
                kind: PhaseKind::Check,
                label: format!("warning-free check {target}"),
                program: "cargo".into(),
                args,
            }
        })
        .collect()
}
fn format_phases(build: &Build) -> Result<Vec<BuildPhase>, String> {
    let output = Command::new("cargo")
        .current_dir(&build.owned.path)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--locked",
            "--manifest-path",
        ])
        .arg(&build.manifest)
        .env("CARGO_TARGET_DIR", &build.target_directory)
        .output()
        .map_err(err)?;
    if !output.status.success() {
        return Err(format!(
            "Cannot inspect Rust editions before formatting: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    if output.stdout.len() > 8 * 1024 * 1024 {
        return Err("Cargo package metadata exceeds 8 MiB".into());
    }
    let metadata = json::parse(&output.stdout).map_err(str::to_owned)?;
    let packages = metadata
        .get("packages")
        .and_then(Value::as_arr)
        .ok_or("Cargo metadata has no packages")?;
    let mut editions: Vec<(PathBuf, String)> = vec![];
    for package in packages {
        if let (Some(manifest), Some(edition)) = (
            package.get("manifest_path").and_then(Value::as_str),
            package.get("edition").and_then(Value::as_str),
        ) {
            if let Some(parent) = Path::new(manifest).parent() {
                editions.push((parent.to_path_buf(), edition.into()));
            }
        }
    }
    editions.sort_by_key(|(path, _)| std::cmp::Reverse(path.components().count()));
    let state = git::inspect(&build.owned.path)?;
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for change in state.changes {
        let path = build.owned.path.join(&change.path);
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") || !path.is_file() {
            continue;
        }
        if change
            .path
            .split('/')
            .any(|part| part == "tests" || part == "test")
            || path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(|stem| {
                    stem.ends_with("_tests") || stem.ends_with("_test") || stem.starts_with("test_")
                })
        {
            return Err(format!(
                "Test edits cannot enter this build iteration: {}",
                change.path
            ));
        }
        let edition = editions
            .iter()
            .find(|(root, _)| path.starts_with(root))
            .map(|(_, edition)| edition)
            .ok_or_else(|| format!("No Cargo package owns changed Rust source {}", change.path))?;
        groups
            .entry(edition.clone())
            .or_default()
            .push(path.to_string_lossy().into_owned());
    }
    let mut phases = vec![];
    for (edition, files) in groups {
        for chunk in files.chunks(64) {
            let mut args = vec![
                "--edition".into(),
                edition.clone(),
                "--config".into(),
                "disable_all_formatting=false,skip_children=true".into(),
            ];
            args.extend(chunk.iter().cloned());
            phases.push(BuildPhase {
                kind: PhaseKind::Format,
                label: format!("rustfmt {} changed Rust files", chunk.len()),
                program: "rustfmt".into(),
                args,
            });
        }
    }
    Ok(phases)
}
fn rust_host() -> Result<String, String> {
    let output = Command::new("rustc").arg("-vV").output().map_err(err)?;
    if !output.status.success() {
        return Err("rustc could not report the native target".into());
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .map(str::to_owned)
        .ok_or_else(|| "rustc did not report a host target".into())
}
fn rustc_guard() -> Result<PathBuf, String> {
    Ok(std::env::current_exe()
        .map_err(err)?
        .with_file_name(if cfg!(windows) {
            "director-rustc-guard.exe"
        } else {
            "director-rustc-guard"
        }))
}
fn source_fingerprint(repository: &Path) -> Result<String, String> {
    use std::hash::{Hash, Hasher};
    let state = git::inspect(repository)?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    state.head.hash(&mut hash);
    for change in state.changes {
        change.path.hash(&mut hash);
        change.status.hash(&mut hash);
        if change.status != "??" {
            // Tracked content: the blob id of what is on disk now, or its absence.
            let path = repository.join(&change.path);
            if path.is_file() {
                git::content_hash(&path)?.hash(&mut hash);
            } else {
                "deleted".hash(&mut hash);
            }
        } else {
            let path = repository.join(&change.path);
            let metadata = fs::symlink_metadata(&path).map_err(err)?;
            if !metadata.is_file() {
                return Err("Untracked source entries must be regular files".into());
            }
            if metadata.len() > 16 * 1024 * 1024 {
                return Err(format!("Untracked source {} exceeds 16 MiB", change.path));
            }
            let mut file = File::open(path).map_err(err)?;
            let mut buffer = [0u8; 16384];
            loop {
                let count = file.read(&mut buffer).map_err(err)?;
                if count == 0 {
                    break;
                }
                buffer[..count].hash(&mut hash);
            }
        }
    }
    Ok(format!("{:016x}", hash.finish()))
}
fn find_executable(log: &Path, binary: &str) -> Result<PathBuf, String> {
    let text = bounded_read(log, BUILD_LOG_LIMIT as usize)?;
    let mut executable = None;
    for line in text.lines().filter(|line| line.starts_with('{')) {
        let Ok(value) = json::parse(line.as_bytes()) else {
            continue;
        };
        if value.get("reason").and_then(Value::as_str) != Some("compiler-artifact") {
            continue;
        }
        if value
            .get("target")
            .and_then(|target| target.get("name"))
            .and_then(Value::as_str)
            != Some(binary)
        {
            continue;
        }
        if let Some(path) = value.get("executable").and_then(Value::as_str) {
            executable = Some(PathBuf::from(path));
        }
    }
    executable.ok_or_else(|| "Cargo succeeded without reporting the requested executable".into())
}
fn copy_immutable(source: &Path, destination: &Path) -> Result<(), String> {
    let parent = destination
        .parent()
        .ok_or("Immutable output has no parent")?;
    fs::create_dir_all(parent).map_err(err)?;
    let mut input = File::open(source).map_err(err)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(err)?;
    let result = (|| {
        std::io::copy(&mut input, &mut output).map_err(err)?;
        output.sync_all().map_err(err)?;
        fs::set_permissions(
            destination,
            fs::metadata(source).map_err(err)?.permissions(),
        )
        .map_err(err)
    })();
    if result.is_err() {
        let _ = fs::remove_file(destination);
    }
    result
}
/// What a guarded close reply proved about who owns the app.
enum CloseEvidence {
    /// HTTP 409, or the person's input counter moved: they are using it.
    Refused(String),
    /// The reply ended on the counter the test started with, or the test had
    /// not established a guard yet and the app did not refuse.
    Verified,
    /// A guard exists but no ending counter came back.
    Unknown,
}

fn close_evidence(run: &AppRun) -> CloseEvidence {
    let code = bounded_read(&run.directory.join("close.code"), 16)
        .ok()
        .and_then(|code| code.trim().parse::<u16>().ok());
    if code == Some(409) {
        return CloseEvidence::Refused(
            "the app refused the close with HTTP 409 because a person is using it".into(),
        );
    }
    let ending = bounded_read(&run.directory.join("close.headers"), 64 * 1024)
        .ok()
        .and_then(|headers| test_user_seq_header(&headers));
    match (run.user_seq, ending) {
        (Some(expected), Some(ending)) if expected != ending => CloseEvidence::Refused(format!(
            "the person's input counter moved from {expected} to {ending} during the close"
        )),
        (Some(_), None) => CloseEvidence::Unknown,
        _ => CloseEvidence::Verified,
    }
}

/// Whether automation may still end this process by force. Never an app a
/// person took over. A guarded AI test (automatic close, guard established)
/// only once its latest close request was answered with the unchanged input
/// counter; a request still in flight has proven nothing. Human apps and the
/// person's own close of a test app are not restricted here.
fn may_force_kill(run: &AppRun) -> bool {
    if run.handed_off {
        return false;
    }
    let guarded = run.role == iteration::RunRole::AiTest && !run.operator_close;
    !guarded || run.user_seq.is_none() || (run.close_verified && run.close_request.is_none())
}

/// Automation lets go of an app: nothing closes, kills or replaces it any
/// more except the person's own close from Director.
fn hand_off(run: &mut AppRun, reason: String) {
    run.handed_off = true;
    run.handoff_reason.get_or_insert(reason);
    run.closing = None;
    run.close_stage = 0;
}

fn log_tail(path: &Path, limit: usize) -> Result<String, String> {
    use std::io::Seek;
    let mut file = File::open(path).map_err(err)?;
    let length = file.metadata().map_err(err)?.len();
    file.seek(std::io::SeekFrom::Start(
        length.saturating_sub(limit as u64),
    ))
    .map_err(err)?;
    let mut bytes = vec![];
    file.take(limit as u64)
        .read_to_end(&mut bytes)
        .map_err(err)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
fn log_has_warning(path: &Path) -> Result<bool, String> {
    let file = File::open(path).map_err(err)?;
    if file.metadata().map_err(err)?.len() > BUILD_LOG_LIMIT {
        return Err("Build log exceeds its validation read limit".into());
    }
    let mut read = 0u64;
    for line in BufReader::new(file.take(BUILD_LOG_LIMIT + 1)).lines() {
        let line = line.map_err(err)?;
        read = read.saturating_add(line.len() as u64 + 1);
        if read > BUILD_LOG_LIMIT {
            return Err("Build log grew beyond its validation read limit".into());
        }
        let line = line.trim_start();
        if line.starts_with("warning:")
            || line.starts_with("Warning:")
            || line.starts_with("warning[")
        {
            return Ok(true);
        }
    }
    Ok(false)
}
fn configure_process_group(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(not(unix))]
    {
        let _ = command;
    }
}
fn stop_build_process(child: &mut Child) {
    if child.try_wait().ok().flatten().is_some() {
        return;
    }
    #[cfg(unix)]
    {
        unsafe extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        // Every build was placed in its own process group before spawn.
        // Terminate compiler/test descendants together, without touching other jobs.
        if let Ok(pid) = i32::try_from(child.id()) {
            unsafe {
                kill(-pid, 9);
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
}
fn read_app_log(run: &mut AppRun) -> Result<(), String> {
    use std::io::Seek;
    let mut file = File::open(&run.log).map_err(err)?;
    let length = file.metadata().map_err(err)?.len();
    if length > APP_LOG_LIMIT {
        return Err("App log exceeds 32 MiB; close this app to stop further output".into());
    }
    if length < run.log_offset {
        return Err("Owned app log was truncated; endpoint observation is incomplete".into());
    }
    file.seek(std::io::SeekFrom::Start(run.log_offset))
        .map_err(err)?;
    let mut bytes = vec![];
    file.take(128 * 1024).read_to_end(&mut bytes).map_err(err)?;
    run.log_offset += bytes.len() as u64;
    run.log_partial.push_str(&String::from_utf8_lossy(&bytes));
    while let Some(end) = run.log_partial.find('\n') {
        let line: String = run.log_partial.drain(..=end).collect();
        if line.contains("[makepad-remote]") && line.contains("user closed") {
            run.user_closed = true;
        }
        if let Some(info) = line.strip_prefix("[makepad-remote] listening on 127.0.0.1:") {
            let mut fields = info.split_whitespace();
            let port = fields.next().and_then(|value| value.parse::<u16>().ok());
            let pid = fields
                .next()
                .and_then(|value| value.strip_prefix("pid="))
                .and_then(|value| value.parse::<u32>().ok());
            if pid == Some(run.child.id()) {
                run.port = port;
                if let Some((_, path)) = line.split_once(" grabs=") {
                    run.grab_directory = Some(PathBuf::from(path.trim()));
                }
            }
        }
    }
    if run.log_partial.len() > 128 * 1024 {
        run.log_partial.clear();
        return Err("App emitted an oversized log line".into());
    }
    Ok(())
}
fn retain_close_capture(run: &AppRun) -> Result<(), String> {
    let path = run.directory.join("close.json");
    if !path.exists() {
        return Ok(());
    }
    let text = bounded_read(&path, 1024 * 1024)?;
    let value = json::parse(text.as_bytes()).map_err(str::to_owned)?;
    let Some(grab_directory) = &run.grab_directory else {
        return Ok(());
    };
    let root = fs::canonicalize(grab_directory).map_err(err)?;
    let mut paths = vec![];
    if let Some(path) = value.get("png").and_then(Value::as_str) {
        paths.push(path.to_owned());
    }
    if let Some(Value::Arr(values)) = value.get("png") {
        for value in values {
            if let Some(path) = value.as_str() {
                paths.push(path.to_owned());
            }
        }
    }
    if let Some(Value::Arr(windows)) = value.get("windows") {
        for window in windows {
            if let Some(path) = window.get("png").and_then(Value::as_str) {
                paths.push(path.to_owned());
            }
        }
    }
    for (index, path) in paths.into_iter().take(16).enumerate() {
        let source = fs::canonicalize(path).map_err(err)?;
        if !source.starts_with(&root)
            || fs::metadata(&source).map_err(err)?.len() > 16 * 1024 * 1024
        {
            return Err(
                "Final capture is outside its owned app grab directory or exceeds 16 MiB".into(),
            );
        }
        let destination = run.directory.join(format!("frozen-{index}.png"));
        if !destination.exists() {
            copy_immutable(&source, &destination)?;
        }
    }
    Ok(())
}
