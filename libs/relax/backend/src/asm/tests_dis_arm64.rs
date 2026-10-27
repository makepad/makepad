//! Tests of the AArch64 decoder: a round trip with the encoder of this module (decode a word,
//! assemble its text, get the word back), the canonical spelling of the aliases, and a corpus run
//! over a real binary (`DIS_CORPUS=<arm64 Mach-O or ELF>`, optionally compared with the listing of
//! a system disassembler, `DIS_REF=<file of "addr:\ttext" lines>`).
use super::dis::{decode, Class, Insn};
use super::tests_arm64::{fp_corpus, neon_corpus, BRANCH, CORE, MEM};
use super::*;
use std::collections::HashMap;

fn asm_word(text: &str) -> Result<u32, String> {
    let o = assemble(Arch::Arm64, Format::Elf, Kind::Global, &[Piece::Text(text.to_string())], &[], false).map_err(|e| e.to_string())?;
    let b = &o.sections[0].bytes;
    if b.len() != 4 {
        return Err(format!("{} bytes", b.len()));
    }
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn dec(w: u32) -> Insn {
    decode(Arch::Arm64, &w.to_le_bytes(), 0).remove(0)
}

fn lines(src: &str) -> Vec<String> {
    src.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty() && !l.ends_with(':')).collect()
}

/// Every line of the encoder's own corpora, assembled, decoded, the decoded text assembled again:
/// the word must come back, and decoding it again must give the same text.
#[test]
fn round_trip_encoder_corpora() {
    let mut all = lines(CORE);
    all.extend(lines(BRANCH));
    all.extend(lines(MEM));
    all.extend(lines(&fp_corpus()));
    all.extend(lines(&neon_corpus()));
    let mut unknown = Vec::new();
    let mut differ = Vec::new();
    let mut unstable = Vec::new();
    let mut n = 0;
    for l in &all {
        let Ok(w) = asm_word(l) else { continue };
        n += 1;
        let i = dec(w);
        if i.class == Class::Unknown {
            unknown.push(format!("{:08x} {}", w, l));
            continue;
        }
        let t = i.text();
        match asm_word(&t) {
            Ok(w2) if w2 == w => {}
            Ok(w2) => differ.push(format!("{} -> {:08x} -> `{}` -> {:08x}", l, w, t, w2)),
            Err(_) if t.contains("0xffffffff") => {}
            Err(e) => differ.push(format!("{} -> {:08x} -> `{}` rejected: {}", l, w, t, e)),
        }
        if let Ok(w2) = asm_word(&t) {
            let t2 = dec(w2).text();
            if t2 != t {
                unstable.push(format!("{} -> `{}` -> `{}`", l, t, t2));
            }
        }
    }
    println!("{} words, {} unknown, {} differ, {} unstable", n, unknown.len(), differ.len(), unstable.len());
    for x in unknown.iter().take(60) {
        println!("UNKNOWN {}", x);
    }
    for x in differ.iter().take(80) {
        println!("DIFFER {}", x);
    }
    for x in unstable.iter().take(20) {
        println!("UNSTABLE {}", x);
    }
    assert!(n > 1500);
    assert!(differ.is_empty() && unstable.is_empty(), "round trip failures");
    assert!(unknown.is_empty(), "unknown words");
}

