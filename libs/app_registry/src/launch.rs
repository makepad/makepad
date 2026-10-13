//! The shared build-then-launch flow for hosted registry apps. Hosts supply
//! process I/O and their hub environment; this module chooses the executable,
//! build command, arguments and transition from a completed build to the app.

use crate::{build_argv, built_binary, has_sources, AppEntry};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};

/// Automatic preparation can require an existing binary; an explicit open
/// can build a missing binary or rebuild one the host knows is stale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildPolicy {
    BuiltOnly,
    IfMissing,
    Rebuild,
}

pub enum LaunchEvent {
    Started,
    Exited(ExitStatus),
}

/// Each piped stream ends after its final line, so even a very fast failed
/// build delivers all diagnostics before the host displays its result.
#[derive(Clone, Debug)]
pub enum ProcessOutput {
    Line(String),
    Closed,
}

/// The built binary, `--stdin-loop`, registry arguments, then host arguments.
pub fn launch_argv(app: &AppEntry, root: Option<&Path>, extra: &[String]) -> Result<(PathBuf, Vec<String>), String> {
    let program = built_binary(&app.target(), root).ok_or_else(|| match root {
        Some(_) => format!("not built yet: {}", app.bin),
        None => format!("binary not found: {}", app.bin),
    })?;
    let mut args = vec!["--stdin-loop".to_string()];
    args.extend(app.args.iter().map(|arg| arg.to_string()));
    args.extend(extra.iter().cloned());
    Ok((program, args))
}

/// A host keeps the returned child for its normal process-group cleanup.
/// `poll` only uses `try_wait`: compilation never blocks the UI thread.
pub struct AppLaunch {
    app: &'static AppEntry,
    root: Option<PathBuf>,
    extra: Vec<String>,
    building: bool,
    status: String,
    progress: String,
    diagnostic: String,
    last_output: String,
    closed_streams: usize,
}

impl AppLaunch {
    /// The spawn callback pipes both build streams through `read_output`
    /// and feeds its events to `output`. App I/O is the host's choice.
    pub fn start(
        app: &'static AppEntry,
        root: Option<PathBuf>,
        extra: Vec<String>,
        policy: BuildPolicy,
        mut spawn: impl FnMut(Command, bool) -> Result<Child, String>,
    ) -> Result<(Self, Child), String> {
        let building = root.is_some() && match policy {
            BuildPolicy::BuiltOnly => false,
            BuildPolicy::IfMissing => built_binary(&app.target(), root.as_deref()).is_none(),
            BuildPolicy::Rebuild => true,
        };
        let launch = Self {
            app, root, extra, building,
            status: if building { format!("building {}…", app.label) } else { format!("starting {}…", app.label) },
            progress: String::new(),
            diagnostic: String::new(),
            last_output: String::new(),
            closed_streams: 0,
        };
        let child = spawn(launch.command()?, building)?;
        Ok((launch, child))
    }

