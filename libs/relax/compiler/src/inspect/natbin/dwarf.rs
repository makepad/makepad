//! DWARF 4/5 reader for natbin: compile-unit headers, abbreviations, the DIE tree (just the tags
//! the inline tree needs), range lists, and the line-number program. Everything is parsed per
//! compile unit on demand and cached; nothing walks the whole of a big binary's debug info.

use super::{InlinedCall, LineRow};
use crate::inspect::symbols::demangle;
use std::collections::HashMap;
use std::sync::Arc;

const NONE: u32 = u32::MAX;

/// (offset, length) of a section inside `Dbg::data`.
pub(super) type Span = (usize, usize);

#[derive(Default, Clone)]
pub(super) struct Secs {
    pub info: Span,
    pub abbrev: Span,
    pub line: Span,
    pub str_: Span,
    pub line_str: Span,
    pub str_offsets: Span,
    pub addr: Span,
    pub rnglists: Span,
    pub ranges: Span,
}

impl Secs {
    /// Records a section by its ELF (`.debug_info`) or Mach-O (`__debug_info`) name.
    pub fn set(&mut self, name: &str, span: Span) {
        let Some(n) = name.strip_prefix(".debug_").or_else(|| name.strip_prefix("__debug_")) else { return };
        match n {
            "info" => self.info = span,
            "abbrev" => self.abbrev = span,
            "line" => self.line = span,
            "str" => self.str_ = span,
            "line_str" => self.line_str = span,
            "str_offsets" | "str_offs" => self.str_offsets = span,
            "addr" => self.addr = span,
            "rnglists" => self.rnglists = span,
            "ranges" => self.ranges = span,
            _ => {}
        }
    }
}

/// Source file names shared by every compile unit: `LineRow::file` indexes `names`.
#[derive(Default)]
pub(super) struct Interner {
    pub names: Vec<String>,
    map: HashMap<String, u32>,
}

impl Interner {
    pub fn id(&mut self, s: String) -> u32 {
        if let Some(&i) = self.map.get(&s) {
            return i;
        }
        let i = self.names.len() as u32;
        self.names.push(s.clone());
        self.map.insert(s, i);
        i
    }
}

/// A function of a relocatable object mapped to its place in the linked binary (the debug map).
#[derive(Clone, Copy)]
pub(super) struct MapEnt {
    pub lo: u64,
    pub hi: u64,
    pub to: u64,
}

#[derive(Clone)]
struct CuHdr {
    off: usize,
    end: usize,
    ver: u16,
    addr_size: u8,
    off64: bool,
    abbrev_off: usize,
    die_off: usize,
}

struct Abbrev {
    tag: u32,
    children: bool,
    attrs: Vec<(u32, u32, i64)>,
}

#[derive(Clone, Copy, Debug)]
enum Val {
    Addr(u64),
    U(u64),
    I(i64),
    StrP(u64),
    LineStrP(u64),
    StrX(u64),
    StrInline(usize),
    AddrX(u64),
    RngX(u64),
    Ref(u64),
    Block,
}

impl Val {
    fn uint(self) -> Option<u64> {
        match self {
            Val::U(x) | Val::Addr(x) | Val::Ref(x) => Some(x),
            Val::I(x) => Some(x as u64),
            _ => None,
        }
    }
}

#[derive(Clone, Default)]
struct Ctx {
    str_base: u64,
    addr_base: u64,
    rng_base: u64,
    low: u64,
}

struct Rd<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Rd<'a> {
    fn new(b: &'a [u8], p: usize) -> Rd<'a> {
        Rd { b, p }
    }
    fn u8(&mut self) -> Option<u8> {
        let v = *self.b.get(self.p)?;
        self.p += 1;
        Some(v)
    }
    fn un(&mut self, n: usize) -> Option<u64> {
        let s = self.b.get(self.p..self.p + n)?;
        self.p += n;
        let mut v = 0u64;
        for (i, &x) in s.iter().take(8).enumerate() {
            v |= (x as u64) << (8 * i);
        }
        Some(v)
    }
    fn uleb(&mut self) -> Option<u64> {
        let mut v = 0u64;
        let mut sh = 0;
        loop {
            let x = self.u8()?;
            if sh < 64 {
                v |= ((x & 0x7f) as u64) << sh;
            }
            sh += 7;
            if x & 0x80 == 0 {
                return Some(v);
            }
        }
    }
    fn sleb(&mut self) -> Option<i64> {
        let mut v = 0i64;
        let mut sh = 0;
        loop {
            let x = self.u8()?;
            if sh < 64 {
                v |= ((x & 0x7f) as i64) << sh;
            }
            sh += 7;
            if x & 0x80 == 0 {
                if sh < 64 && x & 0x40 != 0 {
                    v |= -1i64 << sh;
                }
                return Some(v);
            }
        }
    }
    fn off(&mut self, o64: bool) -> Option<u64> {
        self.un(if o64 { 8 } else { 4 })
    }
    fn skip(&mut self, n: usize) -> Option<()> {
        if self.p.checked_add(n)? > self.b.len() {
            return None;
        }
        self.p += n;
        Some(())
    }
    /// A NUL-terminated string at the cursor: (start, text).
    fn cstr(&mut self) -> Option<(usize, &'a [u8])> {
        let s = self.b.get(self.p..)?;
        let n = s.iter().position(|&c| c == 0)?;
        let start = self.p;
        self.p += n + 1;
        Some((start, &s[..n]))
    }
}

