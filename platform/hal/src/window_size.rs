//! A window's size in pixels: the one conversion from a window's points and dpi, shared by the HAL,
//! the boot screen, a WM and an app host. No dependencies, so the platform crate compiles this file
//! in directly on the web (no HAL there), the way it does `window_controls`.

/// A window of `points` at `dpi` in whole pixels: round(points × dpi), at least one.
///
/// The one points-to-pixels conversion of a window: a WM makes a window's shared images this
/// size, the HAL's window shell and a session's embedded window take it as the window's pixels,
/// and the boot screen and an app's frames size the window's pass to it. The size and dpi are
/// taken at f32 first, as they reach an app (`HalEvent::Geom` carries an f32 dpi, a session keeps
/// f32 geometry), so every side makes the same pixels from the numbers it has. A frame a pixel
/// larger than its window's shared images is drawn offscreen and never shown: the boot screen's
/// f32 points × dpi made 1352 × 789 frames for the WM's 1352 × 788 images of a tile at 2.43, and
/// the app behind it never started.
pub fn window_pixels(points: [f64; 2], dpi: f64) -> [u32; 2] {
    let dpi = dpi as f32 as f64;
    points.map(|p| (p as f32 as f64 * dpi).round().max(1.0) as u32)
}
