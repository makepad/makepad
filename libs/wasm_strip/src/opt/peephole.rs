//! Local rewrites that only ever shrink a body:
//! - code after an unconditional branch, up to the end of its block, goes;
//! - a `block`/`loop` no branch targets is unwrapped (inner depths fixed);
//! - stores to a local nothing reads become `drop`s (a `tee` just vanishes);
//! - window rules: `local.set x; local.get x` → `local.tee x`,
//!   `local.tee x; drop` → `local.set x`, a pure value then `drop` → nothing,
//!   `local.get x; local.set x` → nothing, integer constant folding and
//!   identities (`x + 0`, `x * 1`, `x & -1`, …), double `eqz` before a test,
//!   `br_if` on a constant, `nop`.

use super::ir::*;

pub fn run(module: &mut Module) {
    let types = module.types.clone();
    for func in &mut module.funcs {
        let num_params = types[func.ty as usize].params.len();
        optimize_func(func, num_params);
    }
}

pub fn optimize_func(func: &mut Func, num_params: usize) {
    unwrap_blocks(&mut func.body);
    loop {
        let before = func.body.len();
        drop_dead_stores(func, num_params);
        func.body = rewrite(std::mem::take(&mut func.body));
        if func.body.len() == before {
            break;
        }
    }
}

fn is_unconditional(instr: &Instr) -> bool {
    matches!(
        instr,
        Instr::Unreachable
            | Instr::Br(_)
            | Instr::BrTable(..)
            | Instr::Return
            | Instr::ReturnCall(_)
            | Instr::ReturnCallIndirect { .. }
    )
}

/// Instructions without inputs or effects that push one value.
fn is_pure_value(instr: &Instr) -> bool {
    matches!(
        instr,
        Instr::LocalGet(_)
            | Instr::GlobalGet(_)
            | Instr::I32Const(_)
            | Instr::I64Const(_)
            | Instr::F32Const(_)
            | Instr::F64Const(_)
            | Instr::V128Const(_)
            | Instr::RefNull(_)
            | Instr::RefFunc(_)
    )
}

fn depths_mut(instr: &mut Instr, f: &mut impl FnMut(&mut u32)) {
    match instr {
        Instr::Br(depth) | Instr::BrIf(depth) => f(depth),
        Instr::BrTable(targets, default) => {
            for target in targets.iter_mut() {
                f(target);
            }
            f(default);
        }
        _ => {}
    }
}

/// Removes `block`s and `loop`s that no branch targets.
pub fn unwrap_blocks(body: &mut Vec<Instr>) {
    let mut targeted = vec![false; body.len()];
    let mut stack: Vec<usize> = Vec::new();
    for (i, instr) in body.iter_mut().enumerate() {
        match instr {
            Instr::Block(_) | Instr::Loop(_) | Instr::If(_) => stack.push(i),
            Instr::End => {
                stack.pop();
            }
            _ => {
                let len = stack.len();
                depths_mut(instr, &mut |depth| {
                    if let Some(at) = len.checked_sub(1 + *depth as usize) {
                        targeted[stack[at]] = true;
                    }
                });
            }
        }
    }
    let removable = |i: usize, instr: &Instr| {
        matches!(instr, Instr::Block(_) | Instr::Loop(_)) && !targeted[i]
    };
    if !body.iter().enumerate().any(|(i, instr)| removable(i, instr)) {
        return;
    }
    let mut out = Vec::with_capacity(body.len());
    let mut removed: Vec<bool> = Vec::new();
    for (i, mut instr) in std::mem::take(body).into_iter().enumerate() {
        match instr {
            Instr::Block(_) | Instr::Loop(_) | Instr::If(_) => {
                let remove = removable(i, &instr);
                removed.push(remove);
                if remove {
                    continue;
                }
            }
            Instr::End => {
                if removed.pop() == Some(true) {
                    continue;
                }
            }
            _ => {
                let len = removed.len();
                depths_mut(&mut instr, &mut |depth| {
                    let d = (*depth as usize).min(len);
                    let skipped = removed[len - d..].iter().filter(|r| **r).count();
                    *depth -= skipped as u32;
                });
            }
        }
        out.push(instr);
    }
    *body = out;
}

