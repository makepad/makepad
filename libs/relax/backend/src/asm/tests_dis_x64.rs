//! Tests of the x64 decoder ([`super::dis_x64`]): classes and spelling of hand-picked encodings,
//! the round trip with this module's assembler over its own test corpus, and (gated on
//! `DIS_CORPUS=<ELF or Mach-O x86-64 binary>`, `--ignored`) the decoder over a whole real binary,
//! compared with the system disassembler when it is installed.
use super::dis::{decode, Class, Insn};
use super::*;
use std::collections::HashMap;

fn asm(src: &str) -> Result<Vec<u8>, AsmError> {
    assemble(Arch::X64, Format::Elf, Kind::Global, &[Piece::Text(src.to_string())], &[], false).map(|o| o.sections[0].bytes.clone())
}

fn dec(b: &[u8]) -> Vec<Insn> {
    decode(Arch::X64, b, 0x1000)
}

fn one(b: &[u8]) -> Insn {
    let v = dec(b);
    assert_eq!(v.len(), 1, "{:02x?} decoded to {:?}", b, v);
    assert_eq!(v[0].len as usize, b.len(), "{:02x?}: {:?}", b, v[0]);
    v[0].clone()
}

fn chk(b: &[u8], text: &str, class: Class) -> Insn {
    let i = one(b);
    assert_eq!((i.text().as_str(), i.class), (text, class), "{:02x?}", b);
    i
}

