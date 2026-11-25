//! Hot call sites of counted code's O3 build call through the callee's body word (`Reloc::Body`)
//! when the slots' table words name entry wrappers (a live test session's reach marks): any block
//! that ran (count above 0), its tail calls too. Every other site calls the
//! thunk (`Reloc::Call`), as does every site without counts, without wrappers or with
//! RELAX_OPT=-bypass. The host's backend, code generation only.
#![cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
use relax_backend::rir::*;

/// `f(n) = if n == 0 { k(g(n)) } else { h(n) }`: slots g 1, k 2, h 3; entry and the `g` block ran
/// `hot` times (negative: no counts), the `h` block never.
fn caller(hot: f32) -> Func {
    let mut f = Func::new("caller".into(), 0);
    let n = f.vreg(Cls::I);
    f.params = vec![n];
    f.rets = vec![Cls::I];
    let (z, r, s, t) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    let mut block = |insts: Vec<Inst>, term: Term, count: f32| {
        let b = f.block();
        let blk = &mut f.blocks[b as usize];
        blk.pos = vec![0; insts.len()];
        blk.insts = insts;
        blk.term = term;
        blk.count = count;
    };
    block(vec![Inst::ICmpI(Cond::Eq, false, z, n, 0)], Term::Branch(z, 1, 2), hot);
    block(vec![Inst::Call(Callee::Fn(1), vec![n], vec![r]), Inst::Call(Callee::Fn(2), vec![r], vec![s])], Term::Ret(vec![s]), hot);
    block(vec![Inst::Call(Callee::Fn(3), vec![n], vec![t])], Term::Ret(vec![t]), if hot < 0.0 { hot } else { 0.0 });
    f.verify().unwrap();
    f
}

/// The host backend's code (its text) and relocations for `f` with slot table `table`.
#[cfg(target_arch = "x86_64")]
fn compile(options: &relax_session::Options, f: &Func, table: &SlotTable) -> (Vec<u8>, Vec<RelocAt>) {
    let env = relax_backend::x64::Env { slots: SlotsPtr(table), tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: true };
    let c = relax_backend::x64::compile(options, f, &env).unwrap();
    (c.code[..c.text_len as usize].to_vec(), c.relocs)
}

#[cfg(target_arch = "aarch64")]
fn compile(options: &relax_session::Options, f: &Func, table: &SlotTable) -> (Vec<u8>, Vec<RelocAt>) {
    let env = relax_backend::arm64::Env { slots: SlotsPtr(table), tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: true };
    let c = relax_backend::arm64::compile(options, f, &env).unwrap();
    (c.code[..c.text_len as usize].to_vec(), c.relocs)
}

/// The slot each call relocation names, as (callee, through its body word).
fn calls(options: &relax_session::Options, f: &Func, table: &SlotTable) -> Vec<(u32, bool)> {
    let mut v: Vec<(u32, bool)> = compile(options, f, table).1.iter().filter_map(|r| match r.target {
        Reloc::Call(c) => Some((c, false)),
        Reloc::Body(c) => Some((c, true)),
        _ => None,
    }).collect();
    v.sort_unstable();
    v
}

#[test]
fn hot_sites_of_counted_code_call_through_the_body_word_only_past_entry_wrappers() {
    let plain = relax_session::Options::default();
    let mut off = relax_session::Options::default();
    off.opt.push("-bypass".into());
    // table words 16 bytes apart: a wrapper's and the body's (a live test session)
    let wrapped = SlotTable::new(32, 32, 16, 48);
    let single = SlotTable::new(32, 32, 8, 48);
    let thunks = vec![(1, false), (2, false), (3, false)];
    assert_eq!(calls(&plain, &caller(10.0), &wrapped), vec![(1, true), (2, true), (3, false)], "the calls that ran, the tail call too, through the body word; the one that never ran through the thunk");
    // the call through the word and k's sibling call as a jump through it
    let (code, _) = compile(&plain, &caller(10.0), &wrapped);
    #[cfg(target_arch = "x86_64")]
    let (call, jump) = (code.windows(3).any(|w| w == [0x41, 0xff, 0x13]), code.windows(3).any(|w| w == [0x41, 0xff, 0x23]));
    #[cfg(target_arch = "aarch64")]
    let (call, jump) = {
        let words: Vec<u32> = code.chunks_exact(4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]])).collect();
        // ldr x16, [x16] then blr x16 / br x16
        let after_load = |op: u32| words.windows(2).any(|w| w[0] == 0xF940_0210 && w[1] == op);
        (after_load(0xD63F_0200), after_load(0xD61F_0200))
    };
    assert!(call && jump, "a call and a sibling jump through the loaded body word (call {call}, jump {jump})");
    assert_eq!(calls(&plain, &caller(-1.0), &wrapped), thunks, "code without counts calls the thunks");
    assert_eq!(calls(&plain, &caller(10.0), &single), thunks, "without entry wrappers the thunks");
    assert_eq!(calls(&off, &caller(10.0), &wrapped), thunks, "RELAX_OPT=-bypass calls the thunks");
}
