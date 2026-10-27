//! arm64 instruction decoder for [`super::dis`] (makepad inspect `asm`).
//!
//! Text is the syntax [`super::arm64`] reads (lowercase, preferred aliases such as `mov`, `cmp`,
//! `cset`, `ubfx`, `lsl #n`): the two encoders and this decoder are checked against each other in
//! tests_dis_arm64.rs. The decoder works on bit fields with `match`, never fails and allocates only
//! the two strings of an instruction: a word that is not decoded exactly becomes one
//! `.word 0x...` [`Class::Unknown`] entry and decoding goes on with the next word.
//!
//! Not decoded (Unknown): SVE / SME, pointer authentication, memory tagging, FEAT_MOPS, the dot
//! product / fp16-vector / bf16 / sha / sm3 / sm4 extensions.

use super::arm64::SYSREGS;
use super::dis::{Class, Insn};
use std::fmt::{self, Write};

/// Decodes `code` mapped at `addr`; see [`super::dis::decode`].
pub fn decode(code: &[u8], addr: u64) -> Vec<Insn> {
    let mut out = Vec::with_capacity(code.len() / 4 + 1);
    let mut off = 0usize;
    while off + 4 <= code.len() {
        let w = u32::from_le_bytes([code[off], code[off + 1], code[off + 2], code[off + 3]]);
        let pc = addr.wrapping_add(off as u64);
        let d = decode_word(w, pc).unwrap_or_else(|| D::unknown(w));
        out.push(Insn { off: off as u32, len: 4, mnem: d.mnem, ops: d.ops, class: d.class, target: d.target, pc_ref: d.pc_ref, mem: d.mem, stack: d.stack });
        off += 4;
    }
    while off < code.len() {
        out.push(Insn { off: off as u32, len: 1, mnem: ".byte".into(), ops: format!("0x{:02x}", code[off]), class: Class::Unknown, target: None, pc_ref: None, mem: false, stack: false });
        off += 1;
    }
    out
}

/// One decoded instruction under construction.
struct D {
    mnem: String,
    ops: String,
    class: Class,
    target: Option<u64>,
    pc_ref: Option<u64>,
    mem: bool,
    stack: bool,
}

impl D {
    fn new(m: &str, class: Class) -> D {
        D { mnem: m.to_string(), ops: String::with_capacity(24), class, target: None, pc_ref: None, mem: false, stack: false }
    }
    fn unknown(w: u32) -> D {
        D { mnem: ".word".into(), ops: format!("0x{:08x}", w), class: Class::Unknown, target: None, pc_ref: None, mem: false, stack: false }
    }
}

macro_rules! wr {
    ($d:expr, $($a:tt)*) => {
        let _ = write!($d.ops, $($a)*);
    };
}

// ---------------------------------------------------------------- small helpers

#[inline]
fn f(w: u32, lo: u32, n: u32) -> u32 {
    (w >> lo) & ((1u32 << n) - 1)
}
#[inline]
fn bit(w: u32, i: u32) -> bool {
    (w >> i) & 1 != 0
}
/// `n`-bit two's complement value
#[inline]
fn sx(v: u32, n: u32) -> i64 {
    (((v << (32 - n)) as i32) >> (32 - n)) as i64
}

/// A general register: `sp` selects the stack pointer / `wsp` for register 31, otherwise zr.
#[derive(Clone, Copy)]
struct Gp {
    sf: bool,
    n: u32,
    sp: bool,
}
impl fmt::Display for Gp {
    fn fmt(&self, o: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.n == 31 {
            o.write_str(match (self.sp, self.sf) {
                (true, true) => "sp",
                (true, false) => "wsp",
                (false, true) => "xzr",
                (false, false) => "wzr",
            })
        } else {
            write!(o, "{}{}", if self.sf { 'x' } else { 'w' }, self.n)
        }
    }
}
/// zr form
fn gp(sf: bool, n: u32) -> Gp {
    Gp { sf, n, sp: false }
}
/// sp form
fn gs(sf: bool, n: u32) -> Gp {
    Gp { sf, n, sp: true }
}
/// scalar fp / simd register `b0` .. `q31`
#[derive(Clone, Copy)]
struct Fr(char, u32);
impl fmt::Display for Fr {
    fn fmt(&self, o: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(o, "{}{}", self.0, self.1)
    }
}
fn fchar(size: u32) -> char {
    ['b', 'h', 's', 'd', 'q'][size as usize]
}

const CONDS: [&str; 16] = ["eq", "ne", "hs", "lo", "mi", "pl", "vs", "vc", "hi", "ls", "ge", "lt", "gt", "le", "al", "nv"];

/// vector arrangement of `size` (0 b, 1 h, 2 s, 3 d) and Q; `1d` included
fn arr(size: u32, q: u32) -> Option<&'static str> {
    Some(match (size, q) {
        (0, 0) => "8b",
        (0, 1) => "16b",
        (1, 0) => "4h",
        (1, 1) => "8h",
        (2, 0) => "2s",
        (2, 1) => "4s",
        (3, 0) => "1d",
        (3, 1) => "2d",
        _ => return None,
    })
}
/// as [`arr`] without `1d`
fn arr2(size: u32, q: u32) -> Option<&'static str> {
    if size == 3 && q == 0 {
        None
    } else {
        arr(size, q)
    }
}
/// floating point arrangement: sz 0 = single, 1 = double
fn farr(sz: u32, q: u32) -> Option<&'static str> {
    match (sz, q) {
        (0, 0) => Some("2s"),
        (0, 1) => Some("4s"),
        (1, 1) => Some("2d"),
        _ => None,
    }
}
/// the widened arrangement of a long operation on `size` (8h / 4s / 2d)
fn arr_wide(size: u32) -> Option<&'static str> {
    match size {
        0 => Some("8h"),
        1 => Some("4s"),
        2 => Some("2d"),
        _ => None,
    }
}
fn elem(size: u32) -> &'static str {
    ["b", "h", "s", "d"][size as usize]
}

/// `[xN]`, `[xN, #imm]`, `[xN, #imm]!` or `[xN], #imm`
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Off,
    Pre,
    Post,
}
fn put_mem(d: &mut D, base: u32, imm: i64, mode: Mode) {
    let b = gs(true, base);
    match mode {
        Mode::Off if imm == 0 => {
            wr!(d, "[{}]", b);
        }
        Mode::Off => {
            wr!(d, "[{}, #{}]", b, imm);
        }
        Mode::Pre => {
            wr!(d, "[{}, #{}]!", b, imm);
        }
        Mode::Post => {
            wr!(d, "[{}], #{}", b, imm);
        }
    }
}

fn shift_name(s: u32) -> &'static str {
    ["lsl", "lsr", "asr", "ror"][s as usize]
}

/// Appends `, lsl #n` style shift text for a shifted register operand (nothing for `lsl #0`).
fn put_shift(d: &mut D, shift: u32, amt: u32) {
    if shift != 0 || amt != 0 {
        wr!(d, ", {} #{}", shift_name(shift), amt);
    }
}

// ---------------------------------------------------------------- top level

fn decode_word(w: u32, pc: u64) -> Option<D> {
    match f(w, 25, 4) {
        0 => {
            if w >> 16 == 0 {
                let mut d = D::new("udf", Class::Trap);
                wr!(d, "#0x{:x}", w & 0xffff);
                Some(d)
            } else {
                None
            }
        }
        8 | 9 => dp_imm(w, pc),
        10 | 11 => branch(w, pc),
        4 | 6 | 12 | 14 => ldst(w, pc),
        5 | 13 => dp_reg(w),
        7 | 15 => simd_fp(w),
        _ => None,
    }
}

// ---------------------------------------------------------------- data processing, immediate

/// The value of a logical immediate (`N:immr:imms`), None for the reserved encodings.
fn decode_bitmask(n: bool, imms: u32, immr: u32, sf: bool) -> Option<u64> {
    let x = ((n as u32) << 6) | (!imms & 0x3f);
    if x == 0 {
        return None;
    }
    let len = 31 - x.leading_zeros();
    let esize = 1u32 << len;
    if !sf && esize > 32 {
        return None;
    }
    let levels = esize - 1;
    let s = imms & levels;
    let r = immr & levels;
    if s == levels {
        return None;
    }
    let mut welem: u64 = (1u64 << (s + 1)) - 1;
    if r != 0 {
        let mask = if esize == 64 { !0u64 } else { (1u64 << esize) - 1 };
        welem = ((welem >> r) | (welem << (esize - r))) & mask;
    }
    let mut v = welem;
    let mut e = esize;
    while e < 64 {
        v |= v << e;
        e *= 2;
    }
    Some(if sf { v } else { v & 0xffff_ffff })
}

/// Whether a single movz / movn can produce `v` (then the assembler would not pick orr for it).
fn mov_wide_able(v: u64, sf: bool) -> bool {
    let mask = if sf { !0u64 } else { 0xffff_ffff };
    let one = |x: u64| (0..4).filter(|h| (x >> (16 * h)) & 0xffff != 0).count() <= 1;
    one(v & mask) || one(!v & mask)
}

fn dp_imm(w: u32, pc: u64) -> Option<D> {
    let sf = bit(w, 31);
    let rd = f(w, 0, 5);
    let rn = f(w, 5, 5);
    match f(w, 23, 3) {
        0 | 1 => {
            let imm = sx(f(w, 5, 19) << 2 | f(w, 29, 2), 21);
            let (mn, a) = if sf { ("adrp", ((pc >> 12).wrapping_add(imm as u64)) << 12) } else { ("adr", pc.wrapping_add(imm as u64)) };
            let mut d = D::new(mn, Class::Addr);
            wr!(d, "{}, 0x{:x}", gp(true, rd), a);
            d.pc_ref = Some(a);
            Some(d)
        }
        2 => {
            let op = bit(w, 30);
            let s = bit(w, 29);
            let imm = f(w, 10, 12);
            let sh = bit(w, 22);
            if !s && !op && imm == 0 && !sh && (rd == 31 || rn == 31) {
                let mut d = D::new("mov", Class::MovReg);
                wr!(d, "{}, {}", gs(sf, rd), gs(sf, rn));
                return Some(d);
            }
            let mut d;
            if s && rd == 31 {
                d = D::new(if op { "cmp" } else { "cmn" }, Class::Cmp);
                wr!(d, "{}, #{}", gs(sf, rn), imm);
            } else {
                d = D::new(["add", "adds", "sub", "subs"][(op as usize) << 1 | s as usize], Class::Alu);
                let rdr = if s { gp(sf, rd) } else { gs(sf, rd) };
                wr!(d, "{}, {}, #{}", rdr, gs(sf, rn), imm);
            }
            if sh {
                wr!(d, ", lsl #12");
            }
            Some(d)
        }
        4 => {
            let opc = f(w, 29, 2);
            let n = bit(w, 22);
            if !sf && n {
                return None;
            }
            let imm = decode_bitmask(n, f(w, 10, 6), f(w, 16, 6), sf)?;
            let rdr = if opc == 3 { gp(sf, rd) } else { gs(sf, rd) };
            if opc == 1 && rn == 31 && (rd == 31 || !mov_wide_able(imm, sf)) {
                let mut d = D::new("mov", Class::MovImm);
                if !sf && imm >= 1 << 31 {
                    wr!(d, "{}, #{}", gs(sf, rd), imm as u32 as i32);
                } else {
                    wr!(d, "{}, #0x{:x}", gs(sf, rd), imm);
                }
                return Some(d);
            }
            if opc == 3 && rd == 31 {
                let mut d = D::new("tst", Class::Cmp);
                wr!(d, "{}, #0x{:x}", gp(sf, rn), imm);
                return Some(d);
            }
            let mut d = D::new(["and", "orr", "eor", "ands"][opc as usize], Class::Alu);
            wr!(d, "{}, {}, #0x{:x}", rdr, gp(sf, rn), imm);
            Some(d)
        }
        5 => {
            let opc = f(w, 29, 2);
            let hw = f(w, 21, 2);
            let imm = f(w, 5, 16);
            if opc == 1 || (!sf && hw > 1) {
                return None;
            }
            let sh = 16 * hw;
            let alias_ok = !(imm == 0 && hw != 0);
            let mut d = D::new("mov", Class::MovImm);
            match opc {
                2 if alias_ok => {
                    let v = (imm as u64) << sh;
                    if sf {
                        wr!(d, "{}, #{}", gp(sf, rd), v as i64);
                    } else {
                        wr!(d, "{}, #{}", gp(sf, rd), v as u32 as i32);
                    }
                }
                0 if alias_ok && !(!sf && imm == 0xffff) => {
                    let v = !((imm as u64) << sh);
                    if sf {
                        wr!(d, "{}, #{}", gp(sf, rd), v as i64);
                    } else {
                        wr!(d, "{}, #{}", gp(sf, rd), v as u32 as i32);
                    }
                }
                _ => {
                    d.mnem = ["movn", "", "movz", "movk"][opc as usize].to_string();
                    wr!(d, "{}, #{}", gp(sf, rd), imm);
                    if sh != 0 {
                        wr!(d, ", lsl #{}", sh);
                    }
                }
            }
            Some(d)
        }
        6 => {
            let opc = f(w, 29, 2);
            let n = bit(w, 22);
            let immr = f(w, 16, 6);
            let imms = f(w, 10, 6);
            if n != sf || opc == 3 || (!sf && (immr > 31 || imms > 31)) {
                return None;
            }
            let width = if sf { 64 } else { 32 };
            let lsb_fix = (width - immr) % width;
            let (rdr, rnr) = (gp(sf, rd), gp(sf, rn));
            let mut d;
            match opc {
                0 => {
                    if imms == width - 1 {
                        d = D::new("asr", Class::Alu);
                        wr!(d, "{}, {}, #{}", rdr, rnr, immr);
                    } else if immr == 0 && (imms == 7 || imms == 15 || (imms == 31 && sf)) {
                        d = D::new(match imms {
                            7 => "sxtb",
                            15 => "sxth",
                            _ => "sxtw",
                        }, Class::MovReg);
                        wr!(d, "{}, {}", rdr, gp(false, rn));
                    } else if imms < immr {
                        d = D::new("sbfiz", Class::Alu);
                        wr!(d, "{}, {}, #{}, #{}", rdr, rnr, lsb_fix, imms + 1);
                    } else {
                        d = D::new("sbfx", Class::Alu);
                        wr!(d, "{}, {}, #{}, #{}", rdr, rnr, immr, imms - immr + 1);
                    }
                }
                2 => {
                    if imms != width - 1 && imms + 1 == immr {
                        d = D::new("lsl", Class::Alu);
                        wr!(d, "{}, {}, #{}", rdr, rnr, width - 1 - imms);
                    } else if imms == width - 1 {
                        d = D::new("lsr", Class::Alu);
                        wr!(d, "{}, {}, #{}", rdr, rnr, immr);
                    } else if !sf && immr == 0 && (imms == 7 || imms == 15) {
                        d = D::new(if imms == 7 { "uxtb" } else { "uxth" }, Class::MovReg);
                        wr!(d, "{}, {}", rdr, rnr);
                    } else if imms < immr {
                        d = D::new("ubfiz", Class::Alu);
                        wr!(d, "{}, {}, #{}, #{}", rdr, rnr, lsb_fix, imms + 1);
                    } else {
                        d = D::new("ubfx", Class::Alu);
                        wr!(d, "{}, {}, #{}, #{}", rdr, rnr, immr, imms - immr + 1);
                    }
                }
                _ => {
                    if imms < immr {
                        if rn == 31 {
                            d = D::new("bfc", Class::Alu);
                            wr!(d, "{}, #{}, #{}", rdr, lsb_fix, imms + 1);
                        } else {
                            d = D::new("bfi", Class::Alu);
                            wr!(d, "{}, {}, #{}, #{}", rdr, rnr, lsb_fix, imms + 1);
                        }
                    } else {
                        d = D::new("bfxil", Class::Alu);
                        wr!(d, "{}, {}, #{}, #{}", rdr, rnr, immr, imms - immr + 1);
                    }
                }
            }
            Some(d)
        }
        7 => {
            let imms = f(w, 10, 6);
            if f(w, 29, 2) != 0 || bit(w, 21) || bit(w, 22) != sf || (!sf && imms > 31) {
                return None;
            }
            let rm = f(w, 16, 5);
            let mut d;
            if rn == rm {
                d = D::new("ror", Class::Alu);
                wr!(d, "{}, {}, #{}", gp(sf, rd), gp(sf, rn), imms);
            } else {
                d = D::new("extr", Class::Alu);
                wr!(d, "{}, {}, {}, #{}", gp(sf, rd), gp(sf, rn), gp(sf, rm), imms);
            }
            Some(d)
        }
        _ => None,
    }
}

