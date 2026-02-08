pub mod animate;
pub mod color;
pub mod document;
pub mod gradient;
pub mod parse;
pub mod path_data;
pub mod render;
pub mod style;
pub mod transform;
pub mod units;

pub use document::*;
pub use parse::parse_svg;
pub use render::render_svg;
