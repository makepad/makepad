// Native-only research CLI: standard clocks are used only for offline profiling.
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

//! Karaoke word-alignment research harness.
//!
//! The tight loop the timing work iterates in — no UI, no app, no decks:
//!
//! ```text
//! cargo run --release -p makepad-vj --bin karaoke_align -- --all
//! cargo run --release -p makepad-vj --bin karaoke_align -- --digest <hex>
//! cargo run --release -p makepad-vj --bin karaoke_align -- --wav path.wav
//! ```
//!
//! Input: a VJ stem-cache digest (`local/vj/stem-cache/<digest>`, the
//! separated vocals of a real track) or any wav. Output, per track:
//!
//! * a per-word evidence table: segment-proportional baseline → pass-1 DTW →
//!   teacher-forced → onset-snapped → independent ground-truth onset → error;
//! * error histograms per stage and per track region (the first 30 s is its
//!   own named case — that is where every karaoke session starts and where
//!   the field reports said it hurt);
//! * the gate verdict (≥90 % within ±50 ms, none beyond ±100 ms, mean bias
//!   within ±15 ms, monotonic, no skipped lines);
//! * an audible verification wav: the vocals stem with a click at every
//!   predicted word start (higher pitch at line starts). Alignment quality
//!   is something you HEAR — `afplay local/vj/karaoke-audit/<digest>.clicks.wav`.
//!
//! Flags: `--no-force`, `--no-snap` skip stages; `--table N` rows of
//! evidence (default 40, 0 = all).

use makepad_audio_lyrics::align as lyrics_align;

use lyrics_align::{OnsetPreset, SegmentWords, TimedLine, VocalAnalysis};
use makepad_ai_speech::whisper::{WhisperModel, WhisperParams, WhisperState};
use std::io::Write as _;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn model_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("MAKEPAD_VOICE_MODEL") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let root = repo_root();
    ["ggml-large-v3-turbo.bin", "local/ggml-large-v3-turbo.bin", "local/models/ggml-large-v3-turbo.bin"]
        .into_iter()
        .map(|name| root.join(name))
        .find(|path| path.is_file())
}

// ---------------------------------------------------------------------------
// stem cache reading (direct: header + spans + gains + vocals.pcm)
// ---------------------------------------------------------------------------

const STEM_VOCALS: usize = 3; // drums, bass, other, VOCALS
const NUM_STEMS: usize = 4;

struct Vocals {
    mono: Vec<f32>,
    rate: f64,
}

