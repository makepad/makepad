//! MiniMax-Music3 stage validation against the official ModularPipeline
//! oracle dumps (`music3_oracle_dump.py` → `C:\ai\music3_oracle\<fixture>`).
//!
//! Stages:
//!   inventory  parse safetensors headers, assert official names/shapes
//!   tokenize   assemble caption+lyrics and match dump `text_ids.npy`
//!   cond       CPU condition encoder vs dump `cond_enc_in/out.npy`
//!   lm         CUDA Qwen3-8B prefill vs `lm_prefill_last_hidden` / logits
//!   rvq        CUDA depth decoder vs `rvq_step0_in/out.npy`
//!   dit        CUDA flow DiT step-0 vs `dit_step0_v_cond.npy`
//!   vocoder    CPU Flow-VAE vs `vocoder_out.npy`
//!   ar         KV-cache LM decode + RVQ replay vs `cond_enc_in.npy`
//!   all        inventory + tokenize + cond + lm + rvq + dit + vocoder
//!
//! Usage:
//!   music3-validate --weights <MiniMax-Music3 dir> [--dump <dir>] [--stage inventory|tokenize|cond|lm|rvq|dit|vocoder|ar|all]

use makepad_diffusion::music3::{
    assemble_prompt, inventory_weights, load_tokenizer, tokenize_cfg_pair, validate_shape_canaries,
    Music3ConditionEncoder, MUSIC3_AR_CFG, MUSIC3_AR_TOP_K, MUSIC3_AUDIO_CFG_TOKEN_ID,
    MUSIC3_AUDIO_CODE_OFFSET, MUSIC3_AUDIO_END_TOKEN_ID, MUSIC3_AUDIO_VOCAB, MUSIC3_COND_HIDDEN,
    MUSIC3_COND_LAYERS, MUSIC3_SEMANTIC_VOCAB,
    MUSIC3_COND_OUT, MUSIC3_DIT_IN_CHANNELS, MUSIC3_DIT_LAYERS, MUSIC3_FLOW_CFG, MUSIC3_FLOW_STEPS,
    MUSIC3_FRAME_RATE, MUSIC3_LM_HIDDEN, MUSIC3_LM_LAYERS, MUSIC3_LM_VOCAB, MUSIC3_NUM_CODEBOOKS,
    MUSIC3_RVQ_HIDDEN, MUSIC3_SAMPLE_RATE,
};
use makepad_diffusion::music3_ar::{
    music3_ar_emitted_frames, music3_ar_replay, music3_ar_sample,
};
use makepad_diffusion::music3_dit::{music3_dit_evict, music3_dit_forward, Music3DitPrepared};
use makepad_diffusion::music3_lm::{
    music3_decode_attn_from_qkv, music3_down_from_swiglu, music3_embed_audio_frame,
    music3_l0_attn_from_qkv, music3_layer_prefill_pair,
    music3_lm_evict, music3_lm_head, music3_lm_prefill_pair, music3_mlp_from_attn_resid,
    music3_mlp_from_post_norm, Music3LmPrepared, Music3LmSession, Music3MlpDump,
};
use makepad_diffusion::music3_pipeline::{
    music3_generate, music3_planar_stereo, music3_render_hiddens, Music3Generate,
};
use makepad_diffusion::music3_rvq::{
    music3_rvq_audio_head_rows, music3_rvq_evict, music3_rvq_forward, music3_rvq_forward_pair,
    music3_rvq_project_rows, Music3RvqPrepared,
};
use makepad_diffusion::music3_vocoder::Music3Vocoder;
use makepad_diffusion::music3_weights::Music3Shards;
use std::path::Path;
use std::time::Instant;

struct Npy {
    shape: Vec<usize>,
    descr: String,
    fortran_order: bool,
    data: Vec<u8>,
}

fn load_npy(path: &Path) -> Result<Npy, String> {
    let bytes = std::fs::read(path).map_err(|err| format!("{}: {err}", path.display()))?;
    if bytes.len() < 10 || &bytes[..6] != b"\x93NUMPY" {
        return Err(format!("{}: not an npy file", path.display()));
    }
    let major = bytes[6];
    let (header_len, header_start) = if major == 1 {
        (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10usize)
    } else {
        (
            u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
            12usize,
        )
    };
    let header =
        String::from_utf8_lossy(&bytes[header_start..header_start + header_len]).to_string();
    let descr = header
        .split("'descr':")
        .nth(1)
        .and_then(|rest| rest.split('\'').nth(1))
        .ok_or_else(|| format!("{}: no descr", path.display()))?
        .to_string();
    let shape_text = header
        .split("'shape':")
        .nth(1)
        .and_then(|rest| rest.split('(').nth(1))
        .and_then(|rest| rest.split(')').next())
        .ok_or_else(|| format!("{}: no shape", path.display()))?;
    let shape: Vec<usize> = shape_text
        .split(',')
        .filter_map(|part| part.trim().parse::<usize>().ok())
        .collect();
    let fortran_order = header.contains("'fortran_order': True")
        || header.contains("'fortran_order':True");
    Ok(Npy {
        shape,
        descr,
        fortran_order,
        data: bytes[header_start + header_len..].to_vec(),
    })
}

impl Npy {
    fn as_i64(&self) -> Result<Vec<i64>, String> {
        let raw: Vec<i64> = match self.descr.as_str() {
            "<i8" => self
                .data
                .chunks_exact(8)
                .map(|c| i64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]))
                .collect(),
            "<i4" => self
                .data
                .chunks_exact(4)
                .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]) as i64)
                .collect(),
            other => return Err(format!("npy descr {other} is not integer")),
        };
        Ok(if self.fortran_order {
            reorder_fortran(&raw, &self.shape)
        } else {
            raw
        })
    }

    fn as_f32(&self) -> Result<Vec<f32>, String> {
        let raw: Vec<f32> = match self.descr.as_str() {
            "<f4" => self
                .data
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect(),
            "<f8" => self
                .data
                .chunks_exact(8)
                .map(|c| {
                    f64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]) as f32
                })
                .collect(),
            other => return Err(format!("npy descr {other} not f32-convertible")),
        };
        Ok(if self.fortran_order {
            reorder_fortran(&raw, &self.shape)
        } else {
            raw
        })
    }
}

fn reorder_fortran<T: Copy + Default>(data: &[T], shape: &[usize]) -> Vec<T> {
    let n = data.len();
    if shape.is_empty() || n == 0 {
        return data.to_vec();
    }
    let mut out = vec![T::default(); n];
    for c_idx in 0..n {
        let mut rest = c_idx;
        let mut coords = vec![0usize; shape.len()];
        for d in (0..shape.len()).rev() {
            coords[d] = rest % shape[d];
            rest /= shape[d];
        }
        let mut f_idx = 0usize;
        let mut stride = 1usize;
        for d in 0..shape.len() {
            f_idx += coords[d] * stride;
            stride *= shape[d];
        }
        out[c_idx] = data[f_idx];
    }
    out
}

fn compare(ours: &[f32], reference: &[f32]) -> Result<(f64, f32, f32), String> {
    if ours.len() != reference.len() {
        return Err(format!(
            "length mismatch ours={} ref={}",
            ours.len(),
            reference.len()
        ));
    }
    let mut dot = 0f64;
    let mut na = 0f64;
    let mut nb = 0f64;
    let mut max_abs = 0f32;
    let mut sum_abs = 0f64;
    for (&a, &b) in ours.iter().zip(reference) {
        dot += a as f64 * b as f64;
        na += a as f64 * a as f64;
        nb += b as f64 * b as f64;
        let d = (a - b).abs();
        if d > max_abs {
            max_abs = d;
        }
        sum_abs += d as f64;
    }
    let cos = dot / (na.sqrt() * nb.sqrt()).max(1e-30);
    Ok((cos, max_abs, (sum_abs / ours.len() as f64) as f32))
}

