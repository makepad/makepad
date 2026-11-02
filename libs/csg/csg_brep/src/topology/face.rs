//! Faces (`SmFace`): bounded regions of a surface. The record
//! ([`crate::topology::Face`]) lives in the B-rep's arenas; the methods are split over
//! the plan units `topology.face_p1` .. `_p4` and `query.face_p1` .. `_p3`.
//!
//! Upstream: `source/SMLib/src/SmFace.cpp`, `source/SMLib/inc/SmFace.h`.

pub mod assert_tests;
pub mod p1;
pub mod p2;
pub mod p3;
pub mod p4;
