//! Dead data: what in memory 0's data segments no live code can reach.
//! Data has no symbols, so where an object ends is unknown; two ways to
//! clear it safely:
//! - text regions ([`zero_dead_data`]): from an address code pointed at to
//!   the next one, cleared when only removed code pointed there and the
//!   region is text (a text's start is exact and nothing points into its
//!   middle; binary data cut at a guessed end can break a live object);
//! - objects of known extent ([`Objects`]): what panic sites pass, a
//!   message as a (pointer, length) `&str` and a `core::panic::Location`
//!   (16 bytes, with its file name), cleared when nothing live points into
//!   them anymore. This relies on such text being reached only through
//!   pointers to it (code constants, data words, `&str`s in the data),
//!   which is how Rust reaches a string literal.
//! Cleared runs of 64 bytes and more are cut out of their segments (memory
//! starts zeroed).

use super::ir::*;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// The data segments of memory 0 with a known address: (start, end,
/// index). An active segment's is its constant offset; a passive one's
/// (a threaded build's, copied in by `__wasm_init_memory`) is the constant
/// destination of the `memory.init` that copies it.
pub(crate) fn data_spans(module: &Module) -> Vec<(u32, u32, usize)> {
    let mut out = Vec::new();
    let mut passive: HashMap<usize, u32> = HashMap::new();
    for func in &module.funcs {
        for w in func.body.windows(4) {
            if let [Instr::I32Const(dst), Instr::I32Const(src), Instr::I32Const(_), Instr::MemoryInit { data, memory: 0 }] = w {
                passive.entry(*data as usize).or_insert((*dst as u32).wrapping_sub(*src as u32));
            }
        }
    }
    for (i, data) in module.datas.iter().enumerate() {
        let start = match &data.mode {
            DataMode::Active { memory: 0, offset } => match offset.as_slice() {
                [Instr::I32Const(at), Instr::End] => Some(*at as u32),
                _ => None,
            },
            DataMode::Passive => passive.get(&i).copied(),
            _ => None,
        };
        if let Some(start) = start {
            out.push((start, start + data.bytes.len() as u32, i));
        }
    }
    out
}

/// The data addresses code refers to: `i32.const`s, memory-access offsets,
/// and a constant base plus the offset of the access that follows it.
pub fn code_anchors(module: &Module, skip: &HashSet<u32>) -> BTreeSet<u32> {
    let spans = data_spans(module);
    let inside = |a: u32| spans.iter().any(|(s, e, _)| a >= *s && a < *e);
    let mut out = BTreeSet::new();
    let imported = module.num_imported_funcs();
    let mut visit = |instrs: &[Instr]| {
        let mut last_const: Option<u32> = None;
        for instr in instrs {
            let offset = match instr {
                Instr::Load(_, arg) | Instr::Store(_, arg) | Instr::SimdMem(_, arg) | Instr::SimdMemLane(_, arg, _) | Instr::Atomic(_, arg) if arg.memory == 0 => Some(arg.offset),
                _ => None,
            };
            if let Some(offset) = offset {
                if inside(offset) {
                    out.insert(offset);
                }
                if let Some(base) = last_const {
                    let at = base.wrapping_add(offset);
                    if inside(at) {
                        out.insert(at);
                    }
                }
            }
            last_const = match instr {
                Instr::I32Const(v) => {
                    let v = *v as u32;
                    if inside(v) {
                        out.insert(v);
                    }
                    Some(v)
                }
                _ => None,
            };
        }
    };
    for (i, func) in module.funcs.iter().enumerate() {
        if !skip.contains(&(imported + i as u32)) {
            visit(&func.body);
        }
    }
    for global in &module.globals {
        visit(&global.init);
    }
    for elem in &module.elems {
        if let ElemMode::Active { offset, .. } = &elem.mode {
            visit(offset);
        }
    }
    out
}

/// The data's regions and which of them live code reaches. A region runs
/// from an anchor to the next one (of any kind: `extra` anchors, the
/// code's, or a pointer-shaped word in the data); it is live when live code
/// points at it or a live region holds a word that does.
pub(crate) struct DataLive {
    pub(crate) spans: Vec<(u32, u32, usize)>,
    pub(crate) anchors: BTreeSet<u32>,
    pub(crate) code: BTreeSet<u32>,
    pub(crate) live: BTreeSet<u32>,
}

impl DataLive {
    pub(crate) fn new(module: &Module, extra: &BTreeSet<u32>) -> DataLive {
        let spans = data_spans(module);
        let code = code_anchors(module, &HashSet::new());
        let inside = |a: u32| spans.iter().any(|(s, e, _)| a >= *s && a < *e);
        let mut words: BTreeMap<u32, u32> = BTreeMap::new();
        for (s, e, i) in &spans {
            let bytes = &module.datas[*i].bytes;
            let mut a = (s + 3) & !3;
            while a + 4 <= *e {
                let at = (a - s) as usize;
                let w = u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
                if inside(w) {
                    words.insert(a, w);
                }
                a += 4;
            }
        }
        let mut anchors: BTreeSet<u32> = extra.union(&code).copied().collect();
        anchors.extend(words.values().copied());
        let mut out = DataLive { spans, anchors, code, live: BTreeSet::new() };
        let mut work: Vec<u32> = out.code.iter().copied().collect();
        while let Some(a) = work.pop() {
            let (start, end) = out.region(a);
            if !out.live.insert(start) {
                continue;
            }
            for (_, w) in words.range(start..end) {
                work.push(*w);
            }
        }
        out
    }

