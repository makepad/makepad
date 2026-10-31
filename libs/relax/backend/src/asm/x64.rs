//! x86-64 instruction encoder for [`super`] (Intel syntax, the asm! default; AT&T with
//! `options(att_syntax)` / `.att_syntax`).
//!
//! Encodings follow LLVM's integrated assembler (what rustc uses): the oracle test in
//! tests_x64.rs assembles every supported form with clang and compares bytes.
//!
//! Deliberate differences from LLVM:
//! - Branches (jmp / jcc / call) to a label defined LATER or to a symbol always use the rel32
//!   form (the assembler is single pass; no relaxation). A backward label within rel8 range
//!   uses the short form, as LLVM does. `{disp32} jmp` in oracle sources forces LLVM's long form.
//! - EVEX (xmm16-31, zmm, opmask) is not supported.

use super::{split_commas, Ctx, Encoder, FixKind, PReg, Tok, Value};

thread_local! {
    /// Where the legacy prefix statements (`lock`, `rep` ...) of the current instruction start.
    /// LLVM emits them after the segment / address-size / operand-size prefixes, so the byte
    /// emitters take them back and place them there.
    static PFX_START: std::cell::Cell<Option<u32>> = const { std::cell::Cell::new(None) };
}

/// The statement prefixes emitted since the instruction began (removed from the section).
fn take_pfx(ctx: &mut Ctx) -> Vec<u8> {
    match PFX_START.with(|c| c.take()) {
        Some(s) if (s as usize) < ctx.pos() as usize => ctx.bytes_mut().drain(s as usize..).collect(),
        _ => Vec::new(),
    }
}

const GPR64: [&str; 16] = ["rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi", "r8", "r9", "r10", "r11", "r12", "r13", "r14", "r15"];
const GPR32: [&str; 16] = ["eax", "ecx", "edx", "ebx", "esp", "ebp", "esi", "edi", "r8d", "r9d", "r10d", "r11d", "r12d", "r13d", "r14d", "r15d"];
const GPR16: [&str; 16] = ["ax", "cx", "dx", "bx", "sp", "bp", "si", "di", "r8w", "r9w", "r10w", "r11w", "r12w", "r13w", "r14w", "r15w"];
const GPR8: [&str; 16] = ["al", "cl", "dl", "bl", "spl", "bpl", "sil", "dil", "r8b", "r9b", "r10b", "r11b", "r12b", "r13b", "r14b", "r15b"];

/// A register by name (case-insensitive): the register, its width in bits (8, 16, 32, 64 for
/// integers; 128 xmm, 256 ymm, 512 zmm) and whether it is a high byte register (ah..bh, which
/// shares `PReg::int(0..=3)` with al..bl).
pub fn parse_reg(name: &str) -> Option<(PReg, u16, bool)> {
    let l = name.to_ascii_lowercase();
    let l = l.as_str();
    for (tab, bits) in [(&GPR64, 64u16), (&GPR32, 32), (&GPR16, 16), (&GPR8, 8)] {
        if let Some(n) = tab.iter().position(|x| *x == l) {
            return Some((PReg::int(n as u8), bits, false));
        }
    }
    match l {
        "ah" => return Some((PReg::int(0), 8, true)),
        "ch" => return Some((PReg::int(1), 8, true)),
        "dh" => return Some((PReg::int(2), 8, true)),
        "bh" => return Some((PReg::int(3), 8, true)),
        _ => {}
    }
    if let Some(rest) = l.strip_prefix('r') {
        if let Some(d) = rest.strip_suffix('l') {
            if let Ok(n) = d.parse::<u8>() {
                if (8..16).contains(&n) && d == n.to_string() {
                    return Some((PReg::int(n), 8, false));
                }
            }
        }
    }
    for (pre, bits) in [("xmm", 128u16), ("ymm", 256), ("zmm", 512)] {
        if let Some(d) = l.strip_prefix(pre) {
            if let Ok(n) = d.parse::<u8>() {
                if n < 32 && d == n.to_string() {
                    return Some((PReg::vec(n), bits, false));
                }
            }
        }
    }
    None
}

#[derive(Clone, Copy, PartialEq, Debug)]
struct R {
    n: u8,
    bits: u16,
    high: bool,
    vec: bool,
}

impl R {
    fn enc(&self) -> u8 {
        if self.high {
            self.n + 4
        } else {
            self.n
        }
    }
    fn rex8(&self) -> bool {
        self.bits == 8 && !self.high && !self.vec && (4..8).contains(&self.n)
    }
}

fn gpr(n: u8, bits: u16) -> R {
    R { n, bits, high: false, vec: false }
}

fn reg_of(t: &Tok) -> Option<R> {
    match t {
        Tok::Reg(p, m) => {
            if p.vec {
                let bits = match m {
                    Some('y') => 256,
                    Some('z') => 512,
                    _ => 128,
                };
                Some(R { n: p.n, bits, high: false, vec: true })
            } else {
                let (bits, high) = match m {
                    Some('l') => (8, false),
                    Some('h') => (8, true),
                    Some('x') => (16, false),
                    Some('e') => (32, false),
                    _ => (64, false),
                };
                Some(R { n: p.n, bits, high, vec: false })
            }
        }
        Tok::Ident(s) => parse_reg(s).map(|(p, bits, high)| R { n: p.n, bits, high, vec: p.vec }),
        _ => None,
    }
}

#[derive(Clone, Debug)]
struct M {
    size: u16,
    seg: u8,
    base: Option<R>,
    index: Option<(R, u8)>,
    rip: bool,
    disp: Value,
}

#[derive(Clone, Debug)]
enum Op {
    R(R),
    M(M),
    I(Value),
}

fn seg_prefix(s: &str) -> Option<u8> {
    Some(match s.to_ascii_lowercase().as_str() {
        "es" => 0x26,
        "cs" => 0x2e,
        "ss" => 0x36,
        "ds" => 0x3e,
        "fs" => 0x64,
        "gs" => 0x65,
        _ => return None,
    })
}

fn size_kw(s: &str) -> Option<u16> {
    Some(match s.to_ascii_lowercase().as_str() {
        "byte" => 8,
        "word" => 16,
        "dword" => 32,
        "qword" => 64,
        "tbyte" | "tword" => 80,
        "fword" => 48,
        "xmmword" | "oword" => 128,
        "ymmword" => 256,
        "zmmword" => 512,
        _ => return None,
    })
}

fn vadd(a: Value, b: Value) -> Result<Value, String> {
    Ok(match (a, b) {
        (Value::Const(x), Value::Const(y)) => Value::Const(x.wrapping_add(y)),
        (Value::Label(l, k), Value::Const(y)) | (Value::Const(y), Value::Label(l, k)) => Value::Label(l, k + y as i64),
        (Value::Sym(s, k), Value::Const(y)) | (Value::Const(y), Value::Sym(s, k)) => Value::Sym(s, k + y as i64),
        _ => return Err("unsupported combination of symbols in an address".to_string()),
    })
}

fn vneg(v: Value) -> Result<Value, String> {
    match v {
        Value::Const(x) => Ok(Value::Const(-x)),
        _ => Err("cannot subtract a symbol in an address".to_string()),
    }
}

fn mem_new() -> M {
    M { size: 0, seg: 0, base: None, index: None, rip: false, disp: Value::Const(0) }
}

fn is_op(t: &Tok, c: u8) -> bool {
    t.is(c)
}

/// The inside of Intel `[ ... ]`.
fn parse_bracket(ctx: &Ctx, t: &[Tok], m: &mut M) -> Result<(), String> {
    let mut k = 0;
    let mut first = true;
    while k < t.len() {
        let mut neg = false;
        if is_op(&t[k], b'+') {
            k += 1;
        } else if is_op(&t[k], b'-') {
            neg = true;
            k += 1;
        } else if !first {
            return Err("invalid memory operand".to_string());
        }
        first = false;
        let mut e = k;
        let mut depth = 0i32;
        while e < t.len() {
            match &t[e] {
                Tok::Punct(b'(') => depth += 1,
                Tok::Punct(b')') => depth -= 1,
                Tok::Punct(b'+') | Tok::Punct(b'-') if depth == 0 && e > k => break,
                _ => {}
            }
            e += 1;
        }
        let term = &t[k..e];
        if term.is_empty() {
            return Err("invalid memory operand".to_string());
        }
        k = e;
        let one = |x: &Tok| reg_of(x);
        let (r_idx, scale): (Option<R>, Option<u8>) = if term.len() == 1 {
            (one(&term[0]), None)
        } else if term.len() == 3 && term[1].is(b'*') {
            match (one(&term[0]), &term[2], &term[0], one(&term[2])) {
                (Some(r), Tok::Int(s), _, _) => (Some(r), Some(*s as u8)),
                (None, _, Tok::Int(s), Some(r)) => (Some(r), Some(*s as u8)),
                _ => (None, None),
            }
        } else {
            (None, None)
        };
        let is_rip = matches!(term, [Tok::Ident(s)] if s.eq_ignore_ascii_case("rip"));
        if is_rip {
            if neg {
                return Err("invalid memory operand".to_string());
            }
            m.rip = true;
        } else if let Some(r) = r_idx {
            if neg {
                return Err("cannot subtract a register in an address".to_string());
            }
            if r.vec {
                return Err("vector index (VSIB) addressing is not supported".to_string());
            }
            match scale {
                Some(s) => {
                    if !matches!(s, 1 | 2 | 4 | 8) {
                        return Err("invalid scale in memory operand".to_string());
                    }
                    if m.index.is_some() {
                        return Err("invalid memory operand: two index registers".to_string());
                    }
                    m.index = Some((r, s));
                }
                None => {
                    if m.base.is_none() {
                        m.base = Some(r);
                    } else if m.index.is_none() {
                        m.index = Some((r, 1));
                    } else {
                        return Err("invalid memory operand: too many registers".to_string());
                    }
                }
            }
        } else {
            let mut i = 0;
            let mut v = ctx.expr(term, &mut i)?;
            if i != term.len() {
                return Err("invalid expression in memory operand".to_string());
            }
            if neg {
                v = vneg(v)?;
            }
            m.disp = vadd(std::mem::replace(&mut m.disp, Value::Const(0)), v)?;
        }
    }
    Ok(())
}

fn parse_imm(ctx: &Ctx, t: &[Tok]) -> Result<Value, String> {
    let mut i = 0;
    let v = ctx.expr(t, &mut i)?;
    if i != t.len() {
        return Err("invalid operand".to_string());
    }
    Ok(v)
}

fn parse_intel(ctx: &Ctx, t: &[Tok]) -> Result<Op, String> {
    let mut i = 0;
    let mut size = 0u16;
    if let Some(Tok::Ident(s)) = t.get(i) {
        if let Some(sz) = size_kw(s) {
            size = sz;
            i += 1;
            if matches!(t.get(i), Some(Tok::Ident(p)) if p.eq_ignore_ascii_case("ptr")) {
                i += 1;
            }
        }
    }
    if matches!(t.get(i), Some(Tok::Ident(p)) if p.eq_ignore_ascii_case("offset")) {
        return Ok(Op::I(parse_imm(ctx, &t[i + 1..])?));
    }
    let mut seg = 0u8;
    if let (Some(Tok::Ident(s)), Some(c)) = (t.get(i), t.get(i + 1)) {
        if c.is(b':') {
            if let Some(p) = seg_prefix(s) {
                seg = p;
                i += 2;
            }
        }
    }
    let rest = &t[i..];
    if rest.first().is_some_and(|x| x.is(b'[')) && rest.last().is_some_and(|x| x.is(b']')) {
        let mut m = mem_new();
        m.size = size;
        m.seg = seg;
        parse_bracket(ctx, &rest[1..rest.len() - 1], &mut m)?;
        return Ok(Op::M(m));
    }
    if size == 0 && seg == 0 {
        if rest.len() == 1 {
            if let Some(r) = reg_of(&rest[0]) {
                return Ok(Op::R(r));
            }
        }
        if rest.len() == 2 && rest[0].is(b'%') {
            if let Some(r) = reg_of(&rest[1]) {
                return Ok(Op::R(r));
            }
        }
        return Ok(Op::I(parse_imm(ctx, rest)?));
    }
    // `dword ptr sym` / `fs:8`: an absolute address
    let mut m = mem_new();
    m.size = size;
    m.seg = seg;
    m.disp = parse_imm(ctx, rest)?;
    Ok(Op::M(m))
}

