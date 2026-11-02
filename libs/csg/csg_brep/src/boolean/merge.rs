//! The merge (boolean) engine `SmMerge`: intersecting and combining the topologies of
//! two B-reps.
//!
//! Upstream: `source/SMLib/src/SmMerge.cpp`, `source/SMLib/inc/SmMerge.h`.
//! The file is split over several plan units (`boolean.merge_p1`, `_p2`, ...); each
//! lives in its own submodule. Merge control, vertex copies, face coincidence and
//! cleanup helpers are in [`p1`]; the constructor, edge/face-use copies and the
//! merge of added topology are in [`p2`]; the list and tree Booleans are in [`p3`];
//! the manifold Boolean is in [`p4`]; the wire/shell-vertex stage is in [`p5`]. The
//! options record was first co-ported by `boolean.api_brep`. Topology mutations
//! awaiting other units return `Unported`.

pub mod p1;
pub mod p2;
pub mod p3;
pub mod p4;
pub mod p5;

pub use p1::MergeOptions;
