use makepad_uv_unwrap::*;

fn run(positions: &[[f64; 3]], indices: &[u32]) -> Unwrapped {
    let input = MeshInput { positions, indices, polygons: None, seams: &[] };
    unwrap(
        &input,
        &ChartOptions::default(),
        &PackOptions { target: AtlasTarget::Fit { width: 1024, height: 1024 }, padding: 2, obstacles: &[] },
        &mut |_| true,
    )
    .unwrap()
}

/// Every triangle has a chart and uvs in the unit square, and no two charts
/// come closer than the padding.
fn check(result: &Unwrapped, triangles: usize, padding: u32) {
    assert_eq!(result.charting.triangle_chart.len(), triangles);
    assert_eq!(result.uv.len(), triangles * 3);
    assert_eq!(result.indices.len(), triangles * 3);
    let listed: usize = result.charting.charts.iter().map(|c| c.triangles.len()).sum();
    assert_eq!(listed, triangles, "a triangle was lost or listed twice");
    for (t, &chart) in result.charting.triangle_chart.iter().enumerate() {
        assert!((chart as usize) < result.charting.charts.len(), "triangle {t} has no chart");
    }
    for uv in &result.uv {
        assert!(uv[0].is_finite() && uv[1].is_finite());
        assert!((-1e-9..=1.0 + 1e-9).contains(&uv[0]) && (-1e-9..=1.0 + 1e-9).contains(&uv[1]), "{uv:?}");
    }
    let (w, h) = (result.atlas.width as usize, result.atlas.height as usize);
    let mut owner = vec![u32::MAX; w * h];
    let p = padding as i64;
    for (index, placement) in result.atlas.placements.iter().enumerate() {
        for row in 0..placement.mask.height {
            for &(a, b) in placement.mask.row(row) {
                for x in a..=b {
                    let (cx, cy) = (placement.x as i64 + x as i64, placement.y as i64 + row as i64);
                    assert!(cx >= p && cy >= p && cx < w as i64 - p && cy < h as i64 - p, "chart {index} in the border");
                    owner_check(&mut owner, w, h, cx, cy, p, index as u32);
                }
            }
        }
    }
}

fn owner_check(owner: &mut [u32], w: usize, h: usize, cx: i64, cy: i64, p: i64, me: u32) {
    for y in (cy - p).max(0)..=(cy + p).min(h as i64 - 1) {
        for x in (cx - p).max(0)..=(cx + p).min(w as i64 - 1) {
            let there = owner[y as usize * w + x as usize];
            assert!(there == u32::MAX || there == me, "charts {there} and {me} within padding at {x},{y}");
        }
    }
    owner[cy as usize * w + cx as usize] = me;
}

fn grid(n: usize, height: impl Fn(f64, f64) -> f64) -> (Vec<[f64; 3]>, Vec<u32>) {
    let mut positions = Vec::new();
    for z in 0..=n {
        for x in 0..=n {
            let (fx, fz) = (x as f64 / n as f64, z as f64 / n as f64);
            positions.push([fx, height(fx, fz), fz]);
        }
    }
    let mut indices = Vec::new();
    let at = |x: usize, z: usize| (z * (n + 1) + x) as u32;
    for z in 0..n {
        for x in 0..n {
            indices.extend_from_slice(&[at(x, z), at(x, z + 1), at(x + 1, z + 1), at(x, z), at(x + 1, z + 1), at(x + 1, z)]);
        }
    }
    (positions, indices)
}

fn sphere(segments: usize, rings: usize) -> (Vec<[f64; 3]>, Vec<u32>) {
    let (sin, cos) = (f64::sin, f64::cos);
    let mut positions = vec![[0.0, 1.0, 0.0], [0.0, -1.0, 0.0]];
    for r in 1..rings {
        let a = std::f64::consts::PI * r as f64 / rings as f64;
        for s in 0..segments {
            let b = std::f64::consts::TAU * s as f64 / segments as f64;
            positions.push([sin(a) * cos(b), cos(a), sin(a) * sin(b)]);
        }
    }
    let at = |r: usize, s: usize| (2 + (r - 1) * segments + s % segments) as u32;
    let mut indices = Vec::new();
    for s in 0..segments {
        indices.extend_from_slice(&[0, at(1, s + 1), at(1, s)]);
        indices.extend_from_slice(&[1, at(rings - 1, s), at(rings - 1, s + 1)]);
        for r in 1..rings - 1 {
            indices.extend_from_slice(&[at(r, s), at(r, s + 1), at(r + 1, s + 1), at(r, s), at(r + 1, s + 1), at(r + 1, s)]);
        }
    }
    (positions, indices)
}

