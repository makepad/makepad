//! x64 instruction decoder for [`super::dis`] (makepad inspect `asm`).
//!
//! Output is this module's assembler syntax (Intel, `qword ptr [rax+rcx*8+0x10]`), so a decoded
//! instruction assembles back ([`super::x64`]). The decoder is a match over the opcode maps
//! (one-byte, `0F`, `0F 38`, `0F 3A`) plus one table for the SSE / AVX families, which is shared
//! by the legacy, VEX and EVEX encodings: a vector instruction is one table entry (name, operand
//! form, register widths, memory size) and the encoding only decides the prefix bits around it.
//!
//! It never misdecodes on purpose: an opcode it does not know becomes one `Class::Unknown`
//! instruction covering the whole instruction when its length is still certain (the operand
//! encoding is uniform within a map), or one byte when it is not, and decoding goes on.

use super::dis::{Class, Insn};

/// Decodes `code` mapped at `addr`; see [`super::dis::decode`].
pub fn decode(code: &[u8], addr: u64) -> Vec<Insn> {
    let mut out = Vec::with_capacity(code.len() / 3 + 1);
    let mut p = 0usize;
    while p < code.len() {
        let mut d = Dec::new(code, p, addr);
        let ins = match d.insn() {
            Ok(i) => i,
            Err(Bad::Trunc) => unknown(code, p, (code.len() - p).min(15)),
            Err(Bad::Inval) => unknown(code, p, 1),
            Err(Bad::Unk) => {
                let n = d.p - p;
                unknown(code, p, if (1..=15).contains(&n) { n } else { 1 })
            }
        };
        p += ins.len as usize;
        out.push(ins);
    }
    out
}

fn unknown(code: &[u8], p: usize, n: usize) -> Insn {
    let mut ops = String::new();
    for (i, b) in code[p..p + n].iter().enumerate() {
        if i > 0 {
            ops.push_str(", ");
        }
        ops.push_str(&format!("{:#04x}", b));
    }
    Insn { off: p as u32, len: n as u8, mnem: ".byte".into(), ops, class: Class::Unknown, target: None, pc_ref: None, mem: false, stack: false }
}

type R<T> = Result<T, Bad>;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Bad {
    /// the code ends inside the instruction
    Trunc,
    /// not an instruction in 64-bit mode: one byte, resync at the next
    Inval,
    /// a real instruction this decoder does not spell; `Dec::p` is its end
    Unk,
}

const R64: [&str; 16] = ["rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi", "r8", "r9", "r10", "r11", "r12", "r13", "r14", "r15"];
const R32: [&str; 16] = ["eax", "ecx", "edx", "ebx", "esp", "ebp", "esi", "edi", "r8d", "r9d", "r10d", "r11d", "r12d", "r13d", "r14d", "r15d"];
const R16: [&str; 16] = ["ax", "cx", "dx", "bx", "sp", "bp", "si", "di", "r8w", "r9w", "r10w", "r11w", "r12w", "r13w", "r14w", "r15w"];
const R8: [&str; 16] = ["al", "cl", "dl", "bl", "spl", "bpl", "sil", "dil", "r8b", "r9b", "r10b", "r11b", "r12b", "r13b", "r14b", "r15b"];
const R8H: [&str; 4] = ["ah", "ch", "dh", "bh"];
const CC: [&str; 16] = ["o", "no", "b", "ae", "e", "ne", "be", "a", "s", "ns", "p", "np", "l", "ge", "le", "g"];
const ALU: [&str; 8] = ["add", "or", "adc", "sbb", "and", "sub", "xor", "cmp"];
const SEG: [&str; 8] = ["es", "cs", "ss", "ds", "fs", "gs", "?", "?"];

#[derive(Clone, Copy)]
struct Mem {
    base: Option<u8>,
    index: Option<u8>,
    scale: u8,
    disp: i32,
    has_disp: bool,
    rip: bool,
}

#[derive(Clone, Copy)]
enum Rm {
    R(u8),
    M(Mem),
}

fn kw(bits: u16) -> &'static str {
    match bits {
        8 => "byte",
        16 => "word",
        32 => "dword",
        48 => "fword",
        64 => "qword",
        80 => "tbyte",
        128 => "xmmword",
        256 => "ymmword",
        512 => "zmmword",
        _ => "",
    }
}

fn w_simm(o: &mut String, v: i64) {
    if v < 0 {
        o.push_str("-0x");
        push_hex(o, v.unsigned_abs());
    } else {
        o.push_str("0x");
        push_hex(o, v as u64);
    }
}

fn w_uimm(o: &mut String, v: u64) {
    o.push_str("0x");
    push_hex(o, v);
}

fn push_hex(o: &mut String, mut v: u64) {
    if v == 0 {
        o.push('0');
        return;
    }
    let mut buf = [0u8; 16];
    let mut i = 16;
    while v != 0 {
        i -= 1;
        buf[i] = b"0123456789abcdef"[(v & 15) as usize];
        v >>= 4;
    }
    o.push_str(std::str::from_utf8(&buf[i..]).unwrap());
}

fn push_dec(o: &mut String, v: u32) {
    o.push_str(&v.to_string());
}

/// Register width classes of vector operands: the instruction's vector length, always xmm, or
/// half the vector length (the narrow side of a conversion).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Rk {
    V,
    X,
    H,
    /// an opmask register (compare results)
    K,
}

/// How much memory an operand reads or writes, relative to the vector length.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Ms {
    V,
    H,
    Q,
    E,
    B(u16),
    /// vmovddup: 64 bits at 128, the full length above
    D,
}

/// Operand forms of a vector table entry (Intel order; `v` = VEX.vvvv source, only printed in
/// the VEX / EVEX encodings).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Fm {
    Rvm,
    Rm,
    Mr,
    RvmI,
    RmI,
    MrI,
}

struct Ent {
    n: String,
    f: Fm,
    dk: Rk,
    sk: Rk,
    ms: Ms,
    cls: Class,
    /// a data move: a memory operand makes it a load / store, register to register a MovReg
    mv: bool,
    /// no legacy encoding exists
    vo: bool,
    memonly: bool,
    /// no 66 prefix: the operands are mmx registers in the legacy encoding
    mmx: bool,
}

impl Ent {
    fn new(n: &str, f: Fm) -> Ent {
        Ent { n: n.to_string(), f, dk: Rk::V, sk: Rk::V, ms: Ms::V, cls: Class::Vector, mv: false, vo: false, memonly: false, mmx: false }
    }
    fn x(mut self) -> Ent {
        self.dk = Rk::X;
        self.sk = Rk::X;
        self
    }
    fn dst(mut self, k: Rk) -> Ent {
        self.dk = k;
        self
    }
    fn src(mut self, k: Rk) -> Ent {
        self.sk = k;
        self
    }
    fn ms(mut self, m: Ms) -> Ent {
        self.ms = m;
        self
    }
    fn fp(mut self) -> Ent {
        self.cls = Class::Fp;
        self
    }
    fn mv(mut self) -> Ent {
        self.mv = true;
        self
    }
    fn vo(mut self) -> Ent {
        self.vo = true;
        self
    }
    fn mem(mut self) -> Ent {
        self.memonly = true;
        self
    }
    fn k(mut self) -> Ent {
        self.dk = Rk::K;
        self
    }
    fn mmx(mut self) -> Ent {
        self.mmx = true;
        self
    }
    /// scalar single / double: xmm registers, 32 / 64 bit memory, Fp class
    fn sc(self, double: bool) -> Ent {
        self.x().ms(Ms::B(if double { 64 } else { 32 })).fp()
    }
}

/// The ps / pd / ss / sd member (by mandatory prefix 0 / 66 / F3 / F2) of an arithmetic family.
fn fam(stem: &str, pp: u8, f: Fm) -> Ent {
    let sfx = ["ps", "pd", "ss", "sd"][pp as usize & 3];
    let e = Ent::new(&format!("{}{}", stem, sfx), f);
    if pp >= 2 {
        e.sc(pp == 3)
    } else {
        e
    }
}

fn int_name(op: u8) -> Option<&'static str> {
    Some(match op {
        0xD1 => "psrlw",
        0xD2 => "psrld",
        0xD3 => "psrlq",
        0xD4 => "paddq",
        0xD5 => "pmullw",
        0xD8 => "psubusb",
        0xD9 => "psubusw",
        0xDA => "pminub",
        0xDB => "pand",
        0xDC => "paddusb",
        0xDD => "paddusw",
        0xDE => "pmaxub",
        0xDF => "pandn",
        0xE0 => "pavgb",
        0xE1 => "psraw",
        0xE2 => "psrad",
        0xE3 => "pavgw",
        0xE4 => "pmulhuw",
        0xE5 => "pmulhw",
        0xE8 => "psubsb",
        0xE9 => "psubsw",
        0xEA => "pminsw",
        0xEB => "por",
        0xEC => "paddsb",
        0xED => "paddsw",
        0xEE => "pmaxsw",
        0xEF => "pxor",
        0xF1 => "psllw",
        0xF2 => "pslld",
        0xF3 => "psllq",
        0xF4 => "pmuludq",
        0xF5 => "pmaddwd",
        0xF6 => "psadbw",
        0xF8 => "psubb",
        0xF9 => "psubw",
        0xFA => "psubd",
        0xFB => "psubq",
        0xFC => "paddb",
        0xFD => "paddw",
        0xFE => "paddd",
        _ => return None,
    })
}

const LOW_INT: [&str; 14] = ["punpcklbw", "punpcklwd", "punpckldq", "packsswb", "pcmpgtb", "pcmpgtw", "pcmpgtd", "packuswb", "punpckhbw", "punpckhwd", "punpckhdq", "packssdw", "punpcklqdq", "punpckhqdq"];

