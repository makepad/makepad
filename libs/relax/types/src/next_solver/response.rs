// ported from N:rustc_type_ir/src/solve/mod.rs @8d1a764
use super::{GenericArg, Goal};
use crate::infer::opaque_types::OpaqueTypeKey;
use crate::types::{TyId, RegionId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NoSolution;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GoalSource { Misc, ImplWhereBound, AliasBoundConstCondition, AliasWellFormed, Normalization }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RegionConstraint { RegionOutlives(RegionId, RegionId), TypeOutlives(TyId, RegionId), Eq(RegionId, RegionId) }

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ExternalConstraintsData {
    pub region_constraints: Vec<(RegionConstraint, bool)>,
    pub opaque_types: Vec<(OpaqueTypeKey, TyId)>,
    pub normalization_nested_goals: Vec<(GoalSource, Goal)>,
}

impl ExternalConstraintsData {
    pub fn is_empty(&self) -> bool {
        self.region_constraints.is_empty() && self.opaque_types.is_empty() && self.normalization_nested_goals.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Response {
    pub certainty: Certainty,
    pub var_values: Vec<GenericArg>,
    pub external_constraints: ExternalConstraintsData,
}
#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug)]
pub enum Certainty {
    Yes,
    Maybe(MaybeInfo),
}

#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug)]
pub struct MaybeInfo {
    pub cause: MaybeCause,
    pub opaque_types_jank: OpaqueTypesJank,
    pub stalled_on_coroutines: StalledOnCoroutines,
}

impl MaybeInfo {
    pub const AMBIGUOUS: MaybeInfo = MaybeInfo {
        cause: MaybeCause::Ambiguity,
        opaque_types_jank: OpaqueTypesJank::AllGood,
        stalled_on_coroutines: StalledOnCoroutines::No,
    };

    fn and(self, other: MaybeInfo) -> MaybeInfo {
        MaybeInfo {
            cause: self.cause.and(other.cause),
            opaque_types_jank: self.opaque_types_jank.and(other.opaque_types_jank),
            stalled_on_coroutines: self.stalled_on_coroutines.and(other.stalled_on_coroutines),
        }
    }

    pub fn or(self, other: MaybeInfo) -> MaybeInfo {
        MaybeInfo {
            cause: self.cause.or(other.cause),
            opaque_types_jank: self.opaque_types_jank.or(other.opaque_types_jank),
            stalled_on_coroutines: self.stalled_on_coroutines.or(other.stalled_on_coroutines),
        }
    }
}

#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug)]
pub enum OpaqueTypesJank {
    AllGood,
    ErrorIfRigidSelfTy,
}
impl OpaqueTypesJank {
    fn and(self, other: OpaqueTypesJank) -> OpaqueTypesJank {
        match (self, other) {
            (OpaqueTypesJank::AllGood, OpaqueTypesJank::AllGood) => OpaqueTypesJank::AllGood,
            (OpaqueTypesJank::ErrorIfRigidSelfTy, _) | (_, OpaqueTypesJank::ErrorIfRigidSelfTy) => {
                OpaqueTypesJank::ErrorIfRigidSelfTy
            }
        }
    }

    pub fn or(self, other: OpaqueTypesJank) -> OpaqueTypesJank {
        match (self, other) {
            (OpaqueTypesJank::ErrorIfRigidSelfTy, OpaqueTypesJank::ErrorIfRigidSelfTy) => {
                OpaqueTypesJank::ErrorIfRigidSelfTy
            }
            (OpaqueTypesJank::AllGood, _) | (_, OpaqueTypesJank::AllGood) => {
                OpaqueTypesJank::AllGood
            }
        }
    }
}

#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug)]
pub enum StalledOnCoroutines {
    Yes,
    No,
}

impl StalledOnCoroutines {
    fn and(self, other: StalledOnCoroutines) -> StalledOnCoroutines {
        match (self, other) {
            (StalledOnCoroutines::No, StalledOnCoroutines::No) => StalledOnCoroutines::No,
            (StalledOnCoroutines::Yes, _) | (_, StalledOnCoroutines::Yes) => {
                StalledOnCoroutines::Yes
            }
        }
    }

    pub fn or(self, other: StalledOnCoroutines) -> StalledOnCoroutines {
        StalledOnCoroutines::and(self, other)
    }
}

impl Certainty {
    pub const AMBIGUOUS: Certainty = Certainty::Maybe(MaybeInfo::AMBIGUOUS);

    pub fn and(self, other: Certainty) -> Certainty {
        match (self, other) {
            (Certainty::Yes, Certainty::Yes) => Certainty::Yes,
            (Certainty::Yes, Certainty::Maybe { .. }) => other,
            (Certainty::Maybe { .. }, Certainty::Yes) => self,
            (Certainty::Maybe(a_maybe), Certainty::Maybe(b_maybe)) => {
                Certainty::Maybe(a_maybe.and(b_maybe))
            }
        }
    }

    pub const fn overflow(suggest_increasing_limit: bool) -> Certainty {
        Certainty::Maybe(MaybeInfo {
            cause: MaybeCause::Overflow { suggest_increasing_limit, keep_constraints: false },
            opaque_types_jank: OpaqueTypesJank::AllGood,
            stalled_on_coroutines: StalledOnCoroutines::No,
        })
    }

    pub fn is_yes(&self) -> bool {
        match self {
            Certainty::Yes => true,
            Certainty::Maybe(_) => false,
        }
    }

    pub fn is_overflow(&self) -> bool {
        match self {
            Certainty::Maybe(MaybeInfo { cause: MaybeCause::Overflow { .. }, .. }) => true,
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug)]
pub enum MaybeCause {
    Ambiguity,
    Overflow { suggest_increasing_limit: bool, keep_constraints: bool },
}

impl MaybeCause {
    fn and(self, other: MaybeCause) -> MaybeCause {
        match (self, other) {
            (MaybeCause::Ambiguity, MaybeCause::Ambiguity) => MaybeCause::Ambiguity,
            (MaybeCause::Ambiguity, MaybeCause::Overflow { .. }) => other,
            (MaybeCause::Overflow { .. }, MaybeCause::Ambiguity) => self,
            (
                MaybeCause::Overflow {
                    suggest_increasing_limit: limit_a,
                    keep_constraints: keep_a,
                },
                MaybeCause::Overflow {
                    suggest_increasing_limit: limit_b,
                    keep_constraints: keep_b,
                },
            ) => MaybeCause::Overflow {
                suggest_increasing_limit: limit_a && limit_b,
                keep_constraints: keep_a && keep_b,
            },
        }
    }

    pub fn or(self, other: MaybeCause) -> MaybeCause {
        match (self, other) {
            (MaybeCause::Ambiguity, MaybeCause::Ambiguity) => MaybeCause::Ambiguity,

            (
                MaybeCause::Ambiguity,
                MaybeCause::Overflow { suggest_increasing_limit, keep_constraints: _ },
            ) => MaybeCause::Overflow { suggest_increasing_limit, keep_constraints: true },
            (
                MaybeCause::Overflow { suggest_increasing_limit, keep_constraints: _ },
                MaybeCause::Ambiguity,
            ) => MaybeCause::Overflow { suggest_increasing_limit, keep_constraints: true },

            (
                MaybeCause::Overflow {
                    suggest_increasing_limit: limit_a,
                    keep_constraints: keep_a,
                },
                MaybeCause::Overflow {
                    suggest_increasing_limit: limit_b,
                    keep_constraints: keep_b,
                },
            ) => MaybeCause::Overflow {
                suggest_increasing_limit: limit_a || limit_b,
                keep_constraints: keep_a || keep_b,
            },
        }
    }
}

#[derive(Clone, Debug)]
pub enum SucceededInErased { Yes { accessed_opaques: super::AccessedOpaques }, No }
#[derive(Clone, Debug)]
pub enum GoalStalledOnOpaques {
    No,
    Yes { num_opaques_in_storage: usize, previously_succeeded_in_erased: SucceededInErased },
}
#[derive(Clone, Debug)]
pub struct GoalStalledOn {
    pub stalled_vars: Vec<GenericArg>,
    pub sub_roots: Vec<u32>,
    pub stalled_maybe_info: MaybeInfo,
    pub opaques: GoalStalledOnOpaques,
}