/// (source text, canonical decoded text, class)
const CANON: &[(&str, &str, Class)] = &[
    ("mov x0, x1", "mov x0, x1", Class::MovReg),
    ("orr x0, xzr, x1", "mov x0, x1", Class::MovReg),
    ("mov x29, sp", "mov x29, sp", Class::MovReg),
    ("add x0, sp, #0", "mov x0, sp", Class::MovReg),
    ("add x0, x1, #1", "add x0, x1, #1", Class::Alu),
    ("sub sp, sp, #32", "sub sp, sp, #32", Class::Alu),
    ("add x0, x1, #1, lsl #12", "add x0, x1, #1, lsl #12", Class::Alu),
    ("cmp x0, #1", "cmp x0, #1", Class::Cmp),
    ("subs xzr, x0, x1", "cmp x0, x1", Class::Cmp),
    ("cmn w0, #4", "cmn w0, #4", Class::Cmp),
    ("tst x0, #0xff", "tst x0, #0xff", Class::Cmp),
    ("ands xzr, x1, x2", "tst x1, x2", Class::Cmp),
    ("neg x0, x1", "neg x0, x1", Class::Alu),
    ("mvn x0, x1", "mvn x0, x1", Class::Alu),
    ("add x0, x1, x2, lsl #3", "add x0, x1, x2, lsl #3", Class::Addr),
    ("add x0, x1, w2, sxtw #2", "add x0, x1, w2, sxtw #2", Class::Addr),
    ("add x0, x1, x2", "add x0, x1, x2", Class::Alu),
    ("movz x0, #1", "mov x0, #1", Class::MovImm),
    ("movz x0, #0, lsl #16", "movz x0, #0, lsl #16", Class::MovImm),
    ("movn x0, #0", "mov x0, #-1", Class::MovImm),
    ("movn w0, #0xffff", "movn w0, #65535", Class::MovImm),
    ("movk x0, #4660, lsl #16", "movk x0, #4660, lsl #16", Class::MovImm),
    ("mov x0, #0x5555555555555555", "mov x0, #0x5555555555555555", Class::MovImm),
    ("lsl x0, x1, #3", "lsl x0, x1, #3", Class::Alu),
    ("lsr w0, w1, #3", "lsr w0, w1, #3", Class::Alu),
    ("asr x0, x1, #63", "asr x0, x1, #63", Class::Alu),
    ("ubfx x0, x1, #4, #8", "ubfx x0, x1, #4, #8", Class::Alu),
    ("sbfx w0, w1, #4, #8", "sbfx w0, w1, #4, #8", Class::Alu),
    ("ubfiz x0, x1, #4, #8", "ubfiz x0, x1, #4, #8", Class::Alu),
    ("bfi x0, x1, #4, #8", "bfi x0, x1, #4, #8", Class::Alu),
    ("bfxil x0, x1, #4, #8", "bfxil x0, x1, #4, #8", Class::Alu),
    ("uxtb w0, w1", "uxtb w0, w1", Class::MovReg),
    ("sxtw x0, w1", "sxtw x0, w1", Class::MovReg),
    ("sxth x0, w1", "sxth x0, w1", Class::MovReg),
    ("ror x0, x1, #5", "ror x0, x1, #5", Class::Alu),
    ("extr x0, x1, x2, #5", "extr x0, x1, x2, #5", Class::Alu),
    ("cset w0, eq", "cset w0, eq", Class::Select),
    ("csetm x0, lt", "csetm x0, lt", Class::Select),
    ("cinc x0, x1, ne", "cinc x0, x1, ne", Class::Select),
    ("csinc x0, x1, x2, ne", "csinc x0, x1, x2, ne", Class::Select),
    ("csel x0, x1, x2, hs", "csel x0, x1, x2, hs", Class::Select),
    ("cneg x0, x1, mi", "cneg x0, x1, mi", Class::Select),
    ("ccmp x0, x1, #4, ne", "ccmp x0, x1, #4, ne", Class::Cmp),
    ("ccmn w0, #3, #0, eq", "ccmn w0, #3, #0, eq", Class::Cmp),
    ("mul x0, x1, x2", "mul x0, x1, x2", Class::Mul),
    ("mneg x0, x1, x2", "mneg x0, x1, x2", Class::Mul),
    ("madd x0, x1, x2, x3", "madd x0, x1, x2, x3", Class::Mul),
    ("umulh x0, x1, x2", "umulh x0, x1, x2", Class::Mul),
    ("smull x0, w1, w2", "smull x0, w1, w2", Class::Mul),
    ("umaddl x0, w1, w2, x3", "umaddl x0, w1, w2, x3", Class::Mul),
    ("udiv x0, x1, x2", "udiv x0, x1, x2", Class::Div),
    ("sdiv w0, w1, w2", "sdiv w0, w1, w2", Class::Div),
    ("ldr x0, [x1]", "ldr x0, [x1]", Class::Load),
    ("ldr x0, [x1, #8]", "ldr x0, [x1, #8]", Class::Load),
    ("ldr x0, [sp, #8]", "ldr x0, [sp, #8]", Class::StackLoad),
    ("str x0, [x29, #-8]", "stur x0, [x29, #-8]", Class::StackStore),
    ("str w0, [x1, x2, lsl #2]", "str w0, [x1, x2, lsl #2]", Class::Store),
    ("ldr w0, [x1, w2, uxtw #2]", "ldr w0, [x1, w2, uxtw #2]", Class::Load),
    ("ldrb w0, [x1, x2]", "ldrb w0, [x1, x2]", Class::Load),
    ("ldrsw x0, [x1, #4]", "ldrsw x0, [x1, #4]", Class::Load),
    ("ldur x0, [x1, #-3]", "ldur x0, [x1, #-3]", Class::Load),
    ("ldr x0, [x1, #8]!", "ldr x0, [x1, #8]!", Class::Load),
    ("ldr x0, [x1], #8", "ldr x0, [x1], #8", Class::Load),
    ("ldr q0, [x1, #16]", "ldr q0, [x1, #16]", Class::Load),
    ("str d0, [sp, #8]", "str d0, [sp, #8]", Class::StackStore),
    ("stp x29, x30, [sp, #-16]!", "stp x29, x30, [sp, #-16]!", Class::PushPop),
    ("ldp x29, x30, [sp], #16", "ldp x29, x30, [sp], #16", Class::PushPop),
    ("stp x19, x20, [sp, #16]", "stp x19, x20, [sp, #16]", Class::StackStore),
    ("ldp x0, x1, [x2]", "ldp x0, x1, [x2]", Class::Load),
    ("ldp q0, q1, [x2, #32]", "ldp q0, q1, [x2, #32]", Class::Load),
    ("ldpsw x0, x1, [x2, #8]", "ldpsw x0, x1, [x2, #8]", Class::Load),
    ("ldxr x0, [x1]", "ldxr x0, [x1]", Class::Atomic),
    ("stlxr w2, x0, [x1]", "stlxr w2, x0, [x1]", Class::Atomic),
    ("ldar w0, [x1]", "ldar w0, [x1]", Class::Atomic),
    ("stlrb w0, [x1]", "stlrb w0, [x1]", Class::Atomic),
    ("ldaxp x0, x1, [x2]", "ldaxp x0, x1, [x2]", Class::Atomic),
    ("ldadd x0, x1, [x2]", "ldadd x0, x1, [x2]", Class::Atomic),
    ("ldaddal w0, w1, [x2]", "ldaddal w0, w1, [x2]", Class::Atomic),
    ("stadd w0, [x2]", "stadd w0, [x2]", Class::Atomic),
    ("swpa x0, x1, [x2]", "swpa x0, x1, [x2]", Class::Atomic),
    ("casal x0, x1, [x2]", "casal x0, x1, [x2]", Class::Atomic),
    ("casp x0, x1, x2, x3, [x4]", "casp x0, x1, x2, x3, [x4]", Class::Atomic),
    ("dmb ish", "dmb ish", Class::Atomic),
    ("dsb sy", "dsb sy", Class::Atomic),
    ("isb", "isb", Class::Atomic),
    ("nop", "nop", Class::Nop),
    ("brk #0x1", "brk #0x1", Class::Trap),
    ("udf #0", "udf #0x0", Class::Trap),
    ("ret", "ret", Class::Ret),
    ("ret x1", "ret x1", Class::Ret),
    ("br x16", "br x16", Class::IndirectBranch),
    ("blr x8", "blr x8", Class::IndirectCall),
    ("b #8", "b 0x8", Class::Branch),
    ("bl #-4", "bl 0xfffffffffffffffc", Class::Call),
    ("b.ne #12", "b.ne 0xc", Class::CondBranch),
    ("cbz x0, #8", "cbz x0, 0x8", Class::CondBranch),
    ("cbnz w3, #8", "cbnz w3, 0x8", Class::CondBranch),
    ("tbz w0, #3, #8", "tbz w0, #3, 0x8", Class::CondBranch),
    ("tbnz x0, #40, #8", "tbnz x0, #40, 0x8", Class::CondBranch),
    ("mrs x0, tpidr_el0", "mrs x0, tpidr_el0", Class::Other),
    ("msr daifset, #2", "msr daifset, #2", Class::Other),
    ("fadd d0, d1, d2", "fadd d0, d1, d2", Class::Fp),
    ("fmul s0, s1, s2", "fmul s0, s1, s2", Class::Fp),
    ("fmadd d0, d1, d2, d3", "fmadd d0, d1, d2, d3", Class::Fp),
    ("fcvt d0, s1", "fcvt d0, s1", Class::Fp),
    ("scvtf d0, x1", "scvtf d0, x1", Class::Fp),
    ("fcvtzs w0, s1", "fcvtzs w0, s1", Class::Fp),
    ("fcmp d0, d1", "fcmp d0, d1", Class::Fp),
    ("fcmp s0, #0.0", "fcmp s0, #0.0", Class::Fp),
    ("fcsel d0, d1, d2, gt", "fcsel d0, d1, d2, gt", Class::Select),
    ("fmov d0, d1", "fmov d0, d1", Class::MovReg),
    ("fmov x0, d1", "fmov x0, d1", Class::MovReg),
    ("fmov d0, #1.0", "fmov d0, #1.00000000", Class::MovImm),
    ("fsqrt s0, s1", "fsqrt s0, s1", Class::Fp),
    ("add v0.4s, v1.4s, v2.4s", "add v0.4s, v1.4s, v2.4s", Class::Vector),
    ("fadd v0.2d, v1.2d, v2.2d", "fadd v0.2d, v1.2d, v2.2d", Class::Vector),
    ("orr v0.16b, v1.16b, v1.16b", "mov v0.16b, v1.16b", Class::Vector),
    ("eor v0.16b, v1.16b, v2.16b", "eor v0.16b, v1.16b, v2.16b", Class::Vector),
    ("movi v0.2d, #0", "movi v0.2d, #0x0", Class::MovImm),
    ("movi v0.4s, #1, lsl #8", "movi v0.4s, #1, lsl #8", Class::MovImm),
    ("dup v0.4s, w1", "dup v0.4s, w1", Class::Vector),
    ("dup v0.4s, v1.s[2]", "dup v0.4s, v1.s[2]", Class::Vector),
    ("umov w0, v1.b[3]", "umov w0, v1.b[3]", Class::Vector),
    ("umov x0, v1.d[1]", "mov x0, v1.d[1]", Class::Vector),
    ("smov x0, v1.h[1]", "smov x0, v1.h[1]", Class::Vector),
    ("ins v0.s[1], w2", "mov v0.s[1], w2", Class::Vector),
    ("zip1 v0.4s, v1.4s, v2.4s", "zip1 v0.4s, v1.4s, v2.4s", Class::Vector),
    ("ext v0.16b, v1.16b, v2.16b, #3", "ext v0.16b, v1.16b, v2.16b, #3", Class::Vector),
    ("tbl v0.16b, {v1.16b, v2.16b}, v3.16b", "tbl v0.16b, { v1.16b, v2.16b }, v3.16b", Class::Vector),
    ("ushll v0.8h, v1.8b, #3", "ushll v0.8h, v1.8b, #3", Class::Vector),
    ("uxtl v0.8h, v1.8b", "uxtl v0.8h, v1.8b", Class::Vector),
    ("shl v0.4s, v1.4s, #3", "shl v0.4s, v1.4s, #3", Class::Vector),
    ("addv s0, v1.4s", "addv s0, v1.4s", Class::Vector),
    ("cnt v0.16b, v1.16b", "cnt v0.16b, v1.16b", Class::Vector),
    ("mul v0.4s, v1.4s, v2.s[3]", "mul v0.4s, v1.4s, v2.s[3]", Class::Vector),
    ("fmla v0.2d, v1.2d, v2.d[1]", "fmla v0.2d, v1.2d, v2.d[1]", Class::Vector),
    ("ld1 {v0.16b}, [x0]", "ld1 { v0.16b }, [x0]", Class::Load),
    ("ld1 {v0.4s, v1.4s}, [x0], #32", "ld1 { v0.4s, v1.4s }, [x0], #32", Class::Load),
    ("st1 {v0.s}[1], [x0]", "st1 { v0.s }[1], [x0]", Class::Store),
    ("ld1r {v0.4s}, [x0]", "ld1r { v0.4s }, [x0]", Class::Load),
    ("ld2 {v0.8b, v1.8b}, [sp]", "ld2 { v0.8b, v1.8b }, [sp]", Class::StackLoad),
    ("add d0, d1, d2", "add d0, d1, d2", Class::Fp),
        ("faddp d0, v1.2d", "faddp d0, v1.2d", Class::Fp),
];

