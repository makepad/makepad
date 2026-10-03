//! `rapid live <v0.rs> <edit1.rs> ...`: runs a crate's tests in-process, then applies
//! each edit as a live patch (only changed functions recompile and swap their slot),
//! reruns, and on a panic in patched code rolls the patch back and reruns.

use crate::jit::{self, FnKey, Unit};
use crate::program::DefId;
use std::time::Instant;

fn run_tests(u: &mut Unit, tests: &[DefId]) -> Option<jit::PanicInfo> {
    for d in tests {
        let name = u.prog.def_path(*d);
        let id = u.fn_id(FnKey::Inst(*d, Vec::new()));
        let entry = u.entry(id);
        let t0 = Instant::now();
        jit::watchdog(10_000);
        let up: *mut Unit = u;
        let r = unsafe { jit::call_jit(up, entry, 0) };
        jit::watchdog_done();
        let dt = t0.elapsed().as_secs_f64() * 1e3;
        for e in std::mem::take(&mut u.errors) {
            println!("  {}", e);
        }
        match r {
            Ok(_) => println!("  test {} ok ({:.2} ms)", name, dt),
            Err(p) => {
                println!("  test {} PANICKED ({:.2} ms)", name, dt);
                return Some(p);
            }
        }
    }
    None
}

pub fn cmd_live(args: &[String]) -> i32 {
    let (prog, core, krate) = match crate::load_with_std(&args[0], "live", true) {
        Ok(x) => x,
        Err(e) => {
            for m in e {
                eprintln!("error: {}", m);
            }
            return 1;
        }
    };
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    let tests = jit::find_tests(&prog, krate);
    // the edited file is the crate root
    let mut file = 0;
    for (i, p) in prog.file_paths.iter().enumerate() {
        if *p == args[0] {
            file = i as u32;
        }
    }
    let mut u = Unit::new(prog, tcx, core);
    println!("v0: first run (lazy compile on first call)");
    run_tests(&mut u, &tests);
    println!("v0: second run (all compiled)");
    run_tests(&mut u, &tests);
    for path in &args[1..] {
        let src = match std::fs::read(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{}: {}", path, e);
                return 1;
            }
        };
        let t0 = Instant::now();
        let changed = match u.prog.replace_file(file, path, src) {
            Ok(c) => c,
            Err(e) => {
                println!("edit {}: {}", path, e);
                continue;
            }
        };
        let t1 = Instant::now();
        // recompile every instance of a changed fn, plus the callers that inlined it
        let mut slots = Vec::new();
        for (i, f) in u.fns.iter().enumerate() {
            if let FnKey::Inst(d, _) = &f.key {
                if changed.contains(d) && (f.compiled || f.rir.is_some()) {
                    slots.push(i as u32);
                }
            }
        }
        let mut k = 0;
        while k < slots.len() {
            let deps = u.fns[slots[k] as usize].inlined_into.clone();
            for d in deps {
                if !slots.contains(&d) {
                    slots.push(d);
                }
            }
            k += 1;
        }
        for id in &slots {
            u.fns[*id as usize].rir = None;
            u.fns[*id as usize].inlined_into.clear();
        }
        let gen = u.begin_patch();
        let mut names = Vec::new();
        for id in &slots {
            if !u.fns[*id as usize].compiled {
                continue;
            }
            let up: *mut Unit = &mut *u;
            match unsafe { jit::compile_fn(up, *id) } {
                Ok((code, map)) => {
                    u.install(*id, &code, map, gen);
                    names.push(u.fns[*id as usize].name.clone());
                }
                Err(e) => println!("  patch compile error: {}", e),
            }
        }
        let t2 = Instant::now();
        println!(
            "edit {}: {} changed fn(s) {:?}; reparse {:.3} ms, recompile+swap {:.3} ms, total {:.3} ms",
            path,
            slots.len(),
            names,
            (t1 - t0).as_secs_f64() * 1e3,
            (t2 - t1).as_secs_f64() * 1e3,
            (t2 - t0).as_secs_f64() * 1e3
        );
        if let Some(p) = run_tests(&mut u, &tests) {
            println!("  crash report: {}", u.report(&p));
            // roll back the most recent patch group that has code on the stack
            let mut culprit = None;
            let mut best_gen = 0;
            for pc in &p.frames {
                if let Some(id) = u.find_fn(*pc) {
                    let g = u.fns[id as usize].patch_gen;
                    if g > best_gen {
                        best_gen = g;
                        culprit = Some(id);
                    }
                }
            }
            match culprit {
                Some(_) => {
                    let t = Instant::now();
                    let ids = u.rollback_gen(best_gen);
                    let mut ns = Vec::new();
                    for id in ids {
                        ns.push(u.fns[id as usize].name.clone());
                    }
                    println!("  rolled back patch {} ({:?}) in {:.3} ms; rerun:", best_gen, ns, t.elapsed().as_secs_f64() * 1e3);
                    run_tests(&mut u, &tests);
                }
                None => println!("  no patched function on the stack"),
            }
        }
    }
    0
}
