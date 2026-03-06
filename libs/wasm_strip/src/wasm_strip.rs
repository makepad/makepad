use std::{collections::BTreeMap, mem};

#[derive(Clone, Debug)]
struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WasmParseError;

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader { bytes, offset: 0 }
    }

    fn skip(&mut self, count: usize) -> Result<(), WasmParseError> {
        if count > self.bytes.len() {
            return Err(WasmParseError);
        }
        self.offset += count;
        self.bytes = &self.bytes[count..];
        Ok(())
    }

    fn read(&mut self, bytes: &mut [u8]) -> Result<(), WasmParseError> {
        if bytes.len() > self.bytes.len() {
            return Err(WasmParseError);
        }
        bytes.copy_from_slice(&self.bytes[..bytes.len()]);
        self.bytes = &self.bytes[bytes.len()..];
        self.offset += bytes.len();
        Ok(())
    }

    fn read_u8(&mut self) -> Result<u8, WasmParseError> {
        let mut bytes = [0; mem::size_of::<u8>()];
        self.read(&mut bytes)?;
        Ok(u8::from_le_bytes(bytes))
    }

    fn read_u32(&mut self) -> Result<u32, WasmParseError> {
        let mut bytes = [0; mem::size_of::<u32>()];
        self.read(&mut bytes)?;
        Ok(u32::from_le_bytes(bytes))
    }

    fn read_var_u32(&mut self) -> Result<u32, WasmParseError> {
        let byte = self.read_u8()? as u32;
        if byte & 0x80 == 0 {
            return Ok(byte);
        }

        let mut result = byte & 0x7F;
        let mut shift = 7;
        loop {
            let byte = self.read_u8()?;
            result |= ((byte & 0x7F) as u32) << shift;
            if shift >= 25 && (byte >> (32 - shift)) != 0 {
                // The continuation bit or unused bits are set.
                return Err(WasmParseError);
            }
            shift += 7;
            if (byte & 0x80) == 0 {
                break;
            }
        }
        Ok(result)
    }

    fn read_var_i32(&mut self) -> Result<i32, WasmParseError> {
        let mut result = 0i32;
        let mut shift = 0;
        let mut byte;

        loop {
            byte = self.read_u8()?;
            result |= ((byte & 0x7f) as i32) << shift;
            shift += 7;
            if (byte & 0x80) == 0 {
                break;
            }
            if shift >= 35 {
                return Err(WasmParseError);
            }
        }

        if shift < 32 && (byte & 0x40) != 0 {
            result |= !0 << shift;
        }

        Ok(result)
    }

    fn read_vec(&mut self, len: usize) -> Result<Vec<u8>, WasmParseError> {
        if len > self.bytes.len() {
            return Err(WasmParseError);
        }
        let out = self.bytes[..len].to_vec();
        self.skip(len)?;
        Ok(out)
    }
}