fn read_form(r: &mut Rd, form: u64, ic: i64, cu: &CuHdr) -> Option<Val> {
    Some(match form {
        0x01 => Val::Addr(r.un(cu.addr_size as usize)?),
        0x03 => {
            let n = r.un(2)? as usize;
            r.skip(n)?;
            Val::Block
        }
        0x04 => {
            let n = r.un(4)? as usize;
            r.skip(n)?;
            Val::Block
        }
        0x05 => Val::U(r.un(2)?),
        0x06 => Val::U(r.un(4)?),
        0x07 => Val::U(r.un(8)?),
        0x0b => Val::U(r.un(1)?),
        0x08 => {
            let (s, _) = r.cstr()?;
            Val::StrInline(s)
        }
        0x09 | 0x18 => {
            let n = r.uleb()? as usize;
            r.skip(n)?;
            Val::Block
        }
        0x0a => {
            let n = r.u8()? as usize;
            r.skip(n)?;
            Val::Block
        }
        0x0c => Val::U(r.u8()? as u64),
        0x0d => Val::I(r.sleb()?),
        0x0e => Val::StrP(r.off(cu.off64)?),
        0x0f => Val::U(r.uleb()?),
        0x10 => Val::Ref(if cu.ver <= 2 { r.un(cu.addr_size as usize)? } else { r.off(cu.off64)? }),
        0x11 => Val::Ref(cu.off as u64 + r.un(1)?),
        0x12 => Val::Ref(cu.off as u64 + r.un(2)?),
        0x13 => Val::Ref(cu.off as u64 + r.un(4)?),
        0x14 => Val::Ref(cu.off as u64 + r.un(8)?),
        0x15 => Val::Ref(cu.off as u64 + r.uleb()?),
        0x16 => {
            let f = r.uleb()?;
            let ic = if f == 0x21 { r.sleb()? } else { 0 };
            return read_form(r, f, ic, cu);
        }
        0x17 => Val::U(r.off(cu.off64)?),
        0x19 => Val::U(1),
        0x1a | 0x1f02 => Val::StrX(r.uleb()?),
        0x25 => Val::StrX(r.un(1)?),
        0x26 => Val::StrX(r.un(2)?),
        0x27 => Val::StrX(r.un(3)?),
        0x28 => Val::StrX(r.un(4)?),
        0x1b | 0x1f01 => Val::AddrX(r.uleb()?),
        0x29 => Val::AddrX(r.un(1)?),
        0x2a => Val::AddrX(r.un(2)?),
        0x2b => Val::AddrX(r.un(3)?),
        0x2c => Val::AddrX(r.un(4)?),
        0x1c => {
            r.skip(4)?;
            Val::U(0)
        }
        0x1d | 0x1f20 | 0x1f21 => {
            r.skip(if cu.off64 { 8 } else { 4 })?;
            Val::U(0)
        }
        0x1e => {
            r.skip(16)?;
            Val::U(0)
        }
        0x1f => Val::LineStrP(r.off(cu.off64)?),
        0x20 | 0x24 => {
            r.skip(8)?;
            Val::U(0)
        }
        0x21 => Val::I(ic),
        0x22 => Val::U(r.uleb()?),
        0x23 => Val::RngX(r.uleb()?),
        _ => return None,
    })
}

/// One DIE the inline tree cares about (compile unit, namespaces and types for names, subprograms,
/// inlined calls, lexical blocks), in DFS order. `parent` is the nearest stored ancestor.
pub(super) struct Die {
    off: u64,
    tag: u32,
    parent: u32,
    name: Option<Box<str>>,
    link: Option<Box<str>>,
    origin: Option<u64>,
    spec: Option<u64>,
    decl: bool,
    call_file: u32,
    call_line: u32,
    ranges: Vec<(u64, u64)>,
}

pub(super) struct Tree {
    dies: Vec<Die>,
    stmt_list: Option<u64>,
    comp_dir: String,
}

pub(super) struct SubProg {
    pub ranges: Vec<(u64, u64)>,
    pub inl: Vec<InlinedCall>,
}

pub(super) struct CuData {
    /// sorted by address
    pub rows: Vec<LineRow>,
    pub subs: Vec<SubProg>,
}

