//! `makepad-web-trace`: an app's web build traced on this machine.
//!
//! ```text
//! makepad-web-trace trace [--script <file>] [--window WxH@dpi] [--ms <virtual ms>] [--out <file.json>]
//!                         [--serve /<prefix>=<dir>]... [--query <search>] [--until <text>] [--stress]
//!                         [--record-pack <file>] [--pack <file>]... [--cfg <name>]...
//!                         <package> [cargo flags]
//! makepad-web-trace merge <out.json> <in.json>...
//! makepad-web-trace gate <A.json> <B.json>      replay-B gate: what B ran that A did not (stub hits)
//! ```
//! `trace` reads the package as the web target (`MAKEPAD_RUN_CHECK_TARGET=wasm32-unknown-unknown`:
//! cfg(target_arch = "wasm32"), the web platform backend, the script VM, all from source), boots it
//! in Relax with its `wasm_*` exports published instead of `main` (run.rs `--exports`), and plays
//! the page against them (page.rs) on a virtual clock: the script's input at its times, animation
//! frames at 60 Hz, for `--ms` (default 5000) virtual milliseconds. Then it writes the coverage
//! file (trace.rs): the functions and blocks the program ran (relax coverage.rs), the imports and
//! exports the page used, the messages, the assets with the byte ranges read, the frames' GL
//! digests. `merge` joins several runs' files (two window sizes, several scripts). `--until <text>`
//! ends the run at the first console line holding `<text>` (`stage-player: exit`), else at `--ms`.
//! `--stress` runs the rare-but-required paths too (DESIGN.md "Rare-but-required paths"): the
//! script heap collects at every safepoint (`wasm_script_gc_stress`).
//! Shaders (pass 1 / pass 2 of an export): `--record-pack <file>` records every draw shader the run
//! applies into a shader pack, as the web build compiles it (the pack's GLSL is checked against
//! every program the page compiled: the same text), and writes it; `--pack <file> --cfg
//! makepad_precompiled_shaders` installs it in a build without the shader compiler, so the
//! coverage holds no compiler; a shader the pack lacks is recorded (`pack_misses`). `--cfg` is the
//! program's (as `makepad run --cfg`).
//!
//! `--serve /<prefix>=<dir>` serves `<dir>` as the page's `/<prefix>/` (`--serve /=<dir>`: the
//! page's own folder); other paths are `/<crate>/<path>` of the program's crates. `--query` is the
//! page's `location.search` (`?doc=pdoom/pdoom_film.cut.splash&deterministic`). Every file and
//! directory listing served is recorded with the byte ranges read (`assets`).
//!
//! Any other command line is the loader's (`makepad ...`): build scripts run this binary as Relax.
//!
//! Script: one event per line, `#` comments, times in virtual ms:
//! ```text
//! at <ms> click <x> <y>            mouse down and up (left button) at CSS pixel x, y
//! at <ms> down|up <x> <y>          mouse button 0 down / up
//! at <ms> move <x> <y>
//! at <ms> scroll <x> <y> <dx> <dy>
//! at <ms> text <string>            text input (the rest of the line)
//! at <ms> key <name|code>          key down and up (Return, Escape, Backspace, Tab, Space, arrows, or a number)
//! at <ms> resize <W>x<H>@<dpi>
//! end <ms>                         the run's length
//! ```

mod gl;
mod imports;
mod page;
mod trace;

use page::{Input, Page, Window};

fn parse_window(s: &str) -> Option<Window> {
    let (wh, dpi) = s.split_once('@').unwrap_or((s, "1"));
    let (w, h) = wh.split_once('x')?;
    Some(Window { width: w.parse().ok()?, height: h.parse().ok()?, dpi: dpi.parse().ok()? })
}

fn key_code(name: &str) -> Option<u32> {
    Some(match name {
        "Return" | "Enter" => 13,
        "Escape" => 27,
        "Backspace" => 8,
        "Tab" => 9,
        "Space" => 32,
        "Delete" => 46,
        "ArrowLeft" => 37,
        "ArrowUp" => 38,
        "ArrowRight" => 39,
        "ArrowDown" => 40,
        n => n.parse().ok()?,
    })
}