// ---------------------------------------------------------------- branches, system

fn branch(w: u32, pc: u64) -> Option<D> {
    if f(w, 26, 5) == 0b00101 {
        let t = pc.wrapping_add((sx(f(w, 0, 26), 26) * 4) as u64);
        let mut d = D::new(if bit(w, 31) { "bl" } else { "b" }, if bit(w, 31) { Class::Call } else { Class::Branch });
        wr!(d, "0x{:x}", t);
        d.target = Some(t);
        return Some(d);
    }
    if f(w, 25, 6) == 0b011010 {
        let sf = bit(w, 31);
        let t = pc.wrapping_add((sx(f(w, 5, 19), 19) * 4) as u64);
        let mut d = D::new(if bit(w, 24) { "cbnz" } else { "cbz" }, Class::CondBranch);
        wr!(d, "{}, 0x{:x}", gp(sf, f(w, 0, 5)), t);
        d.target = Some(t);
        return Some(d);
    }
    if f(w, 25, 6) == 0b011011 {
        let b = (w >> 31) << 5 | f(w, 19, 5);
        let t = pc.wrapping_add((sx(f(w, 5, 14), 14) * 4) as u64);
        let mut d = D::new(if bit(w, 24) { "tbnz" } else { "tbz" }, Class::CondBranch);
        wr!(d, "{}, #{}, 0x{:x}", gp(b >= 32, f(w, 0, 5)), b, t);
        d.target = Some(t);
        return Some(d);
    }
    if f(w, 24, 8) == 0x54 {
        if bit(w, 4) {
            return None;
        }
        let t = pc.wrapping_add((sx(f(w, 5, 19), 19) * 4) as u64);
        let mut d = D::new(&format!("b.{}", CONDS[f(w, 0, 4) as usize]), Class::CondBranch);
        wr!(d, "0x{:x}", t);
        d.target = Some(t);
        return Some(d);
    }
    if f(w, 24, 8) == 0xD4 {
        let opc = f(w, 21, 3);
        let op2 = f(w, 2, 3);
        let ll = f(w, 0, 2);
        let imm = f(w, 5, 16);
        let (mn, class) = match (opc, op2, ll) {
            (0, 0, 1) => ("svc", Class::Other),
            (0, 0, 2) => ("hvc", Class::Other),
            (0, 0, 3) => ("smc", Class::Other),
            (1, 0, 0) => ("brk", Class::Trap),
            (2, 0, 0) => ("hlt", Class::Trap),
            _ => return None,
        };
        let mut d = D::new(mn, class);
        wr!(d, "#0x{:x}", imm);
        return Some(d);
    }
    if f(w, 25, 7) == 0b1101011 {
        let opc = f(w, 21, 4);
        let rn = f(w, 5, 5);
        if f(w, 16, 5) != 31 {
            return None;
        }
        let op3 = f(w, 10, 6);
        let op4 = f(w, 0, 5);
        if op3 == 0 && op4 == 0 {
            return match opc {
                0 => {
                    let mut d = D::new("br", Class::IndirectBranch);
                    wr!(d, "{}", gp(true, rn));
                    Some(d)
                }
                1 => {
                    let mut d = D::new("blr", Class::IndirectCall);
                    wr!(d, "{}", gp(true, rn));
                    Some(d)
                }
                2 => {
                    let mut d = D::new("ret", Class::Ret);
                    if rn != 30 {
                        wr!(d, "{}", gp(true, rn));
                    }
                    Some(d)
                }
                4 if rn == 31 => Some(D::new("eret", Class::Other)),
                5 if rn == 31 => Some(D::new("drps", Class::Other)),
                _ => None,
            };
        }
        return match w {
            0xD65F_0BFF => Some(D::new("retaa", Class::Ret)),
            0xD65F_0FFF => Some(D::new("retab", Class::Ret)),
            0xD69F_0BFF => Some(D::new("eretaa", Class::Other)),
            0xD69F_0FFF => Some(D::new("eretab", Class::Other)),
            _ => None,
        };
    }
    if f(w, 22, 10) == 0b1101010100 {
        return system(w);
    }
    None
}

const HINTS: [(u32, &str, Class); 24] = [
    (0, "nop", Class::Nop),
    (1, "yield", Class::Nop),
    (2, "wfe", Class::Other),
    (3, "wfi", Class::Other),
    (4, "sev", Class::Other),
    (5, "sevl", Class::Other),
    (6, "dgh", Class::Nop),
    (7, "xpaclri", Class::Other),
    (8, "pacia1716", Class::Other),
    (10, "pacib1716", Class::Other),
    (12, "autia1716", Class::Other),
    (14, "autib1716", Class::Other),
    (16, "esb", Class::Nop),
    (20, "csdb", Class::Nop),
    (24, "paciaz", Class::Other),
    (25, "paciasp", Class::Other),
    (26, "pacibz", Class::Other),
    (27, "pacibsp", Class::Other),
    (28, "autiaz", Class::Other),
    (29, "autiasp", Class::Other),
    (30, "autibz", Class::Other),
    (31, "autibsp", Class::Other),
    (32, "bti", Class::Nop),
    (34, "bti c", Class::Nop),
];

fn barrier_name(crm: u32) -> Option<&'static str> {
    Some(match crm {
        1 => "oshld",
        2 => "oshst",
        3 => "osh",
        5 => "nshld",
        6 => "nshst",
        7 => "nsh",
        9 => "ishld",
        10 => "ishst",
        11 => "ish",
        13 => "ld",
        14 => "st",
        15 => "sy",
        _ => return None,
    })
}

/// (op1, CRn, CRm, op2, takes a register, mnemonic, operation)
const SYS_ALIASES: &[(u32, u32, u32, u32, bool, &str, &str)] = &[
    (3, 7, 4, 1, true, "dc", "zva"),
    (0, 7, 6, 1, true, "dc", "ivac"),
    (0, 7, 6, 2, true, "dc", "isw"),
    (3, 7, 10, 1, true, "dc", "cvac"),
    (0, 7, 10, 2, true, "dc", "csw"),
    (3, 7, 11, 1, true, "dc", "cvau"),
    (3, 7, 12, 1, true, "dc", "cvap"),
    (3, 7, 13, 1, true, "dc", "cvadp"),
    (3, 7, 14, 1, true, "dc", "civac"),
    (0, 7, 14, 2, true, "dc", "cisw"),
    (0, 7, 1, 0, false, "ic", "ialluis"),
    (0, 7, 5, 0, false, "ic", "iallu"),
    (3, 7, 5, 1, true, "ic", "ivau"),
    (0, 7, 8, 0, true, "at", "s1e1r"),
    (0, 7, 8, 1, true, "at", "s1e1w"),
    (0, 7, 8, 2, true, "at", "s1e0r"),
    (0, 7, 8, 3, true, "at", "s1e0w"),
    (0, 8, 3, 0, false, "tlbi", "vmalle1is"),
    (0, 8, 3, 1, true, "tlbi", "vae1is"),
    (0, 8, 3, 2, true, "tlbi", "aside1is"),
    (0, 8, 3, 3, true, "tlbi", "vaae1is"),
    (0, 8, 3, 5, true, "tlbi", "vale1is"),
    (0, 8, 3, 7, true, "tlbi", "vaale1is"),
    (0, 8, 7, 0, false, "tlbi", "vmalle1"),
    (0, 8, 7, 1, true, "tlbi", "vae1"),
    (0, 8, 7, 2, true, "tlbi", "aside1"),
    (0, 8, 7, 3, true, "tlbi", "vaae1"),
    (0, 8, 7, 5, true, "tlbi", "vale1"),
    (0, 8, 7, 7, true, "tlbi", "vaale1"),
    (4, 8, 3, 4, false, "tlbi", "alle1is"),
    (4, 8, 7, 4, false, "tlbi", "alle1"),
];

fn system(w: u32) -> Option<D> {
    let l = bit(w, 21);
    let op0 = f(w, 19, 2);
    let op1 = f(w, 16, 3);
    let crn = f(w, 12, 4);
    let crm = f(w, 8, 4);
    let op2 = f(w, 5, 3);
    let rt = f(w, 0, 5);
    if op0 == 0 {
        if l {
            return None;
        }
        match crn {
            2 if op1 == 3 => {
                if rt != 31 {
                    return None;
                }
                let n = crm << 3 | op2;
                if let Some(&(_, name, class)) = HINTS.iter().find(|h| h.0 == n) {
                    return Some(D::new(name, class));
                }
                match n {
                    17 | 18 => {
                        let mut d = D::new(if n == 17 { "psb" } else { "tsb" }, Class::Nop);
                        wr!(d, "csync");
                        return Some(d);
                    }
                    36 => return Some(D::new("bti j", Class::Nop)),
                    38 => return Some(D::new("bti jc", Class::Nop)),
                    _ => {}
                }
                let mut d = D::new("hint", Class::Nop);
                wr!(d, "#{}", n);
                Some(d)
            }
            3 if op1 == 3 && rt == 31 => {
                match op2 {
                    2 => {
                        let mut d = D::new("clrex", Class::Atomic);
                        if crm != 15 {
                            wr!(d, "#{}", crm);
                        }
                        Some(d)
                    }
                    4 | 5 => {
                        if op2 == 4 && crm == 0 {
                            return Some(D::new("ssbb", Class::Atomic));
                        }
                        if op2 == 4 && crm == 4 {
                            return Some(D::new("pssbb", Class::Atomic));
                        }
                        let mut d = D::new(if op2 == 4 { "dsb" } else { "dmb" }, Class::Atomic);
                        match barrier_name(crm) {
                            Some(n) => {
                                wr!(d, "{}", n);
                            }
                            None => {
                                wr!(d, "#{}", crm);
                            }
                        }
                        Some(d)
                    }
                    6 => {
                        let mut d = D::new("isb", Class::Atomic);
                        if crm != 15 {
                            wr!(d, "#{}", crm);
                        }
                        Some(d)
                    }
                    7 if crm == 0 => Some(D::new("sb", Class::Atomic)),
                    _ => None,
                }
            }
            4 if rt == 31 => {
                let name = match (op1, op2) {
                    (0, 5) => "spsel",
                    (3, 6) => "daifset",
                    (3, 7) => "daifclr",
                    (0, 3) => "uao",
                    (0, 4) => "pan",
                    (3, 2) => "dit",
                    (3, 1) => "ssbs",
                    (3, 4) => "tco",
                    _ => return None,
                };
                let mut d = D::new("msr", Class::Other);
                wr!(d, "{}, #{}", name, crm);
                Some(d)
            }
            _ => None,
        }
    } else if op0 == 1 {
        if !l {
            if let Some(a) = SYS_ALIASES.iter().find(|a| a.0 == op1 && a.1 == crn && a.2 == crm && a.3 == op2 && (a.4 || rt == 31)) {
                let mut d = D::new(a.5, Class::Other);
                if a.4 {
                    wr!(d, "{}, {}", a.6, gp(true, rt));
                } else {
                    wr!(d, "{}", a.6);
                }
                return Some(d);
            }
            let mut d = D::new("sys", Class::Other);
            wr!(d, "#{}, c{}, c{}, #{}", op1, crn, crm, op2);
            if rt != 31 {
                wr!(d, ", {}", gp(true, rt));
            }
            Some(d)
        } else {
            let mut d = D::new("sysl", Class::Other);
            wr!(d, "{}, #{}, c{}, c{}, #{}", gp(true, rt), op1, crn, crm, op2);
            Some(d)
        }
    } else {
        let mut d = D::new(if l { "mrs" } else { "msr" }, Class::Other);
        let mut name = None;
        if let Some(e) = SYSREGS.iter().find(|e| e.1 == op0 && e.2 == op1 && e.3 == crn && e.4 == crm && e.5 == op2) {
            name = Some(e.0);
        }
        let reg = gp(true, rt);
        if l {
            match name {
                Some(n) => {
                    wr!(d, "{}, {}", reg, n);
                }
                None => {
                    wr!(d, "{}, s{}_{}_c{}_c{}_{}", reg, op0, op1, crn, crm, op2);
                }
            }
        } else {
            match name {
                Some(n) => {
                    wr!(d, "{}, {}", n, reg);
                }
                None => {
                    wr!(d, "s{}_{}_c{}_c{}_{}, {}", op0, op1, crn, crm, op2, reg);
                }
            }
        }
        Some(d)
    }
}

// ---------------------------------------------------------------- loads and stores

fn mem_flags(d: &mut D, base: u32, load: bool, mode: Mode) {
    d.mem = true;
    d.stack = base == 31 || base == 29;
    d.class = if base == 31 && mode != Mode::Off {
        Class::PushPop
    } else {
        match (load, d.stack) {
            (true, true) => Class::StackLoad,
            (true, false) => Class::Load,
            (false, true) => Class::StackStore,
            (false, false) => Class::Store,
        }
    };
}

