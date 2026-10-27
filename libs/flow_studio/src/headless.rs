//! A Flow host without a window: `<app> --headless <package>` and
//! `<app> --check <package>`, for any app that runs workflow packages (the
//! Flow app, and apps embedding the studio with their own node types).
//!
//! Opens a workflow package, loads its entry flow into the embedded flow
//! engine, runs it and prints the result. Everything goes through the
//! engine's bounded nonblocking handle: commands are submitted, never
//! called; replies, transitions and observations are polled, never awaited;
//! a gap in the transition lane is recovered from the snapshot. This main
//! thread is not a UI thread, so between polls it simply sleeps.

use makepad_flow::document::ident::{DNode, ParamAddr, ParamRef};
use makepad_flow::document::interface::FlowInterface;
use makepad_flow::document::package::Package;
use makepad_flow::runtime::{
    Engine, EngineCommand, EngineConfig, EngineHandle, EngineSnapshot, HostServices, Refusal, Reply,
    ReplyPayload, RequestId, StdHostWork, Submit, Transition, TransitionKind, TransitionPoll,
    READY_REQUEST,
};
use makepad_flow::{InstanceId, Literal, Owner, PortType, PortValue, RunEvent, RunId, RunState, Sequence, Shape, Value, ValueSlotRef};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The usage text, with the app's program name.
fn usage(program: &str) -> String {
    format!("\
usage:
  {program} [--package <package-dir>] [--window-size <width>x<height>]
  {program} --headless <package-dir> [--input <node>=<value>]... [--output <node>]...
                                 [--control <node>.<param>=<number>]... [--quiet]
  {program} --check <package-dir>

  <value> is text; for a json or list input it is JSON; `@<file>` reads a
  media input from a file. --control writes a control-rate parameter; only
  endpoints a prepared plan admitted can be written. Without --headless or
  --check, `{program}` opens its window: --package opens a package in it,
  --window-size sets its size (200..4096 each, default 1800x1000).")
}

const POLL: Duration = Duration::from_millis(2);
const REPLY_DEADLINE: Duration = Duration::from_secs(60);
const STOP_DEADLINE: Duration = Duration::from_secs(15);

/// Whether the arguments ask for the headless host rather than the window.
pub fn wants_headless(args: &[String]) -> bool {
    args.iter()
        .any(|arg| matches!(arg.as_str(), "--headless" | "--check" | "--help" | "-h"))
}

/// The registries the headless host runs packages with.
pub type Registries = fn() -> Result<(Arc<makepad_flow::document::Registry>, Arc<makepad_flow::engine::providers::ExecutorRegistry>), String>;

/// Run the headless host with the process arguments (program name removed),
/// the app's node types and its program name for the usage text.
pub fn main(args: Vec<String>, registries: Registries, program: &str) -> ExitCode {
    match run(args, registries, program) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("flow: {message}");
            ExitCode::from(2)
        }
    }
}

struct Args {
    package: PathBuf,
    check_only: bool,
    /// Print the entry flow in the graph writer's form and exit.
    canonical: bool,
    inputs: Vec<(String, String)>,
    outputs: Vec<String>,
    controls: Vec<(String, String, f64)>,
    quiet: bool,
}

fn parse(args: Vec<String>, program: &str) -> Result<Args, String> {
    let usage = usage(program);
    let mut package = None;
    let mut headless = false;
    let mut check_only = false;
    let mut canonical = false;
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();
    let mut controls = Vec::new();
    let mut quiet = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => return Err(usage),
            "--headless" => headless = true,
            "--check" => check_only = true,
            "--canonical" => canonical = true,
            "--quiet" => quiet = true,
            "--input" => {
                let pair = args.next().ok_or("--input needs <node>=<value>")?;
                let (node, value) = pair.split_once('=').ok_or("--input needs <node>=<value>")?;
                inputs.push((node.to_string(), value.to_string()));
            }
            "--output" => outputs.push(args.next().ok_or("--output needs a node id")?),
            "--control" => {
                let usage = "--control needs <node>.<param>=<number>";
                let pair = args.next().ok_or(usage)?;
                let (target, value) = pair.split_once('=').ok_or(usage)?;
                let (node, param) = target.split_once('.').ok_or(usage)?;
                let value: f64 = value.parse().map_err(|_| usage)?;
                controls.push((node.to_string(), param.to_string(), value));
            }
            other if other.starts_with("--") => return Err(format!("unknown option `{other}`\n{usage}")),
            other => {
                if package.replace(PathBuf::from(other)).is_some() {
                    return Err(format!("more than one package directory\n{usage}"));
                }
            }
        }
    }
    let package = package.ok_or_else(|| usage.clone())?;
    if !headless && !check_only {
        return Err(format!("a package directory needs --headless or --check\n{usage}"));
    }
    Ok(Args { package, check_only, canonical, inputs, outputs, controls, quiet })
}

