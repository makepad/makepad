//! A world at the scale the light bake asks for: 77 models, 13,496 square
//! metres of surface, one layout per model at 4 texels per metre with 4 texels
//! of padding and a 512 edge. The models are generated stand-ins (walls, towers,
//! rough rock, trim) in the size spread of a walled town.

use makepad_uv_unwrap::*;
use std::f64::consts::{PI, TAU};
use std::time::Instant;

/// FNV over the bits, comparable between machines.
pub fn bits_hash(values: &[[f64; 2]]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for v in values {
        for c in v {
            h = (h ^ (c + 0.0).to_bits()).wrapping_mul(0x100000001b3);
        }
    }
    h
}

struct Model {
    positions: Vec<[f64; 3]>,
    indices: Vec<u32>,
}

impl Model {
    fn sheet(&mut self, rows: usize, columns: usize, f: impl Fn(f64, f64) -> [f64; 3]) {
        let base = self.positions.len() as u32;
        for v in 0..=rows {
            for u in 0..=columns {
                self.positions.push(f(u as f64 / columns as f64, v as f64 / rows as f64));
            }
        }
        let at = |u: usize, v: usize| base + (v * (columns + 1) + u) as u32;
        for v in 0..rows {
            for u in 0..columns {
                self.indices.extend_from_slice(&[at(u, v), at(u, v + 1), at(u + 1, v + 1), at(u, v), at(u + 1, v + 1), at(u + 1, v)]);
            }
        }
    }

    /// A box whose six faces each have their own vertices, as modelling kits do.
    fn block(&mut self, o: [f64; 3], s: [f64; 3], n: usize) {
        let [x, y, z] = o;
        let [a, b, c] = s;
        self.sheet(n, n, |u, v| [x + a * u, y + b * v, z]);
        self.sheet(n, n, |u, v| [x + a * v, y + b * u, z + c]);
        self.sheet(n, n, |u, v| [x, y + b * u, z + c * v]);
        self.sheet(n, n, |u, v| [x + a, y + b * v, z + c * u]);
        self.sheet(n, n, |u, v| [x + a * v, y, z + c * u]);
        self.sheet(n, n, |u, v| [x + a * u, y + b, z + c * v]);
    }

    fn tower(&mut self, o: [f64; 3], radius: f64, height: f64, segments: usize) {
        self.sheet(4, segments, |u, v| {
            let a = u * TAU;
            [o[0] + radius * a.cos(), o[1] + height * v, o[2] + radius * a.sin()]
        });
        self.sheet(segments / 2, segments, |u, v| {
            let (a, b) = (u * TAU, v * PI * 0.5);
            [o[0] + radius * b.cos() * a.cos(), o[1] + height + radius * b.sin(), o[2] + radius * b.cos() * a.sin()]
        });
    }

    fn rock(&mut self, o: [f64; 3], radius: f64, n: usize) {
        self.sheet(n, n * 2, |u, v| {
            let (a, b) = (v * PI, u * TAU);
            let r = radius * (1.0 + 0.2 * (b * 3.0).sin() * (a * 4.0).cos() + 0.06 * ((u * 900.0).round() * 12.9898 + (v * 900.0).round() * 78.233).sin());
            [o[0] + r * a.sin() * b.cos(), o[1] + r * a.cos(), o[2] + r * a.sin() * b.sin()]
        });
    }

    fn area(&self) -> f64 {
        self.indices
            .chunks_exact(3)
            .map(|t| {
                let p = [t[0], t[1], t[2]].map(|i| self.positions[i as usize]);
                let e = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
                let f = [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]];
                let c = [e[1] * f[2] - e[2] * f[1], e[2] * f[0] - e[0] * f[2], e[0] * f[1] - e[1] * f[0]];
                (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt() * 0.5
            })
            .sum()
    }
}

