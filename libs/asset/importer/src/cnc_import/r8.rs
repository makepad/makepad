use std::fmt;

const IMAGE_HEADER_SIZE: usize = 29;
const INLINE_PALETTE_SIZE: usize = 8 + 256 * 2;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum R8Error {
    Truncated,
    InvalidKind(u8),
    InvalidBpp(u8),
    InvalidDimensions,
}

impl fmt::Display for R8Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => f.write_str("truncated R8 image bundle"),
            Self::InvalidKind(kind) => write!(f, "invalid R8 entry kind {kind}"),
            Self::InvalidBpp(bpp) => write!(f, "unsupported R8 bit depth {bpp}"),
            Self::InvalidDimensions => f.write_str("invalid R8 image dimensions"),
        }
    }
}

impl std::error::Error for R8Error {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct R8Image {
    pub w: u32,
    pub h: u32,
    pub x: u32,
    pub y: u32,
    pub frame_w: u8,
    pub frame_h: u8,
    pub pixels: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct R8 {
    entries: Vec<R8Image>,
}

impl R8 {
    /// Parses Westwood's serialized Dune 2000 8-bit image sequence.
    ///
    /// Image kinds 1 and 2 use a 29-byte little-endian header: `u8 kind`,
    /// six `u32`s (`width`, `height`, `x`, `y`, image handle, palette
    /// handle), then `u8 bpp`, frame height, frame width and alignment. The
    /// header is followed by `width * height` palette indices. Kind 0 is an
    /// empty slot serialized as only its one-byte discriminator.
    ///
    /// This layout is empirical. Each of the seven 842,400-byte `BLOX*.R8`
    /// fixtures walks as exactly 800 kind-1 32x32 images and lands at EOF:
    /// `800 * (29 + 1024)`. `DATA.R8` also lands exactly at EOF with 6,555
    /// entries (1,450 empty, 5,015 kind 1 and 90 kind 2). It additionally
    /// has 29 images followed by a 520-byte palette object: a small observed
    /// descriptor (`1`, `2`, `4`, `5` or `8`), a discarded handle, and 256
    /// RGB565 words. These sidecars are skipped because the requested API
    /// exposes the indexed image only.
    pub fn parse(bytes: &[u8]) -> Result<Self, R8Error> {
        let mut entries = Vec::new();
        let mut at = 0usize;
        while at < bytes.len() {
            let kind = *bytes.get(at).ok_or(R8Error::Truncated)?;
            if kind == 0 {
                entries.push(R8Image {
                    w: 0,
                    h: 0,
                    x: 0,
                    y: 0,
                    frame_w: 0,
                    frame_h: 0,
                    pixels: None,
                });
                // The shipped DATA.R8 uses a one-byte empty slot. Accept the
                // natural full-header spelling as well when it is
                // unambiguous, which is useful for producers of this format.
                let full_empty_header = at
                    .checked_add(1)
                    .zip(at.checked_add(IMAGE_HEADER_SIZE))
                    .and_then(|(start, end)| bytes.get(start..end))
                    .is_some_and(|header| {
                        header[..24].iter().all(|&byte| byte == 0)
                            && header[24] == 8
                            && header[25..].iter().all(|&byte| byte == 0)
                    });
                at = at
                    .checked_add(if full_empty_header {
                        IMAGE_HEADER_SIZE
                    } else {
                        1
                    })
                    .ok_or(R8Error::Truncated)?;
                continue;
            }
            if !matches!(kind, 1 | 2) {
                return Err(R8Error::InvalidKind(kind));
            }

            let header_end = at
                .checked_add(IMAGE_HEADER_SIZE)
                .ok_or(R8Error::Truncated)?;
            bytes.get(at..header_end).ok_or(R8Error::Truncated)?;
            let w = read_u32(bytes, at + 1)?;
            let h = read_u32(bytes, at + 5)?;
            let x = read_u32(bytes, at + 9)?;
            let y = read_u32(bytes, at + 13)?;
            let palette_handle = read_u32(bytes, at + 21)?;
            let bpp = bytes[at + 25];
            let frame_h = bytes[at + 26];
            let frame_w = bytes[at + 27];
            if bpp != 8 {
                return Err(R8Error::InvalidBpp(bpp));
            }
            if w == 0 || h == 0 {
                return Err(R8Error::InvalidDimensions);
            }
            let pixel_count = usize::try_from(w)
                .ok()
                .and_then(|w| usize::try_from(h).ok().and_then(|h| w.checked_mul(h)))
                .ok_or(R8Error::InvalidDimensions)?;
            let pixel_end = header_end
                .checked_add(pixel_count)
                .ok_or(R8Error::InvalidDimensions)?;
            let pixels = bytes
                .get(header_end..pixel_end)
                .ok_or(R8Error::Truncated)?
                .to_vec();
            entries.push(R8Image {
                w,
                h,
                x,
                y,
                frame_w,
                frame_h,
                pixels: Some(pixels),
            });
            at = pixel_end;

            if palette_handle != 0 && bytes.len() - at >= 4 {
                let descriptor = read_u32(bytes, at)?;
                if matches!(descriptor, 1 | 2 | 4 | 5 | 8) {
                    at = at
                        .checked_add(INLINE_PALETTE_SIZE)
                        .ok_or(R8Error::Truncated)?;
                    if at > bytes.len() {
                        return Err(R8Error::Truncated);
                    }
                }
            }
        }
        Ok(Self { entries })
    }

    pub fn entries(&self) -> &[R8Image] {
        &self.entries
    }
}

fn read_u32(bytes: &[u8], at: usize) -> Result<u32, R8Error> {
    let end = at.checked_add(4).ok_or(R8Error::Truncated)?;
    let value = bytes.get(at..end).ok_or(R8Error::Truncated)?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cnc_import_r8_empty_and_image_entries() {
        let mut bytes = vec![0, 1];
        for value in [2u32, 2, 7, 9, 0, 0] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&[8, 2, 2, 0]);
        bytes.extend_from_slice(&[1, 2, 3, 4]);

        let bundle = R8::parse(&bytes).unwrap();
        assert_eq!(bundle.entries().len(), 2);
        assert_eq!(bundle.entries()[0].pixels, None);
        assert_eq!(bundle.entries()[1].w, 2);
        assert_eq!(bundle.entries()[1].h, 2);
        assert_eq!(bundle.entries()[1].x, 7);
        assert_eq!(bundle.entries()[1].y, 9);
        assert_eq!(bundle.entries()[1].pixels.as_deref(), Some(&[1, 2, 3, 4][..]));
    }

    #[test]
    fn cnc_import_r8_accepts_full_empty_header() {
        let mut bytes = vec![0];
        bytes.extend_from_slice(&[0; 24]);
        bytes.extend_from_slice(&[8, 0, 0, 0]);
        assert_eq!(R8::parse(&bytes).unwrap().entries().len(), 1);
        assert_eq!(R8::parse(&bytes).unwrap().entries()[0].pixels, None);
    }

    #[test]
    fn cnc_import_r8_rejects_hostile_lengths() {
        let mut bytes = vec![1];
        for value in [u32::MAX, u32::MAX, 0, 0, 0, 0] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&[8, 0, 0, 0]);
        assert!(matches!(
            R8::parse(&bytes),
            Err(R8Error::Truncated | R8Error::InvalidDimensions)
        ));
        assert_eq!(R8::parse(&[3]), Err(R8Error::InvalidKind(3)));
    }
}