#[test]
fn dis_x64_classes() {
    let i = chk(&[0x48, 0x89, 0x44, 0x24, 0x08], "mov qword ptr [rsp+0x8], rax", Class::StackStore);
    assert!(i.mem && i.stack);
    let i = chk(&[0x48, 0x8b, 0x45, 0xf8], "mov rax, qword ptr [rbp-0x8]", Class::StackLoad);
    assert!(i.mem && i.stack);
    let i = chk(&[0x48, 0x8b, 0x00], "mov rax, qword ptr [rax]", Class::Load);
    assert!(i.mem && !i.stack);
    chk(&[0x89, 0x08], "mov dword ptr [rax], ecx", Class::Store);
    chk(&[0x31, 0xc0], "xor eax, eax", Class::MovImm);
    chk(&[0x31, 0xc8], "xor eax, ecx", Class::Alu);
    chk(&[0x48, 0x89, 0xe5], "mov rbp, rsp", Class::MovReg);
    chk(&[0x0f, 0xb6, 0xc1], "movzx eax, cl", Class::MovReg);
    chk(&[0x48, 0x63, 0xc1], "movsxd rax, ecx", Class::MovReg);
    chk(&[0x48, 0x83, 0xec, 0x28], "sub rsp, 0x28", Class::Alu);
    chk(&[0x48, 0x83, 0xe4, 0xf0], "and rsp, -0x10", Class::Alu);
    chk(&[0x48, 0x01, 0x4c, 0x24, 0x10], "add qword ptr [rsp+0x10], rcx", Class::StackStore);
    chk(&[0x48, 0x3b, 0x45, 0xf0], "cmp rax, qword ptr [rbp-0x10]", Class::StackLoad);
    chk(&[0x48, 0x85, 0xc0], "test rax, rax", Class::Cmp);
    chk(&[0x0f, 0x94, 0xc0], "sete al", Class::Select);
    chk(&[0x48, 0x0f, 0x44, 0xc1], "cmove rax, rcx", Class::Select);
    chk(&[0x48, 0x8d, 0x44, 0x88, 0x10], "lea rax, [rax+rcx*4+0x10]", Class::Addr);
    chk(&[0x50], "push rax", Class::PushPop);
    chk(&[0x41, 0x5f], "pop r15", Class::PushPop);
    chk(&[0xc3], "ret", Class::Ret);
    chk(&[0xcc], "int3", Class::Trap);
    chk(&[0x0f, 0x0b], "ud2", Class::Trap);
    chk(&[0x90], "nop", Class::Nop);
    chk(&[0x0f, 0x1f, 0x44, 0x00, 0x00], "nop dword ptr [rax+rax*1+0x0]", Class::Nop);
    chk(&[0x66, 0x0f, 0x1f, 0x44, 0x00, 0x00], "nop word ptr [rax+rax*1+0x0]", Class::Nop);
    chk(&[0xf3, 0x0f, 0x1e, 0xfa], "endbr64", Class::Nop);
    chk(&[0xf0, 0x48, 0x0f, 0xb1, 0x0a], "lock cmpxchg qword ptr [rdx], rcx", Class::Atomic);
    chk(&[0xf0, 0x83, 0x00, 0x01], "lock add dword ptr [rax], 0x1", Class::Atomic);
    chk(&[0x48, 0x87, 0x08], "xchg qword ptr [rax], rcx", Class::Atomic);
    chk(&[0x0f, 0xae, 0xf0], "mfence", Class::Atomic);
    chk(&[0xff, 0xd0], "call rax", Class::IndirectCall);
    chk(&[0xff, 0xe0], "jmp rax", Class::IndirectBranch);
    chk(&[0xff, 0x24, 0xc5, 0x00, 0x10, 0x00, 0x00], "jmp qword ptr [rax*8+0x1000]", Class::IndirectBranch);
    chk(&[0x48, 0xf7, 0xf1], "div rcx", Class::Div);
    chk(&[0x48, 0x0f, 0xaf, 0xc1], "imul rax, rcx", Class::Mul);
    chk(&[0x48, 0x6b, 0xc1, 0x0c], "imul rax, rcx, 0xc", Class::Mul);
    chk(&[0x48, 0xb8, 1, 2, 3, 4, 5, 6, 7, 8], "movabs rax, 0x807060504030201", Class::MovImm);
    chk(&[0xb8, 0xff, 0xff, 0xff, 0xff], "mov eax, 0xffffffff", Class::MovImm);
    chk(&[0x48, 0xc7, 0x44, 0x24, 0x08, 0xff, 0xff, 0xff, 0xff], "mov qword ptr [rsp+0x8], -0x1", Class::StackStore);
    chk(&[0xf3, 0x48, 0xa5], "rep movsq", Class::Other);
    chk(&[0xf3, 0xaa], "rep stosb", Class::Store);
    chk(&[0x64, 0x48, 0x8b, 0x04, 0x25, 0, 0, 0, 0], "mov rax, qword ptr fs:[0x0]", Class::Load);
    chk(&[0x40, 0x88, 0xf0], "mov al, sil", Class::MovReg);
    chk(&[0x88, 0xe0], "mov al, ah", Class::MovReg);
    // direct branches and rip-relative operands are absolute
    let i = chk(&[0xe8, 0x10, 0x00, 0x00, 0x00], "call 0x1015", Class::Call);
    assert_eq!(i.target, Some(0x1015));
    let i = chk(&[0xeb, 0xfe], "jmp 0x1000", Class::Branch);
    assert_eq!(i.target, Some(0x1000));
    let i = chk(&[0x0f, 0x85, 0x00, 0x01, 0x00, 0x00], "jne 0x1106", Class::CondBranch);
    assert_eq!(i.target, Some(0x1106));
    let i = chk(&[0x48, 0x8d, 0x05, 0x10, 0x00, 0x00, 0x00], "lea rax, [rip+0x10]", Class::Addr);
    assert_eq!(i.pc_ref, Some(0x1017));
    assert!(!i.mem);
    let i = chk(&[0x48, 0x8b, 0x05, 0xf0, 0xff, 0xff, 0xff], "mov rax, qword ptr [rip-0x10]", Class::Load);
    assert_eq!(i.pc_ref, Some(0x1007 - 0x10));
    // vectors
    chk(&[0x66, 0x0f, 0xef, 0xc0], "pxor xmm0, xmm0", Class::MovImm);
    chk(&[0xc5, 0xf9, 0xef, 0xc0], "vpxor xmm0, xmm0, xmm0", Class::MovImm);
    chk(&[0x0f, 0x28, 0xc1], "movaps xmm0, xmm1", Class::MovReg);
    chk(&[0x0f, 0x29, 0x44, 0x24, 0x10], "movaps xmmword ptr [rsp+0x10], xmm0", Class::StackStore);
    chk(&[0xf3, 0x0f, 0x10, 0x45, 0xf8], "movss xmm0, dword ptr [rbp-0x8]", Class::StackLoad);
    chk(&[0xf2, 0x0f, 0x58, 0xc1], "addsd xmm0, xmm1", Class::Fp);
    chk(&[0x0f, 0x58, 0xc1], "addps xmm0, xmm1", Class::Vector);
    chk(&[0xc5, 0xfc, 0x58, 0xc1], "vaddps ymm0, ymm0, ymm1", Class::Vector);
    chk(&[0xc5, 0xfd, 0x6f, 0x00], "vmovdqa ymm0, ymmword ptr [rax]", Class::Load);
    chk(&[0xc4, 0xe2, 0x71, 0xa9, 0xc2], "vfmadd213ss xmm0, xmm1, xmm2", Class::Fp);
    chk(&[0xc4, 0xe2, 0x70, 0xf2, 0xc2], "andn eax, ecx, edx", Class::Alu);
    chk(&[0xc5, 0xf8, 0x77], "vzeroupper", Class::Vector);
    chk(&[0xf2, 0x48, 0x0f, 0x2a, 0xc0], "cvtsi2sd xmm0, rax", Class::Fp);
    chk(&[0x66, 0x48, 0x0f, 0x6e, 0xc0], "movq xmm0, rax", Class::MovReg);
    chk(&[0x66, 0x0f, 0x73, 0xd8, 0x04], "psrldq xmm0, 0x4", Class::Vector);
    chk(&[0x0f, 0xc2, 0xc1, 0x01], "cmpltps xmm0, xmm1", Class::Vector);
    // EVEX
    chk(&[0x62, 0xf1, 0xfe, 0x48, 0x6f, 0x00], "vmovdqu64 zmm0, zmmword ptr [rax]", Class::Load);
    chk(&[0x62, 0xf1, 0x7d, 0x48, 0xef, 0xc0], "vpxord zmm0, zmm0, zmm0", Class::MovImm);
    chk(&[0x62, 0xf1, 0x6c, 0x49, 0x58, 0xcb], "vaddps zmm1 {k1}, zmm2, zmm3", Class::Vector);
    chk(&[0x62, 0xf1, 0x7c, 0x48, 0x10, 0x40, 0x01], "vmovups zmm0, zmmword ptr [rax+0x40]", Class::Load);
    chk(&[0x62, 0xf1, 0x7c, 0x58, 0x58, 0x40, 0x01], "vaddps zmm0, zmm0, dword ptr [rax+0x4]{1to16}", Class::Vector);
}

