//! AArch64 instruction encoder for [`super`] (the syntax LLVM's integrated assembler reads: Mach-O
//! `sym@PAGE` / `sym@PAGEOFF`, ELF `:lo12:sym`; both are accepted on both formats).
//!
//! Encodings follow the Arm ARM; every form is checked against `clang --target=aarch64` in
//! tests_arm64.rs. Where LLVM picks among equivalent encodings the same choice is made (mov
//! immediates: movz, then movn, then orr; ldr with an unaligned or negative offset becomes ldur;
//! add with a negative immediate becomes sub; add/sub with sp use the extended-register form).
//! Not supported: SVE/SME, pointer authentication, the crypto and dot-product extensions,
//! `ldr x, =imm` literal pools, half-precision vector by-element forms.

use super::{split_commas, Ctx, Encoder, FixKind, PReg, Tok, Value};

#[derive(Default)]
pub struct Arm64 {}

type Res<T> = Result<T, String>;

/// Sentinel error: the operands do not fit the mnemonic (the message names the mnemonic).
fn bad<T>() -> Res<T> {
    Err("!".to_string())
}
fn msg<T>(s: impl Into<String>) -> Res<T> {
    Err(s.into())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum K {
    X,
    W,
    B,
    H,
    S,
    D,
    Q,
    V,
}

/// A register as written: `sp` marks register 31 as the stack pointer (otherwise it is zr).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Reg {
    k: K,
    n: u8,
    sp: bool,
}

impl Reg {
    fn is_gp(self) -> bool {
        matches!(self.k, K::X | K::W)
    }
    fn is_fp(self) -> bool {
        matches!(self.k, K::B | K::H | K::S | K::D | K::Q)
    }
    fn sf(self) -> u32 {
        (self.k == K::X) as u32
    }
    fn n(self) -> u32 {
        self.n as u32
    }
    fn zr(k: K) -> Reg {
        Reg { k, n: 31, sp: false }
    }
    /// 32 or 64
    fn bits(self) -> u32 {
        if self.k == K::X {
            64
        } else {
            32
        }
    }
}

/// A vector arrangement: `n` lanes of `1 << es` bytes (es 4 = q); n = 0: an element (`v0.s[1]`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Arr {
    n: u8,
    es: u8,
}

impl Arr {
    fn q(self) -> u32 {
        (self.n as u32 * (1 << self.es) == 16) as u32
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Spec {
    None,
    /// `sym@PAGE`
    Page,
    /// `sym@PAGEOFF` / `:lo12:sym`
    PageOff,
}

enum Off {
    None,
    Imm(Value),
    Sym(Value, Spec),
    Reg(Reg, Option<(String, Option<i64>)>),
}

struct Mem {
    base: Reg,
    off: Off,
    pre: bool,
}

enum Op<'a> {
    Reg(Reg, Option<Arr>, Option<i64>),
    Expr { toks: &'a [Tok], spec: Spec },
    /// a shift or extend: `lsl #3`, `uxtw`, `sxtx #2`, `msl #8`
    Ext(String, Option<i64>),
    Mem(Mem),
    List(Vec<Reg>, Option<Arr>, Option<i64>),
}

// ---------------------------------------------------------------- registers

fn reg_digits(s: &str) -> Option<u8> {
    if s.is_empty() || s.len() > 2 || !s.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// A register name and the arrangement suffix after its `.` ("" when none).
fn parse_reg_text(name: &str) -> Option<(Reg, &str)> {
    let (base, suf) = match name.find('.') {
        Some(i) => (&name[..i], &name[i + 1..]),
        None => (name, ""),
    };
    let b = base.to_ascii_lowercase();
    let r = match b.as_str() {
        "sp" => Reg { k: K::X, n: 31, sp: true },
        "wsp" => Reg { k: K::W, n: 31, sp: true },
        "xzr" => Reg::zr(K::X),
        "wzr" => Reg::zr(K::W),
        "lr" => Reg { k: K::X, n: 30, sp: false },
        "fp" => Reg { k: K::X, n: 29, sp: false },
        "ip0" => Reg { k: K::X, n: 16, sp: false },
        "ip1" => Reg { k: K::X, n: 17, sp: false },
        _ => {
            let k = match b.as_bytes().first()? {
                b'x' => K::X,
                b'w' => K::W,
                b'v' => K::V,
                b'b' => K::B,
                b'h' => K::H,
                b's' => K::S,
                b'd' => K::D,
                b'q' => K::Q,
                _ => return None,
            };
            let n = reg_digits(&b[1..])?;
            if n > if matches!(k, K::X | K::W) { 30 } else { 31 } {
                return None;
            }
            Reg { k, n, sp: false }
        }
    };
    if !suf.is_empty() && r.k != K::V {
        return None;
    }
    Some((r, suf))
}

/// A register named in an `in("x0")` / `out("v8")` operand: its physical register and the width
/// in bits (w: 32, x: 64, b/h/s/d/q: 8/16/32/64/128, v: 128). Case-insensitive; `lr` and `fp`
/// are x30 and x29. `sp` and the zero registers are not allocatable registers.
pub fn parse_reg(name: &str) -> Option<(PReg, u8)> {
    let (r, suf) = parse_reg_text(name)?;
    if !suf.is_empty() || (r.n == 31 && matches!(r.k, K::X | K::W)) {
        return None;
    }
    let bits = match r.k {
        K::W => 32,
        K::X => 64,
        K::B => 8,
        K::H => 16,
        K::S => 32,
        K::D => 64,
        K::Q | K::V => 128,
    };
    Some((if matches!(r.k, K::X | K::W) { PReg::int(r.n) } else { PReg::vec(r.n) }, bits))
}

fn parse_arr(s: &str) -> Option<Arr> {
    let s = s.to_ascii_lowercase();
    let digits = s.bytes().take_while(|c| c.is_ascii_digit()).count();
    let n: u8 = if digits == 0 { 0 } else { s[..digits].parse().ok()? };
    let es = match &s[digits..] {
        "b" => 0,
        "h" => 1,
        "s" => 2,
        "d" => 3,
        "q" => 4,
        _ => return None,
    };
    if n != 0 {
        let bytes = n as u32 * (1 << es);
        if bytes != 8 && bytes != 16 {
            return None;
        }
    }
    Some(Arr { n, es })
}

fn reg_from_tok(p: PReg, m: Option<char>) -> Res<Reg> {
    let k = match (p.vec, m.unwrap_or(if p.vec { 'v' } else { 'x' })) {
        (false, 'x') => K::X,
        (false, 'w') => K::W,
        (true, 'v') => K::V,
        (true, 'b') => K::B,
        (true, 'h') => K::H,
        (true, 's') => K::S,
        (true, 'd') => K::D,
        (true, 'q') => K::Q,
        (_, m) => return msg(format!("invalid register template modifier `{}` for this register", m)),
    };
    Ok(Reg { k, n: p.n, sp: false })
}

/// A register (with its arrangement suffix) at t[i]: the register, the arrangement, the index
/// after it.
fn reg_at(t: &[Tok], i: usize) -> Res<Option<(Reg, Option<Arr>, usize)>> {
    match t.get(i) {
        Some(Tok::Reg(p, m)) => {
            let r = reg_from_tok(*p, *m)?;
            if r.k == K::V {
                if let Some(Tok::Ident(s)) = t.get(i + 1) {
                    if let Some(rest) = s.strip_prefix('.') {
                        let a = parse_arr(rest).ok_or_else(|| format!("invalid vector arrangement `{}`", s))?;
                        return Ok(Some((r, Some(a), i + 2)));
                    }
                }
            }
            Ok(Some((r, None, i + 1)))
        }
        Some(Tok::Ident(name)) => match parse_reg_text(name) {
            Some((r, suf)) => {
                if suf.is_empty() {
                    Ok(Some((r, None, i + 1)))
                } else {
                    let a = parse_arr(suf).ok_or_else(|| format!("invalid vector arrangement `{}`", suf))?;
                    Ok(Some((r, Some(a), i + 1)))
                }
            }
            None => Ok(None),
        },
        _ => Ok(None),
    }
}

// ---------------------------------------------------------------- operands

fn ext_name(s: &str) -> bool {
    matches!(s, "lsl" | "lsr" | "asr" | "ror" | "msl" | "uxtb" | "uxth" | "uxtw" | "uxtx" | "sxtb" | "sxth" | "sxtw" | "sxtx")
}

fn spec_of(name: &str) -> Res<Spec> {
    match name.to_ascii_uppercase().as_str() {
        "PAGE" => Ok(Spec::Page),
        "PAGEOFF" | "LO12" => Ok(Spec::PageOff),
        other => msg(format!("unsupported relocation specifier `{}`", other.to_ascii_lowercase())),
    }
}

/// An expression operand, with a `@PAGE` / `@PAGEOFF` suffix or a `:lo12:` prefix split off.
fn parse_expr_op(t: &[Tok]) -> Res<Op<'_>> {
    if t.is_empty() {
        return msg("missing operand");
    }
    if t[0].is(b':') {
        if let (Some(Tok::Ident(n)), Some(c)) = (t.get(1), t.get(2)) {
            if c.is(b':') {
                let spec = spec_of(n)?;
                return Ok(Op::Expr { toks: &t[3..], spec });
            }
        }
        return msg("malformed relocation specifier");
    }
    if let Some(p) = t.iter().position(|x| x.is(b'@')) {
        return match t.get(p + 1) {
            Some(Tok::Ident(n)) if p + 2 == t.len() => Ok(Op::Expr { toks: &t[..p], spec: spec_of(n)? }),
            _ => msg("malformed relocation specifier"),
        };
    }
    Ok(Op::Expr { toks: t, spec: Spec::None })
}

fn const_i64(ctx: &Ctx, t: &[Tok]) -> Res<i64> {
    let mut i = 0;
    let v = ctx.const_expr(t, &mut i)?;
    if i != t.len() {
        return msg("unexpected tokens after the expression");
    }
    Ok(v as i64)
}

fn val(ctx: &Ctx, t: &[Tok]) -> Res<Value> {
    let mut i = 0;
    let v = ctx.expr(t, &mut i)?;
    if i != t.len() {
        return msg("unexpected tokens after the expression");
    }
    Ok(v)
}

fn parse_op<'a>(ctx: &Ctx, t: &'a [Tok]) -> Res<Op<'a>> {
    match t.first() {
        None => msg("missing operand"),
        Some(Tok::Punct(b'[')) => parse_mem(ctx, t),
        Some(Tok::Punct(b'{')) => parse_list(ctx, t),
        Some(Tok::Punct(b'#')) => parse_expr_op(&t[1..]),
        Some(Tok::Punct(b'=')) => msg("`=value` literal-pool operands are not supported (use movz/movk or adr)"),
        _ => {
            if let Some((r, arr, next)) = reg_at(t, 0)? {
                let mut lane = None;
                let mut i = next;
                if t.get(i).is_some_and(|x| x.is(b'[')) {
                    let mut j = i + 1;
                    let v = ctx.const_expr(t, &mut j)?;
                    if !t.get(j).is_some_and(|x| x.is(b']')) {
                        return msg("expected `]`");
                    }
                    lane = Some(v as i64);
                    i = j + 1;
                }
                if i != t.len() {
                    return msg("unexpected tokens after the register");
                }
                return Ok(Op::Reg(r, arr, lane));
            }
            if let Some(Tok::Ident(n)) = t.first() {
                let l = n.to_ascii_lowercase();
                if ext_name(&l) {
                    let rest = &t[1..];
                    let rest = if rest.first().is_some_and(|x| x.is(b'#')) { &rest[1..] } else { rest };
                    let amt = if rest.is_empty() { None } else { Some(const_i64(ctx, rest)?) };
                    return Ok(Op::Ext(l, amt));
                }
            }
            parse_expr_op(t)
        }
    }
}

fn parse_mem<'a>(ctx: &Ctx, t: &'a [Tok]) -> Res<Op<'a>> {
    let mut depth = 0;
    let mut close = None;
    for (i, x) in t.iter().enumerate() {
        match x {
            Tok::Punct(b'[') => depth += 1,
            Tok::Punct(b']') => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(close) = close else { return msg("expected `]`") };
    let pre = match &t[close + 1..] {
        [] => false,
        [x] if x.is(b'!') => true,
        _ => return msg("unexpected tokens after `]`"),
    };
    let parts = split_commas(&t[1..close]);
    let Some((base_t, rest)) = parts.split_first() else { return msg("empty memory operand") };
    let base = match reg_at(base_t, 0)? {
        Some((r, None, n)) if n == base_t.len() && r.k == K::X && (r.n != 31 || r.sp) => r,
        _ => return msg("a memory operand needs an x register or sp as its base"),
    };
    let off = match rest.first() {
        None => Off::None,
        Some(&o) => {
            if let Some((rm, None, n)) = reg_at(o, 0)? {
                if n != o.len() || !rm.is_gp() || rm.sp {
                    return msg("bad register offset");
                }
                let ext = match rest.get(1) {
                    None => None,
                    Some(e) => match parse_op(ctx, e)? {
                        Op::Ext(n, a) => Some((n, a)),
                        _ => return msg("expected a shift or extend after the offset register"),
                    },
                };
                if rest.len() > 2 {
                    return msg("too many operands in the memory operand");
                }
                Off::Reg(rm, ext)
            } else {
                if rest.len() > 1 {
                    return msg("too many operands in the memory operand");
                }
                let o = if o.first().is_some_and(|x| x.is(b'#')) { &o[1..] } else { o };
                match parse_expr_op(o)? {
                    Op::Expr { toks, spec: Spec::None } => Off::Imm(val(ctx, toks)?),
                    Op::Expr { toks, spec } => Off::Sym(val(ctx, toks)?, spec),
                    _ => unreachable!(),
                }
            }
        }
    };
    Ok(Op::Mem(Mem { base, off, pre }))
}

fn parse_list<'a>(ctx: &Ctx, t: &'a [Tok]) -> Res<Op<'a>> {
    let mut regs: Vec<Reg> = Vec::new();
    let mut arr: Option<Arr> = None;
    let mut i = 1;
    loop {
        let Some((r, a, n)) = reg_at(t, i)? else { return msg("expected a register in the register list") };
        if r.k != K::V {
            return msg("a register list holds vector registers");
        }
        if arr.is_some_and(|x| Some(x) != a) {
            return msg("registers in a list must have the same arrangement");
        }
        arr = a;
        i = n;
        if t.get(i).is_some_and(|x| x.is(b'-')) {
            let Some((r2, a2, n2)) = reg_at(t, i + 1)? else { return msg("expected a register after `-`") };
            if a2 != arr {
                return msg("registers in a list must have the same arrangement");
            }
            let count = (r2.n as i32 - r.n as i32).rem_euclid(32) + 1;
            for k in 0..count {
                regs.push(Reg { k: K::V, n: ((r.n as i32 + k) % 32) as u8, sp: false });
            }
            i = n2;
        } else {
            regs.push(r);
        }
        match t.get(i) {
            Some(x) if x.is(b',') => i += 1,
            Some(x) if x.is(b'}') => {
                i += 1;
                break;
            }
            _ => return msg("expected `,` or `}` in the register list"),
        }
    }
    if regs.len() > 4 {
        return msg("a register list holds at most 4 registers");
    }
    let mut lane = None;
    if t.get(i).is_some_and(|x| x.is(b'[')) {
        let mut j = i + 1;
        let v = ctx.const_expr(t, &mut j)?;
        if !t.get(j).is_some_and(|x| x.is(b']')) {
            return msg("expected `]`");
        }
        lane = Some(v as i64);
        i = j + 1;
    }
    if i != t.len() {
        return msg("unexpected tokens after the register list");
    }
    Ok(Op::List(regs, arr, lane))
}

fn parse_ops<'a>(ctx: &Ctx, toks: &'a [Tok]) -> Res<Vec<Op<'a>>> {
    if toks.is_empty() {
        return Ok(Vec::new());
    }
    split_commas(toks).into_iter().map(|p| parse_op(ctx, p)).collect()
}

// operand accessors: a mismatch is the `bad` sentinel

/// a general register or the zero register (not sp)
fn gp(o: Option<&Op>) -> Res<Reg> {
    match o {
        Some(Op::Reg(r, None, None)) if r.is_gp() && !r.sp => Ok(*r),
        _ => bad(),
    }
}
/// a general register, the zero register or sp
fn gpsp(o: Option<&Op>) -> Res<Reg> {
    match o {
        Some(Op::Reg(r, None, None)) if r.is_gp() => Ok(*r),
        _ => bad(),
    }
}
fn gp_k(o: Option<&Op>, k: K) -> Res<Reg> {
    let r = gp(o)?;
    if r.k == k {
        Ok(r)
    } else {
        bad()
    }
}
fn fpr(o: Option<&Op>) -> Res<Reg> {
    match o {
        Some(Op::Reg(r, None, None)) if r.is_fp() => Ok(*r),
        _ => bad(),
    }
}
/// a vector register with a full arrangement
fn vr(o: Option<&Op>) -> Res<(Reg, Arr)> {
    match o {
        Some(Op::Reg(r, Some(a), None)) if r.k == K::V && a.n != 0 => Ok((*r, *a)),
        _ => bad(),
    }
}
/// a vector element `v0.s[1]`: register, element size, index
fn velem(o: Option<&Op>) -> Res<(Reg, u8, i64)> {
    match o {
        Some(Op::Reg(r, Some(Arr { n: 0, es }), Some(i))) if r.k == K::V => Ok((*r, *es, *i)),
        _ => bad(),
    }
}
fn is_imm(o: Option<&Op>) -> bool {
    matches!(o, Some(Op::Expr { spec: Spec::None, .. }))
}
fn imm(ctx: &Ctx, o: Option<&Op>) -> Res<i64> {
    match o {
        Some(Op::Expr { toks, spec: Spec::None }) => const_i64(ctx, toks),
        _ => bad(),
    }
}
/// a single identifier operand (condition code, barrier option, system register name ...)
fn name(o: Option<&Op>) -> Res<String> {
    match o {
        Some(Op::Expr { toks: [Tok::Ident(n)], spec: Spec::None }) => Ok(n.to_ascii_lowercase()),
        _ => bad(),
    }
}
fn range(v: i64, lo: i64, hi: i64, what: &str) -> Res<u32> {
    if v < lo || v > hi {
        return msg(format!("{} out of range ({} to {})", what, lo, hi));
    }
    Ok(v as u32)
}
fn uimm(ctx: &Ctx, o: Option<&Op>, max: i64, what: &str) -> Res<u32> {
    let v = imm(ctx, o)?;
    range(v, 0, max, what)
}

fn cond_code(s: &str) -> Option<u32> {
    Some(match s {
        "eq" => 0,
        "ne" => 1,
        "cs" | "hs" => 2,
        "cc" | "lo" => 3,
        "mi" => 4,
        "pl" => 5,
        "vs" => 6,
        "vc" => 7,
        "hi" => 8,
        "ls" => 9,
        "ge" => 10,
        "lt" => 11,
        "gt" => 12,
        "le" => 13,
        "al" => 14,
        "nv" => 15,
        _ => return None,
    })
}
fn cond(o: Option<&Op>) -> Res<u32> {
    cond_code(&name(o)?).ok_or_else(|| "unknown condition code".to_string())
}

fn w(ctx: &mut Ctx, v: u32) {
    ctx.emit_u32(v);
}

// ---------------------------------------------------------------- integer core

/// N:immr:imms of a logical (bitmask) immediate, as LLVM's processLogicalImmediate.
fn enc_bitmask(v: u64, size: u32) -> Option<u32> {
    let mask = if size == 64 { !0u64 } else { (1u64 << size) - 1 };
    let v = v & mask;
    if v == 0 || v == mask {
        return None;
    }
    let mut e = size;
    while e > 2 {
        let half = e / 2;
        let m = (1u64 << half) - 1;
        if (v & m) == ((v >> half) & m) {
            e = half;
        } else {
            break;
        }
    }
    let em = if e == 64 { !0u64 } else { (1u64 << e) - 1 };
    let elem = v & em;
    let ones = elem.count_ones();
    let pat = (1u64 << ones) - 1;
    for r in 0..e {
        let rot = if r == 0 { pat } else { ((pat >> r) | (pat << (e - r))) & em };
        if rot == elem {
            let imms = ((0x7eu32 << e.trailing_zeros()) & 0x3f) | (ones - 1);
            return Some(((e == 64) as u32) << 12 | (r << 6) | imms);
        }
    }
    None
}

