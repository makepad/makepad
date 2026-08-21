use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::Instant;

use makepad_ai_llm::{LlamaModel, LlamaSession, LlamaSessionConfig, LlamaStopReason, LlamaVocab};

const DEFAULT_MAX_NEW_TOKENS: usize = 64;
const DEFAULT_UPSTREAM_COMPLETION_BIN: &str =
    "local/llama.cpp/build-arm64-apple-clang-release/bin/llama-completion";

struct Args {
    model_path: PathBuf,
    prompt: String,
    max_new_tokens: usize,
    max_context: Option<usize>,
    prefill_batch_size: usize,
    upstream_completion_bin: PathBuf,
    no_bos: bool,
    parse_special: bool,
    dump_token_ids: bool,
    no_stream: bool,
    verify_upstream: bool,
    bench_pp: Option<usize>,
    bench_tg: Option<usize>,
    spec_draft_max: usize,
    spec_gate: bool,
    spec_sample_gate: bool,
    build_draft_vocab: bool,
    draft_vocab_coverage: f64,
    draft_vocab_text: Vec<PathBuf>,
    draft_vocab_eval: Vec<PathBuf>,
    spec_determinism_runs: usize,
    prompt_dir: Option<PathBuf>,
    runs: usize,
    temperature: f32,
    top_p: f32,
    seed: u64,
}

