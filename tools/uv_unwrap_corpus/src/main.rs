//! Runs the unwrapper over generated parts and every GLB it can find, printing
//! distortion, chart count, fill and time per mesh.
//!
//!   makepad-uv-unwrap-corpus [--filter text] [--size 1024] [--padding 2]
//!                            [--render dir] [roots...]
//!
//! Angle distortion is the larger singular value of a triangle's map over the
//! smaller, 1 for a conformal map. Area distortion is the map's area scale
//! against the mesh's mean, folded so 2 means twice or half: it counts texel
//! density differences between charts, which is why it is taken over the whole
//! atlas. Both are weighted by surface area.

mod old;
mod parts;
mod render;
mod world;

use makepad_uv_unwrap::*;
use std::time::Instant;

pub struct Mesh {
    pub name: String,
    pub positions: Vec<[f64; 3]>,
    pub indices: Vec<u32>,
}

pub struct Layout {
    pub charts: usize,
    /// Atlas uv in texels of a `size` square, per corner.
    pub uv: Vec<[f64; 2]>,
    pub fill: f64,
    pub seconds: f64,
    pub detail: String,
}

fn quantile(mut samples: Vec<(f64, f64)>, q: f64) -> f64 {
    samples.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
    let total: f64 = samples.iter().map(|s| s.1).sum();
    let mut seen = 0.0;
    for s in &samples {
        seen += s.1;
        if seen >= total * q {
            return s.0;
        }
    }
    samples.last().map_or(1.0, |s| s.0)
}

struct Quality {
    angle_mean: f64,
    angle_p95: f64,
    area_mean: f64,
    area_p95: f64,
    flipped: f64,
}

fn measure(mesh: &Mesh, uv: &[[f64; 2]]) -> Quality {
    let triangles = mesh.indices.len() / 3;
    let mut angle = Vec::with_capacity(triangles);
    let mut scale = Vec::with_capacity(triangles);
    let (mut area_3d, mut area_uv, mut flipped) = (0.0, 0.0, 0.0);
    for t in 0..triangles {
        let p = [0, 1, 2].map(|k| mesh.positions[mesh.indices[t * 3 + k] as usize]);
        let q = [uv[t * 3], uv[t * 3 + 1], uv[t * 3 + 2]];
        let (e, f) = (
            [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]],
            [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]],
        );
        let c = [e[1] * f[2] - e[2] * f[1], e[2] * f[0] - e[0] * f[2], e[0] * f[1] - e[1] * f[0]];
        let a = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt() * 0.5;
        if !(a > 0.0) {
            continue;
        }
        let signed = ((q[1][0] - q[0][0]) * (q[2][1] - q[0][1]) - (q[1][1] - q[0][1]) * (q[2][0] - q[0][0])) * 0.5;
        area_3d += a;
        area_uv += signed.abs();
        if signed <= 0.0 {
            flipped += a;
        }
        let (large, small) = triangle_singular_values(p, q);
        angle.push(((large / small.max(1.0e-9)).min(1.0e3), a));
        scale.push((large * small, a));
    }
    let mean_scale = if area_3d > 0.0 { area_uv / area_3d } else { 1.0 };
    let fold = scale
        .iter()
        .map(|&(s, a)| {
            let r = (s / mean_scale).max(1.0e-9);
            (r.max(1.0 / r).min(1.0e3), a)
        })
        .collect::<Vec<_>>();
    let weighted = |v: &[(f64, f64)]| {
        let total: f64 = v.iter().map(|s| s.1).sum();
        v.iter().map(|s| s.0 * s.1).sum::<f64>() / total.max(1.0e-300)
    };
    Quality {
        angle_mean: weighted(&angle),
        angle_p95: quantile(angle, 0.95),
        area_mean: weighted(&fold),
        area_p95: quantile(fold, 0.95),
        flipped: flipped / area_3d.max(1.0e-300),
    }
}

/// Share of the charted texels that more than one triangle of a different
/// chart would write. Zero is required of a lightmap chart set.
fn overlap(mesh: &Mesh, layout: &Layout, size: u32, chart_of: Option<&[u32]>) -> f64 {
    let Some(chart_of) = chart_of else { return f64::NAN };
    let s = size as usize;
    let mut owner = vec![u32::MAX; s * s];
    let (mut touched, mut shared) = (0u64, 0u64);
    for t in 0..mesh.indices.len() / 3 {
        let q = [layout.uv[t * 3], layout.uv[t * 3 + 1], layout.uv[t * 3 + 2]];
        render::for_texels(q, s, |x, y| {
            let slot = &mut owner[y * s + x];
            if *slot == u32::MAX {
                *slot = chart_of[t];
                touched += 1;
            } else if *slot != chart_of[t] {
                shared += 1;
            }
        });
    }
    shared as f64 / touched.max(1) as f64
}

