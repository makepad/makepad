//! Size profile of a wasm module: bytes per function (named from the `name`
//! section and demangled), grouped by crate and by module path prefix, plus
//! section and data segment sizes. Reads the sections directly, so it works
//! on any module the section layout of which is sound, optimised or not.

use super::decode::{expr, Reader};
use super::demangle::demangle;
use super::ir::{DataMode, Instr};
use super::Res;
use crate::wasm_strip::WasmParseError;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct ProfileOptions {
    /// How many owner path segments (crate included) the module grouping
    /// keeps: 2 groups `makepad_draw::text::loader::f` under
    /// `makepad_draw::text`.
    pub module_depth: usize,
}

impl Default for ProfileOptions {
    fn default() -> ProfileOptions {
        ProfileOptions { module_depth: 2 }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FunctionSize {
    pub index: u32,
    /// The symbol as the name section has it (empty when unnamed).
    pub symbol: String,
    pub name: String,
    pub crate_name: String,
    pub module: String,
    /// Bytes of the body in the code section, its size prefix included.
    pub bytes: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GroupSize {
    pub name: String,
    pub bytes: usize,
    pub functions: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DataSegmentSize {
    pub index: usize,
    pub bytes: usize,
    /// The constant offset of an active segment.
    pub offset: Option<i64>,
    pub passive: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SectionSize {
    pub name: String,
    pub bytes: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Profile {
    pub total_bytes: usize,
    pub sections: Vec<SectionSize>,
    pub code_bytes: usize,
    pub data_bytes: usize,
    /// Every defined function, largest first.
    pub functions: Vec<FunctionSize>,
    pub crates: Vec<GroupSize>,
    pub modules: Vec<GroupSize>,
    pub datas: Vec<DataSegmentSize>,
    pub named_functions: usize,
}

const UNNAMED: &str = "(unnamed)";
const NOT_RUST: &str = "(non-rust)";

fn section_name(id: u8) -> &'static str {
    match id {
        1 => "type",
        2 => "import",
        3 => "function",
        4 => "table",
        5 => "memory",
        6 => "global",
        7 => "export",
        8 => "start",
        9 => "element",
        10 => "code",
        11 => "data",
        12 => "datacount",
        13 => "tag",
        _ => "unknown",
    }
}

fn read_func_names(data: &[u8]) -> Res<HashMap<u32, String>> {
    let mut r = Reader::new(data);
    let mut out = HashMap::new();
    while !r.is_empty() {
        let id = r.u8()?;
        let len = r.u32()? as usize;
        let mut sub = Reader::new(r.bytes(len)?);
        if id == 1 {
            let count = sub.u32()?;
            for _ in 0..count {
                let index = sub.u32()?;
                out.insert(index, sub.name()?);
            }
        }
    }
    Ok(out)
}

fn profile(buf: &[u8], opts: &ProfileOptions) -> Res<Profile> {
    let mut r = Reader::new(buf);
    if r.bytes(8)? != b"\0asm\x01\0\0\0" {
        return Err("not a wasm module".into());
    }
    let mut out = Profile {
        total_bytes: buf.len(),
        ..Profile::default()
    };
    let mut imported_funcs = 0u32;
    let mut bodies: Vec<usize> = Vec::new();
    let mut names = HashMap::new();
    while !r.is_empty() {
        let start = r.pos();
        let id = r.u8()?;
        let len = r.u32()? as usize;
        let payload = r.bytes(len)?;
        let bytes = r.pos() - start;
        let mut s = Reader::new(payload);
        let name = if id == 0 {
            let name = s.name()?;
            if name == "name" {
                names = read_func_names(&payload[s.pos()..]).unwrap_or_default();
            }
            format!("custom:{name}")
        } else {
            section_name(id).to_string()
        };
        out.sections.push(SectionSize { name, bytes });
        match id {
            2 => {
                for _ in 0..s.u32()? {
                    s.name()?;
                    s.name()?;
                    match s.u8()? {
                        0 => {
                            s.u32()?;
                            imported_funcs += 1;
                        }
                        1 => {
                            s.u8()?;
                            let flags = s.u8()?;
                            s.u32()?;
                            if flags & 1 != 0 {
                                s.u32()?;
                            }
                        }
                        2 => {
                            let flags = s.u8()?;
                            s.u32()?;
                            if flags & 1 != 0 {
                                s.u32()?;
                            }
                        }
                        3 => {
                            s.u8()?;
                            s.u8()?;
                        }
                        _ => return Err("unknown import kind".into()),
                    }
                }
            }
            10 => {
                out.code_bytes = bytes;
                for _ in 0..s.u32()? {
                    let at = s.pos();
                    let size = s.u32()? as usize;
                    s.bytes(size)?;
                    bodies.push(s.pos() - at);
                }
            }
            11 => {
                out.data_bytes = bytes;
                for index in 0..s.u32()? as usize {
                    let mode = match s.u32()? {
                        0 => DataMode::Active {
                            memory: 0,
                            offset: expr(&mut s)?,
                        },
                        1 => DataMode::Passive,
                        2 => {
                            let memory = s.u32()?;
                            DataMode::Active {
                                memory,
                                offset: expr(&mut s)?,
                            }
                        }
                        _ => return Err("unknown data segment kind".into()),
                    };
                    let size = s.u32()? as usize;
                    s.bytes(size)?;
                    let (offset, passive) = match mode {
                        DataMode::Active { offset, .. } => (
                            match offset.as_slice() {
                                [Instr::I32Const(value), Instr::End] => Some(*value as u32 as i64),
                                _ => None,
                            },
                            false,
                        ),
                        DataMode::Passive => (None, true),
                    };
                    out.datas.push(DataSegmentSize {
                        index,
                        bytes: size,
                        offset,
                        passive,
                    });
                }
            }
            _ => {}
        }
    }

    let mut crates: HashMap<String, GroupSize> = HashMap::new();
    let mut modules: HashMap<String, GroupSize> = HashMap::new();
    for (i, bytes) in bodies.iter().copied().enumerate() {
        let index = imported_funcs + i as u32;
        let symbol = names.get(&index).cloned().unwrap_or_default();
        let (name, crate_name, module) = if symbol.is_empty() {
            (format!("func[{index}]"), UNNAMED.to_string(), UNNAMED.to_string())
        } else if let Some(d) = demangle(&symbol) {
            let depth = opts.module_depth.max(1).min(d.owner.len());
            (d.name, d.owner[0].clone(), d.owner[..depth].join("::"))
        } else {
            (symbol.clone(), NOT_RUST.to_string(), NOT_RUST.to_string())
        };
        if !symbol.is_empty() {
            out.named_functions += 1;
        }
        for (map, key) in [(&mut crates, &crate_name), (&mut modules, &module)] {
            let group = map.entry(key.clone()).or_insert_with(|| GroupSize {
                name: key.clone(),
                ..GroupSize::default()
            });
            group.bytes += bytes;
            group.functions += 1;
        }
        out.functions.push(FunctionSize {
            index,
            symbol,
            name,
            crate_name,
            module,
            bytes,
        });
    }
    out.functions
        .sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.index.cmp(&b.index)));
    let sorted = |map: HashMap<String, GroupSize>| {
        let mut list: Vec<GroupSize> = map.into_values().collect();
        list.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.name.cmp(&b.name)));
        list
    };
    out.crates = sorted(crates);
    out.modules = sorted(modules);
    Ok(out)
}

/// Profiles a module's size. Function names come from the `name` section, so
/// profile a module built (or optimised) with names kept.
pub fn wasm_size_profile(buf: &[u8], opts: &ProfileOptions) -> Result<Profile, WasmParseError> {
    profile(buf, opts).map_err(|_| WasmParseError)
}

fn pct(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 * 100.0 / whole as f64
    }
}