struct RawRow {
    addr: u64,
    file: u32,
    line: u32,
    col: u32,
    end: bool,
}

const TAG_CLASS: u32 = 0x02;
const TAG_ENUM: u32 = 0x04;
const TAG_LEXICAL: u32 = 0x0b;
const TAG_STRUCT: u32 = 0x13;
const TAG_UNION: u32 = 0x17;
const TAG_CU: u32 = 0x11;
const TAG_INLINED: u32 = 0x1d;
const TAG_SUBPROGRAM: u32 = 0x2e;
const TAG_NAMESPACE: u32 = 0x39;

fn naming_tag(t: u32) -> bool {
    matches!(t, TAG_CLASS | TAG_ENUM | TAG_STRUCT | TAG_UNION | TAG_NAMESPACE | TAG_SUBPROGRAM)
}
fn stored_tag(t: u32) -> bool {
    naming_tag(t) || matches!(t, TAG_LEXICAL | TAG_CU | TAG_INLINED)
}

/// The debug info of one file (a linked binary, a dSYM, or a relocatable object with a debug map).
pub(super) struct Dbg {
    pub data: Arc<Vec<u8>>,
    pub secs: Secs,
    /// object address -> binary address, for relocatable objects; sorted by `lo`
    pub map: Option<Vec<MapEnt>>,
    cus: Option<Vec<CuHdr>>,
    cu_ranges: Option<Vec<(u64, u64, usize)>>,
    abbrevs: HashMap<usize, Arc<Vec<Option<Abbrev>>>>,
    trees: HashMap<usize, Arc<Tree>>,
    cu_data: HashMap<usize, Arc<CuData>>,
    names: HashMap<u64, String>,
}

impl Dbg {
    pub fn new(data: Arc<Vec<u8>>, secs: Secs, map: Option<Vec<MapEnt>>) -> Dbg {
        let n = data.len();
        let clip = |s: Span| if s.0 <= n && s.0 + s.1 <= n { s } else { (0, 0) };
        let secs = Secs {
            info: clip(secs.info),
            abbrev: clip(secs.abbrev),
            line: clip(secs.line),
            str_: clip(secs.str_),
            line_str: clip(secs.line_str),
            str_offsets: clip(secs.str_offsets),
            addr: clip(secs.addr),
            rnglists: clip(secs.rnglists),
            ranges: clip(secs.ranges),
        };
        Dbg { data, secs, map, cus: None, cu_ranges: None, abbrevs: HashMap::new(), trees: HashMap::new(), cu_data: HashMap::new(), names: HashMap::new() }
    }

    pub fn has_info(&self) -> bool {
        self.secs.info.1 > 0
    }

    fn sec(&self, s: Span) -> &[u8] {
        &self.data[s.0..s.0 + s.1]
    }

    fn tr(&self, a: u64) -> Option<u64> {
        let Some(m) = &self.map else { return Some(a) };
        let i = m.partition_point(|e| e.lo <= a);
        let e = m.get(i.checked_sub(1)?)?;
        (a < e.hi).then(|| a - e.lo + e.to)
    }

    fn tr_range(&self, lo: u64, hi: u64) -> Option<(u64, u64)> {
        let Some(m) = &self.map else { return Some((lo, hi)) };
        let i = m.partition_point(|e| e.lo <= lo);
        let e = m.get(i.checked_sub(1)?)?;
        (lo < e.hi).then(|| (lo - e.lo + e.to, hi.min(e.hi) - e.lo + e.to))
    }

    // ------------------------------------------------------------------ units

    fn cus(&mut self) -> &Vec<CuHdr> {
        if self.cus.is_none() {
            let info = self.sec(self.secs.info);
            let mut v = Vec::new();
            let mut r = Rd::new(info, 0);
            while r.p + 11 <= info.len() {
                let off = r.p;
                let Some(h) = (|| -> Option<CuHdr> {
                    let mut len = r.un(4)?;
                    let off64 = len == 0xffff_ffff;
                    if off64 {
                        len = r.un(8)?;
                    }
                    let end = r.p.checked_add(len as usize)?;
                    let ver = r.un(2)? as u16;
                    let (abbrev_off, addr_size, unit_type);
                    if ver >= 5 {
                        unit_type = r.u8()?;
                        addr_size = r.u8()?;
                        abbrev_off = r.off(off64)?;
                    } else {
                        abbrev_off = r.off(off64)?;
                        addr_size = r.u8()?;
                        unit_type = 1;
                    }
                    match unit_type {
                        4 | 5 => r.skip(8)?,
                        2 | 6 => {
                            r.skip(8)?;
                            r.skip(if off64 { 8 } else { 4 })?;
                        }
                        _ => {}
                    }
                    Some(CuHdr { off, end: end.min(info.len()), ver, addr_size, off64, abbrev_off: abbrev_off as usize, die_off: r.p })
                })() else {
                    break;
                };
                r.p = h.end;
                v.push(h);
            }
            self.cus = Some(v);
        }
        self.cus.as_ref().unwrap()
    }

