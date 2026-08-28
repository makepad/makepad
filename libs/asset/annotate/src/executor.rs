//! The executor seam: which vision model runs, and how a batch reaches it.
//!
//! The model is never linked in. An executor is a subprocess speaking the
//! batch protocol at the top of `libs/ai/llm/src/bin/vlm_annotate.rs`:
//!
//! ```text
//! <exe> [args…] --jobs J --prompt-file P --out O
//!   J: TSV  <id>\t<image.ppm>[\t<per-job context>]
//!   O: TSV  <id>\t<ok|err>\t<escaped reply>
//! ```
//!
//! Two implement it today — `tools/remote_executor.sh` (Qwen3.8-27B on the
//! fleet box, through the makepad tunnel) and `vlm-annotate` (Qwen3.5-9B on
//! this machine's Metal) — and [`choose`] picks between them so that both
//! hosts of the pass (the CLI and the queue worker) make the same choice
//! for the same reasons.
//!
//! Batch size matters more than it looks: every invocation pays a one-off
//! model load (~15 s remote), so a batch of one costs five times a batch
//! of sixteen per sheet.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

/// The executor a run will talk to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutorChoice {
    /// argv the batch flags are appended to.
    pub argv: Vec<String>,
    /// Label-safe model identity, published as the `vlm-m-<tag>` tag.
    pub model_tag: String,
    /// Where this choice came from — one log line per run, so a slow pass
    /// is never a mystery about which model answered.
    pub source: String,
}

impl ExecutorChoice {
    /// "remote qwen38-27b" — the tail of a status line.
    pub fn label(&self) -> String {
        format!("{} {}", self.source, self.model_tag)
    }
}

/// Everything [`choose`] reads, so a test can hand it a world.
pub struct ExecutorEnv {
    pub env_executor: Option<String>,
    pub env_model_tag: Option<String>,
    /// Checkout root.
    pub repo: PathBuf,
    /// Directory holding the calling binary — where a release build's
    /// sibling `vlm-annotate` lives.
    pub exe_dir: PathBuf,
}

impl ExecutorEnv {
    /// The usual world: `MAKEPAD_VLM_EXECUTOR`/`MAKEPAD_VLM_MODEL_TAG` from
    /// the environment, the repo where the caller says it is.
    pub fn from_env(repo: PathBuf, exe_dir: PathBuf) -> Self {
        Self {
            env_executor: std::env::var("MAKEPAD_VLM_EXECUTOR").ok(),
            env_model_tag: std::env::var("MAKEPAD_VLM_MODEL_TAG").ok(),
            repo,
            exe_dir,
        }
    }
}

pub fn remote_script(repo: &Path) -> PathBuf {
    repo.join("libs/asset/annotate/tools/remote_executor.sh")
}

pub const LOCAL_MODEL: &str = "local/models/Qwen3.5-9B-UD-Q4_K_XL.gguf";
pub const LOCAL_MMPROJ: &str = "local/models/Qwen3.5-9B-mmproj-F16.gguf";

/// A helper binary beside the caller's, else the checkout's release dir.
pub fn sibling_or_target(
    env: &ExecutorEnv,
    name: &str,
    present: &dyn Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let sibling = env.exe_dir.join(name);
    if present(&sibling) {
        return Some(sibling);
    }
    let target = env.repo.join("target/release").join(name);
    present(&target).then_some(target)
}

fn local_argv(env: &ExecutorEnv, present: &dyn Fn(&Path) -> bool) -> Option<Vec<String>> {
    let exe = sibling_or_target(env, "vlm-annotate", present)?;
    let model = env.repo.join(LOCAL_MODEL);
    let mmproj = env.repo.join(LOCAL_MMPROJ);
    if !present(&model) || !present(&mmproj) {
        return None;
    }
    Some(vec![
        exe.to_string_lossy().into_owned(),
        model.to_string_lossy().into_owned(),
        mmproj.to_string_lossy().into_owned(),
    ])
}

/// Pick the vision executor: the operator's own override, then the fleet
/// box, then this machine.
///
/// `present` answers "is there a file here" and `probe` runs the remote
/// script's `--probe`; both are injected so the ladder is testable without
/// a filesystem or a box. A box that is down falls back to local rather
/// than failing — the pass is idempotent and a local run is a real run.
pub fn choose(
    env: &ExecutorEnv,
    present: &dyn Fn(&Path) -> bool,
    probe: &dyn Fn(&Path) -> bool,
) -> Result<ExecutorChoice, String> {
    if let Some(raw) = env.env_executor.as_deref() {
        let argv = shell_split(raw);
        if !argv.is_empty() {
            return Ok(ExecutorChoice {
                argv,
                model_tag: env
                    .env_model_tag
                    .clone()
                    .filter(|t| !t.trim().is_empty())
                    .unwrap_or_else(|| "custom".to_string()),
                source: "env".to_string(),
            });
        }
    }
    let script = remote_script(&env.repo);
    if present(&script) && probe(&script) {
        return Ok(ExecutorChoice {
            argv: vec![script.to_string_lossy().into_owned()],
            model_tag: "qwen38-27b".to_string(),
            source: "remote".to_string(),
        });
    }
    if let Some(argv) = local_argv(env, present) {
        return Ok(ExecutorChoice {
            argv,
            model_tag: "qwen35-9b".to_string(),
            source: "local".to_string(),
        });
    }
    Err(format!(
        "no vision executor: set MAKEPAD_VLM_EXECUTOR, provision {} on the fleet box, \
         or build vlm-annotate and put the Qwen3.5-9B gguf + mmproj in {}",
        script.display(),
        env.repo.join("local/models").display()
    ))
}

