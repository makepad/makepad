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
    /// pcs of the module minus this = addresses in the CFI's bytes: 0 for this process's own
    /// modules; another process's module (`Foreign`) reads a copy of its file's CFI
    delta: u64,
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
    refresh_index();
}

pub(super) fn refresh_nowait() {
    if let Ok(_g) = REFRESH.try_lock() { refresh_index(); }
}

fn refresh_index() {
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
                out.push(Module { lo, hi, hdr, delta: 0 });
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
                // the whole row, the CFA with the register rules, as libgcc, LLVM's libunwind and
                // gimli restore it: gcc ends a mid-function epilogue with it (remember_state,
                // `leave` or pops moving the CFA to rsp + 8, ret, restore_state), and the code
                // after that `ret` has the body's CFA again (glibc's pthread_cond_wait, built with
                // frame pointers: rbp + 16; keeping rsp + 8 read a heap pointer as its return
                // address and a layout patch was refused, 11-24)
                *row = stack[depth];
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
    /// the outermost frame: its CFI leaves the return address undefined (a thread's start, the
    /// process's entry) or the return address is 0; `r` is unchanged
    End,
    /// the frame's CFA is rbp-based, `r.rbp` is 0 (unknown: a walk that started from a syscall's
    /// sp and pc) and no call found on the stack proves it ([`rbp_of_its_call`]); `r` is unchanged
    NeedRbp,
    /// the step cannot be proven, and why (no FDE, a rule this walker does not follow, an
    /// unreadable stack); `r` is unchanged
    Fail(&'static str),
}

/// One step from the frame `r` describes to its caller (`is_top`: r.rip is the interrupted
/// instruction itself, otherwise a return address and the lookup uses rip - 1). `r.rbp == 0`
/// means rbp is not known. Only what the frame's CFI says and the stack holds where it says:
/// a step it does not prove is not taken (no frame-pointer or stack-content guess).
pub fn step(r: &mut Regs, is_top: bool, read: &dyn Fn(u64) -> Option<u64>) -> Step {
    let pc = if is_top { r.rip } else { r.rip.wrapping_sub(1) };
    match module(pc) {
        Some(m) => step_in(m, pc, r, read),
        None => Step::Fail("no loaded module with CFI covers it"),
    }
}

/// The frame pointer of the function at [lo, hi) when no frame below saved it (a thread blocked in
/// a syscall gives its sp and pc only, and a libc built with frame pointers bases its CFA on it): the
/// slot above `rsp` holding a return address whose call calls this very function. Earlier calls
/// leave frame records on the stack, so the shape of one proves nothing; the call does. `ra_off`:
/// where the return address is from the frame pointer. Ok(None): no call proves one; an error: a
/// call's stub chain ran past `STUB_HOPS` with nothing else proven.
pub(super) fn rbp_of_its_call(lo: u64, hi: u64, rsp: u64, ra_off: i64, read: &dyn Fn(u64) -> Option<u64>) -> Result<Option<u64>, &'static str> {
    // past a frame's locals: what a libc function blocking in a syscall keeps
    const REACH: u64 = 512;
    let mut why = Ok(None);
    for p in (0..REACH).map(|k| rsp.wrapping_add(k * 8)) {
        let Some(ret) = read(p.wrapping_add(ra_off as u64)) else { continue };
        match call_reaches(ret, lo, hi, read) {
            Ok(true) => return Ok(Some(p)),
            Ok(false) => {}
            Err(e) => why = Err(e),
        }
    }
    why
}

/// Jump stubs a call may go through before the code: the JIT's longest chain is three (a thunk's
/// head, its table path, a count stub). Past this the chain is a cycle or something unknown, and
/// the walk says so (`STUB_CHAIN_TOO_LONG`) rather than that no call was found.
const STUB_HOPS: usize = 5;
pub const STUB_CHAIN_TOO_LONG: &str = "a call above its sp goes through more jump stubs than any known chain (a cycle?), so its frame is not proven";