    fn abbrevs(&mut self, off: usize) -> Option<Arc<Vec<Option<Abbrev>>>> {
        if let Some(a) = self.abbrevs.get(&off) {
            return Some(a.clone());
        }
        let mut r = Rd::new(self.sec(self.secs.abbrev), off);
        let mut v: Vec<Option<Abbrev>> = Vec::new();
        loop {
            let code = r.uleb()? as usize;
            if code == 0 {
                break;
            }
            if code > (1 << 24) {
                return None;
            }
            let tag = r.uleb()? as u32;
            let children = r.u8()? != 0;
            let mut attrs = Vec::new();
            loop {
                let at = r.uleb()? as u32;
                let form = r.uleb()? as u32;
                let ic = if form == 0x21 { r.sleb()? } else { 0 };
                if at == 0 && form == 0 {
                    break;
                }
                attrs.push((at, form, ic));
            }
            if v.len() <= code {
                v.resize_with(code + 1, || None);
            }
            v[code] = Some(Abbrev { tag, children, attrs });
        }
        let a = Arc::new(v);
        self.abbrevs.insert(off, a.clone());
        Some(a)
    }

    fn read_die(&self, r: &mut Rd, cu: &CuHdr, ab: &[Option<Abbrev>], out: &mut Vec<(u32, Val)>) -> Option<Option<(u32, bool)>> {
        let code = r.uleb()? as usize;
        if code == 0 {
            return Some(None);
        }
        let a = ab.get(code)?.as_ref()?;
        out.clear();
        for &(at, form, ic) in &a.attrs {
            out.push((at, read_form(r, form as u64, ic, cu)?));
        }
        Some(Some((a.tag, a.children)))
    }

    fn ctx_of(&self, attrs: &[(u32, Val)], cu: &CuHdr) -> Ctx {
        let o = if cu.off64 { 16 } else { 8 };
        let mut c = Ctx { str_base: o, addr_base: o, rng_base: if cu.off64 { 20 } else { 12 }, low: 0 };
        for &(at, v) in attrs {
            match at {
                0x72 => c.str_base = v.uint().unwrap_or(c.str_base),
                0x73 => c.addr_base = v.uint().unwrap_or(c.addr_base),
                0x74 => c.rng_base = v.uint().unwrap_or(c.rng_base),
                _ => {}
            }
        }
        for &(at, v) in attrs {
            if at == 0x11 {
                c.low = self.addr_of(v, cu, &c).unwrap_or(0);
            }
        }
        c
    }

    fn str_of(&self, v: Val, cu: &CuHdr, c: &Ctx) -> Option<String> {
        let cs = |sec: &[u8], o: usize| -> Option<String> {
            let s = sec.get(o..)?;
            let n = s.iter().position(|&x| x == 0)?;
            Some(String::from_utf8_lossy(&s[..n]).into_owned())
        };
        match v {
            Val::StrP(o) => cs(self.sec(self.secs.str_), o as usize),
            Val::LineStrP(o) => cs(self.sec(self.secs.line_str), o as usize),
            Val::StrInline(o) => cs(self.sec(self.secs.info), o),
            Val::StrX(i) => {
                let sz = if cu.off64 { 8 } else { 4 };
                let mut r = Rd::new(self.sec(self.secs.str_offsets), (c.str_base + i * sz as u64) as usize);
                let o = r.un(sz)?;
                cs(self.sec(self.secs.str_), o as usize)
            }
            _ => None,
        }
    }

    fn addr_of(&self, v: Val, cu: &CuHdr, c: &Ctx) -> Option<u64> {
        match v {
            Val::Addr(a) => Some(a),
            Val::AddrX(i) => {
                let sz = cu.addr_size as usize;
                Rd::new(self.sec(self.secs.addr), (c.addr_base + i * sz as u64) as usize).un(sz)
            }
            _ => None,
        }
    }

    /// The address ranges of a DIE from low/high pc or DW_AT_ranges (object addresses).
    fn ranges_of(&self, attrs: &[(u32, Val)], cu: &CuHdr, c: &Ctx) -> Vec<(u64, u64)> {
        let get = |at: u32| attrs.iter().find(|a| a.0 == at).map(|a| a.1);
        if let Some(v) = get(0x55) {
            return self.range_list(cu, c, v);
        }
        let (Some(lo), Some(hi)) = (get(0x11), get(0x12)) else { return Vec::new() };
        let Some(lo) = self.addr_of(lo, cu, c) else { return Vec::new() };
        let hi = match hi {
            Val::Addr(_) | Val::AddrX(_) => self.addr_of(hi, cu, c),
            h => h.uint().map(|n| lo.wrapping_add(n)),
        };
        match hi {
            Some(hi) if hi > lo => vec![(lo, hi)],
            _ => Vec::new(),
        }
    }

