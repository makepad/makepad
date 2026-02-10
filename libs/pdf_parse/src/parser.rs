use crate::filter;
use crate::lexer::*;
use crate::object::*;
use std::collections::HashMap;

/// Parsed cross-reference entry.
#[derive(Clone, Debug)]
pub struct XRefEntry {
    pub offset: usize,
    pub gen: u16,
    pub in_use: bool,
}

/// Parsed cross-reference table + trailer.
#[derive(Clone, Debug)]
pub struct XRefTable {
    pub entries: HashMap<u32, XRefEntry>,
    pub trailer: PdfDict,
}

/// Find startxref from the end of the file.
pub fn find_startxref(data: &[u8]) -> PdfResult<usize> {
    // Search backwards from the end for "startxref"
    let search_range = data.len().min(1024);
    let tail = &data[data.len() - search_range..];
    let pos =
        Lexer::rfind(tail, b"startxref").ok_or_else(|| PdfError::new("cannot find startxref"))?;
    let abs_pos = data.len() - search_range + pos;

    // Parse the offset after "startxref"
    let mut lex = Lexer::new(data, abs_pos + 9); // skip "startxref"
    lex.skip_whitespace();
    let obj = lex.read_object()?;
    match obj {
        PdfObj::Int(n) if n >= 0 => Ok(n as usize),
        _ => Err(PdfError::new("invalid startxref value")),
    }
}

/// Parse the cross-reference table(s) and trailer(s).
/// Handles traditional xref tables. For xref streams (PDF 1.5+), falls back to brute-force scan.
pub fn parse_xref(data: &[u8]) -> PdfResult<XRefTable> {
    let xref_offset = find_startxref(data)?;

    // Check if it's a traditional xref table or an xref stream
    let mut lex = Lexer::new(data, xref_offset);
    lex.skip_whitespace();

    if lex.starts_with(b"xref") {
        parse_xref_table(data, xref_offset)
    } else {
        // Might be an xref stream (PDF 1.5+) or a damaged file.
        // Try to parse as xref stream object
        parse_xref_stream(data, xref_offset).or_else(|_| brute_force_xref(data))
    }
}

fn parse_xref_table(data: &[u8], offset: usize) -> PdfResult<XRefTable> {
    let mut lex = Lexer::new(data, offset);
    let kw = lex.read_keyword()?;
    if kw != "xref" {
        return Err(PdfError::new("expected 'xref' keyword"));
    }

    let mut entries = HashMap::new();

    // Read subsections
    loop {
        lex.skip_whitespace();
        if lex.is_eof() || lex.starts_with(b"trailer") {
            break;
        }

        // Read start_obj count
        let start_obj = match lex.read_object()? {
            PdfObj::Int(n) => n as u32,
            _ => break,
        };
        let count = match lex.read_object()? {
            PdfObj::Int(n) => n as u32,
            _ => return Err(PdfError::new("expected count in xref subsection")),
        };

        for i in 0..count {
            lex.skip_whitespace();
            // Each entry is: OFFSET GEN [n|f]\r\n (20 bytes fixed)
            let offset_obj = lex.read_object()?;
            let gen_obj = lex.read_object()?;
            let flag = lex.read_keyword()?;

            let entry_offset = offset_obj.as_int().unwrap_or(0) as usize;
            let entry_gen = gen_obj.as_int().unwrap_or(0) as u16;
            let in_use = flag == "n";

            if in_use {
                entries.insert(
                    start_obj + i,
                    XRefEntry {
                        offset: entry_offset,
                        gen: entry_gen,
                        in_use: true,
                    },
                );
            }
        }
    }

    // Parse trailer
    lex.skip_whitespace();
    let kw = lex.read_keyword()?;
    if kw != "trailer" {
        return Err(PdfError::new("expected 'trailer' keyword"));
    }
    let trailer_obj = lex.read_object()?;
    let trailer = match trailer_obj {
        PdfObj::Dict(d) => d,
        _ => return Err(PdfError::new("trailer must be a dict")),
    };

    // Handle /Prev for incremental updates
    let mut result = XRefTable {
        entries,
        trailer: trailer.clone(),
    };
    if let Some(prev_offset) = trailer.get_int("Prev") {
        if prev_offset >= 0 {
            if let Ok(prev_xref) = parse_xref_table(data, prev_offset as usize) {
                // Merge: current entries take precedence over previous
                for (k, v) in prev_xref.entries {
                    result.entries.entry(k).or_insert(v);
                }
            }
        }
    }

    Ok(result)
}

