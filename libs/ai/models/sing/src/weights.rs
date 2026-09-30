//! The MKSING weight file: a text header (magic, `key=value` config lines,
//! one `tensor <name> <rows> <cols> <f32|f16>` line per tensor, `---`), then
//! the tensors' little-endian data in header order. The trainer writes it;
//! inference reads it. Optimiser state goes in a second file of the same form.

use crate::nn::{Params, Tensor};
use std::io::{self, Read, Write};

pub const MAGIC: &str = "MKSING01";

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Dtype {
    F32,
    F16,
}

pub fn f32_to_f16(v: f32) -> u16 {
    let x = v.to_bits();
    let sign = ((x >> 16) & 0x8000) as u16;
    let exp = ((x >> 23) & 0xff) as i32;
    let mant = x & 0x7f_ffff;
    if exp == 0xff {
        return sign | 0x7c00 | if mant != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = (mant | 0x80_0000) >> (1 - e);
        let round = (m >> 12) & 1;
        return sign | ((m >> 13) + round) as u16;
    }
    let m = mant >> 13;
    let round = (mant >> 12) & 1;
    (sign | ((e as u16) << 10) | m as u16).wrapping_add(round as u16)
}

pub fn f16_to_f32(h: u16) -> f32 {
    let sign = ((h & 0x8000) as u32) << 16;
    let exp = ((h >> 10) & 0x1f) as u32;
    let mant = (h & 0x3ff) as u32;
    let bits = if exp == 0 {
        if mant == 0 {
            sign
        } else {
            let mut e = 127 - 15 + 1;
            let mut m = mant;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            sign | ((e as u32) << 23) | ((m & 0x3ff) << 13)
        }
    } else if exp == 0x1f {
        sign | 0x7f80_0000 | (mant << 13)
    } else {
        sign | ((exp + 127 - 15) << 23) | (mant << 13)
    };
    f32::from_bits(bits)
}

pub struct WeightFile {
    pub config: Vec<(String, String)>,
    pub params: Params,
}

pub fn write(path: &std::path::Path, config: &[(String, String)], params: &Params, dtype: Dtype) -> io::Result<()> {
    let mut head = format!("{MAGIC}\n");
    for (k, v) in config {
        head.push_str(&format!("{k}={v}\n"));
    }
    let ds = match dtype {
        Dtype::F32 => "f32",
        Dtype::F16 => "f16",
    };
    for (n, t) in params.names.iter().zip(&params.vals) {
        head.push_str(&format!("tensor {n} {} {} {ds}\n", t.rows, t.cols));
    }
    head.push_str("---\n");
    let tmp = path.with_extension("tmp");
    {
        let mut f = io::BufWriter::new(std::fs::File::create(&tmp)?);
        f.write_all(head.as_bytes())?;
        for t in &params.vals {
            match dtype {
                Dtype::F32 => {
                    for v in &t.data {
                        f.write_all(&v.to_le_bytes())?;
                    }
                }
                Dtype::F16 => {
                    for v in &t.data {
                        f.write_all(&f32_to_f16(*v).to_le_bytes())?;
                    }
                }
            }
        }
        f.flush()?;
    }
    std::fs::rename(tmp, path)
}

pub fn read(path: &std::path::Path) -> io::Result<WeightFile> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?.read_to_end(&mut bytes)?;
    parse(&bytes)
}

pub fn parse(bytes: &[u8]) -> io::Result<WeightFile> {
    let bad = |m: &str| io::Error::new(io::ErrorKind::InvalidData, m.to_string());
    let sep = b"\n---\n";
    let end = bytes.windows(sep.len()).position(|w| w == sep).ok_or_else(|| bad("MKSING: no header end"))?;
    let head = std::str::from_utf8(&bytes[..end]).map_err(|_| bad("MKSING: header not UTF-8"))?;
    let mut lines = head.lines();
    if lines.next() != Some(MAGIC) {
        return Err(bad("MKSING: bad magic"));
    }
    let mut config = Vec::new();
    let mut tensors = Vec::new();
    for l in lines {
        if let Some(rest) = l.strip_prefix("tensor ") {
            let f: Vec<&str> = rest.split(' ').collect();
            if f.len() != 4 {
                return Err(bad("MKSING: bad tensor line"));
            }
            let rows: usize = f[1].parse().map_err(|_| bad("rows"))?;
            let cols: usize = f[2].parse().map_err(|_| bad("cols"))?;
            let dt = if f[3] == "f16" { Dtype::F16 } else { Dtype::F32 };
            tensors.push((f[0].to_string(), rows, cols, dt));
        } else if let Some((k, v)) = l.split_once('=') {
            config.push((k.to_string(), v.to_string()));
        }
    }
    let mut at = end + sep.len();
    let mut params = Params::new();
    for (name, rows, cols, dt) in tensors {
        let n = rows * cols;
        let w = if dt == Dtype::F16 { 2 } else { 4 };
        if at + n * w > bytes.len() {
            return Err(bad("MKSING: truncated"));
        }
        let data = match dt {
            Dtype::F32 => bytes[at..at + 4 * n].chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect(),
            Dtype::F16 => bytes[at..at + 2 * n].chunks_exact(2).map(|c| f16_to_f32(u16::from_le_bytes([c[0], c[1]]))).collect(),
        };
        at += n * w;
        params.insert(&name, Tensor::new(rows, cols, data));
    }
    Ok(WeightFile { config, params })
}

/// Config lines with a prefix, prefix removed (`ac.d=256` -> `d=256`).
pub fn section(config: &[(String, String)], prefix: &str) -> Vec<(String, String)> {
    config.iter().filter_map(|(k, v)| k.strip_prefix(prefix).map(|k| (k.to_string(), v.clone()))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f16_round_trip() {
        for v in [0.0f32, 1.0, -2.5, 0.000123, 65504.0, 1e-6, -0.3333] {
            let r = f16_to_f32(f32_to_f16(v));
            assert!((r - v).abs() <= v.abs() * 1e-3 + 1e-7, "{v} -> {r}");
        }
    }
}