/// The engine handle plus what was polled and not consumed yet.
struct Host {
    handle: EngineHandle,
    replies: Vec<Reply>,
    transitions: Vec<Transition>,
    /// The transition lane reported a gap. This is the snapshot to
    /// resynchronise from; it is handled before any transition is acted on.
    resync: Option<Arc<EngineSnapshot>>,
    quiet: bool,
}

impl Host {
    fn pump(&mut self) {
        self.handle.poll_replies(&mut self.replies, 64);
        if let TransitionPoll::Resync { snapshot, .. } =
            self.handle.poll_transitions(&mut self.transitions, 256)
        {
            // The snapshot covers everything polled so far, this call's
            // entries included (the engine already removed those).
            self.transitions.retain(|transition| transition.seq > snapshot.transition_seq);
            self.resync = Some(snapshot);
        }
        let mut observations = Vec::new();
        self.handle.poll_observations(&mut observations, 256);
        if !self.quiet {
            for observation in observations {
                if let RunEvent::NodeProgress { node, stage, permille, .. } = observation.event {
                    match permille {
                        Some(permille) => eprintln!("  {node}: {stage} {}%", permille / 10),
                        None => eprintln!("  {node}: {stage}"),
                    }
                }
            }
        }
    }

    fn take_reply(&mut self, id: RequestId) -> Option<Reply> {
        let index = self.replies.iter().position(|reply| reply.id == id)?;
        Some(self.replies.remove(index))
    }

    fn wait_reply(&mut self, id: RequestId, deadline: Duration) -> Result<ReplyPayload, String> {
        let started = Instant::now();
        loop {
            self.pump();
            if let Some(reply) = self.take_reply(id) {
                return reply.result.map_err(|error| error.to_string());
            }
            if !self.handle.is_alive() {
                return Err("the engine is gone".to_string());
            }
            if started.elapsed() > deadline {
                return Err("the engine did not answer in time".to_string());
            }
            std::thread::sleep(POLL);
        }
    }

    /// Submit and wait for the terminal reply. A full queue is retried; the
    /// refused command always comes back, so nothing is lost.
    fn request(&mut self, mut command: EngineCommand) -> Result<ReplyPayload, String> {
        let started = Instant::now();
        let id = loop {
            match self.handle.submit(command) {
                Submit::Accepted(id) => break id,
                Submit::Full(returned) => {
                    if started.elapsed() > REPLY_DEADLINE {
                        return Err("the engine's command queue stayed full".to_string());
                    }
                    command = returned;
                    self.pump();
                    std::thread::sleep(POLL);
                }
                Submit::Refused(_, refusal) => {
                    return Err(match refusal {
                        Refusal::UnknownEndpoint => {
                            "refused: no prepared plan admitted that control endpoint".to_string()
                        }
                    })
                }
                Submit::Closed(_) => return Err("the engine stopped".to_string()),
            }
        };
        self.wait_reply(id, REPLY_DEADLINE)
    }
}

fn run(args: Vec<String>, registries: Registries, program: &str) -> Result<ExitCode, String> {
    let args = parse(args, program)?;
    let package = Package::open(&args.package).map_err(|error| error.to_string())?;
    // This host binds no capability yet. A package that requires one cannot
    // run here, and saying so beats parsing the declaration and ignoring it.
    let unmet: Vec<String> = package
        .manifest
        .roles
        .iter()
        .filter(|role| role.required)
        .map(|role| format!("`{}` ({})", role.role, role.kind))
        .collect();
    if !unmet.is_empty() {
        return Err(format!(
            "package `{}` requires capability roles this headless host cannot bind: {}",
            package.manifest.id.0,
            unmet.join(", ")
        ));
    }
    if !args.quiet {
        for role in &package.manifest.roles {
            eprintln!("flow: optional role `{}` ({}) is unbound in the headless host", role.role, role.kind);
        }
    }
    if package.manifest.surface.is_some() && !args.quiet {
        eprintln!("flow: the package's surface is not shown by the headless host");
    }

    // The engine spills large values here. Only the directory this process
    // created is removed, and only what the value store put in it.
    let scratch = std::env::temp_dir().join(format!("flow-headless-{}", std::process::id()));
    let result = run_package(&args, &package, &scratch, registries);
    let _ = std::fs::remove_dir_all(scratch.join("values"));
    let _ = std::fs::remove_dir(&scratch);
    result
}