fn main() {
    match run() {
        Ok(()) => {}
        Err(err) => {
            eprintln!("llama-generate failed: {err}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_args(std::env::args_os())?;
    let model = LlamaModel::load(&args.model_path)?;
    let vocab = LlamaVocab::from_model(&model)?;
    let prompt_token_ids = if args.prompt.is_empty() {
        Vec::new()
    } else {
        let ids = tokenize_prompt(&vocab, &args)?;
        if ids.is_empty() {
            return Err("tokenizer produced no prompt tokens".into());
        }
        ids
    };

    if args.dump_token_ids {
        eprintln!("prompt.token_ids: {:?}", prompt_token_ids);
    }

    if args.bench_pp.is_some() || args.bench_tg.is_some() {
        return run_bench(&model, &vocab, &args, prompt_token_ids);
    }

    if args.build_draft_vocab {
        return run_build_draft_vocab(&model, &vocab, &args);
    }

    if !args.draft_vocab_eval.is_empty() {
        return run_draft_vocab_eval(&model, &vocab, &args);
    }

    if args.spec_sample_gate {
        return run_spec_sample_gate(&model, &vocab, &args);
    }

    if args.spec_determinism_runs > 0 {
        return run_spec_determinism(&model, &args, prompt_token_ids);
    }

    if args.spec_gate {
        return run_spec_gate(&model, &args, prompt_token_ids);
    }

    let max_context = match args.max_context {
        Some(value) => value,
        None => prompt_token_ids
            .len()
            .checked_add(args.max_new_tokens)
            .ok_or("overflow computing total generation context")?,
    };
    let mut session = LlamaSession::from_model(
        &model,
        LlamaSessionConfig {
            max_context: Some(u32::try_from(max_context)?),
            prefill_batch_size: args.prefill_batch_size,
            spec_draft_max: args.spec_draft_max,
            ..LlamaSessionConfig::default()
        },
    )?;

    if !args.no_stream {
        print!("{}", args.prompt);
        std::io::stdout().flush()?;
    }

    let total_start = Instant::now();
    let prefill_start = Instant::now();
    session.append_tokens(&prompt_token_ids)?;
    let prefill_elapsed = prefill_start.elapsed();
    if std::env::var_os("MAKEPAD_LLAMA_DUMP_STATE").is_some() {
        for (name, sum, max, nans) in session.debug_cache_fingerprints() {
            eprintln!("state.{}: sum={:.3} max={:.3} nans={}", name, sum, max, nans);
        }
    }
    // Cross-backend numerical comparison: exact post-prefill logits stats.
    if std::env::var_os("MAKEPAD_LLAMA_LOGITS_STATS").is_some() {
        if let Some(logits) = session.last_logits() {
            let (argmax, top) = logits.iter().copied().enumerate().fold(
                (0usize, f32::NEG_INFINITY),
                |acc, (index, value)| if value > acc.1 { (index, value) } else { acc },
            );
            let l2 = logits.iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>().sqrt();
            eprintln!(
                "logits.stats: n={} argmax={} top={:.6} l2={:.4} head={:?}",
                logits.len(),
                argmax,
                top,
                l2,
                &logits[..8.min(logits.len())]
            );
        }
    }

    let generation_start = Instant::now();
    let generation = session.continue_greedy(args.max_new_tokens)?;
    let generation_elapsed = generation_start.elapsed();
    let total_elapsed = total_start.elapsed();

    if !args.no_stream {
        print!("{}", generation.text);
        println!();
    }

    if args.dump_token_ids {
        eprintln!("generated.token_ids: {:?}", generation.token_ids);
    }

    if args.verify_upstream {
        let upstream_output = run_upstream_completion(&args, prompt_token_ids.len())?;
        verify_exact_output(&generation.text, &upstream_output)?;
        eprintln!("verify.upstream.exact_text_match: true");
        eprintln!("verify.upstream.generated_bytes: {}", upstream_output.len());
    }

    eprintln!("stop.reason: {}", stop_reason_name(generation.stop_reason));
    eprintln!("prefill.batch_size: {}", args.prefill_batch_size);
    eprintln!("prompt.tokens: {}", prompt_token_ids.len());
    eprintln!("generated.tokens: {}", generation.token_ids.len());
    eprintln!("prefill.seconds: {:.3}", prefill_elapsed.as_secs_f64());
    eprintln!(
        "prefill.tok_s: {:.3}",
        tok_per_second(prompt_token_ids.len(), prefill_elapsed.as_secs_f64())
    );
    eprintln!(
        "generation.seconds: {:.3}",
        generation_elapsed.as_secs_f64()
    );
    eprintln!(
        "generation.tok_s: {:.3}",
        tok_per_second(generation.token_ids.len(), generation_elapsed.as_secs_f64())
    );
    eprintln!("total.seconds: {:.3}", total_elapsed.as_secs_f64());
    eprintln!(
        "total.tok_s: {:.3}",
        tok_per_second(
            prompt_token_ids.len() + generation.token_ids.len(),
            total_elapsed.as_secs_f64()
        )
    );
    Ok(())
}

fn parse_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<Args, Box<dyn std::error::Error>> {
    let mut args = args.into_iter();
    let _exe = args.next();

    let mut model_path = None;
    let mut prompt = None;
    let mut prompt_parts = Vec::new();
    let mut max_new_tokens = DEFAULT_MAX_NEW_TOKENS;
    let mut max_context = None;
    let mut prefill_batch_size = 1usize;
    let mut upstream_completion_bin = PathBuf::from(DEFAULT_UPSTREAM_COMPLETION_BIN);
    let mut no_bos = false;
    let mut parse_special = true;
    let mut dump_token_ids = false;
    let mut no_stream = false;
    let mut verify_upstream = false;
    let mut bench_pp = None;
    let mut bench_tg = None;
    let mut spec_draft_max = 0usize;
    let mut spec_gate = false;
    let mut spec_sample_gate = false;
    let mut build_draft_vocab = false;
    let mut draft_vocab_coverage = 0.975f64;
    let mut draft_vocab_text: Vec<PathBuf> = Vec::new();
    let mut draft_vocab_eval: Vec<PathBuf> = Vec::new();
    let mut spec_determinism_runs = 0usize;
    let mut prompt_dir = None;
    let mut runs = 20usize;
    let mut temperature = 0.7f32;
    let mut top_p = 0.9f32;
    let mut seed = 7u64;

    while let Some(arg) = args.next() {
        match arg.to_string_lossy().as_ref() {
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            "--max-new-tokens" => {
                let value = args.next().ok_or("--max-new-tokens requires a value")?;
                max_new_tokens = value.to_string_lossy().parse()?;
            }
            "--prefill-batch-size" => {
                let value = args.next().ok_or("--prefill-batch-size requires a value")?;
                prefill_batch_size = value.to_string_lossy().parse()?;
            }
            "--max-context" => {
                let value = args.next().ok_or("--max-context requires a value")?;
                max_context = Some(value.to_string_lossy().parse()?);
            }
            "--prompt" => {
                let value = args.next().ok_or("--prompt requires a value")?;
                prompt = Some(value.to_string_lossy().into_owned());
            }
            "--prompt-file" => {
                let value = args.next().ok_or("--prompt-file requires a value")?;
                prompt = Some(std::fs::read_to_string(PathBuf::from(value))?);
            }
            "--tokenize-bin" => {
                let _ = args.next().ok_or("--tokenize-bin requires a value")?;
            }
            "--upstream-completion-bin" => {
                let value = args
                    .next()
                    .ok_or("--upstream-completion-bin requires a value")?;
                upstream_completion_bin = PathBuf::from(value);
            }
            "--no-bos" => {
                no_bos = true;
            }
            "--parse-special" => {
                parse_special = true;
            }
            "--no-parse-special" => {
                parse_special = false;
            }
            "--dump-token-ids" => {
                dump_token_ids = true;
            }
            "--no-stream" => {
                no_stream = true;
            }
            "--verify-upstream" => {
                verify_upstream = true;
            }
            "--bench-pp" => {
                let value = args.next().ok_or("--bench-pp requires a value")?;
                bench_pp = Some(value.to_string_lossy().parse()?);
            }
            "--bench-tg" => {
                let value = args.next().ok_or("--bench-tg requires a value")?;
                bench_tg = Some(value.to_string_lossy().parse()?);
            }
            "--spec-draft-max" => {
                let value = args.next().ok_or("--spec-draft-max requires a value")?;
                spec_draft_max = value.to_string_lossy().parse()?;
            }
            "--spec-gate" => {
                spec_gate = true;
            }
            "--spec-sample-gate" => {
                spec_sample_gate = true;
            }
            "--build-draft-vocab" => {
                build_draft_vocab = true;
            }
            "--draft-vocab-coverage" => {
                let value = args.next().ok_or("--draft-vocab-coverage requires a value")?;
                draft_vocab_coverage = value.to_string_lossy().parse()?;
            }
            "--draft-vocab-text" => {
                let value = args.next().ok_or("--draft-vocab-text requires a value")?;
                draft_vocab_text.push(PathBuf::from(value));
            }
            "--draft-vocab-eval" => {
                let value = args.next().ok_or("--draft-vocab-eval requires a value")?;
                draft_vocab_eval.push(PathBuf::from(value));
            }
            "--spec-determinism-runs" => {
                let value = args.next().ok_or("--spec-determinism-runs requires a value")?;
                spec_determinism_runs = value.to_string_lossy().parse()?;
            }
            "--prompt-dir" => {
                let value = args.next().ok_or("--prompt-dir requires a value")?;
                prompt_dir = Some(PathBuf::from(value));
            }
            "--runs" => {
                let value = args.next().ok_or("--runs requires a value")?;
                runs = value.to_string_lossy().parse()?;
            }
            "--temperature" => {
                let value = args.next().ok_or("--temperature requires a value")?;
                temperature = value.to_string_lossy().parse()?;
            }
            "--top-p" => {
                let value = args.next().ok_or("--top-p requires a value")?;
                top_p = value.to_string_lossy().parse()?;
            }
            "--seed" => {
                let value = args.next().ok_or("--seed requires a value")?;
                seed = value.to_string_lossy().parse()?;
            }
            _ if model_path.is_none() => {
                model_path = Some(PathBuf::from(arg));
            }
            _ => {
                prompt_parts.push(arg.to_string_lossy().into_owned());
            }
        }
    }

    let model_path = model_path.ok_or_else(|| {
        print_usage();
        "usage: llama-generate <model.gguf> [--max-new-tokens N] [--prompt TEXT | prompt words ...]"
    })?;
    let prompt = prompt.unwrap_or_else(|| prompt_parts.join(" "));
    if prompt.is_empty() && prompt_dir.is_none() && draft_vocab_eval.is_empty() {
        return Err("missing prompt text".into());
    }

    Ok(Args {
        model_path,
        prompt,
        max_new_tokens,
        max_context,
        prefill_batch_size,
        upstream_completion_bin,
        no_bos,
        parse_special,
        dump_token_ids,
        no_stream,
        verify_upstream,
        bench_pp,
        bench_tg,
        spec_draft_max,
        spec_gate,
        spec_sample_gate,
        build_draft_vocab,
        draft_vocab_coverage,
        draft_vocab_text,
        draft_vocab_eval,
        spec_determinism_runs,
        prompt_dir,
        runs,
        temperature,
        top_p,
        seed,
    })
}


/// Greedy determinism hammer: N identical runs, speculation off then on. Any
/// run-to-run difference is a nondeterminism bug, and any base-vs-spec
/// difference is a losslessness bug.
fn run_spec_determinism(
    model: &LlamaModel,
    args: &Args,
    prompt_token_ids: Vec<i32>,
) -> Result<(), Box<dyn std::error::Error>> {
    let max_context = args
        .max_context
        .unwrap_or(prompt_token_ids.len() + args.max_new_tokens + 16)
        .max(256);
    let mut results: Vec<(usize, Vec<Vec<i32>>)> = Vec::new();
    for spec_draft_max in [0usize, args.spec_draft_max] {
        let mut runs = Vec::with_capacity(args.spec_determinism_runs);
        for _ in 0..args.spec_determinism_runs {
            let mut session = LlamaSession::from_model(
                model,
                LlamaSessionConfig {
                    max_context: Some(u32::try_from(max_context)?),
                    prefill_batch_size: args.prefill_batch_size,
                    spec_draft_max,
                    ..LlamaSessionConfig::default()
                },
            )?;
            session.append_tokens(&prompt_token_ids)?;
            runs.push(session.continue_greedy(args.max_new_tokens)?.token_ids);
        }
        results.push((spec_draft_max, runs));
        if args.spec_draft_max == 0 {
            break;
        }
    }

    let mut ok = true;
    for (spec_draft_max, runs) in &results {
        let identical = runs.iter().all(|run| run == &runs[0]);
        println!(
            "spec_determinism.draft_max{}: {}/{} identical ({} tokens)",
            spec_draft_max,
            runs.iter().filter(|run| *run == &runs[0]).count(),
            runs.len(),
            runs[0].len()
        );
        ok &= identical;
    }
    if results.len() == 2 {
        let lossless = results[0].1[0] == results[1].1[0];
        println!("spec_determinism.lossless: {lossless}");
        ok &= lossless;
    }
    if !ok {
        return Err("determinism/losslessness hammer failed".into());
    }
    Ok(())
}

/// Does `tokens` contain an 8-gram that repeats 4+ times inside any 256-token
/// window? That is the shape a degenerate loop takes.
fn has_degenerate_repeat(tokens: &[i32]) -> bool {
    const GRAM: usize = 8;
    const WINDOW: usize = 256;
    if tokens.len() < GRAM {
        return false;
    }
    let grams: Vec<&[i32]> = tokens.windows(GRAM).collect();
    for start in (0..grams.len()).step_by(WINDOW / 2) {
        let end = (start + WINDOW).min(grams.len());
        let mut counts: std::collections::HashMap<&[i32], usize> = std::collections::HashMap::new();
        for gram in &grams[start..end] {
            let count = counts.entry(*gram).or_insert(0);
            *count += 1;
            if *count >= 4 {
                return true;
            }
        }
        if end == grams.len() {
            break;
        }
    }
    false
}

/// Minimal strict JSON validator: does `text` contain a complete, well-formed
/// JSON object? Used to score tool-call outputs without pulling in a parser.
fn contains_valid_json_object(text: &str) -> bool {
    let bytes = text.as_bytes();
    for (start, byte) in bytes.iter().enumerate() {
        if *byte != b'{' {
            continue;
        }
        let mut parser = JsonParser {
            bytes,
            position: start,
        };
        if parser.value() && parser.position > start {
            return true;
        }
    }
    false
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl JsonParser<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.position += 1;
        }
    }

    fn literal(&mut self, text: &str) -> bool {
        if self.bytes[self.position..].starts_with(text.as_bytes()) {
            self.position += text.len();
            true
        } else {
            false
        }
    }

    fn string(&mut self) -> bool {
        if self.peek() != Some(b'"') {
            return false;
        }
        self.position += 1;
        while let Some(byte) = self.peek() {
            self.position += 1;
            match byte {
                b'"' => return true,
                b'\\' => {
                    if self.peek().is_none() {
                        return false;
                    }
                    self.position += 1;
                }
                _ => {}
            }
        }
        false
    }

    fn number(&mut self) -> bool {
        let start = self.position;
        if self.peek() == Some(b'-') {
            self.position += 1;
        }
        while matches!(self.peek(), Some(b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')) {
            self.position += 1;
        }
        self.position > start
    }

    fn value(&mut self) -> bool {
        self.skip_whitespace();
        match self.peek() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => self.string(),
            Some(b't') => self.literal("true"),
            Some(b'f') => self.literal("false"),
            Some(b'n') => self.literal("null"),
            Some(_) => self.number(),
            None => false,
        }
    }

    fn object(&mut self) -> bool {
        self.position += 1;
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.position += 1;
            return true;
        }
        loop {
            self.skip_whitespace();
            if !self.string() {
                return false;
            }
            self.skip_whitespace();
            if self.peek() != Some(b':') {
                return false;
            }
            self.position += 1;
            if !self.value() {
                return false;
            }
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.position += 1,
                Some(b'}') => {
                    self.position += 1;
                    return true;
                }
                _ => return false,
            }
        }
    }

    fn array(&mut self) -> bool {
        self.position += 1;
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.position += 1;
            return true;
        }
        loop {
            if !self.value() {
                return false;
            }
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.position += 1,
                Some(b']') => {
                    self.position += 1;
                    return true;
                }
                _ => return false,
            }
        }
    }
}

