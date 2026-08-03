//! `arcade-eval` — measures one-shot AI game generation for Makepad Arcade.
//!
//! For each prompt: hand the REAL agent Arcade's real system prompt and tool
//! policy in a fresh dir, wait for `game.splash`, then evaluate that script
//! headlessly through `ScriptHost` — eval errors, 600 ticks of runtime errors,
//! entity counts, which verbs it reached for. No window: the sim is Cx-free by
//! design and a bare `Cx` is enough to run the script VM.
//!
//! The point is the failure list. It is cheaper to fix what the model is TOLD
//! than to fix each game it writes, so the report ranks failure modes and the
//! context gets edited against them — never against an individual prompt.
//!
//! Usage:
//!   cargo run -p makepad-arcade-eval --release -- --tag before
//!   cargo run -p makepad-arcade-eval --release -- --tag after --only racing
//!   cargo run -p makepad-arcade-eval --release -- --dry-run      (costs nothing)

mod cases;
mod report;

use cases::{Case, CASES};
use makepad_ai::agent::{Agent, AgentEvent};
use makepad_ai::types::StopReason;
use makepad_ai::backends::claude_code::ClaudeCodeAgent;
use makepad_game_script::ScriptHost;
use makepad_widgets::*;
use report::{Outcome, Run};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const TICKS: usize = 600;
const TICK_DT: f32 = 1.0 / 60.0;

struct Args {
    tag: String,
    out: PathBuf,
    only: Option<String>,
    limit: usize,
    dry_run: bool,
    fresh: bool,
    timeout: Duration,
    model: Option<String>,
    /// Evaluate one existing .splash and report — no agent, no suite. The
    /// quickest way to ask "does this game actually run".
    eval_file: Option<PathBuf>,
}

fn parse_args() -> Args {
    let mut a = Args {
        tag: "run".into(),
        out: PathBuf::from(
            std::env::var("ARCADE_EVAL_OUT").unwrap_or_else(|_| "/tmp/arcade_eval".into()),
        ),
        only: None,
        limit: usize::MAX,
        dry_run: std::env::var("ARCADE_EVAL_NO_AGENT").is_ok(),
        fresh: false,
        timeout: Duration::from_secs(300),
        model: None,
        eval_file: None,
    };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        let next = |i: usize| argv.get(i + 1).cloned().unwrap_or_default();
        match argv[i].as_str() {
            "--tag" => {
                a.tag = next(i);
                i += 1;
            }
            "--out" => {
                a.out = PathBuf::from(next(i));
                i += 1;
            }
            "--only" => {
                a.only = Some(next(i));
                i += 1;
            }
            "--limit" => {
                a.limit = next(i).parse().unwrap_or(usize::MAX);
                i += 1;
            }
            "--timeout" => {
                a.timeout = Duration::from_secs(next(i).parse().unwrap_or(300));
                i += 1;
            }
            "--model" => {
                a.model = Some(next(i));
                i += 1;
            }
            "--eval" => {
                a.eval_file = Some(PathBuf::from(next(i)));
                i += 1;
            }
            // Re-runs are cached by default: an agent call costs real money, so
            // an existing game.splash is reused unless asked otherwise.
            "--fresh" => a.fresh = true,
            "--dry-run" => a.dry_run = true,
            other => eprintln!("arcade-eval: ignoring unknown arg {other:?}"),
        }
        i += 1;
    }
    a
}

