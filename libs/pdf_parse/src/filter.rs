use crate::lexer::{PdfError, PdfResult};
use crate::object::PdfDict;

/// Apply a chain of PDF stream filters to decompress data.
pub fn decode_stream(data: &[u8], dict: &PdfDict) -> PdfResult<Vec<u8>> {
    let filters = get_filter_names(dict);
    let decode_parms = get_decode_parms(dict);

    let mut current = data.to_vec();
    for (i, filter) in filters.iter().enumerate() {
        let parms = decode_parms.get(i).and_then(|p| p.as_ref());
        current = apply_filter(filter, &current, parms)?;
    }
    Ok(current)
}

fn get_filter_names(dict: &PdfDict) -> Vec<String> {
    match dict.get("Filter") {
        Some(crate::object::PdfObj::Name(n)) => vec![n.clone()],
        Some(crate::object::PdfObj::Array(arr)) => arr
            .iter()
            .filter_map(|o| o.as_name().map(|s| s.to_string()))
            .collect(),
        _ => vec![],
    }
}

fn get_decode_parms(dict: &PdfDict) -> Vec<Option<PdfDict>> {
    match dict.get("DecodeParms") {
        Some(crate::object::PdfObj::Dict(d)) => vec![Some(d.clone())],
        Some(crate::object::PdfObj::Array(arr)) => arr
            .iter()
            .map(|o| match o {
                crate::object::PdfObj::Dict(d) => Some(d.clone()),
                _ => None,
            })
            .collect(),
        _ => vec![],
    }
}

fn apply_filter(name: &str, data: &[u8], parms: Option<&PdfDict>) -> PdfResult<Vec<u8>> {
    match name {
        "FlateDecode" | "Fl" => decode_flate(data, parms),
        "ASCIIHexDecode" | "AHx" => decode_ascii_hex(data),
        "ASCII85Decode" | "A85" => decode_ascii85(data),
        "DCTDecode" => Ok(data.to_vec()), // JPEG: pass through raw
        "JPXDecode" => Ok(data.to_vec()), // JPEG2000: pass through raw
        _ => Err(PdfError::new(format!("unsupported filter: {}", name))),
    }
}

fn decode_flate(data: &[u8], parms: Option<&PdfDict>) -> PdfResult<Vec<u8>> {
    use makepad_zune_inflate::DeflateDecoder;
    let mut decoder = DeflateDecoder::new_with_options(
        data,
        makepad_zune_inflate::DeflateOptions::default().set_confirm_checksum(false),
    );
    let decompressed = decoder
        .decode_zlib()
        .map_err(|e| PdfError::new(format!("FlateDecode error: {:?}", e)))?;

    // Apply predictor if specified
    if let Some(p) = parms {
        let predictor = p.get_int("Predictor").unwrap_or(1);
        if predictor > 1 {
            return apply_predictor(predictor, &decompressed, p);
        }
    }
    Ok(decompressed)
}

