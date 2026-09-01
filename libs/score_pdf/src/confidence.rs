//! Per-attribute confidence and inspectable evidence.

#[derive(Clone, Debug, PartialEq)]
pub enum Evidence {
    SmuflName(String),
    SmuflCodepoint(char),
    VendorAlias { vendor: String, source: String },
    StructuralName(String),
    GeometryMatch { canonical: String, distance: f32 },
    StaffResidual(f32),
    ClefInForce { primitive: u64 },
    KeySignature(i8),
    MeasureAccidental { primitive: u64 },
    BeamLevels(u8),
    FlagLevels(u8),
    DotCount(u8),
    StemDirection(i8),
    SharedStem,
    SamePitchEndpoints,
    DifferentPitchEndpoints,
    AttachmentDistance(f32),
    MeterConflict,
    NoEvidence(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Verification {
    Certain,
    Inferred,
    UserVerified,
    Ambiguous,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Estimate<T> {
    pub value: T,
    pub probability: f32,
    pub runner_up_margin: f32,
    pub evidence: Vec<Evidence>,
    pub verification: Verification,
}

impl<T> Estimate<T> {
    pub fn new(
        value: T,
        probability: f32,
        runner_up_margin: f32,
        evidence: Vec<Evidence>,
        verification: Verification,
    ) -> Self {
        Self {
            value,
            probability: probability.clamp(0.0, 1.0),
            runner_up_margin: runner_up_margin.clamp(0.0, 1.0),
            evidence,
            verification,
        }
    }

    pub fn certain(value: T, evidence: Evidence) -> Self {
        Self::new(value, 1.0, 1.0, vec![evidence], Verification::Certain)
    }

    pub fn inferred(value: T, probability: f32, evidence: Vec<Evidence>) -> Self {
        Self::new(
            value,
            probability,
            (probability - 0.5).max(0.0),
            evidence,
            Verification::Inferred,
        )
    }
}