/// `$` immediates arrive either as Ident("$...") or Punct('$') + expression.
fn att_imm(ctx: &Ctx, t: &[Tok]) -> Result<Option<Value>, String> {
    match t.first() {
        Some(Tok::Punct(b'$')) => Ok(Some(parse_imm(ctx, &t[1..])?)),
        Some(Tok::Ident(s)) if s.starts_with('$') => {
            let rest = &s[1..];
            if rest.is_empty() {
                return Ok(Some(parse_imm(ctx, &t[1..])?));
            }
            let mut v = t.to_vec();
            v[0] = Tok::Ident(rest.to_string());
            // a number lexed into the identifier
            if let Some(n) = parse_num(rest) {
                v[0] = Tok::Int(n);
            }
            Ok(Some(parse_imm(ctx, &v)?))
        }
        _ => Ok(None),
    }
}

fn parse_num(s: &str) -> Option<i128> {
    let (radix, d) = if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        (16, h)
    } else if let Some(b) = s.strip_prefix("0b") {
        (2, b)
    } else if s.len() > 1 && s.starts_with('0') {
        (8, &s[1..])
    } else {
        (10, s)
    };
    i128::from_str_radix(d, radix).ok()
}

fn parse_att(ctx: &Ctx, t: &[Tok], branch: bool) -> Result<Op, String> {
    let mut t = t;
    if t.first().is_some_and(|x| x.is(b'*')) {
        t = &t[1..];
    }
    if let Some(v) = att_imm(ctx, t)? {
        return Ok(Op::I(v));
    }
    if t.len() == 2 && t[0].is(b'%') {
        if let Some(r) = reg_of(&t[1]) {
            return Ok(Op::R(r));
        }
    }
    if t.len() == 1 {
        if let Some(r @ R { .. }) = reg_of(&t[0]) {
            if matches!(t[0], Tok::Reg(..)) {
                return Ok(Op::R(r));
            }
        }
    }
    let mut m = mem_new();
    let mut i = 0;
    if t.len() >= 4 && t[0].is(b'%') && t[2].is(b':') {
        if let Tok::Ident(s) = &t[1] {
            if let Some(p) = seg_prefix(s) {
                m.seg = p;
                i = 3;
            }
        }
    }
    let rest = &t[i..];
    let mut disp_end = rest.len();
    let mut paren: Option<&[Tok]> = None;
    if rest.last().is_some_and(|x| x.is(b')')) {
        let mut depth = 0;
        let mut k = rest.len();
        while k > 0 {
            k -= 1;
            match &rest[k] {
                Tok::Punct(b')') => depth += 1,
                Tok::Punct(b'(') => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
        }
        let inner = &rest[k + 1..rest.len() - 1];
        let looks = inner.first().is_some_and(|x| x.is(b'%') || x.is(b',') || matches!(x, Tok::Reg(..)));
        if looks {
            paren = Some(inner);
            disp_end = k;
        }
    }
    if paren.is_none() && branch && m.seg == 0 {
        return Ok(Op::I(parse_imm(ctx, rest)?));
    }
    if disp_end > 0 {
        m.disp = parse_imm(ctx, &rest[..disp_end])?;
    }
    if let Some(inner) = paren {
        let parts = split_commas(inner);
        let reg = |p: &[Tok]| -> Result<Option<R>, String> {
            if p.is_empty() {
                return Ok(None);
            }
            if p.len() == 2 && p[0].is(b'%') {
                if let Tok::Ident(s) = &p[1] {
                    if s.eq_ignore_ascii_case("rip") {
                        return Ok(None);
                    }
                }
                return reg_of(&p[1]).map(Some).ok_or_else(|| "invalid register in memory operand".to_string());
            }
            if p.len() == 1 {
                return reg_of(&p[0]).map(Some).ok_or_else(|| "invalid register in memory operand".to_string());
            }
            Err("invalid memory operand".to_string())
        };
        let is_rip = parts.first().is_some_and(|p| p.len() == 2 && p[0].is(b'%') && matches!(&p[1], Tok::Ident(s) if s.eq_ignore_ascii_case("rip")));
        if is_rip {
            m.rip = true;
        } else {
            m.base = reg(parts.first().copied().unwrap_or(&[]))?;
        }
        if let Some(ip) = parts.get(1) {
            if let Some(r) = reg(ip)? {
                let mut s = 1u8;
                if let Some(sp) = parts.get(2) {
                    s = match parse_imm(ctx, sp)? {
                        Value::Const(k) if matches!(k, 1 | 2 | 4 | 8) => k as u8,
                        _ => return Err("invalid scale in memory operand".to_string()),
                    };
                }
                m.index = Some((r, s));
            }
        }
    }
    Ok(Op::M(m))
}

// ---------------------------------------------------------------- byte emission

#[derive(Clone, Copy, Default)]
struct F {
    /// mandatory prefix (0x66 / 0xf2 / 0xf3) and the operand-size prefix
    pfx: u8,
    p66: bool,
    w: bool,
    map: u8,
    op: u8,
    rex8: bool,
    high: bool,
}

fn f1(op: u8) -> F {
    F { op, ..F::default() }
}
fn f0f(op: u8) -> F {
    F { op, map: 1, ..F::default() }
}

fn konst(v: &Value) -> Result<i128, String> {
    match v {
        Value::Const(k) => Ok(*k),
        _ => Err("a symbol is not allowed as an immediate here".to_string()),
    }
}

fn imm_le(v: i128, n: usize) -> Result<Vec<u8>, String> {
    let bits = n * 8;
    let lo = -(1i128 << (bits - 1));
    let hi = (1i128 << bits) - 1;
    if v < lo || v > hi {
        return Err(format!("immediate {} out of range for {} bytes", v, n));
    }
    Ok(v.to_le_bytes()[..n].to_vec())
}

/// Operand-size setup: 16 -> 0x66, 64 -> REX.W; REX/high-byte needs of the operands.
fn mk(bits: u16, ops: &[Op], map: u8, op: u8) -> F {
    let mut f = F { op, map, p66: bits == 16, w: bits == 64, ..F::default() };
    for o in ops {
        if let Op::R(r) = o {
            f.rex8 |= r.rex8();
            f.high |= r.high;
        }
    }
    f
}

fn rm_xb(rm: &Op) -> Result<(u8, u8), String> {
    match rm {
        Op::R(r) => {
            if r.vec && r.n >= 16 {
                return Err("registers xmm16-xmm31 need EVEX, which is not supported".to_string());
            }
            Ok((0, r.enc() >> 3))
        }
        Op::M(m) => {
            let b = m.base.map_or(0, |b| b.n >> 3);
            let x = m.index.map_or(0, |(i, _)| i.n >> 3);
            Ok((x, b))
        }
        Op::I(_) => Err("internal: immediate used as r/m".to_string()),
    }
}

fn put_pre(ctx: &mut Ctx, rm: &Op) {
    if let Op::M(m) = rm {
        if m.seg != 0 {
            ctx.emit_u8(m.seg);
        }
        let a32 = m.base.is_some_and(|b| b.bits == 32) || m.index.is_some_and(|(i, _)| i.bits == 32);
        if a32 {
            ctx.emit_u8(0x67);
        }
    }
}

fn put_disp32(ctx: &mut Ctx, v: &Value, kind: Option<FixKind>) -> Result<(), String> {
    match v {
        Value::Const(k) => {
            let b = imm_le(*k, 4)?;
            ctx.emit(&b);
        }
        _ => {
            let at = ctx.pos();
            ctx.emit(&[0; 4]);
            ctx.fixup(at, kind.unwrap_or(FixKind::Abs32), v.clone());
        }
    }
    Ok(())
}

fn put_modrm(ctx: &mut Ctx, reg: u8, rm: &Op, tail: usize) -> Result<(), String> {
    let r3 = (reg & 7) << 3;
    match rm {
        Op::R(r) => ctx.emit_u8(0xC0 | r3 | (r.enc() & 7)),
        Op::M(m) => {
            if m.rip {
                if m.base.is_some() || m.index.is_some() {
                    return Err("invalid memory operand: rip with a register".to_string());
                }
                ctx.emit_u8(r3 | 5);
                // a constant displacement is the raw field, as LLVM does for `[rip + 8]`
                put_disp32(ctx, &m.disp, Some(FixKind::X64Rel32 { tail: tail as u8 }))?;
                return Ok(());
            }
            if let Some((i, _)) = m.index {
                if i.n == 4 {
                    return Err("rsp cannot be an index register".to_string());
                }
            }
            let scale_bits = |s: u8| match s {
                1 => 0u8,
                2 => 1,
                4 => 2,
                _ => 3,
            };
            match (m.base, m.index) {
                (None, None) => {
                    ctx.emit_u8(r3 | 4);
                    ctx.emit_u8(0x25);
                    put_disp32(ctx, &m.disp, None)?;
                }
                (None, Some((i, s))) => {
                    ctx.emit_u8(r3 | 4);
                    ctx.emit_u8((scale_bits(s) << 6) | ((i.n & 7) << 3) | 5);
                    put_disp32(ctx, &m.disp, None)?;
                }
                (Some(b), idx) => {
                    let b3 = b.n & 7;
                    let (md, d8, d32): (u8, Option<i8>, bool) = match &m.disp {
                        Value::Const(0) if b3 != 5 => (0, None, false),
                        Value::Const(k) if (-128..128).contains(k) => (1, Some(*k as i8), false),
                        _ => (2, None, true),
                    };
                    if idx.is_some() || b3 == 4 {
                        ctx.emit_u8((md << 6) | r3 | 4);
                        let (ib, s) = match idx {
                            Some((i, s)) => (i.n & 7, s),
                            None => (4, 1),
                        };
                        ctx.emit_u8((scale_bits(s) << 6) | (ib << 3) | b3);
                    } else {
                        ctx.emit_u8((md << 6) | r3 | b3);
                    }
                    if let Some(d) = d8 {
                        ctx.emit_u8(d as u8);
                    }
                    if d32 {
                        put_disp32(ctx, &m.disp, None)?;
                    }
                }
            }
        }
        Op::I(_) => return Err("internal: immediate used as r/m".to_string()),
    }
    Ok(())
}

fn gen(ctx: &mut Ctx, f: &F, reg: u8, rm: &Op, imm: &[u8]) -> Result<(), String> {
    if reg >= 16 {
        return Err("registers xmm16-xmm31 need EVEX, which is not supported".to_string());
    }
    let (x, b) = rm_xb(rm)?;
    let r = reg >> 3;
    let rex = 0x40 | ((f.w as u8) << 3) | (r << 2) | (x << 1) | b;
    let need = rex != 0x40 || f.rex8;
    if need && f.high {
        return Err("cannot encode a high-byte register (ah, ch, dh, bh) with a REX prefix".to_string());
    }
    let sp = take_pfx(ctx);
    put_pre(ctx, rm);
    if f.p66 {
        ctx.emit_u8(0x66);
    }
    ctx.emit(&sp);
    if f.pfx != 0 {
        ctx.emit_u8(f.pfx);
    }
    if need {
        ctx.emit_u8(rex);
    }
    if f.map >= 1 {
        ctx.emit_u8(0x0F);
    }
    if f.map == 2 {
        ctx.emit_u8(0x38);
    } else if f.map == 3 {
        ctx.emit_u8(0x3A);
    }
    ctx.emit_u8(f.op);
    put_modrm(ctx, reg, rm, imm.len())?;
    ctx.emit(imm);
    Ok(())
}

/// Opcode + register in the low 3 bits (push, pop, mov imm, bswap, xchg, accumulator forms).
fn gen_plus(ctx: &mut Ctx, f: &F, r: &R, imm: &[u8]) -> Result<(), String> {
    let b = r.enc() >> 3;
    let rex = 0x40 | ((f.w as u8) << 3) | b;
    let need = rex != 0x40 || f.rex8;
    if need && f.high {
        return Err("cannot encode a high-byte register (ah, ch, dh, bh) with a REX prefix".to_string());
    }
    let sp = take_pfx(ctx);
    if f.p66 {
        ctx.emit_u8(0x66);
    }
    ctx.emit(&sp);
    if f.pfx != 0 {
        ctx.emit_u8(f.pfx);
    }
    if need {
        ctx.emit_u8(rex);
    }
    if f.map >= 1 {
        ctx.emit_u8(0x0F);
    }
    ctx.emit_u8(f.op + (r.enc() & 7));
    ctx.emit(imm);
    Ok(())
}

/// VEX-encoded instruction. `pp`: 0 none, 1 = 66, 2 = F3, 3 = F2.
fn gen_vex(ctx: &mut Ctx, pp: u8, map: u8, w: bool, l: bool, op: u8, reg: u8, vvvv: u8, rm: &Op, imm: &[u8]) -> Result<(), String> {
    if reg >= 16 || vvvv >= 16 {
        return Err("registers xmm16-xmm31 need EVEX, which is not supported".to_string());
    }
    let (x, b) = rm_xb(rm)?;
    let r = reg >> 3;
    let sp = take_pfx(ctx);
    put_pre(ctx, rm);
    ctx.emit(&sp);
    if map == 1 && !w && x == 0 && b == 0 {
        ctx.emit_u8(0xC5);
        ctx.emit_u8(((r ^ 1) << 7) | ((!vvvv & 15) << 3) | ((l as u8) << 2) | pp);
    } else {
        ctx.emit_u8(0xC4);
        ctx.emit_u8(((r ^ 1) << 7) | ((x ^ 1) << 6) | ((b ^ 1) << 5) | map);
        ctx.emit_u8(((w as u8) << 7) | ((!vvvv & 15) << 3) | ((l as u8) << 2) | pp);
    }
    ctx.emit_u8(op);
    put_modrm(ctx, reg, rm, imm.len())?;
    ctx.emit(imm);
    Ok(())
}

// ---------------------------------------------------------------- integer instructions

fn bad(mn: &str) -> String {
    format!("unsupported operands for `{}`", mn)
}

fn cc_of(s: &str) -> Option<u8> {
    Some(match s {
        "o" => 0,
        "no" => 1,
        "b" | "c" | "nae" => 2,
        "ae" | "nb" | "nc" => 3,
        "e" | "z" => 4,
        "ne" | "nz" => 5,
        "be" | "na" => 6,
        "a" | "nbe" => 7,
        "s" => 8,
        "ns" => 9,
        "p" | "pe" => 10,
        "np" | "po" => 11,
        "l" | "nge" => 12,
        "ge" | "nl" => 13,
        "le" | "ng" => 14,
        "g" | "nle" => 15,
        _ => return None,
    })
}

fn ibits(o: &Op) -> Option<u16> {
    match o {
        Op::R(r) if !r.vec => Some(r.bits),
        Op::M(m) if m.size != 0 => Some(m.size),
        _ => None,
    }
}

/// The operand size of an instruction from its operands (a register wins over a memory keyword).
fn opsize(ops: &[Op]) -> Result<u16, String> {
    for o in ops {
        if let Op::R(r) = o {
            if !r.vec {
                return Ok(r.bits);
            }
        }
    }
    for o in ops {
        if let Op::M(m) = o {
            if m.size != 0 {
                return Ok(m.size);
            }
        }
    }
    Err("ambiguous operand size: add `byte ptr` / `word ptr` / `dword ptr` / `qword ptr`".to_string())
}

fn is_int_rm(o: &Op) -> bool {
    matches!(o, Op::R(r) if !r.vec) || matches!(o, Op::M(_))
}

fn is_acc(o: &Op) -> bool {
    matches!(o, Op::R(r) if r.n == 0 && !r.high && !r.vec)
}

fn same_size(a: &R, b: &R, mn: &str) -> Result<(), String> {
    if a.bits != b.bits {
        return Err(format!("operand size mismatch for `{}`", mn));
    }
    Ok(())
}

/// Normalises an immediate to the signed value of an operand of `bits` (32-bit 0xffffffff = -1).
fn norm_imm(k: i128, bits: u16) -> Result<i128, String> {
    let (lo, hi, m) = match bits {
        8 => (-128i128, 255i128, 1i128 << 8),
        16 => (-32768, 65535, 1 << 16),
        32 => (-(1i128 << 31), (1i128 << 32) - 1, 1 << 32),
        _ => (-(1i128 << 31), (1i128 << 31) - 1, 0),
    };
    if k < lo || k > hi {
        return Err(format!("immediate {} out of range for a {}-bit operand", k, bits));
    }
    if m != 0 && k >= m / 2 {
        return Ok(k - m);
    }
    Ok(k)
}

fn alu(ctx: &mut Ctx, mn: &str, n: u8, ops: &[Op]) -> Result<(), String> {
    if ops.len() != 2 {
        return Err(bad(mn));
    }
    let nb = |bits: u16| (bits != 8) as u8;
    match (&ops[0], &ops[1]) {
        (Op::R(a), Op::R(b)) if !a.vec && !b.vec => {
            same_size(a, b, mn)?;
            gen(ctx, &mk(a.bits, ops, 0, (n << 3) + nb(a.bits)), b.enc(), &ops[0], &[])
        }
        (Op::R(a), Op::M(_)) if !a.vec => gen(ctx, &mk(a.bits, ops, 0, (n << 3) + 2 + nb(a.bits)), a.enc(), &ops[1], &[]),
        (Op::M(_), Op::R(b)) if !b.vec => gen(ctx, &mk(b.bits, ops, 0, (n << 3) + nb(b.bits)), b.enc(), &ops[0], &[]),
        (dst, Op::I(v)) if is_int_rm(dst) => {
            let bits = opsize(&ops[..1])?;
            let k = norm_imm(konst(v)?, bits)?;
            let f = |op: u8| mk(bits, &ops[..1], 0, op);
            if bits == 8 {
                if is_acc(dst) {
                    return gen_plus(ctx, &f((n << 3) | 4), &gpr(0, 8), &imm_le(k, 1)?);
                }
                return gen(ctx, &f(0x80), n, dst, &imm_le(k, 1)?);
            }
            if (-128..128).contains(&k) {
                return gen(ctx, &f(0x83), n, dst, &imm_le(k, 1)?);
            }
            let isz = if bits == 16 { 2 } else { 4 };
            if is_acc(dst) {
                return gen_plus(ctx, &f((n << 3) | 5), &gpr(0, bits), &imm_le(k, isz)?);
            }
            gen(ctx, &f(0x81), n, dst, &imm_le(k, isz)?)
        }
        _ => Err(bad(mn)),
    }
}

fn mov(ctx: &mut Ctx, mn: &str, ops: &[Op], force_abs: bool) -> Result<(), String> {
    if ops.len() != 2 {
        return Err(bad(mn));
    }
    let nb = |bits: u16| (bits != 8) as u8;
    match (&ops[0], &ops[1]) {
        (Op::R(a), Op::R(b)) if !a.vec && !b.vec => {
            same_size(a, b, mn)?;
            gen(ctx, &mk(a.bits, ops, 0, 0x88 + nb(a.bits)), b.enc(), &ops[0], &[])
        }
        (Op::R(a), Op::M(_)) if !a.vec => gen(ctx, &mk(a.bits, ops, 0, 0x8A + nb(a.bits)), a.enc(), &ops[1], &[]),
        (Op::M(_), Op::R(b)) if !b.vec => gen(ctx, &mk(b.bits, ops, 0, 0x88 + nb(b.bits)), b.enc(), &ops[0], &[]),
        (Op::R(a), Op::I(v)) if !a.vec => {
            let f = mk(a.bits, ops, 0, 0);
            match v {
                Value::Const(k) => {
                    let k = *k;
                    match a.bits {
                        8 => gen_plus(ctx, &F { op: 0xB0, ..f }, a, &imm_le(norm_imm(k, 8)?, 1)?),
                        16 => gen_plus(ctx, &F { op: 0xB8, ..f }, a, &imm_le(norm_imm(k, 16)?, 2)?),
                        32 => gen_plus(ctx, &F { op: 0xB8, ..f }, a, &imm_le(norm_imm(k, 32)?, 4)?),
                        _ => {
                            if !force_abs && (-(1i128 << 31)..(1i128 << 31)).contains(&k) {
                                gen(ctx, &F { op: 0xC7, ..f }, 0, &ops[0], &imm_le(k, 4)?)
                            } else {
                                if k < i64::MIN as i128 || k > u64::MAX as i128 {
                                    return Err(format!("immediate {} out of range for a 64-bit operand", k));
                                }
                                gen_plus(ctx, &F { op: 0xB8, ..f }, a, &(k as u64).to_le_bytes())
                            }
                        }
                    }
                }
                _ => match a.bits {
                    32 => {
                        gen_plus(ctx, &F { op: 0xB8, ..f }, a, &[])?;
                        let at = ctx.pos();
                        ctx.emit(&[0; 4]);
                        ctx.fixup(at, FixKind::Abs32, v.clone());
                        Ok(())
                    }
                    64 => {
                        gen_plus(ctx, &F { op: 0xB8, ..f }, a, &[])?;
                        let at = ctx.pos();
                        ctx.emit(&[0; 8]);
                        ctx.fixup(at, FixKind::Abs64, v.clone());
                        Ok(())
                    }
                    _ => Err("a symbol needs a 32 or 64-bit register".to_string()),
                },
            }
        }
        (Op::M(m), Op::I(v)) => {
            let bits = if m.size != 0 { m.size } else { return Err(format!("ambiguous operand size for `{}`", mn)) };
            let k = norm_imm(konst(v)?, bits)?;
            let isz = match bits {
                8 => 1,
                16 => 2,
                _ => 4,
            };
            gen(ctx, &mk(bits, ops, 0, 0xC6 + nb(bits)), 0, &ops[0], &imm_le(k, isz)?)
        }
        _ => Err(bad(mn)),
    }
}

/// F6/F7 and FE/FF unary groups.
fn group3(ctx: &mut Ctx, mn: &str, base: u8, n: u8, ops: &[Op]) -> Result<(), String> {
    if ops.len() != 1 || !is_int_rm(&ops[0]) {
        return Err(bad(mn));
    }
    let bits = opsize(ops)?;
    gen(ctx, &mk(bits, ops, 0, base + (bits != 8) as u8), n, &ops[0], &[])
}

fn shift(ctx: &mut Ctx, mn: &str, n: u8, ops: &[Op]) -> Result<(), String> {
    if ops.is_empty() || ops.len() > 2 || !is_int_rm(&ops[0]) {
        return Err(bad(mn));
    }
    let bits = opsize(&ops[..1])?;
    let nb = (bits != 8) as u8;
    let f = |op: u8| mk(bits, &ops[..1], 0, op + nb);
    match ops.get(1) {
        None => gen(ctx, &f(0xD0), n, &ops[0], &[]),
        Some(Op::R(r)) if !r.vec && r.n == 1 && r.bits == 8 => gen(ctx, &f(0xD2), n, &ops[0], &[]),
        Some(Op::I(v)) => {
            let k = konst(v)?;
            if k == 1 {
                gen(ctx, &f(0xD0), n, &ops[0], &[])
            } else {
                gen(ctx, &f(0xC0), n, &ops[0], &imm_le(k, 1)?)
            }
        }
        _ => Err(bad(mn)),
    }
}

fn is_cl(o: &Op) -> bool {
    matches!(o, Op::R(r) if !r.vec && r.n == 1 && r.bits == 8)
}

/// A relative branch. `short`/`near` opcodes: jmp EB/E9, jcc 7x/0F 8x, call E8 only.
fn branch(ctx: &mut Ctx, mn: &str, ops: &[Op], kind: u8 /*0 jmp, 1 call, 2 jcc*/, cc: u8) -> Result<(), String> {
    if ops.len() != 1 {
        return Err(bad(mn));
    }
    match &ops[0] {
        Op::I(v) => {
            if let Value::Const(_) = v {
                return Err(format!("`{}` to an absolute address is not supported", mn));
            }
            if kind != 1 {
                if let Some((sec, off)) = ctx.label_now(v) {
                    if sec == ctx.section() {
                        let rel = off - (ctx.pos() as i64 + 2);
                        if (-128..128).contains(&rel) {
                            let op = if kind == 0 { 0xEB } else { 0x70 + cc };
                            ctx.emit(&[op, rel as i8 as u8]);
                            return Ok(());
                        }
                    }
                }
            }
            match kind {
                0 => ctx.emit_u8(0xE9),
                1 => ctx.emit_u8(0xE8),
                _ => ctx.emit(&[0x0F, 0x80 + cc]),
            }
            let at = ctx.pos();
            ctx.emit(&[0; 4]);
            ctx.fixup(at, FixKind::X64Branch32, v.clone());
            Ok(())
        }
        o if kind != 2 => match o {
            Op::R(r) if !r.vec && r.bits == 64 => gen(ctx, &mk(32, ops, 0, 0xFF), if kind == 0 { 4 } else { 2 }, o, &[]),
            Op::M(m) if m.size == 0 || m.size == 64 => gen(ctx, &f1(0xFF), if kind == 0 { 4 } else { 2 }, o, &[]),
            _ => Err(bad(mn)),
        },
        _ => Err(bad(mn)),
    }
}

fn short_only(ctx: &mut Ctx, mn: &str, ops: &[Op], pre: &[u8]) -> Result<(), String> {
    if let [Op::I(v)] = ops {
        if let Some((sec, off)) = ctx.label_now(v) {
            if sec == ctx.section() {
                let rel = off - (ctx.pos() as i64 + pre.len() as i64 + 1);
                if (-128..128).contains(&rel) {
                    ctx.emit(pre);
                    ctx.emit_u8(rel as i8 as u8);
                    return Ok(());
                }
            }
        }
        return Err(format!("`{}` needs a backward label within 128 bytes", mn));
    }
    Err(bad(mn))
}

fn simple(mn: &str) -> Option<&'static [u8]> {
    Some(match mn {
        "ud2" => &[0x0F, 0x0B],
        "int3" => &[0xCC],
        "int1" => &[0xF1],
        "hlt" => &[0xF4],
        "pause" => &[0xF3, 0x90],
        "cpuid" => &[0x0F, 0xA2],
        "rdtsc" => &[0x0F, 0x31],
        "rdtscp" => &[0x0F, 0x01, 0xF9],
        "rdpmc" => &[0x0F, 0x33],
        "rdmsr" => &[0x0F, 0x32],
        "wrmsr" => &[0x0F, 0x30],
        "xgetbv" => &[0x0F, 0x01, 0xD0],
        "xsetbv" => &[0x0F, 0x01, 0xD1],
        "syscall" => &[0x0F, 0x05],
        "sysret" | "sysretl" => &[0x0F, 0x07],
        "sysretq" => &[0x48, 0x0F, 0x07],
        "sysenter" => &[0x0F, 0x34],
        "swapgs" => &[0x0F, 0x01, 0xF8],
        "lfence" => &[0x0F, 0xAE, 0xE8],
        "mfence" => &[0x0F, 0xAE, 0xF0],
        "sfence" => &[0x0F, 0xAE, 0xF8],
        "cld" => &[0xFC],
        "std" => &[0xFD],
        "clc" => &[0xF8],
        "stc" => &[0xF9],
        "cmc" => &[0xF5],
        "cli" => &[0xFA],
        "sti" => &[0xFB],
        "cqo" => &[0x48, 0x99],
        "cdq" => &[0x99],
        "cwd" => &[0x66, 0x99],
        "cdqe" => &[0x48, 0x98],
        "cwde" => &[0x98],
        "cbw" => &[0x66, 0x98],
        "leave" => &[0xC9],
        "ret" | "retq" => &[0xC3],
        "iret" | "iretd" => &[0xCF],
        "iretq" => &[0x48, 0xCF],
        "sahf" => &[0x9E],
        "lahf" => &[0x9F],
        "pushf" | "pushfq" => &[0x9C],
        "popf" | "popfq" => &[0x9D],
        "movsb" => &[0xA4],
        "movsw" => &[0x66, 0xA5],
        "movsd" => &[0xA5],
        "movsq" => &[0x48, 0xA5],
        "cmpsb" => &[0xA6],
        "cmpsw" => &[0x66, 0xA7],
        "cmpsd" => &[0xA7],
        "cmpsq" => &[0x48, 0xA7],
        "stosb" => &[0xAA],
        "stosw" => &[0x66, 0xAB],
        "stosd" => &[0xAB],
        "stosq" => &[0x48, 0xAB],
        "lodsb" => &[0xAC],
        "lodsw" => &[0x66, 0xAD],
        "lodsd" => &[0xAD],
        "lodsq" => &[0x48, 0xAD],
        "scasb" => &[0xAE],
        "scasw" => &[0x66, 0xAF],
        "scasd" => &[0xAF],
        "scasq" => &[0x48, 0xAF],
        "xlatb" => &[0xD7],
        "wait" | "fwait" => &[0x9B],
        "endbr64" => &[0xF3, 0x0F, 0x1E, 0xFA],
        "emms" => &[0x0F, 0x77],
        "vzeroupper" => &[0xC5, 0xF8, 0x77],
        "vzeroall" => &[0xC5, 0xFC, 0x77],
        "xend" => &[0x0F, 0x01, 0xD5],
        "xtest" => &[0x0F, 0x01, 0xD6],
        _ => return None,
    })
}