fn prfm_name(rt: u32) -> Option<String> {
    let ty = ["pld", "pli", "pst"].get((rt >> 3) as usize)?;
    let lvl = ["l1", "l2", "l3"].get(((rt >> 1) & 3) as usize)?;
    Some(format!("{}{}{}", ty, lvl, if rt & 1 == 0 { "keep" } else { "strm" }))
}

fn ldst(w: u32, pc: u64) -> Option<D> {
    let rt = f(w, 0, 5);
    let rn = f(w, 5, 5);
    let v = bit(w, 26);
    // advanced simd load/store structures
    if !bit(w, 31) && f(w, 25, 5) == 0b00110 && v {
        return simd_struct(w);
    }
    match f(w, 27, 3) {
        0b011 if f(w, 24, 2) == 0 => {
            let opc = f(w, 30, 2);
            let t = pc.wrapping_add((sx(f(w, 5, 19), 19) * 4) as u64);
            let mut d;
            if v {
                if opc == 3 {
                    return None;
                }
                d = D::new("ldr", Class::Load);
                wr!(d, "{}, 0x{:x}", Fr(['s', 'd', 'q'][opc as usize], rt), t);
            } else {
                match opc {
                    0 | 1 => {
                        d = D::new("ldr", Class::Load);
                        wr!(d, "{}, 0x{:x}", gp(opc == 1, rt), t);
                    }
                    2 => {
                        d = D::new("ldrsw", Class::Load);
                        wr!(d, "{}, 0x{:x}", gp(true, rt), t);
                    }
                    _ => {
                        d = D::new("prfm", Class::Other);
                        match prfm_name(rt) {
                            Some(n) => {
                                wr!(d, "{}, 0x{:x}", n, t);
                            }
                            None => {
                                wr!(d, "#{}, 0x{:x}", rt, t);
                            }
                        }
                    }
                }
            }
            d.mem = true;
            d.pc_ref = Some(t);
            Some(d)
        }
        0b101 => ldst_pair(w),
        0b111 => ldst_reg(w),
        0b001 if f(w, 24, 3) == 0 && !v => ldst_excl(w),
        0b011 if f(w, 24, 3) == 1 && !v && !bit(w, 21) && f(w, 10, 2) == 0 => {
            // ldapur / stlur
            let size = f(w, 30, 2);
            let opc = f(w, 22, 2);
            let imm = sx(f(w, 12, 9), 9);
            let sfx = ["b", "h", "", ""][size as usize];
            let (mn, sf) = match (opc, size) {
                (0, _) => (format!("stlur{}", sfx), size == 3),
                (1, _) => (format!("ldapur{}", sfx), size == 3),
                (2, 0) => ("ldapursb".to_string(), true),
                (2, 1) => ("ldapursh".to_string(), true),
                (2, 2) => ("ldapursw".to_string(), true),
                (3, 0) => ("ldapursb".to_string(), false),
                (3, 1) => ("ldapursh".to_string(), false),
                _ => return None,
            };
            let mut d = D::new(&mn, Class::Atomic);
            wr!(d, "{}, ", gp(sf, rt));
            put_mem(&mut d, rn, imm, Mode::Off);
            d.mem = true;
            d.stack = rn == 31 || rn == 29;
            Some(d)
        }
        _ => None,
    }
}

fn ldst_pair(w: u32) -> Option<D> {
    let opc = f(w, 30, 2);
    let v = bit(w, 26);
    let ty = f(w, 23, 2);
    let load = bit(w, 22);
    let rt = f(w, 0, 5);
    let rn = f(w, 5, 5);
    let rt2 = f(w, 10, 5);
    let imm7 = sx(f(w, 15, 7), 7);
    let (mn_stem, scale, kind): (&str, u32, Result<bool, char>) = if v {
        if opc == 3 {
            return None;
        }
        ("p", 2 + opc, Err(['s', 'd', 'q'][opc as usize]))
    } else {
        match opc {
            0 => ("p", 2, Ok(false)),
            1 if load => ("psw", 2, Ok(true)),
            2 => ("p", 3, Ok(true)),
            _ => return None,
        }
    };
    let mn = match (ty, load) {
        (0, true) => format!("ldn{}", mn_stem),
        (0, false) => format!("stn{}", mn_stem),
        (_, true) => format!("ld{}", mn_stem),
        (_, false) => format!("st{}", mn_stem),
    };
    let mode = match ty {
        1 => Mode::Post,
        3 => Mode::Pre,
        _ => Mode::Off,
    };
    let mut d = D::new(&mn, Class::Load);
    match kind {
        Ok(sf) => {
            wr!(d, "{}, {}, ", gp(sf, rt), gp(sf, rt2));
        }
        Err(c) => {
            wr!(d, "{}, {}, ", Fr(c, rt), Fr(c, rt2));
        }
    }
    put_mem(&mut d, rn, imm7 << scale, mode);
    mem_flags(&mut d, rn, load, mode);
    Some(d)
}

fn ldst_reg(w: u32) -> Option<D> {
    let size = f(w, 30, 2);
    let v = bit(w, 26);
    let opc = f(w, 22, 2);
    let rt = f(w, 0, 5);
    let rn = f(w, 5, 5);
    // atomic memory operations
    if !v && !bit(w, 24) && bit(w, 21) && f(w, 10, 2) == 0 {
        return lse_rmw(w);
    }
    // stem: 0 = ldr/str, with extension letters; reg: Ok(sf) general, Err(letter) simd
    let load;
    let ext: &str;
    let reg: Result<bool, char>;
    let scale;
    let mut prefetch = false;
    if v {
        load = opc & 1 != 0;
        ext = "";
        if opc >> 1 == 0 {
            reg = Err(fchar(size));
            scale = size;
        } else if size == 0 {
            reg = Err('q');
            scale = 4;
        } else {
            return None;
        }
    } else {
        scale = size;
        match (size, opc) {
            (0, 0) => (load, ext, reg) = (false, "b", Ok(false)),
            (0, 1) => (load, ext, reg) = (true, "b", Ok(false)),
            (0, 2) => (load, ext, reg) = (true, "sb", Ok(true)),
            (0, 3) => (load, ext, reg) = (true, "sb", Ok(false)),
            (1, 0) => (load, ext, reg) = (false, "h", Ok(false)),
            (1, 1) => (load, ext, reg) = (true, "h", Ok(false)),
            (1, 2) => (load, ext, reg) = (true, "sh", Ok(true)),
            (1, 3) => (load, ext, reg) = (true, "sh", Ok(false)),
            (2, 0) => (load, ext, reg) = (false, "", Ok(false)),
            (2, 1) => (load, ext, reg) = (true, "", Ok(false)),
            (2, 2) => (load, ext, reg) = (true, "sw", Ok(true)),
            (3, 0) => (load, ext, reg) = (false, "", Ok(true)),
            (3, 1) => (load, ext, reg) = (true, "", Ok(true)),
            (3, 2) => {
                load = true;
                ext = "";
                reg = Ok(true);
                prefetch = true;
            }
            _ => return None,
        }
    }
    let (mn, mode, mem_kind): (String, Mode, u32) = if bit(w, 24) {
        (if prefetch { "prfm".into() } else { format!("{}r{}", if load { "ld" } else { "st" }, ext) }, Mode::Off, 0)
    } else if bit(w, 21) {
        if f(w, 10, 2) != 2 {
            return None;
        }
        (if prefetch { "prfm".into() } else { format!("{}r{}", if load { "ld" } else { "st" }, ext) }, Mode::Off, 3)
    } else {
        match f(w, 10, 2) {
            0 => (if prefetch { "prfum".into() } else { format!("{}ur{}", if load { "ld" } else { "st" }, ext) }, Mode::Off, 1),
            1 => (if prefetch { return None } else { format!("{}r{}", if load { "ld" } else { "st" }, ext) }, Mode::Post, 1),
            2 => (if prefetch { return None } else { format!("{}tr{}", if load { "ld" } else { "st" }, ext) }, Mode::Off, 1),
            _ => (if prefetch { return None } else { format!("{}r{}", if load { "ld" } else { "st" }, ext) }, Mode::Pre, 1),
        }
    };
    let mut d = D::new(&mn, Class::Load);
    if prefetch {
        match prfm_name(rt) {
            Some(n) => {
                wr!(d, "{}, ", n);
            }
            None => {
                wr!(d, "#{}, ", rt);
            }
        }
    } else {
        match reg {
            Ok(sf) => {
                wr!(d, "{}, ", gp(sf, rt));
            }
            Err(c) => {
                wr!(d, "{}, ", Fr(c, rt));
            }
        }
    }
    match mem_kind {
        0 => {
            let imm = (f(w, 10, 12) as i64) << scale;
            put_mem(&mut d, rn, imm, Mode::Off);
        }
        1 => {
            let imm = sx(f(w, 12, 9), 9);
            put_mem(&mut d, rn, imm, mode);
        }
        _ => {
            let opt = f(w, 13, 3);
            let s = bit(w, 12);
            let rm = f(w, 16, 5);
            let (ename, x) = match opt {
                2 => ("uxtw", false),
                3 => ("lsl", true),
                6 => ("sxtw", false),
                7 => ("sxtx", true),
                _ => return None,
            };
            wr!(d, "[{}, {}", gs(true, rn), gp(x, rm));
            if s {
                wr!(d, ", {} #{}", ename, scale);
            } else if opt != 3 {
                wr!(d, ", {}", ename);
            }
            wr!(d, "]");
        }
    }
    mem_flags(&mut d, rn, load, mode);
    if prefetch {
        d.class = Class::Other;
    }
    Some(d)
}

fn lse_rmw(w: u32) -> Option<D> {
    let size = f(w, 30, 2);
    let a = bit(w, 23);
    let r = bit(w, 22);
    let rs = f(w, 16, 5);
    let o3 = bit(w, 15);
    let opc = f(w, 12, 3);
    let rn = f(w, 5, 5);
    let rt = f(w, 0, 5);
    let x = size == 3;
    let sfx = ["b", "h", "", ""][size as usize];
    let ar = match (a, r) {
        (false, false) => "",
        (true, false) => "a",
        (false, true) => "l",
        (true, true) => "al",
    };
    let mut d;
    if !o3 {
        let stem = ["add", "clr", "eor", "set", "smax", "smin", "umax", "umin"][opc as usize];
        if rt == 31 && !a {
            d = D::new(&format!("st{}{}{}", stem, ar, sfx), Class::Atomic);
            wr!(d, "{}, ", gp(x, rs));
        } else {
            d = D::new(&format!("ld{}{}{}", stem, ar, sfx), Class::Atomic);
            wr!(d, "{}, {}, ", gp(x, rs), gp(x, rt));
        }
    } else if opc == 0 {
        d = D::new(&format!("swp{}{}", ar, sfx), Class::Atomic);
        wr!(d, "{}, {}, ", gp(x, rs), gp(x, rt));
    } else if opc == 4 && a && !r && rs == 31 {
        d = D::new(&format!("ldapr{}", sfx), Class::Atomic);
        wr!(d, "{}, ", gp(x, rt));
    } else {
        return None;
    }
    put_mem(&mut d, rn, 0, Mode::Off);
    d.mem = true;
    d.stack = rn == 31 || rn == 29;
    Some(d)
}

fn ldst_excl(w: u32) -> Option<D> {
    let size = f(w, 30, 2);
    let o2 = bit(w, 23);
    let l = bit(w, 22);
    let o1 = bit(w, 21);
    let rs = f(w, 16, 5);
    let o0 = bit(w, 15);
    let rt2 = f(w, 10, 5);
    let rn = f(w, 5, 5);
    let rt = f(w, 0, 5);
    let x = size == 3;
    let sfx = ["b", "h", "", ""][size as usize];
    let mut d;
    match (o2, o1) {
        (false, false) => {
            if rt2 != 31 || (l && rs != 31) {
                return None;
            }
            let stem = match (l, o0) {
                (false, false) => "stxr",
                (false, true) => "stlxr",
                (true, false) => "ldxr",
                (true, true) => "ldaxr",
            };
            d = D::new(&format!("{}{}", stem, sfx), Class::Atomic);
            if !l {
                wr!(d, "{}, ", gp(false, rs));
            }
            wr!(d, "{}, ", gp(x, rt));
        }
        (false, true) => {
            if size < 2 {
                // casp
                let sf = bit(w, 30);
                if rt2 != 31 || rs % 2 != 0 || rt % 2 != 0 {
                    return None;
                }
                let ar = match (l, o0) {
                    (false, false) => "",
                    (true, false) => "a",
                    (false, true) => "l",
                    (true, true) => "al",
                };
                d = D::new(&format!("casp{}", ar), Class::Atomic);
                wr!(d, "{}, {}, {}, {}, ", gp(sf, rs), gp(sf, rs + 1), gp(sf, rt), gp(sf, rt + 1));
            } else {
                if l && rs != 31 {
                    return None;
                }
                let stem = match (l, o0) {
                    (false, false) => "stxp",
                    (false, true) => "stlxp",
                    (true, false) => "ldxp",
                    (true, true) => "ldaxp",
                };
                d = D::new(stem, Class::Atomic);
                if !l {
                    wr!(d, "{}, ", gp(false, rs));
                }
                wr!(d, "{}, {}, ", gp(x, rt), gp(x, rt2));
            }
        }
        (true, false) => {
            if rt2 != 31 || rs != 31 {
                return None;
            }
            let stem = match (l, o0) {
                (false, false) => "stllr",
                (false, true) => "stlr",
                (true, false) => "ldlar",
                (true, true) => "ldar",
            };
            d = D::new(&format!("{}{}", stem, sfx), Class::Atomic);
            wr!(d, "{}, ", gp(x, rt));
        }
        (true, true) => {
            if rt2 != 31 {
                return None;
            }
            let ar = match (l, o0) {
                (false, false) => "",
                (true, false) => "a",
                (false, true) => "l",
                (true, true) => "al",
            };
            d = D::new(&format!("cas{}{}", ar, sfx), Class::Atomic);
            wr!(d, "{}, {}, ", gp(x, rs), gp(x, rt));
        }
    }
    put_mem(&mut d, rn, 0, Mode::Off);
    d.mem = true;
    d.stack = rn == 31 || rn == 29;
    Some(d)
}

fn reg_list(d: &mut D, first: u32, n: u32, t: &str, lane: Option<u32>) {
    wr!(d, "{{ ");
    for i in 0..n {
        if i > 0 {
            wr!(d, ", ");
        }
        match lane {
            Some(_) => {
                wr!(d, "v{}.{}", (first + i) % 32, t);
            }
            None => {
                wr!(d, "v{}.{}", (first + i) % 32, t);
            }
        }
    }
    wr!(d, " }}");
    if let Some(l) = lane {
        wr!(d, "[{}]", l);
    }
}

