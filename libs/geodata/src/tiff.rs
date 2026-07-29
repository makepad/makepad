//! GeoTIFF subset reader for the raster sources we actually use (Copernicus
//! GLO-30 COGs, JRC flood hazard, RIVM noise, AHN BigTIFF): classic TIFF and
//! BigTIFF, single image
//! (first IFD), tiled or striped, compression none/LZW/deflate, predictor
//! 1/2/3, sample formats uint/int/float 8-64 bit, single band (extra bands
//! ignored). Everything is surfaced as f32 with an optional nodata value.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub struct Tiff {
    file: std::fs::File,
    le: bool,
    pub width: u32,
    pub height: u32,
    bits: u16,
    sample_format: u16,
    samples_per_pixel: u16,
    compression: u16,
    predictor: u16,
    tiled: bool,
    pub block_w: u32,
    pub block_h: u32,
    offsets: Vec<u64>,
    counts: Vec<u64>,
    /// (origin_x, origin_y, scale_x, scale_y) in model (geo) coordinates;
    /// pixel (0,0) top-left corner maps to (origin_x, origin_y), y decreasing.
    pub geo: Option<(f64, f64, f64, f64)>,
    pub nodata: Option<f64>,
    cache: HashMap<u32, Vec<f32>>,
    cache_order: Vec<u32>,
}

const CACHE_BLOCKS: usize = 64;

impl Tiff {
    pub fn open(path: &Path) -> Result<Self, String> {
        let mut file =
            std::fs::File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
        let mut header = [0u8; 8];
        file.read_exact(&mut header)
            .map_err(|e| format!("tiff header: {e}"))?;
        let le = match &header[0..2] {
            b"II" => true,
            b"MM" => false,
            _ => return Err("not a tiff".into()),
        };
        let magic = read_u16(&header[2..4], le);
        // BigTIFF (magic 43): 8-byte offsets, u64 IFD count, 20-byte entries.
        let big = magic == 43;
        if !big && magic != 42 {
            return Err("bad tiff magic".into());
        }
        let ifd_offset = if big {
            let mut rest = [0u8; 8];
            file.read_exact(&mut rest)
                .map_err(|e| format!("bigtiff header: {e}"))?;
            if read_u16(&header[4..6], le) != 8 || read_u16(&header[6..8], le) != 0 {
                return Err("bad bigtiff header".into());
            }
            read_u64(&rest, le)
        } else {
            u64::from(read_u32(&header[4..8], le))
        };

        let mut tags: HashMap<u16, (u16, u64, Vec<u8>)> = HashMap::new();
        file.seek(SeekFrom::Start(ifd_offset))
            .map_err(|e| format!("seek ifd: {e}"))?;
        let entry_count = if big {
            let mut count_buf = [0u8; 8];
            file.read_exact(&mut count_buf)
                .map_err(|e| format!("ifd count: {e}"))?;
            read_u64(&count_buf, le)
        } else {
            let mut count_buf = [0u8; 2];
            file.read_exact(&mut count_buf)
                .map_err(|e| format!("ifd count: {e}"))?;
            u64::from(read_u16(&count_buf, le))
        };
        let entry_size = if big { 20 } else { 12 };
        let inline_max = if big { 8 } else { 4 };
        let mut entries = vec![0u8; entry_count as usize * entry_size];
        file.read_exact(&mut entries)
            .map_err(|e| format!("ifd entries: {e}"))?;
        for chunk in entries.chunks_exact(entry_size) {
            let tag = read_u16(&chunk[0..2], le);
            let field_type = read_u16(&chunk[2..4], le);
            let count = if big {
                read_u64(&chunk[4..12], le)
            } else {
                u64::from(read_u32(&chunk[4..8], le))
            };
            let type_size = tiff_type_size(field_type);
            let total = count * type_size;
            let inline = if big { &chunk[12..20] } else { &chunk[8..12] };
            let data = if total <= inline_max {
                inline[..total.min(inline_max) as usize].to_vec()
            } else {
                let offset = if big {
                    read_u64(inline, le)
                } else {
                    u64::from(read_u32(inline, le))
                };
                let mut buf = vec![0u8; total as usize];
                file.seek(SeekFrom::Start(offset))
                    .map_err(|e| format!("seek tag {tag}: {e}"))?;
                file.read_exact(&mut buf)
                    .map_err(|e| format!("read tag {tag}: {e}"))?;
                buf
            };
            tags.insert(tag, (field_type, count, data));
        }

        let scalar = |tag: u16| -> Option<u64> {
            let (t, _, data) = tags.get(&tag)?;
            read_tag_values(*t, data, le).first().copied().map(|v| v as u64)
        };
        let values = |tag: u16| -> Vec<u64> {
            tags.get(&tag)
                .map(|(t, _, data)| {
                    read_tag_values(*t, data, le)
                        .into_iter()
                        .map(|v| v as u64)
                        .collect()
                })
                .unwrap_or_default()
        };
        let doubles = |tag: u16| -> Vec<f64> {
            tags.get(&tag)
                .map(|(t, _, data)| read_tag_doubles(*t, data, le))
                .unwrap_or_default()
        };

        let width = scalar(256).ok_or("no width")? as u32;
        let height = scalar(257).ok_or("no height")? as u32;
        let bits = scalar(258).unwrap_or(8) as u16;
        let compression = scalar(259).unwrap_or(1) as u16;
        let samples_per_pixel = scalar(277).unwrap_or(1) as u16;
        let sample_format = scalar(339).unwrap_or(1) as u16;
        let predictor = scalar(317).unwrap_or(1) as u16;

        let (tiled, block_w, block_h, offsets, counts) = if tags.contains_key(&322) {
            (
                true,
                scalar(322).unwrap_or(256) as u32,
                scalar(323).unwrap_or(256) as u32,
                values(324),
                values(325),
            )
        } else {
            let rows_per_strip = scalar(278).unwrap_or(u64::from(height)) as u32;
            (false, width, rows_per_strip, values(273), values(279))
        };
        if offsets.is_empty() {
            return Err("tiff has no data blocks".into());
        }

        // GeoTIFF georeferencing: ModelPixelScale (33550) + ModelTiepoint (33922).
        let scale = doubles(33550);
        let tiepoint = doubles(33922);
        let geo = if scale.len() >= 2 && tiepoint.len() >= 6 {
            let origin_x = tiepoint[3] - tiepoint[0] * scale[0];
            let origin_y = tiepoint[4] + tiepoint[1] * scale[1];
            Some((origin_x, origin_y, scale[0], scale[1]))
        } else {
            None
        };
        // GDAL_NODATA (42113) is ASCII.
        let nodata = tags.get(&42113).and_then(|(_, _, data)| {
            std::str::from_utf8(data)
                .ok()?
                .trim_end_matches('\0')
                .trim()
                .parse()
                .ok()
        });

        Ok(Tiff {
            file,
            le,
            width,
            height,
            bits,
            sample_format,
            samples_per_pixel,
            compression,
            predictor,
            tiled,
            block_w,
            block_h,
            offsets,
            counts,
            geo,
            nodata,
            cache: HashMap::new(),
            cache_order: Vec::new(),
        })
    }

