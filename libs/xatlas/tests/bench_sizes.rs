//! Wall-clock of `parametrize` on remesh-sized meshes. Run:
//!   cargo test -p makepad-xatlas --release --test bench_sizes -- --nocapture --ignored

use makepad_xatlas::parametrize;
use std::time::Instant;

fn grid(cols: u32, rows: u32) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let mut positions = Vec::new();
    for y in 0..=rows {
        for x in 0..=cols {
            positions.push([x as f32, y as f32, 0.0]);
        }
    }
    let mut faces = Vec::new();
    let w = cols + 1;
    for y in 0..rows {
        for x in 0..cols {
            let i = y * w + x;
            faces.push([i, i + 1, i + w + 1]);
            faces.push([i, i + w + 1, i + w]);
        }
    }
    (positions, faces)
}

/// Closed box with per-face verts, then tessellated so xatlas has 6 charts
/// plus internal splits — closer to a remeshed prop than a flat grid.
fn tess_cube(div: u32) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let faces = [
        ([1.0f32, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
        ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
        ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]),
    ];
    let mut positions = Vec::new();
    let mut tris = Vec::new();
    for (n, u, v) in faces {
        let base = positions.len() as u32;
        for j in 0..=div {
            for i in 0..=div {
                let su = (i as f32 / div as f32) - 0.5;
                let sv = (j as f32 / div as f32) - 0.5;
                positions.push([
                    n[0] * 0.5 + u[0] * su + v[0] * sv,
                    n[1] * 0.5 + u[1] * su + v[1] * sv,
                    n[2] * 0.5 + u[2] * su + v[2] * sv,
                ]);
            }
        }
        let w = div + 1;
        for j in 0..div {
            for i in 0..div {
                let i0 = base + j * w + i;
                tris.push([i0, i0 + 1, i0 + w + 1]);
                tris.push([i0, i0 + w + 1, i0 + w]);
            }
        }
    }
    (positions, tris)
}

/// Heightfield with enough normal variation that xatlas grows many charts.
fn noisy_terrain(cols: u32, rows: u32) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let mut positions = Vec::new();
    for y in 0..=rows {
        for x in 0..=cols {
            let xf = x as f32 * 0.08;
            let yf = y as f32 * 0.08;
            let z = (xf * 1.7).sin() * 0.35
                + (yf * 2.3).cos() * 0.28
                + ((xf + yf) * 4.1).sin() * 0.12
                + ((xf * 9.0).sin() * (yf * 8.0).cos()) * 0.08;
            positions.push([xf, yf, z]);
        }
    }
    let mut faces = Vec::new();
    let w = cols + 1;
    for y in 0..rows {
        for x in 0..cols {
            let i = y * w + x;
            faces.push([i, i + 1, i + w + 1]);
            faces.push([i, i + w + 1, i + w]);
        }
    }
    (positions, faces)
}

fn time_one(name: &str, positions: &[[f32; 3]], faces: &[[u32; 3]]) {
    let t0 = Instant::now();
    let out = parametrize(positions, faces).expect(name);
    let dt = t0.elapsed();
    eprintln!(
        "{name:>16}  in_v={:<6} in_f={:<6}  out_v={:<6} charts={:<4} atlas={}x{}  {:>8.3}s",
        positions.len(),
        faces.len(),
        out.vertices.len(),
        out.chart_count,
        out.width,
        out.height,
        dt.as_secs_f64()
    );
}

#[test]
#[ignore]
fn time_remesh_sized_unwraps() {
    // Warm the allocator / caches.
    let (p, f) = tess_cube(4);
    let _ = parametrize(&p, &f);

    // Flat grid: few charts (optimistic).
    let (p, f) = grid(77, 77); // ~12k tris
    time_one("grid 12k", &p, &f);
    let (p, f) = grid(100, 100); // 20k tris
    time_one("grid 20k", &p, &f);

    // Tessellated cube: 6 charts, prop/character densities.
    // 6 * 2 * d^2 faces.
    let (p, f) = tess_cube(32); // 6*2*1024 = 12288
    time_one("cube 12k", &p, &f);
    let (p, f) = tess_cube(41); // 6*2*1681 = 20172
    time_one("cube 20k", &p, &f);
    let (p, f) = tess_cube(82); // 6*2*6724 = 80688
    time_one("cube 80k", &p, &f);

    let (p, f) = noisy_terrain(77, 77);
    time_one("terrain 12k", &p, &f);
    let (p, f) = noisy_terrain(100, 100);
    time_one("terrain 20k", &p, &f);
    let (p, f) = noisy_terrain(200, 200);
    time_one("terrain 80k", &p, &f);
}
