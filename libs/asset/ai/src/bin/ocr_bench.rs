//! OCR bench — every page in a directory through the resident `ocr` backend,
//! one HTML per page, and the numbers that pick the model and size the box.
//!
//! ```text
//! ocr-bench run --model <llm.gguf> --mmproj <mmproj.gguf> --pages <dir> --out <dir>
//!               [--limit N] [--resume] [--prompt text|layout|<custom>] [--prompt-file <path>]
//!               [--max-tokens N] [--retries N]
//! ocr-bench score --pages <dir> --candidates <outdir> [<outdir>...] [--loose] [--texts <texts.tsv>]
//! ocr-bench vision-parity --mmproj <mmproj.gguf> --pages <dir> [--limit N] [--tolerance R]
//!                         [--keep-reference VAR,VAR]
//! ```
//!
//! `run` walks `<dir>` for `.png` / `.jpg` / `.mov` pages (the layout the
//! source-library exporter writes: `pages/<class>/<book>_p<page>.png` with a
//! `.ocr.txt` reference beside it), sorts them by fitted size so consecutive
//! pages share a compiled tower graph, transcribes each through
//! `OcrBackend::ocr_page` — the exact request path minus the wire — writes
//! `<out>/<class>/<stem>.html`, appends a row to `<out>/results.tsv`
//! (sizes, image/output tokens, attempts, looped, encode/prefill/decode
//! seconds) and ends with the aggregate: pages, wall seconds per page,
//! pages per second, decode tokens per second. `--resume` skips pages whose
//! HTML already exists, so a long batch can be run in bounded slices.
//!
//! `score` ranks transcriptions: for every page that has a `.ocr.txt`
//! reference (the corpus' own OCR — a machine transcription too, so this is
//! DISTANCE to it, not error against truth) and every candidate directory
//! holding `<class>/<stem>.html`, it reports character error rate, word
//! error rate and length ratio after normalisation (tags stripped, entities
//! decoded, whitespace collapsed; `--loose` also folds case, u/v, i/j, long
//! s and joins line-end hyphenation), per class and overall, plus every
//! candidate pair's mutual distance. Two candidates that agree with each
//! other but not with the reference are the interesting case.
//!
//! `--texts <tsv>` (book, page, ocr rows for whole books) re-aligns the
//! reference per page: the site's page texts run off by one around plates
//! and inserted leaves, so for each page the neighbouring texts (±3) are
//! tried and the one closest to the first candidate is used for every
//! candidate; pages that needed an offset are reported.
//!
//! `vision-parity` is the gate for a vision-tower kernel change: it encodes
//! one page per distinct patch grid twice in the same process — once with
//! every fast CUDA kernel disabled, once with them on — and reports the
//! relative RMS, largest absolute difference and cosine between the two sets
//! of embeddings. No LLM is loaded; it measures the tower alone.
//! `--keep-reference` leaves named kill switches set in the fast arm, which
//! turns a total into an attribution: one kernel at a time carries the blame.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("run") => run(&args[1..]),
        Some("score") => score(&args[1..]),
        Some("vision-parity") => vision_parity(&args[1..]),
        _ => {
            eprintln!(
                "usage:\n  ocr-bench run --model <llm.gguf> --mmproj <mmproj.gguf> --pages <dir> --out <dir> [--limit N] [--resume] [--prompt text|layout|<custom>] [--max-tokens N] [--retries N]\n  ocr-bench score --pages <dir> --candidates <outdir> [<outdir>...] [--loose] [--texts <texts.tsv>]\n  ocr-bench vision-parity --mmproj <mmproj.gguf> --pages <dir> [--limit N] [--tolerance R] [--keep-reference VAR,VAR]"
            );
            2
        }
    };
    std::process::exit(code);
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

fn has_flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

/// Every page image under `dir`, with its class (the parent directory
/// name) and its stem, in path order.
fn collect_pages(dir: &Path) -> Vec<(PathBuf, String, String)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        let mut entries: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "mov" | "mp4") {
                continue;
            }
            let class = p
                .parent()
                .and_then(|c| c.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
            out.push((p, class, stem));
        }
    }
    out.sort();
    out
}

