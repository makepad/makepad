//! Asynchronous back-to-front ordering of splats for the ViewSplat renderer.
//!
//! A worker thread owns a read-only mirror of the scene (centers + a radius
//! bound per splat) and answers `Sort` requests with the list of VISIBLE
//! splat ids ordered far-to-near, as f32 ids ready to be the instance stream
//! of one instanced quad draw. The sort is a parallel counting sort on
//! 16-bit depth keys (std::thread::scope; input-parallel histograms, one
//! prefix, bucket-parallel scatter so every thread writes its own contiguous
//! output range — no unsafe), with frustum / behind-camera / tiny-splat
//! culling folded into the key pass so culled splats never reach the GPU.
//!
//! The caller keeps the previous order rendering until a result lands and
//! only asks again when the camera moved past its thresholds; at most one
//! request is in flight and the newest camera wins.

use makepad_widgets::makepad_draw::*;
use std::time::Instant;

/// Keys 0..=CULLED-1 are depth buckets; CULLED marks a splat that is not
/// drawn for this camera.
const CULLED: u16 = u16::MAX;
const BUCKETS: usize = CULLED as usize; // 65535 usable depth buckets

/// Sort-side mirror of a scene, in GPU record order.
#[derive(Clone, Debug, Default)]
pub struct SortScene {
    pub centers: Vec<[f32; 3]>,
    /// Largest axis length per splat (local units), for the projected-size
    /// bound; negative marks a padding slot that is never drawn.
    pub radius_bound: Vec<f32>,
    /// Product of the two largest axis lengths: the splat's face area scale,
    /// for the overdraw estimate.
    pub axis_product: Vec<f32>,
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
}

impl SortScene {
    pub fn new(centers: Vec<[f32; 3]>, radius_bound: Vec<f32>, axis_product: Vec<f32>) -> Self {
        debug_assert_eq!(centers.len(), radius_bound.len());
        debug_assert_eq!(centers.len(), axis_product.len());
        let mut bounds_min = [f32::INFINITY; 3];
        let mut bounds_max = [f32::NEG_INFINITY; 3];
        for c in &centers {
            for axis in 0..3 {
                bounds_min[axis] = bounds_min[axis].min(c[axis]);
                bounds_max[axis] = bounds_max[axis].max(c[axis]);
            }
        }
        if centers.is_empty() {
            bounds_min = [0.0; 3];
            bounds_max = [0.0; 3];
        }
        Self {
            centers,
            radius_bound,
            axis_product,
            bounds_min,
            bounds_max,
        }
    }
}

/// Everything a sort needs from the frame: the camera and the same
/// projection knobs the vertex shader culls with, so CPU culling is never
/// tighter than the shader's own.
#[derive(Clone, Copy, Debug)]
pub struct SortCamera {
    pub view: Mat4f,
    pub model: Mat4f,
    pub projection: Mat4f,
    /// true: order by distance to the camera (rotation-invariant, used for
    /// world scenes); false: by view-space depth.
    pub radial: bool,
    pub focal_px: f32,
    pub ndc_per_px: Vec2f,
    pub splat_std_dev: f32,
    pub coarse_cull_guard: f32,
    pub min_pixel_radius: f32,
    pub max_pixel_radius: f32,
    /// Extra NDC kept outside the frustum edge so splats that enter the view
    /// between sorts already render with the stale order.
    pub cull_margin_ndc: f32,
    /// View-space distance behind the camera plane still kept.
    pub behind_margin: f32,
    pub viewport_px: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SortStats {
    pub total: usize,
    pub visible: usize,
    /// Estimated fragments shaded per viewport pixel: sum over visible
    /// splats of the face-on quad area (2*sigma*a)(2*sigma*b) at the splat's
    /// depth, each clamped to the max pixel radius, over the viewport area.
    /// Face-on is the worst case per splat, so this bounds the real overdraw
    /// from above (edge-on splats are thinner).
    pub est_quad_overdraw: f32,
    pub sort_ms: f64,
}

pub enum SortRequest {
    SetScene {
        generation: u64,
        scene: SortScene,
    },
    Sort {
        generation: u64,
        request_id: u64,
        camera: SortCamera,
        /// A previous order buffer handed back for reuse.
        recycled: Option<Vec<f32>>,
    },
}

pub struct SortResult {
    pub generation: u64,
    pub request_id: u64,
    /// Visible splat ids, far to near, as exact f32 integers.
    pub order: Vec<f32>,
    pub stats: SortStats,
}

/// Reused per-worker buffers.
#[derive(Default)]
pub struct SortScratch {
    keys: Vec<u16>,
    histograms: Vec<Vec<u32>>,
    prefix: Vec<u32>,
}

fn worker_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .saturating_sub(1)
        .clamp(1, 16)
}

