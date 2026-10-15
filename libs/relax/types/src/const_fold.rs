// ported from N:rustc_const_eval/src/interpret/operator.rs @8d1a764 (round-30 restricted scalar folding)
//! Constant-folding fast path. Expressions beyond folding are evaluated through
//! the const_eval_resolve provider by compiling and running their initializer.

use crate::ast::{BinOp, ExprId, ExprKind, LitKind, UnOp};
use crate::program::{DefKind, Program};
use crate::tcx::Tcx;

pub fn parse_int_lit(text: &str) -> Option<u128> {
    let mut s = String::new();
    for c in text.chars() {
        if c != '_' {
            s.push(c);
        }
    }
    // strip suffix
    let (radix, digits) = if let Some(x) = s.strip_prefix("0x") {
        (16, x.to_string())
    } else if let Some(x) = s.strip_prefix("0o") {
        (8, x.to_string())
    } else if let Some(x) = s.strip_prefix("0b") {
        (2, x.to_string())
    } else {
        (10, s.clone())
    };
    let mut end = digits.len();
    for (i, c) in digits.char_indices() {
        let ok = match radix {
            16 => c.is_ascii_hexdigit(),
            _ => c.is_ascii_digit(),
        };
        if !ok {
            end = i;
            break;
        }
    }
    u128::from_str_radix(&digits[..end], radix).ok()
}

/// Integer suffix of a literal (`u8`, `usize`...), if any.
pub fn int_suffix(text: &str) -> Option<&str> {
    let hex = text.starts_with("0x");
    for suf in ["u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize", "f32", "f64"] {
        if text.ends_with(suf) {
            // hex digits can end in `f32`-like text only if that is a real suffix: rare, accept
            if hex && suf.starts_with('f') {
                continue;
            }
            return Some(suf);
        }
    }
    None
}