fn addsub_core(ctx: &mut Ctx, rd: Reg, rn: Reg, third: Option<&Op>, ext: Option<&Op>, mut sub: bool, s: bool) -> Res<()> {
    if rd.k != rn.k {
        return bad();
    }
    let sf = rd.sf();
    if s && rd.sp {
        return msg("the destination of a flag-setting instruction cannot be sp");
    }
    match third {
        Some(Op::Expr { toks, spec }) => {
            let shift = match ext {
                None => 0,
                Some(Op::Ext(n, a)) if n == "lsl" => a.unwrap_or(0),
                _ => return bad(),
            };
            match spec {
                Spec::Page => msg("@PAGE is for adrp"),
                Spec::PageOff => {
                    if rd.k != K::X || shift != 0 || sub || (rn.n == 31 && !rn.sp) || (rd.n == 31 && !rd.sp && !s) {
                        return bad();
                    }
                    let v = val(ctx, toks)?;
                    let at = ctx.pos();
                    let word = 0x1100_0000 | sf << 31 | (s as u32) << 29 | rn.n() << 5 | rd.n();
                    match v {
                        Value::Const(k) => w(ctx, word | ((k as u32) & 0xfff) << 10),
                        other => {
                            w(ctx, word);
                            ctx.fixup(at, FixKind::A64Lo12 { scale: 0 }, other);
                        }
                    }
                    Ok(())
                }
                Spec::None => {
                    if (rn.n == 31 && !rn.sp) || (rd.n == 31 && !rd.sp && !s) {
                        return bad();
                    }
                    let mut k = const_i64(ctx, toks)?;
                    if k < 0 {
                        k = k.checked_neg().ok_or("immediate out of range")?;
                        sub = !sub;
                    }
                    let (imm12, sh) = match shift {
                        0 if k < 4096 => (k, 0),
                        0 if k & 0xfff == 0 && (k >> 12) < 4096 => (k >> 12, 1),
                        12 if k < 4096 => (k, 1),
                        _ => return msg("immediate out of range for add/sub (0 to 4095, optionally shifted left by 12)"),
                    };
                    w(ctx, 0x1100_0000 | sf << 31 | (sub as u32) << 30 | (s as u32) << 29 | (sh as u32) << 22 | (imm12 as u32) << 10 | rn.n() << 5 | rd.n());
                    Ok(())
                }
            }
        }
        Some(Op::Reg(rm, None, None)) if rm.is_gp() && !rm.sp => {
            let ext_kind = matches!(ext, Some(Op::Ext(n, _)) if n.starts_with("uxt") || n.starts_with("sxt"));
            // `add x0, x1, w2, lsl #3` is the extended form with uxtw
            let lsl_ext = matches!(ext, Some(Op::Ext(n, _)) if n == "lsl") && s && rd.k == K::X && rm.k == K::W;
            if (rn.sp || (rd.sp && !s)) && rd.k == K::X && rm.k == K::W && !ext_kind && !(s && lsl_ext) {
                return bad();
            }
            let need_ext = ext_kind || lsl_ext || rn.sp || (rd.sp && !s);
            if !need_ext {
                let (shift, amt) = match ext {
                    None => (0, 0),
                    Some(Op::Ext(n, a)) => (
                        match n.as_str() {
                            "lsl" => 0,
                            "lsr" => 1,
                            "asr" => 2,
                            _ => return bad(),
                        },
                        a.unwrap_or(0),
                    ),
                    _ => return bad(),
                };
                if rm.k != rd.k {
                    return bad();
                }
                let amt = range(amt, 0, rd.bits() as i64 - 1, "shift amount")?;
                w(ctx, 0x0B00_0000 | sf << 31 | (sub as u32) << 30 | (s as u32) << 29 | shift << 22 | rm.n() << 16 | amt << 10 | rn.n() << 5 | rd.n());
            } else {
                if (rn.n == 31 && !rn.sp) || (rd.n == 31 && !rd.sp && !s) {
                    return bad();
                }
                let lsl_opt = if rm.k == K::X { 3 } else { 2 };
                let (opt, amt) = match ext {
                    None => (lsl_opt, 0),
                    Some(Op::Ext(n, a)) => (
                        match n.as_str() {
                            "lsl" => lsl_opt,
                            "uxtb" => 0,
                            "uxth" => 1,
                            "uxtw" => 2,
                            "uxtx" => 3,
                            "sxtb" => 4,
                            "sxth" => 5,
                            "sxtw" => 6,
                            "sxtx" => 7,
                            _ => return bad(),
                        },
                        a.unwrap_or(0),
                    ),
                    _ => return bad(),
                };
                // x registers extended from w (uxtb .. sxtw), or from x (uxtx / sxtx / lsl); a w
                // destination takes w sources with any option
                let ok = if rd.k == K::X {
                    (opt & 3 == 3) == (rm.k == K::X) || (s && rm.k == K::W && opt == 7)
                } else {
                    rm.k == K::W
                };
                if !ok {
                    return bad();
                }
                let amt = range(amt, 0, 4, "extend shift amount")?;
                w(ctx, 0x0B20_0000 | sf << 31 | (sub as u32) << 30 | (s as u32) << 29 | rm.n() << 16 | opt << 13 | amt << 10 | rn.n() << 5 | rd.n());
            }
            Ok(())
        }
        _ => bad(),
    }
}

fn logical_core(ctx: &mut Ctx, rd: Reg, rn: Reg, third: Option<&Op>, ext: Option<&Op>, opc: u32, inv: bool) -> Res<()> {
    if rd.k != rn.k {
        return bad();
    }
    let sf = rd.sf();
    match third {
        Some(Op::Expr { toks, spec: Spec::None }) => {
            if ext.is_some() || rn.sp || (rd.sp && opc == 3) || (rd.n == 31 && !rd.sp && opc != 3) {
                return bad();
            }
            let k = const_i64(ctx, toks)?;
            let bits = rd.bits();
            let mut v: u64 = if bits == 32 {
                if k < i32::MIN as i64 || k > u32::MAX as i64 {
                    return msg("immediate out of range for a 32-bit register");
                }
                k as u32 as u64
            } else {
                k as u64
            };
            if inv {
                v = !v;
            }
            let e = enc_bitmask(v, bits).ok_or("immediate cannot be encoded as a logical immediate")?;
            w(ctx, 0x1200_0000 | sf << 31 | opc << 29 | e << 10 | rn.n() << 5 | rd.n());
            Ok(())
        }
        Some(Op::Reg(rm, None, None)) if rm.is_gp() && !rm.sp => {
            if rd.sp || rn.sp || rm.k != rd.k {
                return bad();
            }
            let (shift, amt) = match ext {
                None => (0, 0),
                Some(Op::Ext(n, a)) => (
                    match n.as_str() {
                        "lsl" => 0,
                        "lsr" => 1,
                        "asr" => 2,
                        "ror" => 3,
                        _ => return bad(),
                    },
                    a.unwrap_or(0),
                ),
                _ => return bad(),
            };
            let amt = range(amt, 0, rd.bits() as i64 - 1, "shift amount")?;
            w(ctx, 0x0A00_0000 | sf << 31 | opc << 29 | shift << 22 | (inv as u32) << 21 | rm.n() << 16 | amt << 10 | rn.n() << 5 | rd.n());
            Ok(())
        }
        _ => bad(),
    }
}

fn bfm(ctx: &mut Ctx, base: u32, rd: Reg, rn: Reg, immr: i64, imms: i64) -> Res<()> {
    let sf = rd.sf();
    w(ctx, base | sf << 31 | sf << 22 | (immr as u32 & 0x3f) << 16 | (imms as u32 & 0x3f) << 10 | rn.n() << 5 | rd.n());
    Ok(())
}
const UBFM: u32 = 0x5300_0000;
const SBFM: u32 = 0x1300_0000;
const BFM: u32 = 0x3300_0000;

fn mov_imm(ctx: &mut Ctx, rd: Reg, k: i64) -> Res<()> {
    let bits = rd.bits();
    let sf = rd.sf();
    let mask = if bits == 32 { 0xffff_ffffu64 } else { !0u64 };
    // LLVM truncates the immediate of a w register to 32 bits
    let v: u64 = if bits == 32 { k as u32 as u64 } else { k as u64 };
    let wide = |val: u64| -> Option<(u32, u32)> {
        if val == 0 {
            return Some((0, 0));
        }
        (0..bits / 16).find(|hw| val & !(0xffffu64 << (16 * hw)) == 0).map(|hw| (hw, (val >> (16 * hw)) as u32))
    };
    if rd.sp {
        if bits == 32 && !(i32::MIN as i64..=u32::MAX as i64).contains(&k) {
            return msg("immediate out of range for a 32-bit register");
        }
        let e = enc_bitmask(v, bits).ok_or("immediate cannot be encoded as a logical immediate")?;
        w(ctx, 0x3200_0000 | sf << 31 | e << 10 | 31 << 5 | 31);
        return Ok(());
    }
    if let Some((hw, i)) = wide(v) {
        w(ctx, 0x5280_0000 | sf << 31 | hw << 21 | i << 5 | rd.n());
    } else if let Some((hw, i)) = wide(!v & mask) {
        w(ctx, 0x1280_0000 | sf << 31 | hw << 21 | i << 5 | rd.n());
    } else if let (Some(e), false, true) = (enc_bitmask(v, bits), rd.n == 31, bits == 64 || (i32::MIN as i64..=u32::MAX as i64).contains(&k)) {
        w(ctx, 0x3200_0000 | sf << 31 | e << 10 | 31 << 5 | rd.n());
    } else {
        return msg("immediate cannot be moved by a single instruction");
    }
    Ok(())
}

fn branch_target(ctx: &mut Ctx, base: u32, kind: FixKind, o: Option<&Op>, bits: u32, shift: u32) -> Res<()> {
    let Some(Op::Expr { toks, spec: Spec::None }) = o else { return bad() };
    match val(ctx, toks)? {
        Value::Const(k) => {
            if k % 4 != 0 {
                return msg("branch offset is not a multiple of 4");
            }
            let words = k / 4;
            if words < -(1i128 << (bits - 1)) || words >= 1i128 << (bits - 1) {
                return msg("branch offset out of range");
            }
            w(ctx, base | ((words as u32) & ((1u32 << bits) - 1)) << shift);
        }
        other => {
            let at = ctx.pos();
            w(ctx, base);
            ctx.fixup(at, kind, other);
        }
    }
    Ok(())
}

fn int_insn(ctx: &mut Ctx, mn: &str, o: &[Op]) -> Res<bool> {
    match mn {
        "add" | "adds" | "sub" | "subs" => {
            let (sub, s) = (mn.starts_with("sub"), mn.ends_with('s'));
            addsub_core(ctx, gpsp(o.first())?, gpsp(o.get(1))?, o.get(2), o.get(3), sub, s)?;
        }
        "cmp" | "cmn" => {
            let rn = gpsp(o.first())?;
            addsub_core(ctx, Reg::zr(rn.k), rn, o.get(1), o.get(2), mn == "cmp", true)?;
        }
        "neg" | "negs" => {
            let rd = gp(o.first())?;
            addsub_core(ctx, rd, Reg::zr(rd.k), o.get(1), o.get(2), true, mn == "negs")?;
        }
        "adc" | "adcs" | "sbc" | "sbcs" | "ngc" | "ngcs" => {
            let rd = gp(o.first())?;
            let (rn, rm) = if mn.starts_with("ngc") { (Reg::zr(rd.k), gp_k(o.get(1), rd.k)?) } else { (gp_k(o.get(1), rd.k)?, gp_k(o.get(2), rd.k)?) };
            let (sub, s) = (mn.starts_with("sbc") || mn.starts_with("ngc"), mn.ends_with('s'));
            w(ctx, 0x1A00_0000 | rd.sf() << 31 | (sub as u32) << 30 | (s as u32) << 29 | rm.n() << 16 | rn.n() << 5 | rd.n());
        }
        "and" | "orr" | "eor" | "ands" | "bic" | "orn" | "eon" | "bics" => {
            let (opc, inv) = match mn {
                "and" => (0, false),
                "orr" => (1, false),
                "eor" => (2, false),
                "ands" => (3, false),
                "bic" => (0, true),
                "orn" => (1, true),
                "eon" => (2, true),
                _ => (3, true),
            };
            logical_core(ctx, gpsp(o.first())?, gp(o.get(1))?, o.get(2), o.get(3), opc, inv)?;
        }
        "tst" => {
            let rn = gp(o.first())?;
            logical_core(ctx, Reg::zr(rn.k), rn, o.get(1), o.get(2), 3, false)?;
        }
        "mvn" => {
            let rd = gp(o.first())?;
            logical_core(ctx, rd, Reg::zr(rd.k), o.get(1), o.get(2), 1, true)?;
        }
        "mov" => {
            if o.len() != 2 {
                return bad();
            }
            match (&o[0], &o[1]) {
                (Op::Reg(rd, None, None), Op::Reg(rm, None, None)) if rd.is_gp() && rm.is_gp() => {
                    if rd.k != rm.k || (rd.sp && rm.n == 31 && !rm.sp) || (rm.sp && rd.n == 31 && !rd.sp) {
                        return bad();
                    }
                    if rd.sp || rm.sp {
                        w(ctx, 0x1100_0000 | rd.sf() << 31 | rm.n() << 5 | rd.n());
                    } else {
                        w(ctx, 0x2A00_03E0 | rd.sf() << 31 | rm.n() << 16 | rd.n());
                    }
                }
                (Op::Reg(rd, None, None), Op::Expr { toks, spec: Spec::None }) if rd.is_gp() => {
                    let k = const_i64(ctx, toks)?;
                    mov_imm(ctx, *rd, k)?;
                }
                _ => return vec_mov(ctx, o).map(|_| true),
            }
        }
        "movz" | "movn" | "movk" => {
            let rd = gp(o.first())?;
            let k = imm(ctx, o.get(1))?;
            let sh = match o.get(2) {
                None => 0,
                Some(Op::Ext(n, a)) if n == "lsl" => a.unwrap_or(0),
                _ => return bad(),
            };
            if sh % 16 != 0 || sh < 0 || sh >= rd.bits() as i64 {
                return msg("movz/movn/movk shift must be 0, 16, 32 or 48");
            }
            let i = range(k, 0, 0xffff, "16-bit immediate")?;
            let base = match mn {
                "movz" => 0x5280_0000,
                "movn" => 0x1280_0000,
                _ => 0x7280_0000,
            };
            w(ctx, base | rd.sf() << 31 | ((sh / 16) as u32) << 21 | i << 5 | rd.n());
        }
        "mul" | "mneg" | "madd" | "msub" => {
            let rd = gp(o.first())?;
            let (rn, rm) = (gp_k(o.get(1), rd.k)?, gp_k(o.get(2), rd.k)?);
            let ra = if mn == "mul" || mn == "mneg" { Reg::zr(rd.k) } else { gp_k(o.get(3), rd.k)? };
            let sub = mn == "mneg" || mn == "msub";
            w(ctx, 0x1B00_0000 | rd.sf() << 31 | rm.n() << 16 | (sub as u32) << 15 | ra.n() << 10 | rn.n() << 5 | rd.n());
        }
        "smull" | "umull" | "smnegl" | "umnegl" | "smaddl" | "umaddl" | "smsubl" | "umsubl" => {
            let rd = gp_k(o.first(), K::X)?;
            let (rn, rm) = (gp_k(o.get(1), K::W)?, gp_k(o.get(2), K::W)?);
            let short = matches!(mn, "smull" | "umull" | "smnegl" | "umnegl");
            let ra = if short { Reg::zr(K::X) } else { gp_k(o.get(3), K::X)? };
            let unsigned = mn.starts_with('u');
            let sub = mn.contains("neg") || mn.contains("sub");
            w(ctx, 0x9B20_0000 | (unsigned as u32) << 23 | rm.n() << 16 | (sub as u32) << 15 | ra.n() << 10 | rn.n() << 5 | rd.n());
        }
        "smulh" | "umulh" => {
            let rd = gp_k(o.first(), K::X)?;
            let (rn, rm) = (gp_k(o.get(1), K::X)?, gp_k(o.get(2), K::X)?);
            w(ctx, if mn == "smulh" { 0x9B40_7C00 } else { 0x9BC0_7C00 } | rm.n() << 16 | rn.n() << 5 | rd.n());
        }
        "sdiv" | "udiv" => {
            let rd = gp(o.first())?;
            let (rn, rm) = (gp_k(o.get(1), rd.k)?, gp_k(o.get(2), rd.k)?);
            w(ctx, 0x1AC0_0800 | rd.sf() << 31 | ((mn == "sdiv") as u32) << 10 | rm.n() << 16 | rn.n() << 5 | rd.n());
        }
        "lsl" | "lsr" | "asr" | "ror" => {
            let kind = match mn {
                "lsl" => 0,
                "lsr" => 1,
                "asr" => 2,
                _ => 3,
            };
            let rd = gp(o.first())?;
            let rn = gp_k(o.get(1), rd.k)?;
            let sf = rd.sf();
            let size = rd.bits() as i64;
            match o.get(2) {
                Some(Op::Reg(rm, None, None)) if rm.k == rd.k && !rm.sp => w(ctx, 0x1AC0_2000 | sf << 31 | kind << 10 | rm.n() << 16 | rn.n() << 5 | rd.n()),
                _ => {
                    let k = imm(ctx, o.get(2))?;
                    range(k, 0, size - 1, "shift amount")?;
                    match kind {
                        0 => bfm(ctx, UBFM, rd, rn, (size - k) % size, size - 1 - k)?,
                        1 => bfm(ctx, UBFM, rd, rn, k, size - 1)?,
                        2 => bfm(ctx, SBFM, rd, rn, k, size - 1)?,
                        _ => w(ctx, 0x1380_0000 | sf << 31 | sf << 22 | rn.n() << 16 | (k as u32) << 10 | rn.n() << 5 | rd.n()),
                    }
                }
            }
        }
        "ubfm" | "sbfm" | "bfm" => {
            let rd = gp(o.first())?;
            let rn = gp_k(o.get(1), rd.k)?;
            let max = rd.bits() as i64 - 1;
            let (a, b) = (imm(ctx, o.get(2))?, imm(ctx, o.get(3))?);
            range(a, 0, max, "immr")?;
            range(b, 0, max, "imms")?;
            bfm(ctx, if mn == "ubfm" { UBFM } else if mn == "sbfm" { SBFM } else { BFM }, rd, rn, a, b)?;
        }
        "ubfx" | "sbfx" | "ubfiz" | "sbfiz" | "bfi" | "bfxil" | "bfc" => {
            let rd = gp(o.first())?;
            let (rn, at) = if mn == "bfc" { (Reg::zr(rd.k), 1) } else { (gp_k(o.get(1), rd.k)?, 2) };
            let size = rd.bits() as i64;
            let (lsb, width) = (imm(ctx, o.get(at))?, imm(ctx, o.get(at + 1))?);
            range(lsb, 0, size - 1, "lsb")?;
            range(width, 1, size - lsb, "width")?;
            match mn {
                "ubfx" => bfm(ctx, UBFM, rd, rn, lsb, lsb + width - 1)?,
                "sbfx" => bfm(ctx, SBFM, rd, rn, lsb, lsb + width - 1)?,
                "bfxil" => bfm(ctx, BFM, rd, rn, lsb, lsb + width - 1)?,
                "ubfiz" => bfm(ctx, UBFM, rd, rn, (size - lsb) % size, width - 1)?,
                "sbfiz" => bfm(ctx, SBFM, rd, rn, (size - lsb) % size, width - 1)?,
                _ => bfm(ctx, BFM, rd, rn, (size - lsb) % size, width - 1)?,
            }
        }
        "uxtb" | "uxth" | "sxtb" | "sxth" | "sxtw" => {
            let rd = gp(o.first())?;
            let rn = gp(o.get(1))?;
            let imms = match &mn[3..] {
                "b" => 7,
                "h" => 15,
                _ => 31,
            };
            let signed_x = mn.starts_with('s') && rd.k == K::X && rn.k == K::X;
            if mn == "sxtw" && rd.k != K::X || (rn.k != K::W && !signed_x) {
                return bad();
            }
            // `uxtb x0, w1` is the 32-bit uxtb
            let rd = if mn.starts_with('u') { Reg { k: K::W, ..rd } } else { rd };
            bfm(ctx, if mn.starts_with('u') { UBFM } else { SBFM }, rd, Reg { k: rd.k, ..rn }, 0, imms)?;
        }
        "extr" => {
            let rd = gp(o.first())?;
            let (rn, rm) = (gp_k(o.get(1), rd.k)?, gp_k(o.get(2), rd.k)?);
            let k = imm(ctx, o.get(3))?;
            let k = range(k, 0, rd.bits() as i64 - 1, "lsb")?;
            w(ctx, 0x1380_0000 | rd.sf() << 31 | rd.sf() << 22 | rm.n() << 16 | k << 10 | rn.n() << 5 | rd.n());
        }
        "uxtw" => {
            let rd = gp_k(o.first(), K::X)?;
            let rn = gp(o.get(1))?;
            bfm(ctx, UBFM, rd, Reg { k: K::X, ..rn }, 0, 31)?;
        }
        m if m.starts_with("crc32") && (m.len() == 6 || m.len() == 7) => {
            let rest = &m[5..];
            let (c, sz) = match rest.strip_prefix('c') {
                Some(r) => (1, r),
                None => (0, rest),
            };
            let size = match sz {
                "b" => 0,
                "h" => 1,
                "w" => 2,
                "x" => 3,
                _ => return Ok(false),
            };
            let rd = gp_k(o.first(), K::W)?;
            let rn = gp_k(o.get(1), K::W)?;
            let rm = gp_k(o.get(2), if size == 3 { K::X } else { K::W })?;
            w(ctx, 0x1AC0_4000 | ((size == 3) as u32) << 31 | rm.n() << 16 | c << 12 | size << 10 | rn.n() << 5 | rd.n());
        }
        "clz" | "cls" | "rbit" | "rev" | "rev16" | "rev32" | "rev64" => {
            let rd = gp(o.first())?;
            let rn = gp_k(o.get(1), rd.k)?;
            let op = match (mn, rd.k) {
                ("rbit", _) => 0,
                ("clz", _) => 4,
                ("cls", _) => 5,
                ("rev16", _) => 1,
                ("rev32", K::X) => 2,
                ("rev", K::W) => 2,
                ("rev", _) | ("rev64", K::X) => 3,
                _ => return bad(),
            };
            w(ctx, 0x5AC0_0000 | rd.sf() << 31 | op << 10 | rn.n() << 5 | rd.n());
        }
        "csel" | "csinc" | "csinv" | "csneg" => {
            let rd = gp(o.first())?;
            let (rn, rm) = (gp_k(o.get(1), rd.k)?, gp_k(o.get(2), rd.k)?);
            let c = cond(o.get(3))?;
            csel(ctx, mn, rd, rn, rm, c);
        }
        "cset" | "csetm" => {
            let rd = gp(o.first())?;
            let c = cond(o.get(1))?;
            if c >= 14 {
                return msg("the condition of cset / csetm cannot be al or nv");
            }
            let c = c ^ 1;
            csel(ctx, if mn == "cset" { "csinc" } else { "csinv" }, rd, Reg::zr(rd.k), Reg::zr(rd.k), c);
        }
        "cinc" | "cinv" | "cneg" => {
            let rd = gp(o.first())?;
            let rn = gp_k(o.get(1), rd.k)?;
            let c = cond(o.get(2))?;
            if c >= 14 {
                return msg("the condition of cinc / cinv / cneg cannot be al or nv");
            }
            csel(ctx, &format!("cs{}", &mn[1..]), rd, rn, rn, c ^ 1);
        }
        "ccmp" | "ccmn" => {
            let rn = gp(o.first())?;
            let nzcv = uimm(ctx, o.get(2), 15, "nzcv")?;
            let c = cond(o.get(3))?;
            let base = 0x3A40_0000 | ((mn == "ccmp") as u32) << 30 | rn.sf() << 31 | c << 12 | rn.n() << 5 | nzcv;
            match o.get(1) {
                Some(Op::Reg(rm, None, None)) if rm.k == rn.k => w(ctx, base | rm.n() << 16),
                _ => {
                    let k = uimm(ctx, o.get(1), 31, "5-bit immediate")?;
                    w(ctx, base | k << 16 | 1 << 11);
                }
            }
        }
        "adr" | "adrp" => {
            let rd = gp_k(o.first(), K::X)?;
            let (base, kind) = if mn == "adr" { (0x1000_0000, FixKind::A64Adr21) } else { (0x9000_0000, FixKind::A64Page21) };
            let Some(Op::Expr { toks, spec }) = o.get(1) else { return bad() };
            if (*spec == Spec::PageOff) || (mn == "adr" && *spec != Spec::None) {
                return bad();
            }
            match val(ctx, toks)? {
                Value::Const(k) => {
                    let k = if mn == "adrp" {
                        if k & 0xfff != 0 {
                            return msg("adrp offset must be a multiple of 4096");
                        }
                        k >> 12
                    } else {
                        k
                    };
                    if !(-(1i128 << 20)..(1i128 << 20)).contains(&k) {
                        return msg("adr/adrp offset out of range");
                    }
                    let k = k as u32;
                    w(ctx, base | (k & 3) << 29 | ((k >> 2) & 0x7ffff) << 5 | rd.n());
                }
                other => {
                    let at = ctx.pos();
                    w(ctx, base | rd.n());
                    ctx.fixup(at, kind, other);
                }
            }
        }
        "b" | "bl" => branch_target(ctx, if mn == "b" { 0x1400_0000 } else { 0x9400_0000 }, FixKind::A64Branch26, o.first(), 26, 0)?,
        "br" | "blr" => {
            let rn = gp_k(o.first(), K::X)?;
            w(ctx, if mn == "br" { 0xD61F_0000 } else { 0xD63F_0000 } | rn.n() << 5);
        }
        "ret" => {
            let rn = if o.is_empty() { 30 } else { gp_k(o.first(), K::X)?.n() };
            w(ctx, 0xD65F_0000 | (rn as u32) << 5);
        }
        "cbz" | "cbnz" => {
            let rt = gp(o.first())?;
            branch_target(ctx, 0x3400_0000 | rt.sf() << 31 | ((mn == "cbnz") as u32) << 24 | rt.n(), FixKind::A64Branch19, o.get(1), 19, 5)?;
        }
        "tbz" | "tbnz" => {
            let rt = gp(o.first())?;
            let bit = uimm(ctx, o.get(1), 63, "bit number")?;
            if bit >= 32 && rt.k != K::X {
                return msg("bit number out of range for a w register");
            }
            branch_target(ctx, 0x3600_0000 | (bit >> 5) << 31 | ((mn == "tbnz") as u32) << 24 | (bit & 31) << 19 | rt.n(), FixKind::A64Branch14, o.get(2), 14, 5)?;
        }
        "eret" => w(ctx, 0xD69F_03E0),
        _ => return Ok(false),
    }
    Ok(true)
}

