pub mod color;
pub mod font;
pub mod font_family;
pub mod fonts;
pub mod layouter;
pub mod loader;
#[cfg(target_arch = "wasm32")]
pub mod rasterizer;
#[cfg(not(target_arch = "wasm32"))]
#[path = "rasterizer_hal.rs"]
pub mod rasterizer;
pub mod scan;
pub mod selection;
pub mod shaper;
pub mod slice;
pub mod slug_atlas;

// The font tables, outlines and rasterisation live in `makepad-font-core`, shared with the HAL's
// font service; these paths stay the text stack's.
pub use makepad_font_core::{font_atlas, font_face, geom, glyph_outline, glyph_raster_image, image, intern, msdfer, num, sdfer};
pub mod substr;

// Debug test commented out - requires png encoder
// #[cfg(test)]
// mod tests {
//     #[test]
//     fn test() { ... }
// }
pub mod async_labels;
