//! `llama-decode-bench` — the serving shape, timed and fingerprinted.
//!
//! N lanes through the shipping `LaneExecutor`, MTP speculation at the served
//! depth, a chosen KV precision, every lane holding `--fill` tokens of real
//! context before it decodes. It prints the decode rate of the window in which
//! every lane is decoding, and a hash of every lane's token stream, so one run
//! answers both questions a kernel change has to answer: is it faster, and is
//! it the same model.
//!
//! The fill is real text, cut from the `--corpus` files: a decode step's cost
//! does not depend on which ids sit in the cache, but MTP acceptance does, and
//! acceptance is half of the decode rate. Each lane reads its own slice of the
//! corpus, so neighbours do not share a prefix.
//!
//! Prefill is timed separately and every lane is ingested before the decode
//! window opens: the scheduler prefers prefill over decode, so the window is
//! pure width-N. It closes when the first lane stops, because past that point
//! the remaining lanes run a narrower kernel shape.
//!
//! The hash covers the whole of every stream, not just the window, and the run
//! reports both the greedy and (with `--temp`) the seeded sampled stream.
//! Identical hashes across two builds mean identical tokens; a kernel change
//! that keeps every logit bit-identical keeps both.
//!
//! ```text
//! llama-decode-bench <model.gguf> --corpus FILE [--corpus FILE ...]
//!     [--kv q8|f16]      attention cache precision (default q8, the served one)
//!     [--lanes N]        slots and concurrent lanes (default 1)
//!     [--fill N]         context tokens per lane before decoding (default 8192)
//!     [--tokens N]       tokens each lane generates (default 256)
//!     [--context N]      per-lane context (default fill + tokens + 512, 256-aligned)
//!     [--spec N]         MTP draft depth (default 3, the served one)
//!     [--offset N]       first corpus token lane 0 reads (default 0)
//!     [--temp T]         sample at T with --seed instead of greedy
//!     [--seed S]         sampler seed (default 7)
//!     [--top-p P]        nucleus mass when sampling (default the served 0.9)
//!     [--top-k K]        top-k when sampling, 0 for none (default 0)
//!     [--repeat N]       timed repetitions in this process (default 1)
//!     [--label TEXT]     tag printed on every row
//!     [--force-depth N]  batched lanes speculate at depth N whatever the cost
//!                        model says (diagnostic: reaches the batched round)
//!     [--weigh-on-host]  weigh sampled rows on the host, the sampler's
//!                        reference, instead of on the device
//!
//! llama-decode-bench --ab [--rounds N] [--require-identical]
//!     --arm NAME=BINARY [--arm NAME=BINARY ...] [--arm-args NAME=ARGS ...]
//!     --cell ARGS [--cell ARGS ...] -- <common bench args>
//! ```
//!
//! `--ab` is the A/B driver: for every round, every cell and every arm it runs
//! that arm's binary with `<common args> <cell args> <arm args>` (arguments
//! split on whitespace, never through a shell), alternating the arm order
//! each round so a box that drifts warmer or cooler does not favour one arm.
//! It then prints, per cell and arm, the median, min and max decode rate, and
//! whether every arm and every round produced the same stream hashes.
//! `--require-identical` makes a hash disagreement the exit status.

use std::collections::HashMap;
use std::time::Instant;

use makepad_ai_llm::{
    LaneEvent, LaneExecutor, LaneRequest, LaneScheduler, LaneSpeculationOptions, LlamaModel,
    LlamaSamplingParams, LlamaSession, LlamaSessionConfig, LlamaVocab, TensorType,
};

struct Args {
    model: String,
    corpus: Vec<String>,
    kv: TensorType,
    lanes: usize,
    fill: usize,
    tokens: usize,
    context: usize,
    spec: usize,
    offset: usize,
    temp: f32,
    seed: u64,
    top_p: f32,
    top_k: usize,
    repeat: usize,
    label: String,
    weigh_on_host: bool,
    force_depth: Option<usize>,
}

fn usage() -> ! {
    eprintln!(
        "usage: llama-decode-bench <model.gguf> --corpus FILE [--corpus FILE ...] [--kv q8|f16]\n\
         \x20      [--lanes N] [--fill N] [--tokens N] [--context N] [--spec N] [--offset N]\n\
         \x20      [--temp T] [--seed S] [--top-p P] [--top-k K] [--repeat N] [--label TEXT]\n\
         \x20      [--force-depth N] [--weigh-on-host]"
    );
    std::process::exit(2);
}

