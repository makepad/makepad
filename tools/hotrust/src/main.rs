//! hotrust: command line driver.
//!   hotrust parse <units.json> [threads]   parse the crate graph, time it, write the census
//!   hotrust file <path> [edition]          parse one file, report errors

pub mod ast;
pub mod cfg;
pub mod census;
pub mod crates;
pub mod json;
pub mod lexer;
pub mod live;
pub mod parser;
pub mod consteval;
pub mod jit;
pub mod layout;
pub mod lower;
pub mod opt;
pub mod regalloc;
pub mod rir;
pub mod tcx;
pub mod typeck;
pub mod x64;
pub mod arm64;
pub mod cabi;
pub mod types;
pub mod program;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: hotrust parse <units.json> [threads] | hotrust file <path> [edition]");
        std::process::exit(2);
    }
    match args[1].as_str() {
        "parse" => {
            let threads = if args.len() > 3 { args[3].parse::<usize>().unwrap_or(16) } else { 16 };
            cmd_parse(&args[2], threads);
        }
        "live" => {
            let code = live::cmd_live(&args[2..]);
            std::process::exit(code);
        }
        "resolve" => {
            cmd_resolve(&args[2]);
        }
        "test" => {
            let code = cmd_test(&args[2..]);
            std::process::exit(code);
        }
        "file" => {
            let ed = if args.len() > 3 { args[3].parse::<u16>().unwrap_or(2021) } else { 2021 };
            let src = std::fs::read(&args[2]).expect("read");
            match parser::parse_source(&src, ed) {
                Ok(p) => println!("ok: {} items, {} exprs", p.ast.items.len(), p.ast.exprs.len()),
                Err((pos, msg)) => {
                    let (l, c) = lexer::line_col(&src, pos);
                    println!("{}:{}:{}: {}", args[2], l, c, msg);
                    std::process::exit(1);
                }
            }
        }
        _ => {
            eprintln!("unknown command");
            std::process::exit(2);
        }
    }
}

fn count_lines(s: &[u8]) -> usize {
    let mut n = 0;
    for &c in s {
        if c == b'\n' {
            n += 1;
        }
    }
    n
}

/// Parses all files on `threads` threads; returns (seconds, errors).
fn parse_all(files: &[crates::SrcFile], threads: usize) -> (f64, usize) {
    parse_all_mode(files, threads, false)
}

fn parse_all_mode(files: &[crates::SrcFile], threads: usize, lazy: bool) -> (f64, usize) {
    let next = AtomicUsize::new(0);
    let errors = AtomicUsize::new(0);
    let t0 = Instant::now();
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= files.len() {
                    break;
                }
                let f = &files[i];
                if parser::parse_source_mode(&f.src, f.edition, lazy).is_err() {
                    errors.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
    });
    (t0.elapsed().as_secs_f64(), errors.load(Ordering::Relaxed))
}

fn lex_all(files: &[crates::SrcFile]) -> (f64, usize) {
    let t0 = Instant::now();
    let mut ntok = 0;
    let mut toks = Vec::new();
    for f in files {
        toks.clear();
        let _ = lexer::lex(&f.src, f.edition, &mut toks);
        ntok += toks.len();
    }
    (t0.elapsed().as_secs_f64(), ntok)
}