#[derive(Default)]
struct SampleGateTally {
    runs: usize,
    tokens: usize,
    hit_max_tokens: usize,
    degenerate: usize,
    valid_json: usize,
    seconds: f64,
}

impl SampleGateTally {
    fn line(&self, label: &str) -> String {
        format!(
            "{label}: runs={} mean_tokens={:.1} hit_max={} degenerate={} valid_json={} tok_s={:.2}",
            self.runs,
            self.tokens as f64 / self.runs.max(1) as f64,
            self.hit_max_tokens,
            self.degenerate,
            self.valid_json,
            self.tokens as f64 / self.seconds.max(1e-9)
        )
    }
}

/// Sampled anti-madness gate: for every prompt class, run the fleet's default
/// sampler with speculation off and on and compare the shape of the output —
/// mean length, runs that never stop, degenerate loops, and JSON validity.
/// Speculative rejection sampling is distribution-preserving, so these counts
/// must be statistically indistinguishable; a bad rollback shows up here as
/// loops or truncation long before it shows up as a crash.
fn run_spec_sample_gate(
    model: &LlamaModel,
    vocab: &LlamaVocab,
    args: &Args,
) -> Result<(), Box<dyn std::error::Error>> {
    let dir = args
        .prompt_dir
        .as_ref()
        .ok_or("--spec-sample-gate requires --prompt-dir")?;
    let mut classes: Vec<(String, String)> = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("txt") {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("prompt")
            .to_string();
        classes.push((name, std::fs::read_to_string(&path)?));
    }
    classes.sort_by(|a, b| a.0.cmp(&b.0));
    if classes.is_empty() {
        return Err("--prompt-dir contains no .txt prompt files".into());
    }

    for (name, prompt) in &classes {
        let tokens = vocab.tokenize(prompt, !args.no_bos, args.parse_special)?;
        let max_context = args
            .max_context
            .unwrap_or(tokens.len() + args.max_new_tokens + 64)
            .max(256);
        let mut tallies = Vec::new();
        for spec_draft_max in [0usize, args.spec_draft_max] {
            let mut tally = SampleGateTally::default();
            let mut acceptance = None;
            for run in 0..args.runs {
                let mut session = LlamaSession::from_model(
                    model,
                    LlamaSessionConfig {
                        max_context: Some(u32::try_from(max_context)?),
                        prefill_batch_size: args.prefill_batch_size,
                        spec_draft_max,
                        ..LlamaSessionConfig::default()
                    },
                )?;
                session.append_tokens(&tokens)?;
                let params = makepad_ai_llm::LlamaSamplingParams {
                    temperature: args.temperature,
                    top_p: args.top_p,
                    top_k: 0,
                    seed: args.seed + run as u64,
                };
                let start = Instant::now();
                let generation = session.continue_sampled(args.max_new_tokens, params)?;
                tally.seconds += start.elapsed().as_secs_f64();
                tally.runs += 1;
                tally.tokens += generation.token_ids.len();
                if matches!(generation.stop_reason, LlamaStopReason::MaxNewTokens) {
                    tally.hit_max_tokens += 1;
                }
                if has_degenerate_repeat(&generation.token_ids) {
                    tally.degenerate += 1;
                }
                if contains_valid_json_object(&generation.text) {
                    tally.valid_json += 1;
                }
                acceptance = session.speculative_stats();
            }
            let label = if spec_draft_max == 0 {
                format!("sample_gate.{name}.base")
            } else {
                format!("sample_gate.{name}.spec{spec_draft_max}")
            };
            println!("{}", tally.line(&label));
            if let Some(stats) = acceptance {
                println!(
                    "sample_gate.{name}.acceptance: {:.4} ({:.3} tokens/round)",
                    stats.acceptance(),
                    stats.tokens_per_round()
                );
            }
            tallies.push(tally);
            if args.spec_draft_max == 0 {
                break;
            }
        }
        if tallies.len() == 2 {
            println!(
                "sample_gate.{name}.delta: mean_tokens {:+.1} hit_max {:+} degenerate {:+} valid_json {:+}",
                tallies[1].tokens as f64 / tallies[1].runs.max(1) as f64
                    - tallies[0].tokens as f64 / tallies[0].runs.max(1) as f64,
                tallies[1].hit_max_tokens as i64 - tallies[0].hit_max_tokens as i64,
                tallies[1].degenerate as i64 - tallies[0].degenerate as i64,
                tallies[1].valid_json as i64 - tallies[0].valid_json as i64,
            );
        }
    }
    Ok(())
}