fn parse_args() -> Args {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let Some(model) = argv.first().cloned() else {
        usage();
    };
    let mut args = Args {
        model,
        corpus: Vec::new(),
        kv: TensorType::Q8_0,
        lanes: 1,
        fill: 8192,
        tokens: 256,
        context: 0,
        spec: 3,
        offset: 0,
        temp: 0.0,
        seed: 7,
        top_p: LlamaSamplingParams::default().top_p,
        top_k: LlamaSamplingParams::default().top_k,
        repeat: 1,
        label: String::new(),
        weigh_on_host: false,
        force_depth: None,
    };
    let mut i = 1;
    while i < argv.len() {
        let value = |i: &mut usize| -> String {
            *i += 1;
            argv.get(*i).cloned().unwrap_or_else(|| usage())
        };
        let number = |text: String| -> usize { text.parse().unwrap_or_else(|_| usage()) };
        match argv[i].as_str() {
            "--corpus" => args.corpus.push(value(&mut i)),
            "--kv" => {
                args.kv = match value(&mut i).as_str() {
                    "q8" | "q8_0" => TensorType::Q8_0,
                    "f16" => TensorType::F16,
                    _ => usage(),
                }
            }
            "--lanes" => args.lanes = number(value(&mut i)).max(1),
            "--fill" => args.fill = number(value(&mut i)),
            "--tokens" => args.tokens = number(value(&mut i)).max(1),
            "--context" => args.context = number(value(&mut i)),
            "--spec" => args.spec = number(value(&mut i)),
            "--offset" => args.offset = number(value(&mut i)),
            "--temp" => args.temp = value(&mut i).parse().unwrap_or_else(|_| usage()),
            "--seed" => args.seed = value(&mut i).parse().unwrap_or_else(|_| usage()),
            "--top-p" => args.top_p = value(&mut i).parse().unwrap_or_else(|_| usage()),
            "--top-k" => args.top_k = number(value(&mut i)),
            "--repeat" => args.repeat = number(value(&mut i)).max(1),
            "--label" => args.label = value(&mut i),
            "--weigh-on-host" => args.weigh_on_host = true,
            "--force-depth" => args.force_depth = Some(number(value(&mut i))),
            other => {
                eprintln!("llama-decode-bench: unknown argument {other:?}");
                usage();
            }
        }
        i += 1;
    }
    if args.corpus.is_empty() {
        usage();
    }
    if args.context == 0 {
        args.context = (args.fill + args.tokens + 512).next_multiple_of(256);
    }
    args
}