/// Integer / system instructions. Ok(false) when `mn` is not one of them.
fn int_insn(ctx: &mut Ctx, mn: &str, ops: &[Op]) -> Result<bool, String> {
    const ALU: [&str; 8] = ["add", "or", "adc", "sbb", "and", "sub", "xor", "cmp"];
    if let Some(n) = ALU.iter().position(|x| *x == mn) {
        alu(ctx, mn, n as u8, ops)?;
        return Ok(true);
    }
    if let Some(b) = simple(mn) {
        if ops.is_empty() {
            let sp = take_pfx(ctx);
            ctx.emit(&sp);
            ctx.emit(b);
            return Ok(true);
        }
        if !matches!(mn, "movsd" | "cmpsd" | "ret" | "retq") {
            return Err(bad(mn));
        }
    }
    // conditional families
    for (pre, kind) in [("cmov", 0), ("set", 1), ("j", 2)] {
        if let Some(c) = mn.strip_prefix(pre) {
            if let Some(cc) = cc_of(c) {
                match kind {
                    0 => match ops {
                        [Op::R(a), src] if !a.vec && is_int_rm(src) && a.bits != 8 => gen(ctx, &mk(a.bits, ops, 1, 0x40 + cc), a.enc(), src, &[])?,
                        _ => return Err(bad(mn)),
                    },
                    1 => match ops {
                        [d] if is_int_rm(d) && ibits(d).is_none_or(|b| b == 8) => gen(ctx, &mk(8, ops, 1, 0x90 + cc), 0, d, &[])?,
                        _ => return Err(bad(mn)),
                    },
                    _ => branch(ctx, mn, ops, 2, cc)?,
                }
                return Ok(true);
            }
        }
    }
    match mn {
        "mov" => mov(ctx, mn, ops, false)?,
        "movabs" => mov(ctx, mn, ops, true)?,
        "movzx" | "movsx" | "movsxd" => {
            let (Some(Op::R(d)), Some(s)) = (ops.first(), ops.get(1)) else { return Err(bad(mn)) };
            if ops.len() != 2 || d.vec || !is_int_rm(s) {
                return Err(bad(mn));
            }
            let sb = ibits(s).ok_or_else(|| format!("ambiguous operand size for `{}`", mn))?;
            let zx = mn == "movzx";
            if sb == 32 && !zx {
                gen(ctx, &mk(d.bits, ops, 0, 0x63), d.enc(), s, &[])?;
            } else if sb == 8 || sb == 16 {
                if sb >= d.bits {
                    return Err(bad(mn));
                }
                let op = (if zx { 0xB6 } else { 0xBE }) + (sb == 16) as u8;
                gen(ctx, &mk(d.bits, ops, 1, op), d.enc(), s, &[])?;
            } else {
                return Err(bad(mn));
            }
        }
        "lea" => match ops {
            [Op::R(d), m @ Op::M(_)] if !d.vec && d.bits != 8 => gen(ctx, &mk(d.bits, &ops[..1], 0, 0x8D), d.enc(), m, &[])?,
            _ => return Err(bad(mn)),
        },
        "test" => {
            if ops.len() != 2 {
                return Err(bad(mn));
            }
            match (&ops[0], &ops[1]) {
                (a, Op::R(b)) if is_int_rm(a) && !b.vec => gen(ctx, &mk(b.bits, ops, 0, 0x84 + (b.bits != 8) as u8), b.enc(), a, &[])?,
                (Op::R(a), m @ Op::M(_)) if !a.vec => gen(ctx, &mk(a.bits, ops, 0, 0x84 + (a.bits != 8) as u8), a.enc(), m, &[])?,
                (dst, Op::I(v)) if is_int_rm(dst) => {
                    let bits = opsize(&ops[..1])?;
                    let k = norm_imm(konst(v)?, bits)?;
                    let isz = match bits {
                        8 => 1,
                        16 => 2,
                        _ => 4,
                    };
                    let f = |op: u8| mk(bits, &ops[..1], 0, op);
                    if is_acc(dst) {
                        gen_plus(ctx, &f(0xA8 + (bits != 8) as u8), &gpr(0, bits), &imm_le(k, isz)?)?;
                    } else {
                        gen(ctx, &f(0xF6 + (bits != 8) as u8), 0, dst, &imm_le(k, isz)?)?;
                    }
                }
                _ => return Err(bad(mn)),
            }
        }
        "not" => group3(ctx, mn, 0xF6, 2, ops)?,
        "neg" => group3(ctx, mn, 0xF6, 3, ops)?,
        "mul" => group3(ctx, mn, 0xF6, 4, ops)?,
        "div" => group3(ctx, mn, 0xF6, 6, ops)?,
        "idiv" => group3(ctx, mn, 0xF6, 7, ops)?,
        "inc" => group3(ctx, mn, 0xFE, 0, ops)?,
        "dec" => group3(ctx, mn, 0xFE, 1, ops)?,
        "imul" => match ops {
            [a] if is_int_rm(a) => group3(ctx, mn, 0xF6, 5, ops)?,
            [Op::R(d), s] if !d.vec && is_int_rm(s) && d.bits != 8 => gen(ctx, &mk(d.bits, ops, 1, 0xAF), d.enc(), s, &[])?,
            [Op::R(d), s, Op::I(v)] | [Op::R(d), s @ Op::R(_), Op::I(v)] if !d.vec && is_int_rm(s) && d.bits != 8 => {
                let k = norm_imm(konst(v)?, d.bits)?;
                if (-128..128).contains(&k) {
                    gen(ctx, &mk(d.bits, ops, 0, 0x6B), d.enc(), s, &imm_le(k, 1)?)?;
                } else {
                    gen(ctx, &mk(d.bits, ops, 0, 0x69), d.enc(), s, &imm_le(k, if d.bits == 16 { 2 } else { 4 })?)?;
                }
            }
            [Op::R(d), Op::I(v)] if !d.vec && d.bits != 8 => {
                let k = norm_imm(konst(v)?, d.bits)?;
                if (-128..128).contains(&k) {
                    gen(ctx, &mk(d.bits, ops, 0, 0x6B), d.enc(), &ops[0], &imm_le(k, 1)?)?;
                } else {
                    gen(ctx, &mk(d.bits, ops, 0, 0x69), d.enc(), &ops[0], &imm_le(k, if d.bits == 16 { 2 } else { 4 })?)?;
                }
            }
            _ => return Err(bad(mn)),
        },
        "rol" => shift(ctx, mn, 0, ops)?,
        "ror" => shift(ctx, mn, 1, ops)?,
        "rcl" => shift(ctx, mn, 2, ops)?,
        "rcr" => shift(ctx, mn, 3, ops)?,
        "shl" | "sal" => shift(ctx, mn, 4, ops)?,
        "shr" => shift(ctx, mn, 5, ops)?,
        "sar" => shift(ctx, mn, 7, ops)?,
        "shld" | "shrd" => {
            let base = if mn == "shld" { 0xA4 } else { 0xAC };
            match ops {
                [d, Op::R(s), Op::I(v)] if is_int_rm(d) && !s.vec => gen(ctx, &mk(s.bits, ops, 1, base), s.enc(), d, &imm_le(konst(v)?, 1)?)?,
                [d, Op::R(s), c] if is_int_rm(d) && !s.vec && is_cl(c) => gen(ctx, &mk(s.bits, &ops[..2], 1, base + 1), s.enc(), d, &[])?,
                _ => return Err(bad(mn)),
            }
        }
        "bt" | "bts" | "btr" | "btc" => {
            let (rop, n) = match mn {
                "bt" => (0xA3, 4),
                "bts" => (0xAB, 5),
                "btr" => (0xB3, 6),
                _ => (0xBB, 7),
            };
            match ops {
                [d, Op::R(s)] if is_int_rm(d) && !s.vec => gen(ctx, &mk(s.bits, ops, 1, rop), s.enc(), d, &[])?,
                [d, Op::I(v)] if is_int_rm(d) => {
                    let bits = opsize(&ops[..1])?;
                    gen(ctx, &mk(bits, &ops[..1], 1, 0xBA), n, d, &imm_le(konst(v)?, 1)?)?
                }
                _ => return Err(bad(mn)),
            }
        }
        "bsf" | "bsr" | "popcnt" | "lzcnt" | "tzcnt" => match ops {
            [Op::R(d), s] if !d.vec && is_int_rm(s) && d.bits != 8 => {
                let (pfx, op) = match mn {
                    "bsf" => (0, 0xBC),
                    "bsr" => (0, 0xBD),
                    "popcnt" => (0xF3, 0xB8),
                    "lzcnt" => (0xF3, 0xBD),
                    _ => (0xF3, 0xBC),
                };
                gen(ctx, &F { pfx, ..mk(d.bits, ops, 1, op) }, d.enc(), s, &[])?
            }
            _ => return Err(bad(mn)),
        },
        "bswap" => match ops {
            [Op::R(r)] if !r.vec && (r.bits == 32 || r.bits == 64) => gen_plus(ctx, &mk(r.bits, ops, 1, 0xC8), r, &[])?,
            _ => return Err(bad(mn)),
        },
        "xchg" => match ops {
            [Op::R(a), Op::R(b)] if !a.vec && !b.vec => {
                same_size(a, b, mn)?;
                if a.bits == 64 && a.n == 0 && b.n == 0 {
                    ctx.emit_u8(0x90);
                } else if a.bits != 8 && a != b && (a.n == 0 || b.n == 0) {
                    let other = if a.n == 0 { b } else { a };
                    gen_plus(ctx, &mk(a.bits, ops, 0, 0x90), other, &[])?;
                } else {
                    gen(ctx, &mk(a.bits, ops, 0, 0x86 + (a.bits != 8) as u8), a.enc(), &ops[1], &[])?;
                }
            }
            [Op::R(a), m @ Op::M(_)] | [m @ Op::M(_), Op::R(a)] if !a.vec => gen(ctx, &mk(a.bits, &[Op::R(*a)], 0, 0x86 + (a.bits != 8) as u8), a.enc(), m, &[])?,
            _ => return Err(bad(mn)),
        },
        "cmpxchg" | "xadd" => {
            let base = if mn == "cmpxchg" { 0xB0 } else { 0xC0 };
            match ops {
                [d, Op::R(s)] if is_int_rm(d) && !s.vec => gen(ctx, &mk(s.bits, ops, 1, base + (s.bits != 8) as u8), s.enc(), d, &[])?,
                _ => return Err(bad(mn)),
            }
        }
        "cmpxchg16b" | "cmpxchg8b" => match ops {
            [m @ Op::M(_)] => gen(ctx, &F { w: mn == "cmpxchg16b", ..f0f(0xC7) }, 1, m, &[])?,
            _ => return Err(bad(mn)),
        },
        "jmp" => branch(ctx, mn, ops, 0, 0)?,
        "call" => branch(ctx, mn, ops, 1, 0)?,
        "loop" => short_only(ctx, mn, ops, &[0xE2])?,
        "loope" | "loopz" => short_only(ctx, mn, ops, &[0xE1])?,
        "loopne" | "loopnz" => short_only(ctx, mn, ops, &[0xE0])?,
        "jrcxz" => short_only(ctx, mn, ops, &[0xE3])?,
        "jecxz" => short_only(ctx, mn, ops, &[0x67, 0xE3])?,
        "push" | "pop" => {
            let push = mn == "push";
            match ops {
                [Op::R(r)] if !r.vec && (r.bits == 64 || r.bits == 16) => gen_plus(ctx, &F { p66: r.bits == 16, op: if push { 0x50 } else { 0x58 }, ..F::default() }, r, &[])?,
                [m @ Op::M(mm)] if mm.size == 0 || mm.size == 64 || mm.size == 16 => {
                    let f = F { p66: mm.size == 16, op: if push { 0xFF } else { 0x8F }, ..F::default() };
                    gen(ctx, &f, if push { 6 } else { 0 }, m, &[])?
                }
                [Op::I(v)] if push => {
                    let k = konst(v)?;
                    if (-128..128).contains(&k) {
                        ctx.emit(&[0x6A]);
                        ctx.emit(&imm_le(k, 1)?);
                    } else {
                        ctx.emit_u8(0x68);
                        ctx.emit(&imm_le(norm_imm(k, 64)?, 4)?);
                    }
                }
                _ => return Err(bad(mn)),
            }
        }
        "ret" | "retq" | "retn" => match ops {
            [Op::I(v)] => {
                ctx.emit_u8(0xC2);
                ctx.emit(&imm_le(konst(v)?, 2)?);
            }
            _ => return Err(bad(mn)),
        },
        "nop" => match ops {
            [] => ctx.emit_u8(0x90),
            [o] if is_int_rm(o) => {
                let bits = ibits(o).unwrap_or(32);
                gen(ctx, &mk(bits, ops, 1, 0x1F), 0, o, &[])?
            }
            _ => return Err(bad(mn)),
        },
        "int" => match ops {
            [Op::I(v)] => {
                let k = konst(v)?;
                if k == 3 {
                    ctx.emit_u8(0xCC);
                } else {
                    ctx.emit_u8(0xCD);
                    ctx.emit(&imm_le(k, 1)?);
                }
            }
            _ => return Err(bad(mn)),
        },
        "enter" => match ops {
            [Op::I(a), Op::I(b)] => {
                ctx.emit_u8(0xC8);
                ctx.emit(&imm_le(konst(a)?, 2)?);
                ctx.emit(&imm_le(konst(b)?, 1)?);
            }
            _ => return Err(bad(mn)),
        },
        "rdrand" | "rdseed" => match ops {
            [o @ Op::R(r)] if !r.vec && r.bits != 8 => gen(ctx, &mk(r.bits, ops, 1, 0xC7), if mn == "rdrand" { 6 } else { 7 }, o, &[])?,
            _ => return Err(bad(mn)),
        },
        "rdfsbase" | "rdgsbase" | "wrfsbase" | "wrgsbase" => match ops {
            [o @ Op::R(r)] if !r.vec && r.bits >= 32 => {
                let n = match mn {
                    "rdfsbase" => 0,
                    "rdgsbase" => 1,
                    "wrfsbase" => 2,
                    _ => 3,
                };
                gen(ctx, &F { pfx: 0xF3, ..mk(r.bits, ops, 1, 0xAE) }, n, o, &[])?
            }
            _ => return Err(bad(mn)),
        },
        "prefetchnta" | "prefetcht0" | "prefetcht1" | "prefetcht2" | "prefetchw" | "prefetchwt1" => match ops {
            [m @ Op::M(_)] => {
                let (op, n) = match mn {
                    "prefetchnta" => (0x18, 0),
                    "prefetcht0" => (0x18, 1),
                    "prefetcht1" => (0x18, 2),
                    "prefetcht2" => (0x18, 3),
                    "prefetchw" => (0x0D, 1),
                    _ => (0x0D, 2),
                };
                gen(ctx, &f0f(op), n, m, &[])?
            }
            _ => return Err(bad(mn)),
        },
        "clflush" | "clflushopt" | "clwb" | "stmxcsr" | "ldmxcsr" | "fxsave" | "fxrstor" | "fxsave64" | "fxrstor64" | "xsave" | "xrstor" | "xsaveopt" | "xsavec" | "xsave64" | "xrstor64" | "xsaveopt64" | "xsavec64" => match ops {
            [m @ Op::M(_)] => {
                let name = mn.strip_suffix("64").unwrap_or(mn);
                let w = name.len() != mn.len();
                let (pfx, map_op, n) = match name {
                    "clflush" => (0, 0xAE, 7),
                    "clflushopt" => (0x66, 0xAE, 7),
                    "clwb" => (0x66, 0xAE, 6),
                    "stmxcsr" => (0, 0xAE, 3),
                    "ldmxcsr" => (0, 0xAE, 2),
                    "fxsave" => (0, 0xAE, 0),
                    "fxrstor" => (0, 0xAE, 1),
                    "xsave" => (0, 0xAE, 4),
                    "xrstor" => (0, 0xAE, 5),
                    "xsaveopt" => (0, 0xAE, 6),
                    _ => (0, 0xC7, 4),
                };
                if name == "xsavec" {
                    gen(ctx, &F { w, ..f0f(0xC7) }, 4, m, &[])?
                } else {
                    gen(ctx, &F { pfx, w, ..f0f(map_op) }, n, m, &[])?
                }
            }
            _ => return Err(bad(mn)),
        },
        "vstmxcsr" | "vldmxcsr" => match ops {
            [m @ Op::M(_)] => gen_vex(ctx, 0, 1, false, false, 0xAE, if mn == "vstmxcsr" { 3 } else { 2 }, 0, m, &[])?,
            _ => return Err(bad(mn)),
        },
        _ => return Ok(false),
    }
    Ok(true)
}