/// Whether the call before return address `ret` goes to [lo, hi), directly or through jump stubs
/// (a PLT entry, a JIT veneer, a JIT slot's thunk and count stub: `jump_stub_target`); an error
/// past `STUB_HOPS` stubs.
fn call_reaches(ret: u64, lo: u64, hi: u64, read: &dyn Fn(u64) -> Option<u64>) -> Result<bool, &'static str> {
    let Some(before) = read(ret.wrapping_sub(8)) else { return Ok(false) };
    let b = before.to_le_bytes();
    let disp = |at: usize| i32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]) as i64 as u64;
    let mut target = if b[3] == 0xe8 {
        ret.wrapping_add(disp(4))
    } else if b[2] == 0xff && b[3] == 0x15 {
        // call [rip + disp]
        match read(ret.wrapping_add(disp(4))) {
            Some(t) => t,
            None => return Ok(false),
        }
    } else if b[6] == 0xff && (0xd0..=0xd7).contains(&b[7]) {
        let extended = b[5] == 0x41;
        let call = ret.wrapping_sub(if extended { 3 } else { 2 });
        let rex = if extended { 0x4c } else { 0x48 };
        let modrm = ((b[7] & 7) << 3) | 5;
        // 32 bytes leaves room around the observed 13-byte sequence for argument setup.
        // A pattern inside another instruction is accepted only if its target names this very function.
        let load = (7..=32).find_map(|back| {
            let at = call.wrapping_sub(back);
            let w = read(at)?.to_le_bytes();
            if w[0] != rex || !matches!(w[1], 0x8b | 0x8d) || w[2] != modrm {
                return None;
            }
            let disp = i32::from_le_bytes([w[3], w[4], w[5], w[6]]);
            Some((w[1], at.wrapping_add(7).wrapping_add_signed(i64::from(disp))))
        });
        match load {
            Some((0x8b, slot)) => match read(slot) { Some(t) => t, None => return Ok(false) },
            Some((_, addr)) => addr,
            None => return Ok(false),
        }
    } else {
        return Ok(false);
    };
    for _ in 0..=STUB_HOPS {
        if (lo..hi).contains(&target) {
            return Ok(true);
        }
        match jump_stub_target(target, read) {
            Some(t) => target = t,
            None => return Ok(false),
        }
    }
    Err(STUB_CHAIN_TOO_LONG)
}

/// Where a stub at `t` jumps:
/// - `[endbr64] [bnd] jmp [rip + disp]` (a PLT entry, a veneer);
/// - `jmp rel32` (a JIT slot's thunk head: to the code, or to its table path);
/// - `mov rax, slot; jmp [rax]` (that table path: the slot's word);
/// - `mov rax, counter; sub qword [rax], imm32; jle hot; jmp rel32` or `...; mov r11, code;
///   jmp r11` (a tier-up count stub, relax_backend x64::count_stub: on to the code).
/// (`mov reg, imm` in each form the backend's mov_ri gives a nonzero value: `mov_imm`)
fn jump_stub_target(t: u64, read: &dyn Fn(u64) -> Option<u64>) -> Option<u64> {
    let rel32 = |at: u64, b: &[u8]| at.wrapping_add(i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as i64 as u64);
    let mut at = t;
    if read(at)? as u32 == 0xfa1e_0ff3 {
        at = at.wrapping_add(4);
    }
    let w = read(at)?.to_le_bytes();
    if w[0] == 0xe9 {
        return Some(rel32(at.wrapping_add(5), &w[1..5]));
    }
    if let Some((0, imm, len)) = mov_imm(at, read) {
        let at = at.wrapping_add(len);
        let next = read(at)?.to_le_bytes();
        if next[0] == 0xff && next[1] == 0x20 {
            return read(imm);
        }
        if next[..3] != [0x48, 0x81, 0x28] || next[7] != 0x7e {
            return None;
        }
        let jmp = read(at.wrapping_add(9))?.to_le_bytes();
        if jmp[0] == 0xe9 {
            return Some(rel32(at.wrapping_add(14), &jmp[1..5]));
        }
        let (11, code, len) = mov_imm(at.wrapping_add(9), read)? else { return None };
        let far = read(at.wrapping_add(9 + len))?.to_le_bytes();
        return (far[..3] == [0x41, 0xff, 0xe3]).then_some(code);
    }
    let skip = (w[0] == 0xf2) as usize;
    if w[skip] != 0xff || w[skip + 1] != 0x25 {
        return None;
    }
    let disp = i32::from_le_bytes([w[skip + 2], w[skip + 3], w[skip + 4], w[skip + 5]]) as i64 as u64;
    read(at.wrapping_add(skip as u64 + 6).wrapping_add(disp))
}

