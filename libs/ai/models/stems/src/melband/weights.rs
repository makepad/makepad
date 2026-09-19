//! The Mel-Band checkpoint's weight plan.
//!
//! Loading is the four-stem loader ([`StemsWeights::load_plan`]) given this
//! plan instead of its own; the two load-time rewrites it describes — dim
//! reversal for free, the GLU's last `Linear` stored as separate value and
//! gate halves — apply here unchanged.
//!
//! What does NOT carry over is band grouping. The four-stem band table is
//! sorted by width, so equal-width bands stack into seven 3-D weights. These
//! bands come in 39 runs of equal width, 35 of them a single band, so there is
//! little to stack; and a 3-D weight leaves the BLAS path on CUDA for a
//! per-element kernel, which at this mask head's 1536x1536 layer per band
//! would be most of a chunk's time. Every per-band weight therefore stays its
//! own 2-D tensor and every per-band matmul is a plain GEMM.
//!
//! The trunk tensors keep the four-stem graph names (`block{b}.{axis}...`)
//! because the trunk IS the four-stem transformer builder, which looks them
//! up by those names.

use super::config::*;
use crate::config::{DIM, DIM_INNER, FF_INNER, HEADS};
use crate::weights::{
    attn_name, axis_tag, ckpt_transformer, ff_name, item, mat, PlanItem, Source, StemsWeights,
};
use makepad_ai_common::Result;
use std::path::Path;

pub fn band_gamma(band: usize) -> String {
    format!("bandsplit.gamma.b{band}")
}
pub fn band_weight(band: usize) -> String {
    format!("bandsplit.weight.b{band}")
}
pub fn band_bias(band: usize) -> String {
    format!("bandsplit.bias.b{band}")
}
/// The norm a transformer ends in. `axis` is 0 for the time transformer, 1
/// for the frequency transformer.
pub fn norm_name(block: usize, axis: usize) -> String {
    format!("block{block}.{}.norm", axis_tag(axis))
}
pub fn mask_name(band: usize, part: &str) -> String {
    format!("mask.b{band}.{part}")
}

/// Loads the checkpoint into the layout [`super::graph`] consumes.
pub fn load(path: impl AsRef<Path>, extra_bytes: usize, f16: bool) -> Result<StemsWeights> {
    StemsWeights::load_plan(path, &weight_plan(), extra_bytes, f16)
}

