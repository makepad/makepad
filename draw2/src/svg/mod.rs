pub mod render;

pub use makepad_svg::animate;
pub use makepad_svg::document::*;
pub use makepad_svg::parse::parse_svg;
pub use makepad_svg::path::{LineCap, LineJoin};
pub use makepad_svg::path_data::parse_path_data;
pub use makepad_svg::transform::parse_transform;
pub use makepad_svg::units::viewbox_transform;
pub use render::render_svg;
