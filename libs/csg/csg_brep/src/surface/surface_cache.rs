//! The surface cache `SmSurfaceCache`: the Bezier decomposition of a surface into a
//! binary tree of sub-domains with nested bounding boxes, plus the boundary bounding
//! planes of a patch and the alternate (silhouette curve) caches attached to it.
//!
//! Upstream: `source/SMLib/src/SmSurfaceCache.cpp`, `source/SMLib/inc/SmSurfaceCache.h`.
//! The file is split over several plan units (`surface.surface_cache_p1`, `_p2`,
//! `_p3`); each lives in its own submodule and adds methods to [`SurfaceCache`] (or
//! free functions).

pub mod face_context;
pub mod p1;
pub mod p2;
pub mod p3;

pub use p1::{
    AltSrfCache, AltSrfCacheType, BezierAux2d, BezierPatch, PatchBoundaryBoundingPlanes,
    SurfaceCache, SurfaceCacheOptions,
};
