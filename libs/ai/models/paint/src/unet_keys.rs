//! Expected state-dict inventory for the pinned Hunyuan3D-Paint-2.1 UNet2p5D
//! checkpoint, and the verifier that compares it against a real tensor index
//! (loaded via [`crate::torch_bin`]) — acceptance gate B1: never claim
//! checkpoint fit from file size; enumerate and match every tensor.
//!
//! Structure (pinned from unet/config.json + modules.py @ the revisions in
//! [`crate::hunyuan`]):
//! * `unet.*` — SD2.1 UNet2DConditionModel (blocks `[320,640,1280,1280]`,
//!   layers_per_block 2, cross-attn 1024, linear projection, GN32) with
//!   `conv_in` widened to 12 input channels;
//! * `unet_dual.*` — the dump-pinned dual-stream reference copy (original 4
//!   input channels; the deepcopy happens before the conv_in swap), included
//!   exactly in the required inventory;
//! * `unet.learned_text_clip_{albedo,mr,ref}` — [77, 1024] learned prompts;
//! * `unet.image_proj_model_dino.*` — DINO feature projector 1536 -> 4x1024;
//! * dump-pinned attention-processor weights (material `_mr` clones,
//!   multiview and reference attention) attached under processor paths.

use std::collections::BTreeMap;

pub const BLOCK_OUT: [usize; 4] = [320, 640, 1280, 1280];
pub const TIME_EMBED_DIM: usize = 1280;
pub const CROSS_DIM: usize = 1024;
pub const MAIN_IN_CHANNELS: usize = 12;
pub const DUAL_IN_CHANNELS: usize = 4;
pub const OUT_CHANNELS: usize = 4;
pub const TEXT_TOKENS: usize = 77;
pub const DINO_DIM: usize = 1536;
pub const DINO_TOKENS: usize = 4;

/// One expected tensor: name and shape (the pinned checkpoint stores fp16).
pub type KeyShape = (String, Vec<usize>);

fn push(out: &mut Vec<KeyShape>, name: String, shape: &[usize]) {
    out.push((name, shape.to_vec()));
}

fn linear(out: &mut Vec<KeyShape>, prefix: &str, rows: usize, cols: usize, bias: bool) {
    push(out, format!("{prefix}.weight"), &[rows, cols]);
    if bias {
        push(out, format!("{prefix}.bias"), &[rows]);
    }
}

fn conv(out: &mut Vec<KeyShape>, prefix: &str, cout: usize, cin: usize, k: usize) {
    push(out, format!("{prefix}.weight"), &[cout, cin, k, k]);
    push(out, format!("{prefix}.bias"), &[cout]);
}

fn norm(out: &mut Vec<KeyShape>, prefix: &str, ch: usize) {
    push(out, format!("{prefix}.weight"), &[ch]);
    push(out, format!("{prefix}.bias"), &[ch]);
}

fn resnet(out: &mut Vec<KeyShape>, prefix: &str, cin: usize, cout: usize) {
    norm(out, &format!("{prefix}.norm1"), cin);
    conv(out, &format!("{prefix}.conv1"), cout, cin, 3);
    linear(out, &format!("{prefix}.time_emb_proj"), cout, TIME_EMBED_DIM, true);
    norm(out, &format!("{prefix}.norm2"), cout);
    conv(out, &format!("{prefix}.conv2"), cout, cout, 3);
    if cin != cout {
        conv(out, &format!("{prefix}.conv_shortcut"), cout, cin, 1);
    }
}

fn attn(out: &mut Vec<KeyShape>, prefix: &str, ch: usize, kv_dim: usize) {
    linear(out, &format!("{prefix}.to_q"), ch, ch, false);
    linear(out, &format!("{prefix}.to_k"), ch, kv_dim, false);
    linear(out, &format!("{prefix}.to_v"), ch, kv_dim, false);
    linear(out, &format!("{prefix}.to_out.0"), ch, ch, true);
}

