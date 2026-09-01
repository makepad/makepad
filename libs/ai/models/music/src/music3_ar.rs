//! Autoregressive semantic loop: LM KV-cache decode + RVQ depth per frame.
//! Replay-codes path feeds dump semantic tokens / residual codes. Sampled
//! AR uses official `_sample_top_k` (cond top-50, softmax, torch CUDA
//! Gumbel-max / Philox multinomial).

use crate::music3::{
    MUSIC3_COND_HIDDEN, MUSIC3_COND_LAYERS, MUSIC3_NUM_CODEBOOKS, MUSIC3_RVQ_HIDDEN,
};
use crate::music3_lm::{
    music3_embed_audio_frame, music3_lm_head, music3_lm_head_audio_host,
    Music3LmPrepared, Music3LmSession,
};
use crate::backend::{gpu_concat_rows, gpu_download, gpu_slice_rows, gpu_upload};
use crate::music3_rvq::{
    music3_rvq_audio_head, music3_rvq_depth_replay, music3_rvq_forward,
    music3_rvq_project, rvq_audio_head_t, rvq_forward_batch_t, rvq_project_t,
    Music3RvqPrepared,
};
use crate::music3_weights::Music3Shards;
use crate::{DiffusionError, Result};

/// Replay dump codes. `semantic_tokens` are raw LM sample ids (offset
/// included). `rvq_codes` is `[frames_in, 7]`. Returns fused per-emitted-frame
/// hiddens `[emitted, 8 * 4096]` (global last + 7 residual steps).
pub fn music3_ar_replay(
    lm: &Music3Shards,
    lm_prep: &Music3LmPrepared,
    rvq: &Music3Shards,
    rvq_prep: &Music3RvqPrepared,
    cond_ids: &[u32],
    semantic_tokens: &[u32],
    rvq_codes: &[u32],
) -> Result<Vec<f32>> {
    let n_in = semantic_tokens.len();
    if n_in == 0 || rvq_codes.len() != n_in * (MUSIC3_NUM_CODEBOOKS - 1) {
        return Err(DiffusionError::model(format!(
            "ar replay: {} semantic vs {} residual",
            n_in,
            rvq_codes.len()
        )));
    }
    let (mut session, mut last) = Music3LmSession::prefill(lm, lm_prep, cond_ids)?;
    let mut hiddens = Vec::new();
    for i in 0..n_in {
        let sem_tok = semantic_tokens[i];
        let resid = &rvq_codes[i * (MUSIC3_NUM_CODEBOOKS - 1)..(i + 1) * (MUSIC3_NUM_CODEBOOKS - 1)];
        let sem_embed = lm.tensor_row_f32(
            "model.embed_tokens.weight",
            sem_tok as u64,
        )?;
        let depth = music3_rvq_depth_replay(rvq, rvq_prep, &last, &sem_embed, resid)?;
        if i > 0 {
            hiddens.extend_from_slice(&last);
            hiddens.extend_from_slice(&depth);
            // Dummy first sample is not emitted; dump has N codes and N-1 frames.
            if music3_ar_emitted_frames(&hiddens) >= n_in.saturating_sub(1) {
                break;
            }
        }
        if i + 1 == n_in {
            break;
        }
        let feedback = music3_embed_audio_frame(lm, rvq, sem_tok, resid)?;
        last = session.step_embeds(lm, lm_prep, &feedback)?;
    }
    Ok(hiddens)
}

/// Count emitted frames in a fused hidden buffer.
pub fn music3_ar_emitted_frames(hiddens: &[f32]) -> usize {
    let width = MUSIC3_COND_LAYERS * MUSIC3_COND_HIDDEN;
    if width == 0 {
        0
    } else {
        hiddens.len() / width
    }
}

/// Sample residual codes (7 × 1024) from the depth decoder. Not torch-bit
/// exact; seed is a product RNG, same distribution family as official top-k.
pub fn music3_rvq_depth_sample(
    weights: &Music3Shards,
    prepared: &Music3RvqPrepared,
    last_hidden: &[f32],
    semantic_embed: &[f32],
    rng: &mut TorchPhilox,
    top_k: usize,
) -> Result<(Vec<u32>, Vec<f32>)> {
    use crate::music3::MUSIC3_AUDIO_VOCAB;
    if last_hidden.len() != MUSIC3_RVQ_HIDDEN || semantic_embed.len() != MUSIC3_RVQ_HIDDEN {
        return Err(DiffusionError::model("rvq depth sample shapes"));
    }
    let p0 = music3_rvq_project(weights, last_hidden)?;
    let p1 = music3_rvq_project(weights, semantic_embed)?;
    let mut seq = Vec::with_capacity(MUSIC3_NUM_CODEBOOKS * MUSIC3_RVQ_HIDDEN);
    seq.extend_from_slice(&p0);
    seq.extend_from_slice(&p1);
    let mut codes = Vec::with_capacity(MUSIC3_NUM_CODEBOOKS - 1);
    let mut parts = Vec::with_capacity((MUSIC3_NUM_CODEBOOKS - 1) * MUSIC3_RVQ_HIDDEN);
    for head in 0..(MUSIC3_NUM_CODEBOOKS - 1) {
        let n = seq.len() / MUSIC3_RVQ_HIDDEN;
        let out = music3_rvq_forward(weights, prepared, &seq, n)?;
        let last = &out[(n - 1) * MUSIC3_RVQ_HIDDEN..];
        parts.extend_from_slice(last);
        let logits = music3_rvq_audio_head(weights, last, head)?;
        let code = sample_top_k(&logits, top_k, rng);
        codes.push(code as u32);
        if head + 1 < MUSIC3_NUM_CODEBOOKS - 1 {
            let idx = code as u64 + head as u64 * MUSIC3_AUDIO_VOCAB as u64;
            let embed = weights.tensor_row_f32("audio_embeddings.weight", idx)?;
            let proj = music3_rvq_project(weights, &embed)?;
            seq.extend_from_slice(&proj);
        }
    }
    Ok((codes, parts))
}

