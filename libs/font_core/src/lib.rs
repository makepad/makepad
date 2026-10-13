//! Font tables, glyph outlines and glyph rasterisation: the one copy shared by the text layout
//! (`makepad-draw`, which shapes and lays out text in the app) and the HAL's font service (which
//! owns the glyph atlas). Shaping needs the font tables app-side, rasterising needs the outlines
//! HAL-side; both read them through [`font::Font`].

#[cfg(target_os = "macos")]
pub mod coretext;
pub mod font;
pub mod font_atlas;
pub mod font_face;
pub mod geom;
pub mod glyph_outline;
pub mod glyph_raster_image;
pub mod image;
pub mod intern;
pub mod msdfer;
pub mod num;
pub mod raster;
pub mod sdfer;
pub mod slug;

pub use makepad_shared_bytes::{MappedBytes, SharedBytes};
pub use rustybuzz;