fn drop_dead_stores(func: &mut Func, num_params: usize) {
    let num_locals = num_params + func.locals.len();
    let mut read = vec![false; num_locals];
    for instr in &func.body {
        if let Instr::LocalGet(index) = instr {
            read[*index as usize] = true;
        }
    }
    if read.iter().all(|read| *read) {
        return;
    }
    func.body.retain_mut(|instr| match instr {
        Instr::LocalSet(index) if !read[*index as usize] => {
            *instr = Instr::Drop;
            true
        }
        Instr::LocalTee(index) => read[*index as usize],
        _ => true,
    });
}

fn fold_i32_bin(op: u8, a: i32, b: i32) -> Option<Instr> {
    let (ua, ub) = (a as u32, b as u32);
    let bool32 = |v: bool| Instr::I32Const(v as i32);
    Some(match op {
        0x46 => bool32(a == b),
        0x47 => bool32(a != b),
        0x48 => bool32(a < b),
        0x49 => bool32(ua < ub),
        0x4a => bool32(a > b),
        0x4b => bool32(ua > ub),
        0x4c => bool32(a <= b),
        0x4d => bool32(ua <= ub),
        0x4e => bool32(a >= b),
        0x4f => bool32(ua >= ub),
        0x6a => Instr::I32Const(a.wrapping_add(b)),
        0x6b => Instr::I32Const(a.wrapping_sub(b)),
        0x6c => Instr::I32Const(a.wrapping_mul(b)),
        0x6d if b != 0 && !(a == i32::MIN && b == -1) => Instr::I32Const(a / b),
        0x6e if b != 0 => Instr::I32Const((ua / ub) as i32),
        0x6f if b != 0 => Instr::I32Const(a.wrapping_rem(b)),
        0x70 if b != 0 => Instr::I32Const((ua % ub) as i32),
        0x71 => Instr::I32Const(a & b),
        0x72 => Instr::I32Const(a | b),
        0x73 => Instr::I32Const(a ^ b),
        0x74 => Instr::I32Const(a.wrapping_shl(ub)),
        0x75 => Instr::I32Const(a.wrapping_shr(ub)),
        0x76 => Instr::I32Const(ua.wrapping_shr(ub) as i32),
        0x77 => Instr::I32Const(ua.rotate_left(ub % 32) as i32),
        0x78 => Instr::I32Const(ua.rotate_right(ub % 32) as i32),
        _ => return None,
    })
}

fn fold_i64_bin(op: u8, a: i64, b: i64) -> Option<Instr> {
    let (ua, ub) = (a as u64, b as u64);
    let bool32 = |v: bool| Instr::I32Const(v as i32);
    Some(match op {
        0x51 => bool32(a == b),
        0x52 => bool32(a != b),
        0x53 => bool32(a < b),
        0x54 => bool32(ua < ub),
        0x55 => bool32(a > b),
        0x56 => bool32(ua > ub),
        0x57 => bool32(a <= b),
        0x58 => bool32(ua <= ub),
        0x59 => bool32(a >= b),
        0x5a => bool32(ua >= ub),
        0x7c => Instr::I64Const(a.wrapping_add(b)),
        0x7d => Instr::I64Const(a.wrapping_sub(b)),
        0x7e => Instr::I64Const(a.wrapping_mul(b)),
        0x7f if b != 0 && !(a == i64::MIN && b == -1) => Instr::I64Const(a / b),
        0x80 if b != 0 => Instr::I64Const((ua / ub) as i64),
        0x81 if b != 0 => Instr::I64Const(a.wrapping_rem(b)),
        0x82 if b != 0 => Instr::I64Const((ua % ub) as i64),
        0x83 => Instr::I64Const(a & b),
        0x84 => Instr::I64Const(a | b),
        0x85 => Instr::I64Const(a ^ b),
        0x86 => Instr::I64Const(a.wrapping_shl(ub as u32)),
        0x87 => Instr::I64Const(a.wrapping_shr(ub as u32)),
        0x88 => Instr::I64Const(ua.wrapping_shr(ub as u32) as i64),
        0x89 => Instr::I64Const(ua.rotate_left((ub % 64) as u32) as i64),
        0x8a => Instr::I64Const(ua.rotate_right((ub % 64) as u32) as i64),
        _ => return None,
    })
}

