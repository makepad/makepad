//! Perf measurement vs the reference numbers (run explicitly):
//!   cargo test -p makepad-remesh --release --test bench -- --ignored --nocapture
//!
//! Bars: H100 reference totals 0.70s @512^3 (encode 0.52 + decode 0.17);
//! reference-on-this-mac (polyfilled CPU oracle) is far slower (36-238s @512).

mod common;

use common::{dump_tri_soup, faithc_ref_dir, FcDump};
use makepad_remesh::{decode, encode, EncodeOptions, TriangulationMode};

fn bench_asset(name: &str, res: u32, reps: usize) {
    let Some(dir) = faithc_ref_dir() else { return };
    let path = dir.join("dumps").join(format!("{name}_r{res}.bin"));
    if !path.is_file() {
        eprintln!("SKIP {name}_r{res}");
        return;
    }
    let dump = FcDump::load(&path).unwrap();
    let tris = dump_tri_soup(&dump);
    let opts = EncodeOptions::default();

    // warmup + best-of-reps (steady state, honest for a pipeline tool)
    let mut enc_best = f64::INFINITY;
    let mut dec_best = f64::INFINITY;
    let mut k = 0;
    let mut nf = 0;
    for _ in 0..reps {
        let t0 = std::time::Instant::now();
        let tokens = encode(&tris, res, &opts);
        let te = t0.elapsed().as_secs_f64();
        let t1 = std::time::Instant::now();
        let dec = decode(
            res,
            &tokens.voxel_indices,
            &tokens.anchors,
            &tokens.flux,
            Some(&tokens.normals),
            TriangulationMode::Auto,
        );
        let td = t1.elapsed().as_secs_f64();
        enc_best = enc_best.min(te);
        dec_best = dec_best.min(td);
        k = tokens.voxel_indices.len();
        nf = dec.faces.len();
    }
    println!(
        "{name}_r{res}: tris={} K={k} faces={nf} encode={enc_best:.3}s decode={dec_best:.3}s total={:.3}s",
        tris.len(),
        enc_best + dec_best
    );
}

#[test]
#[ignore = "perf: run explicitly"]
fn bench_all() {
    println!("threads={}", makepad_csg_math::thread_pool::thread_count());
    for (name, res, reps) in [
        ("corgi_traveller", 128u32, 5usize),
        ("light_bulb", 128, 5),
        ("cloth", 128, 5),
        ("pirateship", 128, 5),
        ("cloth", 512, 3),
        ("pirateship", 512, 3),
    ] {
        bench_asset(name, res, reps);
    }
}