fn simd_struct(w: u32) -> Option<D> {
    let q = f(w, 30, 1);
    let l = bit(w, 22);
    let post = bit(w, 23);
    let rm = f(w, 16, 5);
    let rn = f(w, 5, 5);
    let rt = f(w, 0, 5);
    let size = f(w, 10, 2);
    let mut d;
    let total;
    if !bit(w, 24) {
        // multiple structures
        let opcode = f(w, 12, 4);
        let (stem, n) = match opcode {
            0 => (4, 4),
            2 => (1, 4),
            4 => (3, 3),
            6 => (1, 3),
            7 => (1, 1),
            8 => (2, 2),
            10 => (1, 2),
            _ => return None,
        };
        if (!post && rm != 0) || bit(w, 21) {
            return None;
        }
        if stem != 1 && size == 3 && q == 0 {
            return None;
        }
        let t = arr(size, q)?;
        d = D::new(&format!("{}{}", if l { "ld" } else { "st" }, stem), if l { Class::Load } else { Class::Store });
        reg_list(&mut d, rt, n, t, None);
        total = n * if q == 1 { 16 } else { 8 };
    } else {
        let r = bit(w, 21) as u32;
        let opcode = f(w, 13, 3);
        let s = f(w, 12, 1);
        let n = ((opcode & 1) << 1 | r) + 1;
        if !post && rm != 0 {
            return None;
        }
        let stem = format!("{}{}", if l { "ld" } else { "st" }, n);
        match opcode >> 1 {
            0 => {
                d = D::new(&stem, if l { Class::Load } else { Class::Store });
                reg_list(&mut d, rt, n, "b", Some(q << 3 | s << 2 | size));
                total = n;
            }
            1 => {
                if size & 1 != 0 {
                    return None;
                }
                d = D::new(&stem, if l { Class::Load } else { Class::Store });
                reg_list(&mut d, rt, n, "h", Some(q << 2 | s << 1 | size >> 1));
                total = n * 2;
            }
            2 => {
                if size == 0 {
                    d = D::new(&stem, if l { Class::Load } else { Class::Store });
                    reg_list(&mut d, rt, n, "s", Some(q << 1 | s));
                    total = n * 4;
                } else if size == 1 && s == 0 {
                    d = D::new(&stem, if l { Class::Load } else { Class::Store });
                    reg_list(&mut d, rt, n, "d", Some(q));
                    total = n * 8;
                } else {
                    return None;
                }
            }
            _ => {
                if !l || s != 0 {
                    return None;
                }
                let t = arr(size, q)?;
                d = D::new(&format!("ld{}r", n), Class::Load);
                reg_list(&mut d, rt, n, t, None);
                total = n << size;
            }
        }
    }
    wr!(d, ", ");
    if post {
        if rm == 31 {
            put_mem(&mut d, rn, total as i64, Mode::Post);
        } else {
            wr!(d, "[{}], {}", gs(true, rn), gp(true, rm));
        }
    } else {
        put_mem(&mut d, rn, 0, Mode::Off);
    }
    d.mem = true;
    d.stack = rn == 31 || rn == 29;
    if d.stack {
        d.class = if l { Class::StackLoad } else { Class::StackStore };
    }
    Some(d)
}

// ---------------------------------------------------------------- data processing, register

fn dp_reg(w: u32) -> Option<D> {
    let sf = bit(w, 31);
    let rd = f(w, 0, 5);
    let rn = f(w, 5, 5);
    let rm = f(w, 16, 5);
    if !bit(w, 28) {
        let shift = f(w, 22, 2);
        let imm6 = f(w, 10, 6);
        if !sf && imm6 > 31 && !(bit(w, 24) && bit(w, 21)) {
            return None;
        }
        if !bit(w, 24) {
            // logical, shifted register
            let opc = f(w, 29, 2);
            let n = bit(w, 21);
            let mut d;
            if opc == 1 && !n && rn == 31 && shift == 0 && imm6 == 0 {
                d = D::new("mov", Class::MovReg);
                wr!(d, "{}, {}", gp(sf, rd), gp(sf, rm));
                return Some(d);
            }
            if opc == 1 && n && rn == 31 {
                d = D::new("mvn", Class::Alu);
                wr!(d, "{}, {}", gp(sf, rd), gp(sf, rm));
            } else if opc == 3 && !n && rd == 31 {
                d = D::new("tst", Class::Cmp);
                wr!(d, "{}, {}", gp(sf, rn), gp(sf, rm));
            } else {
                d = D::new(["and", "bic", "orr", "orn", "eor", "eon", "ands", "bics"][(opc as usize) << 1 | n as usize], Class::Alu);
                wr!(d, "{}, {}, {}", gp(sf, rd), gp(sf, rn), gp(sf, rm));
            }
            put_shift(&mut d, shift, imm6);
            return Some(d);
        }
        let op = bit(w, 30);
        let s = bit(w, 29);
        if !bit(w, 21) {
            // add / subtract, shifted register
            if shift == 3 {
                return None;
            }
            let mut d;
            if s && rd == 31 {
                d = D::new(if op { "cmp" } else { "cmn" }, Class::Cmp);
                wr!(d, "{}, {}", gp(sf, rn), gp(sf, rm));
            } else if op && rn == 31 {
                d = D::new(if s { "negs" } else { "neg" }, Class::Alu);
                wr!(d, "{}, {}", gp(sf, rd), gp(sf, rm));
            } else {
                let addr = !op && !s && shift == 0 && imm6 != 0;
                d = D::new(["add", "adds", "sub", "subs"][(op as usize) << 1 | s as usize], if addr { Class::Addr } else { Class::Alu });
                wr!(d, "{}, {}, {}", gp(sf, rd), gp(sf, rn), gp(sf, rm));
            }
            put_shift(&mut d, shift, imm6);
            return Some(d);
        }
        // add / subtract, extended register
        if f(w, 22, 2) != 0 || imm6 & 7 > 4 {
            return None;
        }
        let option = f(w, 13, 3);
        let imm3 = f(w, 10, 3);
        let xm = option & 3 == 3;
        let mut d;
        let rdr = if s { gp(sf, rd) } else { gs(sf, rd) };
        if s && rd == 31 {
            d = D::new(if op { "cmp" } else { "cmn" }, Class::Cmp);
            wr!(d, "{}, {}", gs(sf, rn), gp(xm, rm));
        } else {
            d = D::new(["add", "adds", "sub", "subs"][(op as usize) << 1 | s as usize], Class::Addr);
            wr!(d, "{}, {}, {}", rdr, gs(sf, rn), gp(xm, rm));
        }
        let lsl_form = (option == 3 && sf || option == 2 && !sf) && ((rd == 31 && !s) || rn == 31);
        if lsl_form {
            if imm3 != 0 {
                wr!(d, ", lsl #{}", imm3);
            }
        } else {
            wr!(d, ", {}", ["uxtb", "uxth", "uxtw", "uxtx", "sxtb", "sxth", "sxtw", "sxtx"][option as usize]);
            if imm3 != 0 {
                wr!(d, " #{}", imm3);
            }
        }
        return Some(d);
    }
    if bit(w, 24) {
        return dp3(w);
    }
    match f(w, 21, 3) {
        0 => {
            if f(w, 10, 6) != 0 {
                return None;
            }
            let op = bit(w, 30);
            let s = bit(w, 29);
            let mut d;
            if op && rn == 31 {
                d = D::new(if s { "ngcs" } else { "ngc" }, Class::Alu);
                wr!(d, "{}, {}", gp(sf, rd), gp(sf, rm));
            } else {
                d = D::new(["adc", "adcs", "sbc", "sbcs"][(op as usize) << 1 | s as usize], Class::Alu);
                wr!(d, "{}, {}, {}", gp(sf, rd), gp(sf, rn), gp(sf, rm));
            }
            Some(d)
        }
        2 => {
            if !bit(w, 29) || bit(w, 10) || bit(w, 4) {
                return None;
            }
            let mut d = D::new(if bit(w, 30) { "ccmp" } else { "ccmn" }, Class::Cmp);
            if bit(w, 11) {
                wr!(d, "{}, #{}", gp(sf, rn), rm);
            } else {
                wr!(d, "{}, {}", gp(sf, rn), gp(sf, rm));
            }
            wr!(d, ", #{}, {}", f(w, 0, 4), CONDS[f(w, 12, 4) as usize]);
            Some(d)
        }
        4 => {
            if bit(w, 29) {
                return None;
            }
            let op = bit(w, 30);
            let op2 = f(w, 10, 2);
            if op2 > 1 {
                return None;
            }
            let c = f(w, 12, 4);
            let inv = CONDS[(c ^ 1) as usize];
            let mut d;
            let alias = c < 14;
            match (op, op2) {
                (false, 0) => {
                    d = D::new("csel", Class::Select);
                    wr!(d, "{}, {}, {}, {}", gp(sf, rd), gp(sf, rn), gp(sf, rm), CONDS[c as usize]);
                }
                (false, _) | (true, 0) | (true, _) => {
                    let name = match (op, op2) {
                        (false, _) => "csinc",
                        (true, 0) => "csinv",
                        _ => "csneg",
                    };
                    if alias && rn == rm && op2 == 1 && !op && rn == 31 {
                        d = D::new("cset", Class::Select);
                        wr!(d, "{}, {}", gp(sf, rd), inv);
                    } else if alias && rn == rm && !op && op2 == 1 {
                        d = D::new("cinc", Class::Select);
                        wr!(d, "{}, {}, {}", gp(sf, rd), gp(sf, rn), inv);
                    } else if alias && rn == rm && op && op2 == 0 && rn == 31 {
                        d = D::new("csetm", Class::Select);
                        wr!(d, "{}, {}", gp(sf, rd), inv);
                    } else if alias && rn == rm && op && op2 == 0 {
                        d = D::new("cinv", Class::Select);
                        wr!(d, "{}, {}, {}", gp(sf, rd), gp(sf, rn), inv);
                    } else if alias && rn == rm && op && op2 == 1 {
                        d = D::new("cneg", Class::Select);
                        wr!(d, "{}, {}, {}", gp(sf, rd), gp(sf, rn), inv);
                    } else {
                        d = D::new(name, Class::Select);
                        wr!(d, "{}, {}, {}, {}", gp(sf, rd), gp(sf, rn), gp(sf, rm), CONDS[c as usize]);
                    }
                }
            }
            Some(d)
        }
        6 => {
            if bit(w, 29) {
                return None;
            }
            let opcode = f(w, 10, 6);
            if bit(w, 30) {
                // one source
                if f(w, 16, 5) != 0 {
                    return None;
                }
                let name = match (opcode, sf) {
                    (0, _) => "rbit",
                    (1, _) => "rev16",
                    (2, false) => "rev",
                    (2, true) => "rev32",
                    (3, true) => "rev",
                    (4, _) => "clz",
                    (5, _) => "cls",
                    _ => return None,
                };
                let mut d = D::new(name, Class::Alu);
                wr!(d, "{}, {}", gp(sf, rd), gp(sf, rn));
                return Some(d);
            }
            let (name, class) = match opcode {
                2 => ("udiv", Class::Div),
                3 => ("sdiv", Class::Div),
                8 => ("lsl", Class::Alu),
                9 => ("lsr", Class::Alu),
                10 => ("asr", Class::Alu),
                11 => ("ror", Class::Alu),
                16..=23 => {
                    let size = opcode & 3;
                    if sf != (size == 3) {
                        return None;
                    }
                    let name = [["crc32b", "crc32h", "crc32w", "crc32x"], ["crc32cb", "crc32ch", "crc32cw", "crc32cx"]][((opcode >> 2) & 1) as usize][size as usize];
                    let mut d = D::new(name, Class::Alu);
                    wr!(d, "{}, {}, {}", gp(false, rd), gp(false, rn), gp(size == 3, rm));
                    return Some(d);
                }
                _ => return None,
            };
            let mut d = D::new(name, class);
            wr!(d, "{}, {}, {}", gp(sf, rd), gp(sf, rn), gp(sf, rm));
            Some(d)
        }
        _ => None,
    }
}

fn dp3(w: u32) -> Option<D> {
    let sf = bit(w, 31);
    if f(w, 29, 2) != 0 {
        return None;
    }
    let op31 = f(w, 21, 3);
    let o0 = bit(w, 15);
    let rd = f(w, 0, 5);
    let rn = f(w, 5, 5);
    let rm = f(w, 16, 5);
    let ra = f(w, 10, 5);
    let mut d;
    match op31 {
        0 => {
            if ra == 31 {
                d = D::new(if o0 { "mneg" } else { "mul" }, Class::Mul);
                wr!(d, "{}, {}, {}", gp(sf, rd), gp(sf, rn), gp(sf, rm));
            } else {
                d = D::new(if o0 { "msub" } else { "madd" }, Class::Mul);
                wr!(d, "{}, {}, {}, {}", gp(sf, rd), gp(sf, rn), gp(sf, rm), gp(sf, ra));
            }
        }
        1 | 5 if sf => {
            let u = op31 == 5;
            if ra == 31 {
                d = D::new(match (u, o0) {
                    (false, false) => "smull",
                    (false, true) => "smnegl",
                    (true, false) => "umull",
                    (true, true) => "umnegl",
                }, Class::Mul);
                wr!(d, "{}, {}, {}", gp(true, rd), gp(false, rn), gp(false, rm));
            } else {
                d = D::new(match (u, o0) {
                    (false, false) => "smaddl",
                    (false, true) => "smsubl",
                    (true, false) => "umaddl",
                    (true, true) => "umsubl",
                }, Class::Mul);
                wr!(d, "{}, {}, {}, {}", gp(true, rd), gp(false, rn), gp(false, rm), gp(true, ra));
            }
        }
        2 | 6 if sf && !o0 && ra == 31 => {
            d = D::new(if op31 == 2 { "smulh" } else { "umulh" }, Class::Mul);
            wr!(d, "{}, {}, {}", gp(true, rd), gp(true, rn), gp(true, rm));
        }
        _ => return None,
    }
    Some(d)
}

// ---------------------------------------------------------------- scalar floating point

fn ftype(t: u32) -> Option<char> {
    match t {
        0 => Some('s'),
        1 => Some('d'),
        3 => Some('h'),
        _ => None,
    }
}