#[test]
fn canonical_spellings_and_classes() {
    let mut bad = Vec::new();
    for (src, want, class) in CANON {
        let w = match asm_word(src) {
            Ok(w) => w,
            Err(e) => {
                bad.push(format!("`{}` does not assemble: {}", src, e));
                continue;
            }
        };
        let i = dec(w);
        let t = i.text();
        if t != *want || i.class != *class {
            bad.push(format!("`{}` {:08x}: got `{}` {:?}, want `{}` {:?}", src, w, t, i.class, want, class));
        }
    }
    for b in &bad {
        println!("{}", b);
    }
    assert!(bad.is_empty());
}

#[test]
fn flags_and_targets() {
    // b +8 at 0x1000, adrp, literal ldr, stack accesses
    let code: Vec<u8> = [0x1400_0002u32, 0x9000_0000, 0x5800_0040, 0xF940_03E0, 0xF900_23A1, 0xD65F_03C0].iter().flat_map(|w| w.to_le_bytes()).collect();
    let v = decode(Arch::Arm64, &code, 0x1000);
    assert_eq!(v.len(), 6);
    assert_eq!((v[0].mnem.as_str(), v[0].ops.as_str(), v[0].target), ("b", "0x1008", Some(0x1008)));
    assert_eq!(v[1].pc_ref, Some(0x1000));
    assert_eq!(v[2].pc_ref, Some(0x1010));
    assert!(v[2].mem);
    assert!(v[3].mem && v[3].stack && v[3].class == Class::StackLoad);
    assert!(v[4].mem && v[4].stack && v[4].class == Class::StackStore);
    assert_eq!(v[5].class, Class::Ret);
    assert_eq!(v[5].off, 20);
}

