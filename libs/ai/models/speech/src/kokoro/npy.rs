//! Minimal `.npy` reader, used only to diff against the ONNX reference dumps.
//!
//! Supports what `ref_dump.py` writes: little-endian f32, C order.

use crate::TtsError;

pub struct Npy {
    pub shape: Vec<usize>,
    pub data: Vec<f32>,
}

fn bad(message: impl Into<String>) -> TtsError {
    TtsError::Backend(message.into())
}

impl Npy {
    pub fn load(path: &str) -> Result<Self, TtsError> {
        let bytes = std::fs::read(path).map_err(|err| bad(format!("{path}: {err}")))?;
        if bytes.len() < 10 || &bytes[..6] != b"\x93NUMPY" {
            return Err(bad(format!("{path}: not a .npy")));
        }
        let major = bytes[6];
        let (header_len, header_at) = match major {
            1 => (u16::from_le_bytes(bytes[8..10].try_into().unwrap()) as usize, 10),
            2 => (u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize, 12),
            other => return Err(bad(format!("{path}: npy version {other}"))),
        };
        let header = std::str::from_utf8(&bytes[header_at..header_at + header_len])
            .map_err(|_| bad("npy header is not utf8"))?;

        if !header.contains("'<f4'") {
            return Err(bad(format!("{path}: only little-endian f32 is supported")));
        }
        if header.contains("'fortran_order': True") {
            return Err(bad(format!("{path}: fortran order is not supported")));
        }

        let shape = parse_shape(header).ok_or_else(|| bad(format!("{path}: bad shape")))?;
        let expected: usize = shape.iter().product();

        let data_at = header_at + header_len;
        let floats = &bytes[data_at..];
        if floats.len() < expected * 4 {
            return Err(bad(format!("{path}: truncated data")));
        }
        let data = floats[..expected * 4]
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect();

        Ok(Self { shape, data })
    }
}

fn parse_shape(header: &str) -> Option<Vec<usize>> {
    let start = header.find("'shape':")? + "'shape':".len();
    let open = header[start..].find('(')? + start + 1;
    let close = header[open..].find(')')? + open;
    Some(
        header[open..close]
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse().ok())
            .collect(),
    )
}

/// Largest absolute difference, and where it is.
pub fn max_abs_diff(a: &[f32], b: &[f32]) -> (f32, usize) {
    a.iter()
        .zip(b)
        .enumerate()
        .map(|(index, (x, y))| ((x - y).abs(), index))
        .fold((0.0, 0), |worst, current| {
            if current.0 > worst.0 {
                current
            } else {
                worst
            }
        })
}