/// An fp immediate (`fmov #imm8`) as text in the precision of `double`.
fn fp_imm_text(imm8: u32, double: bool) -> String {
    let sign = imm8 >> 7;
    let b = (imm8 >> 6) & 1;
    let lo = imm8 & 0x3f;
    if double {
        let bits = (sign as u64) << 63 | ((b ^ 1) as u64) << 62 | (if b == 1 { 0xffu64 } else { 0 }) << 54 | (lo as u64) << 48;
        format!("{:.8}", f64::from_bits(bits))
    } else {
        let bits = sign << 31 | (b ^ 1) << 30 | (if b == 1 { 0x1f } else { 0 }) << 25 | lo << 19;
        format!("{:.8}", f32::from_bits(bits))
    }
}

fn fp_scalar(w: u32) -> Option<D> {
    let sf = bit(w, 31);
    let ty = f(w, 22, 2);
    let rd = f(w, 0, 5);
    let rn = f(w, 5, 5);
    let rm = f(w, 16, 5);
    if f(w, 29, 2) != 0 {
        return None;
    }
    if f(w, 24, 5) == 0b11111 {
        if sf {
            return None;
        }
        let c = ftype(ty)?;
        let mn = ["fmadd", "fmsub", "fnmadd", "fnmsub"][(bit(w, 21) as usize) << 1 | bit(w, 15) as usize];
        let mut d = D::new(mn, Class::Fp);
        wr!(d, "{}, {}, {}, {}", Fr(c, rd), Fr(c, rn), Fr(c, rm), Fr(c, f(w, 10, 5)));
        return Some(d);
    }
    if !bit(w, 21) {
        // fixed point conversions
        let scale = f(w, 10, 6);
        if !sf && scale < 32 {
            return None;
        }
        let c = ftype(ty)?;
        let fbits = 64 - scale;
        let mut d;
        match (f(w, 19, 2), f(w, 16, 3)) {
            (0, 2) | (0, 3) => {
                d = D::new(if bit(w, 16) { "ucvtf" } else { "scvtf" }, Class::Fp);
                wr!(d, "{}, {}, #{}", Fr(c, rd), gp(sf, rn), fbits);
            }
            (3, 0) | (3, 1) => {
                d = D::new(if bit(w, 16) { "fcvtzu" } else { "fcvtzs" }, Class::Fp);
                wr!(d, "{}, {}, #{}", gp(sf, rd), Fr(c, rn), fbits);
            }
            _ => return None,
        }
        return Some(d);
    }
    if f(w, 10, 6) == 0 {
        return fp_int(w);
    }
    if sf {
        return None;
    }
    let c = ftype(ty)?;
    match f(w, 10, 2) {
        1 => {
            let mut d = D::new(if bit(w, 4) { "fccmpe" } else { "fccmp" }, Class::Fp);
            wr!(d, "{}, {}, #{}, {}", Fr(c, rn), Fr(c, rm), f(w, 0, 4), CONDS[f(w, 12, 4) as usize]);
            Some(d)
        }
        2 => {
            let mn = ["fmul", "fdiv", "fadd", "fsub", "fmax", "fmin", "fmaxnm", "fminnm", "fnmul"].get(f(w, 12, 4) as usize)?;
            let mut d = D::new(mn, Class::Fp);
            wr!(d, "{}, {}, {}", Fr(c, rd), Fr(c, rn), Fr(c, rm));
            Some(d)
        }
        3 => {
            let mut d = D::new("fcsel", Class::Select);
            wr!(d, "{}, {}, {}, {}", Fr(c, rd), Fr(c, rn), Fr(c, rm), CONDS[f(w, 12, 4) as usize]);
            Some(d)
        }
        _ => {
            if bit(w, 12) {
                if f(w, 5, 5) != 0 {
                    return None;
                }
                let mut d = D::new("fmov", Class::MovImm);
                if ty == 3 {
                    let imm8 = f(w, 13, 8);
                    let b = (imm8 >> 6) & 1;
                    let e = ((b ^ 1) << 4 | (if b == 1 { 3 } else { 0 }) << 2 | (imm8 >> 4) & 3) as i32;
                    let m = (imm8 & 15) as f32;
                    let v = (1.0 + m / 16.0) * 2f32.powi(e - 15);
                    wr!(d, "{}, #{:.8}", Fr(c, rd), if imm8 >> 7 == 1 { -v } else { v });
                } else {
                    wr!(d, "{}, #{}", Fr(c, rd), fp_imm_text(f(w, 13, 8), ty == 1));
                }
                Some(d)
            } else if bit(w, 13) {
                if f(w, 14, 2) != 0 || f(w, 0, 3) != 0 {
                    return None;
                }
                let mut d = D::new(if bit(w, 4) { "fcmpe" } else { "fcmp" }, Class::Fp);
                if bit(w, 3) {
                    if rm != 0 {
                        return None;
                    }
                    wr!(d, "{}, #0.0", Fr(c, rn));
                } else {
                    wr!(d, "{}, {}", Fr(c, rn), Fr(c, rm));
                }
                Some(d)
            } else if bit(w, 14) {
                let opcode = f(w, 15, 6);
                let mut d;
                match opcode {
                    0 => {
                        d = D::new("fmov", Class::MovReg);
                        wr!(d, "{}, {}", Fr(c, rd), Fr(c, rn));
                    }
                    1..=3 | 8..=12 | 14 | 15 => {
                        let mn = match opcode {
                            1 => "fabs",
                            2 => "fneg",
                            3 => "fsqrt",
                            8 => "frintn",
                            9 => "frintp",
                            10 => "frintm",
                            11 => "frintz",
                            12 => "frinta",
                            14 => "frintx",
                            _ => "frinti",
                        };
                        d = D::new(mn, Class::Fp);
                        wr!(d, "{}, {}", Fr(c, rd), Fr(c, rn));
                    }
                    4 | 5 | 7 => {
                        let dc = ['s', 'd', 'x', 'h'][opcode as usize & 3];
                        let dc = if opcode == 7 { 'h' } else { dc };
                        if dc == c {
                            return None;
                        }
                        d = D::new("fcvt", Class::Fp);
                        wr!(d, "{}, {}", Fr(dc, rd), Fr(c, rn));
                    }
                    _ => return None,
                }
                Some(d)
            } else {
                None
            }
        }
    }
}

fn fp_int(w: u32) -> Option<D> {
    let sf = bit(w, 31);
    let ty = f(w, 22, 2);
    let rmode = f(w, 19, 2);
    let opcode = f(w, 16, 3);
    let rd = f(w, 0, 5);
    let rn = f(w, 5, 5);
    let mut d;
    match (rmode, opcode) {
        (0, 2) | (0, 3) => {
            let c = ftype(ty)?;
            d = D::new(if opcode == 3 { "ucvtf" } else { "scvtf" }, Class::Fp);
            wr!(d, "{}, {}", Fr(c, rd), gp(sf, rn));
        }
        (0, 0) | (0, 1) | (0, 4) | (0, 5) | (1, 0) | (1, 1) | (2, 0) | (2, 1) | (3, 0) | (3, 1) => {
            let c = ftype(ty)?;
            let mn = match (rmode, opcode) {
                (0, 0) => "fcvtns",
                (0, 1) => "fcvtnu",
                (0, 4) => "fcvtas",
                (0, 5) => "fcvtau",
                (1, 0) => "fcvtps",
                (1, 1) => "fcvtpu",
                (2, 0) => "fcvtms",
                (2, 1) => "fcvtmu",
                (3, 0) => "fcvtzs",
                _ => "fcvtzu",
            };
            d = D::new(mn, Class::Fp);
            wr!(d, "{}, {}", gp(sf, rd), Fr(c, rn));
        }
        (0, 6) | (0, 7) => {
            let c = ftype(ty)?;
            if (ty == 0 && sf) || (ty == 1 && !sf) {
                return None;
            }
            d = D::new("fmov", Class::MovReg);
            if opcode == 6 {
                wr!(d, "{}, {}", gp(sf, rd), Fr(c, rn));
            } else {
                wr!(d, "{}, {}", Fr(c, rd), gp(sf, rn));
            }
        }
        (1, 6) | (1, 7) if sf && ty == 2 => {
            d = D::new("fmov", Class::MovReg);
            if opcode == 6 {
                wr!(d, "{}, v{}.d[1]", gp(true, rd), rn);
            } else {
                wr!(d, "v{}.d[1], {}", rd, gp(true, rn));
            }
        }
        (3, 6) if !sf && ty == 1 => {
            d = D::new("fjcvtzs", Class::Fp);
            wr!(d, "{}, d{}", gp(false, rd), rn);
        }
        _ => return None,
    }
    Some(d)
}

// ---------------------------------------------------------------- advanced simd

fn simd_fp(w: u32) -> Option<D> {
    if w & 0xFFFF_0C00 == 0x4E28_0800 {
        let name = match f(w, 12, 4) {
            4 => "aese",
            5 => "aesd",
            6 => "aesmc",
            7 => "aesimc",
            _ => return None,
        };
        let mut d = D::new(name, Class::Vector);
        wr!(d, "v{}.16b, v{}.16b", f(w, 0, 5), f(w, 5, 5));
        return Some(d);
    }
    // sha3: eor3 / bcax / xar / rax1
    if w & 0xFFC0_8000 == 0xCE00_0000 {
        let (rd, rn, rm, ra) = (f(w, 0, 5), f(w, 5, 5), f(w, 16, 5), f(w, 10, 5));
        let mut d = D::new(if bit(w, 21) { "bcax" } else { "eor3" }, Class::Vector);
        wr!(d, "v{}.16b, v{}.16b, v{}.16b, v{}.16b", rd, rn, rm, ra);
        return Some(d);
    }
    if w & 0xFFE0_0000 == 0xCE80_0000 {
        let mut d = D::new("xar", Class::Vector);
        wr!(d, "v{}.2d, v{}.2d, v{}.2d, #{}", f(w, 0, 5), f(w, 5, 5), f(w, 16, 5), f(w, 10, 6));
        return Some(d);
    }
    if w & 0xFFE0_FC00 == 0xCE60_8C00 {
        let mut d = D::new("rax1", Class::Vector);
        wr!(d, "v{}.2d, v{}.2d, v{}.2d", f(w, 0, 5), f(w, 5, 5), f(w, 16, 5));
        return Some(d);
    }
    // sdot / udot
    if w & 0x9F20_FC00 == 0x0E00_9400 && f(w, 22, 2) == 2 {
        let q = f(w, 30, 1);
        let mut d = D::new(if bit(w, 29) { "udot" } else { "sdot" }, Class::Vector);
        wr!(d, "v{}.{}, v{}.{}, v{}.{}", f(w, 0, 5), if q == 1 { "4s" } else { "2s" }, f(w, 5, 5), if q == 1 { "16b" } else { "8b" }, f(w, 16, 5), if q == 1 { "16b" } else { "8b" });
        return Some(d);
    }
    if w & 0x9F20_F400 == 0x0F00_E000 && f(w, 22, 2) == 2 {
        let q = f(w, 30, 1);
        let mut d = D::new(if bit(w, 29) { "udot" } else { "sdot" }, Class::Vector);
        wr!(d, "v{}.{}, v{}.{}, v{}.4b[{}]", f(w, 0, 5), if q == 1 { "4s" } else { "2s" }, f(w, 5, 5), if q == 1 { "16b" } else { "8b" }, bit(w, 20) as u32 * 16 + f(w, 16, 4), f(w, 11, 1) << 1 | f(w, 21, 1));
        return Some(d);
    }
    if w & 0xFFFF_0C00 == 0x5E28_0800 {
        let (name, ops) = match f(w, 12, 4) {
            0 => ("sha1h", format!("s{}, s{}", f(w, 0, 5), f(w, 5, 5))),
            1 => ("sha1su1", format!("v{}.4s, v{}.4s", f(w, 0, 5), f(w, 5, 5))),
            2 => ("sha256su0", format!("v{}.4s, v{}.4s", f(w, 0, 5), f(w, 5, 5))),
            _ => return None,
        };
        let mut d = D::new(name, Class::Vector);
        d.ops = ops;
        return Some(d);
    }
    if w & 0xFFE0_8C00 == 0x5E00_0000 {
        let (rd, rn, rm) = (f(w, 0, 5), f(w, 5, 5), f(w, 16, 5));
        let (name, ops) = match f(w, 12, 3) {
            0 => ("sha1c", format!("q{}, s{}, v{}.4s", rd, rn, rm)),
            1 => ("sha1p", format!("q{}, s{}, v{}.4s", rd, rn, rm)),
            2 => ("sha1m", format!("q{}, s{}, v{}.4s", rd, rn, rm)),
            3 => ("sha1su0", format!("v{}.4s, v{}.4s, v{}.4s", rd, rn, rm)),
            4 => ("sha256h", format!("q{}, q{}, v{}.4s", rd, rn, rm)),
            5 => ("sha256h2", format!("q{}, q{}, v{}.4s", rd, rn, rm)),
            6 => ("sha256su1", format!("v{}.4s, v{}.4s, v{}.4s", rd, rn, rm)),
            _ => return None,
        };
        let mut d = D::new(name, Class::Vector);
        d.ops = ops;
        return Some(d);
    }
    match f(w, 24, 5) {
        0b11110 | 0b11111 => {
            if f(w, 30, 2) == 1 {
                simd_scalar(w)
            } else if !bit(w, 30) && !bit(w, 29) {
                fp_scalar(w)
            } else {
                None
            }
        }
        0b01110 if !bit(w, 31) => simd_vector(w),
        0b01111 if !bit(w, 31) => simd_vec_imm(w),
        _ => None,
    }
}

/// The element index and the register number of an indexed-element operand (`vM.T[idx]`).
fn elem_ref(size: u32, h: u32, l: u32, m: u32, rm4: u32) -> Option<(u32, u32)> {
    match size {
        1 => Some((rm4, h << 2 | l << 1 | m)),
        2 => Some((m << 4 | rm4, h << 1 | l)),
        3 if l == 0 => Some((m << 4 | rm4, h)),
        _ => None,
    }
}