/// One Transformer2DModel with the Basic2p5DTransformerBlock wrapping
/// observed in the checkpoint dump: the original block sits under
/// `transformer_blocks.0.transformer.*`; the 2.5D additions (material `_mr`
/// processor clones, reference/multiview/DINO attention) sit beside it.
/// `extras=false` reproduces the dual-stream copy (plain wrapped block).
fn transformer(out: &mut Vec<KeyShape>, prefix: &str, ch: usize, extras: bool) {
    norm(out, &format!("{prefix}.norm"), ch);
    linear(out, &format!("{prefix}.proj_in"), ch, ch, true);
    let tb = format!("{prefix}.transformer_blocks.0");
    let inner = format!("{tb}.transformer");
    norm(out, &format!("{inner}.norm1"), ch);
    attn(out, &format!("{inner}.attn1"), ch, ch);
    if extras {
        // Material branch clones on the self-attention processor.
        linear(out, &format!("{inner}.attn1.processor.to_q_mr"), ch, ch, false);
        linear(out, &format!("{inner}.attn1.processor.to_k_mr"), ch, ch, false);
        linear(out, &format!("{inner}.attn1.processor.to_v_mr"), ch, ch, false);
        linear(out, &format!("{inner}.attn1.processor.to_out_mr.0"), ch, ch, true);
    }
    norm(out, &format!("{inner}.norm2"), ch);
    attn(out, &format!("{inner}.attn2"), ch, CROSS_DIM);
    norm(out, &format!("{inner}.norm3"), ch);
    linear(out, &format!("{inner}.ff.net.0.proj"), 8 * ch, ch, true);
    linear(out, &format!("{inner}.ff.net.2"), ch, 4 * ch, true);
    if extras {
        attn(out, &format!("{tb}.attn_refview"), ch, ch);
        linear(out, &format!("{tb}.attn_refview.processor.to_v_mr"), ch, ch, false);
        linear(out, &format!("{tb}.attn_refview.processor.to_out_mr.0"), ch, ch, true);
        attn(out, &format!("{tb}.attn_multiview"), ch, ch);
        attn(out, &format!("{tb}.attn_dino"), ch, CROSS_DIM);
    }
    linear(out, &format!("{prefix}.proj_out"), ch, ch, true);
}

/// The exact wrapped-UNet key set for the pinned config, parameterized on
/// conv_in channels (12 main / 4 dual) and 2.5D extras (main only).
pub fn base_unet_keys(in_channels: usize, extras: bool) -> Vec<KeyShape> {
    let mut out = Vec::with_capacity(700);
    conv(&mut out, "conv_in", BLOCK_OUT[0], in_channels, 3);
    linear(&mut out, "time_embedding.linear_1", TIME_EMBED_DIM, BLOCK_OUT[0], true);
    linear(&mut out, "time_embedding.linear_2", TIME_EMBED_DIM, TIME_EMBED_DIM, true);

    // Down path: CrossAttn x3 + plain DownBlock2D, two resnets each,
    // downsampler on all but the last block.
    let mut cin = BLOCK_OUT[0];
    for (i, &cout) in BLOCK_OUT.iter().enumerate() {
        let prefix = format!("down_blocks.{i}");
        for r in 0..2 {
            let rin = if r == 0 { cin } else { cout };
            resnet(&mut out, &format!("{prefix}.resnets.{r}"), rin, cout);
        }
        if i < 3 {
            for a in 0..2 {
                transformer(&mut out, &format!("{prefix}.attentions.{a}"), cout, extras);
            }
            conv(&mut out, &format!("{prefix}.downsamplers.0.conv"), cout, cout, 3);
        }
        cin = cout;
    }

    // Mid block.
    resnet(&mut out, "mid_block.resnets.0", 1280, 1280);
    transformer(&mut out, "mid_block.attentions.0", 1280, extras);
    resnet(&mut out, "mid_block.resnets.1", 1280, 1280);

    // Up path (reversed): UpBlock2D + CrossAttn x3, three resnets each,
    // upsampler on all but the last. Skip-concat input channels follow the
    // standard diffusers accounting.
    let rev: Vec<usize> = BLOCK_OUT.iter().rev().copied().collect(); // [1280,1280,640,320]
    for (i, &cout) in rev.iter().enumerate() {
        let prefix = format!("up_blocks.{i}");
        let prev = if i == 0 { 1280 } else { rev[i - 1] };
        // Skip channels come from the down path, consumed in reverse.
        let skips: [usize; 3] = match i {
            0 => [1280, 1280, 1280],
            1 => [1280, 1280, 640],
            2 => [640, 640, 320],
            _ => [320, 320, 320],
        };
        for r in 0..3 {
            let rin = if r == 0 { prev } else { cout };
            resnet(
                &mut out,
                &format!("{prefix}.resnets.{r}"),
                rin + skips[r],
                cout,
            );
        }
        if i > 0 {
            for a in 0..3 {
                transformer(&mut out, &format!("{prefix}.attentions.{a}"), cout, extras);
            }
        }
        if i < 3 {
            conv(&mut out, &format!("{prefix}.upsamplers.0.conv"), cout, cout, 3);
        }
    }

    norm(&mut out, "conv_norm_out", BLOCK_OUT[0]);
    conv(&mut out, "conv_out", OUT_CHANNELS, BLOCK_OUT[0], 3);
    out
}