/// The usual choice against the real filesystem and a real probe.
pub fn choose_real(env: &ExecutorEnv) -> Result<ExecutorChoice, String> {
    choose(env, &|p: &Path| p.is_file(), &probe_remote)
}

/// Run `remote_executor.sh --probe`: exit 0 means the box, the executable
/// and the weights are all there (or a batch is already running on it).
/// Anything else — missing script, no tunnel, no weights — is "no remote".
pub fn probe_remote(script: &Path) -> bool {
    Command::new(script)
        .arg("--probe")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// POSIX-ish argv split: whitespace separates, single and double quotes
/// group. Enough for `MAKEPAD_VLM_EXECUTOR="ssh box vlm-annotate /m/a /m/b"`,
/// and it never runs a shell.
pub fn shell_split(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut has = false;
    for c in raw.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                has = true;
            }
            None if c.is_whitespace() => {
                if has || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            None => cur.push(c),
        }
    }
    if has || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// One meaningful line of executor output.
#[derive(Clone, Debug, PartialEq)]
pub enum ExecutorLine {
    /// `progress 10/16 (10 ok, 0 err) last 3.12s avg 3.05s eta 0.7min`
    Progress { done: usize, total: usize },
    Other,
}

/// Read one line of executor chatter. The batch protocol does not specify
/// progress, so this is best effort: an executor that prints nothing simply
/// reports no progress, and the pass still measures its own rate.
pub fn parse_executor_line(line: &str) -> ExecutorLine {
    let t = line.trim();
    let Some(rest) = t.strip_prefix("progress ") else {
        return ExecutorLine::Other;
    };
    let Some(frac) = rest.split_whitespace().next() else {
        return ExecutorLine::Other;
    };
    let Some((a, b)) = frac.split_once('/') else {
        return ExecutorLine::Other;
    };
    match (a.parse::<usize>(), b.parse::<usize>()) {
        (Ok(done), Ok(total)) => ExecutorLine::Progress { done, total },
        _ => ExecutorLine::Other,
    }
}

/// Unescape the reply column of the batch protocol's output TSV.
pub fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// What one batch produced: replies by job id, plus every error line the
/// executor reported for a job it could not answer.
#[derive(Clone, Debug, Default)]
pub struct BatchReplies {
    pub ok: std::collections::BTreeMap<String, String>,
    pub err: std::collections::BTreeMap<String, String>,
}