fn simd_scalar(w: u32) -> Option<D> {
    let u = bit(w, 29);
    let size = f(w, 22, 2);
    let rd = f(w, 0, 5);
    let rn = f(w, 5, 5);
    let rm = f(w, 16, 5);
    let sz = size & 1;
    let hi = size >> 1;
    let fc = if sz == 0 { 's' } else { 'd' };
    if f(w, 24, 5) == 0b11110 {
        if bit(w, 21) {
            if bit(w, 10) {
                let opcode = f(w, 11, 5);
                let dname = match (u, opcode) {
                    (false, 0x06) => "cmgt",
                    (false, 0x07) => "cmge",
                    (false, 0x08) => "sshl",
                    (false, 0x0a) => "srshl",
                    (false, 0x10) => "add",
                    (false, 0x11) => "cmtst",
                    (true, 0x06) => "cmhi",
                    (true, 0x07) => "cmhs",
                    (true, 0x08) => "ushl",
                    (true, 0x0a) => "urshl",
                    (true, 0x10) => "sub",
                    (true, 0x11) => "cmeq",
                    _ => "",
                };
                if !dname.is_empty() {
                    if size != 3 {
                        return None;
                    }
                    let mut d = D::new(dname, Class::Fp);
                    wr!(d, "d{}, d{}, d{}", rd, rn, rm);
                    return Some(d);
                }
                let iname = match (u, opcode) {
                    (false, 1) => "sqadd",
                    (true, 1) => "uqadd",
                    (false, 5) => "sqsub",
                    (true, 5) => "uqsub",
                    (false, 9) => "sqshl",
                    (true, 9) => "uqshl",
                    (false, 0xb) => "sqrshl",
                    (true, 0xb) => "uqrshl",
                    (false, 0x16) => "sqdmulh",
                    (true, 0x16) => "sqrdmulh",
                    _ => "",
                };
                if !iname.is_empty() {
                    if opcode == 0x16 && (size == 0 || size == 3) {
                        return None;
                    }
                    let c = fchar(size);
                    let mut d = D::new(iname, Class::Fp);
                    wr!(d, "{}, {}, {}", Fr(c, rd), Fr(c, rn), Fr(c, rm));
                    return Some(d);
                }
                let fname = match (u, opcode, hi) {
                    (false, 0x1b, 0) => "fmulx",
                    (false, 0x1c, 0) => "fcmeq",
                    (false, 0x1f, 0) => "frecps",
                    (false, 0x1f, 1) => "frsqrts",
                    (true, 0x1a, 1) => "fabd",
                    (true, 0x1c, 0) => "fcmge",
                    (true, 0x1c, 1) => "fcmgt",
                    (true, 0x1d, 0) => "facge",
                    (true, 0x1d, 1) => "facgt",
                    _ => return None,
                };
                let mut d = D::new(fname, Class::Fp);
                wr!(d, "{}, {}, {}", Fr(fc, rd), Fr(fc, rn), Fr(fc, rm));
                return Some(d);
            }
            return match f(w, 10, 2) {
                0 => {
                    let name = match (u, f(w, 12, 4)) {
                        (false, 9) => "sqdmlal",
                        (false, 11) => "sqdmlsl",
                        (false, 13) => "sqdmull",
                        _ => return None,
                    };
                    if size != 1 && size != 2 {
                        return None;
                    }
                    let mut d = D::new(name, Class::Fp);
                    wr!(d, "{}, {}, {}", Fr(fchar(size + 1), rd), Fr(fchar(size), rn), Fr(fchar(size), rm));
                    Some(d)
                }
                2 => {
                    let opcode = f(w, 12, 5);
                    match f(w, 17, 5) {
                        0x10 => scalar_two_misc(u, size, opcode, rd, rn),
                        0x18 => {
                            let mut d;
                            match (u, opcode) {
                                (false, 0x1b) if size == 3 => {
                                    d = D::new("addp", Class::Fp);
                                    wr!(d, "d{}, v{}.2d", rd, rn);
                                }
                                (true, 0x0c) | (true, 0x0d) | (true, 0x0f) => {
                                    let name = match (opcode, hi) {
                                        (0x0c, 0) => "fmaxnmp",
                                        (0x0c, _) => "fminnmp",
                                        (0x0d, 0) => "faddp",
                                        (0x0f, 0) => "fmaxp",
                                        (0x0f, _) => "fminp",
                                        _ => return None,
                                    };
                                    d = D::new(name, Class::Fp);
                                    wr!(d, "{}{}, v{}.2{}", fc, rd, rn, fc);
                                }
                                _ => return None,
                            }
                            Some(d)
                        }
                        _ => None,
                    }
                }
                _ => None,
            };
        }
        // scalar copy (dup element)
        if !u && f(w, 22, 2) == 0 && !bit(w, 15) && bit(w, 10) && f(w, 11, 4) == 0 {
            let imm5 = f(w, 16, 5);
            if imm5 == 0 {
                return None;
            }
            let sz = imm5.trailing_zeros();
            if sz > 3 {
                return None;
            }
            let mut d = D::new("mov", Class::Fp);
            wr!(d, "{}, v{}.{}[{}]", Fr(fchar(sz), rd), rn, elem(sz), imm5 >> (sz + 1));
            return Some(d);
        }
        return None;
    }
    // 11111: shift by immediate, indexed element
    if bit(w, 10) {
        let immh = f(w, 19, 4);
        if immh == 0 || bit(w, 23) {
            return None;
        }
        let immhb = f(w, 16, 7);
        let idx = 31 - immh.leading_zeros();
        let esize = 8u32 << idx;
        let right = 2 * esize - immhb;
        let left = immhb - esize;
        let opcode = f(w, 11, 5);
        let mut d;
        match (u, opcode) {
            (_, 0x00) | (_, 0x02) | (_, 0x04) | (_, 0x06) | (true, 0x08) => {
                if idx != 3 {
                    return None;
                }
                let name = match (u, opcode) {
                    (false, 0) => "sshr",
                    (true, 0) => "ushr",
                    (false, 2) => "ssra",
                    (true, 2) => "usra",
                    (false, 4) => "srshr",
                    (true, 4) => "urshr",
                    (false, 6) => "srsra",
                    (true, 6) => "ursra",
                    _ => "sri",
                };
                d = D::new(name, Class::Fp);
                wr!(d, "d{}, d{}, #{}", rd, rn, right);
            }
            (_, 0x0a) => {
                if idx != 3 {
                    return None;
                }
                d = D::new(if u { "sli" } else { "shl" }, Class::Fp);
                wr!(d, "d{}, d{}, #{}", rd, rn, left);
            }
            (true, 0x0c) | (_, 0x0e) => {
                let name = match (u, opcode) {
                    (true, 0x0c) => "sqshlu",
                    (false, _) => "sqshl",
                    _ => "uqshl",
                };
                d = D::new(name, Class::Fp);
                wr!(d, "{}, {}, #{}", Fr(fchar(idx), rd), Fr(fchar(idx), rn), left);
            }
            (_, 0x10) | (_, 0x11) | (_, 0x12) | (_, 0x13) => {
                if idx == 3 {
                    return None;
                }
                let name = match (u, opcode) {
                    (true, 0x10) => "sqshrun",
                    (true, 0x11) => "sqrshrun",
                    (false, 0x12) => "sqshrn",
                    (true, 0x12) => "uqshrn",
                    (false, 0x13) => "sqrshrn",
                    (true, 0x13) => "uqrshrn",
                    _ => return None,
                };
                d = D::new(name, Class::Fp);
                wr!(d, "{}, {}, #{}", Fr(fchar(idx), rd), Fr(fchar(idx + 1), rn), right);
            }
            (_, 0x1c) | (_, 0x1f) => {
                if idx < 2 {
                    return None;
                }
                let name = match (u, opcode) {
                    (false, 0x1c) => "scvtf",
                    (true, 0x1c) => "ucvtf",
                    (false, _) => "fcvtzs",
                    _ => "fcvtzu",
                };
                d = D::new(name, Class::Fp);
                wr!(d, "{}, {}, #{}", Fr(fchar(idx), rd), Fr(fchar(idx), rn), right);
            }
            _ => return None,
        }
        return Some(d);
    }
    let l = f(w, 21, 1);
    let m = f(w, 20, 1);
    let h = f(w, 11, 1);
    let opcode = f(w, 12, 4);
    let (rmv, idx) = elem_ref(size, h, l, m, f(w, 16, 4))?;
    let mut d;
    match (u, opcode) {
        (false, 1) | (false, 5) | (false, 9) | (true, 9) if size >= 2 => {
            let name = match (u, opcode) {
                (false, 1) => "fmla",
                (false, 5) => "fmls",
                (false, _) => "fmul",
                _ => "fmulx",
            };
            d = D::new(name, Class::Fp);
            wr!(d, "{}, {}, v{}.{}[{}]", Fr(fc, rd), Fr(fc, rn), rmv, elem(size), idx);
        }
        (false, 3) | (false, 7) | (false, 11) if size == 1 || size == 2 => {
            let name = match opcode {
                3 => "sqdmlal",
                7 => "sqdmlsl",
                _ => "sqdmull",
            };
            d = D::new(name, Class::Fp);
            wr!(d, "{}, {}, v{}.{}[{}]", Fr(fchar(size + 1), rd), Fr(fchar(size), rn), rmv, elem(size), idx);
        }
        (false, 12) | (false, 13) if size == 1 || size == 2 => {
            d = D::new(if opcode == 12 { "sqdmulh" } else { "sqrdmulh" }, Class::Fp);
            wr!(d, "{}, {}, v{}.{}[{}]", Fr(fchar(size), rd), Fr(fchar(size), rn), rmv, elem(size), idx);
        }
        _ => return None,
    }
    Some(d)
}

fn scalar_two_misc(u: bool, size: u32, opcode: u32, rd: u32, rn: u32) -> Option<D> {
    let sz = size & 1;
    let hi = size >> 1;
    let fc = if sz == 0 { 's' } else { 'd' };
    let mut d;
    match (u, opcode) {
        (false, 3) | (true, 3) | (false, 7) | (true, 7) => {
            let name = match (u, opcode) {
                (false, 3) => "suqadd",
                (true, 3) => "usqadd",
                (false, _) => "sqabs",
                _ => "sqneg",
            };
            d = D::new(name, Class::Fp);
            wr!(d, "{}, {}", Fr(fchar(size), rd), Fr(fchar(size), rn));
        }
        (_, 8) | (_, 9) | (false, 10) => {
            if size != 3 {
                return None;
            }
            let name = match (u, opcode) {
                (false, 8) => "cmgt",
                (true, 8) => "cmge",
                (false, 9) => "cmeq",
                (true, 9) => "cmle",
                _ => "cmlt",
            };
            d = D::new(name, Class::Fp);
            wr!(d, "d{}, d{}, #0", rd, rn);
        }
        (_, 11) => {
            if size != 3 {
                return None;
            }
            d = D::new(if u { "neg" } else { "abs" }, Class::Fp);
            wr!(d, "d{}, d{}", rd, rn);
        }
        (true, 0x12) | (false, 0x14) | (true, 0x14) => {
            if size == 3 {
                return None;
            }
            let name = match (u, opcode) {
                (true, 0x12) => "sqxtun",
                (false, _) => "sqxtn",
                _ => "uqxtn",
            };
            d = D::new(name, Class::Fp);
            wr!(d, "{}, {}", Fr(fchar(size), rd), Fr(fchar(size + 1), rn));
        }
        (true, 0x16) if hi == 0 && sz == 1 => {
            d = D::new("fcvtxn", Class::Fp);
            wr!(d, "s{}, d{}", rd, rn);
        }
        (_, 0x0c) | (_, 0x0d) | (false, 0x0e) if hi == 1 => {
            let name = match (u, opcode) {
                (false, 0x0c) => "fcmgt",
                (true, 0x0c) => "fcmge",
                (false, 0x0d) => "fcmeq",
                (true, 0x0d) => "fcmle",
                _ => "fcmlt",
            };
            d = D::new(name, Class::Fp);
            wr!(d, "{}{}, {}{}, #0.0", fc, rd, fc, rn);
        }
        (_, 0x1a) | (_, 0x1b) | (_, 0x1c) | (_, 0x1d) | (false, 0x1f) => {
            let name = match (u, opcode, hi) {
                (false, 0x1a, 0) => "fcvtns",
                (false, 0x1a, _) => "fcvtps",
                (true, 0x1a, 0) => "fcvtnu",
                (true, 0x1a, _) => "fcvtpu",
                (false, 0x1b, 0) => "fcvtms",
                (false, 0x1b, _) => "fcvtzs",
                (true, 0x1b, 0) => "fcvtmu",
                (true, 0x1b, _) => "fcvtzu",
                (false, 0x1c, 0) => "fcvtas",
                (true, 0x1c, 0) => "fcvtau",
                (false, 0x1d, 0) => "scvtf",
                (false, 0x1d, _) => "frecpe",
                (true, 0x1d, 0) => "ucvtf",
                (true, 0x1d, _) => "frsqrte",
                (false, 0x1f, 1) => "frecpx",
                _ => return None,
            };
            d = D::new(name, Class::Fp);
            wr!(d, "{}{}, {}{}", fc, rd, fc, rn);
        }
        _ => return None,
    }
    Some(d)
}

fn int3(u: bool, opc: u32) -> Option<&'static str> {
    Some(match (u, opc) {
        (false, 0) => "shadd",
        (false, 1) => "sqadd",
        (false, 2) => "srhadd",
        (false, 4) => "shsub",
        (false, 5) => "sqsub",
        (false, 6) => "cmgt",
        (false, 7) => "cmge",
        (false, 8) => "sshl",
        (false, 9) => "sqshl",
        (false, 10) => "srshl",
        (false, 11) => "sqrshl",
        (false, 12) => "smax",
        (false, 13) => "smin",
        (false, 14) => "sabd",
        (false, 15) => "saba",
        (false, 16) => "add",
        (false, 17) => "cmtst",
        (false, 18) => "mla",
        (false, 19) => "mul",
        (false, 20) => "smaxp",
        (false, 21) => "sminp",
        (false, 22) => "sqdmulh",
        (false, 23) => "addp",
        (true, 0) => "uhadd",
        (true, 1) => "uqadd",
        (true, 2) => "urhadd",
        (true, 4) => "uhsub",
        (true, 5) => "uqsub",
        (true, 6) => "cmhi",
        (true, 7) => "cmhs",
        (true, 8) => "ushl",
        (true, 9) => "uqshl",
        (true, 10) => "urshl",
        (true, 11) => "uqrshl",
        (true, 12) => "umax",
        (true, 13) => "umin",
        (true, 14) => "uabd",
        (true, 15) => "uaba",
        (true, 16) => "sub",
        (true, 17) => "cmeq",
        (true, 18) => "mls",
        (true, 19) => "pmul",
        (true, 20) => "umaxp",
        (true, 21) => "uminp",
        (true, 22) => "sqrdmulh",
        _ => return None,
    })
}

