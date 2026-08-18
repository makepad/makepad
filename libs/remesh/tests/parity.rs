//! End-to-end codec parity vs the reference oracle dumps
//! (author's FCTEncoder/FCTDecoder run unchanged on CPU via the validated
//! polyfill — see local/faithc_ref/make_dumps.py).
//!
//! Bars:
//! - active voxel set AND order: EXACT
//! - flux signs: EXACT
//! - QEF anchors (noclamp): max |d| <= 1e-5 (only LU-solve fp order differs)
//! - normals: fp-close except voxels whose weighted normal sum nearly cancels
//!   (opposite-facing walls in one voxel -> direction is fp noise in ANY
//!   backend, including the reference's own GPU vs CPU); those are counted
//! - clamp stage on identical inputs: bit-close (isolated stage test); the
//!   full clamp pipeline additionally sees input-sensitivity flips where a
//!   ~2e-6 anchor nudge lands on a different closest triangle -> counted
//! - decode from reference tokens: vertices bit-equal; face-split decisions
//!   equal except inside the consistency tie band (|c02-c13| in f64 below
//!   fp32 noise), where torch's own reduction order is the only difference

mod common;

use std::sync::Arc;

use common::{dump_tri_soup, faithc_ref_dir, FcDump};
use makepad_remesh::encoder::clamp_and_project_anchors;
use makepad_remesh::spatial::BinGrid;
use makepad_remesh::{decode, encode, DecodedMesh, EncodeOptions, TriangulationMode};

struct TokenRef {
    voxels: Vec<i64>,
    anchors: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    flux: Vec<[i8; 12]>,
}

fn load_tokens(dump: &FcDump, prefix: &str) -> TokenRef {
    let (_, voxels) = dump.i64(&format!("{prefix}active_voxel_indices"));
    let anchors = dump.v3s(&format!("{prefix}anchor"));
    let normals = dump.v3s(&format!("{prefix}normal"));
    let (fd, flux) = dump.i8(&format!("{prefix}edge_flux_sign"));
    assert_eq!(fd[1], 12);
    TokenRef {
        voxels: voxels.to_vec(),
        anchors,
        normals,
        flux: flux.chunks_exact(12).map(|c| c.try_into().unwrap()).collect(),
    }
}

fn diff3(a: &[f32; 3], b: &[f32; 3]) -> f32 {
    (a[0] - b[0])
        .abs()
        .max((a[1] - b[1]).abs())
        .max((a[2] - b[2]).abs())
}

/// count of elementwise diffs above each threshold + the max
fn diff_stats(a: &[[f32; 3]], b: &[[f32; 3]], thresholds: &[f32]) -> (Vec<usize>, f32) {
    let mut counts = vec![0usize; thresholds.len()];
    let mut mx = 0.0f32;
    for (x, y) in a.iter().zip(b) {
        let d = diff3(x, y);
        mx = mx.max(d);
        for (c, &t) in counts.iter_mut().zip(thresholds) {
            if d > t {
                *c += 1;
            }
        }
    }
    (counts, mx)
}

