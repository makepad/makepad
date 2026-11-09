//! CUDA implementation of the deterministic host PatchMatch search.

use crate::patchmatch::{check_arguments, DepthNormalMap, PatchMatchParams};
use crate::views::{relative_pose, GrayView};
use makepad_ai_cuda::geometry_ops::{self, GeometryCamera, GeometryInitial, GeometryParams};

/// Same hypotheses, candidate order and output conventions as the host reference.
/// GPU arithmetic uses f32; returned components are widened to f64.
/// At most sixteen sources and f32-representable cameras/parameters are supported.
pub fn patchmatch_cuda(
    views: &[GrayView], reference: usize, sources: &[usize], init: Option<&DepthNormalMap>,
    params: &PatchMatchParams,
) -> Result<DepthNormalMap, String> {
    check_arguments(views, reference, sources, init, params)?;
    if sources.len() > 16 {
        return Err("CUDA PatchMatch supports at most sixteen sources".into());
    }
    let r = &views[reference];
    let mut images = Vec::new();
    let mut cameras = Vec::new();
    for index in std::iter::once(reference).chain(sources.iter().copied()) {
        let v = &views[index];
        let (rotation, translation) = relative_pose(r, v);
        cameras.push(GeometryCamera {
            k: v.k.map(|x| x as f32),
            rotation: std::array::from_fn(|i| rotation[i / 3][i % 3] as f32),
            translation: translation.map(|x| x as f32),
            width: u32::try_from(v.width).map_err(|_| "image width exceeds u32")?,
            height: u32::try_from(v.height).map_err(|_| "image height exceeds u32")?,
            offset: images.len() as u64,
        });
        images.extend_from_slice(&v.pixels);
    }
    let p = GeometryParams {
        seed: params.seed,
        width: cameras[0].width,
        height: cameras[0].height,
        radius: u32::try_from(params.radius).map_err(|_| "radius exceeds u32")?,
        step: u32::try_from(params.step).map_err(|_| "step exceeds u32")?,
        top_k: params.top_k.min(sources.len()) as u32,
        sources: sources.len() as u32,
        sigma_spatial: params.sigma_spatial as f32,
        sigma_color: params.sigma_color as f32,
        depth_min: params.depth_min as f32,
        depth_max: params.depth_max as f32,
    };
    let depths: Vec<f32> = init.map_or_else(Vec::new, |m| m.depth.iter().map(|&v| v as f32).collect());
    let normals: Vec<[f32; 3]> = init.map_or_else(Vec::new, |m| m.normal.iter().map(|n| n.map(|v| v as f32)).collect());
    let initial = init.map(|_| GeometryInitial { depth: &depths, normal: &normals });
    let map = geometry_ops::patchmatch(&images, &cameras, &p, initial, params.iterations)?;
    Ok(DepthNormalMap { width: r.width, height: r.height,
        depth: map.depth.into_iter().map(f64::from).collect(),
        normal: map.normal.into_iter().map(|n| n.map(f64::from)).collect(),
        cost: map.cost.into_iter().map(f64::from).collect() })
}