// ---------------------------------------------------------------- SSE / AVX

#[derive(Clone, Copy, PartialEq)]
enum K {
    /// dst, src (legacy); dst, src1, src2 (VEX)
    A,
    /// dst, src in both
    B,
    /// A with a trailing imm8
    AI,
    /// B with a trailing imm8
    BI,
}

#[derive(Clone, Copy)]
struct I {
    pfx: u8,
    map: u8,
    op: u8,
    k: K,
    w: bool,
    /// scalar: VEX.L is 0 whatever the register width
    lig: bool,
}

fn info(pfx: u8, map: u8, op: u8, k: K) -> Option<I> {
    Some(I { pfx, map, op, k, w: false, lig: false })
}

fn sse_info(mn: &str) -> Option<I> {
    // the ps / pd / ss / sd families
    if mn.len() > 3 {
        let (stem, suf) = mn.split_at(mn.len() - 2);
        let sfx = match suf {
            "ps" => Some((0u8, false)),
            "pd" => Some((0x66, false)),
            "ss" => Some((0xF3, true)),
            "sd" => Some((0xF2, true)),
            _ => None,
        };
        if let Some((pfx, scalar)) = sfx {
            let all = |op: u8| Some(I { pfx, map: 1, op, k: K::A, w: false, lig: scalar });
            let r = match stem {
                "add" => all(0x58),
                "mul" => all(0x59),
                "sub" => all(0x5C),
                "min" => all(0x5D),
                "div" => all(0x5E),
                "max" => all(0x5F),
                "sqrt" => Some(I { pfx, map: 1, op: 0x51, k: if scalar { K::A } else { K::B }, w: false, lig: scalar }),
                "rcp" | "rsqrt" if pfx == 0 || pfx == 0xF3 => Some(I { pfx, map: 1, op: if stem == "rcp" { 0x53 } else { 0x52 }, k: if scalar { K::A } else { K::B }, w: false, lig: scalar }),
                "and" | "andn" | "or" | "xor" if !scalar => all(match stem {
                    "and" => 0x54,
                    "andn" => 0x55,
                    "or" => 0x56,
                    _ => 0x57,
                })
                .map(|i| I { lig: false, ..i }),
                "unpckl" if !scalar => all(0x14),
                "unpckh" if !scalar => all(0x15),
                "shuf" if !scalar => Some(I { pfx, map: 1, op: 0xC6, k: K::AI, w: false, lig: false }),
                "cmp" => Some(I { pfx, map: 1, op: 0xC2, k: K::AI, w: false, lig: scalar }),
                _ if stem.starts_with("cmp") && cmp_pred(mn).is_some() => Some(I { pfx, map: 1, op: 0xC2, k: K::AI, w: false, lig: scalar }),
                "ucomi" | "comi" if scalar => {
                    // ucomiss / ucomisd: "ss" means single (no prefix), "sd" double (66)
                    let p = if suf == "ss" { 0 } else { 0x66 };
                    Some(I { pfx: p, map: 1, op: if stem == "ucomi" { 0x2E } else { 0x2F }, k: K::B, w: false, lig: true })
                }
                "hadd" if suf == "ps" || suf == "pd" => Some(I { pfx: if suf == "ps" { 0xF2 } else { 0x66 }, map: 1, op: 0x7C, k: K::A, w: false, lig: false }),
                "hsub" if suf == "ps" || suf == "pd" => Some(I { pfx: if suf == "ps" { 0xF2 } else { 0x66 }, map: 1, op: 0x7D, k: K::A, w: false, lig: false }),
                "addsub" if suf == "ps" || suf == "pd" => Some(I { pfx: if suf == "ps" { 0xF2 } else { 0x66 }, map: 1, op: 0xD0, k: K::A, w: false, lig: false }),
                "round" => Some(I { pfx: 0x66, map: 3, op: match suf { "ps" => 0x08, "pd" => 0x09, "ss" => 0x0A, _ => 0x0B }, k: if scalar { K::AI } else { K::BI }, w: false, lig: scalar }),
                "blend" if !scalar => Some(I { pfx: 0x66, map: 3, op: if suf == "ps" { 0x0C } else { 0x0D }, k: K::AI, w: false, lig: false }),
                "insert" if suf == "ps" => Some(I { pfx: 0x66, map: 3, op: 0x21, k: K::AI, w: false, lig: true }),
                "dp" if !scalar => Some(I { pfx: 0x66, map: 3, op: if suf == "ps" { 0x40 } else { 0x41 }, k: K::AI, w: false, lig: false }),
                _ => None,
            };
            if r.is_some() {
                return r;
            }
        }
    }
    match mn {
        "cvtss2sd" => Some(I { pfx: 0xF3, map: 1, op: 0x5A, k: K::A, w: false, lig: true }),
        "cvtsd2ss" => Some(I { pfx: 0xF2, map: 1, op: 0x5A, k: K::A, w: false, lig: true }),
        "cvtps2pd" => info(0, 1, 0x5A, K::B),
        "cvtpd2ps" => info(0x66, 1, 0x5A, K::B),
        "cvtdq2ps" => info(0, 1, 0x5B, K::B),
        "cvtps2dq" => info(0x66, 1, 0x5B, K::B),
        "cvttps2dq" => info(0xF3, 1, 0x5B, K::B),
        "cvtdq2pd" => info(0xF3, 1, 0xE6, K::B),
        "cvtpd2dq" => info(0xF2, 1, 0xE6, K::B),
        "cvttpd2dq" => info(0x66, 1, 0xE6, K::B),
        "movhlps" => info(0, 1, 0x12, K::A),
        "movlhps" => info(0, 1, 0x16, K::A),
        "pshufd" => info(0x66, 1, 0x70, K::BI),
        "pshufhw" => info(0xF3, 1, 0x70, K::BI),
        "pshuflw" => info(0xF2, 1, 0x70, K::BI),
        "palignr" => info(0x66, 3, 0x0F, K::AI),
        "pblendw" => info(0x66, 3, 0x0E, K::AI),
        "pclmulqdq" => info(0x66, 3, 0x44, K::AI),
        "mpsadbw" => info(0x66, 3, 0x42, K::AI),
        "ptest" => info(0x66, 2, 0x17, K::B),
        "aesimc" => info(0x66, 2, 0xDB, K::B),
        "aesenc" => info(0x66, 2, 0xDC, K::A),
        "aesenclast" => info(0x66, 2, 0xDD, K::A),
        "aesdec" => info(0x66, 2, 0xDE, K::A),
        "aesdeclast" => info(0x66, 2, 0xDF, K::A),
        "aeskeygenassist" => info(0x66, 3, 0xDF, K::BI),
        _ => {
            // 66 0F integer ops
            let op = match mn {
                "pxor" => 0xEF,
                "pand" => 0xDB,
                "pandn" => 0xDF,
                "por" => 0xEB,
                "paddb" => 0xFC,
                "paddw" => 0xFD,
                "paddd" => 0xFE,
                "paddq" => 0xD4,
                "paddsb" => 0xEC,
                "paddsw" => 0xED,
                "paddusb" => 0xDC,
                "paddusw" => 0xDD,
                "psubb" => 0xF8,
                "psubw" => 0xF9,
                "psubd" => 0xFA,
                "psubq" => 0xFB,
                "psubsb" => 0xE8,
                "psubsw" => 0xE9,
                "psubusb" => 0xD8,
                "psubusw" => 0xD9,
                "pcmpeqb" => 0x74,
                "pcmpeqw" => 0x75,
                "pcmpeqd" => 0x76,
                "pcmpgtb" => 0x64,
                "pcmpgtw" => 0x65,
                "pcmpgtd" => 0x66,
                "punpcklbw" => 0x60,
                "punpcklwd" => 0x61,
                "punpckldq" => 0x62,
                "punpckhbw" => 0x68,
                "punpckhwd" => 0x69,
                "punpckhdq" => 0x6A,
                "punpcklqdq" => 0x6C,
                "punpckhqdq" => 0x6D,
                "packsswb" => 0x63,
                "packuswb" => 0x67,
                "packssdw" => 0x6B,
                "pmullw" => 0xD5,
                "pmulhw" => 0xE5,
                "pmulhuw" => 0xE4,
                "pmuludq" => 0xF4,
                "pmaddwd" => 0xF5,
                "pminub" => 0xDA,
                "pmaxub" => 0xDE,
                "pminsw" => 0xEA,
                "pmaxsw" => 0xEE,
                "pavgb" => 0xE0,
                "pavgw" => 0xE3,
                "psadbw" => 0xF6,
                "psllw" => 0xF1,
                "pslld" => 0xF2,
                "psllq" => 0xF3,
                "psrlw" => 0xD1,
                "psrld" => 0xD2,
                "psrlq" => 0xD3,
                "psraw" => 0xE1,
                "psrad" => 0xE2,
                _ => 0,
            };
            if op != 0 {
                return info(0x66, 1, op, K::A);
            }
            // 66 0F 38
            let (op, k) = match mn {
                "pshufb" => (0x00, K::A),
                "phaddw" => (0x01, K::A),
                "phaddd" => (0x02, K::A),
                "phsubw" => (0x05, K::A),
                "phsubd" => (0x06, K::A),
                "psignb" => (0x08, K::A),
                "psignw" => (0x09, K::A),
                "psignd" => (0x0A, K::A),
                "pmulhrsw" => (0x0B, K::A),
                "pabsb" => (0x1C, K::B),
                "pabsw" => (0x1D, K::B),
                "pabsd" => (0x1E, K::B),
                "pmovsxbw" => (0x20, K::B),
                "pmovsxbd" => (0x21, K::B),
                "pmovsxbq" => (0x22, K::B),
                "pmovsxwd" => (0x23, K::B),
                "pmovsxwq" => (0x24, K::B),
                "pmovsxdq" => (0x25, K::B),
                "pmuldq" => (0x28, K::A),
                "pcmpeqq" => (0x29, K::A),
                "packusdw" => (0x2B, K::A),
                "pmovzxbw" => (0x30, K::B),
                "pmovzxbd" => (0x31, K::B),
                "pmovzxbq" => (0x32, K::B),
                "pmovzxwd" => (0x33, K::B),
                "pmovzxwq" => (0x34, K::B),
                "pmovzxdq" => (0x35, K::B),
                "pcmpgtq" => (0x37, K::A),
                "pminsb" => (0x38, K::A),
                "pminsd" => (0x39, K::A),
                "pminuw" => (0x3A, K::A),
                "pminud" => (0x3B, K::A),
                "pmaxsb" => (0x3C, K::A),
                "pmaxsd" => (0x3D, K::A),
                "pmaxuw" => (0x3E, K::A),
                "pmaxud" => (0x3F, K::A),
                "pmulld" => (0x40, K::A),
                _ => return None,
            };
            info(0x66, 2, op, k)
        }
    }
}