/// The table for the vector families: `map` 1 = 0F, 2 = 0F 38, 3 = 0F 3A; `pp` the mandatory
/// prefix (0 none, 1 66, 2 F3, 3 F2). Instructions with a special operand shape (gpr operands,
/// immediates in the modrm.reg field, 3-operand moves) are decoded before the table is asked.
fn tab(map: u8, pp: u8, op: u8, w: bool) -> Option<Ent> {
    use Fm::*;
    let p01 = pp < 2;
    let e = Ent::new;
    Some(match map {
        1 => match op {
            0x10 | 0x11 if p01 => e(["movups", "movupd"][pp as usize], if op == 0x10 { Rm } else { Mr }).mv(),
            0x12 if pp == 2 => e("movsldup", Rm).mv(),
            0x12 if pp == 3 => e("movddup", Rm).ms(Ms::D).mv(),
            0x14 | 0x15 if p01 => e(&format!("unpck{}{}", if op == 0x14 { 'l' } else { 'h' }, ["ps", "pd"][pp as usize]), Rvm),
            0x16 if pp == 2 => e("movshdup", Rm).mv(),
            0x28 | 0x29 if p01 => e(["movaps", "movapd"][pp as usize], if op == 0x28 { Rm } else { Mr }).mv(),
            0x2B if p01 => e(["movntps", "movntpd"][pp as usize], Mr).mv().mem(),
            0x2E | 0x2F if p01 => e(&format!("{}comis{}", if op == 0x2E { "u" } else { "" }, ["s", "d"][pp as usize]), Rm).sc(pp == 1),
            0x51 => fam("sqrt", pp, if pp < 2 { Rm } else { Rvm }),
            0x52 | 0x53 if pp == 0 || pp == 2 => fam(if op == 0x52 { "rsqrt" } else { "rcp" }, pp, if pp == 0 { Rm } else { Rvm }),
            0x54..=0x57 if p01 => fam(["and", "andn", "or", "xor"][(op - 0x54) as usize], pp, Rvm),
            0x58 => fam("add", pp, Rvm),
            0x59 => fam("mul", pp, Rvm),
            0x5C => fam("sub", pp, Rvm),
            0x5D => fam("min", pp, Rvm),
            0x5E => fam("div", pp, Rvm),
            0x5F => fam("max", pp, Rvm),
            0x5A => match pp {
                0 => e("cvtps2pd", Rm).src(Rk::H).ms(Ms::H),
                1 => e("cvtpd2ps", Rm).dst(Rk::H),
                2 => e("cvtss2sd", Rvm).sc(false),
                _ => e("cvtsd2ss", Rvm).sc(true),
            },
            0x5B => e(["cvtdq2ps", "cvtps2dq", "cvttps2dq"][(pp as usize).min(2)], Rm).tap_if(pp == 3),
            0x60..=0x6D if pp == 1 || (pp == 0 && op < 0x6C) => e(LOW_INT[(op - 0x60) as usize], Rvm).mmx_if(pp == 0),
            0x6F => match pp {
                0 => e("movq", Rm).mv().mmx(),
                1 => e("movdqa", Rm).mv(),
                2 => e("movdqu", Rm).mv(),
                _ => return None,
            },
            0x70 => match pp {
                0 => e("pshufw", RmI).mmx(),
                1 => e("pshufd", RmI),
                2 => e("pshufhw", RmI),
                _ => e("pshuflw", RmI),
            },
            0x74..=0x76 if pp < 2 => e(["pcmpeqb", "pcmpeqw", "pcmpeqd"][(op - 0x74) as usize], Rvm).mmx_if(pp == 0),
            0x7C | 0x7D if pp == 1 || pp == 3 => e(&format!("{}{}", if op == 0x7C { "hadd" } else { "hsub" }, if pp == 1 { "pd" } else { "ps" }), Rvm),
            0x7F => match pp {
                0 => e("movq", Mr).mv().mmx(),
                1 => e("movdqa", Mr).mv(),
                2 => e("movdqu", Mr).mv(),
                _ => return None,
            },
            0xC6 if p01 => e(["shufps", "shufpd"][pp as usize], RvmI),
            0xD0 if pp == 1 || pp == 3 => e(if pp == 1 { "addsubpd" } else { "addsubps" }, Rvm),
            0xD1..=0xD3 | 0xE1 | 0xE2 | 0xF1..=0xF3 if pp < 2 => e(int_name(op)?, Rvm).src(Rk::X).ms(Ms::B(128)).mmx_if(pp == 0),
            0xE6 => match pp {
                1 => e("cvttpd2dq", Rm).dst(Rk::H),
                2 => e("cvtdq2pd", Rm).src(Rk::H).ms(Ms::H),
                3 => e("cvtpd2dq", Rm).dst(Rk::H),
                _ => return None,
            },
            0xE7 if pp < 2 => e(["movntq", "movntdq"][pp as usize], Mr).mv().mem().mmx_if(pp == 0),
            0xF0 if pp == 3 => e("lddqu", Rm).mv().mem(),
            0xD4 | 0xD5 | 0xD8..=0xDF | 0xE0 | 0xE3..=0xE5 | 0xE8..=0xEF | 0xF4..=0xF6 | 0xF8..=0xFE if pp < 2 => e(int_name(op)?, Rvm).mmx_if(pp == 0),
            _ => return None,
        },
        2 => match (pp, op) {
            (0 | 1, 0x00..=0x0B) => e(["pshufb", "phaddw", "phaddd", "phaddsw", "pmaddubsw", "phsubw", "phsubd", "phsubsw", "psignb", "psignw", "psignd", "pmulhrsw"][op as usize], Rvm).mmx_if(pp == 0),
            (0 | 1, 0x1C..=0x1E) => e(["pabsb", "pabsw", "pabsd"][(op - 0x1C) as usize], Rm).mmx_if(pp == 0),
            (1, 0x0C) => e("permilps", Rvm).vo(),
            (1, 0x0D) => e("permilpd", Rvm).vo(),
            (1, 0x0E) => e("testps", Rm).vo(),
            (1, 0x0F) => e("testpd", Rm).vo(),
            (1, 0x13) => e("cvtph2ps", Rm).src(Rk::H).ms(Ms::H).vo(),
            (1, 0x16) => e("permps", Rvm).vo(),
            (1, 0x17) => e("ptest", Rm),
            (1, 0x18) => e("broadcastss", Rm).src(Rk::X).ms(Ms::B(32)).mv().vo(),
            (1, 0x19) => e("broadcastsd", Rm).src(Rk::X).ms(Ms::B(64)).mv().vo(),
            (1, 0x1A) => e("broadcastf128", Rm).src(Rk::X).ms(Ms::B(128)).mv().mem().vo(),
            (1, 0x20..=0x25) => {
                let n = ["pmovsxbw", "pmovsxbd", "pmovsxbq", "pmovsxwd", "pmovsxwq", "pmovsxdq"][(op - 0x20) as usize];
                e(n, Rm).src(Rk::X).ms([Ms::H, Ms::Q, Ms::E, Ms::H, Ms::Q, Ms::H][(op - 0x20) as usize])
            }
            (1, 0x30..=0x35) => {
                let n = ["pmovzxbw", "pmovzxbd", "pmovzxbq", "pmovzxwd", "pmovzxwq", "pmovzxdq"][(op - 0x30) as usize];
                e(n, Rm).src(Rk::X).ms([Ms::H, Ms::Q, Ms::E, Ms::H, Ms::Q, Ms::H][(op - 0x30) as usize])
            }
            (1, 0x28) => e("pmuldq", Rvm),
            (1, 0x29) => e("pcmpeqq", Rvm),
            (1, 0x2A) => e("movntdqa", Rm).mv().mem(),
            (1, 0x2B) => e("packusdw", Rvm),
            (1, 0x36) => e("permd", Rvm).vo(),
            (1, 0x37) => e("pcmpgtq", Rvm),
            (1, 0x38..=0x3F) => e(["pminsb", "pminsd", "pminuw", "pminud", "pmaxsb", "pmaxsd", "pmaxuw", "pmaxud"][(op - 0x38) as usize], Rvm),
            (1, 0x40) => e("pmulld", Rvm),
            (1, 0x41) => e("phminposuw", Rm).x(),
            (1, 0x45) => e(if w { "psrlvq" } else { "psrlvd" }, Rvm).vo(),
            (1, 0x46) if !w => e("psravd", Rvm).vo(),
            (1, 0x47) => e(if w { "psllvq" } else { "psllvd" }, Rvm).vo(),
            (1, 0x58) => e("pbroadcastd", Rm).src(Rk::X).ms(Ms::B(32)).mv().vo(),
            (1, 0x59) => e("pbroadcastq", Rm).src(Rk::X).ms(Ms::B(64)).mv().vo(),
            (1, 0x5A) => e("broadcasti128", Rm).src(Rk::X).ms(Ms::B(128)).mv().mem().vo(),
            (1, 0x78) => e("pbroadcastb", Rm).src(Rk::X).ms(Ms::B(8)).mv().vo(),
            (1, 0x79) => e("pbroadcastw", Rm).src(Rk::X).ms(Ms::B(16)).mv().vo(),
            (1, 0xDB) => e("aesimc", Rm).x(),
            (1, 0xDC) => e("aesenc", Rvm),
            (1, 0xDD) => e("aesenclast", Rvm),
            (1, 0xDE) => e("aesdec", Rvm),
            (1, 0xDF) => e("aesdeclast", Rvm),
            (1, 0x96..=0x9F | 0xA6..=0xAF | 0xB6..=0xBF) => {
                let kind = ["fmaddsub", "fmsubadd", "fmadd", "fmadd", "fmsub", "fmsub", "fnmadd", "fnmadd", "fnmsub", "fnmsub"][(op & 15) as usize - 6];
                let order = match op >> 4 {
                    9 => "132",
                    0xA => "213",
                    _ => "231",
                };
                let scalar = op & 15 >= 8 && op & 1 == 1;
                if scalar {
                    e(&format!("{}{}{}", kind, order, if w { "sd" } else { "ss" }), Rvm).vo().sc(w)
                } else {
                    e(&format!("{}{}{}", kind, order, if w { "pd" } else { "ps" }), Rvm).vo()
                }
            }
            _ => return None,
        },
        3 => match (pp, op) {
            (1, 0x00) if w => e("permq", RmI).vo(),
            (1, 0x01) if w => e("permpd", RmI).vo(),
            (1, 0x02) if !w => e("pblendd", RvmI).vo(),
            (1, 0x04) => e("permilps", RmI).vo(),
            (1, 0x05) => e("permilpd", RmI).vo(),
            (1, 0x06) => e("perm2f128", RvmI).vo(),
            (1, 0x08) => e("roundps", RmI),
            (1, 0x09) => e("roundpd", RmI),
            (1, 0x0A) => e("roundss", RvmI).sc(false),
            (1, 0x0B) => e("roundsd", RvmI).sc(true),
            (1, 0x0C) => e("blendps", RvmI),
            (1, 0x0D) => e("blendpd", RvmI),
            (1, 0x0E) => e("pblendw", RvmI),
            (0 | 1, 0x0F) => e("palignr", RvmI).mmx_if(pp == 0),
            (1, 0x18) => e("insertf128", RvmI).src(Rk::X).ms(Ms::B(128)).vo(),
            (1, 0x19) => e("extractf128", MrI).dst(Rk::X).ms(Ms::B(128)).vo(),
            (1, 0x1D) => e("cvtps2ph", MrI).dst(Rk::H).ms(Ms::H).vo(),
            (1, 0x21) => e("insertps", RvmI).x().ms(Ms::B(32)),
            (1, 0x38) => e("inserti128", RvmI).src(Rk::X).ms(Ms::B(128)).vo(),
            (1, 0x39) => e("extracti128", MrI).dst(Rk::X).ms(Ms::B(128)).vo(),
            (1, 0x40) => e("dpps", RvmI),
            (1, 0x41) => e("dppd", RvmI),
            (1, 0x42) => e("mpsadbw", RvmI),
            (1, 0x44) => e("pclmulqdq", RvmI),
            (1, 0x46) => e("perm2i128", RvmI).vo(),
            (1, 0x60) => e("pcmpestrm", RmI).x(),
            (1, 0x61) => e("pcmpestri", RmI).x(),
            (1, 0x62) => e("pcmpistrm", RmI).x(),
            (1, 0x63) => e("pcmpistri", RmI).x(),
            (1, 0xDF) => e("aeskeygenassist", RmI).x(),
            _ => return None,
        },
        _ => return None,
    })
}

impl Ent {
    fn mmx_if(self, c: bool) -> Ent {
        if c {
            self.mmx()
        } else {
            self
        }
    }
    /// pp == 3 has no member in the 5B family
    fn tap_if(self, none: bool) -> Ent {
        if none {
            Ent { n: String::new(), ..self }
        } else {
            self
        }
    }
}

struct Dec<'a> {
    c: &'a [u8],
    p: usize,
    s: usize,
    addr: u64,
    // legacy prefixes
    p66: bool,
    p67: bool,
    lock: bool,
    /// last of F2 / F3, or 0
    rep: u8,
    seg: u8,
    /// a 3E prefix (notrack on indirect branches)
    ds: bool,
    has_rex: bool,
    // REX / VEX / EVEX register extension bits
    w: bool,
    rr: u8,
    rx: u8,
    rb: u8,
    /// EVEX.R' (register bit 4 of the modrm.reg operand)
    rr2: u8,
    /// EVEX.V' (register bit 4 of vvvv)
    vv2: u8,
    /// EVEX mask register, zeroing and broadcast bits
    aaa: u8,
    zero: bool,
    bcast: bool,
    /// EVEX.L'L as sent (the rounding mode of a register-register broadcast bit)
    ll: u8,
    /// a memory operand's `{1toN}` broadcast count (0: none)
    bcn: u16,
    // VEX / EVEX
    vex: bool,
    evex: bool,
    vl: u8,
    vvvv: u8,
    // modrm
    have_modrm: bool,
    mode: u8,
    reg: u8,
    rm: Rm,
    nomem: bool,
    /// an implicit memory operand (string instructions)
    imem: bool,
    tgt: Option<u64>,
    pre: &'static str,
}

impl<'a> Dec<'a> {
    fn new(c: &'a [u8], p: usize, addr: u64) -> Dec<'a> {
        Dec {
            c,
            p,
            s: p,
            addr,
            p66: false,
            p67: false,
            lock: false,
            rep: 0,
            seg: 0,
            ds: false,
            has_rex: false,
            w: false,
            rr: 0,
            rx: 0,
            rb: 0,
            rr2: 0,
            vv2: 0,
            aaa: 0,
            zero: false,
            bcast: false,
            ll: 0,
            bcn: 0,
            vex: false,
            evex: false,
            vl: 0,
            vvvv: 0,
            have_modrm: false,
            mode: 0,
            reg: 0,
            rm: Rm::R(0),
            nomem: false,
            imem: false,
            tgt: None,
            pre: "",
        }
    }

    fn u8(&mut self) -> R<u8> {
        match self.c.get(self.p) {
            Some(b) => {
                self.p += 1;
                Ok(*b)
            }
            None => Err(Bad::Trunc),
        }
    }
    fn le(&mut self, n: usize) -> R<u64> {
        if self.p + n > self.c.len() {
            return Err(Bad::Trunc);
        }
        let mut v = 0u64;
        for i in 0..n {
            v |= (self.c[self.p + i] as u64) << (8 * i);
        }
        self.p += n;
        Ok(v)
    }
    fn i8(&mut self) -> R<i64> {
        Ok(self.u8()? as i8 as i64)
    }
    fn i16(&mut self) -> R<i64> {
        Ok(self.le(2)? as u16 as i16 as i64)
    }
    fn i32(&mut self) -> R<i64> {
        Ok(self.le(4)? as u32 as i32 as i64)
    }

    /// Operand size of the integer instruction families.
    fn osz(&self) -> u16 {
        if self.w {
            64
        } else if self.p66 {
            16
        } else {
            32
        }
    }

    /// The length is known and the instruction is not spelled: skip the modrm and `imm` bytes.
    fn unk<T>(&mut self, imm: usize) -> R<T> {
        self.modrm()?;
        if self.p + imm > self.c.len() {
            return Err(Bad::Trunc);
        }
        self.p += imm;
        Err(Bad::Unk)
    }

    fn modrm(&mut self) -> R<()> {
        if self.have_modrm {
            return Ok(());
        }
        let b = self.u8()?;
        self.have_modrm = true;
        self.mode = b >> 6;
        self.reg = (b >> 3) & 7;
        let rm = b & 7;
        if self.mode == 3 {
            // EVEX.X extends a register operand to 32 vector registers
            self.rm = Rm::R(rm | self.rb << 3 | if self.evex { self.rx << 4 } else { 0 });
            return Ok(());
        }
        let mut m = Mem { base: None, index: None, scale: 1, disp: 0, has_disp: false, rip: false };
        if rm == 4 {
            let sib = self.u8()?;
            m.scale = 1 << (sib >> 6);
            let idx = (sib >> 3) & 7 | self.rx << 3;
            if idx != 4 || self.vex_vsib() {
                m.index = Some(idx);
            }
            let base = sib & 7;
            if base == 5 && self.mode == 0 {
                m.disp = self.i32()? as i32;
                m.has_disp = true;
            } else {
                m.base = Some(base | self.rb << 3);
            }
        } else if rm == 5 && self.mode == 0 {
            m.rip = true;
            m.disp = self.i32()? as i32;
            m.has_disp = true;
        } else {
            m.base = Some(rm | self.rb << 3);
        }
        match self.mode {
            1 => {
                m.disp = self.i8()? as i32;
                m.has_disp = true;
            }
            2 => {
                m.disp = self.i32()? as i32;
                m.has_disp = true;
            }
            _ => {}
        }
        self.rm = Rm::M(m);
        Ok(())
    }

    /// A VSIB index may be register 4 (gathers); decoding them is left to the Unknown path, but
    /// the length must treat the index as present either way (it does not change the length).
    fn vex_vsib(&self) -> bool {
        false
    }

    fn is_mem(&self) -> bool {
        self.have_modrm && matches!(self.rm, Rm::M(_)) && !self.nomem || self.imem
    }
    fn is_reg(&self) -> bool {
        self.have_modrm && matches!(self.rm, Rm::R(_))
    }
    fn rm_n(&self) -> u8 {
        match self.rm {
            Rm::R(n) => n,
            _ => 0,
        }
    }
    fn reg_n(&self) -> u8 {
        self.reg | self.rr << 3 | if self.evex { self.rr2 << 4 } else { 0 }
    }
}

// ---------------------------------------------------------------------------- spelling

impl<'a> Dec<'a> {
    fn w_g(&self, o: &mut String, n: u8, bits: u16) {
        let n = n as usize & 15;
        o.push_str(match bits {
            8 => {
                if !self.has_rex && (4..8).contains(&n) {
                    R8H[n - 4]
                } else {
                    R8[n]
                }
            }
            16 => R16[n],
            32 => R32[n],
            _ => R64[n],
        });
    }