/// Build the MTP draft head's restricted output vocabulary from the model's
/// OWN outputs: sample a corpus across the prompt classes, count which token
/// ids the model actually emits, and keep the smallest set covering
/// `--draft-vocab-coverage` of those occurrences. Writes `<model>.draftvocab`,
/// which every later session picks up automatically.
///
/// Speculation is off while generating, so the corpus is the plain model's
/// distribution and cannot be biased by the very head it is used to build.
fn run_build_draft_vocab(
    model: &LlamaModel,
    vocab: &LlamaVocab,
    args: &Args,
) -> Result<(), Box<dyn std::error::Error>> {
    let dir = args
        .prompt_dir
        .as_ref()
        .ok_or("--build-draft-vocab requires --prompt-dir")?;
    let mut classes: Vec<(String, String)> = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("txt") {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("prompt")
            .to_string();
        classes.push((name, std::fs::read_to_string(&path)?));
    }
    classes.sort_by(|a, b| a.0.cmp(&b.0));
    if classes.is_empty() {
        return Err("--prompt-dir contains no .txt prompt files".into());
    }

    let mut counts = vec![0u64; vocab.len()];
    let mut generated = 0u64;
    let mut context_tokens = 0u64;

    // Reference text broadens the set far more cheaply than generation does
    // (tokenising is free) and it is the same kind of distribution the model
    // emits. Outputs alone, at any corpus size we can afford to generate, badly
    // over-fit: a 25 k-token outputs-only set collapsed draft acceptance from
    // 0.74 to 0.24 on prompts outside the corpus.
    for path in &args.draft_vocab_text {
        let text = std::fs::read_to_string(path)?;
        let tokens = vocab.tokenize(&text, false, false)?;
        for token in &tokens {
            if let Ok(index) = usize::try_from(*token) {
                if index < counts.len() {
                    counts[index] += 1;
                    context_tokens += 1;
                }
            }
        }
        println!(
            "draft_vocab.text.{}: {} tokens",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("text"),
            tokens.len()
        );
    }

    for (name, prompt) in &classes {
        let tokens = vocab.tokenize(prompt, !args.no_bos, args.parse_special)?;
        // The prompts are themselves representative text the model continues.
        for token in &tokens {
            if let Ok(index) = usize::try_from(*token) {
                if index < counts.len() {
                    counts[index] += 1;
                    context_tokens += 1;
                }
            }
        }
        let max_context = args
            .max_context
            .unwrap_or(tokens.len() + args.max_new_tokens + 64)
            .max(256);
        let mut class_tokens = 0u64;
        for run in 0..args.runs {
            let mut session = LlamaSession::from_model(
                model,
                LlamaSessionConfig {
                    max_context: Some(u32::try_from(max_context)?),
                    prefill_batch_size: args.prefill_batch_size,
                    spec_draft_max: 0,
                    ..LlamaSessionConfig::default()
                },
            )?;
            session.append_tokens(&tokens)?;
            let params = makepad_ai_llm::LlamaSamplingParams {
                temperature: args.temperature,
                top_p: args.top_p,
                top_k: 0,
                seed: args.seed + run as u64,
            };
            let generation = session.continue_sampled(args.max_new_tokens, params)?;
            for token in &generation.token_ids {
                if let Ok(index) = usize::try_from(*token) {
                    if index < counts.len() {
                        counts[index] += 1;
                        class_tokens += 1;
                    }
                }
            }
        }
        generated += class_tokens;
        println!("draft_vocab.corpus.{name}: {class_tokens} tokens over {} runs", args.runs);
    }

    // A draft head that cannot propose the stop tokens turns every end of turn
    // into a forced rejection, so they are pinned in regardless of frequency.
    let mut required = Vec::new();
    required.extend(vocab.eos_token_id());
    required.extend(vocab.padding_token_id());
    required.extend(vocab.bos_token_id());

    let draft_vocab =
        makepad_ai_llm::DraftVocab::select(&counts, args.draft_vocab_coverage, &required, 256)?;
    let distinct = counts.iter().filter(|count| **count > 0).count();
    let path = makepad_ai_llm::DraftVocab::sidecar_path(&model.gguf.path);
    draft_vocab.write(&path)?;

    println!("draft_vocab.generated_tokens: {generated}");
    println!("draft_vocab.context_tokens: {context_tokens}");
    println!("draft_vocab.corpus_tokens: {}", generated + context_tokens);
    println!("draft_vocab.distinct_tokens: {distinct}");
    println!("draft_vocab.full_vocab: {}", vocab.len());
    println!("draft_vocab.kept: {}", draft_vocab.len());
    println!("draft_vocab.coverage: {:.4}", draft_vocab.coverage());
    println!(
        "draft_vocab.head_fraction: {:.4}",
        draft_vocab.len() as f64 / vocab.len() as f64
    );
    println!("draft_vocab.path: {}", path.display());
    Ok(())
}