#[test]
fn unknown_and_tail_bytes() {
    // 0x00200000 is reserved; SVE (0x04200000) is not decoded
    let code: Vec<u8> = [0x0020_0000u32, 0x0420_0000, 0xD503_201F].iter().flat_map(|w| w.to_le_bytes()).chain([1u8, 2]).collect();
    let v = decode(Arch::Arm64, &code, 0);
    assert_eq!(v.len(), 5);
    assert_eq!((v[0].class, v[0].mnem.as_str(), v[0].ops.as_str(), v[0].len), (Class::Unknown, ".word", "0x00200000", 4));
    assert_eq!(v[1].class, Class::Unknown);
    assert_eq!(v[2].class, Class::Nop);
    assert_eq!((v[3].mnem.as_str(), v[3].len, v[4].off), (".byte", 1, 13));
}

/// A sweep over structured words: every word of a few thousand random-ish patterns decodes to
/// something (no panic) and whatever the assembler accepts of the text assembles back.
#[test]
fn no_panics_and_stable_text() {
    let mut x: u64 = 0x1234_5678_9abc_def1;
    let mut n = 0;
    let mut bad = Vec::new();
    for _ in 0..400_000 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let w = (x >> 16) as u32;
        let i = dec(w);
        if i.class == Class::Unknown {
            continue;
        }
        n += 1;
        let t = i.text();
        // pc-relative operands are absolute addresses here; the assembler reads them as offsets
        if let Ok(w2) = asm_word(&t) {
            if dec(w2).text() != t && !(matches!(i.class, Class::Branch | Class::Call | Class::CondBranch) || i.pc_ref.is_some()) {
                bad.push(format!("{:08x} `{}` -> {:08x}", w, t, w2));
            }
        }
    }
    println!("{} decoded of 400000 random words, {} not reproduced", n, bad.len());
    for b in bad.iter().take(60) {
        println!("{}", b);
    }
    assert!(bad.is_empty());
}

