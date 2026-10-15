//! Stepping host frames by their DWARF call frame information (Linux x86_64; DESIGN.md §3.3).
//! rustc's release code, std and libc keep no frame pointers there, so a frame-pointer walk stops
//! at the first compiled-in frame. Each loaded module's `.eh_frame_hdr` (its binary search table
//! of FDEs) is found once with `dl_iterate_phdr` (`refresh`, never while a thread is held: that
//! takes the loader lock); `step` then runs on a stopped thread's stack from another thread with
//! no allocation and no lock: it reads the modules' mapped CFI directly and the stack through the
//! caller's fault-free `read`.

use std::sync::atomic::{AtomicPtr, Ordering::{Acquire, Release}};
use relax_session::sync::Mutex;

#[derive(Clone, Copy, Default, Debug)]
pub struct Regs {
    pub rip: u64,
    pub rsp: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
}

struct Module {
    lo: u64,
    hi: u64,
    /// address of the module's .eh_frame_hdr
    hdr: u64,
}

struct Index {
    mods: Vec<Module>,
}

/// The current index (immutable once published; replaced ones are leaked on purpose: a walk may
/// still read them, and there are a handful per process lifetime).
static INDEX: AtomicPtr<Index> = AtomicPtr::new(std::ptr::null_mut());
static REFRESH: Mutex<()> = Mutex::new(());

/// Rebuilds the module index. Allocates and takes the loader lock: only while no thread is held.
pub fn refresh() {
    let _g = REFRESH.lock().unwrap_or_else(|e| e.into_inner());
    use super::symbols::{dl_iterate_phdr, PhdrInfo};
    extern "C" fn cb(info: *const PhdrInfo, _size: usize, data: *mut u8) -> i32 {
        unsafe {
            let out = &mut *(data as *mut Vec<Module>);
            let i = &*info;
            let (mut lo, mut hi, mut hdr) = (u64::MAX, 0u64, 0u64);
            for k in 0..i.phnum as usize {
                let ph = i.phdr.add(k * 56);
                let ty = *(ph as *const u32);
                let flags = *(ph.add(4) as *const u32);
                let vaddr = *(ph.add(16) as *const u64);
                let memsz = *(ph.add(40) as *const u64);
                if ty == 1 && flags & 1 != 0 {
                    lo = lo.min(i.addr + vaddr);
                    hi = hi.max(i.addr + vaddr + memsz);
                }
                if ty == 0x6474_e550 {
                    hdr = i.addr + vaddr; // PT_GNU_EH_FRAME
                }
            }
            if hdr != 0 && lo < hi {
                out.push(Module { lo, hi, hdr });
            }
        }
        0
    }
    let mut mods: Vec<Module> = Vec::new();
    unsafe { dl_iterate_phdr(cb, &mut mods as *mut Vec<Module> as *mut u8) };
    mods.sort_by_key(|m| m.lo);
    INDEX.store(Box::into_raw(Box::new(Index { mods })), Release);
}

fn module(pc: u64) -> Option<&'static Module> {
    let p = INDEX.load(Acquire);
    if p.is_null() {
        return None;
    }
    let idx = unsafe { &*p };
    let k = idx.mods.partition_point(|m| m.lo <= pc);
    let m = idx.mods.get(k.checked_sub(1)?)?;
    (pc < m.hi).then_some(m)
}

/// Is `pc` inside a module the index knows (with CFI)?
pub fn covers(pc: u64) -> bool {
    module(pc).is_some()
}

// ---------------------------------------------------------------- reading CFI

/// A cursor over mapped CFI bytes.
struct Cur {
    p: u64,
}

