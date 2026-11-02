//! The healer's data (`SmHealData`): the target topology, the per-object property
//! records (`SmVertexProps`, `SmEdgeProps`, the face records of
//! [`crate::heal::face_props`]), the problem lists (`SmProbArray` and its derived
//! classes) and the cache and fix steps of the heal sequence.
//!
//! Upstream: `source/SMLib/src/SmHealData.cpp`, `source/SMLib/inc/SmHealData.h`,
//! `source/SMLib/inc/SmHealProbArray.h`, `source/SMLib/src/SmEdgeProps.cpp`,
//! `source/SMLib/src/SmVertexProps.cpp`. The functions are split over the units
//! `heal.heal_data_p1` .. `heal.heal_data_p3`; [`p2`] holds the types they share.

pub mod p1;
pub mod p2;
pub mod p3;