fn run_package(args: &Args, package: &Package, scratch: &Path, registries: Registries) -> Result<ExitCode, String> {
    // Two pool workers for finite jobs; the engine worker and one run lane
    // are the only long-lived loops this host admits.
    let work = StdHostWork::new(2, 32, 2).map_err(|error| format!("start worker pool: {error}"))?;
    let mut config = EngineConfig::new(scratch.join("values"));
    let (registry, executors) = registries()?;
    config.registry = registry.clone();
    config.executors = executors;
    config.run_lanes = 1;
    let handle = Engine::start(
        config,
        HostServices {
            work,
            results: None,
            log: Arc::new(|message: &str| eprintln!("flow: {message}")),
            // This host polls on its own clock.
            wake: None,
        },
    )
    .map_err(|error| error.to_string())?;
    let mut host = Host {
        handle,
        replies: Vec::new(),
        transitions: Vec::new(),
        resync: None,
        quiet: args.quiet,
    };

    match host.wait_reply(READY_REQUEST, REPLY_DEADLINE)? {
        ReplyPayload::Ready => {
            if !args.quiet {
                eprintln!("flow: engine ready");
            }
        }
        other => return Err(format!("expected Ready, got {other:?}")),
    }

    let outcome = drive(&mut host, args, package, &registry);

    // Stop through the API whatever happened above, and wait for `Stopped`.
    let stopped = match host.handle.stop() {
        Submit::Accepted(id) => match host.wait_reply(id, STOP_DEADLINE) {
            Ok(ReplyPayload::Stopped { abandoned_runs: 0 }) => Ok(()),
            Ok(ReplyPayload::Stopped { abandoned_runs }) => Err(format!(
                "the engine stopped with {abandoned_runs} runs still in flight"
            )),
            Ok(other) => Err(format!("unexpected reply to stop: {other:?}")),
            Err(error) => Err(error),
        },
        Submit::Closed(_) => Ok(()),
        Submit::Full(_) | Submit::Refused(..) => Err("the engine did not accept stop".to_string()),
    };
    if !args.quiet {
        match &stopped {
            Ok(()) => eprintln!("flow: engine stopped"),
            Err(error) => eprintln!("flow: {error}"),
        }
    }
    let code = outcome?;
    stopped?;
    Ok(code)
}