pub struct WasmSection {
    pub type_id: u8,
    pub start: usize,
    pub end: usize,
    pub payload_start: usize,
    pub name: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WasmSectionSummary {
    pub total_bytes: usize,
    pub counts: BTreeMap<String, usize>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WasmSizeReport {
    pub original_bytes: usize,
    pub stripped_bytes: usize,
    pub optimized_bytes: usize,
    pub debug_sections: WasmSectionSummary,
    pub custom_sections: WasmSectionSummary,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WasmDataSplitResult {
    pub primary_wasm: Vec<u8>,
    pub split_data: Vec<u8>,
    pub segment_count: usize,
}

fn read_wasm_sections(buf: &[u8]) -> Result<Vec<WasmSection>, WasmParseError> {
    let mut sections = Vec::new();
    let mut reader = Reader::new(buf);
    if reader.read_u32()? != 0x6d736100 {
        println!("Not a wasm file!");
        return Err(WasmParseError);
    }
    if reader.read_u32()? != 0x1 {
        println!("Wrong version");
        return Err(WasmParseError);
    }
    loop {
        let offset = reader.offset;
        if let Ok(type_id) = reader.read_u8() {
            let payload_len = reader.read_var_u32()? as usize;
            let start = reader.offset;
            if type_id == 0 {
                let name_len = reader.read_var_u32()? as usize;
                if let Ok(name) = std::str::from_utf8(&reader.bytes[0..name_len]) {
                    sections.push(WasmSection {
                        start: offset,
                        type_id,
                        end: offset + payload_len + (start - offset),
                        payload_start: start,
                        name: name.to_string(),
                    })
                } else {
                    return Err(WasmParseError);
                }
                let end = reader.offset;
                reader.skip(payload_len - (end - start))?;
            } else {
                sections.push(WasmSection {
                    start: offset,
                    type_id,
                    end: offset + payload_len + (start - offset),
                    payload_start: start,
                    name: "".to_string(),
                });
                reader.skip(payload_len)?;
            }
        } else {
            break;
        }
    }
    Ok(sections)
}

fn is_debug_section(section: &WasmSection) -> bool {
    section.type_id == 0 && section.name.starts_with(".debug")
}

fn is_custom_section(section: &WasmSection) -> bool {
    section.type_id == 0
}

fn summarize_sections<F>(sections: &[WasmSection], filter: F) -> WasmSectionSummary
where
    F: Fn(&WasmSection) -> bool,
{
    let mut summary = WasmSectionSummary::default();
    for section in sections.iter().filter(|section| filter(section)) {
        summary.total_bytes += section.end - section.start;
        let key = if section.name.is_empty() {
            format!("section-{}", section.type_id)
        } else {
            section.name.clone()
        };
        *summary.counts.entry(key).or_insert(0) += 1;
    }
    summary
}

fn rewrite_wasm<F>(buf: &[u8], keep_section: F) -> Result<Vec<u8>, WasmParseError>
where
    F: Fn(&WasmSection) -> bool,
{
    let sections = read_wasm_sections(buf)?;
    let mut rewritten = Vec::with_capacity(buf.len());
    rewritten.extend_from_slice(&buf[..8]);
    for section in &sections {
        if keep_section(section) {
            rewritten.extend_from_slice(&buf[section.start..section.end]);
        }
    }
    Ok(rewritten)
}

fn rewrite_wasm_section(
    buf: &[u8],
    section_type_id: u8,
    replacement_payload: &[u8],
) -> Result<Vec<u8>, WasmParseError> {
    let sections = read_wasm_sections(buf)?;
    let mut rewritten = Vec::with_capacity(buf.len());
    rewritten.extend_from_slice(&buf[..8]);

    for section in &sections {
        if section.type_id == section_type_id {
            rewritten.push(section.type_id);
            rewritten.extend_from_slice(&encode_var_u32(replacement_payload.len() as u32));
            rewritten.extend_from_slice(replacement_payload);
        } else {
            rewritten.extend_from_slice(&buf[section.start..section.end]);
        }
    }

    Ok(rewritten)
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum WasmDataSegmentKind {
    Active { memory_index: u32, offset: u32 },
    Passive,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct WasmDataSegment {
    kind: WasmDataSegmentKind,
    bytes: Vec<u8>,
}

const SPLIT_DATA_VERSION_V2: u32 = 2;

fn encode_var_u32(mut value: u32) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
    out
}

fn encode_var_i32(mut value: i32) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let byte = (value as u8) & 0x7f;
        value >>= 7;
        let done = (value == 0 && (byte & 0x40) == 0) || (value == -1 && (byte & 0x40) != 0);
        if done {
            out.push(byte);
            break;
        } else {
            out.push(byte | 0x80);
        }
    }
    out
}

fn encode_const_i32_expr(offset: u32) -> Vec<u8> {
    let mut out = vec![0x41];
    out.extend_from_slice(&encode_var_i32(offset as i32));
    out.push(0x0b);
    out
}

fn parse_const_i32_expr(reader: &mut Reader<'_>) -> Result<u32, WasmParseError> {
    if reader.read_u8()? != 0x41 {
        return Err(WasmParseError);
    }
    let value = reader.read_var_i32()?;
    if value < 0 {
        return Err(WasmParseError);
    }
    if reader.read_u8()? != 0x0b {
        return Err(WasmParseError);
    }
    Ok(value as u32)
}

fn parse_data_segments(
    buf: &[u8],
    data_section: &WasmSection,
) -> Result<Vec<WasmDataSegment>, WasmParseError> {
    let mut reader = Reader::new(&buf[data_section.payload_start..data_section.end]);
    let segment_count = reader.read_var_u32()? as usize;
    let mut segments = Vec::with_capacity(segment_count);

    for _ in 0..segment_count {
        let flags = reader.read_var_u32()?;
        let kind = match flags {
            0 => WasmDataSegmentKind::Active {
                memory_index: 0,
                offset: parse_const_i32_expr(&mut reader)?,
            },
            1 => WasmDataSegmentKind::Passive,
            2 => WasmDataSegmentKind::Active {
                memory_index: reader.read_var_u32()?,
                offset: parse_const_i32_expr(&mut reader)?,
            },
            _ => return Err(WasmParseError),
        };
        let len = reader.read_var_u32()? as usize;
        let bytes = reader.read_vec(len)?;
        segments.push(WasmDataSegment { kind, bytes });
    }

    if !reader.bytes.is_empty() {
        return Err(WasmParseError);
    }

    Ok(segments)
}

fn encode_split_data(segments: &[WasmDataSegment]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"MPDS");
    out.extend_from_slice(&SPLIT_DATA_VERSION_V2.to_le_bytes());
    out.extend_from_slice(&(segments.len() as u32).to_le_bytes());
    for segment in segments {
        match segment.kind {
            WasmDataSegmentKind::Active {
                memory_index,
                offset,
            } => {
                out.push(0);
                out.extend_from_slice(&memory_index.to_le_bytes());
                out.extend_from_slice(&offset.to_le_bytes());
            }
            WasmDataSegmentKind::Passive => {
                out.push(1);
                out.extend_from_slice(&0u32.to_le_bytes());
                out.extend_from_slice(&0u32.to_le_bytes());
            }
        }
        out.extend_from_slice(&(segment.bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&segment.bytes);
    }
    out
}

fn encode_rewritten_data_section(segments: &[WasmDataSegment]) -> Result<Vec<u8>, WasmParseError> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&encode_var_u32(segments.len() as u32));

