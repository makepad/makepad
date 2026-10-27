//! File-backed child output avoids pipe deadlocks and per-command reader threads.
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};

pub type Result<T> = std::result::Result<T, String>;

/// `yield_to` in ci.toml: a directory whose `*.cmd` files mean another tenant of this machine
/// needs it quiet (on the mini: the bench worker's spool, which holds a `.cmd` while a timing job
/// is queued to it or running). While one is there no command starts, and a running one's
/// process group is stopped (SIGSTOP) until the directory has none again.
static YIELD_TO: OnceLock<PathBuf> = OnceLock::new();
pub fn set_yield_to(dir: PathBuf) {
    let _ = YIELD_TO.set(dir);
}
fn must_yield() -> bool {
    YIELD_TO.get().is_some_and(|dir| {
        std::fs::read_dir(dir).is_ok_and(|mut entries| {
            entries.any(|e| e.is_ok_and(|e| e.path().extension().is_some_and(|x| x == "cmd")))
        })
    })
}
/// Waits while a `yield_to` job is there; returns how long it waited.
fn wait_for_quiet(control: &Control, log: &mut dyn FnMut(&str)) -> Result<Duration> {
    let start = Instant::now();
    if must_yield() {
        log("ci: yielding the machine to a bench job (yield_to in ci.toml)");
        while must_yield() {
            control.sleep(2.0)?;
        }
        log(&format!("ci: resumed after {:.0} s", start.elapsed().as_secs_f64()));
    }
    Ok(start.elapsed())
}
#[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
const SIGSTOP_CONT: (i32, i32) = (19, 18);
#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
const SIGSTOP_CONT: (i32, i32) = (17, 19);
#[cfg(unix)]
fn signal_group(pid: u32, sig: i32) {
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    unsafe {
        kill(-(pid as i32), sig);
    }
}

#[derive(Clone, Default)]
pub struct Control {
    pub stop: Arc<AtomicBool>,
    pub shutdown: Arc<AtomicBool>,
    pub model_cancel: makepad_ai_hub::backend::CancelToken,
}
impl Control {
    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::Acquire) || self.shutdown.load(Ordering::Acquire)
    }
    pub fn check(&self) -> Result<()> {
        if self.stopped() {
            Err("stopped by user".into())
        } else {
            Ok(())
        }
    }
    pub fn sleep(&self, secs: f64) -> Result<()> {
        if !secs.is_finite() || !(0.0..=86400.0).contains(&secs) {
            return Err("invalid sleep duration".into());
        }
        let end = Instant::now() + Duration::from_secs_f64(secs);
        while Instant::now() < end {
            self.check()?;
            thread::sleep(
                end.saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(100)),
            );
        }
        self.check()
    }
}

pub struct ChildLog {
    pub child: Child,
    reader: File,
    pub text: String,
    offset: u64,
    partial: Vec<u8>,
}
impl ChildLog {
    pub fn spawn(command: &mut Command, path: PathBuf) -> Result<Self> {
        let file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        command
            .stdin(Stdio::null())
            .stdout(Stdio::from(file.try_clone().map_err(|e| e.to_string())?))
            .stderr(Stdio::from(file));
        let child = command.spawn().map_err(|e| format!("{command:?}: {e}"))?;
        let reader = File::open(&path).map_err(|e| e.to_string())?;
        Ok(Self {
            child,
            reader,
            text: String::new(),
            offset: 0,
            partial: Vec::new(),
        })
    }
    pub fn drain(&mut self, log: &mut dyn FnMut(&str)) -> Result<()> {
        self.reader
            .seek(SeekFrom::Start(self.offset))
            .map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        self.reader
            .by_ref()
            .take(1024 * 1024)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        self.offset += bytes.len() as u64;
        self.partial.extend(bytes);
        while let Some(pos) = self.partial.iter().position(|b| *b == b'\n') {
            let line = String::from_utf8_lossy(&self.partial[..pos])
                .trim_end_matches('\r')
                .to_string();
            self.text.push_str(&line);
            self.text.push('\n');
            log(&line);
            self.partial.drain(..=pos);
        }
        Ok(())
    }
    pub fn finish_output(&mut self, log: &mut dyn FnMut(&str)) -> Result<()> {
        while self.offset < self.reader.metadata().map_err(|e| e.to_string())?.len() {
            self.drain(log)?;
        }
        if !self.partial.is_empty() {
            let line = String::from_utf8_lossy(&self.partial).into_owned();
            self.text.push_str(&line);
            log(&line);
            self.partial.clear();
        }
        Ok(())
    }
    pub fn alive(&mut self) -> Result<bool> {
        self.child
            .try_wait()
            .map(|s| s.is_none())
            .map_err(|e| e.to_string())
    }
    pub fn kill_exact(&mut self) -> Result<()> {
        if self.alive()? {
            self.child.kill().map_err(|e| e.to_string())?;
        }
        self.child.wait().map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct Output {
    pub code: i32,
    pub out: String,
}

pub fn run(
    program: &str,
    args: &[String],
    cwd: &Path,
    env: &[(String, String)],
    timeout_secs: u64,
    path: PathBuf,
    control: &Control,
    log: &mut dyn FnMut(&str),
) -> Result<Output> {
    control.check()?;
    wait_for_quiet(control, log)?;
    let mut command = if program == "cargo" {
        let mut c = Command::new("nice");
        c.arg("cargo");
        c
    } else {
        Command::new(program)
    };
    command
        .args(args)
        .current_dir(cwd)
        .envs(env.iter().cloned());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = ChildLog::spawn(&mut command, path)?;
    let start = Instant::now();
    let mut paused = Duration::ZERO;
    let mut polls = 0u32;
    let result = loop {
        if let Err(e) = child.drain(log) {
            break Err(e);
        }
        match child.child.try_wait() {
            Ok(Some(status)) => {
                break child.finish_output(log).map(|_| Output {
                    code: status.code().unwrap_or(-1),
                    out: child.text.clone(),
                });
            }
            Err(e) => break Err(e.to_string()),
            Ok(None) => {}
        }
        polls += 1;
        #[cfg(unix)]
        if polls % 10 == 0 && must_yield() {
            signal_group(child.child.id(), SIGSTOP_CONT.0);
            let waited = wait_for_quiet(control, log);
            signal_group(child.child.id(), SIGSTOP_CONT.1);
            match waited {
                Ok(d) => paused += d,
                Err(e) => break Err(e),
            }
        }
        if control.stopped() || start.elapsed().saturating_sub(paused) >= Duration::from_secs(timeout_secs.max(1)) {
            break Err(if control.stopped() {
                "stopped by user".into()
            } else {
                format!("{program} timed out after {timeout_secs}s")
            });
        }
        thread::sleep(Duration::from_millis(100));
    };
    if result.is_err() {
        #[cfg(unix)]
        signal_group(child.child.id(), 15);
        let end = Instant::now() + Duration::from_secs(2);
        while child.alive().unwrap_or(false) && Instant::now() < end {
            thread::sleep(Duration::from_millis(50));
        }
        #[cfg(unix)]
        signal_group(child.child.id(), 9);
        let _ = child.kill_exact();
        let _ = child.finish_output(log);
    }
    result
}

pub fn error_lines(out: &str) -> String {
    let errors: Vec<_> = out
        .lines()
        .filter(|s| s.to_ascii_lowercase().contains("error"))
        .take(40)
        .collect();
    if !errors.is_empty() {
        return errors.join("\n");
    }
    let lines: Vec<_> = out.lines().collect();
    lines[lines.len().saturating_sub(40)..].join("\n")
}
