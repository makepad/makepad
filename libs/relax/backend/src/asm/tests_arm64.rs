//! AArch64 encoder tests: an oracle against clang's integrated assembler (skipped without
//! clang), and unit tests for placeholders, fixups, labels, Mach-O syntax and errors.
use super::*;
use super::arm64::{parse_reg, SYSREGS};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

fn ours(src: &str) -> Result<Output, AsmError> {
    assemble(Arch::Arm64, Format::Elf, Kind::Global, &[Piece::Text(src.to_string())], &[], false)
}
fn ours_bytes(src: &str) -> Result<Vec<u8>, String> {
    ours(src).map(|o| o.sections[0].bytes.clone()).map_err(|e| e.to_string())
}

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn clang_works() -> bool {
    Command::new("clang").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// The `.text` section of an ELF64 little-endian object.
fn elf_text(obj: &[u8]) -> Vec<u8> {
    let u16_at = |o: usize| u16::from_le_bytes([obj[o], obj[o + 1]]) as usize;
    let u32_at = |o: usize| u32::from_le_bytes(obj[o..o + 4].try_into().unwrap()) as usize;
    let u64_at = |o: usize| u64::from_le_bytes(obj[o..o + 8].try_into().unwrap()) as usize;
    assert_eq!(&obj[..4], b"\x7fELF");
    let shoff = u64_at(0x28);
    let shentsize = u16_at(0x3A);
    let shnum = u16_at(0x3C);
    let shstrndx = u16_at(0x3E);
    let sh = |i: usize| shoff + i * shentsize;
    let stroff = u64_at(sh(shstrndx) + 0x18);
    for i in 0..shnum {
        let name_off = stroff + u32_at(sh(i));
        let end = obj[name_off..].iter().position(|c| *c == 0).unwrap() + name_off;
        if &obj[name_off..end] == b".text" {
            let off = u64_at(sh(i) + 0x18);
            let size = u64_at(sh(i) + 0x20);
            return obj[off..off + size].to_vec();
        }
    }
    panic!("no .text in the object");
}

fn clang_bytes(src: &str) -> Vec<u8> {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir();
    let s = dir.join(format!("relax_asm_a64_{}_{}.s", std::process::id(), id));
    let o = dir.join(format!("relax_asm_a64_{}_{}.o", std::process::id(), id));
    std::fs::write(&s, src).unwrap();
    let out = Command::new("clang").args(["-c", "-x", "assembler", "--target=aarch64-unknown-linux-gnu", "-o"]).arg(&o).arg(&s).output().unwrap();
    let _ = std::fs::remove_file(&s);
    if !out.status.success() {
        let _ = std::fs::remove_file(&o);
        panic!("clang rejected the corpus:\n{}", String::from_utf8_lossy(&out.stderr));
    }
    let obj = std::fs::read(&o).unwrap();
    let _ = std::fs::remove_file(&o);
    elf_text(&obj)
}

const HEADER: &str = ".arch armv8.5-a+lse+fp16+lor+crc+sb+rcpc+dotprod+rdm+aes+sha2+rng+mte+sve\n";

fn clang_run(text: &str) -> (bool, String, Vec<u8>) {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir();
    let s = dir.join(format!("relax_asm_a64_{}_{}.s", std::process::id(), id));
    let o = dir.join(format!("relax_asm_a64_{}_{}.o", std::process::id(), id));
    std::fs::write(&s, text).unwrap();
    let out = Command::new("clang").args(["-c", "-x", "assembler", "--target=aarch64-unknown-linux-gnu", "-ferror-limit=0", "-Wno-unused-command-line-argument", "-o"]).arg(&o).arg(&s).output().unwrap();
    let _ = std::fs::remove_file(&s);
    let bytes = if out.status.success() { elf_text(&std::fs::read(&o).unwrap()) } else { Vec::new() };
    let _ = std::fs::remove_file(&o);
    (out.status.success(), String::from_utf8_lossy(&out.stderr).into_owned(), bytes)
}

/// For every line (a single instruction): clang's four bytes, or None when clang rejects it.
fn clang_each(lines: &[&str]) -> Vec<Option<Vec<u8>>> {
    let text = format!("{}{}\n", HEADER, lines.join("\n"));
    let (ok, stderr, bytes) = clang_run(&text);
    if ok {
        assert_eq!(bytes.len(), lines.len() * 4, "every line must be one instruction");
        return bytes.chunks(4).map(|c| Some(c.to_vec())).collect();
    }
    let mut rejected = std::collections::HashSet::new();
    for l in stderr.lines() {
        if let Some(p) = l.find(".s:") {
            let rest = &l[p + 3..];
            let n: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if rest[n.len()..].contains(": error") {
                rejected.insert(n.parse::<usize>().unwrap() - 2);
            }
        }
    }
    let kept: Vec<&str> = lines.iter().enumerate().filter(|(i, _)| !rejected.contains(i)).map(|(_, l)| *l).collect();
    let (ok, stderr, bytes) = clang_run(&format!("{}{}\n", HEADER, kept.join("\n")));
    assert!(ok, "clang rejected lines it did not report:\n{}", stderr);
    assert_eq!(bytes.len(), kept.len() * 4);
    let mut it = bytes.chunks(4);
    (0..lines.len()).map(|i| if rejected.contains(&i) { None } else { it.next().map(|c| c.to_vec()) }).collect()
}

fn clang_rejects(line: &str) -> bool {
    clang_each(&[line])[0].is_none()
}

fn hex(b: &[u8]) -> String {
    b.chunks(4).map(|w| w.iter().rev().map(|x| format!("{:02x}", x)).collect::<String>()).collect::<Vec<_>>().join(" ")
}

/// Assembles `src` (one instruction per line; `name:` lines are labels) with us and with clang
/// and compares the bytes; reports the lines that differ.
fn oracle(name: &str, src: &str) {
    oracle_x(name, src, false)
}

/// With `rejects`, lines we reject must be rejected by clang too (and are left out of the byte
/// comparison).
fn oracle_x(name: &str, src: &str, rejects: bool) {
    if !clang_works() {
        println!("{}: clang not found, oracle skipped", name);
        return;
    }
    let mut lines: Vec<&str> = src.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    if rejects {
        let mut kept = Vec::new();
        for l in lines {
            if ours_bytes(l).is_ok() {
                kept.push(l);
            } else {
                assert!(clang_rejects(l), "{}: we reject `{}` but clang accepts it", name, l);
            }
        }
        lines = kept;
    }
    let mut lens = Vec::new();
    let mut errs = Vec::new();
    for l in &lines {
        if l.ends_with(':') {
            lens.push(0);
            continue;
        }
        match ours_bytes(l) {
            Ok(b) => lens.push(b.len()),
            Err(e) => {
                errs.push(format!("  `{}`: {}", l, e));
                lens.push(0);
            }
        }
    }
    if !errs.is_empty() {
        panic!("{}: {} lines rejected by us:\n{}", name, errs.len(), errs.join("\n"));
    }
    let text = format!("{}{}\n", HEADER, lines.join("\n"));
    let got = ours_bytes(&text).unwrap_or_else(|e| panic!("{}: {}", name, e));
    let want = clang_bytes(&text);
    if got == want {
        println!("{}: {} instructions, {} bytes identical to clang", name, lines.iter().filter(|l| !l.ends_with(':') && !l.starts_with('.')).count(), got.len());
        return;
    }
    let mut bad = Vec::new();
    let mut off = 0;
    for (l, n) in lines.iter().zip(&lens) {
        let (g, w) = (got.get(off..off + n), want.get(off..off + n));
        if g != w {
            bad.push(format!("  `{}`: ours {} clang {}", l, g.map(hex).unwrap_or_default(), w.map(hex).unwrap_or_default()));
        }
        off += n;
    }
    panic!("{}: {} of {} lines differ (sizes ours {} clang {}):\n{}", name, bad.len(), lines.len(), got.len(), want.len(), bad.iter().take(40).cloned().collect::<Vec<_>>().join("\n"));
}

pub(super) const CORE: &str = r#"
add x0, x1, x2
add w0, w1, w2
add x0, x1, x2, lsl #3
add w0, w1, w2, lsr #5
add x0, x1, x2, asr #63
add x0, x1, #0
add x0, x1, #4095
add x0, x1, #1, lsl #12
add x0, x1, #4096
add x0, x1, #0xfff000
add x0, x1, #-1
add w0, w1, #-4095
add sp, sp, #16
add x0, sp, #16
add sp, x29, #16
add x2, x29, #16
add x0, x1, w2, uxtw
add x0, x1, w2, sxtw #2
add x0, x1, x2, uxtx #1
add x0, x1, w2, uxtb
add x0, x1, w2, sxth #3
add w0, w1, w2, uxtb #1
add sp, sp, x9
add x0, sp, x1
add x0, sp, x1, lsl #3
add sp, sp, x1, lsl #2
adds x0, x1, x2
adds w0, w1, #5
adds x0, sp, #5
adds x0, sp, x1
sub x0, x1, x2
sub sp, sp, #208
sub sp, sp, x9
sub sp, x29, #16
sub x0, x1, #-5
sub w0, w1, w2, lsl #1
subs x0, x1, x2, asr #4
subs xzr, x1, #5
subs wzr, w1, w2
cmp x10, x3
cmp w0, #5
cmp x0, #-5
cmp x0, #4096
cmp sp, x1
cmp sp, #4
cmp x1, x2, lsl #3
cmp x1, w2, sxtw
cmn x0, #1
cmn w0, w1
cmn x0, #-3
neg x0, x1
neg w0, w1, lsl #2
negs x0, x1
adc x0, x1, x2
adcs w0, w1, w2
sbc x0, x1, x2
sbcs x0, x1, x2
ngc x0, x1
ngcs w0, w1
and x0, x1, x2
and w0, w1, w2, lsl #3
and x0, x1, x2, ror #4
ands x0, x1, x2
orr x0, x1, x2, lsr #7
eor x0, x1, x2, asr #9
bic x0, x1, x2
bics w0, w1, w2
orn x0, x1, x2
eon x0, x1, x2, lsl #5
and x9, x9, #-16
and x0, x1, #0xff
and w0, w1, #0xff00
orr x0, x1, #1
orr w0, w1, #0x80000000
eor x0, x1, #0x5555555555555555
ands x0, x1, #0xf
and sp, x1, #0xfff
orr sp, x1, #0xf0
eor sp, x1, #3
bic x0, x1, #0xff
bic w0, w1, #0xff
tst x0, x1
tst w0, #0xff
tst x0, x1, lsl #3
tst x0, #0x8000000000000000
mvn x0, x1
mvn w0, w1, lsl #4
mov x0, x1
mov w0, w1
mov w0, w17
mov x2, x29
mov x3, x30
mov sp, x1
mov x1, sp
mov sp, sp
mov wsp, w1
mov w0, wsp
mov x20, sp
mov sp, x20
mov sp, x29
mov x29, sp
mov x0, xzr
mov w0, wzr
mov x10, #0
mov x0, #1
mov w0, #-1
mov x0, #-1
mov x0, #0x10000
mov x0, #0xffff0000
mov x0, #0x0000ffff00000000
mov x0, #0xffff000000000000
mov x0, #0xffffffffffff0000
mov x0, #0xffffffff0000ffff
mov x0, #-2
mov x0, #0x5555555555555555
mov x0, #0xfffffffffffffffe
mov w0, #0xffff0000
mov w0, #0x55555555
mov w0, #0xfffe
mov w0, #-2
mov w0, #0x1234
mov x0, #0x1234
mov x0, #0x8000000000000000
mov w0, #0x80000000
movz w17, #5
movz x0, #0xffff, lsl #48
movz w0, #1, lsl #16
movn x0, #5
movn w0, #0xffff, lsl #16
movk x0, #1, lsl #32
movk w16, #3, lsl #16
movk x0, #0
mul x0, x1, x2
mul w0, w1, w2
mneg x0, x1, x2
madd x0, x1, x2, x3
msub w0, w1, w2, w3
smull x0, w1, w2
umull x0, w1, w2
smnegl x0, w1, w2
umnegl x0, w1, w2
smaddl x0, w1, w2, x3
umaddl x0, w1, w2, x3
smsubl x0, w1, w2, x3
umsubl x0, w1, w2, x3
smulh x0, x1, x2
umulh x0, x1, x2
sdiv x0, x1, x2
udiv w0, w1, w2
lsl x0, x1, #3
lsl w0, w1, #31
lsl x9, x3, #3
lsl x0, x1, #0
lsl x0, x1, #63
lsr x0, x1, #7
lsr w0, w1, #1
asr x0, x1, #63
asr w0, w1, #5
ror x0, x1, #5
ror w0, w1, #31
lsl x0, x1, x2
lsr w0, w1, w2
asr x0, x1, x2
ror w0, w1, w2
ubfx x0, x1, #4, #8
ubfx w0, w1, #0, #32
sbfx x0, x1, #3, #5
sbfx w0, w1, #31, #1
ubfiz x0, x1, #4, #8
ubfiz w0, w1, #1, #3
sbfiz x0, x1, #60, #4
bfi x0, x1, #8, #8
bfi w0, w1, #31, #1
bfxil x0, x1, #8, #8
bfc x0, #4, #8
bfc w0, #0, #1
ubfm x0, x1, #3, #9
sbfm w0, w1, #3, #9
bfm x0, x1, #3, #9
uxtb w0, w1
uxth w0, w1
sxtb w0, w1
sxtb x0, w1
sxth w0, w1
sxth x0, w1
sxtw x0, w1
extr x0, x1, x2, #5
extr w0, w1, w2, #31
clz x0, x1
clz w0, w1
cls x0, x1
rbit w0, w1
rbit x0, x1
rev x0, x1
rev w0, w1
rev16 x0, x1
rev16 w0, w1
rev32 x0, x1
csel x0, x1, x2, eq
csel w0, w1, w2, hs
csinc x0, x1, x2, lo
csinv x0, x1, x2, ge
csneg w0, w1, w2, lt
cset x0, eq
cset w0, hi
csetm x0, ne
csetm w0, ls
cinc x0, x1, gt
cinv w0, w1, le
cneg x0, x1, cs
ccmp x0, x1, #4, eq
ccmp w0, #5, #0, ne
ccmn x0, x1, #15, lt
ccmn w0, #31, #2, mi
adr x0, Lf
adr x0, Lb
adrp x0, Lf
adr x0, #8
adr x0, #-4
adrp x0, #4096
adrp x1, #-8192
adrp x16, ext
adr x16, ext
"#;

#[test]
fn oracle_core() {
    oracle("core", &format!("Lb:\nnop\n{}\nLf:\nnop\n", CORE));
}

pub(super) const BRANCH: &str = r#"
b Lf
b Lb
bl Lf
bl Lb
b ext
bl ext
b #8
bl #-4
br x16
blr x19
blr x30
ret
ret x1
ret x16
b.eq Lf
b.ne Lb
b.hs Lf
b.lo Lf
b.cs Lf
b.cc Lf
b.mi Lf
b.pl Lf
b.vs Lf
b.vc Lf
b.hi Lf
b.ls Lf
b.ge Lf
b.lt Lb
b.gt Lf
b.le Lf
b.al Lf
cbz x0, Lf
cbnz w1, Lb
cbnz x0, Lf
cbz w7, Lb
tbz x0, #3, Lf
tbnz w1, #31, Lb
tbz x5, #63, Lf
tbnz x5, #40, Lb
tbz w0, #0, Lf
eret
udf #0
udf #0xffff
nop
yield
wfe
wfi
sev
sevl
svc #0
svc #0x80
brk #0
brk #1000
hlt #3
hvc #1
smc #0
dsb ish
dsb sy
dsb ishst
dsb ishld
dsb ld
dsb st
dsb osh
dsb nsh
dmb ish
dmb ishst
dmb ishld
dmb sy
dmb oshst
dmb #3
isb
isb sy
clrex
clrex #3
hint #0
hint #34
csdb
bti
bti c
bti j
bti jc
mrs x0, fpcr
msr fpcr, x1
mrs x0, fpsr
msr fpsr, x3
mrs x0, nzcv
msr nzcv, x0
mrs x0, tpidr_el0
msr tpidr_el0, x1
mrs x0, tpidrro_el0
mrs x1, ctr_el0
mrs x1, dczid_el0
mrs x2, cntvct_el0
mrs x2, cntfrq_el0
mrs x2, midr_el1
mrs x2, mpidr_el1
mrs x3, id_aa64isar0_el1
mrs x3, id_aa64pfr0_el1
mrs x3, s3_3_c4_c4_0
msr s3_3_c4_c4_1, x2
mrs x3, s3_0_c15_c2_5
mrs x3, daif
msr daifset, #2
msr daifclr, #7
msr spsel, #1
dc cvau, x0
dc civac, x1
dc zva, x2
dc cvac, x3
dc ivac, x4
dc cvap, x5
ic ivau, x0
ic iallu
ic ialluis
sys #3, c7, c11, #1, x5
sys #0, c8, c3, #0
sysl x1, #0, c1, c2, #3
tlbi vmalle1is
tlbi vae1is, x0
at s1e1r, x0
"#;

#[test]
fn oracle_branch_sys() {
    oracle("branch_sys", &format!("Lb:\nnop\n{}\nLf:\nnop\n", BRANCH));
}

pub(super) const MEM: &str = r#"
ldr x0, [x1]
ldr x0, [x1, #8]
ldr x0, [x1, #32760]
ldr x0, [sp, #192]
ldr x8, [x20, #192]
ldr w0, [x1, #4]
ldr w0, [x1, #16380]
ldr x0, [x1, #-8]
ldr x0, [x1, #3]
ldr w0, [x1, #-256]
ldr x0, [x1, #255]
ldr x0, [x1, #256]
ldr x16, [x0]
ldr x16, [x17, x16, lsl #3]
ldr x11, [x2, x10, lsl #3]
str x11, [sp, x10, lsl #3]
ldr x0, [x1, x2]
ldr x0, [x1, x2, lsl #0]
ldr w0, [x1, x2, lsl #2]
ldr w0, [x1, w2, uxtw]
ldr w0, [x1, w2, uxtw #2]
ldr x0, [x1, w2, sxtw #3]
ldr x0, [x1, w2, sxtw]
ldr x0, [x1, x2, sxtx #3]
ldr x0, [x1, x2, sxtx]
ldr x0, [sp, x2]
ldr x0, [x1, #8]!
ldr x0, [x1, #-8]!
ldr x0, [x1], #8
ldr x0, [x1], #-16
ldr w0, [sp], #4
str x0, [x1, #8]!
str w0, [x1], #4
str x16, [x2]
str x8, [sp, #192]
str w0, [x1, #4]
str x0, [x1, #-8]
str xzr, [x1]
str wzr, [x1, #4]
ldrb w0, [x1]
ldrb w0, [x1, #1]
ldrb w0, [x1, #4095]
ldrb w0, [x1, #-1]
ldrb w0, [x1, x2]
ldrb w0, [x1, x2, lsl #0]
ldrb w0, [x1, w2, sxtw]
ldrb w0, [x1, w2, uxtw #0]
ldrb w0, [x1, #1]!
ldrb w0, [x1], #1
strb w0, [x1, #2]
strb w0, [x1, x2]
ldrh w0, [x1, #2]
ldrh w0, [x1, #3]
ldrh w0, [x1, x2, lsl #1]
strh w0, [x1, #2]
strh w0, [x1], #2
ldrsb w0, [x1]
ldrsb x0, [x1, #1]
ldrsb x0, [x1, x2]
ldrsh w0, [x1, #2]
ldrsh x0, [x1, #2]
ldrsh x0, [x1, x2, lsl #1]
ldrsw x0, [x1, #4]
ldrsw x0, [x1, x2, lsl #2]
ldrsw x0, [x1], #4
ldrsw x0, [x1, #4]!
ldur x0, [x1, #-8]
ldur x0, [x1, #8]
ldur w0, [x1, #-1]
stur x0, [x1, #-8]
stur w0, [x1, #3]
ldurb w0, [x1, #-1]
sturb w0, [x1, #-1]
ldurh w0, [x1, #-2]
sturh w0, [x1, #-2]
ldursb w0, [x1, #-1]
ldursb x0, [x1, #-1]
ldursh w0, [x1, #-2]
ldursh x0, [x1, #-2]
ldursw x0, [x1, #-4]
ldr b0, [x1]
ldr h0, [x1, #2]
ldr s0, [x1, #4]
ldr d0, [x1, #8]
ldr q0, [x1, #16]
ldr q0, [x1, #32]
ldr d0, [x1, #-8]
ldr q0, [x1, #-16]
ldr q0, [x1, #17]
ldr s0, [x1, x2, lsl #2]
ldr d0, [x1, x2, lsl #3]
ldr q0, [x1, x2, lsl #4]
ldr q0, [x1, x2]
ldr b0, [x1, x2]
ldr b0, [x1, x2, lsl #0]
ldr q0, [x1, #16]!
ldr q0, [x1], #16
ldr d0, [x1], #8
str b0, [x1]
str h0, [x1, #2]
str s0, [x1, #4]
str d0, [x1, #8]
str q0, [x1, #16]
str q0, [x1, #-16]!
str q0, [x1], #-16
str d8, [sp, #8]
stur q0, [x1, #-1]
ldur d0, [x1, #1]
ldr x0, Lf
ldr w0, Lb
ldr s0, Lf
ldr d0, Lf
ldr q0, Lf
ldrsw x0, Lf
ldr x0, #8
ldr q0, #-16
prfm pldl1keep, [x0]
prfm pldl1strm, [x0, #8]
prfm pstl3strm, [x0, #32760]
prfm plil2keep, [x0, #-8]
prfm pldl1keep, [x0, x1]
prfm pldl1keep, [x0, x1, lsl #3]
prfm #5, [x0]
prfm pldl1keep, Lf
prfum pldl1keep, [x0, #-1]
stp x29, x30, [sp, #-16]!
ldp x29, x30, [sp], #16
stp x0, x1, [sp, #0]
stp x0, x1, [sp]
ldp x0, x1, [sp, #16]
stp w0, w1, [x2, #8]
ldp w0, w1, [x2], #8
ldp w0, w1, [x2, #-8]!
stp q0, q1, [sp, #64]
ldp q6, q7, [x20, #160]
stp d8, d9, [sp, #16]
ldp d8, d9, [sp, #16]
stp s0, s1, [x0, #-8]
ldp s0, s1, [x0], #4
stp q0, q1, [sp, #-32]!
ldp x0, x1, [x2, #504]
ldp x0, x1, [x2, #-512]
ldpsw x0, x1, [x2, #8]
ldpsw x0, x1, [x2], #8
ldpsw x0, x1, [x2, #-8]!
ldnp x0, x1, [x2, #16]
stnp q0, q1, [x2, #32]
stnp w0, w1, [x2]
ldxr x0, [x1]
ldxr w0, [x1]
ldxrb w0, [x1]
ldxrh w0, [x1]
ldaxr x0, [x1]
ldaxrb w0, [x1]
ldaxrh w0, [x1]
stxr w2, x0, [x1]
stxr w2, w0, [x1]
stxrb w2, w0, [x1]
stxrh w2, w0, [x1]
stlxr w2, x0, [x1]
stlxrb w2, w0, [x1]
stlxrh w2, w0, [x1, #0]
ldxp x0, x1, [x2]
ldaxp w0, w1, [x2]
stxp w3, x0, x1, [x2]
stlxp w3, w0, w1, [x2]
ldar x0, [x1]
ldar w0, [x1]
ldarb w0, [x1]
ldarh w0, [x1]
stlr x0, [x1]
stlr w0, [x1]
stlrb w0, [x1]
stlrh w0, [x1]
ldlar x0, [x1]
stllr w0, [x1]
cas w0, w1, [x2]
casa x0, x1, [x2]
casl x0, x1, [x2]
casal x0, x1, [x2]
casb w0, w1, [x2]
cash w0, w1, [x2]
casalb w0, w1, [x2]
casah w0, w1, [x2]
swp w0, w1, [x2]
swpa x0, x1, [x2]
swpl x0, x1, [x2]
swpal x0, x1, [x2]
swpb w0, w1, [x2]
swplh w0, w1, [x2]
ldadd w0, w1, [x2]
ldadda x0, x1, [x2]
ldaddl x0, x1, [x2]
ldaddal x0, x1, [x2]
ldaddb w0, w1, [x2]
ldaddalh w0, w1, [x2]
ldclr x0, x1, [x2]
ldeor x0, x1, [x2]
ldset x0, x1, [x2]
ldsmax x0, x1, [x2]
ldsmin x0, x1, [x2]
ldumax x0, x1, [x2]
ldumin x0, x1, [x2]
ldsetal x0, x1, [x2]
stadd w0, [x2]
staddl x0, [x2]
staddb w0, [x2]
staddlh w0, [x2]
stclr x0, [x2]
steor x0, [x2]
stset x0, [x2]
stsmax x0, [x2]
stumin w0, [x2]
"#;

#[test]
fn oracle_mem() {
    oracle("mem", &format!("Lb:\nnop\n{}\nLf:\nnop\n", MEM));
}

#[test]
fn oracle_symbols() {
    // ELF forms with relocation specifiers: the fields stay zero (the linker fills them)
    oracle(
        "symbols",
        r#"
adrp x16, sym
adrp x16, sym + 8
ldr x16, [x16, :lo12:sym]
ldr x16, [x16, :lo12:sym + 8]
ldr w16, [x16, :lo12:sym + 4]
ldr q0, [x16, :lo12:sym + 16]
ldr b0, [x16, :lo12:sym]
ldrb w0, [x16, :lo12:sym]
ldrh w0, [x16, :lo12:sym]
str x0, [x16, :lo12:sym]
str d0, [x16, :lo12:sym]
add x0, x0, :lo12:sym
add x0, x0, :lo12:sym + 24
ldr x0, [x1, #:lo12:sym]
add x0, x1, #:lo12:sym
prfm pldl1keep, [x0, :lo12:sym]
"#,
    );
}

pub(super) fn fp_corpus() -> String {
    let mut src = String::new();
    src += r#"
fmov d0, d1
fmov s0, s1
fmov h0, h1
fmov w0, s1
fmov s0, w1
fmov x0, d1
fmov d0, x1
fmov x0, v1.d[1]
fmov v1.d[1], x0
fmov w0, h1
fmov x0, h1
fmov h0, w1
fmov h0, x1
fmov d0, xzr
fmov s0, wzr
fmov d0, #1.0
fmov s0, #-1.5
fmov d0, #0.5
fmov h0, #2.0
fmov d0, #31.0
fmov d0, #0.125
fmov d0, #-0.125
fmov d0, #1.9375
fmov d0, #3.875
fmov s0, #1
fmov v0.4s, #1.0
fmov v0.2d, #-2.0
fmov v0.2s, #0.25
fcvt s0, h1
fcvt d0, h1
fcvt h0, s1
fcvt h0, d1
fcvt d0, s1
fcvt s0, d1
fcmp s0, s1
fcmp d0, d1
fcmp h0, h1
fcmp d0, #0.0
fcmp s0, #0.0
fcmpe s0, s1
fcmpe d0, #0.0
fccmp s0, s1, #4, eq
fccmpe d0, d1, #0, ne
fcsel d0, d1, d2, eq
fcsel s0, s1, s2, ge
fcsel h0, h1, h2, lo
fcvtzs x0, d1
fcvtzs w0, s1
fcvtzs w0, d1
fcvtzs x0, s1
fcvtzs w0, h1
fcvtzs x0, s1, #4
fcvtzs w0, d1, #32
fcvtzu x0, d1
fcvtzu w0, s1, #3
scvtf d0, x1
scvtf s0, w1
scvtf d0, w1
scvtf s0, x1
scvtf h0, x1
scvtf d0, x1, #8
scvtf s0, w1, #32
ucvtf d0, x1
ucvtf s0, w1, #5
ucvtf d0, w1
"#;
    for f in ["fadd", "fsub", "fmul", "fdiv", "fmax", "fmin", "fmaxnm", "fminnm", "fnmul"] {
        for r in ["s", "d", "h"] {
            src += &format!("\n{f} {r}0, {r}1, {r}2\n{f} {r}31, {r}30, {r}29");
        }
    }
    for f in ["fabs", "fneg", "fsqrt", "frintn", "frintp", "frintm", "frintz", "frinta", "frintx", "frinti"] {
        for r in ["s", "d", "h"] {
            src += &format!("\n{f} {r}0, {r}1\n{f} {r}9, {r}9");
        }
    }
    for f in ["fmadd", "fmsub", "fnmadd", "fnmsub"] {
        for r in ["s", "d", "h"] {
            src += &format!("\n{f} {r}0, {r}1, {r}2, {r}3");
        }
    }
    for f in ["fcvtns", "fcvtnu", "fcvtas", "fcvtau", "fcvtps", "fcvtpu", "fcvtms", "fcvtmu"] {
        src += &format!("\n{f} x0, d1\n{f} w0, s1\n{f} w0, d1\n{f} x0, h1");
    }
    src
}

#[test]
fn oracle_fp() {
    oracle("fp", &fp_corpus());
}

pub(super) fn neon_corpus() -> String {
    let mut src = String::new();
    src += r#"
add d0, d1, d2
sub d0, d1, d2
orr v0.4s, #1
orr v0.4s, #1, lsl #8
orr v0.8h, #255, lsl #8
orr v0.2s, #3, lsl #16
bic v0.4s, #1, lsl #24
bic v0.4h, #7
mov v0.16b, v1.16b
mov v0.8b, v1.8b
mov v0.s[1], v1.s[3]
mov v0.d[0], v1.d[1]
mov v0.b[15], v1.b[0]
mov v0.h[3], v1.h[2]
mov v0.s[1], w1
mov v0.d[1], x1
mov v0.b[0], w2
mov w0, v1.s[1]
mov x0, v1.d[1]
mov b0, v1.b[3]
mov h0, v1.h[1]
mov s0, v1.s[2]
mov d0, v1.d[1]
ins v0.s[1], w0
ins v0.s[1], v1.s[2]
ins v0.d[1], x0
umov w0, v1.b[3]
umov w0, v1.h[1]
umov w0, v1.s[1]
umov x0, v1.d[1]
smov w0, v1.b[3]
smov x0, v1.b[3]
smov x0, v1.h[1]
smov w0, v1.h[7]
smov x0, v1.s[3]
dup v0.4s, v1.s[1]
dup v0.16b, v1.b[15]
dup v0.2d, v1.d[1]
dup v0.8h, v1.h[0]
dup v0.2s, v1.s[0]
dup v0.4s, w1
dup v0.2d, x1
dup v0.16b, w1
dup v0.8b, w1
dup d0, v1.d[1]
dup s0, v1.s[3]
movi v0.16b, #0
movi v0.16b, #255
movi v0.8b, #3
movi v0.4s, #1
movi v0.4s, #1, lsl #8
movi v0.4s, #1, lsl #16
movi v0.4s, #1, lsl #24
movi v0.2s, #255, lsl #16
movi v0.8h, #1
movi v0.8h, #1, lsl #8
movi v0.4h, #7
movi v0.4s, #1, msl #8
movi v0.4s, #1, msl #16
movi v0.2s, #9, msl #8
movi v0.2d, #0
movi v0.2d, #0xff00ff00ff00ff00
movi v0.2d, #0xffffffffffffffff
movi d0, #0xff
movi d0, #0xff00ff00
mvni v0.4s, #1
mvni v0.4s, #1, lsl #16
mvni v0.8h, #1
mvni v0.2s, #5, msl #8
mvn v0.16b, v1.16b
not v0.8b, v1.8b
cnt v0.16b, v1.16b
cnt v0.8b, v1.8b
rbit v0.16b, v1.16b
rbit v0.8b, v1.8b
rev64 v0.4s, v1.4s
rev64 v0.16b, v1.16b
rev64 v0.4h, v1.4h
rev32 v0.8h, v1.8h
rev32 v0.16b, v1.16b
rev16 v0.16b, v1.16b
abs v0.4s, v1.4s
abs v0.2d, v1.2d
neg v0.2d, v1.2d
neg v0.8b, v1.8b
cls v0.4s, v1.4s
clz v0.16b, v1.16b
clz v0.8h, v1.8h
sqabs v0.4s, v1.4s
sqneg v0.8h, v1.8h
cmeq v0.4s, v1.4s, v2.4s
cmeq v0.4s, v1.4s, #0
cmgt v0.16b, v1.16b, #0
cmge v0.2d, v1.2d, #0
cmle v0.8h, v1.8h, #0
cmlt v0.2s, v1.2s, #0
cmhi v0.4s, v1.4s, v2.4s
cmhs v0.4s, v1.4s, v2.4s
cmtst v0.4s, v1.4s, v2.4s
cmgt v0.4s, v1.4s, v2.4s
cmge v0.4s, v1.4s, v2.4s
cmle v0.4s, v1.4s, v2.4s
cmlt v0.4s, v1.4s, v2.4s
cmls v0.4s, v1.4s, v2.4s
cmlo v0.4s, v1.4s, v2.4s
addp d0, v1.2d
addv b0, v1.16b
addv h0, v1.8h
addv s0, v1.4s
addv b0, v1.8b
addv h0, v1.4h
umaxv b0, v1.16b
uminv h0, v1.4h
smaxv s0, v1.4s
sminv b0, v1.8b
uaddlv h0, v1.16b
uaddlv s0, v1.8h
uaddlv d0, v1.4s
saddlv h0, v1.8b
saddlv s0, v1.4h
faddp s0, v1.2s
faddp d0, v1.2d
fcvtl v0.4s, v1.4h
fcvtl v0.2d, v1.2s
fcvtl2 v0.4s, v1.8h
fcvtl2 v0.2d, v1.4s
fcvtn v0.4h, v1.4s
fcvtn v0.2s, v1.2d
fcvtn2 v0.8h, v1.4s
fcvtn2 v0.4s, v1.2d
xtn v0.8b, v1.8h
xtn v0.4h, v1.4s
xtn v0.2s, v1.2d
xtn2 v0.16b, v1.8h
xtn2 v0.8h, v1.4s
xtn2 v0.4s, v1.2d
sxtl v0.8h, v1.8b
uxtl v0.4s, v1.4h
uxtl v0.2d, v1.2s
sxtl2 v0.8h, v1.16b
uxtl2 v0.4s, v1.8h
uxtl2 v0.2d, v1.4s
sshll v0.8h, v1.8b, #3
ushll2 v0.2d, v1.4s, #5
ushll v0.4s, v1.4h, #15
ext v0.16b, v1.16b, v2.16b, #5
ext v0.16b, v1.16b, v2.16b, #15
ext v0.8b, v1.8b, v2.8b, #7
tbl v0.16b, {v1.16b}, v2.16b
tbl v0.8b, {v1.16b, v2.16b}, v3.8b
tbl v0.16b, {v1.16b, v2.16b, v3.16b}, v4.16b
tbx v0.16b, {v1.16b, v2.16b, v3.16b, v4.16b}, v5.16b
tbl v0.16b, {v30.16b, v31.16b}, v4.16b
tbl v0.16b, {v31.16b, v0.16b}, v4.16b
ld1 {v0.16b}, [x0]
ld1 {v0.4s, v1.4s}, [x0]
ld1 {v0.2d, v1.2d, v2.2d}, [x0]
ld1 {v0.8h, v1.8h, v2.8h, v3.8h}, [x0]
ld1 {v0.8b}, [x0]
ld1 {v0.1d}, [x0]
ld1 {v0.2s}, [sp]
ld1 {v0.16b, v1.16b}, [x0], #32
ld1 {v0.16b}, [x0], #16
ld1 {v0.8b}, [x0], #8
ld1 {v0.16b}, [x0], x2
ld1 {v0.16b-v3.16b}, [x0]
st1 {v0.4s, v1.4s}, [x0], #32
st1 {v0.16b, v1.16b, v2.16b}, [sp], #48
st1 {v31.2d, v0.2d}, [x1]
ld2 {v0.4s, v1.4s}, [x0]
ld2 {v0.8b, v1.8b}, [x0], #16
ld3 {v0.8b, v1.8b, v2.8b}, [x0], #24
ld4 {v0.16b, v1.16b, v2.16b, v3.16b}, [x0], #64
st2 {v0.2d, v1.2d}, [x0]
st3 {v0.4s, v1.4s, v2.4s}, [x0]
st4 {v0.8h, v1.8h, v2.8h, v3.8h}, [x0], x3
ld1 {v0.s}[1], [x0]
ld1 {v0.b}[15], [x0]
ld1 {v0.h}[7], [x0], #2
ld1 {v0.d}[1], [x0], x1
ld1 {v0.s}[3], [x0], #4
st1 {v0.s}[3], [x0]
st1 {v0.b}[0], [x0], #1
st1 {v0.d}[0], [x0]
ld2 {v0.s, v1.s}[1], [x0]
ld3 {v0.h, v1.h, v2.h}[3], [x0]
ld4 {v0.d, v1.d, v2.d, v3.d}[1], [x0], #32
st2 {v0.b, v1.b}[8], [x0]
st3 {v0.s, v1.s, v2.s}[2], [x0], #12
ld1r {v0.4s}, [x0]
ld1r {v0.16b}, [x0], #1
ld1r {v0.2d}, [x0], #8
ld2r {v0.8h, v1.8h}, [x0]
ld3r {v0.4s, v1.4s, v2.4s}, [x0]
ld4r {v0.2d, v1.2d, v2.2d, v3.2d}, [x0], #32
"#;
    let v16 = ["16b", "8b", "8h", "4h", "4s", "2s", "2d"];
    for f in ["add", "sub", "cmeq", "cmgt", "cmge", "cmhi", "cmhs", "cmtst", "sshl", "ushl", "srshl", "urshl", "sqadd", "uqadd", "sqsub", "uqsub", "addp"] {
        for a in ["16b", "8b", "8h", "4h", "4s", "2s", "2d"] {
            src += &format!("\n{f} v0.{a}, v1.{a}, v2.{a}");
        }
    }
    let _ = v16;
    for f in ["mul", "mla", "mls", "smax", "smin", "umax", "umin", "sabd", "uabd", "saba", "uaba", "shadd", "uhadd", "srhadd", "urhadd", "shsub", "uhsub", "smaxp", "sminp", "umaxp", "uminp"] {
        for a in ["16b", "8b", "8h", "4h", "4s", "2s"] {
            src += &format!("\n{f} v0.{a}, v1.{a}, v2.{a}");
        }
    }
    for f in ["sqdmulh", "sqrdmulh"] {
        for a in ["8h", "4h", "4s", "2s"] {
            src += &format!("\n{f} v3.{a}, v4.{a}, v5.{a}");
        }
    }
    for f in ["and", "orr", "eor", "bic", "orn", "bsl", "bit", "bif", "pmul"] {
        for a in ["16b", "8b"] {
            src += &format!("\n{f} v0.{a}, v1.{a}, v2.{a}");
        }
    }
    for f in ["fadd", "fsub", "fmul", "fdiv", "fmax", "fmin", "fmaxnm", "fminnm", "fmla", "fmls", "faddp", "fmaxp", "fminp", "fmaxnmp", "fminnmp", "fabd", "fmulx", "fcmeq", "fcmge", "fcmgt", "facge", "facgt", "frecps", "frsqrts"] {
        for a in ["4s", "2s", "2d", "8h", "4h"] {
            src += &format!("\n{f} v0.{a}, v1.{a}, v2.{a}");
        }
    }
    for f in ["fabs", "fneg", "fsqrt", "frintn", "frintp", "frintm", "frintz", "frinta", "frintx", "frinti", "scvtf", "ucvtf", "fcvtzs", "fcvtzu", "fcvtns", "fcvtnu", "fcvtms", "fcvtmu", "fcvtas", "fcvtau", "fcvtps", "fcvtpu", "frecpe", "frsqrte"] {
        for a in ["4s", "2s", "2d"] {
            src += &format!("\n{f} v0.{a}, v1.{a}");
        }
    }
    for f in ["abs", "neg"] {
        for a in ["16b", "8h", "4s", "2d"] {
            src += &format!("\n{f} v0.{a}, v1.{a}");
        }
    }
    for f in ["smull", "umull", "smlal", "umlal", "smlsl", "umlsl", "saddl", "uaddl", "ssubl", "usubl", "sabdl", "uabdl"] {
        src += &format!("\n{f} v0.8h, v1.8b, v2.8b\n{f} v0.4s, v1.4h, v2.4h\n{f} v0.2d, v1.2s, v2.2s\n{f}2 v0.8h, v1.16b, v2.16b\n{f}2 v0.4s, v1.8h, v2.8h\n{f}2 v0.2d, v1.4s, v2.4s");
    }
    for (f, left) in [("shl", true), ("sli", true), ("sshr", false), ("ssra", false), ("srshr", false), ("srsra", false), ("ushr", false), ("usra", false), ("urshr", false), ("ursra", false), ("sri", false)] {
        for (a, bits) in [("16b", 8), ("8b", 8), ("8h", 16), ("4h", 16), ("4s", 32), ("2s", 32), ("2d", 64)] {
            let lo = if left { 0 } else { 1 };
            let hi = if left { bits - 1 } else { bits };
            src += &format!("\n{f} v0.{a}, v1.{a}, #{lo}\n{f} v0.{a}, v1.{a}, #{hi}\n{f} v7.{a}, v9.{a}, #{}", (lo + hi) / 2);
        }
    }
    for f in ["zip1", "zip2", "uzp1", "uzp2", "trn1", "trn2"] {
        for a in ["16b", "8b", "8h", "4h", "4s", "2s", "2d"] {
            src += &format!("\n{f} v0.{a}, v1.{a}, v2.{a}");
        }
    }
    src
}

#[test]
fn oracle_neon() {
    oracle("neon", &neon_corpus());
}

#[test]
fn oracle_logical_imm() {
    // every shape of bitmask immediate: element sizes 2..64, run lengths, rotations
    let mut src = String::new();
    for e in [2u32, 4, 8, 16, 32, 64] {
        for ones in 1..e {
            for r in 0..e {
                let em: u128 = if e == 64 { u64::MAX as u128 } else { (1u128 << e) - 1 };
                let run: u128 = (1u128 << ones) - 1;
                let elem = ((run << r) | (run >> (e - r))) & em;
                let mut v: u64 = 0;
                let mut at = 0;
                while at < 64 {
                    v |= (elem as u64) << at;
                    at += e;
                }
                if (r + ones) % 7 == 0 || e >= 32 && r % 5 != 0 && ones % 4 != 1 {
                    continue;
                }
                src += &format!("\nand x1, x2, #{:#x}\norr x3, x4, #{:#x}", v, v);
                if e <= 32 {
                    src += &format!("\neor w5, w6, #{:#x}\nands w7, w8, #{:#x}", v as u32, v as u32);
                }
            }
        }
    }
    oracle("logical_imm", &src);
}

#[test]
fn oracle_mov_imm() {
    let mut src = String::new();
    let mut vals: Vec<u64> = Vec::new();
    for sh in [0, 16, 32, 48] {
        for c in [0u64, 1, 0x1234, 0x8000, 0xffff, 0xfffe] {
            let v = c << sh;
            vals.push(v);
            vals.push(!v);
            vals.push(v | 0xffff_0000_ffff_0000 & !(0xffffu64 << sh));
            vals.push((!v) & 0xffff_ffff);
        }
    }
    vals.extend([0x5555_5555_5555_5555, 0xaaaa_aaaa_aaaa_aaaa, 0x00ff_00ff_00ff_00ff, 0x0f0f_0f0f_0f0f_0f0f, 0xffff_ffff_0000_0000, 0x0000_ffff_0000_ffff, 0x7fff_ffff_ffff_ffff, 0x8000_0000_8000_0000, 0xffff_ffff, 0xffff_fffe, 0x1_0000_0000, 0x0001_0001_0001_0001, 0x1234_5678, 0xffff_0000_0000_ffff]);
    for v in &vals {
        src += &format!("\nmov x0, #{:#x}", v);
        src += &format!("\nmov x0, #{}", *v as i64);
        let w = *v as u32;
        src += &format!("\nmov w0, #{:#x}", w);
        src += &format!("\nmov w0, #{}", w as i32);
    }
    oracle_x("mov_imm", &src, true);
}

// ---------------------------------------------------------------- unit tests

fn words(b: &[u8]) -> Vec<u32> {
    b.chunks(4).map(|w| u32::from_le_bytes(w.try_into().unwrap())).collect()
}

fn run(format: Format, kind: Kind, tpl: &str, args: &[Arg]) -> Output {
    let names: Vec<(String, u32)> = Vec::new();
    let pieces = parse_template(&[tpl], &names, args.len() as u32, args.len() as u32, false).unwrap();
    assemble(Arch::Arm64, format, kind, &pieces, args, false).unwrap_or_else(|e| panic!("`{}`: {}", tpl, e))
}

fn run_named(format: Format, tpl: &str, names: &[(&str, u32)], args: &[Arg]) -> Output {
    let names: Vec<(String, u32)> = names.iter().map(|(n, k)| (n.to_string(), *k)).collect();
    let pieces = parse_template(&[tpl], &names, 0, args.len() as u32, false).unwrap();
    assemble(Arch::Arm64, format, Kind::Inline, &pieces, args, false).unwrap_or_else(|e| panic!("`{}`: {}", tpl, e))
}

fn text(o: &Output) -> &[u8] {
    &o.sections[0].bytes
}

/// The template with its operands against clang's bytes for the equivalent concrete text.
fn same_as_clang(format: Format, tpl: &str, args: &[Arg], concrete: &str) -> Output {
    let o = run(format, Kind::Inline, tpl, args);
    if clang_works() {
        let want = clang_bytes(&format!("{}\n", concrete));
        assert_eq!(text(&o), &want[..], "`{}` vs clang `{}`: ours {} clang {}", tpl, concrete, hex(text(&o)), hex(&want));
    }
    o
}

fn sym(n: &str) -> Arg {
    Arg::Sym(SymRef::Name(n.to_string()))
}
fn symv(n: &str, k: i64) -> Value {
    Value::Sym(SymRef::Name(n.to_string()), k)
}
fn r(n: u8) -> Arg {
    Arg::Reg(PReg::int(n))
}
fn v(n: u8) -> Arg {
    Arg::Reg(PReg::vec(n))
}

#[test]
fn macho_dispatch_stub() {
    // adrp x16, {t}@PAGE ; ldr x16, [x16, {t}@PAGEOFF] ; br x16
    let o = run(Format::MachO, Kind::Global, "adrp x16, {0}@PAGE\nldr x16, [x16, {0}@PAGEOFF]\nbr x16", &[sym("table")]);
    assert_eq!(words(text(&o)), vec![0x9000_0010, 0xF940_0210, 0xD61F_0200]);
    assert_eq!(o.fixups.len(), 2);
    assert_eq!((o.fixups[0].at, o.fixups[0].kind, &o.fixups[0].target), (0, FixKind::A64Page21, &symv("table", 0)));
    assert_eq!((o.fixups[1].at, o.fixups[1].kind, &o.fixups[1].target), (4, FixKind::A64Lo12 { scale: 3 }, &symv("table", 0)));
}

#[test]
fn macho_add_pageoff_and_access_sizes() {
    let o = run(Format::MachO, Kind::Global, "add x17, x17, {0}@PAGEOFF\nldr w1, [x2, {0}@PAGEOFF]\nldr q0, [x2, {0}@PAGEOFF]\nldrb w3, [x2, {0}@PAGEOFF]\nldrh w3, [x2, {0}@PAGEOFF]\nstr d0, [x2, {0}@PAGEOFF]", &[sym("tab")]);
    let scales: Vec<FixKind> = o.fixups.iter().map(|f| f.kind).collect();
    assert_eq!(
        scales,
        vec![FixKind::A64Lo12 { scale: 0 }, FixKind::A64Lo12 { scale: 2 }, FixKind::A64Lo12 { scale: 4 }, FixKind::A64Lo12 { scale: 0 }, FixKind::A64Lo12 { scale: 1 }, FixKind::A64Lo12 { scale: 3 }]
    );
    assert_eq!(words(text(&o))[0], 0x9100_0231);
}

#[test]
fn elf_relocation_forms() {
    let o = run(Format::Elf, Kind::Global, "adrp x16, {0} + 16\nldr x16, [x16, :lo12:{0} + 16]\nadd x0, x0, :lo12:{0}", &[sym("s")]);
    assert_eq!(words(text(&o)), vec![0x9000_0010, 0xF940_0210, 0x9100_0000]);
    assert_eq!(o.fixups[0].kind, FixKind::A64Page21);
    assert_eq!(o.fixups[0].target, symv("s", 16));
    assert_eq!(o.fixups[1].kind, FixKind::A64Lo12 { scale: 3 });
    assert_eq!(o.fixups[1].target, symv("s", 16));
    assert_eq!(o.fixups[2].kind, FixKind::A64Lo12 { scale: 0 });
    // the table index form of the inventory: sym + k*8
    let o = run(Format::Elf, Kind::Global, "adrp x16, {0} + 3*8\nldr x16, [x16, :lo12:{0} + 3*8]", &[sym("tbl")]);
    assert_eq!(o.fixups[1].target, symv("tbl", 24));
}

#[test]
fn branches_to_symbols() {
    let o = run(Format::MachO, Kind::Global, "b {0}\nbl {1}", &[sym("f"), sym("enter")]);
    assert_eq!(words(text(&o)), vec![0x1400_0000, 0x9400_0000]);
    assert_eq!((o.fixups[0].at, o.fixups[0].kind, &o.fixups[0].target), (0, FixKind::A64Branch26, &symv("f", 0)));
    assert_eq!((o.fixups[1].at, o.fixups[1].kind, &o.fixups[1].target), (4, FixKind::A64Branch26, &symv("enter", 0)));
    // an undefined name in the text is a symbol too
    let o = run(Format::Elf, Kind::Global, "bl some_c_function\nb.eq other\ncbz x0, other\ntbz x0, #3, other\nldr x0, lit", &[]);
    let kinds: Vec<FixKind> = o.fixups.iter().map(|f| f.kind).collect();
    assert_eq!(kinds, vec![FixKind::A64Branch26, FixKind::A64Branch19, FixKind::A64Branch19, FixKind::A64Branch14, FixKind::A64Branch19]);
    assert_eq!(o.fixups[0].target, symv("some_c_function", 0));
}

#[test]
fn register_operands_and_modifiers() {
    same_as_clang(Format::MachO, "blr {0}", &[r(19)], "blr x19");
    same_as_clang(Format::MachO, "mov sp, {0}", &[r(20)], "mov sp, x20");
    same_as_clang(Format::MachO, "mov {0}, sp", &[r(20)], "mov x20, sp");
    same_as_clang(Format::MachO, "mov {0:w}, {1:w}", &[r(0), r(17)], "mov w0, w17");
    same_as_clang(Format::MachO, "mov {0}, {1}", &[r(2), r(29)], "mov x2, x29");
    same_as_clang(Format::MachO, "add {0}, {1}, #1", &[r(3), r(4)], "add x3, x4, #1");
    same_as_clang(Format::MachO, "add {0:w}, {1:w}, #1", &[r(3), r(4)], "add w3, w4, #1");
    same_as_clang(Format::MachO, "mrs {}, fpcr", &[r(5)], "mrs x5, fpcr");
    same_as_clang(Format::MachO, "msr fpcr, {}", &[r(5)], "msr fpcr, x5");
    same_as_clang(Format::MachO, "mrs {}, tpidr_el0", &[r(1)], "mrs x1, tpidr_el0");
    same_as_clang(Format::MachO, "mrs {}, ctr_el0", &[r(1)], "mrs x1, ctr_el0");
    same_as_clang(Format::MachO, "dc cvau, {}", &[r(1)], "dc cvau, x1");
    same_as_clang(Format::MachO, "ic ivau, {}", &[r(1)], "ic ivau, x1");
    same_as_clang(Format::MachO, "ldr x11, [{0}, {1}, lsl #3]", &[r(2), r(10)], "ldr x11, [x2, x10, lsl #3]");
    same_as_clang(Format::MachO, "str {0}, [sp, {1}, lsl #3]", &[r(11), r(10)], "str x11, [sp, x10, lsl #3]");
    same_as_clang(Format::MachO, "ldr x16, [{0}, {1}, lsl #3]", &[r(17), r(16)], "ldr x16, [x17, x16, lsl #3]");
    same_as_clang(Format::MachO, "stp q0, q1, [{0}, #64]", &[r(20)], "stp q0, q1, [x20, #64]");
    same_as_clang(Format::MachO, "ldp q6, q7, [{0}, #160]", &[r(20)], "ldp q6, q7, [x20, #160]");
}

#[test]
fn vector_and_float_modifiers() {
    same_as_clang(Format::MachO, "fcvt {0:s}, {1:h}", &[v(0), v(1)], "fcvt s0, h1");
    same_as_clang(Format::MachO, "fcvt {0:d}, {1:h}", &[v(2), v(3)], "fcvt d2, h3");
    same_as_clang(Format::MachO, "fcvt {0:h}, {1:s}", &[v(4), v(5)], "fcvt h4, s5");
    same_as_clang(Format::MachO, "fcvt {0:h}, {1:d}", &[v(6), v(7)], "fcvt h6, d7");
    same_as_clang(Format::MachO, "fcvtl {0:v}.4s, {1:v}.4h", &[v(1), v(2)], "fcvtl v1.4s, v2.4h");
    // named operands
    let o = run_named(Format::MachO, "fcvtl {out:v}.4s, {inp:v}.4h", &[("out", 0), ("inp", 1)], &[v(1), v(2)]);
    assert_eq!(words(text(&o)).len(), 1);
    same_as_clang(Format::MachO, "fcvtl {0:v}.2d, {2:v}.2s", &[v(0), v(9), v(3)], "fcvtl v0.2d, v3.2s");
    same_as_clang(Format::MachO, "fcvtl2 {1:v}.2d, {2:v}.4s", &[v(9), v(0), v(3)], "fcvtl2 v0.2d, v3.4s");
    same_as_clang(Format::MachO, "fcvtn {1:v}.2s, {2:v}.2d", &[v(9), v(0), v(3)], "fcvtn v0.2s, v3.2d");
    same_as_clang(Format::MachO, "fcvtn2 {1:v}.4s, {3:v}.2d", &[v(9), v(0), v(9), v(3)], "fcvtn2 v0.4s, v3.2d");
    same_as_clang(Format::MachO, "fcvtn {0:v}.4h, {1:v}.4s", &[v(0), v(1)], "fcvtn v0.4h, v1.4s");
    for (f, c) in [("fadd", "fadd"), ("fsub", "fsub"), ("fmul", "fmul"), ("fdiv", "fdiv")] {
        for (m, rn) in [('h', "h"), ('s', "s"), ('d', "d")] {
            same_as_clang(Format::MachO, &format!("{f} {{0:{m}}}, {{1:{m}}}, {{2:{m}}}"), &[v(0), v(1), v(2)], &format!(".arch armv8.2-a+fp16\n{c} {rn}0, {rn}1, {rn}2"));
        }
    }
    let o = run_named(Format::MachO, "fmadd {x:d}, {x:d}, {y:d}, {z:d}", &[("x", 0), ("y", 1), ("z", 2)], &[v(1), v(2), v(3)]);
    if clang_works() {
        assert_eq!(words(text(&o)), words(&clang_bytes("fmadd d1, d1, d2, d3\n")));
    }
    same_as_clang(Format::MachO, "frintn {0:d}, {0:d}", &[v(1)], "frintn d1, d1");
    same_as_clang(Format::MachO, "frintn {0:s}, {0:s}", &[v(1)], "frintn s1, s1");
    same_as_clang(Format::MachO, "fsqrt {0:d}, {1:d}", &[v(1), v(2)], "fsqrt d1, d2");
    same_as_clang(Format::MachO, "fsqrt {0:s}, {1:s}", &[v(1), v(2)], "fsqrt s1, s2");
    same_as_clang(Format::MachO, "ldr {0:q}, [{1}]", &[v(1), r(2)], "ldr q1, [x2]");
    same_as_clang(Format::MachO, "stp {0:d}, {1:d}, [sp, #16]", &[v(8), v(9)], "stp d8, d9, [sp, #16]");
    same_as_clang(Format::MachO, "add {0:v}.4s, {1:v}.4s, {2:v}.4s", &[v(0), v(1), v(2)], "add v0.4s, v1.4s, v2.4s");
    same_as_clang(Format::MachO, "mov {0:v}.s[1], {1:w}", &[v(0), r(1)], "mov v0.s[1], w1");
}

#[test]
fn constants_and_set_symbols() {
    // movz w17, #rip_k (a .set symbol) and a const operand
    let o = run(Format::MachO, Kind::Inline, ".set rip_k, 77\nmovz w17, #rip_k", &[]);
    assert_eq!(words(text(&o)), vec![0x5280_0000 | 77 << 5 | 17]);
    let o = run(Format::MachO, Kind::Inline, "movz w16, #({0} & 0xffff)\nmovk w16, #({0} >> 16), lsl #16", &[Arg::Const(0x1234_5678)]);
    assert_eq!(words(text(&o)), vec![0x5280_0000 | 0x5678 << 5 | 16, 0x7280_0000 | 1 << 21 | 0x1234 << 5 | 16]);
    let o = run(Format::MachO, Kind::Inline, "sub sp, sp, #{0}\nldr x8, [x20, #{1}]\nmov x0, #{2}", &[Arg::Const(208), Arg::Const(192), Arg::Const(-1)]);
    assert_eq!(words(text(&o)), vec![0xD103_43FF, 0xF940_6288, 0x9280_0000]);
}

#[test]
fn numeric_and_named_labels() {
    // cbnz x0, 5f ; b.ge 4f ; b 2f ; b 3b ... with forward and backward numeric references
    let src = "2:\nnop\ncbnz x0, 5f\nb.ge 4f\nb 2b\n4:\nnop\n5:\nret\nb 2f\n2:\nnop\nb 4b";
    if clang_works() {
        let want = clang_bytes(&format!("{}\n", src));
        let got = ours_bytes(src).unwrap();
        assert_eq!(hex(&got), hex(&want));
    }
    let o = run(Format::Elf, Kind::Inline, "2:\nnop\ncbnz x0, 5f\nb.ge 4f\nb 2b\n4:\nnop\n5:\nret", &[]);
    assert_eq!(words(text(&o)), vec![0xD503_201F, 0xB500_0080, 0x5400_004A, 0x17FF_FFFD, 0xD503_201F, 0xD65F_03C0]);
    assert!(o.fixups.is_empty());
    // named labels, forward and backward, with Mach-O comments
    let o = run(Format::MachO, Kind::Inline, "top: ; loop head\n  sub x0, x0, #1 ; count\n  cbnz x0, top\n  b done\ndone:\n  ret", &[]);
    assert_eq!(words(text(&o)), vec![0xD100_0400, 0xB5FF_FFE0, 0x1400_0001, 0xD65F_03C0]);
    // a missing numeric label is an error
    assert!(ours_bytes("b 7f").is_err());
}

#[test]
fn adr_to_label_and_literal_loads() {
    let src = "adr x0, here\nnop\nhere:\nldr x1, lit\nnop\nlit:\n.quad 5";
    if clang_works() {
        assert_eq!(hex(&ours_bytes(src).unwrap()), hex(&clang_bytes(&format!("{}\n", src))));
    }
}

#[test]
fn pcrel_out_of_range_is_an_error() {
    let e = ours_bytes("tbz x0, #1, far\n.zero 40000\nfar:\nnop").unwrap_err();
    assert!(e.contains("out of range"), "{}", e);
}

#[test]
fn errors_name_the_instruction() {
    let cases: &[(&str, &str)] = &[
        ("frobnicate x0", "unsupported instruction `frobnicate`"),
        ("add x0, x1", "unsupported operands for `add`"),
        ("add x0, w1, w2", "unsupported operands for `add`"),
        ("ldr x0, [x1, #32768]", "out of range"),
        ("add x0, x1, #4097", "out of range"),
        ("mov x0, #0x12345678", "cannot be moved by a single instruction"),
        ("movz x0, #0x10000", "out of range"),
        ("and x0, x1, #0x12345", "logical immediate"),
        ("b.xx 1f", "unknown condition"),
        ("mrs x0, nosuchreg", "unknown system register"),
        ("ldp x0, x1, [x2, #4]", "multiple of 8"),
        ("ldr x0, [x1, #1]!!", "unexpected"),
        ("svc #70000", "out of range"),
        ("fmov d0, #1.234", "not encodable"),
        ("ld1 {v0.4s, v2.4s}, [x0]", "consecutive"),
        ("lsl x0, x1, #64", "out of range"),
        ("tbz w0, #33, 1f", "out of range"),
    ];
    for (src, want) in cases {
        let e = ours_bytes(src).unwrap_err();
        assert!(e.contains(want), "`{}`: {} (wanted `{}`)", src, e, want);
    }
}

#[test]
fn parse_reg_names() {
    assert_eq!(parse_reg("x0"), Some((PReg::int(0), 64)));
    assert_eq!(parse_reg("X28"), Some((PReg::int(28), 64)));
    assert_eq!(parse_reg("w20"), Some((PReg::int(20), 32)));
    assert_eq!(parse_reg("lr"), Some((PReg::int(30), 64)));
    assert_eq!(parse_reg("FP"), Some((PReg::int(29), 64)));
    assert_eq!(parse_reg("v0"), Some((PReg::vec(0), 128)));
    assert_eq!(parse_reg("V31"), Some((PReg::vec(31), 128)));
    assert_eq!(parse_reg("q5"), Some((PReg::vec(5), 128)));
    assert_eq!(parse_reg("d8"), Some((PReg::vec(8), 64)));
    assert_eq!(parse_reg("s1"), Some((PReg::vec(1), 32)));
    assert_eq!(parse_reg("h2"), Some((PReg::vec(2), 16)));
    assert_eq!(parse_reg("b3"), Some((PReg::vec(3), 8)));
    for bad in ["x31", "w31", "sp", "xzr", "wzr", "v32", "x", "xx", "y0", "x0.4s", "v0.4s", "", "x-1", "x+1", "d100"] {
        assert_eq!(parse_reg(bad), None, "{}", bad);
    }
}

#[test]
fn macho_separators_and_comments() {
    let o = assemble(Arch::Arm64, Format::MachO, Kind::Inline, &[Piece::Text("mov x0, #1 ; one\nmov x1, #2 %% mov x2, #3\n// c\nret".to_string())], &[], false).unwrap();
    assert_eq!(words(text(&o)), vec![0xD280_0020, 0xD280_0041, 0xD280_0062, 0xD65F_03C0]);
}

#[test]
fn inst_and_arch_directives() {
    let o = ours(".arch armv8.2-a\n.inst 0xd503201f\n.inst 0xd65f03c0, 0x12345678\nnop").unwrap();
    assert_eq!(words(text(&o)), vec![0xD503_201F, 0xD65F_03C0, 0x1234_5678, 0xD503_201F]);
}

#[test]
fn macro_and_rept_with_instructions() {
    let src = ".macro save a, b\n stp \\a, \\b, [sp, #-16]!\n.endm\nsave x19, x20\nsave x21, x22\n.rept 3\nnop\n.endr";
    if clang_works() {
        assert_eq!(hex(&ours_bytes(src).unwrap()), hex(&clang_bytes(&format!("{}\n", src))));
    }
    assert_eq!(text(&ours(src).unwrap()).len(), 20);
}

#[test]
fn alignment_fills_with_nops() {
    let o = ours("nop\n.p2align 4\nret").unwrap();
    assert_eq!(words(text(&o)), vec![0xD503_201F, 0xD503_201F, 0xD503_201F, 0xD503_201F, 0xD65F_03C0]);
}

#[test]
fn case_insensitive_and_aliases() {
    let a = ours_bytes("ADD X0, X1, X2\nMOV Sp, X20\nB.GE 1f\n1:\nRET LR\nLDR X0, [SP, #8]\nfmov D0, D1\nMOV V0.16B, V1.16B").unwrap();
    let b = ours_bytes("add x0, x1, x2\nmov sp, x20\nb.ge 1f\n1:\nret x30\nldr x0, [sp, #8]\nfmov d0, d1\nmov v0.16b, v1.16b").unwrap();
    assert_eq!(a, b);
}

#[test]
fn global_labels_and_data() {
    let o = ours(".globl entry\nentry:\n  adr x0, tbl\n  ret\ntbl:\n  .quad entry\n  .word 7").unwrap();
    assert_eq!(o.defs.iter().map(|d| (d.name.as_str(), d.offset, d.global)).collect::<Vec<_>>(), vec![("entry", 0, true), ("tbl", 8, false)]);
    assert_eq!(o.fixups.len(), 1);
    assert_eq!(o.fixups[0].kind, FixKind::Abs64);
}

/// Exploration: `RELAX_A64_PROBE=<file> cargo test probe -- --ignored --nocapture` compares every
/// line of the file with clang one by one and lists the gaps.
#[test]
#[ignore]
fn probe() {
    let Ok(path) = std::env::var("RELAX_A64_PROBE") else { return };
    let src = std::fs::read_to_string(path).unwrap();
    let lines: Vec<&str> = src.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    let theirs = clang_each(&lines);
    let (mut gaps, mut diffs, mut over, mut same) = (Vec::new(), Vec::new(), Vec::new(), 0);
    for (l, t) in lines.iter().zip(theirs) {
        match (ours_bytes(l), t) {
            (Ok(a), Some(b)) if a == b => same += 1,
            (Ok(a), Some(b)) => diffs.push(format!("{}: ours {} clang {}", l, hex(&a), hex(&b))),
            (Err(e), Some(_)) => gaps.push(format!("{}: {}", l, e)),
            (Ok(_), None) => over.push(l.to_string()),
            (Err(_), None) => same += 1,
        }
    }
    println!("same {} | gaps {} | diffs {} | accepted-but-invalid {}", same, gaps.len(), diffs.len(), over.len());
    for g in &gaps {
        println!("GAP {}", g);
    }
    for g in &diffs {
        println!("DIFF {}", g);
    }
    for g in &over {
        println!("OVER {}", g);
    }
}

/// Wrong operands: whatever clang rejects, we must reject too (we may reject more).
#[test]
fn oracle_mutations_are_rejected() {
    if !clang_works() {
        return;
    }
    let mut base: Vec<String> = Vec::new();
    for c in [CORE.to_string(), BRANCH.to_string(), MEM.to_string(), fp_corpus(), neon_corpus()] {
        for l in c.lines().map(|l| l.trim()) {
            if !l.is_empty() && !l.ends_with(':') && !l.starts_with('.') {
                base.push(l.to_string());
            }
        }
    }
    base.sort();
    base.dedup();
    let mut muts: Vec<String> = Vec::new();
    for l in &base {
        muts.push(format!("{}, x0", l));
        muts.push(format!("{}, #1", l));
        muts.push(format!("{}, x1, x2", l));
        if let Some(p) = l.rfind(", ") {
            muts.push(l[..p].to_string());
        }
        if l.contains(" x") {
            muts.push(l.replacen(" x", " w", 1));
        } else if l.contains(" w") {
            muts.push(l.replacen(" w", " x", 1));
        }
        if let Some(p) = l.find(' ') {
            muts.push(format!("{} v7.4s, {}", &l[..p], &l[p + 1..]));
            muts.push(format!("{} xzr, {}", &l[..p], &l[p + 1..]));
        }
    }
    muts.sort();
    muts.dedup();
    let refs: Vec<&str> = muts.iter().map(|s| s.as_str()).collect();
    let theirs = clang_each(&refs);
    let (mut over, mut diffs, mut gaps, mut same) = (Vec::new(), Vec::new(), 0, 0);
    for (l, t) in refs.iter().zip(theirs) {
        match (ours_bytes(l), t) {
            (Ok(a), Some(b)) if a == b => same += 1,
            (Ok(a), Some(b)) => diffs.push(format!("{}: ours {} clang {}", l, hex(&a), hex(&b))),
            (Ok(_), None) => over.push(l.to_string()),
            (Err(_), Some(_)) => gaps += 1,
            (Err(_), None) => {}
        }
    }
    println!("mutations: {} lines, {} identical, {} accepted by clang only, {} different, {} accepted by us only", refs.len(), same, gaps, diffs.len(), over.len());
    assert!(over.is_empty() && diffs.is_empty(), "accepted but invalid:\n{}\ndifferent:\n{}", over.iter().take(400).cloned().collect::<Vec<_>>().join("\n"), diffs.iter().take(30).cloned().collect::<Vec<_>>().join("\n"));
}

#[test]
fn oracle_system_registers() {
    let mut src = String::new();
    for (n, ..) in SYSREGS {
        src += &format!("\nmrs x0, {}\nmsr {}, x1", n, n.to_uppercase());
    }
    for n in ["pmevcntr0_el0", "pmevcntr17_el0", "pmevcntr30_el0", "pmevtyper5_el0", "pmevtyper30_el0", "s3_3_c4_c4_0", "S3_1_C15_C2_7", "s2_0_c0_c0_0"] {
        src += &format!("\nmrs x0, {}\nmsr {}, x1", n, n);
    }
    // read-only / write-only registers: clang rejects the other direction and so do we
    let lines: Vec<&str> = src.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    let theirs = clang_each(&lines);
    let mut bad = Vec::new();
    for (l, t) in lines.iter().zip(&theirs) {
        match (ours_bytes(l), t) {
            (Ok(a), Some(b)) if &a == b => {}
            (Err(_), None) => {}
            (a, b) => bad.push(format!("{}: ours {:?} clang {:?}", l, a.map(|x| hex(&x)), b.as_ref().map(|x| hex(x)))),
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn oracle_crypto_and_misc() {
    oracle(
        "crypto_misc",
        r#"
aese v0.16b, v1.16b
aesd v0.16b, v1.16b
aesmc v0.16b, v1.16b
aesimc v0.16b, v1.16b
sha1c q0, s1, v2.4s
sha1p q0, s1, v2.4s
sha1m q0, s1, v2.4s
sha1h s0, s1
sha1su0 v0.4s, v1.4s, v2.4s
sha1su1 v0.4s, v1.4s
sha256h q0, q1, v2.4s
sha256h2 q0, q1, v2.4s
sha256su0 v0.4s, v1.4s
sha256su1 v0.4s, v1.4s, v2.4s
fjcvtzs w0, d1
pmull v0.1q, v1.1d, v2.1d
pmull2 v0.1q, v1.2d, v2.2d
pmull v0.8h, v1.8b, v2.8b
pmull2 v0.8h, v1.16b, v2.16b
"#,
    );
}

// ---------------------------------------------------------------- operand matrices

/// Every combination of the slots (`$0`, `$1` ... in the template).
fn combos(template: &str, slots: &[&[&str]]) -> Vec<String> {
    let mut out = vec![template.to_string()];
    for (i, set) in slots.iter().enumerate() {
        let key = format!("${}", i);
        let mut next = Vec::with_capacity(out.len() * set.len());
        for t in &out {
            for s in set.iter() {
                next.push(t.replace(&key, s));
            }
        }
        out = next;
    }
    out
}

/// Single-instruction lines: acceptance and bytes must be those of clang (both ways).
fn matrix(name: &str, lines: &[String]) {
    if !clang_works() {
        println!("{}: clang not found, matrix skipped", name);
        return;
    }
    let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
    let theirs = clang_each(&refs);
    let (mut bad, mut accepted) = (Vec::new(), 0);
    for (l, t) in refs.iter().zip(theirs) {
        match (ours_bytes(l), t) {
            (Ok(a), Some(b)) => {
                accepted += 1;
                if a != b {
                    bad.push(format!("DIFF {}: ours {} clang {}", l, hex(&a), hex(&b)));
                }
            }
            (Ok(_), None) => bad.push(format!("OVER {}", l)),
            (Err(e), Some(_)) => bad.push(format!("GAP {}: {}", l, e.split(" (").next().unwrap_or(""))),
            (Err(_), None) => {}
        }
    }
    println!("{}: {} lines, {} valid, {} problems", name, refs.len(), accepted, bad.len());
    if !bad.is_empty() {
        // a summary: the first few problems of each kind and mnemonic
        let mut seen: std::collections::BTreeMap<String, (usize, Vec<String>)> = Default::default();
        for p in &bad {
            let mut it = p.split_whitespace();
            let kind = it.next().unwrap_or("");
            let mn = it.next().unwrap_or("");
            let e = seen.entry(format!("{} {}", kind, mn)).or_default();
            e.0 += 1;
            if e.1.len() < 4 {
                e.1.push(p.clone());
            }
        }
        for (k, (n, samples)) in &seen {
            println!("SUMMARY {} x{}", k, n);
            for sm in samples {
                println!("    {}", sm);
            }
        }
    }
    assert!(bad.is_empty(), "{}: {} problems\n{}", name, bad.len(), bad.iter().take(60).cloned().collect::<Vec<_>>().join("\n"));
}

const GPR: &[&str] = &["x0", "x28", "sp", "xzr", "w0", "w30", "wsp", "wzr"];
const SHIFTS: &[&str] = &["", ", lsl #3", ", lsr #1", ", asr #63", ", ror #4", ", uxtw", ", sxtx #2", ", uxtb", ", sxth #4", ", uxtx #5", ", lsl #0", ", lsl #64", ", lsl #31", ", lsr #32", ", sxtw #4"];

#[test]
fn matrix_addsub_registers() {
    let mut lines = Vec::new();
    for op in ["add", "adds", "sub", "subs"] {
        lines.extend(combos(&format!("{} $0, $1, $2$3", op), &[GPR, GPR, GPR, SHIFTS]));
    }
    for op in ["cmp", "cmn"] {
        lines.extend(combos(&format!("{} $0, $1$2", op), &[GPR, GPR, SHIFTS]));
    }
    for op in ["neg", "negs"] {
        lines.extend(combos(&format!("{} $0, $1$2", op), &[GPR, GPR, SHIFTS]));
    }
    matrix("addsub_registers", &lines);
}

#[test]
fn matrix_addsub_immediates() {
    let imms = &["#0", "#1", "#4095", "#4096", "#0xfff000", "#-1", "#-4096", "#0x1001", "#0xfff001", "#-0xfff000", "#0x1000000", "#4095, lsl #12", "#1, lsl #0", "#1, lsl #24", "#0, lsl #12", "#-1, lsl #12"];
    let mut lines = Vec::new();
    for op in ["add", "adds", "sub", "subs"] {
        lines.extend(combos(&format!("{} $0, $1, $2", op), &[GPR, GPR, imms]));
    }
    for op in ["cmp", "cmn"] {
        lines.extend(combos(&format!("{} $0, $1", op), &[GPR, imms]));
    }
    matrix("addsub_immediates", &lines);
}

#[test]
fn matrix_logical() {
    let imms = &["#0", "#1", "#-1", "#0xff", "#0xff00ff00", "#0x5555555555555555", "#0x80000000", "#0xffffffff", "#0x100000000", "#-16", "#0xf0f0f0f0f0f0f0f0", "#0xfffffffffffffffe", "#0x7fffffff", "#-2147483648"];
    let mut lines = Vec::new();
    for op in ["and", "orr", "eor", "ands", "bic", "orn", "eon", "bics"] {
        lines.extend(combos(&format!("{} $0, $1, $2$3", op), &[GPR, GPR, GPR, SHIFTS]));
        lines.extend(combos(&format!("{} $0, $1, $2", op), &[GPR, GPR, imms]));
    }
    for op in ["tst"] {
        lines.extend(combos(&format!("{} $0, $1$2", op), &[GPR, GPR, SHIFTS]));
        lines.extend(combos(&format!("{} $0, $1", op), &[GPR, imms]));
    }
    lines.extend(combos("mvn $0, $1$2", &[GPR, GPR, SHIFTS]));
    lines.extend(combos("mov $0, $1", &[GPR, GPR]));
    lines.extend(combos("mov $0, $1", &[GPR, imms]));
    matrix("logical", &lines);
}

#[test]
fn matrix_arith_misc() {
    let mut lines = Vec::new();
    for op in ["mul", "mneg", "sdiv", "udiv", "adc", "adcs", "sbc", "sbcs", "lsl", "lsr", "asr", "ror", "smull", "umull", "smnegl", "umnegl", "smulh", "umulh", "crc32w", "crc32cx", "crc32b"] {
        lines.extend(combos(&format!("{} $0, $1, $2", op), &[GPR, GPR, GPR]));
    }
    for op in ["madd", "msub", "smaddl", "umaddl", "smsubl", "umsubl"] {
        lines.extend(combos(&format!("{} $0, $1, $2, $3", op), &[&["x0", "xzr", "w1", "sp"], &["x1", "w2", "wzr"], &["x2", "w3", "xzr"], &["x3", "w4", "xzr"]]));
    }
    for op in ["clz", "cls", "rbit", "rev", "rev16", "rev32", "rev64", "ngc", "ngcs", "uxtb", "uxth", "sxtb", "sxth", "sxtw", "uxtw"] {
        lines.extend(combos(&format!("{} $0, $1", op), &[GPR, GPR]));
    }
    for op in ["lsl", "lsr", "asr", "ror"] {
        lines.extend(combos(&format!("{} $0, $1, $2", op), &[&["x0", "w1", "xzr"], &["x1", "w2", "wzr"], &["#0", "#1", "#31", "#32", "#63", "#64", "#-1"]]));
    }
    for op in ["csel", "csinc", "csinv", "csneg"] {
        lines.extend(combos(&format!("{} $0, $1, $2, $3", op), &[&["x0", "w1", "sp"], &["x1", "w2", "xzr", "wzr"], &["x2", "w3", "xzr", "wzr"], &["eq", "hs", "lo", "al", "nv", "ge"]]));
    }
    for op in ["cset", "csetm"] {
        lines.extend(combos(&format!("{} $0, $1", op), &[&["x0", "w1", "sp", "xzr"], &["eq", "hs", "al", "nv", "lt"]]));
    }
    for op in ["cinc", "cinv", "cneg"] {
        lines.extend(combos(&format!("{} $0, $1, $2", op), &[&["x0", "w1", "sp"], &["x1", "w2", "xzr", "wzr"], &["eq", "hs", "al", "nv", "lt"]]));
    }
    for op in ["ccmp", "ccmn"] {
        lines.extend(combos(&format!("{} $0, $1, $2, $3", op), &[&["x0", "w1", "xzr"], &["x1", "w2", "#0", "#31", "#32", "wzr"], &["#0", "#15", "#16"], &["eq", "al", "lt"]]));
    }
    for op in ["extr"] {
        lines.extend(combos(&format!("{} $0, $1, $2, $3", op), &[&["x0", "w1"], &["x1", "w2"], &["x2", "w3"], &["#0", "#31", "#32", "#63", "#64"]]));
    }
    for op in ["ubfx", "sbfx", "ubfiz", "sbfiz", "bfi", "bfxil"] {
        lines.extend(combos(&format!("{} $0, $1, $2, $3", op), &[&["x0", "w1"], &["x1", "w2"], &["#0", "#1", "#31", "#32", "#63"], &["#1", "#8", "#32", "#33", "#64"]]));
    }
    for op in ["ubfm", "sbfm", "bfm"] {
        lines.extend(combos(&format!("{} $0, $1, $2, $3", op), &[&["x0", "w1"], &["x1", "w2"], &["#0", "#31", "#32", "#63", "#64"], &["#0", "#31", "#32", "#63"]]));
    }
    lines.extend(combos("bfc $0, $1, $2", &[&["x0", "w1", "xzr"], &["#0", "#8", "#32", "#63"], &["#1", "#8", "#32", "#64"]]));
    for op in ["movz", "movn", "movk"] {
        lines.extend(combos(&format!("{} $0, $1$2", op), &[&["x0", "w1", "xzr", "sp"], &["#0", "#0xffff", "#0x10000", "#-1"], &["", ", lsl #0", ", lsl #16", ", lsl #32", ", lsl #48", ", lsl #8", ", lsl #64"]]));
    }
    matrix("arith_misc", &lines);
}

#[test]
fn matrix_memory() {
    let rt = &["x0", "w0", "xzr", "wzr", "sp", "b0", "h0", "s0", "d0", "q0"];
    let addr = &["", ", #8", ", #-8", ", #3", ", #4096", ", #32760", ", #-256", ", #255", ", #256", ", #-257", ", x2", ", x2, lsl #3", ", x2, lsl #2", ", w2, uxtw", ", w2, sxtw #3", ", x2, sxtx", ", w2, sxtw #2", ", x2, lsl #0", ", w2, uxtw #0", ", x2, lsl #1", ", x2, lsl #4", ", w2, sxtw", ", x2, uxtx", ", x2, sxtw", ", w2, lsl #3"];
    let bases = &["x1", "sp", "w1", "xzr"];
    let tails = &["", "!", "], #8"];
    let mut lines = Vec::new();
    for op in ["ldr", "str", "ldrb", "strb", "ldrh", "strh", "ldrsb", "ldrsh", "ldrsw", "ldur", "stur", "ldurb", "ldursw", "ldtr", "sttrb", "ldapur", "stlurh", "ldapursw", "prfm", "prfum"] {
        for t in tails {
            let tmpl = if t.is_empty() { format!("{} $0, [$1$2]", op) } else if *t == "!" { format!("{} $0, [$1$2]!", op) } else { format!("{} $0, [$1$2{}", op, t) };
            lines.extend(combos(&tmpl, &[rt, bases, addr]));
        }
    }
    for op in ["ldp", "stp", "ldpsw", "ldnp", "stnp"] {
        for t in tails {
            let tmpl = if t.is_empty() { format!("{} $0, $0, [$1$2]", op) } else if *t == "!" { format!("{} $0, $0, [$1$2]!", op) } else { format!("{} $0, $0, [$1$2{}", op, t) };
            lines.extend(combos(&tmpl, &[&["x0", "w0", "xzr", "s0", "d0", "q0", "x1"], &["x1", "sp"], &["", ", #8", ", #-8", ", #4", ", #16", ", #504", ", #512", ", #-512", ", #252", ", #1008", ", #-1024", ", #-1040", ", x2"]]));
        }
    }
    matrix("memory", &lines);
}

#[test]
fn matrix_branches_and_system() {
    let mut lines = Vec::new();
    for op in ["br", "blr", "ret"] {
        lines.extend(combos(&format!("{} $0", op), &[&["x0", "x30", "w0", "xzr", "sp", "lr", ""]]));
    }
    for op in ["cbz", "cbnz"] {
        lines.extend(combos(&format!("{} $0, $1", op), &[&["x0", "w1", "xzr", "sp"], &["lab", "#8", "#4", "#3", "#1048576", "#-1048576", "#1048580"]]));
    }
    for op in ["tbz", "tbnz"] {
        lines.extend(combos(&format!("{} $0, $1, $2", op), &[&["x0", "w1", "xzr", "sp"], &["#0", "#31", "#32", "#63", "#64"], &["lab", "#8", "#32768", "#-32768", "#32764"]]));
    }
    for op in ["b", "bl", "b.eq", "b.lt"] {
        lines.extend(combos(&format!("{} $0", op), &[&["lab", "#8", "#4", "#2", "#134217728", "#-134217728", "#134217724", "#1048576"]]));
    }
    for op in ["adr", "adrp"] {
        lines.extend(combos(&format!("{} $0, $1", op), &[&["x0", "w1", "xzr", "sp"], &["lab", "#0", "#4", "#4096", "#8192", "#-4096", "#1048575", "#1048576", "#4294967296", "#4294963200", "#-4294967296"]]));
    }
    for op in ["svc", "hvc", "smc", "brk", "hlt", "udf", "hint"] {
        lines.extend(combos(&format!("{} $0", op), &[&["#0", "#1", "#255", "#127", "#128", "#65535", "#65536", "#-1"]]));
    }
    for op in ["dmb", "dsb"] {
        lines.extend(combos(&format!("{} $0", op), &[&["sy", "ish", "ishst", "ishld", "ld", "st", "osh", "oshld", "oshst", "nsh", "nshld", "nshst", "#0", "#4", "#15", "#16", "isb", "foo"]]));
    }
    lines.extend(combos("isb $0", &[&["", "sy", "#0", "#15", "#16", "ish"]]));
    lines.extend(combos("clrex $0", &[&["", "#0", "#15", "#16"]]));
    lines.extend(combos("msr $0, #$1", &[&["daifset", "daifclr", "spsel", "uao", "pan", "dit", "ssbs", "tco", "fpcr"], &["0", "1", "7", "15", "16"]]));
    for op in ["dc", "ic", "tlbi", "at"] {
        lines.extend(combos(&format!("{} $0$1", op), &[&["zva", "cvac", "cvau", "civac", "ivac", "isw", "csw", "cisw", "cvap", "iallu", "ialluis", "ivau", "vmalle1is", "vae1is", "vmalle1", "s1e1r", "s1e0w", "foo"], &["", ", x0", ", xzr", ", w0", ", sp"]]));
    }
    for op in ["mrs", "msr"] {
        lines.extend(combos(&format!("{} $0", op), &[&["x0, nzcv", "x0, fpcr", "xzr, tpidr_el0", "w0, fpcr", "sp, fpsr", "x0, x1", "x0, s3_3_c4_c4_0", "x0, s4_3_c4_c4_0", "x0, s3_8_c4_c4_0", "x0, s3_3_c16_c4_0", "x0, s3_3_c4_c4_8", "fpcr, x0", "fpcr, xzr", "tpidr_el0, w0", "s3_3_c4_c4_0, x0", "nzcv, #1", "ctr_el0, x0"]]));
    }
    matrix("branches_and_system", &lines);
}

#[test]
fn unencodable_logical_immediates_are_errors() {
    for l in ["eor x0, x0, #0x55", "and x0, x1, #0", "orr x0, x1, #-1", "and w0, w1, #0xffffffff", "orr x0, x1, #0x12345"] {
        let e = ours_bytes(l).unwrap_err();
        assert!(e.contains("logical immediate"), "{}: {}", l, e);
    }
}
