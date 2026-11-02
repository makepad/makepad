//! Edge uses (`SmEdgeuse`) and face uses (`SmFaceuse`): the oriented uses of edges in
//! loops and wires and of faces in shells. The records ([`crate::topology::Edgeuse`],
//! [`crate::topology::Faceuse`]) live in the B-rep's arenas; the methods are split over
//! the plan units `topology.edgeuse_p1`, `_p2` and `_p3`.
//!
//! Upstream: `source/SMLib/src/SmEdgeuse.cpp`, `source/SMLib/inc/SmEdgeuse.h`,
//! `source/SMLib/src/SmFaceuse.cpp`, `source/SMLib/inc/SmFaceuse.h`.

pub mod p1;
pub mod p2;
pub mod p3;