fn model(i: usize) -> Model {
    let mut m = Model { positions: Vec::new(), indices: Vec::new() };
    let k = 1.0 + (i % 9) as f64;
    match i % 7 {
        0 => {
            for w in 0..(i % 11 + 3) {
                m.block([w as f64 * 8.0, 0.0, 0.0], [7.5, 3.0 + k, 1.2], 2 + i % 3);
            }
        }
        1 => m.tower([0.0; 3], 1.5 + k * 0.4, 6.0 + k, 16 + i % 32),
        2 => {
            m.block([0.0; 3], [6.0 + k, 4.0 + k * 0.5, 5.0 + k], 3);
            m.block([1.0, 4.0 + k * 0.5, 1.0], [4.0 + k, 2.0, 3.0 + k], 2);
            m.tower([3.0, 0.0, 9.0 + k], 1.0, 8.0, 12);
        }
        3 => m.rock([0.0; 3], 1.0 + k * 0.6, 30 + i % 50),
        4 => {
            for w in 0..40 + i {
                m.block([(w % 8) as f64, (w / 8) as f64 * 0.4, 0.0], [0.5, 0.3, 0.2], 1);
            }
        }
        5 => {
            m.block([0.0; 3], [12.0, 1.0 + k * 0.2, 12.0], 10);
            m.tower([6.0, 1.0, 6.0], 2.0, 3.0, 24);
        }
        _ => {
            for w in 0..5 {
                m.tower([w as f64 * 4.0, 0.0, 0.0], 0.6, 3.0 + k * 0.5, 10);
                m.block([w as f64 * 4.0 + 0.6, 2.0, -0.2], [3.0, 0.4, 0.4], 1);
            }
        }
    }
    m
}

pub fn run() {
    let mut models: Vec<Model> = (0..77).map(model).collect();
    let total: f64 = models.iter().map(Model::area).sum();
    let scale = (13_496.0 / total).sqrt();
    for m in &mut models {
        for p in &mut m.positions {
            for c in p.iter_mut() {
                *c *= scale;
            }
        }
    }
    let request = LayoutRequest { texels_per_unit: 4.0, max_size: 512, padding: 4, weld_tolerance: 1.0e-5 };
    println!("model\ttris\tarea\tcharts\tsize\tdensity\tfill%\tcollapsed\tms\tus_per_tri\tbits");
    let (mut ms, mut tris, mut charts, mut low, mut hash) = (0.0, 0usize, 0usize, f64::INFINITY, 0u64);
    let mut slowest = 0.0f64;
    for (i, m) in models.iter().enumerate() {
        let input = MeshInput { positions: &m.positions, indices: &m.indices, polygons: None, seams: &[] };
        let start = Instant::now();
        let result = layout(&input, &request, &mut |_| true).unwrap();
        let elapsed = start.elapsed().as_secs_f64() * 1e3;
        let count = m.indices.len() / 3;
        let bits = bits_hash(&result.uv);
        println!(
            "{i}\t{count}\t{:.0}\t{}\t{}x{}\t{:.2}\t{:.1}\t{}\t{:.1}\t{:.2}\t{bits:016x}",
            m.area(),
            result.charting.charts.len(),
            result.atlas.width,
            result.atlas.height,
            result.atlas.texels_per_unit,
            result.atlas.fill * 100.0,
            result.collapsed_charts,
            elapsed,
            elapsed * 1e3 / count as f64,
        );
        ms += elapsed;
        tris += count;
        charts += result.charting.charts.len();
        low = low.min(result.atlas.texels_per_unit);
        slowest = slowest.max(elapsed);
        hash = (hash ^ bits).wrapping_mul(0x100000001b3);
    }
    println!(
        "WORLD models 77 area {:.0} tris {tris} charts {charts} lowest_density {low:.2} total_ms {ms:.1} slowest_ms {slowest:.1} us_per_tri {:.2} bits {hash:016x}",
        models.iter().map(Model::area).sum::<f64>(),
        ms * 1e3 / tris as f64
    );
}