    fn range_list(&self, cu: &CuHdr, c: &Ctx, v: Val) -> Vec<(u64, u64)> {
        let mut out = Vec::new();
        let asz = cu.addr_size as usize;
        if cu.ver < 5 {
            let Some(off) = v.uint() else { return out };
            let mut r = Rd::new(self.sec(self.secs.ranges), off as usize);
            let max = if asz >= 8 { u64::MAX } else { (1u64 << (8 * asz)) - 1 };
            let mut base = c.low;
            while let (Some(a), Some(b)) = (r.un(asz), r.un(asz)) {
                if a == 0 && b == 0 {
                    break;
                }
                if a == max {
                    base = b;
                } else if b > a {
                    out.push((base.wrapping_add(a), base.wrapping_add(b)));
                }
            }
            return out;
        }
        let sec = self.sec(self.secs.rnglists);
        let off = match v {
            Val::RngX(i) => {
                let sz = if cu.off64 { 8 } else { 4 };
                let Some(o) = Rd::new(sec, (c.rng_base + i * sz as u64) as usize).un(sz) else { return out };
                c.rng_base + o
            }
            other => match other.uint() {
                Some(o) => o,
                None => return out,
            },
        };
        let mut r = Rd::new(sec, off as usize);
        let mut base = c.low;
        let ax = |i: u64| self.addr_of(Val::AddrX(i), cu, c);
        loop {
            let Some(k) = r.u8() else { break };
            let ent: Option<(u64, u64)> = match k {
                0 => break,
                1 => {
                    if let Some(b) = r.uleb().and_then(ax) {
                        base = b;
                    }
                    continue;
                }
                2 => (|| Some((ax(r.uleb()?)?, ax(r.uleb()?)?)))(),
                3 => (|| {
                    let a = ax(r.uleb()?)?;
                    Some((a, a + r.uleb()?))
                })(),
                4 => (|| Some((base + r.uleb()?, base + r.uleb()?)))(),
                5 => {
                    if let Some(b) = r.un(asz) {
                        base = b;
                    }
                    continue;
                }
                6 => (|| Some((r.un(asz)?, r.un(asz)?)))(),
                7 => (|| {
                    let a = r.un(asz)?;
                    Some((a, a + r.uleb()?))
                })(),
                _ => break,
            };
            match ent {
                Some((a, b)) if b > a => out.push((a, b)),
                Some(_) => {}
                None => break,
            }
        }
        out
    }

    // ------------------------------------------------------------------ DIE trees

    /// Ranges of the unit's root DIE, read without building the tree.
    fn root_ranges(&mut self, ci: usize) -> Vec<(u64, u64)> {
        let hdr = self.cus()[ci].clone();
        let Some(ab) = self.abbrevs(hdr.abbrev_off) else { return Vec::new() };
        let mut r = Rd::new(self.sec(self.secs.info), hdr.die_off);
        let mut attrs = Vec::new();
        match self.read_die(&mut r, &hdr, &ab, &mut attrs) {
            Some(Some(_)) => {
                let c = self.ctx_of(&attrs, &hdr);
                self.ranges_of(&attrs, &hdr, &c)
            }
            _ => Vec::new(),
        }
    }

    /// The compile units whose code intersects [lo, hi) (binary addresses).
    pub fn cus_for(&mut self, lo: u64, hi: u64) -> Vec<usize> {
        let n = self.cus().len();
        if self.map.is_some() {
            return (0..n).collect();
        }
        if self.cu_ranges.is_none() {
            let mut v = Vec::new();
            for ci in 0..n {
                for (a, b) in self.root_ranges(ci) {
                    v.push((a, b, ci));
                }
            }
            v.sort();
            self.cu_ranges = Some(v);
        }
        let mut out: Vec<usize> = self.cu_ranges.as_ref().unwrap().iter().filter(|r| r.0 < hi && r.1 > lo).map(|r| r.2).collect();
        out.sort();
        out.dedup();
        out
    }

    fn tree(&mut self, ci: usize) -> Option<Arc<Tree>> {
        if let Some(t) = self.trees.get(&ci) {
            return Some(t.clone());
        }
        let hdr = self.cus().get(ci)?.clone();
        let ab = self.abbrevs(hdr.abbrev_off)?;
        let t = Arc::new(self.build_tree(&hdr, &ab)?);
        self.trees.insert(ci, t.clone());
        Some(t)
    }

