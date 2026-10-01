//! Local coalescing and renumbering. Liveness is computed over the basic
//! blocks of the structured body; two locals of the same type that are never
//! live at once share one slot. A local read before any write on some path
//! relies on its zero initial value, so it interferes with every parameter
//! (parameters hold their argument there). Parameters keep their indices;
//! the remaining slots are numbered by type, most used first, so the hot
//! ones get one-byte indices and the declarations stay in few groups.

use super::ir::*;

pub fn run(module: &mut Module) {
    let types = module.types.clone();
    for func in &mut module.funcs {
        coalesce(func, &types[func.ty as usize].params);
    }
}

/// Above this many locals the interference matrix gets large; such a
/// function is only renumbered, not coalesced.
const MAX_COALESCE_LOCALS: usize = 8192;

struct Bits {
    words: usize,
    data: Vec<u64>,
}

impl Bits {
    fn new(rows: usize, bits: usize) -> Bits {
        let words = (bits + 63) / 64;
        Bits {
            words,
            data: vec![0; rows * words],
        }
    }

    fn row(&self, row: usize) -> &[u64] {
        &self.data[row * self.words..(row + 1) * self.words]
    }

    fn row_mut(&mut self, row: usize) -> &mut [u64] {
        &mut self.data[row * self.words..(row + 1) * self.words]
    }

    fn get(&self, row: usize, bit: usize) -> bool {
        self.data[row * self.words + bit / 64] & (1 << (bit % 64)) != 0
    }

    fn set(&mut self, row: usize, bit: usize) {
        self.data[row * self.words + bit / 64] |= 1 << (bit % 64);
    }
}

fn set_bit(row: &mut [u64], bit: usize) {
    row[bit / 64] |= 1 << (bit % 64);
}

fn clear_bit(row: &mut [u64], bit: usize) {
    row[bit / 64] &= !(1 << (bit % 64));
}

fn for_each_bit(row: &[u64], mut f: impl FnMut(usize)) {
    for (w, word) in row.iter().enumerate() {
        let mut word = *word;
        while word != 0 {
            let bit = word.trailing_zeros() as usize;
            f(w * 64 + bit);
            word &= word - 1;
        }
    }
}

fn is_control(instr: &Instr) -> bool {
    matches!(
        instr,
        Instr::Block(_)
            | Instr::Loop(_)
            | Instr::If(_)
            | Instr::Else
            | Instr::End
            | Instr::Br(_)
            | Instr::BrIf(_)
            | Instr::BrTable(..)
            | Instr::Return
            | Instr::Unreachable
            | Instr::ReturnCall(_)
            | Instr::ReturnCallIndirect { .. }
    )
}

/// Basic blocks of a body: `starts[b]..starts[b + 1]`, and successors.
struct Cfg {
    starts: Vec<usize>,
    succs: Vec<Vec<usize>>,
}

fn build_cfg(body: &[Instr]) -> Cfg {
    let n = body.len();
    // Matching end (and else) of every block start, and of every else.
    let mut end_of = vec![usize::MAX; n];
    let mut else_of = vec![usize::MAX; n];
    let mut stack: Vec<usize> = Vec::new();
    for (i, instr) in body.iter().enumerate() {
        match instr {
            Instr::Block(_) | Instr::Loop(_) | Instr::If(_) => stack.push(i),
            Instr::Else => {
                let start = *stack.last().unwrap();
                else_of[start] = i;
                stack.push(i);
            }
            Instr::End => {
                if let Some(mut start) = stack.pop() {
                    if matches!(body[start], Instr::Else) {
                        end_of[start] = i;
                        start = stack.pop().unwrap();
                    }
                    end_of[start] = i;
                }
            }
            _ => {}
        }
    }

    let mut leader = vec![false; n + 1];
    leader[0] = true;
    for (i, instr) in body.iter().enumerate() {
        if is_control(instr) {
            leader[i] = true;
            leader[i + 1] = true;
        }
    }
    let mut block_of = vec![0usize; n + 1];
    let mut starts = Vec::new();
    for i in 0..n {
        if leader[i] {
            starts.push(i);
        }
        block_of[i] = starts.len() - 1;
    }
    let num_blocks = starts.len();
    starts.push(n);

    let mut succs = vec![Vec::new(); num_blocks];
    // Enclosing block starts at each instruction.
    let mut frames: Vec<usize> = Vec::new();
    let target = |frames: &[usize], depth: u32| -> Option<usize> {
        let at = frames.len().checked_sub(1 + depth as usize)?;
        let start = frames[at];
        Some(if matches!(body[start], Instr::Loop(_)) {
            start + 1
        } else {
            end_of[start]
        })
    };
    for b in 0..num_blocks {
        // Only a block's last instruction can branch.
        let i = starts[b + 1] - 1;
        let instr = &body[i];
        let mut out: Vec<usize> = Vec::new();
        let next = |out: &mut Vec<usize>| {
            if i + 1 < n {
                out.push(i + 1)
            }
        };
        match instr {
            Instr::Block(_) | Instr::Loop(_) => next(&mut out),
            Instr::If(_) => {
                next(&mut out);
                if else_of[i] != usize::MAX {
                    out.push(else_of[i] + 1);
                } else {
                    out.push(end_of[i]);
                }
            }
            Instr::Else => out.push(end_of[i]),
            Instr::End => next(&mut out),
            Instr::Br(depth) => out.extend(target(&frames, *depth)),
            Instr::BrIf(depth) => {
                out.extend(target(&frames, *depth));
                next(&mut out);
            }
            Instr::BrTable(targets, default) => {
                for depth in targets.iter().chain(std::iter::once(default)) {
                    out.extend(target(&frames, *depth));
                }
            }
            Instr::Return
            | Instr::Unreachable
            | Instr::ReturnCall(_)
            | Instr::ReturnCallIndirect { .. } => {}
            _ => next(&mut out),
        }
        let mut blocks: Vec<usize> = out.into_iter().map(|i| block_of[i]).collect();
        blocks.sort_unstable();
        blocks.dedup();
        succs[b] = blocks;
        // Frames after this block (only control instructions change them,
        // and those end their block).
        match instr {
            Instr::Block(_) | Instr::Loop(_) | Instr::If(_) => frames.push(i),
            Instr::End => {
                frames.pop();
            }
            _ => {}
        }
    }
    Cfg { starts, succs }
}

