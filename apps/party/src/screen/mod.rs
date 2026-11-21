//! The big screen's widgets.
//!
//! - `wall`: the loop wall, the join code and attract mode.
//! - `frames`: guests' JPEGs decoded on a worker into textures.
//! - `camera`: the box's own camera between live guests.

pub mod camera;
pub mod frames;
pub mod wall;