#[test]
fn dis_x64_unknown_and_resync() {
    // an x87 instruction keeps its length; an invalid opcode is one byte and decoding goes on
    let v = dec(&[0xd9, 0xc0, 0x90]);
    assert_eq!(v.len(), 2);
    assert_eq!((v[0].class, v[0].len), (Class::Unknown, 2));
    assert_eq!(v[1].class, Class::Nop);
    let v = dec(&[0x06, 0x90]);
    assert_eq!((v[0].class, v[0].len), (Class::Unknown, 1));
    assert_eq!(v[1].class, Class::Nop);
    // truncated
    let v = dec(&[0x48, 0x8b]);
    assert_eq!((v.len(), v[0].class), (1, Class::Unknown));
    let v = dec(&[]);
    assert!(v.is_empty());
}

fn is_plain(s: &str) -> bool {
    !s.contains('\n') && !s.contains('@') && !s.contains("foo") && !s.contains('.')
}

/// Every instruction the assembler's own test corpus accepts decodes to one instruction whose
/// text assembles and decodes to the same text again.
#[test]
fn dis_x64_roundtrip_corpus() {
    let mut texts = super::tests_x64::corpus();
    texts.extend(super::tests_x64::sse_corpus());
    let (mut n, mut skipped, mut bytes_differ) = (0, 0, 0);
    let mut fails: Vec<String> = Vec::new();
    for src in texts.iter().filter(|s| is_plain(s)) {
        let Ok(b) = asm(src) else {
            skipped += 1;
            continue;
        };
        if b.is_empty() {
            continue;
        }
        let v = dec(&b);
        if v.len() != 1 || v[0].len as usize != b.len() || v[0].class == Class::Unknown {
            fails.push(format!("`{}` {:02x?} decoded as {:?}", src, b, v.iter().map(|i| format!("{} [{}]", i.text(), i.len)).collect::<Vec<_>>()));
            continue;
        }
        n += 1;
        let t = v[0].text();
        match asm(&t) {
            Err(e) => fails.push(format!("`{}` -> `{}` does not assemble: {}", src, t, e)),
            Ok(b2) => {
                if b2 != b {
                    bytes_differ += 1;
                }
                let v2 = dec(&b2);
                if v2.len() != 1 || v2[0].text() != t {
                    fails.push(format!("`{}` -> `{}` -> {:?}", src, t, v2.iter().map(|i| i.text()).collect::<Vec<_>>()));
                }
            }
        }
    }
    eprintln!("dis_x64 roundtrip: {} instructions, {} not assembled, {} re-encoded differently, {} failures", n, skipped, bytes_differ, fails.len());
    for f in fails.iter().take(80) {
        eprintln!("  {}", f);
    }
    assert!(fails.is_empty(), "{} round trip failures", fails.len());
}

