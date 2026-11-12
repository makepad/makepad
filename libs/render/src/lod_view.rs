//! Convert a view's model LOD distances to the screen reference used to
//! generate the levels, so zoomed or taller views retain subpixel details.

use makepad_draw::Mat4f;

/// Output height used when generating model LODs: details below about one
/// pixel at a level's switch distance may be simplified or dropped.
pub const LOD_REFERENCE_HEIGHT_PX: f32 = 2160.0;
/// Vertical field of view, in degrees, used with [`LOD_REFERENCE_HEIGHT_PX`]
/// when generating model LODs.
pub const LOD_REFERENCE_FOV_DEG: f32 = 62.0;

/// Scale camera-to-instance distance for model LOD comparisons only.
///
/// `scale = tan(fov / 2) / tan(62deg / 2) * 2160 / view_height_px`.
/// The height is the render target's physical pixel height, not layout
/// points. At the reference the scale is one; narrower lenses and taller
/// targets reduce it, keeping details until they reach the reference pixel
/// size. Compute once per view and share it with that view's shadow casters.
/// Nonpositive or non-finite inputs (or an unrepresentable result) return
/// one. For a perspective matrix, `tan_half_fov = 1 / projection[1][1]`;
/// orthographic views must use scale one instead.
pub fn lod_view_scale(tan_half_fov: f32, view_height_px: f32) -> f32 {
    if !tan_half_fov.is_finite() || tan_half_fov <= 0.0
        || !view_height_px.is_finite() || view_height_px <= 0.0
    {
        return 1.0;
    }
    let reference = (LOD_REFERENCE_FOV_DEG.to_radians() * 0.5).tan();
    let scale = (tan_half_fov as f64 / reference as f64
        * (LOD_REFERENCE_HEIGHT_PX as f64 / view_height_px as f64)) as f32;
    if scale.is_finite() && scale > 0.0 { scale } else { 1.0 }
}

/// Read the vertical perspective scale, including off-axis/jittered views.
/// Orthographic or invalid projections have no usable perspective FOV.
pub(crate) fn lod_projection_scale(projection: &Mat4f, view_height_px: f32) -> f32 {
    let p = &projection.v;
    if p.iter().any(|v| !v.is_finite()) || p[0] <= 0.0 || p[5] <= 0.0
        || p[3] != 0.0 || p[7] != 0.0 || p[11] != -1.0 || p[14] == 0.0 || p[15] != 0.0
    {
        return 1.0;
    }
    lod_view_scale(1.0 / p[5], view_height_px)
}