fn local_index(instr: &Instr) -> Option<(u32, bool)> {
    match instr {
        Instr::LocalGet(index) => Some((*index, false)),
        Instr::LocalSet(index) | Instr::LocalTee(index) => Some((*index, true)),
        _ => None,
    }
}

fn coalesce(func: &mut Func, params: &[ValType]) {
    let num_params = params.len();
    let num_locals = num_params + func.locals.len();
    if func.locals.is_empty() {
        return;
    }
    let mut types: Vec<ValType> = params.to_vec();
    types.extend_from_slice(&func.locals);
    let mut uses = vec![0usize; num_locals];
    for instr in &func.body {
        if let Some((index, _)) = local_index(instr) {
            uses[index as usize] += 1;
        }
    }

    // Slot assignment: params fixed, then locals by use count.
    let mut slot_of = vec![usize::MAX; num_locals];
    let mut slot_types: Vec<ValType> = params.to_vec();
    for p in 0..num_params {
        slot_of[p] = p;
    }
    let mut order: Vec<usize> = (num_params..num_locals).filter(|l| uses[*l] > 0).collect();
    order.sort_by(|a, b| uses[*b].cmp(&uses[*a]).then(a.cmp(b)));

    if num_locals <= MAX_COALESCE_LOCALS {
        let adj = interference(&func.body, num_locals, num_params);
        // Per slot: the union of its members' neighbours.
        let mut conflicts = Bits::new(num_locals, num_locals);
        for p in 0..num_params {
            let row = adj.row(p).to_vec();
            conflicts.row_mut(p).copy_from_slice(&row);
        }
        for local in order.iter().copied() {
            let slot = (0..slot_types.len())
                .find(|s| slot_types[*s] == types[local] && !conflicts.get(*s, local))
                .unwrap_or_else(|| {
                    slot_types.push(types[local]);
                    slot_types.len() - 1
                });
            slot_of[local] = slot;
            let row = adj.row(local).to_vec();
            for (dst, src) in conflicts.row_mut(slot).iter_mut().zip(row) {
                *dst |= src;
            }
        }
    } else {
        for local in order.iter().copied() {
            slot_types.push(types[local]);
            slot_of[local] = slot_types.len() - 1;
        }
    }

    // Number the non-param slots: grouped by type, groups by total use,
    // slots by use within a group.
    let num_slots = slot_types.len();
    let mut slot_uses = vec![0usize; num_slots];
    for local in 0..num_locals {
        if slot_of[local] != usize::MAX {
            slot_uses[slot_of[local]] += uses[local];
        }
    }
    let mut type_uses: Vec<(ValType, usize)> = Vec::new();
    for s in num_params..num_slots {
        match type_uses.iter_mut().find(|(ty, _)| *ty == slot_types[s]) {
            Some((_, total)) => *total += slot_uses[s],
            None => type_uses.push((slot_types[s], slot_uses[s])),
        }
    }
    type_uses.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut slots: Vec<usize> = (num_params..num_slots).collect();
    slots.sort_by_key(|s| {
        let group = type_uses.iter().position(|(ty, _)| *ty == slot_types[*s]).unwrap();
        (group, std::cmp::Reverse(slot_uses[*s]), *s)
    });
    let mut final_index = vec![0u32; num_slots];
    for p in 0..num_params {
        final_index[p] = p as u32;
    }
    for (i, s) in slots.iter().enumerate() {
        final_index[*s] = (num_params + i) as u32;
    }
    for instr in &mut func.body {
        match instr {
            Instr::LocalGet(index) | Instr::LocalSet(index) | Instr::LocalTee(index) => {
                *index = final_index[slot_of[*index as usize]];
            }
            _ => {}
        }
    }
    func.locals = slots.iter().map(|s| slot_types[*s]).collect();
}