fn run(args: &[String]) -> i32 {
    let Some(model) = flag(args, "--model") else {
        eprintln!("run: --model <llm.gguf> is required");
        return 2;
    };
    let Some(mmproj) = flag(args, "--mmproj") else {
        eprintln!("run: --mmproj <mmproj.gguf> is required");
        return 2;
    };
    let pages_dir = PathBuf::from(flag(args, "--pages").unwrap_or("local/ocr-bench/pages"));
    let out_dir = PathBuf::from(flag(args, "--out").unwrap_or("local/ocr-bench/out"));
    let limit: usize = flag(args, "--limit").and_then(|v| v.parse().ok()).unwrap_or(usize::MAX);
    let prompt_text = match flag(args, "--prompt-file") {
        Some(path) => match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) => {
                eprintln!("run: cannot read --prompt-file {path}: {e}");
                return 2;
            }
        },
        None => flag(args, "--prompt").map(|p| if p == "text" { "" } else { p }).unwrap_or("").to_string(),
    };
    let prompt = makepad_asset_ai::ocr_backend::OcrPrompt::from_wire(&prompt_text);
    let max_new_tokens: u32 = flag(args, "--max-tokens")
        .and_then(|v| v.parse().ok())
        .unwrap_or(makepad_asset_ai::ocr_backend::DEFAULT_NEW_TOKENS);
    let retries: u32 = flag(args, "--retries")
        .and_then(|v| v.parse().ok())
        .unwrap_or(makepad_asset_ai::ocr_backend::DEFAULT_RETRIES);
    let resume = has_flag(args, "--resume");

    use makepad_asset_ai::backend::CancelToken;
    use makepad_asset_ai::ocr_backend::{page_fit, OcrBackend, OcrRequest, MAX_INPUT_PIXELS};
    use makepad_asset_ai::vision_backend::decode_image_rgb8_within;

    let mut pages = collect_pages(&pages_dir);
    if pages.is_empty() {
        eprintln!("run: no page images under {}", pages_dir.display());
        return 1;
    }
    // Decode dims first (cheap: headers) so the batch can be ordered by the
    // fitted size, which is what decides the tower graph.
    let mut decoded: Vec<((usize, usize), PathBuf, String, String)> = Vec::new();
    for (path, class, stem) in pages.drain(..) {
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("skip {}: {e}", path.display());
                continue;
            }
        };
        match decode_image_rgb8_within(&bytes, MAX_INPUT_PIXELS) {
            Ok((_, w, h)) => decoded.push((page_fit(w, h, 32), path, class, stem)),
            Err(e) => eprintln!("skip {}: {e}", path.display()),
        }
    }
    decoded.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    if resume {
        let before = decoded.len();
        decoded.retain(|(_, _, class, stem)| !out_dir.join(class).join(format!("{stem}.html")).is_file());
        eprintln!("[bench] resume: {} of {before} pages already transcribed", before - decoded.len());
    }
    decoded.truncate(limit);
    let total = decoded.len();
    eprintln!("[bench] {total} pages under {}", pages_dir.display());

    let mut backend = OcrBackend::new("ocr-bench");
    let t_load = Instant::now();
    if let Err(e) = backend.load_from_paths(
        PathBuf::from(model),
        PathBuf::from(mmproj),
        &mut |stage, frac| eprintln!("[bench] load {stage} {:.0}%", frac * 100.0),
    ) {
        eprintln!("run: load failed: {e}");
        return 1;
    }
    eprintln!("[bench] loaded in {:.1}s", t_load.elapsed().as_secs_f64());

    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        eprintln!("run: cannot create {}: {e}", out_dir.display());
        return 1;
    }
    let results_path = out_dir.join("results.tsv");
    let header = "file\tclass\twidth\theight\tfed_width\tfed_height\timage_tokens\toutput_tokens\tattempts\tlooped\tencode_s\tprefill_s\tdecode_s\ttotal_s\n";
    // A resumed run appends to the earlier rows instead of restarting them.
    let mut rows = match (resume, std::fs::read_to_string(&results_path)) {
        (true, Ok(existing)) if existing.starts_with(header) => existing,
        _ => String::from(header),
    };
    let t_all = Instant::now();
    let mut done = 0usize;
    let mut sum_total = 0.0f64;
    let mut sum_decode = 0.0f64;
    let mut sum_out_tokens = 0usize;
    let mut sum_image_tokens = 0usize;
    let mut looped_pages = 0usize;
    let mut retried_pages = 0usize;
    for (index, (_, path, class, stem)) in decoded.iter().enumerate() {
        let bytes = std::fs::read(path).expect("read page");
        let (rgb, w, h) = decode_image_rgb8_within(&bytes, MAX_INPUT_PIXELS).expect("decode page");
        let t_page = Instant::now();
        let page = match backend.ocr_page(
            OcrRequest {
                prompt: prompt.clone(),
                rgb,
                width: w,
                height: h,
                max_new_tokens,
                retries,
            },
            &CancelToken::new(),
            &mut |_, _| {},
            &mut |_| {},
        ) {
            Ok(page) => page,
            Err(e) => {
                eprintln!("[bench] {}/{total} {} FAILED: {e}", index + 1, path.display());
                continue;
            }
        };
        let total_s = t_page.elapsed().as_secs_f64();
        let class_dir = out_dir.join(class);
        let _ = std::fs::create_dir_all(&class_dir);
        let html_path = class_dir.join(format!("{stem}.html"));
        if let Err(e) = std::fs::write(&html_path, &page.html) {
            eprintln!("[bench] cannot write {}: {e}", html_path.display());
        }
        rows.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\n",
            path.strip_prefix(&pages_dir).unwrap_or(path).display(),
            class,
            w,
            h,
            page.fed_width,
            page.fed_height,
            page.image_tokens,
            page.output_tokens,
            page.attempts,
            page.looped,
            page.encode_s,
            page.prefill_s,
            page.decode_s,
            total_s
        ));
        let _ = std::fs::write(&results_path, &rows);
        done += 1;
        sum_total += total_s;
        sum_decode += page.decode_s;
        sum_out_tokens += page.output_tokens;
        sum_image_tokens += page.image_tokens;
        if page.looped {
            looped_pages += 1;
        }
        if page.attempts > 1 {
            retried_pages += 1;
        }
        eprintln!(
            "[bench] {}/{total} {} {}x{} -> {} img tok, {} out tok, {} att{}, enc {:.2}s pre {:.2}s dec {:.2}s = {:.1}s ({:.1} tok/s)",
            index + 1,
            stem,
            w,
            h,
            page.image_tokens,
            page.output_tokens,
            page.attempts,
            if page.looped { " LOOPED" } else { "" },
            page.encode_s,
            page.prefill_s,
            page.decode_s,
            total_s,
            page.output_tokens as f64 / page.decode_s.max(1e-6)
        );
    }
    let wall = t_all.elapsed().as_secs_f64();
    if done == 0 {
        eprintln!("[bench] no page transcribed");
        return 1;
    }
    println!(
        "pages {done}  wall {wall:.1}s  {:.2} s/page  {:.3} pages/s  decode {:.1} tok/s  avg {:.0} image tok  avg {:.0} out tok  retried {retried_pages}  still looped {looped_pages}",
        wall / done as f64,
        done as f64 / wall,
        sum_out_tokens as f64 / sum_decode.max(1e-6),
        sum_image_tokens as f64 / done as f64,
        sum_out_tokens as f64 / done as f64,
    );
    let _ = sum_total;
    println!("results: {}", results_path.display());
    0
}