/// Measure an existing `<model>.gguf.draftvocab` against text it was not built
/// from. This is the instrument the corpus law needs: a set that covers its own
/// corpus proves nothing, and a set that misses the held-out distribution turns
/// speculation into a slowdown (every uncovered position is a forced
/// rejection). Reports, per file and in total, the share of token
/// **occurrences** the set covers — the quantity that predicts acceptance —
/// alongside the distinct-token share and the heaviest misses.
fn run_draft_vocab_eval(
    model: &LlamaModel,
    vocab: &LlamaVocab,
    args: &Args,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = makepad_ai_llm::DraftVocab::sidecar_path(&model.gguf.path);
    let draft_vocab = makepad_ai_llm::DraftVocab::read(&path)?;
    if draft_vocab.vocab_size as usize != vocab.len() {
        return Err(format!(
            "{} was built for a {}-token vocabulary, model has {}",
            path.display(),
            draft_vocab.vocab_size,
            vocab.len()
        )
        .into());
    }
    let mut kept = vec![false; vocab.len()];
    for id in &draft_vocab.ids {
        if let Ok(index) = usize::try_from(*id) {
            if index < kept.len() {
                kept[index] = true;
            }
        }
    }
    println!("draft_vocab.path: {}", path.display());
    println!("draft_vocab.kept: {}", draft_vocab.len());
    println!("draft_vocab.build_coverage: {:.4}", draft_vocab.coverage());

    let mut total_counts = vec![0u64; vocab.len()];
    for file in &args.draft_vocab_eval {
        let text = std::fs::read_to_string(file)?;
        // Same tokenisation as `--draft-vocab-text`, so build and eval count
        // the same way.
        let tokens = vocab.tokenize(&text, false, false)?;
        let mut hit = 0u64;
        let mut seen = std::collections::BTreeSet::new();
        let mut seen_hit = std::collections::BTreeSet::new();
        for token in &tokens {
            let Ok(index) = usize::try_from(*token) else {
                continue;
            };
            if index >= kept.len() {
                continue;
            }
            total_counts[index] += 1;
            seen.insert(index);
            if kept[index] {
                hit += 1;
                seen_hit.insert(index);
            }
        }
        let name = file
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("text");
        let total = tokens.len().max(1) as f64;
        println!(
            "draft_vocab.eval.{name}: tokens={} covered={:.4} distinct={} distinct_covered={:.4}",
            tokens.len(),
            hit as f64 / total,
            seen.len(),
            seen_hit.len() as f64 / seen.len().max(1) as f64
        );
    }

    let total: u64 = total_counts.iter().sum();
    let covered: u64 = total_counts
        .iter()
        .enumerate()
        .filter(|(index, _)| kept[*index])
        .map(|(_, count)| *count)
        .sum();
    let distinct = total_counts.iter().filter(|count| **count > 0).count();
    let distinct_covered = total_counts
        .iter()
        .enumerate()
        .filter(|(index, count)| **count > 0 && kept[*index])
        .count();
    println!("draft_vocab.eval.total_tokens: {total}");
    println!(
        "draft_vocab.eval.coverage: {:.4}",
        covered as f64 / total.max(1) as f64
    );
    println!(
        "draft_vocab.eval.distinct_coverage: {:.4} ({distinct_covered} of {distinct})",
        distinct_covered as f64 / distinct.max(1) as f64
    );

    // The heaviest misses are the actionable output: they say what the corpus
    // is missing, or that the coverage knob is set too low.
    let mut misses: Vec<(u64, usize)> = total_counts
        .iter()
        .enumerate()
        .filter(|(index, count)| **count > 0 && !kept[*index])
        .map(|(index, count)| (*count, index))
        .collect();
    misses.sort_unstable_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    for (count, index) in misses.iter().take(20) {
        let piece = vocab
            .escaped_piece(*index as i32)
            .unwrap_or_else(|| "<invalid-token-id>".to_owned());
        println!("draft_vocab.eval.miss: {count}\t{index}\t{piece}");
    }
    Ok(())
}