    /// Vector register `n`: kind 0 xmm, 1 ymm, 2 zmm, 3 mmx.
    fn w_v(&self, o: &mut String, n: u8, k: u8) {
        o.push_str(["xmm", "ymm", "zmm", "mm", "k"][k as usize]);
        push_dec(o, if k >= 3 { n as u32 & 7 } else { n as u32 });
    }

    fn w_reg(&self, o: &mut String, bits: u16) {
        self.w_g(o, self.reg | self.rr << 3, bits);
    }

    fn w_rm(&self, o: &mut String, bits: u16) {
        match self.rm {
            Rm::R(n) => self.w_g(o, n, bits),
            Rm::M(_) => self.w_mem(o, bits, 1),
        }
    }

    /// The memory operand; `bits` = 0 omits the size keyword, `dsc` scales an 8 bit displacement
    /// (EVEX compressed displacement).
    fn w_mem(&self, o: &mut String, bits: u16, dsc: i64) {
        let Rm::M(m) = self.rm else { return };
        if bits != 0 {
            o.push_str(kw(bits));
            o.push_str(" ptr ");
        }
        if self.seg == 0x64 {
            o.push_str("fs:");
        } else if self.seg == 0x65 {
            o.push_str("gs:");
        }
        o.push('[');
        let mut first = true;
        if m.rip {
            o.push_str(if self.p67 { "eip" } else { "rip" });
            first = false;
        }
        if let Some(b) = m.base {
            o.push_str(if self.p67 { R32[b as usize & 15] } else { R64[b as usize & 15] });
            first = false;
        }
        if let Some(i) = m.index {
            if !first {
                o.push('+');
            }
            o.push_str(if self.p67 { R32[i as usize & 15] } else { R64[i as usize & 15] });
            o.push('*');
            push_dec(o, m.scale as u32);
            first = false;
        }
        // EVEX compresses an 8 bit displacement by the size of the access
        let _ = dsc;
        let d = m.disp as i64 * if self.mode == 1 && self.evex { (bits.max(8) / 8) as i64 } else { 1 };
        if first {
            // absolute address
            w_simm(o, d);
        } else if m.has_disp {
            o.push(if d < 0 { '-' } else { '+' });
            o.push_str("0x");
            push_hex(o, d.unsigned_abs());
        }
        o.push(']');
        if self.bcn != 0 {
            o.push_str("{1to");
            push_dec(o, self.bcn as u32);
            o.push('}');
        }
    }

    /// Side of a memory access as a class.
    fn side(&self, wr: bool) -> Class {
        let stack = match self.rm {
            Rm::M(m) => !m.rip && matches!(m.base, Some(4 | 5)),
            _ => false,
        };
        match (stack, wr) {
            (true, true) => Class::StackStore,
            (true, false) => Class::StackLoad,
            (false, true) => Class::Store,
            (false, false) => Class::Load,
        }
    }

    /// Class of an integer instruction: with a memory operand the class is the memory side,
    /// except for the classes that say more than the side (multiply, divide, address, ...).
    fn cls_int(&self, base: Class, wr: bool) -> Class {
        if !self.is_mem() {
            return base;
        }
        if self.lock {
            return Class::Atomic;
        }
        match base {
            Class::Alu | Class::Cmp | Class::MovReg | Class::MovImm | Class::Select => self.side(wr),
            _ => base,
        }
    }

    fn fin_int(&mut self, mnem: &str, ops: String, base: Class, wr: bool) -> R<Insn> {
        let c = self.cls_int(base, wr);
        self.fin(mnem, ops, c)
    }

    fn fin(&mut self, mnem: &str, ops: String, class: Class) -> R<Insn> {
        let len = self.p - self.s;
        if len > 15 {
            return Err(Bad::Inval);
        }
        let m = match (self.have_modrm, self.rm) {
            (true, Rm::M(m)) => Some(m),
            _ => None,
        };
        let mem = (m.is_some() && !self.nomem) || self.imem;
        let stack = mem && m.is_some_and(|m| !m.rip && matches!(m.base, Some(4 | 5)));
        let pc_ref = m.filter(|m| m.rip && !self.p67).map(|m| self.addr.wrapping_add(self.p as u64).wrapping_add(m.disp as i64 as u64));
        let mut mn = String::with_capacity(mnem.len() + 6);
        if self.lock {
            mn.push_str("lock ");
        }
        mn.push_str(self.pre);
        mn.push_str(mnem);
        Ok(Insn { off: self.s as u32, len: len as u8, mnem: mn, ops, class, target: self.tgt, pc_ref, mem, stack })
    }

    fn rel(&mut self, size: usize) -> R<String> {
        let rel = if size == 1 { self.i8()? } else { self.i32()? };
        let t = self.addr.wrapping_add(self.p as u64).wrapping_add(rel as u64);
        self.tgt = Some(t);
        let mut o = String::new();
        w_uimm(&mut o, t);
        Ok(o)
    }

    /// F2 on a branch is a bound check prefix.
    fn branch_pre(&mut self) {
        if self.rep == 0xF2 {
            self.pre = "bnd ";
        }
    }

    fn imm_z(&mut self, bits: u16) -> R<i64> {
        if bits == 16 {
            self.i16()
        } else {
            self.i32()
        }
    }

    /// `rm, reg`
    fn o_er(&self, bits: u16) -> String {
        let mut o = String::new();
        self.w_rm(&mut o, bits);
        o.push_str(", ");
        self.w_reg(&mut o, bits);
        o
    }

    /// `reg, rm`
    fn o_re(&self, bits: u16) -> String {
        let mut o = String::new();
        self.w_reg(&mut o, bits);
        o.push_str(", ");
        self.w_rm(&mut o, bits);
        o
    }

    // ------------------------------------------------------------------------ driver

    fn insn(&mut self) -> R<Insn> {
        let mut n = 0;
        let op = loop {
            let b = self.u8()?;
            n += 1;
            if n > 14 {
                return Err(Bad::Inval);
            }
            match b {
                0x66 | 0x67 | 0xF0 | 0xF2 | 0xF3 | 0x26 | 0x2E | 0x36 | 0x3E | 0x64 | 0x65 => {
                    self.has_rex = false;
                    self.w = false;
                    self.rr = 0;
                    self.rx = 0;
                    self.rb = 0;
                    match b {
                        0x66 => self.p66 = true,
                        0x67 => self.p67 = true,
                        0xF0 => self.lock = true,
                        0xF2 | 0xF3 => self.rep = b,
                        0x3E => self.ds = true,
                        0x64 | 0x65 => self.seg = b,
                        _ => {}
                    }
                }
                0x40..=0x4F => {
                    self.has_rex = true;
                    self.w = b & 8 != 0;
                    self.rr = (b >> 2) & 1;
                    self.rx = (b >> 1) & 1;
                    self.rb = b & 1;
                }
                _ => break b,
            }
        };
        match op {
            0x0F => self.two(),
            0xC4 | 0xC5 => self.vexp(op),
            0x62 => self.evexp(),
            _ => self.one(op),
        }
    }

    // ------------------------------------------------------------------------ one-byte map

    fn alu(&mut self, op: u8) -> R<Insn> {
        let name = ALU[(op >> 3) as usize];
        let cmp = op >> 3 == 7;
        let base = if cmp { Class::Cmp } else { Class::Alu };
        match op & 7 {
            0..=3 => {
                self.modrm()?;
                let bits = if op & 1 == 0 { 8 } else { self.osz() };
                let o = if op & 2 == 0 { self.o_er(bits) } else { self.o_re(bits) };
                if op >> 3 == 6 && self.is_reg() && self.rm_n() == self.reg | self.rr << 3 {
                    return self.fin(name, o, Class::MovImm);
                }
                self.fin_int(name, o, base, !cmp && op & 2 == 0)
            }
            4 => {
                let v = self.i8()?;
                let mut o = String::from("al, ");
                w_simm(&mut o, v);
                self.fin(name, o, base)
            }
            _ => {
                let bits = self.osz();
                let v = self.imm_z(bits)?;
                let mut o = String::new();
                self.w_g(&mut o, 0, bits);
                o.push_str(", ");
                w_simm(&mut o, v);
                self.fin(name, o, base)
            }
        }
    }

