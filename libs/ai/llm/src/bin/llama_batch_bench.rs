//! `llama-batch-bench` — what one decode step costs as a function of how many
//! token columns it carries.
//!
//! This is the measurement the continuous-batching decision rests on. A decode
//! step reads every weight of the model to produce one token, so in theory the
//! second, third and fourth column of a batch ride along for free and N chats
//! cost what one chat costs. In practice they do not, and the marginal cost per
//! extra column is the number that sets how many chats a card can serve. This
//! bin measures that marginal directly, at both of the shapes that matter:
//!
//! **`n_outputs = 1` (the `--cols` curve).** `n_tokens` columns through the
//! body, one row through the LM head. This is a prefill/verify-body shape and
//! it isolates the body's per-column cost from the head's.
//!
//! **`n_outputs = n_tokens` (the `--verify-widths` curve).** Every column also
//! produces its own logits row, which is what a real multi-slot decode step
//! needs — each slot samples its own next token. Over a 248k-row head that is
//! not a rounding error, so a cost model calibrated on the first curve alone
//! under-counts. There is no public API for a bare `n_outputs = B` decode, but
//! MTP speculative verification is exactly that shape: one verify batch per
//! round, always `spec_draft_max + 1` columns wide, timed by the session's own
//! counters. So this mode loads one session per width and reads them back.
//!
//! Everything here is read-only: it loads a gguf, prefills, times decode steps
//! and prints. No service, no files written unless `--csv` is given.
//!
//! ```text
//! llama-batch-bench <model.gguf>
//!     [--fill N]              tokens of context every measurement runs at (default 4096)
//!     [--cols 1,2,4,8,...]    column counts for the n_outputs=1 curve
//!     [--reps N]              timed steps per column count (default 12)
//!     [--warmup N]            untimed steps first (default 3; >=2 so the CUDA graph is captured)
//!     [--max-context N]       session context (default fill + 4096, min 8192)
//!     [--verify-widths 2,3,5] MTP verify widths for the n_outputs=B curve (default off)
//!     [--verify-tokens N]     tokens to generate per verify width (default 64)
//!     [--csv PATH]            also write the rows as csv
//!     [--label TEXT]          tag printed with every row (which build this was)
//!     [--host-split]          report host/GPU timing components
//! ```

use std::time::{Duration, Instant};

use makepad_ai_llm::{LlamaSession, LlamaSessionConfig, LlamaStopReason};

const DEFAULT_COLS: &[usize] = &[1, 2, 3, 4, 6, 8, 10, 12, 16, 24, 32, 64, 128, 256];

struct Args {
    model: String,
    fill: usize,
    cols: Vec<usize>,
    reps: usize,
    warmup: usize,
    max_context: usize,
    verify_widths: Vec<usize>,
    verify_tokens: usize,
    csv: Option<String>,
    label: String,
    host_split: bool,
}