fn csel(ctx: &mut Ctx, mn: &str, rd: Reg, rn: Reg, rm: Reg, c: u32) {
    let (op, o2) = match mn {
        "csel" => (0, 0),
        "csinc" => (0, 1),
        "csinv" => (1, 0),
        _ => (1, 1),
    };
    w(ctx, 0x1A80_0000 | rd.sf() << 31 | op << 30 | rm.n() << 16 | c << 12 | o2 << 10 | rn.n() << 5 | rd.n());
}

// ---------------------------------------------------------------- loads and stores

/// (size bits, V, opc, scale) of a single-register access.
fn ls_class(base: &str, rt: Reg) -> Res<(u32, u32, u32, u32)> {
    let ld = base.starts_with("ldr") as u32;
    Ok(match base {
        "ldr" | "str" => match rt.k {
            K::X => (3, 0, ld, 3),
            K::W => (2, 0, ld, 2),
            K::B => (0, 1, ld, 0),
            K::H => (1, 1, ld, 1),
            K::S => (2, 1, ld, 2),
            K::D => (3, 1, ld, 3),
            K::Q => (0, 1, if ld == 1 { 3 } else { 2 }, 4),
            K::V => return bad(),
        },
        "ldrb" | "strb" if rt.k == K::W => (0, 0, ld, 0),
        "ldrh" | "strh" if rt.k == K::W => (1, 0, ld, 1),
        "ldrsb" if rt.k == K::X => (0, 0, 2, 0),
        "ldrsb" if rt.k == K::W => (0, 0, 3, 0),
        "ldrsh" if rt.k == K::X => (1, 0, 2, 1),
        "ldrsh" if rt.k == K::W => (1, 0, 3, 1),
        "ldrsw" if rt.k == K::X => (2, 0, 2, 2),
        _ => return bad(),
    })
}

fn mem_const(m: &Mem) -> Res<i64> {
    match &m.off {
        Off::None => Ok(0),
        Off::Imm(Value::Const(k)) => Ok(*k as i64),
        Off::Imm(_) => msg("a memory offset must be a constant (use :lo12: or @PAGEOFF for a symbol)"),
        _ => bad(),
    }
}

fn ldst(ctx: &mut Ctx, mn: &str, o: &[Op]) -> Res<()> {
    // kind 0: ldr / str, 1: the unscaled ldur / stur, 2: unprivileged ldtr / sttr, 3: ldapur / stlur
    let (kind, base) = if let Some(r) = mn.strip_prefix("ldur") {
        (1, format!("ldr{}", r))
    } else if let Some(r) = mn.strip_prefix("stur") {
        (1, format!("str{}", r))
    } else if let Some(r) = mn.strip_prefix("ldtr") {
        (2, format!("ldr{}", r))
    } else if let Some(r) = mn.strip_prefix("sttr") {
        (2, format!("str{}", r))
    } else if let Some(r) = mn.strip_prefix("ldapur") {
        (3, format!("ldr{}", r))
    } else if let Some(r) = mn.strip_prefix("stlur") {
        (3, format!("str{}", r))
    } else {
        (0, mn.to_string())
    };
    let unscaled = kind != 0;
    let rt = match o.first() {
        Some(Op::Reg(r, None, None)) if (r.is_gp() && !r.sp) || r.is_fp() => *r,
        _ => return bad(),
    };
    let (size, v, opc, scale) = ls_class(&base, rt)?;
    if kind >= 2 && v == 1 {
        return bad();
    }
    let head = match kind {
        2 => 0x3800_0800,
        3 => 0x1900_0000,
        _ => 0x3800_0000,
    } | size << 30
        | v << 26
        | opc << 22;
    let t = rt.n();
    match o.get(1) {
        Some(Op::Mem(m)) => {
            let post = o.get(2);
            let n = m.base.n() as u32;
            match &m.off {
                Off::Reg(rm, ext) => {
                    if post.is_some() || m.pre || unscaled {
                        return bad();
                    }
                    let (opt, amt) = match ext {
                        None => (3, None),
                        Some((e, a)) => (
                            match e.as_str() {
                                "lsl" => 3,
                                "uxtw" => 2,
                                "sxtw" => 6,
                                "sxtx" => 7,
                                _ => return bad(),
                            },
                            *a,
                        ),
                    };
                    if (opt & 1 == 1) != (rm.k == K::X) {
                        return bad();
                    }
                    let s = match amt {
                        None => 0,
                        Some(0) => (scale == 0) as u32,
                        Some(a) if a == scale as i64 => 1,
                        Some(_) => return msg(format!("the shift of a register offset must be 0 or {} here", scale)),
                    };
                    w(ctx, head | 0x20_0800 | rm.n() << 16 | opt << 13 | s << 12 | n << 5 | t);
                }
                Off::Sym(value, spec) => {
                    if *spec != Spec::PageOff || post.is_some() || m.pre || unscaled {
                        return bad();
                    }
                    let at = ctx.pos();
                    w(ctx, 0x0100_0000 | head | n << 5 | t);
                    ctx.fixup(at, FixKind::A64Lo12 { scale: scale as u8 }, value.clone());
                }
                _ => {
                    let k = mem_const(m)?;
                    if (kind >= 1 && (post.is_some() || m.pre)) || (m.pre && matches!(m.off, Off::None)) {
                        return bad();
                    }
                    if let Some(p) = post {
                        let pk = imm(ctx, Some(p))?;
                        if k != 0 || m.pre {
                            return bad();
                        }
                        let i = range(pk, -256, 255, "post-index offset")? & 0x1ff;
                        w(ctx, head | 0x400 | i << 12 | n << 5 | t);
                    } else if m.pre {
                        let i = range(k, -256, 255, "pre-index offset")? & 0x1ff;
                        w(ctx, head | 0xC00 | i << 12 | n << 5 | t);
                    } else if !unscaled && k >= 0 && k & ((1 << scale) - 1) == 0 && (k >> scale) < 4096 {
                        w(ctx, 0x0100_0000 | head | ((k >> scale) as u32) << 10 | n << 5 | t);
                    } else if (-256..=255).contains(&k) {
                        w(ctx, head | ((k as u32) & 0x1ff) << 12 | n << 5 | t);
                    } else {
                        return msg(format!("memory offset {} out of range for `{}`", k, mn));
                    }
                }
            }
            Ok(())
        }
        Some(Op::Expr { spec: Spec::None, .. }) if base == "ldr" || base == "ldrsw" => {
            if unscaled || o.len() != 2 {
                return bad();
            }
            let (opc, v) = match (base.as_str(), rt.k) {
                ("ldrsw", K::X) => (2, 0),
                ("ldr", K::W) | ("ldr", K::S) => (0, (rt.k == K::S) as u32),
                ("ldr", K::X) | ("ldr", K::D) => (1, (rt.k == K::D) as u32),
                ("ldr", K::Q) => (2, 1),
                _ => return bad(),
            };
            branch_target(ctx, 0x1800_0000 | opc << 30 | v << 26 | t, FixKind::A64Branch19, o.get(1), 19, 5)
        }
        _ => bad(),
    }
}

fn prfm_op(ctx: &Ctx, o: Option<&Op>) -> Res<u32> {
    if is_imm(o) {
        if let Ok(n) = name(o) {
            let (ty, rest) = n.split_at(n.len().min(3));
            let base = match ty {
                "pld" => 0,
                "pli" => 8,
                "pst" => 16,
                _ => return msg("unknown prefetch operation"),
            };
            let lvl = match rest {
                "l1keep" => 0,
                "l1strm" => 1,
                "l2keep" => 2,
                "l2strm" => 3,
                "l3keep" => 4,
                "l3strm" => 5,
                _ => return msg("unknown prefetch operation"),
            };
            return Ok(base + lvl);
        }
        return uimm(ctx, o, 31, "prefetch operation");
    }
    bad()
}

fn prfm(ctx: &mut Ctx, mn: &str, o: &[Op]) -> Res<()> {
    let t = prfm_op(ctx, o.first())?;
    match o.get(1) {
        Some(Op::Mem(m)) if !m.pre && o.len() == 2 => {
            let n = m.base.n() as u32;
            match &m.off {
                Off::Reg(rm, ext) => {
                    let (opt, s) = match ext {
                        None => (3, 0),
                        Some((e, a)) => (
                            match e.as_str() {
                                "lsl" => 3,
                                "uxtw" => 2,
                                "sxtw" => 6,
                                "sxtx" => 7,
                                _ => return bad(),
                            },
                            match a {
                                None | Some(0) => 0,
                                Some(3) => 1,
                                _ => return bad(),
                            },
                        ),
                    };
                    if (opt & 1 == 1) != (rm.k == K::X) {
                        return bad();
                    }
                    w(ctx, 0xF8A0_0800 | rm.n() << 16 | opt << 13 | s << 12 | n << 5 | t);
                }
                Off::Sym(value, Spec::PageOff) => {
                    let at = ctx.pos();
                    w(ctx, 0xF980_0000 | n << 5 | t);
                    ctx.fixup(at, FixKind::A64Lo12 { scale: 3 }, value.clone());
                }
                _ => {
                    let k = mem_const(m)?;
                    if mn == "prfm" && k >= 0 && k & 7 == 0 && (k >> 3) < 4096 {
                        w(ctx, 0xF980_0000 | ((k >> 3) as u32) << 10 | n << 5 | t);
                    } else if (-256..=255).contains(&k) {
                        w(ctx, 0xF880_0000 | ((k as u32) & 0x1ff) << 12 | n << 5 | t);
                    } else {
                        return msg("prefetch offset out of range");
                    }
                }
            }
            Ok(())
        }
        Some(Op::Expr { spec: Spec::None, .. }) if mn == "prfm" => branch_target(ctx, 0xD800_0000 | t, FixKind::A64Branch19, o.get(1), 19, 5),
        _ => bad(),
    }
}

fn ldstp(ctx: &mut Ctx, mn: &str, o: &[Op]) -> Res<()> {
    let load = mn.starts_with("ld") as u32;
    let nt = mn.ends_with("np");
    let rt = match o.first() {
        Some(Op::Reg(r, None, None)) if (r.is_gp() && !r.sp) || r.is_fp() => *r,
        _ => return bad(),
    };
    let rt2 = match o.get(1) {
        Some(Op::Reg(r, None, None)) if r.k == rt.k => *r,
        _ => return bad(),
    };
    let (opc, v, scale) = match (rt.k, mn) {
        (K::X, "ldpsw") => (1, 0, 2),
        (K::W, _) if mn != "ldpsw" => (0, 0, 2),
        (K::X, _) if mn != "ldpsw" => (2, 0, 3),
        (K::S, _) => (0, 1, 2),
        (K::D, _) => (1, 1, 3),
        (K::Q, _) => (2, 1, 4),
        _ => return bad(),
    };
    let Some(Op::Mem(m)) = o.get(2) else { return bad() };
    let post = o.get(3);
    let k = mem_const(m)?;
    if (m.pre && matches!(m.off, Off::None)) || (load == 1 && rt.n == rt2.n && !nt && (rt.is_fp() || (!m.pre && (post.is_none() || mn == "ldpsw")))) || ((m.pre || post.is_some()) && !rt.is_fp() && m.base.n != 31 && (m.base.n == rt.n || m.base.n == rt2.n)) {
        return bad();
    }
    let idx = if nt {
        if post.is_some() || m.pre {
            return bad();
        }
        0
    } else if post.is_some() {
        if k != 0 || m.pre {
            return bad();
        }
        1
    } else if m.pre {
        3
    } else {
        2
    };
    let k = if post.is_some() { imm(ctx, post)? } else { k };
    if k & ((1 << scale) - 1) != 0 {
        return msg(format!("offset must be a multiple of {}", 1 << scale));
    }
    let i = range(k >> scale, -64, 63, "pair offset")? & 0x7f;
    w(ctx, 0x2800_0000 | opc << 30 | v << 26 | idx << 23 | load << 22 | i << 15 | rt2.n() << 10 | (m.base.n() as u32) << 5 | rt.n());
    Ok(())
}

fn mem_base_only(ctx: &Ctx, o: Option<&Op>) -> Res<u32> {
    let _ = ctx;
    match o {
        Some(Op::Mem(m)) if !m.pre && matches!(m.off, Off::None | Off::Imm(Value::Const(0))) => Ok(m.base.n() as u32),
        Some(Op::Mem(_)) => msg("this instruction takes only a base register `[xN]`"),
        _ => bad(),
    }
}