    fn one(&mut self, op: u8) -> R<Insn> {
        match op {
            0x00..=0x3F if op & 7 < 6 => self.alu(op),
            0x50..=0x5F => {
                let bits = if self.p66 { 16 } else { 64 };
                let mut o = String::new();
                self.w_g(&mut o, op & 7 | self.rb << 3, bits);
                self.fin(if op < 0x58 { "push" } else { "pop" }, o, Class::PushPop)
            }
            0x63 => {
                self.modrm()?;
                let bits = self.osz();
                let mut o = String::new();
                self.w_reg(&mut o, bits);
                o.push_str(", ");
                self.w_rm(&mut o, 32);
                self.fin_int("movsxd", o, Class::MovReg, false)
            }
            0x68 | 0x6A => {
                let v = if op == 0x68 { if self.p66 { self.i16()? } else { self.i32()? } } else { self.i8()? };
                let mut o = String::new();
                w_simm(&mut o, v);
                self.fin("push", o, Class::PushPop)
            }
            0x69 | 0x6B => {
                self.modrm()?;
                let bits = self.osz();
                let mut o = self.o_re(bits);
                let v = if op == 0x69 { self.imm_z(bits)? } else { self.i8()? };
                o.push_str(", ");
                w_simm(&mut o, v);
                self.fin_int("imul", o, Class::Mul, false)
            }
            0x6C..=0x6F => {
                self.imem = true;
                self.pre = if self.rep == 0xF3 { "rep " } else { "" };
                let sfx = if op & 1 == 0 { 'b' } else { 'd' };
                let n = if op < 0x6E { format!("ins{}", sfx) } else { format!("outs{}", sfx) };
                self.fin(&n, String::new(), Class::Other)
            }
            0x70..=0x7F => {
                self.branch_pre();
                let o = self.rel(1)?;
                self.fin(&format!("j{}", CC[(op & 15) as usize]), o, Class::CondBranch)
            }
            0x80 | 0x81 | 0x83 => {
                self.modrm()?;
                let bits = if op == 0x80 { 8 } else { self.osz() };
                let v = if op == 0x81 { self.imm_z(bits)? } else { self.i8()? };
                let mut o = String::new();
                self.w_rm(&mut o, bits);
                o.push_str(", ");
                w_simm(&mut o, v);
                let cmp = self.reg == 7;
                self.fin_int(ALU[self.reg as usize], o, if cmp { Class::Cmp } else { Class::Alu }, !cmp)
            }
            0x84 | 0x85 => {
                self.modrm()?;
                let bits = if op == 0x84 { 8 } else { self.osz() };
                let o = self.o_er(bits);
                self.fin_int("test", o, Class::Cmp, false)
            }
            0x86 | 0x87 => {
                self.modrm()?;
                let bits = if op == 0x86 { 8 } else { self.osz() };
                // either operand order encodes the same swap; the lower register number first
                // keeps the text stable across an assembler's choice
                let o = if self.is_reg() && self.reg | self.rr << 3 < self.rm_n() { self.o_re(bits) } else { self.o_er(bits) };
                let c = if self.is_mem() { Class::Atomic } else { Class::Alu };
                self.fin("xchg", o, c)
            }
            0x88..=0x8B => {
                self.modrm()?;
                let bits = if op & 1 == 0 { 8 } else { self.osz() };
                let o = if op < 0x8A { self.o_er(bits) } else { self.o_re(bits) };
                self.fin_int("mov", o, Class::MovReg, op < 0x8A)
            }
            0x8C | 0x8E => {
                self.modrm()?;
                if self.reg > 5 {
                    return Err(Bad::Inval);
                }
                let mut o = String::new();
                let bits = if self.is_mem() { 16 } else { self.osz() };
                if op == 0x8C {
                    self.w_rm(&mut o, bits);
                    o.push_str(", ");
                    o.push_str(SEG[self.reg as usize]);
                } else {
                    o.push_str(SEG[self.reg as usize]);
                    o.push_str(", ");
                    self.w_rm(&mut o, if self.is_mem() { 16 } else { 32 });
                }
                self.fin("mov", o, Class::Other)
            }
            0x8D => {
                self.modrm()?;
                if self.is_reg() {
                    return Err(Bad::Inval);
                }
                self.nomem = true;
                let bits = self.osz();
                let mut o = String::new();
                self.w_reg(&mut o, bits);
                o.push_str(", ");
                self.w_mem(&mut o, 0, 1);
                self.fin("lea", o, Class::Addr)
            }
            0x8F => {
                self.modrm()?;
                if self.reg != 0 {
                    return Err(Bad::Unk);
                }
                let mut o = String::new();
                self.w_rm(&mut o, if self.p66 { 16 } else { 64 });
                self.fin("pop", o, Class::PushPop)
            }
            0x90 => {
                if self.rep == 0xF3 {
                    return self.fin("pause", String::new(), Class::Nop);
                }
                if self.rb == 1 {
                    return self.xchg_acc(op);
                }
                self.fin("nop", String::new(), Class::Nop)
            }
            0x91..=0x97 => self.xchg_acc(op),
            0x98 => {
                let n = if self.w { "cdqe" } else if self.p66 { "cbw" } else { "cwde" };
                self.fin(n, String::new(), Class::Alu)
            }
            0x99 => {
                let n = if self.w { "cqo" } else if self.p66 { "cwd" } else { "cdq" };
                self.fin(n, String::new(), Class::Alu)
            }
            0x9B => self.fin("wait", String::new(), Class::Other),
            0x9C => self.fin("pushfq", String::new(), Class::PushPop),
            0x9D => self.fin("popfq", String::new(), Class::PushPop),
            0x9E => self.fin("sahf", String::new(), Class::Alu),
            0x9F => self.fin("lahf", String::new(), Class::Alu),
            0xA0..=0xA3 => {
                self.p += if self.p67 { 4 } else { 8 };
                if self.p > self.c.len() {
                    return Err(Bad::Trunc);
                }
                Err(Bad::Unk)
            }
            0xA4..=0xA7 | 0xAA..=0xAF => {
                self.imem = true;
                let sfx = if op & 1 == 0 {
                    'b'
                } else {
                    match self.osz() {
                        16 => 'w',
                        32 => 'd',
                        _ => 'q',
                    }
                };
                let (stem, cls, cmpish) = match op & 0xFE {
                    0xA4 => ("movs", Class::Other, false),
                    0xA6 => ("cmps", Class::Cmp, true),
                    0xAA => ("stos", Class::Store, false),
                    0xAC => ("lods", Class::Load, false),
                    _ => ("scas", Class::Cmp, true),
                };
                self.pre = match self.rep {
                    0xF3 if cmpish => "repe ",
                    0xF3 => "rep ",
                    0xF2 => "repne ",
                    _ => "",
                };
                self.fin(&format!("{}{}", stem, sfx), String::new(), cls)
            }
            0xA8 | 0xA9 => {
                let bits = if op == 0xA8 { 8 } else { self.osz() };
                let v = if op == 0xA8 { self.i8()? } else { self.imm_z(bits)? };
                let mut o = String::new();
                self.w_g(&mut o, 0, bits);
                o.push_str(", ");
                w_simm(&mut o, v);
                self.fin("test", o, Class::Cmp)
            }
            0xB0..=0xB7 => {
                let v = self.u8()?;
                let mut o = String::new();
                self.w_g(&mut o, op & 7 | self.rb << 3, 8);
                o.push_str(", ");
                w_uimm(&mut o, v as u64);
                self.fin("mov", o, Class::MovImm)
            }
            0xB8..=0xBF => {
                let bits = self.osz();
                let mut o = String::new();
                self.w_g(&mut o, op & 7 | self.rb << 3, bits);
                o.push_str(", ");
                let mn = match bits {
                    64 => {
                        let v = self.le(8)?;
                        w_uimm(&mut o, v);
                        "movabs"
                    }
                    16 => {
                        let v = self.le(2)?;
                        w_uimm(&mut o, v);
                        "mov"
                    }
                    _ => {
                        let v = self.le(4)?;
                        w_uimm(&mut o, v);
                        "mov"
                    }
                };
                self.fin(mn, o, Class::MovImm)
            }
            0xC0 | 0xC1 | 0xD0..=0xD3 => {
                self.modrm()?;
                let bits = if op & 1 == 0 { 8 } else { self.osz() };
                let mut o = String::new();
                self.w_rm(&mut o, bits);
                o.push_str(", ");
                match op {
                    0xC0 | 0xC1 => {
                        let v = self.u8()?;
                        w_uimm(&mut o, v as u64);
                    }
                    0xD0 | 0xD1 => o.push('1'),
                    _ => o.push_str("cl"),
                }
                self.fin_int(["rol", "ror", "rcl", "rcr", "shl", "shr", "shl", "sar"][self.reg as usize], o, Class::Alu, true)
            }
            0xC2 | 0xC3 => {
                let mut o = String::new();
                if op == 0xC2 {
                    let v = self.le(2)?;
                    w_uimm(&mut o, v);
                }
                self.pre = match self.rep {
                    0xF3 => "rep ",
                    0xF2 => "bnd ",
                    _ => "",
                };
                self.fin("ret", o, Class::Ret)
            }
            0xC6 | 0xC7 => {
                self.modrm()?;
                if self.reg != 0 {
                    return self.unk(0);
                }
                let bits = if op == 0xC6 { 8 } else { self.osz() };
                let mut o = String::new();
                self.w_rm(&mut o, bits);
                o.push_str(", ");
                match bits {
                    8 => {
                        let v = self.u8()?;
                        w_uimm(&mut o, v as u64);
                    }
                    16 => {
                        let v = self.le(2)?;
                        w_uimm(&mut o, v);
                    }
                    32 => {
                        let v = self.le(4)?;
                        w_uimm(&mut o, v);
                    }
                    _ => {
                        let v = self.i32()?;
                        w_simm(&mut o, v);
                    }
                }
                self.fin_int("mov", o, Class::MovImm, true)
            }
            0xC8 => {
                let a = self.le(2)?;
                let b = self.u8()?;
                let mut o = String::new();
                w_uimm(&mut o, a);
                o.push_str(", ");
                w_uimm(&mut o, b as u64);
                self.fin("enter", o, Class::PushPop)
            }
            0xC9 => self.fin("leave", String::new(), Class::PushPop),
            0xCA => {
                let a = self.le(2)?;
                let mut o = String::new();
                w_uimm(&mut o, a);
                self.fin("retf", o, Class::Ret)
            }
            0xCB => self.fin("retf", String::new(), Class::Ret),
            0xCC => self.fin("int3", String::new(), Class::Trap),
            0xCD => {
                let v = self.u8()?;
                let mut o = String::new();
                w_uimm(&mut o, v as u64);
                self.fin("int", o, Class::Other)
            }
            0xCF => self.fin(if self.w { "iretq" } else { "iretd" }, String::new(), Class::Ret),
            0xD7 => self.fin("xlatb", String::new(), Class::Load),
            0xD8..=0xDF => self.unk(0),
            0xE0..=0xE3 => {
                let o = self.rel(1)?;
                let n = match op {
                    0xE0 => "loopne",
                    0xE1 => "loope",
                    0xE2 => "loop",
                    _ => {
                        if self.p67 {
                            "jecxz"
                        } else {
                            "jrcxz"
                        }
                    }
                };
                self.fin(n, o, Class::CondBranch)
            }
            0xE4..=0xE7 | 0xEC..=0xEF => {
                let bits = if op & 1 == 0 { 8 } else if self.p66 { 16 } else { 32 };
                let mut o = String::new();
                let imm = if op < 0xE8 { Some(self.u8()?) } else { None };
                let port = |o: &mut String| match imm {
                    Some(v) => w_uimm(o, v as u64),
                    None => o.push_str("dx"),
                };
                if op & 2 == 0 {
                    self.w_g(&mut o, 0, bits);
                    o.push_str(", ");
                    port(&mut o);
                    self.fin("in", o, Class::Other)
                } else {
                    port(&mut o);
                    o.push_str(", ");
                    self.w_g(&mut o, 0, bits);
                    self.fin("out", o, Class::Other)
                }
            }
            0xE8 => {
                self.branch_pre();
                let o = self.rel(4)?;
                self.fin("call", o, Class::Call)
            }
            0xE9 | 0xEB => {
                self.branch_pre();
                let o = self.rel(if op == 0xE9 { 4 } else { 1 })?;
                self.fin("jmp", o, Class::Branch)
            }
            0xF1 => self.fin("int1", String::new(), Class::Trap),
            0xF4 => self.fin("hlt", String::new(), Class::Trap),
            0xF5 => self.fin("cmc", String::new(), Class::Alu),
            0xF8 => self.fin("clc", String::new(), Class::Alu),
            0xF9 => self.fin("stc", String::new(), Class::Alu),
            0xFA => self.fin("cli", String::new(), Class::Other),
            0xFB => self.fin("sti", String::new(), Class::Other),
            0xFC => self.fin("cld", String::new(), Class::Alu),
            0xFD => self.fin("std", String::new(), Class::Alu),
            0xF6 | 0xF7 => {
                self.modrm()?;
                let bits = if op == 0xF6 { 8 } else { self.osz() };
                let mut o = String::new();
                self.w_rm(&mut o, bits);
                match self.reg {
                    0 | 1 => {
                        let v = if bits == 8 { self.i8()? } else { self.imm_z(bits)? };
                        o.push_str(", ");
                        w_simm(&mut o, v);
                        self.fin_int("test", o, Class::Cmp, false)
                    }
                    2 => self.fin_int("not", o, Class::Alu, true),
                    3 => self.fin_int("neg", o, Class::Alu, true),
                    4 => self.fin_int("mul", o, Class::Mul, false),
                    5 => self.fin_int("imul", o, Class::Mul, false),
                    6 => self.fin_int("div", o, Class::Div, false),
                    _ => self.fin_int("idiv", o, Class::Div, false),
                }
            }
            0xFE => {
                self.modrm()?;
                if self.reg > 1 {
                    return Err(Bad::Inval);
                }
                let mut o = String::new();
                self.w_rm(&mut o, 8);
                self.fin_int(if self.reg == 0 { "inc" } else { "dec" }, o, Class::Alu, true)
            }
            0xFF => {
                self.modrm()?;
                let mut o = String::new();
                match self.reg {
                    0 | 1 => {
                        self.w_rm(&mut o, self.osz());
                        self.fin_int(if self.reg == 0 { "inc" } else { "dec" }, o, Class::Alu, true)
                    }
                    2 | 4 => {
                        self.w_rm(&mut o, 64);
                        if self.ds {
                            self.pre = "notrack ";
                        } else {
                            self.branch_pre();
                        }
                        if self.reg == 2 {
                            self.fin("call", o, Class::IndirectCall)
                        } else {
                            self.fin("jmp", o, Class::IndirectBranch)
                        }
                    }
                    6 => {
                        self.w_rm(&mut o, if self.p66 { 16 } else { 64 });
                        self.fin("push", o, Class::PushPop)
                    }
                    3 | 5 if self.is_mem() => Err(Bad::Unk),
                    _ => Err(Bad::Inval),
                }
            }
            _ => Err(Bad::Inval),
        }
    }

    fn xchg_acc(&mut self, op: u8) -> R<Insn> {
        let bits = self.osz();
        let mut o = String::new();
        self.w_g(&mut o, op & 7 | self.rb << 3, bits);
        o.push_str(", ");
        self.w_g(&mut o, 0, bits);
        self.fin("xchg", o, Class::Alu)
    }
}

// ---------------------------------------------------------------------------- 0F maps

impl<'a> Dec<'a> {
    /// The mandatory prefix of a legacy encoding: 0 none, 1 66, 2 F3, 3 F2.
    fn pp_legacy(&self) -> u8 {
        match (self.rep, self.p66) {
            (0xF3, _) => 2,
            (0xF2, _) => 3,
            (_, true) => 1,
            _ => 0,
        }
    }