/// Official RVQ: cond+uncond last-hidden, CFG 1.5 on each residual head,
/// one shared code, cond-row hiddens only (`encoders._generate_depth_codes`).
fn music3_rvq_depth_sample_cfg(
    weights: &Music3Shards,
    prepared: &Music3RvqPrepared,
    cond_hidden: &[f32],
    uncond_hidden: &[f32],
    semantic_embed: &[f32],
    rng: &mut TorchPhilox,
    top_k: usize,
) -> Result<(Vec<u32>, Vec<f32>)> {
    use crate::music3::{MUSIC3_AR_CFG, MUSIC3_AUDIO_VOCAB};
    if cond_hidden.len() != MUSIC3_RVQ_HIDDEN
        || uncond_hidden.len() != MUSIC3_RVQ_HIDDEN
        || semantic_embed.len() != MUSIC3_RVQ_HIDDEN
    {
        return Err(DiffusionError::model("rvq cfg depth sample shapes"));
    }
    // Official `_generate_depth_codes`: projection / decoder / heads are
    // one forward on the [cond, uncond] pair (`[2, seq]`), not two graphs.
    // Device-resident: the depth sequence, per-head last rows, and next-row
    // projections stay on the GPU — same kernels, same `[2n, H]` batches,
    // same values (f32 host round-trips are exact); only the ~17 per-frame
    // host syncs shrink to 7 logit downloads + 1 deferred parts download.
    let mut last_both = Vec::with_capacity(2 * MUSIC3_RVQ_HIDDEN);
    last_both.extend_from_slice(cond_hidden);
    last_both.extend_from_slice(uncond_hidden);
    let last_pair_t = gpu_upload(&last_both, 2, MUSIC3_RVQ_HIDDEN)
        .map_err(DiffusionError::model)?;
    let p0 = rvq_project_t(weights, &last_pair_t)?;
    let mut sem_both = Vec::with_capacity(2 * MUSIC3_RVQ_HIDDEN);
    sem_both.extend_from_slice(semantic_embed);
    sem_both.extend_from_slice(semantic_embed);
    let sem_pair_t = gpu_upload(&sem_both, 2, MUSIC3_RVQ_HIDDEN)
        .map_err(DiffusionError::model)?;
    let p1 = rvq_project_t(weights, &sem_pair_t)?;
    let mut seq_c = gpu_concat_rows(
        &gpu_slice_rows(&p0, 0, 1).map_err(DiffusionError::model)?,
        &gpu_slice_rows(&p1, 0, 1).map_err(DiffusionError::model)?,
    )
    .map_err(DiffusionError::model)?;
    let mut seq_u = gpu_concat_rows(
        &gpu_slice_rows(&p0, 1, 1).map_err(DiffusionError::model)?,
        &gpu_slice_rows(&p1, 1, 1).map_err(DiffusionError::model)?,
    )
    .map_err(DiffusionError::model)?;
    let mut codes = Vec::with_capacity(MUSIC3_NUM_CODEBOOKS - 1);
    let mut last_rows: Vec<crate::backend::GpuTensor> =
        Vec::with_capacity(MUSIC3_NUM_CODEBOOKS - 1);
    for head in 0..(MUSIC3_NUM_CODEBOOKS - 1) {
        let n = seq_c.rows();
        let input = gpu_concat_rows(&seq_c, &seq_u).map_err(DiffusionError::model)?;
        let out = rvq_forward_batch_t(weights, prepared, &input, n, 2)?;
        let last_c = gpu_slice_rows(&out, n - 1, 1).map_err(DiffusionError::model)?;
        let last_u = gpu_slice_rows(&out, 2 * n - 1, 1).map_err(DiffusionError::model)?;
        let last_pair = gpu_concat_rows(&last_c, &last_u).map_err(DiffusionError::model)?;
        last_rows.push(last_c);
        let logits_pair = gpu_download(&rvq_audio_head_t(weights, &last_pair, head)?)
            .map_err(DiffusionError::model)?;
        let (logits_c, logits_u) = logits_pair.split_at(logits_pair.len() / 2);
        let mut guided = logits_c.to_vec();
        for (c, u) in guided.iter_mut().zip(logits_u.iter()) {
            *c = *u + MUSIC3_AR_CFG * (*c - *u);
        }
        if let Ok(path) = std::env::var("MAKEPAD_MUSIC3_DUMP_RVQ_LOGITS") {
            if head == 2 {
                static HEAD2: std::sync::atomic::AtomicUsize =
                    std::sync::atomic::AtomicUsize::new(0);
                let which = HEAD2.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let mut idx: Vec<usize> = (0..guided.len()).collect();
                idx.sort_by(|&a, &b| {
                    guided[b]
                        .partial_cmp(&guided[a])
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                let top: Vec<String> = idx
                    .iter()
                    .take(8)
                    .map(|&i| format!("{i}:{:.4}", guided[i]))
                    .collect();
                let line = format!("frame={which} head={head} top8={}\n", top.join(","));
                let _ = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .and_then(|mut f| {
                        use std::io::Write;
                        f.write_all(line.as_bytes())
                    });
                if which == 10 || which == 12 {
                    let npy = format!(
                        "{}_f{which}.npy",
                        path.trim_end_matches(".txt")
                    );
                    let _ = write_npy_f32(&npy, &guided, &[guided.len()]);
                    eprintln!("dumped rvq logits frame{which} head2 -> {npy}");
                }
            }
        }
        let code = sample_top_k(&guided, top_k, rng);
        codes.push(code as u32);
        if head + 1 < MUSIC3_NUM_CODEBOOKS - 1 {
            let idx = code as u64 + head as u64 * MUSIC3_AUDIO_VOCAB as u64;
            let embed = weights.tensor_row_f32("audio_embeddings.weight", idx)?;
            let mut embed_pair = Vec::with_capacity(2 * embed.len());
            embed_pair.extend_from_slice(&embed);
            embed_pair.extend_from_slice(&embed);
            let embed_t = gpu_upload(&embed_pair, 2, MUSIC3_RVQ_HIDDEN)
                .map_err(DiffusionError::model)?;
            let proj = rvq_project_t(weights, &embed_t)?;
            seq_c = gpu_concat_rows(
                &seq_c,
                &gpu_slice_rows(&proj, 0, 1).map_err(DiffusionError::model)?,
            )
            .map_err(DiffusionError::model)?;
            seq_u = gpu_concat_rows(
                &seq_u,
                &gpu_slice_rows(&proj, 1, 1).map_err(DiffusionError::model)?,
            )
            .map_err(DiffusionError::model)?;
        }
    }
    // One deferred download for the 7 depth last-token rows (`parts`).
    let mut acc = gpu_slice_rows(&last_rows[0], 0, 1).map_err(DiffusionError::model)?;
    for row in &last_rows[1..] {
        acc = gpu_concat_rows(&acc, row).map_err(DiffusionError::model)?;
    }
    let parts = gpu_download(&acc).map_err(DiffusionError::model)?;
    Ok((codes, parts))
}

/// Reference-audio constraint for the semantic draw (`music3.md`): on every
/// `interval`-th EMITTED frame inside the encoded clip, the `c0` sample is
/// restricted to the reference encoder's top-5 semantic codes. The seven
/// acoustic books are never constrained — they stay MiniMax's. Built from
/// `music3_reference::Music3ReferenceEncoding::c0_topk`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Music3ReferenceMask {
    /// Per reference frame, semantic codes `0..16384` (no vocab offset).
    pub c0_topk: Vec<[u32; crate::music3_reference::MUSIC3_REFERENCE_CANDIDATES]>,
    /// `1..=10`: 1 constrains every frame, 10 every tenth.
    pub interval: usize,
}

impl Music3ReferenceMask {
    /// Candidates for emitted frame `frame` (the AR warm-up draw is frame
    /// -1 and never constrained, as in the adapter). `None` = free draw.
    pub fn candidates(
        &self,
        frame: usize,
    ) -> Option<&[u32; crate::music3_reference::MUSIC3_REFERENCE_CANDIDATES]> {
        if self.interval == 0 || frame % self.interval != 0 {
            return None;
        }
        self.c0_topk.get(frame)
    }
}

/// Apply a reference mask to CFG-guided semantic logits, after the cond
/// top-50 cut and before sampling. `guided` / `raw` share one layout (band
/// or full vocab; `index_of` maps a GLOBAL vocab id into it); `raw` is the
/// same CFG value before the top-50 cut. Rule: keep `audio_end` as it is
/// (EOS must still be able to fire) and the five candidates; everything
/// else goes to -inf. If none of the candidates survived the top-50 cut,
/// the candidates are restored from `raw` — the clip is never silently
/// ignored. Sampling then runs the same Philox draw as the free path, so
/// unconstrained frames stay bit-identical to text-only generation.
pub(crate) fn apply_reference_mask(
    guided: &mut [f32],
    raw: &[f32],
    index_of: &dyn Fn(u32) -> Option<usize>,
    candidates: &[u32; crate::music3_reference::MUSIC3_REFERENCE_CANDIDATES],
    end: u32,
    lo: u32,
) {
    const NONE: usize = usize::MAX;
    let mut keep = [NONE; 1 + crate::music3_reference::MUSIC3_REFERENCE_CANDIDATES];
    keep[0] = index_of(end).filter(|i| *i < guided.len()).unwrap_or(NONE);
    let mut survives = false;
    for (slot, code) in candidates.iter().enumerate() {
        let idx = index_of(lo.wrapping_add(*code)).filter(|i| *i < guided.len());
        keep[slot + 1] = idx.unwrap_or(NONE);
        if let Some(i) = idx {
            if guided[i].is_finite() {
                survives = true;
            }
        }
    }
    for (i, v) in guided.iter_mut().enumerate() {
        if !keep.contains(&i) {
            *v = f32::NEG_INFINITY;
        }
    }
    if !survives {
        for &i in &keep[1..] {
            if i != NONE && i < raw.len() {
                guided[i] = raw[i];
            }
        }
    }
}

/// Sampled AR: cond+uncond KV decode, CFG 1.5, top-k 50. Stops on
/// `<|audio_end|>` after `min_frames`, or at `max_frames`.
pub fn music3_ar_sample(
    lm: &Music3Shards,
    lm_prep: &Music3LmPrepared,
    rvq: &Music3Shards,
    rvq_prep: &Music3RvqPrepared,
    cond_ids: &[u32],
    uncond_ids: &[u32],
    max_frames: usize,
    min_frames: usize,
    seed: u64,
) -> Result<Vec<f32>> {
    music3_ar_sample_with_progress(
        lm,
        lm_prep,
        rvq,
        rvq_prep,
        cond_ids,
        uncond_ids,
        max_frames,
        min_frames,
        seed,
        None,
        &|| false,
        &mut |_, _, _| {},
    )
}

/// Same as [`music3_ar_sample`]. `progress` is `(stage, done, total)`:
/// `prefill-cond` / `prefill-uncond` over 36 layers, then `lm` over frames.
/// `reference` constrains the semantic draw on its frames (see
/// [`Music3ReferenceMask`]); `None` is the text-only path, unchanged.
pub fn music3_ar_sample_with_progress(
    lm: &Music3Shards,
    lm_prep: &Music3LmPrepared,
    rvq: &Music3Shards,
    rvq_prep: &Music3RvqPrepared,
    cond_ids: &[u32],
    uncond_ids: &[u32],
    max_frames: usize,
    min_frames: usize,
    seed: u64,
    reference: Option<&Music3ReferenceMask>,
    should_cancel: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(&str, usize, usize),
) -> Result<Vec<f32>> {
    use crate::music3::{
        MUSIC3_AR_CFG, MUSIC3_AR_TOP_K, MUSIC3_AUDIO_CODE_OFFSET, MUSIC3_AUDIO_END_TOKEN_ID,
        MUSIC3_SEMANTIC_VOCAB,
    };
    if cond_ids.len() != uncond_ids.len() || cond_ids.is_empty() {
        return Err(DiffusionError::model("ar sample: cfg pair length"));
    }
    // Official `language_model.model` is one forward on batch 2 (cond+uncond):
    // pair prefill, then in-place pair decode over reserved caches.
    let t_prefill = std::time::Instant::now();
    let (mut cond_s, mut cond_h, mut uncond_s, mut uncond_h) =
        Music3LmSession::prefill_pair_with_progress(
            lm,
            lm_prep,
            cond_ids,
            uncond_ids,
            &mut |k, n| progress("prefill-cond", k, n),
        )?;
    if std::env::var_os("MAKEPAD_MUSIC3_BENCH").is_some() {
        eprintln!("BENCH prefill_wall={:.3}", t_prefill.elapsed().as_secs_f64());
    }
    // Reserve once so decode appends the new K/V row in place. Without this
    // every frame regrows all four caches — O(n²) copy traffic over a
    // 1500-frame song. The appended rows are copied verbatim, so the
    // attention input bits are unchanged (--stage decodepair).
    cond_s.reserve(max_frames + 2)?;
    uncond_s.reserve(max_frames + 2)?;
    let mut rng = TorchPhilox::new(seed);
    let mut hiddens = Vec::new();
    let mut semantic_out: Vec<u32> = Vec::new();
    let mut rvq_out: Vec<u32> = Vec::new();
    let lo = MUSIC3_AUDIO_CODE_OFFSET as usize;
    let hi = lo + MUSIC3_SEMANTIC_VOCAB;
    let end = MUSIC3_AUDIO_END_TOKEN_ID as usize;
    let mut ns_head = 0u128;
    let mut ns_rvq = 0u128;
    let mut ns_step = 0u128;
    let trace = std::env::var_os("MAKEPAD_MUSIC3_TRACE_TOKENS").is_some();
    progress("lm", 0, max_frames);
    // Official SemanticGenerationStep: dummy first decode is not emitted
    // (`range(max_frames+1)`, emit if frame_index>0). Sample is cond top-50
    // then guided; RVQ is m=2 CFG; stop on the first <|audio_end|>.
    let _ = min_frames;
    let force_no_eos = std::env::var_os("MAKEPAD_MUSIC3_NO_EOS").is_some();
    // MAKEPAD_MUSIC3_FORCE_SEM / _RVQ (whitespace-separated int files) +
    // MAKEPAD_MUSIC3_FORCE_K: teacher-force the first K frames with dump
    // codes, then free-run. Measurement gate for free-run content drift; the
    // semantic RNG still advances so the continuation stays a valid draw.
    let force: Option<(Vec<u32>, Vec<u32>, usize)> = match (
        std::env::var("MAKEPAD_MUSIC3_FORCE_SEM"),
        std::env::var("MAKEPAD_MUSIC3_FORCE_RVQ"),
        std::env::var("MAKEPAD_MUSIC3_FORCE_K"),
    ) {
        (Ok(sp), Ok(rp), Ok(kp)) => {
            let parse = |p: &str| -> Vec<u32> {
                std::fs::read_to_string(p)
                    .unwrap_or_default()
                    .split_whitespace()
                    .filter_map(|t| t.parse().ok())
                    .collect()
            };
            let k: usize = kp.parse().unwrap_or(0);
            let fs = parse(&sp);
            let fr = parse(&rp);
            eprintln!("FORCE_PREFIX k={k} sem={} rvq={}", fs.len(), fr.len());
            Some((fs, fr, k))
        }
        _ => None,
    };
    // Band head: lm_head only over `audio_end` + the semantic codebook
    // (16 385 rows) instead of the 200 k vocab — the full-path mask kills
    // everything else before sampling anyway, and `sample_top_k_ids` draws
    // Philox by the GLOBAL vocab index, so the packed vector samples
    // bit-identically to the full head. Full head stays for the
    // DUMP_GUIDED bias instrument or `MAKEPAD_MUSIC3_FULL_HEAD=1`.
    let full_head = std::env::var_os("MAKEPAD_MUSIC3_DUMP_GUIDED").is_some()
        || std::env::var_os("MAKEPAD_MUSIC3_FULL_HEAD").is_some();
    let band_ids: Vec<u32> = std::iter::once(end as u32)
        .chain((lo as u32)..(hi as u32))
        .collect();
    // Map a global vocab index into the band vector (band path only).
    let band_index = |global: usize| -> Option<usize> {
        if global == end {
            Some(0)
        } else if global >= lo && global < hi {
            Some(global - lo + 1)
        } else {
            None
        }
    };
    for frame_index in 0..=max_frames {
        if should_cancel() {
            return Err(DiffusionError::Cancelled);
        }
        let t0 = std::time::Instant::now();
        // Emitted frame = frame_index - 1; the warm-up draw is free.
        let mask_candidates = reference
            .and_then(|mask| frame_index.checked_sub(1).and_then(|f| mask.candidates(f)));
        let guided = if full_head {
            let mut cond_logits = music3_lm_head(lm, &cond_h)?;
            let mut uncond_logits = music3_lm_head(lm, &uncond_h)?;
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
            let thresh = kth_largest(&cond_logits, MUSIC3_AR_TOP_K);
            let mut guided = cond_logits.clone();
            let mut raw = mask_candidates.map(|_| Vec::with_capacity(guided.len()));
            for (g, (c, u)) in guided
                .iter_mut()
                .zip(cond_logits.iter().zip(uncond_logits.iter()))
            {
                let v = *u + MUSIC3_AR_CFG * (*c - *u);
                if let Some(raw) = raw.as_mut() {
                    raw.push(v);
                }
                *g = if *c < thresh { f32::NEG_INFINITY } else { v };
            }
            for (i, v) in guided.iter_mut().enumerate() {
                if i != end && !(i >= lo && i < hi) {
                    *v = f32::NEG_INFINITY;
                }
            }
            if let (Some(candidates), Some(raw)) = (mask_candidates, raw.as_ref()) {
                apply_reference_mask(
                    &mut guided,
                    raw,
                    &|global| Some(global as usize),
                    candidates,
                    end as u32,
                    lo as u32,
                );
            }
            // MAKEPAD_MUSIC3_DUMP_GUIDED=<dir>: dump the vocab-masked
            // cond+uncond logits every 5th frame (bias measurement leg).
            if frame_index % 5 == 0 {
                if let Ok(dir) = std::env::var("MAKEPAD_MUSIC3_DUMP_GUIDED") {
                    let mut both = cond_logits.clone();
                    both.extend_from_slice(&uncond_logits);
                    let out = format!("{dir}/native_logits_f{frame_index}.npy");
                    if let Err(err) = write_npy_f32(&out, &both, &[2, cond_logits.len()]) {
                        eprintln!("dump guided f{frame_index}: {err}");
                    }
                }
            }
            guided
        } else {
            let (end_c, band_c) = music3_lm_head_audio_host(lm, &cond_h)?;
            let (end_u, band_u) = music3_lm_head_audio_host(lm, &uncond_h)?;
            let mut cond_logits = Vec::with_capacity(1 + band_c.len());
            cond_logits.push(end_c);
            cond_logits.extend_from_slice(&band_c);
            let mut uncond_logits = Vec::with_capacity(1 + band_u.len());
            uncond_logits.push(end_u);
            uncond_logits.extend_from_slice(&band_u);
            let thresh = kth_largest(&cond_logits, MUSIC3_AR_TOP_K);
            let mut guided = cond_logits;
            let mut raw = mask_candidates.map(|_| Vec::with_capacity(guided.len()));
            for (g, u) in guided.iter_mut().zip(uncond_logits.iter()) {
                let c = *g;
                let v = *u + MUSIC3_AR_CFG * (c - *u);
                if let Some(raw) = raw.as_mut() {
                    raw.push(v);
                }
                *g = if c < thresh { f32::NEG_INFINITY } else { v };
            }
            if let (Some(candidates), Some(raw)) = (mask_candidates, raw.as_ref()) {
                apply_reference_mask(
                    &mut guided,
                    raw,
                    &|global| band_index(global as usize),
                    candidates,
                    end as u32,
                    lo as u32,
                );
            }
            guided
        };
        let mut tok = if full_head {
            sample_top_k(&guided, MUSIC3_AR_TOP_K, &mut rng) as u32
        } else {
            band_ids[sample_top_k_ids(&guided, &band_ids, MUSIC3_AR_TOP_K, &mut rng)]
        };
        if let Some((fs, _, k)) = force.as_ref() {
            if frame_index < *k && frame_index < fs.len() {
                tok = fs[frame_index];
            }
        }
        semantic_out.push(tok);
        ns_head += t0.elapsed().as_nanos();
        if trace && frame_index < 64 {
            let finite = guided.iter().filter(|v| v.is_finite()).count();
            let mut best = (0usize, f32::NEG_INFINITY);
            for (i, &v) in guided.iter().enumerate() {
                if v.is_finite() && v > best.1 {
                    best = (i, v);
                }
            }
            let argmax = if full_head {
                best.0
            } else {
                band_ids[best.0] as usize
            };
            eprintln!(
                "ARTOK frame={frame_index} tok={tok} argmax={argmax} finite={finite} eos={}",
                tok == MUSIC3_AUDIO_END_TOKEN_ID
            );
        }
        if tok == MUSIC3_AUDIO_END_TOKEN_ID && !force_no_eos {
            break;
        }
        let semantic = if tok == MUSIC3_AUDIO_END_TOKEN_ID {
            lo as u32
        } else {
            tok
        };
        let t1 = std::time::Instant::now();
        let sem_embed = lm.tensor_row_f32("model.embed_tokens.weight", semantic as u64)?;
        {
            const DUMP_FRAMES: &[usize] = &[0, 1, 2, 3, 4, 5, 8, 10, 12, 15];
            if DUMP_FRAMES.contains(&frame_index) {
                if let Ok(path) = std::env::var("MAKEPAD_MUSIC3_DUMP_HIDDEN_F10") {
                    let out = path.replace("f10", &format!("f{frame_index}"));
                    let mut both = cond_h.clone();
                    both.extend_from_slice(&uncond_h);
                    if let Err(err) = write_npy_f32(&out, &both, &[2, cond_h.len()]) {
                        eprintln!("dump hidden f{frame_index}: {err}");
                    } else {
                        eprintln!("dumped last_hidden f{frame_index} -> {out}");
                    }
                }
            }
            if frame_index == 12 || frame_index == 15 {
                let mut best = (0usize, f32::NEG_INFINITY);
                for (i, &v) in guided.iter().enumerate() {
                    if v.is_finite() && v > best.1 {
                        best = (i, v);
                    }
                }
                let argmax = if full_head {
                    best.0
                } else {
                    band_ids[best.0] as usize
                };
                // Read `guided` at a GLOBAL vocab index in either layout.
                let at = |global: usize| -> f32 {
                    if full_head {
                        guided.get(global).copied().unwrap_or(f32::NAN)
                    } else {
                        band_index(global)
                            .and_then(|i| guided.get(i).copied())
                            .unwrap_or(f32::NEG_INFINITY)
                    }
                };
                let a = at(155120);
                let b = at(156729);
                let c = at(166934);
                eprintln!(
                    "SAMPLED_LOGITS f{frame_index} tok={tok} argmax={argmax} a155120={a:.4} b156729={b:.4} c166934={c:.4} gap_ab={:.4} off_logit={:.4}",
                    a - b,
                    at(if frame_index == 12 { 156729 } else { 155120 }),
                );
                if let Ok(dir) = std::env::var("MAKEPAD_MUSIC3_DUMP_SEMANTIC") {
                    let out = dir.replace("native_semantic_codes.npy", &format!("native_logits_f{frame_index}.npy"));
                    // Band layout scatters into a vocab-sized -inf vector so
                    // the npy matches the full-head dump byte-for-byte.
                    let full: Vec<f32>;
                    let dump_slice: &[f32] = if full_head {
                        &guided
                    } else {
                        let mut v = vec![f32::NEG_INFINITY; crate::music3::MUSIC3_LM_VOCAB];
                        for (i, &g) in band_ids.iter().enumerate() {
                            v[g as usize] = guided[i];
                        }
                        full = v;
                        &full
                    };
                    if let Err(err) = write_npy_f32(&out, dump_slice, &[dump_slice.len()]) {
                        eprintln!("dump logits f{frame_index}: {err}");
                    } else {
                        eprintln!("dumped logits f{frame_index} -> {out}");
                    }
                }
            }
        }
        let forced_resid = force.as_ref().and_then(|(_, fr, k)| {
            let w = MUSIC3_NUM_CODEBOOKS - 1;
            if frame_index < *k && (frame_index + 1) * w <= fr.len() {
                Some(fr[frame_index * w..(frame_index + 1) * w].to_vec())
            } else {
                None
            }
        });
        let (resid, depth) = if let Some(fresid) = forced_resid {
            let depth = music3_rvq_depth_replay(rvq, rvq_prep, &cond_h, &sem_embed, &fresid)?;
            (fresid, depth)
        } else {
            music3_rvq_depth_sample_cfg(
                rvq,
                rvq_prep,
                &cond_h,
                &uncond_h,
                &sem_embed,
                &mut rng,
                MUSIC3_AR_TOP_K,
            )?
        };
        rvq_out.extend_from_slice(&resid);
        if trace && frame_index < 16 {
            eprintln!("ARRVQ frame={frame_index} codes={resid:?}");
        }
        if frame_index > 0 {
            hiddens.extend_from_slice(&cond_h);
            hiddens.extend_from_slice(&depth);
        }
        ns_rvq += t1.elapsed().as_nanos();
        let done = music3_ar_emitted_frames(&hiddens);
        if done == 1 || done == max_frames || done % 25 == 0 {
            let n = done.max(1) as u128;
            progress(
                &format!(
                    "AR {}/{} head={}ms rvq={}ms step={}ms",
                    done,
                    max_frames,
                    (ns_head / n) / 1_000_000,
                    (ns_rvq / n) / 1_000_000,
                    (ns_step / n) / 1_000_000
                ),
                done,
                max_frames,
            );
        } else {
            progress("lm", done, max_frames);
        }
        if done >= max_frames {
            break;
        }
        let t2 = std::time::Instant::now();
        let feedback = music3_embed_audio_frame(lm, rvq, semantic, &resid)?;
        {
            const DUMP_FRAMES: &[usize] = &[0, 1, 2, 3, 4, 5, 8, 10, 12, 15];
            if DUMP_FRAMES.contains(&frame_index) {
                if let Ok(path) = std::env::var("MAKEPAD_MUSIC3_DUMP_HIDDEN_F10") {
                    let out = path
                        .replace("last_hidden_f10", &format!("feedback_f{frame_index}"))
                        .replace("f10", &format!("f{frame_index}"));
                    let mut both = feedback.clone();
                    both.extend_from_slice(&feedback);
                    if let Err(err) = write_npy_f32(&out, &both, &[2, feedback.len()]) {
                        eprintln!("dump feedback f{frame_index}: {err}");
                    } else {
                        eprintln!("dumped feedback f{frame_index} -> {out}");
                    }
                }
            }
        }
        let pair = Music3LmSession::step_embeds_pair(
            &mut cond_s,
            &mut uncond_s,
            lm,
            lm_prep,
            &feedback,
            &feedback,
        )?;
        cond_h = pair.0;
        uncond_h = pair.1;
        ns_step += t2.elapsed().as_nanos();
    }
    if std::env::var_os("MAKEPAD_MUSIC3_BENCH").is_some() {
        let frames = music3_ar_emitted_frames(&hiddens).max(1) as u128;
        eprintln!(
            "BENCH ar_split head_ms={:.2} rvq_ms={:.2} step_ms={:.2} frames={frames}",
            (ns_head / frames) as f64 / 1.0e6,
            (ns_rvq / frames) as f64 / 1.0e6,
            (ns_step / frames) as f64 / 1.0e6,
        );
    }
    if let Ok(path) = std::env::var("MAKEPAD_MUSIC3_DUMP_SEMANTIC") {
        let vals: Vec<i64> = semantic_out.iter().map(|&t| t as i64).collect();
        if let Err(err) = write_npy_i64(&path, &vals, &[vals.len()]) {
            eprintln!("dump semantic {path}: {err}");
        } else if trace {
            eprintln!("dumped semantic {} -> {path}", vals.len());
        }
    }
    if let Ok(path) = std::env::var("MAKEPAD_MUSIC3_DUMP_RVQ") {
        let width = MUSIC3_NUM_CODEBOOKS - 1;
        let n = if width == 0 { 0 } else { rvq_out.len() / width };
        let vals: Vec<i64> = rvq_out.iter().map(|&t| t as i64).collect();
        if let Err(err) = write_npy_i64(&path, &vals, &[n, width]) {
            eprintln!("dump rvq {path}: {err}");
        } else if trace {
            eprintln!("dumped rvq {n}x{width} -> {path}");
        }
    }
    if trace {
        eprintln!("ARSEMANTIC n={} {:?}", semantic_out.len(), semantic_out);
    }
    if hiddens.is_empty() {
        return Err(DiffusionError::workflow("music3 AR produced no frames"));
    }
    Ok(hiddens)
}

fn write_npy_i64(path: &str, data: &[i64], shape: &[usize]) -> std::io::Result<()> {
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
        "{{'descr': '<i8', 'fortran_order': False, 'shape': {shape_txt}, }}"
    );
    // npy v1: 6 magic + 1 ver + 1 minor + 2 header_len + header, 64-aligned.
    let prefix = 10usize;
    let mut header_len = dict.len() + 1;
    let pad = (16 - ((prefix + header_len) % 16)) % 16;
    header_len += pad;
    dict.push_str(&" ".repeat(pad));
    dict.push('\n');
    let mut f = std::fs::File::create(path)?;
    f.write_all(b"\x93NUMPY\x01\x00")?;
    f.write_all(&(header_len as u16).to_le_bytes())?;
    f.write_all(dict.as_bytes())?;
    for v in data {
        f.write_all(&v.to_le_bytes())?;
    }
    Ok(())
}

