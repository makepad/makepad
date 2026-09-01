//! sa3-bake: prompt -> playable SFZ sampler bank on the local GPU.
//!
//! The model has no pitch control, so the baker never asks for one: it
//! generates takes with register-nudged prompts, DETECTS each take's actual
//! f0 (YIN), and lays the accepted takes out multi-sample style — every key
//! plays the take nearest to it in pitch (sampler shift <= ~half an anchor
//! gap), with exact-octave sinc resamples of the extreme takes extending the
//! compass. See libs/ai/models/sfx/src/sa3_bake.rs for the DSP and the
//! measurement-driven reasoning.
//!
//! Usage:
//!   sa3-bake bake --prompt "space harpsichord" --out <bankdir>
//!            [--takes 16] [--seconds 4] [--steps 8] [--seed 1] [--weights <dir>]
//!            [--reuse-takes] [--plain-prompt]
//!   sa3-bake analyze <wav> [<wav>...]
//!
//! Machine-readable stdout for the host app:
//!   PROGRESS <0..1> <label>        TAKE <i>/<n> midi=.. conf=.. ...
//!   ROOT key=.. span=..            BANK <path to bank.sfz>
//!   ERROR <message> (+ non-zero exit)

use makepad_diffusion::sa3_bake::{
    analyze_take, apply_fades, envelope_distance, plan_anchor_layout, read_wav, resample,
    write_sfz, write_wav_stereo16, AnchorRegion, TakeForBank, BAKE_SAMPLE_RATE,
};
use makepad_diffusion::sa3_pipeline::{Sa3Pipeline, Sa3SeededNoise};
use makepad_diffusion::sa3_tokenizer::Sa3Tokenizer;
use makepad_diffusion::sa3_transformer::Sa3PadMode;
use std::path::{Path, PathBuf};

fn fail(msg: &str) -> ! {
    println!("ERROR {msg}");
    std::process::exit(1);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("analyze") => analyze_cmd(&args[2..]),
        Some("bake") => bake_cmd(&args[2..]),
        _ => {
            eprintln!("usage: sa3-bake bake --prompt <text> --out <dir> [...]");
            eprintln!("       sa3-bake analyze <wav> [<wav>...]");
            std::process::exit(2);
        }
    }
}

// ---------------------------------------------------------------------------
// analyze: characterisation of existing wavs
// ---------------------------------------------------------------------------

fn analyze_cmd(files: &[String]) {
    let mut rows: Vec<(String, Option<f32>, Vec<f32>)> = Vec::new();
    for file in files {
        let wav = match read_wav(Path::new(file)) {
            Ok(wav) => wav,
            Err(e) => {
                println!("ANALYZE file={file} error={e}");
                continue;
            }
        };
        let mono = wav.mono();
        let a = analyze_take(&mono, wav.sample_rate);
        match &a.pitch {
            Some(p) => println!(
                "ANALYZE file={file} f0={:.2} midi={:.2} note={} conf={:.3} stab_cents={:.1} voiced={:.2} events={} dur={:.2} onset_ms={:.0} sound_ms={:.0} peak={:.3} loop={}",
                p.f0_hz,
                p.midi,
                note_name(p.midi),
                p.confidence,
                p.stability_cents,
                p.voiced_ratio,
                a.extent.events,
                a.duration_s,
                a.extent.onset as f32 / wav.sample_rate as f32 * 1000.0,
                (a.extent.end - a.extent.onset) as f32 / wav.sample_rate as f32 * 1000.0,
                a.peak,
                match a.loop_region {
                    Some((s, e)) => format!("{s}..{e}"),
                    None => "none".into(),
                }
            ),
            None => println!(
                "ANALYZE file={file} f0=none events={} dur={:.2} sound_ms={:.0} peak={:.3}",
                a.extent.events,
                a.duration_s,
                (a.extent.end - a.extent.onset) as f32 / wav.sample_rate as f32 * 1000.0,
                a.peak
            ),
        }
        rows.push((file.clone(), a.pitch.as_ref().map(|p| p.midi), a.envelope));
    }
    // pairwise spectral-envelope distances (timbre agreement)
    if rows.len() > 1 {
        let mut dists = Vec::new();
        for i in 0..rows.len() {
            for j in i + 1..rows.len() {
                let d = envelope_distance(&rows[i].2, &rows[j].2);
                dists.push(d);
                println!(
                    "ENVDIST {} {} {:.2}",
                    short(&rows[i].0),
                    short(&rows[j].0),
                    d
                );
            }
        }
        dists.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = dists[dists.len() / 2];
        let mean = dists.iter().sum::<f32>() / dists.len() as f32;
        println!(
            "ENVSTATS pairs={} median_db={:.2} mean_db={:.2} min_db={:.2} max_db={:.2}",
            dists.len(),
            median,
            mean,
            dists[0],
            dists[dists.len() - 1]
        );
    }
}