fn json_str(out: &mut String, s: &str) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn json_groups(out: &mut String, groups: &[GroupSize], top: usize) {
    out.push('[');
    for (i, group) in groups.iter().take(top).enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"name\":");
        json_str(out, &group.name);
        out.push_str(&format!(
            ",\"bytes\":{},\"functions\":{}}}",
            group.bytes, group.functions
        ));
    }
    out.push(']');
}

impl Profile {
    pub fn to_text(&self, top: usize) -> String {
        let mut out = format!(
            "wasm size profile: {} bytes, code {} ({:.1}%), data {} ({:.1}%), {} functions ({} named)\n",
            self.total_bytes,
            self.code_bytes,
            pct(self.code_bytes, self.total_bytes),
            self.data_bytes,
            pct(self.data_bytes, self.total_bytes),
            self.functions.len(),
            self.named_functions
        );
        out.push_str("\nsections:\n");
        for section in &self.sections {
            out.push_str(&format!(
                "  {:>10} {:>5.1}%  {}\n",
                section.bytes,
                pct(section.bytes, self.total_bytes),
                section.name
            ));
        }
        let groups = |out: &mut String, title: &str, list: &[GroupSize]| {
            out.push_str(&format!("\n{title}:\n"));
            for group in list.iter().take(top) {
                out.push_str(&format!(
                    "  {:>10} {:>5.1}% {:>6} fns  {}\n",
                    group.bytes,
                    pct(group.bytes, self.code_bytes),
                    group.functions,
                    group.name
                ));
            }
            if list.len() > top {
                let rest: usize = list[top..].iter().map(|group| group.bytes).sum();
                out.push_str(&format!("  {:>10}        ({} more)\n", rest, list.len() - top));
            }
        };
        groups(&mut out, "code by crate", &self.crates);
        groups(&mut out, "code by module", &self.modules);
        out.push_str(&format!("\ntop {top} functions:\n"));
        for func in self.functions.iter().take(top) {
            out.push_str(&format!(
                "  {:>10} {:>5.1}%  {}\n",
                func.bytes,
                pct(func.bytes, self.code_bytes),
                func.name
            ));
        }
        if !self.datas.is_empty() {
            let mut datas: Vec<&DataSegmentSize> = self.datas.iter().collect();
            datas.sort_by(|a, b| b.bytes.cmp(&a.bytes));
            out.push_str(&format!("\ndata segments ({}):\n", self.datas.len()));
            for data in datas.iter().take(top) {
                let at = match (data.passive, data.offset) {
                    (true, _) => "passive".to_string(),
                    (false, Some(offset)) => format!("at {offset:#x}"),
                    (false, None) => "at a computed offset".to_string(),
                };
                out.push_str(&format!("  {:>10}  segment {} {at}\n", data.bytes, data.index));
            }
        }
        out
    }