    pub fn blocks_across(&self) -> u32 {
        self.width.div_ceil(self.block_w)
    }
    pub fn blocks_down(&self) -> u32 {
        self.height.div_ceil(self.block_h)
    }

    /// Sample the first band at pixel coords; None outside or nodata.
    pub fn sample(&mut self, x: i64, y: i64) -> Option<f32> {
        if x < 0 || y < 0 || x >= i64::from(self.width) || y >= i64::from(self.height) {
            return None;
        }
        let (x, y) = (x as u32, y as u32);
        let block_index = if self.tiled {
            (y / self.block_h) * self.blocks_across() + (x / self.block_w)
        } else {
            y / self.block_h
        };
        let block_w = self.block_w as usize;
        let local_x = (x % self.block_w) as usize;
        let local_y = (y % self.block_h) as usize;
        let block = self.block(block_index).ok()?;
        let value = *block.get(local_y * block_w + local_x)?;
        if let Some(nodata) = self.nodata {
            if (f64::from(value) - nodata).abs() < 1e-6 || value.is_nan() {
                return None;
            }
        } else if value.is_nan() {
            return None;
        }
        Some(value)
    }

    /// Sample at model (geo) coordinates with bilinear interpolation.
    pub fn sample_geo(&mut self, gx: f64, gy: f64) -> Option<f32> {
        let (ox, oy, sx, sy) = self.geo?;
        let fx = (gx - ox) / sx - 0.5;
        let fy = (oy - gy) / sy - 0.5;
        let x0 = fx.floor() as i64;
        let y0 = fy.floor() as i64;
        let tx = (fx - x0 as f64) as f32;
        let ty = (fy - y0 as f64) as f32;
        let p00 = self.sample(x0, y0);
        let p10 = self.sample(x0 + 1, y0);
        let p01 = self.sample(x0, y0 + 1);
        let p11 = self.sample(x0 + 1, y0 + 1);
        match (p00, p10, p01, p11) {
            (Some(a), Some(b), Some(c), Some(d)) => {
                Some(a * (1.0 - tx) * (1.0 - ty) + b * tx * (1.0 - ty)
                    + c * (1.0 - tx) * ty + d * tx * ty)
            }
            // Fall back to nearest if some corners are nodata.
            _ => self.sample(fx.round() as i64, fy.round() as i64),
        }
    }

