use makepad_ai_geometry::{
    cuda::patchmatch_cuda,
    patchmatch::{patchmatch_reference, DepthNormalMap, PatchMatchParams},
    synthetic::textured_plane,
};
use std::time::Instant;

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}

// Normalise for angular comparison: widened f32 unit vectors have small
// length errors, which acos(dot) alone would mistake for angular errors.
fn angle(a: [f64; 3], b: [f64; 3]) -> f64 {
    (dot(a, b) / (dot(a, a) * dot(b, b)).sqrt()).clamp(-1.0, 1.0).acos().to_degrees()
}

fn pixels(map: &DepthNormalMap, radius: usize) -> Vec<usize> {
    (radius..map.height.saturating_sub(radius))
        .flat_map(|y| (radius..map.width.saturating_sub(radius)).map(move |x| y * map.width + x))
        .collect()
}

fn fraction(indices: &[usize], predicate: impl Fn(usize) -> bool) -> f64 {
    indices.iter().filter(|&&i| predicate(i)).count() as f64 / indices.len() as f64
}

fn stats(label: &str, degrees: f64, initial: &str, map: &DepthNormalMap,
    truth: &DepthNormalMap, indices: &[usize]) {
    let mut depth: Vec<f64> = indices.iter().map(|&i| (map.depth[i] / truth.depth[i] - 1.0).abs()).collect();
    let mut normal: Vec<f64> = indices.iter().map(|&i| angle(map.normal[i], truth.normal[i])).collect();
    let mut cost: Vec<f64> = indices.iter().map(|&i| map.cost[i]).collect();
    depth.sort_by(f64::total_cmp);
    normal.sort_by(f64::total_cmp);
    cost.sort_by(f64::total_cmp);
    let p50 = (indices.len() * 50).div_ceil(100) - 1;
    let p95 = (indices.len() * 95).div_ceil(100) - 1;
    println!("{label} {degrees} {initial} depth_p50={:.9} depth_p95={:.9} normal_p50_deg={:.9} normal_p95_deg={:.9} cost_p50={:.9}",
        depth[p50], depth[p95], normal[p50], normal[p95], cost[p50]);
}

fn bar(failures: &mut usize, passes: &mut usize, good: bool, message: String) {
    if good { *passes += 1; } else { *failures += 1; println!("FAIL {message}"); }
}

fn run() -> Result<bool, String> {
    let mut failures = 0;
    let mut passes = 0;
    for degrees in [0.0_f64, 30.0] {
        let (views, truth) = textured_plane(96, 120.0, degrees.to_radians(), &[-0.10, 0.10]);
        let mut params = PatchMatchParams { depth_min: 0.3, depth_max: 1.0, iterations: 0, ..Default::default() };
        let indices = pixels(&truth, params.radius);
        let host = patchmatch_reference(&views, 0, &[1, 2], None, &params)?;
        let gpu = patchmatch_cuda(&views, 0, &[1, 2], None, &params)?;
        let mut depth = 0.0_f64;
        let mut normal = 0.0_f64;
        let mut cost = 0.0_f64;
        // All initialized hypotheses, including the unchanged border, must match.
        for i in 0..host.depth.len() {
            depth = depth.max((gpu.depth[i] / host.depth[i] - 1.0).abs());
            normal = normal.max(angle(gpu.normal[i], host.normal[i]));
            if host.cost[i] < 2.0 && gpu.cost[i] < 2.0 {
                cost = cost.max((gpu.cost[i] - host.cost[i]).abs());
            }
        }
        let valid = fraction(&indices, |i| (host.cost[i] < 2.0) == (gpu.cost[i] < 2.0));
        println!("INIT {degrees} max_depth_rel={depth:.9} max_normal_deg={normal:.9} max_cost_abs={cost:.9} valid_agree={valid:.9}");
        for (good, what) in [(depth <= 1e-5, "depth"), (normal <= 1e-3, "normal"),
            (cost <= 1e-4, "cost"), (valid >= 0.999, "valid")] {
            bar(&mut failures, &mut passes, good, format!("INIT {degrees} {what}"));
        }
        params.iterations = 1;
        let host = patchmatch_reference(&views, 0, &[1, 2], None, &params)?;
        let gpu = patchmatch_cuda(&views, 0, &[1, 2], None, &params)?;
        println!("ONEITER {degrees} depth_within_1e-4={:.9}",
            fraction(&indices, |i| (gpu.depth[i] / host.depth[i] - 1.0).abs() <= 1e-4));
        params.iterations = PatchMatchParams::default().iterations;
        let host = patchmatch_reference(&views, 0, &[1, 2], None, &params)?;
        let host_truth = patchmatch_reference(&views, 0, &[1, 2], Some(&truth), &params)?;
        let gpu = patchmatch_cuda(&views, 0, &[1, 2], None, &params)?;
        let gpu_truth = patchmatch_cuda(&views, 0, &[1, 2], Some(&truth), &params)?;
        stats("PMHOST", degrees, "random", &host, &truth, &indices);
        stats("PMHOST", degrees, "truth", &host_truth, &truth, &indices);
        stats("PMGPU", degrees, "random", &gpu, &truth, &indices);
        stats("PMGPU", degrees, "truth", &gpu_truth, &truth, &indices);
        let agree = fraction(&indices, |i| (gpu.depth[i] / host.depth[i] - 1.0).abs() <= 0.01);
        println!("AGREE {degrees} depth_within_1pct={agree:.9}");
        let objective = fraction(&indices, |i| gpu.cost[i] <= gpu_truth.cost[i] + 0.02);
        let depth = fraction(&indices, |i| (gpu.depth[i] / truth.depth[i] - 1.0).abs() <= 0.01);
        let normal = fraction(&indices, |i| dot(gpu.normal[i], truth.normal[i]) >= 20.0_f64.to_radians().cos());
        println!("COVERAGE {degrees} objective={objective:.9} depth={depth:.9} normal={normal:.9}");
        for (good, what) in [(objective >= 0.95, "objective"), (depth >= 0.95, "depth"),
            (normal >= 0.90, "normal"), (agree >= 0.98, "agreement")] {
            bar(&mut failures, &mut passes, good, format!("PMGPU {degrees} {what}"));
        }
    }
    let (views, _) = textured_plane(518, 647.5, 0.5236, &[-0.10, -0.05, 0.05, 0.10]);
    let params = PatchMatchParams { depth_min: 0.3, depth_max: 1.0, ..Default::default() };
    let start = Instant::now();
    std::hint::black_box(patchmatch_reference(&views, 0, &[1, 2, 3, 4], None, &params)?);
    let host_s = start.elapsed().as_secs_f64();
    std::hint::black_box(patchmatch_cuda(&views, 0, &[1, 2, 3, 4], None, &params)?);
    let mut times = Vec::new();
    for _ in 0..5 {
        let start = Instant::now();
        std::hint::black_box(patchmatch_cuda(&views, 0, &[1, 2, 3, 4], None, &params)?);
        times.push(start.elapsed().as_secs_f64());
    }
    times.sort_by(f64::total_cmp);
    println!("TIME size=518 sources=4 host_s={host_s:.6} gpu_s={:.6}", times[2]);
    println!("CHECK passes={passes} failures={failures}");
    Ok(failures == 0)
}

fn main() {
    match run() {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(error) => { println!("FAIL {error}"); std::process::exit(1); }
    }
}