    fn build_tree(&self, hdr: &CuHdr, ab: &[Option<Abbrev>]) -> Option<Tree> {
        let mut r = Rd::new(self.sec(self.secs.info), hdr.die_off);
        let mut attrs = Vec::new();
        let mut dies: Vec<Die> = Vec::new();
        let mut stack: Vec<u32> = Vec::new();
        let mut ctx = Ctx::default();
        let mut tree = Tree { dies: Vec::new(), stmt_list: None, comp_dir: String::new() };
        let mut first = true;
        while r.p < hdr.end {
            let off = r.p as u64;
            let Some((tag, children)) = self.read_die(&mut r, hdr, ab, &mut attrs)? else {
                stack.pop();
                if stack.is_empty() {
                    break;
                }
                continue;
            };
            if first {
                ctx = self.ctx_of(&attrs, hdr);
                for &(at, v) in &attrs {
                    match at {
                        0x10 => tree.stmt_list = v.uint(),
                        0x1b => tree.comp_dir = self.str_of(v, hdr, &ctx).unwrap_or_default(),
                        _ => {}
                    }
                }
            }
            let parent = stack.last().copied().unwrap_or(NONE);
            let me = if stored_tag(tag) {
                let mut d = Die { off, tag, parent, name: None, link: None, origin: None, spec: None, decl: false, call_file: 0, call_line: 0, ranges: Vec::new() };
                for &(at, v) in &attrs {
                    match at {
                        0x03 if naming_tag(tag) => d.name = self.str_of(v, hdr, &ctx).map(|s| s.into_boxed_str()),
                        0x6e | 0x2007 if tag == TAG_SUBPROGRAM => d.link = self.str_of(v, hdr, &ctx).map(|s| s.into_boxed_str()),
                        0x31 => d.origin = v.uint(),
                        0x47 => d.spec = v.uint(),
                        0x3c => d.decl = true,
                        0x58 => d.call_file = v.uint().unwrap_or(0) as u32,
                        0x59 => d.call_line = v.uint().unwrap_or(0) as u32,
                        _ => {}
                    }
                }
                if matches!(tag, TAG_SUBPROGRAM | TAG_INLINED | TAG_CU) {
                    d.ranges = self.ranges_of(&attrs, hdr, &ctx);
                }
                dies.push(d);
                (dies.len() - 1) as u32
            } else {
                parent
            };
            if children {
                stack.push(me);
            } else if first {
                break;
            }
            first = false;
        }
        tree.dies = dies;
        Some(tree)
    }

    /// (unit, die index) of the DIE at a .debug_info offset.
    fn locate(&mut self, off: u64) -> Option<(usize, usize)> {
        let ci = {
            let cus = self.cus();
            let i = cus.partition_point(|c| (c.off as u64) <= off);
            let c = cus.get(i.checked_sub(1)?)?;
            if off >= c.end as u64 {
                return None;
            }
            i - 1
        };
        let t = self.tree(ci)?;
        let j = t.dies.binary_search_by_key(&off, |d| d.off).ok()?;
        Some((ci, j))
    }

    /// The demangled name of a subprogram DIE: its linkage name, else through its abstract origin /
    /// specification, else its own name qualified by the enclosing namespaces.
    fn die_name(&mut self, ci: usize, idx: usize, depth: u32) -> Option<String> {
        let t = self.tree(ci)?;
        let d = t.dies.get(idx)?;
        if let Some(n) = self.names.get(&d.off) {
            return Some(n.clone());
        }
        let name = (|| {
            if let Some(l) = &d.link {
                return Some(demangle(l));
            }
            if depth > 8 {
                return None;
            }
            for target in [d.origin, d.spec].into_iter().flatten() {
                if let Some((cj, j)) = self.locate(target) {
                    if let Some(n) = self.die_name(cj, j, depth + 1) {
                        return Some(n);
                    }
                }
            }
            let mut parts: Vec<&str> = vec![d.name.as_deref()?];
            let mut p = d.parent;
            while p != NONE {
                let pd = &t.dies[p as usize];
                if let Some(n) = &pd.name {
                    parts.push(n);
                }
                p = pd.parent;
            }
            parts.reverse();
            Some(parts.join("::"))
        })()?;
        self.names.insert(d.off, name.clone());
        Some(name)
    }

    // ------------------------------------------------------------------ line programs

