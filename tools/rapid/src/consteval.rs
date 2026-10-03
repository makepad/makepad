//! Small syntactic constant evaluation used before type checking (array lengths,
//! enum discriminants). Full `const` items are evaluated by running their JIT'ed
//! initializer (see jit.rs).

use crate::ast::{BinOp, ExprId, ExprKind, ItemKind, LitKind, UnOp};
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

pub fn eval_int_literal(prog: &Program, file: u32, e: ExprId) -> Option<i128> {
    let f = &prog.files[file as usize];
    let ex = f.ast.expr(e);
    match &ex.kind {
        ExprKind::Lit(LitKind::Int) => {
            let t = std::str::from_utf8(&f.src[ex.lo as usize..ex.hi as usize]).ok()?;
            parse_int_lit(t).map(|v| v as i128)
        }
        ExprKind::Unary(UnOp::Neg, x) => eval_int_literal(prog, file, *x).map(|v| -v),
        ExprKind::Paren(x) => eval_int_literal(prog, file, *x),
        ExprKind::Cast(x, _) => eval_int_literal(prog, file, *x),
        _ => None,
    }
}

/// Array lengths: literals, paths to consts with literal-like initializers, arithmetic.
pub fn eval_usize_expr(prog: &Program, tcx: &Tcx, file: u32, module: u32, e: ExprId) -> Option<u64> {
    eval_i(prog, tcx, file, module, e, 0).map(|v| v as u64)
}

fn eval_i(prog: &Program, tcx: &Tcx, file: u32, module: u32, e: ExprId, depth: u32) -> Option<i128> {
    if depth > 32 {
        return None;
    }
    let f = &prog.files[file as usize];
    let ex = f.ast.expr(e);
    match &ex.kind {
        ExprKind::Lit(LitKind::Int) => {
            let t = std::str::from_utf8(&f.src[ex.lo as usize..ex.hi as usize]).ok()?;
            parse_int_lit(t).map(|v| v as i128)
        }
        ExprKind::Paren(x) | ExprKind::Cast(x, _) => eval_i(prog, tcx, file, module, *x, depth + 1),
        ExprKind::Block(b, _) => {
            let blk = f.ast.block(*b);
            if blk.stmts.len() == 1 {
                if let crate::ast::Stmt::Expr(x, false) = blk.stmts[0] {
                    return eval_i(prog, tcx, file, module, x, depth + 1);
                }
            }
            None
        }
        ExprKind::Unary(UnOp::Neg, x) => eval_i(prog, tcx, file, module, *x, depth + 1).map(|v| -v),
        ExprKind::Binary(op, a, b) => {
            let a = eval_i(prog, tcx, file, module, *a, depth + 1)?;
            let b = eval_i(prog, tcx, file, module, *b, depth + 1)?;
            Some(match op {
                BinOp::Add => a + b,
                BinOp::Sub => a - b,
                BinOp::Mul => a * b,
                BinOp::Div => {
                    if b == 0 {
                        return None;
                    }
                    a / b
                }
                BinOp::Rem => {
                    if b == 0 {
                        return None;
                    }
                    a % b
                }
                BinOp::Shl => a << b,
                BinOp::Shr => a >> b,
                BinOp::BitAnd => a & b,
                BinOp::BitOr => a | b,
                _ => return None,
            })
        }
        ExprKind::Path(p) => {
            let segs = crate::tcx::path_segs(prog, file, p);
            let d = if segs.len() == 1 {
                prog.lookup_in_scope(module, segs[0].1, false)?
            } else {
                let parent = prog.resolve_mod_path(module, p.global, &segs[..segs.len() - 1], false, false)?;
                prog.lookup_in_container(parent, segs[segs.len() - 1].1, false)?
            };
            let def = prog.def(d);
            if def.kind != DefKind::Const {
                return None;
            }
            let ast = &prog.files[def.file as usize].ast;
            if let ItemKind::Const(_, Some(init)) = &ast.item(def.item).kind {
                return eval_i(prog, tcx, def.file, def.scope, *init, depth + 1);
            }
            None
        }
        _ => None,
    }
}
