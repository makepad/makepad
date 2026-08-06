//! Generation cost and triangle counts. `cargo run --release -p makepad-game-gen --example costs`
//!
//! Release numbers only — debug is 10-30x slower and would mislead anyone
//! sizing a Quest budget from this table.

use makepad_game_gen::*;
use std::time::Instant;

fn bench(label: &str, tris: usize, bytes: usize, f: impl Fn()) {
    // Warm once so the first allocation isn't counted as generation cost.
    f();
    let n = 20;
    let t = Instant::now();
    for _ in 0..n {
        f();
    }
    let us = t.elapsed().as_secs_f64() * 1.0e6 / n as f64;
    println!("{label:<26} {us:>9.1} us  {tris:>7} tris  {:>8} B", bytes);
}

fn main() {
    println!("--- trees (height 4, Medium LOD) ---");
    for name in SPECIES {
        let p = TreeParams {
            seed: 1,
            ..Default::default()
        };
        let m = tree(name, p);
        bench(name, m.triangle_count(), m.gpu_bytes(), || {
            let _ = tree(name, p);
        });
    }

    println!("\n--- tree LOD (oak) ---");
    for (label, lod) in [("oak Low", Lod::Low), ("oak Medium", Lod::Medium), ("oak High", Lod::High)]
    {
        let p = TreeParams {
            seed: 1,
            lod,
            ..Default::default()
        };
        let m = tree("oak", p);
        bench(label, m.triangle_count(), m.gpu_bytes(), || {
            let _ = tree("oak", p);
        });
    }

    println!("\n--- blobs (surface nets, resolution 16) ---");
    for kind in BLOBS {
        let p = BlobParams {
            seed: 1,
            ..Default::default()
        };
        let m = blob(kind, p);
        bench(kind, m.triangle_count(), m.gpu_bytes(), || {
            let _ = blob(kind, p);
        });
    }

    println!("\n--- track (closed oval, 10 control points) ---");
    for res in [4usize, 8, 16] {
        let s = oval(120.0, 80.0, 10);
        let p = TrackParams {
            resolution: res,
            rail_height: 1.0,
            ..Default::default()
        };
        let t = track(&s, p);
        bench(
            &format!("track res={res}"),
            t.mesh.triangle_count(),
            t.mesh.gpu_bytes(),
            || {
                let _ = track(&s, p);
            },
        );
    }

    println!("\n--- textures (256px + full mip chain) ---");
    for name in MATERIALS {
        let chain = material_mipped(name, 256, 1);
        bench(name, 0, chain.total_bytes(), || {
            let _ = material_mipped(name, 256, 1);
        });
    }

    println!("\n--- scatter ---");
    for (label, spacing, extent) in [
        ("scatter 60x60 s=4", 4.0f32, 60.0f32),
        ("scatter 120x120 s=4", 4.0, 120.0),
        ("scatter 200x200 s=6", 6.0, 200.0),
    ] {
        let p = ScatterParams {
            seed: 1,
            spacing,
            extent: makepad_math::vec2f(extent, extent),
            max_count: 20_000,
            ..Default::default()
        };
        let n = scatter(&p).len();
        bench(&format!("{label} ({n})"), 0, 0, || {
            let _ = scatter(&p);
        });
    }

    println!("\n--- a realistic forest ---");
    let t = Instant::now();
    let mut cache = GenCache::default();
    let placements = scatter(&ScatterParams {
        seed: 7,
        spacing: 5.0,
        extent: makepad_math::vec2f(150.0, 150.0),
        variants: 6,
        ..Default::default()
    });
    let species = ["oak", "pine", "bush"];
    for p in &placements {
        let sp = species[(p.variant as usize) % species.len()];
        cache.tree(
            sp,
            TreeParams {
                seed: p.variant as u64,
                ..Default::default()
            },
        );
    }
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    println!(
        "{} trees placed, {} distinct meshes generated ({} hits), {:.2} ms total, {} KB resident",
        placements.len(),
        cache.misses(),
        cache.hits(),
        ms,
        cache.bytes() / 1024
    );
}
