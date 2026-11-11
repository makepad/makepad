//! The f32 PatchMatch search as a compute program, in-process or on a compute node.
//! Kernels run on the session's workers, or its GPU when one is supplied. The
//! blocking convenience function belongs on a worker, not on the UI thread.

use crate::patchmatch::{check_arguments, DepthNormalMap, PatchMatchParams};
use crate::views::{relative_pose, GrayView};
use makepad_compute_program::gpu::GpuLink;
use makepad_compute_program::{
    ElemTy, Input, Limits, OutputData, Program, Session, SessionConfig, Updates, Value,
};

/// The program's Splash source.
pub const PATCHMATCH: &str = include_str!("../programs/patchmatch.splash");

pub fn program() -> Program {
    Program::new("patchmatch.splash", PATCHMATCH)
}

/// Counted work and buffer bytes are limited by the device, never by a timer.
pub fn limits() -> Limits {
    Limits {
        fuel: u64::MAX,
        work: u64::MAX,
        buffer_bytes: u64::MAX,
        ..Limits::default()
    }
}

fn f32_input(name: &str, values: impl Iterator<Item = f64>) -> Input {
    Input::words(
        name,
        ElemTy::F32,
        values.map(|v| (v as f32).to_bits()).collect(),
    )
}

/// Pack all views in their original order. `seed` is two unsigned 32-bit words,
/// low first, so the wire's f64 numbers preserve every bit of the u64 seed.
/// Cameras contain f64 intrinsics followed by the row-major camera-to-world 3x4;
/// the program builds the relative poses. Initial costs are deliberately omitted.
pub fn job(
    views: &[GrayView],
    reference: usize,
    sources: &[usize],
    init: Option<&DepthNormalMap>,
    params: &PatchMatchParams,
) -> Result<(Value, Vec<Input>), String> {
    check_arguments(views, reference, sources, init, params)?;
    let r = &views[reference];
    // Buffer addressing is signed i32 in the compute kernel language.
    let mut total = 0usize;
    for view in views {
        view.check()?;
        total = total
            .checked_add(view.pixels.len())
            .ok_or("PatchMatch image buffer extent overflow")?;
    }
    if total > i32::MAX as usize
        || views.len() > i32::MAX as usize / 32
        || sources.len() > i32::MAX as usize
        || params.radius > i32::MAX as usize / 2
    {
        return Err("PatchMatch inputs exceed the compute language's i32 addressing".into());
    }
    // Program parameters cross the wire as f64 numbers.
    if params.iterations as u128 > (1u128 << 53) {
        return Err("PatchMatch iteration count is not exactly representable on the wire".into());
    }
    let low = params.depth_min as f32;
    let high = params.depth_max as f32;
    if !low.is_finite()
        || !high.is_finite()
        || low <= 0.0
        || high <= low
        || !(1.0 / low).is_finite()
        || [params.sigma_spatial, params.sigma_color]
            .iter()
            .any(|&v| !(v as f32).is_finite() || v as f32 <= 0.0)
    {
        return Err("inconsistent f32 PatchMatch parameters".into());
    }
    let mut pixels = Vec::with_capacity(total);
    let mut shapes = Vec::with_capacity(views.len() * 3);
    let mut cameras = Vec::with_capacity(views.len() * 32);
    for view in views {
        let (rotation, translation) = relative_pose(r, view);
        if view
            .k
            .iter()
            .chain(rotation.iter().flatten())
            .chain(&translation)
            .any(|&v| !(v as f32).is_finite())
            || view.k[0] as f32 <= 0.0
            || view.k[1] as f32 <= 0.0
        {
            return Err("camera is not representable in f32 PatchMatch".into());
        }
        shapes.extend([view.width as u32, view.height as u32, pixels.len() as u32]);
        pixels.extend(view.pixels.iter().map(|v| v.to_bits()));
        for &value in view.k.iter().chain(view.cam_to_world[..3].iter().flatten()) {
            let bits = value.to_bits();
            cameras.extend([bits as u32, (bits >> 32) as u32]);
        }
    }
    let mut inputs = vec![
        Input::words("images", ElemTy::F32, pixels.into()),
        Input::words("shape", ElemTy::I32, shapes.into()),
        Input::words("cameras", ElemTy::F64, cameras.into()),
    ];
    if let Some(map) = init {
        for (i, (&d, n)) in map.depth.iter().zip(&map.normal).enumerate() {
            let d = d as f32;
            let n = n.map(|v| v as f32);
            let ray = [
                ((i % r.width) as f32 - r.k[2] as f32) / r.k[0] as f32,
                ((i / r.width) as f32 - r.k[3] as f32) / r.k[1] as f32,
                1.0,
            ];
            let length = n[0] * n[0] + n[1] * n[1] + n[2] * n[2];
            let facing = n[0] * ray[0] + n[1] * ray[1] + n[2] * ray[2];
            if !d.is_finite() || d < low || d > high || (length - 1.0).abs() > 2e-6 || facing >= 0.0 {
                return Err("initial map is not representable in f32 PatchMatch".into());
            }
        }
        inputs.push(f32_input("depth", map.depth.iter().copied()));
        for (axis, name) in ["nx", "ny", "nz"].into_iter().enumerate() {
            inputs.push(f32_input(name, map.normal.iter().map(|n| n[axis])));
        }
    }
    let n = Value::Number;
    let p = Value::object(&[
        ("width", n(r.width as f64)),
        ("height", n(r.height as f64)),
        ("views", n(views.len() as f64)),
        ("reference", n(reference as f64)),
        ("sources", Value::Array(sources.iter().map(|&i| n(i as f64)).collect())),
        ("source_count", n(sources.len() as f64)),
        ("radius", n(params.radius as f64)),
        ("step", n(params.step as f64)),
        ("sigma_spatial", n(params.sigma_spatial)),
        ("sigma_color", n(params.sigma_color)),
        ("depth_min", n(params.depth_min)),
        ("depth_max", n(params.depth_max)),
        ("top_k", n(params.top_k as f64)),
        ("iterations", n(params.iterations as f64)),
        ("seed", Value::Array(vec![n(params.seed as u32 as f64), n((params.seed >> 32) as f64)])),
        ("initial", Value::Bool(init.is_some())),
    ]);
    Ok((p, inputs))
}