fn drive(host: &mut Host, args: &Args, package: &Package, registry: &makepad_flow::document::Registry) -> Result<ExitCode, String> {
    // The same namespaced name the window installs a package under.
    let flow = package.definition_name();
    // The same closure the window installs: pinned modules in dependency
    // order, refused here with the offending file and line otherwise.
    let closure = package
        .closure()
        .map_err(|error| format!("{}:{}:{}: {}", error.file, error.line, error.col, error.message))?;
    let loaded = host.request(EngineCommand::LoadDefinition {
        name: flow.clone(),
        source: package.entry_source.clone(),
        file_name: package.entry_file.clone(),
        closure: Some(Arc::new(closure)),
        ephemeral: true,
    })?;
    let ReplyPayload::Definition { revision, error, interface, graph, .. } = loaded else {
        return Err(format!("unexpected reply to LoadDefinition: {loaded:?}"));
    };
    if let Some(error) = error {
        return Err(format!("{}:{}:{}: {}", error.file, error.line, error.col, error.message));
    }
    if args.canonical {
        // The entry flow in the graph writer's form: what a canvas edit
        // would write back, so an author can adopt it as the file.
        let graph = graph.ok_or("the engine returned no graph for a valid flow")?;
        print!("{}", makepad_flow::graph::write_with_registry(&graph, registry).map_err(|error| error.to_string())?);
        return Ok(ExitCode::SUCCESS);
    }
    let interface = interface.ok_or("the engine returned no interface for a valid flow")?;
    if !args.quiet {
        eprintln!(
            "flow: loaded `{}` ({}) revision {revision}: {} inputs, {} outputs",
            package.manifest.id.0,
            interface.label.as_deref().unwrap_or(&flow),
            interface.inputs.len(),
            interface.outputs.len()
        );
    }
    if args.check_only {
        for input in &interface.inputs {
            println!(
                "input  {} : {}{}",
                input.node,
                Shape { ty: input.ty.clone(), many: input.many }.describe(),
                if input.ask { " (ask)" } else { "" }
            );
        }
        for output in &interface.outputs {
            println!("output {} : {}", output.node, Shape { ty: output.ty.clone(), many: output.many }.describe());
        }
        return Ok(ExitCode::SUCCESS);
    }

    let ReplyPayload::Instance(instance) =
        host.request(EngineCommand::Instantiate {
            flow,
            label: None,
            pinned: false,
            owner: Owner::Service,
            inputs: Default::default(),
        })?
    else {
        return Err("unexpected reply to Instantiate".to_string());
    };

    // Package defaults first, then the command line on top: for a one
    // input the last value given; for an ordered input every `--input`
    // for it, in order, replacing the default as a whole.
    let mut inputs: Vec<(String, String, PortValue)> = Vec::new();
    for (node, literal) in &package.manifest.defaults {
        inputs.push(input_value(&interface, node, InputText::Literal(literal), registry)?);
    }
    let mut given: Vec<String> = Vec::new();
    for (node, text) in &args.inputs {
        let (node, port, value) = input_value(&interface, node, InputText::Arg(text), registry)?;
        let ordered = interface.input(&node).is_some_and(|input| input.many.is_some());
        if ordered && given.iter().any(|seen| *seen == node) {
            let slot = inputs
                .iter_mut()
                .find(|(existing, _, _)| *existing == node)
                .expect("an ordered input seen before is listed");
            match (&mut slot.2, value) {
                (PortValue::Seq(sequence), PortValue::Seq(more)) => sequence.values.extend(more.values),
                _ => unreachable!("an ordered input carries a sequence"),
            }
        } else {
            inputs.retain(|(existing, _, _)| *existing != node);
            inputs.push((node.clone(), port, value));
        }
        given.push(node);
    }
    if !inputs.is_empty() {
        host.request(EngineCommand::SetInputs { instance: instance.clone(), inputs, actor: "flow".to_string() })?;
    }

    for (node, param, value) in &args.controls {
        let addr = ParamAddr {
            instance: instance.clone(),
            def: ParamRef { node: DNode::root(node.clone()), param: param.clone() },
        };
        // `request` reports a refusal; an admitted write replies like any
        // other command.
        host.request(EngineCommand::SetControl { addr, value: *value })
            .map_err(|error| format!("control `{node}.{param}`: {error}"))?;
    }

    let outputs = (!args.outputs.is_empty()).then(|| args.outputs.clone());
    let ReplyPayload::RunStarted { run, .. } =
        host.request(EngineCommand::StartRun { instance: instance.clone(), outputs })?
    else {
        return Err("unexpected reply to StartRun".to_string());
    };

    let finished = follow_run(host, &interface, &instance, &run)?;
    for warning in &finished.warnings {
        eprintln!("flow: warning: {warning}");
    }
    for (node, value) in &finished.outputs {
        match value {
            PortValue::One(value) => println!("{node}: {}", describe(value)),
            PortValue::Seq(sequence) => {
                println!("{node}: ({} {} values)", sequence.values.len(), sequence.ty.as_str());
                for (index, value) in sequence.values.iter().enumerate() {
                    println!("{node}[{index}]: {}", describe(value));
                }
            }
        }
    }
    Ok(match finished.state {
        RunState::Done => ExitCode::SUCCESS,
        state => {
            eprintln!("flow: run {state:?}");
            ExitCode::from(1)
        }
    })
}

struct Finished {
    state: RunState,
    outputs: Vec<(String, PortValue)>,
    warnings: Vec<String>,
}