// ------------------------------------------------------------ tower parity

/// The kernel kill switches that put the CUDA vision tower back on the
/// arithmetic every fast path replaced: the dot-product-per-element f16
/// matmul instead of cuBLAS, and the stems-shaped attention kernel instead
/// of the register-tiled one. Setting all of them is the reference; clearing
/// them is what ships.
const REFERENCE_KERNEL_ENV: &[&str] =
    &["MKLLM_DISABLE_GEMM_F16", "MKLLM_DISABLE_ROFORMER_TILED"];

/// Sets every reference switch, or clears all but `keep`.
///
/// `keep` is what turns one number into an attribution: leaving a switch set
/// in the fast arm measures the tower with every kernel but that one
/// replaced, so a failing total can be charged to the kernel that actually
/// moved it rather than to the change as a whole.
fn reference_kernels(on: bool, keep: &[String]) {
    for var in REFERENCE_KERNEL_ENV {
        if on || keep.iter().any(|k| k == var) {
            std::env::set_var(var, "1");
        } else {
            std::env::remove_var(var);
        }
    }
}

/// Encodes each page twice in one process — once with the reference kernels,
/// once with the fast ones — and reports how far apart the two towers land.
///
/// This is the gate a vision-tower kernel change has to pass. Measuring it
/// inside one binary, on the real corpus, at the real patch grids, is the
/// point: a remembered number from another build cannot tell you whether the
/// arithmetic you just replaced still agrees with the arithmetic it replaced,
/// and a synthetic image cannot tell you whether a real page's activations
/// stay inside the range a narrower dtype can hold.
///
/// Two towers rather than one because the kernel choice is made when a graph
/// is planned, and each tower caches its plan per patch grid; the switches
/// are set immediately before every encode so a cache miss re-plans into the
/// configuration that encode is supposed to be measuring.
fn vision_parity(args: &[String]) -> i32 {
    use makepad_ai_llm::{preprocess_rgb8, GgufFile, VisionConfig, VisionTower};
    use makepad_asset_ai::ocr_backend::{page_fit, resample_rgb8, MAX_INPUT_PIXELS};
    use makepad_asset_ai::vision_backend::decode_image_rgb8_within;

    let Some(mmproj) = flag(args, "--mmproj") else {
        eprintln!("vision-parity: --mmproj <mmproj.gguf> is required");
        return 2;
    };
    let pages_dir = PathBuf::from(flag(args, "--pages").unwrap_or("local/ocr-bench/pages"));
    let limit: usize = flag(args, "--limit").and_then(|v| v.parse().ok()).unwrap_or(usize::MAX);
    let tolerance: f64 = flag(args, "--tolerance").and_then(|v| v.parse().ok()).unwrap_or(2e-3);
    // Kill switches to leave set in the fast arm, so one kernel at a time can
    // be charged for the difference it makes.
    let keep: Vec<String> = flag(args, "--keep-reference")
        .map(|list| list.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    for var in &keep {
        if !REFERENCE_KERNEL_ENV.contains(&var.as_str()) {
            eprintln!("vision-parity: --keep-reference {var} is not one of {REFERENCE_KERNEL_ENV:?}");
            return 2;
        }
    }
    if !keep.is_empty() {
        println!("keeping reference kernels: {}", keep.join(","));
    }

    let file = match GgufFile::open(mmproj) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("vision-parity: open mmproj {mmproj}: {e:?}");
            return 1;
        }
    };
    let config = match VisionConfig::from_gguf(&file) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("vision-parity: mmproj vision config: {e:?}");
            return 1;
        }
    };
    drop(file);

    let mut pages = collect_pages(&pages_dir);
    if pages.is_empty() {
        eprintln!("vision-parity: no page images under {}", pages_dir.display());
        return 1;
    }
    // Largest pages first: the shapes a kernel change is most likely to move
    // are the ones a short run should not skip.
    let mut decoded: Vec<((usize, usize), PathBuf, String)> = Vec::new();
    for (path, _, stem) in pages.drain(..) {
        let Ok(bytes) = std::fs::read(&path) else { continue };
        if let Ok((_, w, h)) = decode_image_rgb8_within(&bytes, MAX_INPUT_PIXELS) {
            decoded.push((page_fit(w, h, 32), path, stem));
        }
    }
    decoded.sort_by(|a, b| (b.0).cmp(&a.0).then(a.1.cmp(&b.1)));
    // One page per distinct fitted size: two encodes of the same grid drive the
    // same graph, so extra pages of a size already covered buy nothing.
    let mut seen = BTreeMap::new();
    decoded.retain(|(fit, _, _)| seen.insert(*fit, ()).is_none());
    decoded.truncate(limit);

    let mut towers = Vec::new();
    for reference in [true, false] {
        reference_kernels(reference, &keep);
        match VisionTower::load(mmproj) {
            Ok(t) => towers.push(t),
            Err(e) => {
                eprintln!("vision-parity: load vision tower: {e:?}");
                return 1;
            }
        }
    }
    let (fast_tower, ref_tower) = {
        let mut it = towers.drain(..);
        let r = it.next().unwrap();
        (it.next().unwrap(), r)
    };
    let mut ref_tower = ref_tower;
    let mut fast_tower = fast_tower;

    println!(
        "{:<44} {:>7} {:>9} {:>11} {:>11} {:>12}",
        "page", "tokens", "grid", "rel_rms", "max_abs", "cosine"
    );
    let mut worst = 0f64;
    let mut shapes = 0usize;
    for (fit, path, stem) in &decoded {
        let Ok(bytes) = std::fs::read(path) else { continue };
        let Ok((rgb, w, h)) = decode_image_rgb8_within(&bytes, MAX_INPUT_PIXELS) else { continue };
        let (fw, fh) = *fit;
        let fitted = resample_rgb8(&rgb, w, h, fw, fh);
        let mut fitted_config = config.clone();
        fitted_config.min_pixels = 0;
        fitted_config.max_pixels = usize::MAX / 4;
        let prepared = match preprocess_rgb8(&fitted, fw, fh, &fitted_config) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("vision-parity: preprocess {}: {e:?}", path.display());
                continue;
            }
        };
        drop(fitted);

        reference_kernels(true, &keep);
        let reference = match ref_tower.encode(&prepared) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("vision-parity: reference encode {}: {e:?}", path.display());
                return 1;
            }
        };
        reference_kernels(false, &keep);
        let fast = match fast_tower.encode(&prepared) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("vision-parity: fast encode {}: {e:?}", path.display());
                return 1;
            }
        };
        if reference.len() != fast.len() {
            eprintln!(
                "vision-parity: {} length mismatch {} vs {}",
                path.display(),
                reference.len(),
                fast.len()
            );
            return 1;
        }

        let mut sq_diff = 0f64;
        let mut sq_ref = 0f64;
        let mut max_abs = 0f64;
        let mut dot = 0f64;
        let mut sq_fast = 0f64;
        for (&r, &f) in reference.iter().zip(&fast) {
            let (r, f) = (r as f64, f as f64);
            let d = r - f;
            sq_diff += d * d;
            sq_ref += r * r;
            sq_fast += f * f;
            dot += r * f;
            max_abs = max_abs.max(d.abs());
        }
        let rel_rms = (sq_diff / sq_ref.max(f64::MIN_POSITIVE)).sqrt();
        let cosine = dot / (sq_ref.sqrt() * sq_fast.sqrt()).max(f64::MIN_POSITIVE);
        worst = worst.max(rel_rms);
        shapes += 1;
        println!(
            "{:<44} {:>7} {:>9} {:>11.3e} {:>11.3e} {:>12.8}",
            stem.chars().take(44).collect::<String>(),
            prepared.n_tokens(),
            format!("{}x{}", prepared.grid_w, prepared.grid_h),
            rel_rms,
            max_abs,
            cosine
        );
    }
    if shapes == 0 {
        eprintln!("vision-parity: no page encoded");
        return 1;
    }
    println!("shapes {shapes}  worst rel_rms {worst:.3e}  tolerance {tolerance:.3e}");
    if worst > tolerance {
        println!("PARITY-FAIL");
        return 1;
    }
    println!("PARITY-OK");
    0
}