/// f64 recomputation of the normal_abs consistency separation for one quad.
fn tie_separation(verts: &[[f32; 3]], normals: &[[f32; 3]], quad: [u32; 4]) -> f64 {
    let v: Vec<[f64; 3]> = quad
        .iter()
        .map(|&i| {
            let p = verts[i as usize];
            [p[0] as f64, p[1] as f64, p[2] as f64]
        })
        .collect();
    let n: Vec<[f64; 3]> = quad
        .iter()
        .map(|&i| {
            let p = normals[i as usize];
            [p[0] as f64, p[1] as f64, p[2] as f64]
        })
        .collect();
    let tri_sets: [[[usize; 3]; 2]; 2] = [[[0, 1, 2], [0, 2, 3]], [[0, 1, 3], [1, 2, 3]]];
    let mut cons = [0.0f64; 2];
    for (pi, pat) in tri_sets.iter().enumerate() {
        let mut sum = 0.0f64;
        for tri in pat {
            let a = v[tri[0]];
            let b = v[tri[1]];
            let c = v[tri[2]];
            let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let g = [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                e1[0] * e2[1] - e1[1] * e2[0],
            ];
            let l = (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt().max(1e-9);
            for &vi in tri {
                let d = (g[0] * n[vi][0] + g[1] * n[vi][1] + g[2] * n[vi][2]) / l;
                sum += d.abs();
            }
        }
        cons[pi] = sum / 6.0;
    }
    (cons[0] - cons[1]).abs()
}

/// Compare decoded faces; differing rows must lie in the tie band.
/// Returns the number of differing rows.
fn check_faces(
    label: &str,
    dec: &DecodedMesh,
    dec_normals: &[[f32; 3]],
    ref_faces: &[i64],
) -> usize {
    assert_eq!(dec.faces.len() * 3, ref_faces.len(), "{label}: face count");
    let mut rows_diff = 0usize;
    let mut max_sep = 0.0f64;
    let mut q = 0;
    while q * 2 < dec.faces.len() {
        let r0 = dec.faces[q * 2];
        let r1 = dec.faces[q * 2 + 1];
        let ref0 = [
            ref_faces[q * 6] as u32,
            ref_faces[q * 6 + 1] as u32,
            ref_faces[q * 6 + 2] as u32,
        ];
        let ref1 = [
            ref_faces[q * 6 + 3] as u32,
            ref_faces[q * 6 + 4] as u32,
            ref_faces[q * 6 + 5] as u32,
        ];
        if r0 != ref0 || r1 != ref1 {
            rows_diff += (r0 != ref0) as usize + (r1 != ref1) as usize;
            // reconstruct the quad from our two rows (pattern detection)
            let quad = if r1[0] == r0[0] {
                [r0[0], r0[1], r0[2], r1[2]] // pattern 0
            } else {
                [r0[0], r0[1], r1[1], r0[2]] // pattern 1
            };
            let sep = tie_separation(&dec.vertices, dec_normals, quad);
            max_sep = max_sep.max(sep);
            assert!(
                sep < 1e-6,
                "{label}: quad {q} split differs OUTSIDE tie band (sep {sep:.3e})"
            );
        }
        q += 1;
    }
    if rows_diff > 0 {
        println!(
            "  {label}: {rows_diff}/{} face rows differ, all in tie band (max sep {max_sep:.2e})",
            dec.faces.len()
        );
    }
    // primary bar is the per-quad tie-band assert above; this only guards
    // against wholesale divergence (symmetric assets tie on ~5% of quads)
    let quads = dec.faces.len() / 2;
    assert!(
        rows_diff * 10 <= dec.faces.len().max(1),
        "{label}: {rows_diff} differing rows > 10% of {quads} quads"
    );
    rows_diff
}

fn check_asset(name: &str, res: u32) {
    let Some(dir) = faithc_ref_dir() else {
        eprintln!("SKIP {name}: local/faithc_ref not present");
        return;
    };
    let path = dir.join("dumps").join(format!("{name}_r{res}.bin"));
    if !path.is_file() {
        eprintln!("SKIP {name}_r{res}: dump not present");
        return;
    }
    let dump = FcDump::load(&path).unwrap();
    let tris = dump_tri_soup(&dump);

    let variants: &[(&str, bool)] = if res > 128 {
        &[("", true)]
    } else {
        // noclamp first: isolates the QEF path from the clamp/UDF path
        &[("noclamp_", false), ("", true)]
    };
    for &(prefix, clamp) in variants {
        let r = load_tokens(&dump, prefix);
        let k = r.voxels.len();
        let opts = EncodeOptions {
            clamp_anchors: clamp,
            ..Default::default()
        };
        let ours = encode(&tris, res, &opts);

        // active voxel set AND order exact
        assert_eq!(ours.voxel_indices.len(), k, "{name}_r{res} {prefix}: voxel count");
        assert_eq!(ours.voxel_indices, r.voxels, "{name}_r{res} {prefix}: voxel ids/order");

        // flux EXACT
        let flux_diff = (0..k).filter(|&i| ours.flux[i] != r.flux[i]).count();
        assert_eq!(flux_diff, 0, "{name}_r{res} {prefix}: {flux_diff} voxels differ in flux");

        // anchors
        let (ac, amax) = diff_stats(&ours.anchors, &r.anchors, &[1e-5, 1e-4]);
        // normals: near-cancellation voxels are counted, not bounded
        let (nc, nmax) = diff_stats(&ours.normals, &r.normals, &[2e-5, 1e-4]);
        println!(
            "{name}_r{res} {}: K={k} anchors max|d|={amax:.2e} (>1e-5:{} >1e-4:{}) normals max|d|={nmax:.2e} (>2e-5:{} >1e-4:{})",
            if clamp { "clamp" } else { "noclamp" },
            ac[0], ac[1], nc[0], nc[1]
        );
        if clamp {
            // pipeline: a ~2e-6 pre-clamp nudge may flip the closest triangle
            // for anchors sitting on a voxel face -> bounded COUNT, not size
            assert!(
                ac[1] <= (k / 1000).max(4),
                "{name}_r{res}: {} clamp anchors differ >1e-4 (cap {})",
                ac[1],
                (k / 1000).max(4)
            );
        } else {
            assert!(amax <= 1e-5, "{name}_r{res} {prefix}: anchor diff {amax:.3e}");
        }
        assert!(
            nc[0] <= (k / 5000).max(4),
            "{name}_r{res} {prefix}: {} normals differ >2e-5",
            nc[0]
        );

        // isolated clamp stage on IDENTICAL inputs (reference pre-clamp anchors)
        if clamp && res <= 128 {
            let pre = load_tokens(&dump, "noclamp_");
            let bin_grid = Arc::new(BinGrid::build(&tris));
            let vox = Arc::new(r.voxels.clone());
            let staged = clamp_and_project_anchors(res, &tris, &bin_grid, &vox, &pre.anchors);
            let (sc, smax) = diff_stats(&staged, &r.anchors, &[1e-6, 1e-4]);
            println!(
                "  clamp stage on ref inputs: max|d|={smax:.2e} >1e-6:{} >1e-4:{}",
                sc[0], sc[1]
            );
            assert!(
                sc[0] <= (k / 20000).max(2),
                "{name}_r{res}: clamp stage diverges on identical inputs ({} > 1e-6)",
                sc[0]
            );
        }

        // decode from REFERENCE tokens -> dumped decoded mesh
        let dec = decode(
            res,
            &r.voxels,
            &r.anchors,
            &r.flux,
            Some(&r.normals),
            TriangulationMode::Auto,
        );
        let ref_verts = dump.v3s(&format!("{prefix}decoded_vertices"));
        let (_, ref_faces) = dump.i64(&format!("{prefix}decoded_faces"));
        assert_eq!(dec.vertices.len(), ref_verts.len(), "{name}_r{res} {prefix}: decoded V");
        for (i, (a, b)) in dec.vertices.iter().zip(&ref_verts).enumerate() {
            assert_eq!(
                [a[0].to_bits(), a[1].to_bits(), a[2].to_bits()],
                [b[0].to_bits(), b[1].to_bits(), b[2].to_bits()],
                "{name}_r{res} {prefix}: decoded vertex {i}"
            );
        }
        let dec_normals: Vec<[f32; 3]> = dec
            .used_voxels
            .iter()
            .map(|&u| r.normals[u as usize])
            .collect();
        check_faces(
            &format!("{name}_r{res} {prefix}decode(ref tokens)"),
            &dec,
            &dec_normals,
            ref_faces,
        );

        // decode from OUR tokens: same topology; splits near-identical
        let dec2 = decode(
            res,
            &ours.voxel_indices,
            &ours.anchors,
            &ours.flux,
            Some(&ours.normals),
            TriangulationMode::Auto,
        );
        assert_eq!(dec2.vertices.len(), ref_verts.len());
        assert_eq!(dec2.faces.len() * 3, ref_faces.len());
        let mut rows2 = 0usize;
        for (i, f) in dec2.faces.iter().enumerate() {
            let rf = [
                ref_faces[i * 3] as u32,
                ref_faces[i * 3 + 1] as u32,
                ref_faces[i * 3 + 2] as u32,
            ];
            if *f != rf {
                rows2 += 1;
            }
        }
        println!(
            "  decode(our tokens): V={} F={} rows differing: {rows2}/{}",
            dec2.vertices.len(),
            dec2.faces.len(),
            dec2.faces.len()
        );
        assert!(
            rows2 * 10 <= dec2.faces.len().max(1),
            "{name}_r{res} {prefix}: {rows2} own-token face rows differ"
        );
    }
}

#[test]
fn parity_corgi_128() {
    check_asset("corgi_traveller", 128);
}

#[test]
fn parity_light_bulb_128() {
    check_asset("light_bulb", 128);
}

#[test]
fn parity_cloth_128() {
    check_asset("cloth", 128);
}

#[test]
fn parity_pirateship_128() {
    check_asset("pirateship", 128);
}

#[test]
#[ignore = "large: run explicitly with --release -- --ignored"]
fn parity_cloth_512() {
    check_asset("cloth", 512);
}

#[test]
#[ignore = "large: run explicitly with --release -- --ignored"]
fn parity_pirateship_512() {
    check_asset("pirateship", 512);
}
