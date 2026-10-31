//! C signatures from Rust types (the classification itself is relax-codegen's cabi).

use relax_backend::rir::{CAgg, CArg, CConv, CSig, Mem};
use crate::types::{Abi, TyId, TyKind};

fn mem_size(m: Mem) -> u32 {
    match m {
        Mem::Int(b, _) => b as u32,
        Mem::F32 => 4,
        Mem::F64 => 8,
    }
}

fn round_up(x: u32, a: u32) -> u32 {
    if a <= 1 {
        x
    } else {
        (x + a - 1) / a * a
    }
}

// ------------------------------------------------------------ CSig from Rust types

/// Flattens the scalar fields of `t` (structs, tuples, arrays; at most 64 fields).
fn flatten(u: &crate::jit::Unit, t: TyId, base: u32, out: &mut Vec<(u32, Mem)>) -> bool {
    if out.len() > 64 {
        return false;
    }
    let l = u.layout_of(t);
    let k = u.tcx.tys.kind(t).clone();
    match k {
        TyKind::Array(e, n) => {
            let n = n.try_to_target_usize(&u.tcx.tys).expect("normalized array length");
            let el = u.layout_of(e);
            let stride = round_up(el.size, el.align);
            for i in 0..n as u32 {
                if !flatten(u, e, base + i * stride, out) {
                    return false;
                }
            }
            true
        }
        TyKind::Tuple(v) => {
            for (i, ft) in u.tcx.tys.list(v).iter().enumerate() {
                if !flatten(u, *ft, base + l.fields[i], out) {
                    return false;
                }
            }
            true
        }
        TyKind::Adt(d, args, _) => {
            let adt = match u.tcx.adts.get(&d) {
                Some(a) => a.clone(),
                None => return true,
            };
            if adt.is_enum {
                // a null-pointer niche enum is its dataful variant's fields (Option<fn()> is a
                // nullable pointer to C)
                if let (crate::layout::TagEnc::Niche { untagged: dvi, .. }, Some(_)) = (l.enc, &l.leaves) {
                    for i in 0..adt.variants[dvi as usize].fields.len() {
                        let ft = u.tcx.field_ty(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), d, dvi, i as u32, u.tcx.tys.list(args));
                        let off = l.variant_fields[dvi as usize].get(i).copied().unwrap_or(0);
                        if !flatten(u, ft, base + off, out) {
                            return false;
                        }
                    }
                    return true;
                }
                // fieldless (C-like) enums are their tag
                if l.variant_fields.iter().all(|v| v.is_empty()) {
                    if let Some((_, m)) = l.tag {
                        out.push((base, m));
                        return true;
                    }
                }
                return false;
            }
            if adt.is_union {
                return false;
            }
            for i in 0..adt.variants[0].fields.len() {
                let ft = u.tcx.field_ty(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), d, 0, i as u32, u.tcx.tys.list(args));
                if !flatten(u, ft, base + l.fields[i], out) {
                    return false;
                }
            }
            true
        }
        _ => match &l.leaves {
            Some(lv) => {
                for x in lv {
                    out.push((base + x.off, x.mem));
                }
                true
            }
            None => false,
        },
    }
}

/// How `t` crosses a C boundary: a scalar (one register-sized leaf) or an aggregate.
pub fn c_arg(u: &crate::jit::Unit, t: TyId) -> CArg {
    let l = u.layout_of(t);
    if let Some(lv) = &l.leaves {
        if lv.len() == 1 && lv[0].off == 0 && l.size == mem_size(lv[0].mem) {
            return CArg::Scalar(lv[0].mem);
        }
    }
    let mut fields = Vec::new();
    if !flatten(u, t, 0, &mut fields) {
        fields.clear();
    }
    CArg::Agg(CAgg { size: l.size, align: l.align, fields })
}

/// C signature of a call or body with Rust parameter types `params` and result `ret`.
/// `n_fixed`: Some(n) for a variadic call with n fixed parameters (the rest of `params`
/// are the variadic arguments' types).
pub fn c_sig(u: &crate::jit::Unit, params: &[TyId], ret: TyId, n_fixed: Option<usize>, abi: Abi) -> CSig {
    let mut args = Vec::new();
    for p in params {
        let l = u.layout_of(*p);
        if l.size == 0 {
            continue; // zero-sized arguments are not passed
        }
        args.push(c_arg(u, *p));
    }
    let rl = u.layout_of(ret);
    let mut ret_narrow = None;
    let ret = if rl.size == 0 {
        None
    } else {
        match c_arg(u, ret) {
            CArg::Scalar(Mem::Int(n, signed)) => {
                if n < 8 {
                    ret_narrow = Some(relax_backend::rir::IntTy { bits: n * 8, signed });
                }
                None
            }
            CArg::Scalar(_) => None,
            CArg::Agg(a) => Some(a),
        }
    };
    let conv = match abi {
        Abi::SysV64 | Abi::SysV64Unwind => CConv::SysV64,
        Abi::Win64 | Abi::Win64Unwind => CConv::Win64,
        _ => CConv::Host,
    };
    CSig { conv, args, ret, n_fixed: match n_fixed {
        Some(n) => n as u32,
        None => u32::MAX,
    }, ret_narrow, unwinds: abi.unwinds() }
}