fn cmd_parse(units_path: &str, threads: usize) {
    let crates = match crates::load_units(units_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };
    let t0 = Instant::now();
    let mut cfgs = Vec::new();
    for c in &crates {
        cfgs.push(cfg::CfgSet::host(&c.features));
    }
    let (files, missing, errors) = crates::discover(&crates, &cfgs, threads);
    let discover_s = t0.elapsed().as_secs_f64();
    let mut lines = 0usize;
    let mut bytes = 0usize;
    let mut act_files = 0usize;
    let mut act_lines = 0usize;
    for f in &files {
        let n = count_lines(&f.src);
        lines += n;
        bytes += f.src.len();
        if f.active {
            act_files += 1;
            act_lines += n;
        }
    }
    println!("crates: {}  files: {}  lines: {}  bytes: {}", crates.len(), files.len(), lines, bytes);
    println!("active on x86_64 linux (cfg): files: {}  lines: {}", act_files, act_lines);
    println!("discover (read + parse + follow mods, {} threads): {:.3} s", threads, discover_s);
    println!("parse errors: {}", errors.len());
    for e in &errors {
        println!("  ERROR {}", e);
    }
    println!("declared mod files not found: {}", missing.len());
    for m in &missing {
        println!("  missing {}", m);
    }

    // timings (sources in memory)
    let mut best_lex = f64::MAX;
    let mut ntok = 0;
    for _ in 0..3 {
        let (t, n) = lex_all(&files);
        if t < best_lex {
            best_lex = t;
        }
        ntok = n;
    }
    let mut best1 = f64::MAX;
    for _ in 0..3 {
        let (t, _) = parse_all(&files, 1);
        if t < best1 {
            best1 = t;
        }
    }
    let mut bestn = f64::MAX;
    for _ in 0..5 {
        let (t, _) = parse_all(&files, threads);
        if t < bestn {
            bestn = t;
        }
    }
    let mut best1l = f64::MAX;
    for _ in 0..3 {
        let (t, _) = parse_all_mode(&files, 1, true);
        if t < best1l {
            best1l = t;
        }
    }
    let mut bestnl = f64::MAX;
    for _ in 0..5 {
        let (t, _) = parse_all_mode(&files, threads, true);
        if t < bestnl {
            bestnl = t;
        }
    }
    // Splash bytes (script_mod! bodies, never tokenised as Rust)
    let mut splash_bytes = 0usize;
    let mut splash_blocks = 0usize;
    let mut toks = Vec::new();
    for f in &files {
        toks.clear();
        let _ = lexer::lex(&f.src, f.edition, &mut toks);
        for t in &toks {
            if t.kind == lexer::T::Splash {
                splash_bytes += (t.hi - t.lo) as usize;
                splash_blocks += 1;
            }
        }
    }
    let l = lines as f64;
    let mb = bytes as f64 / 1e6;
    println!("splash: {} script_mod! bodies, {} bytes ({:.1}% of source)", splash_blocks, splash_bytes, splash_bytes as f64 * 100.0 / bytes as f64);
    println!("lazy bodies, 1 thread:   {:.3} s  {:.2} M lines/s  {:.0} MB/s", best1l, l / best1l / 1e6, mb / best1l);
    println!("lazy bodies, {} threads: {:.3} s  {:.2} M lines/s  {:.0} MB/s", threads, bestnl, l / bestnl / 1e6, mb / bestnl);
    println!("tokens: {}", ntok);
    println!("lex only, 1 thread:    {:.3} s  {:.2} M lines/s  {:.0} MB/s", best_lex, l / best_lex / 1e6, bytes as f64 / best_lex / 1e6);
    println!("full parse, 1 thread:    {:.3} s  {:.2} M lines/s  {:.0} MB/s", best1, l / best1 / 1e6, mb / best1);
    println!("full parse, {} threads:  {:.3} s  {:.2} M lines/s  {:.0} MB/s", threads, bestn, l / bestn / 1e6, mb / bestn);

    // census per crate
    let mut per: Vec<census::Census> = Vec::new();
    for _ in 0..crates.len() {
        per.push(census::Census::new());
    }
    for f in &files {
        if !f.active {
            continue;
        }
        if let Ok(p) = parser::parse_source(&f.src, f.edition) {
            let mut w = census::Walker::new(&p, &cfgs[f.crate_idx as usize]);
            w.file();
            per[f.crate_idx as usize].merge(&w.cen);
        }
    }
    let mut total = census::Census::new();
    for c in &per {
        total.merge(c);
    }
    let out = census_report(&crates, &per, &total);
    let path = "census.md";
    std::fs::write(path, out).expect("write census");
    println!("census written to {}", path);
}