/// The assembler's canonical spelling is the decoder's: `text -> bytes -> text` is the identity.
#[test]
fn dis_x64_canonical_text() {
    let mut fails = Vec::new();
    for t in [
        "mov rax, rbx",
        "mov eax, dword ptr [rsp+0x8]",
        "mov qword ptr [rax+rcx*8+0x10], r15",
        "mov byte ptr [r13+0x0], 0x7",
        "movzx eax, word ptr [rdi]",
        "lea r9, [rax+rax*2]",
        "add rsp, -0x80",
        "imul r10, qword ptr [rsi+0x20], 0x1000",
        "shl rax, 0x5",
        "shr edx, cl",
        "sar r8, 1",
        "bt rax, 0x3",
        "cmovl ecx, edx",
        "setne r9b",
        "push 0x12345678",
        "movabs r11, 0x1122334455667788",
        "vaddps ymm1, ymm2, ymmword ptr [rax+0x20]",
        "vfmadd231pd ymm0, ymm1, ymm2",
        "vpshufb xmm3, xmm4, xmm5",
        "pshufd xmm0, xmm1, 0x1b",
        "movd xmm0, eax",
        "pextrd eax, xmm1, 0x2",
        "vextracti128 xmm0, ymm1, 0x1",
        "vbroadcastss ymm0, dword ptr [rax]",
        "cvttsd2si rax, xmm0",
        "vcvtps2pd ymm0, xmm1",
        "popcnt rax, rcx",
        "tzcnt eax, dword ptr [rbx]",
    ] {
        match asm(t) {
            Err(e) => fails.push(format!("`{}`: {}", t, e)),
            Ok(b) => {
                let v = dec(&b);
                if v.len() != 1 || v[0].text() != t {
                    fails.push(format!("`{}` {:02x?} -> {:?}", t, b, v.iter().map(|i| i.text()).collect::<Vec<_>>()));
                }
            }
        }
    }
    for f in &fails {
        eprintln!("  {}", f);
    }
    assert!(fails.is_empty());
}

// ---------------------------------------------------------------- real binaries

/// Executable bytes of an ELF (`.text`) or Mach-O (`__text`) x86-64 file: (virtual address, bytes).
fn text_section(f: &[u8]) -> (u64, Vec<u8>) {
    let u16at = |o: usize| u16::from_le_bytes([f[o], f[o + 1]]) as usize;
    let u32at = |o: usize| u32::from_le_bytes(f[o..o + 4].try_into().unwrap()) as usize;
    let u64at = |o: usize| u64::from_le_bytes(f[o..o + 8].try_into().unwrap());
    if f.starts_with(b"\x7fELF") {
        let shoff = u64at(0x28) as usize;
        let (shentsize, shnum, shstrndx) = (u16at(0x3A), u16at(0x3C), u16at(0x3E));
        let sh = |i: usize| shoff + i * shentsize;
        let stroff = u64at(sh(shstrndx) + 0x18) as usize;
        for i in 0..shnum {
            let s = stroff + u32at(sh(i));
            let mut e = s;
            while f[e] != 0 {
                e += 1;
            }
            if &f[s..e] == b".text" {
                let addr = u64at(sh(i) + 0x10);
                let off = u64at(sh(i) + 0x18) as usize;
                let size = u64at(sh(i) + 0x20) as usize;
                return (addr, f[off..off + size].to_vec());
            }
        }
        panic!("no .text section");
    }
    // 64-bit little endian Mach-O
    assert_eq!(u32at(0), 0xfeedfacf, "not an ELF or 64-bit Mach-O file");
    let ncmds = u32at(16);
    let mut o = 32;
    for _ in 0..ncmds {
        let (cmd, size) = (u32at(o), u32at(o + 4));
        if cmd == 0x19 {
            let nsects = u32at(o + 64);
            for s in 0..nsects {
                let so = o + 72 + s * 80;
                if f[so..so + 16].starts_with(b"__text\0") {
                    let addr = u64at(so + 32);
                    let size = u64at(so + 40) as usize;
                    let off = u32at(so + 48);
                    return (addr, f[off..off + size].to_vec());
                }
            }
        }
        o += size;
    }
    panic!("no __text section");
}