    pub(crate) fn region(&self, a: u32) -> (u32, u32) {
        let start = self.anchors.range(..=a).next_back().copied().unwrap_or(a);
        let end = self.anchors.range(a.saturating_add(1)..).next().copied().unwrap_or(u32::MAX);
        (start, end)
    }
}

/// Clears the text only stripped code pointed at (see [`is_text`]).
/// `before`: the anchors of the module before the strip. Cleared runs of 64 bytes and more are cut
/// out of their segment (memory starts zeroed). Returns the bytes cleared.
pub fn zero_dead_data(module: &mut Module, before: &BTreeSet<u32>) -> usize {
    if data_spans(module).is_empty() {
        return 0;
    }
    let data = DataLive::new(module, before);
    let mut cleared = 0;
    for a in before.difference(&data.code) {
        let (start, end) = data.region(*a);
        if data.live.contains(&start) {
            continue;
        }
        for (s, e, i) in &data.spans {
            let (lo, hi) = (start.max(*s), end.min(*e));
            if lo < hi {
                let bytes = &mut module.datas[*i].bytes[(lo - s) as usize..(hi - s) as usize];
                // Text only (Splash sources, SVG, theme files): where a
                // region ends is a guess, and binary data (a vtable, a
                // table) cut short breaks live code; a text's start is
                // exact and nothing points into its middle.
                if !is_text(bytes) {
                    continue;
                }
                cleared += bytes.iter().filter(|b| **b != 0).count();
                bytes.fill(0);
            }
        }
    }
    split_zero_runs(module, 64);
    cleared
}

/// A run of at least 64 bytes of which at least 98% is printable text.
fn is_text(bytes: &[u8]) -> bool {
    if bytes.len() < 64 {
        return false;
    }
    let text = bytes.iter().filter(|b| matches!(b, b'\t' | b'\n' | b'\r' | 0x20..=0x7e) || **b >= 0x80).count();
    text * 100 >= bytes.len() * 98
}

/// Cuts runs of at least `min` zero bytes out of active constant-offset
/// segments of memory 0 (splitting a segment in two where a run is inside).
pub(crate) fn split_zero_runs(module: &mut Module, min: usize) {
    // Passive segments are named by index (memory.init, data.drop): leave
    // the module alone when it has any.
    if module.datas.iter().any(|d| matches!(d.mode, DataMode::Passive)) {
        return;
    }
    let mut out = Vec::new();
    for data in std::mem::take(&mut module.datas) {
        let DataMode::Active { memory: 0, offset } = &data.mode else {
            out.push(data);
            continue;
        };
        let [Instr::I32Const(base), Instr::End] = offset.as_slice() else {
            out.push(data);
            continue;
        };
        let base = *base;
        let bytes = &data.bytes;
        let mut start = 0;
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == 0 {
                let mut j = i;
                while j < bytes.len() && bytes[j] == 0 {
                    j += 1;
                }
                if j - i >= min {
                    if i > start {
                        out.push(Data { mode: DataMode::Active { memory: 0, offset: vec![Instr::I32Const(base + start as i32), Instr::End] }, bytes: bytes[start..i].to_vec() });
                    }
                    start = j;
                }
                i = j;
            } else {
                i += 1;
            }
        }
        if start < bytes.len() {
            out.push(Data { mode: DataMode::Active { memory: 0, offset: vec![Instr::I32Const(base + start as i32), Instr::End] }, bytes: bytes[start..].to_vec() });
        }
    }
    module.datas = out;
    if module.data_count.is_some() {
        module.data_count = Some(module.datas.len() as u32);
    }
}


/// Memory 0 as the constant-address data segments lay it out.
struct Image<'a> {
    spans: Vec<(u32, u32, usize)>,
    module: &'a Module,
}

impl<'a> Image<'a> {
    fn new(module: &'a Module) -> Image<'a> {
        Image { spans: data_spans(module), module }
    }

    fn bytes(&self, at: u32, len: u32) -> Option<&'a [u8]> {
        let end = at.checked_add(len)?;
        self.spans.iter().find_map(|(s, e, i)| {
            (at >= *s && end <= *e).then(|| &self.module.datas[*i].bytes[(at - s) as usize..(end - s) as usize])
        })
    }

    fn u32(&self, at: u32) -> Option<u32> {
        Some(u32::from_le_bytes(self.bytes(at, 4)?.try_into().ok()?))
    }

    /// Printable UTF-8 text of `len` bytes at `at`.
    fn is_text(&self, at: u32, len: u32) -> bool {
        len > 0
            && len <= 1 << 20
            && self
                .bytes(at, len)
                .and_then(|bytes| std::str::from_utf8(bytes).ok())
                .is_some_and(|text| text.chars().all(|c| !c.is_control() || matches!(c, '\n' | '\t' | '\r')))
    }

