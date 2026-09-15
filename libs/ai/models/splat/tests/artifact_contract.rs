//! Artifact and checkpoint contracts.
//!
//! * The PLY this crate writes must load with `makepad_splat`, the in-repo
//!   3DGS reader the viewer uses — including the activations that reader
//!   applies (exp on scales, sigmoid on opacity, SH DC -> color), which is
//!   what proves the writer emitted PRE-activation values.
use makepad_ai_splat::splat_ply::{write_ply, PlySplat};
use makepad_ai_splat::splat_rand::SplatRng;

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