fn parse_list(text: &str) -> Vec<usize> {
    text.split(',')
        .filter_map(|part| part.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .collect()
}

fn parse_args() -> Args {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let Some(model) = argv.first().cloned() else {
        eprintln!(
            "usage: llama-batch-bench <model.gguf> [--fill N] [--cols 1,2,4,...] [--reps N]\n\
             \x20      [--warmup N] [--max-context N] [--verify-widths 2,3,5]\n\
             \x20      [--verify-tokens N] [--csv PATH] [--label TEXT] [--host-split]"
        );
        std::process::exit(2);
    };
    let mut args = Args {
        model,
        fill: 4096,
        cols: DEFAULT_COLS.to_vec(),
        reps: 12,
        warmup: 3,
        max_context: 0,
        verify_widths: Vec::new(),
        verify_tokens: 64,
        csv: None,
        label: String::new(),
        host_split: false,
    };
    let mut i = 1;
    while i < argv.len() {
        let take = |i: &mut usize| -> String {
            *i += 1;
            argv.get(*i).cloned().unwrap_or_default()
        };
        match argv[i].as_str() {
            "--fill" => args.fill = take(&mut i).parse().unwrap_or(args.fill),
            "--cols" => args.cols = parse_list(&take(&mut i)),
            "--reps" => args.reps = take(&mut i).parse().unwrap_or(args.reps),
            "--warmup" => args.warmup = take(&mut i).parse().unwrap_or(args.warmup),
            "--max-context" => args.max_context = take(&mut i).parse().unwrap_or(0),
            "--verify-widths" => args.verify_widths = parse_list(&take(&mut i)),
            "--verify-tokens" => args.verify_tokens = take(&mut i).parse().unwrap_or(64),
            "--csv" => args.csv = Some(take(&mut i)),
            "--label" => args.label = take(&mut i),
            "--host-split" => args.host_split = true,
            other => {
                eprintln!("llama-batch-bench: unknown argument {other:?}");
                std::process::exit(2);
            }
        }
        i += 1;
    }
    if args.cols.is_empty() {
        args.cols = DEFAULT_COLS.to_vec();
    }
    // Headroom for the timed steps on top of the fill, plus a floor so short
    // fills still exercise a realistic cache.
    let widest = args.cols.iter().copied().max().unwrap_or(1);
    let needed = args.fill + widest * (args.reps + args.warmup + 2) + 256;
    if args.max_context < needed {
        args.max_context = needed.max(8192);
    }
    args.warmup = args.warmup.max(2);
    args
}

/// A paragraph of ordinary prose, cycled to reach the requested fill.
///
/// A decode step's cost does not depend on *which* ids sit in the cache, so
/// synthetic ids would do for the `--cols` curve. They will not do for
/// `--verify-widths`: that mode has to actually generate, and a model fed
/// random ids answers with an immediate end-of-sequence, so no verify round
/// ever runs. Real text keeps the model writing.
const FILLER_TEXT: &str = "\
The harbour was busy that morning. Cranes swung over the quay, gulls argued \
above the fish market, and the ferry from the north island came in twenty \
minutes late as it always did. Anna counted the crates as they landed, marking \
each one in the ledger she carried under her arm, and thought about the letter \
she had not yet answered. The tide would turn at four. By then the last of the \
cargo would be stacked in the long shed, the tally would balance or it would \
not, and she would walk home along the seawall with the wind behind her. ";

fn filler_tokens(session: &LlamaSession, count: usize) -> Vec<i32> {
    if count == 0 {
        return Vec::new();
    }
    let seed = session
        .vocab()
        .tokenize(FILLER_TEXT, false, true)
        .unwrap_or_default();
    if seed.is_empty() {
        // No usable tokenizer: fall back to synthetic ids. Fine for --cols,
        // and --verify-widths will report that it could not generate.
        let span = session.vocab().len().saturating_sub(2048).max(1024);
        return (0..count)
            .map(|i| (1024 + (i.wrapping_mul(7919)) % span) as i32)
            .collect();
    }
    seed.iter().copied().cycle().take(count).collect()
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    if values.is_empty() {
        return 0.0;
    }
    values[values.len() / 2]
}

struct Row {
    cols: usize,
    fill_at_start: usize,
    min_ms: f64,
    median_ms: f64,
    mean_ms: f64,
}

fn main() {
    let args = parse_args();
    makepad_ai_llm::cuda_exec::host_split_set_enabled(args.host_split);
    println!(
        "llama-batch-bench{}{}",
        if args.label.is_empty() { "" } else { " " },
        args.label
    );
    println!(
        "model={} fill={} max_context={} reps={} warmup={}",
        args.model, args.fill, args.max_context, args.reps, args.warmup
    );
    let widest = args.cols.iter().copied().max().unwrap_or(1);
    let config = LlamaSessionConfig {
        max_context: Some(args.max_context as u32),
        // One graph execution per timed step: never let the session split a
        // measurement into several batches behind our back.
        prefill_batch_size: widest.max(64),
        spec_draft_max: 0,
        ..LlamaSessionConfig::default()
    };
    let started = Instant::now();
    let mut session = match LlamaSession::load(&args.model, config) {
        Ok(session) => session,
        Err(err) => {
            eprintln!("llama-batch-bench: load: {err}");
            std::process::exit(1);
        }
    };
    println!("loaded in {:.1}s", started.elapsed().as_secs_f64());

    let rows = match measure_columns(&mut session, &args) {
        Ok(rows) => rows,
        Err(err) => {
            eprintln!("llama-batch-bench: {err}");
            std::process::exit(1);
        }
    };
    report_columns(&rows, &args);
    drop(session);

    if !args.verify_widths.is_empty() {
        measure_verify_widths(&args);
    }
}

fn measure_columns(session: &mut LlamaSession, args: &Args) -> Result<Vec<Row>, String> {
    let mut rows = Vec::new();
    for &cols in &args.cols {
        // Every column count is measured from the SAME fill, so the rows differ
        // only in width. Re-prefill rather than letting the cache drift.
        session.reset().map_err(|e| format!("reset: {e:?}"))?;
        let prompt = filler_tokens(session, args.fill);
        let prefill_started = Instant::now();
        session
            .append_tokens(&prompt)
            .map_err(|e| format!("prefill: {e:?}"))?;
        let prefill_s = prefill_started.elapsed().as_secs_f64();
        if cols == args.cols[0] {
            println!(
                "prefill {} tok in {:.3}s ({:.0} tok/s) at batch {}",
                args.fill,
                prefill_s,
                args.fill as f64 / prefill_s,
                args.cols.iter().copied().max().unwrap_or(1).max(64),
            );
        }
        let fill_at_start = session.token_count();

        let step = filler_tokens(session, cols);
        for _ in 0..args.warmup {
            session
                .append_tokens(&step)
                .map_err(|e| format!("warmup at {cols} cols: {e:?}"))?;
        }
        let mut samples = Vec::with_capacity(args.reps);
        for _ in 0..args.reps {
            let started = Instant::now();
            session
                .append_tokens(&step)
                .map_err(|e| format!("step at {cols} cols: {e:?}"))?;
            samples.push(started.elapsed().as_secs_f64() * 1e3);
        }
        let min_ms = samples.iter().copied().fold(f64::INFINITY, f64::min);
        let mean_ms = samples.iter().sum::<f64>() / samples.len() as f64;
        rows.push(Row {
            cols,
            fill_at_start,
            min_ms,
            median_ms: median(samples),
            mean_ms,
        });
        let row = rows.last().unwrap();
        println!(
            "  cols={:<4} fill={:<6} median={:8.3} ms  min={:8.3}  mean={:8.3}",
            row.cols, row.fill_at_start, row.median_ms, row.min_ms, row.mean_ms
        );
    }
    Ok(rows)
}

fn report_columns(rows: &[Row], args: &Args) {
    let Some(base) = rows.iter().find(|row| row.cols == 1) else {
        println!("\n(no cols=1 row: marginal cost is relative to the narrowest measured width)");
        return;
    };
    println!("\nn_outputs=1 column-cost curve  (fill {} tok)", args.fill);
    println!(
        "{:>5}  {:>10}  {:>12}  {:>12}  {:>12}",
        "cols", "ms/step", "ms/column", "marginal", "tok/s if all"
    );
    println!("{:->5}  {:->10}  {:->12}  {:->12}  {:->12}", "", "", "", "", "");
    for row in rows {
        // `marginal` is what each column past the first cost, which is the
        // number the aggregate ceiling (1/marginal) comes from.
        let marginal = if row.cols > 1 {
            (row.median_ms - base.median_ms) / (row.cols - 1) as f64
        } else {
            f64::NAN
        };
        println!(
            "{:>5}  {:>10.3}  {:>12.3}  {:>12}  {:>12.1}",
            row.cols,
            row.median_ms,
            row.median_ms / row.cols as f64,
            if marginal.is_nan() {
                "-".to_string()
            } else {
                format!("{marginal:.3}")
            },
            row.cols as f64 * 1e3 / row.median_ms,
        );
    }
    println!(
        "\nsolo decode (cols=1): {:.3} ms/step = {:.1} tok/s",
        base.median_ms,
        1e3 / base.median_ms
    );

    if let Some(csv_path) = &args.csv {
        let mut out = String::from("label,shape,cols,fill,median_ms,min_ms,mean_ms\n");
        for row in rows {
            out.push_str(&format!(
                "{},n_outputs=1,{},{},{:.4},{:.4},{:.4}\n",
                args.label, row.cols, row.fill_at_start, row.median_ms, row.min_ms, row.mean_ms
            ));
        }
        match std::fs::write(csv_path, out) {
            Ok(()) => println!("csv: {csv_path}"),
            Err(err) => eprintln!("csv write failed: {err}"),
        }
    }
}

/// The `n_outputs = B` curve, read off MTP speculative verification.
///
/// A verify batch is `spec_draft_max + 1` columns wide and produces a logits
/// row per column, which is exactly a B-slot decode step's shape. The session
/// times those calls itself (`SpeculativeStats::verify_nanos` over `rounds`),
/// so a short greedy generation per width is enough. The draft forwards and
/// the sampling are timed separately and excluded.
///
/// **Warm every shape the window can take, or the compile lands in the
/// window.** A `continue_greedy(n)` drafts `min(spec_draft_max, remaining - 1)`
/// tokens per round, so its last rounds run NARROWER verify batches — each a
/// graph of its own, compiled on first use and captured on the second. Left to
/// the measured window, that compile-and-capture sits inside `verify_nanos`
/// and inflates the per-round mean by several ms: the 2026-08-25 sweep on
/// `.165` read 26.1 ms at 4 columns where the live service, running the same
/// verify at a longer context, reads 21.8. So every tail shape is run twice
/// before anything is timed, and the wide shape long enough to be replaying.
/// Two windows are then measured and both printed: the second is the one to
/// quote, and a first window that disagrees with it is the compile showing.
fn measure_verify_widths(args: &Args) {
    let split = args.host_split;
    println!("\nn_outputs=B verify-shape curve  (fill {} tok)", args.fill);
    println!(
        "{:>5}  {:>3}  {:>10}  {:>12}  {:>10}  {:>8}  {:>9}  {:>9}  {:>10}{}",
        "B",
        "win",
        "ms/step",
        "ms/column",
        "rounds",
        "accept",
        "draft/rd",
        "catch/rd",
        "wall/rd",
        if split { "  gpu graph/rd  d2h/rd" } else { "" }
    );
    println!(
        "{:->5}  {:->3}  {:->10}  {:->12}  {:->10}  {:->8}  {:->9}  {:->9}  {:->10}",
        "", "", "", "", "", "", "", "", ""
    );
    for &width in &args.verify_widths {
        if width < 2 {
            println!("{width:>5}  (verify batches start at 2 columns; cols=1 is the solo row above)");
            continue;
        }
        let config = LlamaSessionConfig {
            max_context: Some(args.max_context as u32),
            prefill_batch_size: 256,
            spec_draft_max: width - 1,
            ..LlamaSessionConfig::default()
        };
        let mut session = match LlamaSession::load(&args.model, config) {
            Ok(session) => session,
            Err(err) => {
                eprintln!("  B={width}: load failed: {err}");
                continue;
            }
        };
        if !session.speculative_enabled() {
            println!("{width:>5}  (this gguf carries no MTP draft head — verify shape unavailable)");
            continue;
        }
        let prompt = filler_tokens(&session, args.fill);
        if let Err(err) = session.append_tokens(&prompt) {
            eprintln!("  B={width}: prefill failed: {err:?}");
            continue;
        }
        // Warm-up, as documented above: every narrower tail shape twice, then
        // the full-width shape until it replays.
        let mut warm_ok = true;
        let mut warm_plan: Vec<usize> = Vec::new();
        for _ in 0..2 {
            warm_plan.extend(1..width);
        }
        warm_plan.push(width * 4 + 16);
        for want in warm_plan {
            match session.continue_greedy(want) {
                Ok(warm) if warm.stop_reason != LlamaStopReason::MaxNewTokens => {
                    println!("{width:>5}  (warmup stopped: {:?})", warm.stop_reason);
                    warm_ok = false;
                    break;
                }
                Err(err) => {
                    eprintln!("  B={width}: warmup failed: {err:?}");
                    warm_ok = false;
                    break;
                }
                Ok(_) => {}
            }
        }
        if !warm_ok {
            continue;
        }
        for window in 1..=2 {
            let before = session.speculative_stats();
            let split_before = split.then(makepad_ai_llm::cuda_exec::host_split_snapshot);
            let wall = Instant::now();
            let generated = match session.continue_greedy(args.verify_tokens) {
                Ok(generated) => generated,
                Err(err) => {
                    eprintln!("  B={width}: generate failed: {err:?}");
                    break;
                }
            };
            let wall_ms = wall.elapsed().as_secs_f64() * 1e3;
            let after = session.speculative_stats();
            let (Some(before), Some(after)) = (before, after) else {
                break;
            };
            let rounds = after.rounds.saturating_sub(before.rounds);
            let nanos = after.verify_nanos.saturating_sub(before.verify_nanos);
            let draft_nanos = after.draft_nanos.saturating_sub(before.draft_nanos);
            let catchup_nanos = after.catchup_nanos.saturating_sub(before.catchup_nanos);
            let drafted = after.drafted.saturating_sub(before.drafted);
            let accepted = after.accepted.saturating_sub(before.accepted);
            if rounds == 0 {
                println!(
                    "{width:>5}  (no verify rounds ran: {} tokens, stop {:?})",
                    generated.token_ids.len(),
                    generated.stop_reason
                );
                break;
            }
            let per_round = |nanos: u64| Duration::from_nanos(nanos).as_secs_f64() * 1e3 / rounds as f64;
            let ms = per_round(nanos);
            let split_cols = match split_before {
                Some(before) => {
                    let now = makepad_ai_llm::cuda_exec::host_split_snapshot();
                    format!(
                        "  {:>12.3}  {:>6.3}",
                        (now.gpu_graph_ms - before.gpu_graph_ms) / rounds as f64,
                        (now.gpu_d2h_ms - before.gpu_d2h_ms) / rounds as f64,
                    )
                }
                None => String::new(),
            };
            println!(
                "{:>5}  {:>3}  {:>10.3}  {:>12.3}  {:>10}  {:>8.3}  {:>9.3}  {:>9.3}  {:>10.3}{}",
                width,
                window,
                ms,
                ms / width as f64,
                rounds,
                if drafted == 0 {
                    0.0
                } else {
                    accepted as f64 / drafted as f64
                },
                per_round(draft_nanos),
                per_round(catchup_nanos),
                wall_ms / rounds as f64,
                split_cols,
            );
            if generated.stop_reason != LlamaStopReason::MaxNewTokens {
                println!("{width:>5}  (window {window} stopped early: {:?})", generated.stop_reason);
                break;
            }
        }
    }
}
