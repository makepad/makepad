//! Fillet corners: how the fillets of the edges meeting at a vertex are combined
//! (vertex blends, bevels, end caps).
//!
//! Upstream: `source/SMLib/inc/SmFilletCorner.h`, `source/SMLib/src/SmFilletCorner.cpp`.
//! The file is split into parts (`plan/modules.toml`, units `blend.fillet_corner_p1` ..
//! `blend.fillet_corner_p3`); each part is a submodule.

pub mod p1;
pub mod p2;
pub mod p3;
pub mod topology;