/// The opcode bytes (after prefixes) of an unknown instruction, as a histogram key.
fn unknown_key(code: &[u8], i: &Insn) -> String {
    let b = &code[i.off as usize..i.off as usize + i.len as usize];
    let mut k = 0;
    while k < b.len() && matches!(b[k], 0x66 | 0x67 | 0xf0 | 0xf2 | 0xf3 | 0x2e | 0x3e | 0x26 | 0x36 | 0x64 | 0x65 | 0x40..=0x4f) {
        k += 1;
    }
    let end = (k + 4).min(b.len());
    b[k.min(b.len())..end].iter().map(|x| format!("{:02x}", x)).collect::<Vec<_>>().join(" ")
}

#[test]
#[ignore]
fn dis_x64_binary_corpus() {
    let Ok(path) = std::env::var("DIS_CORPUS") else { return };
    let f = std::fs::read(&path).unwrap();
    let (addr, code) = text_section(&f);
    let t0 = std::time::Instant::now();
    let v = decode(Arch::X64, &code, addr);
    let dt = t0.elapsed();
    let mut unk_bytes = 0usize;
    let mut unk_n = 0usize;
    let mut hist: HashMap<String, (usize, usize)> = HashMap::new();
    let mut classes: HashMap<Class, usize> = HashMap::new();
    for i in &v {
        *classes.entry(i.class).or_default() += 1;
        if i.class == Class::Unknown {
            unk_n += 1;
            unk_bytes += i.len as usize;
            let e = hist.entry(unknown_key(&code, i)).or_default();
            e.0 += 1;
            e.1 += i.len as usize;
        }
    }
    eprintln!("dis_x64 corpus {}: {} bytes, {} instructions in {:?} ({:.1} M insn/s), unknown {} insns {} bytes = {:.4}%", path, code.len(), v.len(), dt, v.len() as f64 / dt.as_secs_f64() / 1e6, unk_n, unk_bytes, unk_bytes as f64 * 100.0 / code.len() as f64);
    let mut h: Vec<_> = hist.into_iter().collect();
    h.sort_by(|a, b| b.1 .1.cmp(&a.1 .1));
    for (k, (n, bytes)) in h.iter().take(30) {
        eprintln!("  unknown {:<14} {:>8} insns {:>9} bytes", k, n, bytes);
    }
    let mut cs: Vec<_> = classes.into_iter().collect();
    cs.sort_by(|a, b| b.1.cmp(&a.1));
    eprintln!("  classes: {}", cs.iter().map(|(c, n)| format!("{}={}", c.name(), n)).collect::<Vec<_>>().join(" "));
    assert!((unk_bytes as f64) < code.len() as f64 * 0.001, "unknown bytes above 0.1%");
}

/// First word of an instruction after the prefixes.
fn norm_mnem(text: &str) -> String {
    let mut words = text.split_whitespace().peekable();
    let mut m = String::new();
    while let Some(w) = words.next() {
        let w = w.to_ascii_lowercase();
        if matches!(w.as_str(), "lock" | "rep" | "repz" | "repe" | "repnz" | "repne" | "bnd" | "notrack" | "cs" | "ds" | "es" | "ss" | "data16" | "addr32" | "xacquire" | "xrelease") && words.peek().is_some() {
            continue;
        }
        m = w;
        break;
    }
    match m.as_str() {
        "sal" => "shl".to_string(),
        "retq" => "ret".to_string(),
        "cltq" => "cdqe".to_string(),
        "jz" => "je".to_string(),
        "jnz" => "jne".to_string(),
        "pclmullqlqdq" | "pclmulhqlqdq" | "pclmullqhqdq" | "pclmulhqhqdq" => "pclmulqdq".to_string(),
        _ => m,
    }
}