/// Anti-madness gate for MTP speculative decoding.
///
/// 1. lossless: greedy output with speculation ON must be token-identical to
///    greedy output with it OFF;
/// 2. rollback: after replaying the produced tokens through a fresh
///    non-speculative session, the next-token logits must match the ones the
///    speculative session ended on. That is the GatedDeltaNet state-rollback
///    proof — a stale recurrent state would show up here even when the greedy
///    argmax happened to agree.
fn run_spec_gate(
    model: &LlamaModel,
    args: &Args,
    prompt_token_ids: Vec<i32>,
) -> Result<(), Box<dyn std::error::Error>> {
    if args.spec_draft_max == 0 {
        return Err("--spec-gate requires --spec-draft-max N (N >= 1)".into());
    }
    let max_context = args
        .max_context
        .unwrap_or(prompt_token_ids.len() + args.max_new_tokens + 16)
        .max(256);

    let run = |spec_draft_max: usize| -> Result<(Vec<i32>, String, Vec<f32>, Option<makepad_ai_llm::SpeculativeStats>, f64), Box<dyn std::error::Error>> {
        let mut session = LlamaSession::from_model(
            model,
            LlamaSessionConfig {
                max_context: Some(u32::try_from(max_context)?),
                prefill_batch_size: args.prefill_batch_size,
                spec_draft_max,
                ..LlamaSessionConfig::default()
            },
        )?;
        session.append_tokens(&prompt_token_ids)?;
        let start = Instant::now();
        let generation = session.continue_greedy(args.max_new_tokens)?;
        let elapsed = start.elapsed().as_secs_f64();
        let tok_s = if elapsed > 0.0 {
            generation.token_ids.len() as f64 / elapsed
        } else {
            0.0
        };
        let logits = session
            .last_logits()
            .ok_or("session produced no logits")?
            .to_vec();
        Ok((
            generation.token_ids,
            generation.text,
            logits,
            session.speculative_stats(),
            tok_s,
        ))
    };

    let (base_tokens, base_text, base_logits, _, base_tok_s) = run(0)?;
    let (spec_tokens, spec_text, spec_logits, stats, spec_tok_s) = run(args.spec_draft_max)?;

    println!("spec_gate.prompt_tokens: {}", prompt_token_ids.len());
    println!("spec_gate.draft_max: {}", args.spec_draft_max);
    println!("spec_gate.tokens.base: {}", base_tokens.len());
    println!("spec_gate.tokens.spec: {}", spec_tokens.len());
    println!("spec_gate.tok_s.base: {:.2}", base_tok_s);
    println!("spec_gate.tok_s.spec: {:.2}", spec_tok_s);
    println!("spec_gate.speedup: {:.3}x", spec_tok_s / base_tok_s.max(1e-9));
    if let Some(stats) = stats {
        println!(
            "spec_gate.acceptance: {:.4} ({}/{} over {} rounds, {:.3} tokens/round)",
            stats.acceptance(),
            stats.accepted,
            stats.drafted,
            stats.rounds,
            stats.tokens_per_round()
        );
        println!(
            "spec_gate.seconds: draft={:.3} verify={:.3} catchup={:.3}",
            stats.draft_nanos as f64 / 1e9,
            stats.verify_nanos as f64 / 1e9,
            stats.catchup_nanos as f64 / 1e9
        );
    }

    let identical = base_tokens == spec_tokens;
    println!("spec_gate.lossless: {}", identical);
    if !identical {
        let first_diff = base_tokens
            .iter()
            .zip(spec_tokens.iter())
            .position(|(a, b)| a != b);
        println!("spec_gate.first_divergence: {:?}", first_diff);
        println!("spec_gate.text.base: {:?}", base_text);
        println!("spec_gate.text.spec: {:?}", spec_text);
    }

    // Rollback proof: the speculative session's final logits versus the same
    // prefix replayed without speculation.
    let max_abs_delta = if base_logits.len() == spec_logits.len() {
        base_logits
            .iter()
            .zip(spec_logits.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max)
    } else {
        f32::INFINITY
    };
    println!("spec_gate.logits_max_abs_delta: {:.6e}", max_abs_delta);

    if !identical {
        return Err("speculative greedy output diverged from the non-speculative run".into());
    }
    Ok(())
}