    for segment in segments {
        match segment.kind {
            WasmDataSegmentKind::Active {
                memory_index,
                offset,
            } => {
                if memory_index == 0 {
                    payload.push(0);
                } else {
                    payload.push(2);
                    payload.extend_from_slice(&encode_var_u32(memory_index));
                }
                payload.extend_from_slice(&encode_const_i32_expr(offset));
                payload.push(0);
            }
            WasmDataSegmentKind::Passive => {
                payload.push(1);
                payload.push(0);
            }
        }
    }

    Ok(payload)
}

pub fn wasm_size_report(buf: &[u8]) -> Result<WasmSizeReport, WasmParseError> {
    let sections = read_wasm_sections(buf)?;
    let stripped = rewrite_wasm(buf, |section| !is_debug_section(section))?;
    let optimized = rewrite_wasm(buf, |section| !is_custom_section(section))?;
    Ok(WasmSizeReport {
        original_bytes: buf.len(),
        stripped_bytes: stripped.len(),
        optimized_bytes: optimized.len(),
        debug_sections: summarize_sections(&sections, is_debug_section),
        custom_sections: summarize_sections(&sections, is_custom_section),
    })
}

pub fn wasm_strip_debug(buf: &[u8]) -> Result<Vec<u8>, WasmParseError> {
    rewrite_wasm(buf, |section| !is_debug_section(section))
}

pub fn wasm_strip_custom_sections(buf: &[u8]) -> Result<Vec<u8>, WasmParseError> {
    rewrite_wasm(buf, |section| !is_custom_section(section))
}

pub fn wasm_optimize_size(buf: &[u8]) -> Result<Vec<u8>, WasmParseError> {
    wasm_strip_custom_sections(buf)
}