fn write_npy_f32_val(path: &str, data: &[f32], shape: &[usize]) -> Result<(), String> {
    use std::io::Write;
    let shape_txt = if shape.len() == 1 {
        format!("({},)", shape[0])
    } else {
        format!(
            "({})",
            shape
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let mut dict = format!(
        "{{'descr': '<f4', 'fortran_order': False, 'shape': {shape_txt}, }}"
    );
    let prefix = 10usize;
    let mut header_len = dict.len() + 1;
    let pad = (16 - ((prefix + header_len) % 16)) % 16;
    header_len += pad;
    dict.push_str(&" ".repeat(pad));
    dict.push('\n');
    let mut f = std::fs::File::create(path).map_err(|e| e.to_string())?;
    f.write_all(b"\x93NUMPY\x01\x00").map_err(|e| e.to_string())?;
    f.write_all(&(header_len as u16).to_le_bytes())
        .map_err(|e| e.to_string())?;
    f.write_all(dict.as_bytes()).map_err(|e| e.to_string())?;
    for v in data {
        f.write_all(&v.to_le_bytes()).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut opts = std::collections::HashMap::new();
    let mut i = 1;
    while i < args.len() {
        if let Some(key) = args[i].strip_prefix("--") {
            if i + 1 < args.len() && !args[i + 1].starts_with("--") {
                opts.insert(key.to_string(), args[i + 1].clone());
                i += 2;
                continue;
            }
            opts.insert(key.to_string(), String::new());
        }
        i += 1;
    }
    let weights = opts.get("weights").cloned().unwrap_or_else(|| {
        r"C:\ai\asset_node_cache\music\MiniMax-Music3".to_string()
    });
    let dump = opts.get("dump").cloned();
    let stage = opts.get("stage").cloned().unwrap_or_else(|| "all".to_string());
    if let Err(err) = run(Path::new(&weights), dump.as_deref().map(Path::new), &stage) {
        eprintln!("music3-validate FAILED: {err}");
        std::process::exit(1);
    }
}

fn run(weights: &Path, dump: Option<&Path>, stage: &str) -> Result<(), String> {
    let all = stage == "all";
    println!("music3-validate weights={}", weights.display());
    println!(
        "  contract sr={MUSIC3_SAMPLE_RATE} fps={MUSIC3_FRAME_RATE} flow_steps={MUSIC3_FLOW_STEPS} flow_cfg={MUSIC3_FLOW_CFG} lm_layers={MUSIC3_LM_LAYERS} dit_layers={MUSIC3_DIT_LAYERS}"
    );
    println!(
        "  tokens audio_end={MUSIC3_AUDIO_END_TOKEN_ID} audio_cfg={MUSIC3_AUDIO_CFG_TOKEN_ID} audio_code_offset={MUSIC3_AUDIO_CODE_OFFSET}"
    );

    if all || stage == "inventory" {
        println!("== inventory ==");
        let inv = inventory_weights(weights).map_err(|err| err.to_string())?;
        println!("  shards={} tensors={}", inv.files.len(), inv.tensors.len());
        let canaries = validate_shape_canaries(&inv).map_err(|err| err.to_string())?;
        for line in &canaries {
            println!("  ok {line}");
        }
        println!("  inventory PASS ({} canaries)", canaries.len());
    }

    if all || stage == "tokenize" {
        match dump {
            None if stage == "tokenize" => return Err("--dump is required for tokenize".into()),
            None => println!("== tokenize == skipped (no --dump)"),
            Some(dump) => {
                println!("== tokenize ==");
                run_tokenize(weights, dump)?;
            }
        }
    }

    if all || stage == "cond" {
        match dump {
            None if stage == "cond" => return Err("--dump is required for cond".into()),
            None => println!("== cond == skipped (no --dump)"),
            Some(dump) => {
                println!("== cond ==");
                run_cond(weights, dump)?;
            }
        }
    }

    if all || stage == "lm" {
        match dump {
            None if stage == "lm" => return Err("--dump is required for lm".into()),
            None => println!("== lm == skipped (no --dump)"),
            Some(dump) => {
                println!("== lm ==");
                run_lm(weights, dump)?;
            }
        }
    }

    if all || stage == "rvq" {
        match dump {
            None if stage == "rvq" => return Err("--dump is required for rvq".into()),
            None => println!("== rvq == skipped (no --dump)"),
            Some(dump) => {
                println!("== rvq ==");
                run_rvq(weights, dump)?;
            }
        }
    }

    if all || stage == "dit" {
        match dump {
            None if stage == "dit" => return Err("--dump is required for dit".into()),
            None => println!("== dit == skipped (no --dump)"),
            Some(dump) => {
                println!("== dit ==");
                run_dit(weights, dump)?;
            }
        }
    }

    if all || stage == "vocoder" {
        match dump {
            None if stage == "vocoder" => return Err("--dump is required for vocoder".into()),
            None => println!("== vocoder == skipped (no --dump)"),
            Some(dump) => {
                println!("== vocoder ==");
                run_vocoder(weights, dump)?;
            }
        }
    }

    if stage == "ar" {
        let dump = dump.ok_or("--dump is required for ar")?;
        println!("== ar ==");
        run_ar(weights, dump)?;
    }
    if stage == "sample" {
        let dump = dump.ok_or("--dump is required for sample")?;
        println!("== sample ==");
        run_sample(weights, dump)?;
    }
    if stage == "teacher" {
        let dump = dump.ok_or("--dump is required for teacher")?;
        println!("== teacher ==");
        run_teacher(weights, dump)?;
    }
    if stage == "decode1" {
        let dump = dump.ok_or("--dump is required for decode1")?;
        println!("== decode1 ==");
        run_decode1(weights, dump)?;
    }
    if stage == "l0mlp" {
        println!("== l0mlp ==");
        run_l0mlp(weights)?;
    }
    if stage == "layer1" {
        println!("== layer1 ==");
        run_layer1(weights)?;
    }
    if stage == "layer1mlp" {
        println!("== layer1mlp ==");
        run_layer1mlp(weights)?;
    }
    if stage == "layer2" {
        println!("== layer2 ==");
        run_layer_n(weights, 2)?;
    }
    if stage == "l0attn" {
        println!("== l0attn ==");
        run_l0attn_offin(weights)?;
    }
    if stage == "decodeattn" {
        println!("== decodeattn ==");
        run_decodeattn(weights)?;
    }
    if stage == "layers" {
        let dump = dump.ok_or("--dump is required for layers")?;
        println!("== layers ==");
        run_prefill_layers(weights, dump)?;
    }
    if stage == "rvqf12" {
        println!("== rvqf12 ==");
        run_rvq_f12(weights)?;
    }
    if stage == "replaywav" {
        let dump = dump.ok_or("--dump is required for replaywav (text_ids.npy)")?;
        let argv: Vec<String> = std::env::args().collect();
        let get = |k: &str| argv.windows(2).find(|w| w[0] == k).map(|w| w[1].clone());
        let sem = get("--sem").ok_or("--sem <semantic codes npy> required")?;
        let rvq = get("--rvqcodes").ok_or("--rvqcodes <rvq codes npy> required")?;
        let out = get("--out")
            .unwrap_or_else(|| r"C:\ai\music3_compare\replay_codes.wav".into());
        let seed: u64 = get("--seed").and_then(|v| v.parse().ok()).unwrap_or(7);
        println!("== replaywav seed={seed} sem={sem} rvq={rvq} -> {out} ==");
        run_replay_wav(
            weights,
            dump,
            Path::new(&sem),
            Path::new(&rvq),
            seed,
            Path::new(&out),
        )?;
    }
    if stage == "decodepair" {
        println!("== decodepair ==");
        run_decode_pair_check()?;
    }
    if stage == "generate" {
        let seconds: f64 = std::env::args()
            .collect::<Vec<_>>()
            .windows(2)
            .find(|w| w[0] == "--seconds")
            .and_then(|w| w[1].parse().ok())
            .unwrap_or(5.0);
        let out = std::env::args()
            .collect::<Vec<_>>()
            .windows(2)
            .find(|w| w[0] == "--out")
            .map(|w| w[1].clone())
            .unwrap_or_else(|| r"C:\ai\music3_compare\native_good_5s.wav".into());
        let seed: u64 = std::env::args()
            .collect::<Vec<_>>()
            .windows(2)
            .find(|w| w[0] == "--seed")
            .and_then(|w| w[1].parse().ok())
            .unwrap_or(7);
        let caption = if let Some(path) = std::env::args()
            .collect::<Vec<_>>()
            .windows(2)
            .find(|w| w[0] == "--caption-file")
            .map(|w| w[1].clone())
        {
            std::fs::read_to_string(&path).map_err(|e| format!("--caption-file {path}: {e}"))?
        } else {
            std::env::args()
                .collect::<Vec<_>>()
                .windows(2)
                .find(|w| w[0] == "--caption")
                .map(|w| w[1].clone())
                .unwrap_or_else(|| "a classical piece of music".into())
        };
        let lyrics = if let Some(path) = std::env::args()
            .collect::<Vec<_>>()
            .windows(2)
            .find(|w| w[0] == "--lyrics-file")
            .map(|w| w[1].clone())
        {
            std::fs::read_to_string(&path).map_err(|e| format!("--lyrics-file {path}: {e}"))?
        } else {
            std::env::args()
                .collect::<Vec<_>>()
                .windows(2)
                .find(|w| w[0] == "--lyrics")
                .map(|w| w[1].clone())
                .unwrap_or_else(|| "[Instrumental]".into())
        };
        println!(
            "== generate {seconds}s seed={seed} caption={:?} lyrics_chars={} -> {out} ==",
            caption,
            lyrics.chars().count()
        );
        run_generate(
            weights,
            seconds,
            seed,
            Path::new(&out),
            &caption,
            &lyrics,
        )?;
    }
    println!("music3-validate PASS");
    Ok(())
}

/// Byte-identity + timing check: parallel pair decode kernel vs the serial
/// GQA kernel on the row-concatenated caches. Music3 LM dims, several KV
/// lengths spanning the 611-token lyric prefix through a 60 s song tail.
fn run_decode_pair_check() -> Result<(), String> {
    use makepad_diffusion::backend::{
        gpu_attention_gqa_decode_bf16, gpu_attention_gqa_decode_pair_bf16, gpu_concat_rows,
        gpu_download, gpu_slice_rows, gpu_upload,
    };
    let query_heads = 32usize;
    let kv_heads = 8usize;
    let head_dim = 128usize;
    let kv_width = kv_heads * head_dim;
    let scale = 1.0 / (head_dim as f32).sqrt();
    let mut state = 0x1234_5678_9abc_def0u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        ((state >> 11) as f64 / (1u64 << 53) as f64) as f32 * 2.0 - 1.0
    };
    for &seq in &[3usize, 612, 1361, 2112] {
        let cap = seq + 7;
        let mut gen = |n: usize| (0..n).map(|_| next()).collect::<Vec<f32>>();
        let q = gpu_upload(&gen(2 * query_heads * head_dim), 2, query_heads * head_dim)?;
        let k_cond = gpu_upload(&gen(cap * kv_width), cap, kv_width)?;
        let v_cond = gpu_upload(&gen(cap * kv_width), cap, kv_width)?;
        let k_uncond = gpu_upload(&gen(cap * kv_width), cap, kv_width)?;
        let v_uncond = gpu_upload(&gen(cap * kv_width), cap, kv_width)?;
        let k = gpu_concat_rows(
            &gpu_slice_rows(&k_cond, 0, seq)?,
            &gpu_slice_rows(&k_uncond, 0, seq)?,
        )?;
        let v = gpu_concat_rows(
            &gpu_slice_rows(&v_cond, 0, seq)?,
            &gpu_slice_rows(&v_uncond, 0, seq)?,
        )?;
        let reference = gpu_attention_gqa_decode_bf16(&q, &k, &v, query_heads, kv_heads, scale)?;
        let fast = gpu_attention_gqa_decode_pair_bf16(
            &q, &k_cond, &v_cond, &k_uncond, &v_uncond, seq, query_heads, kv_heads, scale,
        )?;
        let a = gpu_download(&reference)?;
        let b = gpu_download(&fast)?;
        if a.len() != b.len() {
            return Err(format!("decodepair seq={seq}: len {} vs {}", a.len(), b.len()));
        }
        let mut mismatches = 0usize;
        let mut first: Option<usize> = None;
        for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
            if x.to_bits() != y.to_bits() {
                mismatches += 1;
                if first.is_none() {
                    first = Some(i);
                }
            }
        }
        if let Some(i) = first {
            return Err(format!(
                "decodepair seq={seq}: {mismatches} bit mismatches, first at {i}: {} ({:#010x}) vs {} ({:#010x})",
                a[i],
                a[i].to_bits(),
                b[i],
                b[i].to_bits()
            ));
        }
        // Timing: serial kernel (includes its per-frame concat) vs pair kernel.
        let runs = 30usize;
        let t0 = Instant::now();
        for _ in 0..runs {
            let k = gpu_concat_rows(
                &gpu_slice_rows(&k_cond, 0, seq)?,
                &gpu_slice_rows(&k_uncond, 0, seq)?,
            )?;
            let v = gpu_concat_rows(
                &gpu_slice_rows(&v_cond, 0, seq)?,
                &gpu_slice_rows(&v_uncond, 0, seq)?,
            )?;
            let out = gpu_attention_gqa_decode_bf16(&q, &k, &v, query_heads, kv_heads, scale)?;
            gpu_download(&out)?;
        }
        let serial_ms = t0.elapsed().as_secs_f64() * 1000.0 / runs as f64;
        let t1 = Instant::now();
        for _ in 0..runs {
            let out = gpu_attention_gqa_decode_pair_bf16(
                &q, &k_cond, &v_cond, &k_uncond, &v_uncond, seq, query_heads, kv_heads, scale,
            )?;
            gpu_download(&out)?;
        }
        let pair_ms = t1.elapsed().as_secs_f64() * 1000.0 / runs as f64;
        // Burst: back-to-back pair launches, one final sync — separates raw
        // kernel throughput from the per-call download sync above.
        let t2 = Instant::now();
        let mut last = None;
        for _ in 0..runs {
            last = Some(gpu_attention_gqa_decode_pair_bf16(
                &q, &k_cond, &v_cond, &k_uncond, &v_uncond, seq, query_heads, kv_heads, scale,
            )?);
        }
        if let Some(out) = last.take() {
            gpu_download(&out)?;
        }
        let burst_ms = t2.elapsed().as_secs_f64() * 1000.0 / runs as f64;
        println!(
            "  seq={seq} BIT-IDENTICAL ({} floats)  serial {serial_ms:.3} ms  pair {pair_ms:.3} ms  ({:.1}x)  burst {burst_ms:.3} ms",
            a.len(),
            serial_ms / pair_ms.max(1e-9)
        );
    }
    println!("  decodepair PASS");
    Ok(())
}

fn run_tokenize(weights: &Path, dump: &Path) -> Result<(), String> {
    let meta_path = dump.join("meta.json");
    let meta_text = std::fs::read_to_string(&meta_path)
        .map_err(|err| format!("{}: {err}", meta_path.display()))?;
    let prompt = json_string(&meta_text, "prompt")
        .ok_or_else(|| format!("{}: no prompt", meta_path.display()))?;
    let lyrics = json_string(&meta_text, "lyrics")
        .ok_or_else(|| format!("{}: no lyrics", meta_path.display()))?;
    let assembled_ref = json_string(&meta_text, "assembled_prompt");

    let assembled = assemble_prompt(&prompt, &lyrics);
    if let Some(expected) = assembled_ref {
        if assembled != expected {
            return Err(format!(
                "assembled prompt mismatch\nours:\n{assembled}\nref:\n{expected}"
            ));
        }
        println!("  assembled prompt matches ({} chars)", assembled.len());
    } else {
        println!("  assembled (no dump text): {} chars", assembled.len());
    }

    let tokenizer = load_tokenizer(&weights.join("tokenizer")).map_err(|err| err.to_string())?;
    let pairs = tokenize_cfg_pair(&tokenizer, &prompt, &lyrics).map_err(|err| err.to_string())?;
    let ref_ids = load_npy(&dump.join("text_ids.npy"))?;
    let ref_vals = ref_ids.as_i64()?;
    // dump is [2, T] row-major: cond row then uncond row.
    if ref_ids.shape.len() != 2 || ref_ids.shape[0] != 2 {
        return Err(format!("text_ids shape {:?}, expected [2, T]", ref_ids.shape));
    }
    let t = ref_ids.shape[1];
    if pairs.len() != t {
        return Err(format!("token length ours={} dump={t}", pairs.len()));
    }
    let mut mismatches = 0usize;
    for i in 0..t {
        let cond = ref_vals[i];
        let uncond = ref_vals[t + i];
        if pairs[i][0] as i64 != cond || pairs[i][1] as i64 != uncond {
            if mismatches < 8 {
                println!(
                    "  mismatch i={i} ours=[{}, {}] dump=[{cond}, {uncond}]",
                    pairs[i][0], pairs[i][1]
                );
            }
            mismatches += 1;
        }
    }
    if mismatches > 0 {
        return Err(format!("tokenize: {mismatches}/{t} tokens differ"));
    }
    println!("  text_ids [2, {t}] exact match");
    println!("  tokenize PASS");
    Ok(())
}

fn run_cond(weights: &Path, dump: &Path) -> Result<(), String> {
    let enc = Music3ConditionEncoder::load(weights).map_err(|err| err.to_string())?;
    let input = load_npy(&dump.join("cond_enc_in.npy"))?;
    let reference = load_npy(&dump.join("cond_enc_out.npy"))?;
    // dump is (1, frames, layers*hidden)
    if input.shape.len() != 3 || input.shape[0] != 1 {
        return Err(format!("cond_enc_in shape {:?}, expected [1, F, 32768]", input.shape));
    }
    let frames = input.shape[1];
    if input.shape[2] != MUSIC3_COND_LAYERS * MUSIC3_COND_HIDDEN {
        return Err(format!(
            "cond_enc_in last dim {}, expected {}",
            input.shape[2],
            MUSIC3_COND_LAYERS * MUSIC3_COND_HIDDEN
        ));
    }
    let hidden = input.as_f32()?;
    let ours = enc.forward(&hidden, frames).map_err(|err| err.to_string())?;
    let ref_vals = reference.as_f32()?;
    if reference.shape != [1, ours.len() / MUSIC3_COND_OUT, MUSIC3_COND_OUT] {
        // dump is (1, L, 2048)
        if reference.shape.len() != 3 || reference.shape[2] != MUSIC3_COND_OUT {
            return Err(format!("cond_enc_out shape {:?}, expected [1, L, 2048]", reference.shape));
        }
        let latents = reference.shape[1];
        if ours.len() != latents * MUSIC3_COND_OUT {
            return Err(format!(
                "cond encoder out {} values, dump latents={latents}",
                ours.len()
            ));
        }
    }
    let (cos, max_abs, mean_abs) = compare(&ours, &ref_vals)?;
    println!(
        "  cond_enc_out [{frames} frames -> {} latents x {MUSIC3_COND_OUT}] cos={cos:.7} max_abs={max_abs:.3e} mean_abs={mean_abs:.3e}",
        ours.len() / MUSIC3_COND_OUT
    );
    if cos < 0.999 || max_abs > 0.25 {
        return Err(format!(
            "cond encoder mismatch cos={cos:.7} max_abs={max_abs:.3e}"
        ));
    }
    println!("  cond PASS");
    Ok(())
}

fn report_first_sample_logits(dump: &Path, logits: &[f32]) -> Result<(), String> {
    if logits.len() != 2 * MUSIC3_LM_VOCAB {
        return Err(format!("first_sample logits {}", logits.len()));
    }
    let cond_logits = &logits[..MUSIC3_LM_VOCAB];
    let uncond_logits = &logits[MUSIC3_LM_VOCAB..];
    let lo = MUSIC3_AUDIO_CODE_OFFSET as usize;
    let hi = lo + MUSIC3_SEMANTIC_VOCAB;
    let end = MUSIC3_AUDIO_END_TOKEN_ID as usize;
    let mut cond_m = cond_logits.to_vec();
    let mut uncond_m = uncond_logits.to_vec();
    for (i, v) in cond_m.iter_mut().enumerate() {
        if i != end && !(i >= lo && i < hi) {
            *v = f32::NEG_INFINITY;
        }
    }
    for (i, v) in uncond_m.iter_mut().enumerate() {
        if i != end && !(i >= lo && i < hi) {
            *v = f32::NEG_INFINITY;
        }
    }
    let mut cond_fin: Vec<f32> = cond_m.iter().copied().filter(|v| v.is_finite()).collect();
    cond_fin.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    let thresh = cond_fin
        .get(MUSIC3_AR_TOP_K.min(cond_fin.len()).saturating_sub(1))
        .copied()
        .unwrap_or(f32::NEG_INFINITY);
    let mut guided = vec![0f32; MUSIC3_LM_VOCAB];
    for i in 0..MUSIC3_LM_VOCAB {
        let g = uncond_m[i] + MUSIC3_AR_CFG * (cond_m[i] - uncond_m[i]);
        guided[i] = if cond_m[i] < thresh || !g.is_finite() {
            f32::NEG_INFINITY
        } else {
            g
        };
    }
    let _ = write_npy_f32_val(
        r"C:\ai\music3_compare\native_first_sample_logits.npy",
        &guided,
        &[1, MUSIC3_LM_VOCAB],
    );
    let href = load_npy(&dump.join("first_sample_logits.npy"))?;
    let href_v = href.as_f32()?;
    if href_v.len() != MUSIC3_LM_VOCAB {
        return Err(format!("first_sample_logits dump {}", href_v.len()));
    }
    let mut ours_fin = Vec::new();
    let mut ref_fin = Vec::new();
    let mut both = 0usize;
    let mut best_ours = (0usize, f32::NEG_INFINITY);
    let mut best_ref = (0usize, f32::NEG_INFINITY);
    for i in 0..MUSIC3_LM_VOCAB {
        let a = guided[i];
        let b = href_v[i];
        if a.is_finite() && a > best_ours.1 {
            best_ours = (i, a);
        }
        if b.is_finite() && b > best_ref.1 {
            best_ref = (i, b);
        }
        if a.is_finite() && b.is_finite() {
            both += 1;
            ours_fin.push(a);
            ref_fin.push(b);
        }
    }
    let (cos, max_abs, mean_abs) = if ours_fin.is_empty() {
        (0.0, 0.0, 0.0)
    } else {
        compare(&ours_fin, &ref_fin)?
    };
    println!(
        "  first_sample_logits finite_overlap={both} cos={cos:.7} max_abs={max_abs:.3e} mean_abs={mean_abs:.3e}"
    );
    println!(
        "  first_sample argmax ours={} ({:.4}) dump={} ({:.4})",
        best_ours.0, best_ours.1, best_ref.0, best_ref.1
    );
    Ok(())
}

fn run_lm(weights: &Path, dump: &Path) -> Result<(), String> {
    let ids = load_npy(&dump.join("text_ids.npy"))?;
    if ids.shape.len() != 2 || ids.shape[0] != 2 {
        return Err(format!("text_ids shape {:?}, expected [2, T]", ids.shape));
    }
    let t = ids.shape[1];
    let vals = ids.as_i64()?;
    let cond: Vec<u32> = vals[..t].iter().map(|&v| v as u32).collect();
    let uncond: Vec<u32> = vals[t..].iter().map(|&v| v as u32).collect();

    let shards = Music3Shards::load(weights.join("language_model")).map_err(|err| err.to_string())?;
    let prepared = Music3LmPrepared::prepare(&shards).map_err(|err| err.to_string())?;
    let started = Instant::now();
    let (hidden, logits) = music3_lm_prefill_pair(&shards, &prepared, &cond, &uncond)
        .map_err(|err| err.to_string())?;
    let elapsed = started.elapsed();
    println!("  prefill wall {:.3}s", elapsed.as_secs_f64());

    let href = load_npy(&dump.join("lm_prefill_last_hidden.npy"))?;
    let lref = load_npy(&dump.join("lm_prefill_logits.npy"))?;
    let href_v = href.as_f32()?;
    let lref_v = lref.as_f32()?;
    if hidden.len() != 2 * t * MUSIC3_LM_HIDDEN {
        return Err(format!(
            "hidden {} expected {}",
            hidden.len(),
            2 * t * MUSIC3_LM_HIDDEN
        ));
    }
    if href_v.len() != hidden.len() {
        return Err(format!(
            "dump hidden {} ours {}",
            href_v.len(),
            hidden.len()
        ));
    }
    if lref_v.len() != 2 * MUSIC3_LM_VOCAB || logits.len() != 2 * MUSIC3_LM_VOCAB {
        return Err(format!(
            "logits ours {} dump {} expected {}",
            logits.len(),
            lref_v.len(),
            2 * MUSIC3_LM_VOCAB
        ));
    }
    let (hcos, hmax, hmean) = compare(&hidden, &href_v)?;
    let (lcos, lmax, lmean) = compare(&logits, &lref_v)?;
    println!(
        "  last_hidden [2, {t}, {MUSIC3_LM_HIDDEN}] cos={hcos:.7} max_abs={hmax:.3e} mean_abs={hmean:.3e}"
    );
    for row in 0..t {
        let a = &hidden[row * MUSIC3_LM_HIDDEN..(row + 1) * MUSIC3_LM_HIDDEN];
        let b = &href_v[row * MUSIC3_LM_HIDDEN..(row + 1) * MUSIC3_LM_HIDDEN];
        let (cos, mx, mean) = compare(a, b)?;
        let ua = &hidden[(t + row) * MUSIC3_LM_HIDDEN..(t + row + 1) * MUSIC3_LM_HIDDEN];
        let ub = &href_v[(t + row) * MUSIC3_LM_HIDDEN..(t + row + 1) * MUSIC3_LM_HIDDEN];
        let (_, umx, _) = compare(ua, ub)?;
        if row == 0 || row + 1 == t || mx > 0.3 || umx > 0.3 {
            println!("  pos{row} cond_cos={cos:.6} cond_maxabs={mx:.4} unc_maxabs={umx:.4}");
        }
    }
    println!(
        "  logits [2, {MUSIC3_LM_VOCAB}] cos={lcos:.7} max_abs={lmax:.3e} mean_abs={lmean:.3e}"
    );
    let _ = write_npy_f32_val(
        r"C:\ai\music3_compare\native_fullseq_last_hidden.npy",
        &hidden,
        &[2, t, MUSIC3_LM_HIDDEN],
    );
    let _ = write_npy_f32_val(
        r"C:\ai\music3_compare\native_prefill_logits.npy",
        &logits,
        &[2, MUSIC3_LM_VOCAB],
    );
    report_first_sample_logits(dump, &logits)?;
    // Last-token pair [2, 4096] vs official SDPA dump (the token-flip gate).
    let last_off = (t - 1) * MUSIC3_LM_HIDDEN;
    let mut last_pair = Vec::with_capacity(2 * MUSIC3_LM_HIDDEN);
    last_pair.extend_from_slice(&hidden[last_off..last_off + MUSIC3_LM_HIDDEN]);
    last_pair.extend_from_slice(
        &hidden[t * MUSIC3_LM_HIDDEN + last_off..t * MUSIC3_LM_HIDDEN + last_off + MUSIC3_LM_HIDDEN],
    );
    for name in [
        r"C:\ai\music3_compare\official_last_hidden_f0_sdpa.npy",
        r"C:\ai\music3_compare\official_last_hidden_f0.npy",
    ] {
        let path = Path::new(name);
        if !path.exists() {
            continue;
        }
        match load_npy(path).and_then(|n| n.as_f32()) {
            Ok(off) if off.len() == last_pair.len() => {
                let (cos, max_abs, mean) = compare(&last_pair, &off)?;
                println!(
                    "  f0_vs_{} cos={cos:.8} maxabs={max_abs:.6} mean={mean:.6}",
                    path.file_stem().unwrap_or_default().to_string_lossy()
                );
            }
            Ok(off) => println!("  skip {} len {}", name, off.len()),
            Err(err) => println!("  skip {name}: {err}"),
        }
    }
    let _ = music3_lm_evict();
    if hcos < 0.999 || lcos < 0.999 {
        return Err(format!(
            "lm prefill mismatch hidden_cos={hcos:.7} logits_cos={lcos:.7}"
        ));
    }
    println!("  lm PASS");
    Ok(())
}

fn run_prefill_layers(weights: &Path, dump: &Path) -> Result<(), String> {
    let ids = load_npy(&dump.join("text_ids.npy"))?;
    if ids.shape.len() != 2 || ids.shape[0] != 2 {
        return Err(format!("text_ids shape {:?}, expected [2, T]", ids.shape));
    }
    let t = ids.shape[1];
    let vals = ids.as_i64()?;
    let cond: Vec<u32> = vals[..t].iter().map(|&v| v as u32).collect();
    let uncond: Vec<u32> = vals[t..].iter().map(|&v| v as u32).collect();
    if std::env::var_os("MAKEPAD_MUSIC3_DUMP_PREFILL_LAYERS").is_none() {
        std::env::set_var(
            "MAKEPAD_MUSIC3_DUMP_PREFILL_LAYERS",
            r"C:\ai\music3_compare",
        );
    }
    let shards = Music3Shards::load(weights.join("language_model")).map_err(|err| err.to_string())?;
    let prepared = Music3LmPrepared::prepare(&shards).map_err(|err| err.to_string())?;
    let started = Instant::now();
    let (_c, last_c, _u, last_u) = Music3LmSession::prefill_pair_with_progress(
        &shards,
        &prepared,
        &cond,
        &uncond,
        &mut |done, total| {
            if done == 0 || done == total || done % 8 == 0 {
                eprintln!("prefill_layers {done}/{total}");
            }
        },
    )
    .map_err(|err| err.to_string())?;
    println!(
        "  pair prefill wall {:.3}s last_c={} last_u={}",
        started.elapsed().as_secs_f64(),
        last_c.len(),
        last_u.len()
    );
    let _ = music3_lm_evict();
    println!("  layers PASS");
    Ok(())
}

fn run_rvq(weights: &Path, dump: &Path) -> Result<(), String> {
    let inn = load_npy(&dump.join("rvq_step0_in.npy"))?;
    let out = load_npy(&dump.join("rvq_step0_out.npy"))?;
    if inn.shape.len() != 3 || inn.shape[2] != MUSIC3_RVQ_HIDDEN {
        return Err(format!("rvq_step0_in shape {:?}", inn.shape));
    }
    let batch = inn.shape[0];
    let seq = inn.shape[1];
    let inn_v = inn.as_f32()?;
    let out_v = out.as_f32()?;
    let shards = Music3Shards::load(weights.join("rvq_depth_decoder")).map_err(|e| e.to_string())?;
    let prepared = Music3RvqPrepared::prepare(&shards).map_err(|e| e.to_string())?;
    let mut ours = Vec::with_capacity(batch * seq * MUSIC3_RVQ_HIDDEN);
    let t0 = Instant::now();
    for b in 0..batch {
        let start = b * seq * MUSIC3_RVQ_HIDDEN;
        let y = music3_rvq_forward(
            &shards,
            &prepared,
            &inn_v[start..start + seq * MUSIC3_RVQ_HIDDEN],
            seq,
        )
        .map_err(|e| e.to_string())?;
        ours.extend_from_slice(&y);
    }
    println!("  rvq wall {:.3}s", t0.elapsed().as_secs_f64());
    let (cos, max_abs, mean_abs) = compare(&ours, &out_v)?;
    println!(
        "  rvq_step0 [{batch}, {seq}, {MUSIC3_RVQ_HIDDEN}] cos={cos:.7} max_abs={max_abs:.3e} mean_abs={mean_abs:.3e}"
    );
    let _ = music3_rvq_evict();
    if cos < 0.999 {
        return Err(format!("rvq mismatch cos={cos:.7} max_abs={max_abs:.3e}"));
    }
    println!("  rvq PASS");
    Ok(())
}

fn run_dit(weights: &Path, dump: &Path) -> Result<(), String> {
    let x = load_npy(&dump.join("dit_step0_x.npy"))?;
    let cond = load_npy(&dump.join("dit_step0_cond.npy"))?;
    let v = load_npy(&dump.join("dit_step0_v_cond.npy"))?;
    let t = load_npy(&dump.join("dit_step0_t.npy"))?.as_f32()?;
    let timestep = t.first().copied().unwrap_or(0.0);
    if x.shape.len() != 3 || x.shape[1] != MUSIC3_DIT_IN_CHANNELS {
        return Err(format!("dit_step0_x shape {:?}", x.shape));
    }
    let tokens = x.shape[2];
    let shards = Music3Shards::load(weights.join("transformer")).map_err(|e| e.to_string())?;
    let prepared = Music3DitPrepared::prepare(&shards).map_err(|e| e.to_string())?;
    let t0 = Instant::now();
    let ours = music3_dit_forward(
        &shards,
        &prepared,
        &x.as_f32()?,
        &cond.as_f32()?,
        tokens,
        timestep,
    )
    .map_err(|e| e.to_string())?;
    println!("  dit step0 wall {:.3}s t={timestep}", t0.elapsed().as_secs_f64());
    let (cos, max_abs, mean_abs) = compare(&ours, &v.as_f32()?)?;
    println!(
        "  dit_step0_v_cond [1, {MUSIC3_DIT_IN_CHANNELS}, {tokens}] cos={cos:.7} max_abs={max_abs:.3e} mean_abs={mean_abs:.3e}"
    );
    let _ = music3_dit_evict();
    if cos < 0.995 {
        return Err(format!("dit mismatch cos={cos:.7} max_abs={max_abs:.3e}"));
    }
    println!("  dit PASS");
    Ok(())
}

fn run_vocoder(weights: &Path, dump: &Path) -> Result<(), String> {
    let inn = load_npy(&dump.join("vocoder_in.npy"))?;
    if inn.shape.len() != 3 || inn.shape[1] != 128 {
        return Err(format!("vocoder_in shape {:?}", inn.shape));
    }
    let frames = inn.shape[2];
    let voc = Music3Vocoder::load(weights).map_err(|e| e.to_string())?;
    let t0 = Instant::now();
    let ours = voc.decode(&inn.as_f32()?, frames).map_err(|e| e.to_string())?;
    println!("  vocoder wall {:.3}s", t0.elapsed().as_secs_f64());
    let _ = write_npy_f32_val(
        r"C:\ai\music3_compare\native_vocoder_out.npy",
        &ours,
        &[2, ours.len() / 2],
    );
    let out_path = dump.join("vocoder_out.npy");
    let (ref_v, ref_src) = if out_path.exists() {
        (load_npy(&out_path)?.as_f32()?, "vocoder_out.npy")
    } else {
        // 60s dump omitted vocoder_out; first official vocoder call is the
        // first [2, frames*512] slice of audio.npy (hop=512).
        let audio_path = dump.join("audio.npy");
        if !audio_path.exists() {
            return Err(format!(
                "vocoder_out.npy missing and no audio.npy in {}",
                dump.display()
            ));
        }
        let audio = load_npy(&audio_path)?;
        let audio_v = audio.as_f32()?;
        if audio_v.len() < ours.len() {
            return Err(format!(
                "audio.npy {} shorter than vocoder out {}",
                audio_v.len(),
                ours.len()
            ));
        }
        // audio is [2, T] row-major. Compare first hop*frames samples/channel.
        let t_audio = audio_v.len() / 2;
        let t_ours = ours.len() / 2;
        let mut prefix = vec![0f32; ours.len()];
        prefix[..t_ours].copy_from_slice(&audio_v[..t_ours]);
        prefix[t_ours..].copy_from_slice(&audio_v[t_audio..t_audio + t_ours]);
        println!(
            "  vocoder_out.npy missing; compare audio.npy prefix [2, {t_ours}] of [2, {t_audio}]"
        );
        (prefix, "audio.npy-prefix")
    };
    if ours.len() != ref_v.len() {
        return Err(format!("vocoder out {} dump {}", ours.len(), ref_v.len()));
    }
    let (cos, max_abs, mean_abs) = compare(&ours, &ref_v)?;
    println!(
        "  vocoder_out [2, {}] vs {ref_src} cos={cos:.7} max_abs={max_abs:.3e} mean_abs={mean_abs:.3e}",
        ours.len() / 2
    );
    if cos < 0.999 {
        return Err(format!("vocoder mismatch cos={cos:.7} max_abs={max_abs:.3e}"));
    }
    println!("  vocoder PASS");
    Ok(())
}

fn run_ar(weights: &Path, dump: &Path) -> Result<(), String> {
    let ids = load_npy(&dump.join("text_ids.npy"))?;
    let t = ids.shape[1];
    let vals = ids.as_i64()?;
    let cond: Vec<u32> = vals[..t].iter().map(|&v| v as u32).collect();
    let sem = load_npy(&dump.join("semantic_codes.npy"))?;
    let semantic: Vec<u32> = sem.as_i64()?.iter().map(|&v| v as u32).collect();
    let rvq = load_npy(&dump.join("rvq_codes.npy"))?;
    let resid: Vec<u32> = rvq.as_i64()?.iter().map(|&v| v as u32).collect();
    if resid.len() != semantic.len() * (MUSIC3_NUM_CODEBOOKS - 1) {
        return Err(format!(
            "ar dump codes semantic={} residual={}",
            semantic.len(),
            resid.len()
        ));
    }
    let lm = Music3Shards::load(weights.join("language_model")).map_err(|e| e.to_string())?;
    let lm_prep = Music3LmPrepared::prepare(&lm).map_err(|e| e.to_string())?;
    let rvq_w = Music3Shards::load(weights.join("rvq_depth_decoder")).map_err(|e| e.to_string())?;
    let rvq_prep = Music3RvqPrepared::prepare(&rvq_w).map_err(|e| e.to_string())?;
    let t0 = Instant::now();
    let ours = music3_ar_replay(&lm, &lm_prep, &rvq_w, &rvq_prep, &cond, &semantic, &resid)
        .map_err(|e| e.to_string())?;
    let elapsed = t0.elapsed().as_secs_f64();
    let href = load_npy(&dump.join("cond_enc_in.npy"))?;
    let href_v = href.as_f32()?;
    let frames = music3_ar_emitted_frames(&ours);
    println!(
        "  ar replay wall {elapsed:.3}s emitted={frames} codes={}",
        semantic.len()
    );
    if ours.len() != href_v.len() {
        return Err(format!(
            "ar hidden {} dump {} frames ours={frames} dump={:?}",
            ours.len(),
            href_v.len(),
            href.shape
        ));
    }
    let (cos, max_abs, mean_abs) = compare(&ours, &href_v)?;
    println!(
        "  frame_hiddens [{frames}, {}] cos={cos:.7} max_abs={max_abs:.3e} mean_abs={mean_abs:.3e}",
        MUSIC3_COND_LAYERS * MUSIC3_COND_HIDDEN
    );
    let _ = music3_lm_evict();
    let _ = music3_rvq_evict();
    if cos < 0.999 {
        return Err(format!("ar mismatch cos={cos:.7} max_abs={max_abs:.3e}"));
    }
    println!("  ar PASS");
    Ok(())
}

fn run_teacher(weights: &Path, dump: &Path) -> Result<(), String> {
    use makepad_diffusion::music3::{
        MUSIC3_AR_CFG, MUSIC3_AR_TOP_K, MUSIC3_AUDIO_CODE_OFFSET, MUSIC3_AUDIO_END_TOKEN_ID,
        MUSIC3_NUM_CODEBOOKS, MUSIC3_SEMANTIC_VOCAB,
    };
    let ids = load_npy(&dump.join("text_ids.npy"))?;
    let t = ids.shape[1];
    let vals = ids.as_i64()?;
    let cond: Vec<u32> = vals[..t].iter().map(|&v| v as u32).collect();
    let uncond: Vec<u32> = vals[t..].iter().map(|&v| v as u32).collect();
    let sem = load_npy(&dump.join("semantic_codes.npy"))?.as_i64()?;
    let rvq = load_npy(&dump.join("rvq_codes.npy"))?.as_i64()?;
    let width = MUSIC3_NUM_CODEBOOKS - 1;
    let n = sem.len().min(rvq.len() / width).min(20);
    let lm = Music3Shards::load(weights.join("language_model")).map_err(|e| e.to_string())?;
    let lm_prep = Music3LmPrepared::prepare(&lm).map_err(|e| e.to_string())?;
    let rvq_w = Music3Shards::load(weights.join("rvq_depth_decoder")).map_err(|e| e.to_string())?;
    let (mut cond_s, mut cond_h, mut uncond_s, mut uncond_h) =
        Music3LmSession::prefill_pair_with_progress(
            &lm,
            &lm_prep,
            &cond,
            &uncond,
            &mut |_, _| {},
        )
        .map_err(|e| e.to_string())?;
    let lo = MUSIC3_AUDIO_CODE_OFFSET as usize;
    let hi = lo + MUSIC3_SEMANTIC_VOCAB;
    let end = MUSIC3_AUDIO_END_TOKEN_ID as usize;
    const A: usize = 155120;
    const B: usize = 156729;
    for frame in 0..n {
        let mut cond_logits = music3_lm_head(&lm, &cond_h).map_err(|e| e.to_string())?;
        let mut uncond_logits = music3_lm_head(&lm, &uncond_h).map_err(|e| e.to_string())?;
        for (i, v) in cond_logits.iter_mut().enumerate() {
            if i != end && !(i >= lo && i < hi) {
                *v = f32::NEG_INFINITY;
            }
        }
        for (i, v) in uncond_logits.iter_mut().enumerate() {
            if i != end && !(i >= lo && i < hi) {
                *v = f32::NEG_INFINITY;
            }
        }
        let mut finite: Vec<f32> = cond_logits.iter().copied().filter(|v| v.is_finite()).collect();
        finite.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        let thresh = finite
            .get(MUSIC3_AR_TOP_K.min(finite.len()).saturating_sub(1))
            .copied()
            .unwrap_or(f32::NEG_INFINITY);
        let mut guided = vec![0f32; cond_logits.len()];
        let mut best = (0usize, f32::NEG_INFINITY);
        for i in 0..guided.len() {
            let g = uncond_logits[i] + MUSIC3_AR_CFG * (cond_logits[i] - uncond_logits[i]);
            guided[i] = if cond_logits[i] < thresh || !g.is_finite() {
                f32::NEG_INFINITY
            } else {
                g
            };
            if guided[i].is_finite() && guided[i] > best.1 {
                best = (i, guided[i]);
            }
        }
        let off = sem[frame] as usize;
        let off_logit = guided.get(off).copied().unwrap_or(f32::NAN);
        let arg_logit = best.1;
        let dec_gap = off_logit - arg_logit;
        let flip = if best.0 == off {
            "OK"
        } else if dec_gap.abs() < 0.15 {
            "NEAR"
        } else {
            "FLIP"
        };
        println!(
            "  f{frame} official={off} argmax={} off_logit={off_logit:.4} arg_logit={arg_logit:.4} gap={dec_gap:.4} {flip} a155120={:.4} b156729={:.4}",
            best.0,
            guided.get(A).copied().unwrap_or(f32::NAN),
            guided.get(B).copied().unwrap_or(f32::NAN),
        );
        {
            let mut both = cond_h.clone();
            both.extend_from_slice(&uncond_h);
            let path = format!(r"C:\ai\music3_compare\teacher_last_hidden_f{frame}.npy");
            if let Err(err) = write_npy_f32_val(&path, &both, &[2, cond_h.len()]) {
                println!("  dump {path}: {err}");
            }
        }
        if frame == 12 {
            let path = r"C:\ai\music3_compare\teacher_logits_f12.npy";
            if let Err(err) = write_npy_f32_val(path, &guided, &[guided.len()]) {
                println!("  dump {path}: {err}");
            } else {
                println!("  dumped {path}");
            }
        }
        let resid: Vec<u32> = rvq[frame * width..(frame + 1) * width]
            .iter()
            .map(|&c| c as u32)
            .collect();
        let feedback = music3_embed_audio_frame(&lm, &rvq_w, sem[frame] as u32, &resid)
            .map_err(|e| e.to_string())?;
        let pair = Music3LmSession::step_embeds_pair(
            &mut cond_s,
            &mut uncond_s,
            &lm,
            &lm_prep,
            &feedback,
            &feedback,
        )
        .map_err(|e| e.to_string())?;
        cond_h = pair.0;
        uncond_h = pair.1;
    }
    let _ = music3_lm_evict();
    println!("  teacher PASS");
    Ok(())
}

fn report_l0(tag: &str, ours: &[f32], official: &[f32]) -> Result<(f64, f32, f32), String> {
    let (cos, mx, mean) = compare(ours, official)?;
    let verdict = if mx <= 0.125 { "PASS" } else { "FAIL" };
    println!("  {tag} {verdict} cos={cos:.8} maxabs={mx:.6} mean={mean:.6}");
    Ok((cos, mx, mean))
}

fn load_cmp(path: &str) -> Result<Vec<f32>, String> {
    load_npy(Path::new(path))?.as_f32()
}

fn report_mlp(prefix: &str, ours: &Music3MlpDump, dir: &str, names: &[&str; 6]) -> Result<(), String> {
    let off_post = load_cmp(&format!("{dir}/{}", names[0]))?;
    let off_gate = load_cmp(&format!("{dir}/{}", names[1]))?;
    let off_up = load_cmp(&format!("{dir}/{}", names[2]))?;
    let off_sw = load_cmp(&format!("{dir}/{}", names[3]))?;
    let off_down = load_cmp(&format!("{dir}/{}", names[4]))?;
    let off_layer = load_cmp(&format!("{dir}/{}", names[5]))?;
    report_l0(&format!("{prefix}.post_norm"), &ours.post_norm, &off_post)?;
    report_l0(&format!("{prefix}.gate"), &ours.gate, &off_gate)?;
    report_l0(&format!("{prefix}.up"), &ours.up, &off_up)?;
    report_l0(&format!("{prefix}.swiglu"), &ours.swiglu, &off_sw)?;
    report_l0(&format!("{prefix}.down"), &ours.down, &off_down)?;
    report_l0(&format!("{prefix}.layer"), &ours.layer_out, &off_layer)?;
    Ok(())
}

/// Official-input L0 MLP: feed official attn residual / post_norm / swiglu
/// into the token-best native kernels and compare official intermediates.
fn run_l0mlp(weights: &Path) -> Result<(), String> {
    let dir = r"C:\ai\music3_compare";
    let lm = Music3Shards::load(weights.join("language_model")).map_err(|e| e.to_string())?;
    let prep = Music3LmPrepared::prepare(&lm).map_err(|e| e.to_string())?;

    println!("  -- disk native_l0 vs official_l0 (prefill last-token) --");
    let disk = [
        ("attn_resid", "native_l0_attn_resid.npy", "official_l0_attn_resid.npy"),
        ("post_norm", "native_l0_post_norm.npy", "official_l0_post_norm.npy"),
        ("gate", "native_l0_gate.npy", "official_l0_gate.npy"),
        ("up", "native_l0_up.npy", "official_l0_up.npy"),
        ("swiglu", "native_l0_swiglu.npy", "official_l0_swiglu.npy"),
        ("down", "native_l0_down.npy", "official_l0_down.npy"),
        ("layer", "native_l0_attn_resid.npy", "official_l0_layer.npy"),
    ];
    // layer disk compare uses native layer dump if present
    for (tag, nat, off) in disk {
        let np = format!("{dir}/{nat}");
        let op = format!("{dir}/{off}");
        if !Path::new(&np).exists() || !Path::new(&op).exists() {
            println!("  {tag} SKIP missing {nat} or {off}");
            continue;
        }
        let a = load_cmp(&np)?;
        let b = load_cmp(&op)?;
        if a.len() != b.len() {
            println!("  {tag} SKIP len native={} official={}", a.len(), b.len());
            continue;
        }
        if tag == "layer" {
            continue;
        }
        report_l0(&format!("disk.{tag}"), &a, &b)?;
    }
    if Path::new(&format!("{dir}/native_l0_down.npy")).exists()
        && Path::new(&format!("{dir}/official_l0_layer.npy")).exists()
        && Path::new(&format!("{dir}/native_l0_attn_resid.npy")).exists()
    {
        let resid = load_cmp(&format!("{dir}/native_l0_attn_resid.npy"))?;
        let down = load_cmp(&format!("{dir}/native_l0_down.npy"))?;
        if resid.len() == down.len() {
            let layer: Vec<f32> = resid.iter().zip(&down).map(|(a, b)| a + b).collect();
            let off = load_cmp(&format!("{dir}/official_l0_layer.npy"))?;
            if layer.len() == off.len() {
                report_l0("disk.layer(resid+down)", &layer, &off)?;
            }
        }
    }

    println!("  -- official attn_resid -> native MLP --");
    let off_resid = load_cmp(&format!("{dir}/official_l0_attn_resid.npy"))?;
    let from_resid = music3_mlp_from_attn_resid(&lm, &prep, 0, &off_resid).map_err(|e| e.to_string())?;
    report_mlp(
        "off_resid",
        &from_resid,
        dir,
        &[
            "official_l0_post_norm.npy",
            "official_l0_gate.npy",
            "official_l0_up.npy",
            "official_l0_swiglu.npy",
            "official_l0_down.npy",
            "official_l0_layer.npy",
        ],
    )?;

    println!("  -- official post_norm -> native gate/up/swiglu/down --");
    let off_post = load_cmp(&format!("{dir}/official_l0_post_norm.npy"))?;
    let from_post =
        music3_mlp_from_post_norm(&lm, 0, &off_resid, &off_post).map_err(|e| e.to_string())?;
    report_mlp(
        "off_post",
        &from_post,
        dir,
        &[
            "official_l0_post_norm.npy",
            "official_l0_gate.npy",
            "official_l0_up.npy",
            "official_l0_swiglu.npy",
            "official_l0_down.npy",
            "official_l0_layer.npy",
        ],
    )?;

    println!("  -- official swiglu -> native down_proj --");
    let off_sw = load_cmp(&format!("{dir}/official_l0_swiglu.npy"))?;
    let down = music3_down_from_swiglu(&lm, 0, &off_sw).map_err(|e| e.to_string())?;
    let off_down = load_cmp(&format!("{dir}/official_l0_down.npy"))?;
    report_l0("off_swiglu.down", &down, &off_down)?;

    if Path::new(&format!("{dir}/official_f1_L0_attn_resid.npy")).exists() {
        println!("  -- official f1 attn_resid -> native MLP --");
        let resid = load_cmp(&format!("{dir}/official_f1_L0_attn_resid.npy"))?;
        let dump = music3_mlp_from_attn_resid(&lm, &prep, 0, &resid).map_err(|e| e.to_string())?;
        report_l0("f1_off_resid.post_norm", &dump.post_norm, &load_cmp(&format!("{dir}/official_f1_L0_post_norm.npy"))?)?;
        report_l0("f1_off_resid.gate", &dump.gate, &load_cmp(&format!("{dir}/official_f1_L0_gate.npy"))?)?;
        report_l0("f1_off_resid.up", &dump.up, &load_cmp(&format!("{dir}/official_f1_L0_up.npy"))?)?;
        report_l0("f1_off_resid.swiglu", &dump.swiglu, &load_cmp(&format!("{dir}/official_f1_L0_swiglu.npy"))?)?;
        report_l0("f1_off_resid.down", &dump.down, &load_cmp(&format!("{dir}/official_f1_L0_down.npy"))?)?;
        println!("  -- official f1 swiglu -> native down_proj --");
        let sw = load_cmp(&format!("{dir}/official_f1_L0_swiglu.npy"))?;
        let d = music3_down_from_swiglu(&lm, 0, &sw).map_err(|e| e.to_string())?;
        let od = load_cmp(&format!("{dir}/official_f1_L0_down.npy"))?;
        report_l0("f1_off_swiglu.down", &d, &od)?;
    }

    let _ = music3_lm_evict();
    println!("  l0mlp done");
    Ok(())
}

fn report_toks(tag: &str, ours: &[f32], official: &[f32], tokens: usize) -> Result<(), String> {
    if ours.len() != official.len() || ours.len() != 2 * tokens * MUSIC3_LM_HIDDEN {
        println!(
            "  {tag} SKIP lens ours={} off={} expected={}",
            ours.len(),
            official.len(),
            2 * tokens * MUSIC3_LM_HIDDEN
        );
        return Ok(());
    }
    report_l0(tag, ours, official)?;
    for tok in 0..tokens {
        let c0 = tok * MUSIC3_LM_HIDDEN;
        let u0 = (tokens + tok) * MUSIC3_LM_HIDDEN;
        let (_, cm, _) = compare(
            &ours[c0..c0 + MUSIC3_LM_HIDDEN],
            &official[c0..c0 + MUSIC3_LM_HIDDEN],
        )?;
        let (_, um, _) = compare(
            &ours[u0..u0 + MUSIC3_LM_HIDDEN],
            &official[u0..u0 + MUSIC3_LM_HIDDEN],
        )?;
        if tok == 0 || tok + 1 == tokens || cm > 0.125 || um > 0.125 {
            println!("    {tag} tok{tok} cond_maxabs={cm:.6} unc_maxabs={um:.6}");
        }
    }
    Ok(())
}

/// Official-input L1: official full-seq L0 hidden → native layer 1.
fn run_layer1(weights: &Path) -> Result<(), String> {
    let dir = r"C:\ai\music3_compare";
    std::env::set_var("MAKEPAD_MUSIC3_DUMP_LAYER1", dir);
    let off_l0 = load_cmp(&format!("{dir}/official_fullseq_L0.npy"))?;
    let tokens = 18usize;
    if off_l0.len() != 2 * tokens * MUSIC3_LM_HIDDEN {
        return Err(format!(
            "official_fullseq_L0 len {} expected {}",
            off_l0.len(),
            2 * tokens * MUSIC3_LM_HIDDEN
        ));
    }
    let lm = Music3Shards::load(weights.join("language_model")).map_err(|e| e.to_string())?;
    let prep = Music3LmPrepared::prepare(&lm).map_err(|e| e.to_string())?;
    let ours = music3_layer_prefill_pair(&lm, &prep, 1, &off_l0, tokens).map_err(|e| e.to_string())?;
    let last_off = (tokens - 1) * MUSIC3_LM_HIDDEN;
    let mut last_pair = Vec::with_capacity(2 * MUSIC3_LM_HIDDEN);
    last_pair.extend_from_slice(&ours[last_off..last_off + MUSIC3_LM_HIDDEN]);
    last_pair.extend_from_slice(
        &ours[tokens * MUSIC3_LM_HIDDEN + last_off
            ..tokens * MUSIC3_LM_HIDDEN + last_off + MUSIC3_LM_HIDDEN],
    );
    let off_last = load_cmp(&format!("{dir}/official_prefill_layer1.npy"))?;
    report_l0("layer1 last-token vs official_prefill_layer1", &last_pair, &off_last)?;
    let full_path = format!("{dir}/official_fullseq_L1.npy");
    if Path::new(&full_path).exists() {
        let off_full = load_cmp(&full_path)?;
        if off_full.len() == ours.len() {
            report_l0("layer1 fullseq vs official_fullseq_L1", &ours, &off_full)?;
            for tok in 0..tokens {
                let c0 = tok * MUSIC3_LM_HIDDEN;
                let u0 = (tokens + tok) * MUSIC3_LM_HIDDEN;
                let (_, cm, _) = compare(&ours[c0..c0 + MUSIC3_LM_HIDDEN], &off_full[c0..c0 + MUSIC3_LM_HIDDEN])?;
                let (_, um, _) = compare(&ours[u0..u0 + MUSIC3_LM_HIDDEN], &off_full[u0..u0 + MUSIC3_LM_HIDDEN])?;
                if tok == 0 || tok + 1 == tokens || cm > 0.125 || um > 0.125 {
                    println!("    tok{tok} cond_maxabs={cm:.6} unc_maxabs={um:.6}");
                }
            }
        } else {
            println!(
                "  official_fullseq_L1 len {} ours {}",
                off_full.len(),
                ours.len()
            );
        }
    } else {
        println!("  official_fullseq_L1 missing; last-token only");
    }
    let _ = write_npy_f32_val(
        &format!("{dir}/native_offin_L1.npy"),
        &ours,
        &[2, tokens, MUSIC3_LM_HIDDEN],
    );

    println!("  -- L1 ops on official L0 (attn vs MLP) --");
    let attn_path = format!("{dir}/native_offin_L1_attn.npy");
    let resid_path = format!("{dir}/native_offin_L1_attn_resid.npy");
    let off_attn_path = format!("{dir}/official_fullseq_L1_attn.npy");
    if Path::new(&attn_path).exists() && Path::new(&off_attn_path).exists() {
        let nat_attn = load_cmp(&attn_path)?;
        let off_attn = load_cmp(&off_attn_path)?;
        report_toks("L1_attn(o_proj)", &nat_attn, &off_attn, tokens)?;
        if Path::new(&resid_path).exists() {
            let nat_resid = load_cmp(&resid_path)?;
            let off_resid: Vec<f32> = off_l0
                .iter()
                .zip(&off_attn)
                .map(|(a, b)| a + b)
                .collect();
            report_toks("L1_attn_resid(L0+attn)", &nat_resid, &off_resid, tokens)?;
            let nat_mlp: Vec<f32> = ours
                .iter()
                .zip(&nat_resid)
                .map(|(o, r)| o - r)
                .collect();
            if let Ok(off_full) = load_cmp(&full_path) {
                if off_full.len() == off_resid.len() {
                    let off_mlp: Vec<f32> = off_full
                        .iter()
                        .zip(&off_resid)
                        .map(|(o, r)| o - r)
                        .collect();
                    report_toks("L1_mlp(down)", &nat_mlp, &off_mlp, tokens)?;
                }
            }
        }
    } else {
        println!("  L1 attn dumps missing native={attn_path} official={off_attn_path}");
    }

    let _ = music3_lm_evict();
    println!("  layer1 done");
    Ok(())
}

/// Official-input layer N: official full-seq L{N-1} → native layer N.
fn run_layer_n(weights: &Path, layer: usize) -> Result<(), String> {
    let dir = r"C:\ai\music3_compare";
    let tokens = 18usize;
    let prev = format!("{dir}/official_fullseq_L{}.npy", layer - 1);
    let off_prev = load_cmp(&prev)?;
    if off_prev.len() != 2 * tokens * MUSIC3_LM_HIDDEN {
        return Err(format!(
            "{} len {} expected {}",
            prev,
            off_prev.len(),
            2 * tokens * MUSIC3_LM_HIDDEN
        ));
    }
    let lm = Music3Shards::load(weights.join("language_model")).map_err(|e| e.to_string())?;
    let prep = Music3LmPrepared::prepare(&lm).map_err(|e| e.to_string())?;
    let ours = music3_layer_prefill_pair(&lm, &prep, layer, &off_prev, tokens)
        .map_err(|e| e.to_string())?;
    let last_off = (tokens - 1) * MUSIC3_LM_HIDDEN;
    let mut last_pair = Vec::with_capacity(2 * MUSIC3_LM_HIDDEN);
    last_pair.extend_from_slice(&ours[last_off..last_off + MUSIC3_LM_HIDDEN]);
    last_pair.extend_from_slice(
        &ours[tokens * MUSIC3_LM_HIDDEN + last_off
            ..tokens * MUSIC3_LM_HIDDEN + last_off + MUSIC3_LM_HIDDEN],
    );
    let last_path = format!("{dir}/official_prefill_layer{layer}.npy");
    if Path::new(&last_path).exists() {
        report_l0(
            &format!("L{layer} last-token vs official_prefill_layer{layer}"),
            &last_pair,
            &load_cmp(&last_path)?,
        )?;
    }
    let full_path = format!("{dir}/official_fullseq_L{layer}.npy");
    if Path::new(&full_path).exists() {
        report_toks(
            &format!("L{layer} fullseq"),
            &ours,
            &load_cmp(&full_path)?,
            tokens,
        )?;
    } else {
        println!("  official_fullseq_L{layer} missing");
    }
    let _ = write_npy_f32_val(
        &format!("{dir}/native_offin_L{layer}.npy"),
        &ours,
        &[2, tokens, MUSIC3_LM_HIDDEN],
    );
    let _ = music3_lm_evict();
    println!("  layer{layer} done");
    Ok(())
}

/// Official qrope/krope/v → native attn+o_proj vs official L0 attn.
fn run_l0attn_offin(weights: &Path) -> Result<(), String> {
    let dir = r"C:\ai\music3_compare";
    let tokens = 18usize;
    let q = load_cmp(&format!("{dir}/official_fullseq_L0_qrope.npy"))?;
    let k = load_cmp(&format!("{dir}/official_fullseq_L0_krope.npy"))?;
    let v = load_cmp(&format!("{dir}/official_fullseq_L0_v.npy"))?;
    let lm = Music3Shards::load(weights.join("language_model")).map_err(|e| e.to_string())?;
    let ours = music3_l0_attn_from_qkv(&lm, &q, &k, &v, tokens).map_err(|e| e.to_string())?;
    let off = load_cmp(&format!("{dir}/official_fullseq_L0_attn.npy"))?;
    report_toks("off_qkv → native attn+o_proj", &ours, &off, tokens)?;
    let _ = music3_lm_evict();
    println!("  l0attn done");
    Ok(())
}

fn extract_tok_qkv(
    q: &[f32],
    k: &[f32],
    v: &[f32],
    tokens: usize,
    tok: usize,
) -> Result<(Vec<f32>, Vec<f32>, Vec<f32>, usize), String> {
    let q_inner = MUSIC3_LM_HIDDEN;
    let kv_inner = MUSIC3_LM_HIDDEN / 4;
    if tok >= tokens {
        return Err(format!("tok {tok} >= tokens {tokens}"));
    }
    if q.len() != 2 * tokens * q_inner || k.len() != 2 * tokens * kv_inner || v.len() != 2 * tokens * kv_inner
    {
        return Err(format!(
            "fullseq qkv len q={} k={} v={} tokens={tokens}",
            q.len(),
            k.len(),
            v.len()
        ));
    }
    let seq = tok + 1;
    let mut q_out = Vec::with_capacity(2 * q_inner);
    q_out.extend_from_slice(&q[tok * q_inner..(tok + 1) * q_inner]);
    q_out.extend_from_slice(&q[(tokens + tok) * q_inner..(tokens + tok + 1) * q_inner]);
    let mut k_out = Vec::with_capacity(2 * seq * kv_inner);
    let mut v_out = Vec::with_capacity(2 * seq * kv_inner);
    k_out.extend_from_slice(&k[..seq * kv_inner]);
    k_out.extend_from_slice(&k[tokens * kv_inner..(tokens + seq) * kv_inner]);
    v_out.extend_from_slice(&v[..seq * kv_inner]);
    v_out.extend_from_slice(&v[tokens * kv_inner..(tokens + seq) * kv_inner]);
    Ok((q_out, k_out, v_out, seq))
}

fn pair_tok(full: &[f32], tokens: usize, tok: usize) -> Result<Vec<f32>, String> {
    let cols = MUSIC3_LM_HIDDEN;
    if full.len() != 2 * tokens * cols {
        return Err(format!("pair_tok len {} tokens={tokens}", full.len()));
    }
    let mut out = Vec::with_capacity(2 * cols);
    out.extend_from_slice(&full[tok * cols..(tok + 1) * cols]);
    out.extend_from_slice(&full[(tokens + tok) * cols..(tokens + tok + 1) * cols]);
    Ok(out)
}

fn run_decode_kernel(
    weights: &Music3Shards,
    tag: &str,
    q: &[f32],
    k: &[f32],
    v: &[f32],
    seq: usize,
    off: &[f32],
    with_o_proj: bool,
) -> Result<(f64, f32, f32), String> {
    let ours =
        music3_decode_attn_from_qkv(weights, q, k, v, seq, with_o_proj).map_err(|e| e.to_string())?;
    report_l0(tag, &ours, off)
}

/// Teacher-force decode-step attn on official QKV.
///
/// 1. Prefill official full-seq Q/K/V: last + mid tokens through the GQA
///    decode kernel vs official last/mid attn.
/// 2. Official decode-step dumps at f1/f6/f12 if present.
fn run_decodeattn(weights: &Path) -> Result<(), String> {
    let dir = r"C:\ai\music3_compare";
    let tokens = 18usize;
    let q = load_cmp(&format!("{dir}/official_fullseq_L0_qrope.npy"))?;
    let k = load_cmp(&format!("{dir}/official_fullseq_L0_krope.npy"))?;
    let v = load_cmp(&format!("{dir}/official_fullseq_L0_v.npy"))?;
    let off_attn = load_cmp(&format!("{dir}/official_fullseq_L0_attn.npy"))?;
    let lm = Music3Shards::load(weights.join("language_model")).map_err(|e| e.to_string())?;
    let mid = tokens / 2;
    println!("  -- official prefill QKV → decode kernel last+mid --");
    for tok in [0usize, mid, tokens - 1] {
        let (qt, kt, vt, seq) = extract_tok_qkv(&q, &k, &v, tokens, tok)?;
        let off = pair_tok(&off_attn, tokens, tok)?;
        let tag = format!("prefill gqa tok{tok}(seq={seq}) o_proj");
        let (cos, mx, mean) = run_decode_kernel(&lm, &tag, &qt, &kt, &vt, seq, &off, true)?;
        let _ = (cos, mx, mean);
    }
    println!("  -- official decode-step QKV f1/f6/f12 --");
    for frame in [1usize, 6, 12] {
        let q_path = format!("{dir}/official_decode_f{frame}_L0_q.npy");
        let k_path = format!("{dir}/official_decode_f{frame}_L0_k.npy");
        let v_path = format!("{dir}/official_decode_f{frame}_L0_v.npy");
        let attn_path = format!("{dir}/official_decode_f{frame}_L0_attn.npy");
        let oproj_path = format!("{dir}/official_decode_f{frame}_L0_oproj.npy");
        if !Path::new(&q_path).exists() || !Path::new(&k_path).exists() || !Path::new(&v_path).exists()
        {
            println!("  f{frame} SKIP missing official_decode_f{frame}_L0_q/k/v.npy");
            continue;
        }
        let qt = load_cmp(&q_path)?;
        let kt = load_cmp(&k_path)?;
        let vt = load_cmp(&v_path)?;
        let kv_inner = MUSIC3_LM_HIDDEN / 4;
        if kt.len() % (2 * kv_inner) != 0 {
            println!("  f{frame} SKIP k len {}", kt.len());
            continue;
        }
        let seq = kt.len() / (2 * kv_inner);
        if Path::new(&attn_path).exists() {
            let off = load_cmp(&attn_path)?;
            let tag = format!("decode f{frame} gqa attn");
            let _ = run_decode_kernel(&lm, &tag, &qt, &kt, &vt, seq, &off, false)?;
        } else {
            println!("  f{frame} gqa attn SKIP missing official_decode_f{frame}_L0_attn.npy");
        }
        if Path::new(&oproj_path).exists() {
            let off = load_cmp(&oproj_path)?;
            let tag = format!("decode f{frame} gqa o_proj");
            let _ = run_decode_kernel(&lm, &tag, &qt, &kt, &vt, seq, &off, true)?;
        }
    }
    let _ = music3_lm_evict();
    println!("  decodeattn done");
    Ok(())
}

/// Official L1 attn residual → native L1 MLP ops vs official intermediates.
fn run_layer1mlp(weights: &Path) -> Result<(), String> {
    let dir = r"C:\ai\music3_compare";
    let tokens = 18usize;
    let off_l0 = load_cmp(&format!("{dir}/official_fullseq_L0.npy"))?;
    let off_attn = load_cmp(&format!("{dir}/official_fullseq_L1_attn.npy"))?;
    if off_l0.len() != off_attn.len() || off_l0.len() != 2 * tokens * MUSIC3_LM_HIDDEN {
        return Err(format!(
            "L1 resid shape l0={} attn={} expected={}",
            off_l0.len(),
            off_attn.len(),
            2 * tokens * MUSIC3_LM_HIDDEN
        ));
    }
    let sum_resid: Vec<f32> = off_l0.iter().zip(&off_attn).map(|(a, b)| a + b).collect();
    let resid_path = format!("{dir}/official_fullseq_L1_resid.npy");
    let resid = if Path::new(&resid_path).exists() {
        let off_resid = load_cmp(&resid_path)?;
        if off_resid.len() == sum_resid.len() {
            report_toks("L0plusAttn vs official_resid", &sum_resid, &off_resid, tokens)?;
        }
        println!("  using official_fullseq_L1_resid.npy as MLP input");
        off_resid
    } else {
        println!("  official resid dump missing; using f32(L0)+f32(attn)");
        sum_resid
    };
    let lm = Music3Shards::load(weights.join("language_model")).map_err(|e| e.to_string())?;
    let prep = Music3LmPrepared::prepare(&lm).map_err(|e| e.to_string())?;

    println!("  -- official L1 resid -> native L1 MLP --");
    let from_resid =
        music3_mlp_from_attn_resid(&lm, &prep, 1, &resid).map_err(|e| e.to_string())?;
    let pairs = [
        ("post_norm", from_resid.post_norm.as_slice(), "official_fullseq_L1_post_norm.npy"),
        ("gate", from_resid.gate.as_slice(), "official_fullseq_L1_gate.npy"),
        ("up", from_resid.up.as_slice(), "official_fullseq_L1_up.npy"),
        ("swiglu", from_resid.swiglu.as_slice(), "official_fullseq_L1_swiglu.npy"),
        ("down", from_resid.down.as_slice(), "official_fullseq_L1_down.npy"),
    ];
    for (tag, ours, name) in pairs {
        let path = format!("{dir}/{name}");
        if !Path::new(&path).exists() {
            println!("  L1_mlp.{tag} SKIP missing {name}");
            continue;
        }
        let off = load_cmp(&path)?;
        if ours.len() != off.len() {
            println!("  L1_mlp.{tag} SKIP len ours={} off={}", ours.len(), off.len());
            continue;
        }
        let cols = ours.len() / (2 * tokens);
        report_l0(&format!("L1_mlp.{tag}"), ours, &off)?;
        for tok in 0..tokens {
            let c0 = tok * cols;
            let u0 = (tokens + tok) * cols;
            let (_, cm, _) = compare(&ours[c0..c0 + cols], &off[c0..c0 + cols])?;
            let (_, um, _) = compare(&ours[u0..u0 + cols], &off[u0..u0 + cols])?;
            if tok == 0 || tok + 1 == tokens || cm > 0.125 || um > 0.125 {
                println!("    L1_mlp.{tag} tok{tok} cond_maxabs={cm:.6} unc_maxabs={um:.6}");
            }
        }
    }

    if Path::new(&format!("{dir}/official_fullseq_L1_post_norm.npy")).exists() {
        println!("  -- official L1 post_norm -> native gate/up/swiglu/down --");
        let off_post = load_cmp(&format!("{dir}/official_fullseq_L1_post_norm.npy"))?;
        let from_post =
            music3_mlp_from_post_norm(&lm, 1, &resid, &off_post).map_err(|e| e.to_string())?;
        for (tag, ours, name) in [
            ("gate", from_post.gate.as_slice(), "official_fullseq_L1_gate.npy"),
            ("up", from_post.up.as_slice(), "official_fullseq_L1_up.npy"),
            ("swiglu", from_post.swiglu.as_slice(), "official_fullseq_L1_swiglu.npy"),
            ("down", from_post.down.as_slice(), "official_fullseq_L1_down.npy"),
        ] {
            let path = format!("{dir}/{name}");
            if !Path::new(&path).exists() {
                continue;
            }
            let off = load_cmp(&path)?;
            if ours.len() == off.len() {
                report_l0(&format!("L1_offpost.{tag}"), ours, &off)?;
            }
        }
    }

    if Path::new(&format!("{dir}/official_fullseq_L1_swiglu.npy")).exists() {
        println!("  -- official L1 swiglu -> native down --");
        let sw = load_cmp(&format!("{dir}/official_fullseq_L1_swiglu.npy"))?;
        let down = music3_down_from_swiglu(&lm, 1, &sw).map_err(|e| e.to_string())?;
        let off_down = load_cmp(&format!("{dir}/official_fullseq_L1_down.npy"))?;
        if down.len() == off_down.len() {
            report_l0("L1_offswiglu.down", &down, &off_down)?;
            let cols = MUSIC3_LM_HIDDEN;
            for tok in [0usize, tokens - 1] {
                let c0 = tok * cols;
                let u0 = (tokens + tok) * cols;
                let (_, cm, _) = compare(&down[c0..c0 + cols], &off_down[c0..c0 + cols])?;
                let (_, um, _) = compare(&down[u0..u0 + cols], &off_down[u0..u0 + cols])?;
                println!("    L1_offswiglu.down tok{tok} cond_maxabs={cm:.6} unc_maxabs={um:.6}");
            }
        }
    }

    let _ = music3_lm_evict();
    println!("  layer1mlp done");
    Ok(())
}

/// Official-input first decode: prefill → official dummy codes → embed vs
/// official_feedback_f0 → native step_embeds_pair vs official_last_hidden_f1.
fn run_decode1(weights: &Path, dump: &Path) -> Result<(), String> {
    let ids = load_npy(&dump.join("text_ids.npy"))?;
    if ids.shape.len() != 2 || ids.shape[0] != 2 {
        return Err(format!("text_ids shape {:?}, expected [2, T]", ids.shape));
    }
    let t = ids.shape[1];
    let vals = ids.as_i64()?;
    let cond: Vec<u32> = vals[..t].iter().map(|&v| v as u32).collect();
    let uncond: Vec<u32> = vals[t..].iter().map(|&v| v as u32).collect();
    let sem = load_npy(&dump.join("semantic_codes.npy"))?.as_i64()?;
    let rvq = load_npy(&dump.join("rvq_codes.npy"))?.as_i64()?;
    let width = MUSIC3_NUM_CODEBOOKS - 1;
    let resid: Vec<u32> = rvq[..width].iter().map(|&c| c as u32).collect();
    let lm = Music3Shards::load(weights.join("language_model")).map_err(|e| e.to_string())?;
    let lm_prep = Music3LmPrepared::prepare(&lm).map_err(|e| e.to_string())?;
    let rvq_w = Music3Shards::load(weights.join("rvq_depth_decoder")).map_err(|e| e.to_string())?;
    let (mut cond_s, mut cond_h, mut uncond_s, mut uncond_h) =
        Music3LmSession::prefill_pair_with_progress(&lm, &lm_prep, &cond, &uncond, &mut |_, _| {})
            .map_err(|e| e.to_string())?;
    let native_fb =
        music3_embed_audio_frame(&lm, &rvq_w, sem[0] as u32, &resid).map_err(|e| e.to_string())?;
    let fb_path = r"C:\ai\music3_compare\official_feedback_f0.npy";
    let off_fb = load_npy(Path::new(fb_path))?.as_f32()?;
    if off_fb.len() >= MUSIC3_LM_HIDDEN {
        let (cos, mx, mean) = compare(&native_fb, &off_fb[..MUSIC3_LM_HIDDEN])?;
        println!("  feedback_f0 vs official row0 cos={cos:.8} maxabs={mx:.6} mean={mean:.6}");
        if off_fb.len() >= 2 * MUSIC3_LM_HIDDEN {
            let (cos2, mx2, mean2) = compare(&native_fb, &off_fb[MUSIC3_LM_HIDDEN..])?;
            println!(
                "  feedback_f0 vs official row1 cos={cos2:.8} maxabs={mx2:.6} mean={mean2:.6}"
            );
        }
    } else {
        println!("  official_feedback_f0 short {}", off_fb.len());
    }
    // Official-input decode: same official feedback on both CFG rows (Python
    // repeats the sampled codes).
    let fb = if off_fb.len() >= MUSIC3_LM_HIDDEN {
        off_fb[..MUSIC3_LM_HIDDEN].to_vec()
    } else {
        native_fb.clone()
    };
    let pair = Music3LmSession::step_embeds_pair(
        &mut cond_s,
        &mut uncond_s,
        &lm,
        &lm_prep,
        &fb,
        &fb,
    )
    .map_err(|e| e.to_string())?;
    cond_h = pair.0;
    uncond_h = pair.1;
    let mut both = cond_h.clone();
    both.extend_from_slice(&uncond_h);
    let off_f1 = load_npy(Path::new(r"C:\ai\music3_compare\official_last_hidden_f1.npy"))?.as_f32()?;
    if off_f1.len() == both.len() {
        let (cos, mx, mean) = compare(&both, &off_f1)?;
        println!("  decode1 last_hidden vs official_f1 cos={cos:.8} maxabs={mx:.6} mean={mean:.6}");
        let (c0, m0, _) = compare(&cond_h, &off_f1[..MUSIC3_LM_HIDDEN])?;
        let (c1, m1, _) = compare(&uncond_h, &off_f1[MUSIC3_LM_HIDDEN..])?;
        println!("    cond cos={c0:.8} maxabs={m0:.6}  unc cos={c1:.8} maxabs={m1:.6}");
    } else {
        println!(
            "  official_f1 len {} ours {}",
            off_f1.len(),
            both.len()
        );
    }
    let _ = music3_lm_evict();
    println!("  decode1 done");
    Ok(())
}

fn run_sample(weights: &Path, dump: &Path) -> Result<(), String> {
    use makepad_diffusion::music3::{MUSIC3_AR_CFG, MUSIC3_AR_TOP_K, MUSIC3_SEMANTIC_VOCAB};
    let ids = load_npy(&dump.join("text_ids.npy"))?;
    if ids.shape.len() != 2 || ids.shape[0] != 2 {
        return Err(format!("text_ids shape {:?}", ids.shape));
    }
    let t = ids.shape[1];
    let vals = ids.as_i64()?;
    let cond: Vec<u32> = vals[..t].iter().map(|&v| v as u32).collect();
    let uncond: Vec<u32> = vals[t..].iter().map(|&v| v as u32).collect();
    let lm = Music3Shards::load(weights.join("language_model")).map_err(|e| e.to_string())?;
    let lm_prep = Music3LmPrepared::prepare(&lm).map_err(|e| e.to_string())?;
    let (_hidden, logits) = music3_lm_prefill_pair(&lm, &lm_prep, &cond, &uncond)
        .map_err(|e| e.to_string())?;
    let cond_logits = &logits[..MUSIC3_LM_VOCAB];
    let uncond_logits = &logits[MUSIC3_LM_VOCAB..];
    let lo = MUSIC3_AUDIO_CODE_OFFSET as usize;
    let hi = lo + MUSIC3_SEMANTIC_VOCAB;
    let end = MUSIC3_AUDIO_END_TOKEN_ID as usize;
    let mut cond_m = cond_logits.to_vec();
    let mut uncond_m = uncond_logits.to_vec();
    for (i, v) in cond_m.iter_mut().enumerate() {
        if i != end && !(i >= lo && i < hi) {
            *v = f32::NEG_INFINITY;
        }
    }
    for (i, v) in uncond_m.iter_mut().enumerate() {
        if i != end && !(i >= lo && i < hi) {
            *v = f32::NEG_INFINITY;
        }
    }
    let mut cond_fin: Vec<f32> = cond_m.iter().copied().filter(|v| v.is_finite()).collect();
    cond_fin.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    let thresh = cond_fin
        .get(MUSIC3_AR_TOP_K.min(cond_fin.len()).saturating_sub(1))
        .copied()
        .unwrap_or(f32::NEG_INFINITY);
    let mut guided = vec![0f32; MUSIC3_LM_VOCAB];
    for i in 0..MUSIC3_LM_VOCAB {
        let g = uncond_m[i] + MUSIC3_AR_CFG * (cond_m[i] - uncond_m[i]);
        guided[i] = if cond_m[i] < thresh || !g.is_finite() {
            f32::NEG_INFINITY
        } else {
            g
        };
    }
    let href = load_npy(&dump.join("first_sample_logits.npy"))?;
    let href_v = href.as_f32()?;
    if href_v.len() != MUSIC3_LM_VOCAB {
        return Err(format!("first_sample_logits {}", href_v.len()));
    }
    let mut ours_fin = Vec::new();
    let mut ref_fin = Vec::new();
    let mut both = 0usize;
    let mut only_ours = 0usize;
    let mut only_ref = 0usize;
    let mut best_ours = (0usize, f32::NEG_INFINITY);
    let mut best_ref = (0usize, f32::NEG_INFINITY);
    for i in 0..MUSIC3_LM_VOCAB {
        let a = guided[i];
        let b = href_v[i];
        if a.is_finite() && a > best_ours.1 {
            best_ours = (i, a);
        }
        if b.is_finite() && b > best_ref.1 {
            best_ref = (i, b);
        }
        match (a.is_finite(), b.is_finite()) {
            (true, true) => {
                both += 1;
                ours_fin.push(a);
                ref_fin.push(b);
            }
            (true, false) => only_ours += 1,
            (false, true) => only_ref += 1,
            _ => {}
        }
    }
    let (cos, max_abs, mean_abs) = if ours_fin.is_empty() {
        (0.0, 0.0, 0.0)
    } else {
        compare(&ours_fin, &ref_fin)?
    };
    println!(
        "  first guided finite ours={} dump={} both={both} only_ours={only_ours} only_ref={only_ref}",
        both + only_ours,
        both + only_ref
    );
    println!(
        "  finite-overlap cos={cos:.7} max_abs={max_abs:.3e} mean_abs={mean_abs:.3e}"
    );
    println!(
        "  argmax ours={} ({:.4}) dump={} ({:.4})",
        best_ours.0, best_ours.1, best_ref.0, best_ref.1
    );
    let sem = load_npy(&dump.join("semantic_codes.npy"))?;
    let official0 = sem.as_i64()?[0];
    println!("  official first token={official0}");
    let rvq = Music3Shards::load(weights.join("rvq_depth_decoder")).map_err(|e| e.to_string())?;
    let rvq_prep = Music3RvqPrepared::prepare(&rvq).map_err(|e| e.to_string())?;
    std::env::set_var("MAKEPAD_MUSIC3_TRACE_TOKENS", "1");
    if std::env::var_os("MAKEPAD_MUSIC3_DUMP_SEMANTIC").is_none() {
        std::env::set_var(
            "MAKEPAD_MUSIC3_DUMP_SEMANTIC",
            r"C:\ai\music3_compare\native_semantic_codes.npy",
        );
    }
    if std::env::var_os("MAKEPAD_MUSIC3_DUMP_RVQ").is_none() {
        std::env::set_var(
            "MAKEPAD_MUSIC3_DUMP_RVQ",
            r"C:\ai\music3_compare\native_rvq_codes.npy",
        );
    }
    if std::env::var_os("MAKEPAD_MUSIC3_DUMP_RVQ_LOGITS").is_none() {
        std::env::set_var(
            "MAKEPAD_MUSIC3_DUMP_RVQ_LOGITS",
            r"C:\ai\music3_compare\native_rvq_head2.txt",
        );
    }
    if std::env::var_os("MAKEPAD_MUSIC3_DUMP_HIDDEN_F10").is_none() {
        std::env::set_var(
            "MAKEPAD_MUSIC3_DUMP_HIDDEN_F10",
            r"C:\ai\music3_compare\native_last_hidden_f10.npy",
        );
    }
    let _ = music3_ar_sample(
        &lm,
        &lm_prep,
        &rvq,
        &rvq_prep,
        &cond,
        &uncond,
        36,
        1,
        7,
    )
    .map_err(|e| e.to_string())?;
    let _ = dump_official_hidden_rvq_head2(weights, &lm, &rvq, &rvq_prep);
    let sem_dump = std::env::var("MAKEPAD_MUSIC3_DUMP_SEMANTIC").unwrap_or_else(|_| {
        r"C:\ai\music3_compare\native_semantic_codes.npy".into()
    });
    let native_sem = load_npy(Path::new(&sem_dump))?;
    let native_sem_v = native_sem.as_i64()?;
    let official_sem = sem.as_i64()?;
    let ncmp = native_sem_v.len().min(official_sem.len());
    let mut first_mismatch = None;
    for i in 0..ncmp {
        if native_sem_v[i] != official_sem[i] {
            first_mismatch = Some(i);
            break;
        }
    }
    println!(
        "  semantic native={} official={} compared={ncmp} first_mismatch={}",
        native_sem_v.len(),
        official_sem.len(),
        first_mismatch
            .map(|i| format!(
                "{i} native={} official={}",
                native_sem_v[i], official_sem[i]
            ))
            .unwrap_or_else(|| "-".into())
    );
    if ncmp > 0 {
        let show = ncmp.min(24);
        println!("  native[:{show}]={:?}", &native_sem_v[..show]);
        println!("  officl[:{show}]={:?}", &official_sem[..show]);
    }
    let native_rvq_path = Path::new(r"C:\ai\music3_compare\native_rvq_codes.npy");
    if native_rvq_path.exists() {
        let native_rvq = load_npy(native_rvq_path)?;
        let native_rvq_v = native_rvq.as_i64()?;
        let official_rvq = load_npy(&dump.join("rvq_codes.npy"))?.as_i64()?;
        let rcmp = native_rvq_v.len().min(official_rvq.len());
        let mut rvq_mis = None;
        for i in 0..rcmp {
            if native_rvq_v[i] != official_rvq[i] {
                rvq_mis = Some(i);
                break;
            }
        }
        println!(
            "  rvq native={} official={} compared={rcmp} first_mismatch={}",
            native_rvq_v.len(),
            official_rvq.len(),
            rvq_mis
                .map(|i| format!(
                    "{i} native={} official={}",
                    native_rvq_v[i], official_rvq[i]
                ))
                .unwrap_or_else(|| "-".into())
        );
    }
    let _ = music3_lm_evict();
    let _ = music3_rvq_evict();
    if let Some(i) = first_mismatch {
        return Err(format!(
            "sampled semantic first_mismatch={i} native={} official={}",
            native_sem_v[i], official_sem[i]
        ));
    }
    if both < 40 || only_ours + only_ref > 10 {
        return Err(format!(
            "sample0 support mismatch both={both} only_ours={only_ours} only_ref={only_ref}"
        ));
    }
    if cos < 0.999 {
        return Err(format!("sample0 guided mismatch cos={cos:.7} max_abs={max_abs:.3e}"));
    }
    println!("  sample PASS");
    Ok(())
}

/// Official last_hidden f10 + official residual prefix [449,800] → native
/// batched RVQ head-2 logits. Isolates decoder GEMM from LM last_hidden drift.
fn dump_official_hidden_rvq_head2(
    _weights: &Path,
    lm: &Music3Shards,
    rvq: &Music3Shards,
    rvq_prep: &Music3RvqPrepared,
) -> Result<(), String> {
    let path = Path::new(r"C:\ai\music3_compare\official_last_hidden_f10.npy");
    if !path.exists() {
        println!("  official-hidden RVQ skip (no {})", path.display());
        return Ok(());
    }
    let hidden = load_npy(path)?;
    let h = hidden.as_f32()?;
    if h.len() != 2 * MUSIC3_RVQ_HIDDEN {
        return Err(format!("official last_hidden f10 len {}", h.len()));
    }
    let cond = &h[..MUSIC3_RVQ_HIDDEN];
    let uncond = &h[MUSIC3_RVQ_HIDDEN..];
    let sem = 155_120u32;
    let embed = lm
        .tensor_row_f32("model.embed_tokens.weight", sem as u64)
        .map_err(|e| e.to_string())?;
    let mut last_both = Vec::with_capacity(2 * MUSIC3_RVQ_HIDDEN);
    last_both.extend_from_slice(cond);
    last_both.extend_from_slice(uncond);
    let p0 = music3_rvq_project_rows(rvq, &last_both, 2).map_err(|e| e.to_string())?;
    let mut sem_both = Vec::with_capacity(2 * MUSIC3_RVQ_HIDDEN);
    sem_both.extend_from_slice(&embed);
    sem_both.extend_from_slice(&embed);
    let p1 = music3_rvq_project_rows(rvq, &sem_both, 2).map_err(|e| e.to_string())?;
    let mut seq_c = Vec::new();
    let mut seq_u = Vec::new();
    seq_c.extend_from_slice(&p0[..MUSIC3_RVQ_HIDDEN]);
    seq_u.extend_from_slice(&p0[MUSIC3_RVQ_HIDDEN..]);
    seq_c.extend_from_slice(&p1[..MUSIC3_RVQ_HIDDEN]);
    seq_u.extend_from_slice(&p1[MUSIC3_RVQ_HIDDEN..]);
    let prefix = [449u32, 800u32];
    for (head, &code) in prefix.iter().enumerate() {
        let n = seq_c.len() / MUSIC3_RVQ_HIDDEN;
        let (out_c, out_u) =
            music3_rvq_forward_pair(rvq, rvq_prep, &seq_c, &seq_u, n).map_err(|e| e.to_string())?;
        let last_c = &out_c[(n - 1) * MUSIC3_RVQ_HIDDEN..];
        let last_u = &out_u[(n - 1) * MUSIC3_RVQ_HIDDEN..];
        let mut last_pair = Vec::with_capacity(2 * MUSIC3_RVQ_HIDDEN);
        last_pair.extend_from_slice(last_c);
        last_pair.extend_from_slice(last_u);
        let _ = music3_rvq_audio_head_rows(rvq, &last_pair, head, 2).map_err(|e| e.to_string())?;
        let idx = code as u64 + head as u64 * MUSIC3_AUDIO_VOCAB as u64;
        let emb = rvq
            .tensor_row_f32("audio_embeddings.weight", idx)
            .map_err(|e| e.to_string())?;
        let mut emb2 = Vec::with_capacity(2 * emb.len());
        emb2.extend_from_slice(&emb);
        emb2.extend_from_slice(&emb);
        let proj = music3_rvq_project_rows(rvq, &emb2, 2).map_err(|e| e.to_string())?;
        seq_c.extend_from_slice(&proj[..MUSIC3_RVQ_HIDDEN]);
        seq_u.extend_from_slice(&proj[MUSIC3_RVQ_HIDDEN..]);
    }
    let n = seq_c.len() / MUSIC3_RVQ_HIDDEN;
    let (out_c, out_u) =
        music3_rvq_forward_pair(rvq, rvq_prep, &seq_c, &seq_u, n).map_err(|e| e.to_string())?;
    let last_c = &out_c[(n - 1) * MUSIC3_RVQ_HIDDEN..];
    let last_u = &out_u[(n - 1) * MUSIC3_RVQ_HIDDEN..];
    let mut last_pair = Vec::with_capacity(2 * MUSIC3_RVQ_HIDDEN);
    last_pair.extend_from_slice(last_c);
    last_pair.extend_from_slice(last_u);
    let logits = music3_rvq_audio_head_rows(rvq, &last_pair, 2, 2).map_err(|e| e.to_string())?;
    let (lc, lu) = logits.split_at(logits.len() / 2);
    let mut guided: Vec<f32> = lc
        .iter()
        .zip(lu.iter())
        .map(|(c, u)| *u + MUSIC3_AR_CFG * (*c - *u))
        .collect();
    let off_path = Path::new(r"C:\ai\music3_compare\official_rvq_f10_h2.npy");
    if off_path.exists() {
        let off = load_npy(off_path)?.as_f32()?;
        let n = guided.len().min(off.len());
        let (cos, max_abs, _) = compare(&guided[..n], &off[..n])?;
        let mut best_n = (0usize, f32::NEG_INFINITY);
        let mut best_o = (0usize, f32::NEG_INFINITY);
        for i in 0..n {
            if guided[i] > best_n.1 {
                best_n = (i, guided[i]);
            }
            if off[i] > best_o.1 {
                best_o = (i, off[i]);
            }
        }
        println!(
            "  official-hidden RVQ f10 h2 cos={cos:.7} max_abs={max_abs:.3e} native[641]={:.4} native[776]={:.4} off[641]={:.4} off[776]={:.4} argmax_n={} argmax_o={}",
            guided.get(641).copied().unwrap_or(0.0),
            guided.get(776).copied().unwrap_or(0.0),
            off.get(641).copied().unwrap_or(0.0),
            off.get(776).copied().unwrap_or(0.0),
            best_n.0,
            best_o.0
        );
    } else {
        let _ = &mut guided;
        println!("  official-hidden RVQ f10 h2 computed (no official logits npy)");
    }
    Ok(())
}

/// Official last_hidden f12 + official residual prefix [234,14] → native
/// RVQ tensors at each residual head. Isolates decoder from LM drift.
fn run_rvq_f12(weights: &Path) -> Result<(), String> {
    let path = Path::new(r"C:\ai\music3_compare\official_last_hidden_f12.npy");
    if !path.exists() {
        return Err(format!("missing {}", path.display()));
    }
    let hidden = load_npy(path)?;
    let h = hidden.as_f32()?;
    if h.len() != 2 * MUSIC3_RVQ_HIDDEN {
        return Err(format!("official last_hidden f12 len {}", h.len()));
    }
    let cond = &h[..MUSIC3_RVQ_HIDDEN];
    let uncond = &h[MUSIC3_RVQ_HIDDEN..];
    let lm = Music3Shards::load(weights.join("language_model")).map_err(|e| e.to_string())?;
    let rvq = Music3Shards::load(weights.join("rvq_depth_decoder")).map_err(|e| e.to_string())?;
    let rvq_prep = Music3RvqPrepared::prepare(&rvq).map_err(|e| e.to_string())?;
    let sem = 156_729u32;
    let embed = lm
        .tensor_row_f32("model.embed_tokens.weight", sem as u64)
        .map_err(|e| e.to_string())?;
    let mut last_both = Vec::with_capacity(2 * MUSIC3_RVQ_HIDDEN);
    last_both.extend_from_slice(cond);
    last_both.extend_from_slice(uncond);
    let p0 = music3_rvq_project_rows(&rvq, &last_both, 2).map_err(|e| e.to_string())?;
    write_npy_f32_val(r"C:\ai\music3_compare\native_rvq_f12_p0.npy", &p0, &[2, MUSIC3_RVQ_HIDDEN])?;
    cmp_named("f12_p0", r"C:\ai\music3_compare\official_rvq_f12_p0.npy", &p0)?;
    let mut sem_both = Vec::with_capacity(2 * MUSIC3_RVQ_HIDDEN);
    sem_both.extend_from_slice(&embed);
    sem_both.extend_from_slice(&embed);
    write_npy_f32_val(
        r"C:\ai\music3_compare\native_rvq_f12_sem_embed.npy",
        &sem_both,
        &[2, MUSIC3_RVQ_HIDDEN],
    )?;
    cmp_named(
        "f12_sem_embed",
        r"C:\ai\music3_compare\official_rvq_f12_sem_embed.npy",
        &sem_both,
    )?;
    let p1 = music3_rvq_project_rows(&rvq, &sem_both, 2).map_err(|e| e.to_string())?;
    write_npy_f32_val(r"C:\ai\music3_compare\native_rvq_f12_p1.npy", &p1, &[2, MUSIC3_RVQ_HIDDEN])?;
    cmp_named("f12_p1", r"C:\ai\music3_compare\official_rvq_f12_p1.npy", &p1)?;
    let mut seq_c = Vec::new();
    let mut seq_u = Vec::new();
    seq_c.extend_from_slice(&p0[..MUSIC3_RVQ_HIDDEN]);
    seq_u.extend_from_slice(&p0[MUSIC3_RVQ_HIDDEN..]);
    seq_c.extend_from_slice(&p1[..MUSIC3_RVQ_HIDDEN]);
    seq_u.extend_from_slice(&p1[MUSIC3_RVQ_HIDDEN..]);
    let official_prefix = [234u32, 14, 776, 5, 505, 366, 909];
    for head in 0..7 {
        let n = seq_c.len() / MUSIC3_RVQ_HIDDEN;
        let (out_c, out_u) =
            music3_rvq_forward_pair(&rvq, &rvq_prep, &seq_c, &seq_u, n).map_err(|e| e.to_string())?;
        let last_c = &out_c[(n - 1) * MUSIC3_RVQ_HIDDEN..];
        let last_u = &out_u[(n - 1) * MUSIC3_RVQ_HIDDEN..];
        let mut last_pair = Vec::with_capacity(2 * MUSIC3_RVQ_HIDDEN);
        last_pair.extend_from_slice(last_c);
        last_pair.extend_from_slice(last_u);
        let hid_path = format!(r"C:\ai\music3_compare\native_rvq_f12_h{head}_hidden.npy");
        write_npy_f32_val(&hid_path, &last_pair, &[2, MUSIC3_RVQ_HIDDEN])?;
        cmp_named(
            &format!("f12_h{head}_hidden"),
            &format!(r"C:\ai\music3_compare\official_rvq_f12_h{head}_hidden.npy"),
            &last_pair,
        )?;
        let logits = music3_rvq_audio_head_rows(&rvq, &last_pair, head, 2).map_err(|e| e.to_string())?;
        write_npy_f32_val(
            &format!(r"C:\ai\music3_compare\native_rvq_f12_h{head}_logits.npy"),
            &logits,
            &[2, MUSIC3_AUDIO_VOCAB],
        )?;
        cmp_named(
            &format!("f12_h{head}_logits"),
            &format!(r"C:\ai\music3_compare\official_rvq_f12_h{head}_logits.npy"),
            &logits,
        )?;
        let (lc, lu) = logits.split_at(logits.len() / 2);
        let guided: Vec<f32> = lc
            .iter()
            .zip(lu.iter())
            .map(|(c, u)| *u + MUSIC3_AR_CFG * (*c - *u))
            .collect();
        write_npy_f32_val(
            &format!(r"C:\ai\music3_compare\native_rvq_f12_h{head}_guided.npy"),
            &guided,
            &[guided.len()],
        )?;
        cmp_named(
            &format!("f12_h{head}_guided"),
            &format!(r"C:\ai\music3_compare\official_rvq_f12_h{head}_guided.npy"),
            &guided,
        )?;
        let mut best = (0usize, f32::NEG_INFINITY);
        for (i, &v) in guided.iter().enumerate() {
            if v > best.1 {
                best = (i, v);
            }
        }
        println!(
            "  native f12 h{head} argmax={} ({:.4}) 654={:.4} 776={:.4} 761={:.4}",
            best.0,
            best.1,
            guided.get(654).copied().unwrap_or(f32::NAN),
            guided.get(776).copied().unwrap_or(f32::NAN),
            guided.get(761).copied().unwrap_or(f32::NAN)
        );
        if head + 1 < 7 {
            let code = official_prefix[head];
            let idx = code as u64 + head as u64 * MUSIC3_AUDIO_VOCAB as u64;
            let emb = rvq
                .tensor_row_f32("audio_embeddings.weight", idx)
                .map_err(|e| e.to_string())?;
            let mut emb2 = Vec::with_capacity(2 * emb.len());
            emb2.extend_from_slice(&emb);
            emb2.extend_from_slice(&emb);
            write_npy_f32_val(
                &format!(r"C:\ai\music3_compare\native_rvq_f12_h{head}_embed.npy"),
                &emb2,
                &[2, MUSIC3_RVQ_HIDDEN],
            )?;
            cmp_named(
                &format!("f12_h{head}_embed"),
                &format!(r"C:\ai\music3_compare\official_rvq_f12_h{head}_embed.npy"),
                &emb2,
            )?;
            let proj = music3_rvq_project_rows(&rvq, &emb2, 2).map_err(|e| e.to_string())?;
            write_npy_f32_val(
                &format!(r"C:\ai\music3_compare\native_rvq_f12_h{head}_proj.npy"),
                &proj,
                &[2, MUSIC3_RVQ_HIDDEN],
            )?;
            cmp_named(
                &format!("f12_h{head}_proj"),
                &format!(r"C:\ai\music3_compare\official_rvq_f12_h{head}_proj.npy"),
                &proj,
            )?;
            seq_c.extend_from_slice(&proj[..MUSIC3_RVQ_HIDDEN]);
            seq_u.extend_from_slice(&proj[MUSIC3_RVQ_HIDDEN..]);
        }
    }
    let _ = music3_rvq_evict();
    println!("  rvqf12 PASS");
    Ok(())
}

fn cmp_named(name: &str, official: &str, native: &[f32]) -> Result<(), String> {
    let path = Path::new(official);
    if !path.exists() {
        println!("  {name} official missing ({official})");
        return Ok(());
    }
    let off = load_npy(path)?.as_f32()?;
    let n = native.len().min(off.len());
    if n == 0 {
        println!("  {name} empty");
        return Ok(());
    }
    let (cos, max_abs, mean_abs) = compare(&native[..n], &off[..n])?;
    println!("  {name} cos={cos:.8} maxabs={max_abs:.6} mean={mean_abs:.6} n={n}");
    Ok(())
}

/// Replay sampled codes (semantic + 7-wide RVQ npy) through the exact
/// `music3_ar_replay` → cond → DiT → vocoder chain and write a wav.
/// Muffled here = the codes themselves are degenerate (sampling/feedback);
/// wideband here = generate's inline fuse/DiT handoff diverges from replay.
fn run_replay_wav(
    weights: &Path,
    dump: &Path,
    sem_path: &Path,
    rvq_path: &Path,
    seed: u64,
    out: &Path,
) -> Result<(), String> {
    let ids = load_npy(&dump.join("text_ids.npy"))?;
    let t = ids.shape[1];
    let vals = ids.as_i64()?;
    let cond: Vec<u32> = vals[..t].iter().map(|&v| v as u32).collect();
    let sem = load_npy(sem_path)?;
    let semantic: Vec<u32> = sem.as_i64()?.iter().map(|&v| v as u32).collect();
    let rvq = load_npy(rvq_path)?;
    let resid: Vec<u32> = rvq.as_i64()?.iter().map(|&v| v as u32).collect();
    if semantic.is_empty() || resid.len() != semantic.len() * (MUSIC3_NUM_CODEBOOKS - 1) {
        return Err(format!(
            "replaywav codes semantic={} residual={}",
            semantic.len(),
            resid.len()
        ));
    }
    let lm = Music3Shards::load(weights.join("language_model")).map_err(|e| e.to_string())?;
    let lm_prep = Music3LmPrepared::prepare(&lm).map_err(|e| e.to_string())?;
    let rvq_w = Music3Shards::load(weights.join("rvq_depth_decoder")).map_err(|e| e.to_string())?;
    let rvq_prep = Music3RvqPrepared::prepare(&rvq_w).map_err(|e| e.to_string())?;
    let t0 = Instant::now();
    let hiddens = music3_ar_replay(&lm, &lm_prep, &rvq_w, &rvq_prep, &cond, &semantic, &resid)
        .map_err(|e| e.to_string())?;
    let frames = music3_ar_emitted_frames(&hiddens);
    println!(
        "  replay codes={} emitted={frames} {:.2}s",
        semantic.len(),
        t0.elapsed().as_secs_f64()
    );
    let _ = music3_lm_evict();
    let _ = music3_rvq_evict();
    let audio = music3_render_hiddens(weights, &hiddens, seed, &mut |_, _| {})
        .map_err(|e| e.to_string())?;
    let (left, right) = music3_planar_stereo(&audio).map_err(|e| e.to_string())?;
    let n = left.len().min(right.len());
    let mut stereo = Vec::with_capacity(2 * n);
    stereo.extend_from_slice(&left[..n]);
    stereo.extend_from_slice(&right[..n]);
    write_wav_i16(out, &stereo, n, MUSIC3_SAMPLE_RATE as u32)?;
    let peak = stereo.iter().fold(0.0f32, |a, &v| a.max(v.abs()));
    let rms = (stereo.iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>()
        / stereo.len().max(1) as f64)
        .sqrt();
    println!(
        "  wrote {} samples={} peak={peak:.3} rms={rms:.4} total {:.2}s",
        out.display(),
        n,
        t0.elapsed().as_secs_f64()
    );
    Ok(())
}

fn run_generate(
    weights: &Path,
    seconds: f64,
    seed: u64,
    out: &Path,
    caption: &str,
    lyrics: &str,
) -> Result<(), String> {
    // MAKEPAD_MUSIC3_BENCH_RUNS=N: run 1 warm-up + N timed generates in-process
    // (weights stay GPU-cached), print each wall and the median. Matches the
    // python protocol: time the generate call only, wav write excluded.
    let bench_runs: usize = std::env::var("MAKEPAD_MUSIC3_BENCH_RUNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let req = Music3Generate {
        caption: caption.to_string(),
        lyrics: lyrics.to_string(),
        seconds,
        seed,
    };
    let t0 = Instant::now();
    let mut audio = music3_generate(weights, &req).map_err(|e| e.to_string())?;
    if bench_runs > 0 {
        println!(
            "GENBENCH run=0 wall={:.3}s samples={} (warm-up, discarded)",
            t0.elapsed().as_secs_f64(),
            audio.len() / 2
        );
        let mut walls = Vec::new();
        for run in 1..=bench_runs {
            let t = Instant::now();
            audio = music3_generate(weights, &req).map_err(|e| e.to_string())?;
            let w = t.elapsed().as_secs_f64();
            println!(
                "GENBENCH run={run} wall={w:.3}s samples={}",
                audio.len() / 2
            );
            walls.push(w);
        }
        walls.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        println!(
            "GENBENCH median={:.3}s over {} timed runs",
            walls[walls.len() / 2],
            walls.len()
        );
    }
    let (left, right) = music3_planar_stereo(&audio).map_err(|e| e.to_string())?;
    let n = left.len().min(right.len());
    let mut stereo = Vec::with_capacity(2 * n);
    stereo.extend_from_slice(&left[..n]);
    stereo.extend_from_slice(&right[..n]);
    write_wav_i16(out, &stereo, n, MUSIC3_SAMPLE_RATE as u32)?;
    let peak = stereo.iter().fold(0.0f32, |a, &v| a.max(v.abs()));
    let rms = (stereo.iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>()
        / stereo.len().max(1) as f64)
        .sqrt();
    println!(
        "  wrote {} samples={} peak={peak:.3} rms={rms:.4} {:.2}s",
        out.display(),
        n,
        t0.elapsed().as_secs_f64()
    );
    Ok(())
}

fn write_wav_i16(path: &Path, stereo: &[f32], _frames: usize, sr: u32) -> Result<(), String> {
    let n = stereo.len() / 2;
    let mut pcm = Vec::with_capacity(n * 4);
    for i in 0..n {
        for ch in 0..2 {
            let s = (stereo[ch * n + i].clamp(-1.0, 1.0) * 32767.0) as i16;
            pcm.extend_from_slice(&s.to_le_bytes());
        }
    }
    let mut buf = Vec::new();
    buf.extend_from_slice(b"RIFF");
    let size = 36 + pcm.len() as u32;
    buf.extend_from_slice(&size.to_le_bytes());
    buf.extend_from_slice(b"WAVEfmt ");
    buf.extend_from_slice(&16u32.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes());
    buf.extend_from_slice(&2u16.to_le_bytes());
    buf.extend_from_slice(&sr.to_le_bytes());
    buf.extend_from_slice(&(sr * 4).to_le_bytes());
    buf.extend_from_slice(&4u16.to_le_bytes());
    buf.extend_from_slice(&16u16.to_le_bytes());
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    buf.extend_from_slice(&pcm);
    std::fs::write(path, buf).map_err(|e| e.to_string())
}

/// Tiny extractor for a top-level JSON string field.
fn json_string(text: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let rest = text.split(&needle).nth(1)?;
    let rest = rest.trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    if !rest.starts_with('"') {
        return None;
    }
    let mut out = String::new();
    let mut chars = rest[1..].chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    let code = u32::from_str_radix(&hex, 16).ok()?;
                    out.push(char::from_u32(code)?);
                }
                other => out.push(other),
            },
            '"' => return Some(out),
            other => out.push(other),
        }
    }
    None
}