fn new_layout(mesh: &Mesh, size: u32, padding: u32, options: &ChartOptions) -> Result<(Layout, Vec<u32>), UnwrapError> {
    let input = MeshInput { positions: &mesh.positions, indices: &mesh.indices, polygons: None, seams: &[] };
    let start = Instant::now();
    let charting = parameterize(&input, options, &mut |_| true)?;
    let charted = start.elapsed().as_secs_f64();
    let start = Instant::now();
    let pack_options = PackOptions { target: AtlasTarget::Fit { width: size, height: size }, padding, obstacles: &[] };
    let atlas = pack(&charting, &pack_options, &mut |_| true)?;
    let packed = start.elapsed().as_secs_f64();
    let uv = (0..charting.corner_uv.len())
        .map(|c| atlas.placements[charting.triangle_chart[c / 3] as usize].map(charting.corner_uv[c]))
        .collect();
    let flat = charting.charts.iter().filter(|c| c.flat).count();
    Ok((
        Layout {
            charts: charting.charts.len(),
            uv,
            fill: atlas.fill,
            seconds: charted + packed,
            detail: format!(
                "chart {:.0}ms pack {:.0}ms flat {flat} cover {:.1}% bits {:016x}",
                charted * 1e3,
                packed * 1e3,
                atlas.coverage * 100.0,
                world::bits_hash(&charting.corner_uv)
            ),
        },
        charting.triangle_chart,
    ))
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (mut filter, mut size, mut padding, mut render_dir, mut stretch) = (String::new(), 1024u32, 2u32, None, 1.25f64);
    let mut roots: Vec<String> = Vec::new();
    let mut old_too = true;
    let mut out: Option<String> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--filter" => filter = args.next().unwrap(),
            "--size" => size = args.next().unwrap().parse().unwrap(),
            "--padding" => padding = args.next().unwrap().parse().unwrap(),
            "--stretch" => stretch = args.next().unwrap().parse().unwrap(),
            "--render" => render_dir = args.next(),
            "--new-only" => old_too = false,
            "--out" => out = args.next(),
            "--world" => {
                world::run();
                return;
            }
            _ => roots.push(arg),
        }
    }
    let options = ChartOptions { max_stretch: stretch, ..ChartOptions::default() };
    let mut table = String::new();
    let header = format!("mesh\ttris\talgo\tcharts\tangle_mean\tangle_p95\tarea_mean\tarea_p95\tflipped%\toverlap%\tfill%\tms\tdetail");
    println!("{header}");
    table.push_str(&header);
    table.push('\n');
    let meshes = parts::all(&roots, &filter);
    for mesh in meshes {
        let triangles = mesh.indices.len() / 3;
        let mut row = |algo: &str, layout: &Layout, chart_of: Option<&[u32]>| {
            let q = measure(&mesh, &layout.uv);
            let line = format!(
                "{}\t{}\t{}\t{}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.2}\t{:.2}\t{:.1}\t{:.1}\t{}",
                mesh.name,
                triangles,
                algo,
                layout.charts,
                q.angle_mean,
                q.angle_p95,
                q.area_mean,
                q.area_p95,
                q.flipped * 100.0,
                overlap(&mesh, layout, size, chart_of) * 100.0,
                layout.fill * 100.0,
                layout.seconds * 1e3,
                layout.detail
            );
            println!("{line}");
            table.push_str(&line);
            table.push('\n');
        };
        if old_too {
            let (layout, chart_of) = old::mesh_edit(&mesh, size, padding);
            row("old-edit", &layout, Some(&chart_of));
            let (layout, chart_of) = old::ao_atlas(&mesh, size);
            row("old-ao", &layout, Some(&chart_of));
        }
        match new_layout(&mesh, size, padding, &options) {
            Ok((layout, chart_of)) => {
                row("new", &layout, Some(&chart_of));
                if let Some(dir) = &render_dir {
                    render::write(dir, &mesh, &layout, &chart_of, size);
                }
            }
            Err(error) => println!("{}\t{}\tnew\tERROR {error}", mesh.name, triangles),
        }
    }
    if let Some(path) = out {
        std::fs::write(path, table).unwrap();
    }
}
