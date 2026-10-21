//! x86-64 encoder tests: an oracle comparison against clang (LLVM's integrated assembler) and
//! unit tests of placeholders, fixups and errors.
use super::x64::parse_reg;
use super::*;
use std::process::Command;

fn asm_intel(src: &str) -> Result<Output, AsmError> {
    assemble(Arch::X64, Format::Elf, Kind::Global, &[Piece::Text(src.to_string())], &[], false)
}

fn asm_att(src: &str) -> Result<Output, AsmError> {
    assemble(Arch::X64, Format::Elf, Kind::Global, &[Piece::Text(src.to_string())], &[], true)
}

fn text(o: &Output) -> Vec<u8> {
    o.sections[0].bytes.clone()
}

fn bytes(src: &str) -> Vec<u8> {
    text(&asm_intel(src).unwrap_or_else(|e| panic!("`{}`: {}", src, e)))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{:02x}", x)).collect::<Vec<_>>().join(" ")
}

// ---------------------------------------------------------------- ELF reader

fn elf_sections(b: &[u8]) -> Vec<(String, Vec<u8>)> {
    let u16at = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]) as usize;
    let u32at = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as usize;
    let u64at = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap()) as usize;
    let shoff = u64at(0x28);
    let shentsize = u16at(0x3A);
    let shnum = u16at(0x3C);
    let shstrndx = u16at(0x3E);
    let sh = |i: usize| shoff + i * shentsize;
    let stroff = u64at(sh(shstrndx) + 0x18);
    let mut out = Vec::new();
    for i in 0..shnum {
        let name_off = u32at(sh(i));
        let mut e = stroff + name_off;
        while b[e] != 0 {
            e += 1;
        }
        let name = String::from_utf8_lossy(&b[stroff + name_off..e]).into_owned();
        let ty = u32at(sh(i) + 4);
        let off = u64at(sh(i) + 0x18);
        let size = u64at(sh(i) + 0x20);
        if ty == 1 {
            out.push((name, b[off..off + size].to_vec()));
        }
    }
    out
}

static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn clang_ok() -> bool {
    Command::new("clang").arg("--version").output().is_ok_and(|o| o.status.success())
}

/// Assembles each block in its own section with clang; `@L` becomes `{disp32} ` there.
fn clang_blocks(blocks: &[String], att: bool) -> Vec<Result<Vec<u8>, String>> {
    let dir = std::env::temp_dir();
    let pid = std::process::id();
    let mut res: Vec<Result<Vec<u8>, String>> = Vec::new();
    // clang stops at the first error; assemble in chunks and fall back to single blocks
    let one = |chunk: &[(usize, &String)], tag: &str| -> Result<Vec<Vec<u8>>, String> {
        let mut s = String::new();
        s.push_str(if att { ".att_syntax prefix\n" } else { ".intel_syntax noprefix\n" });
        for (i, b) in chunk {
            s.push_str(&format!(".section s{},\"ax\"\n{}\n", i, b.replace("@L", "{disp32} ")));
        }
        let uniq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tag = format!("{}_{}", tag, uniq);
        let sp = dir.join(format!("relax_x64_{}_{}.s", pid, tag));
        let op = dir.join(format!("relax_x64_{}_{}.o", pid, tag));
        std::fs::write(&sp, s).unwrap();
        let out = Command::new("clang").args(["-c", "-x", "assembler", "--target=x86_64-unknown-linux-gnu", "-o"]).arg(&op).arg(&sp).output().unwrap();
        let _ = std::fs::remove_file(&sp);
        if !out.status.success() {
            let _ = std::fs::remove_file(&op);
            return Err(String::from_utf8_lossy(&out.stderr).into_owned());
        }
        let data = std::fs::read(&op).unwrap();
        let _ = std::fs::remove_file(&op);
        let secs = elf_sections(&data);
        Ok(chunk.iter().map(|(i, _)| secs.iter().find(|(n, _)| *n == format!("s{}", i)).map(|x| x.1.clone()).unwrap_or_default()).collect())
    };
    let items: Vec<(usize, &String)> = blocks.iter().enumerate().collect();
    for (ci, chunk) in items.chunks(200).enumerate() {
        match one(chunk, &format!("c{}", ci)) {
            Ok(v) => res.extend(v.into_iter().map(Ok)),
            Err(_) => {
                for (k, it) in chunk.iter().enumerate() {
                    match one(std::slice::from_ref(it), &format!("c{}_{}", ci, k)) {
                        Ok(mut v) => res.push(Ok(v.remove(0))),
                        Err(e) => res.push(Err(e.lines().find(|l| l.contains("error")).unwrap_or("error").to_string())),
                    }
                }
            }
        }
    }
    res
}

/// Compares our bytes with clang's for every block; returns the mismatch report.
fn oracle(blocks: &[String], att: bool) -> Vec<String> {
    let clang = clang_blocks(blocks, att);
    let mut bad = Vec::new();
    for (b, c) in blocks.iter().zip(clang) {
        let ours = {
            let src = b.replace("@L", "");
            if att { asm_att(&src) } else { asm_intel(&src) }
        };
        match (ours, c) {
            (Ok(o), Ok(c)) => {
                let t = text(&o);
                if t != c {
                    bad.push(format!("`{}`: ours {} / clang {}", b.replace('\n', " ; "), hex(&t), hex(&c)));
                }
            }
            (Err(e), Ok(c)) => bad.push(format!("`{}`: ours ERROR {} / clang {}", b.replace('\n', " ; "), e.msg, hex(&c))),
            (Ok(o), Err(e)) => bad.push(format!("`{}`: ours {} / clang rejects ({})", b.replace('\n', " ; "), hex(&text(&o)), e)),
            (Err(_), Err(_)) => {}
        }
    }
    bad
}

// ---------------------------------------------------------------- corpus

const R64: [&str; 10] = ["rax", "rcx", "rsp", "rbp", "rsi", "r8", "r12", "r13", "r15", "rdi"];
const R32: [&str; 10] = ["eax", "ecx", "esp", "ebp", "esi", "r8d", "r12d", "r13d", "r15d", "edi"];
const R16: [&str; 10] = ["ax", "cx", "sp", "bp", "si", "r8w", "r12w", "r13w", "r15w", "di"];
const R8: [&str; 10] = ["al", "cl", "spl", "bpl", "sil", "r8b", "r12b", "r13b", "r15b", "dil"];
const MEMS: [&str; 22] = [
    "[rax]", "[rsp]", "[rbp]", "[r12]", "[r13]", "[rax+8]", "[rax-8]", "[rax+0x1000]", "[rax+rcx*4]", "[rsp+rax*8+16]", "[rbp+r13*2+0x200]", "[r12+r15*1]", "[rcx*8+0x10]", "[0x10]", "gs:[8]", "fs:[0x1478]", "[rip+0x20]", "[rip+foo]", "[eax]", "[r13+r12*4]", "[rax+rax]", "[foo]",
];

