//! Plane-based multi-view PatchMatch with deterministic per-pixel draws.

/// Camera intrinsics and reference-to-camera rigid transform. Images are
/// concatenated in camera order; the first camera is the reference.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GeometryCamera {
    pub k: [f32; 4],
    pub rotation: [f32; 9],
    pub translation: [f32; 3],
    pub width: u32,
    pub height: u32,
    pub offset: u64,
}

/// GPU search parameters, matching the host's window and draw conventions.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GeometryParams {
    pub seed: u64,
    pub width: u32,
    pub height: u32,
    pub radius: u32,
    pub step: u32,
    pub top_k: u32,
    pub sources: u32,
    pub sigma_spatial: f32,
    pub sigma_color: f32,
    pub depth_min: f32,
    pub depth_max: f32,
}

/// Optional initial hypotheses, in reference-image row-major order.
pub struct GeometryInitial<'a> {
    pub depth: &'a [f32],
    pub normal: &'a [[f32; 3]],
}

/// Final reference-image maps, including unchanged border hypotheses.
pub struct GeometryMap {
    pub depth: Vec<f32>,
    pub normal: Vec<[f32; 3]>,
    pub cost: Vec<f32>,
}

#[cfg(all(any(target_os = "linux", target_os = "windows"), makepad_ai_cuda_kernels))]
pub(crate) mod ffi {
    use super::{GeometryCamera, GeometryParams};
    use crate::{cudaError_t, cudaStream_t};
    cuda_ffi! {
        pub fn makepad_cuda_geometry_init(
            images: *const f32, cameras: *const GeometryCamera,
            params: GeometryParams, map: *mut f32, initial: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;
        pub fn makepad_cuda_geometry_sweep(
            images: *const f32, cameras: *const GeometryCamera,
            params: GeometryParams, map: *mut f32, sweep: GeometrySweep,
            stream: cudaStream_t,
        ) -> cudaError_t;
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct GeometrySweep {
        pub iteration: u64,
        pub colour: u32,
        pub scale: f32,
    }
}

/// Run init and all red/black sweeps on the backend stream, then download once.
/// Up to sixteen source views are supported. Invalid buffers, parameters, or
/// hypotheses return an error before any GPU operation.
pub fn patchmatch(
    images: &[f32], cameras: &[GeometryCamera], params: &GeometryParams,
    initial: Option<GeometryInitial<'_>>, iterations: usize,
) -> Result<GeometryMap, String> {
    if cameras.len() < 2 || cameras.len() > 17 || params.sources as usize != cameras.len() - 1 {
        return Err("PatchMatch requires between one and sixteen source cameras".into());
    }
    if params.radius == 0 || params.radius > i32::MAX as u32 / 2 || params.step == 0
        || params.top_k == 0 || !params.depth_min.is_finite() || params.depth_min <= 0.0
        || !params.depth_max.is_finite() || params.depth_max <= params.depth_min
        || !(1.0 / params.depth_min).is_finite()
        || !params.sigma_spatial.is_finite() || params.sigma_spatial <= 0.0
        || !params.sigma_color.is_finite() || params.sigma_color <= 0.0
    {
        return Err("inconsistent f32 PatchMatch parameters".into());
    }
    let mut total = 0usize;
    for camera in cameras {
        if camera.width == 0 || camera.height == 0
            || camera.width > i32::MAX as u32 || camera.height > i32::MAX as u32
            || camera.offset != total as u64
            || camera.k.iter().chain(&camera.rotation).chain(&camera.translation).any(|v| !v.is_finite())
            || camera.k[0] <= 0.0 || camera.k[1] <= 0.0
        {
            return Err("invalid geometry camera or image offset".into());
        }
        let size = (camera.width as usize).checked_mul(camera.height as usize)
            .ok_or("geometry image size overflow")?;
        total = total.checked_add(size).ok_or("geometry image buffer overflow")?;
    }
    if images.len() != total || images.iter().any(|v| !v.is_finite() || !(0.0..=1.0).contains(v)) {
        return Err("invalid geometry image buffer".into());
    }
    let reference = &cameras[0];
    if params.width != reference.width || params.height != reference.height {
        return Err("geometry reference dimensions do not match parameters".into());
    }
    let size = reference.width as usize * reference.height as usize;
    // The counter reserves forty bits for the pixel index.
    if size as u64 >= 1u64 << 40 || size.checked_mul(5 * size_of::<f32>()).is_none() {
        return Err("geometry reference image exceeds addressable extent".into());
    }
    if let Some(init) = &initial {
        if init.depth.len() != size || init.normal.len() != size {
            return Err("initial geometry buffers do not match the reference".into());
        }
        for (i, (&depth, normal)) in init.depth.iter().zip(init.normal).enumerate() {
            let ray = [((i % reference.width as usize) as f32 - reference.k[2]) / reference.k[0],
                ((i / reference.width as usize) as f32 - reference.k[3]) / reference.k[1], 1.0];
            let length: f32 = normal.iter().map(|v| v * v).sum();
            let facing: f32 = normal.iter().zip(ray).map(|(a, b)| a * b).sum();
            if !depth.is_finite() || depth < params.depth_min || depth > params.depth_max
                || normal.iter().any(|v| !v.is_finite()) || (length - 1.0).abs() > 2e-6 || facing >= 0.0
            {
                return Err("initial geometry depths or normals are invalid".into());
            }
        }
    }
    run(images, cameras, params, initial, iterations)
}

#[cfg(all(any(target_os = "linux", target_os = "windows"), makepad_ai_cuda_kernels))]
fn run(images: &[f32], cameras: &[GeometryCamera], params: &GeometryParams,
    initial: Option<GeometryInitial<'_>>, iterations: usize) -> Result<GeometryMap, String> {
    crate::launch::gpu_geometry_patchmatch(images, cameras, params, initial, iterations)
}

#[cfg(not(all(any(target_os = "linux", target_os = "windows"), makepad_ai_cuda_kernels)))]
fn run(_: &[f32], _: &[GeometryCamera], _: &GeometryParams,
    _: Option<GeometryInitial<'_>>, _: usize) -> Result<GeometryMap, String> {
    Err("CUDA geometry kernels are unavailable in this build".into())
}
