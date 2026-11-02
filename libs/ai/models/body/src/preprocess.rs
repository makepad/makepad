//! CPU crop, camera conditioning, and patch-ray construction.

use crate::{IMAGE_SIZE, PATCH};

const IMAGENET_MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const IMAGENET_STD: [f32; 3] = [0.229, 0.224, 0.225];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CropGeometry {
    pub center: [f32; 2],
    pub side: f32,
    /// Row-major 2x3 affine mapping full-image pixels to crop pixels.
    pub affine: [f32; 6],
    pub focal: f32,
    pub principal: [f32; 2],
    /// The crop's pixel size (square, a multiple of the patch): 512 is what
    /// the model was trained at; smaller trades accuracy for speed.
    pub crop: usize,
}

pub fn crop_geometry(
    bbox_xyxy: [f32; 4],
    image_w: usize,
    image_h: usize,
    intrinsics: Option<[f32; 3]>,
) -> CropGeometry {
    crop_geometry_at(bbox_xyxy, image_w, image_h, intrinsics, IMAGE_SIZE, 1.25)
}

pub fn crop_geometry_at(
    bbox_xyxy: [f32; 4],
    image_w: usize,
    image_h: usize,
    intrinsics: Option<[f32; 3]>,
    crop: usize,
    padding: f32,
) -> CropGeometry {
    let center = [
        0.5 * (bbox_xyxy[0] + bbox_xyxy[2]),
        0.5 * (bbox_xyxy[1] + bbox_xyxy[3]),
    ];
    let mut scale = [
        (bbox_xyxy[2] - bbox_xyxy[0]) * padding,
        (bbox_xyxy[3] - bbox_xyxy[1]) * padding,
    ];
    if scale[0] > scale[1] * 0.75 {
        scale[1] = scale[0] / 0.75;
    } else {
        scale[0] = scale[1] * 0.75;
    }
    let side = scale[0].max(scale[1]);
    let k = crop as f32 / side;
    let affine = [
        k,
        0.0,
        0.5 * crop as f32 - k * center[0],
        0.0,
        k,
        0.5 * crop as f32 - k * center[1],
    ];
    let [focal, cx, cy] = intrinsics.unwrap_or_else(|| {
        let w = image_w as f32;
        let h = image_h as f32;
        [(w * w + h * h).sqrt(), 0.5 * w, 0.5 * h]
    });
    CropGeometry {
        center,
        side,
        affine,
        focal,
        principal: [cx, cy],
        crop,
    }
}

fn rgb_at(rgb: &[u8], w: usize, h: usize, x: isize, y: isize, c: usize) -> f32 {
    if x < 0 || y < 0 || x >= w as isize || y >= h as isize {
        return 0.0;
    }
    let index = (y as usize * w + x as usize) * 3 + c;
    rgb.get(index).copied().unwrap_or(0) as f32
}

fn bilinear_zero_border(rgb: &[u8], w: usize, h: usize, x: f32, y: f32, c: usize) -> f32 {
    let x0 = x.floor() as isize;
    let y0 = y.floor() as isize;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let top = rgb_at(rgb, w, h, x0, y0, c) * (1.0 - fx)
        + rgb_at(rgb, w, h, x0 + 1, y0, c) * fx;
    let bottom = rgb_at(rgb, w, h, x0, y0 + 1, c) * (1.0 - fx)
        + rgb_at(rgb, w, h, x0 + 1, y0 + 1, c) * fx;
    (top * (1.0 - fy) + bottom * fy).clamp(0.0, 255.0)
}

pub fn crop_normalized(
    rgb: &[u8],
    w: usize,
    h: usize,
    geo: &CropGeometry,
) -> Vec<f32> {
    crop_normalized_mirrored(rgb, w, h, geo, false)
}

/// Crop and normalise, optionally sampling the horizontally mirrored source
/// image. Mirroring happens before the affine warp, matching a real flipped
/// full-image buffer without allocating that buffer.
pub fn crop_normalized_mirrored(
    rgb: &[u8],
    w: usize,
    h: usize,
    geo: &CropGeometry,
    mirror: bool,
) -> Vec<f32> {
    let crop = geo.crop;
    let mut output = vec![0.0; 3 * crop * crop];
    let k = geo.affine[0];
    let plane = crop * crop;
    // Row bands across the machine's cores: the warp is 0.8M samples of
    // scalar bilinear work, 3 ms on one core.
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 16);
    let band = crop.div_ceil(threads);
    let (r, g, b) = {
        let (r, rest) = output.split_at_mut(plane);
        let (g, b) = rest.split_at_mut(plane);
        (r, g, b)
    };
    std::thread::scope(|scope| {
        for (((r_band, g_band), b_band), band_index) in r
            .chunks_mut(band * crop)
            .zip(g.chunks_mut(band * crop))
            .zip(b.chunks_mut(band * crop))
            .zip(0..)
        {
            scope.spawn(move || {
                let planes = [r_band, g_band, b_band];
                let v0 = band_index * band;
                for (row, v) in (v0..).enumerate().take(planes[0].len() / crop) {
                    let src_y = (v as f32 - geo.affine[5]) / k;
                    for u in 0..crop {
                        let mut src_x = (u as f32 - geo.affine[2]) / k;
                        if mirror {
                            src_x = w as f32 - 1.0 - src_x;
                        }
                        for c in 0..3 {
                            let pixel = bilinear_zero_border(rgb, w, h, src_x, src_y, c) / 255.0;
                            planes[c][row * crop + u] = (pixel - IMAGENET_MEAN[c]) / IMAGENET_STD[c];
                        }
                    }
                }
            });
        }
    });
    output
}

