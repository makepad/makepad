//! `makepad live <v0.rs> <edit1.rs> ...`: runs a crate's tests in-process, then applies
//! each edit as a live patch (only changed functions recompile and swap their slot),
//! reruns, and on a panic in patched code rolls the patch back and reruns.

use crate::jit::{self, FnKey, Unit};
use crate::program::DefId;
use std::time::Instant;

fn run_tests(u: &mut Unit, tests: &[DefId]) -> Option<jit::PanicInfo> {
    for d in tests {
        let name = u.prog.def_path(*d);
        // speculative workers compile concurrently: unit access under the lock
        let entry = crate::owner::call(crate::owner::Class::App, || {
            let id = u.fn_id(FnKey::Inst(*d, Vec::new()));
            u.entry(id)
        });
        let t0 = Instant::now();
        jit::watchdog(10_000);
        let up: *mut Unit = u;
        let r = unsafe { jit::call_jit(up, entry, 0) };
        jit::watchdog_done();
        let dt = t0.elapsed().as_secs_f64() * 1e3;
        let errs = crate::owner::call(crate::owner::Class::App, || std::mem::take(&mut u.errors));
        for e in errs {
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

/// The files of an edit step: a file (the crate root's new text) or a directory (an overlay:
/// each file in it replaces the loaded file at the same path relative to the crate root's
/// directory). All of a step's files apply as one transaction.
fn step_files(arg: &str, root_dir: &str) -> Result<Vec<(String, String)>, String> {
    let p = std::path::Path::new(arg);
    if !p.is_dir() {
        return Ok(vec![(String::new(), arg.to_string())]);
    }
    let mut out = Vec::new();
    let mut stack = vec![p.to_path_buf()];
    while let Some(d) = stack.pop() {
        let rd = std::fs::read_dir(&d).map_err(|e| format!("{}: {}", d.display(), e))?;
        for e in rd.flatten() {
            let q = e.path();
            if q.is_dir() {
                stack.push(q);
            } else if q.extension().map_or(false, |x| x == "rs" || x == "context") {
                let rel = q.strip_prefix(p).unwrap().to_string_lossy().into_owned();
                out.push((format!("{}/{}", root_dir, rel), q.to_string_lossy().into_owned()));
            }
        }
    }
    out.sort();
    Ok(out)
}

pub fn cmd_live(sess: &std::sync::Arc<crate::Session>, args: &[String]) -> i32 {
    let options = &sess.options;
    // a live session from the first parse (derive.rs: `#[rust(expr)]` field defaults)
    crate::pmhost::install();
    let (prog, core, krate) = match crate::load_with_std(sess, &args[0], "live", true) {
        Ok(x) => x,
        Err(e) => {
            for m in e {
                eprintln!("error: {}", m);
            }
            return 1;
        }
    };
    // a live session: the crate's structs take the live layout (M10, layout.rs live_editable)
    crate::layout::LIVE_SESSION.store(true, std::sync::atomic::Ordering::Relaxed);
    crate::layout::set_root_crate(krate);
    // Single-file live sessions have one app crate and no Cargo unit graph.
    crate::borrowck::event::set_app_scope(vec![prog.crates[krate as usize].root_path().to_string()]);
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    let tests = jit::find_tests(&prog, krate);
    // loaded path -> its running file index (a patch makes a new version)
    let mut running: Vec<(String, u32)> = prog.file_paths.iter().enumerate().map(|(i, p)| (p.clone(), i as u32)).collect();
    let root_dir = crate::program::dir_of(&args[0]).to_string();
    let mut u = Unit::new(prog, tcx, core);
    let enum_errors = u.tcx.check_enum_discriminants(&u.prog, krate);
    if !enum_errors.is_empty() {
        for error in enum_errors { eprintln!("error: {}", error); }
        return 1;
    }
    jit::enable_pipelining(options);
    println!("v0: first run (lazy compile on first call)");
    run_tests(&mut u, &tests);
    println!("v0: second run (all compiled)");
    run_tests(&mut u, &tests);
    let mut undos: Vec<crate::reload::Undo> = Vec::new();
    for step in &args[1..] {
        let files = match step_files(step, &root_dir) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("{}", e);
                return 1;
            }
        };
        let read_started = Instant::now();
        let mut edits = Vec::new();
        let mut bad = None;
        for (loaded, src_path) in files {
            let loaded = if loaded.is_empty() { args[0].clone() } else { loaded };
            let file = match running.iter().find(|(p, _)| *p == loaded) {
                Some((_, f)) => *f,
                None => {
                    bad = Some(format!("{} is not a file of the running crate", loaded));
                    break;
                }
            };
            match std::fs::read(&src_path) {
                Ok(src) => edits.push(crate::reload::Edit { file, path: src_path.clone(), src }),
                Err(e) => {
                    bad = Some(format!("{}: {}", src_path, e));
                    break;
                }
            }
        }
        if let Some(b) = bad {
            println!("edit {}: rung=refuse {}", step, b);
            continue;
        }
        crate::reload::trace_input(0.0, read_started.elapsed().as_secs_f64() * 1000., edits.len());
        let olds: Vec<u32> = edits.iter().map(|e| e.file).collect();
        let up: *mut Unit = &mut *u;
        let o = if options.live_stream && edits.len() == 1 {
            // The streaming cases feed whole prefixes through the public entry point.
            // Space harness steps beyond the coalescing window so each expected output is
            // observable; real callers may submit much faster and let the watcher drain.
            std::thread::sleep(std::time::Duration::from_millis(17));
            let edit = edits.into_iter().next().unwrap();
            let path = u.prog.file_paths[edit.file as usize].clone();
            unsafe { crate::reload::apply_stream(up, &path, edit.src) }
        } else {
            unsafe { crate::reload::apply(up, edits) }
        };
        println!(
            "edit {}: rung={} {} changed item(s), {} fn(s) installed {:?}; parse+classify {:.3} ms, check {:.3} ms, compile {:.3} ms, install {:.3} ms",
            step,
            o.rung.name(),
            o.changed,
            o.names.len(),
            o.names,
            o.ms[0],
            o.ms[1],
            o.ms[2],
            o.ms[3]
        );
        for l in &o.lines {
            println!("  {}", l);
        }
        for (k, old) in olds.iter().enumerate() {
            for r in running.iter_mut() {
                if r.1 == *old {
                    r.1 = o.files[k];
                }
            }
        }
        if let Some(undo) = o.undo {
            undos.push(undo);
        }
        if let Some(p) = run_tests(&mut u, &tests) {
            let best_gen = crate::owner::call(crate::owner::Class::App, || {
                println!("  crash report: {}", u.report(&p));
                // roll back the most recent patch group that has code on the stack
                let mut best_gen = 0;
                for pc in &p.frames {
                    if let Some(id) = u.find_fn(*pc) {
                        best_gen = best_gen.max(u.fns[id as usize].patch_gen);
                    }
                }
                best_gen
            });
            match undos.iter().position(|x| x.group == best_gen) {
                Some(i) if best_gen > 0 => {
                    let undo = undos.remove(i);
                    let t = Instant::now();
                    let back: Vec<(u32, u32)> = undo.versions.clone();
                    let up: *mut Unit = &mut *u;
                    let ns = unsafe { crate::reload::rollback(up, undo) };
                    for (o, n) in back {
                        for r in running.iter_mut() {
                            if r.1 == n {
                                r.1 = o;
                            }
                        }
                    }
                    println!("  rolled back patch {} ({:?}) in {:.3} ms; rerun:", best_gen, ns, t.elapsed().as_secs_f64() * 1e3);
                    run_tests(&mut u, &tests);
                }
                _ => println!("  no patched function on the stack"),
            }
        }
    }
    // threads the program left running (a live case's parked frame) still call into the unit
    // (lazy compiles): it lives to process exit, as `makepad run`'s program unit does. Dropping it
    // here crashed those threads at exit (body_on_stack, lock_released, recursion).
    std::mem::forget(u);
    0
}
