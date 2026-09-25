//! 3D gizmos as pure math: a transform gizmo that moves, turns and scales a
//! model matrix, and an orientation gizmo (a view cube, or a ball of labelled
//! axes) that turns a camera.
//!
//! Nothing here knows about `Cx`, widgets or shaders. The caller hands over
//! the matrices it renders with and the pointer in the same pixels its
//! viewport is laid out in, and gets back three things:
//!
//! - which handle is under the pointer ([`Gizmo::hover`], [`ViewCube::hover`]),
//! - what a drag does to the matrix it manipulates ([`Gizmo::begin_drag`],
//!   [`Gizmo::drag`], [`Gizmo::end_drag`]; [`ViewCube::snap_view`],
//!   [`orbit_view`], [`interpolate_view`]),
//! - the picture, as screen-space [`GizmoPrim`]s a 2D renderer paints in
//!   order ([`Gizmo::geometry`], [`ViewCube::geometry`]).
//!
//! # Conventions
//!
//! These are `makepad-math`'s: a [`Mat4f`] is column-major with column
//! vectors, `Mat4f::mul(&a, &b)` is `a * b`, a point goes to clip space as
//! `proj * view * model * p`, and camera space looks down its negative z
//! with y up (`Mat4f::look_at`, `Mat4f::perspective`). Screen space is the
//! viewport's: pixels (or layout points), origin at the top left, y down.
//! Both perspective and orthographic projections are handled.
//!
//! Colours are straight (not premultiplied) RGBA. A primitive whose fill has
//! zero alpha has no fill; one with a zero stroke width has no stroke.
pub use makepad_math;
pub use makepad_math::{Mat4f, Vec2f, Vec3f, Vec4f};

mod camera;
mod prim;
mod transform;
mod view_cube;

pub use camera::*;
pub use prim::*;
pub use transform::*;
pub use view_cube::*;