    fn no_ops(&mut self, n: &str, c: Class) -> R<Insn> {
        self.fin(n, String::new(), c)
    }

    fn two(&mut self) -> R<Insn> {
        let op = self.u8()?;
        match op {
            0x05 => self.no_ops("syscall", Class::Other),
            0x06 => self.no_ops("clts", Class::Other),
            0x07 => self.no_ops(if self.w { "sysretq" } else { "sysretl" }, Class::Other),
            0x08 => self.no_ops("invd", Class::Other),
            0x09 => self.no_ops("wbinvd", Class::Other),
            0x0B => self.no_ops("ud2", Class::Trap),
            0x0E => self.no_ops("femms", Class::Other),
            0x30 => self.no_ops("wrmsr", Class::Other),
            0x31 => self.no_ops("rdtsc", Class::Other),
            0x32 => self.no_ops("rdmsr", Class::Other),
            0x33 => self.no_ops("rdpmc", Class::Other),
            0x34 => self.no_ops("sysenter", Class::Other),
            0x35 => self.no_ops("sysexit", Class::Other),
            0xA2 => self.no_ops("cpuid", Class::Other),
            0xAA => self.no_ops("rsm", Class::Other),
            0x77 => self.vec_op(1, 0, 0x77),
            0x00 => self.unk(0),
            0x01 => {
                self.modrm()?;
                if self.is_reg() {
                    let n = match (self.reg, self.rm_n() & 7) {
                        (2, 0) => "xgetbv",
                        (2, 1) => "xsetbv",
                        (2, 5) => "xend",
                        (2, 6) => "xtest",
                        (7, 0) => "swapgs",
                        (7, 1) => "rdtscp",
                        _ => return Err(Bad::Unk),
                    };
                    return self.no_ops(n, Class::Other);
                }
                Err(Bad::Unk)
            }
            0x0D => {
                self.modrm()?;
                if self.is_reg() {
                    return Err(Bad::Inval);
                }
                let n = match self.reg {
                    1 => "prefetchw",
                    2 => "prefetchwt1",
                    _ => "prefetch",
                };
                let mut o = String::new();
                self.w_rm(&mut o, 8);
                self.fin(n, o, Class::Other)
            }
            0x18 | 0x19..=0x1F => {
                self.modrm()?;
                if op == 0x18 && self.is_mem() && self.reg < 4 {
                    let mut o = String::new();
                    self.w_rm(&mut o, 8);
                    return self.fin(["prefetchnta", "prefetcht0", "prefetcht1", "prefetcht2"][self.reg as usize], o, Class::Other);
                }
                if op == 0x1E && self.rep == 0xF3 && self.mode == 3 && self.reg == 7 && (self.rm_n() & 7) >= 2 && (self.rm_n() & 7) <= 3 {
                    return self.no_ops(if self.rm_n() & 7 == 2 { "endbr64" } else { "endbr32" }, Class::Nop);
                }
                self.nomem = true;
                let mut o = String::new();
                self.w_rm(&mut o, self.osz());
                self.fin("nop", o, Class::Nop)
            }
            0x20..=0x23 => {
                self.modrm()?;
                let (kind, n) = if op & 1 == 0 { ("cr", self.reg | self.rr << 3) } else { ("dr", self.reg | self.rr << 3) };
                let mut o = String::new();
                let mut g = String::new();
                self.w_g(&mut g, self.rm_n(), 64);
                let mut s = String::from(kind);
                push_dec(&mut s, n as u32);
                if op < 0x22 {
                    o.push_str(&g);
                    o.push_str(", ");
                    o.push_str(&s);
                } else {
                    o.push_str(&s);
                    o.push_str(", ");
                    o.push_str(&g);
                }
                self.fin("mov", o, Class::Other)
            }
            0x40..=0x4F => {
                self.modrm()?;
                let o = self.o_re(self.osz());
                self.fin_int(&format!("cmov{}", CC[(op & 15) as usize]), o, Class::Select, false)
            }
            0x80..=0x8F => {
                self.branch_pre();
                let o = self.rel(4)?;
                self.fin(&format!("j{}", CC[(op & 15) as usize]), o, Class::CondBranch)
            }
            0x90..=0x9F => {
                self.modrm()?;
                let mut o = String::new();
                self.w_rm(&mut o, 8);
                self.fin_int(&format!("set{}", CC[(op & 15) as usize]), o, Class::Select, true)
            }
            0xA0 | 0xA8 => {
                let n = if op == 0xA0 { "fs" } else { "gs" };
                self.fin("push", n.to_string(), Class::PushPop)
            }
            0xA1 | 0xA9 => {
                let n = if op == 0xA1 { "fs" } else { "gs" };
                self.fin("pop", n.to_string(), Class::PushPop)
            }
            0xA3 | 0xAB | 0xB3 | 0xBB => {
                self.modrm()?;
                let o = self.o_er(self.osz());
                let (n, c, wr) = match op {
                    0xA3 => ("bt", Class::Cmp, false),
                    0xAB => ("bts", Class::Alu, true),
                    0xB3 => ("btr", Class::Alu, true),
                    _ => ("btc", Class::Alu, true),
                };
                self.fin_int(n, o, c, wr)
            }
            0xA4 | 0xA5 | 0xAC | 0xAD => {
                self.modrm()?;
                let mut o = self.o_er(self.osz());
                o.push_str(", ");
                if op & 1 == 0 {
                    let v = self.u8()?;
                    w_uimm(&mut o, v as u64);
                } else {
                    o.push_str("cl");
                }
                self.fin_int(if op < 0xAC { "shld" } else { "shrd" }, o, Class::Alu, true)
            }
            0xAE => self.grp15(),
            0xAF => {
                self.modrm()?;
                let o = self.o_re(self.osz());
                self.fin_int("imul", o, Class::Mul, false)
            }
            0xB0 | 0xB1 | 0xC0 | 0xC1 => {
                self.modrm()?;
                let bits = if op & 1 == 0 { 8 } else { self.osz() };
                let o = self.o_er(bits);
                if op < 0xC0 {
                    self.fin("cmpxchg", o, if self.is_mem() { Class::Atomic } else { Class::Alu })
                } else {
                    self.fin_int("xadd", o, Class::Alu, true)
                }
            }
            0xB6 | 0xB7 | 0xBE | 0xBF => {
                self.modrm()?;
                let mut o = String::new();
                self.w_reg(&mut o, self.osz());
                o.push_str(", ");
                self.w_rm(&mut o, if op & 1 == 0 { 8 } else { 16 });
                self.fin_int(if op < 0xB8 { "movzx" } else { "movsx" }, o, Class::MovReg, false)
            }
            0xB8 if self.rep == 0xF3 => {
                self.modrm()?;
                let o = self.o_re(self.osz());
                self.fin_int("popcnt", o, Class::Alu, false)
            }
            0xB9 | 0xFF => {
                self.modrm()?;
                let o = self.o_re(32);
                self.nomem = true;
                self.fin(if op == 0xFF { "ud0" } else { "ud1" }, o, Class::Trap)
            }
            0xBA => {
                self.modrm()?;
                if self.reg < 4 {
                    return Err(Bad::Inval);
                }
                let mut o = String::new();
                self.w_rm(&mut o, self.osz());
                o.push_str(", ");
                let v = self.u8()?;
                w_uimm(&mut o, v as u64);
                let (n, c, wr) = match self.reg {
                    4 => ("bt", Class::Cmp, false),
                    5 => ("bts", Class::Alu, true),
                    6 => ("btr", Class::Alu, true),
                    _ => ("btc", Class::Alu, true),
                };
                self.fin_int(n, o, c, wr)
            }
            0xBC | 0xBD => {
                self.modrm()?;
                let o = self.o_re(self.osz());
                let n = match (op, self.rep == 0xF3) {
                    (0xBC, false) => "bsf",
                    (0xBC, true) => "tzcnt",
                    (_, false) => "bsr",
                    (_, true) => "lzcnt",
                };
                self.fin_int(n, o, Class::Alu, false)
            }
            0xC3 => {
                self.modrm()?;
                if self.is_reg() {
                    return Err(Bad::Inval);
                }
                let o = self.o_er(self.osz());
                self.fin_int("movnti", o, Class::Store, true)
            }
            0xC7 => {
                self.modrm()?;
                if self.is_mem() {
                    if self.reg == 1 {
                        let mut o = String::new();
                        self.w_rm(&mut o, if self.w { 128 } else { 64 });
                        return self.fin(if self.w { "cmpxchg16b" } else { "cmpxchg8b" }, o, Class::Atomic);
                    }
                    return Err(Bad::Unk);
                }
                if self.reg == 6 || (self.reg == 7 && self.rep != 0xF3) {
                    let mut o = String::new();
                    self.w_rm(&mut o, self.osz());
                    return self.fin(if self.reg == 6 { "rdrand" } else { "rdseed" }, o, Class::Other);
                }
                Err(Bad::Unk)
            }
            0xC8..=0xCF => {
                let mut o = String::new();
                self.w_g(&mut o, op & 7 | self.rb << 3, if self.w { 64 } else { 32 });
                self.fin("bswap", o, Class::Alu)
            }
            0x38 => {
                let op = self.u8()?;
                self.three_38(op)
            }
            0x3A => {
                let op = self.u8()?;
                let pp = self.pp_legacy();
                self.vec_op(3, pp, op)
            }
            _ => {
                let pp = self.pp_legacy();
                self.vec_op(1, pp, op)
            }
        }
    }

    fn grp15(&mut self) -> R<Insn> {
        self.modrm()?;
        if self.is_reg() {
            if self.rep == 0xF3 && self.reg < 4 {
                let mut o = String::new();
                self.w_rm(&mut o, if self.w { 64 } else { 32 });
                return self.fin(["rdfsbase", "rdgsbase", "wrfsbase", "wrgsbase"][self.reg as usize], o, Class::Other);
            }
            return match self.reg {
                5 => self.no_ops("lfence", Class::Atomic),
                6 => self.no_ops("mfence", Class::Atomic),
                7 => self.no_ops("sfence", Class::Atomic),
                _ => Err(Bad::Unk),
            };
        }
        let (n, bits, wr): (&str, u16, Option<bool>) = match (self.reg, self.p66) {
            (0, _) => ("fxsave", 0, Some(true)),
            (1, _) => ("fxrstor", 0, Some(false)),
            (2, _) => ("ldmxcsr", 32, Some(false)),
            (3, _) => ("stmxcsr", 32, Some(true)),
            (4, _) => ("xsave", 0, Some(true)),
            (5, _) => ("xrstor", 0, Some(false)),
            (6, false) => ("xsaveopt", 0, Some(true)),
            (6, true) => ("clwb", 8, None),
            (_, false) => ("clflush", 8, None),
            (_, true) => ("clflushopt", 8, None),
        };
        let mut name = n.to_string();
        if self.w && bits == 0 {
            name.push_str("64");
        }
        let mut o = String::new();
        self.w_rm(&mut o, bits);
        let c = match wr {
            Some(w) => self.side(w),
            None => Class::Other,
        };
        self.fin(&name, o, c)
    }

    fn three_38(&mut self, op: u8) -> R<Insn> {
        let pp = self.pp_legacy();
        match (pp, op) {
            (0, 0xF0) | (0, 0xF1) => {
                self.modrm()?;
                if self.is_reg() {
                    return Err(Bad::Inval);
                }
                let bits = self.osz();
                let (o, wr) = if op == 0xF0 { (self.o_re(bits), false) } else { (self.o_er(bits), true) };
                self.fin_int("movbe", o, Class::MovReg, wr)
            }
            (3, 0xF0) | (3, 0xF1) => {
                self.modrm()?;
                let mut o = String::new();
                self.w_reg(&mut o, if self.w { 64 } else { 32 });
                o.push_str(", ");
                self.w_rm(&mut o, if op == 0xF0 { 8 } else { self.osz() });
                self.fin_int("crc32", o, Class::Alu, false)
            }
            (1, 0xF6) | (2, 0xF6) => {
                self.modrm()?;
                let o = self.o_re(if self.w { 64 } else { 32 });
                self.fin_int(if pp == 1 { "adcx" } else { "adox" }, o, Class::Alu, false)
            }
            _ => self.vec_op(2, pp, op),
        }
    }

    // ------------------------------------------------------------------------ vector families

    /// Register kind for a width class: 0 xmm, 1 ymm, 2 zmm, 3 mmx.
    fn rkind(&self, rk: Rk, mmx: bool) -> u8 {
        if mmx {
            return 3;
        }
        match rk {
            Rk::V => self.vl,
            Rk::X => 0,
            Rk::H => {
                if self.vl == 2 {
                    1
                } else {
                    0
                }
            }
            Rk::K => 4,
        }
    }

    fn ms_bits(&self, ms: Ms, mmx: bool) -> u16 {
        if mmx {
            return 64;
        }
        let v = 128u16 << self.vl;
        match ms {
            Ms::V => v,
            Ms::H => v / 2,
            Ms::Q => v / 4,
            Ms::E => v / 8,
            Ms::B(n) => n,
            Ms::D => {
                if self.vl == 0 {
                    64
                } else {
                    v
                }
            }
        }
    }