    fn parse_line(&self, off: u64, hdr: &CuHdr, comp_dir: &str) -> Option<(Vec<RawRow>, Vec<String>)> {
        let sec = self.sec(self.secs.line);
        let mut r = Rd::new(sec, off as usize);
        let mut len = r.un(4)?;
        let o64 = len == 0xffff_ffff;
        if o64 {
            len = r.un(8)?;
        }
        let end = (r.p.checked_add(len as usize)?).min(sec.len());
        let ver = r.un(2)? as u16;
        let mut asz = hdr.addr_size;
        if ver >= 5 {
            asz = r.u8()?;
            r.u8()?; // segment selector size
        }
        let hlen = r.off(o64)? as usize;
        let prog = r.p.checked_add(hlen)?;
        let min_inst = r.u8()? as u64;
        let max_ops = if ver >= 4 { r.u8()?.max(1) as u64 } else { 1 };
        let default_stmt = r.u8()? != 0;
        let _ = default_stmt;
        let line_base = r.u8()? as i8 as i64;
        let line_range = r.u8()?.max(1) as u64;
        let opcode_base = r.u8()?;
        let mut std_len = Vec::new();
        for _ in 1..opcode_base {
            std_len.push(r.u8()?);
        }
        let fake = CuHdr { off: 0, end: 0, ver: 5, addr_size: asz, off64: o64, abbrev_off: 0, die_off: 0 };
        let ctx = Ctx { str_base: 8, addr_base: 8, rng_base: 12, low: 0 };
        let text = |v: Val| -> String {
            match v {
                Val::StrInline(o) => cstr_at(sec, o),
                v => self.str_of(v, &fake, &ctx).unwrap_or_default(),
            }
        };
        let mut dirs: Vec<String> = Vec::new();
        let mut files: Vec<(String, usize)> = Vec::new();
        if ver >= 5 {
            // entry formats: (content type, form)
            let table = |r: &mut Rd| -> Option<Vec<(String, usize)>> {
                let nf = r.u8()? as usize;
                let mut fmt = Vec::new();
                for _ in 0..nf {
                    fmt.push((r.uleb()?, r.uleb()?));
                }
                let n = r.uleb()? as usize;
                let mut out = Vec::new();
                for _ in 0..n {
                    let (mut path, mut dir) = (String::new(), 0usize);
                    for &(ct, form) in &fmt {
                        let v = read_form(r, form, 0, &fake)?;
                        match ct {
                            1 => path = text(v),
                            2 => dir = v.uint().unwrap_or(0) as usize,
                            _ => {}
                        }
                    }
                    out.push((path, dir));
                }
                Some(out)
            };
            dirs = table(&mut r)?.into_iter().map(|d| d.0).collect();
            files = table(&mut r)?;
        } else {
            dirs.push(String::new()); // index 0 is the compilation directory
            loop {
                let (_, s) = r.cstr()?;
                if s.is_empty() {
                    break;
                }
                dirs.push(String::from_utf8_lossy(s).into_owned());
            }
            files.push((String::new(), 0)); // file numbers start at 1
            loop {
                let (_, s) = r.cstr()?;
                if s.is_empty() {
                    break;
                }
                let d = r.uleb()? as usize;
                r.uleb()?;
                r.uleb()?;
                files.push((String::from_utf8_lossy(s).into_owned(), d));
            }
        }
        let join = |dir: &str, name: &str| -> String {
            if dir.is_empty() {
                name.to_string()
            } else {
                format!("{}/{}", dir.trim_end_matches('/'), name)
            }
        };
        let resolve = |name: &str, d: usize, dirs: &[String]| -> String {
            if name.starts_with('/') {
                return name.to_string();
            }
            let dir = dirs.get(d).map(|s| s.as_str()).unwrap_or("");
            let dir = if dir.starts_with('/') || comp_dir.is_empty() { dir.to_string() } else { join(comp_dir, dir) };
            let dir = if dir.is_empty() { comp_dir.to_string() } else { dir };
            join(&dir, name)
        };
        let mut rows = Vec::new();
        r.p = prog;
        let mut st = (0u64, 0u64, 1u32, 1u32, 0u32); // addr, op_index, file, line, col
        let reset = |st: &mut (u64, u64, u32, u32, u32)| *st = (0, 0, 1, 1, 0);
        let adv = |st: &mut (u64, u64, u32, u32, u32), n: u64| {
            if max_ops == 1 {
                st.0 = st.0.wrapping_add(min_inst.wrapping_mul(n));
            } else {
                let t = st.1 + n;
                st.0 = st.0.wrapping_add(min_inst.wrapping_mul(t / max_ops));
                st.1 = t % max_ops;
            }
        };
        let mut define: Vec<(String, usize)> = Vec::new();
        while r.p < end {
            let op = r.u8()?;
            if op >= opcode_base {
                let a = (op - opcode_base) as u64;
                adv(&mut st, a / line_range);
                st.3 = (st.3 as i64 + line_base + (a % line_range) as i64) as u32;
                rows.push(RawRow { addr: st.0, file: st.2, line: st.3, col: st.4, end: false });
                continue;
            }
            match op {
                0 => {
                    let l = r.uleb()? as usize;
                    let at = r.p;
                    let sub = r.u8()?;
                    match sub {
                        1 => {
                            rows.push(RawRow { addr: st.0, file: st.2, line: 0, col: 0, end: true });
                            reset(&mut st);
                        }
                        2 => st.0 = r.un(l.saturating_sub(1))?,
                        3 => {
                            let (_, s) = r.cstr()?;
                            let d = r.uleb()? as usize;
                            define.push((String::from_utf8_lossy(s).into_owned(), d));
                        }
                        _ => {}
                    }
                    r.p = at + l;
                }
                1 => rows.push(RawRow { addr: st.0, file: st.2, line: st.3, col: st.4, end: false }),
                2 => {
                    let n = r.uleb()?;
                    adv(&mut st, n);
                }
                3 => st.3 = (st.3 as i64 + r.sleb()?) as u32,
                4 => st.2 = r.uleb()? as u32,
                5 => st.4 = r.uleb()? as u32,
                8 => adv(&mut st, ((255 - opcode_base) as u64) / line_range),
                9 => {
                    st.0 = st.0.wrapping_add(r.un(2)?);
                    st.1 = 0;
                }
                _ => {
                    for _ in 0..*std_len.get(op as usize - 1)? {
                        r.uleb()?;
                    }
                }
            }
        }
        files.extend(define);
        let names = files.iter().enumerate().map(|(i, (n, d))| if ver < 5 && i == 0 { String::new() } else { resolve(n, *d, &dirs) }).collect();
        Some((rows, names))
    }