fn write_npy_f32(path: &str, data: &[f32], shape: &[usize]) -> std::io::Result<()> {
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
    let mut f = std::fs::File::create(path)?;
    f.write_all(b"\x93NUMPY\x01\x00")?;
    f.write_all(&(header_len as u16).to_le_bytes())?;
    f.write_all(dict.as_bytes())?;
    for v in data {
        f.write_all(&v.to_le_bytes())?;
    }
    Ok(())
}

fn kth_largest(xs: &[f32], k: usize) -> f32 {
    let mut vals: Vec<f32> = xs.iter().copied().filter(|v| v.is_finite()).collect();
    if vals.is_empty() {
        return f32::NEG_INFINITY;
    }
    let k = k.max(1).min(vals.len());
    vals.select_nth_unstable_by(k - 1, |a, b| {
        b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal)
    });
    vals[k - 1]
}

/// Torch 2.11 CUDA `Generator.manual_seed` + `multinomial` for n_sample=1:
/// Gumbel-max with Philox4x32-10, subsequence = flat index, one counter step
/// per draw. Matches official `_sample_top_k` on this box (seed 7 first
/// semantic = 152931, uniform-1024 = 564).
pub struct TorchPhilox {
    seed: u64,
    counter: u64,
}

impl TorchPhilox {
    pub(crate) fn new(seed: u64) -> Self {
        Self { seed, counter: 0 }
    }