fn parse_xref_stream(data: &[u8], offset: usize) -> PdfResult<XRefTable> {
    // An xref stream is a normal object: "N G obj << ... >> stream ... endstream endobj"
    let obj = parse_indirect_object_at(data, offset)?;
    let stream = obj
        .1
        .as_stream()
        .ok_or_else(|| PdfError::new("xref stream must be a stream object"))?;

    let dict = &stream.dict;
    let type_name = dict.get_name("Type").unwrap_or("");
    if type_name != "XRef" {
        return Err(PdfError::new("not an XRef stream"));
    }

    let size = dict
        .get_int("Size")
        .ok_or_else(|| PdfError::new("XRef stream missing /Size"))? as u32;

    // /W array: field widths
    let w_arr = dict
        .get_array("W")
        .ok_or_else(|| PdfError::new("XRef stream missing /W"))?;
    if w_arr.len() != 3 {
        return Err(PdfError::new("XRef stream /W must have 3 entries"));
    }
    let w: Vec<usize> = w_arr
        .iter()
        .map(|o| o.as_int().unwrap_or(0) as usize)
        .collect();
    let entry_size = w[0] + w[1] + w[2];

    // /Index array (optional): pairs of (first_obj, count)
    let index_pairs: Vec<(u32, u32)> = if let Some(idx) = dict.get_array("Index") {
        idx.chunks(2)
            .map(|chunk| {
                let first = chunk.get(0).and_then(|o| o.as_int()).unwrap_or(0) as u32;
                let count = chunk.get(1).and_then(|o| o.as_int()).unwrap_or(0) as u32;
                (first, count)
            })
            .collect()
    } else {
        vec![(0, size)]
    };

    // Decode stream data
    let decoded = filter::decode_stream(&stream.data, dict)?;

    let mut entries = HashMap::new();
    let mut data_pos = 0;

    for (first_obj, count) in &index_pairs {
        for i in 0..*count {
            if data_pos + entry_size > decoded.len() {
                break;
            }
            let field0 = read_field(&decoded[data_pos..], w[0]);
            let field1 = read_field(&decoded[data_pos + w[0]..], w[1]);
            let field2 = read_field(&decoded[data_pos + w[0] + w[1]..], w[2]);
            data_pos += entry_size;

            let obj_type = if w[0] == 0 { 1 } else { field0 }; // default type=1 if w[0]==0
            let obj_num = first_obj + i;

            match obj_type {
                0 => {} // free object
                1 => {
                    // Normal object: field1=offset, field2=gen
                    entries.insert(
                        obj_num,
                        XRefEntry {
                            offset: field1 as usize,
                            gen: field2 as u16,
                            in_use: true,
                        },
                    );
                }
                2 => {
                    // Compressed object in object stream: field1=stream_obj_num, field2=index
                    // Store with a special marker - we'll handle these in document.rs
                    entries.insert(
                        obj_num,
                        XRefEntry {
                            offset: field1 as usize, // obj stream number
                            gen: field2 as u16,      // index within obj stream
                            in_use: true,
                        },
                    );
                }
                _ => {}
            }
        }
    }

    // Build trailer from stream dict (it serves as the trailer)
    let trailer = dict.clone();

    // Handle /Prev
    let mut result = XRefTable {
        entries,
        trailer: trailer.clone(),
    };
    if let Some(prev_offset) = trailer.get_int("Prev") {
        if prev_offset >= 0 {
            if let Ok(prev) = parse_xref_stream(data, prev_offset as usize)
                .or_else(|_| parse_xref_table(data, prev_offset as usize))
            {
                for (k, v) in prev.entries {
                    result.entries.entry(k).or_insert(v);
                }
            }
        }
    }

    Ok(result)
}