    // ------------------------------------------------------------------ per-unit results

    /// Line rows and the inline tree of one compile unit, in binary addresses.
    pub fn cu_data(&mut self, ci: usize, files: &mut Interner) -> Option<Arc<CuData>> {
        if let Some(d) = self.cu_data.get(&ci) {
            return Some(d.clone());
        }
        let hdr = self.cus().get(ci)?.clone();
        let tree = self.tree(ci)?;
        let mut rows: Vec<LineRow> = Vec::new();
        let mut fmap: Vec<u32> = Vec::new();
        if let Some(off) = tree.stmt_list {
            if let Some((raw, names)) = self.parse_line(off, &hdr, &tree.comp_dir) {
                fmap = names.into_iter().map(|n| files.id(n)).collect();
                for r in raw {
                    let a = if r.end {
                        match &self.map {
                            None => Some(r.addr),
                            Some(_) => self.tr(r.addr.wrapping_sub(1)).map(|x| x + 1),
                        }
                    } else {
                        self.tr(r.addr)
                    };
                    if let Some(addr) = a {
                        rows.push(LineRow { addr, file: fmap.get(r.file as usize).copied().unwrap_or(0), line: r.line, col: r.col });
                        if r.end {
                            rows.last_mut().unwrap().line = 0;
                            // sorts before real rows at the same address (see below)
                            rows.last_mut().unwrap().col = u32::MAX;
                        }
                    }
                }
            }
        }
        // end-of-sequence rows (col == MAX) first at equal addresses, then collapse to the last row
        rows.sort_by_key(|r| (r.addr, r.col != u32::MAX));
        let mut out: Vec<LineRow> = Vec::with_capacity(rows.len());
        for r in rows {
            if out.last().map_or(false, |l| l.addr == r.addr) {
                out.pop();
            }
            out.push(r);
        }
        for r in out.iter_mut() {
            if r.line == 0 && r.col == u32::MAX {
                r.col = 0;
            }
        }
        let n = tree.dies.len();
        let mut own = vec![NONE; n];
        let mut dep = vec![0u32; n];
        let mut subs: Vec<SubProg> = Vec::new();
        for i in 0..n {
            let d = &tree.dies[i];
            let (po, pd) = if d.parent == NONE { (NONE, 0) } else { (own[d.parent as usize], dep[d.parent as usize]) };
            match d.tag {
                TAG_SUBPROGRAM if !d.decl && !d.ranges.is_empty() => {
                    own[i] = subs.len() as u32;
                    subs.push(SubProg { ranges: d.ranges.iter().filter_map(|&(l, h)| self.tr_range(l, h)).collect(), inl: Vec::new() });
                }
                TAG_INLINED => {
                    own[i] = po;
                    dep[i] = pd + 1;
                    if po != NONE {
                        let ranges: Vec<(u64, u64)> = d.ranges.iter().filter_map(|&(l, h)| self.tr_range(l, h)).collect();
                        let name = d.origin.and_then(|o| self.locate(o)).and_then(|(cj, j)| self.die_name(cj, j, 0)).unwrap_or_else(|| "?".to_string());
                        let cf = fmap.get(d.call_file as usize).copied().unwrap_or(0);
                        subs[po as usize].inl.push(InlinedCall { ranges, name, call_file: cf, call_line: d.call_line, depth: pd + 1 });
                    }
                }
                TAG_LEXICAL => {
                    own[i] = po;
                    dep[i] = pd;
                }
                _ => {}
            }
        }
        let d = Arc::new(CuData { rows: out, subs });
        self.cu_data.insert(ci, d.clone());
        Some(d)
    }
}

fn cstr_at(b: &[u8], o: usize) -> String {
    let Some(s) = b.get(o..) else { return String::new() };
    let n = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    String::from_utf8_lossy(&s[..n]).into_owned()
}
