//! `makepad-kinetic-type`: kinetic type as engine primitives (KINETIC-TYPE
//! design): a text becomes glyph shapes and per-glyph records ([`shapes`],
//! [`records`]), a Splash animator runs per glyph as a compute kernel that
//! writes the glyph draw's own instance record in place ([`kernel`]), and
//! the glyphs draw instanced with a Splash look ([`draw`]). What a kit
//! looks like and how it moves is Splash ([`kit`]); nothing here knows a
//! preset.

pub mod curve;
pub mod draw;
pub mod host;
pub mod kernel;
pub mod kit;
pub mod records;
pub mod shapes;
pub mod view;
pub use host::KineticHost;
pub use view::{script_mod, FrameStats, KineticFrame, KineticView};
pub use records::Karaoke;
/// Where a kit's letters' font comes from ([`KineticView::set_font`]).
pub use makepad_text_mesh::letters::FontSource;
