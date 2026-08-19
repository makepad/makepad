//! Artifact and checkpoint contracts.
//!
//! * The PLY this crate writes must load with `makepad_splat`, the in-repo
//!   3DGS reader the viewer uses — including the activations that reader
//!   applies (exp on scales, sigmoid on opacity, SH DC -> color), which is
//!   what proves the writer emitted PRE-activation values.
//! * The state-dict contracts are checked against the real released
//!   safetensors headers when `MAKEPAD_SPLAT_WEIGHTS_DIR` points at a
//!   directory holding the pinned files. Without it those checks skip, so
//!   the suite still runs on a machine with no weights.

use makepad_ai_splat::splat::SplatWeights;
use makepad_ai_splat::splat_decoder::decoder_expected_tensors;
use makepad_ai_splat::splat_dino::SplatDino;
use makepad_ai_splat::splat_flow::flow_expected_tensors;
use makepad_ai_splat::splat_ply::{write_ply, PlySplat};
use makepad_ai_splat::splat_rand::{gaussian_offset_perturbation, SplatRng};
use std::path::PathBuf;

fn sample_splats(count: usize) -> Vec<PlySplat> {
    let mut rng = SplatRng::new(5);
    (0..count)
        .map(|_| {
            let mut rotation = [rng.normal(), rng.normal(), rng.normal(), rng.normal()];
            let norm: f32 = rotation.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-6);
            for value in &mut rotation {
                *value /= norm;
            }
            PlySplat {
                position: [rng.normal(), rng.normal(), rng.normal()],
                f_dc: [rng.normal(), rng.normal(), rng.normal()],
                opacity: rng.normal(),
                scale: [-5.0 + rng.normal(), -5.0 + rng.normal(), -5.0 + rng.normal()],
                rotation,
            }
        })
        .collect()
}

#[test]
fn generated_ply_loads_with_the_in_repo_reader() {
    let splats = sample_splats(64);
    let bytes = write_ply(&splats);
    // No path hint: the reader must recognise the PLY from its own magic.
    let scene =
        makepad_splat::load_splat_from_bytes(&bytes, None).expect("in-repo reader must accept it");
    assert_eq!(scene.splats.len(), splats.len());

    const SH_C0: f32 = 0.282_094_8;
    for (loaded, source) in scene.splats.iter().zip(&splats) {
        // Positions pass through unchanged.
        for axis in 0..3 {
            assert!((loaded.position[axis] - source.position[axis]).abs() < 1e-6);
        }
        // The reader exponentiates the scale columns: the writer must have
        // emitted LOG scales.
        for axis in 0..3 {
            assert!(
                (loaded.scale[axis] - source.scale[axis].exp()).abs() < 1e-6,
                "{} vs {}",
                loaded.scale[axis],
                source.scale[axis].exp()
            );
        }
        // ... sigmoids the opacity column: the writer emitted a LOGIT.
        let want_alpha = 1.0 / (1.0 + (-source.opacity).exp());
        assert!((loaded.color[3] - want_alpha).abs() < 1e-6);
        // ... and reads f_dc as the SH DC term.
        for channel in 0..3 {
            let want = (0.5 + SH_C0 * source.f_dc[channel]).clamp(0.0, 1.0);
            assert!((loaded.color[channel] - want).abs() < 1e-6);
        }
        // The reader stores rotations xyzw; the writer emits wxyz.
        assert!((loaded.rotation[3] - source.rotation[0]).abs() < 1e-5);
        assert!((loaded.rotation[0] - source.rotation[1]).abs() < 1e-5);
    }
    // Bounds are recomputed, so a non-degenerate cloud must have extent.
    assert!(scene.bounds_max[0] > scene.bounds_min[0]);
}

#[test]
fn empty_and_single_splat_plys_are_still_valid() {
    let scene = makepad_splat::load_splat_from_bytes(&write_ply(&[]), None).unwrap();
    assert!(scene.splats.is_empty());
    let bytes = write_ply(&sample_splats(1));
    let scene = makepad_splat::load_splat_from_bytes(&bytes, Some(std::path::Path::new("a.ply")))
        .unwrap();
    assert_eq!(scene.splats.len(), 1);
}

/// `MAKEPAD_SPLAT_WEIGHTS_DIR` layout: the pinned files under their HF paths
/// or flat by basename.
fn weights_dir() -> Option<PathBuf> {
    std::env::var_os("MAKEPAD_SPLAT_WEIGHTS_DIR").map(PathBuf::from)
}

fn open(dir: &std::path::Path, name: &str) -> Option<SplatWeights> {
    let flat = dir.join(name);
    if flat.exists() {
        return SplatWeights::load(&flat).ok();
    }
    None
}

fn assert_contract(weights: &SplatWeights, expected: &[(String, Vec<usize>)], label: &str) {
    let mut missing = Vec::new();
    for (name, shape) in expected {
        match weights.dtype_shape(name) {
            Ok((_dtype, actual)) => assert_eq!(
                &actual, shape,
                "{label}: {name} is {actual:?}, the port expects {shape:?}"
            ),
            Err(_) => missing.push(name.clone()),
        }
    }
    assert!(missing.is_empty(), "{label}: missing {missing:?}");
    // Every tensor in the file must be consumed (the DINO repack carries one
    // extra `mask_token` the reference also drops).
    let expected_names: std::collections::HashSet<&str> =
        expected.iter().map(|(n, _)| n.as_str()).collect();
    let unused: Vec<&String> = weights
        .tensor_names()
        .filter(|name| !expected_names.contains(name.as_str()))
        .filter(|name| name.as_str() != "embeddings.mask_token")
        .collect();
    assert!(unused.is_empty(), "{label}: unread tensors {unused:?}");
}

#[test]
fn state_dict_contracts_match_the_released_checkpoints() {
    let Some(dir) = weights_dir() else {
        eprintln!("skipping: set MAKEPAD_SPLAT_WEIGHTS_DIR to check the real headers");
        return;
    };
    let mut checked = 0usize;
    if let Some(weights) = open(&dir, "triposplat_fp16.safetensors") {
        assert_contract(&weights, &flow_expected_tensors(), "flow");
        checked += 1;
    }
    if let Some(weights) = open(&dir, "triposplat_vae_decoder_fp16.safetensors") {
        assert_contract(&weights, &decoder_expected_tensors(), "decoder");
        checked += 1;
        // The Hammersley generator must reproduce the checkpoint's own
        // points_offset_perturbation buffer.
        let stored = weights
            .f32_shaped("gs.points_offset_perturbation", &[32, 3])
            .unwrap();
        let generated = gaussian_offset_perturbation(1.5);
        for (a, b) in stored.iter().zip(&generated) {
            // fp16 storage: compare at fp16 resolution.
            assert!((a - b).abs() < 5e-3, "{a} vs {b}");
        }
    }
    if let Some(weights) = open(&dir, "dino_v3_vit_h.safetensors") {
        assert_contract(&weights, &SplatDino::expected_tensors(), "dino");
        checked += 1;
    }
    // Pointing the variable at the wrong directory must fail loudly rather
    // than silently reporting a green contract.
    assert_eq!(
        checked, 3,
        "MAKEPAD_SPLAT_WEIGHTS_DIR={} held {checked}/3 pinned checkpoints",
        dir.display()
    );
}