fn main() {
    let args = parse_args();

    if let Some(path) = &args.eval_file {
        let source = std::fs::read_to_string(path).expect("read splash file");
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let case = &CASES[0];
        let mut run = Run::new("file", "ad-hoc");
        run.lines = source.lines().count();
        run.verbs = verbs_used(&source);
        let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            evaluate(&mut cx, &source, case, &mut run);
        }));
        if ok.is_err() {
            run.outcome = Outcome::Panicked;
        }
        println!(
            "{}: {}\n  entities {} -> {}  verbs: {}",
            path.display(),
            run.one_line(),
            run.entities,
            run.entities_after,
            run.verbs.join(", ")
        );
        return;
    }

    let root = args.out.join(&args.tag);
    std::fs::create_dir_all(&root).expect("create out dir");

    let assets_ready = asset_tool_available();
    println!(
        "arcade-eval: tag={} out={} agent={} assets={}",
        args.tag,
        root.display(),
        if args.dry_run { "DRY-RUN" } else { "claude-code" },
        if assets_ready { "ready" } else { "absent (asset prompts skipped)" }
    );

    let mut cx = Cx::new(Box::new(|_, _| {}));
    let mut runs = Vec::new();

    for case in CASES.iter().take(args.limit) {
        if let Some(only) = &args.only {
            if case.slug != only {
                continue;
            }
        }
        if case.needs_assets && !assets_ready {
            println!("  {:<18} SKIP (needs the asset index)", case.slug);
            continue;
        }
        let run = run_case(&mut cx, case, &root, &args);
        println!("  {:<18} {}", case.slug, run.one_line());
        runs.push(run);
    }

    let md = report::render(&args.tag, &runs);
    let path = root.join("report.md");
    std::fs::write(&path, &md).expect("write report");
    println!("\n{md}\narcade-eval: wrote {}", path.display());
}

/// The asset fork's `find_model` tool — optional, so its prompts can be enabled
/// the moment that crate lands without touching the harness.
fn asset_tool_available() -> bool {
    Path::new("libs/game/assets/src/lib.rs").exists()
}

fn run_case(cx: &mut Cx, case: &Case, root: &Path, args: &Args) -> Run {
    let dir = root.join(case.slug);
    let _ = std::fs::create_dir_all(&dir);
    let game = dir.join("game.splash");
    if args.fresh {
        let _ = std::fs::remove_file(&game);
    }

    let mut run = Run::new(case.slug, case.prompt);
    if !game.exists() {
        if args.dry_run {
            run.outcome = Outcome::Skipped("dry run, no game generated".into());
            return run;
        }
        match generate(cx, case, &dir, args) {
            Ok((secs, turns, tools)) => {
                run.gen_secs = secs;
                run.turns = turns;
                run.tool_calls = tools;
            }
            Err(e) => {
                run.outcome = Outcome::NoFile(e);
                return run;
            }
        }
    } else {
        run.cached = true;
    }

    let Ok(source) = std::fs::read_to_string(&game) else {
        run.outcome = Outcome::NoFile("game.splash never appeared".into());
        return run;
    };
    run.lines = source.lines().count();
    run.verbs = verbs_used(&source);
    // A generated game that panics the engine must not abort the suite — that
    // IS a result, and the most serious kind. Cx is not UnwindSafe, but the
    // process is single-threaded here and a poisoned Cx is not reused: each
    // case gets a fresh ScriptHost, and a panicking one is reported and left.
    let evaluated = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        evaluate(cx, &source, case, &mut run);
    }));
    if evaluated.is_err() {
        run.outcome = Outcome::Panicked;
    }
    run
}