pub fn condition_info(geo: &CropGeometry) -> [f32; 3] {
    [
        (geo.center[0] - geo.principal[0]) / geo.focal,
        (geo.center[1] - geo.principal[1]) / geo.focal,
        geo.side / geo.focal,
    ]
}

/// Where patch `index` samples the 512-wide crop axis: the reference shrinks
/// the ray field by 16 with an antialiased bilinear filter, a triangle of
/// half-width 16 taps centred on `(index + 0.5) * 16 - 0.5`, normalised over
/// the taps that fall inside the image. The rays are affine, so the filter
/// reduces to sampling at the taps' weighted mean position: the block centre
/// for interior patches, pulled inward at the two edges (oracle-verified:
/// block centres are 0.1 off in the conditioned context, this is 1e-3).
pub fn patch_sample_coord(index: usize) -> f32 {
    patch_sample_coord_at(index, IMAGE_SIZE)
}

pub fn patch_sample_coord_at(index: usize, crop: usize) -> f32 {
    let centre = (index as f32 + 0.5) * PATCH as f32 - 0.5;
    let mut weight_sum = 0.0f32;
    let mut coord_sum = 0.0f32;
    let lo = (centre - PATCH as f32).floor().max(0.0) as usize;
    let hi = ((centre + PATCH as f32).ceil() as usize).min(crop - 1);
    for tap in lo..=hi {
        let weight = (1.0 - (tap as f32 - centre).abs() / PATCH as f32).max(0.0);
        weight_sum += weight;
        coord_sum += weight * tap as f32;
    }
    coord_sum / weight_sum
}

pub fn patch_rays(geo: &CropGeometry) -> Vec<f32> {
    let side = geo.crop / PATCH;
    let mut rays = Vec::with_capacity(side * side * 2);
    let k = geo.affine[0];
    let coords: Vec<f32> = (0..side).map(|i| patch_sample_coord_at(i, geo.crop)).collect();
    for gy in 0..side {
        let full_y = (coords[gy] - geo.affine[5]) / k;
        for gx in 0..side {
            let full_x = (coords[gx] - geo.affine[2]) / k;
            rays.push((full_x - geo.principal[0]) / geo.focal);
            rays.push((full_y - geo.principal[1]) / geo.focal);
        }
    }
    rays
}

pub fn full_to_crop(kp2d_full: &[f32], geo: &CropGeometry) -> Vec<f32> {
    let mut output = Vec::with_capacity(kp2d_full.len() / 2 * 2);
    for point in kp2d_full.chunks_exact(2) {
        let x = geo.affine[0] * point[0] + geo.affine[1] * point[1] + geo.affine[2];
        let y = geo.affine[3] * point[0] + geo.affine[4] * point[1] + geo.affine[5];
        output.push(x / geo.crop as f32 - 0.5);
        output.push(y / geo.crop as f32 - 0.5);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PATCHES_SIDE;

    fn assert_close(actual: f32, expected: f32, tolerance: f32) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "actual={actual} expected={expected} tolerance={tolerance}"
        );
    }

    #[test]
    fn geometry_fixes_aspect_and_centers_affine() {
        let geo = crop_geometry([10.0, 20.0, 50.0, 50.0], 100, 80, Some([100.0, 50.0, 40.0]));
        assert_eq!(geo.center, [30.0, 35.0]);
        // 1.25 * width = 50; the 0.75 aspect fix makes height 50 / 0.75.
        assert_close(geo.side, 200.0 / 3.0, 1e-5);
        let mapped_x = geo.affine[0] * geo.center[0] + geo.affine[2];
        let mapped_y = geo.affine[4] * geo.center[1] + geo.affine[5];
        assert_close(mapped_x, 256.0, 1e-5);
        assert_close(mapped_y, 256.0, 1e-5);
    }

    #[test]
    fn patch_rays_use_antialiased_tap_positions() {
        // Interior patches sit on their block centre; the two edge patches
        // are pulled inward by the clipped triangle filter (symmetrically).
        assert_close(patch_sample_coord(1), 23.5, 1e-5);
        assert_close(patch_sample_coord(16), 263.5, 1e-5);
        assert_close(patch_sample_coord(0), 9.026_786, 1e-4);
        assert_close(patch_sample_coord(31), 511.0 - 9.026_786, 1e-4);
        let geo = CropGeometry {
            center: [256.0, 256.0],
            side: 512.0,
            affine: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            focal: 512.0,
            principal: [256.0, 256.0],
            crop: IMAGE_SIZE,
        };
        let rays = patch_rays(&geo);
        assert_eq!(rays.len(), 1024 * 2);
        let token = 5 * PATCHES_SIDE + 3;
        assert_close(rays[token * 2], (55.5 - 256.0) / 512.0, 1e-6);
        assert_close(rays[token * 2 + 1], (87.5 - 256.0) / 512.0, 1e-6);
    }
}