// ---------------------------------------------------------------- corpus

struct Text {
    base: u64,
    bytes: Vec<u8>,
    /// byte ranges (offsets into `bytes`) that are data inside the code
    data: Vec<(usize, usize)>,
}

fn be32(b: &[u8], o: usize) -> u32 {
    u32::from_be_bytes(b[o..o + 4].try_into().unwrap())
}
fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn le64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

fn macho_text(file: &[u8]) -> Text {
    let mut base = 0usize;
    if be32(file, 0) == 0xCAFE_BABE {
        let n = be32(file, 4) as usize;
        let mut found = false;
        for i in 0..n {
            let e = 8 + i * 20;
            if be32(file, e) == 0x0100_000C {
                base = be32(file, e + 8) as usize;
                found = true;
            }
        }
        assert!(found, "no arm64 slice in the universal binary");
    }
    let m = &file[base..];
    assert_eq!(le32(m, 0), 0xFEED_FACF, "not a 64-bit little-endian Mach-O");
    let ncmds = le32(m, 16) as usize;
    let mut off = 32;
    let (mut addr, mut size, mut foff) = (0u64, 0u64, 0u64);
    let mut dice = (0usize, 0usize);
    for _ in 0..ncmds {
        let cmd = le32(m, off);
        let sz = le32(m, off + 4) as usize;
        if cmd == 0x19 {
            let nsects = le32(m, off + 64) as usize;
            for s in 0..nsects {
                let so = off + 72 + s * 80;
                let name = &m[so..so + 16];
                let seg = &m[so + 16..so + 32];
                if name.starts_with(b"__text\0") && seg.starts_with(b"__TEXT\0") {
                    addr = le64(m, so + 32);
                    size = le64(m, so + 40);
                    foff = le32(m, so + 48) as u64;
                }
            }
        } else if cmd == 0x29 {
            dice = (le32(m, off + 8) as usize, le32(m, off + 12) as usize);
        }
        off += sz;
    }
    assert!(size > 0, "no __TEXT,__text");
    let bytes = m[foff as usize..(foff + size) as usize].to_vec();
    let mut data = Vec::new();
    let mut p = dice.0;
    while p + 8 <= dice.0 + dice.1 {
        let o = le32(m, p) as u64;
        let len = u16::from_le_bytes([m[p + 4], m[p + 5]]) as u64;
        if o >= foff && o + len <= foff + size {
            data.push(((o - foff) as usize, (o - foff + len) as usize));
        }
        p += 8;
    }
    data.sort();
    Text { base: addr, bytes, data }
}