/// `mov reg, imm` at `at` as relax_backend's x64 mov_ri encodes a nonzero value: `mov r32, imm32`,
/// `mov r64, simm32`, `movabs r64, imm64`. The register, the value and the instruction's length.
fn mov_imm(at: u64, read: &dyn Fn(u64) -> Option<u64>) -> Option<(u8, u64, u64)> {
    let w = read(at)?.to_le_bytes();
    let (rex, op) = if w[0] & 0xf0 == 0x40 { (w[0], 1) } else { (0, 0) };
    let ext = if rex & 1 != 0 { 8 } else { 0 };
    let imm32 = |k: usize| u32::from_le_bytes([w[k], w[k + 1], w[k + 2], w[k + 3]]);
    match (rex & 0x08 != 0, w[op]) {
        (false, 0xb8..=0xbf) => Some((ext + (w[op] & 7), imm32(op + 1) as u64, op as u64 + 5)),
        (true, 0xb8..=0xbf) => Some((ext + (w[op] & 7), read(at.wrapping_add(2))?, 10)),
        (true, 0xc7) if w[2] & 0xf8 == 0xc0 => Some((ext + (w[2] & 7), imm32(3) as i32 as i64 as u64, 7)),
        _ => None,
    }
}

/// One step of the frame at `pc` (the lookup pc: rip, or rip - 1 for a return address) in module `m`.
fn step_in(m: &Module, pc: u64, r: &mut Regs, read: &dyn Fn(u64) -> Option<u64>) -> Step {
    const UNFOLLOWED: &str = "its CFI uses a rule this walker does not follow";
    // the CFI's address of pc (another process's module: its copy here)
    let pc = pc.wrapping_sub(m.delta);
    let Some(fde) = find_fde(m, pc) else { return Step::Fail("no FDE of its module covers it") };
    let mut c = Cur { p: fde };
    let mut len = c.u32() as u64;
    if len == 0xffff_ffff {
        len = c.u64();
    }
    let end = c.p + len;
    let id_at = c.p;
    let cie_off = c.u32() as u64;
    let Some(cie) = parse_cie(id_at - cie_off) else { return Step::Fail(UNFOLLOWED) };
    let (Some(start), Some(range)) = (c.enc(cie.fde_enc, 0), c.enc(cie.fde_enc & 0x0f, 0)) else { return Step::Fail(UNFOLLOWED) };
    if pc < start || pc >= start + range {
        return Step::Fail("no FDE of its module covers it");
    }
    if cie.has_aug {
        let alen = c.uleb();
        c.p += alen;
    }
    let mut row = Row { cfa_reg: 7, cfa_off: 8, rules: [Rule::Same; NREG] };
    let empty = row;
    if run(&mut row, &empty, &cie, cie.init.0, cie.init.1, start, u64::MAX).is_none() {
        return Step::Fail(UNFOLLOWED);
    }
    let init = row;
    if run(&mut row, &init, &cie, c.p, end, start, pc).is_none() {
        return Step::Fail(UNFOLLOWED);
    }
    // the CFA from rsp or rbp only (the rules rustc, gcc and clang emit); rbp 0 is not known
    let cfa = match row.cfa_reg {
        7 => r.rsp,
        6 if r.rbp == 0 => {
            let Rule::Offset(o) = row.rules[cie.ra as usize % NREG] else { return Step::NeedRbp };
            match rbp_of_its_call(start.wrapping_add(m.delta), start.wrapping_add(m.delta).wrapping_add(range), r.rsp, row.cfa_off as i64 + o as i64, read) {
                Ok(Some(rbp)) => rbp,
                Ok(None) => return Step::NeedRbp,
                Err(why) => return Step::Fail(why),
            }
        }
        6 => r.rbp,
        _ => return Step::Fail("its CFA is based on a register other than rsp or rbp"),
    }
    .wrapping_add(row.cfa_off as u64);
    let ra = match row.rules[cie.ra as usize % NREG] {
        Rule::Offset(o) => match read(cfa.wrapping_add(o as u64)) {
            Some(ra) => ra,
            None => return Step::Fail("its return address slot is unreadable"),
        },
        Rule::Undef => return Step::End,
        _ => return Step::Fail("its return address rule is not a stack slot"),
    };
    if ra == 0 {
        return Step::End;
    }
    if cfa <= r.rsp {
        return Step::Fail("its CFA is not above its stack pointer");
    }
    let mut next = *r;
    for n in [3usize, 6, 12, 13, 14, 15] {
        match row.rules[n] {
            Rule::Offset(o) => match read(cfa.wrapping_add(o as u64)) {
                Some(v) => set(&mut next, n, v),
                None => return Step::Fail("a saved register's slot is unreadable"),
            },
            Rule::ValOffset(o) => set(&mut next, n, cfa.wrapping_add(o as u64)),
            // a register the walk does not know stays unknown (rbp 0)
            Rule::Reg(s) => set(&mut next, n, get(r, s).unwrap_or(0)),
            Rule::Undef => set(&mut next, n, 0),
            Rule::Same => {}
        }
    }
    next.rsp = cfa;
    next.rip = ra;
    *r = next;
    Step::Done
}