fn run_bench(
    model: &LlamaModel,
    vocab: &LlamaVocab,
    args: &Args,
    prompt_token_ids: Vec<i32>,
) -> Result<(), Box<dyn std::error::Error>> {
    let pp = args.bench_pp.unwrap_or(128);
    let tg = args.bench_tg.unwrap_or(32);
    if pp == 0 {
        return Err("--bench-pp must be > 0".into());
    }
    if tg == 0 {
        return Err("--bench-tg must be > 0".into());
    }
    let pad = prompt_token_ids[0];
    let mut tokens = prompt_token_ids;
    if tokens.len() < pp {
        tokens.resize(pp, pad);
    } else {
        tokens.truncate(pp);
    }
    // Official llama.cpp pads n_kv to max(n_pad, 256) so flash-ext never
    // takes the unaligned pad path (llama-kv-cache.cpp get_n_kv). A bench
    // context of just pp+tg (e.g. 21) compiled n_kv=21, hit flash-ext pad
    // at n_q>=20, and page-faulted on M4.
    const OFFICIAL_N_KV_PAD: usize = 256;
    let max_context = args
        .max_context
        .unwrap_or(pp + tg)
        .max(OFFICIAL_N_KV_PAD);
    let prefill_batch_size = if args.prefill_batch_size == 1 {
        pp
    } else {
        args.prefill_batch_size
    };
    let config = LlamaSessionConfig {
        max_context: Some(u32::try_from(max_context)?),
        prefill_batch_size,
        spec_draft_max: args.spec_draft_max,
        ..LlamaSessionConfig::default()
    };

    // Same session for warmup + timed run so Metal pipelines/graphs stay
    // compiled. Dropping the session (old bench) recompiled on the clock.
    eprintln!("bench.start: pp={pp} tg={tg} batch={prefill_batch_size}");
    let mut session = LlamaSession::from_model(model, config)?;
    eprintln!("bench.warmup: append {pp} + 1 decode");
    session.append_tokens(&tokens)?;
    let _ = session.continue_greedy(1)?;
    session.reset()?;
    eprintln!("bench.timed: prefill");

    let prefill_start = Instant::now();
    session.append_tokens(&tokens)?;
    let prefill_elapsed = prefill_start.elapsed();
    eprintln!("bench.timed: decode {tg}");
    let generation_start = Instant::now();
    let generation = session.continue_greedy(tg)?;
    let generation_elapsed = generation_start.elapsed();

    eprintln!("bench.protocol: llama-bench -p {pp} -n {tg}");
    eprintln!("bench.prefill_batch_size: {prefill_batch_size}");
    eprintln!("bench.pp_tokens: {}", tokens.len());
    eprintln!("bench.tg_tokens: {}", generation.token_ids.len());
    eprintln!(
        "bench.pp{pp}: {:.2} tok/s ({:.3}s)",
        tok_per_second(tokens.len(), prefill_elapsed.as_secs_f64()),
        prefill_elapsed.as_secs_f64()
    );
    eprintln!(
        "bench.tg{tg}: {:.2} tok/s ({:.3}s)",
        tok_per_second(generation.token_ids.len(), generation_elapsed.as_secs_f64()),
        generation_elapsed.as_secs_f64()
    );
    let _ = vocab;
    Ok(())
}