/// Drive the real agent to completion in `dir`, using Arcade's own session
/// config so the harness measures the app, not a lookalike.
fn generate(
    cx: &mut Cx,
    case: &Case,
    dir: &Path,
    args: &Args,
) -> Result<(f64, usize, usize), String> {
    let mut agent = ClaudeCodeAgent::new();
    let config = makepad_arcade::ai::authoring_session(
        Some(dir.to_string_lossy().to_string()),
        &makepad_game_script::api_text(),
        args.model.clone(),
    );
    let session = agent.create_session(cx, config);

    let start = Instant::now();
    // The CLI backend only surfaces work on Event::Signal, so a plain poll
    // loop is the whole event pump — no window, no frame clock.
    let deadline = start + args.timeout;
    while !agent.is_session_ready(session) {
        agent.handle_event(cx, &Event::Signal);
        if Instant::now() > deadline {
            return Err("session never became ready".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let prompt = format!(
        "{} — write it into game.splash in this directory.",
        case.prompt
    );
    agent.send_prompt(cx, session, &prompt);

    let (mut turns, mut tools) = (0usize, 0usize);
    loop {
        for ev in agent.handle_event(cx, &Event::Signal) {
            match ev {
                AgentEvent::ToolRequest { .. } => tools += 1,
                AgentEvent::TurnComplete { stop_reason, .. } => {
                    turns += 1;
                    if !matches!(stop_reason, StopReason::MaxTokens) {
                        return Ok((start.elapsed().as_secs_f64(), turns, tools));
                    }
                }
                AgentEvent::PromptError { error, .. } => return Err(error),
                AgentEvent::SessionError { error, .. } => return Err(error),
                _ => {}
            }
        }
        if Instant::now() > deadline {
            return Err(format!("timed out after {}s", args.timeout.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Eval the script and run it, exactly as the app would — then judge it.
fn evaluate(cx: &mut Cx, source: &str, case: &Case, run: &mut Run) {
    let mut host = ScriptHost::new();
    let report = match host.set_source(cx, source) {
        Some(r) => r,
        None => {
            run.outcome = Outcome::EvalError("empty source".into());
            return;
        }
    };
    if !report.ok {
        run.outcome = Outcome::EvalError(report.error.unwrap_or_default());
        return;
    }
    run.entities = report.entities;

    for _ in 0..TICKS {
        host.tick(cx, TICK_DT);
    }
    // Drain so a full queue is not mistaken for a stall in later runs.
    let _ = host.take_audio();
    let _ = host.take_particles();
    run.runtime_error = host.last_error().map(|s| s.to_string());
    run.entities_after = host.world.borrow().entities.len();

    let missing: Vec<String> = case
        .expect
        .needs
        .iter()
        .filter(|v| !run.verbs.iter().any(|u| u == *v))
        .map(|v| v.to_string())
        .collect();
    let unmet: Vec<String> = case
        .expect
        .any_of
        .iter()
        .filter(|group| !group.iter().any(|v| run.verbs.iter().any(|u| u == v)))
        .map(|group| format!("one of {group:?}"))
        .collect();

    let mut why = Vec::new();
    if !missing.is_empty() {
        why.push(format!("missing verbs: {}", missing.join(", ")));
    }
    if !unmet.is_empty() {
        why.push(unmet.join("; "));
    }
    if run.entities < case.expect.min_entities {
        why.push(format!(
            "only {} entities, wanted {}",
            run.entities, case.expect.min_entities
        ));
    }
    if let Some(err) = &run.runtime_error {
        why.push(format!("runtime error: {}", first_line(err)));
    }

    run.outcome = if why.is_empty() {
        Outcome::Pass
    } else {
        Outcome::Failed(why.join(" | "))
    };
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or_default().to_string()
}

/// Every `game.<verb>` the script actually calls, deduped and sorted.
/// Comments are stripped first so a mention in prose is not counted as use.
fn verbs_used(source: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in source.lines() {
        let code = match line.find("//") {
            Some(i) => &line[..i],
            None => line,
        };
        let bytes = code.as_bytes();
        let mut i = 0;
        while let Some(hit) = code[i..].find("game.") {
            let start = i + hit + 5;
            let mut end = start;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                end += 1;
            }
            if end > start {
                let verb = code[start..end].to_string();
                if !out.contains(&verb) {
                    out.push(verb);
                }
            }
            i = end.max(start + 1);
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbs_are_extracted_and_comments_ignored() {
        let src = "game.car({})\n// game.plane is not used here\nlet x = game.pos(a) + game.pos(b)\n";
        assert_eq!(verbs_used(src), vec!["car", "pos"]);
    }

    #[test]
    fn every_case_has_a_reachable_expectation() {
        // A case whose `needs` names a verb the engine does not have could
        // never pass, and would look like a model failure forever.
        let api = makepad_game_script::api_text();
        for case in CASES {
            for verb in case.expect.needs {
                assert!(
                    api.contains(&format!("game.{verb}")),
                    "case {} expects game.{verb}, which the engine does not have",
                    case.slug
                );
            }
        }
    }

    #[test]
    fn slugs_are_unique() {
        let mut seen = Vec::new();
        for c in CASES {
            assert!(!seen.contains(&c.slug), "duplicate slug {}", c.slug);
            seen.push(c.slug);
        }
    }
}
