//! makepad-csg-cad: parametric CAD operations on triangle meshes for
//! printable parts.
//!
//! - [`sketch`]: closed shapes and paths (lines, arcs, splines, corner
//!   fillets and chamfers) into planar [`region::Region`]s with holes.
//! - [`solids`]: extrude (with draft), revolve, sweep and loft.
//! - [`mech`]: involute spur gears, ISO metric threads, hole tools.
//! - [`text`]: text from a font's glyph outlines.
//! - [`edges`]: a mesh's sharp edges, and fillets / chamfers on them.
//! - [`constrain`]: the 2D sketch constraint solver (dimensions and
//!   geometric constraints).
//!
//! All results are closed, outward-facing meshes in millimetres; sketches
//! lie in the XY plane and extrude along +Z.

pub mod bool3;
pub mod constrain;
pub mod edges;
pub mod measure;
pub mod mech;
pub mod region;
pub mod sketch;
pub mod solids;
pub mod text;

pub use bool3::{boolean_in, difference, intersection, union};
pub use makepad_csg_boolean::{BoolOp, MeshAccel};
pub use edges::{apply_edges, edge_tool, feature_edges, EdgeChain, EdgeOp, EdgeShape};
pub use mech::{hole_tool, iso_coarse_pitch, Gear, Hole, Thread};
pub use region::{regions_from_loops, Loop, Region, P2};
pub use sketch::{Seg, Shape, Sketch, Tolerance};
pub use solids::{extrude, helix, loft, revolve, signed_volume, sweep, Section};
pub use text::{text_regions, TextStyle};