/// Follow the run on the transition lane until it finishes. A waiting Ask is
/// answered from stdin, or the run is cancelled when stdin is closed. If the lane
/// reports a gap, the snapshot says whether the run already finished and the
/// outputs are read back by digest.
fn follow_run(
    host: &mut Host,
    interface: &FlowInterface,
    instance: &InstanceId,
    run: &RunId,
) -> Result<Finished, String> {
    // The Ask node already prompted for, so a gap recovery that finds the
    // same waiting record in the snapshot does not prompt twice.
    let mut asked: Option<String> = None;
    loop {
        host.pump();
        // A gap first: the snapshot is the truth, and nothing older than it
        // is acted on. The run may have finished, or it may be parked on an
        // Ask whose NodeWaiting this host never saw.
        if let Some(snapshot) = host.resync.take() {
            if let Some(finished) = finished_from_snapshot(host, &snapshot, run)? {
                return Ok(finished);
            }
            if let Some((node, question, ty)) = waiting_from_snapshot(&snapshot, instance, run) {
                if asked.as_deref() != Some(node.as_str()) {
                    asked = Some(node.clone());
                    if !answer_ask(host, interface, instance, &node, &question, ty)? {
                        eprintln!("flow: no answer on stdin for `{node}`; cancelling the run");
                        host.request(EngineCommand::CancelRun { run: run.clone() })?;
                    }
                }
            } else {
                asked = None;
            }
        }
        for transition in std::mem::take(&mut host.transitions) {
            let TransitionKind::Run { run: event_run, event, .. } = transition.kind else {
                continue;
            };
            if &event_run != run {
                continue;
            }
            match event {
                RunEvent::NodeFailed { node, error } => eprintln!("flow: node `{node}` failed: {error}"),
                RunEvent::NodeSkipped { node, reason } if !host.quiet => {
                    eprintln!("flow: node `{node}` skipped: {reason}")
                }
                RunEvent::NodeWaiting { node, question, ty, .. } => {
                    asked = Some(node.clone());
                    if !answer_ask(host, interface, instance, &node, &question, ty)? {
                        // Nobody can answer: cancel rather than wait forever.
                        eprintln!("flow: no answer on stdin for `{node}`; cancelling the run");
                        host.request(EngineCommand::CancelRun { run: run.clone() })?;
                    }
                }
                RunEvent::NodeAnswered { .. } => asked = None,
                RunEvent::RunFinished { state, outputs, warnings, .. } => {
                    return Ok(Finished { state, outputs, warnings });
                }
                _ => {}
            }
            if host.resync.is_some() {
                // Answering an Ask pumped the lanes and found a gap: handle
                // it before the rest of this batch.
                break;
            }
        }
        if !host.handle.is_alive() {
            return Err("the engine is gone; the run's result is unknown".to_string());
        }
        std::thread::sleep(POLL);
    }
}

/// The Ask the instance is parked on for this run, according to the snapshot.
fn waiting_from_snapshot(
    snapshot: &EngineSnapshot,
    instance: &InstanceId,
    run: &RunId,
) -> Option<(String, String, PortType)> {
    let row = snapshot.instances.iter().find(|row| row.instance == instance.0)?;
    if row.run.as_deref() != Some(run.0.as_str()) {
        return None;
    }
    let waiting = row.waiting.as_ref()?;
    Some((waiting.node.clone(), waiting.question.clone(), waiting.ty.clone()))
}

fn finished_from_snapshot(
    host: &mut Host,
    snapshot: &EngineSnapshot,
    run: &RunId,
) -> Result<Option<Finished>, String> {
    let Some(row) = snapshot.runs.iter().find(|row| row.run_id == run.0) else {
        return Err("the run left the engine's history before its result was read".to_string());
    };
    if !matches!(row.state, RunState::Done | RunState::Failed | RunState::Cancelled) {
        return Ok(None);
    }
    let read = |host: &mut Host, node: &str, value: &makepad_flow::ValueRef| -> Result<Value, String> {
        let digest = digest_bytes(&value.digest).ok_or("the snapshot carries a malformed digest")?;
        match host.request(EngineCommand::ReadTypedValue { digest, ty: value.ty.clone() })? {
            ReplyPayload::Value(Some(mut stored)) => {
                if stored.ty != value.ty || stored.digest != digest || stored.bytes.len() != value.bytes {
                    return Err(format!("stored output `{node}` does not match its snapshot reference"));
                }
                // A byte-identical value of the same type may have been stored
                // with another MIME. This output's reference owns its metadata.
                stored.content_type = value.content_type.clone();
                Ok(stored)
            }
            ReplyPayload::Value(None) => Err(format!("output `{node}` is no longer stored")),
            other => Err(format!("unexpected reply to ReadTypedValue: {other:?}")),
        }
    };
    let mut outputs = Vec::new();
    for (node, slot) in row.output_slots()? {
        let value = match slot {
            ValueSlotRef::One(value) => PortValue::One(read(host, &node, &value)?),
            ValueSlotRef::Seq(sequence) => {
                let values = sequence.values.iter()
                    .map(|value| read(host, &node, value))
                    .collect::<Result<Vec<_>, _>>()?;
                PortValue::Seq(Sequence { ty: sequence.ty.clone(), values })
            }
        };
        outputs.push((node, value));
    }
    Ok(Some(Finished {
        state: row.state,
        outputs,
        warnings: vec!["transitions were dropped; the result was read from the snapshot".to_string()],
    }))
}

