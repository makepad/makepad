// Port of box3d/include/box3d/constants.h
// Constants that scale with the length unit are functions; the rest are consts.

use crate::core::get_length_units_per_meter;
use crate::math_functions::PI;

// Used to detect bad values. In float mode positions greater than about 16km have precision
// problems, so 100km is a safe limit.
pub fn huge() -> f32 {
    1.0e5 * get_length_units_per_meter()
}

/// Maximum parallel workers. Used for some fixed size arrays.
pub const MAX_WORKERS: usize = 32;

/// Maximum number of tasks queued per world step.
pub const MAX_TASKS: usize = 256;

// Maximum number of colors in the constraint graph. Constraints that cannot
// find a color are added to the overflow set which are solved single-threaded.
pub const GRAPH_COLOR_COUNT: usize = 24;

// Number of contact point buckets for counting the number of contact points per
// shape contact pair. This is just for reporting and doesn't affect simulation.
pub const CONTACT_MANIFOLD_COUNT_BUCKETS: usize = 8;

// A small length used as a collision and constraint tolerance. Usually it is
// chosen to be numerically significant, but visually insignificant. In meters.
pub fn linear_slop() -> f32 {
    0.005 * get_length_units_per_meter()
}

pub fn min_capsule_length() -> f32 {
    linear_slop()
}

/// The distance between shapes where they are considered overlapped. This is needed
/// because GJK may return small positive values for overlapped shapes in degenerate
/// configurations.
pub fn overlap_slop() -> f32 {
    0.1 * linear_slop()
}

/// The maximum rotation of a body per time step.
pub const MAX_ROTATION: f32 = 0.25 * PI;

pub fn speculative_distance() -> f32 {
    4.0 * linear_slop()
}

/// The rest offset is used for mesh contact to reduce ghost collisions and assist with CCD.
pub fn mesh_rest_offset() -> f32 {
    1.0 * linear_slop()
}

/// The default contact recycling distance.
pub fn contact_recycle_distance() -> f32 {
    10.0 * linear_slop()
}

/// The default contact recycling world angle threshold. For performance this value
/// is cos(angle/2)^2. This value corresponds to 10 degrees.
pub const CONTACT_RECYCLE_ANGULAR_DISTANCE: f32 = 0.99240388;

/// This is used to fatten AABBs in the dynamic tree. This allows proxies
/// to move by a small amount without triggering a tree adjustment. This is in meters.
pub fn max_aabb_margin() -> f32 {
    0.05 * get_length_units_per_meter()
}

/// Per-shape AABB margin is a fraction of the shape extent (capped by max_aabb_margin).
pub const AABB_MARGIN_FRACTION: f32 = 0.125;

/// The time that a body must be still before it will go to sleep. In seconds.
pub const TIME_TO_SLEEP: f32 = 0.5;

/// Maximum length of the body name (including null termination in C).
pub const BODY_NAME_LENGTH: usize = 18;

/// Maximum length of the shape name.
pub const SHAPE_NAME_LENGTH: usize = 18;

/// The maximum number of contact points between two touching shapes.
pub const MAX_MANIFOLD_POINTS: usize = 4;

/// The maximum number points to use for shape cast proxies (swept point cloud).
pub const MAX_SHAPE_CAST_POINTS: usize = 64;

/// These generous limits allow for easy hashing. See shape pair keys.
pub const SHAPE_POWER: u32 = 22;
pub const CHILD_POWER: u32 = 64 - 2 * SHAPE_POWER;
pub const MAX_SHAPES: i32 = 1 << SHAPE_POWER;
pub const MAX_CHILD_SHAPES: i32 = 1 << CHILD_POWER;