fn apply_predictor(predictor: i64, data: &[u8], parms: &PdfDict) -> PdfResult<Vec<u8>> {
    let columns = parms.get_int("Columns").unwrap_or(1) as usize;
    let colors = parms.get_int("Colors").unwrap_or(1) as usize;
    let bits = parms.get_int("BitsPerComponent").unwrap_or(8) as usize;
    let bytes_per_pixel = (colors * bits + 7) / 8;
    let row_bytes = (columns * colors * bits + 7) / 8;

    match predictor {
        // PNG predictors (10-15): each row has a filter byte prefix
        10..=15 => {
            let stride = row_bytes + 1; // +1 for the filter byte
            let num_rows = data.len() / stride;
            let mut result = Vec::with_capacity(num_rows * row_bytes);
            let mut prev_row = vec![0u8; row_bytes];
            for row_idx in 0..num_rows {
                let row_start = row_idx * stride;
                if row_start >= data.len() {
                    break;
                }
                let filter_type = data[row_start];
                let row_data = &data[row_start + 1..row_start + stride.min(data.len() - row_start)];
                let mut decoded_row = vec![0u8; row_bytes];
                let actual_len = row_data.len().min(row_bytes);
                match filter_type {
                    0 => {
                        // None
                        decoded_row[..actual_len].copy_from_slice(&row_data[..actual_len]);
                    }
                    1 => {
                        // Sub
                        for i in 0..actual_len {
                            let left = if i >= bytes_per_pixel {
                                decoded_row[i - bytes_per_pixel]
                            } else {
                                0
                            };
                            decoded_row[i] = row_data[i].wrapping_add(left);
                        }
                    }
                    2 => {
                        // Up
                        for i in 0..actual_len {
                            decoded_row[i] = row_data[i].wrapping_add(prev_row[i]);
                        }
                    }
                    3 => {
                        // Average
                        for i in 0..actual_len {
                            let left = if i >= bytes_per_pixel {
                                decoded_row[i - bytes_per_pixel] as u16
                            } else {
                                0
                            };
                            let up = prev_row[i] as u16;
                            decoded_row[i] = row_data[i].wrapping_add(((left + up) / 2) as u8);
                        }
                    }
                    4 => {
                        // Paeth
                        for i in 0..actual_len {
                            let left = if i >= bytes_per_pixel {
                                decoded_row[i - bytes_per_pixel]
                            } else {
                                0
                            };
                            let up = prev_row[i];
                            let upper_left = if i >= bytes_per_pixel {
                                prev_row[i - bytes_per_pixel]
                            } else {
                                0
                            };
                            decoded_row[i] = row_data[i].wrapping_add(paeth(left, up, upper_left));
                        }
                    }
                    _ => {
                        // Unknown filter: copy raw
                        decoded_row[..actual_len].copy_from_slice(&row_data[..actual_len]);
                    }
                }
                result.extend_from_slice(&decoded_row);
                prev_row = decoded_row;
            }
            Ok(result)
        }
        // TIFF predictor 2: horizontal differencing
        2 => {
            let mut result = data.to_vec();
            for row in 0..(result.len() / row_bytes) {
                let base = row * row_bytes;
                for i in bytes_per_pixel..row_bytes {
                    if base + i < result.len() {
                        result[base + i] =
                            result[base + i].wrapping_add(result[base + i - bytes_per_pixel]);
                    }
                }
            }
            Ok(result)
        }
        _ => Ok(data.to_vec()),
    }
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let a = a as i16;
    let b = b as i16;
    let c = c as i16;
    let p = a + b - c;
    let pa = (p - a).abs();
    let pb = (p - b).abs();
    let pc = (p - c).abs();
    if pa <= pb && pa <= pc {
        a as u8
    } else if pb <= pc {
        b as u8
    } else {
        c as u8
    }
}

pub fn decode_ascii_hex(data: &[u8]) -> PdfResult<Vec<u8>> {
    let mut result = Vec::new();
    let mut high: Option<u8> = None;
    for &b in data {
        if b == b'>' {
            break;
        }
        if b.is_ascii_whitespace() {
            continue;
        }
        let val = hex_val(b).ok_or_else(|| PdfError::new("invalid hex digit in ASCIIHexDecode"))?;
        match high {
            None => high = Some(val),
            Some(h) => {
                result.push((h << 4) | val);
                high = None;
            }
        }
    }
    if let Some(h) = high {
        result.push(h << 4);
    }
    Ok(result)
}

pub fn decode_ascii85(data: &[u8]) -> PdfResult<Vec<u8>> {
    let mut result = Vec::new();
    let mut i = 0;
    let end = data.len();
    while i < end {
        let b = data[i];
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if b == b'~' {
            break; // end marker ~>
        }
        if b == b'z' {
            // special: 'z' represents four zero bytes
            result.extend_from_slice(&[0, 0, 0, 0]);
            i += 1;
            continue;
        }
        // Collect up to 5 ASCII85 digits
        let mut group = Vec::new();
        while group.len() < 5 && i < end {
            let c = data[i];
            if c == b'~' {
                break;
            }
            if c.is_ascii_whitespace() {
                i += 1;
                continue;
            }
            if c < b'!' || c > b'u' {
                return Err(PdfError::new(format!("invalid ASCII85 char: {}", c)));
            }
            group.push(c - b'!');
            i += 1;
        }
        if group.is_empty() {
            break;
        }
        // Pad with 'u' (84) values to make 5 digits
        let out_count = group.len() - 1;
        while group.len() < 5 {
            group.push(84);
        }
        let mut val: u32 = 0;
        for &d in &group {
            val = val * 85 + d as u32;
        }
        let bytes = val.to_be_bytes();
        for j in 0..out_count {
            result.push(bytes[j]);
        }
    }
    Ok(result)
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
