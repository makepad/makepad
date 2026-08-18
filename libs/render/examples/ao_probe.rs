//! Bake cost and AO distribution over the real Kenney catalogue.
//!
//!   cargo run -p makepad-render --release --example ao_probe
//!
//! Reports parse+bake time per model and library-wide, and how much of each
//! model actually darkens — a bake that leaves everything at 1.0 is doing
//! nothing, and one that drives everything to the floor is mud.

use std::time::Instant;

use makepad_render::model::{StaticModel, MODEL_VERTEX_FLOATS};

fn main() {
    let root = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "apps/sandbox/resources/models/kenney".to_string());
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    collect(std::path::Path::new(&root), &mut files);
    files.sort();
    if files.is_empty() {
        eprintln!("no .glb under {root} — run apps/sandbox/download_assets.sh");
        return;
    }

    let limit: usize = std::env::var("AO_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(usize::MAX);

    let mut total_ms = 0.0f64;
    let mut total_verts = 0usize;
    let mut total_tris = 0usize;
    let mut done = 0usize;
    let mut worst: Vec<(f64, String, usize)> = Vec::new();
    // Distribution of the baked AO term across every vertex in the library.
    let mut buckets = [0usize; 5];

    for f in files.iter().take(limit) {
        let Ok(bytes) = std::fs::read(f) else { continue };
        let t = Instant::now();
        let Ok(m) = StaticModel::parse_glb(&bytes) else { continue };
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        total_ms += ms;
        total_verts += m.vertex_count();
        total_tris += m.triangle_count();
        done += 1;
        worst.push((ms, f.file_name().unwrap().to_string_lossy().into(), m.vertex_count()));

        for i in 0..m.vertex_count() {
            let packed = m.vertices[i * MODEL_VERTEX_FLOATS + 5].to_bits();
            let ao = ((packed >> 24) & 0xff) as f32 / 255.0;
            let b = ((1.0 - ao) * 5.0).min(4.0) as usize;
            buckets[b] += 1;
        }
    }

    worst.sort_by(|a, b| b.0.total_cmp(&a.0));
    println!("models {done}, verts {total_verts}, tris {total_tris}");
    println!(
        "parse+bake {:.1} ms total, {:.3} ms/model avg",
        total_ms,
        total_ms / done.max(1) as f64
    );
    println!("slowest:");
    for (ms, name, v) in worst.iter().take(5) {
        println!("  {ms:7.2} ms  {v:6} verts  {name}");
    }
    let tv = total_verts.max(1) as f32;
    println!("AO spread (share of vertices):");
    for (i, c) in buckets.iter().enumerate() {
        let lo = 1.0 - (i as f32 + 1.0) * 0.2;
        let hi = 1.0 - i as f32 * 0.2;
        println!("  {lo:.2}-{hi:.2}  {:5.1}%", *c as f32 / tv * 100.0);
    }
}

fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().map(|x| x == "glb").unwrap_or(false) {
            out.push(p);
        }
    }
}