    pub(crate) fn uniform_at(&self, index: u64) -> f32 {
        let word = philox4x32_10(self.seed, index, self.counter)[0];
        (word >> 8) as f32 * (1.0 / 16_777_216.0)
    }

    pub(crate) fn next_counter(&mut self) {
        self.counter = self.counter.wrapping_add(1);
    }
}

/// Standard Box-Muller N(0,1). The previous DiT init used Rayleigh * ±1,
/// which is not Gaussian and cannot lock onto official latents.
pub(crate) fn torch_randn(seed: u64, n: usize) -> Vec<f32> {
    let rng = TorchPhilox::new(seed);
    let mut out = vec![0f32; n];
    let pairs = (n + 1) / 2;
    for p in 0..pairs {
        let u1 = rng.uniform_at((2 * p) as u64).max(1e-12);
        let u2 = rng.uniform_at((2 * p + 1) as u64);
        let r = (-2.0 * u1.ln()).sqrt();
        let a = 2.0 * std::f32::consts::PI * u2;
        let i0 = 2 * p;
        out[i0] = r * a.cos();
        let i1 = i0 + 1;
        if i1 < n {
            out[i1] = r * a.sin();
        }
    }
    out
}

fn philox4x32_10(seed: u64, subsequence: u64, counter: u64) -> [u32; 4] {
    const M0: u64 = 0xd251_1f53;
    const M1: u64 = 0xcd9e_8d57;
    const W0: u32 = 0x9e37_79b9;
    const W1: u32 = 0xbb67_ae85;
    let mut value = [
        counter as u32,
        (counter >> 32) as u32,
        subsequence as u32,
        (subsequence >> 32) as u32,
    ];
    let mut key = [seed as u32, (seed >> 32) as u32];
    for _ in 0..10 {
        let product0 = M0.wrapping_mul(value[0] as u64);
        let product1 = M1.wrapping_mul(value[2] as u64);
        value = [
            (product1 >> 32) as u32 ^ value[1] ^ key[0],
            product1 as u32,
            (product0 >> 32) as u32 ^ value[3] ^ key[1],
            product0 as u32,
        ];
        key[0] = key[0].wrapping_add(W0);
        key[1] = key[1].wrapping_add(W1);
    }
    value
}