fn excl(ctx: &mut Ctx, mn: &str, o: &[Op]) -> Res<bool> {
    if matches!(mn, "ldapr" | "ldaprb" | "ldaprh") {
        let rt = gp(o.first())?;
        let n = mem_base_only(ctx, o.get(1))?;
        let base = match (mn, rt.k) {
            ("ldaprb", K::W) => 0x38BF_C000,
            ("ldaprh", K::W) => 0x78BF_C000,
            ("ldapr", K::W) => 0xB8BF_C000,
            ("ldapr", K::X) => 0xF8BF_C000,
            _ => return bad(),
        };
        w(ctx, base | n << 5 | rt.n());
        return Ok(true);
    }
    // (L, o2, o1, o0, has status register)
    let table = |n: &str| -> Option<(u32, u32, u32, u32, bool)> {
        Some(match n {
            "stxr" => (0, 0, 0, 0, true),
            "stlxr" => (0, 0, 0, 1, true),
            "ldxr" => (1, 0, 0, 0, false),
            "ldaxr" => (1, 0, 0, 1, false),
            "stxp" => (0, 0, 1, 0, true),
            "stlxp" => (0, 0, 1, 1, true),
            "ldxp" => (1, 0, 1, 0, false),
            "ldaxp" => (1, 0, 1, 1, false),
            "ldar" => (1, 1, 0, 1, false),
            "stlr" => (0, 1, 0, 1, false),
            "ldlar" => (1, 1, 0, 0, false),
            "stllr" => (0, 1, 0, 0, false),
            _ => return None,
        })
    };
    let (core, fixed) = match table(mn) {
        Some(_) => (mn, None),
        None => match (mn.strip_suffix('b'), mn.strip_suffix('h')) {
            (Some(c), _) if table(c).is_some_and(|t| t.2 == 0) => (c, Some(0)),
            (_, Some(c)) if table(c).is_some_and(|t| t.2 == 0) => (c, Some(1)),
            _ => return Ok(false),
        },
    };
    let (l, o2, o1, o0, status) = table(core).unwrap();
    let mut i = 0;
    let rs = if status {
        i = 1;
        gp_k(o.first(), K::W)?.n()
    } else {
        31
    };
    let rt = gp(o.get(i))?;
    let rt2 = if o1 == 1 {
        i += 1;
        gp_k(o.get(i), rt.k)?.n()
    } else {
        31
    };
    let n = mem_base_only(ctx, o.get(i + 1))?;
    if o.len() != i + 2 {
        return bad();
    }
    let size = fixed.unwrap_or(if rt.k == K::X { 3 } else { 2 });
    if fixed.is_some() && rt.k != K::W {
        return bad();
    }
    w(ctx, 0x0800_0000 | size << 30 | o2 << 23 | l << 22 | o1 << 21 | rs << 16 | o0 << 15 | rt2 << 10 | n << 5 | rt.n());
    Ok(true)
}

fn lse(ctx: &mut Ctx, mn: &str, o: &[Op]) -> Res<bool> {
    if let Some(rest) = mn.strip_prefix("casp") {
        let (a, l) = match rest {
            "" => (0, 0),
            "a" => (1, 0),
            "l" => (0, 1),
            "al" => (1, 1),
            _ => return Ok(false),
        };
        let rs = gp(o.first())?;
        let rs2 = gp_k(o.get(1), rs.k)?;
        let rt = gp_k(o.get(2), rs.k)?;
        let rt2 = gp_k(o.get(3), rs.k)?;
        if o.len() != 5 || rs.n % 2 != 0 || rt.n % 2 != 0 || rs2.n != rs.n + 1 || rt2.n != rt.n + 1 {
            return msg("casp needs even-numbered consecutive register pairs");
        }
        let n = mem_base_only(ctx, o.get(4))?;
        w(ctx, 0x0820_7C00 | rs.sf() << 30 | a << 22 | l << 15 | rs.n() << 16 | n << 5 | rt.n());
        return Ok(true);
    }
    const FAMS: [(&str, u32); 18] = [
        ("cas", 0),
        ("swp", 0x3820_8000),
        ("ldadd", 0x3820_0000),
        ("ldclr", 0x3820_1000),
        ("ldeor", 0x3820_2000),
        ("ldset", 0x3820_3000),
        ("ldsmax", 0x3820_4000),
        ("ldsmin", 0x3820_5000),
        ("ldumax", 0x3820_6000),
        ("ldumin", 0x3820_7000),
        ("stadd", 0x3820_0000),
        ("stclr", 0x3820_1000),
        ("steor", 0x3820_2000),
        ("stset", 0x3820_3000),
        ("stsmax", 0x3820_4000),
        ("stsmin", 0x3820_5000),
        ("stumax", 0x3820_6000),
        ("stumin", 0x3820_7000),
    ];
    for (fam, base) in FAMS {
        let Some(rest) = mn.strip_prefix(fam) else { continue };
        let (a, r, rest) = if let Some(x) = rest.strip_prefix("al") {
            (1, 1, x)
        } else if let Some(x) = rest.strip_prefix('a') {
            (1, 0, x)
        } else if let Some(x) = rest.strip_prefix('l') {
            (0, 1, x)
        } else {
            (0, 0, rest)
        };
        let fixed = match rest {
            "" => None,
            "b" => Some(0),
            "h" => Some(1),
            _ => continue,
        };
        let store_form = fam.starts_with("st");
        if store_form && a == 1 {
            continue;
        }
        let rs = gp(o.first())?;
        let size = fixed.unwrap_or(if rs.k == K::X { 3 } else { 2 });
        if fixed.is_some() && rs.k != K::W {
            return bad();
        }
        let (rt, n) = if store_form {
            if o.len() != 2 {
                return bad();
            }
            (31, mem_base_only(ctx, o.get(1))?)
        } else {
            let rt = gp_k(o.get(1), rs.k)?;
            if o.len() != 3 {
                return bad();
            }
            (rt.n(), mem_base_only(ctx, o.get(2))?)
        };
        if fam == "cas" {
            w(ctx, 0x08A0_7C00 | size << 30 | a << 22 | r << 15 | rs.n() << 16 | n << 5 | rt);
        } else {
            w(ctx, base | size << 30 | a << 23 | r << 22 | rs.n() << 16 | n << 5 | rt);
        }
        return Ok(true);
    }
    Ok(false)
}

// ---------------------------------------------------------------- system

/// op0 (2 or 3), op1, CRn, CRm, op2 of a system register by name.
fn sysreg(n: &str) -> Option<(u32, u32, u32, u32, u32)> {
    if let Some(rest) = n.strip_prefix('s') {
        let p: Vec<&str> = rest.split('_').collect();
        if p.len() == 4 || p.len() == 5 {
            let num = |s: &str, c: Option<char>| -> Option<u32> {
                let s = match c {
                    Some(c) => s.strip_prefix(c)?,
                    None => s,
                };
                s.parse().ok()
            };
            if p.len() == 4 {
                // not the generic form (e.g. `spsel` is handled by the table)
            } else if let (Some(a), Some(b), Some(c), Some(d), Some(e)) = (num(p[0], None), num(p[1], None), num(p[2], Some('c')), num(p[3], Some('c')), num(p[4], None)) {
                if (2..=3).contains(&a) && b < 8 && c < 16 && d < 16 && e < 8 {
                    return Some((a, b, c, d, e));
                }
            }
        }
    }
    // pmevcntr<n>_el0 / pmevtyper<n>_el0
    for (pre, crn_base) in [("pmevcntr", 8u32), ("pmevtyper", 12)] {
        if let Some(rest) = n.strip_prefix(pre).and_then(|r| r.strip_suffix("_el0")) {
            if let Ok(k) = rest.parse::<u32>() {
                if k < 31 {
                    return Some((3, 3, 14, crn_base + (k >> 3), k & 7));
                }
            }
        }
    }
    SYSREGS.iter().find(|e| e.0 == n).map(|e| (e.1, e.2, e.3, e.4, e.5))
}

/// (name, op0, op1, CRn, CRm, op2)
pub(super) const SYSREGS: &[(&str, u32, u32, u32, u32, u32)] = &[
    ("nzcv", 3, 3, 4, 2, 0),
    ("daif", 3, 3, 4, 2, 1),
    ("fpcr", 3, 3, 4, 4, 0),
    ("fpsr", 3, 3, 4, 4, 1),
    ("dit", 3, 3, 4, 2, 5),
    ("ssbs", 3, 3, 4, 2, 6),
    ("tco", 3, 3, 4, 2, 7),
    ("pan", 3, 0, 4, 2, 3),
    ("uao", 3, 0, 4, 2, 4),
    ("tpidr_el0", 3, 3, 13, 0, 2),
    ("tpidrro_el0", 3, 3, 13, 0, 3),
    ("tpidr_el1", 3, 0, 13, 0, 4),
    ("tpidr_el2", 3, 4, 13, 0, 2),
    ("tpidr_el3", 3, 6, 13, 0, 2),
    ("ctr_el0", 3, 3, 0, 0, 1),
    ("dczid_el0", 3, 3, 0, 0, 7),
    ("cntvct_el0", 3, 3, 14, 0, 2),
    ("cntpct_el0", 3, 3, 14, 0, 1),
    ("cntfrq_el0", 3, 3, 14, 0, 0),
    ("cntv_ctl_el0", 3, 3, 14, 3, 1),
    ("cntv_tval_el0", 3, 3, 14, 3, 0),
    ("cntv_cval_el0", 3, 3, 14, 3, 2),
    ("cntp_ctl_el0", 3, 3, 14, 2, 1),
    ("cntp_tval_el0", 3, 3, 14, 2, 0),
    ("cntp_cval_el0", 3, 3, 14, 2, 2),
    ("cntkctl_el1", 3, 0, 14, 1, 0),
    ("cnthctl_el2", 3, 4, 14, 1, 0),
    ("cntvoff_el2", 3, 4, 14, 0, 3),
    ("midr_el1", 3, 0, 0, 0, 0),
    ("mpidr_el1", 3, 0, 0, 0, 5),
    ("revidr_el1", 3, 0, 0, 0, 6),
    ("vpidr_el2", 3, 4, 0, 0, 0),
    ("vmpidr_el2", 3, 4, 0, 0, 5),
    ("id_aa64pfr0_el1", 3, 0, 0, 4, 0),
    ("id_aa64pfr1_el1", 3, 0, 0, 4, 1),
    ("id_aa64zfr0_el1", 3, 0, 0, 4, 4),
    ("id_aa64dfr0_el1", 3, 0, 0, 5, 0),
    ("id_aa64dfr1_el1", 3, 0, 0, 5, 1),
    ("id_aa64afr0_el1", 3, 0, 0, 5, 4),
    ("id_aa64afr1_el1", 3, 0, 0, 5, 5),
    ("id_aa64isar0_el1", 3, 0, 0, 6, 0),
    ("id_aa64isar1_el1", 3, 0, 0, 6, 1),
    ("id_aa64isar2_el1", 3, 0, 0, 6, 2),
    ("id_aa64mmfr0_el1", 3, 0, 0, 7, 0),
    ("id_aa64mmfr1_el1", 3, 0, 0, 7, 1),
    ("id_aa64mmfr2_el1", 3, 0, 0, 7, 2),
    ("ccsidr_el1", 3, 1, 0, 0, 0),
    ("clidr_el1", 3, 1, 0, 0, 1),
    ("aidr_el1", 3, 1, 0, 0, 7),
    ("csselr_el1", 3, 2, 0, 0, 0),
    ("currentel", 3, 0, 4, 2, 2),
    ("spsel", 3, 0, 4, 2, 0),
    ("sp_el0", 3, 0, 4, 1, 0),
    ("sp_el1", 3, 4, 4, 1, 0),
    ("sp_el2", 3, 6, 4, 1, 0),
    ("sctlr_el1", 3, 0, 1, 0, 0),
    ("actlr_el1", 3, 0, 1, 0, 1),
    ("cpacr_el1", 3, 0, 1, 0, 2),
    ("ttbr0_el1", 3, 0, 2, 0, 0),
    ("ttbr1_el1", 3, 0, 2, 0, 1),
    ("tcr_el1", 3, 0, 2, 0, 2),
    ("spsr_el1", 3, 0, 4, 0, 0),
    ("elr_el1", 3, 0, 4, 0, 1),
    ("esr_el1", 3, 0, 5, 2, 0),
    ("far_el1", 3, 0, 6, 0, 0),
    ("par_el1", 3, 0, 7, 4, 0),
    ("mair_el1", 3, 0, 10, 2, 0),
    ("amair_el1", 3, 0, 10, 3, 0),
    ("vbar_el1", 3, 0, 12, 0, 0),
    ("contextidr_el1", 3, 0, 13, 0, 1),
    ("sctlr_el2", 3, 4, 1, 0, 0),
    ("hcr_el2", 3, 4, 1, 1, 0),
    ("cptr_el2", 3, 4, 1, 1, 2),
    ("ttbr0_el2", 3, 4, 2, 0, 0),
    ("tcr_el2", 3, 4, 2, 0, 2),
    ("vttbr_el2", 3, 4, 2, 1, 0),
    ("vtcr_el2", 3, 4, 2, 1, 2),
    ("spsr_el2", 3, 4, 4, 0, 0),
    ("elr_el2", 3, 4, 4, 0, 1),
    ("esr_el2", 3, 4, 5, 2, 0),
    ("far_el2", 3, 4, 6, 0, 0),
    ("hpfar_el2", 3, 4, 6, 0, 4),
    ("mair_el2", 3, 4, 10, 2, 0),
    ("vbar_el2", 3, 4, 12, 0, 0),
    ("sctlr_el3", 3, 6, 1, 0, 0),
    ("scr_el3", 3, 6, 1, 1, 0),
    ("ttbr0_el3", 3, 6, 2, 0, 0),
    ("tcr_el3", 3, 6, 2, 0, 2),
    ("spsr_el3", 3, 6, 4, 0, 0),
    ("elr_el3", 3, 6, 4, 0, 1),
    ("esr_el3", 3, 6, 5, 2, 0),
    ("far_el3", 3, 6, 6, 0, 0),
    ("mair_el3", 3, 6, 10, 2, 0),
    ("vbar_el3", 3, 6, 12, 0, 0),
    ("pmcr_el0", 3, 3, 9, 12, 0),
    ("pmcntenset_el0", 3, 3, 9, 12, 1),
    ("pmcntenclr_el0", 3, 3, 9, 12, 2),
    ("pmselr_el0", 3, 3, 9, 12, 5),
    ("pmccntr_el0", 3, 3, 9, 13, 0),
    ("pmxevtyper_el0", 3, 3, 9, 13, 1),
    ("pmxevcntr_el0", 3, 3, 9, 13, 2),
    ("pmuserenr_el0", 3, 3, 9, 14, 0),
    ("pmccfiltr_el0", 3, 3, 14, 15, 7),
    ("mdscr_el1", 2, 0, 0, 2, 2),
    ("oslar_el1", 2, 0, 1, 0, 4),
    ("rndr", 3, 3, 2, 4, 0),
    ("rndrrs", 3, 3, 2, 4, 1),
    ("zcr_el1", 3, 0, 1, 2, 0),
];

fn barrier_opt(ctx: &Ctx, o: Option<&Op>) -> Res<u32> {
    if let Ok(n) = name(o) {
        return Ok(match n.as_str() {
            "oshld" => 1,
            "oshst" => 2,
            "osh" => 3,
            "nshld" => 5,
            "nshst" => 6,
            "nsh" => 7,
            "ishld" => 9,
            "ishst" => 10,
            "ish" => 11,
            "ld" => 13,
            "st" => 14,
            "sy" => 15,
            _ => return msg(format!("unknown barrier option `{}`", n)),
        });
    }
    uimm(ctx, o, 15, "barrier option")
}