/// The full expected inventory of the checkpoint (dump-verified: main
/// wrapped UNet with extras + dual wrapped UNet without extras + top-level
/// conditioning modules; all tensors fp16).
pub fn expected_keys() -> Vec<KeyShape> {
    let mut out = Vec::with_capacity(1800);
    for (name, shape) in base_unet_keys(MAIN_IN_CHANNELS, true) {
        out.push((format!("unet.{name}"), shape));
    }
    for (name, shape) in base_unet_keys(DUAL_IN_CHANNELS, false) {
        out.push((format!("unet_dual.{name}"), shape));
    }
    for token in ["albedo", "mr", "ref"] {
        push(
            &mut out,
            format!("unet.learned_text_clip_{token}"),
            &[TEXT_TOKENS, CROSS_DIM],
        );
    }
    linear(
        &mut out,
        "unet.image_proj_model_dino.proj",
        DINO_TOKENS * CROSS_DIM,
        DINO_DIM,
        true,
    );
    norm(&mut out, "unet.image_proj_model_dino.norm", CROSS_DIM);
    out
}

/// Parse the on-box dump format `name|storage|shape|stride|offset|key|numel`
/// into a verifier inventory plus the storage-class set.
pub fn parse_dump(text: &str) -> (Vec<(String, Vec<usize>)>, Vec<String>) {
    let mut inventory = Vec::new();
    let mut storage_classes = std::collections::BTreeSet::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split('|').collect();
        if fields.len() < 3 {
            continue;
        }
        let shape: Vec<usize> = if fields[2].is_empty() {
            Vec::new()
        } else {
            fields[2].split(',').filter_map(|d| d.parse().ok()).collect()
        };
        storage_classes.insert(fields[1].to_string());
        inventory.push((fields[0].to_string(), shape));
    }
    (inventory, storage_classes.into_iter().collect())
}

pub fn param_count(keys: &[KeyShape]) -> usize {
    keys.iter()
        .map(|(_, shape)| shape.iter().product::<usize>())
        .sum()
}

/// Verifier report against a real tensor inventory (name -> shape).
#[derive(Debug, Default)]
pub struct KeyReport {
    pub matched: usize,
    pub missing: Vec<String>,
    pub shape_mismatch: Vec<(String, Vec<usize>, Vec<usize>)>,
    /// Repeated names make an inventory ambiguous even if both shapes match.
    pub duplicates: Vec<String>,
    /// Present in the checkpoint but not in the non-processor expectation,
    /// and matching an attention-processor path — pinned from the dump.
    pub processor_extras: Vec<String>,
    /// `unet_dual.*` keys, if the checkpoint stores the reference copy after
    /// all (contradicting the payload-size analysis) — surfaced distinctly.
    pub dual_extras: Vec<String>,
    /// Present in the checkpoint, not expected, not a processor path.
    pub unexpected: Vec<String>,
}

impl KeyReport {
    pub fn clean(&self) -> bool {
        self.missing.is_empty()
            && self.shape_mismatch.is_empty()
            && self.duplicates.is_empty()
            && self.processor_extras.is_empty()
            && self.dual_extras.is_empty()
            && self.unexpected.is_empty()
    }
}