fn regsets() -> Vec<(&'static [&'static str], &'static str, u16)> {
    vec![(&R8, "byte", 8), (&R16, "word", 16), (&R32, "dword", 32), (&R64, "qword", 64)]
}

fn imms(bits: u16) -> Vec<&'static str> {
    match bits {
        8 => vec!["0", "1", "-1", "127", "-128", "255", "0x80"],
        16 => vec!["0", "1", "-1", "127", "128", "-128", "-129", "0x1234", "0x7fff", "0xffff"],
        32 => vec!["0", "1", "-1", "127", "128", "-128", "-129", "0x1234", "0x12345678", "0x7fffffff", "0xffffffff", "0x80000000"],
        _ => vec!["0", "1", "-1", "127", "128", "-128", "-129", "0x1234", "0x12345678", "0x7fffffff", "-0x80000000"],
    }
}

fn corpus() -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    let mut p = |s: String| v.push(s);
    // ---- two-operand integer ALU
    for m in ["add", "or", "adc", "sbb", "and", "sub", "xor", "cmp", "mov", "test"] {
        for (rs, kw, bits) in regsets() {
            for (i, a) in rs.iter().enumerate() {
                let b = rs[(i + 3) % rs.len()];
                p(format!("{} {}, {}", m, a, b));
                p(format!("{} {}, {}", m, a, a));
                for mem in MEMS {
                    p(format!("{} {}, {} ptr {}", m, a, kw, mem));
                    p(format!("{} {} ptr {}, {}", m, kw, mem, a));
                    if m == "test" {
                        p(format!("test {}, {} ptr {}", a, kw, mem));
                    }
                }
                for k in imms(bits) {
                    p(format!("{} {}, {}", m, a, k));
                }
            }
            for mem in MEMS {
                for k in imms(bits) {
                    p(format!("{} {} ptr {}, {}", m, kw, mem, k));
                }
            }
        }
    }
    for s in ["mov ah, bl", "mov ch, 5", "mov al, bh", "add dh, ah", "mov byte ptr [rax], ch", "mov bh, byte ptr [rcx+rdx]", "test ah, 1", "cmp bh, 7"] {
        p(s.to_string());
    }
    for k in ["0", "5", "-1", "0x7fffffff", "-0x80000000", "0x80000000", "0xffffffff", "0x100000000", "0x123456789abcdef0", "-0x123456789"] {
        for r in ["rax", "rcx", "r9", "r15"] {
            p(format!("mov {}, {}", r, k));
            p(format!("movabs {}, {}", r, k));
        }
    }
    for k in ["0", "5", "0x7fffffff", "0x80000000", "0xffffffff", "-1"] {
        for r in ["eax", "ecx", "r9d"] {
            p(format!("mov {}, {}", r, k));
        }
    }
    for k in ["0", "5", "0xffff", "-1"] {
        for r in ["ax", "cx", "r9w"] {
            p(format!("mov {}, {}", r, k));
        }
    }
    for k in ["0", "5", "0xff", "-1"] {
        for r in ["al", "cl", "sil", "r9b", "bh"] {
            p(format!("mov {}, {}", r, k));
        }
    }
    // ---- lea / movzx / movsx
    for (rs, _, _) in regsets().into_iter().skip(1) {
        for a in rs.iter() {
            for mem in MEMS {
                p(format!("lea {}, {}", a, mem));
            }
        }
    }
    for d in ["ax", "r9w", "eax", "r9d", "rax", "r15"] {
        for s in ["al", "cl", "sil", "r9b", "bh"] {
            p(format!("movzx {}, {}", d, s));
            p(format!("movsx {}, {}", d, s));
        }
        for mem in ["[rax]", "[rsp+8]", "[r13]", "gs:[8]"] {
            p(format!("movzx {}, byte ptr {}", d, mem));
            p(format!("movsx {}, byte ptr {}", d, mem));
        }
    }
    for d in ["eax", "r9d", "rax", "r15"] {
        for s in ["ax", "cx", "si", "r9w"] {
            p(format!("movzx {}, {}", d, s));
            p(format!("movsx {}, {}", d, s));
        }
        for mem in ["[rax]", "[rsp+8]", "[r13]"] {
            p(format!("movzx {}, word ptr {}", d, mem));
            p(format!("movsx {}, word ptr {}", d, mem));
        }
    }
    for s in ["eax", "r9d", "esp"] {
        p(format!("movsxd rax, {}", s));
        p(format!("movsxd r15, {}", s));
    }
    p("movsxd rax, dword ptr [rcx+4]".into());
    // ---- unary groups
    for m in ["inc", "dec", "neg", "not", "mul", "div", "idiv", "imul"] {
        for (rs, kw, _) in regsets() {
            for a in rs.iter() {
                p(format!("{} {}", m, a));
            }
            for mem in MEMS {
                p(format!("{} {} ptr {}", m, kw, mem));
            }
        }
    }
    p("inc ah".into());
    p("neg bh".into());
    for (rs, kw, bits) in regsets().into_iter().skip(1) {
        for (i, a) in rs.iter().enumerate() {
            let b = rs[(i + 4) % rs.len()];
            p(format!("imul {}, {}", a, b));
            for mem in MEMS {
                p(format!("imul {}, {} ptr {}", a, kw, mem));
            }
            for k in imms(bits) {
                p(format!("imul {}, {}, {}", a, b, k));
                p(format!("imul {}, {}", a, k));
                p(format!("imul {}, {} ptr [rax+rcx], {}", a, kw, k));
            }
        }
    }
    // ---- shifts
    for m in ["rol", "ror", "rcl", "rcr", "shl", "sal", "shr", "sar"] {
        for (rs, kw, _) in regsets() {
            for a in rs.iter() {
                p(format!("{} {}, 1", m, a));
                p(format!("{} {}, 5", m, a));
                p(format!("{} {}, cl", m, a));
                p(format!("{} {}", m, a));
            }
            for mem in ["[rax]", "[rsp+8]", "[r13]"] {
                p(format!("{} {} ptr {}, 1", m, kw, mem));
                p(format!("{} {} ptr {}, 7", m, kw, mem));
                p(format!("{} {} ptr {}, cl", m, kw, mem));
            }
        }
    }
    for m in ["shld", "shrd"] {
        for (rs, kw, _) in regsets().into_iter().skip(1) {
            for (i, a) in rs.iter().enumerate() {
                let b = rs[(i + 2) % rs.len()];
                p(format!("{} {}, {}, 3", m, a, b));
                p(format!("{} {}, {}, cl", m, a, b));
                p(format!("{} {} ptr [rax+8], {}, 5", m, kw, b));
                p(format!("{} {} ptr [rax+8], {}, cl", m, kw, b));
            }
        }
    }
    // ---- bit ops
    for m in ["bt", "bts", "btr", "btc"] {
        for (rs, kw, _) in regsets().into_iter().skip(1) {
            for (i, a) in rs.iter().enumerate() {
                let b = rs[(i + 2) % rs.len()];
                p(format!("{} {}, {}", m, a, b));
                p(format!("{} {}, 7", m, a));
                p(format!("{} {} ptr [rax+8], {}", m, kw, b));
                p(format!("{} {} ptr [rax+8], 3", m, kw));
            }
        }
    }
    for m in ["bsf", "bsr", "popcnt", "lzcnt", "tzcnt"] {
        for (rs, kw, _) in regsets().into_iter().skip(1) {
            for (i, a) in rs.iter().enumerate() {
                p(format!("{} {}, {}", m, a, rs[(i + 2) % rs.len()]));
                p(format!("{} {}, {} ptr [rax+rcx*2]", m, a, kw));
            }
        }
    }
    for r in ["eax", "ecx", "r9d", "rax", "rcx", "r12", "r15"] {
        p(format!("bswap {}", r));
    }
    // ---- xchg / cmpxchg / xadd
    for (rs, kw, _) in regsets() {
        for (i, a) in rs.iter().enumerate() {
            let b = rs[(i + 1) % rs.len()];
            p(format!("xchg {}, {}", a, b));
            p(format!("xchg {}, {} ptr [rax+8]", a, kw));
            p(format!("xchg {} ptr [rax+8], {}", kw, a));
            p(format!("cmpxchg {}, {}", a, b));
            p(format!("xadd {}, {}", a, b));
            p(format!("lock cmpxchg {} ptr [rdx+8], {}", kw, a));
            p(format!("lock xadd {} ptr gs:[0x10], {}", kw, a));
        }
    }
    p("xchg rax, rax".into());
    p("xchg eax, eax".into());
    p("xchg rbx, rax".into());
    p("xchg rax, rbx".into());
    p("xchg ax, bx".into());
    p("lock cmpxchg16b xmmword ptr [rdi]".into());
    p("lock cmpxchg16b [rdi]".into());
    p("cmpxchg8b qword ptr [rdi+8]".into());
    p("lock add dword ptr [rax], 1".into());
    p("lock inc qword ptr [rax]".into());
    p("lock dec dword ptr [rsp+4]".into());
    // ---- cmov / set / jcc names
    let ccs = ["o", "no", "b", "c", "nae", "ae", "nb", "nc", "e", "z", "ne", "nz", "be", "na", "a", "nbe", "s", "ns", "p", "pe", "np", "po", "l", "nge", "ge", "nl", "le", "ng", "g", "nle"];
    for c in ccs {
        p(format!("cmov{} eax, ecx", c));
        p(format!("cmov{} r9, qword ptr [rax+8]", c));
        p(format!("cmov{} cx, dx", c));
        p(format!("set{} al", c));
        p(format!("set{} r9b", c));
        p(format!("set{} sil", c));
        p(format!("set{} byte ptr [rax]", c));
        p(format!("@Lj{} 1f\nnop\n1:", c));
        p(format!("1:\nj{} 1b", c));
        p(format!("1:\nnop\nj{} 1b", c));
    }
    // ---- control flow
    p("jmp rax".into());
    p("jmp r9".into());
    p("jmp qword ptr [rax]".into());
    p("jmp qword ptr [rip+foo]".into());
    p("jmp [rax+rcx*8]".into());
    p("call rax".into());
    p("call r12".into());
    p("call qword ptr [rax+8]".into());
    p("call [rip+foo]".into());
    p("call foo".into());
    p("jmp foo".into());
    p("@Ljmp 1f\nnop\n1:".into());
    p("1:\njmp 1b".into());
    p("1:\nnop\njmp 1b".into());
    p("call 1f\n1:".into());
    p("2:\ncall 2b".into());
    p("foo:\njmp foo".into());
    p("ret".into());
    p("ret 8".into());
    p("ret 0x1234".into());
    // ---- push / pop
    for r in ["rax", "rcx", "rsp", "rbp", "r8", "r12", "r13", "r15", "ax", "r9w"] {
        p(format!("push {}", r));
        p(format!("pop {}", r));
    }
    for k in ["0", "1", "-1", "127", "128", "-128", "-129", "0x1234", "0x7fffffff", "-0x80000000"] {
        p(format!("push {}", k));
    }
    for mem in MEMS {
        p(format!("push qword ptr {}", mem));
        p(format!("pop qword ptr {}", mem));
        p(format!("push word ptr {}", mem));
        p(format!("pop word ptr {}", mem));
    }
    // ---- no-operand and small system forms
    for s in [
        "nop", "ud2", "int3", "hlt", "pause", "cpuid", "rdtsc", "rdtscp", "xgetbv", "syscall", "lfence", "mfence", "sfence", "cld", "std", "clc", "stc", "cmc", "cqo", "cdq", "cwd", "cdqe", "cwde", "cbw", "leave", "sahf", "lahf", "pushfq", "popfq", "vzeroupper", "vzeroall", "rep movsb", "rep stosb", "rep movsq", "rep stosq", "rep movsd", "rep stosd", "rep movsw", "rep stosw", "repe cmpsb", "repne scasb", "repne scasq", "repe cmpsq", "lodsb", "lodsq", "rep lodsb", "swapgs", "rdmsr", "wrmsr", "endbr64", "int 3", "int 0x80", "int 0x2e", "enter 16, 0", "enter 0x100, 3", "xlatb", "emms",
    ] {
        p(s.to_string());
    }
    for (rs, _, _) in regsets().into_iter().skip(1) {
        for a in rs.iter() {
            p(format!("rdrand {}", a));
            p(format!("rdseed {}", a));
            p(format!("nop {}", a));
        }
    }
    for mem in MEMS {
        for kw in ["word", "dword", "qword"] {
            p(format!("nop {} ptr {}", kw, mem));
        }
        for m in ["prefetchnta", "prefetcht0", "prefetcht1", "prefetcht2", "prefetchw", "clflush", "clflushopt", "clwb", "stmxcsr", "ldmxcsr", "fxsave", "fxrstor", "xsave", "xrstor", "xsaveopt", "fxsave64", "xsave64", "xrstor64", "vstmxcsr", "vldmxcsr"] {
            p(format!("{} {}", m, mem));
        }
    }
    for r in ["rax", "r9", "eax"] {
        for m in ["rdfsbase", "rdgsbase", "wrfsbase", "wrgsbase"] {
            p(format!("{} {}", m, r));
        }
    }
    // ---- the first-party inventory
    for s in [
        "mov rdx, rbp", "mov rcx, [rsp]", "mov r8, [rsp]", "mov r9, [rsp]", "mov r12, rsp", "mov rsp, rcx", "mov rsp, r12", "sub rsp, 32", "push qword ptr gs:[8]", "push qword ptr gs:[0x1478]", "mov gs:[8], rax", "mov gs:[8], r9", "pop qword ptr gs:[16]", "mov rax, qword ptr fs:[0]", "mov rcx, qword ptr fs:[0]", "mov r11, qword ptr fs:[0]", "mov rax, fs:[0]", "sqrtsd xmm0, xmm0", "sqrtss xmm1, xmm1", "vfmadd213sd xmm0, xmm1, xmm2", "vfmadd213ss xmm3, xmm4, xmm5", "vfmadd213sd xmm9, xmm10, xmm11",
    ] {
        p(s.to_string());
    }
    v
}