fn census_report(crates: &[crates::CrateUnit], per: &[census::Census], total: &census::Census) -> String {
    let mut s = String::new();
    s.push_str("# HotRust dialect census (base stack)\n\n");
    s.push_str("| crate | ");
    for n in census::NAMES {
        s.push_str(n);
        s.push_str(" | ");
    }
    s.push_str("build.rs |\n|---|");
    for _ in 0..census::N + 1 {
        s.push_str("---|");
    }
    s.push('\n');
    let mut order: Vec<usize> = Vec::new();
    for i in 0..crates.len() {
        order.push(i);
    }
    order.sort_by(|a, b| per[*b].c[census::C_LINES].cmp(&per[*a].c[census::C_LINES]));
    for &i in &order {
        s.push_str(&format!("| {} | ", crates[i].name));
        for k in 0..census::N {
            s.push_str(&format!("{} | ", per[i].c[k]));
        }
        s.push_str(if crates[i].build_script { "yes |\n" } else { "|\n" });
    }
    s.push_str("| **total** | ");
    for k in 0..census::N {
        s.push_str(&format!("{} | ", total.c[k]));
    }
    s.push_str("|\n\n");
    let kinds: [(u8, &str); 5] = [
        (b'm', "user macro calls (R1/R2)"),
        (b'd', "third-party derives (R3)"),
        (b'o', "our derives"),
        (b'a', "unknown item attributes (attribute macros or derive helpers)"),
        (b'p', "iterator adapters (R5)"),
    ];
    for (k, title) in kinds {
        let mut v: Vec<(String, u64)> = Vec::new();
        for e in &total.names {
            if e.0 == k {
                v.push((e.1.clone(), e.2));
            }
        }
        v.sort_by(|a, b| b.1.cmp(&a.1));
        s.push_str(&format!("## {}\n\n", title));
        let mut n = 0;
        for (name, c) in &v {
            s.push_str(&format!("{} {}, ", name, c));
            n += 1;
            if n >= 80 {
                break;
            }
        }
        s.push_str("\n\n");
    }
    s
}

/// Loads HotRust's core + std and a crate (root file), returns (program, crate index).
/// std sources: $HOTRUST_STD or <repo>/tools/hotrust/std
pub fn std_dir() -> String {
    match std::env::var("HOTRUST_STD") {
        Ok(d) => d,
        Err(_) => {
            let mut d = std::env::current_exe().unwrap_or_default();
            for _ in 0..3 {
                d.pop();
            }
            format!("{}/src/tools/hotrust/std", d.display())
        }
    }
}

/// Loads core, alloc (when present) and std; returns core and the dependency list user crates
/// get ([core, alloc, std]).
pub fn load_std_crates(prog: &mut program::Program) -> Result<(u32, Vec<(program::Sym, u32)>), String> {
    let dir = std_dir();
    let core = prog.load_crate("core", &format!("{}/core/lib.rs", dir), 2021, cfg::CfgSet::host(&[]), Vec::new())?;
    let core_sym = prog.syms.intern("core");
    let mut deps = vec![(core_sym, core)];
    let alloc_root = format!("{}/alloc/lib.rs", dir);
    if std::path::Path::new(&alloc_root).exists() {
        let alloc = prog.load_crate("alloc", &alloc_root, 2021, cfg::CfgSet::host(&[]), deps.clone())?;
        deps.push((prog.syms.intern("alloc"), alloc));
    }
    let std = prog.load_crate("std", &format!("{}/std/lib.rs", dir), 2021, cfg::CfgSet::host(&[]), deps.clone())?;
    deps.push((prog.syms.intern("std"), std));
    Ok((core, deps))
}

