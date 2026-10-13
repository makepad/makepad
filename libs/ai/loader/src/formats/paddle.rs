//! Paddle combined parameter files (`*.pdiparams`, written by `save_combine`):
//! the tensors back to back, each as
//!
//! ```text
//! u32 version (0)
//! u64 lod levels, then per level: u64 byte size + that many bytes
//! u32 tensor version (0)
//! i32 descriptor size, then a TensorDesc protobuf message
//!     (field 1 varint: data type, field 2 varint, repeated or packed: dims)
//! raw little-endian data, product(dims) * element size bytes
//! ```
//!
//! The file carries no names: the program file lists the parameters in the
//! same order.

use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaddleType {
    Bool,
    I16,
    I32,
    I64,
    F16,
    F32,
    F64,
    U8,
    I8,
    Bf16,
}

impl PaddleType {
    fn from_code(code: u64) -> Option<PaddleType> {
        // framework.proto VarType.Type
        Some(match code {
            0 => PaddleType::Bool,
            1 => PaddleType::I16,
            2 => PaddleType::I32,
            3 => PaddleType::I64,
            4 => PaddleType::F16,
            5 => PaddleType::F32,
            6 => PaddleType::F64,
            20 => PaddleType::U8,
            21 => PaddleType::I8,
            22 => PaddleType::Bf16,
            _ => return None,
        })
    }

    pub fn size(self) -> usize {
        match self {
            PaddleType::Bool | PaddleType::U8 | PaddleType::I8 => 1,
            PaddleType::I16 | PaddleType::F16 | PaddleType::Bf16 => 2,
            PaddleType::I32 | PaddleType::F32 => 4,
            PaddleType::I64 | PaddleType::F64 => 8,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PaddleTensor {
    pub dtype: PaddleType,
    pub dims: Vec<usize>,
    pub data: Vec<u8>,
}

impl PaddleTensor {
    /// The values as f32 (from F32, F16 or BF16).
    pub fn to_f32(&self) -> Result<Vec<f32>, String> {
        Ok(match self.dtype {
            PaddleType::F32 => self.data.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect(),
            PaddleType::F16 => self.data.chunks_exact(2).map(|c| crate::quant::f16_to_f32(u16::from_le_bytes([c[0], c[1]]))).collect(),
            PaddleType::Bf16 => self.data.chunks_exact(2).map(|c| f32::from_bits((u16::from_le_bytes([c[0], c[1]]) as u32) << 16)).collect(),
            other => return Err(format!("{other:?} tensor is not floating point")),
        })
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).filter(|e| *e <= self.bytes.len()).ok_or_else(|| format!("truncated at byte {}", self.at))?;
        let out = &self.bytes[self.at..end];
        self.at = end;
        Ok(out)
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
}

fn varint(bytes: &[u8], at: &mut usize) -> Result<u64, String> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let b = *bytes.get(*at).ok_or("truncated varint")?;
        *at += 1;
        v |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            return Ok(v);
        }
    }
    Err("varint too long".into())
}

/// TensorDesc: data type (field 1) and dims (field 2, packed or not).
fn tensor_desc(msg: &[u8]) -> Result<(PaddleType, Vec<usize>), String> {
    let (mut dtype, mut dims) = (None, Vec::new());
    let mut at = 0;
    while at < msg.len() {
        let key = varint(msg, &mut at)?;
        let (field, wire) = (key >> 3, key & 7);
        match (field, wire) {
            (1, 0) => dtype = Some(PaddleType::from_code(varint(msg, &mut at)?).ok_or("unknown Paddle data type")?),
            (2, 0) => dims.push(varint(msg, &mut at)? as i64),
            (2, 2) => {
                let len = varint(msg, &mut at)? as usize;
                let end = at + len;
                while at < end {
                    dims.push(varint(msg, &mut at)? as i64);
                }
            }
            (_, 0) => {
                varint(msg, &mut at)?;
            }
            (_, 2) => {
                let len = varint(msg, &mut at)? as usize;
                at += len;
            }
            _ => return Err(format!("unexpected protobuf wire type {wire}")),
        }
    }
    let dims = dims.into_iter().map(|d| usize::try_from(d).map_err(|_| format!("negative dim {d}"))).collect::<Result<_, _>>()?;
    Ok((dtype.ok_or("tensor descriptor without a data type")?, dims))
}

/// All tensors of a combined parameter file, in file order.
pub fn read_combined(path: &Path) -> Result<Vec<PaddleTensor>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse_combined(&bytes)
}

pub fn parse_combined(bytes: &[u8]) -> Result<Vec<PaddleTensor>, String> {
    let mut r = Reader { bytes, at: 0 };
    let mut out = Vec::new();
    while r.at < bytes.len() {
        if r.u32()? != 0 {
            return Err(format!("unsupported LoD tensor version at byte {}", r.at - 4));
        }
        let levels = r.u64()?;
        for _ in 0..levels {
            let size = r.u64()? as usize;
            r.take(size)?;
        }
        if r.u32()? != 0 {
            return Err("unsupported tensor version".into());
        }
        let desc_len = r.u32()? as usize;
        let (dtype, dims) = tensor_desc(r.take(desc_len)?)?;
        let count: usize = dims.iter().product();
        let data = r.take(count * dtype.size())?.to_vec();
        out.push(PaddleTensor { dtype, dims, data });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tensor(dims: &[u64], values: &[f32]) -> Vec<u8> {
        let mut desc = vec![0x08, 5]; // data type F32
        desc.push(0x12); // dims, packed
        let mut packed = Vec::new();
        for d in dims {
            let mut v = *d;
            loop {
                let b = (v & 0x7f) as u8;
                v >>= 7;
                packed.push(if v == 0 { b } else { b | 0x80 });
                if v == 0 {
                    break;
                }
            }
        }
        desc.push(packed.len() as u8);
        desc.extend(packed);
        let mut out = Vec::new();
        out.extend(0u32.to_le_bytes());
        out.extend(0u64.to_le_bytes());
        out.extend(0u32.to_le_bytes());
        out.extend((desc.len() as u32).to_le_bytes());
        out.extend(desc);
        for v in values {
            out.extend(v.to_le_bytes());
        }
        out
    }

    #[test]
    fn reads_tensors_back_to_back() {
        let mut bytes = tensor(&[2, 3], &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        bytes.extend(tensor(&[200], &vec![0.5; 200]));
        let t = parse_combined(&bytes).unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].dims, vec![2, 3]);
        assert_eq!(t[0].to_f32().unwrap(), vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        assert_eq!(t[1].dims, vec![200]);
        assert!(parse_combined(&bytes[..bytes.len() - 1]).is_err());
    }
}