impl Cur {
    fn u8(&mut self) -> u8 {
        let v = unsafe { *(self.p as *const u8) };
        self.p += 1;
        v
    }
    fn u16(&mut self) -> u16 {
        let v = unsafe { std::ptr::read_unaligned(self.p as *const u16) };
        self.p += 2;
        v
    }
    fn u32(&mut self) -> u32 {
        let v = unsafe { std::ptr::read_unaligned(self.p as *const u32) };
        self.p += 4;
        v
    }
    fn u64(&mut self) -> u64 {
        let v = unsafe { std::ptr::read_unaligned(self.p as *const u64) };
        self.p += 8;
        v
    }
    fn uleb(&mut self) -> u64 {
        let (mut v, mut shift) = (0u64, 0u32);
        loop {
            let b = self.u8();
            if shift < 64 {
                v |= ((b & 0x7f) as u64) << shift;
            }
            shift += 7;
            if b & 0x80 == 0 {
                return v;
            }
        }
    }
    fn sleb(&mut self) -> i64 {
        let (mut v, mut shift) = (0i64, 0u32);
        loop {
            let b = self.u8();
            if shift < 64 {
                v |= ((b & 0x7f) as i64) << shift;
            }
            shift += 7;
            if b & 0x80 == 0 {
                if shift < 64 && b & 0x40 != 0 {
                    v |= -1i64 << shift;
                }
                return v;
            }
        }
    }
    /// A DW_EH_PE-encoded pointer; `data` is the base of datarel encodings. None: unsupported.
    fn enc(&mut self, enc: u8, data: u64) -> Option<u64> {
        if enc == 0xff {
            return Some(0); // DW_EH_PE_omit
        }
        let at = self.p;
        let v = match enc & 0x0f {
            0x00 => self.u64(),
            0x01 => self.uleb(),
            0x02 => self.u16() as u64,
            0x03 => self.u32() as u64,
            0x04 => self.u64(),
            0x09 => self.sleb() as u64,
            0x0a => self.u16() as i16 as i64 as u64,
            0x0b => self.u32() as i32 as i64 as u64,
            0x0c => self.u64(),
            _ => return None,
        };
        let v = match enc & 0x70 {
            0x00 => v,
            0x10 => v.wrapping_add(at),   // pcrel
            0x30 => v.wrapping_add(data), // datarel
            _ => return None,
        };
        // DW_EH_PE_indirect is not used for FDE addresses
        (enc & 0x80 == 0).then_some(v)
    }
}