/// The CFI of another process's modules (`vsnative pid=`: a native build's stacks walked here):
/// each mapped ELF file's `.eh_frame_hdr` + `.eh_frame` copied from its file, at the process's
/// load addresses. Read-only after `open`; `step` reads only these copies and the given stack.
pub struct Foreign {
    mods: Vec<Module>,
    /// the copies the modules' `hdr` point into (never moved: boxed slices)
    _bytes: Vec<Box<[u8]>>,
    /// (lo, hi, file) of each module, for naming pcs
    pub files: Vec<(u64, u64, String)>,
}

impl Foreign {
    /// The executable mappings of process `pid` (from /proc/pid/maps) whose files have CFI.
    pub fn open(pid: u32) -> Result<Foreign, String> {
        let maps = std::fs::read_to_string(format!("/proc/{}/maps", pid)).map_err(|e| format!("cannot read /proc/{}/maps: {}", pid, e))?;
        // per file: its mappings (start, end, file offset)
        let mut by_file: Vec<(String, Vec<(u64, u64, u64)>)> = Vec::new();
        for l in maps.lines() {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 6 || !f[5].starts_with('/') {
                continue;
            }
            let (Some((a, b)), Ok(o)) = (f[0].split_once('-'), u64::from_str_radix(f[2], 16)) else { continue };
            let (Ok(a), Ok(b)) = (u64::from_str_radix(a, 16), u64::from_str_radix(b, 16)) else { continue };
            match by_file.iter_mut().find(|x| x.0 == f[5]) {
                Some(x) => x.1.push((a, b, o)),
                None => by_file.push((f[5].to_string(), vec![(a, b, o)])),
            }
        }
        let mut mods = Vec::new();
        let mut bytes: Vec<Box<[u8]>> = Vec::new();
        let mut files = Vec::new();
        for (path, maps) in by_file {
            let Some(img) = super::natbin::elf_cfi(&path) else { continue };
            // the load bias: a mapping at file offset o sits where the PT_LOAD holding o says
            let Some(bias) = maps.iter().find_map(|&(a, _, o)| img.loads.iter().find(|l| o >= l.0 && o < l.0 + l.2.max(1)).map(|l| a.wrapping_sub(l.1 + (o - l.0)))) else { continue };
            let lo = maps.iter().map(|m| m.0).min().unwrap_or(0);
            let hi = maps.iter().map(|m| m.1).max().unwrap_or(0);
            let b: Box<[u8]> = img.bytes.into_boxed_slice();
            // address here of the CFI's vaddr v: b + (v - img.lo)
            let local_base = (b.as_ptr() as u64).wrapping_sub(img.lo);
            mods.push(Module { lo, hi, hdr: local_base.wrapping_add(img.hdr), delta: bias.wrapping_sub(local_base) });
            files.push((lo, hi, path));
            bytes.push(b);
        }
        if mods.is_empty() {
            return Err(format!("process {}: no mapped file with CFI", pid));
        }
        mods.sort_by_key(|m| m.lo);
        files.sort_by_key(|f| f.0);
        Ok(Foreign { mods, _bytes: bytes, files })
    }