fn cylinder_wall(segments: usize, height: f64) -> (Vec<[f64; 3]>, Vec<u32>) {
    let mut positions = Vec::new();
    for s in 0..segments {
        let b = std::f64::consts::TAU * s as f64 / segments as f64;
        positions.push([b.cos(), 0.0, b.sin()]);
        positions.push([b.cos(), height, b.sin()]);
    }
    let mut indices = Vec::new();
    for s in 0..segments as u32 {
        let (a, b) = (s * 2, (s + 1) % segments as u32 * 2);
        indices.extend_from_slice(&[a, a + 1, b + 1, a, b + 1, b]);
    }
    (positions, indices)
}

#[test]
fn a_flat_quad_is_one_isometric_chart() {
    let positions = [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 0.0, 1.0], [0.0, 0.0, 1.0]];
    let indices = [0, 3, 2, 0, 2, 1];
    let result = run(&positions, &indices);
    check(&result, 2, 2);
    assert_eq!(result.charting.charts.len(), 1);
    assert!(result.charting.charts[0].flat);
    assert!(result.atlas.fill > 0.4, "fill {}", result.atlas.fill);
    assert_eq!(result.vertices.len(), 4, "a chart keeps its vertices welded");
}

#[test]
fn creases_cut_a_box_into_six() {
    let p = (0..8).map(|i| [(i & 1) as f64, (i >> 1 & 1) as f64, (i >> 2 & 1) as f64]).collect::<Vec<_>>();
    let quads = [[0u32, 1, 3, 2], [4, 6, 7, 5], [0, 4, 5, 1], [2, 3, 7, 6], [0, 2, 6, 4], [1, 5, 7, 3]];
    let indices = quads.iter().flat_map(|q| [q[0], q[1], q[2], q[0], q[2], q[3]]).collect::<Vec<_>>();
    let result = run(&p, &indices);
    check(&result, 12, 2);
    assert_eq!(result.charting.charts.len(), 6);
    assert!(result.atlas.fill > 0.55, "fill {}", result.atlas.fill);
}

#[test]
fn a_closed_sphere_splits_into_low_distortion_disks() {
    let (p, i) = sphere(48, 24);
    let result = run(&p, &i);
    check(&result, i.len() / 3, 2);
    let charts = result.charting.charts.len();
    assert!((2..=40).contains(&charts), "{charts} charts");
    for chart in &result.charting.charts {
        assert!(chart.stretch <= 1.25 + 1e-9 || chart.triangles.len() == 1, "stretch {}", chart.stretch);
    }
}

#[test]
fn an_open_cylinder_needs_only_a_couple_of_charts() {
    let (p, i) = cylinder_wall(64, 2.0);
    let result = run(&p, &i);
    check(&result, 128, 2);
    assert!(result.charting.charts.len() <= 4, "{} charts", result.charting.charts.len());
    for chart in &result.charting.charts {
        assert!(chart.stretch < 1.1, "a developable chart stretched {}", chart.stretch);
    }
}

#[test]
fn a_lower_stretch_limit_makes_more_charts() {
    let (p, i) = sphere(48, 24);
    let input = MeshInput { positions: &p, indices: &i, polygons: None, seams: &[] };
    let count = |limit: f64| {
        parameterize(&input, &ChartOptions { max_stretch: limit, ..Default::default() }, &mut |_| true)
            .unwrap()
            .charts
            .len()
    };
    assert!(count(1.05) > count(1.6), "the limit had no effect");
}

#[test]
fn authored_seams_cut_flat_surfaces() {
    let (p, i) = grid(4, |_, _| 0.0);
    let line: Vec<[u32; 2]> = (0..4).map(|z| [(z * 5 + 2) as u32, ((z + 1) * 5 + 2) as u32]).collect();
    let input = MeshInput { positions: &p, indices: &i, polygons: None, seams: &line };
    let charting = parameterize(&input, &ChartOptions::default(), &mut |_| true).unwrap();
    assert_eq!(charting.charts.len(), 2);
}