/// The FDE covering `pc` (binary search of the module's .eh_frame_hdr table).
fn find_fde(m: &Module, pc: u64) -> Option<u64> {
    let mut c = Cur { p: m.hdr };
    let version = c.u8();
    let ptr_enc = c.u8();
    let count_enc = c.u8();
    let table_enc = c.u8();
    if version != 1 {
        return None;
    }
    c.enc(ptr_enc, m.hdr)?;
    let count = c.enc(count_enc, m.hdr)?;
    // only the fixed-size encodings make a searchable table (gcc / lld emit datarel sdata4)
    if table_enc != 0x3b {
        return None;
    }
    let table = c.p;
    let entry = |i: u64| -> (u64, u64) {
        let p = table + i * 8;
        let a = unsafe { std::ptr::read_unaligned(p as *const i32) } as i64 as u64;
        let f = unsafe { std::ptr::read_unaligned((p + 4) as *const i32) } as i64 as u64;
        (m.hdr.wrapping_add(a), m.hdr.wrapping_add(f))
    };
    let (mut lo, mut hi) = (0u64, count);
    while lo < hi {
        let mid = (lo + hi) / 2;
        if entry(mid).0 <= pc {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if lo == 0 {
        return None;
    }
    Some(entry(lo - 1).1)
}

#[derive(Clone, Copy, PartialEq)]
enum Rule {
    Same,
    Offset(i64),
    ValOffset(i64),
    Reg(u16),
    Undef,
}

const NREG: usize = 17;

#[derive(Clone, Copy)]
struct Row {
    cfa_reg: u16,
    cfa_off: i64,
    rules: [Rule; NREG],
}

struct Cie {
    code_align: u64,
    data_align: i64,
    ra: u16,
    fde_enc: u8,
    has_aug: bool,
    init: (u64, u64),
}

fn parse_cie(at: u64) -> Option<Cie> {
    let mut c = Cur { p: at };
    let mut len = c.u32() as u64;
    if len == 0xffff_ffff {
        len = c.u64();
    }
    let end = c.p + len;
    let _id = c.u32();
    let version = c.u8();
    let mut aug = [0u8; 8];
    let mut n = 0;
    loop {
        let b = c.u8();
        if b == 0 {
            break;
        }
        if n < aug.len() {
            aug[n] = b;
            n += 1;
        }
    }
    let aug = &aug[..n];
    if aug.starts_with(b"eh") {
        c.u64();
    }
    let code_align = c.uleb();
    let data_align = c.sleb();
    let ra = if version == 1 { c.u8() as u16 } else { c.uleb() as u16 };
    let mut fde_enc = 0u8;
    let has_aug = aug.first() == Some(&b'z');
    if has_aug {
        let alen = c.uleb();
        let data_end = c.p + alen;
        for &ch in &aug[1..] {
            match ch {
                b'R' => fde_enc = c.u8(),
                b'P' => {
                    let e = c.u8();
                    c.enc(e & 0x7f, 0)?;
                }
                b'L' => {
                    c.u8();
                }
                _ => {}
            }
        }
        c.p = data_end;
    }
    Some(Cie { code_align, data_align, ra, fde_enc, has_aug, init: (c.p, end) })
}

fn run(row: &mut Row, init: &Row, cie: &Cie, from: u64, to: u64, mut loc: u64, target: u64) -> Option<()> {
    let mut c = Cur { p: from };
    let mut stack = [*row; 8];
    let mut depth = 0usize;
    let reg = |r: u64| -> Option<usize> { ((r as usize) < NREG).then_some(r as usize) };
    while c.p < to {
        let op = c.u8();
        let (hi, lo) = (op >> 6, (op & 0x3f) as u64);
        match hi {
            1 => {
                loc += lo * cie.code_align;
                if loc > target {
                    return Some(());
                }
                continue;
            }
            2 => {
                let off = c.uleb() as i64 * cie.data_align;
                if let Some(r) = reg(lo) {
                    row.rules[r] = Rule::Offset(off);
                }
                continue;
            }
            3 => {
                if let Some(r) = reg(lo) {
                    row.rules[r] = init.rules[r];
                }
                continue;
            }
            _ => {}
        }
        match op {
            0x00 => {}
            0x01 => {
                loc = c.enc(cie.fde_enc, 0)?;
                if loc > target {
                    return Some(());
                }
            }
            0x02 | 0x03 | 0x04 => {
                let d = match op {
                    0x02 => c.u8() as u64,
                    0x03 => c.u16() as u64,
                    _ => c.u32() as u64,
                };
                loc += d * cie.code_align;
                if loc > target {
                    return Some(());
                }
            }
            0x05 => {
                let r = c.uleb();
                let off = c.uleb() as i64 * cie.data_align;
                if let Some(r) = reg(r) {
                    row.rules[r] = Rule::Offset(off);
                }
            }
            0x06 => {
                let r = c.uleb();
                if let Some(r) = reg(r) {
                    row.rules[r] = init.rules[r];
                }
            }
            0x07 => {
                let r = c.uleb();
                if let Some(r) = reg(r) {
                    row.rules[r] = Rule::Undef;
                }
            }
            0x08 => {
                let r = c.uleb();
                if let Some(r) = reg(r) {
                    row.rules[r] = Rule::Same;
                }
            }
            0x09 => {
                let r = c.uleb();
                let r2 = c.uleb();
                if let Some(r) = reg(r) {
                    row.rules[r] = Rule::Reg(r2 as u16);
                }
            }
            0x0a => {
                if depth == stack.len() {
                    return None;
                }
                stack[depth] = *row;
                depth += 1;
            }
            0x0b => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
                // the CFA stays as it is (DWARF: restore_state restores register rules)
                let (cr, co) = (row.cfa_reg, row.cfa_off);
                *row = stack[depth];
                row.cfa_reg = cr;
                row.cfa_off = co;
            }
            0x0c => {
                row.cfa_reg = c.uleb() as u16;
                row.cfa_off = c.uleb() as i64;
            }
            0x0d => row.cfa_reg = c.uleb() as u16,
            0x0e => row.cfa_off = c.uleb() as i64,
            0x0f => return None, // def_cfa_expression
            0x10 | 0x16 => {
                // expression / val_expression: the register is not followed
                let r = c.uleb();
                let n = c.uleb();
                c.p += n;
                if let Some(r) = reg(r) {
                    row.rules[r] = Rule::Undef;
                }
            }
            0x11 => {
                let r = c.uleb();
                let off = c.sleb() * cie.data_align;
                if let Some(r) = reg(r) {
                    row.rules[r] = Rule::Offset(off);
                }
            }
            0x12 => {
                row.cfa_reg = c.uleb() as u16;
                row.cfa_off = c.sleb() * cie.data_align;
            }
            0x13 => row.cfa_off = c.sleb() * cie.data_align,
            0x14 => {
                let r = c.uleb();
                let off = c.uleb() as i64 * cie.data_align;
                if let Some(r) = reg(r) {
                    row.rules[r] = Rule::ValOffset(off);
                }
            }
            0x15 => {
                let r = c.uleb();
                let off = c.sleb() * cie.data_align;
                if let Some(r) = reg(r) {
                    row.rules[r] = Rule::ValOffset(off);
                }
            }
            0x2e => {
                c.uleb(); // GNU_args_size
            }
            0x2f => {
                let r = c.uleb();
                let off = -(c.uleb() as i64) * cie.data_align;
                if let Some(r) = reg(r) {
                    row.rules[r] = Rule::Offset(off);
                }
            }
            _ => return None,
        }
    }
    Some(())
}

fn get(r: &Regs, n: u16) -> Option<u64> {
    Some(match n {
        3 => r.rbx,
        6 => r.rbp,
        7 => r.rsp,
        12 => r.r12,
        13 => r.r13,
        14 => r.r14,
        15 => r.r15,
        16 => r.rip,
        _ => return None,
    })
}

fn set(r: &mut Regs, n: usize, v: u64) {
    match n {
        3 => r.rbx = v,
        6 => r.rbp = v,
        12 => r.r12 = v,
        13 => r.r13 = v,
        14 => r.r14 = v,
        15 => r.r15 = v,
        _ => {}
    }
}

/// What a step did.
#[derive(PartialEq, Eq, Debug)]
pub enum Step {
    /// `r` is now the caller's frame
    Done,
    /// the frame's CFA is rbp-based and `r.rbp` is 0 (unknown: a walk that started from a
    /// syscall's sp and pc); `r` is unchanged
    NeedRbp,
    /// no FDE, or rules it cannot follow; `r` is unchanged
    Fail,
}

/// One step from the frame `r` describes to its caller (`is_top`: r.rip is the interrupted
/// instruction itself, otherwise a return address and the lookup uses rip - 1). `r.rbp == 0`
/// means rbp is not known.
pub fn step(r: &mut Regs, is_top: bool, read: &dyn Fn(u64) -> Option<u64>) -> Step {
    let mut need_rbp = false;
    match step_inner(r, is_top, read, &mut need_rbp) {
        Some(()) => Step::Done,
        None if need_rbp => Step::NeedRbp,
        None => Step::Fail,
    }
}

fn step_inner(r: &mut Regs, is_top: bool, read: &dyn Fn(u64) -> Option<u64>, need_rbp: &mut bool) -> Option<()> {
    let pc = if is_top { r.rip } else { r.rip.wrapping_sub(1) };
    let m = module(pc)?;
    let fde = find_fde(m, pc)?;
    let mut c = Cur { p: fde };
    let mut len = c.u32() as u64;
    if len == 0xffff_ffff {
        len = c.u64();
    }
    let end = c.p + len;
    let id_at = c.p;
    let cie_off = c.u32() as u64;
    let cie = parse_cie(id_at - cie_off)?;
    let start = c.enc(cie.fde_enc, 0)?;
    let range = c.enc(cie.fde_enc & 0x0f, 0)?;
    if pc < start || pc >= start + range {
        return None;
    }
    if cie.has_aug {
        let alen = c.uleb();
        c.p += alen;
    }
    let mut row = Row { cfa_reg: 7, cfa_off: 8, rules: [Rule::Same; NREG] };
    let empty = row;
    run(&mut row, &empty, &cie, cie.init.0, cie.init.1, start, u64::MAX)?;
    let init = row;
    run(&mut row, &init, &cie, c.p, end, start, pc)?;
    if row.cfa_reg == 6 && r.rbp == 0 {
        *need_rbp = true;
        return None;
    }
    let cfa = get(r, row.cfa_reg)?.wrapping_add(row.cfa_off as u64);
    let ra = match row.rules[cie.ra as usize % NREG] {
        Rule::Offset(o) => read(cfa.wrapping_add(o as u64))?,
        _ => return None,
    };
    let mut next = *r;
    for n in [3usize, 6, 12, 13, 14, 15] {
        match row.rules[n] {
            Rule::Offset(o) => set(&mut next, n, read(cfa.wrapping_add(o as u64))?),
            Rule::ValOffset(o) => set(&mut next, n, cfa.wrapping_add(o as u64)),
            Rule::Reg(s) => set(&mut next, n, get(r, s)?),
            Rule::Same | Rule::Undef => {}
        }
    }
    next.rsp = cfa;
    next.rip = ra;
    if ra == 0 || cfa <= r.rsp {
        return None;
    }
    *r = next;
    Some(())
}