fn sys_insn(ctx: &mut Ctx, mn: &str, o: &[Op]) -> Res<bool> {
    let hint = |ctx: &mut Ctx, n: u32| w(ctx, 0xD503_201F | n << 5);
    match mn {
        "nop" => hint(ctx, 0),
        "yield" => hint(ctx, 1),
        "wfe" => hint(ctx, 2),
        "wfi" => hint(ctx, 3),
        "sev" => hint(ctx, 4),
        "sevl" => hint(ctx, 5),
        "csdb" => hint(ctx, 20),
        "dgh" => hint(ctx, 6),
        "xpaclri" => hint(ctx, 7),
        "pacia1716" => hint(ctx, 8),
        "pacib1716" => hint(ctx, 10),
        "autia1716" => hint(ctx, 12),
        "autib1716" => hint(ctx, 14),
        "esb" => hint(ctx, 16),
        "paciaz" => hint(ctx, 24),
        "paciasp" => hint(ctx, 25),
        "pacibz" => hint(ctx, 26),
        "pacibsp" => hint(ctx, 27),
        "autiaz" => hint(ctx, 28),
        "autiasp" => hint(ctx, 29),
        "autibz" => hint(ctx, 30),
        "autibsp" => hint(ctx, 31),
        "psb" | "tsb" => {
            if name(o.first())? != "csync" {
                return bad();
            }
            hint(ctx, if mn == "psb" { 17 } else { 18 })
        }
        "sb" => w(ctx, 0xD503_30FF),
        "ssbb" => w(ctx, 0xD503_309F),
        "pssbb" => w(ctx, 0xD503_349F),
        "retaa" => w(ctx, 0xD65F_0BFF),
        "retab" => w(ctx, 0xD65F_0FFF),
        "eretaa" => w(ctx, 0xD69F_0BFF),
        "eretab" => w(ctx, 0xD69F_0FFF),
        "hint" => {
            let n = uimm(ctx, o.first(), 127, "hint number")?;
            hint(ctx, n)
        }
        "bti" => {
            let n = match o.first() {
                None => 0,
                Some(_) => match name(o.first())?.as_str() {
                    "c" => 1,
                    "j" => 2,
                    "jc" => 3,
                    _ => return bad(),
                },
            };
            hint(ctx, 32 + n * 2)
        }
        "dmb" | "dsb" | "isb" => {
            let crm = if mn == "isb" && (o.is_empty() || name(o.first()).is_ok_and(|n| n == "sy")) {
                15
            } else if mn == "isb" && name(o.first()).is_ok() {
                return bad();
            } else {
                barrier_opt(ctx, o.first())?
            };
            w(ctx, (match mn {
                "dsb" => 0xD503_309F,
                "dmb" => 0xD503_30BF,
                _ => 0xD503_30DF,
            }) | crm << 8);
        }
        "clrex" => {
            let crm = if o.is_empty() { 15 } else { uimm(ctx, o.first(), 15, "clrex option")? };
            w(ctx, 0xD503_305F | crm << 8);
        }
        "svc" | "hvc" | "smc" | "brk" | "hlt" | "udf" => {
            let i = uimm(ctx, o.first(), 0xffff, "16-bit immediate")?;
            w(ctx, match mn {
                "svc" => 0xD400_0001,
                "hvc" => 0xD400_0002,
                "smc" => 0xD400_0003,
                "brk" => 0xD420_0000,
                "hlt" => 0xD440_0000,
                _ => 0,
            } | i << (if mn == "udf" { 0 } else { 5 }));
        }
        "mrs" | "msr" => {
            if mn == "msr" && is_imm(o.get(1)) {
                // msr <pstatefield>, #imm
                let (op1, op2) = match name(o.first())?.as_str() {
                    "spsel" => (0, 5),
                    "daifset" => (3, 6),
                    "daifclr" => (3, 7),
                    "uao" => (0, 3),
                    "pan" => (0, 4),
                    "dit" => (3, 2),
                    "ssbs" => (3, 1),
                    "tco" => (3, 4),
                    _ => return bad(),
                };
                let crm = uimm(ctx, o.get(1), 15, "pstate immediate")?;
                w(ctx, 0xD500_401F | op1 << 16 | crm << 8 | op2 << 5);
                return Ok(true);
            }
            let (reg, sysop, l) = if mn == "mrs" { (gp_k(o.first(), K::X)?, o.get(1), 1) } else { (gp_k(o.get(1), K::X)?, o.first(), 0) };
            let n = name(sysop)?;
            let (op0, op1, crn, crm, op2) = sysreg(&n).ok_or_else(|| format!("unknown system register `{}`", n))?;
            let read_only = n.starts_with("id_") || matches!(n.as_str(), "ctr_el0" | "dczid_el0" | "cntvct_el0" | "cntpct_el0" | "midr_el1" | "mpidr_el1" | "revidr_el1" | "ccsidr_el1" | "clidr_el1" | "aidr_el1" | "currentel" | "rndr" | "rndrrs");
            if (l == 0 && read_only) || (l == 1 && n == "oslar_el1") {
                return msg(format!("system register `{}` is {}", n, if l == 0 { "read-only" } else { "write-only" }));
            }
            w(ctx, 0xD510_0000 | l << 21 | (op0 & 1) << 19 | op1 << 16 | crn << 12 | crm << 8 | op2 << 5 | reg.n());
        }
        "sys" | "sysl" => {
            // sys #op1, Cn, Cm, #op2 {, Xt}   /   sysl Xt, #op1, Cn, Cm, #op2
            let (rt, l, at) = if mn == "sys" { (if o.len() > 4 { gp_k(o.get(4), K::X)?.n() } else { 31 }, 0, 0) } else { (gp_k(o.first(), K::X)?.n(), 1, 1) };
            let op1 = uimm(ctx, o.get(at), 7, "op1")?;
            let cn = cn_of(o.get(at + 1))?;
            let cm = cn_of(o.get(at + 2))?;
            let op2 = uimm(ctx, o.get(at + 3), 7, "op2")?;
            w(ctx, 0xD508_0000 | l << 21 | op1 << 16 | cn << 12 | cm << 8 | op2 << 5 | rt);
        }
        "dc" | "ic" | "tlbi" | "at" => {
            let n = name(o.first())?;
            // (op1, CRn, CRm, op2, takes a register)
            let (op1, crn, crm, op2, reg) = match (mn, n.as_str()) {
                ("dc", "zva") => (3, 7, 4, 1, true),
                ("dc", "ivac") => (0, 7, 6, 1, true),
                ("dc", "isw") => (0, 7, 6, 2, true),
                ("dc", "cvac") => (3, 7, 10, 1, true),
                ("dc", "csw") => (0, 7, 10, 2, true),
                ("dc", "cvau") => (3, 7, 11, 1, true),
                ("dc", "cvap") => (3, 7, 12, 1, true),
                ("dc", "cvadp") => (3, 7, 13, 1, true),
                ("dc", "civac") => (3, 7, 14, 1, true),
                ("dc", "cisw") => (0, 7, 14, 2, true),
                ("ic", "ialluis") => (0, 7, 1, 0, false),
                ("ic", "iallu") => (0, 7, 5, 0, false),
                ("ic", "ivau") => (3, 7, 5, 1, true),
                ("at", "s1e1r") => (0, 7, 8, 0, true),
                ("at", "s1e1w") => (0, 7, 8, 1, true),
                ("at", "s1e0r") => (0, 7, 8, 2, true),
                ("at", "s1e0w") => (0, 7, 8, 3, true),
                ("tlbi", "vmalle1is") => (0, 8, 3, 0, false),
                ("tlbi", "vae1is") => (0, 8, 3, 1, true),
                ("tlbi", "aside1is") => (0, 8, 3, 2, true),
                ("tlbi", "vaae1is") => (0, 8, 3, 3, true),
                ("tlbi", "vale1is") => (0, 8, 3, 5, true),
                ("tlbi", "vaale1is") => (0, 8, 3, 7, true),
                ("tlbi", "vmalle1") => (0, 8, 7, 0, false),
                ("tlbi", "vae1") => (0, 8, 7, 1, true),
                ("tlbi", "aside1") => (0, 8, 7, 2, true),
                ("tlbi", "vaae1") => (0, 8, 7, 3, true),
                ("tlbi", "vale1") => (0, 8, 7, 5, true),
                ("tlbi", "vaale1") => (0, 8, 7, 7, true),
                ("tlbi", "alle1is") => (4, 8, 3, 4, false),
                ("tlbi", "alle1") => (4, 8, 7, 4, false),
                _ => return msg(format!("unknown `{}` operation `{}`", mn, n)),
            };
            if o.len() != 1 + reg as usize {
                return bad();
            }
            let rt = if reg { gp_k(o.get(1), K::X)?.n() } else { 31 };
            w(ctx, 0xD508_0000 | op1 << 16 | crn << 12 | crm << 8 | op2 << 5 | rt);
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn cn_of(o: Option<&Op>) -> Res<u32> {
    match name(o) {
        Ok(n) => Ok(n.strip_prefix('c').and_then(|x| x.parse::<u32>().ok()).filter(|x| *x < 16).ok_or("expected c0..c15")?),
        Err(_) => bad(),
    }
}

// ---------------------------------------------------------------- floating point

fn ftype(r: Reg) -> Res<u32> {
    match r.k {
        K::S => Ok(0),
        K::D => Ok(1),
        K::H => Ok(3),
        _ => bad(),
    }
}
fn fp_same(o: Option<&Op>, k: K) -> Res<Reg> {
    let r = fpr(o)?;
    if r.k == k {
        Ok(r)
    } else {
        bad()
    }
}

fn float_lit(t: &[Tok]) -> Option<f64> {
    let mut s = String::new();
    for x in t {
        match x {
            Tok::Int(n) => s.push_str(&n.to_string()),
            Tok::Ident(i) if i.starts_with('.') => s.push_str(i),
            Tok::Punct(c @ (b'-' | b'+')) => s.push(*c as char),
            _ => return None,
        }
    }
    s.parse::<f64>().ok()
}

/// The 8-bit floating-point immediate of `v` (VFPExpandImm).
fn fp_imm8(v: f64) -> Option<u32> {
    (0..256u32).find(|i| {
        let sign = ((i >> 7) & 1) as u64;
        let b = ((i >> 6) & 1) as u64;
        let cd = ((i >> 4) & 3) as u64;
        let frac = (i & 15) as u64;
        let exp = ((1 - b) << 10) | (if b == 1 { 0xff << 2 } else { 0 }) | cd;
        (sign << 63 | exp << 52 | frac << 48) == v.to_bits()
    })
}

fn fp_lit(ctx: &Ctx, o: Option<&Op>) -> Res<f64> {
    let _ = ctx;
    match o {
        Some(Op::Expr { toks, spec: Spec::None }) => float_lit(toks).ok_or_else(|| "expected a floating-point constant".to_string()),
        _ => bad(),
    }
}

fn fmov(ctx: &mut Ctx, o: &[Op]) -> Res<()> {
    if o.len() != 2 {
        return bad();
    }
    match (&o[0], &o[1]) {
        (Op::Reg(rd, None, None), Op::Reg(rn, None, None)) if rd.is_fp() && rn.is_fp() => {
            if rd.k != rn.k || rd.k == K::B || rd.k == K::Q {
                return bad();
            }
            w(ctx, 0x1E20_4000 | ftype(*rd)? << 22 | rn.n() << 5 | rd.n());
        }
        (Op::Reg(rd, None, None), Op::Reg(rn, None, None)) if rd.is_gp() && rn.is_fp() => {
            let base = match (rd.k, rn.k) {
                (K::W, K::S) => 0x1E26_0000,
                (K::X, K::D) => 0x9E66_0000,
                (K::W, K::H) => 0x1EE6_0000,
                (K::X, K::H) => 0x9EE6_0000,
                _ => return bad(),
            };
            w(ctx, base | rn.n() << 5 | rd.n());
        }
        (Op::Reg(rd, None, None), Op::Reg(rn, None, None)) if rd.is_fp() && rn.is_gp() => {
            let base = match (rn.k, rd.k) {
                (K::W, K::S) => 0x1E27_0000,
                (K::X, K::D) => 0x9E67_0000,
                (K::W, K::H) => 0x1EE7_0000,
                (K::X, K::H) => 0x9EE7_0000,
                _ => return bad(),
            };
            w(ctx, base | rn.n() << 5 | rd.n());
        }
        (Op::Reg(rd, None, None), Op::Reg(rn, Some(Arr { n: 0, es: 3 }), Some(1))) if rd.k == K::X && rn.k == K::V => w(ctx, 0x9EAE_0000 | rn.n() << 5 | rd.n()),
        (Op::Reg(rd, Some(Arr { n: 0, es: 3 }), Some(1)), Op::Reg(rn, None, None)) if rd.k == K::V && rn.k == K::X => w(ctx, 0x9EAF_0000 | rn.n() << 5 | rd.n()),
        (Op::Reg(rd, None, None), Op::Expr { .. }) if rd.is_fp() => {
            let v = fp_lit(ctx, o.get(1))?;
            if v == 0.0 && v.is_sign_positive() {
                // fmov Sd, #0.0 is fmov Sd, wzr (LLVM's alias)
                let base = match ftype(*rd)? {
                    0 => 0x1E27_0000,
                    1 => 0x9E67_0000,
                    _ => 0x1EE7_0000,
                };
                w(ctx, base | 31 << 5 | rd.n());
                return Ok(());
            }
            let i = fp_imm8(v).ok_or("floating-point constant is not encodable as an 8-bit immediate")?;
            w(ctx, 0x1E20_1000 | ftype(*rd)? << 22 | i << 13 | rd.n());
        }
        (Op::Reg(rd, Some(a), None), Op::Expr { .. }) if rd.k == K::V && a.n != 0 => {
            let v = fp_lit(ctx, o.get(1))?;
            let i = fp_imm8(v).ok_or("floating-point constant is not encodable as an 8-bit immediate")?;
            let base = match (a.n, a.es) {
                (2, 2) | (4, 2) => 0x0F00_F400 | a.q() << 30,
                (2, 3) => 0x6F00_F400,
                (4, 1) | (8, 1) => 0x0F00_FC00 | a.q() << 30,
                _ => return bad(),
            };
            w(ctx, base | ((i >> 5) & 7) << 16 | (i & 31) << 5 | rd.n());
        }
        _ => return bad(),
    }
    Ok(())
}

/// scalar 2-source arithmetic: opcode field
fn fp2_opcode(mn: &str) -> Option<u32> {
    Some(match mn {
        "fmul" => 0,
        "fdiv" => 1,
        "fadd" => 2,
        "fsub" => 3,
        "fmax" => 4,
        "fmin" => 5,
        "fmaxnm" => 6,
        "fminnm" => 7,
        "fnmul" => 8,
        _ => return None,
    })
}
fn fp1_opcode(mn: &str) -> Option<u32> {
    Some(match mn {
        "fabs" => 1,
        "fneg" => 2,
        "fsqrt" => 3,
        "frintn" => 8,
        "frintp" => 9,
        "frintm" => 10,
        "frintz" => 11,
        "frinta" => 12,
        "frintx" => 14,
        "frinti" => 15,
        _ => return None,
    })
}
/// (rmode, opcode) of the float <-> integer conversions
fn fcvt_int(mn: &str) -> Option<(u32, u32)> {
    Some(match mn {
        "fcvtns" => (0, 0),
        "fcvtnu" => (0, 1),
        "scvtf" => (0, 2),
        "ucvtf" => (0, 3),
        "fcvtas" => (0, 4),
        "fcvtau" => (0, 5),
        "fcvtps" => (1, 0),
        "fcvtpu" => (1, 1),
        "fcvtms" => (2, 0),
        "fcvtmu" => (2, 1),
        "fcvtzs" => (3, 0),
        "fcvtzu" => (3, 1),
        _ => return None,
    })
}

fn is_vec_op(o: Option<&Op>) -> bool {
    matches!(o, Some(Op::Reg(r, _, _)) if r.k == K::V)
}

fn fp_insn(ctx: &mut Ctx, mn: &str, o: &[Op]) -> Res<bool> {
    // scalar forms only (the vector forms are in neon_insn)
    if is_vec_op(o.first()) && mn != "fmov" {
        return Ok(false);
    }
    if let Some(op) = fp2_opcode(mn) {
        let rd = fpr(o.first())?;
        let (rn, rm) = (fp_same(o.get(1), rd.k)?, fp_same(o.get(2), rd.k)?);
        w(ctx, 0x1E20_0800 | ftype(rd)? << 22 | rm.n() << 16 | op << 12 | rn.n() << 5 | rd.n());
        return Ok(true);
    }
    if let Some(op) = fp1_opcode(mn) {
        let rd = fpr(o.first())?;
        let rn = fp_same(o.get(1), rd.k)?;
        w(ctx, 0x1E20_4000 | ftype(rd)? << 22 | op << 15 | rn.n() << 5 | rd.n());
        return Ok(true);
    }
    if let Some((rmode, op)) = fcvt_int(mn) {
        if matches!(o.first(), Some(Op::Reg(r, None, None)) if r.is_fp()) && matches!(o.get(1), Some(Op::Reg(r, None, None)) if r.is_fp()) {
            return Ok(false);
        }
        let to_float = matches!(mn, "scvtf" | "ucvtf");
        let (f, g) = if to_float { (fpr(o.first())?, gp(o.get(1))?) } else { (fpr(o.get(1))?, gp(o.first())?) };
        let ty = ftype(f)?;
        let head = 0x1E00_0000 | g.sf() << 31 | ty << 22 | rmode << 19 | op << 16 | if to_float { g.n() << 5 | f.n() } else { f.n() << 5 | g.n() };
        match o.get(2) {
            None => w(ctx, head | 1 << 21),
            Some(_) => {
                if !(rmode == 0 && op >= 2 && op <= 3 || rmode == 3) {
                    return bad();
                }
                let fb = imm(ctx, o.get(2))?;
                let fb = range(fb, 1, g.bits() as i64, "number of fractional bits")?;
                w(ctx, head | (64 - fb) << 10);
            }
        }
        return Ok(true);
    }
    match mn {
        "fmov" => fmov(ctx, o)?,
        "fcvt" => {
            let (rd, rn) = (fpr(o.first())?, fpr(o.get(1))?);
            let op = match rd.k {
                K::S => 4,
                K::D => 5,
                K::H => 7,
                _ => return bad(),
            };
            if rd.k == rn.k {
                return bad();
            }
            w(ctx, 0x1E20_4000 | ftype(rn)? << 22 | op << 15 | rn.n() << 5 | rd.n());
        }
        "fmadd" | "fmsub" | "fnmadd" | "fnmsub" => {
            let rd = fpr(o.first())?;
            let (rn, rm, ra) = (fp_same(o.get(1), rd.k)?, fp_same(o.get(2), rd.k)?, fp_same(o.get(3), rd.k)?);
            let (o1, o0) = match mn {
                "fmadd" => (0, 0),
                "fmsub" => (0, 1),
                "fnmadd" => (1, 0),
                _ => (1, 1),
            };
            w(ctx, 0x1F00_0000 | ftype(rd)? << 22 | o1 << 21 | rm.n() << 16 | o0 << 15 | ra.n() << 10 | rn.n() << 5 | rd.n());
        }
        "fcmp" | "fcmpe" => {
            let rn = fpr(o.first())?;
            let e = (mn == "fcmpe") as u32;
            let base = 0x1E20_2000 | ftype(rn)? << 22 | rn.n() << 5 | e << 4;
            if is_imm(o.get(1)) {
                if fp_lit(ctx, o.get(1))? != 0.0 {
                    return msg("fcmp can only compare with #0.0");
                }
                w(ctx, base | 8);
            } else {
                let rm = fp_same(o.get(1), rn.k)?;
                w(ctx, base | rm.n() << 16);
            }
        }
        "fccmp" | "fccmpe" => {
            let rn = fpr(o.first())?;
            let rm = fp_same(o.get(1), rn.k)?;
            let nzcv = uimm(ctx, o.get(2), 15, "nzcv")?;
            let c = cond(o.get(3))?;
            w(ctx, 0x1E20_0400 | ftype(rn)? << 22 | rm.n() << 16 | c << 12 | rn.n() << 5 | ((mn == "fccmpe") as u32) << 4 | nzcv);
        }
        "fcsel" => {
            let rd = fpr(o.first())?;
            let (rn, rm) = (fp_same(o.get(1), rd.k)?, fp_same(o.get(2), rd.k)?);
            let c = cond(o.get(3))?;
            w(ctx, 0x1E20_0C00 | ftype(rd)? << 22 | rm.n() << 16 | c << 12 | rn.n() << 5 | rd.n());
        }
        _ => return Ok(false),
    }
    Ok(true)
}

// ---------------------------------------------------------------- NEON

#[derive(Clone, Copy, PartialEq)]
enum M {
    /// b h s d (not 1d)
    All,
    /// b h s
    Nod,
    /// h s
    Hs,
    /// 8b / 16b; the size bits come from the base
    Bytes,
    /// b h
    Bh,
}

fn arr_ok(m: M, a: Arr) -> bool {
    if a.n == 0 || a.es > 3 {
        return false;
    }
    match m {
        M::All => !(a.es == 3 && a.n == 1),
        M::Nod => a.es <= 2,
        M::Hs => a.es == 1 || a.es == 2,
        M::Bytes => a.es == 0,
        M::Bh => a.es <= 1,
    }
}

const THREE: &[(&str, u32, M)] = &[
    ("add", 0x0E20_8400, M::All),
    ("sub", 0x2E20_8400, M::All),
    ("mul", 0x0E20_9C00, M::Nod),
    ("mla", 0x0E20_9400, M::Nod),
    ("mls", 0x2E20_9400, M::Nod),
    ("pmul", 0x2E20_9C00, M::Bytes),
    ("and", 0x0E20_1C00, M::Bytes),
    ("orr", 0x0EA0_1C00, M::Bytes),
    ("eor", 0x2E20_1C00, M::Bytes),
    ("bic", 0x0E60_1C00, M::Bytes),
    ("orn", 0x0EE0_1C00, M::Bytes),
    ("bsl", 0x2E60_1C00, M::Bytes),
    ("bit", 0x2EA0_1C00, M::Bytes),
    ("bif", 0x2EE0_1C00, M::Bytes),
    ("cmeq", 0x2E20_8C00, M::All),
    ("cmtst", 0x0E20_8C00, M::All),
    ("cmgt", 0x0E20_3400, M::All),
    ("cmge", 0x0E20_3C00, M::All),
    ("cmhi", 0x2E20_3400, M::All),
    ("cmhs", 0x2E20_3C00, M::All),
    ("smax", 0x0E20_6400, M::Nod),
    ("smin", 0x0E20_6C00, M::Nod),
    ("umax", 0x2E20_6400, M::Nod),
    ("umin", 0x2E20_6C00, M::Nod),
    ("sabd", 0x0E20_7400, M::Nod),
    ("uabd", 0x2E20_7400, M::Nod),
    ("saba", 0x0E20_7C00, M::Nod),
    ("uaba", 0x2E20_7C00, M::Nod),
    ("shadd", 0x0E20_0400, M::Nod),
    ("uhadd", 0x2E20_0400, M::Nod),
    ("srhadd", 0x0E20_1400, M::Nod),
    ("urhadd", 0x2E20_1400, M::Nod),
    ("shsub", 0x0E20_2400, M::Nod),
    ("uhsub", 0x2E20_2400, M::Nod),
    ("sqadd", 0x0E20_0C00, M::All),
    ("uqadd", 0x2E20_0C00, M::All),
    ("sqsub", 0x0E20_2C00, M::All),
    ("uqsub", 0x2E20_2C00, M::All),
    ("sshl", 0x0E20_4400, M::All),
    ("ushl", 0x2E20_4400, M::All),
    ("srshl", 0x0E20_5400, M::All),
    ("urshl", 0x2E20_5400, M::All),
    ("addp", 0x0E20_BC00, M::All),
    ("smaxp", 0x0E20_A400, M::Nod),
    ("sminp", 0x0E20_AC00, M::Nod),
    ("umaxp", 0x2E20_A400, M::Nod),
    ("uminp", 0x2E20_AC00, M::Nod),
    ("sqdmulh", 0x0E20_B400, M::Hs),
    ("sqrdmulh", 0x2E20_B400, M::Hs),
];

/// float 3-same: (name, base for 2s/4s/2d with sz = 0, base for 4h/8h)
const THREE_F: &[(&str, u32, u32)] = &[
    ("fadd", 0x0E20_D400, 0x0E40_1400),
    ("fsub", 0x0EA0_D400, 0x0EC0_1400),
    ("fmul", 0x2E20_DC00, 0x2E40_1C00),
    ("fdiv", 0x2E20_FC00, 0x2E40_3C00),
    ("fmax", 0x0E20_F400, 0x0E40_3400),
    ("fmin", 0x0EA0_F400, 0x0EC0_3400),
    ("fmaxnm", 0x0E20_C400, 0x0E40_0400),
    ("fminnm", 0x0EA0_C400, 0x0EC0_0400),
    ("fmla", 0x0E20_CC00, 0x0E40_0C00),
    ("fmls", 0x0EA0_CC00, 0x0EC0_0C00),
    ("faddp", 0x2E20_D400, 0x2E40_1400),
    ("fmaxp", 0x2E20_F400, 0x2E40_3400),
    ("fminp", 0x2EA0_F400, 0x2EC0_3400),
    ("fmaxnmp", 0x2E20_C400, 0x2E40_0400),
    ("fminnmp", 0x2EA0_C400, 0x2EC0_0400),
    ("fabd", 0x2EA0_D400, 0x2EC0_1400),
    ("fmulx", 0x0E20_DC00, 0x0E40_1C00),
    ("fcmeq", 0x0E20_E400, 0x0E40_2400),
    ("fcmge", 0x2E20_E400, 0x2E40_2400),
    ("fcmgt", 0x2EA0_E400, 0x2EC0_2400),
    ("facge", 0x2E20_EC00, 0x2E40_2C00),
    ("facgt", 0x2EA0_EC00, 0x2EC0_2C00),
    ("frecps", 0x0E20_FC00, 0x0E40_3C00),
    ("frsqrts", 0x0EA0_FC00, 0x0EC0_3C00),
];

/// float 2-reg misc, sz = 0 bases
const TWO_F: &[(&str, u32)] = &[
    ("fabs", 0x0EA0_F800),
    ("fneg", 0x2EA0_F800),
    ("fsqrt", 0x2EA1_F800),
    ("frintn", 0x0E21_8800),
    ("frintm", 0x0E21_9800),
    ("frintp", 0x0EA1_8800),
    ("frintz", 0x0EA1_9800),
    ("frinta", 0x2E21_8800),
    ("frintx", 0x2E21_9800),
    ("frinti", 0x2EA1_9800),
    ("scvtf", 0x0E21_D800),
    ("ucvtf", 0x2E21_D800),
    ("fcvtzs", 0x0EA1_B800),
    ("fcvtzu", 0x2EA1_B800),
    ("fcvtns", 0x0E21_A800),
    ("fcvtnu", 0x2E21_A800),
    ("fcvtms", 0x0E21_B800),
    ("fcvtmu", 0x2E21_B800),
    ("fcvtas", 0x0E21_C800),
    ("fcvtau", 0x2E21_C800),
    ("fcvtps", 0x0EA1_A800),
    ("fcvtpu", 0x2EA1_A800),
    ("frecpe", 0x0EA1_D800),
    ("frsqrte", 0x2EA1_D800),
];

const TWO: &[(&str, u32, M)] = &[
    ("abs", 0x0E20_B800, M::All),
    ("neg", 0x2E20_B800, M::All),
    ("sqabs", 0x0E20_7800, M::All),
    ("sqneg", 0x2E20_7800, M::All),
    ("cls", 0x0E20_4800, M::Nod),
    ("clz", 0x2E20_4800, M::Nod),
    ("cnt", 0x0E20_5800, M::Bytes),
    ("not", 0x2E20_5800, M::Bytes),
    ("mvn", 0x2E20_5800, M::Bytes),
    ("rbit", 0x2E60_5800, M::Bytes),
    ("rev64", 0x0E20_0800, M::Nod),
    ("rev32", 0x2E20_0800, M::Bh),
    ("rev16", 0x0E20_1800, M::Bytes),
];

const ZERO_CMP: &[(&str, u32)] = &[("cmeq", 0x0E20_9800), ("cmge", 0x2E20_8800), ("cmgt", 0x0E20_8800), ("cmle", 0x2E20_9800), ("cmlt", 0x0E20_A800)];

const LONG3: &[(&str, u32)] = &[
    ("saddl", 0x0E20_0000),
    ("uaddl", 0x2E20_0000),
    ("ssubl", 0x0E20_2000),
    ("usubl", 0x2E20_2000),
    ("smull", 0x0E20_C000),
    ("umull", 0x2E20_C000),
    ("smlal", 0x0E20_8000),
    ("umlal", 0x2E20_8000),
    ("smlsl", 0x0E20_A000),
    ("umlsl", 0x2E20_A000),
    ("sabdl", 0x0E20_7000),
    ("sqdmull", 0x0E20_D000),
    ("uabdl", 0x2E20_7000),
];

const SHIFT_IMM: &[(&str, u32, bool)] = &[
    ("shl", 0x0F00_5400, true),
    ("sli", 0x2F00_5400, true),
    ("sshr", 0x0F00_0400, false),
    ("ssra", 0x0F00_1400, false),
    ("srshr", 0x0F00_2400, false),
    ("srsra", 0x0F00_3400, false),
    ("ushr", 0x2F00_0400, false),
    ("usra", 0x2F00_1400, false),
    ("urshr", 0x2F00_2400, false),
    ("ursra", 0x2F00_3400, false),
    ("sri", 0x2F00_4400, false),
    ("sqshl", 0x0F00_7400, true),
    ("uqshl", 0x2F00_7400, true),
    ("sqshlu", 0x2F00_6400, true),
];

const ACROSS: &[(&str, u32, bool)] = &[
    ("addv", 0x0E31_B800, false),
    ("smaxv", 0x0E30_A800, false),
    ("sminv", 0x0E31_A800, false),
    ("umaxv", 0x2E30_A800, false),
    ("uminv", 0x2E31_A800, false),
    ("saddlv", 0x0E30_3800, true),
    ("uaddlv", 0x2E30_3800, true),
];

fn imm5(es: u8, idx: i64) -> Res<u32> {
    if es > 3 {
        return bad();
    }
    let i = range(idx, 0, (16 >> es) - 1, "lane index")?;
    Ok(i << (es + 1) | 1 << es)
}

fn kind_of_es(es: u8) -> Option<K> {
    Some(match es {
        0 => K::B,
        1 => K::H,
        2 => K::S,
        3 => K::D,
        _ => return None,
    })
}

fn vec_mov(ctx: &mut Ctx, o: &[Op]) -> Res<()> {
    if o.len() != 2 {
        return bad();
    }
    match (&o[0], &o[1]) {
        (Op::Reg(rd, Some(a), None), Op::Reg(rn, Some(b), None)) if rd.k == K::V && rn.k == K::V && a == b && a.es == 0 && a.n != 0 => {
            w(ctx, 0x0EA0_1C00 | a.q() << 30 | rn.n() << 16 | rn.n() << 5 | rd.n());
        }
        (Op::Reg(rd, Some(Arr { n: 0, es }), Some(i)), Op::Reg(rn, Some(Arr { n: 0, es: es2 }), Some(j))) if rd.k == K::V && rn.k == K::V && es == es2 => {
            let a = imm5(*es, *i)?;
            let b = range(*j, 0, (16 >> es) - 1, "lane index")?;
            w(ctx, 0x6E00_0400 | a << 16 | (b << es) << 11 | rn.n() << 5 | rd.n());
        }
        (Op::Reg(rd, Some(Arr { n: 0, es }), Some(i)), Op::Reg(rn, None, None)) if rd.k == K::V && rn.is_gp() => {
            if (*es == 3) != (rn.k == K::X) {
                return bad();
            }
            w(ctx, 0x4E00_1C00 | imm5(*es, *i)? << 16 | rn.n() << 5 | rd.n());
        }
        (Op::Reg(rd, None, None), Op::Reg(rn, Some(Arr { n: 0, es }), Some(i))) if rd.is_gp() && rn.k == K::V => {
            if (*es == 3) != (rd.k == K::X) || *es > 3 {
                return bad();
            }
            w(ctx, 0x0E00_3C00 | ((*es == 3) as u32) << 30 | imm5(*es, *i)? << 16 | rn.n() << 5 | rd.n());
        }
        (Op::Reg(rd, None, None), Op::Reg(rn, Some(Arr { n: 0, es }), Some(i))) if rd.is_fp() && rn.k == K::V && kind_of_es(*es) == Some(rd.k) => {
            w(ctx, 0x5E00_0400 | imm5(*es, *i)? << 16 | rn.n() << 5 | rd.n());
        }
        _ => return bad(),
    }
    Ok(())
}

fn vec_list<'a>(o: Option<&'a Op<'_>>) -> Res<(&'a Vec<Reg>, Option<Arr>, Option<i64>)> {
    match o {
        Some(Op::List(r, a, l)) => Ok((r, *a, *l)),
        _ => bad(),
    }
}

fn ldst_vec(ctx: &mut Ctx, mn: &str, o: &[Op]) -> Res<bool> {
    let load = mn.starts_with("ld") as u32;
    let rep = mn.ends_with('r');
    let selem: u32 = match mn.as_bytes().get(2) {
        Some(b'1') => 1,
        Some(b'2') => 2,
        Some(b'3') => 3,
        Some(b'4') => 4,
        _ => return Ok(false),
    };
    if !(mn.len() == 3 || (rep && mn.len() == 4)) || (rep && load == 0) {
        return Ok(false);
    }
    if o.len() > 3 {
        return bad();
    }
    let (regs, arr, lane) = vec_list(o.first())?;
    let Some(arr) = arr else { return bad() };
    let Some(Op::Mem(m)) = o.get(1) else { return bad() };
    if m.pre || !matches!(m.off, Off::None) {
        return bad();
    }
    let n = m.base.n() as u32;
    let cnt = regs.len() as u32;
    let rt = regs[0].n;
    if !regs.iter().enumerate().all(|(i, r)| r.n == (rt + i as u8) % 32) {
        return msg("registers in a list must be consecutive");
    }
    let rt = rt as u32;
    let post = o.get(2);
    let bytes = |total: u32| -> Res<(u32, u32)> {
        match post {
            None => Ok((0, 0)),
            Some(Op::Reg(r, None, None)) if r.k == K::X && !r.sp => Ok((1, r.n())),
            Some(p @ Op::Expr { .. }) => {
                let k = imm(ctx, Some(p))?;
                if k != total as i64 {
                    return msg(format!("the post-index immediate must be {}", total));
                }
                Ok((1, 31))
            }
            _ => bad(),
        }
    };
    if rep {
        if lane.is_some() || cnt != selem || arr.n == 0 || arr.es > 3 {
            return bad();
        }
        let (p, rm) = bytes(selem << arr.es)?;
        w(ctx, 0x0D40_C000 | arr.q() << 30 | ((selem - 1) & 1) << 21 | ((selem - 1) >> 1) << 13 | (arr.es as u32) << 10 | p << 23 | rm << 16 | n << 5 | rt);
    } else if let Some(idx) = lane {
        if arr.n != 0 || cnt != selem || arr.es > 3 {
            return bad();
        }
        let idx = range(idx, 0, (16 >> arr.es) - 1, "lane index")?;
        let (q, s, size, base3) = match arr.es {
            0 => (idx >> 3, (idx >> 2) & 1, idx & 3, 0),
            1 => (idx >> 2, (idx >> 1) & 1, (idx & 1) << 1, 2),
            2 => (idx >> 1, idx & 1, 0, 4),
            _ => (idx, 0, 1, 4),
        };
        let (p, rm) = bytes(selem << arr.es)?;
        let opc3 = base3 | ((selem - 1) >> 1);
        w(ctx, 0x0D00_0000 | q << 30 | load << 22 | ((selem - 1) & 1) << 21 | opc3 << 13 | s << 12 | size << 10 | p << 23 | rm << 16 | n << 5 | rt);
    } else {
        if arr.n == 0 || arr.es > 3 || (selem > 1 && cnt != selem) {
            return bad();
        }
        let opcode = match (selem, cnt) {
            (1, 1) => 0b0111,
            (1, 2) => 0b1010,
            (1, 3) => 0b0110,
            (1, 4) => 0b0010,
            (2, _) => 0b1000,
            (3, _) => 0b0100,
            _ => 0b0000,
        };
        let (p, rm) = bytes(cnt * if arr.q() == 1 { 16 } else { 8 })?;
        w(ctx, 0x0C00_0000 | arr.q() << 30 | load << 22 | p << 23 | rm << 16 | opcode << 12 | (arr.es as u32) << 10 | n << 5 | rt);
    }
    Ok(true)
}

fn movi_like(ctx: &mut Ctx, mn: &str, o: &[Op]) -> Res<()> {
    // movi / mvni / orr / bic with a vector immediate
    if o.len() > 3 {
        return bad();
    }
    let (rd, a) = match o.first() {
        Some(Op::Reg(r, None, None)) if r.k == K::D && mn == "movi" => (*r, Arr { n: 1, es: 3 }),
        _ => vr(o.first())?,
    };
    let scalar = o.first().is_some_and(|x| matches!(x, Op::Reg(r, None, None) if r.k == K::D));
    let k = imm(ctx, o.get(1))?;
    let (sh_kind, sh) = match o.get(2) {
        None => ("lsl", 0),
        Some(Op::Ext(n, a)) if n == "lsl" || n == "msl" => (if n == "lsl" { "lsl" } else { "msl" }, a.unwrap_or(0)),
        _ => return bad(),
    };
    let inv = (mn == "mvni" || mn == "bic") as u32;
    let (op, cmode, imm8) = if a.es == 3 {
        if mn != "movi" || sh != 0 || (a.n == 1 && !scalar) {
            return bad();
        }
        // a 64-bit mask of 0x00 / 0xff bytes
        let v = k as u64;
        let mut i8 = 0u32;
        for b in 0..8 {
            match (v >> (8 * b)) & 0xff {
                0 => {}
                0xff => i8 |= 1 << b,
                _ => return msg("a 64-bit vector immediate must have only 0x00 and 0xff bytes"),
            }
        }
        (1, 0b1110, i8)
    } else {
        let i = range(k, 0, 255, "8-bit vector immediate")?;
        let or_bit = (mn == "orr" || mn == "bic") as u32;
        let cm = match (a.es, sh_kind) {
            (2, "lsl") if sh % 8 == 0 && (0..=24).contains(&sh) => (sh as u32 / 8) << 1 | or_bit,
            (2, "msl") if mn.ends_with('i') && (sh == 8 || sh == 16) => 0b1100 | (sh as u32 / 8 - 1),
            (1, "lsl") if (sh == 0 || sh == 8) => 0b1000 | (sh as u32 / 8) << 1 | or_bit,
            (0, "lsl") if sh == 0 && mn == "movi" => 0b1110,
            _ => return bad(),
        };
        (inv, cm, i)
    };
    let q = if scalar { 0 } else { a.q() };
    let base = if scalar { 0x2F00_0400 } else { 0x0F00_0400 | op << 29 };
    w(ctx, base | q << 30 | ((imm8 >> 5) & 7) << 16 | cmode << 12 | (imm8 & 31) << 5 | rd.n());
    Ok(())
}

fn neon_insn(ctx: &mut Ctx, mn: &str, o: &[Op]) -> Res<bool> {
    if o.len() == 3 && matches!(o.get(2), Some(Op::Reg(_, Some(Arr { n: 0, .. }), Some(_)))) && elem_insn(ctx, mn, o)? {
        return Ok(true);
    }
    if scalar_simd(ctx, mn, o)? {
        return Ok(true);
    }
    match mn {
        "mov" => {
            vec_mov(ctx, o)?;
            return Ok(true);
        }
        "umov" | "smov" => {
            let rd = gp(o.first())?;
            let (rn, es, i) = velem(o.get(1))?;
            let ok = if mn == "umov" { (es == 3) == (rd.k == K::X) && es <= 3 } else { es <= 2 && (es <= 1 || rd.k == K::X) };
            if !ok {
                return bad();
            }
            w(ctx, if mn == "umov" { 0x0E00_3C00 } else { 0x0E00_2C00 } | rd.sf() << 30 | imm5(es, i)? << 16 | rn.n() << 5 | rd.n());
            return Ok(true);
        }
        "ins" => {
            vec_mov(ctx, o)?;
            return Ok(true);
        }
        "dup" => {
            match (&o[..], o.get(0)) {
                ([Op::Reg(rd, Some(a), None), Op::Reg(rn, Some(Arr { n: 0, es }), Some(i))], _) if rd.k == K::V && a.n != 0 && a.es == *es => {
                    w(ctx, 0x0E00_0400 | a.q() << 30 | imm5(*es, *i)? << 16 | rn.n() << 5 | rd.n());
                }
                ([Op::Reg(rd, Some(a), None), Op::Reg(rn, None, None)], _) if rd.k == K::V && a.n != 0 && rn.is_gp() && (a.es == 3) == (rn.k == K::X) => {
                    w(ctx, 0x0E00_0C00 | a.q() << 30 | imm5(a.es, 0)? << 16 | rn.n() << 5 | rd.n());
                }
                ([Op::Reg(rd, None, None), Op::Reg(rn, Some(Arr { n: 0, es }), Some(i))], _) if rd.is_fp() && kind_of_es(*es) == Some(rd.k) => {
                    w(ctx, 0x5E00_0400 | imm5(*es, *i)? << 16 | rn.n() << 5 | rd.n());
                }
                _ => return bad(),
            }
            return Ok(true);
        }
        "ld1" | "ld2" | "ld3" | "ld4" | "st1" | "st2" | "st3" | "st4" | "ld1r" | "ld2r" | "ld3r" | "ld4r" => return ldst_vec(ctx, mn, o),
        "movi" | "mvni" => {
            movi_like(ctx, mn, o)?;
            return Ok(true);
        }
        "ext" => {
            let (rd, a) = vr(o.first())?;
            let (rn, b) = vr(o.get(1))?;
            let (rm, c) = vr(o.get(2))?;
            if a != b || b != c || a.es != 0 {
                return bad();
            }
            let k = uimm(ctx, o.get(3), if a.n == 16 { 15 } else { 7 }, "ext index")?;
            w(ctx, 0x2E00_0000 | a.q() << 30 | rm.n() << 16 | k << 11 | rn.n() << 5 | rd.n());
            return Ok(true);
        }
        "tbl" | "tbx" => {
            let (rd, a) = vr(o.first())?;
            let (regs, la, lane) = vec_list(o.get(1))?;
            let (rm, c) = vr(o.get(2))?;
            if a.es != 0 || c != a || la != Some(Arr { n: 16, es: 0 }) || lane.is_some() {
                return bad();
            }
            let rt = regs[0].n;
            if !regs.iter().enumerate().all(|(i, r)| r.n == (rt + i as u8) % 32) {
                return msg("registers in a list must be consecutive");
            }
            w(ctx, 0x0E00_0000 | a.q() << 30 | rm.n() << 16 | (regs.len() as u32 - 1) << 13 | ((mn == "tbx") as u32) << 12 | (rt as u32) << 5 | rd.n());
            return Ok(true);
        }
        "zip1" | "zip2" | "uzp1" | "uzp2" | "trn1" | "trn2" => {
            let (rd, a) = vr(o.first())?;
            let (rn, b) = vr(o.get(1))?;
            let (rm, c) = vr(o.get(2))?;
            if a != b || b != c || !arr_ok(M::All, a) {
                return bad();
            }
            let opc = match mn {
                "uzp1" => 1,
                "trn1" => 2,
                "zip1" => 3,
                "uzp2" => 5,
                "trn2" => 6,
                _ => 7,
            };
            w(ctx, 0x0E00_0800 | a.q() << 30 | (a.es as u32) << 22 | rm.n() << 16 | opc << 12 | rn.n() << 5 | rd.n());
            return Ok(true);
        }
        "fcvtl" | "fcvtl2" | "fcvtn" | "fcvtn2" => {
            let (rd, a) = vr(o.first())?;
            let (rn, b) = vr(o.get(1))?;
            let two = mn.ends_with('2');
            let (wide, narrow, base) = if mn.starts_with("fcvtl") { (a, b, 0x0E21_7800) } else { (b, a, 0x0E21_6800) };
            // wide: 4s / 2d; narrow: 4h / 2s (8h / 4s for the "2" forms)
            if wide.es != narrow.es + 1 || !(wide.es == 2 || wide.es == 3) || (wide.n as u32 * (1 << wide.es) != 16) || ((narrow.n as u32 * (1 << narrow.es) == 16) != two) {
                return bad();
            }
            w(ctx, base | (two as u32) << 30 | ((wide.es == 3) as u32) << 22 | rn.n() << 5 | rd.n());
            return Ok(true);
        }
        "xtn" | "xtn2" => {
            let (rd, a) = vr(o.first())?;
            let (rn, b) = vr(o.get(1))?;
            let two = mn.ends_with('2');
            if b.es != a.es + 1 || b.es > 3 || b.n as u32 * (1 << b.es) != 16 || (a.n as u32 * (1 << a.es) == 16) != two {
                return bad();
            }
            w(ctx, 0x0E21_2800 | (two as u32) << 30 | (a.es as u32) << 22 | rn.n() << 5 | rd.n());
            return Ok(true);
        }
        "sxtl" | "uxtl" | "sxtl2" | "uxtl2" | "sshll" | "ushll" | "sshll2" | "ushll2" => {
            let (rd, a) = vr(o.first())?;
            let (rn, b) = vr(o.get(1))?;
            let two = mn.ends_with('2');
            let signed = mn.starts_with('s');
            let sh = if mn.contains("xtl") { 0 } else { imm(ctx, o.get(2))? };
            if a.es != b.es + 1 || a.es > 3 || a.n as u32 * (1 << a.es) != 16 || (b.n as u32 * (1 << b.es) == 16) != two {
                return bad();
            }
            let sh = range(sh, 0, (8 << b.es) - 1, "shift amount")?;
            w(ctx, if signed { 0x0F00_A400 } else { 0x2F00_A400 } | (two as u32) << 30 | ((8u32 << b.es) | sh) << 16 | rn.n() << 5 | rd.n());
            return Ok(true);
        }
        _ => {}
    }
    if mn == "addp" && o.len() == 2 {
        if let Some(Op::Reg(rd, None, None)) = o.first() {
            let (rn, a) = vr(o.get(1))?;
            if rd.k != K::D || a != (Arr { n: 2, es: 3 }) {
                return bad();
            }
            w(ctx, 0x5EF1_B800 | rn.n() << 5 | rd.n());
            return Ok(true);
        }
    }
    if mn == "faddp" && o.len() == 2 {
        if let Some(Op::Reg(rd, None, None)) = o.first() {
            let (rn, a) = vr(o.get(1))?;
            let sz = match (rd.k, a) {
                (K::S, Arr { n: 2, es: 2 }) => 0,
                (K::D, Arr { n: 2, es: 3 }) => 1,
                _ => return bad(),
            };
            w(ctx, 0x7E30_D800 | sz << 22 | rn.n() << 5 | rd.n());
            return Ok(true);
        }
    }
    if let (Some(&(_, base, _)), 2) = (ACROSS.iter().find(|e| e.0 == mn), o.len()) {
        let long = ACROSS.iter().find(|e| e.0 == mn).unwrap().2;
        let rd = fpr(o.first())?;
        let (rn, a) = vr(o.get(1))?;
        let ok = if long { a.es <= 2 && !(a.es == 2 && a.n == 2) } else { a.es <= 2 && !(a.es == 2 && a.n == 2) };
        let want = if long { kind_of_es(a.es + 1) } else { kind_of_es(a.es) };
        if !ok || want != Some(rd.k) {
            return bad();
        }
        w(ctx, base | a.q() << 30 | (a.es as u32) << 22 | rn.n() << 5 | rd.n());
        return Ok(true);
    }
    for &(name, base) in ZERO_CMP {
        if name == mn && is_imm(o.get(2)) && is_vec_op(o.first()) && o.len() == 3 {
            let (rd, a) = vr(o.first())?;
            let (rn, b) = vr(o.get(1))?;
            if a != b || !arr_ok(M::All, a) || imm(ctx, o.get(2))? != 0 {
                return bad();
            }
            w(ctx, base | a.q() << 30 | (a.es as u32) << 22 | rn.n() << 5 | rd.n());
            return Ok(true);
        }
    }
    for &(name, base, _) in THREE_F {
        if name == mn && is_vec_op(o.first()) && o.len() == 3 && !is_imm(o.get(2)) {
            let (rd, a) = vr(o.first())?;
            let (rn, b) = vr(o.get(1))?;
            let (rm, c) = vr(o.get(2))?;
            if a != b || b != c {
                return bad();
            }
            let half = THREE_F.iter().find(|e| e.0 == mn).unwrap().2;
            let word = match (a.es, a.n) {
                (2, 2) | (2, 4) => base | a.q() << 30,
                (3, 2) => base | 1 << 30 | 1 << 22,
                (1, 4) | (1, 8) => half | a.q() << 30,
                _ => return bad(),
            };
            w(ctx, word | rm.n() << 16 | rn.n() << 5 | rd.n());
            return Ok(true);
        }
    }
    for &(name, base) in TWO_F {
        if name == mn && is_vec_op(o.first()) && o.len() == 2 {
            let (rd, a) = vr(o.first())?;
            let (rn, b) = vr(o.get(1))?;
            if a.es == 1 {
                continue;
            }
            if a != b || !matches!((a.es, a.n), (2, 2) | (2, 4) | (3, 2)) {
                return bad();
            }
            w(ctx, base | a.q() << 30 | ((a.es == 3) as u32) << 22 | rn.n() << 5 | rd.n());
            return Ok(true);
        }
    }
    for &(name, base, m) in TWO {
        if name == mn && is_vec_op(o.first()) && o.len() == 2 {
            let (rd, a) = vr(o.first())?;
            let (rn, b) = vr(o.get(1))?;
            if a != b || !arr_ok(m, a) {
                return bad();
            }
            let size = if m == M::Bytes { 0 } else { a.es as u32 };
            w(ctx, base | a.q() << 30 | size << 22 | rn.n() << 5 | rd.n());
            return Ok(true);
        }
    }
    for &(name, base) in LONG3 {
        let two = mn.ends_with('2') && mn.len() == name.len() + 1;
        if (name == mn || two) && mn.starts_with(name) && is_vec_op(o.first()) && o.len() == 3 {
            let (rd, a) = vr(o.first())?;
            let (rn, b) = vr(o.get(1))?;
            let (rm, c) = vr(o.get(2))?;
            if b != c || a.es != b.es + 1 || a.es > 3 || a.n as u32 * (1 << a.es) != 16 || (b.n as u32 * (1 << b.es) == 16) != two {
                return bad();
            }
            w(ctx, base | (two as u32) << 30 | (b.es as u32) << 22 | rm.n() << 16 | rn.n() << 5 | rd.n());
            return Ok(true);
        }
    }
    for &(name, base, left) in SHIFT_IMM {
        if name == mn && is_vec_op(o.first()) && o.len() == 3 && is_imm(o.get(2)) {
            let (rd, a) = vr(o.first())?;
            let (rn, b) = vr(o.get(1))?;
            if a != b || !arr_ok(M::All, a) {
                return bad();
            }
            let esize = 8i64 << a.es;
            let sh = imm(ctx, o.get(2))?;
            let hb = if left { esize + range(sh, 0, esize - 1, "shift amount")? as i64 } else { 2 * esize - range(sh, 1, esize, "shift amount")? as i64 };
            w(ctx, base | a.q() << 30 | (hb as u32) << 16 | rn.n() << 5 | rd.n());
            return Ok(true);
        }
    }
    // cmle / cmlt / cmls / cmlo with registers: the swapped cmge / cmgt / cmhs / cmhi
    let swapped = match mn {
        "cmle" => Some("cmge"),
        "cmlt" => Some("cmgt"),
        "cmls" => Some("cmhs"),
        "cmlo" => Some("cmhi"),
        _ => None,
    };
    if let (Some(real), 3) = (swapped, o.len()) {
        let &(_, base, m) = THREE.iter().find(|e| e.0 == real).unwrap();
        let (rd, a) = vr(o.first())?;
        let (rn, b) = vr(o.get(1))?;
        let (rm, c) = vr(o.get(2))?;
        if a != b || b != c || !arr_ok(m, a) {
            return bad();
        }
        w(ctx, base | a.q() << 30 | (a.es as u32) << 22 | rn.n() << 16 | rm.n() << 5 | rd.n());
        return Ok(true);
    }
    if matches!(mn, "orr" | "bic") && is_imm(o.get(1)) && is_vec_op(o.first()) {
        movi_like(ctx, mn, o)?;
        return Ok(true);
    }
    if let (Some(&(_, base, m)), 3) = (THREE.iter().find(|e| e.0 == mn), o.len()) {
        let (rd, a) = vr(o.first())?;
        let (rn, b) = vr(o.get(1))?;
        let (rm, c) = vr(o.get(2))?;
        if a != b || b != c || !arr_ok(m, a) {
            return bad();
        }
        let size = if m == M::Bytes { 0 } else { a.es as u32 };
        w(ctx, base | a.q() << 30 | size << 22 | rm.n() << 16 | rn.n() << 5 | rd.n());
        return Ok(true);
    }
    neon_extra(ctx, mn, o)
}

// ---------------------------------------------------------------- the encoder

impl Encoder for Arm64 {
    fn instruction(&mut self, ctx: &mut Ctx, mnemonic: &str, ops: &[Tok]) -> Result<(), String> {
        let ops = parse_ops(ctx, ops).map_err(|e| if e == "!" { format!("unsupported operands for `{}`", mnemonic) } else { format!("{} (in `{}`)", e, mnemonic) })?;
        let o = &ops[..];
        let r = encode(ctx, mnemonic, o);
        match r {
            Ok(true) => Ok(()),
            Ok(false) => Err(format!("unsupported instruction `{}`", mnemonic)),
            Err(e) if e == "!" => Err(format!("unsupported operands for `{}`", mnemonic)),
            Err(e) => Err(format!("{} (in `{}`)", e, mnemonic)),
        }
    }
    fn directive(&mut self, ctx: &mut Ctx, name: &str, args: &[Tok]) -> Result<bool, String> {
        match name {
            ".inst" => {
                for p in split_commas(args) {
                    let mut i = 0;
                    let v = ctx.const_expr(p, &mut i)?;
                    ctx.emit_u32(v as u32);
                }
                Ok(true)
            }
            ".arch" | ".cpu" | ".arch_extension" | ".ltorg" | ".pool" | ".tlsdesccall" => Ok(true),
            _ => Ok(false),
        }
    }
    fn code_fill(&self, n: usize) -> Vec<u8> {
        let mut v = Vec::with_capacity(n);
        while v.len() + 4 <= n {
            v.extend_from_slice(&0xd503201fu32.to_le_bytes());
        }
        v.resize(n, 0);
        v
    }
}

/// How many operands a mnemonic takes (min, max); None: not checked here.
fn arity(mn: &str) -> Option<(usize, usize)> {
    Some(match mn {
        "add" | "adds" | "sub" | "subs" | "and" | "eor" | "ands" | "orn" | "eon" | "bics" => (3, 4),
        "orr" | "bic" => (2, 4),
        "cmp" | "cmn" | "tst" | "neg" | "negs" | "mvn" | "movz" | "movn" | "movk" => (2, 3),
        "mov" | "ngc" | "ngcs" | "uxtb" | "uxth" | "sxtb" | "sxth" | "sxtw" | "uxtw" | "clz" | "cls" | "rbit" | "rev" | "rev16" | "rev32" | "rev64" | "cset" | "csetm" | "adr" | "adrp" | "cbz" | "cbnz" | "mrs" | "msr" | "fmov" | "fabs" | "fneg" | "fsqrt" | "frintn" | "frintp" | "frintm" | "frintz" | "frinta" | "frintx" | "frinti" | "fcvt" | "fcmp" | "fcmpe" | "fcvtl" | "fcvtl2" | "fcvtn" | "fcvtn2" | "xtn" | "xtn2" | "sxtl" | "uxtl" | "sxtl2" | "uxtl2" | "cnt" | "not" | "umov" | "smov" | "ins" | "dup" | "sqabs" | "sqneg" | "rbit16" | "dc" | "prfm" | "prfum" => (2, 2),
        "adc" | "adcs" | "sbc" | "sbcs" | "mul" | "mneg" | "sdiv" | "udiv" | "smull" | "umull" | "smnegl" | "umnegl" | "smulh" | "umulh" | "lsl" | "lsr" | "asr" | "ror" | "cinc" | "cinv" | "cneg" | "bfc" | "tbz" | "tbnz" | "sshll" | "ushll" | "sshll2" | "ushll2" | "tbl" | "tbx" | "zip1" | "zip2" | "uzp1" | "uzp2" | "trn1" | "trn2" | "fadd" | "fsub" | "fdiv" | "fmax" | "fmin" | "fmaxnm" | "fminnm" | "fnmul" => (3, 3),
        "madd" | "msub" | "smaddl" | "umaddl" | "smsubl" | "umsubl" | "extr" | "ubfm" | "sbfm" | "bfm" | "ubfx" | "sbfx" | "ubfiz" | "sbfiz" | "bfi" | "bfxil" | "csel" | "csinc" | "csinv" | "csneg" | "ccmp" | "ccmn" | "fmadd" | "fmsub" | "fnmadd" | "fnmsub" | "fccmp" | "fccmpe" | "fcsel" | "ext" => (4, 4),
        "b" | "bl" | "br" | "blr" | "svc" | "hvc" | "smc" | "brk" | "hlt" | "udf" | "hint" | "dmb" | "dsb" => (1, 1),
        "ret" | "isb" | "clrex" | "bti" => (0, 1),
        "nop" | "yield" | "wfe" | "wfi" | "sev" | "sevl" | "eret" | "csdb" => (0, 0),
        "movi" | "mvni" => (2, 3),
        "ldr" | "str" | "ldrb" | "strb" | "ldrh" | "strh" | "ldrsb" | "ldrsh" | "ldrsw" | "ldur" | "stur" => (2, 3),
        "ldp" | "stp" | "ldpsw" => (3, 4),
        "ldnp" | "stnp" => (3, 3),
        "at" => (2, 2),
        "sys" => (4, 5),
        "sysl" => (5, 5),
        "fmul" | "fmla" | "fmls" | "fmulx" => (3, 3),
        "ic" | "tlbi" => (1, 2),
        "scvtf" | "ucvtf" | "fcvtzs" | "fcvtzu" => (2, 3),
        "fcvtns" | "fcvtnu" | "fcvtas" | "fcvtau" | "fcvtps" | "fcvtpu" | "fcvtms" | "fcvtmu" => (2, 2),
        _ => return None,
    })
}

fn encode(ctx: &mut Ctx, mn: &str, o: &[Op]) -> Res<bool> {
    if let Some((lo, hi)) = arity(mn) {
        if o.len() < lo || o.len() > hi {
            return bad();
        }
    }
    if let Some(c) = mn.strip_prefix("b.") {
        if o.len() != 1 {
            return bad();
        }
        let c = cond_code(c).ok_or_else(|| format!("unknown condition code `{}`", c))?;
        branch_target(ctx, 0x5400_0000 | c, FixKind::A64Branch19, o.first(), 19, 5)?;
        return Ok(true);
    }
    // vector / scalar-float first operands belong to the NEON and FP tables
    let fpfirst = matches!(o.first(), Some(Op::Reg(r, _, _)) if r.k == K::V || r.is_fp());
    if !fpfirst || mn == "ldr" || mn == "str" || mn.starts_with("ldu") || mn.starts_with("stu") || mn.starts_with("ldp") || mn.starts_with("stp") || mn.starts_with("ldn") || mn.starts_with("stn") {
        match mn {
            "ldr" | "str" | "ldrb" | "strb" | "ldrh" | "strh" | "ldrsb" | "ldrsh" | "ldrsw" | "ldur" | "stur" | "ldurb" | "sturb" | "ldurh" | "sturh" | "ldursb" | "ldursh" | "ldursw" | "ldtr" | "sttr" | "ldtrb" | "sttrb" | "ldtrh" | "sttrh" | "ldtrsb" | "ldtrsh" | "ldtrsw" | "ldapur" | "stlur" | "ldapurb" | "stlurb" | "ldapurh" | "stlurh" | "ldapursb" | "ldapursh" | "ldapursw" => {
                ldst(ctx, mn, o)?;
                return Ok(true);
            }
            "ldp" | "stp" | "ldpsw" | "ldnp" | "stnp" => {
                ldstp(ctx, mn, o)?;
                return Ok(true);
            }
            "prfm" | "prfum" => {
                prfm(ctx, mn, o)?;
                return Ok(true);
            }
            _ => {}
        }
        if int_insn(ctx, mn, o)? || excl(ctx, mn, o)? || lse(ctx, mn, o)? || sys_insn(ctx, mn, o)? {
            return Ok(true);
        }
    }
    if fp_insn(ctx, mn, o)? {
        return Ok(true);
    }
    neon_insn(ctx, mn, o)
}

// ---------------------------------------------------------------- NEON, the long tail

const SCALAR_D3: &[&str] = &["add", "sub", "cmeq", "cmgt", "cmge", "cmhi", "cmhs", "cmtst", "sshl", "ushl", "srshl", "urshl"];
const SCALAR_F3: &[&str] = &["fabd", "fcmeq", "fcmge", "fcmgt", "facge", "facgt", "fmulx", "frecps", "frsqrts"];
/// float 2-reg ops that also have scalar SIMD forms (s / d registers)
const SCALAR_F2: &[&str] = &["scvtf", "ucvtf", "fcvtzs", "fcvtzu", "fcvtns", "fcvtnu", "fcvtms", "fcvtmu", "fcvtas", "fcvtau", "fcvtps", "fcvtpu", "frecpe", "frsqrte"];

/// float 2-reg misc, half precision (4h / 8h) bases
const TWO_F16: &[(&str, u32)] = &[
    ("fabs", 0x0EF8_F800),
    ("fneg", 0x2EF8_F800),
    ("fsqrt", 0x2EF9_F800),
    ("frintn", 0x0E79_8800),
    ("frintm", 0x0E79_9800),
    ("frintp", 0x0EF9_8800),
    ("frintz", 0x0EF9_9800),
    ("frinta", 0x2E79_8800),
    ("frintx", 0x2E79_9800),
    ("frinti", 0x2EF9_9800),
    ("scvtf", 0x0E79_D800),
    ("ucvtf", 0x2E79_D800),
    ("fcvtzs", 0x0EF9_B800),
    ("fcvtzu", 0x2EF9_B800),
    ("fcvtns", 0x0E79_A800),
    ("fcvtnu", 0x2E79_A800),
    ("fcvtms", 0x0E79_B800),
    ("fcvtmu", 0x2E79_B800),
    ("fcvtas", 0x0E79_C800),
    ("fcvtau", 0x2E79_C800),
    ("fcvtps", 0x0EF9_A800),
    ("fcvtpu", 0x2EF9_A800),
    ("frecpe", 0x0EF9_D800),
    ("frsqrte", 0x2EF9_D800),
];

/// float compare with zero: (name, single / double base, half base)
const FCMP_ZERO: &[(&str, u32, u32)] = &[
    ("fcmeq", 0x0EA0_D800, 0x0EF8_D800),
    ("fcmge", 0x2EA0_C800, 0x2EF8_C800),
    ("fcmgt", 0x0EA0_C800, 0x0EF8_C800),
    ("fcmle", 0x2EA0_D800, 0x2EF8_D800),
    ("fcmlt", 0x0EA0_E800, 0x0EF8_E800),
];

const ELEM_F: &[(&str, u32)] = &[("fmul", 0x0F80_9000), ("fmla", 0x0F80_1000), ("fmls", 0x0F80_5000), ("fmulx", 0x2F80_9000)];
const ELEM_I: &[(&str, u32)] = &[("mul", 0x0F00_8000), ("mla", 0x2F00_0000), ("mls", 0x2F00_4000), ("sqdmulh", 0x0F00_C000), ("sqrdmulh", 0x0F00_D000)];
const ELEM_L: &[(&str, u32)] = &[("smull", 0x0F00_A000), ("umull", 0x2F00_A000), ("smlal", 0x0F00_2000), ("umlal", 0x2F00_2000), ("smlsl", 0x0F00_6000), ("umlsl", 0x2F00_6000), ("sqdmull", 0x0F00_B000)];

const NARROW_SHIFT: &[(&str, u32)] = &[("shrn", 0x0F00_8400), ("rshrn", 0x0F00_8C00), ("sqshrn", 0x0F00_9400), ("sqrshrn", 0x0F00_9C00), ("uqshrn", 0x2F00_9400), ("uqrshrn", 0x2F00_9C00), ("sqshrun", 0x2F00_8400), ("sqrshrun", 0x2F00_8C00)];
const NARROW3: &[(&str, u32)] = &[("addhn", 0x0E20_4000), ("raddhn", 0x2E20_4000), ("subhn", 0x0E20_6000), ("rsubhn", 0x2E20_6000)];
const NARROW2: &[(&str, u32)] = &[("xtn", 0x0E21_2800), ("sqxtn", 0x0E21_4800), ("uqxtn", 0x2E21_4800), ("sqxtun", 0x2E21_2800)];
const PAIRLONG: &[(&str, u32)] = &[("saddlp", 0x0E20_2800), ("uaddlp", 0x2E20_2800), ("sadalp", 0x0E20_6800), ("uadalp", 0x2E20_6800)];

/// L, M, H and the register field of an indexed element `vM.T[idx]`.
fn elem_fields(es: u8, idx: i64, rm: Reg) -> Res<(u32, u32, u32, u32)> {
    Ok(match es {
        1 => {
            let i = range(idx, 0, 7, "lane index")?;
            if rm.n > 15 {
                return msg("the element register must be v0 to v15 for half-word elements");
            }
            ((i >> 1) & 1, i & 1, i >> 2, rm.n())
        }
        2 => {
            let i = range(idx, 0, 3, "lane index")?;
            (i & 1, rm.n() >> 4, i >> 1, rm.n() & 15)
        }
        3 => {
            let i = range(idx, 0, 1, "lane index")?;
            (0, rm.n() >> 4, i, rm.n() & 15)
        }
        _ => return bad(),
    })
}

fn elem_insn(ctx: &mut Ctx, mn: &str, o: &[Op]) -> Res<bool> {
    let (rd, a) = vr(o.first())?;
    let (rn, b) = vr(o.get(1))?;
    let (rm, es, idx) = velem(o.get(2))?;
    if let Some(&(_, base)) = ELEM_F.iter().find(|e| e.0 == mn) {
        if a != b || a.es != es || !matches!((a.n, a.es), (2, 2) | (4, 2) | (2, 3)) {
            return bad();
        }
        let (l, m, h, r) = elem_fields(es, idx, rm)?;
        w(ctx, base | a.q() << 30 | ((es == 3) as u32) << 22 | l << 21 | m << 20 | r << 16 | h << 11 | rn.n() << 5 | rd.n());
        return Ok(true);
    }
    if let Some(&(_, base)) = ELEM_I.iter().find(|e| e.0 == mn) {
        if a != b || a.es != es || !(a.es == 1 || a.es == 2) {
            return bad();
        }
        let (l, m, h, r) = elem_fields(es, idx, rm)?;
        w(ctx, base | a.q() << 30 | (es as u32) << 22 | l << 21 | m << 20 | r << 16 | h << 11 | rn.n() << 5 | rd.n());
        return Ok(true);
    }
    for &(name, base) in ELEM_L {
        let two = mn.len() == name.len() + 1 && mn.ends_with('2');
        if mn == name || (two && mn.starts_with(name)) {
            if b.es != es || !(b.es == 1 || b.es == 2) || a.es != b.es + 1 || a.n as u32 * (1 << a.es) != 16 || (b.n as u32 * (1 << b.es) == 16) != two {
                return bad();
            }
            let (l, m, h, r) = elem_fields(es, idx, rm)?;
            w(ctx, base | (two as u32) << 30 | (es as u32) << 22 | l << 21 | m << 20 | r << 16 | h << 11 | rn.n() << 5 | rd.n());
            return Ok(true);
        }
    }
    Ok(false)
}

/// Scalar SIMD forms: d registers (integer ops) and s / d registers (float ops).
fn scalar_simd(ctx: &mut Ctx, mn: &str, o: &[Op]) -> Res<bool> {
    let Some(Op::Reg(rd, None, None)) = o.first() else { return Ok(false) };
    if !(rd.k == K::D || rd.k == K::S) {
        return Ok(false);
    }
    let sz = (rd.k == K::D) as u32;
    if rd.k == K::D {
        if SCALAR_D3.contains(&mn) && o.len() == 3 && !is_imm(o.get(2)) {
            let &(_, base, _) = THREE.iter().find(|e| e.0 == mn).unwrap();
            let (rn, rm) = (fp_same(o.get(1), K::D)?, fp_same(o.get(2), K::D)?);
            w(ctx, base + 0x5000_0000 | 3 << 22 | rm.n() << 16 | rn.n() << 5 | rd.n());
            return Ok(true);
        }
        if matches!(mn, "neg" | "abs") && o.len() == 2 {
            let rn = fp_same(o.get(1), K::D)?;
            w(ctx, if mn == "neg" { 0x2E20_B800 } else { 0x0E20_B800 } + 0x5000_0000 | 3 << 22 | rn.n() << 5 | rd.n());
            return Ok(true);
        }
        if let Some(&(_, base)) = ZERO_CMP.iter().find(|e| e.0 == mn) {
            if o.len() == 3 && is_imm(o.get(2)) {
                let rn = fp_same(o.get(1), K::D)?;
                if imm(ctx, o.get(2))? != 0 {
                    return bad();
                }
                w(ctx, base + 0x5000_0000 | 3 << 22 | rn.n() << 5 | rd.n());
                return Ok(true);
            }
        }
        if let Some(&(_, base, left)) = SHIFT_IMM.iter().find(|e| e.0 == mn) {
            if o.len() == 3 {
                let rn = fp_same(o.get(1), K::D)?;
                let sh = imm(ctx, o.get(2))?;
                let hb = if left { 64 + range(sh, 0, 63, "shift amount")? } else { 128 - range(sh, 1, 64, "shift amount")? };
                w(ctx, base + 0x5000_0000 | hb << 16 | rn.n() << 5 | rd.n());
                return Ok(true);
            }
        }
    }
    if SCALAR_F3.contains(&mn) && o.len() == 3 && !is_imm(o.get(2)) {
        let &(_, base, _) = THREE_F.iter().find(|e| e.0 == mn).unwrap();
        let (rn, rm) = (fp_same(o.get(1), rd.k)?, fp_same(o.get(2), rd.k)?);
        w(ctx, base + 0x5000_0000 | sz << 22 | rm.n() << 16 | rn.n() << 5 | rd.n());
        return Ok(true);
    }
    if let Some(&(_, base, _)) = FCMP_ZERO.iter().find(|e| e.0 == mn) {
        if o.len() == 3 && is_imm(o.get(2)) {
            let rn = fp_same(o.get(1), rd.k)?;
            if fp_lit(ctx, o.get(2))? != 0.0 {
                return bad();
            }
            w(ctx, base + 0x5000_0000 | sz << 22 | rn.n() << 5 | rd.n());
            return Ok(true);
        }
    }
    if mn == "frecpx" && o.len() == 2 {
        let rn = fp_same(o.get(1), rd.k)?;
        w(ctx, 0x5EA1_F800 | sz << 22 | rn.n() << 5 | rd.n());
        return Ok(true);
    }
    if SCALAR_F2.contains(&mn) && o.len() >= 2 && matches!(o.get(1), Some(Op::Reg(r, None, None)) if r.is_fp()) {
        let &(_, base) = TWO_F.iter().find(|e| e.0 == mn).unwrap();
        let rn = fp_same(o.get(1), rd.k)?;
        if o.len() == 2 {
            w(ctx, base + 0x5000_0000 | sz << 22 | rn.n() << 5 | rd.n());
        } else {
            let fixed = match mn {
                "scvtf" => 0x5F00_E400,
                "ucvtf" => 0x7F00_E400,
                "fcvtzs" => 0x5F00_FC00,
                "fcvtzu" => 0x7F00_FC00,
                _ => return bad(),
            };
            let fb = imm(ctx, o.get(2))?;
            let esize = if rd.k == K::D { 64 } else { 32 };
            let hb = 2 * esize - range(fb, 1, esize as i64, "number of fractional bits")?;
            w(ctx, fixed | hb << 16 | rn.n() << 5 | rd.n());
        }
        return Ok(true);
    }
    Ok(false)
}

fn neon_extra(ctx: &mut Ctx, mn: &str, o: &[Op]) -> Res<bool> {
    // across lanes, float
    if matches!(mn, "fmaxv" | "fminv" | "fmaxnmv" | "fminnmv") {
        let rd = fp_same(o.first(), K::S)?;
        let (rn, a) = vr(o.get(1))?;
        if a != (Arr { n: 4, es: 2 }) || o.len() != 2 {
            return bad();
        }
        let base = match mn {
            "fmaxv" => 0x6E30_F800,
            "fminv" => 0x6EB0_F800,
            "fmaxnmv" => 0x6E30_C800,
            _ => 0x6EB0_C800,
        };
        w(ctx, base | rn.n() << 5 | rd.n());
        return Ok(true);
    }
    for &(name, base) in PAIRLONG {
        if name == mn {
            let (rd, a) = vr(o.first())?;
            let (rn, b) = vr(o.get(1))?;
            if o.len() != 2 || a.es != b.es + 1 || a.es > 3 || a.n * 2 != b.n || b.es > 2 {
                return bad();
            }
            w(ctx, base | b.q() << 30 | (b.es as u32) << 22 | rn.n() << 5 | rd.n());
            return Ok(true);
        }
    }
    for &(name, base) in NARROW2 {
        let two = mn.len() == name.len() + 1 && mn.ends_with('2') && mn.starts_with(name);
        if mn == name || two {
            let (rd, a) = vr(o.first())?;
            let (rn, b) = vr(o.get(1))?;
            if o.len() != 2 || b.es != a.es + 1 || b.es > 3 || b.n as u32 * (1 << b.es) != 16 || (a.n as u32 * (1 << a.es) == 16) != two {
                return bad();
            }
            w(ctx, base | (two as u32) << 30 | (a.es as u32) << 22 | rn.n() << 5 | rd.n());
            return Ok(true);
        }
    }
    for &(name, base) in NARROW3 {
        let two = mn.len() == name.len() + 1 && mn.ends_with('2') && mn.starts_with(name);
        if mn == name || two {
            let (rd, a) = vr(o.first())?;
            let (rn, b) = vr(o.get(1))?;
            let (rm, c) = vr(o.get(2))?;
            if o.len() != 3 || b != c || b.es != a.es + 1 || b.es > 3 || b.n as u32 * (1 << b.es) != 16 || (a.n as u32 * (1 << a.es) == 16) != two {
                return bad();
            }
            w(ctx, base | (two as u32) << 30 | (a.es as u32) << 22 | rm.n() << 16 | rn.n() << 5 | rd.n());
            return Ok(true);
        }
    }
    for &(name, base) in NARROW_SHIFT {
        let two = mn.len() == name.len() + 1 && mn.ends_with('2') && mn.starts_with(name);
        if mn == name || two {
            let (rd, a) = vr(o.first())?;
            let (rn, b) = vr(o.get(1))?;
            if o.len() != 3 || b.es != a.es + 1 || a.es > 2 || b.n as u32 * (1 << b.es) != 16 || (a.n as u32 * (1 << a.es) == 16) != two {
                return bad();
            }
            let esize = 8i64 << a.es;
            let sh = range(imm(ctx, o.get(2))?, 1, esize, "shift amount")?;
            w(ctx, base | (two as u32) << 30 | (2 * esize as u32 - sh) << 16 | rn.n() << 5 | rd.n());
            return Ok(true);
        }
    }
    match mn {
        "aese" | "aesd" | "aesmc" | "aesimc" => {
            let (rd, a) = vr(o.first())?;
            let (rn, b) = vr(o.get(1))?;
            if o.len() != 2 || a != (Arr { n: 16, es: 0 }) || b != a {
                return bad();
            }
            let base = match mn {
                "aese" => 0x4E28_4800,
                "aesd" => 0x4E28_5800,
                "aesmc" => 0x4E28_6800,
                _ => 0x4E28_7800,
            };
            w(ctx, base | rn.n() << 5 | rd.n());
            return Ok(true);
        }
        "sha1c" | "sha1p" | "sha1m" | "sha256h" | "sha256h2" => {
            let rd = fp_same(o.first(), K::Q)?;
            let rn = fpr(o.get(1))?;
            let (rm, c) = vr(o.get(2))?;
            if o.len() != 3 || c != (Arr { n: 4, es: 2 }) || rn.k != if mn.starts_with("sha1") { K::S } else { K::Q } {
                return bad();
            }
            let base = match mn {
                "sha1c" => 0x5E00_0000,
                "sha1p" => 0x5E00_1000,
                "sha1m" => 0x5E00_2000,
                "sha256h" => 0x5E00_4000,
                _ => 0x5E00_5000,
            };
            w(ctx, base | rm.n() << 16 | rn.n() << 5 | rd.n());
            return Ok(true);
        }
        "sha1su0" | "sha256su1" => {
            let (rd, a) = vr(o.first())?;
            let (rn, b) = vr(o.get(1))?;
            let (rm, c) = vr(o.get(2))?;
            if o.len() != 3 || a != (Arr { n: 4, es: 2 }) || b != a || c != a {
                return bad();
            }
            w(ctx, if mn == "sha1su0" { 0x5E00_3000 } else { 0x5E00_6000 } | rm.n() << 16 | rn.n() << 5 | rd.n());
            return Ok(true);
        }
        "sha1su1" | "sha256su0" => {
            let (rd, a) = vr(o.first())?;
            let (rn, b) = vr(o.get(1))?;
            if o.len() != 2 || a != (Arr { n: 4, es: 2 }) || b != a {
                return bad();
            }
            w(ctx, if mn == "sha1su1" { 0x5E28_1800 } else { 0x5E28_2800 } | rn.n() << 5 | rd.n());
            return Ok(true);
        }
        "sha1h" => {
            let rd = fp_same(o.first(), K::S)?;
            let rn = fp_same(o.get(1), K::S)?;
            if o.len() != 2 {
                return bad();
            }
            w(ctx, 0x5E28_0800 | rn.n() << 5 | rd.n());
            return Ok(true);
        }
        "fjcvtzs" => {
            let rd = gp_k(o.first(), K::W)?;
            let rn = fp_same(o.get(1), K::D)?;
            if o.len() != 2 {
                return bad();
            }
            w(ctx, 0x1E7E_0000 | rn.n() << 5 | rd.n());
            return Ok(true);
        }
        "pmull" | "pmull2" => {
            let (rd, a) = vr(o.first())?;
            let (rn, b) = vr(o.get(1))?;
            let (rm, c) = vr(o.get(2))?;
            let two = mn.ends_with('2');
            if o.len() != 3 || b != c {
                return bad();
            }
            let size = match (a, b.es) {
                (Arr { n: 8, es: 1 }, 0) => 0,
                (Arr { n: 1, es: 4 }, 3) => 3,
                _ => return bad(),
            };
            if (b.n as u32 * (1 << b.es) == 16) != two {
                return bad();
            }
            w(ctx, 0x0E20_E000 | (two as u32) << 30 | size << 22 | rm.n() << 16 | rn.n() << 5 | rd.n());
            return Ok(true);
        }
        "sdot" | "udot" => {
            let (rd, a) = vr(o.first())?;
            let (rn, b) = vr(o.get(1))?;
            let (rm, c) = vr(o.get(2))?;
            if o.len() != 3 || b != c || b.es != 0 || a.es != 2 || a.n * 4 != b.n {
                return bad();
            }
            w(ctx, if mn == "sdot" { 0x0E80_9400 } else { 0x2E80_9400 } | a.q() << 30 | rm.n() << 16 | rn.n() << 5 | rd.n());
            return Ok(true);
        }
        "sqrdmlah" | "sqrdmlsh" => {
            let (rd, a) = vr(o.first())?;
            let (rn, b) = vr(o.get(1))?;
            let (rm, c) = vr(o.get(2))?;
            if o.len() != 3 || a != b || b != c || !(a.es == 1 || a.es == 2) {
                return bad();
            }
            w(ctx, if mn == "sqrdmlah" { 0x2E00_8400 } else { 0x2E00_8C00 } | a.q() << 30 | (a.es as u32) << 22 | rm.n() << 16 | rn.n() << 5 | rd.n());
            return Ok(true);
        }
        "urecpe" | "ursqrte" => {
            let (rd, a) = vr(o.first())?;
            let (rn, b) = vr(o.get(1))?;
            if o.len() != 2 || a != b || a.es != 2 {
                return bad();
            }
            w(ctx, if mn == "urecpe" { 0x0EA1_C800 } else { 0x2EA1_C800 } | a.q() << 30 | rn.n() << 5 | rd.n());
            return Ok(true);
        }
        _ => {}
    }
    // sqshl / uqshl / sqrshl / uqrshl with registers (the immediate forms are in SHIFT_IMM)
    let reg3 = match mn {
        "sqshl" => Some(0x0E20_4C00),
        "uqshl" => Some(0x2E20_4C00),
        "sqrshl" => Some(0x0E20_5C00),
        "uqrshl" => Some(0x2E20_5C00),
        _ => None,
    };
    if let Some(base) = reg3 {
        let (rd, a) = vr(o.first())?;
        let (rn, b) = vr(o.get(1))?;
        let (rm, c) = vr(o.get(2))?;
        if o.len() != 3 || a != b || b != c || !arr_ok(M::All, a) {
            return bad();
        }
        w(ctx, base | a.q() << 30 | (a.es as u32) << 22 | rm.n() << 16 | rn.n() << 5 | rd.n());
        return Ok(true);
    }
    // half-precision vectors and float compares with zero
    for &(name, base, half) in FCMP_ZERO {
        if name == mn && is_vec_op(o.first()) && is_imm(o.get(2)) {
            let (rd, a) = vr(o.first())?;
            let (rn, b) = vr(o.get(1))?;
            if o.len() != 3 || a != b || fp_lit(ctx, o.get(2))? != 0.0 {
                return bad();
            }
            let word = match (a.es, a.n) {
                (2, 2) | (2, 4) => base | a.q() << 30,
                (3, 2) => base | 1 << 30 | 1 << 22,
                (1, 4) | (1, 8) => half | a.q() << 30,
                _ => return bad(),
            };
            w(ctx, word | rn.n() << 5 | rd.n());
            return Ok(true);
        }
    }
    for &(name, half) in TWO_F16 {
        if name == mn && is_vec_op(o.first()) && o.len() == 2 {
            let (rd, a) = vr(o.first())?;
            let (rn, b) = vr(o.get(1))?;
            if a != b || a.es != 1 || !(a.n == 4 || a.n == 8) {
                return bad();
            }
            w(ctx, half | a.q() << 30 | rn.n() << 5 | rd.n());
            return Ok(true);
        }
    }
    // fixed-point vector conversions
    if o.len() == 3 && is_imm(o.get(2)) {
        let fixed = match mn {
            "scvtf" => Some(0x0F00_E400),
            "ucvtf" => Some(0x2F00_E400),
            "fcvtzs" => Some(0x0F00_FC00),
            "fcvtzu" => Some(0x2F00_FC00),
            _ => None,
        };
        if let (Some(base), true) = (fixed, is_vec_op(o.first())) {
            let (rd, a) = vr(o.first())?;
            let (rn, b) = vr(o.get(1))?;
            if a != b || !matches!((a.n, a.es), (2, 2) | (4, 2) | (2, 3)) {
                return bad();
            }
            let esize = 8i64 << a.es;
            let fb = range(imm(ctx, o.get(2))?, 1, esize, "number of fractional bits")?;
            w(ctx, base | a.q() << 30 | (2 * esize as u32 - fb) << 16 | rn.n() << 5 | rd.n());
            return Ok(true);
        }
    }
    Ok(false)
}