/// FNV-1a over the little-endian token ids.
fn stream_hash(tokens: &[i32]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for token in tokens {
        for byte in token.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

/// One lane's prompt: a ChatML user turn holding `fill` corpus tokens and an
/// instruction that keeps the model writing, then the open assistant turn.
fn lane_prompt(
    vocab: &LlamaVocab,
    corpus: &[i32],
    start: usize,
    fill: usize,
) -> Result<Vec<i32>, String> {
    let open = vocab
        .tokenize("<|im_start|>user\n", true, true)
        .map_err(|e| e.to_string())?;
    let close = vocab
        .tokenize(
            "\n\nWalk through the text above section by section and explain in detail what \
             each part says or does.<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n",
            false,
            true,
        )
        .map_err(|e| e.to_string())?;
    let body = fill.saturating_sub(open.len() + close.len());
    let mut tokens = open;
    tokens.extend((0..body).map(|k| corpus[(start + k) % corpus.len()]));
    tokens.extend(close);
    Ok(tokens)
}

struct Run {
    streams: HashMap<u64, Vec<i32>>,
    prefill_seconds: f64,
    window_seconds: f64,
    window_tokens: usize,
}

fn run_once(
    session: LlamaSession,
    prompts: &[Vec<i32>],
    args: &Args,
) -> Result<(LlamaSession, Run), String> {
    let mut session = session;
    session.reset().map_err(|e| format!("reset: {e}"))?;
    let table = session.new_slot_table().map_err(|e| e.to_string())?;
    let prefill_chunk = session.config().prefill_batch_size;
    let scheduler = LaneScheduler::new(table, 8).with_prefill_chunk(prefill_chunk);
    let sampling = LlamaSamplingParams {
        temperature: args.temp,
        top_p: args.top_p,
        top_k: args.top_k,
        seed: args.seed,
        ..LlamaSamplingParams::default()
    };
    let mut exec = LaneExecutor::new(session, scheduler, sampling).with_speculation_options(
        LaneSpeculationOptions {
            forced_depth: args.force_depth,
            force_round: false,
        },
    );
    for (index, prompt) in prompts.iter().enumerate() {
        exec.scheduler()
            .submit(LaneRequest {
                job: index as u64 + 1,
                session: format!("bench-{}", index + 1),
                prompt_tokens: prompt.clone(),
                reset_first: true,
                max_new: args.tokens,
                sampling: LlamaSamplingParams {
                    seed: args.seed.wrapping_add(index as u64),
                    ..sampling
                },
            })
            .map_err(|r| format!("submit refused job {}", r.job))?;
    }

    let width = prompts.len();
    let mut run = Run {
        streams: HashMap::new(),
        prefill_seconds: 0.0,
        window_seconds: 0.0,
        window_tokens: 0,
    };
    let mut prefilled = 0usize;
    let mut finished = 0usize;
    let started = Instant::now();
    let mut window: Option<Instant> = None;
    let mut window_open = false;
    while !exec.is_idle() {
        let events = exec.step()?;
        let mut produced = 0usize;
        for event in events {
            match event {
                LaneEvent::Prefilled { .. } => prefilled += 1,
                LaneEvent::Token { job, token, .. } => {
                    run.streams.entry(job).or_default().push(token);
                    produced += 1;
                }
                LaneEvent::Finished { .. } => finished += 1,
            }
        }
        if window_open {
            run.window_tokens += produced;
            if finished > 0 {
                run.window_seconds = window.map(|w| w.elapsed().as_secs_f64()).unwrap_or(0.0);
                window_open = false;
            }
        } else if window.is_none() && prefilled == width {
            // Every lane is in; the tokens of this step were the prefills'
            // own first tokens, so the window starts after it.
            run.prefill_seconds = started.elapsed().as_secs_f64();
            window = Some(Instant::now());
            window_open = finished == 0;
        }
    }
    if window_open {
        run.window_seconds = window.map(|w| w.elapsed().as_secs_f64()).unwrap_or(0.0);
    }
    if window.is_none() {
        return Err("the lanes never all finished prefill".to_string());
    }
    Ok((exec.into_session(), run))
}

/// One `key=value` row a bench run printed.
fn row_field<'a>(row: &'a str, key: &str) -> Option<&'a str> {
    row.split_whitespace()
        .find_map(|field| field.strip_prefix(key)?.strip_prefix('='))
}

