pub use makepad_svg::document;
pub use makepad_svg::paint::*;
pub use makepad_svg::parse;
pub use makepad_svg::path::*;
pub use makepad_svg::tessellate::*;

pub(crate) mod bands;
pub(crate) mod feather;
pub use feather::FeatherSide;
mod triangulate;
pub use triangulate::*;