fn elf_text_section(file: &[u8]) -> Text {
    let u16_at = |o: usize| u16::from_le_bytes([file[o], file[o + 1]]) as usize;
    let u32_at = |o: usize| le32(file, o) as usize;
    let u64_at = |o: usize| le64(file, o) as usize;
    let shoff = u64_at(0x28);
    let shentsize = u16_at(0x3A);
    let shnum = u16_at(0x3C);
    let shstrndx = u16_at(0x3E);
    let sh = |i: usize| shoff + i * shentsize;
    let stroff = u64_at(sh(shstrndx) + 0x18);
    for i in 0..shnum {
        let name_off = stroff + u32_at(sh(i));
        let end = file[name_off..].iter().position(|c| *c == 0).unwrap() + name_off;
        if &file[name_off..end] == b".text" {
            let addr = u64_at(sh(i) + 0x10) as u64;
            let off = u64_at(sh(i) + 0x18);
            let size = u64_at(sh(i) + 0x20);
            return Text { base: addr, bytes: file[off..off + size].to_vec(), data: Vec::new() };
        }
    }
    panic!("no .text in the ELF file");
}

fn load_text(path: &str) -> Text {
    let file = std::fs::read(path).unwrap();
    if file.starts_with(b"\x7fELF") {
        elf_text_section(&file)
    } else {
        macho_text(&file)
    }
}