fn sse_corpus() -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    let mut p = |s: String| v.push(s);
    let xs = ["xmm0", "xmm1", "xmm7", "xmm8", "xmm15"];
    let sm = ["[rax]", "[rsp+8]", "[r13]", "[rax+rcx*4+0x100]", "[rip+foo]", "gs:[8]"];
    // legacy 2-operand A/B
    let two = [
        "addps", "addpd", "addss", "addsd", "subps", "subpd", "subss", "subsd", "mulps", "mulpd", "mulss", "mulsd", "divps", "divpd", "divss", "divsd", "minps", "minpd", "minss", "minsd", "maxps", "maxpd", "maxss", "maxsd", "sqrtps", "sqrtpd", "sqrtss", "sqrtsd", "rcpps", "rcpss", "rsqrtps", "rsqrtss", "andps", "andpd", "andnps", "andnpd", "orps", "orpd", "xorps", "xorpd", "unpcklps", "unpcklpd", "unpckhps", "unpckhpd", "haddps", "haddpd", "hsubps", "hsubpd", "addsubps", "addsubpd", "ucomiss", "ucomisd", "comiss", "comisd", "cvtss2sd", "cvtsd2ss", "cvtps2pd", "cvtpd2ps", "cvtdq2ps", "cvtps2dq", "cvttps2dq", "cvtdq2pd", "cvtpd2dq", "cvttpd2dq", "movhlps", "movlhps", "ptest", "pxor", "pand", "pandn", "por", "paddb", "paddw", "paddd", "paddq", "paddsb", "paddsw", "paddusb", "paddusw", "psubb", "psubw", "psubd", "psubq", "psubsb", "psubsw", "psubusb", "psubusw", "pcmpeqb", "pcmpeqw", "pcmpeqd", "pcmpgtb", "pcmpgtw", "pcmpgtd", "punpcklbw", "punpcklwd", "punpckldq", "punpckhbw", "punpckhwd", "punpckhdq", "punpcklqdq", "punpckhqdq", "packsswb", "packuswb", "packssdw", "pmullw", "pmulhw", "pmulhuw", "pmuludq", "pmaddwd", "pminub", "pmaxub", "pminsw", "pmaxsw", "pavgb", "pavgw", "psadbw", "psllw", "pslld", "psllq", "psrlw", "psrld", "psrlq", "psraw", "psrad", "pshufb", "phaddw", "phaddd", "phsubw", "phsubd", "psignb", "psignw", "psignd", "pmulhrsw", "pabsb", "pabsw", "pabsd", "pmovsxbw", "pmovsxwd", "pmovsxdq", "pmovzxbw", "pmovzxbd", "pmovzxwd", "pmovzxdq", "pmuldq", "pcmpeqq", "packusdw", "pcmpgtq", "pminsb", "pminsd", "pminuw", "pminud", "pmaxsb", "pmaxsd", "pmaxuw", "pmaxud", "pmulld", "aesenc", "aesenclast", "aesdec", "aesdeclast", "aesimc", "movddup", "movshdup", "movsldup", "movaps", "movups", "movapd", "movupd", "movdqa", "movdqu",
    ];
    for m in two {
        for d in xs {
            for s in xs {
                p(format!("{} {}, {}", m, d, s));
            }
        }
        if m == "movhlps" || m == "movlhps" {
            continue;
        }
        for mem in sm {
            p(format!("{} xmm3, {}", m, mem));
            p(format!("{} xmm12, {}", m, mem));
        }
    }
    for m in ["addss", "subss", "mulss", "divss", "sqrtss", "ucomiss", "comiss", "cvtss2sd"] {
        for mem in sm {
            p(format!("{} xmm3, dword ptr {}", m, mem));
        }
    }
    for m in ["addsd", "subsd", "mulsd", "divsd", "sqrtsd", "ucomisd", "comisd", "cvtsd2ss"] {
        for mem in sm {
            p(format!("{} xmm3, qword ptr {}", m, mem));
        }
    }
    // stores and moves
    for m in ["movaps", "movups", "movapd", "movupd", "movdqa", "movdqu", "movntps", "movntpd", "movntdq"] {
        for mem in sm {
            p(format!("{} xmmword ptr {}, xmm5", m, mem));
            p(format!("{} xmmword ptr {}, xmm13", m, mem));
        }
    }
    for m in ["movss", "movsd"] {
        for mem in sm {
            p(format!("{} xmm5, {}", m, mem));
            p(format!("{} {}, xmm5", mem, mem).replace(&format!("{} {}", mem, mem), &format!("{} {}", m, mem)));
            p(format!("{} xmm13, {}", m, mem));
        }
        p(format!("{} xmm1, xmm2", m));
        p(format!("{} xmm9, xmm10", m));
    }
    for m in ["movlps", "movhps", "movlpd", "movhpd"] {
        p(format!("{} xmm1, qword ptr [rax]", m));
        p(format!("{} qword ptr [rax], xmm1", m));
        p(format!("{} xmm9, qword ptr [r13]", m));
    }
    // movd / movq
    for r in ["eax", "ecx", "r9d", "r15d"] {
        p(format!("movd xmm0, {}", r));
        p(format!("movd xmm9, {}", r));
        p(format!("movd {}, xmm0", r));
        p(format!("movd {}, xmm9", r));
    }
    for r in ["rax", "rcx", "r9", "r15"] {
        p(format!("movq xmm0, {}", r));
        p(format!("movq xmm9, {}", r));
        p(format!("movq {}, xmm0", r));
        p(format!("movq {}, xmm9", r));
    }
    for mem in sm {
        p(format!("movd xmm0, dword ptr {}", mem));
        p(format!("movd dword ptr {}, xmm0", mem));
        p(format!("movq xmm0, qword ptr {}", mem));
        p(format!("movq qword ptr {}, xmm0", mem));
    }
    p("movq xmm1, xmm2".into());
    p("movq xmm9, xmm10".into());
    // int <-> float
    for m in ["cvtsi2ss", "cvtsi2sd"] {
        for r in ["eax", "r9d", "rax", "r15"] {
            p(format!("{} xmm0, {}", m, r));
            p(format!("{} xmm9, {}", m, r));
        }
        p(format!("{} xmm0, dword ptr [rax]", m));
        p(format!("{} xmm0, qword ptr [rax]", m));
    }
    for m in ["cvttss2si", "cvttsd2si", "cvtss2si", "cvtsd2si"] {
        for r in ["eax", "r9d", "rax", "r15"] {
            p(format!("{} {}, xmm0", m, r));
            p(format!("{} {}, xmm9", m, r));
        }
        p(format!("{} eax, {} ptr [rax]", m, if m.contains("ss") { "dword" } else { "qword" }));
        p(format!("{} rax, {} ptr [rax]", m, if m.contains("ss") { "dword" } else { "qword" }));
    }
    for m in ["movmskps", "movmskpd", "pmovmskb"] {
        p(format!("{} eax, xmm1", m));
        p(format!("{} r9d, xmm9", m));
    }
    // imm forms
    for m in ["shufps", "shufpd", "pshufd", "pshufhw", "pshuflw", "palignr", "pblendw", "blendps", "blendpd", "roundps", "roundpd", "roundss", "roundsd", "dpps", "dppd", "pclmulqdq", "cmpps", "cmppd", "cmpss", "cmpsd", "insertps", "mpsadbw"] {
        p(format!("{} xmm1, xmm2, 0x1b", m));
        p(format!("{} xmm9, xmm10, 3", m));
        if !matches!(m, "roundss" | "roundsd" | "cmpss" | "cmpsd" | "insertps") {
            p(format!("{} xmm1, xmmword ptr [rax+8], 0x4e", m));
        }
    }
    for pr in ["eq", "lt", "le", "unord", "neq", "nlt", "nle", "ord"] {
        for s in ["ps", "pd", "ss", "sd"] {
            p(format!("cmp{}{} xmm1, xmm2", pr, s));
        }
    }
    for m in ["pslldq", "psrldq", "psllw", "pslld", "psllq", "psrlw", "psrld", "psrlq", "psraw", "psrad"] {
        p(format!("{} xmm1, 3", m));
        p(format!("{} xmm9, 7", m));
    }
    for r in ["eax", "r9d"] {
        p(format!("pextrb {}, xmm1, 3", r));
        p(format!("pextrw {}, xmm1, 3", r));
        p(format!("pextrd {}, xmm9, 3", r));
        p(format!("extractps {}, xmm9, 2", r));
        p(format!("pinsrb xmm1, {}, 3", r));
        p(format!("pinsrw xmm1, {}, 3", r));
        p(format!("pinsrd xmm9, {}, 3", r));
    }
    for r in ["rax", "r9"] {
        p(format!("pextrq {}, xmm1, 1", r));
        p(format!("pinsrq xmm1, {}, 1", r));
    }
    p("pextrd dword ptr [rax], xmm1, 2".into());
    p("pinsrd xmm1, dword ptr [rax], 2".into());
    // VEX
    let vthree = [
        "vaddps", "vaddpd", "vsubps", "vsubpd", "vmulps", "vmulpd", "vdivps", "vdivpd", "vminps", "vminpd", "vmaxps", "vmaxpd", "vandps", "vandpd", "vandnps", "vorps", "vxorps", "vxorpd", "vunpcklps", "vunpckhps", "vhaddps", "vpxor", "vpand", "vpandn", "vpor", "vpaddb", "vpaddw", "vpaddd", "vpaddq", "vpsubb", "vpsubd", "vpsubq", "vpcmpeqb", "vpcmpeqd", "vpcmpgtd", "vpunpcklbw", "vpunpckldq", "vpunpcklqdq", "vpmullw", "vpmulld", "vpmuludq", "vpminub", "vpmaxsd", "vpshufb", "vpackuswb", "vpmaddwd", "vpsadbw",
    ];
    for m in vthree {
        p(format!("{} xmm0, xmm1, xmm2", m));
        p(format!("{} xmm9, xmm10, xmm11", m));
        p(format!("{} ymm0, ymm1, ymm2", m));
        p(format!("{} ymm9, ymm10, ymm11", m));
        p(format!("{} ymm3, ymm4, ymmword ptr [rax+rcx*8+0x40]", m));
        p(format!("{} xmm3, xmm4, xmmword ptr [r13]", m));
        p(format!("{} xmm3, xmm14, xmm5", m));
        p(format!("{} ymm3, ymm12, ymm5", m));
    }
    for m in ["vaddss", "vaddsd", "vsubss", "vsubsd", "vmulss", "vmulsd", "vdivss", "vdivsd", "vminss", "vmaxsd", "vsqrtss", "vsqrtsd", "vcvtss2sd", "vcvtsd2ss", "vrcpss", "vrsqrtss"] {
        p(format!("{} xmm0, xmm1, xmm2", m));
        p(format!("{} xmm9, xmm10, xmm11", m));
        p(format!("{} xmm9, xmm10, {} ptr [rax+8]", m, if (m.ends_with("ss") && m != "vcvtsd2ss") || m == "vcvtss2sd" { "dword" } else { "qword" }));
    }
    for m in ["vsqrtps", "vsqrtpd", "vmovaps", "vmovups", "vmovapd", "vmovupd", "vmovdqa", "vmovdqu", "vucomiss", "vucomisd", "vcomiss", "vptest", "vpabsd", "vcvtdq2ps", "vcvtps2dq", "vcvttps2dq", "vmovddup", "vpmovzxbw"] {
        p(format!("{} xmm0, xmm1", m));
        p(format!("{} xmm9, xmm10", m));
        if !m.contains("comis") && !m.contains("ucomi") && !m.contains("pmovzx") && !m.contains("ddup") {
            p(format!("{} ymm0, ymm1", m));
            p(format!("{} ymm9, ymm10", m));
        }
        if !m.contains("comis") && !m.contains("ucomi") && !m.contains("pmovzx") && !m.contains("ddup") {
            p(format!("{} xmm3, xmmword ptr [rax+8]", m));
        }
    }
    for m in ["vmovaps", "vmovups", "vmovdqa", "vmovdqu", "vmovapd"] {
        p(format!("{} xmmword ptr [rax+8], xmm1", m));
        p(format!("{} ymmword ptr [rax+8], ymm9", m));
        p(format!("{} ymm9, ymmword ptr [r13]", m));
    }
    for m in ["vmovss", "vmovsd"] {
        p(format!("{} xmm1, xmm2, xmm3", m));
        p(format!("{} xmm9, xmm10, xmm11", m));
        p(format!("{} xmm1, {} ptr [rax]", m, if m == "vmovss" { "dword" } else { "qword" }));
        p(format!("{} {} ptr [rax], xmm1", m, if m == "vmovss" { "dword" } else { "qword" }));
    }
    for (i, ord) in ["132", "213", "231"].iter().enumerate() {
        let _ = i;
        for kind in ["fmadd", "fmsub", "fnmadd", "fnmsub"] {
            for s in ["ps", "pd", "ss", "sd"] {
                p(format!("v{}{}{} xmm0, xmm1, xmm2", kind, ord, s));
                p(format!("v{}{}{} xmm9, xmm10, xmm11", kind, ord, s));
                if s.starts_with('p') {
                    p(format!("v{}{}{} ymm0, ymm1, ymm2", kind, ord, s));
                    p(format!("v{}{}{} ymm9, ymm10, ymmword ptr [rax+8]", kind, ord, s));
                }
                if s.starts_with('s') {
                    p(format!("v{}{}{} xmm3, xmm4, {} ptr [rax+8]", kind, ord, s, if s == "ss" { "dword" } else { "qword" }));
                }
            }
        }
    }
    for m in ["vshufps", "vshufpd", "vblendps", "vpalignr", "vpblendw", "vroundss", "vroundsd", "vdpps"] {
        p(format!("{} xmm1, xmm2, xmm3, 0x1b", m));
        p(format!("{} xmm9, xmm10, xmm11, 3", m));
    }
    for m in ["vpshufd", "vroundps", "vpshufhw"] {
        p(format!("{} xmm1, xmm2, 0x1b", m));
        p(format!("{} ymm9, ymm10, 3", m));
    }
    for m in ["vpslld", "vpsrlq", "vpsrad", "vpslldq", "vpsrldq"] {
        p(format!("{} xmm1, xmm2, 3", m));
        p(format!("{} ymm9, ymm10, 7", m));
    }
    p("vmovd xmm0, eax".into());
    p("vmovd eax, xmm9".into());
    p("vmovq xmm0, rax".into());
    p("vmovq r9, xmm9".into());
    p("vmovq xmm1, xmm2".into());
    p("vmovq xmm1, qword ptr [rax]".into());
    p("vmovq qword ptr [rax], xmm1".into());
    p("vcvtsi2sd xmm0, xmm1, eax".into());
    p("vcvtsi2sd xmm0, xmm1, rax".into());
    p("vcvtsi2ss xmm9, xmm10, r15".into());
    p("vcvttsd2si eax, xmm1".into());
    p("vcvttss2si r9, xmm1".into());
    p("vmovmskps eax, ymm1".into());
    p("vpmovmskb eax, ymm9".into());
    p("vbroadcastss ymm0, xmm1".into());
    p("vbroadcastss xmm0, dword ptr [rax]".into());
    p("vbroadcastsd ymm9, qword ptr [rax]".into());
    p("vpbroadcastd ymm0, xmm1".into());
    p("vpbroadcastb xmm9, byte ptr [rax]".into());
    p("vperm2f128 ymm1, ymm2, ymm3, 0x20".into());
    p("vinsertf128 ymm1, ymm2, xmm3, 1".into());
    p("vextractf128 xmm1, ymm2, 1".into());
    p("vextracti128 xmmword ptr [rax], ymm9, 1".into());
    p("vpermq ymm1, ymm2, 0x1b".into());
    p("vpermd ymm1, ymm2, ymm3".into());
    p("vblendvps ymm1, ymm2, ymm3, ymm4".into());
    p("vpblendvb xmm1, xmm2, xmm3, xmm9".into());
    v
}

