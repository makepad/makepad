use crate::decode::{Decode, DecodeError, Decoder};

/// A size range.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Limits {
    pub min: u32,
    pub max: Option<u32>,
}

impl Limits {
    /// Returns `true` if this [`Limits`] is valid within the range `0..=limit`.
    ///
    /// A [`Limits`] is valid within the range `0..=limit` if its minimum is not greater than
    /// `limit` and its maximum, if it exists, is neither less than its minimum nor greater than
    /// `limit`.
    pub fn is_valid(self, limit: u32) -> bool {
        if self.min > limit {
            return false;
        }
        if self.max.map_or(false, |max| max < self.min || max > limit) {
            return false;
        }
        true
    }

    /// Returns `true` if this [`Limits`] is a sublimit of the given [`Limits`].
    ///
    /// A [`Limits`] is a sublimit of another [`Limits`] if its minimum is not less than the
    /// other's, and its maximum, if it exists, is not greater than the other's.
    pub fn is_sublimit_of(self, other: Self) -> bool {
        if self.min < other.min {
            return false;
        }
        if let Some(other_max) = other.max {
            let Some(self_max) = self.max else {
                return false;
            };
            if self_max > other_max {
                return false;
            }
        }
        true
    }
}

impl Decode for Limits {
    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        match decoder.read_byte()? {
            0x00 => Ok(Limits {
                min: decoder.decode()?,
                max: None,
            }),
            0x01 => Ok(Limits {
                min: decoder.decode()?,
                max: Some(decoder.decode()?),
            }),
            _ => Err(DecodeError::new("invalid limits")),
        }
    }
}