fn short(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

fn note_name(midi: f32) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let key = midi.round() as i32;
    let cents = ((midi - key as f32) * 100.0).round() as i32;
    format!(
        "{}{}{:+}c",
        NAMES[key.rem_euclid(12) as usize],
        key / 12 - 1,
        cents
    )
}

// ---------------------------------------------------------------------------
// bake: prompt -> bank
// ---------------------------------------------------------------------------

struct BakeOpts {
    prompt: String,
    out: PathBuf,
    takes: usize,
    seconds: f64,
    steps: usize,
    seed: u64,
    weights: PathBuf,
    reuse_takes: bool,
    plain_prompt: bool,
}

fn parse_opts(args: &[String]) -> BakeOpts {
    let mut map = std::collections::HashMap::new();
    let mut flags = std::collections::HashSet::new();
    let mut i = 0;
    while i < args.len() {
        if let Some(key) = args[i].strip_prefix("--") {
            if matches!(key, "reuse-takes" | "plain-prompt") {
                flags.insert(key.to_string());
                i += 1;
            } else if i + 1 < args.len() {
                map.insert(key.to_string(), args[i + 1].clone());
                i += 2;
            } else {
                fail(&format!("flag --{key} needs a value"));
            }
        } else {
            i += 1;
        }
    }
    let prompt = map
        .get("prompt")
        .cloned()
        .unwrap_or_else(|| fail("bake needs --prompt"));
    let out = PathBuf::from(
        map.get("out")
            .cloned()
            .unwrap_or_else(|| fail("bake needs --out <dir>")),
    );
    BakeOpts {
        prompt,
        out,
        takes: map.get("takes").and_then(|v| v.parse().ok()).unwrap_or(16).clamp(3, 48),
        seconds: map
            .get("seconds")
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(4.0)
            .clamp(1.0, 12.0),
        steps: map.get("steps").and_then(|v| v.parse().ok()).unwrap_or(8).clamp(1, 32),
        seed: map.get("seed").and_then(|v| v.parse().ok()).unwrap_or(1),
        weights: map.get("weights").map(PathBuf::from).unwrap_or_else(default_weights),
        reuse_takes: flags.contains("reuse-takes"),
        plain_prompt: flags.contains("plain-prompt"),
    }
}

fn default_weights() -> PathBuf {
    if let Ok(dir) = std::env::var("MAKEPAD_SA3_WEIGHTS") {
        return PathBuf::from(dir);
    }
    // dev checkout weights, then the asset-ai cache
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dev = repo.join("local/sa3_ref/weights/stable-audio-3-small-sfx");
    if dev.join("model.safetensors").is_file() {
        return dev;
    }
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".makepad/ai_content/audio/sa3")
}

/// Register nudges cycled across takes: the model has no pitch control, but
/// coarse register words spread the detected anchors across the compass.
fn take_prompt(user: &str, plain: bool, index: usize) -> String {
    if plain {
        return user.to_string();
    }
    const REGISTERS: [&str; 3] = [
        "playing one single low bass note",
        "playing one single mid-range note",
        "playing one single high note",
    ];
    format!(
        "{user}, {}, solo, dry, no reverb",
        REGISTERS[index % REGISTERS.len()]
    )
}

