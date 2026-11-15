//! What the server checks in a guest's JPEG before it keeps it: that it is
//! one (start of image, a frame header, its size), and that it carries no
//! metadata. The phone page re-encodes every picture through a canvas, which
//! writes none; [`strip_metadata`] removes it anyway, for a client that did
//! not, so a location or a device name never reaches the gallery.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JpegInfo {
    pub width: u16,
    pub height: u16,
    /// An APP1..APP15 or COM segment was there (EXIF, XMP, ICC, comments).
    pub had_metadata: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JpegError {
    NotJpeg,
    Truncated,
    NoFrame,
}

impl std::fmt::Display for JpegError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            JpegError::NotJpeg => "not a JPEG",
            JpegError::Truncated => "a truncated JPEG",
            JpegError::NoFrame => "a JPEG without a frame header",
        })
    }
}

/// One marker segment before the scan: (marker, whole segment range).
fn segments(jpeg: &[u8]) -> Result<(Vec<(u8, std::ops::Range<usize>)>, usize), JpegError> {
    if jpeg.len() < 4 || jpeg[0] != 0xFF || jpeg[1] != 0xD8 {
        return Err(JpegError::NotJpeg);
    }
    let mut out = Vec::new();
    let mut at = 2;
    loop {
        // Fill bytes (0xFF runs) may precede a marker.
        while jpeg.get(at) == Some(&0xFF) && jpeg.get(at + 1) == Some(&0xFF) {
            at += 1;
        }
        let (Some(&0xFF), Some(&marker)) = (jpeg.get(at), jpeg.get(at + 1)) else {
            return Err(JpegError::Truncated);
        };
        if marker == 0xD9 {
            return Err(JpegError::NoFrame);
        }
        // Standalone markers carry no length.
        if marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            at += 2;
            continue;
        }
        let len = match jpeg.get(at + 2..at + 4) {
            Some(b) => u16::from_be_bytes([b[0], b[1]]) as usize,
            None => return Err(JpegError::Truncated),
        };
        if len < 2 || at + 2 + len > jpeg.len() {
            return Err(JpegError::Truncated);
        }
        out.push((marker, at..at + 2 + len));
        at += 2 + len;
        if marker == 0xDA {
            // Start of scan: entropy-coded data follows to the end.
            return Ok((out, at));
        }
    }
}

fn is_metadata(marker: u8) -> bool {
    (0xE1..=0xEF).contains(&marker) || marker == 0xFE
}

fn is_frame(marker: u8) -> bool {
    (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC)
}

pub fn info(jpeg: &[u8]) -> Result<JpegInfo, JpegError> {
    let (segments, _) = segments(jpeg)?;
    let had_metadata = segments.iter().any(|(m, _)| is_metadata(*m));
    let (_, frame) = segments.iter().find(|(m, _)| is_frame(*m)).ok_or(JpegError::NoFrame)?;
    // marker(2) length(2) precision(1) height(2) width(2)
    let f = &jpeg[frame.clone()];
    if f.len() < 9 {
        return Err(JpegError::Truncated);
    }
    Ok(JpegInfo {
        height: u16::from_be_bytes([f[5], f[6]]),
        width: u16::from_be_bytes([f[7], f[8]]),
        had_metadata,
    })
}

/// The same JPEG without its metadata segments (APP1..APP15, COM).
pub fn strip_metadata(jpeg: &[u8]) -> Result<Vec<u8>, JpegError> {
    let (segments, scan) = segments(jpeg)?;
    if !segments.iter().any(|(m, _)| is_frame(*m)) {
        return Err(JpegError::NoFrame);
    }
    let mut out = Vec::with_capacity(jpeg.len());
    out.extend_from_slice(&jpeg[..2]);
    for (marker, range) in &segments {
        if !is_metadata(*marker) {
            out.extend_from_slice(&jpeg[range.clone()]);
        }
    }
    out.extend_from_slice(&jpeg[scan..]);
    Ok(out)
}

/// A recorded loop as the phone page sends it: each frame a 4-byte
/// big-endian length, then that many bytes of JPEG.
pub fn split_frames(body: &[u8]) -> Result<Vec<&[u8]>, &'static str> {
    let mut frames = Vec::new();
    let mut rest = body;
    while !rest.is_empty() {
        let Some(len) = rest.get(..4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize) else {
            return Err("a frame length is cut short");
        };
        let Some(frame) = rest.get(4..4 + len) else {
            return Err("a frame is cut short");
        };
        frames.push(frame);
        rest = &rest[4 + len..];
    }
    Ok(frames)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A JPEG's structure (no real image data): SOI, APP0, APP1 Exif, COM,
    /// SOF0 of `w` x `h`, SOS, two scan bytes, EOI.
    pub fn sample(w: u16, h: u16) -> Vec<u8> {
        let mut j = vec![0xFF, 0xD8];
        j.extend([0xFF, 0xE0, 0x00, 0x07, b'J', b'F', b'I', b'F', 0x00]);
        j.extend([0xFF, 0xE1, 0x00, 0x0A, b'E', b'x', b'i', b'f', 0x00, 0x00, b'G', b'P']);
        j.extend([0xFF, 0xFE, 0x00, 0x04, b'h', b'i']);
        let [wh, wl] = w.to_be_bytes();
        let [hh, hl] = h.to_be_bytes();
        j.extend([0xFF, 0xC0, 0x00, 0x0B, 0x08, hh, hl, wh, wl, 0x01, 0x01, 0x11, 0x00]);
        j.extend([0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x3F, 0x00]);
        j.extend([0x12, 0x34, 0xFF, 0xD9]);
        j
    }

    #[test]
    fn info_and_strip() {
        let j = sample(640, 480);
        assert_eq!(info(&j), Ok(JpegInfo { width: 640, height: 480, had_metadata: true }));
        let clean = strip_metadata(&j).unwrap();
        assert_eq!(info(&clean), Ok(JpegInfo { width: 640, height: 480, had_metadata: false }));
        assert!(!clean.windows(4).any(|w| w == b"Exif"), "the EXIF segment is gone");
        assert!(clean.windows(4).any(|w| w == b"JFIF"), "APP0 stays");
        assert!(clean.ends_with(&[0x12, 0x34, 0xFF, 0xD9]), "the scan is untouched");
        assert_eq!(strip_metadata(&clean).unwrap(), clean, "stripping twice changes nothing");
    }

    pub fn pack(frames: &[Vec<u8>]) -> Vec<u8> {
        let mut out = Vec::new();
        for f in frames {
            out.extend((f.len() as u32).to_be_bytes());
            out.extend(f);
        }
        out
    }

    #[test]
    fn frames_split_back_and_refuse_a_cut() {
        let frames = vec![sample(4, 4), sample(8, 8), Vec::new()];
        let body = pack(&frames);
        let split = split_frames(&body).unwrap();
        assert_eq!(split.len(), 3);
        assert_eq!(split[1], &frames[1][..]);
        assert!(split_frames(&body[..body.len() - 1]).is_err(), "a cut length");
        assert!(split_frames(&body[..10]).is_err(), "a cut frame");
    }

    #[test]
    fn refuses_what_is_not_a_whole_jpeg() {
        assert_eq!(info(b"\x89PNG\r\n\x1a\n"), Err(JpegError::NotJpeg));
        let j = sample(8, 8);
        assert_eq!(info(&j[..30]), Err(JpegError::Truncated));
        let no_frame = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00, 0xFF, 0xD9];
        assert_eq!(info(&no_frame), Err(JpegError::NoFrame));
    }
}