#[test]
fn oracle_integer() {
    if !clang_ok() {
        eprintln!("clang not found: skipping the x64 oracle test");
        return;
    }
    let c = corpus();
    let bad = oracle(&c, false);
    for b in bad.iter().take(30) {
        eprintln!("{}", b);
    }
    assert!(bad.is_empty(), "{} of {} integer forms differ from clang", bad.len(), c.len());
}

#[test]
fn oracle_sse() {
    if !clang_ok() {
        eprintln!("clang not found: skipping the x64 oracle test");
        return;
    }
    let c = sse_corpus();
    let bad = oracle(&c, false);
    for b in bad.iter().take(30) {
        eprintln!("{}", b);
    }
    assert!(bad.is_empty(), "{} of {} SSE/AVX forms differ from clang", bad.len(), c.len());
}

#[test]
fn oracle_att() {
    if !clang_ok() {
        eprintln!("clang not found: skipping the x64 oracle test");
        return;
    }
    let c: Vec<String> = [
        "movq %rax, %rbx", "movl $5, (%rax)", "movq $-1, 8(%rax,%rcx,4)", "addl $0x1000, %eax", "subq $32, %rsp", "movzbl (%rax), %ecx", "movzwl %ax, %ecx", "movsbq %al, %rcx", "movslq %eax, %rcx", "movsbl 4(%rsp), %eax",
        "leaq 8(%rax,%rbx,2), %rcx", "leaq foo(%rip), %rax", "jmp *%rax", "call *%r12", "jmp *foo(%rip)", "call *8(%rax)", "pushq %rax", "popq %rbx", "pushq $5", "pushq 8(%rsp)", "ret", "retq", "cltq", "cqto", "cltd", "cwtl",
        "movq %fs:0, %rax", "movq %gs:8, %rax", "movq %rax, %gs:8", "pushq %gs:8", "popq %gs:16", "shll $3, %eax", "shlq %cl, %rax", "sarq %rax", "shrl 4(%rax)", "testb $1, (%rax)", "cmpl $5, 4(%rsp)", "incl (%rax)", "negq %rax", "notl %ecx",
        "imull $5, %eax, %ecx", "imulq %rbx, %rax", "xchgq %rax, %rbx", "lock cmpxchgq %rcx, (%rdx)", "lock xaddl %eax, (%rdx)", "lock addl $1, (%rax)", "cmovneq %rax, %rbx", "setne %al", "sete (%rax)", "bsfq %rax, %rcx", "popcntl %eax, %ecx", "btl $3, %eax", "btsq %rax, (%rbx)", "bswapq %rax", "nopl 0(%rax)", "nopw 0(%rax,%rax,1)",
        "movss %xmm0, %xmm1", "movsd 8(%rax), %xmm1", "movaps %xmm0, (%rax)", "addsd %xmm1, %xmm0", "sqrtsd %xmm0, %xmm0", "cvtsi2sdl %eax, %xmm0", "cvtsi2sdq %rax, %xmm0", "cvttsd2si %xmm0, %eax", "movd %eax, %xmm0", "movq %xmm0, %rax", "pxor %xmm0, %xmm0", "pshufd $0x1b, %xmm1, %xmm2", "vaddps %ymm1, %ymm2, %ymm3", "vfmadd213sd %xmm2, %xmm1, %xmm0", "vmovdqu (%rax), %ymm0", "vmovdqu %ymm0, 32(%rax)",
        "rep movsb", "rep stosq", "movabsq $0x123456789abcdef0, %rax", "movq $5, %rax", "movl $5, %eax", "movb $5, %al", "movw $5, %ax", "xorl %eax, %eax", "andq $-16, %rsp", "orb $0x80, (%rdi)",
        "1:\njne 1b", "jmp foo", "call foo", "int $3", "int $0x80", "ret $8", "syscall",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let bad = oracle(&c, true);
    for b in bad.iter().take(30) {
        eprintln!("{}", b);
    }
    assert!(bad.is_empty(), "{} of {} AT&T forms differ from clang", bad.len(), c.len());
}

// ---------------------------------------------------------------- unit tests

fn sym(n: &str) -> SymRef {
    SymRef::Name(n.to_string())
}

fn run_ops(template: &str, args: &[Arg]) -> Output {
    let names: Vec<(String, u32)> = Vec::new();
    let pieces = parse_template(&[template], &names, args.len() as u32, args.len() as u32, false).unwrap();
    assemble(Arch::X64, Format::Elf, Kind::Inline, &pieces, args, false).unwrap_or_else(|e| panic!("{}", e))
}

#[test]
fn placeholders_registers() {
    // mov {0:r}, {1:e}-style operands with the class default modifiers filled by the compiler
    let o = run_ops("mov {}, {}", &[Arg::Reg(PReg::int(2)), Arg::Reg(PReg::int(5))]);
    // modifier None: Tok::Reg(_, None) is read as a 64-bit register
    assert_eq!(text(&o), bytes("mov rdx, rbp"));
    // explicit modifiers
    let pieces = vec![Piece::Text("mov ".into()), Piece::Op(0, Some('e')), Piece::Text(", ".into()), Piece::Op(1, Some('l'))];
    let o = assemble(Arch::X64, Format::Elf, Kind::Inline, &pieces, &[Arg::Reg(PReg::int(8)), Arg::Reg(PReg::int(6))], false);
    assert!(o.is_err(), "size mismatch must be an error");
    let pieces = vec![Piece::Text("add ".into()), Piece::Op(0, Some('l')), Piece::Text(", ".into()), Piece::Op(1, Some('l'))];
    let o = assemble(Arch::X64, Format::Elf, Kind::Inline, &pieces, &[Arg::Reg(PReg::int(6)), Arg::Reg(PReg::int(1))], false).unwrap();
    assert_eq!(text(&o), bytes("add sil, cl"));
    let pieces = vec![Piece::Text("mov ".into()), Piece::Op(0, Some('h')), Piece::Text(", ".into()), Piece::Op(1, Some('l'))];
    let o = assemble(Arch::X64, Format::Elf, Kind::Inline, &pieces, &[Arg::Reg(PReg::int(1)), Arg::Reg(PReg::int(3))], false).unwrap();
    assert_eq!(text(&o), bytes("mov ch, bl"));
    // vector modifiers
    let pieces = vec![Piece::Text("vaddps ".into()), Piece::Op(0, Some('y')), Piece::Text(", ".into()), Piece::Op(1, Some('y')), Piece::Text(", ".into()), Piece::Op(2, Some('y'))];
    let o = assemble(Arch::X64, Format::Elf, Kind::Inline, &pieces, &[Arg::Reg(PReg::vec(1)), Arg::Reg(PReg::vec(9)), Arg::Reg(PReg::vec(3))], false).unwrap();
    assert_eq!(text(&o), bytes("vaddps ymm1, ymm9, ymm3"));
    // call {f} with a register operand; memory with a register base
    let o = run_ops("call {0}\nmov {1}, [{0} + 8]", &[Arg::Reg(PReg::int(11)), Arg::Reg(PReg::int(0))]);
    assert_eq!(text(&o), bytes("call r11\nmov rax, [r11 + 8]"));
}

#[test]
fn placeholders_syms_and_consts() {
    // jmp qword ptr [rip + {t}]
    let o = run_ops("jmp qword ptr [rip + {0}]", &[Arg::Sym(sym("target"))]);
    assert_eq!(text(&o), vec![0xff, 0x25, 0, 0, 0, 0]);
    assert_eq!(o.fixups.len(), 1);
    let f = &o.fixups[0];
    assert_eq!(f.at, 2);
    assert_eq!(f.kind, FixKind::X64Rel32 { tail: 0 });
    assert_eq!(f.target, Value::Sym(sym("target"), 0));
    // jmp {f}: branch fixup
    let o = run_ops("jmp {0}", &[Arg::Sym(sym("f"))]);
    assert_eq!(text(&o), vec![0xe9, 0, 0, 0, 0]);
    assert_eq!(o.fixups[0].kind, FixKind::X64Branch32);
    assert_eq!(o.fixups[0].at, 1);
    let o = run_ops("call {0}", &[Arg::Sym(sym("f"))]);
    assert_eq!(text(&o)[0], 0xe8);
    assert_eq!(o.fixups[0].kind, FixKind::X64Branch32);
    // a tail: mov dword ptr [rip + sym], 5 has 4 more bytes after the disp
    let o = run_ops("mov dword ptr [rip + {0}], 5", &[Arg::Sym(sym("g"))]);
    assert_eq!(o.fixups[0].kind, FixKind::X64Rel32 { tail: 4 });
    assert_eq!(text(&o).len(), 6 + 4);
    let o = run_ops("cmp byte ptr [rip + {0} + 3], 1", &[Arg::Sym(sym("g"))]);
    assert_eq!(o.fixups[0].kind, FixKind::X64Rel32 { tail: 1 });
    assert_eq!(o.fixups[0].target, Value::Sym(sym("g"), 3));
    // const operand
    let o = run_ops("sub rsp, {0}", &[Arg::Const(32)]);
    assert_eq!(text(&o), bytes("sub rsp, 32"));
    // symbol as a 32-bit immediate address
    let o = run_ops("mov eax, {0}", &[Arg::Sym(sym("g"))]);
    assert_eq!(o.fixups[0].kind, FixKind::Abs32);
    let o = run_ops("mov rax, {0}", &[Arg::Sym(sym("g"))]);
    assert_eq!(text(&o).len(), 10);
    assert_eq!(o.fixups[0].kind, FixKind::Abs64);
    assert_eq!(o.fixups[0].at, 2);
    // fs/gs
    let o = run_ops("mov {0}, qword ptr fs:[0]", &[Arg::Reg(PReg::int(7))]);
    assert_eq!(text(&o), bytes("mov rdi, qword ptr fs:[0]"));
}

#[test]
fn labels_and_branches() {
    // forward: rel32 resolved by the core
    let b = bytes("jmp 1f\nnop\n1:\nret");
    assert_eq!(b, vec![0xe9, 1, 0, 0, 0, 0x90, 0xc3]);
    // backward within range: rel8
    let b = bytes("1:\nnop\njmp 1b");
    assert_eq!(b, vec![0x90, 0xeb, 0xfd]);
    let b = bytes("l:\nnop\njne l");
    assert_eq!(b, vec![0x90, 0x75, 0xfd]);
    // backward out of range: rel32
    let mut s = String::from("1:\n");
    for _ in 0..200 {
        s.push_str("nop\n");
    }
    s.push_str("jmp 1b\n");
    let b = bytes(&s);
    assert_eq!(b.len(), 205);
    assert_eq!(b[200], 0xe9);
    assert_eq!(i32::from_le_bytes(b[201..205].try_into().unwrap()), -205);
    // forward jcc and call to a label
    let b = bytes("je 2f\n2:\ncall 2b_missing_is_error_not_here_0\n".replace("2b_missing_is_error_not_here_0", "2f\n2:").as_str());
    assert_eq!(&b[..6], &[0x0f, 0x84, 0, 0, 0, 0]);
    // rip-relative label: lea rax, [rip + data]
    let b = bytes("lea rax, [rip + d]\nnop\nd:\n");
    assert_eq!(b, vec![0x48, 0x8d, 0x05, 1, 0, 0, 0, 0x90]);
    // rip-relative label with an immediate tail
    let b = bytes("mov dword ptr [rip + d], 7\nnop\nd:\n");
    assert_eq!(b, vec![0xc7, 0x05, 1, 0, 0, 0, 7, 0, 0, 0, 0x90]);
    // a pure rip constant is the raw displacement
    assert_eq!(bytes("lea rax, [rip + 0x20]"), vec![0x48, 0x8d, 0x05, 0x20, 0, 0, 0]);
}

#[test]
fn prefix_statements() {
    assert_eq!(bytes("lock cmpxchg qword ptr [rdi], rsi"), vec![0xf0, 0x48, 0x0f, 0xb1, 0x37]);
    assert_eq!(bytes("rep movsb"), vec![0xf3, 0xa4]);
    assert_eq!(bytes("repne scasb"), vec![0xf2, 0xae]);
    assert_eq!(bytes("data16\nnop"), vec![0x66, 0x90]);
}

#[test]
fn errors() {
    for (src, want) in [
        ("frobnicate rax", "unsupported instruction `frobnicate`"),
        ("add rax", "unsupported operands for `add`"),
        ("add eax, rbx", "operand size mismatch"),
        ("mov byte ptr [rax], 300", "out of range"),
        ("inc [rax]", "ambiguous operand size"),
        ("mov ah, sil", "high-byte"),
        ("mov rax, [rsp + rsp]", "rsp cannot be an index"),
        ("mov rax, [rax + rbx*3]", "scale"),
        ("vaddps xmm16, xmm1, xmm2", "EVEX"),
        ("vaddps zmm0, zmm1, zmm2", "zmm"),
        ("jmp 5", "absolute"),
        ("push ebx", "unsupported operands for `push`"),
    ] {
        let e = asm_intel(src).unwrap_err();
        assert!(e.msg.contains(want), "`{}`: got `{}`, wanted `{}`", src, e.msg, want);
    }
}

#[test]
fn parse_reg_names() {
    assert_eq!(parse_reg("RAX"), Some((PReg::int(0), 64, false)));
    assert_eq!(parse_reg("r13d"), Some((PReg::int(13), 32, false)));
    assert_eq!(parse_reg("r9w"), Some((PReg::int(9), 16, false)));
    assert_eq!(parse_reg("r10b"), Some((PReg::int(10), 8, false)));
    assert_eq!(parse_reg("r10l"), Some((PReg::int(10), 8, false)));
    assert_eq!(parse_reg("sil"), Some((PReg::int(6), 8, false)));
    assert_eq!(parse_reg("ah"), Some((PReg::int(0), 8, true)));
    assert_eq!(parse_reg("bh"), Some((PReg::int(3), 8, true)));
    assert_eq!(parse_reg("xmm15"), Some((PReg::vec(15), 128, false)));
    assert_eq!(parse_reg("YMM3"), Some((PReg::vec(3), 256, false)));
    assert_eq!(parse_reg("zmm31"), Some((PReg::vec(31), 512, false)));
    assert_eq!(parse_reg("r16"), None);
    assert_eq!(parse_reg("xmm32"), None);
    assert_eq!(parse_reg("foo"), None);
}

#[test]
fn explicit_text_registers() {
    // registers written in the template arrive as identifiers, in any case
    assert_eq!(bytes("MOV RAX, R12"), bytes("mov rax, r12"));
    assert_eq!(bytes("sqrtsd XMM1, xmm1"), vec![0xf2, 0x0f, 0x51, 0xc9]);
}

#[test]
fn att_syntax_basics() {
    let o = asm_att("movl $5, (%rax)\nmovq %fs:0, %rax").unwrap();
    assert_eq!(text(&o), bytes("mov dword ptr [rax], 5\nmov rax, qword ptr fs:[0]"));
}
