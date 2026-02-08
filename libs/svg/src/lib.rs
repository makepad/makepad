// Vector path/paint types and tessellation
pub mod paint;
pub mod path;
pub mod tessellate;

// SVG parsing and document model
pub mod animate;
pub mod color;
pub mod document;
pub mod gradient;
pub mod parse;
pub mod path_data;
pub mod style;
pub mod transform;
pub mod units;

pub use document::*;
pub use paint::*;
pub use parse::parse_svg;
pub use path::*;
pub use tessellate::*;