    /// Vector register or memory operand of the modrm.rm field.
    fn w_xrm(&self, o: &mut String, k: u8, msbits: u16) {
        match self.rm {
            Rm::R(n) => self.w_v(o, n, k),
            Rm::M(_) => self.w_mem(o, msbits, 1),
        }
    }

    fn w_xreg(&self, o: &mut String, k: u8) {
        self.w_v(o, self.reg_n(), k);
    }

    fn w_vvvv(&self, o: &mut String, k: u8) {
        self.w_v(o, self.vvvv | self.vv2 << 4, k);
    }

    fn vname(&self, n: &str) -> String {
        if self.vex || self.evex {
            format!("v{}", n)
        } else {
            n.to_string()
        }
    }

    /// Class of a vector data move.
    fn mv_cls(&self, store: bool) -> Class {
        if self.is_mem() {
            self.side(store)
        } else {
            Class::MovReg
        }
    }

    fn vec_op(&mut self, map: u8, pp: u8, op: u8) -> R<Insn> {
        if let Some(i) = self.vec_special(map, pp, op)? {
            return Ok(i);
        }
        let imm = if map == 3 || (map == 1 && matches!(op, 0x70 | 0xC2 | 0xC4 | 0xC5 | 0xC6)) { 1 } else { 0 };
        let found = if self.evex { tab_evex(map, pp, op, self.w).or_else(|| tab(map, pp, op, self.w)) } else { tab(map, pp, op, self.w) };
        let en = match found {
            Some(e) if !e.n.is_empty() && !(e.vo && !self.vex && !self.evex) && !(e.mmx && (self.vex || self.evex)) => e,
            _ => return self.unk(imm),
        };
        self.gen_vec(en)
    }

    /// ` {k1}{z}` after the destination operand of an EVEX instruction.
    fn w_mask(&self, o: &mut String) {
        if self.evex && self.aaa != 0 {
            o.push_str(" {k");
            push_dec(o, self.aaa as u32);
            o.push('}');
            if self.zero {
                o.push_str("{z}");
            }
        }
    }

    /// The EVEX broadcast bit: `{1toN}` on a memory operand, embedded rounding / exception
    /// suppression (`, {rn-sae}`) on a register operand, which also makes the length 512. Returns
    /// the text to append after the last operand.
    fn evex_b(&mut self, elem_bits: u16) -> &'static str {
        if !(self.evex && self.bcast) {
            return "";
        }
        if self.is_reg() {
            self.vl = 2;
            return [", {rn-sae}", ", {rd-sae}", ", {ru-sae}", ", {rz-sae}"][self.ll as usize & 3];
        }
        self.bcn = (128u16 << self.vl) / elem_bits;
        ""
    }

    fn gen_vec(&mut self, en: Ent) -> R<Insn> {
        self.modrm()?;
        let imm = matches!(en.f, Fm::RvmI | Fm::RmI | Fm::MrI);
        if en.memonly && self.is_reg() {
            return self.unk(imm as usize);
        }
        let mmx = en.mmx && !self.vex && !self.evex;
        // broadcast: only entries whose memory operand is a full vector of 32 / 64 bit elements
        let elem = if self.w { 64 } else { 32 };
        let tail = if en.ms == Ms::V && matches!(en.f, Fm::Rvm | Fm::RvmI | Fm::Rm | Fm::RmI) { self.evex_b(elem) } else if self.evex && self.bcast { return self.unk(imm as usize) } else { "" };
        let dk = self.rkind(en.dk, mmx);
        let sk = self.rkind(en.sk, mmx);
        let vk = if dk == 4 { self.vl } else { dk };
        let msbits = if self.bcn != 0 { elem } else { self.ms_bits(en.ms, mmx) };
        let mut o = String::new();
        match en.f {
            Fm::Rvm | Fm::RvmI => {
                self.w_xreg(&mut o, dk);
                self.w_mask(&mut o);
                if self.vex || self.evex {
                    o.push_str(", ");
                    self.w_vvvv(&mut o, vk);
                }
                o.push_str(", ");
                self.w_xrm(&mut o, sk, msbits);
            }
            Fm::Rm | Fm::RmI => {
                self.w_xreg(&mut o, dk);
                self.w_mask(&mut o);
                o.push_str(", ");
                self.w_xrm(&mut o, sk, msbits);
            }
            Fm::Mr | Fm::MrI => {
                self.w_xrm(&mut o, dk, msbits);
                self.w_mask(&mut o);
                o.push_str(", ");
                self.w_xreg(&mut o, sk);
            }
        }
        if imm {
            let v = self.u8()?;
            o.push_str(", ");
            w_uimm(&mut o, v as u64);
        }
        o.push_str(tail);
        let mut cls = en.cls;
        if en.mv {
            cls = self.mv_cls(matches!(en.f, Fm::Mr | Fm::MrI));
        }
        if matches!(en.n.as_str(), "pxor" | "xorps" | "xorpd" | "pxord" | "pxorq") && en.f == Fm::Rvm && self.is_reg() && self.rm_n() == self.reg_n() && self.aaa == 0 && (!(self.vex || self.evex) || self.vvvv | self.vv2 << 4 == self.reg_n()) {
            cls = Class::MovImm;
        }
        let n = self.vname(&en.n);
        self.fin(&n, o, cls)
    }