    fn block(&mut self, index: u32) -> Result<&Vec<f32>, String> {
        if !self.cache.contains_key(&index) {
            let data = self.decode_block(index)?;
            if self.cache.len() >= CACHE_BLOCKS {
                if let Some(evict) = self.cache_order.first().copied() {
                    self.cache.remove(&evict);
                    self.cache_order.remove(0);
                }
            }
            self.cache.insert(index, data);
            self.cache_order.push(index);
        }
        Ok(self.cache.get(&index).unwrap())
    }

    fn decode_block(&mut self, index: u32) -> Result<Vec<f32>, String> {
        let offset = *self.offsets.get(index as usize).ok_or("block index oob")?;
        let count = *self.counts.get(index as usize).ok_or("block index oob")? as usize;
        let mut raw = vec![0u8; count];
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|e| format!("seek block: {e}"))?;
        self.file
            .read_exact(&mut raw)
            .map_err(|e| format!("read block: {e}"))?;

        let rows = if self.tiled {
            self.block_h as usize
        } else {
            // last strip may be short
            let strip_row = index * self.block_h;
            (self.height - strip_row).min(self.block_h) as usize
        };
        let sample_bytes = usize::from(self.bits / 8);
        let pixel_bytes = sample_bytes * usize::from(self.samples_per_pixel);
        let row_bytes = self.block_w as usize * pixel_bytes;
        let expected = rows * row_bytes;

        let mut data = match self.compression {
            1 => raw,
            8 | 32946 => {
                let mut out = Vec::with_capacity(expected);
                flate2::read::ZlibDecoder::new(&raw[..])
                    .read_to_end(&mut out)
                    .map_err(|e| format!("tiff deflate: {e}"))?;
                out
            }
            5 => lzw_decode(&raw, expected)?,
            other => return Err(format!("unsupported tiff compression {other}")),
        };
        if data.len() < expected {
            return Err(format!(
                "tiff block short: {} < {expected}",
                data.len()
            ));
        }
        data.truncate(expected);