pub fn load_with_std(root: &str, name: &str, test: bool) -> Result<(program::Program, u32, u32), Vec<String>> {
    let mut prog = program::Program::new();
    let (core, deps) = load_std_crates(&mut prog).map_err(|e| vec![e])?;
    let mut cfgset = cfg::CfgSet::host(&[]);
    if test {
        cfgset.set.push("test".to_string());
    }
    cfgset.set.push("hotrust".to_string());
    let krate = match prog.load_crate(name, root, 2021, cfgset, deps) {
        Ok(c) => c,
        Err(e) => return Err(vec![e]),
    };
    prog.prelude_crate = Some(core);
    prog.resolve_imports();
    if !prog.errors.is_empty() {
        return Err(prog.errors.clone());
    }
    Ok((prog, core, krate))
}

fn cmd_test(args: &[String]) -> i32 {
    let root = &args[0];
    let filter = if args.len() > 1 { args[1].clone() } else { String::new() };
    let t0 = Instant::now();
    let (prog, core, krate) = match load_with_std(root, "test_crate", true) {
        Ok(x) => x,
        Err(e) => {
            for m in e {
                eprintln!("error: {}", m);
            }
            return 1;
        }
    };
    let t1 = Instant::now();
    let mut tcx = tcx::Tcx::new();
    tcx.collect(&prog);
    if !tcx.errors.is_empty() {
        for m in &tcx.errors {
            eprintln!("error: {}", m);
        }
        return 1;
    }
    let t2 = Instant::now();
    let tests = jit::find_tests(&prog, krate);
    let mut u = jit::Unit::new(prog, tcx, core);
    println!(
        "boot: load+parse+resolve {:.2} ms, signatures/layouts {:.2} ms; {} tests",
        (t1 - t0).as_secs_f64() * 1e3,
        (t2 - t1).as_secs_f64() * 1e3,
        tests.len()
    );
    let repeat: usize = std::env::var("HOTRUST_REPEAT").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    let mut passed = 0;
    let mut failed = 0;
    let mut runs = Vec::new();
    for _ in 0..repeat {
        for d in &tests {
            runs.push(*d);
        }
    }
    let total_runs = runs.len();
    for (ri, d) in runs.into_iter().enumerate() {
        let last_round = ri + tests.len() >= total_runs;
        let name = u.prog.def_path(d);
        if !filter.is_empty() && !name.contains(&filter) {
            continue;
        }
        let id = u.fn_id(jit::FnKey::Inst(d, Vec::new()));
        let entry = u.entry(id);
        let ts = Instant::now();
        jit::watchdog(std::env::var("HOTRUST_WATCHDOG_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(20_000));
        let up: *mut jit::Unit = &mut *u;
        let r = unsafe { jit::call_jit(up, entry, 0) };
        jit::watchdog_done();
        let dt = ts.elapsed().as_secs_f64() * 1e3;
        if !last_round {
            continue;
        }
        match r {
            Ok(_) => {
                println!("test {} ... ok ({:.2} ms)", name, dt);
                passed += 1;
            }
            Err(p) => {
                println!("test {} ... FAILED ({:.2} ms)", name, dt);
                println!("  report: {}", u.report(&p));
                failed += 1;
            }
        }
        for e in std::mem::take(&mut u.errors) {
            println!("  {}", e);
        }
    }
    println!(
        "test result: {} passed; {} failed. compiled {} fns ({} bytes, {} RIR insts): typeck {:.2} ms, lower {:.2} ms, opt {:.2} ms, codegen {:.2} ms",
        passed,
        failed,
        u.stats.compiled,
        u.stats.code_bytes,
        u.stats.rir_insts,
        u.stats.typeck_ns as f64 / 1e6,
        u.stats.lower_ns as f64 / 1e6,
        u.stats.opt_ns as f64 / 1e6,
        u.stats.codegen_ns as f64 / 1e6
    );
    let mut rules = String::new();
    for (i, n) in opt::RULE_NAMES.iter().enumerate() {
        rules.push_str(&format!("{} {}  ", n, u.stats.inline_rules[i]));
    }
    println!("inlined call sites by rule: {}", rules);
    if std::env::var("HOTRUST_PASS_TIMES").is_ok() {
        let mut passes = String::new();
        opt::PASS_NS.with(|p| {
            for i in 0..8 {
                passes.push_str(&format!("{} {:.2}  ", opt::PASS_NAMES[i], p[i].get() as f64 / 1e6));
            }
        });
        println!("opt passes (ms): {}", passes);
    }
    if failed > 0 {
        1
    } else {
        0
    }
}

/// M2: loads every crate of the unit graph with HotRust's core/std and resolves all imports.
fn cmd_resolve(units_path: &str) {
    let crates = match crates::load_units(units_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}", e);
            return;
        }
    };
    let t0 = Instant::now();
    let mut prog = program::Program::new();
    let (core, std_deps) = load_std_crates(&mut prog).unwrap();
    let base = prog.crates.len() as u32;
    let mut load_errors = 0;
    for c in &crates {
        let mut deps = std_deps.clone();
        for (i, n) in &c.deps {
            let s = prog.syms.intern(n);
            deps.push((s, base + *i as u32));
        }
        let mut cs = cfg::CfgSet::host(&c.features);
        cs.set.push("hotrust".to_string());
        if let Err(e) = prog.load_crate(&c.name, &c.src_path, c.edition, cs, deps) {
            println!("load error {}: {}", c.name, e);
            load_errors += 1;
        }
    }
    prog.prelude_crate = Some(core);
    let t1 = Instant::now();
    prog.resolve_imports();
    let t2 = Instant::now();
    let mut lines = 0;
    for f in &prog.files {
        lines += count_lines(&f.src);
    }
    let mut std_rooted = 0;
    let mut other = 0;
    let mut std_paths: Vec<(String, u32)> = Vec::new();
    let mut other_samples = Vec::new();
    for e in &prog.errors {
        let path = match e.find('`') {
            Some(a) => {
                let rest = &e[a + 1..];
                match rest.find('`') {
                    Some(b) => rest[..b].to_string(),
                    None => rest.to_string(),
                }
            }
            None => e.clone(),
        };
        if path.starts_with("std::") || path.starts_with("core::") || path.starts_with("alloc::") {
            std_rooted += 1;
            let mut found = false;
            for x in std_paths.iter_mut() {
                if x.0 == path {
                    x.1 += 1;
                    found = true;
                }
            }
            if !found {
                std_paths.push((path, 1));
            }
        } else {
            other += 1;
            if other_samples.len() < 60 {
                other_samples.push(e.clone());
            }
        }
    }
    let mut n_imports = 0;
    for _ in &prog.imports {
        n_imports += 1;
    }
    println!("crates loaded: {} ({} load errors), files {}, lines {}, defs {}, modules {}", crates.len(), load_errors, prog.files.len(), lines, prog.defs.len(), prog.mods.len());
    println!("load (read+parse+collect, 1 thread) {:.1} ms, import resolution {:.1} ms", (t1 - t0).as_secs_f64() * 1e3, (t2 - t1).as_secs_f64() * 1e3);
    println!("imports: {} total, unresolved {} (std/core/alloc-rooted {}, other {})", n_imports, prog.errors.len(), std_rooted, other);
    std_paths.sort_by(|a, b| b.1.cmp(&a.1));
    println!("distinct std paths needed: {}", std_paths.len());
    let mut out = String::new();
    for (p, n) in &std_paths {
        out.push_str(&format!("{} {}\n", n, p));
    }
    let _ = std::fs::write("std_surface.txt", out);
    println!("(std surface written to std_surface.txt)");
    for e in other_samples {
        println!("  {}", e);
    }
}
