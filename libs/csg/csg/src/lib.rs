// makepad-csg: Top-level CSG library
//
// Provides an OpenSCAD-like API for constructive solid geometry.
// Build solids from primitives, combine with boolean operations,
// transform, and export to STL/OBJ for 3D printing.

pub mod solid;
pub mod document;

// Re-export the main types
pub use solid::{
    difference_all, difference_all_with, intersection_all, intersection_all_with, union_all,
    union_all_with, Solid,
};
pub use makepad_csg_boolean::boolean::FinishParams;
pub use document::{
    evaluate_program, mesh_document, render_thumbnail, CsgBudgets, CsgDocument, CsgError,
    CsgAnimKind, CsgAnimation, CsgAxis, MeshedModel, MeshedPart, PartPreview, Thumbnail,
};

// Re-export sub-crate types that users commonly need
pub use makepad_csg_math::{dvec3, BBox3d, Mat4d, Vec3d};
pub use makepad_csg_math::thread_pool::{self as pool, CancelToken};
pub use makepad_csg_mesh::mesh::TriMesh;
pub use makepad_csg_mesh::validate::MeshReport;

// Re-export SDF types
pub use makepad_csg_sdf::{
    sdf_to_mesh,
    Sdf3,
    SdfBlobChain,
    SdfBox,
    SdfCappedCone,
    SdfCapsule,
    SdfCylinder,
    SdfDifference,
    SdfEllipsoid,
    SdfHexPrism,
    SdfIntersection,
    SdfOctahedron,
    SdfOnion,
    SdfPlane,
    SdfRound,
    SdfRoundedBox,
    SdfRoundedCone,
    SdfRoundedCylinder,
    SdfScale,
    SdfSmoothDifference,
    SdfSmoothIntersection,
    SdfSmoothUnion,
    // Primitives
    SdfSphere,
    SdfTorus,
    // Transforms & modifiers
    SdfTranslate,
    SdfTriPrism,
    // Combinators
    SdfUnion,
    // Warping
    SdfWarp,
};