/// Depth metric range over the scene bounds for this camera: view-space z
/// (more negative = farther) or minus the distance to the camera.
fn metric_range(scene: &SortScene, cam: &SortCamera, mv: &Mat4f) -> (f32, f32) {
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    let mut corner_dist_max = 0.0f32;
    let mut box_min_v = [f32::INFINITY; 3];
    let mut box_max_v = [f32::NEG_INFINITY; 3];
    for i in 0..8 {
        let p = [
            if i & 1 == 0 { scene.bounds_min[0] } else { scene.bounds_max[0] },
            if i & 2 == 0 { scene.bounds_min[1] } else { scene.bounds_max[1] },
            if i & 4 == 0 { scene.bounds_min[2] } else { scene.bounds_max[2] },
        ];
        let v = mv.transform_vec4(vec4(p[0], p[1], p[2], 1.0));
        lo = lo.min(v.z);
        hi = hi.max(v.z);
        let d = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt();
        corner_dist_max = corner_dist_max.max(d);
        box_min_v[0] = box_min_v[0].min(v.x);
        box_min_v[1] = box_min_v[1].min(v.y);
        box_min_v[2] = box_min_v[2].min(v.z);
        box_max_v[0] = box_max_v[0].max(v.x);
        box_max_v[1] = box_max_v[1].max(v.y);
        box_max_v[2] = box_max_v[2].max(v.z);
    }
    if cam.radial {
        // Camera is the view-space origin: distance to the transformed box
        // is at least the distance to its axis-aligned hull there.
        let mut d2 = 0.0f32;
        for axis in 0..3 {
            let gap = if box_min_v[axis] > 0.0 {
                box_min_v[axis]
            } else if box_max_v[axis] < 0.0 {
                -box_max_v[axis]
            } else {
                0.0
            };
            d2 += gap * gap;
        }
        (-corner_dist_max, -d2.sqrt())
    } else {
        (lo, hi)
    }
}

