use std::{collections::{BTreeMap, BTreeSet}, mem};

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

    fn read_var_i64(&mut self) -> Result<i64, WasmParseError> {
        let mut result = 0i64;
        let mut shift = 0;
        let mut byte;

        loop {
            byte = self.read_u8()?;
            result |= ((byte & 0x7f) as i64) << shift;
            shift += 7;
            if (byte & 0x80) == 0 {
                break;
            }
            if shift >= 70 {
                return Err(WasmParseError);
            }
        }

        if shift < 64 && (byte & 0x40) != 0 {
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

// ---------------------------------------------------------------------------
// Function splitting: Binaryen-style module splitting
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct WasmFuncType {
    params: Vec<u8>,
    #[allow(dead_code)]
    results: Vec<u8>,
}

#[derive(Clone, Debug)]
struct WasmLimits {
    has_max: bool,
    min: u32,
    max: u32,
}

#[derive(Clone, Debug)]
struct WasmImport {
    module: String,
    field: String,
    kind: u8,
    descriptor: Vec<u8>,
}

#[derive(Clone, Debug)]
struct WasmTableDef {
    reftype: u8,
    limits: WasmLimits,
}

#[derive(Clone, Debug)]
struct WasmMemoryDef {
    limits: WasmLimits,
}

#[derive(Clone, Debug)]
struct WasmGlobalDef {
    valtype: u8,
    mutable: u8,
}

#[derive(Clone, Debug)]
struct WasmExport {
    name: String,
    kind: u8,
    index: u32,
}

/// Byte range [start..end) in the original buffer for a function body entry
/// (includes the body_size prefix).
#[derive(Clone, Debug)]
struct WasmCodeBody {
    start: usize,
    end: usize,
}

#[derive(Clone, Debug)]
struct WasmModuleInfo {
    types: Vec<WasmFuncType>,
    imports: Vec<WasmImport>,
    num_func_imports: u32,
    num_table_imports: u32,
    num_memory_imports: u32,
    num_global_imports: u32,
    func_type_indices: Vec<u32>,
    tables: Vec<WasmTableDef>,
    memories: Vec<WasmMemoryDef>,
    globals: Vec<WasmGlobalDef>,
    exports: Vec<WasmExport>,
    code_bodies: Vec<WasmCodeBody>,
    active_element_func_indices: BTreeSet<u32>,
    start_func_index: Option<u32>,
    sections: Vec<WasmSection>,
}

fn read_limits(reader: &mut Reader<'_>) -> Result<WasmLimits, WasmParseError> {
    let flag = reader.read_u8()?;
    let min = reader.read_var_u32()?;
    let (has_max, max) = if flag & 1 != 0 {
        (true, reader.read_var_u32()?)
    } else {
        (false, 0)
    };
    Ok(WasmLimits { has_max, min, max })
}

fn encode_limits(limits: &WasmLimits) -> Vec<u8> {
    let mut out = Vec::new();
    if limits.has_max {
        out.push(1);
        out.extend_from_slice(&encode_var_u32(limits.min));
        out.extend_from_slice(&encode_var_u32(limits.max));
    } else {
        out.push(0);
        out.extend_from_slice(&encode_var_u32(limits.min));
    }
    out
}

fn read_string(reader: &mut Reader<'_>) -> Result<String, WasmParseError> {
    let len = reader.read_var_u32()? as usize;
    let bytes = reader.read_vec(len)?;
    String::from_utf8(bytes).map_err(|_| WasmParseError)
}

fn encode_string(s: &str) -> Vec<u8> {
    let mut out = encode_var_u32(s.len() as u32);
    out.extend_from_slice(s.as_bytes());
    out
}

fn parse_type_section(buf: &[u8], section: &WasmSection) -> Result<Vec<WasmFuncType>, WasmParseError> {
    let mut reader = Reader::new(&buf[section.payload_start..section.end]);
    let count = reader.read_var_u32()? as usize;
    let mut types = Vec::with_capacity(count);
    for _ in 0..count {
        if reader.read_u8()? != 0x60 {
            return Err(WasmParseError);
        }
        let param_count = reader.read_var_u32()? as usize;
        let params = reader.read_vec(param_count)?;
        let result_count = reader.read_var_u32()? as usize;
        let results = reader.read_vec(result_count)?;
        types.push(WasmFuncType { params, results });
    }
    Ok(types)
}

fn parse_import_section(buf: &[u8], section: &WasmSection) -> Result<Vec<WasmImport>, WasmParseError> {
    let mut reader = Reader::new(&buf[section.payload_start..section.end]);
    let count = reader.read_var_u32()? as usize;
    let mut imports = Vec::with_capacity(count);
    for _ in 0..count {
        let module = read_string(&mut reader)?;
        let field = read_string(&mut reader)?;
        let kind = reader.read_u8()?;
        let desc_start = reader.offset;
        match kind {
            0 => { reader.read_var_u32()?; } // func: type index
            1 => { reader.read_u8()?; read_limits(&mut reader)?; } // table: reftype + limits
            2 => { read_limits(&mut reader)?; } // memory: limits
            3 => { reader.read_u8()?; reader.read_u8()?; } // global: valtype + mut
            _ => return Err(WasmParseError),
        }
        let desc_end = reader.offset;
        let desc_bytes = buf[section.payload_start + desc_start..section.payload_start + desc_end].to_vec();
        imports.push(WasmImport { module, field, kind, descriptor: desc_bytes });
    }
    Ok(imports)
}

fn parse_function_section(buf: &[u8], section: &WasmSection) -> Result<Vec<u32>, WasmParseError> {
    let mut reader = Reader::new(&buf[section.payload_start..section.end]);
    let count = reader.read_var_u32()? as usize;
    let mut indices = Vec::with_capacity(count);
    for _ in 0..count {
        indices.push(reader.read_var_u32()?);
    }
    Ok(indices)
}

fn parse_table_section(buf: &[u8], section: &WasmSection) -> Result<Vec<WasmTableDef>, WasmParseError> {
    let mut reader = Reader::new(&buf[section.payload_start..section.end]);
    let count = reader.read_var_u32()? as usize;
    let mut tables = Vec::with_capacity(count);
    for _ in 0..count {
        let reftype = reader.read_u8()?;
        let limits = read_limits(&mut reader)?;
        tables.push(WasmTableDef { reftype, limits });
    }
    Ok(tables)
}

fn parse_memory_section(buf: &[u8], section: &WasmSection) -> Result<Vec<WasmMemoryDef>, WasmParseError> {
    let mut reader = Reader::new(&buf[section.payload_start..section.end]);
    let count = reader.read_var_u32()? as usize;
    let mut memories = Vec::with_capacity(count);
    for _ in 0..count {
        let limits = read_limits(&mut reader)?;
        memories.push(WasmMemoryDef { limits });
    }
    Ok(memories)
}

fn parse_global_section(buf: &[u8], section: &WasmSection) -> Result<Vec<WasmGlobalDef>, WasmParseError> {
    let mut reader = Reader::new(&buf[section.payload_start..section.end]);
    let count = reader.read_var_u32()? as usize;
    let mut globals = Vec::with_capacity(count);
    for _ in 0..count {
        let valtype = reader.read_u8()?;
        let mutable = reader.read_u8()?;
        // Skip init_expr: scan for end byte (0x0b) at top block level.
        // MVP init_exprs have no nested blocks.
        loop {
            let byte = reader.read_u8()?;
            if byte == 0x0b {
                break;
            }
            // Skip operands of common init_expr instructions
            match byte {
                0x41 => { reader.read_var_i32()?; } // i32.const
                0x42 => { // i64.const (var_i64)
                    loop {
                        let b = reader.read_u8()?;
                        if b & 0x80 == 0 { break; }
                    }
                }
                0x43 => { reader.skip(4)?; } // f32.const
                0x44 => { reader.skip(8)?; } // f64.const
                0x23 => { reader.read_var_u32()?; } // global.get
                0xD2 => { reader.read_var_u32()?; } // ref.func
                0xD0 => { reader.read_u8()?; } // ref.null
                _ => {} // unknown, hope it has no operands
            }
        }
        globals.push(WasmGlobalDef { valtype, mutable });
    }
    Ok(globals)
}

fn parse_export_section(buf: &[u8], section: &WasmSection) -> Result<Vec<WasmExport>, WasmParseError> {
    let mut reader = Reader::new(&buf[section.payload_start..section.end]);
    let count = reader.read_var_u32()? as usize;
    let mut exports = Vec::with_capacity(count);
    for _ in 0..count {
        let name = read_string(&mut reader)?;
        let kind = reader.read_u8()?;
        let index = reader.read_var_u32()?;
        exports.push(WasmExport { name, kind, index });
    }
    Ok(exports)
}

fn parse_expr_func_refs(reader: &mut Reader<'_>) -> Result<BTreeSet<u32>, WasmParseError> {
    let mut refs = BTreeSet::new();
    loop {
        let opcode = reader.read_u8()?;
        if opcode == 0x0b {
            break;
        }
        match opcode {
            0x41 => {
                reader.read_var_i32()?;
            }
            0x42 => {
                reader.read_var_i64()?;
            }
            0x43 => {
                reader.skip(4)?;
            }
            0x44 => {
                reader.skip(8)?;
            }
            0x23 => {
                reader.read_var_u32()?;
            }
            0xd0 => {
                reader.read_var_i32()?;
            }
            0xd2 => {
                refs.insert(reader.read_var_u32()?);
            }
            _ => {}
        }
    }
    Ok(refs)
}

fn parse_element_section(buf: &[u8], section: &WasmSection) -> Result<BTreeSet<u32>, WasmParseError> {
    let mut reader = Reader::new(&buf[section.payload_start..section.end]);
    let count = reader.read_var_u32()? as usize;
    let mut refs = BTreeSet::new();
    for _ in 0..count {
        match reader.read_var_u32()? {
            0 => {
                let _ = parse_expr_func_refs(&mut reader)?;
                let item_count = reader.read_var_u32()? as usize;
                for _ in 0..item_count {
                    refs.insert(reader.read_var_u32()?);
                }
            }
            1 => {
                reader.read_u8()?;
                let item_count = reader.read_var_u32()? as usize;
                for _ in 0..item_count {
                    reader.read_var_u32()?;
                }
            }
            2 => {
                reader.read_var_u32()?;
                let _ = parse_expr_func_refs(&mut reader)?;
                reader.read_u8()?;
                let item_count = reader.read_var_u32()? as usize;
                for _ in 0..item_count {
                    refs.insert(reader.read_var_u32()?);
                }
            }
            3 => {
                reader.read_u8()?;
                let item_count = reader.read_var_u32()? as usize;
                for _ in 0..item_count {
                    reader.read_var_u32()?;
                }
            }
            4 => {
                let _ = parse_expr_func_refs(&mut reader)?;
                let item_count = reader.read_var_u32()? as usize;
                for _ in 0..item_count {
                    refs.extend(parse_expr_func_refs(&mut reader)?);
                }
            }
            5 => {
                reader.read_u8()?;
                let item_count = reader.read_var_u32()? as usize;
                for _ in 0..item_count {
                    let _ = parse_expr_func_refs(&mut reader)?;
                }
            }
            6 => {
                reader.read_var_u32()?;
                let _ = parse_expr_func_refs(&mut reader)?;
                reader.read_u8()?;
                let item_count = reader.read_var_u32()? as usize;
                for _ in 0..item_count {
                    refs.extend(parse_expr_func_refs(&mut reader)?);
                }
            }
            7 => {
                reader.read_u8()?;
                let item_count = reader.read_var_u32()? as usize;
                for _ in 0..item_count {
                    let _ = parse_expr_func_refs(&mut reader)?;
                }
            }
            _ => return Err(WasmParseError),
        }
    }
    Ok(refs)
}

fn parse_code_bodies(buf: &[u8], section: &WasmSection) -> Result<Vec<WasmCodeBody>, WasmParseError> {
    let mut reader = Reader::new(&buf[section.payload_start..section.end]);
    let count = reader.read_var_u32()? as usize;
    let mut bodies = Vec::with_capacity(count);
    for _ in 0..count {
        let body_start = section.payload_start + reader.offset;
        let body_size = reader.read_var_u32()? as usize;
        let content_start = section.payload_start + reader.offset;
        reader.skip(body_size)?;
        let _ = content_start; // body bytes are at content_start..content_start+body_size
        bodies.push(WasmCodeBody {
            start: body_start,
            end: section.payload_start + reader.offset,
        });
    }
    Ok(bodies)
}

fn parse_wasm_module_info(buf: &[u8]) -> Result<WasmModuleInfo, WasmParseError> {
    let sections = read_wasm_sections(buf)?;
    let mut info = WasmModuleInfo {
        types: Vec::new(),
        imports: Vec::new(),
        num_func_imports: 0,
        num_table_imports: 0,
        num_memory_imports: 0,
        num_global_imports: 0,
        func_type_indices: Vec::new(),
        tables: Vec::new(),
        memories: Vec::new(),
        globals: Vec::new(),
        exports: Vec::new(),
        code_bodies: Vec::new(),
        active_element_func_indices: BTreeSet::new(),
        start_func_index: None,
        sections,
    };

    for section in &info.sections {
        match section.type_id {
            1 => info.types = parse_type_section(buf, section)?,
            2 => info.imports = parse_import_section(buf, section)?,
            3 => info.func_type_indices = parse_function_section(buf, section)?,
            4 => info.tables = parse_table_section(buf, section)?,
            5 => info.memories = parse_memory_section(buf, section)?,
            6 => info.globals = parse_global_section(buf, section)?,
            7 => info.exports = parse_export_section(buf, section)?,
            8 => {
                let mut reader = Reader::new(&buf[section.payload_start..section.end]);
                info.start_func_index = Some(reader.read_var_u32()?);
            }
            9 => info.active_element_func_indices = parse_element_section(buf, section)?,
            10 => info.code_bodies = parse_code_bodies(buf, section)?,
            _ => {}
        }
    }

    // Count imports by kind
    for imp in &info.imports {
        match imp.kind {
            0 => info.num_func_imports += 1,
            1 => info.num_table_imports += 1,
            2 => info.num_memory_imports += 1,
            3 => info.num_global_imports += 1,
            _ => {}
        }
    }

    Ok(info)
}

fn skip_block_type(reader: &mut Reader<'_>) -> Result<(), WasmParseError> {
    let byte = reader.read_u8()?;
    match byte {
        0x40 | 0x7f | 0x7e | 0x7d | 0x7c | 0x7b | 0x70 | 0x6f => Ok(()),
        _ => {
            if byte & 0x80 == 0 {
                Ok(())
            } else {
                while reader.read_u8()? & 0x80 != 0 {}
                Ok(())
            }
        }
    }
}

fn skip_memarg(reader: &mut Reader<'_>) -> Result<(), WasmParseError> {
    reader.read_var_u32()?;
    reader.read_var_u32()?;
    Ok(())
}

fn skip_vec_types(reader: &mut Reader<'_>) -> Result<(), WasmParseError> {
    let count = reader.read_var_u32()? as usize;
    for _ in 0..count {
        reader.read_u8()?;
    }
    Ok(())
}

fn scan_prefixed_fc_instruction(reader: &mut Reader<'_>) -> Result<(), WasmParseError> {
    match reader.read_var_u32()? {
        0..=7 => Ok(()),
        8 => {
            reader.read_var_u32()?;
            reader.read_var_u32()?;
            Ok(())
        }
        9 => {
            reader.read_var_u32()?;
            Ok(())
        }
        10 => {
            reader.read_var_u32()?;
            reader.read_var_u32()?;
            Ok(())
        }
        11 => {
            reader.read_var_u32()?;
            Ok(())
        }
        12 => {
            reader.read_var_u32()?;
            reader.read_var_u32()?;
            Ok(())
        }
        13 => {
            reader.read_var_u32()?;
            Ok(())
        }
        14 => {
            reader.read_var_u32()?;
            reader.read_var_u32()?;
            Ok(())
        }
        15..=17 => {
            reader.read_var_u32()?;
            Ok(())
        }
        _ => Err(WasmParseError),
    }
}

fn scan_prefixed_fd_instruction(reader: &mut Reader<'_>) -> Result<(), WasmParseError> {
    match reader.read_var_u32()? {
        0..=11 | 84..=91 | 92..=99 => skip_memarg(reader),
        12 | 13 => {
            reader.skip(16)?;
            Ok(())
        }
        21 | 22 | 23 | 24 | 25 | 26 | 27 | 28 | 29 | 30 | 31 | 32 | 33 | 34 => {
            reader.read_u8()?;
            Ok(())
        }
        35..=83 | 100..=255 => Ok(()),
        _ => Err(WasmParseError),
    }
}

fn scan_prefixed_fe_instruction(reader: &mut Reader<'_>) -> Result<(), WasmParseError> {
    match reader.read_var_u32()? {
        0..=2 => skip_memarg(reader),
        3 => {
            reader.read_u8()?;
            Ok(())
        }
        16..=30 | 31..=78 => skip_memarg(reader),
        _ => Err(WasmParseError),
    }
}

fn scan_function_body_direct_refs(body: &[u8]) -> Result<BTreeSet<u32>, WasmParseError> {
    let mut reader = Reader::new(body);
    let body_size = reader.read_var_u32()? as usize;
    if body_size != reader.bytes.len() {
        return Err(WasmParseError);
    }

    let local_group_count = reader.read_var_u32()? as usize;
    for _ in 0..local_group_count {
        reader.read_var_u32()?;
        reader.read_u8()?;
    }

    let mut refs = BTreeSet::new();
    while !reader.bytes.is_empty() {
        match reader.read_u8()? {
            0x02 | 0x03 | 0x04 => skip_block_type(&mut reader)?,
            0x0c | 0x0d | 0x20 | 0x21 | 0x22 | 0x23 | 0x24 | 0x25 | 0x26 => {
                reader.read_var_u32()?;
            }
            0x0e => {
                let count = reader.read_var_u32()? as usize;
                for _ in 0..count {
                    reader.read_var_u32()?;
                }
                reader.read_var_u32()?;
            }
            0x10 | 0x12 => {
                refs.insert(reader.read_var_u32()?);
            }
            0x11 | 0x13 => {
                reader.read_var_u32()?;
                reader.read_var_u32()?;
            }
            0x14 => {
                reader.read_var_u32()?;
            }
            0x1c => skip_vec_types(&mut reader)?,
            0x28..=0x3e => skip_memarg(&mut reader)?,
            0x3f | 0x40 => {
                reader.read_var_u32()?;
            }
            0x41 => {
                reader.read_var_i32()?;
            }
            0x42 => {
                reader.read_var_i64()?;
            }
            0x43 => {
                reader.skip(4)?;
            }
            0x44 => {
                reader.skip(8)?;
            }
            0xd0 => {
                reader.read_var_i32()?;
            }
            0xd2 => {
                refs.insert(reader.read_var_u32()?);
            }
            0xfc => scan_prefixed_fc_instruction(&mut reader)?,
            0xfd => scan_prefixed_fd_instruction(&mut reader)?,
            0xfe => scan_prefixed_fe_instruction(&mut reader)?,
            _ => {}
        }
    }

    Ok(refs)
}

fn exported_function_indices(info: &WasmModuleInfo) -> BTreeSet<u32> {
    info.exports
        .iter()
        .filter(|export| export.kind == 0x00)
        .map(|export| export.index)
        .collect()
}

fn startup_hot_function_indices(
    buf: &[u8],
    info: &WasmModuleInfo,
) -> Result<BTreeSet<usize>, WasmParseError> {
    let mut hot = BTreeSet::new();
    let mut queue = Vec::new();

    if let Some(start_idx) = info.start_func_index {
        queue.push(start_idx);
    }
    queue.extend(exported_function_indices(info));
    queue.extend(info.active_element_func_indices.iter().copied());

    while let Some(abs_index) = queue.pop() {
        if abs_index < info.num_func_imports {
            continue;
        }
        let defined_index = (abs_index - info.num_func_imports) as usize;
        if defined_index >= info.code_bodies.len() || !hot.insert(defined_index) {
            continue;
        }

        let refs = scan_function_body_direct_refs(&buf[info.code_bodies[defined_index].start..info.code_bodies[defined_index].end])?;
        for callee in refs {
            if callee >= info.num_func_imports {
                queue.push(callee);
            }
        }
    }

    Ok(hot)
}

// Phase 2: Stub generation

/// Generate a forwarding stub body for a function with the given type.
/// The stub forwards all arguments through call_indirect to the given table slot.
fn generate_stub_body(type_idx: u32, param_count: u32, table_slot: u32) -> Vec<u8> {
    let mut body = Vec::new();
    body.push(0x00); // 0 local declaration groups
    for i in 0..param_count {
        body.push(0x20); // local.get
        body.extend_from_slice(&encode_var_u32(i));
    }
    body.push(0x41); // i32.const
    body.extend_from_slice(&encode_var_i32(table_slot as i32));
    body.push(0x11); // call_indirect
    body.extend_from_slice(&encode_var_u32(type_idx));
    body.push(0x00); // table index 0
    body.push(0x0b); // end
    body
}

// Phase 3: Primary module generation
fn encode_base62(mut num: u32) -> String {
    if num == 0 {
        return "0".to_string();
    }
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut out = String::new();
    while num > 0 {
        out.push(ALPHABET[(num % 62) as usize] as char);
        num /= 62;
    }
    out.chars().rev().collect()
}

fn build_primary_module(
    buf: &[u8],
    info: &WasmModuleInfo,
    split_indices: &[usize],
    table_base_slot: u32,
    num_split: u32,
) -> Result<Vec<u8>, WasmParseError> {
    let mut out = Vec::with_capacity(buf.len());
    out.extend_from_slice(&buf[..8]); // magic + version

    // Build set for quick lookup
    let split_set: std::collections::HashSet<usize> = split_indices.iter().copied().collect();

    // Sections must be emitted in order. Walk through original sections and
    // replace/augment as needed. We may also need to INSERT a table section
    // if the original module has none.
    let mut emitted_table_section = false;
    let has_table_section = info.sections.iter().any(|s| s.type_id == 4);

    for section in &info.sections {
        match section.type_id {
            4 => {
                // Rewrite table section: grow first table by num_split
                emitted_table_section = true;
                let mut payload = Vec::new();
                let total = info.num_table_imports as usize + info.tables.len();
                payload.extend_from_slice(&encode_var_u32(info.tables.len() as u32));
                for (i, table) in info.tables.iter().enumerate() {
                    payload.push(table.reftype);
                    if i == 0 {
                        // First defined table: grow it
                        let new_min = table.limits.min + num_split;
                        let new_limits = WasmLimits {
                            has_max: table.limits.has_max,
                            min: new_min,
                            max: if table.limits.has_max {
                                std::cmp::max(table.limits.max, new_min)
                            } else {
                                0
                            },
                        };
                        payload.extend_from_slice(&encode_limits(&new_limits));
                    } else {
                        payload.extend_from_slice(&encode_limits(&table.limits));
                    }
                    let _ = total; // suppress unused
                }
                out.push(4);
                out.extend_from_slice(&encode_var_u32(payload.len() as u32));
                out.extend_from_slice(&payload);
            }
            7 => {
                // Rewrite export section: append exports for defined funcs, tables, memories, globals
                let mut payload = Vec::new();
                let num_existing = info.exports.len() as u32;
                let num_new_func = info.func_type_indices.len() as u32;
                let num_new_table = info.tables.len() as u32;
                let num_new_mem = info.memories.len() as u32;
                let num_new_global = info.globals.len() as u32;
                let num_split_table = 1u32;
                let total_exports = num_existing + num_new_func + num_new_table + num_new_mem + num_new_global + num_split_table;
                payload.extend_from_slice(&encode_var_u32(total_exports));

                // Existing exports
                for ex in &info.exports {
                    payload.extend_from_slice(&encode_string(&ex.name));
                    payload.push(ex.kind);
                    payload.extend_from_slice(&encode_var_u32(ex.index));
                }

                // Export all defined functions as $f<abs_index>
                for i in 0..info.func_type_indices.len() {
                    let abs_idx = info.num_func_imports + i as u32;
                    let name = format!("$f{}", encode_base62(abs_idx));
                    payload.extend_from_slice(&encode_string(&name));
                    payload.push(0x00); // function
                    payload.extend_from_slice(&encode_var_u32(abs_idx));
                }

                // Export the table used by split stubs so the runtime can patch it later.
                payload.extend_from_slice(&encode_string("$s"));
                payload.push(0x01); // table
                payload.extend_from_slice(&encode_var_u32(0));

                // Export all defined tables as $t<abs_index>
                for i in 0..info.tables.len() {
                    let abs_idx = info.num_table_imports + i as u32;
                    let name = format!("$t{}", encode_base62(abs_idx));
                    payload.extend_from_slice(&encode_string(&name));
                    payload.push(0x01); // table
                    payload.extend_from_slice(&encode_var_u32(abs_idx));
                }

                // Export all defined memories as $m<abs_index>
                for i in 0..info.memories.len() {
                    let abs_idx = info.num_memory_imports + i as u32;
                    let name = format!("$m{}", encode_base62(abs_idx));
                    payload.extend_from_slice(&encode_string(&name));
                    payload.push(0x02); // memory
                    payload.extend_from_slice(&encode_var_u32(abs_idx));
                }

                // Export all defined globals as $g<abs_index>
                for i in 0..info.globals.len() {
                    let abs_idx = info.num_global_imports + i as u32;
                    let name = format!("$g{}", encode_base62(abs_idx));
                    payload.extend_from_slice(&encode_string(&name));
                    payload.push(0x03); // global
                    payload.extend_from_slice(&encode_var_u32(abs_idx));
                }

                out.push(7);
                out.extend_from_slice(&encode_var_u32(payload.len() as u32));
                out.extend_from_slice(&payload);
            }
            10 => {
                // Rewrite code section: replace split function bodies with stubs
                let mut payload = Vec::new();
                payload.extend_from_slice(&encode_var_u32(info.code_bodies.len() as u32));

                for (i, body) in info.code_bodies.iter().enumerate() {
                    if split_set.contains(&i) {
                        let split_order = split_indices.iter().position(|&x| x == i).unwrap();
                        let type_idx = info.func_type_indices[i];
                        let param_count = info.types[type_idx as usize].params.len() as u32;
                        let slot = table_base_slot + split_order as u32;
                        let stub = generate_stub_body(type_idx, param_count, slot);
                        payload.extend_from_slice(&encode_var_u32(stub.len() as u32));
                        payload.extend_from_slice(&stub);
                    } else {
                        // Copy original body verbatim
                        payload.extend_from_slice(&buf[body.start..body.end]);
                    }
                }

                out.push(10);
                out.extend_from_slice(&encode_var_u32(payload.len() as u32));
                out.extend_from_slice(&payload);
            }
            _ => {
                // Before emitting sections after table (type_id > 4), insert table
                // section if original had none and we need one.
                if !has_table_section && !emitted_table_section && section.type_id > 4 {
                    emitted_table_section = true;
                    // Insert a new funcref table
                    let mut payload = Vec::new();
                    payload.extend_from_slice(&encode_var_u32(1)); // 1 table
                    payload.push(0x70); // funcref
                    let limits = WasmLimits { has_max: true, min: num_split, max: num_split };
                    payload.extend_from_slice(&encode_limits(&limits));
                    out.push(4);
                    out.extend_from_slice(&encode_var_u32(payload.len() as u32));
                    out.extend_from_slice(&payload);
                }
                // Copy section verbatim
                out.extend_from_slice(&buf[section.start..section.end]);
            }
        }
    }

    Ok(out)
}

// Phase 4: Secondary module generation

fn build_secondary_module(
    buf: &[u8],
    info: &WasmModuleInfo,
    split_indices: &[usize],
    table_base_slot: u32,
) -> Result<Vec<u8>, WasmParseError> {
    let num_defined = info.func_type_indices.len() as u32;
    let num_split = split_indices.len() as u32;

    let mut out = Vec::new();
    // Magic + version
    out.extend_from_slice(&[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]);

    // 1. Type section: copy verbatim from original
    if let Some(section) = info.sections.iter().find(|s| s.type_id == 1) {
        out.extend_from_slice(&buf[section.start..section.end]);
    }

    // 2. Import section: all original imports + all primary defined items
    {
        let mut payload = Vec::new();

        // Count total imports
        let num_func_imports_secondary = info.num_func_imports + num_defined;
        let num_table_imports_secondary = info.num_table_imports + info.tables.len() as u32;
        let num_memory_imports_secondary = info.num_memory_imports + info.memories.len() as u32;
        let num_global_imports_secondary = info.num_global_imports + info.globals.len() as u32;
        let total_imports = num_func_imports_secondary
            + num_table_imports_secondary
            + num_memory_imports_secondary
            + num_global_imports_secondary;
        payload.extend_from_slice(&encode_var_u32(total_imports));

        // Original function imports (preserve func indices 0..num_func_imports-1)
        for imp in &info.imports {
            if imp.kind == 0 {
                payload.extend_from_slice(&encode_string(&imp.module));
                payload.extend_from_slice(&encode_string(&imp.field));
                payload.push(0x00);
                payload.extend_from_slice(&imp.descriptor);
            }
        }

        // Primary defined functions (preserve func indices num_func_imports..num_func_imports+num_defined-1)
        for i in 0..num_defined {
            let abs_idx = info.num_func_imports + i;
            let name = format!("$f{}", encode_base62(abs_idx));
            payload.extend_from_slice(&encode_string("$p"));
            payload.extend_from_slice(&encode_string(&name));
            payload.push(0x00); // function
            payload.extend_from_slice(&encode_var_u32(info.func_type_indices[i as usize]));
        }

        // Original table imports
        for imp in &info.imports {
            if imp.kind == 1 {
                payload.extend_from_slice(&encode_string(&imp.module));
                payload.extend_from_slice(&encode_string(&imp.field));
                payload.push(0x01);
                payload.extend_from_slice(&imp.descriptor);
            }
        }

        // Primary defined tables
        for i in 0..info.tables.len() {
            let abs_idx = info.num_table_imports + i as u32;
            let name = format!("$t{}", encode_base62(abs_idx));
            let table = &info.tables[i];
            payload.extend_from_slice(&encode_string("$p"));
            payload.extend_from_slice(&encode_string(&name));
            payload.push(0x01); // table
            payload.push(table.reftype);
            // Import with the grown limits (primary grew the table)
            let grown_limits = if i == 0 {
                WasmLimits {
                    has_max: table.limits.has_max,
                    min: table.limits.min + num_split,
                    max: if table.limits.has_max {
                        std::cmp::max(table.limits.max, table.limits.min + num_split)
                    } else {
                        0
                    },
                }
            } else {
                table.limits.clone()
            };
            payload.extend_from_slice(&encode_limits(&grown_limits));
        }

        // Original memory imports
        for imp in &info.imports {
            if imp.kind == 2 {
                payload.extend_from_slice(&encode_string(&imp.module));
                payload.extend_from_slice(&encode_string(&imp.field));
                payload.push(0x02);
                payload.extend_from_slice(&imp.descriptor);
            }
        }

        // Primary defined memories
        for i in 0..info.memories.len() {
            let abs_idx = info.num_memory_imports + i as u32;
            let name = format!("$m{}", encode_base62(abs_idx));
            payload.extend_from_slice(&encode_string("$p"));
            payload.extend_from_slice(&encode_string(&name));
            payload.push(0x02); // memory
            payload.extend_from_slice(&encode_limits(&info.memories[i].limits));
        }

        // Original global imports
        for imp in &info.imports {
            if imp.kind == 3 {
                payload.extend_from_slice(&encode_string(&imp.module));
                payload.extend_from_slice(&encode_string(&imp.field));
                payload.push(0x03);
                payload.extend_from_slice(&imp.descriptor);
            }
        }

        // Primary defined globals
        for i in 0..info.globals.len() {
            let abs_idx = info.num_global_imports + i as u32;
            let name = format!("$g{}", encode_base62(abs_idx));
            let global = &info.globals[i];
            payload.extend_from_slice(&encode_string("$p"));
            payload.extend_from_slice(&encode_string(&name));
            payload.push(0x03); // global
            payload.push(global.valtype);
            payload.push(global.mutable);
        }

        out.push(2); // import section
        out.extend_from_slice(&encode_var_u32(payload.len() as u32));
        out.extend_from_slice(&payload);
    }

    // 3. Function section: K entries
    {
        let mut payload = Vec::new();
        payload.extend_from_slice(&encode_var_u32(num_split));
        for &def_idx in split_indices {
            payload.extend_from_slice(&encode_var_u32(info.func_type_indices[def_idx]));
        }
        out.push(3);
        out.extend_from_slice(&encode_var_u32(payload.len() as u32));
        out.extend_from_slice(&payload);
    }

    // 9. Export section: expose split functions by the table slot they should patch.
    {
        let mut payload = Vec::new();
        payload.extend_from_slice(&encode_var_u32(num_split));

        let secondary_func_import_count = info.num_func_imports + num_defined;
        for i in 0..num_split {
            let slot = table_base_slot + i;
            let name = format!("$s{}", slot);
            payload.extend_from_slice(&encode_string(&name));
            payload.push(0x00); // function
            payload.extend_from_slice(&encode_var_u32(secondary_func_import_count + i));
        }

        out.push(7); // export section
        out.extend_from_slice(&encode_var_u32(payload.len() as u32));
        out.extend_from_slice(&payload);
    }

    // 10. Code section: K original function bodies verbatim
    {
        let mut payload = Vec::new();
        payload.extend_from_slice(&encode_var_u32(num_split));
        for &def_idx in split_indices {
            let body = &info.code_bodies[def_idx];
            payload.extend_from_slice(&buf[body.start..body.end]);
        }
        out.push(10); // code section
        out.extend_from_slice(&encode_var_u32(payload.len() as u32));
        out.extend_from_slice(&payload);
    }

    Ok(out)
}

// Phase 5: Public API

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WasmFunctionSplitResult {
    pub primary_wasm: Vec<u8>,
    pub secondary_wasm: Vec<u8>,
    pub split_count: usize,
    pub total_functions: usize,
}

fn empty_function_split_result(buf: &[u8], total_functions: usize) -> WasmFunctionSplitResult {
    WasmFunctionSplitResult {
        primary_wasm: buf.to_vec(),
        secondary_wasm: Vec::new(),
        split_count: 0,
        total_functions,
    }
}

fn selectable_function_indices(info: &WasmModuleInfo) -> Vec<usize> {
    let mut split_indices = Vec::new();
    for i in 0..info.code_bodies.len() {
        if let Some(start_idx) = info.start_func_index {
            if start_idx == info.num_func_imports + i as u32 {
                continue;
            }
        }
        split_indices.push(i);
    }
    split_indices
}

fn build_function_split_result(
    buf: &[u8],
    info: &WasmModuleInfo,
    split_indices: &[usize],
) -> Result<WasmFunctionSplitResult, WasmParseError> {
    let num_defined = info.func_type_indices.len();
    if split_indices.is_empty() {
        return Ok(empty_function_split_result(buf, num_defined));
    }

    let num_split = split_indices.len() as u32;
    let table_base_slot = if let Some(table) = info.tables.first() {
        table.limits.min
    } else {
        0
    };

    let primary_wasm = build_primary_module(buf, info, split_indices, table_base_slot, num_split)?;
    let secondary_wasm = build_secondary_module(buf, info, split_indices, table_base_slot)?;

    Ok(WasmFunctionSplitResult {
        primary_wasm,
        secondary_wasm,
        split_count: split_indices.len(),
        total_functions: num_defined,
    })
}

pub fn wasm_split_functions(
    buf: &[u8],
    threshold: usize,
) -> Result<WasmFunctionSplitResult, WasmParseError> {
    let info = parse_wasm_module_info(buf)?;
    let num_defined = info.func_type_indices.len();

    // Select functions to split based on body size threshold
    let mut split_indices = Vec::new();
    for i in selectable_function_indices(&info) {
        let body = &info.code_bodies[i];
        let body_size = body.end - body.start;
        if body_size >= threshold {
            split_indices.push(i);
        }
    }

    if split_indices.is_empty() {
        return Ok(empty_function_split_result(buf, num_defined));
    }

    build_function_split_result(buf, &info, &split_indices)
}

pub fn wasm_split_functions_to_target_primary_size(
    buf: &[u8],
    target_primary_bytes: usize,
) -> Result<WasmFunctionSplitResult, WasmParseError> {
    let info = parse_wasm_module_info(buf)?;
    let num_defined = info.func_type_indices.len();

    if buf.len() <= target_primary_bytes {
        return Ok(empty_function_split_result(buf, num_defined));
    }

    let mut ranked = selectable_function_indices(&info);
    if ranked.is_empty() {
        return Ok(empty_function_split_result(buf, num_defined));
    }
    ranked.sort_unstable_by(|&a, &b| {
        let size_a = info.code_bodies[a].end - info.code_bodies[a].start;
        let size_b = info.code_bodies[b].end - info.code_bodies[b].start;
        size_b.cmp(&size_a).then_with(|| a.cmp(&b))
    });

    let table_base_slot = info.tables.first().map(|table| table.limits.min).unwrap_or(0);
    let primary_len_for_count = |count: usize| -> Result<usize, WasmParseError> {
        let mut split_indices = ranked[..count].to_vec();
        split_indices.sort_unstable();
        let primary = build_primary_module(
            buf,
            &info,
            &split_indices,
            table_base_slot,
            split_indices.len() as u32,
        )?;
        Ok(primary.len())
    };

    let chosen_count = if primary_len_for_count(ranked.len())? > target_primary_bytes {
        ranked.len()
    } else {
        let mut low = 1usize;
        let mut high = ranked.len();
        let mut best = ranked.len();
        while low <= high {
            let mid = low + (high - low) / 2;
            if primary_len_for_count(mid)? <= target_primary_bytes {
                best = mid;
                if mid == 1 {
                    break;
                }
                high = mid - 1;
            } else {
                low = mid + 1;
            }
        }
        best
    };

    let mut split_indices = ranked[..chosen_count].to_vec();
    split_indices.sort_unstable();
    build_function_split_result(buf, &info, &split_indices)
}

pub fn wasm_split_functions_to_target_primary_size_cold(
    buf: &[u8],
    target_primary_bytes: usize,
) -> Result<WasmFunctionSplitResult, WasmParseError> {
    let info = parse_wasm_module_info(buf)?;
    let num_defined = info.func_type_indices.len();

    if buf.len() <= target_primary_bytes {
        return Ok(empty_function_split_result(buf, num_defined));
    }

    let startup_hot = startup_hot_function_indices(buf, &info)?;
    let mut ranked = selectable_function_indices(&info)
        .into_iter()
        .filter(|index| !startup_hot.contains(index))
        .collect::<Vec<_>>();
    if ranked.is_empty() {
        return Ok(empty_function_split_result(buf, num_defined));
    }

    ranked.sort_unstable_by(|&a, &b| {
        let size_a = info.code_bodies[a].end - info.code_bodies[a].start;
        let size_b = info.code_bodies[b].end - info.code_bodies[b].start;
        size_b.cmp(&size_a).then_with(|| a.cmp(&b))
    });

    let table_base_slot = info.tables.first().map(|table| table.limits.min).unwrap_or(0);
    let primary_len_for_count = |count: usize| -> Result<usize, WasmParseError> {
        let mut split_indices = ranked[..count].to_vec();
        split_indices.sort_unstable();
        let primary = build_primary_module(
            buf,
            &info,
            &split_indices,
            table_base_slot,
            split_indices.len() as u32,
        )?;
        Ok(primary.len())
    };

    let mut chosen_count = ranked.len();
    if primary_len_for_count(ranked.len())? <= target_primary_bytes {
        let mut low = 1usize;
        let mut high = ranked.len();
        while low <= high {
            let mid = low + (high - low) / 2;
            if primary_len_for_count(mid)? <= target_primary_bytes {
                chosen_count = mid;
                if mid == 1 {
                    break;
                }
                high = mid - 1;
            } else {
                low = mid + 1;
            }
        }
    }

    let mut split_indices = ranked[..chosen_count].to_vec();
    split_indices.sort_unstable();
    build_function_split_result(buf, &info, &split_indices)
}

pub fn wasm_split_functions_cold(buf: &[u8]) -> Result<WasmFunctionSplitResult, WasmParseError> {
    let info = parse_wasm_module_info(buf)?;
    let num_defined = info.func_type_indices.len();

    let startup_hot = startup_hot_function_indices(buf, &info)?;
    let mut split_indices = selectable_function_indices(&info)
        .into_iter()
        .filter(|index| !startup_hot.contains(index))
        .collect::<Vec<_>>();

    if split_indices.is_empty() {
        return Ok(empty_function_split_result(buf, num_defined));
    }

    split_indices.sort_unstable();
    build_function_split_result(buf, &info, &split_indices)
}

// ---------------------------------------------------------------------------

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

    // --- Function splitting tests ---

    /// Build a type section with given function types.
    /// Each entry is (param_count, result_count) with all types being i32.
    fn type_section(types: &[(u32, u32)]) -> Vec<u8> {
        let mut payload = encode_var_u32(types.len() as u32);
        for &(params, results) in types {
            payload.push(0x60);
            payload.extend_from_slice(&encode_var_u32(params));
            for _ in 0..params {
                payload.push(0x7f); // i32
            }
            payload.extend_from_slice(&encode_var_u32(results));
            for _ in 0..results {
                payload.push(0x7f); // i32
            }
        }
        let mut section = vec![1]; // type section id
        section.extend_from_slice(&encode_var_u32(payload.len() as u32));
        section.extend_from_slice(&payload);
        section
    }

    /// Build a function section mapping defined functions to type indices.
    fn function_section(type_indices: &[u32]) -> Vec<u8> {
        let mut payload = encode_var_u32(type_indices.len() as u32);
        for &idx in type_indices {
            payload.extend_from_slice(&encode_var_u32(idx));
        }
        let mut section = vec![3]; // function section id
        section.extend_from_slice(&encode_var_u32(payload.len() as u32));
        section.extend_from_slice(&payload);
        section
    }

    /// Build a table section with one funcref table.
    fn table_section(initial: u32, max: Option<u32>) -> Vec<u8> {
        let mut payload = encode_var_u32(1); // 1 table
        payload.push(0x70); // funcref
        if let Some(max) = max {
            payload.push(0x01); // has max
            payload.extend_from_slice(&encode_var_u32(initial));
            payload.extend_from_slice(&encode_var_u32(max));
        } else {
            payload.push(0x00); // no max
            payload.extend_from_slice(&encode_var_u32(initial));
        }
        let mut section = vec![4]; // table section id
        section.extend_from_slice(&encode_var_u32(payload.len() as u32));
        section.extend_from_slice(&payload);
        section
    }

    /// Build an export section exporting a function.
    fn export_section(exports: &[(&str, u8, u32)]) -> Vec<u8> {
        let mut payload = encode_var_u32(exports.len() as u32);
        for &(name, kind, index) in exports {
            payload.extend_from_slice(&encode_var_u32(name.len() as u32));
            payload.extend_from_slice(name.as_bytes());
            payload.push(kind);
            payload.extend_from_slice(&encode_var_u32(index));
        }
        let mut section = vec![7]; // export section id
        section.extend_from_slice(&encode_var_u32(payload.len() as u32));
        section.extend_from_slice(&payload);
        section
    }

    /// Build a code section with given function bodies (raw bytes, without size prefix).
    fn code_section(bodies: &[&[u8]]) -> Vec<u8> {
        let mut payload = encode_var_u32(bodies.len() as u32);
        for body in bodies {
            payload.extend_from_slice(&encode_var_u32(body.len() as u32));
            payload.extend_from_slice(body);
        }
        let mut section = vec![10]; // code section id
        section.extend_from_slice(&encode_var_u32(payload.len() as u32));
        section.extend_from_slice(&payload);
        section
    }

    fn active_element_section(function_indices: &[u32]) -> Vec<u8> {
        let mut payload = encode_var_u32(1);
        payload.extend_from_slice(&encode_var_u32(0));
        payload.push(0x41);
        payload.push(0x00);
        payload.push(0x0b);
        payload.extend_from_slice(&encode_var_u32(function_indices.len() as u32));
        for &index in function_indices {
            payload.extend_from_slice(&encode_var_u32(index));
        }
        let mut section = vec![9];
        section.extend_from_slice(&encode_var_u32(payload.len() as u32));
        section.extend_from_slice(&payload);
        section
    }

    #[test]
    fn split_functions_no_functions_above_threshold() {
        // A module with one small function — nothing should be split
        let small_body = &[0x00, 0x0b]; // 0 locals, end
        let wasm = wasm_with_sections(&[
            type_section(&[(0, 0)]),
            function_section(&[0]),
            code_section(&[small_body]),
        ]);

        let result = wasm_split_functions(&wasm, 100).unwrap();
        assert_eq!(result.split_count, 0);
        assert_eq!(result.total_functions, 1);
        assert!(result.secondary_wasm.is_empty());
        assert_eq!(result.primary_wasm, wasm);
    }

    #[test]
    fn split_functions_splits_large_function() {
        // Create a module with:
        // - Type 0: () -> ()  (no params, no results)
        // - Type 1: (i32) -> (i32)
        // - Function 0: type 0, small body
        // - Function 1: type 1, large body (should be split)
        let small_body = &[0x00, 0x0b]; // 0 locals, end
        let mut large_body = vec![0x00]; // 0 locals
        for _ in 0..250 {
            large_body.push(0x01); // nop
        }
        large_body.push(0x20); // local.get
        large_body.push(0x00); // index 0
        large_body.push(0x0b); // end

        let wasm = wasm_with_sections(&[
            type_section(&[(0, 0), (1, 1)]),
            function_section(&[0, 1]),
            table_section(0, Some(0)),
            export_section(&[("main", 0x00, 0)]),
            code_section(&[small_body, &large_body]),
        ]);

        let result = wasm_split_functions(&wasm, 10).unwrap();
        assert_eq!(result.split_count, 1);
        assert_eq!(result.total_functions, 2);
        assert!(!result.secondary_wasm.is_empty());
        assert!(result.primary_wasm.len() < wasm.len());

        // Validate primary is valid WASM
        assert_eq!(&result.primary_wasm[..4], b"\0asm");
        // Validate secondary is valid WASM
        assert_eq!(&result.secondary_wasm[..4], b"\0asm");
    }

    #[test]
    fn split_functions_auto_target_primary_size() {
        let small_body = &[0x00, 0x0b];
        let mut large_body = vec![0x00];
        for _ in 0..250 {
            large_body.push(0x01);
        }
        large_body.push(0x20);
        large_body.push(0x00);
        large_body.push(0x0b);

        let wasm = wasm_with_sections(&[
            type_section(&[(0, 0), (1, 1)]),
            function_section(&[0, 1]),
            table_section(0, Some(0)),
            export_section(&[("main", 0x00, 0)]),
            code_section(&[small_body, &large_body]),
        ]);

        let result = wasm_split_functions_to_target_primary_size(&wasm, wasm.len() - 20).unwrap();
        assert_eq!(result.split_count, 1);
        assert_eq!(result.total_functions, 2);
        assert!(result.primary_wasm.len() < wasm.len());
        assert!(!result.secondary_wasm.is_empty());
    }

    #[test]
    fn split_functions_primary_exports_split_table() {
        let small_body = &[0x00, 0x0b];
        let mut large_body = vec![0x00];
        for _ in 0..250 {
            large_body.push(0x01);
        }
        large_body.push(0x20);
        large_body.push(0x00);
        large_body.push(0x0b);

        let wasm = wasm_with_sections(&[
            type_section(&[(0, 0), (1, 1)]),
            function_section(&[0, 1]),
            table_section(0, Some(0)),
            export_section(&[("main", 0x00, 0)]),
            code_section(&[small_body, &large_body]),
        ]);

        let result = wasm_split_functions(&wasm, 10).unwrap();
        let sections = read_wasm_sections(&result.primary_wasm).unwrap();
        let export_section = sections.iter().find(|section| section.type_id == 7).unwrap();
        let exports = parse_export_section(&result.primary_wasm, export_section).unwrap();
        assert!(exports
            .iter()
            .any(|export| export.name == "$s" && export.kind == 0x01));
    }

    #[test]
    fn split_functions_secondary_exports_patch_slots_without_element_section() {
        let small_body = &[0x00, 0x0b];
        let mut large_body = vec![0x00];
        for _ in 0..250 {
            large_body.push(0x01);
        }
        large_body.push(0x20);
        large_body.push(0x00);
        large_body.push(0x0b);

        let wasm = wasm_with_sections(&[
            type_section(&[(0, 0), (1, 1)]),
            function_section(&[0, 1]),
            table_section(0, Some(0)),
            export_section(&[("main", 0x00, 0)]),
            code_section(&[small_body, &large_body]),
        ]);

        let result = wasm_split_functions(&wasm, 10).unwrap();
        let sections = read_wasm_sections(&result.secondary_wasm).unwrap();
        assert!(!sections.iter().any(|section| section.type_id == 9));

        let export_section = sections.iter().find(|section| section.type_id == 7).unwrap();
        let exports = parse_export_section(&result.secondary_wasm, export_section).unwrap();
        assert!(exports
            .iter()
            .any(|export| export.name == "$s0" && export.kind == 0x00));
    }

    #[test]
    fn split_functions_auto_target_primary_size_cold_preserves_export_reachable_functions() {
        let main_body = &[0x00, 0x10, 0x01, 0x0b];

        let mut hot_large_body = vec![0x00];
        for _ in 0..240 {
            hot_large_body.push(0x01);
        }
        hot_large_body.push(0x0b);

        let mut cold_large_body = vec![0x00];
        for _ in 0..260 {
            cold_large_body.push(0x01);
        }
        cold_large_body.push(0x0b);

        let wasm = wasm_with_sections(&[
            type_section(&[(0, 0)]),
            function_section(&[0, 0, 0]),
            table_section(0, Some(0)),
            export_section(&[("main", 0x00, 0)]),
            code_section(&[main_body, &hot_large_body, &cold_large_body]),
        ]);

        let result = wasm_split_functions_to_target_primary_size_cold(&wasm, wasm.len() - 40).unwrap();
        assert_eq!(result.split_count, 1);

        let primary_info = parse_wasm_module_info(&result.primary_wasm).unwrap();
        let hot_primary_len = primary_info.code_bodies[1].end - primary_info.code_bodies[1].start;
        let cold_primary_len = primary_info.code_bodies[2].end - primary_info.code_bodies[2].start;

        assert_eq!(hot_primary_len, 244);
        assert!(cold_primary_len < 20);
    }

    #[test]
    fn split_functions_auto_target_primary_size_cold_skips_when_only_hot_functions_remain() {
        let mut main_body = vec![0x00];
        for _ in 0..260 {
            main_body.push(0x01);
        }
        main_body.push(0x0b);

        let wasm = wasm_with_sections(&[
            type_section(&[(0, 0)]),
            function_section(&[0]),
            table_section(0, Some(0)),
            export_section(&[("main", 0x00, 0)]),
            code_section(&[&main_body]),
        ]);

        let result = wasm_split_functions_to_target_primary_size_cold(&wasm, wasm.len() - 40).unwrap();
        assert_eq!(result.split_count, 0);
        assert!(result.secondary_wasm.is_empty());
        assert_eq!(result.primary_wasm, wasm);
    }

    #[test]
    fn split_functions_auto_target_primary_size_cold_keeps_active_element_functions_in_primary() {
        let small_body = &[0x00, 0x0b];

        let mut table_hot_large_body = vec![0x00];
        for _ in 0..240 {
            table_hot_large_body.push(0x01);
        }
        table_hot_large_body.push(0x0b);

        let mut cold_large_body = vec![0x00];
        for _ in 0..260 {
            cold_large_body.push(0x01);
        }
        cold_large_body.push(0x0b);

        let wasm = wasm_with_sections(&[
            type_section(&[(0, 0)]),
            function_section(&[0, 0, 0]),
            table_section(1, Some(1)),
            active_element_section(&[1]),
            code_section(&[small_body, &table_hot_large_body, &cold_large_body]),
        ]);

        let result = wasm_split_functions_to_target_primary_size_cold(&wasm, wasm.len() - 40).unwrap();
        assert_eq!(result.split_count, 1);

        let primary_info = parse_wasm_module_info(&result.primary_wasm).unwrap();
        let table_hot_primary_len = primary_info.code_bodies[1].end - primary_info.code_bodies[1].start;
        let cold_primary_len = primary_info.code_bodies[2].end - primary_info.code_bodies[2].start;

        assert_eq!(table_hot_primary_len, 244);
        assert!(cold_primary_len < 20);
    }

    #[test]
    fn split_functions_stub_body_structure() {
        // Test stub generation for a (i32, i32) -> i32 function
        let stub = generate_stub_body(0, 2, 5);
        // Expected: 0x00 (0 locals), 0x20 0x00 (local.get 0), 0x20 0x01 (local.get 1),
        //           0x41 0x05 (i32.const 5), 0x11 0x00 0x00 (call_indirect type=0 table=0), 0x0b (end)
        assert_eq!(stub[0], 0x00); // 0 local groups
        assert_eq!(stub[1], 0x20); // local.get
        assert_eq!(stub[2], 0x00); // param 0
        assert_eq!(stub[3], 0x20); // local.get
        assert_eq!(stub[4], 0x01); // param 1
        assert_eq!(stub[5], 0x41); // i32.const
        assert_eq!(stub[6], 0x05); // table slot 5
        assert_eq!(stub[7], 0x11); // call_indirect
        assert_eq!(stub[8], 0x00); // type index 0
        assert_eq!(stub[9], 0x00); // table index 0
        assert_eq!(stub[10], 0x0b); // end
    }

    #[test]
    fn split_functions_no_params_stub() {
        // A () -> () function stub
        let stub = generate_stub_body(2, 0, 0);
        // Expected: 0x00, 0x41 0x00, 0x11 0x02 0x00, 0x0b
        assert_eq!(stub[0], 0x00); // 0 local groups
        assert_eq!(stub[1], 0x41); // i32.const
        assert_eq!(stub[2], 0x00); // table slot 0
        assert_eq!(stub[3], 0x11); // call_indirect
        assert_eq!(stub[4], 0x02); // type index 2
        assert_eq!(stub[5], 0x00); // table index 0
        assert_eq!(stub[6], 0x0b); // end
    }

    #[test]
    fn split_functions_parse_module_info() {
        let wasm = wasm_with_sections(&[
            type_section(&[(0, 0), (1, 1)]),
            function_section(&[0, 1]),
            table_section(10, Some(20)),
            export_section(&[("f", 0x00, 0)]),
            code_section(&[&[0x00, 0x0b], &[0x00, 0x0b]]),
        ]);

        let info = parse_wasm_module_info(&wasm).unwrap();
        assert_eq!(info.types.len(), 2);
        assert_eq!(info.types[0].params.len(), 0);
        assert_eq!(info.types[0].results.len(), 0);
        assert_eq!(info.types[1].params.len(), 1);
        assert_eq!(info.types[1].results.len(), 1);
        assert_eq!(info.func_type_indices.len(), 2);
        assert_eq!(info.tables.len(), 1);
        assert_eq!(info.exports.len(), 1);
        assert_eq!(info.exports[0].name, "f");
        assert_eq!(info.code_bodies.len(), 2);
    }
}
