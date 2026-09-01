use super::id::{PartId, StaffId};
use super::pitch::Step;
use super::time::{Alter, Duration, Rational, RationalError, ScoreTime};
use makepad_micro_serde::{DeBin, DeBinErr, SerBin};

/// Scope for an exact-time map change.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, SerBin, DeBin)]
pub enum MapScope {
    Global,
    Part(PartId),
    Staff(StaffId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Change<T> {
    pub at: ScoreTime,
    pub scope: MapScope,
    pub value: T,
}

impl<T: SerBin> SerBin for Change<T> {
    fn ser_bin(&self, output: &mut Vec<u8>) {
        self.at.ser_bin(output);
        self.scope.ser_bin(output);
        self.value.ser_bin(output);
    }
}

impl<T: DeBin> DeBin for Change<T> {
    fn de_bin(offset: &mut usize, input: &[u8]) -> Result<Self, DeBinErr> {
        Ok(Self {
            at: ScoreTime::de_bin(offset, input)?,
            scope: MapScope::de_bin(offset, input)?,
            value: T::de_bin(offset, input)?,
        })
    }
}

/// Tempo in quarter notes per minute. Ramps have explicit exact score bounds.
#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub enum Tempo {
    Instant {
        quarters_per_minute: Rational,
    },
    Ramp {
        from_quarters_per_minute: Rational,
        to_quarters_per_minute: Rational,
        end: ScoreTime,
    },
}

/// A measured or deliberately unmetered span.
#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub enum Meter {
    Measured { groups: Vec<u16>, unit: u16 },
    Free,
}

impl Meter {
    pub fn duration(&self) -> Result<Option<Duration>, RationalError> {
        match self {
            Self::Free => Ok(None),
            Self::Measured { groups, unit } => {
                if *unit == 0 {
                    return Err(RationalError::ZeroDenominator);
                }
                let beats = groups.iter().try_fold(0_u64, |sum, group| {
                    sum.checked_add(u64::from(*group))
                        .ok_or(RationalError::Overflow)
                })?;
                let beats = i64::try_from(beats).map_err(|_| RationalError::Overflow)?;
                Ok(Some(Duration::new(beats, u64::from(*unit))?))
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct KeySignature {
    pub fifths: i8,
    pub custom: Vec<(Step, Alter)>,
}

impl KeySignature {
    pub const C_MAJOR: Self = Self {
        fifths: 0,
        custom: Vec::new(),
    };
}

/// Flat, sorted change maps consumed directly by playback and projections.
#[derive(Clone, Debug, Default, Eq, PartialEq, SerBin, DeBin)]
pub struct GlobalMaps {
    pub tempo: Vec<Change<Tempo>>,
    pub time_signature: Vec<Change<Meter>>,
    pub key: Vec<Change<KeySignature>>,
    /// The sustain pedal, as performed. Empty for an engraved score, which
    /// says "Ped." over a span rather than giving the damper a position; a
    /// score imported from a performance carries every pedal move here.
    pub pedal: Vec<Change<PedalLevel>>,
}

/// Damper position, in the MIDI controller's own units: 0 is fully damped and
/// 127 fully lifted, with everything between a half-pedal.
#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct PedalLevel {
    pub value: u8,
}

impl GlobalMaps {
    pub fn sort(&mut self) {
        self.tempo
            .sort_by_key(|change| (change.at, change.scope));
        self.time_signature
            .sort_by_key(|change| (change.at, change.scope));
        self.key.sort_by_key(|change| (change.at, change.scope));
        self.pedal.sort_by_key(|change| (change.at, change.scope));
    }

    pub fn meter_at(
        &self,
        at: ScoreTime,
        part: Option<PartId>,
        staff: Option<StaffId>,
    ) -> Option<&Meter> {
        best_change(&self.time_signature, at, part, staff).map(|change| &change.value)
    }

    pub fn key_at(
        &self,
        at: ScoreTime,
        part: Option<PartId>,
        staff: Option<StaffId>,
    ) -> Option<&KeySignature> {
        best_change(&self.key, at, part, staff).map(|change| &change.value)
    }
}

fn best_change<T>(
    changes: &[Change<T>],
    at: ScoreTime,
    part: Option<PartId>,
    staff: Option<StaffId>,
) -> Option<&Change<T>> {
    changes
        .iter()
        .filter(|change| change.at <= at && scope_matches(change.scope, part, staff))
        .max_by_key(|change| (change.at, scope_specificity(change.scope)))
}

fn scope_matches(scope: MapScope, part: Option<PartId>, staff: Option<StaffId>) -> bool {
    match scope {
        MapScope::Global => true,
        MapScope::Part(value) => part == Some(value),
        MapScope::Staff(value) => staff == Some(value),
    }
}

const fn scope_specificity(scope: MapScope) -> u8 {
    match scope {
        MapScope::Global => 0,
        MapScope::Part(_) => 1,
        MapScope::Staff(_) => 2,
    }
}