/// The `--ab` driver. Runs the arms as child processes and compares them.
fn drive_ab(argv: &[String]) -> i32 {
    let mut rounds = 3usize;
    let mut require_identical = false;
    let mut arms: Vec<(String, String)> = Vec::new();
    let mut arm_args: HashMap<String, Vec<String>> = HashMap::new();
    let mut cells: Vec<Vec<String>> = Vec::new();
    let mut common: Vec<String> = Vec::new();
    let mut i = 0;
    while i < argv.len() {
        let value = |i: &mut usize| -> String {
            *i += 1;
            argv.get(*i).cloned().unwrap_or_else(|| usage())
        };
        match argv[i].as_str() {
            "--ab" => {}
            "--rounds" => rounds = value(&mut i).parse().unwrap_or_else(|_| usage()),
            "--require-identical" => require_identical = true,
            "--arm" => {
                let text = value(&mut i);
                let Some((name, binary)) = text.split_once('=') else { usage() };
                arms.push((name.to_string(), binary.to_string()));
            }
            "--arm-args" => {
                let text = value(&mut i);
                let Some((name, extra)) = text.split_once('=') else { usage() };
                arm_args.insert(
                    name.to_string(),
                    extra.split_whitespace().map(str::to_string).collect(),
                );
            }
            "--cell" => cells.push(value(&mut i).split_whitespace().map(str::to_string).collect()),
            "--" => {
                common = argv[i + 1..].to_vec();
                break;
            }
            _ => usage(),
        }
        i += 1;
    }
    if arms.is_empty() || common.is_empty() {
        usage();
    }
    if cells.is_empty() {
        cells.push(Vec::new());
    }
    // (cell, arm) -> rates and hash lists over the rounds.
    let mut rates: HashMap<(usize, usize), Vec<f64>> = HashMap::new();
    let mut hashes: HashMap<usize, Vec<(usize, String)>> = HashMap::new();
    let mut failed: Vec<String> = Vec::new();
    for round in 0..rounds {
        for (cell_index, cell) in cells.iter().enumerate() {
            let order: Vec<usize> = if round % 2 == 0 {
                (0..arms.len()).collect()
            } else {
                (0..arms.len()).rev().collect()
            };
            for arm in order {
                let (name, binary) = &arms[arm];
                let mut command = std::process::Command::new(binary);
                command.args(&common).args(cell);
                if let Some(extra) = arm_args.get(name) {
                    command.args(extra);
                }
                command.args(["--label", name]);
                command.stderr(std::process::Stdio::inherit());
                let output = match command.output() {
                    Ok(output) => output,
                    Err(e) => {
                        failed.push(format!("round {round} cell {cell_index} arm {name}: spawn: {e}"));
                        continue;
                    }
                };
                let stdout = String::from_utf8_lossy(&output.stdout);
                print!("{stdout}");
                if !output.status.success() {
                    failed.push(format!(
                        "round {round} cell [{}] arm {name}: {}",
                        cell.join(" "),
                        output.status
                    ));
                    continue;
                }
                for row in stdout.lines().filter(|row| row.starts_with("label=")) {
                    if let Some(rate) = row_field(row, "tok_s").and_then(|t| t.parse::<f64>().ok()) {
                        rates.entry((cell_index, arm)).or_default().push(rate);
                    }
                    if let Some(h) = row_field(row, "hashes") {
                        hashes.entry(cell_index).or_default().push((arm, h.to_string()));
                    }
                }
            }
        }
    }
    println!("\nsummary over {rounds} rounds (decode tok/s of the full-width window)");
    let mut identical = true;
    for (cell_index, cell) in cells.iter().enumerate() {
        println!("cell [{}]", cell.join(" "));
        let base = rates
            .get(&(cell_index, 0))
            .map(|r| median(r))
            .unwrap_or(0.0);
        for (arm, (name, _)) in arms.iter().enumerate() {
            let Some(r) = rates.get(&(cell_index, arm)) else {
                println!("  {name:>10}  no successful run");
                continue;
            };
            let m = median(r);
            let lo = r.iter().copied().fold(f64::INFINITY, f64::min);
            let hi = r.iter().copied().fold(0.0f64, f64::max);
            let delta = if base > 0.0 { 100.0 * (m / base - 1.0) } else { 0.0 };
            println!(
                "  {name:>10}  median {m:8.1}  min {lo:8.1}  max {hi:8.1}  n={}  vs {}: {delta:+.1}%",
                r.len(),
                arms[0].0
            );
        }
        let seen = hashes.get(&cell_index).cloned().unwrap_or_default();
        let distinct: std::collections::BTreeSet<&str> = seen.iter().map(|(_, h)| h.as_str()).collect();
        if distinct.len() <= 1 {
            println!("  streams: identical across arms and rounds ({} runs)", seen.len());
        } else {
            identical = false;
            println!("  streams: DIFFER ({} distinct over {} runs)", distinct.len(), seen.len());
            for (arm, h) in &seen {
                println!("    {:>10} {h}", arms[*arm].0);
            }
        }
    }
    for line in &failed {
        println!("FAILED {line}");
    }
    if !failed.is_empty() {
        return 1;
    }
    if require_identical && !identical {
        println!("FAIL: the arms disagree on the token streams");
        return 3;
    }
    0
}

fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    if sorted.is_empty() {
        0.0
    } else if sorted.len() % 2 == 1 {
        sorted[sorted.len() / 2]
    } else {
        0.5 * (sorted[sorted.len() / 2 - 1] + sorted[sorted.len() / 2])
    }
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.first().map(String::as_str) == Some("--ab") {
        std::process::exit(drive_ab(&argv));
    }
    let args = parse_args();
    let model = LlamaModel::load(std::path::Path::new(&args.model)).unwrap_or_else(|e| {
        eprintln!("load {}: {e}", args.model);
        std::process::exit(1);
    });
    let vocab = LlamaVocab::from_model(&model).unwrap_or_else(|e| {
        eprintln!("vocab: {e}");
        std::process::exit(1);
    });
    let mut corpus = Vec::new();
    for path in &args.corpus {
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| {
            eprintln!("corpus {path}: {e}");
            std::process::exit(1);
        });
        corpus.extend(vocab.tokenize(&text, false, false).unwrap_or_else(|e| {
            eprintln!("tokenize {path}: {e}");
            std::process::exit(1);
        }));
    }
    if corpus.is_empty() {
        eprintln!("the corpus tokenized to nothing");
        std::process::exit(1);
    }
    // Lanes read evenly spaced slices of the corpus, so no two share a prefix
    // until the corpus is shorter than lanes x fill.
    let stride = (corpus.len() / args.lanes).max(1);
    let prompts: Vec<Vec<i32>> = (0..args.lanes)
        .map(|lane| lane_prompt(&vocab, &corpus, args.offset + lane * stride, args.fill))
        .collect::<Result<_, _>>()
        .unwrap_or_else(|e| {
            eprintln!("prompt: {e}");
            std::process::exit(1);
        });

    let config = LlamaSessionConfig {
        max_context: Some(args.context as u32),
        max_sequences: args.lanes as u32,
        attention_k_type: args.kv,
        attention_v_type: args.kv,
        spec_draft_max: args.spec,
        weigh_on_host: args.weigh_on_host,
        ..LlamaSessionConfig::default()
    };
    let mut session = LlamaSession::from_model(&model, config).unwrap_or_else(|e| {
        eprintln!("session: {e}");
        std::process::exit(1);
    });
    let memory = session.memory().ok();
    eprintln!(
        "device {} · kv {:?} · {} lanes x {} context · fill {} · corpus {} tokens · restricted draft head {}",
        session.device_name(),
        args.kv,
        args.lanes,
        args.context,
        args.fill,
        corpus.len(),
        session.has_restricted_draft_head()
    );
    if let Some(memory) = memory {
        eprintln!("memory {memory:?}");
    }
    for rep in 0..args.repeat {
        let before = session.speculative_stats().unwrap_or_default();
        let (returned, run) = run_once(session, &prompts, &args).unwrap_or_else(|e| {
            eprintln!("run: {e}");
            std::process::exit(1);
        });
        session = returned;
        let after = session.speculative_stats().unwrap_or_default();
        let mut jobs: Vec<_> = run.streams.keys().copied().collect();
        jobs.sort_unstable();
        let hashes: Vec<String> = jobs
            .iter()
            .map(|job| format!("{:016x}/{}", stream_hash(&run.streams[job]), run.streams[job].len()))
            .collect();
        let rounds = after.rounds.saturating_sub(before.rounds).max(1) as f64;
        let rate = run.window_tokens as f64 / run.window_seconds.max(1e-9);
        println!(
            "label={} rep={} kv={} lanes={} fill={} temp={} prefill_s={:.2} window_tokens={} \
             window_s={:.3} tok_s={:.1} per_lane={:.1} solo_rounds={} accept={:.3} \
             draft_ms_round={:.2} verify_ms_round={:.2} weighed_rows={} wide_rows={} hashes={}",
            args.label,
            rep,
            if args.kv == TensorType::F16 { "f16" } else { "q8" },
            args.lanes,
            args.fill,
            args.temp,
            run.prefill_seconds,
            run.window_tokens,
            run.window_seconds,
            rate,
            rate / args.lanes as f64,
            after.rounds.saturating_sub(before.rounds),
            {
                let drafted = after.drafted.saturating_sub(before.drafted);
                let accepted = after.accepted.saturating_sub(before.accepted);
                if drafted == 0 { 0.0 } else { accepted as f64 / drafted as f64 }
            },
            after.draft_nanos.saturating_sub(before.draft_nanos) as f64 / rounds / 1e6,
            after.verify_nanos.saturating_sub(before.verify_nanos) as f64 / rounds / 1e6,
            after.weighed_rows.saturating_sub(before.weighed_rows),
            after.wide_rows.saturating_sub(before.wide_rows),
            hashes.join(",")
        );
    }
}