/// Every tensor the forward graph reads, in allocation order.
pub fn weight_plan() -> Vec<PlanItem> {
    let mut plan = Vec::new();
    let dim = DIM as i64;

    // -- band split, one (norm scale, linear) per band --
    for band in 0..NUM_BANDS {
        let w = band_width(band) as i64;
        let src = format!("band_split.to_features.{band}");
        plan.push(item(
            band_gamma(band),
            vec![w],
            Source::Whole(format!("{src}.0.gamma")),
        ));
        plan.push(mat(
            band_weight(band),
            vec![w, dim],
            Source::Whole(format!("{src}.1.weight")),
        ));
        plan.push(item(
            band_bias(band),
            vec![dim],
            Source::Whole(format!("{src}.1.bias")),
        ));
    }

    // -- 6 blocks x {time, freq} transformers, one layer each, and the norm
    //    each transformer ends in --
    for block in 0..DEPTH {
        for axis in 0..2 {
            let src = ckpt_transformer(block, axis);
            plan.push(item(
                attn_name(block, axis, "gamma"),
                vec![dim],
                Source::Whole(format!("{src}.0.norm.gamma")),
            ));
            plan.push(mat(
                attn_name(block, axis, "qkv"),
                vec![dim, (DIM_INNER * 3) as i64],
                Source::Whole(format!("{src}.0.to_qkv.weight")),
            ));
            plan.push(mat(
                attn_name(block, axis, "gates_w"),
                vec![dim, HEADS as i64],
                Source::Whole(format!("{src}.0.to_gates.weight")),
            ));
            plan.push(item(
                attn_name(block, axis, "gates_b"),
                vec![HEADS as i64],
                Source::Whole(format!("{src}.0.to_gates.bias")),
            ));
            plan.push(mat(
                attn_name(block, axis, "out"),
                vec![DIM_INNER as i64, dim],
                Source::Whole(format!("{src}.0.to_out.0.weight")),
            ));

            plan.push(item(
                ff_name(block, axis, "gamma"),
                vec![dim],
                Source::Whole(format!("{src}.1.net.0.gamma")),
            ));
            plan.push(mat(
                ff_name(block, axis, "w1"),
                vec![dim, FF_INNER as i64],
                Source::Whole(format!("{src}.1.net.1.weight")),
            ));
            plan.push(item(
                ff_name(block, axis, "b1"),
                vec![FF_INNER as i64],
                Source::Whole(format!("{src}.1.net.1.bias")),
            ));
            plan.push(mat(
                ff_name(block, axis, "w2"),
                vec![FF_INNER as i64, dim],
                Source::Whole(format!("{src}.1.net.4.weight")),
            ));
            plan.push(item(
                ff_name(block, axis, "b2"),
                vec![dim],
                Source::Whole(format!("{src}.1.net.4.bias")),
            ));

            plan.push(item(
                norm_name(block, axis),
                vec![dim],
                Source::Whole(format!("layers.{block}.{axis}.norm.gamma")),
            ));
        }
    }

    // -- the mask estimator, one three-layer MLP per band --
    let hidden = MASK_HIDDEN as i64;
    for band in 0..NUM_BANDS {
        let width = band_width(band);
        let w = width as i64;
        let src = format!("mask_estimators.0.to_freqs.{band}.0");
        plan.push(mat(
            mask_name(band, "w1"),
            vec![dim, hidden],
            Source::Whole(format!("{src}.0.weight")),
        ));
        plan.push(item(
            mask_name(band, "b1"),
            vec![hidden],
            Source::Whole(format!("{src}.0.bias")),
        ));
        plan.push(mat(
            mask_name(band, "w2"),
            vec![hidden, hidden],
            Source::Whole(format!("{src}.2.weight")),
        ));
        plan.push(item(
            mask_name(band, "b2"),
            vec![hidden],
            Source::Whole(format!("{src}.2.bias")),
        ));
        // nn.GLU(dim=-1): first half is the value, second half the gate.
        let last = vec![format!("{src}.4.weight")];
        let last_bias = vec![format!("{src}.4.bias")];
        plan.push(mat(
            mask_name(band, "wv"),
            vec![hidden, w],
            Source::BandRows {
                names: last.clone(),
                rows: (0, width),
                row_len: MASK_HIDDEN,
            },
        ));
        plan.push(mat(
            mask_name(band, "wg"),
            vec![hidden, w],
            Source::BandRows {
                names: last,
                rows: (width, width),
                row_len: MASK_HIDDEN,
            },
        ));
        plan.push(item(
            mask_name(band, "bv"),
            vec![w],
            Source::BandRows {
                names: last_bias.clone(),
                rows: (0, width),
                row_len: 1,
            },
        ));
        plan.push(item(
            mask_name(band, "bg"),
            vec![w],
            Source::BandRows {
                names: last_bias,
                rows: (width, width),
                row_len: 1,
            },
        ));
    }

    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::weights::plan_total_bytes;
    use makepad_ai_loader::formats::torch_pth::PthStateDict;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::PathBuf;

    /// Checkpoint entries the plan reads, with how many floats of each.
    fn floats_read_per_entry() -> BTreeMap<String, usize> {
        let mut read: BTreeMap<String, usize> = BTreeMap::new();
        for item in weight_plan() {
            let floats = item.elements();
            let names = match item.source {
                Source::Whole(name) => vec![name],
                Source::Bands(names) => names,
                Source::BandRows { names, .. } => names,
            };
            assert_eq!(names.len(), 1, "every tensor of this plan has one source");
            *read.entry(names[0].clone()).or_default() += floats;
        }
        read
    }

    #[test]
    fn the_plan_reads_672_checkpoint_tensors() {
        // The published checkpoint has 684 tensors. 12 are the frozen
        // `rotary_embed.freqs` buffers (one per transformer: 6 time + 6
        // freq), which the runtime's `rope` regenerates from ROPE_THETA
        // rather than reading. The plan consumes the other 672: 3 per band
        // for the split, 11 per transformer, 6 per band for the mask head.
        let read = floats_read_per_entry();
        assert_eq!(read.len(), 684 - 12);
        assert_eq!(read.len(), NUM_BANDS * 3 + DEPTH * 2 * 11 + NUM_BANDS * 6);
        assert!(read.keys().all(|name| !name.contains("rotary_embed")));
    }

    #[test]
    fn the_plan_holds_every_parameter_once() {
        // 228,203,172 floats in the file, less the 12 x 32 rope buffers. The
        // GLU split reads each half of the last layer once, so nothing is
        // counted twice.
        let total: usize = weight_plan().iter().map(|i| i.elements()).sum();
        assert_eq!(total, 228_202_788);
        assert_eq!(total, 228_203_172 - 12 * 32);
    }

    #[test]
    fn plan_names_are_unique() {
        let plan = weight_plan();
        let names: BTreeSet<&str> = plan.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names.len(), plan.len());
    }

    #[test]
    fn every_per_band_weight_is_two_dimensional() {
        let plan = weight_plan();
        let find = |name: String| plan.iter().find(|i| i.name == name).unwrap().clone();
        // Band 0 is 7 bins, band 59 is 130.
        assert_eq!(find(band_gamma(0)).extents, vec![28]);
        assert_eq!(find(band_weight(0)).extents, vec![28, 384]);
        assert_eq!(find(band_bias(0)).extents, vec![384]);
        assert_eq!(find(band_weight(59)).extents, vec![520, 384]);
        assert_eq!(find(mask_name(59, "w1")).extents, vec![384, 1536]);
        assert_eq!(find(mask_name(59, "w2")).extents, vec![1536, 1536]);
        assert_eq!(find(mask_name(59, "wv")).extents, vec![1536, 520]);
        assert_eq!(find(mask_name(59, "wg")).extents, vec![1536, 520]);
        assert_eq!(find(mask_name(59, "bg")).extents, vec![520]);
        assert_eq!(find(norm_name(5, 1)).extents, vec![384]);
        for item in &plan {
            assert!(item.extents.len() <= 2, "{} is {:?}", item.name, item.extents);
            assert_eq!(item.is_matmul_weight, item.extents.len() == 2, "{}", item.name);
        }
    }

    #[test]
    fn the_glu_halves_are_the_two_row_ranges_of_the_last_layer() {
        let plan = weight_plan();
        for band in [0, 17, 59] {
            let width = band_width(band);
            for (part, start, row_len) in [
                ("wv", 0, MASK_HIDDEN),
                ("wg", width, MASK_HIDDEN),
                ("bv", 0, 1),
                ("bg", width, 1),
            ] {
                let item = plan.iter().find(|i| i.name == mask_name(band, part)).unwrap();
                match &item.source {
                    Source::BandRows { rows, row_len: len, .. } => {
                        assert_eq!(*rows, (start, width), "band {band} {part}");
                        assert_eq!(*len, row_len, "band {band} {part}");
                    }
                    other => panic!("band {band} {part} is read as {other:?}"),
                }
            }
        }
    }

    #[test]
    fn arena_size_covers_the_weights_plus_headroom() {
        let plan = weight_plan();
        let total = plan_total_bytes(&plan, false, 0).unwrap();
        let params: usize = plan.iter().map(|i| i.elements()).sum();
        assert!(total >= params * 4);
        let half = plan_total_bytes(&plan, true, 0).unwrap();
        assert!(half < total * 55 / 100, "f16 plan is {half} vs f32 {total}");
    }

    fn checkpoint() -> PathBuf {
        match std::env::var_os("MAKEPAD_MELBAND_CKPT") {
            Some(path) => PathBuf::from(path),
            None => PathBuf::from(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../../local/stems_ref/ckpt/MelBandRoformer.ckpt"
            )),
        }
    }

    /// The two counts above are arithmetic on the plan alone. This holds the
    /// plan to the file: every tensor in it is read, whole, and none that is
    /// not in it is asked for.
    #[test]
    fn the_plan_and_the_checkpoint_name_the_same_672_tensors() {
        let path = checkpoint();
        if !path.is_file() {
            eprintln!("SKIP: checkpoint absent (want {})", path.display());
            return;
        }
        let state = PthStateDict::load(&path).expect("read the checkpoint");
        let read = floats_read_per_entry();
        let mut in_file = 0usize;
        let mut floats = 0usize;
        for name in state.names() {
            if name.contains("rotary_embed") {
                continue;
            }
            in_file += 1;
            let shape = state.shape(name).expect("shape");
            let want: usize = shape.iter().product();
            floats += want;
            assert_eq!(read.get(name).copied(), Some(want), "'{name}' {shape:?}");
        }
        assert_eq!(in_file, 672);
        assert_eq!(read.len(), 672);
        assert_eq!(floats, 228_202_788);
    }
}