    fn module(&self, pc: u64) -> Option<&Module> {
        let k = self.mods.partition_point(|m| m.lo <= pc);
        let m = self.mods.get(k.checked_sub(1)?)?;
        (pc < m.hi).then_some(m)
    }

    /// `step` for a frame of that process (`read`: its copied stack).
    pub fn step(&self, r: &mut Regs, is_top: bool, read: &dyn Fn(u64) -> Option<u64>) -> Step {
        let pc = if is_top { r.rip } else { r.rip.wrapping_sub(1) };
        match self.module(pc) {
            Some(m) => step_in(m, pc, r, read),
            None => Step::Fail("no mapped file with CFI covers it"),
        }
    }
}

#[cfg(test)]
mod run_tests {
    use super::*;

    /// glibc's pthread_cond_wait (2.44, frame pointers): the body's CFA is rbp + 16; an early
    /// return's epilogue remembers the state, moves the CFA to rsp + 8 for its `ret`, and the code
    /// after the `ret` restores the state, the CFA with it. A step anywhere in the rest of the
    /// body needs rbp + 16.
    #[test]
    fn restore_state_brings_back_the_cfa_with_the_rules() {
        let cie = Cie { code_align: 1, data_align: -8, ra: 16, fde_enc: 0, has_aug: false, init: (0, 0) };
        let mut init = Row { cfa_reg: 7, cfa_off: 8, rules: [Rule::Same; NREG] };
        init.rules[16] = Rule::Offset(-8);
        let program: &[u8] = &[
            0x40 | 5, 0x0e, 16, 0x80 | 6, 2, // +5 push rbp: CFA rsp + 16, rbp at CFA - 16
            0x40 | 3, 0x0d, 6, //               +8 mov rbp, rsp: CFA rbp + 16
            0x40 | 9, 0x80 | 3, 7, //            +17 pushes: rbx at CFA - 56
            0x02, 0xcf, 0x0a, 0x0c, 7, 8, //   +224 an early return: remember, CFA rsp + 8
            0x40 | 8, 0x0b, //                  +232 after its ret: restore
        ];
        let at = |target: u64| {
            let mut row = init;
            run(&mut row, &init, &cie, program.as_ptr() as u64, program.as_ptr() as u64 + program.len() as u64, 0, target).unwrap();
            (row.cfa_reg, row.cfa_off, row.rules[3] == Rule::Offset(-56), row.rules[6] == Rule::Offset(-16))
        };
        assert_eq!(at(0x20), (6, 16, true, true), "the body: rbp + 16, rbx and rbp saved");
        assert_eq!(at(0xe4), (7, 8, true, true), "the early return's ret: rsp + 8");
        assert_eq!(at(0x13b), (6, 16, true, true), "after that ret: the body's CFA and rules again");
    }
}

#[cfg(test)]
mod frame_of_its_call_tests {
    use super::*;
    use std::collections::HashMap;

    /// Bytes at addresses, read as `read` reads them: 8 bytes, None past what is mapped.
    fn memory(parts: &[(u64, Vec<u8>)]) -> impl Fn(u64) -> Option<u64> {
        let mut m = HashMap::new();
        for (at, bytes) in parts {
            for (k, b) in bytes.iter().enumerate() {
                m.insert(at + k as u64, *b);
            }
        }
        move |a| {
            let mut w = [0u8; 8];
            for (k, b) in w.iter_mut().enumerate() {
                *b = *m.get(&(a + k as u64))?;
            }
            Some(u64::from_le_bytes(w))
        }
    }