pub fn verify_inventory(inventory: &[(String, Vec<usize>)]) -> KeyReport {
    let expected: BTreeMap<String, Vec<usize>> = expected_keys().into_iter().collect();
    let mut report = KeyReport::default();
    let mut seen = std::collections::BTreeSet::new();
    for (name, shape) in inventory {
        if !seen.insert(name.clone()) {
            report.duplicates.push(name.clone());
            continue;
        }
        match expected.get(name) {
            Some(want) if want == shape => report.matched += 1,
            Some(want) => report
                .shape_mismatch
                .push((name.clone(), want.clone(), shape.clone())),
            None => {
                if name.starts_with("unet_dual.") {
                    report.dual_extras.push(name.clone());
                } else if name.contains(".processor")
                    || name.contains("_mr")
                    || name.contains("attn_multiview")
                    || name.contains("attn_refview")
                    || name.contains("attn_dino")
                {
                    report.processor_extras.push(name.clone());
                } else {
                    report.unexpected.push(name.clone());
                }
            }
        }
    }
    for name in expected.keys() {
        if !seen.contains(name) {
            report.missing.push(name.clone());
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_unet_parameter_count_is_sd21_sized() {
        // SD2.1 UNet is ~865.9M parameters at 4 input channels; the widened
        // 12-channel conv_in adds exactly 8*320*3*3 = 23040 weights.
        let base4 = param_count(&base_unet_keys(4, false));
        let base12 = param_count(&base_unet_keys(12, false));
        assert_eq!(base12 - base4, 8 * 320 * 3 * 3);
        assert!(
            (860_000_000..875_000_000).contains(&base4),
            "base UNet params {base4}"
        );
    }

    #[test]
    fn expected_inventory_is_structurally_consistent() {
        let keys = expected_keys();
        // No duplicate names.
        let mut names: Vec<&String> = keys.iter().map(|(n, _)| n).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), keys.len(), "duplicate key names");
        let main_conv = keys.iter().find(|(n, _)| n == "unet.conv_in.weight").unwrap();
        assert_eq!(main_conv.1, vec![320, 12, 3, 3]);
        // The stored dual copy keeps the original 4-channel conv_in.
        let dual_conv = keys
            .iter()
            .find(|(n, _)| n == "unet_dual.conv_in.weight")
            .unwrap();
        assert_eq!(dual_conv.1, vec![320, 4, 3, 3]);
        // Output head and learned prompts.
        let conv_out = keys.iter().find(|(n, _)| n == "unet.conv_out.weight").unwrap();
        assert_eq!(conv_out.1, vec![4, 320, 3, 3]);
        let clip = keys
            .iter()
            .find(|(n, _)| n == "unet.learned_text_clip_mr")
            .unwrap();
        assert_eq!(clip.1, vec![77, 1024]);
        let dino = keys
            .iter()
            .find(|(n, _)| n == "unet.image_proj_model_dino.proj.weight")
            .unwrap();
        assert_eq!(dino.1, vec![4096, 1536]);
        // Dump-pinned totals: 1747 fp16 tensors; the parameter sum must fit
        // the 3,925,293,863-byte archive at 2 bytes/param (the remainder is
        // zip + pickle overhead, under 4 MB).
        assert_eq!(keys.len(), 1747, "expected key count");
        let total = param_count(&keys);
        let payload_f16 = 3_925_293_863usize / 2;
        assert!(total < payload_f16, "{total} exceeds the fp16 payload");
        assert!(
            payload_f16 - total < 2_000_000,
            "unaccounted payload {} params",
            payload_f16 - total
        );
    }

    #[test]
    fn verifier_reports_each_defect_class() {
        let mut inventory = expected_keys();
        // Remove one -> missing; corrupt one -> mismatch; add strays.
        let removed = inventory.remove(5);
        inventory[0].1 = vec![1, 2, 3];
        let corrupted = inventory[0].0.clone();
        inventory.push(("unet.something_else.weight".to_string(), vec![1]));
        inventory.push((
            "unet.down_blocks.0.attentions.0.transformer_blocks.0.attn1.processor.to_q_mr.weight"
                .to_string(),
            vec![320, 320],
        ));
        inventory.push(inventory[1].clone());
        let report = verify_inventory(&inventory);
        assert!(!report.clean());
        assert!(report.missing.contains(&removed.0));
        assert_eq!(report.shape_mismatch.len(), 1);
        assert_eq!(report.shape_mismatch[0].0, corrupted);
        assert_eq!(report.unexpected, vec!["unet.something_else.weight".to_string()]);
        assert_eq!(report.processor_extras.len(), 1);
        assert_eq!(report.duplicates.len(), 1);
    }

    #[test]
    fn clean_inventory_verifies_clean() {
        let inventory = expected_keys();
        let report = verify_inventory(&inventory);
        assert!(report.clean());
        assert_eq!(report.matched, inventory.len());
        assert!(report.processor_extras.is_empty());
    }
}