#[test]
fn polygons_are_never_separated() {
    let (p, i) = sphere(12, 6);
    let polygons: Vec<u32> = (0..i.len() as u32 / 3).map(|t| t / 2).collect();
    let input = MeshInput { positions: &p, indices: &i, polygons: Some(&polygons), seams: &[] };
    let charting = parameterize(&input, &ChartOptions::default(), &mut |_| true).unwrap();
    for t in (0..polygons.len()).step_by(2) {
        if t + 1 < polygons.len() {
            assert_eq!(charting.triangle_chart[t], charting.triangle_chart[t + 1]);
        }
    }
}

#[test]
fn degenerate_triangles_are_kept() {
    let (mut p, mut i) = grid(3, |_, _| 0.0);
    let n = p.len() as u32;
    // Two corners on one vertex, all corners on one vertex, a collinear sliver.
    i.extend_from_slice(&[0, 0, 1, 5, 5, 5]);
    p.extend_from_slice(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.5, 0.0, 0.0]]);
    i.extend_from_slice(&[n, n + 1, n + 2]);
    // A triangle whose vertices all coincide in space, away from the rest.
    p.extend_from_slice(&[[9.0, 9.0, 9.0]; 3]);
    i.extend_from_slice(&[n + 3, n + 4, n + 5]);
    let result = run(&p, &i);
    check(&result, i.len() / 3, 2);
}

#[test]
fn nothing_but_degenerate_triangles_still_unwraps() {
    let p = [[0.0; 3], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]];
    let result = run(&p, &[0, 1, 2, 0, 0, 0, 1, 2, 0]);
    check(&result, 3, 2);
}

#[test]
fn non_manifold_edges_are_kept() {
    // Four sheets on one edge, and a bowtie vertex.
    let p = [
        [0.0, 0.0, 0.0], [0.0, 0.0, 1.0],
        [1.0, 0.0, 0.5], [-1.0, 0.0, 0.5], [0.0, 1.0, 0.5], [0.0, -1.0, 0.5],
        [3.0, 0.0, 0.0], [4.0, 0.0, 1.0], [4.0, 0.0, -1.0], [2.0, 0.0, 1.0], [2.0, 0.0, -1.0],
    ];
    let i = [0, 1, 2, 1, 0, 3, 0, 1, 4, 1, 0, 5, 6, 7, 8, 6, 9, 10];
    let result = run(&p, &i);
    check(&result, 6, 2);
}

#[test]
fn doubled_shells_do_not_fragment() {
    let (mut p, mut i) = grid(8, |x, z| (x * z) * 0.2);
    let single = i.len() / 3;
    let baseline = run(&p, &i).charting.charts.len();
    // The same sheet again with its winding reversed, on its own vertices.
    let n = p.len() as u32;
    p.extend_from_within(..);
    let flipped: Vec<u32> = i.chunks_exact(3).flat_map(|t| [t[0] + n, t[2] + n, t[1] + n]).collect();
    i.extend(flipped);
    let result = run(&p, &i);
    check(&result, single * 2, 2);
    assert_eq!(result.charting.charts.len(), baseline * 2, "each sheet keeps its own charts");
}

#[test]
fn duplicate_triangles_are_kept() {
    let (p, mut i) = grid(4, |_, _| 0.0);
    let copy = i.clone();
    i.extend(copy);
    let result = run(&p, &i);
    check(&result, i.len() / 3, 2);
}

#[test]
fn a_saddle_keeps_every_triangle_upright() {
    let (p, i) = grid(40, |x, z| (x - 0.5) * (z - 0.5) * 2.0);
    let result = run(&p, &i);
    check(&result, i.len() / 3, 2);
    for (t, tri) in i.chunks_exact(3).enumerate() {
        let c = result.charting.triangle_chart[t] as usize;
        if result.charting.charts[c].stretch > 1.25 {
            continue;
        }
        let uv = [t * 3, t * 3 + 1, t * 3 + 2].map(|k| result.charting.corner_uv[k]);
        let area = (uv[1][0] - uv[0][0]) * (uv[2][1] - uv[0][1]) - (uv[1][1] - uv[0][1]) * (uv[2][0] - uv[0][0]);
        assert!(area > 0.0, "triangle {t} {tri:?} flipped");
    }
}