fn pp_of(pfx: u8) -> u8 {
    match pfx {
        0x66 => 1,
        0xF3 => 2,
        0xF2 => 3,
        _ => 0,
    }
}

fn is_vecr(o: &Op) -> bool {
    matches!(o, Op::R(r) if r.vec)
}
fn is_vecrm(o: &Op) -> bool {
    is_vecr(o) || matches!(o, Op::M(_))
}
fn vn(o: &Op) -> u8 {
    match o {
        Op::R(r) => r.n,
        _ => 0,
    }
}
fn wide(ops: &[Op]) -> bool {
    ops.iter().any(|o| matches!(o, Op::R(r) if r.vec && r.bits == 256))
}
fn no_zmm(ops: &[Op]) -> Result<(), String> {
    if ops.iter().any(|o| matches!(o, Op::R(r) if r.vec && r.bits == 512)) {
        return Err("zmm registers need EVEX, which is not supported".to_string());
    }
    Ok(())
}

/// A vec reg (or gpr) dst + r/m with VEX or legacy encoding.
/// `vv`: the VEX.vvvv register (None = unused).
fn vecgen(ctx: &mut Ctx, vex: bool, pfx: u8, map: u8, w: bool, l: bool, op: u8, reg: u8, vv: Option<u8>, rm: &Op, imm: &[u8], rex8: bool) -> Result<(), String> {
    if vex {
        gen_vex(ctx, pp_of(pfx), map, w, l, op, reg, vv.unwrap_or(0), rm, imm)
    } else {
        gen(ctx, &F { pfx, w, map, op, rex8, ..F::default() }, reg, rm, imm)
    }
}

