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
}

#[derive(Clone, Debug)]
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

#[cfg(test)]
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

fn wasm_strip_custom_sections(buf: &[u8]) -> Result<Vec<u8>, WasmParseError> {
    rewrite_wasm(buf, |section| !is_custom_section(section))
}

pub fn wasm_optimize_size(buf: &[u8]) -> Result<Vec<u8>, WasmParseError> {
    wasm_strip_custom_sections(buf)
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

    fn wasm_with_sections(sections: &[Vec<u8>]) -> Vec<u8> {
        let mut wasm = vec![0, 97, 115, 109, 1, 0, 0, 0];
        for section in sections {
            wasm.extend_from_slice(section);
        }
        wasm
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
}
