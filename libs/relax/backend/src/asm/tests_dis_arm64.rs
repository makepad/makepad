//! Tests of the AArch64 decoder: a round trip with the encoder of this module (decode a word,
//! assemble its text, get the word back), and the canonical spelling of the aliases.
use super::dis::{decode, Class, Insn};
use super::tests_arm64::{fp_corpus, neon_corpus, BRANCH, CORE, MEM};
use super::*;

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