#[test]
fn every_chart_gets_the_same_texel_density() {
    let (p, i) = sphere(32, 16);
    let result = run(&p, &i);
    let scale = result.atlas.texels_per_unit;
    let w = result.atlas.width as f64;
    for t in 0..i.len() / 3 {
        let chart_uv = [0, 1, 2].map(|k| result.charting.corner_uv[t * 3 + k]);
        let atlas_uv = [0, 1, 2].map(|k| result.uv[t * 3 + k]);
        let area = |q: [[f64; 2]; 3]| {
            ((q[1][0] - q[0][0]) * (q[2][1] - q[0][1]) - (q[1][1] - q[0][1]) * (q[2][0] - q[0][0])).abs()
        };
        let (a, b) = (area(chart_uv) * scale * scale, area(atlas_uv) * w * w);
        assert!((a - b).abs() <= a.max(1e-6) * 1e-6, "{a} vs {b}");
    }
}

#[test]
fn an_atlas_that_cannot_fit_says_so() {
    let (p, i) = sphere(24, 12);
    let input = MeshInput { positions: &p, indices: &i, polygons: None, seams: &[] };
    let charting = parameterize(&input, &ChartOptions::default(), &mut |_| true).unwrap();
    let tiny = PackOptions { target: AtlasTarget::Fit { width: 4, height: 4 }, padding: 4, obstacles: &[] };
    let error = pack(&charting, &tiny, &mut |_| true).unwrap_err();
    assert!(matches!(error, UnwrapError::AtlasTooSmall { .. }), "{error}");
    assert!(error.to_string().contains("4x4"), "{error}");

    let dense = PackOptions {
        target: AtlasTarget::Density { texels_per_unit: 4096.0, max_size: 64 },
        padding: 2,
        obstacles: &[],
    };
    assert!(matches!(pack(&charting, &dense, &mut |_| true), Err(UnwrapError::AtlasTooSmall { .. })));
}

#[test]
fn density_mode_keeps_the_density_and_shrinks_the_atlas() {
    let (p, i) = grid(10, |_, _| 0.0);
    let input = MeshInput { positions: &p, indices: &i, polygons: None, seams: &[] };
    let charting = parameterize(&input, &ChartOptions::default(), &mut |_| true).unwrap();
    let target = AtlasTarget::Density { texels_per_unit: 100.0, max_size: 4096 };
    let atlas = pack(&charting, &PackOptions { target, padding: 2, obstacles: &[] }, &mut |_| true).unwrap();
    assert_eq!(atlas.texels_per_unit, 100.0);
    assert!(atlas.width <= 106 && atlas.height <= 106, "{}x{}", atlas.width, atlas.height);
}

#[test]
fn obstacles_stay_free() {
    let (p, i) = grid(4, |_, _| 0.0);
    let input = MeshInput { positions: &p, indices: &i, polygons: None, seams: &[] };
    let charting = parameterize(&input, &ChartOptions::default(), &mut |_| true).unwrap();
    let block = [[[0.0, 0.0], [64.0, 0.0], [64.0, 64.0]], [[0.0, 0.0], [64.0, 64.0], [0.0, 64.0]]];
    let options = PackOptions { target: AtlasTarget::Fit { width: 128, height: 128 }, padding: 2, obstacles: &block };
    let atlas = pack(&charting, &options, &mut |_| true).unwrap();
    let placement = &atlas.placements[0];
    for row in 0..placement.mask.height {
        for &(a, b) in placement.mask.row(row) {
            let (y, x0, x1) = (placement.y + row, placement.x + a, placement.x + b);
            assert!(y >= 66 || x0 >= 66, "chart overlaps the obstacle at row {y} {x0}..{x1}");
        }
    }
}