fn simd_vector(w: u32) -> Option<D> {
    let q = f(w, 30, 1);
    let u = bit(w, 29);
    let size = f(w, 22, 2);
    let rd = f(w, 0, 5);
    let rn = f(w, 5, 5);
    let rm = f(w, 16, 5);
    if bit(w, 21) {
        if bit(w, 10) {
            return three_same(w, q, u, size, rd, rn, rm);
        }
        return match f(w, 10, 2) {
            0 => three_diff(w, q, u, size, rd, rn, rm),
            2 => match f(w, 17, 5) {
                0x10 => two_misc(w, q, u, size, rd, rn),
                0x18 => across(w, q, u, size, rd, rn),
                _ => None,
            },
            _ => None,
        };
    }
    let b15 = bit(w, 15);
    if bit(w, 10) && size == 0 && !b15 {
        return simd_copy(w, q, u, rd, rn);
    }
    if bit(w, 10) && bit(w, 22) && f(w, 14, 2) == 0 {
        return three_same_f16(w, q, u, rd, rn, rm);
    }
    if b15 {
        return None;
    }
    let t = arr2(size, q);
    if u {
        // ext
        if bit(w, 10) || size != 0 || (q == 0 && bit(w, 14)) {
            return None;
        }
        let ta = if q == 1 { "16b" } else { "8b" };
        let mut d = D::new("ext", Class::Vector);
        wr!(d, "v{}.{}, v{}.{}, v{}.{}, #{}", rd, ta, rn, ta, rm, ta, f(w, 11, 4));
        return Some(d);
    }
    match f(w, 10, 2) {
        2 => {
            let name = match f(w, 12, 3) {
                1 => "uzp1",
                2 => "trn1",
                3 => "zip1",
                5 => "uzp2",
                6 => "trn2",
                7 => "zip2",
                _ => return None,
            };
            let t = t?;
            let mut d = D::new(name, Class::Vector);
            wr!(d, "v{}.{}, v{}.{}, v{}.{}", rd, t, rn, t, rm, t);
            Some(d)
        }
        0 => {
            if size != 0 {
                return None;
            }
            let len = f(w, 13, 2) + 1;
            let mut d = D::new(if bit(w, 12) { "tbx" } else { "tbl" }, Class::Vector);
            let ta = if q == 1 { "16b" } else { "8b" };
            wr!(d, "v{}.{}, ", rd, ta);
            reg_list(&mut d, rn, len, "16b", None);
            wr!(d, ", v{}.{}", rm, ta);
            Some(d)
        }
        _ => None,
    }
}

/// half precision three-register-same: `0 Q U 01110 a 10 Rm 00 opcode 1 Rn Rd`
fn three_same_f16(w: u32, q: u32, u: bool, rd: u32, rn: u32, rm: u32) -> Option<D> {
    let a = bit(w, 23);
    let name = match (u, a, f(w, 11, 3)) {
        (false, false, 0) => "fmaxnm",
        (false, false, 1) => "fmla",
        (false, false, 2) => "fadd",
        (false, false, 3) => "fmulx",
        (false, false, 4) => "fcmeq",
        (false, false, 6) => "fmax",
        (false, false, 7) => "frecps",
        (false, true, 0) => "fminnm",
        (false, true, 1) => "fmls",
        (false, true, 2) => "fsub",
        (false, true, 6) => "fmin",
        (false, true, 7) => "frsqrts",
        (true, false, 0) => "fmaxnmp",
        (true, false, 2) => "faddp",
        (true, false, 3) => "fmul",
        (true, false, 4) => "fcmge",
        (true, false, 5) => "facge",
        (true, false, 6) => "fmaxp",
        (true, false, 7) => "fdiv",
        (true, true, 0) => "fminnmp",
        (true, true, 2) => "fabd",
        (true, true, 4) => "fcmgt",
        (true, true, 5) => "facgt",
        (true, true, 6) => "fminp",
        _ => return None,
    };
    let t = if q == 1 { "8h" } else { "4h" };
    let mut d = D::new(name, Class::Vector);
    wr!(d, "v{}.{}, v{}.{}, v{}.{}", rd, t, rn, t, rm, t);
    Some(d)
}

fn three_same(w: u32, q: u32, u: bool, size: u32, rd: u32, rn: u32, rm: u32) -> Option<D> {
    let opcode = f(w, 11, 5);
    let mut d;
    if opcode == 3 {
        let t = if q == 1 { "16b" } else { "8b" };
        let name = match (u, size) {
            (false, 0) => "and",
            (false, 1) => "bic",
            (false, 2) => "orr",
            (false, _) => "orn",
            (true, 0) => "eor",
            (true, 1) => "bsl",
            (true, 2) => "bit",
            (true, _) => "bif",
        };
        if name == "orr" && rn == rm {
            d = D::new("mov", Class::MovReg);
            wr!(d, "v{}.{}, v{}.{}", rd, t, rn, t);
            d.class = Class::Vector;
        } else {
            d = D::new(name, Class::Vector);
            wr!(d, "v{}.{}, v{}.{}, v{}.{}", rd, t, rn, t, rm, t);
        }
        return Some(d);
    }
    if opcode < 0x18 {
        let name = int3(u, opcode)?;
        let t = arr2(size, q)?;
        if size == 3 && matches!(opcode, 0 | 2 | 4 | 12 | 13 | 14 | 15 | 18 | 19 | 20 | 21 | 22) {
            return None;
        }
        if opcode == 22 && size == 0 {
            return None;
        }
        if opcode == 19 && u && size != 0 {
            return None;
        }
        d = D::new(name, Class::Vector);
        wr!(d, "v{}.{}, v{}.{}, v{}.{}", rd, t, rn, t, rm, t);
        return Some(d);
    }
    let hi = size >> 1;
    let t = farr(size & 1, q)?;
    let name = match (u, opcode, hi) {
        (false, 0x18, 0) => "fmaxnm",
        (false, 0x18, _) => "fminnm",
        (false, 0x19, 0) => "fmla",
        (false, 0x19, _) => "fmls",
        (false, 0x1a, 0) => "fadd",
        (false, 0x1a, _) => "fsub",
        (false, 0x1b, 0) => "fmulx",
        (false, 0x1c, 0) => "fcmeq",
        (false, 0x1e, 0) => "fmax",
        (false, 0x1e, _) => "fmin",
        (false, 0x1f, 0) => "frecps",
        (false, 0x1f, _) => "frsqrts",
        (true, 0x18, 0) => "fmaxnmp",
        (true, 0x18, _) => "fminnmp",
        (true, 0x1a, 0) => "faddp",
        (true, 0x1a, _) => "fabd",
        (true, 0x1b, 0) => "fmul",
        (true, 0x1c, 0) => "fcmge",
        (true, 0x1c, _) => "fcmgt",
        (true, 0x1d, 0) => "facge",
        (true, 0x1d, _) => "facgt",
        (true, 0x1e, 0) => "fmaxp",
        (true, 0x1e, _) => "fminp",
        (true, 0x1f, 0) => "fdiv",
        _ => return None,
    };
    d = D::new(name, Class::Vector);
    wr!(d, "v{}.{}, v{}.{}, v{}.{}", rd, t, rn, t, rm, t);
    Some(d)
}

fn three_diff(w: u32, q: u32, u: bool, size: u32, rd: u32, rn: u32, rm: u32) -> Option<D> {
    let opcode = f(w, 12, 4);
    let sfx = if q == 1 { "2" } else { "" };
    if opcode == 14 {
        if u {
            return None;
        }
        let mut d = D::new(&format!("pmull{}", sfx), Class::Vector);
        match size {
            0 => {
                wr!(d, "v{}.8h, v{}.{}, v{}.{}", rd, rn, arr(0, q)?, rm, arr(0, q)?);
            }
            3 => {
                let t = if q == 1 { "2d" } else { "1d" };
                wr!(d, "v{}.1q, v{}.{}, v{}.{}", rd, rn, t, rm, t);
            }
            _ => return None,
        }
        return Some(d);
    }
    let name = match (opcode, u) {
        (0, false) => "saddl",
        (0, true) => "uaddl",
        (1, false) => "saddw",
        (1, true) => "uaddw",
        (2, false) => "ssubl",
        (2, true) => "usubl",
        (3, false) => "ssubw",
        (3, true) => "usubw",
        (4, false) => "addhn",
        (4, true) => "raddhn",
        (5, false) => "sabal",
        (5, true) => "uabal",
        (6, false) => "subhn",
        (6, true) => "rsubhn",
        (7, false) => "sabdl",
        (7, true) => "uabdl",
        (8, false) => "smlal",
        (8, true) => "umlal",
        (9, false) => "sqdmlal",
        (10, false) => "smlsl",
        (10, true) => "umlsl",
        (11, false) => "sqdmlsl",
        (12, false) => "smull",
        (12, true) => "umull",
        (13, false) => "sqdmull",
        _ => return None,
    };
    if matches!(opcode, 9 | 11 | 13) && (size == 0 || size == 3) {
        return None;
    }
    let ta = arr_wide(size)?;
    let tb = arr(size, q)?;
    let mut d = D::new(&format!("{}{}", name, sfx), Class::Vector);
    match opcode {
        1 | 3 => {
            wr!(d, "v{}.{}, v{}.{}, v{}.{}", rd, ta, rn, ta, rm, tb);
        }
        4 | 6 => {
            wr!(d, "v{}.{}, v{}.{}, v{}.{}", rd, tb, rn, ta, rm, ta);
        }
        _ => {
            wr!(d, "v{}.{}, v{}.{}, v{}.{}", rd, ta, rn, tb, rm, tb);
        }
    }
    Some(d)
}

fn two_misc(w: u32, q: u32, u: bool, size: u32, rd: u32, rn: u32) -> Option<D> {
    let opcode = f(w, 12, 5);
    let sfx = if q == 1 { "2" } else { "" };
    let mut d;
    let same = |name: &str, t: &str| -> D {
        let mut d = D::new(name, Class::Vector);
        wr!(d, "v{}.{}, v{}.{}", rd, t, rn, t);
        d
    };
    match (u, opcode) {
        (false, 0) | (true, 0) | (false, 1) | (false, 4) | (true, 4) | (false, 7) | (true, 7) | (false, 11) | (true, 11) | (false, 3) | (true, 3) => {
            let name = match (u, opcode) {
                (false, 0) => "rev64",
                (true, 0) => "rev32",
                (false, 1) => "rev16",
                (false, 4) => "cls",
                (true, 4) => "clz",
                (false, 7) => "sqabs",
                (true, 7) => "sqneg",
                (false, 11) => "abs",
                (true, 11) => "neg",
                (false, 3) => "suqadd",
                _ => "usqadd",
            };
            let ok = match (u, opcode) {
                (false, 0) | (false, 4) | (true, 4) => size < 3,
                (true, 0) => size < 2,
                (false, 1) => size == 0,
                _ => true,
            };
            if !ok {
                return None;
            }
            return Some(same(name, arr2(size, q)?));
        }
        (false, 2) | (true, 2) | (false, 6) | (true, 6) => {
            if size == 3 {
                return None;
            }
            let name = match (u, opcode) {
                (false, 2) => "saddlp",
                (true, 2) => "uaddlp",
                (false, _) => "sadalp",
                _ => "uadalp",
            };
            d = D::new(name, Class::Vector);
            wr!(d, "v{}.{}, v{}.{}", rd, wide_arr(size, q)?, rn, arr(size, q)?);
            return Some(d);
        }
        (false, 5) => {
            if size != 0 {
                return None;
            }
            return Some(same("cnt", arr(0, q)?));
        }
        (true, 5) => {
            return match size {
                0 => Some(same("mvn", arr(0, q)?)),
                1 => Some(same("rbit", arr(0, q)?)),
                _ => None,
            };
        }
        (false, 8) | (false, 9) | (false, 10) | (true, 8) | (true, 9) => {
            let name = match (u, opcode) {
                (false, 8) => "cmgt",
                (false, 9) => "cmeq",
                (false, 10) => "cmlt",
                (true, 8) => "cmge",
                _ => "cmle",
            };
            let t = arr2(size, q)?;
            d = D::new(name, Class::Vector);
            wr!(d, "v{}.{}, v{}.{}, #0", rd, t, rn, t);
            return Some(d);
        }
        (_, 0x12) | (false, 0x14) | (true, 0x14) => {
            if size == 3 {
                return None;
            }
            let name = match (u, opcode) {
                (false, 0x12) => "xtn",
                (true, 0x12) => "sqxtun",
                (false, _) => "sqxtn",
                _ => "uqxtn",
            };
            d = D::new(&format!("{}{}", name, sfx), Class::Vector);
            wr!(d, "v{}.{}, v{}.{}", rd, arr(size, q)?, rn, arr_wide(size)?);
            return Some(d);
        }
        (true, 0x13) => {
            if size == 3 {
                return None;
            }
            d = D::new(&format!("shll{}", sfx), Class::Vector);
            wr!(d, "v{}.{}, v{}.{}, #{}", rd, arr_wide(size)?, rn, arr(size, q)?, 8 << size);
            return Some(d);
        }
        _ => {}
    }
    // floating point
    let sz = size & 1;
    let hi = size >> 1;
    let t = farr(sz, q);
    match (u, opcode) {
        (false, 0x16) | (false, 0x17) => {
            if hi != 0 {
                return None;
            }
            if opcode == 0x16 {
                d = D::new(&format!("fcvtn{}", sfx), Class::Vector);
                if sz == 0 {
                    wr!(d, "v{}.{}, v{}.4s", rd, arr(1, q)?, rn);
                } else {
                    wr!(d, "v{}.{}, v{}.2d", rd, arr(2, q)?, rn);
                }
            } else {
                d = D::new(&format!("fcvtl{}", sfx), Class::Vector);
                if sz == 0 {
                    wr!(d, "v{}.4s, v{}.{}", rd, rn, arr(1, q)?);
                } else {
                    wr!(d, "v{}.2d, v{}.{}", rd, rn, arr(2, q)?);
                }
            }
            return Some(d);
        }
        (true, 0x16) => {
            if hi != 0 || sz != 1 {
                return None;
            }
            d = D::new(&format!("fcvtxn{}", sfx), Class::Vector);
            wr!(d, "v{}.{}, v{}.2d", rd, arr(2, q)?, rn);
            return Some(d);
        }
        _ => {}
    }
    let t = t?;
    let name = match (u, opcode, hi) {
        (false, 0x18, 0) => "frintn",
        (false, 0x18, _) => "frintp",
        (false, 0x19, 0) => "frintm",
        (false, 0x19, _) => "frintz",
        (false, 0x1a, 0) => "fcvtns",
        (false, 0x1a, _) => "fcvtps",
        (false, 0x1b, 0) => "fcvtms",
        (false, 0x1b, _) => "fcvtzs",
        (false, 0x1c, 0) => "fcvtas",
        (false, 0x1d, 0) => "scvtf",
        (false, 0x1d, _) => "frecpe",
        (true, 0x18, 0) => "frinta",
        (true, 0x19, 0) => "frintx",
        (true, 0x19, _) => "frinti",
        (true, 0x1a, 0) => "fcvtnu",
        (true, 0x1a, _) => "fcvtpu",
        (true, 0x1b, 0) => "fcvtmu",
        (true, 0x1b, _) => "fcvtzu",
        (true, 0x1c, 0) => "fcvtau",
        (true, 0x1d, 0) => "ucvtf",
        (true, 0x1d, _) => "frsqrte",
        (false, 0x0f, 1) => "fabs",
        (true, 0x0f, 1) => "fneg",
        (true, 0x1f, 1) => "fsqrt",
        (false, 0x1c, _) if sz == 0 => "urecpe",
        (true, 0x1c, _) if sz == 0 => "ursqrte",
        (false, 0x0c, 1) | (false, 0x0d, 1) | (false, 0x0e, 1) | (true, 0x0c, 1) | (true, 0x0d, 1) => {
            let name = match (u, opcode) {
                (false, 0x0c) => "fcmgt",
                (false, 0x0d) => "fcmeq",
                (false, _) => "fcmlt",
                (true, 0x0c) => "fcmge",
                _ => "fcmle",
            };
            d = D::new(name, Class::Vector);
            wr!(d, "v{}.{}, v{}.{}, #0.0", rd, t, rn, t);
            return Some(d);
        }
        _ => return None,
    };
    // urecpe / ursqrte take 32-bit elements only
    if (name == "urecpe" || name == "ursqrte") && sz != 0 {
        return None;
    }
    Some(same(name, t))
}