// ---------------------------------------------------------------- scoring

/// Drops Chandra's picture blocks — `<div ... data-label="Image">` (and
/// Figure/Diagram) with the description it writes inside — since a caption
/// of an engraving is not page text and no reference has one.
fn strip_picture_blocks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<div") {
        let Some(tag_end) = rest[start..].find('>') else { break };
        let tag = &rest[start..start + tag_end + 1];
        let is_picture = ["Image", "Figure", "Diagram"]
            .iter()
            .any(|label| tag.contains(&format!("data-label=\"{label}\"")));
        if !is_picture {
            out.push_str(&rest[..start + tag_end + 1]);
            rest = &rest[start + tag_end + 1..];
            continue;
        }
        out.push_str(&rest[..start]);
        match rest[start..].find("</div>") {
            Some(close) => rest = &rest[start + close + "</div>".len()..],
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Text normalisation shared by reference and candidates.
fn normalise(text: &str, loose: bool) -> String {
    let text = strip_picture_blocks(text);
    let text = text.as_str();
    // Strip tags: both the site's own (<header>, <margin>, <column-break/>)
    // and Chandra's HTML. A <br> and a block close become a newline so
    // line structure survives into the hyphenation step.
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '<' {
            let mut tag = String::new();
            for t in chars.by_ref() {
                if t == '>' {
                    break;
                }
                tag.push(t);
            }
            let name = tag.trim_start_matches('/').split_whitespace().next().unwrap_or("").to_ascii_lowercase();
            if matches!(name.as_str(), "br" | "p" | "div" | "tr" | "li" | "h1" | "h2" | "h3" | "h4" | "h5" | "header" | "margin" | "page-num" | "column-break" | "sig" | "table" | "pre") {
                out.push('\n');
            } else {
                out.push(' ');
            }
        } else {
            out.push(c);
        }
    }
    let out = out
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ");
    let mut s = out;
    if loose {
        // Join line-end hyphenation in its early-modern spellings.
        for hyphen in ["-\n", "=\n", "¬\n", "‐\n", "\u{2010}\n"] {
            s = s.replace(hyphen, "");
        }
        s = s
            .to_lowercase()
            .replace('ſ', "s")
            .replace('v', "u")
            .replace('j', "i")
            .replace('æ', "ae")
            .replace('œ', "oe")
            .replace('ß', "ss");
        s = s
            .chars()
            .map(|c| if c.is_alphanumeric() || c.is_whitespace() { c } else { ' ' })
            .collect();
    }
    // Collapse whitespace.
    let mut collapsed = String::with_capacity(s.len());
    let mut in_space = true;
    for c in s.chars() {
        if c.is_whitespace() {
            if !in_space {
                collapsed.push(' ');
                in_space = true;
            }
        } else {
            collapsed.push(c);
            in_space = false;
        }
    }
    collapsed.trim().to_string()
}