/// fmadd/fmsub/fnmadd/fnmsub + 132/213/231 + ps/pd/ss/sd: (opcode, VEX.W).
fn fma_info(nm: &str) -> Option<(u8, bool)> {
    let (kind, rest) = if let Some(r) = nm.strip_prefix("fnmadd") {
        (4u8, r)
    } else if let Some(r) = nm.strip_prefix("fnmsub") {
        (6, r)
    } else if let Some(r) = nm.strip_prefix("fmadd") {
        (0, r)
    } else if let Some(r) = nm.strip_prefix("fmsub") {
        (2, r)
    } else {
        return None;
    };
    let base = match rest.get(..3)? {
        "132" => 0x90u8,
        "213" => 0xA0,
        "231" => 0xB0,
        _ => return None,
    };
    let (w, scalar) = match &rest[3..] {
        "ps" => (false, false),
        "pd" => (true, false),
        "ss" => (false, true),
        "sd" => (true, true),
        _ => return None,
    };
    Some((base + 8 + kind + scalar as u8, w))
}

fn sse_insn(ctx: &mut Ctx, mn: &str, ops: &[Op]) -> Result<bool, String> {
    let vex = mn.starts_with('v') && mn.len() > 2;
    let nm = if vex { &mn[1..] } else { mn };
    let r = sse_insn2(ctx, mn, nm, vex, ops)?;
    if !r && vex {
        // legacy mnemonic that happens to start with v: none
        return Ok(false);
    }
    Ok(r)
}