fn nan_to_num_logit(v: f32) -> f32 {
    if v.is_nan() {
        -1.0e9
    } else if v.is_infinite() {
        if v.is_sign_positive() {
            1.0e9
        } else {
            -1.0e9
        }
    } else {
        v
    }
}

/// Official `_sample_top_k`: nan_to_num, top-k mask, softmax, CUDA multinomial.
pub(crate) fn sample_top_k(logits: &[f32], k: usize, rng: &mut TorchPhilox) -> usize {
    if logits.is_empty() {
        rng.next_counter();
        return 0;
    }
    let k = k.max(1).min(logits.len());
    let mut values: Vec<f32> = logits.iter().copied().map(nan_to_num_logit).collect();
    let thresh = {
        let mut sorted = values.clone();
        let t = k - 1;
        sorted.select_nth_unstable_by(t, |a, b| {
            b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal)
        });
        sorted[t]
    };
    for v in &mut values {
        if *v < thresh {
            *v = f32::NEG_INFINITY;
        }
    }
    let max = values
        .iter()
        .copied()
        .filter(|v| v.is_finite())
        .fold(f32::NEG_INFINITY, f32::max);
    if !max.is_finite() {
        rng.next_counter();
        return 0;
    }
    let mut best_i = 0usize;
    let mut best_s = f32::NEG_INFINITY;
    let mut sum = 0.0f32;
    let mut weights: Vec<(usize, f32)> = Vec::with_capacity(k);
    for (i, &v) in values.iter().enumerate() {
        if !v.is_finite() {
            continue;
        }
        let w = (v - max).exp();
        if w.is_finite() && w > 0.0 {
            sum += w;
            weights.push((i, w));
        }
    }
    if weights.is_empty() || sum <= 0.0 {
        rng.next_counter();
        return 0;
    }
    for &(i, w) in &weights {
        let p = w / sum;
        let mut u = rng.uniform_at(i as u64);
        if u < 1.0e-12 {
            u = 1.0e-12;
        } else if u > 1.0 - 1.0e-7 {
            u = 1.0 - 1.0e-7;
        }
        let expo = -u.ln();
        let score = p.ln() - expo.ln();
        if score > best_s {
            best_s = score;
            best_i = i;
        }
    }
    rng.next_counter();
    best_i
}