/// The newest published map once the program ended; merge session updates before
/// calling this, since publications and the end can arrive in separate updates.
pub fn answer(
    updates: &Updates,
    width: usize,
    height: usize,
) -> Option<Result<DepthNormalMap, String>> {
    let end = updates.end.as_ref()?;
    Some((|| {
        let value = end.as_ref().map_err(|e| e.to_string())?;
        if value.get("iterations").and_then(Value::as_number).is_none() {
            return Err(format!("PatchMatch program ended with {value:?}"));
        }
        let size = width.checked_mul(height).ok_or("PatchMatch map extent overflow")?;
        let read = |name: &str| -> Result<Vec<f64>, String> {
            let output = updates.output(name).ok_or_else(|| format!("PatchMatch published no {name}"))?;
            let OutputData::Words(words) = &output.data else {
                return Err(format!("PatchMatch {name} was not published at full precision"));
            };
            if output.ty != ElemTy::F32 || words.len() != size {
                return Err(format!("PatchMatch {name} type or length does not match the map"));
            }
            Ok(words.iter().map(|&w| f64::from(f32::from_bits(w))).collect())
        };
        let depth = read("depth")?;
        let nx = read("nx")?;
        let ny = read("ny")?;
        let nz = read("nz")?;
        let cost = read("cost")?;
        let normal = nx.into_iter().zip(ny).zip(nz).map(|((x, y), z)| [x, y, z]).collect();
        Ok(DepthNormalMap { width, height, depth, normal, cost })
    })())
}

/// Run the program in this process and wait for its answer. With `gpu`, the same
/// f32 kernels and buffers run on that device; otherwise they use the CPU pool.
pub fn patchmatch_program(
    views: &[GrayView],
    reference: usize,
    sources: &[usize],
    init: Option<&DepthNormalMap>,
    params: &PatchMatchParams,
    gpu: Option<GpuLink>,
) -> Result<DepthNormalMap, String> {
    patchmatch_program_while(views, reference, sources, init, params, gpu, &|| true)
}