/// Run one executor invocation over a prepared jobs file.
///
/// `on_line` sees every stdout/stderr line (already parsed for progress),
/// which is how a worker heartbeats a live lease and how an operator sees
/// s/sheet. `stop` kills the child: a shutting-down host must not leave a
/// vision model resident on a GPU.
pub fn run_batch(
    argv: &[String],
    jobs_path: &Path,
    prompt_path: &Path,
    out_path: &Path,
    stop: &AtomicBool,
    on_line: &mut dyn FnMut(&str, ExecutorLine),
) -> Result<BatchReplies, String> {
    if argv.is_empty() {
        return Err("executor argv is empty".to_string());
    }
    let _ = std::fs::remove_file(out_path);
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .arg("--jobs")
        .arg(jobs_path)
        .arg("--prompt-file")
        .arg(prompt_path)
        .arg("--out")
        .arg(out_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group: killing it kills whatever it spawned (an
        // ssh tunnel, a remote runner), not just the wrapper.
        cmd.process_group(0);
    }
    let mut child = cmd.spawn().map_err(|e| format!("spawn {}: {e}", argv[0]))?;
    let pid = child.id();

    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let mut readers = Vec::new();
    for pipe in [
        child.stdout.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
        child.stderr.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
    ]
    .into_iter()
    .flatten()
    {
        let tx = tx.clone();
        readers.push(thread::spawn(move || {
            for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        }));
    }
    drop(tx);
    let mut killed = false;
    loop {
        match rx.recv_timeout(std::time::Duration::from_millis(500)) {
            Ok(line) => {
                let parsed = parse_executor_line(&line);
                on_line(&line, parsed);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // A live batch with a silent executor still needs its host
                // to tick (leases, cancellation).
                on_line("", ExecutorLine::Other);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if stop.load(Ordering::Relaxed) && !killed {
            kill_group(pid);
            killed = true;
        }
    }
    for r in readers {
        let _ = r.join();
    }
    let status = child.wait().map_err(|e| format!("wait: {e}"))?;
    let text = std::fs::read_to_string(out_path).unwrap_or_default();
    let mut replies = BatchReplies::default();
    for line in text.lines() {
        let mut it = line.splitn(3, '\t');
        let (Some(id), Some(state), Some(payload)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        if state == "ok" {
            replies.ok.insert(id.to_string(), unescape(payload));
        } else {
            replies.err.insert(id.to_string(), unescape(payload));
        }
    }
    // A non-zero exit that still produced replies is a partial batch, not a
    // failure: every answered asset is publishable and the rest are simply
    // still owed. Nothing answered AND a bad exit is a real failure.
    if !status.success() && replies.ok.is_empty() {
        return Err(format!("executor exited with {status} without answering anything"));
    }
    Ok(replies)
}

/// Kill a process group. There is no libc here, and the negative-pid form
/// is a shell builtin's job.
pub fn kill_group(pid: u32) {
    if pid == 0 {
        return;
    }
    #[cfg(unix)]
    {
        let _ = Command::new("/bin/sh")
            .arg("-c")
            .arg(format!(
                "kill -TERM -{pid} 2>/dev/null; sleep 1; kill -KILL -{pid} 2>/dev/null"
            ))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
    #[cfg(not(unix))]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(exec: Option<&str>, tag: Option<&str>) -> ExecutorEnv {
        ExecutorEnv {
            env_executor: exec.map(str::to_string),
            env_model_tag: tag.map(str::to_string),
            repo: PathBuf::from("/repo"),
            exe_dir: PathBuf::from("/repo/target/release"),
        }
    }

    const LOCAL: &[&str] = &[
        "/repo/target/release/vlm-annotate",
        "/repo/local/models/Qwen3.5-9B-UD-Q4_K_XL.gguf",
        "/repo/local/models/Qwen3.5-9B-mmproj-F16.gguf",
    ];
    const SCRIPT: &str = "/repo/libs/asset/annotate/tools/remote_executor.sh";

    fn world<'a>(files: &'a [&'a str]) -> impl Fn(&Path) -> bool + 'a {
        move |p: &Path| files.iter().any(|f| Path::new(f) == p)
    }

    #[test]
    fn the_operators_override_wins_over_everything() {
        let e = env(Some("ssh box vlm-annotate /m/a.gguf /m/b.gguf"), Some("mine"));
        let c = choose(&e, &world(LOCAL), &|_| true).unwrap();
        assert_eq!(c.argv, ["ssh", "box", "vlm-annotate", "/m/a.gguf", "/m/b.gguf"]);
        assert_eq!(c.model_tag, "mine");
        assert_eq!(c.source, "env");
        // No tag given: identified as custom, never mislabelled as a model
        // whose output shape we know.
        let e = env(Some("/bin/true"), None);
        assert_eq!(choose(&e, &world(&[]), &|_| false).unwrap().model_tag, "custom");
    }

    #[test]
    fn remote_beats_local_but_only_when_it_probes_clean() {
        let mut files = LOCAL.to_vec();
        files.push(SCRIPT);
        let c = choose(&env(None, None), &world(&files), &|_| true).unwrap();
        assert_eq!(c.argv, [SCRIPT]);
        assert_eq!(c.model_tag, "qwen38-27b");
        assert_eq!(c.label(), "remote qwen38-27b");
        let c = choose(&env(None, None), &world(&files), &|_| false).unwrap();
        assert_eq!(c.source, "local");
        assert_eq!(c.argv, LOCAL);
    }

    #[test]
    fn the_script_is_only_probed_when_it_exists() {
        let probed = std::cell::Cell::new(false);
        let c = choose(&env(None, None), &world(LOCAL), &|_| {
            probed.set(true);
            true
        })
        .unwrap();
        assert!(!probed.get());
        assert_eq!(c.source, "local");
    }

    #[test]
    fn a_missing_model_is_not_a_local_executor() {
        let e = choose(&env(None, None), &world(&LOCAL[..1]), &|_| false).unwrap_err();
        assert!(e.contains("MAKEPAD_VLM_EXECUTOR"), "{e}");
        assert!(e.contains("local/models"), "{e}");
    }

    #[test]
    fn shell_split_groups_quotes() {
        assert_eq!(shell_split("  a  b\tc "), ["a", "b", "c"]);
        assert_eq!(shell_split("ssh box \"a b\" 'c d'"), ["ssh", "box", "a b", "c d"]);
        assert!(shell_split("   ").is_empty());
        assert_eq!(shell_split("a ''"), ["a", ""]);
    }

    #[test]
    fn executor_progress_is_read_and_nothing_else_is() {
        assert_eq!(
            parse_executor_line("progress 10/16 (10 ok, 0 err) last 3.12s avg 3.05s eta 0.7min"),
            ExecutorLine::Progress { done: 10, total: 16 }
        );
        assert_eq!(parse_executor_line("loaded model + tower in 8.20s"), ExecutorLine::Other);
        assert_eq!(parse_executor_line("progress x/y"), ExecutorLine::Other);
    }

    #[test]
    fn the_reply_column_round_trips() {
        assert_eq!(unescape("a\\nb\\tc\\\\d"), "a\nb\tc\\d");
        // An unknown escape survives rather than eating the next character.
        assert_eq!(unescape("a\\qb"), "a\\qb");
    }
}