#[test]
fn bad_input_is_an_error_not_a_panic() {
    let check = |positions: &[[f64; 3]], indices: &[u32]| {
        parameterize(&MeshInput { positions, indices, polygons: None, seams: &[] }, &ChartOptions::default(), &mut |_| true)
    };
    assert!(check(&[[0.0; 3]], &[0, 0]).is_err());
    assert!(check(&[[0.0; 3]], &[0, 0, 1]).is_err());
    assert!(check(&[[f64::NAN, 0.0, 0.0]], &[0, 0, 0]).is_err());
    assert_eq!(check(&[], &[]).unwrap().charts.len(), 0);
}

#[test]
fn cancelling_stops_the_work() {
    let (p, i) = sphere(24, 12);
    let input = MeshInput { positions: &p, indices: &i, polygons: None, seams: &[] };
    assert_eq!(
        parameterize(&input, &ChartOptions::default(), &mut |_| false).unwrap_err(),
        UnwrapError::Cancelled
    );
}

#[test]
fn results_are_deterministic() {
    let (p, i) = sphere(32, 16);
    assert_eq!(run(&p, &i), run(&p, &i));
}

#[test]
fn a_large_mesh_unwraps() {
    let (p, i) = grid(300, |x, z| ((x * 9.0).sin() + (z * 7.0).cos()) * 0.2);
    let result = run(&p, &i);
    assert_eq!(result.charting.triangle_chart.len(), i.len() / 3);
    assert!(result.atlas.fill > 0.5, "fill {}", result.atlas.fill);
}

/// Past world-model scale: 12,000 walls, about 77,000 square metres.
#[test]
fn a_world_model_packs_at_a_fixed_density() {
    let mut positions = Vec::new();
    let mut indices = Vec::new();
    for i in 0..2000u32 {
        let (sx, sy, sz) = (1.0 + (i % 7) as f64, 1.0 + (i % 5) as f64 * 0.7, 0.5 + (i % 3) as f64);
        let (ox, oz) = ((i % 50) as f64 * 12.0, (i / 50) as f64 * 12.0);
        let base = positions.len() as u32;
        for c in 0..8u32 {
            let bit = |b: u32, s: f64| if c >> b & 1 == 1 { s } else { 0.0 };
            positions.push([ox + bit(0, sx), bit(1, sy), oz + bit(2, sz)]);
        }
        for q in [[0u32, 1, 3, 2], [4, 6, 7, 5], [0, 4, 5, 1], [2, 3, 7, 6], [0, 2, 6, 4], [1, 5, 7, 3]] {
            indices.extend_from_slice(&[base + q[0], base + q[1], base + q[2], base + q[0], base + q[2], base + q[3]]);
        }
    }
    let input = MeshInput { positions: &positions, indices: &indices, polygons: None, seams: &[] };
    let target = AtlasTarget::Density { texels_per_unit: 8.0, max_size: 4096 };
    let options = PackOptions { target, padding: 2, obstacles: &[] };
    let start = std::time::Instant::now();
    let result = unwrap(&input, &ChartOptions::default(), &options, &mut |_| true).unwrap();
    println!(
        "world model: {} tris, {} charts, {}x{}, fill {:.1}%, {:.0} ms",
        indices.len() / 3,
        result.charting.charts.len(),
        result.atlas.width,
        result.atlas.height,
        result.atlas.fill * 100.0,
        start.elapsed().as_secs_f64() * 1e3
    );
    let area: f64 = result.charting.charts.iter().map(|c| c.area).sum();
    assert!(area > 13_000.0, "area {area}");
    assert_eq!(result.charting.charts.len(), 12_000);
    assert_eq!(result.atlas.texels_per_unit, 8.0);
    assert!(result.atlas.fill > 0.5, "fill {}", result.atlas.fill);
    check(&result, indices.len() / 3, 2);
}

fn request(texels_per_unit: f64, max_size: u32) -> LayoutRequest {
    LayoutRequest { texels_per_unit, max_size, padding: 4, weld_tolerance: 1.0e-5 }
}

fn lay_out(p: &[[f64; 3]], i: &[u32], r: &LayoutRequest) -> Unwrapped {
    let input = MeshInput { positions: p, indices: i, polygons: None, seams: &[] };
    layout(&input, r, &mut |_| true).unwrap()
}