/// The script's events (seconds) and its end (seconds), or why a line is wrong.
fn parse_script(src: &str) -> Result<(Vec<(f64, Input)>, Option<f64>), String> {
    let mut out = Vec::new();
    let mut end = None;
    for (n, line) in src.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let err = || format!("script line {}: `{}`", n + 1, line);
        let mut it = line.splitn(4, ' ');
        match it.next() {
            Some("end") => end = Some(it.next().and_then(|v| v.parse::<f64>().ok()).ok_or_else(err)? / 1000.0),
            Some("at") => {
                let t = it.next().and_then(|v| v.parse::<f64>().ok()).ok_or_else(err)? / 1000.0;
                let what = it.next().ok_or_else(err)?;
                let rest = it.next().unwrap_or("");
                let nums: Vec<f64> = rest.split_whitespace().filter_map(|v| v.parse().ok()).collect();
                let xy = || if nums.len() >= 2 { Ok((nums[0], nums[1])) } else { Err(err()) };
                match what {
                    "click" => {
                        let (x, y) = xy()?;
                        out.push((t, Input::MouseMove { x, y }));
                        out.push((t, Input::MouseDown { x, y, button: 0 }));
                        out.push((t + 0.05, Input::MouseUp { x, y, button: 0 }));
                    }
                    "down" => { let (x, y) = xy()?; out.push((t, Input::MouseDown { x, y, button: 0 })) }
                    "up" => { let (x, y) = xy()?; out.push((t, Input::MouseUp { x, y, button: 0 })) }
                    "move" => { let (x, y) = xy()?; out.push((t, Input::MouseMove { x, y })) }
                    "scroll" => {
                        if nums.len() < 4 { return Err(err()); }
                        out.push((t, Input::Scroll { x: nums[0], y: nums[1], dx: nums[2], dy: nums[3] }));
                    }
                    "text" => out.push((t, Input::Text(rest.to_string()))),
                    "key" => {
                        let code = key_code(rest.trim()).ok_or_else(err)?;
                        out.push((t, Input::KeyDown { key_code: code, char_code: 0 }));
                        out.push((t + 0.03, Input::KeyUp { key_code: code, char_code: 0 }));
                    }
                    "resize" => out.push((t, Input::Resize(parse_window(rest.trim()).ok_or_else(err)?))),
                    _ => return Err(err()),
                }
            }
            _ => return Err(err()),
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    Ok((out, end))
}

fn trace(args: &[String]) -> Result<(), String> {
    let mut script_path = None;
    let mut window = Window { width: 1280.0, height: 800.0, dpi: 2.0 };
    let mut ms: Option<f64> = None;
    let mut out_path = "web-trace.json".to_string();
    let mut serves: Vec<(String, String)> = Vec::new();
    let mut query = String::new();
    let mut i = 0;
    let mut stress = false;
    let mut record_pack: Option<String> = None;
    let mut install_pack: Vec<Vec<u8>> = Vec::new();
    let mut cfgs: Vec<String> = Vec::new();
    while i < args.len() && args[i].starts_with("--") {
        if args[i] == "--stress" {
            stress = true;
            i += 1;
            continue;
        }
        let v = args.get(i + 1).cloned().ok_or_else(|| format!("{} needs a value", args[i]))?;
        match args[i].as_str() {
            "--script" => script_path = Some(v),
            "--window" => window = parse_window(&v).ok_or_else(|| format!("--window {}: WxH@dpi", v))?,
            "--ms" => ms = Some(v.parse().map_err(|_| format!("--ms {}", v))?),
            "--out" => out_path = v,
            "--until" => *imports::UNTIL.lock().unwrap() = Some(v),
            "--record-pack" => record_pack = Some(v),
            "--pack" => install_pack.push(std::fs::read(&v).map_err(|e| format!("--pack {}: {}", v, e))?),
            "--cfg" => cfgs.push(v),
            "--serve" => {
                let (prefix, dir) = v.split_once('=').ok_or_else(|| format!("--serve {}: /<prefix>=<dir>", v))?;
                let dir = std::fs::canonicalize(dir).map_err(|e| format!("--serve {}: {}", dir, e))?;
                serves.push((prefix.to_string(), dir.to_string_lossy().into_owned()));
            }
            "--query" => query = if v.starts_with('?') || v.is_empty() { v } else { format!("?{}", v) },
            o => return Err(format!("unknown option {}", o)),
        }
        i += 2;
    }
    let rest: Vec<String> = args[i..].to_vec();
    let package = rest.first().cloned().ok_or("trace needs a package")?;
    let (inputs, script_end) = match &script_path {
        Some(p) => parse_script(&std::fs::read_to_string(p).map_err(|e| format!("{}: {}", p, e))?)?,
        None => (Vec::new(), None),
    };
    let end = ms.map(|m| m / 1000.0).or(script_end).unwrap_or(5.0);

    // the web target, its linear memory, coverage, the page's imports: before the program loads
    std::env::set_var("MAKEPAD_RUN_CHECK_TARGET", "wasm32-unknown-unknown");
    std::env::set_var("MAKEPAD_RUN_CACHE", "off");
    relax_compiler::web::activate()?;
    relax_compiler::coverage::enable();
    relax_compiler::web::set_embedder_symbols(imports::symbol);
    makepad_wasm_bridge::MEMORY_BASE.store(relax_compiler::web::BASE as usize, std::sync::atomic::Ordering::Relaxed);

    // not a live session: what runs is the sources it started with (no hot reload of an edit)
    let mut loader_args: Vec<String> = vec!["--quiet".into(), "--no-live".into(), "--exports".into(), "wasm_".into()];
    for c in &cfgs {
        loader_args.push("--cfg".into());
        loader_args.push(c.clone());
    }
    relax_runner::set_host(&makepad_run::HOST);
    let code = relax_runner::run_checkout(loader_args, rest.clone());
    if code != 0 {
        return Err(format!("{} did not boot (status {})", package, code));
    }
    let exports: &'static page::Exports = Box::leak(Box::new(page::Exports::new(relax_compiler::run::exports())));
    if !exports.has("wasm_create_app") {
        return Err(format!("{} exports no wasm_create_app: not an app_main! app", package));
    }

    let mut page = Page::new(exports, window, relax_compiler::run::crate_dirs(), serves.clone(), query.clone());
    page.with_pack(record_pack.is_some(), install_pack);
    exports.record_glyphs(true);
    exports.record_splash(true);
    if stress && !exports.switch("wasm_script_gc_stress", true) {
        return Err("--stress: the program has no wasm_script_gc_stress".into());
    }
    let recorders: Vec<&str> = ["wasm_record_glyphs", "wasm_record_text3d_glyphs", "wasm_record_outline_glyphs"].into_iter().filter(|n| exports.has(n)).collect();
    eprintln!("makepad-web-trace: glyph records: {:?}", recorders);
    page.start();
    page.run(&inputs, end);
    eprintln!("makepad-web-trace: {} at {}x{}@{}: {}", package, window.width, window.height, window.dpi, page.summary());

    // the coverage file
    let up = relax_compiler::jit::current_unit();
    let fns = unsafe { relax_compiler::coverage::functions(up) };
    let mut cov = std::mem::take(&mut page.cov);
    // the Splash the run used (a garbage collection first: what nothing holds is no use)
    if let Some(text) = exports.take_splash_use(page.app()) {
        match relax_compiler::json::parse(text.as_bytes()) {
            Ok(j) => cov.splash.add_record(&j),
            Err(e) => eprintln!("makepad-web-trace: the Splash record does not parse: {}", e),
        }
        for (k, (b, ub, n, un)) in cov.splash.crates() {
            eprintln!("makepad-web-trace: splash {}: {} of {} bytes, {} of {} items kept", k, ub, b, un, n);
        }
    }
    // the shader pack (pass 1): its GLSL is what the page compiled
    let mut pack_note = String::from("null");
    if let Some(path) = &record_pack {
        let pack = exports.take_shader_pack(page.app()).ok_or("--record-pack: the program has no wasm_take_shader_pack")?;
        let contents = makepad_platform::makepad_script::shader_pack::inspect_shader_pack(&pack);
        if let Some(e) = contents.error() {
            return Err(format!("the recorded shader pack does not read: {}", e));
        }
        // The pack holds every draw shader the run's compiler compiled (a shader compiled and not
        // drawn still matters: a pipeline is ready once its shaders are). Each program the page
        // compiled must be an entry's exact GLSL (the web backend prefixes its header: the
        // program ends with the entry's code).
        let entries: Vec<_> = (0..contents.len()).map(|i| contents.entry(i).unwrap()).collect();
        let drawn = entries.iter().filter(|e| page.gl.glsl.iter().any(|(pv, pf)| pv.ends_with(e.vertex()) && pf.ends_with(e.fragment()))).count();
        let page_only = page.gl.glsl.iter().filter(|(pv, pf)| !entries.iter().any(|e| pv.ends_with(e.vertex()) && pf.ends_with(e.fragment()))).count();
        std::fs::write(path, &pack).map_err(|e| format!("{}: {}", path, e))?;
        eprintln!("makepad-web-trace: shader pack: {} entries ({} bytes) -> {}; {} of them drawn (the page compiled their exact GLSL); {} programs the page compiled are in no entry",
            entries.len(), pack.len(), path, drawn, page_only);
        if page_only != 0 {
            return Err(format!("the shader pack misses {} programs the page compiled: not the web backend's code", page_only));
        }
        pack_note = format!("{{\"entries\": {}, \"bytes\": {}, \"drawn\": {}, \"page_programs_not_in_pack\": {}}}", entries.len(), pack.len(), drawn, page_only);
    }
    cov.pack_misses = std::mem::take(&mut *imports::PACK_MISSES.lock().unwrap());
    if !cov.pack_misses.is_empty() {
        eprintln!("makepad-web-trace: {} draw shaders not in the shader pack", cov.pack_misses.len());
    }
    // the glyphs per font file: a record names the file by its bytes' hash
    for (hash, glyphs) in exports.take_glyphs() {
        let url = page.file_hashes.get(&hash).cloned().unwrap_or_else(|| format!("(unserved font {:016x})", hash));
        cov.glyphs.entry(url).or_default().extend(glyphs);
    }
    cov.package = package.clone();
    cov.build = relax_compiler::PROVENANCE.to_string();
    for f in fns {
        cov.functions.insert(f.key.clone(), trace::Func {
            kind: f.kind.to_string(),
            krate: f.krate,
            file: f.file,
            line: f.line as u64,
            col: f.col as u64,
            blocks: f.blocks as u64,
            hit: f.hit.iter().map(|(l, c)| (*l as u64, *c as u64)).collect(),
        });
    }
    for (name, n) in exports.calls.lock().unwrap().iter() {
        *cov.exports.entry(name.to_string()).or_default() += n;
    }
    for (k, name) in imports::NAMES.iter().enumerate() {
        let n = imports::CALLS[k].load(std::sync::atomic::Ordering::Relaxed);
        if n != 0 {
            cov.imports.insert(name.to_string(), n);
        }
    }
    cov.shaders_compiled = page.gl.shaders_compiled as u64;
    let panics: Vec<String> = imports::PANICS.lock().unwrap().iter().map(|p| trace::esc(p)).collect();
    cov.runs.push(format!(
        "{{\"name\": {}, \"script\": {}, \"query\": {}, \"stress\": {}, \"cfgs\": [{}], \"pack\": {}, \"window\": [{}, {}, {}], \"frames\": {}, \"virtual_ms\": {}, \"panics\": [{}]}}",
        trace::esc(&format!("{}@{}x{}", package, window.width, window.height)),
        trace::esc(script_path.as_deref().unwrap_or("")),
        trace::esc(&query),
        stress,
        cfgs.iter().map(|c| trace::esc(c)).collect::<Vec<_>>().join(", "),
        pack_note,
        window.width, window.height, window.dpi, page.frames, (imports::now() * 1000.0).round(), panics.join(", ")
    ));
    for f in &page.gl.frames {
        cov.frames.push(format!("{{\"draw_calls\": {}, \"passes\": {}, \"upload_bytes\": {}, \"texture_bytes\": {}, \"digest\": \"{:016x}\"}}", f.draw_calls, f.passes, f.upload_bytes, f.texture_bytes, f.digest));
    }
    std::fs::write(&out_path, cov.to_json()).map_err(|e| format!("{}: {}", out_path, e))?;
    eprintln!("makepad-web-trace: {} functions, {} assets, {} imports, {} from_wasm / {} to_wasm message kinds -> {}",
        cov.functions.len(), cov.assets.len(), cov.imports.len(), cov.from_wasm.len(), cov.to_wasm.len(), out_path);
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("trace") => {
            let r = trace(&args[2..]);
            if let Err(e) = r {
                eprintln!("makepad-web-trace: {}", e);
                relax_compiler::os::proc::exit(1);
            }
            // the program's threads and the unit live until the process ends
            relax_compiler::os::proc::exit(0);
        }
        Some("gate") if args.len() == 4 => match trace::gate(&args[2], &args[3]) {
            Ok(true) => {}
            Ok(false) => relax_compiler::os::proc::exit(1),
            Err(e) => {
                eprintln!("makepad-web-trace: {}", e);
                relax_compiler::os::proc::exit(2);
            }
        },
        Some("merge") if args.len() >= 4 => {
            if let Err(e) = trace::merge(&args[2], &args[3..]) {
                eprintln!("makepad-web-trace: {}", e);
                relax_compiler::os::proc::exit(1);
            }
        }
        _ => relax_runner::main(&makepad_run::HOST),
    }
}