/// The text of an instruction as both disassemblers can agree on it: whitespace goes, numbers
/// become decimal (64-bit values as signed), and the vector arrangements (which the reference
/// spells on the mnemonic) are dropped from both sides; arrangements are checked by the encoder
/// round trip.
fn norm(s: &str) -> String {
    let s = s.trim();
    let (m, rest) = match s.find(char::is_whitespace) {
        Some(p) => (&s[..p], &s[p..]),
        None => (s, ""),
    };
    let m = match m.find('.') {
        Some(p) if !m.starts_with("b.") => &m[..p],
        _ => m,
    };
    let rest: String = rest.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_lowercase();
    let mut text = format!("{}{}", m.to_ascii_lowercase(), if rest.is_empty() { String::new() } else { format!(" {}", rest) });
    if text.contains(',') && (text.starts_with("ushll") || text.starts_with("sshll")) && text.ends_with(",#0x0") {
        text = text.replace("ushll", "uxtl").replace("sshll", "sxtl").trim_end_matches(",#0x0").to_string();
    }
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < b.len() {
        let prev = if i == 0 { b' ' } else { b[i - 1] };
        if b[i] == b'0' && b.get(i + 1) == Some(&b'x') && matches!(prev, b'#' | b',' | b'[' | b'-' | b' ') {
            let mut j = i + 2;
            while j < b.len() && b[j].is_ascii_hexdigit() {
                j += 1;
            }
            match u64::from_str_radix(&text[i + 2..j], 16) {
                Ok(v) if v >= 1 << 63 && prev != b'-' => out.push_str(&(v as i64).to_string()),
                Ok(v) => out.push_str(&v.to_string()),
                Err(_) => out.push_str(&text[i..j]),
            }
            i = j;
        } else if b[i] == b'.' && prev.is_ascii_digit() {
            // `.4s` after a register number
            let mut j = i + 1;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j < b.len() && matches!(b[j], b'b' | b'h' | b's' | b'd' | b'q') && (j + 1 == b.len() || matches!(b[j + 1], b',' | b'[' | b'}')) {
                i = j + 1;
            } else {
                out.push('.');
                i += 1;
            }
        } else {
            out.push(b[i] as char);
            i += 1;
        }
    }
    out
}

/// A reference line's text with the comment dealt with: adr / adrp take the address of the
/// comment as operand, adr's relative offset becomes an address, `movi` 64-bit masks get `0x`.
fn ref_text(t: &str, addr: u64) -> String {
    let t = t.trim();
    if let Some(p) = t.find(';') {
        let (head, comment) = (&t[..p], t[p + 1..].trim());
        let m = head.split_whitespace().next().unwrap_or("");
        if m == "adr" || m == "adrp" {
            let reg = head.split_whitespace().nth(1).unwrap_or("").trim_end_matches(',');
            return format!("{} {}, {}", m, reg, comment);
        }
        return head.trim().to_string();
    }
    let m = t.split_whitespace().next().unwrap_or("");
    if m == "adr" {
        if let Some((reg, imm)) = t[3..].trim().split_once(',') {
            if let Ok(v) = imm.trim().trim_start_matches('#').parse::<i64>() {
                return format!("adr {}, 0x{:x}", reg.trim(), addr.wrapping_add(v as u64));
            }
        }
    }
    if m.starts_with("movi") {
        if let Some(p) = t.rfind('#') {
            let h = &t[p + 1..];
            if h.len() == 16 && h.bytes().all(|c| c.is_ascii_hexdigit()) {
                return format!("{}#0x{}", &t[..p], h);
            }
        }
    }
    t.to_string()
}

fn is_data_dir(t: &str) -> bool {
    t.starts_with(".long") || t.starts_with(".word") || t.starts_with(".byte") || t.starts_with(".short") || t.starts_with(".quad") || t.starts_with("<")
}

