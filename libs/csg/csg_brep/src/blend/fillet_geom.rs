//! Fillet geometry: the fillet vertex / edge / edge use / vertex use records of the
//! pseudo B-rep, the per-surface fillet geometry and the helpers of
//! `SmFilletGeom.cpp`.
//!
//! Upstream: `source/SMLib/inc/SmFilletGeom.h`, `source/SMLib/src/SmFilletGeom.cpp`.
//! The file is split into parts (`plan/modules.toml`, units `blend.fillet_geom_p1` ..
//! `blend.fillet_geom_p6`); each part is a submodule.

pub mod p1;
pub mod p2;
pub mod p3;
pub mod p4;
pub mod p5;
pub mod p6;
