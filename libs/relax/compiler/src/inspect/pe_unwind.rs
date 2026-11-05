//! Stepping host frames by their PE unwind info (Windows x64; DESIGN.md §3.3), under cfi.rs's
//! interface (`Regs`, `refresh`, `covers`, `step`). Each loaded module's exception directory
//! (.pdata: RUNTIME_FUNCTION entries sorted by address) names its functions' UNWIND_INFO, read the
//! way the system unwinder reads it (RtlVirtualUnwind): the prolog's unwind codes up to the pc, an
//! epilog by its instructions (the interrupted frame only), chained infos, machine frames; a pc
//! no entry covers is a leaf's (its return address at rsp).
//!
//! The module list is taken by `refresh` (K32EnumProcessModules: the loader's list read the way
//! another process's is, without the loader lock), never while a thread is suspended. `step` runs
//! on a suspended thread's stack from another thread with no allocation and no lock
//! (RtlLookupFunctionEntry and RtlVirtualUnwind take the function tables' locks, which the
//! suspended thread may hold); every read, of the stack and of a module's mapped headers, unwind
//! data and code alike, fails instead of faulting (ReadProcessMemory).

use makepad_windows_sys::Win32::Foundation::HMODULE;
use makepad_windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use makepad_windows_sys::Win32::System::ProcessStatus::{K32EnumProcessModules, K32GetModuleFileNameExW};
use makepad_windows_sys::Win32::System::Threading::GetCurrentProcess;
use relax_session::sync::Mutex;
use std::sync::atomic::{AtomicPtr, Ordering::{Acquire, Release}};

/// The registers a walk follows: the Win64 callee-saved ones (the others do not survive a call).
#[derive(Clone, Copy, Default, Debug)]
pub struct Regs {
    pub rip: u64,
    pub rsp: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
}

impl Regs {
    /// Register `n` by its unwind code number (0 rax .. 15 r15); None: a volatile one.
    fn get(&self, n: u8) -> Option<u64> {
        Some(match n {
            3 => self.rbx,
            4 => self.rsp,
            5 => self.rbp,
            6 => self.rsi,
            7 => self.rdi,
            12 => self.r12,
            13 => self.r13,
            14 => self.r14,
            15 => self.r15,
            _ => return None,
        })
    }
    fn set(&mut self, n: u8, v: u64) {
        match n {
            3 => self.rbx = v,
            4 => self.rsp = v,
            5 => self.rbp = v,
            6 => self.rsi = v,
            7 => self.rdi = v,
            12 => self.r12 = v,
            13 => self.r13 = v,
            14 => self.r14 = v,
            15 => self.r15 = v,
            _ => {}
        }
    }
}

