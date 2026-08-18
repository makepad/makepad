//! Reader for the flat `.mktts` weight file produced by `tools/convert_kokoro.py`.

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;

use crate::TtsError;

const MAGIC: &[u8] = b"MKTTS\0\0\0";
const VERSION: u32 = 1;

pub struct TensorInfo {
    pub shape: Vec<usize>,
    byte_offset: usize,
    byte_len: usize,
}

impl TensorInfo {
    pub fn numel(&self) -> usize {
        self.shape.iter().product()
    }
}

pub struct Weights {
    /// The whole file. Held as `f32` so the buffer is 4-byte aligned and tensor
    /// slices can be taken without copying — the converter pads every tensor to
    /// a 32-byte boundary.
    data: Vec<f32>,
    tensors: HashMap<String, TensorInfo>,
}

fn bad(message: impl Into<String>) -> TtsError {
    TtsError::Backend(message.into())
}

impl Weights {
    pub fn load(path: &str) -> Result<Self, TtsError> {
        let mut file = File::open(path).map_err(|err| bad(format!("{path}: {err}")))?;
        let byte_len = file
            .metadata()
            .map_err(|err| bad(err.to_string()))?
            .len() as usize;

        let mut data = vec![0f32; byte_len.div_ceil(4)];
        // Safety: `data` owns at least `byte_len` bytes and any bit pattern is a
        // valid `f32`.
        let bytes = unsafe {
            std::slice::from_raw_parts_mut(data.as_mut_ptr() as *mut u8, byte_len)
        };
        file.read_exact(bytes).map_err(|err| bad(err.to_string()))?;

        let tensors = parse_index(bytes)?;
        Ok(Self { data, tensors })
    }

    pub fn len(&self) -> usize {
        self.tensors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tensors.is_empty()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.tensors.keys().map(String::as_str)
    }

    pub fn info(&self, name: &str) -> Option<&TensorInfo> {
        self.tensors.get(name)
    }

    /// Zero-copy view of a tensor.
    pub fn get(&self, name: &str) -> Option<&[f32]> {
        let info = self.tensors.get(name)?;
        let start = info.byte_offset / 4;
        let end = start + info.byte_len / 4;
        self.data.get(start..end)
    }

    pub fn shape(&self, name: &str) -> Option<&[usize]> {
        self.tensors.get(name).map(|info| info.shape.as_slice())
    }

    /// Rebuild a PyTorch weight-normed kernel: `W = g * v / ‖v‖`, with the norm
    /// taken over everything but the output channel.
    ///
    /// Kokoro's decoder stores `weight_g` and `weight_v` rather than `weight`.
    /// Doing this in Rust rather than the converter keeps the Python side pure
    /// standard library — a norm over 40M floats in interpreted Python is a
    /// minute of nothing.
    pub fn weight_norm(&self, prefix: &str) -> Result<Vec<f32>, TtsError> {
        let g_name = format!("{prefix}.weight_g");
        let v_name = format!("{prefix}.weight_v");

        let g = self
            .get(&g_name)
            .ok_or_else(|| bad(format!("missing {g_name}")))?;
        let v = self
            .get(&v_name)
            .ok_or_else(|| bad(format!("missing {v_name}")))?;
        let shape = self.shape(&v_name).unwrap();

        let out_channels = *shape.first().ok_or_else(|| bad("weight_v is scalar"))?;
        if g.len() != out_channels {
            return Err(bad(format!(
                "{prefix}: weight_g has {} entries for {out_channels} channels",
                g.len()
            )));
        }
        let per_channel = v.len() / out_channels;

        let mut weight = Vec::with_capacity(v.len());
        for channel in 0..out_channels {
            let row = &v[channel * per_channel..(channel + 1) * per_channel];
            let norm = row.iter().map(|x| x * x).sum::<f32>().sqrt();
            // A zero row would divide by zero; upstream never has one, but a
            // silent NaN here would be very hard to trace later.
            let scale = if norm > 0.0 { g[channel] / norm } else { 0.0 };
            weight.extend(row.iter().map(|x| x * scale));
        }
        Ok(weight)
    }
}

fn parse_index(bytes: &[u8]) -> Result<HashMap<String, TensorInfo>, TtsError> {
    if bytes.len() < 16 || &bytes[..8] != MAGIC {
        return Err(bad("not a .mktts file"));
    }
    let version = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    if version != VERSION {
        return Err(bad(format!("unsupported .mktts version {version}")));
    }
    let count = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;

    let mut at = 16;
    let read_u32 = |at: &mut usize| -> u32 {
        let value = u32::from_le_bytes(bytes[*at..*at + 4].try_into().unwrap());
        *at += 4;
        value
    };

    let mut tensors = HashMap::with_capacity(count);
    for _ in 0..count {
        let name_len = read_u32(&mut at) as usize;
        let name = std::str::from_utf8(&bytes[at..at + name_len])
            .map_err(|_| bad("tensor name is not utf8"))?
            .to_string();
        at += name_len;

        let dtype = bytes[at];
        let ndim = bytes[at + 1] as usize;
        at += 2;
        if dtype != 0 {
            return Err(bad(format!("{name}: only f32 is supported")));
        }

        let shape: Vec<usize> = (0..ndim).map(|_| read_u32(&mut at) as usize).collect();
        let byte_offset = u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap()) as usize;
        let byte_len = u64::from_le_bytes(bytes[at + 8..at + 16].try_into().unwrap()) as usize;
        at += 16;

        if byte_offset % 4 != 0 || byte_offset + byte_len > bytes.len() {
            return Err(bad(format!("{name}: bad tensor extent")));
        }
        let expected: usize = shape.iter().product::<usize>() * 4;
        if expected != byte_len {
            return Err(bad(format!(
                "{name}: shape {shape:?} implies {expected} bytes, index says {byte_len}"
            )));
        }

        tensors.insert(
            name,
            TensorInfo {
                shape,
                byte_offset,
                byte_len,
            },
        );
    }
    Ok(tensors)
}
