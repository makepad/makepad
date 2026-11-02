//! Non-manifold topology sweeps and the geometry-creation base shared by both sweep kinds.
//!
//! Upstream: `source/SMLib/inc/SmSweepGeometryCreation.h`,
//! `source/SMLib/src/SmSweepGeometryCreation.cpp`,
//! `source/SMLib/inc/SmTopologySweep.h`, `source/SMLib/src/SmTopologySweep.cpp`,
//! `SmSweepCheckType` of
//! `source/SMLib/inc/SmTypes.h`.
//!
//! | Rust                                         | Upstream                                    |
//! |----------------------------------------------|---------------------------------------------|
//! | [`SweepGeometryCreation`]                    | `SmSweepGeometryCreation` (data members)    |
//! | [`SweepGeometryCreation::new`]               | `SmSweepGeometryCreation(dEps)`             |
//! | [`SweepGeometryCreation::set_make_analytics`] | `SetMakeAnalytics`                         |
//! | [`TopologySweep`] | `SmTopologySweep` |
//! | [`TopologySweep::set_same`] | `SetSame` |
//! | [`TopologySweep::set_higher`] | `SetHigher` |
//! | [`TopologySweep::select_topology_to_keep`] | `SmSweepStitchCallback::SelectTopologyToKeep` |
//! | [`SweepCheckType`]                           | `SmSweepCheckType`                          |
//!
//! Deviations: typed, B-rep-qualified arena identities replace pointers; ordered
//! maps replace pointer hash tables. Output parameters become return values except
//! where partial outputs must survive errors. The geometry creator is an explicit
//! argument; the unused closed-sweep constructor parameter is omitted. A whole-B-rep
//! sweep can nest a single explicit-target wire sweep (maximum call depth two).
//! Repetitions above 4096 and back-face traversals that make no marking progress
//! return an error; temporary wire B-reps are released even on error.
//! See `engine` for algorithm bodies and `access` for unavailable dependency calls.

pub mod access;
pub mod engine;
pub mod geometry;
pub mod store;

use std::collections::BTreeMap;

use crate::error::{Result, SolidError};
use crate::topology::{BrepId, EdgeId, FaceId, RegionId, VertexId};

const OP: &str = "sweep::topology_sweep";

use crate::geom::tol::ZONE_TOL_3D;

/// `SmSweepCheckType`: whether sweeping a curve gives a valid surface; the
/// discriminants are the upstream values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SweepCheckType {
    /// `SM_SC_OKAY`: sweeping the curve gives a valid swept surface.
    Okay = 0,
    /// `SM_SC_ROT_ON_CURVE`: rotating a curve about one of its interior points gives
    /// a cone apex or bow-tie surface.
    RotOnCurve = 1,
    /// `SM_SC_SWEEP_ALONG_LENGTH`: the curve is tangent to the sweep direction.
    SweepAlongLength = 2,
    /// `SM_SC_SELF_INTERSECT`: the swept surface intersects itself.
    SelfIntersect = 3,
    /// `SM_SC_UNKNOWN`: the check failed, validity unknown.
    Unknown = 4,
}

impl SweepCheckType {
    /// The upstream enum value.
    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32
    }
}

/// The data of `SmSweepGeometryCreation`, the base of the sweep geometry creators.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SweepGeometryCreation {
    /// `m_bMakeAnalytics`: copy created surfaces as analytic surfaces where possible.
    pub make_analytics: bool,
    /// `m_dEps`: the tolerance of the base class.
    pub eps: f64,
}

impl Default for SweepGeometryCreation {
    /// `SmSweepGeometryCreation()`: `dEps = SM_ZONE_TOL_3D`.
    fn default() -> Self {
        Self::new(ZONE_TOL_3D)
    }
}

impl SweepGeometryCreation {
    /// `SmSweepGeometryCreation(dEps)`: analytics on.
    #[must_use]
    pub const fn new(eps: f64) -> Self {
        Self {
            make_analytics: true,
            eps,
        }
    }

    /// `SetMakeAnalytics`.
    pub fn set_make_analytics(&mut self, make_analytics: bool) {
        self.make_analytics = make_analytics;
    }
}