fn fold_un(op: u8, value: &Instr) -> Option<Instr> {
    Some(match (op, value) {
        (0x45, Instr::I32Const(a)) => Instr::I32Const((*a == 0) as i32),
        (0x67, Instr::I32Const(a)) => Instr::I32Const(a.leading_zeros() as i32),
        (0x68, Instr::I32Const(a)) => Instr::I32Const(a.trailing_zeros() as i32),
        (0x69, Instr::I32Const(a)) => Instr::I32Const(a.count_ones() as i32),
        (0xc0, Instr::I32Const(a)) => Instr::I32Const(*a as i8 as i32),
        (0xc1, Instr::I32Const(a)) => Instr::I32Const(*a as i16 as i32),
        (0xac, Instr::I32Const(a)) => Instr::I64Const(*a as i64),
        (0xad, Instr::I32Const(a)) => Instr::I64Const(*a as u32 as i64),
        (0x50, Instr::I64Const(a)) => Instr::I32Const((*a == 0) as i32),
        (0x79, Instr::I64Const(a)) => Instr::I64Const(a.leading_zeros() as i64),
        (0x7a, Instr::I64Const(a)) => Instr::I64Const(a.trailing_zeros() as i64),
        (0x7b, Instr::I64Const(a)) => Instr::I64Const(a.count_ones() as i64),
        (0xa7, Instr::I64Const(a)) => Instr::I32Const(*a as i32),
        (0xc2, Instr::I64Const(a)) => Instr::I64Const(*a as i8 as i64),
        (0xc3, Instr::I64Const(a)) => Instr::I64Const(*a as i16 as i64),
        (0xc4, Instr::I64Const(a)) => Instr::I64Const(*a as i32 as i64),
        _ => return None,
    })
}

/// `x op c` that leaves `x` unchanged.
fn is_identity(op: u8, constant: &Instr) -> bool {
    match constant {
        Instr::I32Const(c) => match op {
            0x6a | 0x6b | 0x72 | 0x73 | 0x74 | 0x75 | 0x76 | 0x77 | 0x78 => *c == 0,
            0x6c | 0x6d | 0x6e => *c == 1,
            0x71 => *c == -1,
            _ => false,
        },
        Instr::I64Const(c) => match op {
            0x7c | 0x7d | 0x84 | 0x85 | 0x86 | 0x87 | 0x88 | 0x89 | 0x8a => *c == 0,
            0x7e | 0x7f | 0x80 => *c == 1,
            0x83 => *c == -1,
            _ => false,
        },
        _ => false,
    }
}

/// Applies the window rules to the tail of `out` until none matches.
fn reduce_tail(out: &mut Vec<Instr>) {
    loop {
        let n = out.len();
        let changed = match out.as_slice() {
            [.., Instr::Nop] => {
                out.pop();
                true
            }
            [.., Instr::LocalSet(a), Instr::LocalGet(b)] if a == b => {
                let index = *a;
                out.truncate(n - 2);
                out.push(Instr::LocalTee(index));
                true
            }
            [.., Instr::LocalTee(a), Instr::Drop] => {
                let index = *a;
                out.truncate(n - 2);
                out.push(Instr::LocalSet(index));
                true
            }
            [.., Instr::LocalTee(a), Instr::LocalSet(b)] if a == b => {
                let index = *a;
                out.truncate(n - 2);
                out.push(Instr::LocalSet(index));
                true
            }
            [.., Instr::LocalGet(a), Instr::LocalSet(b)] if a == b => {
                out.truncate(n - 2);
                true
            }
            [.., value, Instr::Drop] if is_pure_value(value) => {
                out.truncate(n - 2);
                true
            }
            [.., a, b, Instr::Num(op)] if matches!((a, b), (Instr::I32Const(_), Instr::I32Const(_)) | (Instr::I64Const(_), Instr::I64Const(_))) => {
                let folded = match (a, b) {
                    (Instr::I32Const(a), Instr::I32Const(b)) => fold_i32_bin(*op, *a, *b),
                    (Instr::I64Const(a), Instr::I64Const(b)) => fold_i64_bin(*op, *a, *b),
                    _ => None,
                };
                match folded {
                    Some(folded) => {
                        out.truncate(n - 3);
                        out.push(folded);
                        true
                    }
                    None => false,
                }
            }
            [.., value, Instr::Num(op)] if fold_un(*op, value).is_some() => {
                let folded = fold_un(*op, value).unwrap();
                out.truncate(n - 2);
                out.push(folded);
                true
            }
            [.., constant, Instr::Num(op)] if is_identity(*op, constant) => {
                out.truncate(n - 2);
                true
            }
            [.., Instr::Num(0x45), Instr::Num(0x45), Instr::BrIf(_) | Instr::If(_)] => {
                let test = out.pop().unwrap();
                out.truncate(n - 3);
                out.push(test);
                true
            }
            [.., Instr::I32Const(c), Instr::BrIf(depth)] => {
                let (c, depth) = (*c, *depth);
                out.truncate(n - 2);
                if c != 0 {
                    out.push(Instr::Br(depth));
                }
                true
            }
            _ => false,
        };
        if !changed {
            return;
        }
    }
}