fn read_field(data: &[u8], width: usize) -> u64 {
    let mut val: u64 = 0;
    for i in 0..width {
        val = (val << 8) | (data.get(i).copied().unwrap_or(0) as u64);
    }
    val
}

/// Brute-force scan: find all "N G obj" patterns in the file.
fn brute_force_xref(data: &[u8]) -> PdfResult<XRefTable> {
    let mut entries = HashMap::new();
    let mut i = 0;
    while i < data.len().saturating_sub(10) {
        // Look for patterns like "123 0 obj"
        if data[i].is_ascii_digit() {
            if let Some((obj_num, gen, end_pos)) = try_parse_obj_header(data, i) {
                entries.insert(
                    obj_num,
                    XRefEntry {
                        offset: i,
                        gen,
                        in_use: true,
                    },
                );
                i = end_pos;
                continue;
            }
        }
        i += 1;
    }

    // Try to find trailer
    let trailer = if let Some(t_pos) = Lexer::rfind(data, b"trailer") {
        let mut lex = Lexer::new(data, t_pos + 7);
        match lex.read_object() {
            Ok(PdfObj::Dict(d)) => d,
            _ => PdfDict::new(),
        }
    } else {
        PdfDict::new()
    };

    if entries.is_empty() {
        return Err(PdfError::new("no objects found in brute-force scan"));
    }

    Ok(XRefTable { entries, trailer })
}

fn try_parse_obj_header(data: &[u8], pos: usize) -> Option<(u32, u16, usize)> {
    let mut lex = Lexer::new(data, pos);
    let n = match lex.read_object().ok()? {
        PdfObj::Int(n) if n >= 0 => n as u32,
        _ => return None,
    };
    let g = match lex.read_object().ok()? {
        PdfObj::Int(g) if g >= 0 => g as u16,
        _ => return None,
    };
    let kw = lex.read_keyword().ok()?;
    if kw == "obj" {
        Some((n, g, lex.pos))
    } else {
        None
    }
}

/// Parse an indirect object at the given offset.
/// Returns (ObjRef, PdfObj).
pub fn parse_indirect_object_at(data: &[u8], offset: usize) -> PdfResult<(ObjRef, PdfObj)> {
    let mut lex = Lexer::new(data, offset);
    lex.skip_whitespace();

    let num = match lex.read_object()? {
        PdfObj::Int(n) => n as u32,
        _ => return Err(PdfError::new("expected object number")),
    };
    let gen = match lex.read_object()? {
        PdfObj::Int(g) => g as u16,
        _ => return Err(PdfError::new("expected generation number")),
    };
    let kw = lex.read_keyword()?;
    if kw != "obj" {
        return Err(PdfError::new(format!("expected 'obj', got '{}'", kw)));
    }

    let obj = lex.read_object()?;

    // Check for stream
    lex.skip_whitespace();
    let obj = if lex.starts_with(b"stream") {
        lex.pos += 6; // skip "stream"
                      // Skip the EOL after "stream" (could be \r\n or \n or \r)
        if lex.pos < data.len() && data[lex.pos] == b'\r' {
            lex.pos += 1;
        }
        if lex.pos < data.len() && data[lex.pos] == b'\n' {
            lex.pos += 1;
        }

        let dict = match obj {
            PdfObj::Dict(d) => d,
            _ => return Err(PdfError::new("stream must follow a dict")),
        };

        // Get stream length
        let length = dict
            .get_int("Length")
            .ok_or_else(|| PdfError::new("stream missing /Length"))? as usize;

        let stream_start = lex.pos;
        let stream_end = (stream_start + length).min(data.len());
        let stream_data = data[stream_start..stream_end].to_vec();

        PdfObj::Stream(PdfStream {
            dict,
            data: stream_data,
        })
    } else {
        obj
    };

    Ok((ObjRef { num, gen }, obj))
}