    pub fn is_building(&self) -> bool {
        self.building
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    /// The parsed compile stage, without the app name or a host's wording.
    pub fn progress(&self) -> &str {
        &self.progress
    }

    /// Feed cleaned output from the build's pipes before polling its child.
    /// Keep compiler diagnostics for a failed build, without rescanning logs.
    pub fn output(&mut self, output: &ProcessOutput) -> Option<(String, bool)> {
        let text = match output {
            ProcessOutput::Line(text) => text,
            ProcessOutput::Closed => {
                if self.building { self.closed_streams += 1; }
                return None;
            }
        };
        if !self.building { return cargo_progress(text); }
        self.last_output.clone_from(text);
        if text.starts_with("error:") || text.starts_with("error[") || !self.diagnostic.is_empty() {
            self.diagnostic.push_str(text);
            self.diagnostic.push('\n');
        }
        let (stage, linked) = cargo_progress(text)?;
        if self.progress == stage { return None; }
        self.status = format!("building {}… {stage}", self.app.label);
        self.progress = stage;
        Some((self.status.clone(), linked))
    }

    fn command(&self) -> Result<Command, String> {
        let (program, args) = if self.building {
            let root = self.root.as_deref().ok_or("no checkout to build from")?;
            if !has_sources(&self.app.target(), root) {
                return Err(format!("sources not found: {}", self.app.dir));
            }
            build_argv(&self.app.target(), root)
        } else {
            launch_argv(self.app, self.root.as_deref(), &self.extra)?
        };
        let mut command = Command::new(program);
        command.args(args);
        if let Some(root) = &self.root {
            command.current_dir(root);
        }
        if self.building {
            command.env("CARGO_TERM_COLOR", "never")
                .env("CARGO_TERM_PROGRESS_WHEN", "always")
                .env("CARGO_TERM_PROGRESS_WIDTH", "100")
                .env("MAKEPAD_BUILD_PROGRESS_LINES", "1");
        }
        Ok(command)
    }

    /// A successful build launches the same registry entry exactly once.
    /// Hosts only react to Started/Exited or display the returned error.
    pub fn poll(
        &mut self,
        child: &mut Option<Child>,
        mut spawn: impl FnMut(Command, bool) -> Result<Child, String>,
    ) -> Result<Option<LaunchEvent>, String> {
        // The two streams are stdout and stderr, both piped by the host's
        // build spawn. Their workers never block this polling thread.
        if self.building && self.closed_streams < 2 { return Ok(None); }
        let Some(process) = child.as_mut() else { return Ok(None) };
        let Some(exit) = process.try_wait().map_err(|error| format!("{}: {error}", self.app.label))? else { return Ok(None) };
        if !self.building {
            // Keep the handle for the host's eventual process-group cleanup.
            return Ok(Some(LaunchEvent::Exited(exit)));
        }
        child.take();
        self.building = false;
        self.progress.clear();
        if !exit.success() {
            self.status = format!("building {} failed ({exit})", self.app.label);
            let detail = if self.diagnostic.is_empty() { &self.last_output } else { &self.diagnostic };
            if !detail.is_empty() {
                self.status.push_str(":\n");
                self.status.push_str(detail.trim_end());
            }
            return Err(self.status.clone());
        }
        self.diagnostic.clear();
        self.last_output.clear();
        *child = Some(spawn(self.command()?, false)?);
        self.status = format!("starting {}…", self.app.label);
        Ok(Some(LaunchEvent::Started))
    }
}

/// Read a pipe on a host's long-lived worker, splitting both newlines and
/// Cargo's carriage-return progress redraws. The host owns wakeups/channels.
pub fn read_output(stream: impl Read, mut log: Option<std::fs::File>, mut emit: impl FnMut(ProcessOutput) -> bool) {
    let mut bytes = BufReader::new(stream).bytes();
    let mut line = Vec::new();
    loop {
        let byte = match bytes.next() {
            Some(Ok(byte)) => Some(byte),
            Some(Err(_)) => break,
            None => None,
        };
        if let Some(byte) = byte.filter(|b| *b != b'\n' && *b != b'\r') {
            line.push(byte);
            continue;
        }
        let raw = String::from_utf8_lossy(&line);
        let text = strip_ansi(&raw).trim().to_string();
        if let Some(file) = log.as_mut().filter(|_| !text.is_empty() && !text.starts_with("Building [")) {
            let _ = writeln!(file, "{raw}");
        }
        if !text.is_empty() && !emit(ProcessOutput::Line(text)) {
            return;
        }
        line.clear();
        if byte.is_none() {
            break;
        }
    }
    emit(ProcessOutput::Closed);
}

/// Cargo (and rustc) paint their progress; a pipe usually turns that off,
/// but a stray CSI sequence must never reach a Label.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            if c != '\r' {
                out.push(c);
            }
            continue;
        }
        // ESC [ … <final byte 0x40..0x7e>, or ESC ] … BEL (OSC).
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                for c in chars.by_ref() {
                    if c == '\u{7}' {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Cargo output that changes the launch panel. Compiler diagnostics stay in
/// the client log and do not overwrite a useful build stage with source text.
pub fn cargo_progress(raw: &str) -> Option<(String, bool)> {
    let raw = raw.trim();
    if raw.starts_with("Blocking waiting for file lock") {
        Some(("waiting for another build…".into(), false))
    } else if raw.starts_with("Running ") || raw.starts_with("Finished ") {
        Some(("launching…".into(), true))
    } else if let Some(rest) = raw.strip_prefix("Building [") {
        // "=====>   ] 73/88: windows, makepad-script"
        let (_, rest) = rest.split_once(']')?;
        let (count, crates) = rest.trim().split_once(':').unwrap_or((rest.trim(), ""));
        let crates = crates.trim();
        Some((
            if crates.is_empty() { format!("compiling {count} crates…") } else { format!("compiling {count} crates · {crates}…") },
            false,
        ))
    } else if let Some(rest) = raw.strip_prefix("Compiling ") {
        let package = rest.split(" (").next().unwrap_or(rest).trim();
        Some((format!("compiling {package}…"), false))
    } else if raw.starts_with("error:") || raw.starts_with("error[") {
        Some(("build failed — see the app log".into(), false))
    } else {
        None
    }
}