/// Levenshtein distance over any comparable items (chars or words).
fn levenshtein<T: PartialEq>(a: &[T], b: &[T]) -> usize {
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

#[derive(Clone, Copy, Debug, Default)]
struct Distance {
    cer: f64,
    wer: f64,
    len_ratio: f64,
}

fn distance(reference: &str, candidate: &str) -> Distance {
    let rc: Vec<char> = reference.chars().collect();
    let cc: Vec<char> = candidate.chars().collect();
    let rw: Vec<&str> = reference.split(' ').filter(|w| !w.is_empty()).collect();
    let cw: Vec<&str> = candidate.split(' ').filter(|w| !w.is_empty()).collect();
    Distance {
        cer: levenshtein(&rc, &cc) as f64 / rc.len().max(1) as f64,
        wer: levenshtein(&rw, &cw) as f64 / rw.len().max(1) as f64,
        len_ratio: cc.len() as f64 / rc.len().max(1) as f64,
    }
}

#[derive(Default)]
struct Agg {
    n: usize,
    cer: f64,
    wer: f64,
    len: f64,
}

impl Agg {
    fn add(&mut self, d: Distance) {
        self.n += 1;
        self.cer += d.cer;
        self.wer += d.wer;
        self.len += d.len_ratio;
    }
    fn line(&self) -> String {
        if self.n == 0 {
            return "n=0".to_string();
        }
        let n = self.n as f64;
        format!(
            "n={:<3} CER {:6.1}%  WER {:6.1}%  len {:5.2}",
            self.n,
            100.0 * self.cer / n,
            100.0 * self.wer / n,
            self.len / n
        )
    }
}

fn score(args: &[String]) -> i32 {
    let pages_dir = PathBuf::from(flag(args, "--pages").unwrap_or("local/ocr-bench/pages"));
    let loose = has_flag(args, "--loose");
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(i) = args.iter().position(|a| a == "--candidates") {
        for a in &args[i + 1..] {
            if a.starts_with("--") {
                break;
            }
            candidates.push(PathBuf::from(a));
        }
    }
    if candidates.is_empty() {
        eprintln!("score: --candidates <outdir> [<outdir>...] is required");
        return 2;
    }
    let names: Vec<String> = candidates
        .iter()
        .map(|c| c.file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string())
        .collect();

    let pages = collect_pages(&pages_dir);
    // (class, stem) -> reference text
    let mut refs: BTreeMap<(String, String), String> = BTreeMap::new();
    for (path, class, stem) in &pages {
        let ref_path = path.with_file_name(format!("{stem}.ocr.txt"));
        if let Ok(text) = std::fs::read_to_string(&ref_path) {
            refs.insert((class.clone(), stem.clone()), normalise(&text, loose));
        }
    }
    // Whole-book texts for re-alignment: (book, page) -> normalised text.
    let mut book_texts: BTreeMap<(String, i64), String> = BTreeMap::new();
    if let Some(tsv) = flag(args, "--texts") {
        match std::fs::read_to_string(tsv) {
            Ok(text) => {
                for line in text.lines().skip(1) {
                    let mut parts = line.splitn(3, '\t');
                    let (Some(book), Some(page), Some(ocr)) = (parts.next(), parts.next(), parts.next()) else { continue };
                    if let Ok(page) = page.parse::<i64>() {
                        book_texts.insert((book.to_string(), page), normalise(&ocr.replace("\\n", "\n"), loose));
                    }
                }
                println!("loaded {} book page texts from {tsv} for re-alignment", book_texts.len());
            }
            Err(e) => {
                eprintln!("score: cannot read --texts {tsv}: {e}");
                return 1;
            }
        }
    }
    let mut realigned: Vec<String> = Vec::new();
    // A corpus without exported `.ocr.txt` references can still answer the
    // question two runs of the same model ask of each other: how far apart are
    // they? That is the gate a kernel change has to pass — the reference is
    // the other candidate, not the site's transcription — so fall through to
    // the pairwise section instead of refusing.
    let compare_only = refs.is_empty();
    if compare_only {
        if candidates.len() < 2 {
            eprintln!(
                "score: no .ocr.txt references beside the pages in {} and only one candidate — \
                 nothing to compare against",
                pages_dir.display()
            );
            return 1;
        }
        for (_, class, stem) in &pages {
            if candidates
                .iter()
                .any(|c| c.join(class).join(format!("{stem}.html")).is_file())
            {
                refs.insert((class.clone(), stem.clone()), String::new());
            }
        }
        if refs.is_empty() {
            eprintln!("score: no candidate holds an HTML for any page under {}", pages_dir.display());
            return 1;
        }
        println!(
            "no .ocr.txt references under {}: comparing {} candidates to each other over {} pages, \
             {} normalisation",
            pages_dir.display(),
            candidates.len(),
            refs.len(),
            if loose { "loose" } else { "strict" }
        );
    } else {
        println!(
            "scoring {} pages with references, {} candidate(s), {} normalisation",
            refs.len(),
            candidates.len(),
            if loose { "loose" } else { "strict" }
        );
        println!("(distance to the corpus' own OCR — a machine reference — lower is closer, not necessarily truer)");
    }

    // per candidate: per class + overall; and the per-page rows.
    let mut per_class: Vec<BTreeMap<String, Agg>> =
        (0..candidates.len()).map(|_| BTreeMap::new()).collect();
    let mut overall: Vec<Agg> = (0..candidates.len()).map(|_| Agg::default()).collect();
    let mut pair: BTreeMap<(usize, usize), Agg> = BTreeMap::new();
    let mut missing: Vec<usize> = vec![0; candidates.len()];
    println!(
        "\nper page (CER% {}):",
        if compare_only { "against the first candidate" } else { "strict-or-loose as chosen" }
    );
    println!("{:<44} {:<14} {}", "page", "class", names.join("  "));
    for ((class, stem), reference) in &refs {
        let mut texts: Vec<Option<String>> = Vec::new();
        for (ci, cand) in candidates.iter().enumerate() {
            let p = cand.join(class).join(format!("{stem}.html"));
            match std::fs::read_to_string(&p) {
                Ok(t) => texts.push(Some(normalise(&t, loose))),
                Err(_) => {
                    missing[ci] += 1;
                    texts.push(None);
                }
            }
        }
        // Re-align: the closest of the site's neighbouring page texts to the
        // first candidate that exists stands in for the exported reference.
        let mut reference = reference.clone();
        if !book_texts.is_empty() {
            if let Some((book, page)) = stem.rsplit_once("_p").and_then(|(b, p)| p.parse::<i64>().ok().map(|p| (b.to_string(), p))) {
                if let Some(anchor) = texts.iter().flatten().next() {
                    let mut best: Option<(f64, i64)> = None;
                    for offset in -3i64..=3 {
                        if let Some(text) = book_texts.get(&(book.clone(), page + offset)) {
                            let d = distance(text, anchor).cer;
                            if best.map_or(true, |(bd, _)| d < bd) {
                                best = Some((d, offset));
                            }
                        }
                    }
                    if let Some((_, offset)) = best {
                        if offset != 0 {
                            realigned.push(format!("{stem} -> page {}", page + offset));
                            reference = book_texts[&(book.clone(), page + offset)].clone();
                        }
                    }
                }
            }
        }
        // Without a corpus reference the first candidate that has this page
        // stands in for one, so the per-page column still says how far each
        // run drifted from the run it is being compared with.
        if compare_only {
            if let Some(first) = texts.iter().flatten().next() {
                reference = first.clone();
            }
        }
        let reference = &reference;
        let mut cells = Vec::new();
        for (ci, t) in texts.iter().enumerate() {
            match t {
                Some(t) => {
                    let d = distance(reference, t);
                    per_class[ci].entry(class.clone()).or_default().add(d);
                    overall[ci].add(d);
                    cells.push(format!("{:>6.1}", 100.0 * d.cer));
                }
                None => cells.push("     -".to_string()),
            }
        }
        for a in 0..texts.len() {
            for b in a + 1..texts.len() {
                if let (Some(ta), Some(tb)) = (&texts[a], &texts[b]) {
                    pair.entry((a, b)).or_default().add(distance(ta, tb));
                }
            }
        }
        println!("{:<44} {:<14} {}", stem, class, cells.join("  "));
    }
    if !realigned.is_empty() {
        println!("\nre-aligned references ({}): {}", realigned.len(), realigned.join(", "));
    }
    if !compare_only {
        println!("\nper class:");
        let classes: std::collections::BTreeSet<String> = refs.keys().map(|(c, _)| c.clone()).collect();
        for class in &classes {
            println!("  {class}");
            for (ci, name) in names.iter().enumerate() {
                if let Some(agg) = per_class[ci].get(class) {
                    println!("    {:<24} {}", name, agg.line());
                }
            }
        }
        println!("\noverall:");
        for (ci, name) in names.iter().enumerate() {
            println!(
                "  {:<24} {}{}",
                name,
                overall[ci].line(),
                if missing[ci] > 0 { format!("  (missing {})", missing[ci]) } else { String::new() }
            );
        }
    } else if missing.iter().any(|m| *m > 0) {
        println!("\nmissing pages:");
        for (ci, name) in names.iter().enumerate() {
            if missing[ci] > 0 {
                println!("  {:<24} {}", name, missing[ci]);
            }
        }
    }
    if !pair.is_empty() {
        println!("\ncandidate vs candidate:");
        for ((a, b), agg) in &pair {
            println!("  {:<24} vs {:<24} {}", names[*a], names[*b], agg.line());
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalisation_strips_tags_and_folds_when_loose() {
        let site = "<page-num>23</page-num>\n<header>Eingang der Vniuersal Tinctur.</header>\n\nsolchen Himmel / nemblich auß Wasser";
        let strict = normalise(site, false);
        assert_eq!(strict, "23 Eingang der Vniuersal Tinctur. solchen Himmel / nemblich auß Wasser");
        let loose = normalise(site, true);
        assert_eq!(loose, "23 eingang der uniuersal tinctur solchen himmel nemblich auss wasser");
        let html = "<p>an=<br>fang &amp; ende</p>";
        assert_eq!(normalise(html, false), "an= fang & ende");
        assert_eq!(normalise(html, true), "anfang ende");
        // A picture block and its description are not page text.
        let plate = "<div data-bbox=\"1 2 3 4\" data-label=\"Text\"><p>ORTVS</p></div><div data-bbox=\"1 2 3 4\" data-label=\"Image\"><img alt=\"A watercolour\"/>A watercolour of a flask.</div><p>Oleum</p>";
        assert_eq!(normalise(plate, false), "ORTVS Oleum");
    }

    #[test]
    fn distances_are_rates_over_the_reference() {
        let d = distance("abcd", "abcd");
        assert_eq!(d.cer, 0.0);
        assert_eq!(d.wer, 0.0);
        let d = distance("abcd efgh", "abxd efgh");
        assert!((d.cer - 1.0 / 9.0).abs() < 1e-9);
        assert!((d.wer - 0.5).abs() < 1e-9);
        assert_eq!(levenshtein(&['a', 'b'], &[]), 2);
    }
}