fn sse_insn2(ctx: &mut Ctx, mn: &str, nm: &str, vex: bool, ops: &[Op]) -> Result<bool, String> {
    no_zmm(ops)?;
    let l = wide(ops);
    // ---- fused multiply-add (VEX only)
    if vex {
        if let Some((op, w)) = fma_info(nm) {
            let scalar = nm.ends_with("ss") || nm.ends_with("sd");
            return match ops {
                [d, s1, s2] if is_vecr(d) && is_vecr(s1) && is_vecrm(s2) => {
                    gen_vex(ctx, 1, 2, w, l && !scalar, op, vn(d), vn(s1), s2, &[])?;
                    Ok(true)
                }
                _ => Err(bad(mn)),
            };
        }
    }
    // ---- moves
    let movlike = match nm {
        "movaps" => Some((0u8, 0x28u8, Some(0x29u8))),
        "movups" => Some((0, 0x10, Some(0x11))),
        "movapd" => Some((0x66, 0x28, Some(0x29))),
        "movupd" => Some((0x66, 0x10, Some(0x11))),
        "movdqa" => Some((0x66, 0x6F, Some(0x7F))),
        "movdqu" => Some((0xF3, 0x6F, Some(0x7F))),
        "movntps" => Some((0, 0, Some(0x2B))),
        "movntpd" => Some((0x66, 0, Some(0x2B))),
        "movntdq" => Some((0x66, 0, Some(0xE7))),
        "movlps" => Some((0, 0x12, Some(0x13))),
        "movhps" => Some((0, 0x16, Some(0x17))),
        "movlpd" => Some((0x66, 0x12, Some(0x13))),
        "movhpd" => Some((0x66, 0x16, Some(0x17))),
        "movddup" => Some((0xF2, 0x12, None)),
        "movshdup" => Some((0xF3, 0x16, None)),
        "movsldup" => Some((0xF3, 0x12, None)),
        "lddqu" => Some((0xF2, 0xF0, None)),
        _ => None,
    };
    if let Some((pfx, load, store)) = movlike {
        let three = vex && matches!(nm, "movlps" | "movhps" | "movlpd" | "movhpd") && ops.len() == 3;
        return match ops {
            [d, s] if is_vecr(d) && is_vecrm(s) && load != 0 => {
                vecgen(ctx, vex, pfx, 1, false, l, load, vn(d), None, s, &[], false)?;
                Ok(true)
            }
            [d @ Op::M(_), s] if is_vecr(s) && store.is_some() => {
                vecgen(ctx, vex, pfx, 1, false, l, store.unwrap(), vn(s), None, d, &[], false)?;
                Ok(true)
            }
            [d, s1, s2 @ Op::M(_)] if three && is_vecr(d) && is_vecr(s1) => {
                gen_vex(ctx, pp_of(pfx), 1, false, false, load, vn(d), vn(s1), s2, &[])?;
                Ok(true)
            }
            _ => Err(bad(mn)),
        };
    }
    if matches!(nm, "movss" | "movsd") && !ops.is_empty() {
        let pfx = if nm == "movss" { 0xF3 } else { 0xF2 };
        return match ops {
            [d, s] if is_vecr(d) && is_vecrm(s) => {
                vecgen(ctx, vex, pfx, 1, false, false, 0x10, vn(d), None, s, &[], false)?;
                Ok(true)
            }
            [d @ Op::M(_), s] if is_vecr(s) => {
                vecgen(ctx, vex, pfx, 1, false, false, 0x11, vn(s), None, d, &[], false)?;
                Ok(true)
            }
            [d, s1, s2] if vex && is_vecr(d) && is_vecr(s1) && is_vecr(s2) => {
                gen_vex(ctx, pp_of(pfx), 1, false, false, 0x10, vn(d), vn(s1), s2, &[])?;
                Ok(true)
            }
            _ => Err(bad(mn)),
        };
    }
    match nm {
        "movd" | "movq" => {
            let q = nm == "movq";
            return match ops {
                [d, s] if is_vecr(d) && matches!(s, Op::R(r) if !r.vec) => {
                    let Op::R(r) = s else { unreachable!() };
                    if r.bits != if q { 64 } else { 32 } {
                        return Err(bad(mn));
                    }
                    vecgen(ctx, vex, 0x66, 1, q, false, 0x6E, vn(d), None, s, &[], false)?;
                    Ok(true)
                }
                [d, s @ Op::M(m)] if is_vecr(d) && (!q || m.size == 0 || m.size == 64) => {
                    if q {
                        vecgen(ctx, vex, 0xF3, 1, false, false, 0x7E, vn(d), None, s, &[], false)?;
                    } else {
                        vecgen(ctx, vex, 0x66, 1, false, false, 0x6E, vn(d), None, s, &[], false)?;
                    }
                    Ok(true)
                }
                [d, s] if q && is_vecr(d) && is_vecr(s) => {
                    vecgen(ctx, vex, 0xF3, 1, false, false, 0x7E, vn(d), None, s, &[], false)?;
                    Ok(true)
                }
                [d @ Op::R(r), s] if !r.vec && is_vecr(s) => {
                    if r.bits != if q { 64 } else { 32 } {
                        return Err(bad(mn));
                    }
                    vecgen(ctx, vex, 0x66, 1, q, false, 0x7E, vn(s), None, d, &[], false)?;
                    Ok(true)
                }
                [d @ Op::M(_), s] if is_vecr(s) => {
                    if q {
                        vecgen(ctx, vex, 0x66, 1, false, false, 0xD6, vn(s), None, d, &[], false)?;
                    } else {
                        vecgen(ctx, vex, 0x66, 1, false, false, 0x7E, vn(s), None, d, &[], false)?;
                    }
                    Ok(true)
                }
                _ => Err(bad(mn)),
            };
        }
        "cvtsi2ss" | "cvtsi2sd" => {
            let pfx = if nm == "cvtsi2ss" { 0xF3 } else { 0xF2 };
            let (d, v1, s) = match ops {
                [d, s] if !vex => (d, None, s),
                [d, s1, s] if vex => (d, Some(s1), s),
                _ => return Err(bad(mn)),
            };
            if !is_vecr(d) || !is_int_rm(s) {
                return Err(bad(mn));
            }
            let bits = ibits(s).ok_or_else(|| format!("ambiguous operand size for `{}`", mn))?;
            if bits != 32 && bits != 64 {
                return Err(bad(mn));
            }
            vecgen(ctx, vex, pfx, 1, bits == 64, false, 0x2A, vn(d), v1.map(vn), s, &[], false)?;
            return Ok(true);
        }
        "cvttss2si" | "cvttsd2si" | "cvtss2si" | "cvtsd2si" => {
            let pfx = if nm.contains("ss") { 0xF3 } else { 0xF2 };
            let op = if nm.starts_with("cvtt") { 0x2C } else { 0x2D };
            return match ops {
                [d @ Op::R(r), s] if !r.vec && (r.bits == 32 || r.bits == 64) && is_vecrm(s) => {
                    vecgen(ctx, vex, pfx, 1, r.bits == 64, false, op, r.enc(), None, s, &[], false)?;
                    let _ = d;
                    Ok(true)
                }
                _ => Err(bad(mn)),
            };
        }
        "movmskps" | "movmskpd" | "pmovmskb" => {
            let pfx = if nm == "movmskps" { 0 } else { 0x66 };
            let op = if nm == "pmovmskb" { 0xD7 } else { 0x50 };
            return match ops {
                [Op::R(r), s] if !r.vec && is_vecr(s) && r.bits >= 32 => {
                    vecgen(ctx, vex, pfx, 1, false, l, op, r.enc(), None, s, &[], false)?;
                    Ok(true)
                }
                _ => Err(bad(mn)),
            };
        }
        "pextrb" | "pextrw" | "pextrd" | "pextrq" | "extractps" => {
            let (op, w) = match nm {
                "pextrb" => (0x14, false),
                "pextrw" => (0x15, false),
                "pextrd" => (0x16, false),
                "pextrq" => (0x16, true),
                _ => (0x17, false),
            };
            return match ops {
                [d, s, Op::I(v)] if is_vecr(s) && is_int_rm(d) => {
                    let imm = imm_le(konst(v)?, 1)?;
                    if nm == "pextrw" {
                        if let Op::R(r) = d {
                            // 66 0F C5 /r ib: reg = gpr, rm = xmm
                            vecgen(ctx, vex, 0x66, 1, false, false, 0xC5, r.enc(), None, s, &imm, false)?;
                            return Ok(true);
                        }
                    }
                    vecgen(ctx, vex, 0x66, 3, w, false, op, vn(s), None, d, &imm, false)?;
                    Ok(true)
                }
                _ => Err(bad(mn)),
            };
        }
        "pinsrb" | "pinsrw" | "pinsrd" | "pinsrq" => {
            let (map, op, w) = match nm {
                "pinsrb" => (3, 0x20, false),
                "pinsrw" => (1, 0xC4, false),
                "pinsrd" => (3, 0x22, false),
                _ => (3, 0x22, true),
            };
            let (d, v1, s, v) = match ops {
                [d, s, Op::I(v)] if !vex => (d, None, s, v),
                [d, s1, s, Op::I(v)] if vex => (d, Some(s1), s, v),
                _ => return Err(bad(mn)),
            };
            if !is_vecr(d) || !is_int_rm(s) {
                return Err(bad(mn));
            }
            let imm = imm_le(konst(v)?, 1)?;
            vecgen(ctx, vex, 0x66, map, w, false, op, vn(d), v1.map(vn), s, &imm, false)?;
            return Ok(true);
        }
        "pslldq" | "psrldq" | "psllw" | "pslld" | "psllq" | "psrlw" | "psrld" | "psrlq" | "psraw" | "psrad" if matches!(ops.last(), Some(Op::I(_))) => {
            let (op, n) = match nm {
                "psllw" => (0x71, 6),
                "pslld" => (0x72, 6),
                "psllq" => (0x73, 6),
                "psrlw" => (0x71, 2),
                "psrld" => (0x72, 2),
                "psrlq" => (0x73, 2),
                "psraw" => (0x71, 4),
                "psrad" => (0x72, 4),
                "pslldq" => (0x73, 7),
                _ => (0x73, 3),
            };
            let Some(Op::I(v)) = ops.last() else { unreachable!() };
            let imm = imm_le(konst(v)?, 1)?;
            return match ops {
                [d, _] if !vex && is_vecr(d) => {
                    vecgen(ctx, false, 0x66, 1, false, false, op, n, None, d, &imm, false)?;
                    Ok(true)
                }
                [d, s, _] if vex && is_vecr(d) && is_vecr(s) => {
                    gen_vex(ctx, 1, 1, false, l, op, n, vn(d), s, &imm)?;
                    Ok(true)
                }
                _ => Err(bad(mn)),
            };
        }
        // VEX-only data movement
        "broadcastss" | "broadcastsd" | "broadcastf128" | "broadcasti128" | "pbroadcastb" | "pbroadcastw" | "pbroadcastd" | "pbroadcastq" if vex => {
            let op = match nm {
                "broadcastss" => 0x18,
                "broadcastsd" => 0x19,
                "broadcastf128" => 0x1A,
                "broadcasti128" => 0x5A,
                "pbroadcastb" => 0x78,
                "pbroadcastw" => 0x79,
                "pbroadcastd" => 0x58,
                _ => 0x59,
            };
            return match ops {
                [d, s] if is_vecr(d) && is_vecrm(s) => {
                    gen_vex(ctx, 1, 2, false, l, op, vn(d), 0, s, &[])?;
                    Ok(true)
                }
                _ => Err(bad(mn)),
            };
        }
        "perm2f128" | "perm2i128" if vex => {
            let op = if nm == "perm2f128" { 0x06 } else { 0x46 };
            return match ops {
                [d, s1, s2, Op::I(v)] if is_vecr(d) && is_vecr(s1) && is_vecrm(s2) => {
                    gen_vex(ctx, 1, 3, false, true, op, vn(d), vn(s1), s2, &imm_le(konst(v)?, 1)?)?;
                    Ok(true)
                }
                _ => Err(bad(mn)),
            };
        }
        "permq" | "permpd" if vex => {
            let op = if nm == "permq" { 0x00 } else { 0x01 };
            return match ops {
                [d, s, Op::I(v)] if is_vecr(d) && is_vecrm(s) => {
                    gen_vex(ctx, 1, 3, true, true, op, vn(d), 0, s, &imm_le(konst(v)?, 1)?)?;
                    Ok(true)
                }
                _ => Err(bad(mn)),
            };
        }
        "permd" | "permps" if vex => {
            let op = if nm == "permd" { 0x36 } else { 0x16 };
            return match ops {
                [d, s1, s2] if is_vecr(d) && is_vecr(s1) && is_vecrm(s2) => {
                    gen_vex(ctx, 1, 2, false, true, op, vn(d), vn(s1), s2, &[])?;
                    Ok(true)
                }
                _ => Err(bad(mn)),
            };
        }
        "insertf128" | "inserti128" if vex => {
            let op = if nm == "insertf128" { 0x18 } else { 0x38 };
            return match ops {
                [d, s1, s2, Op::I(v)] if is_vecr(d) && is_vecr(s1) && is_vecrm(s2) => {
                    gen_vex(ctx, 1, 3, false, true, op, vn(d), vn(s1), s2, &imm_le(konst(v)?, 1)?)?;
                    Ok(true)
                }
                _ => Err(bad(mn)),
            };
        }
        "extractf128" | "extracti128" if vex => {
            let op = if nm == "extractf128" { 0x19 } else { 0x39 };
            return match ops {
                [d, s, Op::I(v)] if is_vecrm(d) && is_vecr(s) => {
                    gen_vex(ctx, 1, 3, false, true, op, vn(s), 0, d, &imm_le(konst(v)?, 1)?)?;
                    Ok(true)
                }
                _ => Err(bad(mn)),
            };
        }
        "blendvps" | "blendvpd" | "pblendvb" if vex => {
            let op = match nm {
                "blendvps" => 0x4A,
                "blendvpd" => 0x4B,
                _ => 0x4C,
            };
            return match ops {
                [d, s1, s2, Op::R(m)] if is_vecr(d) && is_vecr(s1) && is_vecrm(s2) && m.vec => {
                    gen_vex(ctx, 1, 3, false, l, op, vn(d), vn(s1), s2, &[m.n << 4])?;
                    Ok(true)
                }
                _ => Err(bad(mn)),
            };
        }
        _ => {}
    }
    // ---- the generic families
    let Some(mut i) = sse_info(nm) else { return Ok(false) };
    let mut ops2: Vec<Op>;
    let mut ops = ops;
    // cmpeqps ... : the predicate in the mnemonic
    if i.k == K::AI && i.op == 0xC2 && i.map == 1 {
        // `cmp<pred>ps`: handled by the caller's name split; plain `cmpps` has an explicit imm
    }
    let _ = &mut i;
    if let Some(p) = cmp_pred(nm) {
        ops2 = ops.to_vec();
        ops2.push(Op::I(Value::Const(p as i128)));
        ops = &ops2;
    }
    let l = l && !i.lig;
    let imm_of = |ops: &[Op]| -> Result<Vec<u8>, String> {
        match ops.last() {
            Some(Op::I(v)) => imm_le(konst(v)?, 1),
            _ => Err(bad(mn)),
        }
    };
    match i.k {
        K::A | K::AI => {
            let (nreg, imm) = if i.k == K::AI { (ops.len() - 1, imm_of(ops)?) } else { (ops.len(), vec![]) };
            let regs = &ops[..nreg.min(ops.len())];
            if i.k == K::AI && ops.is_empty() {
                return Err(bad(mn));
            }
            match (vex, regs) {
                (false, [d, s]) if is_vecr(d) && is_vecrm(s) => vecgen(ctx, false, i.pfx, i.map, i.w, false, i.op, vn(d), None, s, &imm, false)?,
                (true, [d, s1, s2]) if is_vecr(d) && is_vecr(s1) && is_vecrm(s2) => gen_vex(ctx, pp_of(i.pfx), i.map, i.w, l, i.op, vn(d), vn(s1), s2, &imm)?,
                _ => return Err(bad(mn)),
            }
        }
        K::B | K::BI => {
            let (nreg, imm) = if i.k == K::BI { (ops.len().saturating_sub(1), imm_of(ops)?) } else { (ops.len(), vec![]) };
            let regs = &ops[..nreg];
            match regs {
                [d, s] if is_vecr(d) && is_vecrm(s) => vecgen(ctx, vex, i.pfx, i.map, i.w, l, i.op, vn(d), None, s, &imm, false)?,
                _ => return Err(bad(mn)),
            }
        }
    }
    Ok(true)
}