    fn call_to(ret: u64, target: u64) -> (u64, Vec<u8>) {
        let mut code = vec![0x90, 0x90, 0x90, 0xe8];
        code.extend_from_slice(&(target.wrapping_sub(ret) as u32).to_le_bytes());
        (ret - 8, code)
    }

    const FUNC: (u64, u64) = (0x5000, 0x5100);
    const SP: u64 = 0x9000;

    fn stack(words: &[u64]) -> (u64, Vec<u8>) {
        (SP, words.iter().flat_map(|w| w.to_le_bytes()).collect())
    }

    #[test]
    fn the_frame_record_is_the_one_whose_call_calls_this_function() {
        // locals, then a stale record of a call to something else, then the function's own
        let (stale, own) = (0x7000u64, 0x7100u64);
        let read = memory(&[stack(&[1, 2, 0xa000, stale, 0xa010, own, 3]), call_to(stale, 0x6000), call_to(own, 0x5010)]);
        assert_eq!(rbp_of_its_call(FUNC.0, FUNC.1, SP, 8, &read), Ok(Some(SP + 4 * 8)));
    }

    #[test]
    fn the_call_may_go_through_stubs_or_a_slot() {
        // a JIT veneer: jmp [rip + 0], then the target
        let (ret, veneer) = (0x7000u64, 0x6000u64);
        let mut stub = vec![0xff, 0x25, 0, 0, 0, 0];
        stub.extend_from_slice(&0x5008u64.to_le_bytes());
        stub.extend_from_slice(&[0; 8]);
        let read = memory(&[stack(&[0xa000, ret, 0]), call_to(ret, veneer), (veneer, stub)]);
        assert_eq!(rbp_of_its_call(FUNC.0, FUNC.1, SP, 8, &read), Ok(Some(SP)));
        // a PLT entry with its endbr64 and bnd prefix
        let mut plt = vec![0xf3, 0x0f, 0x1e, 0xfa, 0xf2, 0xff, 0x25];
        plt.extend_from_slice(&0x100i32.to_le_bytes());
        plt.extend_from_slice(&[0; 16]);
        let read = memory(&[stack(&[0xa000, ret, 0]), call_to(ret, veneer), (veneer, plt), (veneer + 11 + 0x100, 0x5020u64.to_le_bytes().to_vec())]);
        assert_eq!(rbp_of_its_call(FUNC.0, FUNC.1, SP, 8, &read), Ok(Some(SP)));
        // call [rip + disp]
        let mut code = vec![0x90, 0x90, 0xff, 0x15];
        code.extend_from_slice(&0x40i32.to_le_bytes());
        let read = memory(&[stack(&[0xa000, ret, 0]), (ret - 8, code), (ret + 0x40, 0x5030u64.to_le_bytes().to_vec())]);
        assert_eq!(rbp_of_its_call(FUNC.0, FUNC.1, SP, 8, &read), Ok(Some(SP)));

        let mut code = vec![0x48, 0x8b, 0x05, 0x39, 0, 0, 0]; // mov rax, [rip + 0x39] (slot at +0x40)
        code.extend_from_slice(&[0x89, 0xdf, 0x89, 0xde, 0xff, 0xd0]); // mov edi, ebx; mov esi, ebx; call rax
        let rax_ret = ret + code.len() as u64;
        code.extend_from_slice(&[0x4c, 0x8d, 0x1d]); // lea r11, [rip + disp]
        let lea_end = ret + code.len() as u64 + 4;
        code.extend_from_slice(&(FUNC.0.wrapping_sub(lea_end) as u32).to_le_bytes());
        code.extend_from_slice(&[0x41, 0xff, 0xd3]);
        let r11_ret = ret + code.len() as u64;
        code.extend_from_slice(&[0x48, 0x8d, 0x05, 0, 0, 0, 0, 0xff, 0xd0]); // lea rax, [rip]; call rax
        let other_ret = ret + code.len() as u64;
        let read = memory(&[(ret, code), (ret + 0x40, FUNC.0.to_le_bytes().to_vec())]);
        assert_eq!(call_reaches(rax_ret, FUNC.0, FUNC.1, &read), Ok(true), "mov from slot into rax across two instructions");
        assert_eq!(call_reaches(r11_ret, FUNC.0, FUNC.1, &read), Ok(true), "lea into r11 with a backward displacement");
        assert_eq!(call_reaches(other_ret, FUNC.0, FUNC.1, &read), Ok(false), "the nearer load into rax names another function");

        // a JIT call: the slot's thunk (relax_backend's own encodings), its head straight to the
        // code, or to its table path, whose word names a count stub that goes on to the code
        use relax_backend::x64::{count_stub, thunk, thunk_head};
        let thunk_at = 0x6000u64;
        let mut direct = thunk(0x6800);
        direct[..8].copy_from_slice(&thunk_head(thunk_at, FUNC.0).to_le_bytes());
        let read = memory(&[stack(&[0xa000, ret, 0]), call_to(ret, thunk_at), (thunk_at, direct)]);
        assert_eq!(rbp_of_its_call(FUNC.0, FUNC.1, SP, 8, &read), Ok(Some(SP)), "a thunk's head jumps to the code");
        // a count stub in reach of the code jumps by rel32, one far from it through r11; a low
        // table word or counter takes mov_ri's short form, a high one its imm64 form
        let high = (0x6100_0000_0000u64, 0x6100_0000_0100u64);
        let cases = [
            (0x6800u64, 0x5800u64, 0x7100u64, FUNC, [0xe9u8, 0], "a low word and counter, the stub's jmp rel32"),
            (0x5f00_0000_6800, 0x5f00_0000_0000, 0x7100_0000_0000, FUNC, [0x41, 0xbb], "a high word and counter, the stub's mov r11, imm32; jmp r11"),
            (0x5f00_0000_6800, 0x5f00_0000_0000, 0x7100_0000_0000, high, [0x49, 0xbb], "high code, the stub's movabs r11; jmp r11"),
        ];
        for (word, stub_at, counter, code, form, what) in cases {
            let mut table = thunk(word);
            table[..8].copy_from_slice(&thunk_head(thunk_at, 0).to_le_bytes());
            let stub = count_stub(stub_at, counter, 3, code.0, 0x7200_0000_0000, 9);
            let jmp_at = if counter >> 32 == 0 { 14 } else { 19 };
            assert!(stub[jmp_at] == form[0] && (form[1] == 0 || stub[jmp_at + 1] == form[1]), "{what}: the encoding this case is about");
            let read = memory(&[stack(&[0xa000, ret, 0]), call_to(ret, thunk_at), (thunk_at, table), (word, stub_at.to_le_bytes().to_vec()), (stub_at, stub)]);
            assert_eq!(rbp_of_its_call(code.0, code.1, SP, 8, &read), Ok(Some(SP)), "the thunk's table path and {what} reach the code");
        }
        // a thunk whose head jumps to itself: a cycle is reported as one, not as no call found
        let mut cycle = thunk(0x6800);
        cycle[..8].copy_from_slice(&thunk_head(thunk_at, thunk_at).to_le_bytes());
        let read = memory(&[stack(&[0xa000, ret, 0]), call_to(ret, thunk_at), (thunk_at, cycle)]);
        assert_eq!(rbp_of_its_call(FUNC.0, FUNC.1, SP, 8, &read), Err(STUB_CHAIN_TOO_LONG), "a stub cycle names itself");
    }

    #[test]
    fn nothing_proven_is_not_a_frame_pointer() {
        // a call through a register with no known target, and words that are no code
        let ret = 0x7000u64;
        let read = memory(&[stack(&[0xa000, ret, 5, 6]), (ret - 8, vec![0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0xff, 0xd0])]);
        assert_eq!(rbp_of_its_call(FUNC.0, FUNC.1, SP, 8, &read), Ok(None));
    }
}