fn print_usage() {
    eprintln!(
        "usage: llama-generate <model.gguf> [--max-new-tokens N] [--prefill-batch-size N] [--upstream-completion-bin PATH] [--no-bos] [--no-parse-special] [--dump-token-ids] [--no-stream] [--verify-upstream] [--prompt TEXT | prompt words ...]\n\
         draft vocabulary: [--build-draft-vocab --prompt-dir DIR [--runs N] [--draft-vocab-text FILE]... [--draft-vocab-coverage C]]\n\
         \x20                [--draft-vocab-eval FILE]...   measure the sidecar on held-out text"
    );
}

fn tokenize_prompt(
    vocab: &LlamaVocab,
    args: &Args,
) -> Result<Vec<i32>, Box<dyn std::error::Error>> {
    Ok(vocab.tokenize(&args.prompt, !args.no_bos, args.parse_special)?)
}

fn run_upstream_completion(
    args: &Args,
    prompt_token_count: usize,
) -> Result<String, Box<dyn std::error::Error>> {
    let max_context = args
        .max_new_tokens
        .checked_add(prompt_token_count)
        .ok_or("overflow computing upstream completion context")?;
    let mut command = Command::new(&args.upstream_completion_bin);
    command
        .arg("-m")
        .arg(&args.model_path)
        .arg("-p")
        .arg(&args.prompt)
        .arg("-n")
        .arg(args.max_new_tokens.to_string())
        .arg("-c")
        .arg(max_context.to_string())
        .arg("-no-cnv")
        .arg("--simple-io")
        .arg("--no-display-prompt")
        .arg("--no-warmup")
        .arg("--seed")
        .arg("0")
        .arg("--temp")
        .arg("0")
        .arg("--top-k")
        .arg("1")
        .arg("--top-p")
        .arg("1")
        .arg("--repeat-penalty")
        .arg("1")
        .arg("--presence-penalty")
        .arg("0")
        .arg("--frequency-penalty")
        .arg("0")
        .arg("--dry-multiplier")
        .arg("0")
        .arg("-fa")
        .arg("on")
        .arg("-ctk")
        .arg("f16")
        .arg("-ctv")
        .arg("f16");
    if args.no_bos {
        command
            .arg("--override-kv")
            .arg("tokenizer.ggml.add_bos_token=bool:false");
    }

    let output = command.output()?;
    ensure_success("llama-completion", &output)?;
    Ok(normalize_upstream_completion_stdout(&String::from_utf8(
        output.stdout,
    )?))
}

fn normalize_upstream_completion_stdout(stdout: &str) -> String {
    stdout.strip_suffix("\n\n").unwrap_or(stdout).to_owned()
}

fn ensure_success(name: &str, output: &Output) -> Result<(), Box<dyn std::error::Error>> {
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "{name} exited with {}.\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .into())
}

fn verify_exact_output(
    rust_output: &str,
    upstream_output: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if rust_output == upstream_output {
        return Ok(());
    }

    let diff = first_diff_byte(rust_output.as_bytes(), upstream_output.as_bytes());
    let diff_message = match diff {
        Some((index, rust_byte, upstream_byte)) => format!(
            "first difference at byte {}: rust={:?} upstream={:?}",
            index, rust_byte, upstream_byte
        ),
        None => format!(
            "output length mismatch: rust={} upstream={}",
            rust_output.len(),
            upstream_output.len()
        ),
    };
    Err(format!(
        "exact upstream verification failed: {diff_message}\nrust.preview: {:?}\nupstream.preview: {:?}",
        preview_text(rust_output, 160),
        preview_text(upstream_output, 160)
    )
    .into())
}

fn first_diff_byte(lhs: &[u8], rhs: &[u8]) -> Option<(usize, Option<u8>, Option<u8>)> {
    let common_len = lhs.len().min(rhs.len());
    for index in 0..common_len {
        if lhs[index] != rhs[index] {
            return Some((index, Some(lhs[index]), Some(rhs[index])));
        }
    }
    if lhs.len() == rhs.len() {
        None
    } else {
        Some((
            common_len,
            lhs.get(common_len).copied(),
            rhs.get(common_len).copied(),
        ))
    }
}

fn preview_text(text: &str, max_chars: usize) -> String {
    let mut preview = text.chars().take(max_chars).collect::<String>();
    if text.chars().count() > max_chars {
        preview.push_str("...");
    }
    preview
}

fn stop_reason_name(reason: LlamaStopReason) -> &'static str {
    match reason {
        LlamaStopReason::MaxNewTokens => "max_new_tokens",
        LlamaStopReason::EndOfSequence => "eos_token",
        LlamaStopReason::PaddingToken => "padding_token",
    }
}

fn tok_per_second(token_count: usize, seconds: f64) -> f64 {
    if seconds > 0.0 {
        token_count as f64 / seconds
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_upstream_completion_stdout;

    #[test]
    fn strips_cli_trailing_newlines_from_upstream_completion() {
        assert_eq!(normalize_upstream_completion_stdout("hello\n\n"), "hello");
    }

    #[test]
    fn leaves_non_terminated_output_unchanged() {
        assert_eq!(normalize_upstream_completion_stdout("hello"), "hello");
    }
}