    /// A `core::panic::Location` at `at`: its file name's (pointer,
    /// length), when the 16 bytes there are one.
    fn location(&self, at: u32) -> Option<(u32, u32)> {
        let (file, len) = (self.u32(at)?, self.u32(at + 4)?);
        let (line, col) = (self.u32(at + 8)?, self.u32(at + 12)?);
        let name = std::str::from_utf8(self.bytes(file, len)?).ok()?;
        (name.ends_with(".rs") && line > 0 && line < 10_000_000 && col < 100_000).then_some((file, len))
    }
}

/// The shortest `&str` taken for a panic message.
const MIN_MESSAGE: i32 = 8;

/// Data objects of known extent: what panic sites pass (the message as a
/// `&str`, the `Location` and its file name). Found before panics become
/// traps (see `panics`), cleared once nothing live points into them.
#[derive(Clone, Debug, Default)]
pub struct Objects {
    /// (start, length), sorted, without duplicates.
    list: Vec<(u32, u32)>,
}

impl Objects {
    pub fn find(module: &Module) -> Objects {
        let image = Image::new(module);
        let imported = module.num_imported_funcs();
        let mut list = Vec::new();
        for func in &module.funcs {
            let body = &func.body;
            for i in 0..body.len() {
                let (Instr::Call(f), Some(Instr::Unreachable)) = (&body[i], body.get(i + 1)) else { continue };
                if *f < imported {
                    continue;
                }
                // The site's arguments: the straight-line code before it.
                let start = body[..i].iter().rposition(|instr| !super::panics::removable(instr)).map_or(0, |at| at + 1);
                let args = &body[start..i];
                for (k, instr) in args.iter().enumerate() {
                    let Instr::I32Const(value) = instr else { continue };
                    let value = *value as u32;
                    if let Some((file, len)) = image.location(value) {
                        list.push((value, 16));
                        list.push((file, len));
                    } else if let Some(Instr::I32Const(len)) = args.get(k + 1) {
                        // A message: text of some length (a short run of
                        // printable bytes may be anything).
                        if *len >= MIN_MESSAGE && image.is_text(value, *len as u32) {
                            list.push((value, *len as u32));
                        }
                    }
                }
            }
        }
        list.sort_unstable();
        list.dedup();
        Objects { list }
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }
}

/// Clears the objects nothing live points into: no live code address in
/// them, no word of the data (outside the cleared objects) pointing into
/// them, and no live (pointer, length) pair in the data overlapping them.
/// Returns the bytes cleared.
pub fn clear_dead_objects(module: &mut Module, objects: &Objects) -> usize {
    let image = Image::new(module);
    let code = code_anchors(module, &HashSet::new());
    let mut dead: Vec<(u32, u32)> = objects
        .list
        .iter()
        .copied()
        .filter(|(a, n)| code.range(*a..a.saturating_add(*n)).next().is_none() && image.bytes(*a, *n).is_some())
        .collect();
    // Every aligned word of the data, where it sits and what it holds.
    let mut words: Vec<(u32, u32)> = Vec::new();
    for (s, e, i) in &image.spans {
        let bytes = &module.datas[*i].bytes;
        let mut a = (s + 3) & !3;
        while a + 4 <= *e {
            let at = (a - s) as usize;
            words.push((a, u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])));
            a += 4;
        }
    }
    let inside = |list: &[(u32, u32)], a: u32| {
        let k = list.partition_point(|(s, _)| *s <= a);
        k > 0 && a < list[k - 1].0.saturating_add(list[k - 1].1)
    };
    // Revive what the data that stays still points into, until nothing
    // changes (a revived object's words count then).
    loop {
        let mut keep: HashSet<(u32, u32)> = HashSet::new();
        for (k, (at, word)) in words.iter().enumerate() {
            if inside(&dead, *at) {
                continue;
            }
            let overlapping = |from: u32, to: u32| {
                let first = dead.partition_point(|(s, n)| s.saturating_add(*n) <= from);
                dead[first..].iter().take_while(|(s, _)| *s < to).copied().collect::<Vec<_>>()
            };
            keep.extend(overlapping(*word, word.saturating_add(1)));
            // A `&str` (pointer, length): the whole text stays.
            if let Some((_, len)) = words.get(k + 1).filter(|(next, _)| *next == at + 4) {
                if image.is_text(*word, *len) {
                    keep.extend(overlapping(*word, word.saturating_add(*len)));
                }
            }
        }
        if keep.is_empty() {
            break;
        }
        dead.retain(|object| !keep.contains(object));
    }
    let spans = image.spans.clone();
    let mut cleared = 0;
    for (a, n) in dead {
        for (s, e, i) in &spans {
            if a >= *s && a + n <= *e {
                let bytes = &mut module.datas[*i].bytes[(a - s) as usize..(a + n - s) as usize];
                cleared += bytes.iter().filter(|b| **b != 0).count();
                bytes.fill(0);
            }
        }
    }
    if cleared > 0 {
        split_zero_runs(module, 64);
    }
    cleared
}