/// the widened arrangement of a pairwise-long operation: (size, q) of the source -> destination
fn wide_arr(size: u32, q: u32) -> Option<&'static str> {
    match (size, q) {
        (0, 0) => Some("4h"),
        (0, 1) => Some("8h"),
        (1, 0) => Some("2s"),
        (1, 1) => Some("4s"),
        (2, 0) => Some("1d"),
        (2, 1) => Some("2d"),
        _ => None,
    }
}

fn across(w: u32, q: u32, u: bool, size: u32, rd: u32, rn: u32) -> Option<D> {
    let opcode = f(w, 12, 5);
    let hi = size >> 1;
    let mut d;
    match (u, opcode) {
        (false, 3) | (true, 3) => {
            if size == 3 || (size == 2 && q == 0) {
                return None;
            }
            d = D::new(if u { "uaddlv" } else { "saddlv" }, Class::Vector);
            wr!(d, "{}, v{}.{}", Fr(fchar(size + 1), rd), rn, arr(size, q)?);
        }
        (_, 0x0a) | (_, 0x1a) | (false, 0x1b) => {
            if size == 3 || (size == 2 && q == 0) || (opcode == 0x1b && u) {
                return None;
            }
            let name = match (u, opcode) {
                (false, 0x0a) => "smaxv",
                (true, 0x0a) => "umaxv",
                (false, 0x1a) => "sminv",
                (true, 0x1a) => "uminv",
                _ => "addv",
            };
            d = D::new(name, Class::Vector);
            wr!(d, "{}, v{}.{}", Fr(fchar(size), rd), rn, arr(size, q)?);
        }
        (true, 0x0c) | (true, 0x0f) => {
            if size & 1 != 0 || q == 0 {
                return None;
            }
            let name = match (opcode, hi) {
                (0x0c, 0) => "fmaxnmv",
                (0x0c, _) => "fminnmv",
                (_, 0) => "fmaxv",
                _ => "fminv",
            };
            d = D::new(name, Class::Vector);
            wr!(d, "s{}, v{}.4s", rd, rn);
        }
        _ => return None,
    }
    Some(d)
}

fn simd_copy(w: u32, q: u32, op: bool, rd: u32, rn: u32) -> Option<D> {
    let imm5 = f(w, 16, 5);
    let imm4 = f(w, 11, 4);
    if imm5 == 0 {
        return None;
    }
    let sz = imm5.trailing_zeros();
    if sz > 3 {
        return None;
    }
    let idx = imm5 >> (sz + 1);
    let mut d;
    if op {
        // ins (element)
        if q == 0 {
            return None;
        }
        let src = imm4 >> sz;
        d = D::new("mov", Class::Vector);
        wr!(d, "v{}.{}[{}], v{}.{}[{}]", rd, elem(sz), idx, rn, elem(sz), src);
        return Some(d);
    }
    match imm4 {
        0 => {
            let t = arr2(sz, q)?;
            d = D::new("dup", Class::Vector);
            wr!(d, "v{}.{}, v{}.{}[{}]", rd, t, rn, elem(sz), idx);
        }
        1 => {
            let t = arr2(sz, q)?;
            d = D::new("dup", Class::Vector);
            wr!(d, "v{}.{}, {}", rd, t, gp(sz == 3, rn));
        }
        3 => {
            if q == 0 {
                return None;
            }
            d = D::new("mov", Class::Vector);
            wr!(d, "v{}.{}[{}], {}", rd, elem(sz), idx, gp(sz == 3, rn));
        }
        5 => {
            if sz == 3 || (q == 0 && sz == 2) {
                return None;
            }
            d = D::new("smov", Class::Vector);
            wr!(d, "{}, v{}.{}[{}]", gp(q == 1, rd), rn, elem(sz), idx);
        }
        7 => {
            if (sz < 2 && q == 1) || (sz == 2 && q == 1) || (sz == 3 && q == 0) {
                return None;
            }
            d = D::new(if sz >= 2 { "mov" } else { "umov" }, Class::Vector);
            wr!(d, "{}, v{}.{}[{}]", gp(q == 1, rd), rn, elem(sz), idx);
        }
        _ => return None,
    }
    Some(d)
}

/// bits 28:24 = 01111: modified immediate, shift by immediate, indexed element
fn simd_vec_imm(w: u32) -> Option<D> {
    let q = f(w, 30, 1);
    let u = bit(w, 29);
    let rd = f(w, 0, 5);
    let rn = f(w, 5, 5);
    if bit(w, 10) {
        if f(w, 19, 5) == 0 {
            return mod_imm(w, q, u, rd);
        }
        if bit(w, 23) {
            return None;
        }
        return vec_shift(w, q, u, rd, rn);
    }
    vec_indexed(w, q, u, rd, rn)
}

fn mod_imm(w: u32, q: u32, op: bool, rd: u32) -> Option<D> {
    let cmode = f(w, 12, 4);
    let imm8 = f(w, 16, 3) << 5 | f(w, 5, 5);
    let mut d;
    let t32 = if q == 1 { "4s" } else { "2s" };
    let t16 = if q == 1 { "8h" } else { "4h" };
    if cmode & 9 == 0 || cmode & 9 == 1 {
        let sh = ((cmode >> 1) & 3) * 8;
        let orr = cmode & 1 == 1;
        let name = match (orr, op) {
            (false, false) => "movi",
            (false, true) => "mvni",
            (true, false) => "orr",
            (true, true) => "bic",
        };
        d = D::new(name, if orr { Class::Vector } else { Class::MovImm });
        wr!(d, "v{}.{}, #{}", rd, t32, imm8);
        if sh != 0 {
            wr!(d, ", lsl #{}", sh);
        }
    } else if cmode & 0xd == 8 || cmode & 0xd == 9 {
        let sh = ((cmode >> 1) & 1) * 8;
        let orr = cmode & 1 == 1;
        let name = match (orr, op) {
            (false, false) => "movi",
            (false, true) => "mvni",
            (true, false) => "orr",
            (true, true) => "bic",
        };
        d = D::new(name, if orr { Class::Vector } else { Class::MovImm });
        wr!(d, "v{}.{}, #{}", rd, t16, imm8);
        if sh != 0 {
            wr!(d, ", lsl #{}", sh);
        }
    } else if cmode >> 1 == 6 {
        d = D::new(if op { "mvni" } else { "movi" }, Class::MovImm);
        wr!(d, "v{}.{}, #{}, msl #{}", rd, t32, imm8, if cmode & 1 == 1 { 16 } else { 8 });
    } else if cmode == 14 {
        if !op {
            d = D::new("movi", Class::MovImm);
            wr!(d, "v{}.{}, #{}", rd, if q == 1 { "16b" } else { "8b" }, imm8);
        } else {
            let mut v = 0u64;
            for i in 0..8 {
                if imm8 >> i & 1 == 1 {
                    v |= 0xffu64 << (8 * i);
                }
            }
            d = D::new("movi", Class::MovImm);
            if q == 1 {
                wr!(d, "v{}.2d, #0x{:x}", rd, v);
            } else {
                wr!(d, "d{}, #0x{:x}", rd, v);
            }
        }
    } else {
        // cmode 1111
        if !op {
            d = D::new("fmov", Class::MovImm);
            wr!(d, "v{}.{}, #{}", rd, t32, fp_imm_text(imm8, false));
        } else {
            if q == 0 {
                return None;
            }
            d = D::new("fmov", Class::MovImm);
            wr!(d, "v{}.2d, #{}", rd, fp_imm_text(imm8, true));
        }
    }
    Some(d)
}

fn vec_shift(w: u32, q: u32, u: bool, rd: u32, rn: u32) -> Option<D> {
    let immh = f(w, 19, 4);
    if immh == 0 {
        return None;
    }
    let immhb = f(w, 16, 7);
    let idx = 31 - immh.leading_zeros();
    let esize = 8u32 << idx;
    let right = 2 * esize - immhb;
    let left = immhb - esize;
    let opcode = f(w, 11, 5);
    let sfx = if q == 1 { "2" } else { "" };
    let mut d;
    match (u, opcode) {
        (_, 0x00) | (_, 0x02) | (_, 0x04) | (_, 0x06) | (true, 0x08) => {
            let t = arr2(idx, q)?;
            let name = match (u, opcode) {
                (false, 0) => "sshr",
                (true, 0) => "ushr",
                (false, 2) => "ssra",
                (true, 2) => "usra",
                (false, 4) => "srshr",
                (true, 4) => "urshr",
                (false, 6) => "srsra",
                (true, 6) => "ursra",
                _ => "sri",
            };
            d = D::new(name, Class::Vector);
            wr!(d, "v{}.{}, v{}.{}, #{}", rd, t, rn, t, right);
        }
        (_, 0x0a) | (true, 0x0c) | (_, 0x0e) => {
            let t = arr2(idx, q)?;
            let name = match (u, opcode) {
                (false, 0x0a) => "shl",
                (true, 0x0a) => "sli",
                (true, 0x0c) => "sqshlu",
                (false, _) => "sqshl",
                _ => "uqshl",
            };
            d = D::new(name, Class::Vector);
            wr!(d, "v{}.{}, v{}.{}, #{}", rd, t, rn, t, left);
        }
        (_, 0x10) | (_, 0x11) | (_, 0x12) | (_, 0x13) => {
            if idx == 3 {
                return None;
            }
            let name = match (u, opcode) {
                (false, 0x10) => "shrn",
                (false, 0x11) => "rshrn",
                (false, 0x12) => "sqshrn",
                (false, 0x13) => "sqrshrn",
                (true, 0x10) => "sqshrun",
                (true, 0x11) => "sqrshrun",
                (true, 0x12) => "uqshrn",
                _ => "uqrshrn",
            };
            d = D::new(&format!("{}{}", name, sfx), Class::Vector);
            wr!(d, "v{}.{}, v{}.{}, #{}", rd, arr(idx, q)?, rn, arr_wide(idx)?, right);
        }
        (_, 0x14) => {
            if idx == 3 {
                return None;
            }
            let tb = arr(idx, q)?;
            let ta = arr_wide(idx)?;
            if left == 0 {
                d = D::new(&format!("{}{}", if u { "uxtl" } else { "sxtl" }, sfx), Class::Vector);
                wr!(d, "v{}.{}, v{}.{}", rd, ta, rn, tb);
            } else {
                d = D::new(&format!("{}{}", if u { "ushll" } else { "sshll" }, sfx), Class::Vector);
                wr!(d, "v{}.{}, v{}.{}, #{}", rd, ta, rn, tb, left);
            }
        }
        (_, 0x1c) | (_, 0x1f) => {
            if idx < 2 {
                return None;
            }
            let t = farr(idx - 2, q)?;
            let name = match (u, opcode) {
                (false, 0x1c) => "scvtf",
                (true, 0x1c) => "ucvtf",
                (false, _) => "fcvtzs",
                _ => "fcvtzu",
            };
            d = D::new(name, Class::Vector);
            wr!(d, "v{}.{}, v{}.{}, #{}", rd, t, rn, t, right);
        }
        _ => return None,
    }
    Some(d)
}

fn vec_indexed(w: u32, q: u32, u: bool, rd: u32, rn: u32) -> Option<D> {
    let size = f(w, 22, 2);
    let l = f(w, 21, 1);
    let m = f(w, 20, 1);
    let h = f(w, 11, 1);
    let opcode = f(w, 12, 4);
    let (rmv, idx) = elem_ref(size, h, l, m, f(w, 16, 4))?;
    let sfx = if q == 1 { "2" } else { "" };
    let mut d;
    match (u, opcode) {
        (false, 1) | (false, 5) | (false, 9) | (true, 9) if size >= 2 => {
            let t = farr(size & 1, q)?;
            let name = match (u, opcode) {
                (false, 1) => "fmla",
                (false, 5) => "fmls",
                (false, _) => "fmul",
                _ => "fmulx",
            };
            d = D::new(name, Class::Vector);
            wr!(d, "v{}.{}, v{}.{}, v{}.{}[{}]", rd, t, rn, t, rmv, elem(size), idx);
        }
        (false, 8) | (true, 0) | (true, 4) | (false, 12) | (false, 13) if size == 1 || size == 2 => {
            let t = arr(size, q)?;
            let name = match (u, opcode) {
                (false, 8) => "mul",
                (true, 0) => "mla",
                (true, 4) => "mls",
                (false, 12) => "sqdmulh",
                _ => "sqrdmulh",
            };
            d = D::new(name, Class::Vector);
            wr!(d, "v{}.{}, v{}.{}, v{}.{}[{}]", rd, t, rn, t, rmv, elem(size), idx);
        }
        (_, 2) | (_, 6) | (_, 10) | (false, 3) | (false, 7) | (false, 11) if size == 1 || size == 2 => {
            let name = match (u, opcode) {
                (false, 2) => "smlal",
                (true, 2) => "umlal",
                (false, 6) => "smlsl",
                (true, 6) => "umlsl",
                (false, 10) => "smull",
                (true, 10) => "umull",
                (false, 3) => "sqdmlal",
                (false, 7) => "sqdmlsl",
                (false, 11) => "sqdmull",
                _ => return None,
            };
            d = D::new(&format!("{}{}", name, sfx), Class::Vector);
            wr!(d, "v{}.{}, v{}.{}, v{}.{}[{}]", rd, arr_wide(size)?, rn, arr(size, q)?, rmv, elem(size), idx);
        }
        _ => return None,
    }
    Some(d)
}