    fn vec_special(&mut self, map: u8, pp: u8, op: u8) -> R<Option<Insn>> {
        let vx = self.vex || self.evex;
        let r = match (map, op) {
            (1, 0x10) | (1, 0x11) if pp >= 2 => {
                self.modrm()?;
                let sd = pp == 3;
                let bits = if sd { 64 } else { 32 };
                let n = self.vname(if sd { "movsd" } else { "movss" });
                let store = op == 0x11;
                let mut o = String::new();
                if self.is_mem() {
                    if store {
                        self.w_mem(&mut o, bits, 1);
                        o.push_str(", ");
                        self.w_xreg(&mut o, 0);
                    } else {
                        self.w_xreg(&mut o, 0);
                        o.push_str(", ");
                        self.w_mem(&mut o, bits, 1);
                    }
                } else {
                    let (a, b) = if store { (self.rm_n(), self.reg_n()) } else { (self.reg_n(), self.rm_n()) };
                    self.w_v(&mut o, a, 0);
                    if vx {
                        o.push_str(", ");
                        self.w_vvvv(&mut o, 0);
                    }
                    o.push_str(", ");
                    self.w_v(&mut o, b, 0);
                }
                let c = self.mv_cls(store);
                self.fin(&n, o, c)?
            }
            (1, 0x12 | 0x13 | 0x16 | 0x17) if pp < 2 => {
                self.modrm()?;
                let store = op & 1 == 1;
                let hi = op >= 0x16;
                let suffix = if pp == 0 { "ps" } else { "pd" };
                if self.is_reg() && (store || pp == 1) {
                    return self.unk(0).map(Some);
                }
                let mut o = String::new();
                let name;
                let c;
                if self.is_reg() {
                    name = if hi { "movlhps" } else { "movhlps" }.to_string();
                    self.w_xreg(&mut o, 0);
                    if vx {
                        o.push_str(", ");
                        self.w_vvvv(&mut o, 0);
                    }
                    o.push_str(", ");
                    self.w_xrm(&mut o, 0, 0);
                    c = Class::Vector;
                } else {
                    name = format!("mov{}{}", if hi { 'h' } else { 'l' }, suffix);
                    if store {
                        self.w_mem(&mut o, 64, 1);
                        o.push_str(", ");
                        self.w_xreg(&mut o, 0);
                    } else {
                        self.w_xreg(&mut o, 0);
                        if vx {
                            o.push_str(", ");
                            self.w_vvvv(&mut o, 0);
                        }
                        o.push_str(", ");
                        self.w_mem(&mut o, 64, 1);
                    }
                    c = self.side(store);
                }
                let n = self.vname(&name);
                self.fin(&n, o, c)?
            }
            (1, 0x2A) if pp >= 2 => {
                self.modrm()?;
                let bits = if self.w { 64 } else { 32 };
                let mut o = String::new();
                self.w_xreg(&mut o, 0);
                if vx {
                    o.push_str(", ");
                    self.w_vvvv(&mut o, 0);
                }
                o.push_str(", ");
                self.w_rm(&mut o, bits);
                let n = self.vname(if pp == 2 { "cvtsi2ss" } else { "cvtsi2sd" });
                self.fin(&n, o, Class::Fp)?
            }
            (1, 0x2C | 0x2D) if pp >= 2 => {
                self.modrm()?;
                let mut o = String::new();
                self.w_reg(&mut o, if self.w { 64 } else { 32 });
                o.push_str(", ");
                self.w_xrm(&mut o, 0, if pp == 2 { 32 } else { 64 });
                let n = self.vname(&format!("cvt{}{}2si", if op == 0x2C { "t" } else { "" }, if pp == 2 { "ss" } else { "sd" }));
                self.fin(&n, o, Class::Fp)?
            }
            (1, 0x50) | (1, 0xD7) if pp < 2 && (op == 0x50 || pp == 1) => {
                self.modrm()?;
                if self.is_mem() {
                    return self.unk(0).map(Some);
                }
                let mut o = String::new();
                self.w_reg(&mut o, 32);
                o.push_str(", ");
                let mmx = op == 0xD7 && pp == 0;
                self.w_v(&mut o, self.rm_n(), if mmx { 3 } else { self.vl });
                let n = self.vname(if op == 0xD7 { "pmovmskb" } else if pp == 0 { "movmskps" } else { "movmskpd" });
                self.fin(&n, o, Class::Vector)?
            }
            (1, 0xD7) if pp == 0 && !vx => {
                self.modrm()?;
                let mut o = String::new();
                self.w_reg(&mut o, 32);
                o.push_str(", ");
                self.w_v(&mut o, self.rm_n(), 3);
                self.fin("pmovmskb", o, Class::Vector)?
            }
            (1, 0x6E) if pp < 2 => {
                self.modrm()?;
                let k = if pp == 0 { 3 } else { 0 };
                let mut o = String::new();
                self.w_v(&mut o, self.reg_n(), k);
                o.push_str(", ");
                self.w_rm(&mut o, if self.w { 64 } else { 32 });
                let n = self.vname(if self.w { "movq" } else { "movd" });
                let c = self.mv_cls(false);
                self.fin(&n, o, c)?
            }
            (1, 0x7E) if pp == 0 || pp == 1 => {
                self.modrm()?;
                let k = if pp == 0 { 3 } else { 0 };
                let mut o = String::new();
                self.w_rm(&mut o, if self.w { 64 } else { 32 });
                o.push_str(", ");
                self.w_v(&mut o, self.reg_n(), k);
                let n = self.vname(if self.w { "movq" } else { "movd" });
                let c = self.mv_cls(true);
                self.fin(&n, o, c)?
            }
            (1, 0x7E) | (1, 0xD6) if (op == 0x7E && pp == 2) || (op == 0xD6 && pp == 1) => {
                self.modrm()?;
                let mut o = String::new();
                if op == 0x7E {
                    self.w_xreg(&mut o, 0);
                    o.push_str(", ");
                    self.w_xrm(&mut o, 0, 64);
                } else {
                    self.w_xrm(&mut o, 0, 64);
                    o.push_str(", ");
                    self.w_xreg(&mut o, 0);
                }
                let n = self.vname("movq");
                let c = self.mv_cls(op == 0xD6);
                self.fin(&n, o, c)?
            }
            (1, 0x71 | 0x72 | 0x73) if pp < 2 => {
                self.modrm()?;
                let name = match (op, self.reg) {
                    (0x72, 0) if self.evex => {
                        if self.w {
                            "prorq"
                        } else {
                            "prord"
                        }
                    }
                    (0x72, 1) if self.evex => {
                        if self.w {
                            "prolq"
                        } else {
                            "prold"
                        }
                    }
                    (0x72, 4) if self.evex && self.w => "psraq",
                    (0x71, 2) => "psrlw",
                    (0x71, 4) => "psraw",
                    (0x71, 6) => "psllw",
                    (0x72, 2) => "psrld",
                    (0x72, 4) => "psrad",
                    (0x72, 6) => "pslld",
                    (0x73, 2) => "psrlq",
                    (0x73, 3) if pp == 1 => "psrldq",
                    (0x73, 6) => "psllq",
                    (0x73, 7) if pp == 1 => "pslldq",
                    _ => return self.unk(1).map(Some),
                };
                if self.is_mem() {
                    return self.unk(1).map(Some);
                }
                let k = if pp == 0 { 3 } else { self.vl };
                let mut o = String::new();
                if vx {
                    self.w_vvvv(&mut o, k);
                    self.w_mask(&mut o);
                    o.push_str(", ");
                }
                self.w_v(&mut o, self.rm_n(), k);
                let v = self.u8()?;
                o.push_str(", ");
                w_uimm(&mut o, v as u64);
                let n = self.vname(name);
                self.fin(&n, o, Class::Vector)?
            }
            (1, 0xAE) if pp == 0 && self.vex => {
                self.modrm()?;
                if self.is_reg() || !(2..=3).contains(&self.reg) {
                    return Err(Bad::Unk);
                }
                let mut o = String::new();
                self.w_mem(&mut o, 32, 1);
                let c = self.side(self.reg == 3);
                self.fin(if self.reg == 2 { "vldmxcsr" } else { "vstmxcsr" }, o, c)?
            }
            (1, 0x77) if pp == 0 => {
                if vx {
                    self.fin(if self.vl == 0 { "vzeroupper" } else { "vzeroall" }, String::new(), Class::Vector)?
                } else {
                    self.fin("emms", String::new(), Class::Other)?
                }
            }
            (1, 0xC2) => {
                self.modrm()?;
                if self.evex && self.bcast {
                    return self.unk(1).map(Some);
                }
                let k = if pp >= 2 { 0 } else { self.vl };
                let mut o = String::new();
                self.w_xreg(&mut o, if self.evex { 4 } else { k });
                self.w_mask(&mut o);
                if vx {
                    o.push_str(", ");
                    self.w_vvvv(&mut o, k);
                }
                o.push_str(", ");
                self.w_xrm(&mut o, k, match pp {
                    2 => 32,
                    3 => 64,
                    _ => 128 << self.vl,
                });
                let v = self.u8()?;
                let sfx = ["ps", "pd", "ss", "sd"][pp as usize];
                let name = if v < 8 {
                    format!("cmp{}{}", ["eq", "lt", "le", "unord", "neq", "nlt", "nle", "ord"][v as usize], sfx)
                } else {
                    o.push_str(", ");
                    w_uimm(&mut o, v as u64);
                    format!("cmp{}", sfx)
                };
                let n = self.vname(&name);
                self.fin(&n, o, if pp >= 2 { Class::Fp } else { Class::Vector })?
            }
            (1, 0xC4) if pp < 2 => {
                self.modrm()?;
                let k = if pp == 0 { 3 } else { 0 };
                let mut o = String::new();
                self.w_v(&mut o, self.reg_n(), k);
                if vx {
                    o.push_str(", ");
                    self.w_vvvv(&mut o, 0);
                }
                o.push_str(", ");
                if self.is_mem() {
                    self.w_mem(&mut o, 16, 1);
                } else {
                    self.w_g(&mut o, self.rm_n(), 32);
                }
                let v = self.u8()?;
                o.push_str(", ");
                w_uimm(&mut o, v as u64);
                let n = self.vname("pinsrw");
                self.fin(&n, o, Class::Vector)?
            }
            (1, 0xC5) if pp < 2 => {
                self.modrm()?;
                if self.is_mem() {
                    return self.unk(1).map(Some);
                }
                let mut o = String::new();
                self.w_reg(&mut o, 32);
                o.push_str(", ");
                self.w_v(&mut o, self.rm_n(), if pp == 0 { 3 } else { 0 });
                let v = self.u8()?;
                o.push_str(", ");
                w_uimm(&mut o, v as u64);
                let n = self.vname("pextrw");
                self.fin(&n, o, Class::Vector)?
            }
            (2, 0x10 | 0x14 | 0x15) if pp == 1 && !vx => {
                self.modrm()?;
                let mut o = String::new();
                self.w_xreg(&mut o, 0);
                o.push_str(", ");
                self.w_xrm(&mut o, 0, 128);
                o.push_str(", xmm0");
                self.fin(["pblendvb", "", "", "", "blendvps", "blendvpd"][(op - 0x10) as usize], o, Class::Vector)?
            }
            (2, 0x2C | 0x2D | 0x2E | 0x2F | 0x8C | 0x8E) if pp == 1 && vx => {
                self.modrm()?;
                if self.is_reg() {
                    return self.unk(0).map(Some);
                }
                let name = match op {
                    0x2C | 0x2E => "maskmovps",
                    0x2D | 0x2F => "maskmovpd",
                    _ => {
                        if self.w {
                            "pmaskmovq"
                        } else {
                            "pmaskmovd"
                        }
                    }
                };
                let store = op == 0x2E || op == 0x2F || op == 0x8E;
                let mut o = String::new();
                if store {
                    self.w_mem(&mut o, 128 << self.vl, 1);
                    o.push_str(", ");
                    self.w_vvvv(&mut o, self.vl);
                    o.push_str(", ");
                    self.w_xreg(&mut o, self.vl);
                } else {
                    self.w_xreg(&mut o, self.vl);
                    o.push_str(", ");
                    self.w_vvvv(&mut o, self.vl);
                    o.push_str(", ");
                    self.w_mem(&mut o, 128 << self.vl, 1);
                }
                let n = self.vname(name);
                let c = self.side(store);
                self.fin(&n, o, c)?
            }
            (2, 0xF2..=0xF7) if self.vex => return self.bmi(pp, op).map(Some),
            (3, 0xF0) if self.vex && pp == 3 => {
                self.modrm()?;
                let bits = if self.w { 64 } else { 32 };
                let mut o = String::new();
                self.w_reg(&mut o, bits);
                o.push_str(", ");
                self.w_rm(&mut o, bits);
                let v = self.u8()?;
                o.push_str(", ");
                w_uimm(&mut o, v as u64);
                self.fin_int("rorx", o, Class::Alu, false)?
            }
            (3, 0x14 | 0x15 | 0x16 | 0x17) if pp == 1 => {
                self.modrm()?;
                let (name, bits) = match op {
                    0x14 => ("pextrb", 8),
                    0x15 => ("pextrw", 16),
                    0x16 => {
                        if self.w {
                            ("pextrq", 64)
                        } else {
                            ("pextrd", 32)
                        }
                    }
                    _ => ("extractps", 32),
                };
                let mut o = String::new();
                if self.is_mem() {
                    self.w_mem(&mut o, bits, 1);
                } else {
                    self.w_g(&mut o, self.rm_n(), if bits == 64 { 64 } else { 32 });
                }
                o.push_str(", ");
                self.w_xreg(&mut o, 0);
                let v = self.u8()?;
                o.push_str(", ");
                w_uimm(&mut o, v as u64);
                let n = self.vname(name);
                let c = if self.is_mem() { self.side(true) } else { Class::Vector };
                self.fin(&n, o, c)?
            }
            (3, 0x20 | 0x22) if pp == 1 => {
                self.modrm()?;
                let (name, bits) = match op {
                    0x20 => ("pinsrb", 8),
                    _ => {
                        if self.w {
                            ("pinsrq", 64)
                        } else {
                            ("pinsrd", 32)
                        }
                    }
                };
                let mut o = String::new();
                self.w_xreg(&mut o, 0);
                if vx {
                    o.push_str(", ");
                    self.w_vvvv(&mut o, 0);
                }
                o.push_str(", ");
                if self.is_mem() {
                    self.w_mem(&mut o, bits, 1);
                } else {
                    self.w_g(&mut o, self.rm_n(), if bits == 64 { 64 } else { 32 });
                }
                let v = self.u8()?;
                o.push_str(", ");
                w_uimm(&mut o, v as u64);
                let n = self.vname(name);
                self.fin(&n, o, Class::Vector)?
            }
            (3, 0x4A | 0x4B | 0x4C) if pp == 1 && vx => {
                self.modrm()?;
                let k = self.vl;
                let mut o = String::new();
                self.w_xreg(&mut o, k);
                o.push_str(", ");
                self.w_vvvv(&mut o, k);
                o.push_str(", ");
                self.w_xrm(&mut o, k, 128 << k);
                let v = self.u8()?;
                o.push_str(", ");
                self.w_v(&mut o, v >> 4, k);
                let n = self.vname(["blendvps", "blendvpd", "pblendvb"][(op - 0x4A) as usize]);
                self.fin(&n, o, Class::Vector)?
            }
            (2, 0x7A | 0x7B | 0x7C) if pp == 1 && self.evex => {
                self.modrm()?;
                let (name, bits) = match op {
                    0x7A => ("pbroadcastb", 32),
                    0x7B => ("pbroadcastw", 32),
                    _ => {
                        if self.w {
                            ("pbroadcastq", 64)
                        } else {
                            ("pbroadcastd", 32)
                        }
                    }
                };
                if self.is_mem() {
                    return Err(Bad::Inval);
                }
                let mut o = String::new();
                self.w_xreg(&mut o, self.vl);
                self.w_mask(&mut o);
                o.push_str(", ");
                self.w_g(&mut o, self.rm_n(), bits);
                let n = self.vname(name);
                self.fin(&n, o, Class::MovReg)?
            }
            (1, 0x41 | 0x42 | 0x44 | 0x45 | 0x46 | 0x47 | 0x4A | 0x4B | 0x90 | 0x91 | 0x92 | 0x93 | 0x98 | 0x99) if self.vex => return self.kop(pp, op).map(Some),
            (3, 0x30..=0x33) if self.vex && pp == 1 => {
                self.modrm()?;
                if self.is_mem() {
                    return self.unk(1).map(Some);
                }
                let name = match (op, self.w) {
                    (0x30, false) => "kshiftrb",
                    (0x30, true) => "kshiftrw",
                    (0x31, false) => "kshiftrd",
                    (0x31, true) => "kshiftrq",
                    (0x32, false) => "kshiftlb",
                    (0x32, true) => "kshiftlw",
                    (0x33, false) => "kshiftld",
                    _ => "kshiftlq",
                };
                let mut o = String::new();
                self.w_v(&mut o, self.reg_n(), 4);
                o.push_str(", ");
                self.w_v(&mut o, self.rm_n(), 4);
                let v = self.u8()?;
                o.push_str(", ");
                w_uimm(&mut o, v as u64);
                self.fin(name, o, Class::Alu)?
            }
            _ => return Ok(None),
        };
        Ok(Some(r))
    }

    /// AVX-512 opmask instructions (VEX encoded): kmov, kand, kortest, ...
    fn kop(&mut self, pp: u8, op: u8) -> R<Insn> {
        self.modrm()?;
        // element width letter: w (none, W0), b (66, W0), q (none, W1), d (66, W1)
        let l = match (pp, self.w) {
            (0, false) => 'w',
            (1, false) => 'b',
            (0, true) => 'q',
            (1, true) => 'd',
            _ => ' ',
        };
        let mut o = String::new();
        let kk = |s: &Self, o: &mut String, n: u8| s.w_v(o, n, 4);
        match op {
            0x90 | 0x91 => {
                if l == ' ' {
                    return Err(Bad::Unk);
                }
                let bits = match l {
                    'w' => 16,
                    'b' => 8,
                    'q' => 64,
                    _ => 32,
                };
                let st = op == 0x91;
                if st && self.is_reg() {
                    return Err(Bad::Unk);
                }
                if st {
                    self.w_mem(&mut o, bits, 1);
                    o.push_str(", ");
                    kk(self, &mut o, self.reg_n());
                } else {
                    kk(self, &mut o, self.reg_n());
                    o.push_str(", ");
                    if self.is_mem() {
                        self.w_mem(&mut o, bits, 1);
                    } else {
                        kk(self, &mut o, self.rm_n());
                    }
                }
                let c = self.mv_cls(st);
                self.fin(&format!("kmov{}", l), o, c)
            }
            0x92 | 0x93 => {
                // kmov k, r / kmov r, k: the letter comes from the prefix F2 (d, q) or none / 66 (w, b)
                let (l, bits) = match (pp, self.w) {
                    (0, false) => ('w', 32),
                    (1, false) => ('b', 32),
                    (3, false) => ('d', 32),
                    (3, true) => ('q', 64),
                    _ => return Err(Bad::Unk),
                };
                if self.is_mem() {
                    return Err(Bad::Unk);
                }
                if op == 0x92 {
                    kk(self, &mut o, self.reg_n());
                    o.push_str(", ");
                    self.w_g(&mut o, self.rm_n(), bits);
                } else {
                    self.w_g(&mut o, self.reg_n(), bits);
                    o.push_str(", ");
                    kk(self, &mut o, self.rm_n());
                }
                self.fin(&format!("kmov{}", l), o, Class::MovReg)
            }
            _ => {
                if l == ' ' || self.is_mem() {
                    return Err(Bad::Unk);
                }
                let (stem, three, cls) = match op {
                    0x41 => ("kand", true, Class::Alu),
                    0x42 => ("kandn", true, Class::Alu),
                    0x44 => ("knot", false, Class::Alu),
                    0x45 => ("kor", true, Class::Alu),
                    0x46 => ("kxnor", true, Class::Alu),
                    0x47 => ("kxor", true, Class::Alu),
                    0x4A => ("kadd", true, Class::Alu),
                    0x4B => ("kunpck", true, Class::Alu),
                    0x98 => ("kortest", false, Class::Cmp),
                    _ => ("ktest", false, Class::Cmp),
                };
                let name = if op == 0x4B {
                    match (pp, self.w) {
                        (1, false) => "kunpckbw".to_string(),
                        (0, false) => "kunpckwd".to_string(),
                        (0, true) => "kunpckdq".to_string(),
                        _ => return Err(Bad::Unk),
                    }
                } else {
                    format!("{}{}", stem, l)
                };
                kk(self, &mut o, self.reg_n());
                if three {
                    o.push_str(", ");
                    kk(self, &mut o, self.vvvv);
                }
                o.push_str(", ");
                kk(self, &mut o, self.rm_n());
                self.fin(&name, o, cls)
            }
        }
    }

