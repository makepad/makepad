//! std::backtrace under the JIT (Relax std's std/backtrace.rs, and its panic hook's RUST_BACKTRACE
//! output): the running thread's frames, then their names and source positions. A JIT frame is
//! named by the unit on the owner (a Program request: the thread asks as it asks for a lazy
//! compile), an optimised body's inlined fns each as a symbol of their own, innermost first (what
//! backtrace-rs resolves from rustc's debug info); a host frame by its image's symbol table.

use std::panic::{catch_unwind, AssertUnwindSafe};

/// The calling thread's frames for a `Backtrace`: return addresses, innermost first, from its JIT
/// caller on. The runtime's frames above that (this entry's own) and below the program's outermost
/// fn (the runtime that started the thread or ran main, its entry glue among them) are left out;
/// the runtime's and C frames between the program's fns (a C library calling back into the
/// program, a catch_unwind's recovery point) stay. Returns how many there are; the first `cap` go
/// to `out` (a caller whose buffer was too small asks again with a larger one). 0: no walk on this
/// target.
pub(crate) extern "C" fn relax_backtrace_capture(out: *mut u64, cap: usize) -> usize {
    catch_unwind(AssertUnwindSafe(|| {
        let mut all = Vec::with_capacity(64);
        super::threads::capture_self(&mut all);
        let up = crate::jit::cur_unit();
        if up.is_null() {
            return 0;
        }
        // a frame of a fn's code (the code tables: a leaf lock, no owner)
        let jit = |pc: &u64| {
            let look = pc.wrapping_sub(1);
            crate::jit::in_jit_code(look) && unsafe { &*up }.find_fn(look).is_some()
        };
        let (Some(first), Some(last)) = (all.iter().position(jit), all.iter().rposition(jit)) else { return 0 };
        let frames = &all[first..=last];
        if !out.is_null() {
            for (i, &pc) in frames.iter().take(cap).enumerate() {
                unsafe { *out.add(i) = pc };
            }
        }
        frames.len()
    }))
    .unwrap_or(0)
}

/// The names and source positions of a `Backtrace`'s frames (`pcs`: return addresses), as lines
/// `<frame>\t<name>\t<file>\t<line>\t<col>\n`, one per symbol, a frame's inlined fns first
/// (innermost first); a field is empty where it is not known, a frame nothing is known of has no
/// line. Returns the text's length; the text goes to `out` when `cap` holds it (else the caller
/// asks again with that much room).
pub(crate) extern "C" fn relax_backtrace_resolve(pcs: *const u64, n: usize, out: *mut u8, cap: usize) -> usize {
    catch_unwind(AssertUnwindSafe(|| {
        let pcs = if n == 0 || pcs.is_null() { &[][..] } else { unsafe { std::slice::from_raw_parts(pcs, n) } };
        let text = resolve(pcs);
        if text.len() <= cap && !out.is_null() {
            unsafe { std::ptr::copy_nonoverlapping(text.as_ptr(), out, text.len()) };
        }
        text.len()
    }))
    .unwrap_or(0)
}

/// (name, file:line:col) of each symbol of a frame
type Symbols = Vec<(String, String)>;

fn resolve(pcs: &[u64]) -> String {
    // the lookup pcs: a return address points past its call
    let looks: Vec<u64> = pcs.iter().map(|pc| pc.wrapping_sub(1)).collect();
    let jit: Vec<bool> = looks.iter().map(|&pc| crate::jit::in_jit_code(pc)).collect();
    let up = crate::jit::cur_unit();
    let named: Vec<Option<Symbols>> = if up.is_null() || !jit.contains(&true) {
        vec![None; pcs.len()]
    } else {
        let (looks, jit) = (&looks, &jit);
        crate::owner::call(crate::owner::Class::Program, move || {
            let u = unsafe { &*up };
            let mut r = super::codever::Resolver::new();
            looks.iter().zip(jit).map(|(&pc, &j)| j.then(|| jit_symbols(u, &mut r, pc))).collect()
        })
    };
    let mut text = String::new();
    for (k, (&look, named)) in looks.iter().zip(named).enumerate() {
        let syms = match named {
            Some(s) => s,
            None => match super::symbols::host_symbol(look) {
                Some((name, _, _)) => vec![(name, String::new())],
                None => Vec::new(),
            },
        };
        for (name, at) in syms {
            let (file, line, col) = split_at(&at);
            let clean = |s: &str| s.replace(['\t', '\n'], " ");
            text.push_str(&format!("{}\t{}\t{}\t{}\t{}\n", k, clean(&name), clean(file), line, col));
        }
    }
    text
}

/// A JIT pc's symbols: the inlined fns its position lies in (the line table's inline stack,
/// innermost first), then the fn whose code it is.
fn jit_symbols(u: &crate::jit::Unit, r: &mut super::codever::Resolver, pc: u64) -> Symbols {
    let Some(c) = super::codever::describe_pc(u, r, pc) else {
        // a slot's thunk or lazy stub, a veneer; the runtime's entries
        if let Some((n, _)) = super::codever::glue_name(u, pc) {
            return vec![(n, String::new())];
        }
        let glue = u.code.read().unwrap_or_else(|e| e.into_inner()).glue_at(pc);
        return match glue {
            Some(at) if at == u.enter => vec![("relax::enter_leave".to_string(), String::new())],
            Some(at) if at == u.lazy_entry => vec![("relax::lazy_entry".to_string(), String::new())],
            Some(_) => vec![("relax::glue".to_string(), String::new())],
            None => Vec::new(),
        };
    };
    let own = std_name(&super::display_name(u, c.slot, 0));
    match c.inlined {
        Some(i) if !i.stack.is_empty() => {
            let last = i.stack.len() - 1;
            i.stack.into_iter().enumerate().map(|(j, (n, _, at))| (if j == last { own.clone() } else { std_name(&n) }, at)).collect()
        }
        // code made before the line table kept inline stacks: the inlined fn's call site is not known
        Some(i) => vec![(std_name(&i.name), c.at), (own, String::new())],
        None => vec![(own, c.at)],
    }
}

/// A fn's name as rustc's symbols spell it: a closure is `<fn>::{{closure}}` (inspect's
/// `{closure} in <fn>`).
fn std_name(n: &str) -> String {
    let mut rest = n;
    let mut closures = 0;
    while let Some(r) = rest.strip_prefix("{closure} in ") {
        rest = r;
        closures += 1;
    }
    let mut s = rest.to_string();
    for _ in 0..closures {
        s.push_str("::{{closure}}");
    }
    s
}

/// `file:line:col` -> (file, line, col); all empty when it names no line.
fn split_at(at: &str) -> (&str, &str, &str) {
    let mut w = at.rsplitn(3, ':');
    match (w.next(), w.next(), w.next()) {
        (Some(c), Some(l), Some(f)) if l.parse::<u32>().is_ok() && c.parse::<u32>().is_ok() => (f, l, c),
        _ => ("", "", ""),
    }
}
