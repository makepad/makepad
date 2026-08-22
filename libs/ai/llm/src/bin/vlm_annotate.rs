// Batch VLM executor: many images, one resident model.
//
// This is the executor half of the asset-annotation pass (libs/asset/annotate).
// The pass runner prepares images and prompts and owns all policy; this binary
// owns nothing but inference. Keeping the split here is what makes the
// executor swappable: a CUDA/fleet executor only has to speak the same batch
// protocol, and the runner never changes.
//
// Protocol (deliberately line-oriented so any executor can implement it):
//   --jobs FILE      TSV, one job per line: <id>\t<image.ppm>[\t<context>]
//                    the optional third column is per-job text appended to the
//                    shared prompt (the pass uses it to name the asset)
//   --prompt-file F  the shared question asked about every image
//   --out FILE       TSV results, one per line: <id>\t<ok|err>\t<escaped text>
// Escaping in the text column: \\ -> \\\\, \n -> \\n, \r -> \\r, \t -> \\t.
// Results are flushed per line, so a killed run leaves a resumable prefix.
//
// usage: vlm-annotate <model.gguf> <mmproj.gguf> --jobs J --prompt-file P --out O
//                     [--max-new-tokens N] [--max-context N] [--limit N]

use makepad_ai_llm::{
    preprocess_rgb8, LlamaSession, LlamaSessionConfig, VisionConfig, VisionTower,
};
use std::io::{BufRead, Write};
use std::time::Instant;

