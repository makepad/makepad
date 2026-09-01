use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PalError {
    InvalidSize,
    ComponentOutOfRange,
}

impl fmt::Display for PalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSize => f.write_str("a PAL palette must contain exactly 768 bytes"),
            Self::ComponentOutOfRange => f.write_str("PAL component is greater than 63"),
        }
    }
}

impl std::error::Error for PalError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pal {
    colors: [[u8; 3]; 256],
}

impl Pal {
    pub fn parse(bytes: &[u8]) -> Result<Self, PalError> {
        if bytes.len() != 768 {
            return Err(PalError::InvalidSize);
        }
        let mut colors = [[0; 3]; 256];
        for (color, source) in colors.iter_mut().zip(bytes.chunks_exact(3)) {
            for (output, &value) in color.iter_mut().zip(source) {
                if value > 63 {
                    return Err(PalError::ComponentOutOfRange);
                }
                *output = (value as u16 * 255 / 63) as u8;
            }
        }
        Ok(Self { colors })
    }

    pub fn rgb(&self, index: u8) -> [u8; 3] {
        self.colors[index as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cnc_import_pal_scales_six_bit_components() {
        let mut bytes = [0u8; 768];
        bytes[..3].copy_from_slice(&[63, 32, 1]);
        assert_eq!(Pal::parse(&bytes).unwrap().rgb(0), [255, 129, 4]);
        bytes[3] = 64;
        assert_eq!(Pal::parse(&bytes), Err(PalError::ComponentOutOfRange));
    }
}
