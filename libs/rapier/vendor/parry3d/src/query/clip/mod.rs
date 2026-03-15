pub use self::clip_aabb_line::clip_aabb_line;
#[cfg(feature = "alloc")]
pub use self::clip_halfspace_polygon::clip_halfspace_polygon;
pub use self::clip_segment_segment::clip_segment_segment;
#[cfg(feature = "dim2")]
pub use self::clip_segment_segment::clip_segment_segment_with_normal;

mod clip_aabb_line;
#[cfg(feature = "alloc")]
mod clip_aabb_polygon;
#[cfg(feature = "alloc")]
mod clip_halfspace_polygon;
mod clip_segment_segment;