fn read_ppm(path: &str) -> Result<(Vec<u8>, usize, usize), String> {
    let data = std::fs::read(path).map_err(|e| format!("read {path}: {e}"))?;
    let mut fields = Vec::new();
    let mut pos = 0usize;
    while fields.len() < 4 {
        while pos < data.len() && (data[pos] as char).is_ascii_whitespace() {
            pos += 1;
        }
        if pos >= data.len() {
            return Err(format!("{path}: truncated ppm header"));
        }
        if data[pos] == b'#' {
            while pos < data.len() && data[pos] != b'\n' {
                pos += 1;
            }
            continue;
        }
        let start = pos;
        while pos < data.len() && !(data[pos] as char).is_ascii_whitespace() {
            pos += 1;
        }
        fields.push(String::from_utf8_lossy(&data[start..pos]).into_owned());
    }
    pos += 1;
    if fields[0] != "P6" || fields[3] != "255" {
        return Err(format!("{path}: not binary P6/255 ppm"));
    }
    let w: usize = fields[1].parse().map_err(|_| format!("{path}: bad width"))?;
    let h: usize = fields[2].parse().map_err(|_| format!("{path}: bad height"))?;
    let need = w * h * 3;
    if data.len() < pos + need {
        return Err(format!("{path}: short pixel payload"));
    }
    Ok((data[pos..pos + need].to_vec(), w, h))
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

struct Args {
    model: String,
    mmproj: String,
    jobs: String,
    prompt_file: String,
    out: String,
    max_new_tokens: usize,
    max_context: u32,
    limit: usize,
}

fn parse_args() -> Args {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 3 {
        eprintln!(
            "usage: {} <model.gguf> <mmproj.gguf> --jobs J --prompt-file P --out O \
             [--max-new-tokens N] [--max-context N] [--limit N]",
            a[0]
        );
        std::process::exit(2);
    }
    let mut args = Args {
        model: a[1].clone(),
        mmproj: a[2].clone(),
        jobs: String::new(),
        prompt_file: String::new(),
        out: String::new(),
        max_new_tokens: 220,
        max_context: 4096,
        limit: usize::MAX,
    };
    let mut i = 3;
    while i < a.len() {
        let need = |i: usize| -> String {
            if i + 1 >= a.len() {
                eprintln!("missing value for {}", a[i]);
                std::process::exit(2);
            }
            a[i + 1].clone()
        };
        match a[i].as_str() {
            "--jobs" => args.jobs = need(i),
            "--prompt-file" => args.prompt_file = need(i),
            "--out" => args.out = need(i),
            "--max-new-tokens" => args.max_new_tokens = need(i).parse().expect("max-new-tokens"),
            "--max-context" => args.max_context = need(i).parse().expect("max-context"),
            "--limit" => args.limit = need(i).parse().expect("limit"),
            other => {
                eprintln!("unknown arg {other}");
                std::process::exit(2);
            }
        }
        i += 2;
    }
    for (name, v) in [("--jobs", &args.jobs), ("--prompt-file", &args.prompt_file), ("--out", &args.out)] {
        if v.is_empty() {
            eprintln!("{name} is required");
            std::process::exit(2);
        }
    }
    args
}

fn main() {
    let args = parse_args();

    let question = std::fs::read_to_string(&args.prompt_file).expect("read prompt file");
    let question = question.trim().to_string();

    let jobs_text = std::fs::read_to_string(&args.jobs).expect("read jobs file");
    let mut jobs: Vec<(String, String, String)> = Vec::new();
    for line in jobs_text.lines() {
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            continue;
        }
        let mut cols = line.splitn(3, '\t');
        let (Some(id), Some(path)) = (cols.next(), cols.next()) else {
            eprintln!("skipping malformed job line: {line}");
            continue;
        };
        jobs.push((id.to_string(), path.to_string(), cols.next().unwrap_or("").to_string()));
        if jobs.len() >= args.limit {
            break;
        }
    }
    eprintln!("vlm-annotate: {} jobs", jobs.len());

    // Vision tower first: its arena is sized from the largest grid we expect,
    // and the runner normalizes every sheet to one size so exactly one vision
    // graph gets compiled and reused.
    // Images are preprocessed lazily (a prepared 512-square sheet is ~6 MB, so
    // caching the whole batch would cost gigabytes). The arena only needs the
    // largest grid, which the first readable sheet establishes because the
    // runner normalizes every sheet to one size.
    let gguf = makepad_ai_llm::GgufFile::open(&args.mmproj).expect("open mmproj");
    let vision_config = VisionConfig::from_gguf(&gguf).expect("vision config");
    let prepare = |path: &str| -> Result<_, String> {
        let (rgb, w, h) = read_ppm(path)?;
        preprocess_rgb8(&rgb, w, h, &vision_config).map_err(|e| format!("{path}: {e:?}"))
    };
    let mut max_patches = 0usize;
    for (_, path, _) in &jobs {
        match prepare(path) {
            Ok(p) => {
                max_patches = p.n_patches();
                break;
            }
            Err(e) => eprintln!("probe preprocess failed: {e}"),
        }
    }
    if max_patches == 0 {
        eprintln!("no readable images");
        std::process::exit(1);
    }
    eprintln!("sheet grid gives {max_patches} patches ({} tokens)", max_patches / 4);

    let t_load = Instant::now();
    let mut tower = VisionTower::load(&args.mmproj, max_patches).expect("vision tower");
    let mut session = LlamaSession::load(
        &args.model,
        LlamaSessionConfig {
            max_context: Some(args.max_context),
            ..LlamaSessionConfig::default()
        },
    )
    .expect("load llama session");
    eprintln!("loaded model + tower in {:.2}s", t_load.elapsed().as_secs_f64());

    let mut out = std::io::BufWriter::new(std::fs::File::create(&args.out).expect("create out"));
    let prefix = "<|im_start|>user\n<|vision_start|>";

    let t_all = Instant::now();
    let mut done = 0usize;
    let mut failed = 0usize;
    for (i, (id, path, context)) in jobs.iter().enumerate() {
        let t0 = Instant::now();
        let prepared = match prepare(path) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("{id} preprocess failed: {e}");
                writeln!(out, "{}\terr\t{}", id, escape(&e)).ok();
                out.flush().ok();
                failed += 1;
                continue;
            }
        };
        let prepared = &prepared;
        let suffix = if context.is_empty() {
            format!(
                "<|vision_end|>{question}<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n"
            )
        } else {
            format!(
                "<|vision_end|>{question}\n\n{context}<|im_end|>\n\
                 <|im_start|>assistant\n<think>\n\n</think>\n\n"
            )
        };
        let result = (|| -> Result<String, String> {
            let embeddings = tower.encode(prepared).map_err(|e| format!("encode: {e:?}"))?;
            // One asset must never see another's context: clear KV + recurrent
            // state between images. This is state-clear only, not a reload.
            session.reset().map_err(|e| format!("reset: {e:?}"))?;
            let prefix_ids = session
                .vocab()
                .tokenize(prefix, false, true)
                .map_err(|e| format!("tokenize prefix: {e:?}"))?;
            session.append_tokens(&prefix_ids).map_err(|e| format!("prefill prefix: {e:?}"))?;
            session
                .append_image_embeddings(&embeddings, prepared.tokens_w(), prepared.tokens_h())
                .map_err(|e| format!("prefill image: {e:?}"))?;
            let suffix_ids = session
                .vocab()
                .tokenize(&suffix, false, true)
                .map_err(|e| format!("tokenize suffix: {e:?}"))?;
            session.append_tokens(&suffix_ids).map_err(|e| format!("prefill suffix: {e:?}"))?;

            let mut text = String::new();
            let mut decoder = session.vocab().text_decoder();
            let mut generated = 0usize;
            while generated < args.max_new_tokens {
                let Some(token) =
                    session.next_greedy_token().map_err(|e| format!("generate: {e:?}"))?
                else {
                    break;
                };
                generated += 1;
                if let Some(chunk) = decoder.push_token(session.vocab(), token) {
                    text.push_str(&chunk);
                }
            }
            Ok(text)
        })();

        match result {
            Ok(text) => {
                writeln!(out, "{}\tok\t{}", id, escape(text.trim())).ok();
                done += 1;
            }
            Err(e) => {
                eprintln!("{id} ({path}) failed: {e}");
                writeln!(out, "{}\terr\t{}", id, escape(&e)).ok();
                failed += 1;
            }
        }
        out.flush().ok();
        if (i + 1) % 10 == 0 || i + 1 == jobs.len() {
            let per = t_all.elapsed().as_secs_f64() / (i + 1) as f64;
            eprintln!(
                "progress {}/{} ({} ok, {} err) last {:.2}s avg {:.2}s eta {:.1}min",
                i + 1,
                jobs.len(),
                done,
                failed,
                t0.elapsed().as_secs_f64(),
                per,
                per * (jobs.len() - i - 1) as f64 / 60.0
            );
        }
    }
    out.flush().ok();
    eprintln!(
        "vlm-annotate done: {done} ok, {failed} err in {:.1}s",
        t_all.elapsed().as_secs_f64()
    );
    if done == 0 {
        std::process::exit(1);
    }
}