/// Same as `sample_top_k`, but Philox subsequence is `ids[i]` (official CUDA
/// multinomial uses the full-vocab index, not the packed audio-slice index).
pub(crate) fn sample_top_k_ids(logits: &[f32], ids: &[u32], k: usize, rng: &mut TorchPhilox) -> usize {
    if logits.is_empty() || ids.len() != logits.len() {
        rng.next_counter();
        return 0;
    }
    let k = k.max(1).min(logits.len());
    let mut values: Vec<f32> = logits.iter().copied().map(nan_to_num_logit).collect();
    let thresh = {
        let mut sorted = values.clone();
        let t = k - 1;
        sorted.select_nth_unstable_by(t, |a, b| {
            b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal)
        });
        sorted[t]
    };
    for v in &mut values {
        if *v < thresh {
            *v = f32::NEG_INFINITY;
        }
    }
    let max = values
        .iter()
        .copied()
        .filter(|v| v.is_finite())
        .fold(f32::NEG_INFINITY, f32::max);
    if !max.is_finite() {
        rng.next_counter();
        return 0;
    }
    let mut best_i = 0usize;
    let mut best_s = f32::NEG_INFINITY;
    let mut sum = 0.0f32;
    let mut weights: Vec<(usize, f32)> = Vec::with_capacity(k);
    for (i, &v) in values.iter().enumerate() {
        if !v.is_finite() {
            continue;
        }
        let w = (v - max).exp();
        if w.is_finite() && w > 0.0 {
            sum += w;
            weights.push((i, w));
        }
    }
    if weights.is_empty() || sum <= 0.0 {
        rng.next_counter();
        return 0;
    }
    for &(i, w) in &weights {
        let p = w / sum;
        let mut u = rng.uniform_at(ids[i] as u64);
        if u < 1.0e-12 {
            u = 1.0e-12;
        } else if u > 1.0 - 1.0e-7 {
            u = 1.0 - 1.0e-7;
        }
        let expo = -u.ln();
        let score = p.ln() - expo.ln();
        if score > best_s {
            best_s = score;
            best_i = i;
        }
    }
    rng.next_counter();
    best_i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn philox_multinomial_matches_torch_cuda_seed7_uniform1024() {
        let logits = vec![0.0f32; 1024];
        let mut rng = TorchPhilox::new(7);
        assert_eq!(sample_top_k(&logits, 1024, &mut rng), 564);
    }

    #[test]
    fn philox_multinomial_matches_torch_cuda_seed7_toy8() {
        let logits = vec![0.0f32; 8];
        let mut rng = TorchPhilox::new(7);
        assert_eq!(sample_top_k(&logits, 8, &mut rng), 0);
    }

    /// Band layout toy: index 0 = audio_end, index i = semantic code i-1.
    fn band(global: u32, end: u32, lo: u32) -> Option<usize> {
        if global == end {
            Some(0)
        } else if global >= lo {
            Some((global - lo) as usize + 1)
        } else {
            None
        }
    }

    #[test]
    fn reference_mask_keeps_candidates_and_eos_only() {
        let (end, lo) = (100u32, 1000u32);
        let raw: Vec<f32> = (0..12).map(|i| i as f32).collect();
        // Top-50 cut already removed code 9 (index 10).
        let mut guided = raw.clone();
        guided[10] = f32::NEG_INFINITY;
        let candidates = [0u32, 2, 4, 6, 9];
        apply_reference_mask(&mut guided, &raw, &|g| band(g, end, lo), &candidates, end, lo);
        // EOS keeps its guided value; candidates 0,2,4,6 keep theirs; 9 was
        // cut by top-50 and stays cut (others survived); the rest is -inf.
        assert_eq!(guided[0], 0.0);
        assert_eq!(guided[1], 1.0);
        assert_eq!(guided[3], 3.0);
        assert_eq!(guided[5], 5.0);
        assert_eq!(guided[7], 7.0);
        assert!(guided[10].is_infinite());
        for i in [2usize, 4, 6, 8, 9, 11] {
            assert!(guided[i].is_infinite(), "index {i} should be masked");
        }
    }

    #[test]
    fn reference_mask_falls_back_to_candidates_when_top50_dropped_them_all() {
        let (end, lo) = (100u32, 1000u32);
        let raw: Vec<f32> = (0..12).map(|i| -(i as f32)).collect();
        let mut guided = raw.clone();
        let candidates = [1u32, 3, 5, 7, 9];
        for c in candidates {
            guided[c as usize + 1] = f32::NEG_INFINITY;
        }
        apply_reference_mask(&mut guided, &raw, &|g| band(g, end, lo), &candidates, end, lo);
        for c in candidates {
            assert_eq!(guided[c as usize + 1], raw[c as usize + 1]);
        }
        assert_eq!(guided[0], raw[0]);
        // Non-candidates (codes 2 and 10 = indices 3 and 11) stay masked.
        assert!(guided[3].is_infinite() && guided[11].is_infinite());
        // A candidate outside the layout is ignored, not a panic.
        let mut guided = raw.clone();
        apply_reference_mask(&mut guided, &raw, &|g| band(g, end, lo), &[0, 50_000, 1, 2, 3], end, lo);
        assert_eq!(guided[1], raw[1]);
    }

    #[test]
    fn reference_mask_frame_rule() {
        let mask = Music3ReferenceMask {
            c0_topk: vec![[1, 2, 3, 4, 5], [6, 7, 8, 9, 10], [11, 12, 13, 14, 15]],
            interval: 2,
        };
        assert_eq!(mask.candidates(0), Some(&[1, 2, 3, 4, 5]));
        assert_eq!(mask.candidates(1), None);
        assert_eq!(mask.candidates(2), Some(&[11, 12, 13, 14, 15]));
        // Past the clip the AR is free again.
        assert_eq!(mask.candidates(4), None);
        let every = Music3ReferenceMask { c0_topk: mask.c0_topk.clone(), interval: 1 };
        assert!(every.candidates(1).is_some());
    }
}
