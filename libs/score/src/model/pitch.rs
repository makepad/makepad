use super::time::{Alter, Rational, RationalError};
use makepad_micro_serde::{DeBin, DeBinErr, SerBin};

/// A notated diatonic pitch name.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, SerBin, DeBin)]
pub enum Step {
    C,
    D,
    E,
    F,
    G,
    A,
    B,
}

impl Step {
    pub const fn index(self) -> i16 {
        match self {
            Self::C => 0,
            Self::D => 1,
            Self::E => 2,
            Self::F => 3,
            Self::G => 4,
            Self::A => 5,
            Self::B => 6,
        }
    }

    const fn natural_semitones(self) -> i16 {
        match self {
            Self::C => 0,
            Self::D => 2,
            Self::E => 4,
            Self::F => 5,
            Self::G => 7,
            Self::A => 9,
            Self::B => 11,
        }
    }

    const fn from_index(index: i16) -> Self {
        match index.rem_euclid(7) {
            0 => Self::C,
            1 => Self::D,
            2 => Self::E,
            3 => Self::F,
            4 => Self::G,
            5 => Self::A,
            _ => Self::B,
        }
    }
}

/// Spelled written pitch. Enharmonic spelling is preserved.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, SerBin, DeBin)]
pub struct Pitch {
    pub step: Step,
    pub alter: Alter,
    pub octave: i8,
}

impl Pitch {
    pub const fn new(step: Step, alter: Alter, octave: i8) -> Self {
        Self {
            step,
            alter,
            octave,
        }
    }

    pub fn sounding(self, transposition: Transposition) -> Result<Self, RationalError> {
        let old_diatonic = i16::from(self.octave)
            .checked_mul(7)
            .and_then(|value| value.checked_add(self.step.index()))
            .ok_or(RationalError::Overflow)?;
        let shift = transposition
            .diatonic_steps
            .checked_add(i16::from(transposition.octave_shift) * 7)
            .ok_or(RationalError::Overflow)?;
        let new_diatonic = old_diatonic
            .checked_add(shift)
            .ok_or(RationalError::Overflow)?;
        let step = Step::from_index(new_diatonic);
        let octave = i8::try_from(new_diatonic.div_euclid(7))
            .map_err(|_| RationalError::Overflow)?;

        let old_natural = i64::from(self.octave) * 12 + i64::from(self.step.natural_semitones());
        let target_natural = i64::from(octave) * 12 + i64::from(step.natural_semitones());
        let base_delta = Rational::new(old_natural - target_natural, 1)?;
        let octave_delta = Rational::new(i64::from(transposition.octave_shift) * 12, 1)?;
        let alter = self
            .alter
            .0
            .checked_add(transposition.chromatic_semitones.0)?
            .checked_add(octave_delta)?
            .checked_add(base_delta)?;
        Ok(Self::new(step, Alter(alter), octave))
    }
}

/// The interval from written pitch to sounding pitch.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, SerBin, DeBin)]
pub struct Transposition {
    pub diatonic_steps: i16,
    pub chromatic_semitones: Alter,
    pub octave_shift: i8,
}

impl Transposition {
    pub const NONE: Self = Self {
        diatonic_steps: 0,
        chromatic_semitones: Alter::NATURAL,
        octave_shift: 0,
    };
}

/// A non-mutating display projection of stored written pitch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PitchProjection {
    pub written: Pitch,
    pub displayed: Pitch,
    pub concert_pitch: bool,
}

impl PitchProjection {
    pub fn new(
        written: Pitch,
        transposition: Transposition,
        concert_pitch: bool,
    ) -> Result<Self, RationalError> {
        let displayed = if concert_pitch {
            written.sounding(transposition)?
        } else {
            written
        };
        Ok(Self {
            written,
            displayed,
            concert_pitch,
        })
    }
}