fn bake_cmd(args: &[String]) {
    let opts = parse_opts(args);
    let takes_dir = opts.out.join("takes");
    if let Err(e) = std::fs::create_dir_all(&takes_dir) {
        fail(&format!("create {}: {e}", takes_dir.display()));
    }

    // --- phase 1: generate takes -------------------------------------------
    println!("PROGRESS 0.00 load model");
    let mut pipeline_slot: Option<(Sa3Pipeline, Sa3Tokenizer)> = None;
    let mut ensure_pipeline = |slot: &mut Option<(Sa3Pipeline, Sa3Tokenizer)>| {
        if slot.is_none() {
            let tokenizer =
                Sa3Tokenizer::load(opts.weights.join("t5gemma-b-b-ul2/tokenizer.model"))
                    .unwrap_or_else(|e| fail(&format!("tokenizer load: {e:?}")));
            let pipeline = Sa3Pipeline::load(
                opts.weights.join("model.safetensors"),
                opts.weights.join("t5gemma-b-b-ul2/model.safetensors"),
                None,
            )
            .unwrap_or_else(|e| fail(&format!("pipeline load: {e:?}")));
            *slot = Some((pipeline, tokenizer));
        }
    };

    let gen_band = 0.86; // fraction of the job spent generating
    let mut take_files: Vec<PathBuf> = Vec::new();
    for i in 0..opts.takes {
        let path = takes_dir.join(format!("take_{i:02}.wav"));
        let fraction = 0.02 + gen_band * i as f64 / opts.takes as f64;
        if opts.reuse_takes && path.is_file() {
            println!("PROGRESS {fraction:.3} take {}/{} (cached)", i + 1, opts.takes);
            take_files.push(path);
            continue;
        }
        ensure_pipeline(&mut pipeline_slot);
        let (pipeline, tokenizer) = pipeline_slot.as_ref().unwrap();
        let prompt = take_prompt(&opts.prompt, opts.plain_prompt, i);
        println!("PROGRESS {fraction:.3} generate take {}/{}", i + 1, opts.takes);
        let (ids, mask) = tokenizer.tokenize_padded(&prompt);
        let mut noise = Sa3SeededNoise::new(opts.seed.wrapping_add(i as u64 * 7919));
        let audio = pipeline
            .generate(
                &ids,
                &mask,
                opts.seconds,
                opts.steps,
                Sa3PadMode::VZero,
                &mut noise,
                None,
                None,
            )
            .unwrap_or_else(|e| fail(&format!("generate take {i}: {e:?}")));
        let left = &audio[0];
        let right = &audio[1];
        write_wav_stereo16(&path, left, right, BAKE_SAMPLE_RATE)
            .unwrap_or_else(|e| fail(&format!("write take {i}: {e}")));
        take_files.push(path);
    }

    // --- phase 2: analyse + accept -----------------------------------------
    println!("PROGRESS 0.90 analyze takes");
    struct Candidate {
        index: usize,
        left: Vec<f32>,
        right: Vec<f32>,
        midi: f32,
        envelope: Vec<f32>,
    }
    let mut candidates: Vec<Candidate> = Vec::new();
    for (i, path) in take_files.iter().enumerate() {
        let wav = read_wav(path).unwrap_or_else(|e| fail(&format!("read take: {e}")));
        let mono = wav.mono();
        let a = analyze_take(&mono, wav.sample_rate);
        let verdict = match &a.pitch {
            None => "reject: unpitched",
            Some(p) if p.confidence < 0.55 => "reject: low confidence",
            Some(p) if p.stability_cents > 70.0 => "reject: unstable pitch",
            Some(p) if p.voiced_ratio < 0.35 => "reject: barely voiced",
            _ if (a.extent.end - a.extent.onset) < BAKE_SAMPLE_RATE as usize / 4 => {
                "reject: too short"
            }
            _ => "accept",
        };
        match &a.pitch {
            Some(p) => println!(
                "TAKE {}/{} midi={:.2} note={} conf={:.3} stab={:.1} events={} {}",
                i + 1,
                take_files.len(),
                p.midi,
                note_name(p.midi),
                p.confidence,
                p.stability_cents,
                a.extent.events,
                verdict
            ),
            None => println!("TAKE {}/{} unpitched {}", i + 1, take_files.len(), verdict),
        }
        if verdict != "accept" {
            continue;
        }
        let pitch = a.pitch.as_ref().unwrap();
        // keep only the first sounding event region, stereo
        let (s, e) = (a.extent.onset, a.extent.end);
        let left = wav.channels[0][s..e].to_vec();
        let right = wav.channels.get(1).map(|c| c[s..e].to_vec()).unwrap_or_else(|| left.clone());
        candidates.push(Candidate {
            index: i,
            left,
            right,
            midi: pitch.midi,
            envelope: a.envelope,
        });
    }
    if candidates.len() < 2 {
        fail(&format!(
            "only {} usable pitched takes out of {} — this prompt does not yield a pitched instrument; try adding words like 'single plucked note'",
            candidates.len(),
            take_files.len()
        ));
    }

    // --- phase 3: assign pitch classes, correct, write ----------------------
    println!("PROGRESS 0.94 assemble bank");
    let for_bank: Vec<TakeForBank> = candidates
        .iter()
        .map(|c| TakeForBank {
            index: c.index,
            midi: c.midi,
            envelope: c.envelope.clone(),
        })
        .collect();
    let regions = plan_anchor_layout(&for_bank);
    if regions.is_empty() {
        fail("no usable anchors after analysis");
    }

    // Render one wav per region. Natural anchors are the trimmed takes
    // themselves; octave extensions are exact factor-of-two sinc resamples.
    let mut written: Vec<(AnchorRegion, String)> = Vec::new();
    let mut anchor_envelopes: Vec<(u8, Vec<f32>)> = Vec::new();
    let mut levels: Vec<(u8, f32)> = Vec::new();
    for region in regions {
        let cand = candidates
            .iter()
            .find(|c| c.index == region.take_index)
            .unwrap();
        let ratio = 2f64.powf(region.octave_shift as f64 / 12.0);
        let (mut left, mut right) = if region.octave_shift == 0 {
            (cand.left.clone(), cand.right.clone())
        } else {
            (resample(&cand.left, ratio), resample(&cand.right, ratio))
        };
        // Even loudness across anchors: match the RMS of the first 800 ms
        // (attack + early decay — what a played note is heard as) to
        // -18 dBFS, with a peak clamp. Peak normalisation alone left a
        // 6.4 dB loudness spread across registers.
        let gain = {
            let probe_len = (BAKE_SAMPLE_RATE as usize * 4 / 5).min(left.len()).max(1);
            let rms = {
                let sum: f32 = left[..probe_len]
                    .iter()
                    .zip(&right[..probe_len])
                    .map(|(l, r)| {
                        let m = 0.5 * (l + r);
                        m * m
                    })
                    .sum();
                (sum / probe_len as f32).sqrt().max(1e-6)
            };
            let peak = left
                .iter()
                .zip(&right)
                .map(|(l, r)| l.abs().max(r.abs()))
                .fold(0.0f32, f32::max)
                .max(1e-6);
            (0.125 / rms).min(0.9 / peak)
        };
        for v in left.iter_mut() {
            *v = (*v * gain).clamp(-1.0, 1.0);
        }
        for v in right.iter_mut() {
            *v = (*v * gain).clamp(-1.0, 1.0);
        }
        let cap = BAKE_SAMPLE_RATE as usize * 8;
        left.truncate(cap);
        right.truncate(cap);
        apply_fades(&mut left, BAKE_SAMPLE_RATE);
        apply_fades(&mut right, BAKE_SAMPLE_RATE);
        let mono: Vec<f32> = left
            .iter()
            .zip(&right)
            .map(|(l, r)| 0.5 * (l + r))
            .collect();
        if region.octave_shift == 0 {
            anchor_envelopes.push((
                region.root_key,
                makepad_diffusion::sa3_bake::spectral_envelope(&mono, BAKE_SAMPLE_RATE),
            ));
        }
        let rms = (mono.iter().map(|v| v * v).sum::<f32>() / mono.len().max(1) as f32).sqrt();
        levels.push((region.root_key, 20.0 * rms.max(1e-9).log10()));
        let file = format!("root_{:03}.wav", region.root_key);
        write_wav_stereo16(&opts.out.join(&file), &left, &right, BAKE_SAMPLE_RATE)
            .unwrap_or_else(|e| fail(&format!("write root: {e}")));
        println!(
            "ROOT key={} span={}..{} take={} tune={}c octave={}",
            region.root_key, region.lo, region.hi, region.take_index,
            region.tune_cents, region.octave_shift
        );
        written.push((region, file));
    }

    // consistency between neighbouring natural anchors (register-adjacent,
    // so the comparison is as honest as a band envelope gets), and loudness
    // evenness across the bank
    let mut neighbour_dists = Vec::new();
    for w in anchor_envelopes.windows(2) {
        neighbour_dists.push(envelope_distance(&w[0].1, &w[1].1));
    }
    if !neighbour_dists.is_empty() {
        let mean = neighbour_dists.iter().sum::<f32>() / neighbour_dists.len() as f32;
        let max = neighbour_dists.iter().cloned().fold(0.0f32, f32::max);
        println!("CONSISTENCY neighbour_mean_db={mean:.2} neighbour_max_db={max:.2}");
    }
    if levels.len() > 1 {
        let min = levels.iter().map(|(_, db)| *db).fold(f32::MAX, f32::min);
        let max = levels.iter().map(|(_, db)| *db).fold(f32::MIN, f32::max);
        println!("LEVELS rms_spread_db={:.2}", max - min);
    }
    let low_key = written.iter().map(|(r, _)| r.lo).min().unwrap();
    let high_key = written.iter().map(|(r, _)| r.hi).max().unwrap();
    let max_shift = written
        .iter()
        .flat_map(|(r, _)| [r.lo as i32 - r.root_key as i32, r.hi as i32 - r.root_key as i32])
        .map(|d| d.abs())
        .max()
        .unwrap_or(0);

    let name = opts
        .out
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "generated".into());
    let sfz = opts.out.join("bank.sfz");
    write_sfz(&sfz, &name, &opts.prompt, &written, 0.35)
        .unwrap_or_else(|e| fail(&format!("write sfz: {e}")));
    println!(
        "RANGE low={low_key} high={high_key} anchors={} max_sampler_shift={max_shift} takes_used={}",
        written.len(),
        candidates.len()
    );
    println!("PROGRESS 1.00 done");
    println!("BANK {}", sfz.display());
}