/// Round-30 design, N:rustc_const_eval/src/interpret/operator.rs:114-240:
/// fold only operations whose operand types are known from the declared scalar
/// type. Casts, calls and associated paths use the typed executable provider;
/// named const items recurse through const_eval_resolve.
pub(crate) fn eval_query_expr(cx: &crate::tcx::EvalCx<'_>, prog: &Program, tcx: &Tcx,
    file: u32, module: u32, e: ExprId, ty: crate::types::TyId,
) -> Option<i128> {
    use crate::types::TyKind;
    let f = &prog.files[file as usize];
    let ex = f.ast.expr(e);
    let (bytes, signed) = match tcx.tys.kind(ty) {
        TyKind::Int(p) => crate::types::Types::int_info(*p),
        TyKind::Bool => (1, false),
        TyKind::Char => (4, false),
        _ => return None,
    };
    let boolean = matches!(tcx.tys.kind(ty), TyKind::Bool);
    let integral = matches!(tcx.tys.kind(ty), TyKind::Int(_));
    let bits = bytes * 8;
    let mask = u128::MAX >> (128 - bits);
    let fits = |v: i128| wrap_int(v, bytes, signed) == v;
    match &ex.kind {
        ExprKind::Paren(x) => eval_query_expr(cx, prog, tcx, file, module, *x, ty),
        ExprKind::Block(block, _) | ExprKind::ConstBlock(block) => match f.ast.stmts(*block) {
            [crate::ast::Stmt::Expr(x, false)] => eval_query_expr(cx, prog, tcx, file, module, *x, ty),
            _ => None,
        },
        ExprKind::Lit(LitKind::Int) if integral => {
            let text = std::str::from_utf8(f.bytes(ex.lo, ex.hi)).ok()?;
            if let Some(suffix) = int_suffix(text) {
                if prim_int(suffix) != Some((bytes, signed)) { return None; }
            }
            let value = parse_int_lit(text)?;
            let max = if signed { mask >> 1 } else { mask };
            if value > max { return None; }
            Some(value as i128)
        }
        ExprKind::Lit(LitKind::Bool(value)) if boolean => Some(*value as i128),
        ExprKind::Lit(LitKind::Char) if matches!(tcx.tys.kind(ty), TyKind::Char) => {
            let text = std::str::from_utf8(f.bytes(ex.lo, ex.hi)).ok()?;
            Some(crate::lit::unescape_str(text).chars().next()? as i128)
        }
        ExprKind::Unary(UnOp::Neg, x) if integral && signed => {
            // Negative literals include MIN; THIR folds their sign into the literal.
            let mut literal = *x;
            while let ExprKind::Paren(inner) = f.ast.expr(literal).kind { literal = inner; }
            let operand = f.ast.expr(literal);
            if matches!(operand.kind, ExprKind::Lit(LitKind::Int)) {
                let text = std::str::from_utf8(f.bytes(operand.lo, operand.hi)).ok()?;
                if let Some(suffix) = int_suffix(text) {
                    if prim_int(suffix) != Some((bytes, signed)) { return None; }
                }
                let value = parse_int_lit(text)?;
                if value > (1u128 << (bits - 1)) { return None; }
                return Some(wrap_int((value as i128).wrapping_neg(), bytes, true));
            }
            let value = eval_query_expr(cx, prog, tcx, file, module, *x, ty)?.checked_neg()?;
            if fits(value) { Some(value) } else { None }
        }
        ExprKind::Unary(UnOp::Not, x) if boolean || integral => {
            let value = eval_query_expr(cx, prog, tcx, file, module, *x, ty)?;
            Some(if boolean { value ^ 1 } else { wrap_int(!value, bytes, signed) })
        }
        ExprKind::Binary(op, a, b) if integral || boolean => {
            let a = eval_query_expr(cx, prog, tcx, file, module, *a, ty)?;
            let b = eval_query_expr(cx, prog, tcx, file, module, *b, ty)?;
            if boolean {
                return match op {
                    BinOp::BitAnd | BinOp::And => Some(a & b),
                    BinOp::BitOr | BinOp::Or => Some(a | b),
                    BinOp::BitXor => Some(a ^ b),
                    _ => None,
                };
            }
            let value = match op {
                BinOp::BitAnd => a & b,
                BinOp::BitOr => a | b,
                BinOp::BitXor => a ^ b,
                BinOp::Shl | BinOp::Shr => {
                    if b < 0 || b >= bits as i128 { return None; }
                    return Some(wrap_int(if *op == BinOp::Shl { a << b }
                        else if signed { a >> b } else { ((a as u128) >> b) as i128 }, bytes, signed));
                }
                BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
                    if signed {
                        if matches!(op, BinOp::Div | BinOp::Rem) && b == -1 && a == wrap_int(1i128 << (bits - 1), bytes, true) { return None; }
                        match op {
                            BinOp::Add => a.checked_add(b)?, BinOp::Sub => a.checked_sub(b)?,
                            BinOp::Mul => a.checked_mul(b)?, BinOp::Div => a.checked_div(b)?,
                            BinOp::Rem => a.checked_rem(b)?, _ => unreachable!(),
                        }
                    } else {
                        let (a, b) = (a as u128, b as u128);
                        let result = match op {
                            BinOp::Add => a.checked_add(b)?, BinOp::Sub => a.checked_sub(b)?,
                            BinOp::Mul => a.checked_mul(b)?, BinOp::Div => a.checked_div(b)?,
                            BinOp::Rem => a.checked_rem(b)?, _ => unreachable!(),
                        };
                        if result > mask { return None; }
                        result as i128
                    }
                }
                _ => return None,
            };
            if fits(value) { Some(value) } else { None }
        }
        ExprKind::Path(path) if path.no_qself() => {
            let segments = crate::program::path_segs(prog, file, path);
            if segments.is_empty() { return None; }
            if !path.global() && segments.len() == 1 {
                let frame = cx.state.const_generic_frame.borrow();
                if let Some((env, args)) = frame.as_ref() {
                    if let Some(index) = env.names.iter().position(|n| *n == segments[0].1) {
                        if let Some(arg) = args.get(index) {
                            if let TyKind::Const(ct) = tcx.tys.kind(*arg) {
                                if let Some(value) = ct.try_to_value(&tcx.tys) {
                                    if value.ty != ty { return None; }
                                    let leaf = value.valtree.try_to_leaf()?;
                                    return Some(wrap_int(leaf.to_bits(leaf.size()) as i128, bytes, signed));
                                }
                                if matches!(tcx.tys.const_kind(*ct), crate::consts::ConstKind::Error) { return None; }
                            }
                        }
                        cx.state.const_too_generic.set(true);
                        return None;
                    }
                }
            }
            let def = if segments.len() == 1 {
                prog.lookup_in_scope(module, segments[0].1, false)?
            } else {
                let container = prog.resolve_mod_path(module, path.global(), &segments[..segments.len() - 1], false, false)?;
                prog.lookup_in_container(container, segments[segments.len() - 1].1, false)?
            };
            if prog.def(def).kind != DefKind::Const || tcx.type_of(prog, def).ok()? != ty { return None; }
            let value = if cx.state.fold_only.get() {
                crate::const_eval::fold_without_execution(tcx, prog, cx.typing_env, def)?.ok()?
            } else { tcx.const_eval_resolve(prog, cx.typing_env, def, &[]).ok()? };
            let leaf = value.try_to_leaf()?;
            Some(wrap_int(leaf.to_bits(leaf.size()) as i128, bytes, signed))
        }
        _ => None,
    }
}

fn wrap_int(v: i128, bytes: u32, signed: bool) -> i128 {
    if bytes >= 16 {
        return v;
    }
    let bits = bytes * 8;
    let m = (1i128 << bits) - 1;
    let u = v & m;
    if signed && u >> (bits - 1) & 1 == 1 {
        u - (1i128 << bits)
    } else {
        u
    }
}

fn prim_int(text: &str) -> Option<(u32, bool)> {
    let p = crate::program::PRIMS.iter().find(|(n, _)| *n == text)?.1;
    match crate::types::Types::int_info(p) {
        (0, _) => None,
        x => Some(x),
    }
}