        match self.predictor {
            1 => {}
            2 => {
                // horizontal differencing per row, per sample channel
                for row in data.chunks_exact_mut(row_bytes) {
                    match sample_bytes {
                        1 => {
                            for i in pixel_bytes..row.len() {
                                row[i] = row[i].wrapping_add(row[i - pixel_bytes]);
                            }
                        }
                        2 => {
                            for i in (pixel_bytes..row.len()).step_by(2) {
                                let prev = read_u16(&row[i - pixel_bytes..], self.le);
                                let cur = read_u16(&row[i..], self.le);
                                let sum = cur.wrapping_add(prev);
                                let bytes = if self.le {
                                    sum.to_le_bytes()
                                } else {
                                    sum.to_be_bytes()
                                };
                                row[i..i + 2].copy_from_slice(&bytes);
                            }
                        }
                        4 => {
                            for i in (pixel_bytes..row.len()).step_by(4) {
                                let prev = read_u32(&row[i - pixel_bytes..], self.le);
                                let cur = read_u32(&row[i..], self.le);
                                let sum = cur.wrapping_add(prev);
                                let bytes = if self.le {
                                    sum.to_le_bytes()
                                } else {
                                    sum.to_be_bytes()
                                };
                                row[i..i + 4].copy_from_slice(&bytes);
                            }
                        }
                        _ => return Err("predictor 2 with odd sample size".into()),
                    }
                }
            }
            3 => {
                // floating-point predictor: per row, bytes are stored
                // plane-separated and horizontally differenced.
                let mut fixed = vec![0u8; data.len()];
                for (row_index, row) in data.chunks_exact(row_bytes).enumerate() {
                    let mut undiff = row.to_vec();
                    for i in 1..undiff.len() {
                        undiff[i] = undiff[i].wrapping_add(undiff[i - 1]);
                    }
                    let out_row =
                        &mut fixed[row_index * row_bytes..(row_index + 1) * row_bytes];
                    let n = self.block_w as usize * usize::from(self.samples_per_pixel);
                    for pixel in 0..n {
                        for byte in 0..sample_bytes {
                            // planes are stored big-endian-first
                            out_row[pixel * sample_bytes + byte] =
                                undiff[byte * n + pixel];
                        }
                    }
                }
                // predictor-3 output is big-endian IEEE floats by definition
                let mut values = Vec::with_capacity(rows * self.block_w as usize);
                for pixel in fixed.chunks_exact(pixel_bytes) {
                    values.push(f32::from_be_bytes(
                        pixel[0..4].try_into().map_err(|_| "pred3 not f32")?,
                    ));
                }
                return Ok(self.pad_block(values, rows));
            }
            other => return Err(format!("unsupported tiff predictor {other}")),
        }

        // Convert first band to f32.
        let mut values = Vec::with_capacity(rows * self.block_w as usize);
        for pixel in data.chunks_exact(pixel_bytes) {
            let sample = &pixel[0..sample_bytes];
            let value = match (self.sample_format, self.bits) {
                (1, 8) => f32::from(sample[0]),
                (1, 16) => f32::from(read_u16(sample, self.le)),
                (1, 32) => read_u32(sample, self.le) as f32,
                (2, 8) => f32::from(sample[0] as i8),
                (2, 16) => f32::from(read_u16(sample, self.le) as i16),
                (2, 32) => (read_u32(sample, self.le) as i32) as f32,
                (3, 32) => {
                    let bits = read_u32(sample, self.le);
                    f32::from_bits(bits)
                }
                (3, 64) => {
                    let mut b = [0u8; 8];
                    b.copy_from_slice(&pixel[0..8]);
                    let bits = if self.le {
                        u64::from_le_bytes(b)
                    } else {
                        u64::from_be_bytes(b)
                    };
                    f64::from_bits(bits) as f32
                }
                (f, b) => return Err(format!("unsupported sample format {f}/{b}")),
            };
            values.push(value);
        }
        Ok(self.pad_block(values, rows))
    }

    /// Pad a short (edge) block to full block_w*block_h with NaN.
    fn pad_block(&self, values: Vec<f32>, rows: usize) -> Vec<f32> {
        let full = self.block_w as usize * self.block_h as usize;
        if values.len() >= full {
            return values;
        }
        let mut padded = values;
        padded.resize(rows * self.block_w as usize, f32::NAN);
        padded.resize(full, f32::NAN);
        padded
    }
}

/// TIFF-flavor LZW (MSB-first bit order, early code-size change).
fn lzw_decode(input: &[u8], expected: usize) -> Result<Vec<u8>, String> {
    const CLEAR: u16 = 256;
    const EOI: u16 = 257;
    let mut out = Vec::with_capacity(expected);
    let mut dict: Vec<Vec<u8>> = Vec::new();
    let reset_dict = |dict: &mut Vec<Vec<u8>>| {
        dict.clear();
        for i in 0..258u16 {
            dict.push(if i < 256 { vec![i as u8] } else { Vec::new() });
        }
    };
    reset_dict(&mut dict);
    let mut code_size = 9u32;
    let mut bit_pos = 0usize;
    let mut prev: Option<u16> = None;

    let read_code = |bit_pos: &mut usize, code_size: u32| -> Option<u16> {
        let mut code = 0u32;
        for _ in 0..code_size {
            let byte = *input.get(*bit_pos / 8)?;
            let bit = (byte >> (7 - (*bit_pos % 8))) & 1;
            code = (code << 1) | u32::from(bit);
            *bit_pos += 1;
        }
        Some(code as u16)
    };

    while out.len() < expected {
        let Some(code) = read_code(&mut bit_pos, code_size) else {
            break;
        };
        if code == EOI {
            break;
        }
        if code == CLEAR {
            reset_dict(&mut dict);
            code_size = 9;
            prev = None;
            continue;
        }
        let entry = if (code as usize) < dict.len() && !(code != CLEAR && dict[code as usize].is_empty() && code >= 258) {
            dict[code as usize].clone()
        } else if let Some(p) = prev {
            let mut e = dict[p as usize].clone();
            e.push(dict[p as usize][0]);
            e
        } else {
            return Err("lzw: bad first code".into());
        };
        out.extend_from_slice(&entry);
        if let Some(p) = prev {
            let mut new_entry = dict[p as usize].clone();
            new_entry.push(entry[0]);
            dict.push(new_entry);
        }
        prev = Some(code);
        // TIFF early change: bump code size one code early.
        if dict.len() + 1 >= (1 << code_size) && code_size < 12 {
            code_size += 1;
        }
    }
    Ok(out)
}