/// `cmpeqps` ... `cmpordsd`: the predicate number.
fn cmp_pred(nm: &str) -> Option<u8> {
    let rest = nm.strip_prefix("cmp")?;
    let (p, _) = rest.split_at(rest.len().checked_sub(2)?);
    Some(match p {
        "eq" => 0,
        "lt" => 1,
        "le" => 2,
        "unord" => 3,
        "neq" => 4,
        "nlt" => 5,
        "nle" => 6,
        "ord" => 7,
        _ => return None,
    })
}

// ---------------------------------------------------------------- the encoder

#[derive(Default)]
pub struct X64 {}

fn is_branch(mn: &str) -> bool {
    matches!(mn, "jmp" | "call" | "loop" | "loope" | "loopne" | "loopz" | "loopnz" | "jrcxz" | "jecxz") || mn.strip_prefix('j').is_some_and(|c| cc_of(c).is_some())
}

fn dispatch(ctx: &mut Ctx, mn: &str, ops: &[Op]) -> Result<bool, String> {
    if int_insn(ctx, mn, ops)? {
        return Ok(true);
    }
    sse_insn(ctx, mn, ops)
}

/// AT&T mnemonic spellings that are not `base + size suffix`.
fn att_alias(mn: &str) -> Option<&'static str> {
    Some(match mn {
        "cltq" => "cdqe",
        "cqto" => "cqo",
        "cltd" => "cdq",
        "cwtl" => "cwde",
        "cbtw" => "cbw",
        "cwtd" => "cwd",
        "movsl" => "movsd",
        "stosl" => "stosd",
        "lodsl" => "lodsd",
        "scasl" => "scasd",
        "cmpsl" => "cmpsd",
        "movslq" => "movsxd",
        "retq" => "ret",
        _ => return None,
    })
}

fn att_bits(c: char) -> Option<u16> {
    Some(match c {
        'b' => 8,
        'w' => 16,
        'l' => 32,
        'q' => 64,
        _ => return None,
    })
}

fn set_mem_size(ops: &mut [Op], bits: u16) {
    for o in ops {
        if let Op::M(m) = o {
            if m.size == 0 {
                m.size = bits;
            }
        }
    }
}

impl Encoder for X64 {
    fn instruction(&mut self, ctx: &mut Ctx, mnemonic: &str, ops: &[Tok]) -> Result<(), String> {
        let mn = mnemonic.to_ascii_lowercase();
        let mn = mn.as_str();
        let prefix = match mn {
            "lock" => Some(0xF0),
            "rep" | "repe" | "repz" | "xrelease" => Some(0xF3),
            "repne" | "repnz" | "xacquire" => Some(0xF2),
            "data16" => Some(0x66),
            "addr32" => Some(0x67),
            _ => None,
        };
        if let Some(p) = prefix {
            if PFX_START.with(|c| c.get()).is_none() {
                PFX_START.with(|c| c.set(Some(ctx.pos())));
            }
            ctx.emit_u8(p);
            let r = match ops.first() {
                None => Ok(()),
                Some(Tok::Ident(n)) => {
                    let n = n.clone();
                    self.instruction(ctx, &n, &ops[1..])
                }
                Some(_) => Err(format!("invalid instruction after `{}`", mn)),
            };
            PFX_START.with(|c| c.set(None));
            return r;
        }
        let att = ctx.att;
        let branch = is_branch(mn) || (att && mn.strip_suffix('q').is_some_and(is_branch));
        let mut parsed: Vec<Op> = Vec::new();
        for g in split_commas(ops) {
            parsed.push(if att { parse_att(ctx, g, branch)? } else { parse_intel(ctx, g)? });
        }
        if att && mn != "enter" {
            parsed.reverse();
        }
        let first = dispatch(ctx, mn, &parsed);
        if let Ok(true) = first {
            return Ok(());
        }
        if att {
            if let Some(a) = att_alias(mn) {
                if dispatch(ctx, a, &parsed)? {
                    return Ok(());
                }
            }
            // movzbl / movzwq / movsbw ...: movz / movs + source + destination size
            if mn.len() == 6 && (mn.starts_with("movz") || mn.starts_with("movs")) {
                let c: Vec<char> = mn.chars().collect();
                if let (Some(sb), Some(_)) = (att_bits(c[4]), att_bits(c[5])) {
                    set_mem_size(&mut parsed, sb);
                    let base = if mn.starts_with("movz") { "movzx" } else { "movsx" };
                    if dispatch(ctx, base, &parsed)? {
                        return Ok(());
                    }
                }
            }
            if let Some(last) = mn.chars().last() {
                if let Some(bits) = att_bits(last) {
                    let base = &mn[..mn.len() - 1];
                    set_mem_size(&mut parsed, bits);
                    if let Ok(true) = dispatch(ctx, base, &parsed) {
                        return Ok(());
                    }
                }
            }
        }
        first?;
        Err(format!("unsupported instruction `{}`", mnemonic))
    }
    fn directive(&mut self, ctx: &mut Ctx, name: &str, _args: &[Tok]) -> Result<bool, String> {
        match name {
            ".intel_syntax" => ctx.att = false,
            ".att_syntax" => ctx.att = true,
            ".code64" => {}
            _ => return Ok(false),
        }
        Ok(true)
    }
    fn code_fill(&self, n: usize) -> Vec<u8> {
        vec![0x90; n]
    }
}