pub fn wasm_split_data_segments(buf: &[u8]) -> Result<WasmDataSplitResult, WasmParseError> {
    let sections = read_wasm_sections(buf)?;
    let Some(data_section) = sections.iter().find(|section| section.type_id == 11) else {
        return Ok(WasmDataSplitResult {
            primary_wasm: buf.to_vec(),
            split_data: Vec::new(),
            segment_count: 0,
        });
    };

    let segments = parse_data_segments(buf, data_section)?;
    let rewritten_payload = encode_rewritten_data_section(&segments)?;
    let primary_wasm = rewrite_wasm_section(buf, 11, &rewritten_payload)?;
    let split_data = encode_split_data(&segments);

    Ok(WasmDataSplitResult {
        primary_wasm,
        split_data,
        segment_count: segments.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom_section(name: &str, payload: &[u8]) -> Vec<u8> {
        let mut data = encode_var_u32(name.len() as u32);
        data.extend_from_slice(name.as_bytes());
        data.extend_from_slice(payload);

        let mut out = vec![0];
        out.extend_from_slice(&encode_var_u32(data.len() as u32));
        out.extend_from_slice(&data);
        out
    }

    fn standard_type_section() -> Vec<u8> {
        vec![1, 4, 1, 0x60, 0, 0]
    }

    fn memory_section() -> Vec<u8> {
        vec![5, 3, 1, 0, 1]
    }

    fn data_section(offset: u8, bytes: &[u8]) -> Vec<u8> {
        let mut payload = encode_var_u32(1);
        payload.extend_from_slice(&active_data_segment(offset, bytes));
        let mut section = vec![11];
        section.extend_from_slice(&encode_var_u32(payload.len() as u32));
        section.extend_from_slice(&payload);
        section
    }

    fn active_data_segment(offset: u8, bytes: &[u8]) -> Vec<u8> {
        let mut out = vec![0, 0x41, offset, 0x0b];
        out.extend_from_slice(&encode_var_u32(bytes.len() as u32));
        out.extend_from_slice(bytes);
        out
    }

    fn active_data_segment_with_memory(offset: u8, bytes: &[u8]) -> Vec<u8> {
        let mut out = vec![2, 0, 0x41, offset, 0x0b];
        out.extend_from_slice(&encode_var_u32(bytes.len() as u32));
        out.extend_from_slice(bytes);
        out
    }

    fn data_count_section(count: u8) -> Vec<u8> {
        vec![12, 1, count]
    }

    fn passive_data_segment(bytes: &[u8]) -> Vec<u8> {
        let mut out = vec![1];
        out.extend_from_slice(&encode_var_u32(bytes.len() as u32));
        out.extend_from_slice(bytes);
        out
    }

    fn raw_data_section(segments: &[Vec<u8>]) -> Vec<u8> {
        let mut payload = encode_var_u32(segments.len() as u32);
        for segment in segments {
            payload.extend_from_slice(segment);
        }
        let mut section = vec![11];
        section.extend_from_slice(&encode_var_u32(payload.len() as u32));
        section.extend_from_slice(&payload);
        section
    }

    fn wasm_with_sections(sections: &[Vec<u8>]) -> Vec<u8> {
        let mut wasm = vec![0, 97, 115, 109, 1, 0, 0, 0];
        for section in sections {
            wasm.extend_from_slice(section);
        }
        wasm
    }

    fn decode_split_data(bytes: &[u8]) -> Vec<WasmDataSegment> {
        assert_eq!(&bytes[..4], b"MPDS");
        let version = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        assert_eq!(version, SPLIT_DATA_VERSION_V2);
        let count = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let mut i = 12;
        let mut out = Vec::new();
        for _ in 0..count {
            let kind = bytes[i];
            i += 1;
            let memory_index = u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
            i += 4;
            let offset = u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
            i += 4;
            let len = u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap()) as usize;
            i += 4;
            let kind = match kind {
                0 => WasmDataSegmentKind::Active {
                    memory_index,
                    offset,
                },
                1 => WasmDataSegmentKind::Passive,
                _ => panic!("unexpected split segment kind"),
            };
            out.push(WasmDataSegment {
                kind,
                bytes: bytes[i..i + len].to_vec(),
            });
            i += len;
        }
        out
    }

    #[test]
    fn strip_debug_removes_only_debug_custom_sections() {
        let debug = custom_section(".debug_info", &[0xaa, 0xbb]);
        let other = custom_section("producers", &[0x01]);
        let ty = standard_type_section();
        let wasm = wasm_with_sections(&[debug, other.clone(), ty.clone()]);

        let stripped = wasm_strip_debug(&wasm).unwrap();
        assert_eq!(stripped, wasm_with_sections(&[other, ty]));
    }

    #[test]
    fn optimize_size_removes_all_custom_sections() {
        let debug = custom_section(".debug_info", &[0xaa, 0xbb]);
        let other = custom_section("name", &[0x01, 0x02]);
        let ty = standard_type_section();
        let wasm = wasm_with_sections(&[debug, ty.clone(), other]);

        let optimized = wasm_optimize_size(&wasm).unwrap();
        assert_eq!(optimized, wasm_with_sections(&[ty]));
    }

    #[test]
    fn optimize_size_preserves_standard_sections_byte_for_byte() {
        let ty = standard_type_section();
        let wasm = wasm_with_sections(std::slice::from_ref(&ty));

        let optimized = wasm_optimize_size(&wasm).unwrap();
        assert_eq!(optimized, wasm);
    }

    #[test]
    fn strip_custom_sections_removes_all_custom_sections() {
        let debug = custom_section(".debug_info", &[0xaa, 0xbb]);
        let other = custom_section("producers", &[0x01]);
        let ty = standard_type_section();
        let wasm = wasm_with_sections(&[debug, other, ty.clone()]);

        let stripped = wasm_strip_custom_sections(&wasm).unwrap();
        assert_eq!(stripped, wasm_with_sections(&[ty]));
    }

    #[test]
    fn optimize_size_rejects_malformed_wasm() {
        let malformed = vec![0, 97, 115, 109, 1, 0, 0, 0, 1, 5, 1];
        assert_eq!(wasm_optimize_size(&malformed), Err(WasmParseError));
    }

    #[test]
    fn size_report_tracks_stripped_and_optimized_sizes() {
        let debug = custom_section(".debug_info", &[0xaa, 0xbb]);
        let other = custom_section("producers", &[0x01]);
        let ty = standard_type_section();
        let wasm = wasm_with_sections(&[debug.clone(), other.clone(), ty]);

        let report = wasm_size_report(&wasm).unwrap();
        assert_eq!(report.original_bytes, wasm.len());
        assert_eq!(report.stripped_bytes, wasm.len() - (debug.len()));
        assert_eq!(
            report.optimized_bytes,
            wasm.len() - (debug.len() + other.len())
        );
        assert_eq!(report.debug_sections.total_bytes, debug.len());
        assert_eq!(
            report.custom_sections.total_bytes,
            debug.len() + other.len()
        );
    }

    #[test]
    fn split_data_segments_extracts_data_section() {
        let ty = standard_type_section();
        let mem = memory_section();
        let data = data_section(7, &[1, 2, 3, 4]);
        let wasm = wasm_with_sections(&[ty.clone(), mem.clone(), data]);

        let split = wasm_split_data_segments(&wasm).unwrap();
        assert_eq!(split.segment_count, 1);
        assert_eq!(
            decode_split_data(&split.split_data),
            vec![WasmDataSegment {
                kind: WasmDataSegmentKind::Active {
                    memory_index: 0,
                    offset: 7,
                },
                bytes: vec![1, 2, 3, 4],
            }]
        );
        assert_eq!(
            split.primary_wasm,
            wasm_with_sections(&[ty, mem, data_section(7, &[])])
        );
    }

    #[test]
    fn split_data_segments_preserves_passive_segments_and_data_count() {
        let ty = standard_type_section();
        let mem = memory_section();
        let data_count = data_count_section(3);
        let data = raw_data_section(&[
            passive_data_segment(&[9, 9]),
            active_data_segment_with_memory(7, &[1, 2, 3, 4]),
            passive_data_segment(&[5]),
        ]);
        let wasm = wasm_with_sections(&[ty.clone(), mem.clone(), data_count.clone(), data]);

        let split = wasm_split_data_segments(&wasm).unwrap();
        assert_eq!(split.segment_count, 3);
        assert_eq!(
            decode_split_data(&split.split_data),
            vec![
                WasmDataSegment {
                    kind: WasmDataSegmentKind::Passive,
                    bytes: vec![9, 9],
                },
                WasmDataSegment {
                    kind: WasmDataSegmentKind::Active {
                        memory_index: 0,
                        offset: 7,
                    },
                    bytes: vec![1, 2, 3, 4],
                },
                WasmDataSegment {
                    kind: WasmDataSegmentKind::Passive,
                    bytes: vec![5],
                },
            ]
        );
        assert_eq!(
            split.primary_wasm,
            wasm_with_sections(&[
                ty,
                mem,
                data_count,
                raw_data_section(&[
                    passive_data_segment(&[]),
                    active_data_segment(7, &[]),
                    passive_data_segment(&[]),
                ]),
            ])
        );
    }
}