fn read_vocals(dir: &Path) -> Result<Vocals, String> {
    let header = std::fs::read_to_string(dir.join("header"))
        .map_err(|error| format!("{}: {error}", dir.display()))?;
    let field = |key: &str| -> Result<u64, String> {
        header
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{key}=")))
            .and_then(|value| value.trim().parse::<u64>().ok())
            .ok_or_else(|| format!("header has no {key}"))
    };
    let rate = field("sample_rate")? as f64;
    let frames = field("frames")? as usize;
    let span_samples = field("span_samples")? as usize;
    let span_count = field("span_count")? as usize;
    let spans = std::fs::read(dir.join("spans")).map_err(|error| error.to_string())?;
    if spans.len() < span_count || spans[..span_count].iter().any(|present| *present != 1) {
        return Err("separation incomplete".into());
    }
    let gains_raw = std::fs::read(dir.join("gains")).map_err(|error| error.to_string())?;
    let gain_at = |span: usize| -> f32 {
        let at = (span * NUM_STEMS + STEM_VOCALS) * 4;
        gains_raw
            .get(at..at + 4)
            .map(|bytes| f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
            .unwrap_or(1.0)
    };
    let pcm = std::fs::read(dir.join("vocals.pcm")).map_err(|error| error.to_string())?;
    let mut mono = Vec::with_capacity(frames);
    for frame in 0..frames.min(pcm.len() / 4) {
        let at = frame * 4;
        let left = i16::from_le_bytes([pcm[at], pcm[at + 1]]) as f32 / 32767.0;
        let right = i16::from_le_bytes([pcm[at + 2], pcm[at + 3]]) as f32 / 32767.0;
        let gain = gain_at(frame / span_samples.max(1));
        mono.push((left + right) * 0.5 * gain);
    }
    if mono.is_empty() {
        return Err("vocals stem is empty".into());
    }
    Ok(Vocals { mono, rate })
}

fn read_wav(path: &Path) -> Result<Vocals, String> {
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a wav".into());
    }
    let mut at = 12usize;
    let mut rate = 16_000u32;
    let mut channels = 1u16;
    let mut bits = 16u16;
    let mut data: &[u8] = &[];
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes([bytes[at + 4], bytes[at + 5], bytes[at + 6], bytes[at + 7]])
            as usize;
        let body = &bytes[at + 8..(at + 8 + size).min(bytes.len())];
        if id == b"fmt " && body.len() >= 16 {
            channels = u16::from_le_bytes([body[2], body[3]]);
            rate = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
            bits = u16::from_le_bytes([body[14], body[15]]);
        } else if id == b"data" {
            data = body;
            break;
        }
        at += 8 + size + (size & 1);
    }
    if bits != 16 || data.is_empty() {
        return Err("expected 16-bit pcm".into());
    }
    let stride = channels.max(1) as usize;
    let mut mono = Vec::with_capacity(data.len() / 2 / stride);
    let mut index = 0usize;
    while (index + stride) * 2 <= data.len() {
        let mut sum = 0.0f32;
        for channel in 0..stride {
            let at = (index + channel) * 2;
            sum += i16::from_le_bytes([data[at], data[at + 1]]) as f32 / 32768.0;
        }
        mono.push(sum / stride as f32);
        index += stride;
    }
    Ok(Vocals { mono, rate: rate as f64 })
}

