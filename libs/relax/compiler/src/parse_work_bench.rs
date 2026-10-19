//! Amp's 20-parse medians (parse_work.rs timers) -- a test of the `relax` crate, not of relax-front's:
//! with MAKEPAD_RUN_PARSE_HOSTED=1 it registers the live-id macro host, which is the JIT's (pmhost).
#![cfg(test)]

#[test]
fn amp_parse_medians() {
    isolated_test!(amp_parse_medians);
    let sess = &std::sync::Arc::new(crate::Session::default());
    let makepad = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let path = makepad.join("apps/amp/src/lib.rs");
    let source = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("skip: {} missing", path.display()); return;
        }
        Err(e) => panic!("{}: {e}", path.display()),
    };
    let path = path.to_str().unwrap();
    // Keep the real app's imports/items and exercise one small hosted call.
    // The wrapper keeps the app's inner documentation valid in either order.
    let app_source = format!("mod amp_source {{\n{source}\n}}\n");
    // Optional real native macro host, built solely from existing local sources.
    // No Cargo graph resolution, downloads, app code execution or windows.
    let hosted = std::env::var("MAKEPAD_RUN_PARSE_HOSTED").as_deref() == Ok("1");
    if hosted {
        let roots = [
            ("makepad_micro_proc_macro", "libs/micro_proc_macro/src/lib.rs", false, vec![]),
            ("makepad_live_id_macros_impl", "libs/live_id/id_macros_impl/src/lib.rs", false, vec![(0, "makepad_micro_proc_macro".into())]),
            ("makepad_live_id_macros", "libs/live_id/id_macros/src/lib.rs", true, vec![(1, "makepad_live_id_macros_impl".into())]),
        ];
        let mut units = roots.into_iter().map(|(name, path, proc_macro, deps)| {
            let path = makepad.join(path);
            assert!(path.exists(), "missing macro source: {}", path.display());
            crate::crates::CrateUnit { name: name.into(), src_path: path.to_string_lossy().into_owned(), edition: 2021,
                features: vec![], deps,  proc_macro, bin: false, build_script: false, version: "1.0.0".into() }
        }).collect::<Vec<_>>();
        crate::pmhost::install();
        crate::macro_reg::register_and_strip(sess, &mut units, &crate::std_dir(), false).unwrap();
        crate::program::share_std_end();
    }
    let fallback = if hosted { "" } else {
        "macro_rules! live_id { ($id:ident) => { 0u64 }; }\n"
    };
    let probe = format!("{fallback}fn __parse_hosted() {{ let _ = live_id!(amp_probe_1360); }}\n");
    let src = format!("{app_source}{probe}").into_bytes();
    let mut table = crate::macros::MacroTable::default();
    table.s2_fmt = true;
    let mut snapshot = crate::program::ParseSnapshot {
        old: 0, path: path.into(), edition: 2021, kname: "amp".into(),
        mod_path: "amp".into(), cfg: crate::cfg::CfgSet::host(sess, &[]),
        macros: std::sync::Arc::new(table), reuse: Default::default(), previous: None,
            context: crate::source_context::Context::new(path, Default::default(), sess),
    };
    let macro_first = format!("{probe}{app_source}").into_bytes();
    for (order, source) in [("macro last", &src), ("macro first", &macro_first)] {
        for inside in [false, true] {
            snapshot.reuse = Default::default();
            let mut samples = Vec::new();
            let mut wall = Vec::new();
            // Warm the JIT and reuse before collecting 20 saves.
            for i in 0..21 {
                let mut text = source.clone();
                if inside {
                    let at = std::str::from_utf8(&text).unwrap().find("amp_probe_1360").unwrap()
                        + "amp_probe_".len();
                    text[at..at + 4].copy_from_slice(format!("{}", 1360 + i).as_bytes());
                } else {
                    text.extend_from_slice(format!("\nfn __parse_bench() -> u32 {{ {i} }}\n").as_bytes());
                }
                crate::parse_work::begin();
                let start = std::time::Instant::now();
                let v = if std::env::var("MAKEPAD_RUN_PARSE_REUSE").as_deref() == Ok("0") {
                    snapshot.parse_full(text).unwrap()
                } else { snapshot.parse(text).unwrap() };
                let elapsed = start.elapsed().as_secs_f64() * 1000.;
                let times = crate::parse_work::finish();
                if i > 0 { wall.push(elapsed); samples.push(times); }
                snapshot.reuse = v.reuse.clone();
                std::hint::black_box(v);
            }
            let mut median = crate::parse_work::Times::default();
            for k in 0..crate::parse_work::SLOTS {
                let mut ns: Vec<f64> = samples.iter().map(|s| s.0[k]).collect();
                ns.sort_by(f64::total_cmp);
                median.0[k] = (ns[9] + ns[10]) / 2.;
            }
            if hosted && inside { assert!(median.0[7] > 0., "inside edit must invoke the hosted macro"); }
            wall.sort_by(f64::total_cmp);
            let edit = if inside { "inside live_id" } else { "append fn" };
            eprintln!("reload-parse: {order}, {edit}, hosted={hosted}: total median {:.3} ms", (wall[9] + wall[10]) / 2.);
            assert!(crate::macro_reg::derive_failures().is_empty(), "hosted derive failed");
            median.print("Amp 20-save median");
        }
    }
    // Optional controlled scaling probe, separate from the real-file medians above.
    // It uses the real hosted input but seeds unrelated expansion bytes; PROFILE=1
    // reports the old implementation too, isolating expansion-buffer scan cost.
    if let Ok(mb) = std::env::var("MAKEPAD_RUN_PARSE_EXT_MB") {
        assert!(hosted, "expansion-buffer probe needs MAKEPAD_RUN_PARSE_HOSTED=1");
        let bytes = mb.parse::<usize>().unwrap() * 1024 * 1024;
        let mut samples = Vec::new();
        for i in 0..21 {
            let mut text = src.clone();
            let at = std::str::from_utf8(&text).unwrap().find("amp_probe_1360").unwrap()
                + "amp_probe_".len();
            text[at..at + 4].copy_from_slice(format!("{}", 1360 + i).as_bytes());
            let call_lo = std::str::from_utf8(&text).unwrap().find("live_id!(").unwrap() as u32;
            let mut toks = Vec::new();
            crate::lexer::lex(&text, 2021, &mut toks).unwrap_or_else(|e| panic!("{}", e.msg));
            let call = toks.iter().position(|t| t.lo == call_lo).unwrap();
            let lo = call + 3; // name, !, (, then the identifier input
            let call_hi = toks[lo + 1].hi;
            let mut p = crate::parser::Parser::new(sess, &text, toks, 2021);
            p.bctx = Some(std::rc::Rc::new(crate::builtins::FileCtx::new("amp", path, "amp", &snapshot.cfg)));
            p.ext = vec![b' '; bytes].into();
            let id = crate::builtins::HOSTED + crate::macro_reg::bang_index("live_id").unwrap() as u8;
            crate::parse_work::begin();
            std::hint::black_box(crate::builtins::expand(&p, id, lo, lo + 1, call_lo, call_hi).unwrap());
            let t = crate::parse_work::finish();
            if i > 0 { samples.push(t.0[7]); }
        }
        samples.sort_by(f64::total_cmp);
        eprintln!("reload-parse: synthetic {mb} MiB prior expansion, inside live_id: input_tokens median {:.3} ms", (samples[9] + samples[10]) / 2.);
    }

}