fn tiff_type_size(field_type: u16) -> u64 {
    match field_type {
        1 | 2 | 6 | 7 => 1, // byte, ascii, sbyte, undefined
        3 | 8 => 2,         // short
        4 | 9 | 11 => 4,    // long, slong, float
        5 | 10 | 12 => 8,   // rational, srational, double
        16 | 17 | 18 => 8,  // long8, slong8, ifd8 (BigTIFF)
        _ => 1,
    }
}

fn read_tag_values(field_type: u16, data: &[u8], le: bool) -> Vec<f64> {
    let size = tiff_type_size(field_type) as usize;
    data.chunks_exact(size)
        .map(|chunk| match field_type {
            1 | 2 | 6 | 7 => f64::from(chunk[0]),
            3 | 8 => f64::from(read_u16(chunk, le)),
            4 | 9 => f64::from(read_u32(chunk, le)),
            11 => {
                let bits = read_u32(chunk, le);
                f64::from(f32::from_bits(bits))
            }
            12 => {
                let mut b = [0u8; 8];
                b.copy_from_slice(chunk);
                f64::from_bits(if le {
                    u64::from_le_bytes(b)
                } else {
                    u64::from_be_bytes(b)
                })
            }
            5 | 10 => {
                let num = read_u32(&chunk[0..4], le);
                let den = read_u32(&chunk[4..8], le);
                if den == 0 {
                    0.0
                } else {
                    f64::from(num) / f64::from(den)
                }
            }
            16 | 17 | 18 => read_u64(chunk, le) as f64,
            _ => 0.0,
        })
        .collect()
}

fn read_tag_doubles(field_type: u16, data: &[u8], le: bool) -> Vec<f64> {
    read_tag_values(field_type, data, le)
}

fn read_u16(data: &[u8], le: bool) -> u16 {
    let bytes: [u8; 2] = data[0..2].try_into().unwrap();
    if le {
        u16::from_le_bytes(bytes)
    } else {
        u16::from_be_bytes(bytes)
    }
}

fn read_u32(data: &[u8], le: bool) -> u32 {
    let bytes: [u8; 4] = data[0..4].try_into().unwrap();
    if le {
        u32::from_le_bytes(bytes)
    } else {
        u32::from_be_bytes(bytes)
    }
}

fn read_u64(data: &[u8], le: bool) -> u64 {
    let bytes: [u8; 8] = data[0..8].try_into().unwrap();
    if le {
        u64::from_le_bytes(bytes)
    } else {
        u64::from_be_bytes(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore] // needs local AHN download
    fn probe_ahn_bigtiff() {
        for name in ["M_25GN1", "R_25GN1"] {
            let path = format!("../../examples/map/local/ahn/{name}.tif");
            let mut t = Tiff::open(std::path::Path::new(&path)).unwrap();
            eprintln!(
                "{name}: {}x{} block {}x{} comp {} pred {} bits {} fmt {} geo {:?} nodata {:?}",
                t.width, t.height, t.block_w, t.block_h, t.compression, t.predictor,
                t.bits, t.sample_format, t.geo, t.nodata
            );
            // Dam square ~ RD (121350, 487350); Oosterdok water ~ (122400, 487650)
            eprintln!("  dam {:?} water {:?}",
                t.sample_geo(121350.0, 487350.0),
                t.sample_geo(122400.0, 487650.0));
        }
    }
}
