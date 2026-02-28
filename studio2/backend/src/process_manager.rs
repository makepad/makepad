use crate::dispatch::StudioEvent;
use crate::protocol::{BuildInfo, QueryId};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

struct RunningBuild {
    info: BuildInfo,
    child: Arc<Mutex<Child>>,
}

#[derive(Default)]
pub struct ProcessManager {
    builds: HashMap<QueryId, RunningBuild>,
}

impl ProcessManager {
    pub fn start_cargo_run(
        &mut self,
        build_id: QueryId,
        mount: String,
        cwd: &Path,
        args: Vec<String>,
        env: HashMap<String, String>,
        studio_addr: Option<String>,
        event_tx: std::sync::mpsc::Sender<StudioEvent>,
    ) -> Result<BuildInfo, String> {
        if self.builds.contains_key(&build_id) {
            return Err(format!("build already exists: {}", build_id.0));
        }

        let package = parse_package_name(&args).unwrap_or_else(|| "unknown".to_string());
        let mut command = Command::new("cargo");
        command.args(&args);
        command.current_dir(cwd);
        command.stdin(Stdio::piped());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        let mut child_env = env;
        child_env.insert("RUST_BACKTRACE".to_string(), "1".to_string());
        child_env.insert("MAKEPAD".to_string(), "lines".to_string());

        if let Some(studio_addr) = studio_addr.as_deref().map(str::trim) {
            if !studio_addr.is_empty() {
                // Force studio routing for stdin-loop apps, regardless of user env overrides.
                child_env.insert("STUDIO".to_string(), studio_addr.to_string());
                child_env.insert("STUDIO_BUILD_ID".to_string(), build_id.0.to_string());
            }
        } else if child_env
            .get("STUDIO")
            .is_some_and(|studio| !studio.trim().is_empty())
        {
            // If caller supplied STUDIO manually, still provide the build-id path segment.
            child_env.insert("STUDIO_BUILD_ID".to_string(), build_id.0.to_string());
        }
        command.envs(child_env.iter());

        let mut child = command.spawn().map_err(|err| {
            format!(
                "failed to spawn cargo in {}: {}",
                cwd.to_string_lossy(),
                err
            )
        })?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let child = Arc::new(Mutex::new(child));

        if let Some(stdout) = stdout {
            spawn_reader(build_id, false, stdout, event_tx.clone());
        }
        if let Some(stderr) = stderr {
            spawn_reader(build_id, true, stderr, event_tx.clone());
        }
        spawn_waiter(build_id, Arc::clone(&child), event_tx);

        let info = BuildInfo {
            build_id,
            mount,
            package,
            active: true,
        };
        self.builds.insert(
            build_id,
            RunningBuild {
                info: info.clone(),
                child,
            },
        );
        Ok(info)
    }

    pub fn stop_build(&mut self, build_id: QueryId) -> Result<(), String> {
        let Some(build) = self.builds.get(&build_id) else {
            return Err(format!("unknown build: {}", build_id.0));
        };

        let mut child = build
            .child
            .lock()
            .map_err(|_| "build process lock poisoned".to_string())?;
        if let Err(err) = child.kill() {
            if err.kind() != std::io::ErrorKind::InvalidInput {
                return Err(format!("failed to stop build {}: {}", build_id.0, err));
            }
        }
        Ok(())
    }

    pub fn mark_exited(&mut self, build_id: QueryId, exit_code: Option<i32>) -> Option<BuildInfo> {
        let mut info = self.builds.remove(&build_id)?.info;
        info.active = false;
        let _ = exit_code;
        Some(info)
    }

    pub fn list_builds(&self) -> Vec<BuildInfo> {
        let mut builds: Vec<BuildInfo> = self.builds.values().map(|b| b.info.clone()).collect();
        builds.sort_by_key(|b| b.build_id.0);
        builds
    }
}

fn spawn_reader<R: Read + Send + 'static>(
    build_id: QueryId,
    is_stderr: bool,
    reader: R,
    event_tx: std::sync::mpsc::Sender<StudioEvent>,
) {
    thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    let line = line.trim_end_matches(&['\r', '\n'][..]).to_string();
                    let _ = event_tx.send(StudioEvent::ProcessOutput {
                        build_id,
                        is_stderr,
                        line,
                    });
                }
                Err(_) => break,
            }
        }
    });
}

fn spawn_waiter(
    build_id: QueryId,
    child: Arc<Mutex<Child>>,
    event_tx: std::sync::mpsc::Sender<StudioEvent>,
) {
    thread::spawn(move || loop {
        let exited = {
            let mut child = match child.lock() {
                Ok(child) => child,
                Err(_) => return,
            };
            match child.try_wait() {
                Ok(Some(status)) => Some(status.code()),
                Ok(None) => None,
                Err(_) => Some(None),
            }
        };

        if let Some(exit_code) = exited {
            let _ = event_tx.send(StudioEvent::ProcessExited {
                build_id,
                exit_code,
            });
            break;
        }

        thread::sleep(Duration::from_millis(30));
    });
}

fn parse_package_name(args: &[String]) -> Option<String> {
    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "-p" | "--package" if i + 1 < args.len() => return Some(args[i + 1].clone()),
            "--bin" if i + 1 < args.len() => return Some(args[i + 1].clone()),
            arg if arg.starts_with("--package=") => {
                return arg.split_once('=').map(|(_, value)| value.to_string());
            }
            arg if arg.starts_with("--bin=") => {
                return arg.split_once('=').map(|(_, value)| value.to_string());
            }
            _ => {}
        }
        i += 1;
    }
    None
}