#[test]
fn corpus_decode_and_compare() {
    let Ok(path) = std::env::var("DIS_CORPUS") else {
        println!("DIS_CORPUS not set, corpus run skipped");
        return;
    };
    let t = load_text(&path);
    let limit: usize = std::env::var("DIS_LIMIT").ok().and_then(|s| s.parse().ok()).unwrap_or(usize::MAX);
    let words = (t.bytes.len() / 4).min(limit);
    let mut data_words = vec![false; words];
    for (a, b) in &t.data {
        for o in (*a / 4)..((*b + 3) / 4).min(words) {
            data_words[o] = true;
        }
    }
    // the reference listing, by instruction index
    let mut refs: HashMap<usize, String> = HashMap::new();
    if let Ok(rp) = std::env::var("DIS_REF") {
        for l in std::fs::read_to_string(&rp).unwrap().lines() {
            let Some((a, rest)) = l.split_once(":\t") else { continue };
            let Ok(a) = u64::from_str_radix(a.trim(), 16) else { continue };
            if a < t.base {
                continue;
            }
            let idx = ((a - t.base) / 4) as usize;
            if idx < words {
                refs.insert(idx, rest.to_string());
            }
        }
        println!("{} reference lines", refs.len());
    }
    let have_refs = !refs.is_empty();
    let mut total = 0usize;
    let mut unknown = 0usize;
    let mut data_skipped = 0usize;
    let mut unk_groups: HashMap<u32, (usize, u64, u32)> = HashMap::new();
    let mut compared = 0usize;
    let mut agree = 0usize;
    let mut we_unknown_ref_known: HashMap<String, (usize, String)> = HashMap::new();
    let mut we_known_ref_data = 0usize;
    let mut data_by_mnem: HashMap<String, usize> = HashMap::new();
    let mut diffs: HashMap<(String, String), (usize, String, String)> = HashMap::new();
    let mut classes: HashMap<Class, usize> = HashMap::new();
    let chunk = 1 << 20;
    let mut start = 0usize;
    while start < words {
        let end = (start + chunk).min(words);
        let insns = decode(Arch::Arm64, &t.bytes[start * 4..end * 4], t.base + (start * 4) as u64);
        for (k, i) in insns.iter().enumerate() {
            let idx = start + k;
            if data_words[idx] {
                data_skipped += 1;
                continue;
            }
            total += 1;
            *classes.entry(i.class).or_insert(0) += 1;
            let w = le32(&t.bytes, idx * 4);
            if i.class == Class::Unknown {
                unknown += 1;
                let e = unk_groups.entry(w & 0xFFE0_FC00).or_insert((0, t.base + (idx * 4) as u64, w));
                e.0 += 1;
            }
            if have_refs {
                let Some(r) = refs.get(&idx) else { continue };
                let rt = ref_text(r, t.base + (idx * 4) as u64);
                if is_data_dir(&rt) {
                    if i.class != Class::Unknown {
                        we_known_ref_data += 1;
                        *data_by_mnem.entry(i.mnem.clone()).or_insert(0) += 1;
                    }
                    continue;
                }
                compared += 1;
                let ours = norm(&i.text());
                let theirs = norm(&rt);
                if ours == theirs {
                    agree += 1;
                } else if i.class == Class::Unknown {
                    let m = rt.split_whitespace().next().unwrap_or("").to_string();
                    let e = we_unknown_ref_known.entry(m).or_insert((0, format!("{:08x} {}", w, rt)));
                    e.0 += 1;
                } else {
                    let rm = rt.split_whitespace().next().unwrap_or("").to_string();
                    let e = diffs.entry((i.mnem.clone(), rm)).or_insert((0, format!("{:08x} @{:x}", w, t.base + (idx * 4) as u64), format!("ours `{}` ref `{}`", i.text(), rt)));
                    e.0 += 1;
                }
            }
        }
        start = end;
    }
    println!("{} instructions ({} data words skipped), {} unknown ({:.4}%)", total, data_skipped, unknown, 100.0 * unknown as f64 / total.max(1) as f64);
    let mut cl: Vec<_> = classes.iter().collect();
    cl.sort_by(|a, b| b.1.cmp(a.1));
    println!("classes: {}", cl.iter().map(|(c, n)| format!("{}={}", c.name(), n)).collect::<Vec<_>>().join(" "));
    let mut ug: Vec<_> = unk_groups.iter().collect();
    ug.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
    for (k, (n, at, w)) in ug.iter().take(40) {
        println!("UNKNOWN class {:08x}: {} words, e.g. {:08x} at {:x}", k, n, w, at);
    }
    if have_refs {
        println!("compared {}: agree {} ({:.4}%)", compared, agree, 100.0 * agree as f64 / compared.max(1) as f64);
        println!("we decode, reference says data: {}", we_known_ref_data);
        let mut dm: Vec<_> = data_by_mnem.iter().collect();
        dm.sort_by(|a, b| b.1.cmp(a.1));
        println!("  by mnemonic: {}", dm.iter().take(25).map(|(m, n)| format!("{}={}", m, n)).collect::<Vec<_>>().join(" "));
        let mut u: Vec<_> = we_unknown_ref_known.iter().collect();
        u.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
        for (m, (n, ex)) in u.iter().take(40) {
            println!("WE-UNKNOWN ref `{}`: {} e.g. {}", m, n, ex);
        }
        let mut d: Vec<_> = diffs.iter().collect();
        d.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
        for ((om, rm), (n, at, ex)) in d.iter().take(80) {
            println!("DIFF {} / {}: {} {} {}", om, rm, n, at, ex);
        }
        let dsum: usize = diffs.values().map(|x| x.0).sum();
        println!("differing (both decode): {}", dsum);
    }
    assert!(unknown * 1000 < total, "Unknown above 0.1%");
}