/// [`patchmatch_program`] for as long as `keep` says: it is asked each time the program
/// publishes (once an iteration), and the first false cancels the program, which then ends
/// with "cancelled". For a match that gives way to work that came in meanwhile.
pub fn patchmatch_program_while(
    views: &[GrayView],
    reference: usize,
    sources: &[usize],
    init: Option<&DepthNormalMap>,
    params: &PatchMatchParams,
    gpu: Option<GpuLink>,
    keep: &dyn Fn() -> bool,
) -> Result<DepthNormalMap, String> {
    let (params, inputs) = job(views, reference, sources, init, params)?;
    let session = Session::open(
        &program(),
        SessionConfig { params, inputs, limits: limits(), gpu, ..SessionConfig::default() },
    );
    let mut updates = Updates::default();
    let mut kept = true;
    loop {
        updates.merge(session.wait());
        if let Some(answer) = answer(&updates, views[reference].width, views[reference].height) {
            return answer;
        }
        if kept && !keep() {
            session.cancel();
            kept = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{patchmatch::patchmatch_reference, synthetic::textured_plane};

    /// An app that quits while a match runs closes its GPU device under it: the match ends as
    /// cancelled. It once ended as "the GPU was lost", which nothing was.
    #[test]
    fn a_device_that_ends_under_a_match_cancels_it() {
        use makepad_compute_program::gpu::{GpuInfo, GpuLink};
        let (views, _) = textured_plane(24, 120.0, 0.0, &[0.10]);
        let (link, device) = GpuLink::new(GpuInfo { target: Default::default(), budget_bytes: 1 << 30 }, std::sync::Arc::new(|| {}));
        // the device takes the session's first message and is gone: an app quitting mid-match
        let quit = std::thread::spawn(move || drop(device.recv()));
        let params = PatchMatchParams { depth_min: 0.3, depth_max: 1.0, iterations: 1, ..Default::default() };
        let end = patchmatch_program(&views, 0, &[1], None, &params, Some(link)).err().unwrap_or_default();
        quit.join().unwrap();
        assert_eq!(end, "cancelled", "a match whose device ended is cancelled, not lost");
        // a match told to give way stops at its next publication, long before its last iteration
        let long = PatchMatchParams { iterations: 64, ..params };
        let asked = std::cell::Cell::new(0);
        let keep = || {
            asked.set(asked.get() + 1);
            false
        };
        let end = patchmatch_program_while(&views, 0, &[1], None, &long, None, &keep).err().unwrap_or_default();
        assert!(end == "cancelled" && asked.get() == 1, "a match told to give way is cancelled, asked once: {end:?} after {} asks", asked.get());
    }

    #[test]
    fn textured_plane_matches_reference_initialisation_and_search() {
        let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let angle = |a, b| (dot(a, b) / (dot(a, a) * dot(b, b)).sqrt()).clamp(-1.0, 1.0).acos().to_degrees();
        // A 16x16 update region includes both colours and all eight neighbours.
        for degrees in [0.0_f64, 30.0] {
            let (views, truth) = textured_plane(24, 120.0, degrees.to_radians(), &[-0.10, 0.10]);
            let mut params = PatchMatchParams { depth_min: 0.3, depth_max: 1.0, iterations: 0, ..Default::default() };
            let indices: Vec<usize> = (params.radius..truth.height - params.radius)
                .flat_map(|y| (params.radius..truth.width - params.radius).map(move |x| y * truth.width + x))
                .collect();
            let fraction = |predicate: &dyn Fn(usize) -> bool| indices.iter().filter(|&&i| predicate(i)).count() as f64 / indices.len() as f64;
            let host = patchmatch_reference(&views, 0, &[1, 2], None, &params).unwrap();
            let computed = patchmatch_program(&views, 0, &[1, 2], None, &params, None).unwrap();
            // INIT bars from patchmatch_check, including the unchanged border.
            for i in 0..host.depth.len() {
                assert!((computed.depth[i] / host.depth[i] - 1.0).abs() <= 1e-5, "INIT {degrees} depth at {i}");
                assert!(angle(computed.normal[i], host.normal[i]) <= 1e-3, "INIT {degrees} normal at {i}");
                if host.cost[i] < 2.0 && computed.cost[i] < 2.0 {
                    assert!((computed.cost[i] - host.cost[i]).abs() <= 1e-4, "INIT {degrees} cost at {i}");
                }
            }
            assert!(fraction(&|i| (host.cost[i] < 2.0) == (computed.cost[i] < 2.0)) >= 0.999, "INIT {degrees} valid agreement");
            params.iterations = PatchMatchParams::default().iterations;
            let host = patchmatch_reference(&views, 0, &[1, 2], None, &params).unwrap();
            let computed = patchmatch_program(&views, 0, &[1, 2], None, &params, None).unwrap();
            let seeded = patchmatch_program(&views, 0, &[1, 2], Some(&truth), &params, None).unwrap();
            // PMGPU bars from patchmatch_check, on the same seed and parameters.
            assert!(fraction(&|i| computed.cost[i] <= seeded.cost[i] + 0.02) >= 0.95, "PMGPU {degrees} objective coverage");
            assert!(fraction(&|i| (computed.depth[i] / truth.depth[i] - 1.0).abs() <= 0.01) >= 0.95, "PMGPU {degrees} depth coverage");
            assert!(fraction(&|i| dot(computed.normal[i], truth.normal[i]) >= 20.0_f64.to_radians().cos()) >= 0.90, "PMGPU {degrees} normal coverage");
            assert!(fraction(&|i| (computed.depth[i] / host.depth[i] - 1.0).abs() <= 0.01) >= 0.98, "PMGPU {degrees} reference agreement");
        }
    }
}