#[test]
fn a_layout_reaches_the_density_when_it_fits() {
    let (p, i) = sphere(32, 16);
    let result = lay_out(&p, &i, &request(16.0, 512));
    assert_eq!(result.atlas.texels_per_unit, 16.0);
    assert_eq!(result.collapsed_charts, 0);
    assert!(result.atlas.width <= 512 && result.atlas.height <= 512);
    check(&result, i.len() / 3, 4);
}

#[test]
fn a_layout_lowers_the_density_to_fit_and_says_so() {
    let (p, i) = sphere(32, 16);
    let result = lay_out(&p, &i, &request(4096.0, 512));
    assert!(result.atlas.texels_per_unit < 4096.0);
    assert!(result.atlas.texels_per_unit > 50.0, "density {}", result.atlas.texels_per_unit);
    assert!(result.atlas.width <= 512 && result.atlas.height <= 512);
    check(&result, i.len() / 3, 4);
}

#[test]
fn layers_concatenated_share_one_layout() {
    let (mut p, mut i) = grid(12, |x, z| x * z * 0.3);
    let split = i.len();
    let n = p.len() as u32;
    let (q, j) = sphere(16, 8);
    p.extend(q.iter().map(|v| [v[0] + 3.0, v[1], v[2]]));
    i.extend(j.iter().map(|v| v + n));
    let result = lay_out(&p, &i, &request(32.0, 512));
    check(&result, i.len() / 3, 4);
    let (first, second) = result.indices.split_at(split);
    assert!(first.iter().all(|&v| result.vertices[v as usize].source < n), "first layer's vertices");
    assert!(second.iter().all(|&v| result.vertices[v as usize].source >= n), "second layer's vertices");
}

#[test]
fn a_rough_dense_mesh_does_not_become_thousands_of_islands() {
    // Every facet folded against its neighbours, so creases cut everywhere.
    let (p, i) = grid(150, |x, z| {
        let a = ((x * 150.0).round() * 12.9898 + (z * 150.0).round() * 78.233).sin() * 43758.5453;
        a.fract() * 0.05
    });
    let result = lay_out(&p, &i, &request(256.0, 512));
    let charts = result.charting.charts.len();
    assert!(charts < 1000, "{charts} charts");
    assert_eq!(result.collapsed_charts, 0);
    assert!(result.atlas.texels_per_unit > 256.0 * 0.5, "density {}", result.atlas.texels_per_unit);
    check(&result, i.len() / 3, 4);
}

#[test]
fn a_triangle_soup_with_no_room_collapses_instead_of_failing() {
    let mut p = Vec::new();
    let mut i = Vec::new();
    for t in 0..20_000u32 {
        let (x, z) = ((t % 200) as f64 * 3.0, (t / 200) as f64 * 3.0);
        p.extend_from_slice(&[[x, 0.0, z], [x + 1.0, 0.0, z], [x, 0.0, z + 1.0]]);
        i.extend_from_slice(&[t * 3, t * 3 + 2, t * 3 + 1]);
    }
    let result = lay_out(&p, &i, &request(64.0, 128));
    assert!(result.collapsed_charts > 0);
    assert_eq!(result.charting.triangle_chart.len(), 20_000);
    check(&result, 20_000, 4);
}

#[test]
fn a_layout_is_the_same_bits_every_time() {
    let (p, i) = grid(40, |x, z| (x * 7.0).sin() * (z * 5.0).cos() * 0.2);
    let bits = |u: &Unwrapped| u.uv.iter().flat_map(|q| [q[0].to_bits(), q[1].to_bits()]).collect::<Vec<_>>();
    let r = request(64.0, 512);
    assert_eq!(bits(&lay_out(&p, &i, &r)), bits(&lay_out(&p, &i, &r)));
}

#[test]
fn a_layout_survives_non_manifold_and_degenerate_input() {
    let p = [
        [0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.5], [-1.0, 0.0, 0.5], [0.0, 1.0, 0.5],
        [0.0, -1.0, 0.5], [5.0, 5.0, 5.0], [5.0, 5.0, 5.0], [5.0, 5.0, 5.0],
    ];
    let i = [0, 1, 2, 1, 0, 3, 0, 1, 4, 1, 0, 5, 6, 7, 8, 0, 0, 1, 2, 2, 2];
    let result = lay_out(&p, &i, &request(32.0, 256));
    check(&result, i.len() / 3, 4);
}