/// What a step did.
#[derive(PartialEq, Eq, Debug)]
pub enum Step {
    /// `r` is now the caller's frame
    Done,
    /// the outermost frame: its return address is 0 (a thread's start); `r` is unchanged
    End,
    /// the step cannot be proven, and why; `r` is unchanged
    Fail(&'static str),
}

struct Module {
    lo: u64,
    hi: u64,
    /// the exception directory: address of its RUNTIME_FUNCTION entries, how many
    funcs: u64,
    n: u32,
}

struct Index {
    mods: Vec<Module>,
}

/// The current index (immutable once published; a replaced one is leaked on purpose: a walk may
/// still read it). Republished only when the loaded modules changed.
static INDEX: AtomicPtr<Index> = AtomicPtr::new(std::ptr::null_mut());
static REFRESH: Mutex<()> = Mutex::new(());

/// Reads `buf.len()` bytes at `addr`; false where any is unreadable.
fn peek(addr: u64, buf: &mut [u8]) -> bool {
    let mut got = 0usize;
    let ok = unsafe { ReadProcessMemory(GetCurrentProcess(), addr as *const std::ffi::c_void, buf.as_mut_ptr() as *mut std::ffi::c_void, buf.len(), Some(&mut got)) }.is_ok();
    ok && got == buf.len()
}

fn u8_at(addr: u64) -> Option<u8> {
    let mut b = [0u8; 1];
    peek(addr, &mut b).then_some(b[0])
}

fn u16_at(addr: u64) -> Option<u16> {
    let mut b = [0u8; 2];
    peek(addr, &mut b).then_some(u16::from_le_bytes(b))
}

fn u32_at(addr: u64) -> Option<u32> {
    let mut b = [0u8; 4];
    peek(addr, &mut b).then_some(u32::from_le_bytes(b))
}

/// The base addresses of the loaded modules.
pub(crate) fn module_bases() -> Vec<u64> {
    let me = unsafe { GetCurrentProcess() };
    let mut hs: Vec<HMODULE> = vec![HMODULE::default(); 256];
    loop {
        let mut needed = 0u32;
        let bytes = (hs.len() * std::mem::size_of::<HMODULE>()) as u32;
        if !unsafe { K32EnumProcessModules(me, hs.as_mut_ptr(), bytes, &mut needed) }.as_bool() {
            return Vec::new();
        }
        let n = needed as usize / std::mem::size_of::<HMODULE>();
        if n <= hs.len() {
            return hs[..n].iter().map(|h| h.0 as u64).collect();
        }
        hs = vec![HMODULE::default(); n + 16];
    }
}

/// A module's file (K32GetModuleFileNameExW: no loader lock).
pub(crate) fn module_path(base: u64) -> String {
    let mut buf = [0u16; 1024];
    let n = unsafe { K32GetModuleFileNameExW(Some(GetCurrentProcess()), Some(HMODULE(base as *mut std::ffi::c_void)), makepad_windows_sys::core::PWSTR(buf.as_mut_ptr()), buf.len() as u32) } as usize;
    String::from_utf16_lossy(&buf[..n.min(buf.len())])
}

/// A PE32+ image's size and exception directory (address, entries), from its mapped headers.
pub(crate) fn image_layout(base: u64) -> Option<(u64, u64, u32)> {
    if u16_at(base)? != 0x5a4d {
        return None;
    }
    let pe = base + u32_at(base + 0x3c)? as u64;
    // "PE\0\0", the file header (20 bytes), then the optional header: PE32+ magic 0x20b
    if u32_at(pe)? != 0x4550 || u16_at(pe + 24)? != 0x20b {
        return None;
    }
    let opt = pe + 24;
    let size = u32_at(opt + 56)? as u64;
    // data directory 3 (exceptions) when the header has that many
    let (rva, len) = if u32_at(opt + 108)? > 3 { (u32_at(opt + 112 + 3 * 8)?, u32_at(opt + 112 + 3 * 8 + 4)?) } else { (0, 0) };
    Some((size, if rva == 0 { 0 } else { base + rva as u64 }, len / 12))
}

/// Rebuilds the module index when the loaded modules changed. Allocates: only while no thread is
/// suspended.
pub fn refresh() {
    let _g = REFRESH.lock().unwrap_or_else(|e| e.into_inner());
    let mut mods: Vec<Module> = module_bases()
        .into_iter()
        .filter_map(|lo| {
            let (size, funcs, n) = image_layout(lo)?;
            Some(Module { lo, hi: lo + size, funcs, n })
        })
        .collect();
    mods.sort_by_key(|m| m.lo);
    let cur = INDEX.load(Acquire);
    if !cur.is_null() {
        let old = unsafe { &*cur };
        if old.mods.len() == mods.len() && old.mods.iter().zip(&mods).all(|(a, b)| a.lo == b.lo && a.hi == b.hi) {
            return;
        }
    }
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

/// Is `pc` inside a loaded module the index knows?
pub fn covers(pc: u64) -> bool {
    module(pc).is_some()
}

/// A RUNTIME_FUNCTION: begin, end, unwind info (RVAs).
#[derive(Clone, Copy)]
struct Func {
    begin: u32,
    end: u32,
    unwind: u32,
}

/// The entry of module `m` covering `rva` (a binary search of the sorted .pdata).
fn function(m: &Module, rva: u32) -> Option<Func> {
    let at = |k: u32| -> Option<Func> {
        let e = m.funcs + k as u64 * 12;
        Some(Func { begin: u32_at(e)?, end: u32_at(e + 4)?, unwind: u32_at(e + 8)? })
    };
    let (mut lo, mut hi) = (0u32, m.n);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if u32_at(m.funcs + mid as u64 * 12)? <= rva {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    let f = at(lo.checked_sub(1)?)?;
    if rva >= f.end {
        return None;
    }
    // unwind data with the low bit set names another entry, whose info this one shares
    if f.unwind & 1 != 0 {
        let e = m.lo + (f.unwind & !1) as u64;
        return Some(Func { begin: u32_at(e)?, end: u32_at(e + 4)?, unwind: u32_at(e + 8)? });
    }
    Some(f)
}

const UNREADABLE: &str = "its unwind info is unreadable";
const SLOT: &str = "a stack slot its unwind info names is unreadable";

/// One step from the frame `r` describes to its caller (`is_top`: r.rip is the interrupted
/// instruction itself, otherwise a return address and the lookup uses rip - 1). Only what the
/// function's unwind info says and the stack holds where it says.
pub fn step(r: &mut Regs, is_top: bool, read: &dyn Fn(u64) -> Option<u64>) -> Step {
    let pc = if is_top { r.rip } else { r.rip.wrapping_sub(1) };
    let Some(m) = module(pc) else { return Step::Fail("no loaded module covers it") };
    let mut c = *r;
    let res = match function(m, (pc - m.lo) as u32) {
        Some(f) => unwind(m, f, is_top, &mut c, read),
        // a leaf: no unwind info, so it moved no stack pointer and saved nothing. Only a leaf
        // may lack it: a frame that made a call has some
        None if is_top => read(c.rsp).map(|ra| {
            c.rip = ra;
            c.rsp += 8;
        }).ok_or(SLOT),
        None => Err("no unwind info of its module covers it (a frame that made a call has some)"),
    };
    if let Err(why) = res {
        return Step::Fail(why);
    }
    if c.rip == 0 {
        return Step::End;
    }
    if c.rsp <= r.rsp {
        return Step::Fail("its unwind info puts its caller's frame below it");
    }
    *r = c;
    Step::Done
}

/// RtlVirtualUnwind's walk of function `f`'s unwind info (and the infos it chains to) on `c`.
fn unwind(m: &Module, mut f: Func, is_top: bool, c: &mut Regs, read: &dyn Fn(u64) -> Option<u64>) -> Result<(), &'static str> {
    let rip = c.rip;
    let mut mach = false;
    // chains are short (a function split in parts): a cycle is malformed info
    for depth in 0..32 {
        let info = m.lo + f.unwind as u64;
        let mut h = [0u8; 4];
        if !peek(info, &mut h) {
            return Err(UNREADABLE);
        }
        let (version, flags, prolog, count) = (h[0] & 7, h[0] >> 3, h[1] as u64, h[2] as usize);
        let (freg, foff) = (h[3] & 15, (h[3] >> 4) as u64 * 16);
        if version != 1 && version != 2 {
            return Err("its unwind info is of a version this walker does not read");
        }
        // the codes (an even count of slots), then a chained entry when UNW_FLAG_CHAININFO
        let slots = (count + 1) & !1;
        let chained = flags & 4 != 0;
        let mut raw = [0u8; 2 * (256 + 6)];
        let want = 2 * slots + if chained { 12 } else { 0 };
        if !peek(info + 4, &mut raw[..want]) {
            return Err(UNREADABLE);
        }
        let code = |k: usize| u16::from_le_bytes([raw[2 * k], raw[2 * k + 1]]);
        let start = m.lo + f.begin as u64;
        // in the prolog: only the codes of the instructions it ran so far apply
        let done = if rip >= start && rip < start + prolog { rip - start } else { u64::MAX };
        if depth == 0 && is_top && done == u64::MAX && count != 0 && in_epilog(m, f, rip) {
            return run_epilog(m, f, rip, c, read);
        }
        // SAVE_NONVOL offsets count from the frame: the frame register's base once the prolog
        // set it, else rsp
        let mut set_fp = false;
        let mut k = 0;
        while k < count {
            let (off, op) = ((code(k) & 0xff) as u64, (code(k) >> 8) & 15);
            set_fp |= op == 3 && off <= done;
            k += size(op, (code(k) >> 12) as u8, version);
        }
        let frame = if freg != 0 && set_fp { c.get(freg).ok_or("its frame register is not one a walk follows")?.wrapping_sub(foff) } else { c.rsp };
        let mut k = 0;
        while k < count {
            let (off, op, at) = ((code(k) & 0xff) as u64, (code(k) >> 8) & 15, (code(k) >> 12) as u8);
            let n = size(op, at, version);
            if k + n > count {
                return Err("its unwind codes are malformed");
            }
            if off <= done {
                match op {
                    // UWOP_PUSH_NONVOL
                    0 => {
                        c.set(at, read(c.rsp).ok_or(SLOT)?);
                        c.rsp += 8;
                    }
                    // UWOP_ALLOC_LARGE
                    1 => c.rsp += if at == 0 { code(k + 1) as u64 * 8 } else { code(k + 1) as u64 | (code(k + 2) as u64) << 16 },
                    // UWOP_ALLOC_SMALL
                    2 => c.rsp += at as u64 * 8 + 8,
                    // UWOP_SET_FPREG
                    3 => c.rsp = c.get(freg).ok_or("its frame register is not one a walk follows")?.wrapping_sub(foff),
                    // UWOP_SAVE_NONVOL, UWOP_SAVE_NONVOL_FAR
                    4 => c.set(at, read(frame + code(k + 1) as u64 * 8).ok_or(SLOT)?),
                    5 => c.set(at, read(frame + (code(k + 1) as u64 | (code(k + 2) as u64) << 16)).ok_or(SLOT)?),
                    // epilog descriptions (version 2), spare, xmm saves: no integer register
                    6..=9 => {}
                    // UWOP_PUSH_MACHFRAME: rip, cs, eflags, rsp, ss (after an error code when info 1)
                    10 => {
                        let fr = c.rsp + at as u64 * 8;
                        c.rip = read(fr).ok_or(SLOT)?;
                        c.rsp = read(fr + 24).ok_or(SLOT)?;
                        mach = true;
                    }
                    _ => return Err("its unwind info has a code this walker does not read"),
                }
            }
            k += n;
        }
        if !chained {
            if !mach {
                c.rip = read(c.rsp).ok_or(SLOT)?;
                c.rsp += 8;
            }
            return Ok(());
        }
        // the parent's info: all of its codes apply (the pc is past its prolog)
        let e = |s: usize| code(s) as u32 | (code(s + 1) as u32) << 16;
        f = Func { begin: e(slots), end: e(slots + 2), unwind: e(slots + 4) };
    }
    Err("its unwind info chains without end")
}

/// Slots an unwind code takes.
fn size(op: u16, info: u8, version: u8) -> usize {
    match op {
        1 => 2 + (info != 0) as usize,
        4 | 8 => 2,
        5 | 9 | 7 => 3,
        // version 2's UWOP_EPILOG takes one; version 1's code 6 (UWOP_SAVE_XMM) two
        6 => if version == 2 { 1 } else { 2 },
        _ => 1,
    }
}

/// Is `rip` in an epilog of `f`: from it on, an optional `add rsp, n` / `lea rsp, [reg + n]`,
/// pops, then `ret`, or a jump out of the function (a tail call), as the system unwinder
/// recognises one?
fn in_epilog(m: &Module, f: Func, rip: u64) -> bool {
    let inside = |a: u64| a >= m.lo + f.begin as u64 && a < m.lo + f.end as u64;
    let b = |a: u64| u8_at(a).unwrap_or(0);
    let mut pc = rip;
    if b(pc) & 0xf8 == 0x48 {
        match b(pc + 1) {
            0x81 if b(pc) == 0x48 && b(pc + 2) == 0xc4 => pc += 7,
            0x83 if b(pc) == 0x48 && b(pc + 2) == 0xc4 => pc += 4,
            0x8d => {
                let modrm = b(pc + 2);
                // rex.RX clear, destination rsp, no SIB
                if b(pc) & 0x06 != 0 || (modrm >> 3) & 7 != 4 || modrm & 7 == 4 {
                    return false;
                }
                match modrm >> 6 {
                    1 => pc += 4,
                    2 => pc += 7,
                    _ => return false,
                }
            }
            0x81 | 0x83 => return false,
            _ => {}
        }
    }
    for _ in 0..32 {
        if b(pc) & 0xf0 == 0x40 {
            pc += 1;
        }
        match b(pc) {
            0x58..=0x5f => pc += 1,
            0xc2 | 0xc3 => return true,
            0xf3 => return b(pc + 1) == 0xc3,
            // jmp qword ptr [rip + n]: a tail call through an import
            0xff => return b(pc + 1) == 0x25,
            0xe9 | 0xeb => {
                let to = jump_target(pc);
                match to {
                    Some(to) if inside(to) => pc = to,
                    Some(_) => return true,
                    None => return false,
                }
            }
            _ => return false,
        }
    }
    false
}

/// The target of the `jmp rel32` / `jmp rel8` at `pc`.
fn jump_target(pc: u64) -> Option<u64> {
    match u8_at(pc)? {
        0xe9 => Some((pc + 5).wrapping_add(u32_at(pc + 1)? as i32 as i64 as u64)),
        0xeb => Some((pc + 2).wrapping_add(u8_at(pc + 1)? as i8 as i64 as u64)),
        _ => None,
    }
}

/// Steps an interrupted epilog by running its instructions from `rip` (in_epilog said it is one).
fn run_epilog(m: &Module, f: Func, rip: u64, c: &mut Regs, read: &dyn Fn(u64) -> Option<u64>) -> Result<(), &'static str> {
    const CODE: &str = "its epilog's code is unreadable";
    let mut pc = rip;
    for _ in 0..64 {
        let mut rex = 0u8;
        let mut op = u8_at(pc).ok_or(CODE)?;
        if op & 0xf0 == 0x40 {
            rex = op & 0x0f;
            pc += 1;
            op = u8_at(pc).ok_or(CODE)?;
        }
        match op {
            0x58..=0x5f => {
                c.set(op - 0x58 + (rex & 1) * 8, read(c.rsp).ok_or(SLOT)?);
                c.rsp += 8;
                pc += 1;
            }
            // add rsp, imm32 / imm8
            0x81 => {
                c.rsp = c.rsp.wrapping_add(u32_at(pc + 2).ok_or(CODE)? as i32 as i64 as u64);
                pc += 6;
            }
            0x83 => {
                c.rsp = c.rsp.wrapping_add(u8_at(pc + 2).ok_or(CODE)? as i8 as i64 as u64);
                pc += 3;
            }
            // lea rsp, [reg + disp8 / disp32]
            0x8d => {
                let modrm = u8_at(pc + 1).ok_or(CODE)?;
                let base = c.get((modrm & 7) + (rex & 1) * 8).ok_or("its epilog's base register is not one a walk follows")?;
                if modrm >> 6 == 1 {
                    c.rsp = base.wrapping_add(u8_at(pc + 2).ok_or(CODE)? as i8 as i64 as u64);
                    pc += 3;
                } else {
                    c.rsp = base.wrapping_add(u32_at(pc + 2).ok_or(CODE)? as i32 as i64 as u64);
                    pc += 6;
                }
            }
            0xc2 => {
                c.rip = read(c.rsp).ok_or(SLOT)?;
                c.rsp += 8 + u16_at(pc + 1).ok_or(CODE)? as u64;
                return Ok(());
            }
            // ret, rep ret, and a tail call (the return address is the caller's)
            0xc3 | 0xf3 | 0xff => {
                c.rip = read(c.rsp).ok_or(SLOT)?;
                c.rsp += 8;
                return Ok(());
            }
            0xe9 | 0xeb => {
                let to = jump_target(pc).ok_or(CODE)?;
                // a jump within the function continues the epilog, one out of it is a tail call
                if to >= m.lo + f.begin as u64 && to < m.lo + f.end as u64 {
                    pc = to;
                } else {
                    c.rip = read(c.rsp).ok_or(SLOT)?;
                    c.rsp += 8;
                    return Ok(());
                }
            }
            _ => return Err("its epilog has an instruction this walker does not run"),
        }
    }
    Err("its epilog does not end")
}