/// Compares the decoder with the system disassembler over a whole .text: instruction starts and
/// lengths must agree, mnemonics after a small normalisation.
#[test]
#[ignore]
fn dis_x64_vs_system_disassembler() {
    let Ok(path) = std::env::var("DIS_CORPUS") else { return };
    let f = std::fs::read(&path).unwrap();
    let (addr, code) = text_section(&f);
    let v = decode(Arch::X64, &code, addr);
    let Ok(out) = std::process::Command::new("objdump").args(["-d", "-M", "intel", "--no-show-raw-insn", "-j", ".text"]).arg(&path).output() else {
        eprintln!("no system disassembler; skipped");
        return;
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut ref_insns: Vec<(u64, String)> = Vec::new();
    for l in text.lines() {
        let Some((a, rest)) = l.split_once(":\t") else { continue };
        let Ok(a) = u64::from_str_radix(a.trim(), 16) else { continue };
        ref_insns.push((a, rest.trim().to_string()));
    }
    let mut by_addr: HashMap<u64, usize> = HashMap::with_capacity(v.len());
    for (k, i) in v.iter().enumerate() {
        by_addr.insert(addr + i.off as u64, k);
    }
    let (mut total, mut len_bad, mut start_bad, mut mnem_bad) = (0usize, 0usize, 0usize, 0usize);
    let mut samples: HashMap<String, (usize, String)> = HashMap::new();
    let end = addr + code.len() as u64;
    for (k, (a, t)) in ref_insns.iter().enumerate() {
        total += 1;
        let next = ref_insns.get(k + 1).map(|x| x.0).unwrap_or(end);
        let Some(&j) = by_addr.get(a) else {
            start_bad += 1;
            samples.entry(format!("start {}", norm_mnem(t))).or_insert((0, t.clone())).0 += 1;
            continue;
        };
        let i = &v[j];
        if addr + i.off as u64 + i.len as u64 != next {
            len_bad += 1;
            samples.entry(format!("len {}", norm_mnem(t))).or_insert((0, format!("{}   <-> ours {} [{}]", t, i.text(), i.len))).0 += 1;
            continue;
        }
        let mut rm = norm_mnem(t);
        if rm == "xchg" && t.contains("ax,ax") {
            rm = "nop".to_string();
        }
        let mut om = norm_mnem(&i.text());
        // movsq / stosb ... against movs / stos
        for s in ["movs", "cmps", "stos", "lods", "scas"] {
            if om.len() == s.len() + 1 && om.starts_with(s) && rm == s {
                om = s.to_string();
            }
        }
        if i.class == Class::Unknown {
            mnem_bad += 1;
            samples.entry(format!("unknown {}", rm)).or_insert((0, t.clone())).0 += 1;
        } else if rm != om {
            mnem_bad += 1;
            samples.entry(format!("mnem {} vs {}", rm, om)).or_insert((0, format!("{}   <-> ours {}", t, i.text()))).0 += 1;
        }
    }
    eprintln!(
        "dis_x64 vs system disassembler: {} instructions; starts missing {} ({:.4}%), length mismatches {} ({:.4}%), mnemonic mismatches {} ({:.4}%); agreement {:.4}%",
        total,
        start_bad,
        start_bad as f64 * 100.0 / total as f64,
        len_bad,
        len_bad as f64 * 100.0 / total as f64,
        mnem_bad,
        mnem_bad as f64 * 100.0 / total as f64,
        (total - start_bad - len_bad - mnem_bad) as f64 * 100.0 / total as f64
    );
    let mut s: Vec<_> = samples.into_iter().collect();
    s.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
    for (k, (n, ex)) in s.iter().take(50) {
        eprintln!("  {:>7}  {:<28} e.g. {}", n, k, ex);
    }
}