/// The kind and arena id of a mapped topology object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SweepEntity {
    /// A vertex (dimension zero).
    Vertex(BrepId, VertexId),
    /// An edge (dimension one).
    Edge(BrepId, EdgeId),
    /// A face (dimension two).
    Face(BrepId, FaceId),
    /// A region (dimension three).
    Region(BrepId, RegionId),
}

/// One of the four upstream pointer maps, including possible null entries.
pub type SweepMap = BTreeMap<Option<SweepEntity>, Option<SweepEntity>>;

/// Sweep options and the four relationships between original and swept topology.
#[derive(Debug, Clone)]
pub struct TopologySweep {
    /// Stitch resulting topology (`bDoStitching`, default true).
    pub do_stitching: bool,
    /// Merge into the destination (`bDoMerge`, default false).
    pub do_merge: bool,
    /// Tag new faces and edges (`bDoTagging`, default false).
    pub do_tagging: bool,
    /// Primary tag id.
    pub tag_id: i64,
    /// Number of incremental sweeps. Zero is logged upstream but still assigned.
    pub repetitions: u64,
    /// Upstream stores this flag although self-intersection checks are unsupported.
    pub do_self_intersection: SelfIntersectionCheck,
    /// Original to copied topology.
    pub to_same: SweepMap,
    /// Original to topology of one higher dimension.
    pub to_higher: SweepMap,
    /// Copied topology to its original.
    pub from_same: SweepMap,
    /// Higher-dimensional topology to its original.
    pub from_lower: SweepMap,
}

/// The stored (unsupported upstream) self-intersection-check flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelfIntersectionCheck {
    /// Disabled (the upstream default).
    #[default]
    Disabled,
    /// Enabled; upstream asserts in debug builds, but stores it in release.
    Enabled,
}

/// Constructor options. Upstream's unused closed-sweep argument is omitted.
#[derive(Debug, Clone, Copy)]
pub struct TopologySweepOptions {
    /// Stitch the resulting topology.
    pub stitching: bool,
    /// Merge the resulting topology into the destination.
    pub merge: bool,
    /// Tag created faces and edges.
    pub tagging: bool,
    /// Primary tag id.
    pub tag_id: i64,
    /// Stored upstream flag.
    pub self_intersection: SelfIntersectionCheck,
}
impl Default for TopologySweepOptions {
    fn default() -> Self {
        Self {
            stitching: true,
            merge: false,
            tagging: false,
            tag_id: 0,
            self_intersection: SelfIntersectionCheck::Disabled,
        }
    }
}
impl Default for TopologySweep {
    fn default() -> Self {
        Self::new(TopologySweepOptions::default())
    }
}
impl TopologySweep {
    /// `SmTopologySweep`: construct empty maps with the requested options.
    #[must_use]
    pub fn new(options: TopologySweepOptions) -> Self {
        Self {
            do_stitching: options.stitching,
            do_merge: options.merge,
            do_tagging: options.tagging,
            tag_id: options.tag_id,
            do_self_intersection: options.self_intersection,
            repetitions: 1,
            to_same: BTreeMap::new(),
            to_higher: BTreeMap::new(),
            from_same: BTreeMap::new(),
            from_lower: BTreeMap::new(),
        }
    }

    /// `SetSame`: preserve an existing image; do not erase stale reverse entries.
    pub fn set_same(&mut self, from: Option<SweepEntity>, to: Option<SweepEntity>) {
        let typed_from = match to {
            Some(SweepEntity::Vertex(..)) => from.filter(|v| matches!(v, SweepEntity::Vertex(..))),
            Some(SweepEntity::Edge(..)) => from.filter(|v| matches!(v, SweepEntity::Edge(..))),
            _ => from.filter(|v| matches!(v, SweepEntity::Face(..))),
        };
        if self.get_to_same(typed_from).is_none() {
            self.to_same.insert(from, to);
            self.from_same.insert(to, from);
        }
    }

    /// `SetHigher`: only vertices preserve an existing image; edges overwrite it.
    pub fn set_higher(&mut self, from: Option<SweepEntity>, to: Option<SweepEntity>) {
        if !matches!(from, Some(SweepEntity::Vertex(..))) || self.get_to_higher(from).is_none() {
            self.to_higher.insert(from, to);
            self.from_lower.insert(to, from);
        }
    }