/// The interference matrix of a body's locals.
fn interference(body: &[Instr], num_locals: usize, num_params: usize) -> Bits {
    let cfg = build_cfg(body);
    let num_blocks = cfg.succs.len();
    let mut gen = Bits::new(num_blocks, num_locals);
    let mut kill = Bits::new(num_blocks, num_locals);
    for b in 0..num_blocks {
        for instr in &body[cfg.starts[b]..cfg.starts[b + 1]] {
            if let Some((index, def)) = local_index(instr) {
                let index = index as usize;
                if def {
                    kill.set(b, index);
                } else if !kill.get(b, index) {
                    gen.set(b, index);
                }
            }
        }
    }
    let words = gen.words;
    let mut live_in = Bits::new(num_blocks, num_locals);
    let mut live_out = Bits::new(num_blocks, num_locals);
    let mut changed = true;
    let mut scratch = vec![0u64; words];
    while changed {
        changed = false;
        for b in (0..num_blocks).rev() {
            scratch.iter_mut().for_each(|w| *w = 0);
            for s in &cfg.succs[b] {
                for (dst, src) in scratch.iter_mut().zip(live_in.row(*s)) {
                    *dst |= *src;
                }
            }
            live_out.row_mut(b).copy_from_slice(&scratch);
            for w in 0..words {
                scratch[w] = gen.row(b)[w] | (scratch[w] & !kill.row(b)[w]);
            }
            if live_in.row(b) != scratch.as_slice() {
                live_in.row_mut(b).copy_from_slice(&scratch);
                changed = true;
            }
        }
    }

    let mut adj = Bits::new(num_locals, num_locals);
    let mut live = vec![0u64; words];
    for b in 0..num_blocks {
        live.copy_from_slice(live_out.row(b));
        for instr in body[cfg.starts[b]..cfg.starts[b + 1]].iter().rev() {
            match local_index(instr) {
                Some((index, true)) => {
                    let index = index as usize;
                    clear_bit(&mut live, index);
                    for_each_bit(&live, |other| {
                        adj.set(index, other);
                        adj.set(other, index);
                    });
                }
                Some((index, false)) => set_bit(&mut live, index as usize),
                None => {}
            }
        }
    }
    // Parameters are written on entry.
    if num_blocks > 0 {
        let entry = live_in.row(0).to_vec();
        for p in 0..num_params {
            for_each_bit(&entry, |other| {
                if other != p {
                    adj.set(p, other);
                    adj.set(other, p);
                }
            });
        }
    }
    adj
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disjoint_locals_share_a_slot() {
        // (local a) (local b): a is dead before b is written.
        let mut func = Func {
            ty: 0,
            locals: vec![ValType::I32, ValType::I32],
            body: vec![
                Instr::I32Const(1),
                Instr::LocalSet(1),
                Instr::LocalGet(1),
                Instr::Drop,
                Instr::I32Const(2),
                Instr::LocalSet(2),
                Instr::LocalGet(2),
                Instr::End,
            ],
        };
        coalesce(&mut func, &[ValType::I32]);
        assert_eq!(func.locals.len(), 0, "both fit in the dead parameter's slot");

        // The parameter is live to the end: a and b share one new slot.
        let mut func = Func {
            ty: 0,
            locals: vec![ValType::I32, ValType::I32],
            body: vec![
                Instr::I32Const(1),
                Instr::LocalSet(1),
                Instr::LocalGet(1),
                Instr::Drop,
                Instr::I32Const(2),
                Instr::LocalSet(2),
                Instr::LocalGet(2),
                Instr::LocalGet(0),
                Instr::Num(0x6a),
                Instr::End,
            ],
        };
        coalesce(&mut func, &[ValType::I32]);
        assert_eq!(func.locals, vec![ValType::I32]);
    }

    #[test]
    fn zero_initial_value_is_kept() {
        // Local 1 is read before written: it must not share the param slot.
        let mut func = Func {
            ty: 0,
            locals: vec![ValType::I32],
            body: vec![
                Instr::LocalGet(0),
                Instr::Drop,
                Instr::LocalGet(1),
                Instr::End,
            ],
        };
        coalesce(&mut func, &[ValType::I32]);
        assert_eq!(func.locals, vec![ValType::I32]);
        assert_eq!(func.body[2], Instr::LocalGet(1));
    }

    #[test]
    fn loop_carried_values_interfere() {
        // a is written before the loop and read inside it after b is written.
        let mut func = Func {
            ty: 0,
            locals: vec![ValType::I32, ValType::I32],
            body: vec![
                Instr::I32Const(5),
                Instr::LocalSet(0),
                Instr::Loop(BlockType::Empty),
                Instr::I32Const(1),
                Instr::LocalSet(1),
                Instr::LocalGet(1),
                Instr::LocalGet(0),
                Instr::Num(0x6a),
                Instr::Drop,
                Instr::LocalGet(0),
                Instr::BrIf(0),
                Instr::End,
                Instr::End,
            ],
        };
        coalesce(&mut func, &[]);
        assert_eq!(func.locals.len(), 2);
    }
}