    pub fn to_json(&self, top: usize) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "{{\"total_bytes\":{},\"code_bytes\":{},\"data_bytes\":{},\"function_count\":{},\"named_functions\":{},\"sections\":[",
            self.total_bytes,
            self.code_bytes,
            self.data_bytes,
            self.functions.len(),
            self.named_functions
        ));
        for (i, section) in self.sections.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str("{\"name\":");
            json_str(&mut out, &section.name);
            out.push_str(&format!(",\"bytes\":{}}}", section.bytes));
        }
        out.push_str("],\"crates\":");
        json_groups(&mut out, &self.crates, usize::MAX);
        out.push_str(",\"modules\":");
        json_groups(&mut out, &self.modules, usize::MAX);
        out.push_str(",\"functions\":[");
        for (i, func) in self.functions.iter().take(top).enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&format!("{{\"index\":{},\"bytes\":{},\"name\":", func.index, func.bytes));
            json_str(&mut out, &func.name);
            out.push_str(",\"crate\":");
            json_str(&mut out, &func.crate_name);
            out.push_str(",\"module\":");
            json_str(&mut out, &func.module);
            out.push('}');
        }
        out.push_str("],\"datas\":[");
        for (i, data) in self.datas.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&format!(
                "{{\"index\":{},\"bytes\":{},\"passive\":{},\"offset\":{}}}",
                data.index,
                data.bytes,
                data.passive,
                data.offset.map_or("null".to_string(), |offset| offset.to_string())
            ));
        }
        out.push_str("]}");
        out
    }

    /// Per-crate code size change from `self` (before) to `after`, largest
    /// change first.
    pub fn diff_text(&self, after: &Profile, top: usize) -> String {
        let mut rows: HashMap<&str, (usize, usize)> = HashMap::new();
        for group in &self.crates {
            rows.entry(&group.name).or_default().0 = group.bytes;
        }
        for group in &after.crates {
            rows.entry(&group.name).or_default().1 = group.bytes;
        }
        let mut rows: Vec<(&str, usize, usize)> =
            rows.into_iter().map(|(name, (a, b))| (name, a, b)).collect();
        rows.sort_by_key(|(name, a, b)| (-((*b as i64 - *a as i64).abs()), name.to_string()));
        let mut out = format!(
            "total {} -> {} ({:+}), code {} -> {} ({:+})\n",
            self.total_bytes,
            after.total_bytes,
            after.total_bytes as i64 - self.total_bytes as i64,
            self.code_bytes,
            after.code_bytes,
            after.code_bytes as i64 - self.code_bytes as i64
        );
        for (name, a, b) in rows.into_iter().take(top) {
            out.push_str(&format!("  {a:>10} -> {b:>10}  {:>+9}  {name}\n", b as i64 - a as i64));
        }
        out
    }
}