/// Sort the visible splats of `scene` for `cam` into `out` (cleared first).
/// Returns the stats; `out` holds `stats.visible` ids.
pub fn sort_visible(
    scene: &SortScene,
    cam: &SortCamera,
    scratch: &mut SortScratch,
    out: &mut Vec<f32>,
) -> SortStats {
    let started = Instant::now();
    let count = scene.centers.len();
    out.clear();
    if count == 0 {
        return SortStats::default();
    }
    // Small scenes do not amortize thread spawns: ~32k splats per thread.
    let threads = worker_threads().min(count.div_ceil(32768).max(1));
    scratch.keys.resize(count, CULLED);
    scratch.histograms.resize_with(threads, Vec::new);
    for hist in scratch.histograms.iter_mut() {
        hist.clear();
        hist.resize(BUCKETS, 0);
    }

    let mv = Mat4f::mul(&cam.view, &cam.model);
    let (metric_lo, metric_hi) = metric_range(scene, cam, &mv);
    let metric_scale = if metric_hi > metric_lo {
        (BUCKETS as f32 - 1.0) / (metric_hi - metric_lo)
    } else {
        0.0
    };
    let proj = cam.projection;
    let std_dev_bound = cam.splat_std_dev * 1.732051; // max axis -> sphere bound (as in the shader)
    let focal = cam.focal_px.max(1e-5);
    let cull_guard = cam.coarse_cull_guard.max(0.0);
    let ndc_per_px = vec2(cam.ndc_per_px.x.max(1e-6), cam.ndc_per_px.y.max(1e-6));
    let min_px = cam.min_pixel_radius;
    let max_px = if cam.max_pixel_radius > 0.0 {
        cam.max_pixel_radius
    } else {
        1.0e6
    };
    let margin = cam.cull_margin_ndc.max(0.0);
    let behind = cam.behind_margin.max(0.0);
    let inv_viewport = 1.0 / cam.viewport_px.max(1.0);

    // Pass 1 (input-parallel): keys + per-thread histograms + stats.
    let chunk = count.div_ceil(threads);
    let mut thread_stats = vec![(0usize, 0.0f32); threads];
    std::thread::scope(|scope| {
        let centers = &scene.centers;
        let radius_bound = &scene.radius_bound;
        let axis_product = &scene.axis_product;
        for (((t, keys), hist), stats_slot) in scratch
            .keys
            .chunks_mut(chunk)
            .enumerate()
            .zip(scratch.histograms.iter_mut())
            .zip(thread_stats.iter_mut())
        {
            let start = t * chunk;
            scope.spawn(move || {
                let mut visible = 0usize;
                let mut area = 0.0f32;
                for (k, key) in keys.iter_mut().enumerate() {
                    let i = start + k;
                    let bound = radius_bound[i];
                    if bound < 0.0 {
                        // Chunk padding slot.
                        *key = CULLED;
                        continue;
                    }
                    let c = centers[i];
                    let v = mv.transform_vec4(vec4(c[0], c[1], c[2], 1.0));
                    if v.z > behind {
                        *key = CULLED;
                        continue;
                    }
                    let clip = proj.transform_vec4(v);
                    let inv_w = 1.0 / clip.w.abs().max(1e-6);
                    let ndc_x = clip.x * inv_w;
                    let ndc_y = clip.y * inv_w;
                    let inv_depth = 1.0 / (-v.z).max(1e-6);
                    let ndc_guard = 1.0 + cull_guard * ndc_x.abs().max(ndc_y.abs());
                    let rough_px = std_dev_bound * bound * focal * inv_depth;
                    let rough_px_guarded = rough_px * ndc_guard;
                    if rough_px_guarded < min_px {
                        *key = CULLED;
                        continue;
                    }
                    if ndc_x.abs() > 1.0 + margin + rough_px_guarded * ndc_per_px.x
                        || ndc_y.abs() > 1.0 + margin + rough_px_guarded * ndc_per_px.y
                    {
                        *key = CULLED;
                        continue;
                    }
                    let metric = if cam.radial {
                        -(v.x * v.x + v.y * v.y + v.z * v.z).sqrt()
                    } else {
                        v.z
                    };
                    let b = ((metric - metric_lo) * metric_scale)
                        .clamp(0.0, BUCKETS as f32 - 1.0) as u16;
                    *key = b;
                    hist[b as usize] += 1;
                    visible += 1;
                    // Face-on quad: half-extents std_dev * axis * focal / depth.
                    let px_scale = cam.splat_std_dev * focal * inv_depth;
                    let r = (px_scale * bound).min(max_px);
                    let ab = (px_scale * px_scale * axis_product[i]).min(r * r);
                    area += 4.0 * ab;
                }
                *stats_slot = (visible, area);
            });
        }
    });

    let visible: usize = thread_stats.iter().map(|s| s.0).sum();
    let area: f32 = thread_stats.iter().map(|s| s.1).sum();

    // Exclusive prefix over buckets of the summed histogram: stream the
    // per-thread rows into one total (contiguous adds), then scan it.
    scratch.prefix.clear();
    scratch.prefix.resize(BUCKETS + 1, 0);
    {
        let (first, rest) = scratch.histograms.split_first().expect("threads >= 1");
        let total = &mut scratch.prefix[..BUCKETS];
        total.copy_from_slice(first);
        for hist in rest {
            for (t, h) in total.iter_mut().zip(hist.iter()) {
                *t += *h;
            }
        }
    }
    let mut running = 0u32;
    for b in 0..BUCKETS {
        let n = scratch.prefix[b];
        scratch.prefix[b] = running;
        running += n;
    }
    scratch.prefix[BUCKETS] = running;
    debug_assert_eq!(running as usize, visible);

    // Pass 2 (bucket-parallel): each thread owns a bucket range whose output
    // region is a contiguous, disjoint slice; it walks all keys in input
    // order (stable) and writes the ones in its range.
    out.resize(visible, 0.0);
    let per_thread = visible.div_ceil(threads).max(1);
    let mut ranges: Vec<(usize, usize)> = Vec::with_capacity(threads); // bucket [lo, hi)
    let mut b = 0usize;
    while b < BUCKETS {
        let target = (scratch.prefix[b] as usize + per_thread).min(visible);
        let mut hi = b + 1;
        // `<=` so the range runs through empty tail buckets (once every
        // element is assigned the rest of the bucket space is one range).
        while hi < BUCKETS && (scratch.prefix[hi] as usize) <= target {
            hi += 1;
        }
        ranges.push((b, hi));
        b = hi;
    }
    debug_assert!(ranges.len() <= threads + 1, "{} ranges", ranges.len());
    std::thread::scope(|scope| {
        let keys = &scratch.keys;
        let prefix = &scratch.prefix;
        let mut rest: &mut [f32] = out.as_mut_slice();
        let mut consumed = 0usize;
        for (lo, hi) in ranges {
            let begin = prefix[lo] as usize;
            let end = prefix[hi] as usize;
            let (_, tail) = std::mem::take(&mut rest).split_at_mut(begin - consumed);
            let (mine, tail) = tail.split_at_mut(end - begin);
            rest = tail;
            consumed = end;
            scope.spawn(move || {
                let lo16 = lo as u16;
                let hi16 = hi as u16;
                let mut cursors: Vec<u32> = (lo..hi).map(|b| prefix[b] - prefix[lo]).collect();
                for (i, &key) in keys.iter().enumerate() {
                    if key >= lo16 && key < hi16 {
                        let cursor = &mut cursors[(key - lo16) as usize];
                        mine[*cursor as usize] = i as f32;
                        *cursor += 1;
                    }
                }
            });
        }
    });

    SortStats {
        total: count,
        visible,
        est_quad_overdraw: area * inv_viewport,
        sort_ms: started.elapsed().as_secs_f64() * 1000.0,
    }
}

pub fn run_sort_worker(request_rx: FromUIReceiver<SortRequest>, result_tx: ToUISender<SortResult>) {
    let mut scene_generation = 0u64;
    let mut scene = SortScene::default();
    let mut scratch = SortScratch::default();
    let mut spare: Vec<f32> = Vec::new();
    while let Ok(request) = request_rx.recv() {
        match request {
            SortRequest::SetScene {
                generation,
                scene: new_scene,
            } => {
                scene_generation = generation;
                scene = new_scene;
                scratch = SortScratch::default();
                spare = Vec::new();
            }
            SortRequest::Sort {
                generation,
                request_id,
                camera,
                recycled,
            } => {
                if let Some(buffer) = recycled {
                    if buffer.capacity() > spare.capacity() {
                        spare = buffer;
                    }
                }
                let mut order = std::mem::take(&mut spare);
                let stats = if generation == scene_generation {
                    sort_visible(&scene, &camera, &mut scratch, &mut order)
                } else {
                    order.clear();
                    SortStats::default()
                };
                let _ = result_tx.send(SortResult {
                    generation,
                    request_id,
                    order,
                    stats,
                });
            }
        }
    }
}

#[cfg(test)]
include!("../tests/obj/splat_sort.rs");