    /// BMI1 / BMI2 (VEX-encoded general purpose instructions).
    fn bmi(&mut self, pp: u8, op: u8) -> R<Insn> {
        self.modrm()?;
        let bits = if self.w { 64 } else { 32 };
        let by = |s: &Self, o: &mut String| s.w_g(o, s.vvvv, bits);
        let mut o = String::new();
        let (n, cls) = match (op, pp) {
            (0xF2, 0) => ("andn", Class::Alu),
            (0xF3, 0) => {
                let n = match self.reg {
                    1 => "blsr",
                    2 => "blsmsk",
                    3 => "blsi",
                    _ => return self.unk(0),
                };
                by(self, &mut o);
                o.push_str(", ");
                self.w_rm(&mut o, bits);
                return self.fin_int(n, o, Class::Alu, false);
            }
            (0xF5, 0) => ("bzhi", Class::Alu),
            (0xF5, 2) => ("pext", Class::Alu),
            (0xF5, 3) => ("pdep", Class::Alu),
            (0xF6, 3) => ("mulx", Class::Mul),
            (0xF7, 0) => ("bextr", Class::Alu),
            (0xF7, 1) => ("shlx", Class::Alu),
            (0xF7, 2) => ("sarx", Class::Alu),
            (0xF7, 3) => ("shrx", Class::Alu),
            _ => return self.unk(0),
        };
        self.w_reg(&mut o, bits);
        o.push_str(", ");
        if matches!((op, pp), (0xF5, 0) | (0xF7, _)) {
            self.w_rm(&mut o, bits);
            o.push_str(", ");
            by(self, &mut o);
        } else {
            by(self, &mut o);
            o.push_str(", ");
            self.w_rm(&mut o, bits);
        }
        self.fin_int(n, o, cls, false)
    }

    // ------------------------------------------------------------------------ VEX / EVEX

    fn vexp(&mut self, first: u8) -> R<Insn> {
        let b1 = self.u8()?;
        let (map, pp);
        self.vex = true;
        if first == 0xC5 {
            self.rr = (!b1 >> 7) & 1;
            map = 1;
            self.vvvv = (!b1 >> 3) & 15;
            self.vl = (b1 >> 2) & 1;
            pp = b1 & 3;
        } else {
            let b2 = self.u8()?;
            self.rr = (!b1 >> 7) & 1;
            self.rx = (!b1 >> 6) & 1;
            self.rb = (!b1 >> 5) & 1;
            map = b1 & 31;
            self.w = b2 & 0x80 != 0;
            self.vvvv = (!b2 >> 3) & 15;
            self.vl = (b2 >> 2) & 1;
            pp = b2 & 3;
        }
        let op = self.u8()?;
        if !(1..=3).contains(&map) {
            return Err(Bad::Inval);
        }
        self.vec_op(map, pp, op)
    }

    fn evexp(&mut self) -> R<Insn> {
        let (p0, p1, p2) = (self.u8()?, self.u8()?, self.u8()?);
        if p0 & 0x08 != 0 || p1 & 4 == 0 {
            return Err(Bad::Inval);
        }
        let map = p0 & 7;
        if !(1..=3).contains(&map) {
            return Err(Bad::Inval);
        }
        self.evex = true;
        self.rr = (!p0 >> 7) & 1;
        self.rx = (!p0 >> 6) & 1;
        self.rb = (!p0 >> 5) & 1;
        self.rr2 = (!p0 >> 4) & 1;
        self.w = p1 & 0x80 != 0;
        self.vvvv = (!p1 >> 3) & 15;
        let pp = p1 & 3;
        self.zero = p2 & 0x80 != 0;
        self.ll = (p2 >> 5) & 3;
        self.bcast = p2 & 0x10 != 0;
        self.vv2 = (!p2 >> 3) & 1;
        self.aaa = p2 & 7;
        if self.ll == 3 && !self.bcast {
            return Err(Bad::Inval);
        }
        self.vl = self.ll.min(2);
        let op = self.u8()?;
        self.vec_op(map, pp, op)
    }
}

/// Where the EVEX encoding gives an opcode another meaning than the VEX / legacy table does
/// (element-size variants chosen by EVEX.W, compares that write an opmask) and the AVX-512 only
/// instructions.
fn tab_evex(map: u8, pp: u8, op: u8, w: bool) -> Option<Ent> {
    use Fm::*;
    let e = Ent::new;
    let dq = |d: &str, q: &str| if w { q.to_string() } else { d.to_string() };
    Some(match (map, pp, op) {
        (1, 1, 0x6F | 0x7F) => e(&dq("movdqa32", "movdqa64"), if op == 0x6F { Rm } else { Mr }).mv(),
        (1, 2, 0x6F | 0x7F) => e(&dq("movdqu32", "movdqu64"), if op == 0x6F { Rm } else { Mr }).mv(),
        (1, 3, 0x6F | 0x7F) => e(&dq("movdqu8", "movdqu16"), if op == 0x6F { Rm } else { Mr }).mv(),
        (1, 1, 0xDB) => e(&dq("pandd", "pandq"), Rvm),
        (1, 1, 0xDF) => e(&dq("pandnd", "pandnq"), Rvm),
        (1, 1, 0xEB) => e(&dq("pord", "porq"), Rvm),
        (1, 1, 0xEF) => e(&dq("pxord", "pxorq"), Rvm),
        (1, 1, 0x74 | 0x75 | 0x76) => e(["pcmpeqb", "pcmpeqw", "pcmpeqd"][(op - 0x74) as usize], Rvm).k(),
        (1, 1, 0x64 | 0x65 | 0x66) => e(["pcmpgtb", "pcmpgtw", "pcmpgtd"][(op - 0x64) as usize], Rvm).k(),
        (2, 1, 0x29) => e("pcmpeqq", Rvm).k(),
        (2, 1, 0x37) => e("pcmpgtq", Rvm).k(),
        (1, 1, 0xE2) if w => e("psraq", Rvm).src(Rk::X).ms(Ms::B(128)),
        (2, 1, 0x40) if w => e("pmullq", Rvm),
        (2, 1, 0x39) => e(&dq("pminsd", "pminsq"), Rvm),
        (2, 1, 0x3B) => e(&dq("pminud", "pminuq"), Rvm),
        (2, 1, 0x3D) => e(&dq("pmaxsd", "pmaxsq"), Rvm),
        (2, 1, 0x3F) => e(&dq("pmaxud", "pmaxuq"), Rvm),
        (2, 1, 0x1F) if w => e("pabsq", Rm),
        (2, 1, 0x36) => e(&dq("permd", "permq"), Rvm),
        (2, 1, 0x16) => e(&dq("permps", "permpd"), Rvm),
        (2, 1, 0x75) => e(&dq("permi2b", "permi2w"), Rvm),
        (2, 1, 0x76) => e(&dq("permi2d", "permi2q"), Rvm),
        (2, 1, 0x77) => e(&dq("permi2ps", "permi2pd"), Rvm),
        (2, 1, 0x7D) => e(&dq("permt2b", "permt2w"), Rvm),
        (2, 1, 0x7E) => e(&dq("permt2d", "permt2q"), Rvm),
        (2, 1, 0x7F) => e(&dq("permt2ps", "permt2pd"), Rvm),
        (2, 1, 0x8D) => e(&dq("permb", "permw"), Rvm),
        (2, 1, 0x64) => e(&dq("pblendmd", "pblendmq"), Rvm),
        (2, 1, 0x65) => e(&dq("blendmps", "blendmpd"), Rvm),
        (2, 1, 0x66) => e(&dq("pblendmb", "pblendmw"), Rvm),
        (2, 1, 0x59) if !w => e("broadcasti32x2", Rm).src(Rk::X).ms(Ms::B(64)).mv(),
        (2, 1, 0x19) if !w => e("broadcastf32x2", Rm).src(Rk::X).ms(Ms::B(64)).mv(),
        (2, 1, 0x1A) => e(&dq("broadcastf32x4", "broadcastf64x2"), Rm).src(Rk::X).ms(Ms::B(128)).mv().mem(),
        (2, 1, 0x1B) => e(&dq("broadcastf32x8", "broadcastf64x4"), Rm).src(Rk::H).ms(Ms::B(256)).mv().mem(),
        (2, 1, 0x5A) => e(&dq("broadcasti32x4", "broadcasti64x2"), Rm).src(Rk::X).ms(Ms::B(128)).mv().mem(),
        (2, 1, 0x5B) => e(&dq("broadcasti32x8", "broadcasti64x4"), Rm).src(Rk::H).ms(Ms::B(256)).mv().mem(),
        (2, 1, 0x4C) => e(&dq("rcp14ps", "rcp14pd"), Rm),
        (2, 1, 0x4E) => e(&dq("rsqrt14ps", "rsqrt14pd"), Rm),
        (2, 1, 0x2C) => e(&dq("scalefps", "scalefpd"), Rvm),
        (2, 1, 0x54) => e(&dq("popcntb", "popcntw"), Rm),
        (2, 1, 0x55) => e(&dq("popcntd", "popcntq"), Rm),
        (2, 1, 0x44) => e(&dq("plzcntd", "plzcntq"), Rm),
        (2, 1, 0xC4) => e(&dq("pconflictd", "pconflictq"), Rm),
        (2, 1, 0x50) => e("pdpbusd", Rvm),
        (2, 1, 0x51) => e("pdpbusds", Rvm),
        (2, 1, 0x52) => e("pdpwssd", Rvm),
        (2, 1, 0x53) => e("pdpwssds", Rvm),
        (2, 1, 0xB4) if w => e("pmadd52luq", Rvm),
        (2, 1, 0xB5) if w => e("pmadd52huq", Rvm),
        (2, 1, 0x27) => e(&dq("ptestmd", "ptestmq"), Rvm).k(),
        (2, 2, 0x27) => e(&dq("ptestnmd", "ptestnmq"), Rvm).k(),
        (2, 1, 0x26) => e(&dq("ptestmb", "ptestmw"), Rvm).k(),
        (2, 2, 0x26) => e(&dq("ptestnmb", "ptestnmw"), Rvm).k(),
        (2, 2, 0x10..=0x15 | 0x20..=0x25 | 0x30..=0x35) => {
            let base = ["wb", "db", "qb", "dw", "qw", "qd"][(op & 15) as usize];
            let pre = match op >> 4 {
                1 => "pmovus",
                2 => "pmovs",
                _ => "pmov",
            };
            let (dk, ms) = match base {
                "wb" | "dw" | "qd" => (Rk::H, Ms::H),
                "db" | "qw" => (Rk::X, Ms::Q),
                _ => (Rk::X, Ms::E),
            };
            e(&format!("{}{}", pre, base), Mr).dst(dk).ms(ms)
        }
        (3, 1, 0x25) => e(&dq("pternlogd", "pternlogq"), RvmI),
        (3, 1, 0x03) => e(&dq("alignd", "alignq"), RvmI),
        (3, 1, 0x1E) => e(&dq("pcmpud", "pcmpuq"), RvmI).k(),
        (3, 1, 0x1F) => e(&dq("pcmpd", "pcmpq"), RvmI).k(),
        (3, 1, 0x3E) => e(&dq("pcmpub", "pcmpuw"), RvmI).k(),
        (3, 1, 0x3F) => e(&dq("pcmpb", "pcmpw"), RvmI).k(),
        (3, 1, 0x43) => e(&dq("shufi32x4", "shufi64x2"), RvmI),
        (3, 1, 0x23) => e(&dq("shuff32x4", "shuff64x2"), RvmI),
        (3, 1, 0x18) => e(&dq("insertf32x4", "insertf64x2"), RvmI).src(Rk::X).ms(Ms::B(128)),
        (3, 1, 0x1A) => e(&dq("insertf32x8", "insertf64x4"), RvmI).src(Rk::H).ms(Ms::B(256)),
        (3, 1, 0x38) => e(&dq("inserti32x4", "inserti64x2"), RvmI).src(Rk::X).ms(Ms::B(128)),
        (3, 1, 0x3A) => e(&dq("inserti32x8", "inserti64x4"), RvmI).src(Rk::H).ms(Ms::B(256)),
        (3, 1, 0x19) => e(&dq("extractf32x4", "extractf64x2"), MrI).dst(Rk::X).ms(Ms::B(128)),
        (3, 1, 0x1B) => e(&dq("extractf32x8", "extractf64x4"), MrI).dst(Rk::H).ms(Ms::B(256)),
        (3, 1, 0x39) => e(&dq("extracti32x4", "extracti64x2"), MrI).dst(Rk::X).ms(Ms::B(128)),
        (3, 1, 0x3B) => e(&dq("extracti32x8", "extracti64x4"), MrI).dst(Rk::H).ms(Ms::B(256)),
        (3, 1, 0x08) => e("rndscaleps", RmI),
        (3, 1, 0x09) => e("rndscalepd", RmI),
        (3, 1, 0x0A) => e("rndscaless", RvmI).sc(false),
        (3, 1, 0x0B) => e("rndscalesd", RvmI).sc(true),
        // 64 bit integer / quad conversions are not spelled
        (1, 0, 0x5B) | (1, 2 | 3, 0xE6) if w => Ent { n: String::new(), ..e("", Rm) },
        _ => return None,
    })
}