fn write_wav_mono16(path: &Path, samples: &[f32], rate: u32) -> std::io::Result<()> {
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    let data_len = (samples.len() * 2) as u32;
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&((sample.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    std::fs::write(path, out)
}

/// The audible verdict: clicks over the stem. 1 kHz per word, 1.6 kHz where
/// a line starts.
fn click_track(mono: &[f32], rate: f64, lines: &[TimedLine]) -> Vec<f32> {
    let mut out: Vec<f32> = mono.iter().map(|sample| sample * 0.6).collect();
    let mut click = |time: f64, freq: f64, secs: f64, level: f32| {
        let from = (time * rate) as usize;
        let count = (secs * rate) as usize;
        for k in 0..count {
            let Some(sample) = out.get_mut(from + k) else { break };
            let t = k as f64 / rate;
            let envelope = (-t * 90.0).exp();
            *sample += ((2.0 * std::f64::consts::PI * freq * t).sin() * envelope) as f32 * level;
        }
    };
    for line in lines {
        if line.words.is_empty() {
            click(line.start, 400.0, 0.05, 0.5); // unaligned line: low thud
            continue;
        }
        for (index, word) in line.words.iter().enumerate() {
            if index == 0 {
                click(*word, 1600.0, 0.045, 0.55);
            } else {
                click(*word, 1000.0, 0.03, 0.45);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// evaluation
// ---------------------------------------------------------------------------

/// One word's journey through the stages, in seconds.
struct Evidence {
    text: String,
    line: usize,
    baseline: f64,
    dtw: f64,
    forced: f64,
    snapped: f64,
    truth: Option<f64>,
    score: f32,
}

fn flatten_starts(segments: &[SegmentWords]) -> Vec<f64> {
    segments
        .iter()
        .flat_map(|segment| segment.words.iter().map(|word| word.start))
        .collect()
}

/// Chars-proportional word times across each segment's stamps — what a
/// pipeline without any word alignment can do, the "before" of every
/// histogram.
fn baseline_starts(segments: &[SegmentWords]) -> Vec<f64> {
    let mut out = Vec::new();
    for segment in segments {
        let words: Vec<&str> = segment.text.split_whitespace().collect();
        let total: usize = words.iter().map(|word| word.chars().count() + 1).sum();
        let span = (segment.end - segment.start).max(1e-3);
        let mut consumed = 0usize;
        for word in &words {
            out.push(segment.start + span * consumed as f64 / total.max(1) as f64);
            consumed += word.chars().count() + 1;
        }
        // Only words that survived into `segment.words` are evaluated; the
        // counts match because collect_words demands exact word parity.
        if segment.words.is_empty() {
            out.truncate(out.len() - words.len());
        }
    }
    out
}

/// Greedy unique matching: every final word claims its nearest unclaimed
/// ground-truth onset within the window, closest pairs first.
fn match_truth(finals: &[f64], truth: &VocalAnalysis, window: f64) -> Vec<Option<f64>> {
    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    for (word, start) in finals.iter().enumerate() {
        for (index, onset) in truth.onsets.iter().enumerate() {
            let delta = (onset.time - start).abs();
            if delta <= window {
                pairs.push((delta, word, index));
            }
        }
    }
    pairs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut out = vec![None; finals.len()];
    let mut used = vec![false; truth.onsets.len()];
    for (_, word, index) in pairs {
        if out[word].is_none() && !used[index] {
            out[word] = Some(truth.onsets[index].time);
            used[index] = true;
        }
    }
    out
}

struct StageStats {
    name: &'static str,
    matched: usize,
    within_25: usize,
    within_50: usize,
    within_100: usize,
    beyond_100: usize,
    mean_abs_ms: f64,
    bias_ms: f64,
    p95_ms: f64,
    worst_ms: f64,
}

fn stage_stats(name: &'static str, starts: &[f64], truth: &[Option<f64>]) -> StageStats {
    let mut errors: Vec<f64> = Vec::new();
    let mut signed = 0.0f64;
    for (start, gt) in starts.iter().zip(truth) {
        if let Some(gt) = gt {
            errors.push(((start - gt) * 1000.0).abs());
            signed += (start - gt) * 1000.0;
        }
    }
    let matched = errors.len();
    let mut sorted = errors.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p95 = if matched == 0 { 0.0 } else { sorted[((matched - 1) as f64 * 0.95) as usize] };
    StageStats {
        name,
        matched,
        within_25: errors.iter().filter(|error| **error <= 25.0).count(),
        within_50: errors.iter().filter(|error| **error <= 50.0).count(),
        within_100: errors.iter().filter(|error| **error <= 100.0).count(),
        beyond_100: errors.iter().filter(|error| **error > 100.0).count(),
        mean_abs_ms: if matched == 0 { 0.0 } else { errors.iter().sum::<f64>() / matched as f64 },
        bias_ms: if matched == 0 { 0.0 } else { signed / matched as f64 },
        p95_ms: p95,
        worst_ms: sorted.last().copied().unwrap_or(0.0),
    }
}

fn print_stats(report: &mut String, stats: &[StageStats]) {
    line(report, format!(
        "  {:<10} {:>7} {:>7} {:>7} {:>7} {:>7} {:>9} {:>8} {:>8} {:>8}",
        "stage", "matched", "<=25ms", "<=50ms", "<=100", ">100", "mean|e|", "bias", "p95", "worst"
    ));
    for stat in stats {
        line(report, format!(
            "  {:<10} {:>7} {:>6.1}% {:>6.1}% {:>6.1}% {:>7} {:>7.1}ms {:>+6.1}ms {:>6.1}ms {:>6.1}ms",
            stat.name,
            stat.matched,
            percent(stat.within_25, stat.matched),
            percent(stat.within_50, stat.matched),
            percent(stat.within_100, stat.matched),
            stat.beyond_100,
            stat.mean_abs_ms,
            stat.bias_ms,
            stat.p95_ms,
            stat.worst_ms,
        ));
    }
}

fn percent(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 * 100.0 / whole as f64
    }
}

fn histogram(report: &mut String, title: &str, starts: &[f64], truth: &[Option<f64>]) {
    const EDGES: [f64; 7] = [10.0, 25.0, 50.0, 75.0, 100.0, 150.0, f64::INFINITY];
    const LABELS: [&str; 7] = ["<=10", "<=25", "<=50", "<=75", "<=100", "<=150", ">150"];
    let mut buckets = [0usize; 7];
    let mut total = 0usize;
    for (start, gt) in starts.iter().zip(truth) {
        if let Some(gt) = gt {
            let error = ((start - gt) * 1000.0).abs();
            let slot = EDGES.iter().position(|edge| error <= *edge).unwrap_or(6);
            buckets[slot] += 1;
            total += 1;
        }
    }
    line(report, format!("  {title}"));
    for (slot, label) in LABELS.iter().enumerate() {
        let bar = "#".repeat((buckets[slot] * 50 / total.max(1)).min(50));
        line(report, format!("    {:>6} {:>4} {}", label, buckets[slot], bar));
    }
}

fn line(report: &mut String, text: impl AsRef<str>) {
    println!("{}", text.as_ref());
    report.push_str(text.as_ref());
    report.push('\n');
}

// ---------------------------------------------------------------------------
// the run
// ---------------------------------------------------------------------------

struct Options {
    force: bool,
    snap: bool,
    table_rows: usize,
    focus: Vec<(f64, f64)>,
}

/// Permanent regression fixtures, by track label — the exact spots the user
/// called out from the decks. Never remove one; add the Gimme! Gimme!
/// Gimme! "man after midnight" window the day that track's stems are cached.
fn named_windows(label: &str) -> Vec<(f64, f64, &'static str)> {
    if label.starts_with("58ea58c84b88") {
        // Dancing Queen: the continuous-phrase run around "…dig in the
        // dancing queen" that whisper hears as "a king" — lines flow with no
        // acoustic gap, so any segment-stamp timing lights the next line
        // early. Field-reported twice.
        return vec![(44.0, 68.0, "dancing-queen 'a king' cluster")];
    }
    if label.starts_with("9578408f63bd") {
        // Gimme! Gimme! Gimme!: "…gimme, gimme, gimme / (a) man after
        // midnight" — the original field report: the second half of the
        // phrase lit long before it was sung, because the lyric line break
        // sits mid-phrase with no silence for the cutter to find. Line
        // starts MUST come from the first word's aligned time here.
        return vec![(230.0, 254.0, "gimme 'man after midnight'")];
    }
    Vec::new()
}

fn audit_track(
    label: &str,
    vocals: Vocals,
    model: &WhisperModel,
    state: &mut WhisperState,
    options: &Options,
    out_dir: &Path,
) {
    let mut report = String::new();
    let duration = vocals.mono.len() as f64 / vocals.rate;
    line(&mut report, format!("== {label} ({duration:.1}s @ {:.0}Hz) ==", vocals.rate));

    let started = std::time::Instant::now();
    let samples_16k = lyrics_align::resample(&vocals.mono, vocals.rate, lyrics_align::WHISPER_RATE);
    let snap_analysis = lyrics_align::analyze_vocals(&vocals.mono, vocals.rate, OnsetPreset::Snapping);
    let truth_analysis =
        lyrics_align::analyze_vocals(&vocals.mono, vocals.rate, OnsetPreset::GroundTruth);
    line(&mut report, format!(
        "  analysis: {} snap onsets, {} ground-truth onsets ({:.1}s)",
        snap_analysis.onsets.len(),
        truth_analysis.onsets.len(),
        started.elapsed().as_secs_f64()
    ));

    // Pass 1: transcription with cross-attention capture.
    let started = std::time::Instant::now();
    let mut params = WhisperParams::default();
    params.language = std::env::var("VJ_LYRICS_LANG").unwrap_or_else(|_| "en".into());
    params.temperature = 0.0;
    let aligned = state.transcribe_aligned(model, &samples_16k, &params);
    line(&mut report, format!(
        "  pass1: {} segments, {} aligned words ({:.1}s, backend {})",
        aligned.len(),
        aligned.iter().map(|segment| segment.words.len()).sum::<usize>(),
        started.elapsed().as_secs_f64(),
        makepad_ai_speech::whisper::accel_backend_name(),
    ));

    // Stage snapshots.
    let mut segments = lyrics_align::collect_words(aligned, &snap_analysis, duration);
    lyrics_align::enforce_monotonic(&mut segments);
    let dtw_starts = flatten_starts(&segments);
    let baseline = baseline_starts(&segments);

    let started = std::time::Instant::now();
    if options.force {
        lyrics_align::force_align_segments(
            state,
            model,
            &samples_16k,
            &mut segments,
            &params.language,
        );
        lyrics_align::enforce_monotonic(&mut segments);
    }
    let forced_starts = flatten_starts(&segments);
    let forced_count = segments.iter().filter(|segment| segment.forced).count();
    line(&mut report, format!(
        "  pass2: {}/{} segments teacher-forced ({:.1}s)",
        forced_count,
        segments.len(),
        started.elapsed().as_secs_f64()
    ));

    // Word-parity accounting: every whitespace word of every kept segment
    // should end up timed; a segment that could not be is a named loss.
    for segment in &segments {
        let expected = segment.text.split_whitespace().count();
        if segment.words.len() != expected {
            line(&mut report, format!(
                "  PARITY LOSS [{:.1}-{:.1}s] {} words expected, {} timed, forced={}: {:?}",
                segment.start,
                segment.end,
                expected,
                segment.words.len(),
                segment.forced,
                segment.text.chars().take(60).collect::<String>(),
            ));
        }
    }

    lyrics_align::rescue_absorbed_words(&mut segments, &snap_analysis);
    lyrics_align::enforce_monotonic(&mut segments);
    if options.snap {
        lyrics_align::snap_words(&mut segments, &snap_analysis);
    }
    let snapped_starts = flatten_starts(&segments);
    let lines = lyrics_align::assemble_lines(&segments, duration);

    // Two complementary reads, because no single pairing is neutral:
    //
    // * The STAGE table pairs each stage's own times against the strict
    //   ground-truth events independently (nearest unclaimed attack within
    //   150 ms) — "how close is this stage to a real audible attack". For
    //   the snapped stage this partially measures detector agreement; the
    //   forced column is the detector-independent witness (whisper's
    //   attention never saw any onset detector).
    // * The evidence table and regions use the FINAL stage's pairing, which
    //   is what the display actually shows.
    let truth = match_truth(&snapped_starts, &truth_analysis, 0.15);
    let truth_baseline = match_truth(&baseline, &truth_analysis, 0.15);
    let truth_dtw = match_truth(&dtw_starts, &truth_analysis, 0.15);
    let truth_forced = match_truth(&forced_starts, &truth_analysis, 0.15);

    let flat_words: Vec<(usize, String, f32)> = {
        let mut out = Vec::new();
        let mut word_index = 0usize;
        for (line_index, timed) in lines.iter().enumerate() {
            for _ in &timed.words {
                out.push((line_index, String::new(), 0.0));
                word_index += 1;
            }
        }
        let _ = word_index;
        let mut texts = segments
            .iter()
            .flat_map(|segment| segment.words.iter().map(|word| (word.text.clone(), word.score)));
        for slot in out.iter_mut() {
            if let Some((text, score)) = texts.next() {
                slot.1 = text;
                slot.2 = score;
            }
        }
        out
    };

    let stats = vec![
        stage_stats("baseline", &baseline, &truth_baseline),
        stage_stats("dtw", &dtw_starts, &truth_dtw),
        stage_stats("forced", &forced_starts, &truth_forced),
        stage_stats("snapped", &snapped_starts, &truth),
    ];
    line(&mut report, String::new());
    print_stats(&mut report, &stats);

    // Identity consensus: pass 1 and the teacher-forced pass decode the same
    // text through DIFFERENT windows and contexts; where they agree on a
    // word's time, the word's identity is verified by two independent runs
    // of the model — no onset detector involved.
    {
        let mut agree_80 = 0usize;
        let mut agree_150 = 0usize;
        let mut snapped_count = 0usize;
        let mut snap_ms = Vec::new();
        for (dtw, forced) in dtw_starts.iter().zip(&forced_starts) {
            let delta = (dtw - forced).abs();
            if delta <= 0.08 {
                agree_80 += 1;
            }
            if delta <= 0.15 {
                agree_150 += 1;
            }
        }
        for segment in &segments {
            for word in &segment.words {
                if let Some(delta) = word.snap {
                    snapped_count += 1;
                    snap_ms.push(delta.abs() * 1000.0);
                }
            }
        }
        snap_ms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let snap_p50 = snap_ms.get(snap_ms.len() / 2).copied().unwrap_or(0.0);
        let snap_p95 = if snap_ms.is_empty() {
            0.0
        } else {
            snap_ms[((snap_ms.len() - 1) as f64 * 0.95) as usize]
        };
        line(&mut report, String::new());
        line(&mut report, format!(
            "  identity: pass1 vs teacher-forced agree on {}/{} words within 80ms ({} within 150ms); \
             snap moved {}/{} words (p50 {:.0}ms, p95 {:.0}ms)",
            agree_80,
            dtw_starts.len(),
            agree_150,
            snapped_count,
            snapped_starts.len(),
            snap_p50,
            snap_p95,
        ));
    }

    line(&mut report, String::new());
    histogram(&mut report, "histogram BEFORE (baseline):", &baseline, &truth_baseline);
    histogram(&mut report, "histogram AFTER (snapped):", &snapped_starts, &truth);

    // Region cases — the beginning of the track is where karaoke starts.
    line(&mut report, String::new());
    for (name, from, to) in [
        ("first 30s", 0.0, 30.0f64.min(duration)),
        ("mid", 30.0, (duration - 30.0).max(30.0)),
        ("last 30s", (duration - 30.0).max(30.0), duration),
    ] {
        let region: Vec<usize> = snapped_starts
            .iter()
            .enumerate()
            .filter(|(_, start)| **start >= from && **start < to)
            .map(|(index, _)| index)
            .collect();
        let starts: Vec<f64> = region.iter().map(|index| snapped_starts[*index]).collect();
        let gts: Vec<Option<f64>> = region.iter().map(|index| truth[*index]).collect();
        let stat = stage_stats("snapped", &starts, &gts);
        line(&mut report, format!(
            "  region {name:<9} ({from:>6.1}-{to:>6.1}s): {} matched, {:>5.1}% <=50ms, {} beyond 100ms, bias {:+.1}ms, worst {:.1}ms",
            stat.matched,
            percent(stat.within_50, stat.matched),
            stat.beyond_100,
            stat.bias_ms,
            stat.worst_ms,
        ));
    }

    // The gate.
    line(&mut report, String::new());
    let final_stat = &stats[3];
    let unmatched = snapped_starts.len() - final_stat.matched;
    let order_ok = snapped_starts.windows(2).all(|pair| pair[1] > pair[0]);
    let lines_ordered = lines.windows(2).all(|pair| pair[1].start >= pair[0].start);
    let lines_overlap = lines.windows(2).any(|pair| pair[0].end > pair[1].start + 1e-9);
    let confident = lines.iter().filter(|timed| timed.confident).count();
    let boundary_ok = lines.iter().all(|timed| {
        timed.words.is_empty() || (timed.words[0] - timed.start).abs() < 1e-9
    });
    let gate_pct = final_stat.matched > 0 && percent(final_stat.within_50, final_stat.matched) >= 90.0;
    let gate_hard = final_stat.beyond_100 == 0;
    let gate_bias = final_stat.bias_ms.abs() <= 15.0;
    line(&mut report, format!(
        "  gate: within50 {} ({:.1}%), beyond100 {} ({}), bias {} ({:+.1}ms), word order {}, lines ordered {}, overlap-free {}, line-start=first-word {}",
        if gate_pct { "PASS" } else { "FAIL" },
        percent(final_stat.within_50, final_stat.matched),
        if gate_hard { "PASS" } else { "FAIL" },
        final_stat.beyond_100,
        if gate_bias { "PASS" } else { "FAIL" },
        final_stat.bias_ms,
        if order_ok { "PASS" } else { "FAIL" },
        if lines_ordered { "PASS" } else { "FAIL" },
        if !lines_overlap { "PASS" } else { "FAIL" },
        if boundary_ok { "PASS" } else { "FAIL" },
    ));
    line(&mut report, format!(
        "  lines: {} total, {confident} confident; words: {} aligned, {unmatched} without a ground-truth onset within 150ms",
        lines.len(),
        snapped_starts.len(),
    ));

    // Evidence table.
    let rows = if options.table_rows == 0 { usize::MAX } else { options.table_rows };
    line(&mut report, String::new());
    line(&mut report, format!(
        "  {:<16} {:>4} {:>9} {:>9} {:>9} {:>9} {:>9} {:>8} {:>6}",
        "word", "line", "baseline", "dtw", "forced", "snapped", "truth", "err", "score"
    ));
    let mut evidence: Vec<Evidence> = Vec::new();
    for index in 0..snapped_starts.len() {
        evidence.push(Evidence {
            text: flat_words.get(index).map(|word| word.1.clone()).unwrap_or_default(),
            line: flat_words.get(index).map(|word| word.0).unwrap_or(0),
            baseline: baseline.get(index).copied().unwrap_or(0.0),
            dtw: dtw_starts.get(index).copied().unwrap_or(0.0),
            forced: forced_starts.get(index).copied().unwrap_or(0.0),
            snapped: snapped_starts[index],
            truth: truth[index],
            score: flat_words.get(index).map(|word| word.2).unwrap_or(0.0),
        });
    }
    for item in evidence.iter().take(rows) {
        let (truth_text, error_text) = match item.truth {
            Some(gt) => (
                format!("{gt:9.3}"),
                format!("{:+7.0}ms", (item.snapped - gt) * 1000.0),
            ),
            None => ("        -".into(), "       -".into()),
        };
        line(&mut report, format!(
            "  {:<16} {:>4} {:>9.3} {:>9.3} {:>9.3} {:>9.3} {} {} {:>6.2}",
            item.text.chars().take(16).collect::<String>(),
            item.line,
            item.baseline,
            item.dtw,
            item.forced,
            item.snapped,
            truth_text,
            error_text,
            item.score,
        ));
    }
    if evidence.len() > rows {
        line(&mut report, format!("  … {} more words (pass --table 0 for all)", evidence.len() - rows));
    }

    // Named regression windows — the user's acceptance ears, kept forever.
    let mut windows = named_windows(label);
    windows.extend(options.focus.iter().map(|(a, b)| (*a, *b, "focus")));
    for (from, to, name) in &windows {
        line(&mut report, String::new());
        line(&mut report, format!("  == window {name} ({from:.1}-{to:.1}s) =="));
        for (index, timed) in lines.iter().enumerate() {
            if timed.end < *from || timed.start > *to {
                continue;
            }
            line(&mut report, format!(
                "    line {index:>3} {:>8.3}-{:>8.3} {} {:?}",
                timed.start,
                timed.end,
                if timed.confident { "hop  " } else { "SWEEP" },
                timed.text,
            ));
        }
        for item in &evidence {
            if item.snapped < *from || item.snapped > *to {
                continue;
            }
            let (truth_text, error_text) = match item.truth {
                Some(gt) => (
                    format!("{gt:9.3}"),
                    format!("{:+7.0}ms", (item.snapped - gt) * 1000.0),
                ),
                None => ("        -".into(), "       -".into()),
            };
            line(&mut report, format!(
                "    {:<16} {:>4} {:>9.3} {:>9.3} {:>9.3} {} {} {:>6.2}",
                item.text.chars().take(16).collect::<String>(),
                item.line,
                item.dtw,
                item.forced,
                item.snapped,
                truth_text,
                error_text,
                item.score,
            ));
        }
    }

    // Artifacts.
    let _ = std::fs::create_dir_all(out_dir);
    let clicks = click_track(&vocals.mono, vocals.rate, &lines);
    let wav_path = out_dir.join(format!("{label}.clicks.wav"));
    if write_wav_mono16(&wav_path, &clicks, vocals.rate as u32).is_ok() {
        line(&mut report, String::new());
        line(&mut report, format!("  listen: afplay '{}'", wav_path.display()));
    }
    let report_path = out_dir.join(format!("{label}.report.txt"));
    if let Ok(mut file) = std::fs::File::create(&report_path) {
        let _ = file.write_all(report.as_bytes());
        println!("  report: {}", report_path.display());
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut digests: Vec<String> = Vec::new();
    let mut wavs: Vec<PathBuf> = Vec::new();
    let mut all = false;
    let mut options = Options { force: true, snap: true, table_rows: 40, focus: Vec::new() };
    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "--digest" => {
                index += 1;
                digests.push(args[index].clone());
            }
            "--wav" => {
                index += 1;
                wavs.push(PathBuf::from(&args[index]));
            }
            "--all" => all = true,
            "--no-force" => options.force = false,
            "--no-snap" => options.snap = false,
            "--table" => {
                index += 1;
                options.table_rows = args[index].parse().unwrap_or(40);
            }
            "--focus" => {
                index += 1;
                if let Some((a, b)) = args[index].split_once('-') {
                    if let (Ok(a), Ok(b)) = (a.parse(), b.parse()) {
                        options.focus.push((a, b));
                    }
                }
            }
            other => {
                eprintln!("unknown flag {other}");
                std::process::exit(2);
            }
        }
        index += 1;
    }

    let stem_root = repo_root().join("local/vj/stem-cache");
    if all {
        if let Ok(entries) = std::fs::read_dir(&stem_root) {
            for entry in entries.flatten() {
                if entry.path().join("header").is_file() {
                    digests.push(entry.file_name().to_string_lossy().to_string());
                }
            }
        }
        digests.sort();
    }
    if digests.is_empty() && wavs.is_empty() {
        eprintln!("usage: karaoke_align --all | --digest <hex> | --wav <path> [--no-force] [--no-snap] [--table N]");
        std::process::exit(2);
    }

    let Some(model_file) = model_path() else {
        eprintln!("no whisper checkpoint (ggml-large-v3-turbo.bin) found");
        std::process::exit(1);
    };
    eprintln!("loading model {}", model_file.display());
    let model = WhisperModel::load_file(&model_file.to_string_lossy()).expect("model load");
    let mut state = WhisperState::new(&model);
    let out_dir = repo_root().join("local/vj/karaoke-audit");

    for digest in &digests {
        match read_vocals(&stem_root.join(digest)) {
            Ok(vocals) => {
                let label: String = digest.chars().take(12).collect();
                audit_track(&label, vocals, &model, &mut state, &options, &out_dir);
            }
            Err(error) => eprintln!("{digest}: {error}"),
        }
    }
    for wav in &wavs {
        match read_wav(wav) {
            Ok(vocals) => {
                let label = wav
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().to_string())
                    .unwrap_or_else(|| "wav".into());
                audit_track(&label, vocals, &model, &mut state, &options, &out_dir);
            }
            Err(error) => eprintln!("{}: {error}", wav.display()),
        }
    }
}
