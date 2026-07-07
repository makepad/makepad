pub mod color;
#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
pub mod color_emoji_render;
pub mod font;
pub mod font_atlas;
pub mod font_face;
pub mod font_family;
pub mod fonts;
pub mod geom;
pub mod glyph_outline;
pub mod glyph_raster_image;
#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
pub mod hvgl_render;
pub mod image;
pub mod intern;
pub mod layouter;
pub mod loader;
pub mod msdfer;
pub mod num;
pub mod rasterizer;
pub mod sdfer;
pub mod selection;
pub mod shaper;
pub mod slice;
pub mod slug_atlas;
pub mod substr;

// Debug test commented out - requires png encoder
// #[cfg(test)]
// mod tests {
//     #[test]
//     fn test() { ... }
// }