fn rewrite(body: Vec<Instr>) -> Vec<Instr> {
    let mut out: Vec<Instr> = Vec::with_capacity(body.len());
    // Inside dead code: how many blocks deep below the dead point we are.
    let mut dead: Option<usize> = None;
    for instr in body {
        if let Some(depth) = dead.as_mut() {
            match instr {
                Instr::Block(_) | Instr::Loop(_) | Instr::If(_) => {
                    *depth += 1;
                    continue;
                }
                Instr::Else | Instr::End if *depth == 0 => dead = None,
                Instr::End => {
                    *depth -= 1;
                    continue;
                }
                _ => continue,
            }
        }
        out.push(instr);
        reduce_tail(&mut out);
        if out.last().map_or(false, is_unconditional) {
            dead = Some(0);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn func(body: Vec<Instr>, locals: Vec<ValType>) -> Func {
        Func { ty: 0, locals, body }
    }

    #[test]
    fn folds_and_drops() {
        let mut f = func(
            vec![
                Instr::I32Const(2),
                Instr::I32Const(3),
                Instr::Num(0x6c),
                Instr::I32Const(0),
                Instr::Num(0x6a),
                Instr::End,
            ],
            vec![],
        );
        optimize_func(&mut f, 0);
        assert_eq!(f.body, vec![Instr::I32Const(6), Instr::End]);

        let mut f = func(
            vec![Instr::I32Const(7), Instr::I32Const(0), Instr::Num(0x6d), Instr::Drop, Instr::End],
            vec![],
        );
        optimize_func(&mut f, 0);
        // Division by zero traps: it stays.
        assert_eq!(f.body.len(), 5);
    }

    #[test]
    fn locals() {
        let mut f = func(
            vec![
                Instr::LocalGet(0),
                Instr::LocalSet(1),
                Instr::LocalGet(1),
                Instr::LocalSet(2),
                Instr::LocalGet(1),
                Instr::End,
            ],
            vec![ValType::I32, ValType::I32],
        );
        optimize_func(&mut f, 1);
        // Local 2 is never read, so its store goes; then set/get of local 1
        // becomes a tee of a local nothing else reads, which goes too.
        assert_eq!(f.body, vec![Instr::LocalGet(0), Instr::End]);
    }

    #[test]
    fn dead_code_and_blocks() {
        let mut f = func(
            vec![
                Instr::Block(BlockType::Empty),
                Instr::Block(BlockType::Empty),
                Instr::LocalGet(0),
                Instr::BrIf(1),
                Instr::Br(0),
                Instr::I32Const(1),
                Instr::Drop,
                Instr::End,
                Instr::End,
                Instr::End,
            ],
            vec![],
        );
        optimize_func(&mut f, 1);
        assert_eq!(
            f.body,
            vec![
                Instr::Block(BlockType::Empty),
                Instr::Block(BlockType::Empty),
                Instr::LocalGet(0),
                Instr::BrIf(1),
                Instr::Br(0),
                Instr::End,
                Instr::End,
                Instr::End,
            ]
        );

        // An untargeted block goes and the branch through it is renumbered.
        let mut f = func(
            vec![
                Instr::Block(BlockType::Empty),
                Instr::Block(BlockType::Empty),
                Instr::LocalGet(0),
                Instr::BrIf(1),
                Instr::End,
                Instr::End,
                Instr::End,
            ],
            vec![],
        );
        optimize_func(&mut f, 1);
        assert_eq!(
            f.body,
            vec![
                Instr::Block(BlockType::Empty),
                Instr::LocalGet(0),
                Instr::BrIf(0),
                Instr::End,
                Instr::End,
            ]
        );
    }
}