fn answer_ask(
    host: &mut Host,
    interface: &FlowInterface,
    instance: &InstanceId,
    node: &str,
    question: &str,
    ty: PortType,
) -> Result<bool, String> {
    let port = interface
        .input(node)
        .map(|input| input.port.clone())
        .unwrap_or_else(|| ty.as_str().to_string());
    eprintln!("{question}");
    let mut line = String::new();
    let read = std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|error| format!("read the answer: {error}"))?;
    if read == 0 {
        // End of input is not an answer.
        return Ok(false);
    }
    let value = text_value(ty, line.trim_end_matches(['\r', '\n']))?;
    host.request(EngineCommand::SetInputs {
        instance: instance.clone(),
        inputs: vec![(node.to_string(), port, PortValue::One(value))],
        actor: "flow".to_string(),
    })?;
    Ok(true)
}

enum InputText<'a> {
    Arg(&'a str),
    Literal(&'a Literal),
}

fn input_value(
    interface: &FlowInterface,
    node: &str,
    text: InputText<'_>,
    registry: &makepad_flow::document::Registry,
) -> Result<(String, String, PortValue), String> {
    let input = interface
        .input(node)
        .filter(|input| !input.ask)
        .ok_or_else(|| format!("the flow has no Input node `{node}`"))?;
    let one = |text: &str| -> Result<Value, String> {
        match text.strip_prefix('@') {
            Some(path) if input.ty.is_media() || matches!(&input.ty, PortType::Registered(_)) => {
                let bytes = std::fs::read(path).map_err(|error| format!("{path}: {error}"))?;
                Value::artifact(input.ty.clone(), content_type(path), bytes, registry.types())
            }
            _ => text_value(input.ty.clone(), text),
        }
    };
    // An ordered input's default is a literal list; one `--input` is one
    // element (the caller gathers them in order).
    let value = match (text, input.many) {
        (InputText::Literal(Literal::Arr(items)), Some(_)) => PortValue::Seq(Sequence {
            ty: input.ty.clone(),
            values: items
                .iter()
                .map(|item| Value::from_literal(input.ty.clone(), item))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| format!("default for `{node}`: {error}"))?,
        }),
        (InputText::Literal(_), Some(_)) => {
            return Err(format!("default for `{node}`: an ordered input's default is a list"))
        }
        (InputText::Literal(literal), None) => PortValue::One(
            Value::from_literal(input.ty.clone(), literal).map_err(|error| format!("default for `{node}`: {error}"))?,
        ),
        (InputText::Arg(text), Some(_)) => PortValue::Seq(Sequence { ty: input.ty.clone(), values: vec![one(text)?] }),
        (InputText::Arg(text), None) => PortValue::One(one(text)?),
    };
    Ok((node.to_string(), input.port.clone(), value))
}

fn text_value(ty: PortType, text: &str) -> Result<Value, String> {
    match ty {
        PortType::Text => Ok(Value::text(text)),
        PortType::Json => Ok(Value::json(text)),
        PortType::List => Ok(Value::list(text)),
        ty => Err(format!("a {} value is given as `@<file>`", ty.as_str())),
    }
}

fn content_type(path: &str) -> &'static str {
    match Path::new(path).extension().and_then(|ext| ext.to_str()).unwrap_or("") {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "wav" => "audio/wav",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "glb" => "model/gltf-binary",
        _ => "application/octet-stream",
    }
}

fn describe(value: &Value) -> String {
    match &value.ty {
        PortType::Text | PortType::Json | PortType::List => value
            .as_text()
            .map(str::to_string)
            .unwrap_or_else(|error| format!("<{error}>")),
        ty => format!("{} {} bytes sha256:{}", ty.as_str(), value.bytes.len(), value.digest_hex()),
    }
}

fn digest_bytes(hex: &str) -> Option<[u8; 32]> {
    let hex = hex.strip_prefix("sha256:").unwrap_or(hex);
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (index, pair) in hex.as_bytes().chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(pair).ok()?;
        out[index] = u8::from_str_radix(text, 16).ok()?;
    }
    Some(out)
}
