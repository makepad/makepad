//! `makepad-geom3d`: CPU geometry (KERNELS.md P4), plain data with no `Cx`.
//!
//! - [`mesh`]: the indexed triangle [`mesh::Mesh`] every builder returns
//!   (packs to `geom.PbrVertex`), with the small vector helpers;
//! - [`primitives`]: box, sphere, torus, torus knot, cylinder, plane,
//!   circle, ring, capsule, polyhedra;
//! - [`shape`]: 2D shapes and SVG documents, triangulated caps, extrusion
//!   with bevels, lathe;
//! - [`curve`]: `Curve3` (lines, splines, arcs, helices, knots, `FnCurve`
//!   style functions), arc-length tables, frames, tubes;
//! - [`procedural`]: parametric surfaces, heightfields, subdivision and
//!   deformers (twist, bend, taper, noise);
//! - [`fx_mesh`]: the 12-float `geom.CubeVertex` stream effect engines and
//!   the text extruder write;
//! - [`math`]: transforms, quaternions, deterministic hashes and noise.

pub mod curve;
pub mod fx_mesh;
pub mod math;
pub mod mesh;
pub mod primitives;
pub mod procedural;
pub mod shape;

pub use curve::*;
pub use primitives::*;
pub use procedural::*;
pub use shape::*;