    /// `GetToSamePriv` (also the three typed `GetToSame` overloads).
    #[must_use]
    pub fn get_to_same(&self, from: Option<SweepEntity>) -> Option<SweepEntity> {
        self.to_same.get(&from).copied().flatten()
    }

    /// `GetFromSamePriv` / `GetFromSame`.
    #[must_use]
    pub fn get_from_same(&self, to: Option<SweepEntity>) -> Option<SweepEntity> {
        self.from_same.get(&to).copied().flatten()
    }

    /// `GetToHigherPriv` / `GetToHigher`.
    #[must_use]
    pub fn get_to_higher(&self, from: Option<SweepEntity>) -> Option<SweepEntity> {
        self.to_higher.get(&from).copied().flatten()
    }

    /// `GetFromLowerPriv` / `GetFromLower`.
    #[must_use]
    pub fn get_from_lower(&self, to: Option<SweepEntity>) -> Option<SweepEntity> {
        self.from_lower.get(&to).copied().flatten()
    }

    /// `RemoveFromToSamePriv`: erase only the forward entry.
    pub fn remove_from_to_same(&mut self, from: Option<SweepEntity>) {
        self.to_same.remove(&from);
    }

    /// `RemoveFromFromSamePriv`: erase only the reverse entry.
    pub fn remove_from_from_same(&mut self, to: Option<SweepEntity>) {
        self.from_same.remove(&to);
    }

    /// `SmSweepStitchCallback::SelectTopologyToKeep`.
    ///
    /// `maximum_distance` and `keep_first` are initialised before any error, as
    /// upstream's output references are. Distance tolerance is unused upstream.
    pub fn select_topology_to_keep(
        &mut self,
        first: Option<SweepEntity>,
        second: Option<SweepEntity>,
        _distance_tolerance: f64,
        maximum_distance: &mut f64,
        keep_first: &mut bool,
    ) -> Result<()> {
        *maximum_distance = 0.0;
        *keep_first = true;
        let to1 = self.get_to_same(first);
        let to2 = self.get_to_same(second);
        let from1 = self.get_from_same(first);
        let from2 = self.get_from_same(second);
        if to1.is_some() && to2.is_some() {
            return Err(SolidError::sm_err(OP));
        }
        if to2.is_some() && to1.is_none() {
            if to2 == first {
                self.remove_from_to_same(second);
                self.remove_from_from_same(first);
                self.set_same(second, second);
            }
            *keep_first = false;
        }
        if to1.is_some() && to2.is_none() {
            if to1 == second {
                self.remove_from_to_same(first);
                self.remove_from_from_same(second);
                self.set_same(first, first);
            }
            *keep_first = true;
        }
        if to1.is_some() || to2.is_some() {
            return Ok(());
        }
        if from1.is_some() && from2.is_some() {
            return Err(SolidError::sm_err(OP));
        }
        if from1.is_none() && from2.is_some() {
            *keep_first = false;
        }
        if from1.is_some() && from2.is_none() {
            *keep_first = true;
        }
        if from1.is_some() || from2.is_some() {
            return Ok(());
        }
        let lower1 = self.get_from_lower(first);
        let lower2 = self.get_from_lower(second);
        if lower1.is_some() && lower2.is_some() {
            return Err(SolidError::sm_err(OP));
        }
        if lower1.is_some() {
            *keep_first = true;
        } else if lower2.is_some() {
            *keep_first = false;
        } else {
            let higher1 = self.get_to_higher(first);
            let higher2 = self.get_to_higher(second);
            if higher1.is_some() && higher2.is_some() {
                return Err(SolidError::sm_err(OP));
            }
            if higher1.is_some() {
                *keep_first = true;
            } else if higher2.is_some() {
                *keep_first = false;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let g = SweepGeometryCreation::default();
        assert!(g.make_analytics);
        assert_eq!(g.eps, ZONE_TOL_3D);
        let mut h = SweepGeometryCreation::new(1e-3);
        h.set_make_analytics(false);
        assert!(!h.make_analytics);
        assert_eq!(SweepCheckType::Unknown.code(), 4);
    }
}
