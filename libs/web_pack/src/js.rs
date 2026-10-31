//! The platform's web runtime as ONE JavaScript module for a packed app,
//! taken from the source tree at build time (never a copy kept here): the
//! wasm bridge, the browser host (`web.js`) and its WebGL backend
//! (`web_gl.js`) concatenated with their imports and exports resolved,
//! minus the marked sections the app does not use (`// @section <name>`
//! ... `// @end <name>` in those files; [`SECTIONS`]), plus the audio
//! worklet as an inline module and the app's start (`start_app`: fetch the
//! wasm, brotli when the host or the browser can decode it, plain
//! otherwise). Not minified: brotli takes the slack, and it stays readable.

use std::path::Path;

/// The platform's JavaScript, in concatenation order (path from the
/// source root).
pub const RUNTIME_JS: [&str; 3] = ["libs/wasm_bridge/src/wasm_bridge.js", "platform/src/os/web/web.js", "platform/src/os/web/web_gl.js"];
pub const AUDIO_WORKLET_JS: &str = "platform/src/os/web/audio_worklet.js";
/// The worker a threaded build's threads run in (a module without imports).
pub const WEB_WORKER_JS: &str = "platform/src/os/web/web_worker.js";

/// Every marked section of the runtime, by what it serves (the manifest's
/// list).
pub const SECTIONS: [&str; 14] = makepad_web_manifest::JS_SECTIONS;

/// The sections a playback-only app (a Stage film) strips: text input and
/// IME, the clipboard, file dialogs and drops, MIDI, XR, websockets, live
/// reload, storage, crash uploads (a static host has no endpoint), browser
/// history, geolocation, the legacy XHR path and permission prompts. Video
/// playback stays, for documents with video layers.
pub const PLAYBACK_JS_STRIPPED: [&str; 13] = [
    "text-input", "clipboard", "file-dialog", "midi", "xr", "websocket", "live-reload", "storage", "crash-upload", "history", "geolocation", "legacy-http", "permissions",
];

/// Removes the sections named in `strip` (`// @section <name>` through
/// `// @end <name>`, both lines included). Unbalanced markers are an error:
/// a half-stripped runtime would fail in the browser instead of here.
pub fn strip_sections(source: &str, strip: &[&str], file: &str) -> Result<String, String> {
    let mut out = String::with_capacity(source.len());
    let mut skipping: Option<String> = None;
    for (i, line) in source.lines().enumerate() {
        let t = line.trim();
        if let Some(name) = t.strip_prefix("// @section ") {
            let name = name.trim();
            if skipping.is_some() {
                return Err(format!("{file}:{}: section `{name}` starts inside another", i + 1));
            }
            if strip.contains(&name) {
                skipping = Some(name.to_string());
                continue;
            }
        } else if let Some(name) = t.strip_prefix("// @end ") {
            if skipping.as_deref() == Some(name.trim()) {
                skipping = None;
                continue;
            }
        }
        if skipping.is_none() {
            out.push_str(line);
            out.push('\n');
        }
    }
    if let Some(name) = skipping {
        return Err(format!("{file}: section `{name}` has no `// @end {name}`"));
    }
    Ok(out)
}

/// One ES module out of the runtime's modules: their `import` lines go
/// (they import each other), `export` keywords go (one scope).
fn unmodule(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut in_import = false;
    for line in source.lines() {
        let t = line.trim_start();
        if in_import {
            if t.contains(" from ") || t.ends_with(';') {
                in_import = false;
            }
            continue;
        }
        if t.starts_with("import ") && !t.starts_with("import(") {
            in_import = !(t.contains(" from ") || t.ends_with(';'));
            continue;
        }
        if let Some(rest) = t.strip_prefix("export ") {
            let indent = &line[..line.len() - t.len()];
            out.push_str(indent);
            out.push_str(rest);
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}


/// The runtime as one module for an app whose wasm is `<wasm_name>.wasm`
/// (and `.wasm.br`), without the sections in `stripped`.
pub fn runtime_js(source_root: &Path, stripped: &[&str], wasm_name: &str) -> Result<String, String> {
    let mut js = format!("// {wasm_name}.js: Makepad's web runtime and the app's start (packed by makepad-web-pack).\n");
    js.push_str(&runtime_body(source_root, stripped)?);
    js.push_str(&START_JS.replace("__WASM__", wasm_name));
    Ok(js)
}

/// The runtime without an app's start: the wasm bridge, the browser host,
/// its WebGL backend, the audio worklet and the threads' worker (as blob
/// sources). A packer appends its own start (`relax export-web`'s fetches
/// one deflated wasm).
pub fn runtime_body(source_root: &Path, stripped: &[&str]) -> Result<String, String> {
    let read = |rel: &str| std::fs::read_to_string(source_root.join(rel)).map_err(|e| format!("{}: {e}", source_root.join(rel).display()));
    let mut js = String::new();
    for rel in RUNTIME_JS {
        let source = strip_sections(&read(rel)?, stripped, rel)?;
        js.push_str(&format!("\n// ---- {rel}\n"));
        js.push_str(&unmodule(&source));
    }
    if !stripped.contains(&"threads") {
        let worklet = read(AUDIO_WORKLET_JS)?;
        js.push_str("\n// ---- the audio worklet, loaded from a blob (one file)\n");
        js.push_str(&format!("const MAKEPAD_AUDIO_WORKLET_SOURCE = {};\n", js_string(&worklet)));
        let worker = read(WEB_WORKER_JS)?;
        js.push_str("\n// ---- the threads' worker, loaded from a blob (threaded builds)\n");
        js.push_str(&format!("const MAKEPAD_WEB_WORKER_SOURCE = {};\n", js_string(&worker)));
    }
    Ok(js)
}

/// The runtime and the start of a page whose ONE `<wasm_name>.wasm` ships raw
/// (`wasm-export`; the host compresses it on the wire): fetched once. The rest
/// of the start is [`runtime_js`]'s.
pub fn one_wasm_js(source_root: &Path, stripped: &[&str], wasm_name: &str) -> Result<String, String> {
    let from = START_JS.find(FETCH_START).ok_or("web_pack: START_JS lost its fetch")?;
    let to = START_JS.find(APP_START).ok_or("web_pack: START_JS lost start_app")?;
    let start = format!("{}{}{}", &START_JS[..from], ONE_WASM_FETCH_JS, &START_JS[to..]);
    let start = strip_sections(&start, &["threads"], "single-thread startup")?;
    let start = start.replace("makepad_crash_reporter.set_wasm(wasm);", "if (wasm._has_thread_support) throw new Error(\"This page requires an unshared wasm memory\");\n    makepad_crash_reporter.set_wasm(wasm);");
    let mut stripped = stripped.to_vec();
    stripped.push("threads");
    let mut js = format!("// {wasm_name}.js: Makepad's web runtime and the page's start (one wasm).\n");
    js.push_str(&runtime_body(source_root, &stripped)?);
    js.push_str(&start.replace("__WASM__", wasm_name));
    Ok(js)
}

const FETCH_START: &str = "// The wasm's bytes: __WASM__.wasm.br";
const APP_START: &str = "export async function start_app";

/// Static export accesses in our generated glue. Computed accesses are rejected:
/// adding a new callback must extend the package contract, never evade it.
pub fn export_references(js: &str) -> Result<std::collections::BTreeSet<String>, String> {
    let mut names = std::collections::BTreeSet::new();
    let tokens: Vec<&str> = js.split(|c: char| c.is_whitespace()).filter(|s| !s.is_empty()).collect();
    let compact = tokens.concat();
    for rest in compact.split(".exports").skip(1) {
        if rest.starts_with('[') || rest.starts_with("?.") {
            return Err("computed wasm export access in packaged glue".into());
        }
        if let Some(rest) = rest.strip_prefix('.') {
            let n = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$')).unwrap_or(rest.len());
            if n == 0 { return Err("invalid wasm export access in packaged glue".into()); }
            names.insert(rest[..n].to_string());
        }
    }
    Ok(names)
}

/// Check every static glue access against the linked module before writing a
/// page. Thread-only accesses have already been removed for this profile.
pub fn check_exports(js: &str, wasm: &[u8]) -> Result<(), String> {
    fn leb(bytes: &[u8], at: &mut usize) -> Result<usize, String> {
        let mut value = 0usize;
        for shift in (0..35).step_by(7) {
            let b = *bytes.get(*at).ok_or("truncated wasm export contract")?;
            *at += 1;
            if shift == 28 && b > 15 { return Err("invalid wasm u32".into()); }
            value |= ((b & 127) as usize) << shift;
            if b < 128 { return Ok(value); }
        }
        Err("invalid wasm u32".into())
    }
    if wasm.get(..8) != Some(b"\0asm\x01\0\0\0") { return Err("invalid wasm header".into()); }
    let mut exports = std::collections::BTreeMap::new();
    let mut at = 8;
    while at < wasm.len() {
        let section = wasm[at]; at += 1;
        let size = leb(wasm, &mut at)?;
        let end = at.checked_add(size).filter(|end| *end <= wasm.len()).ok_or("truncated wasm section")?;
        if section == 5 {
            let mut p = at;
            if leb(wasm, &mut p)? != 1 || leb(wasm, &mut p)? & 2 != 0 {
                return Err("single-thread page requires one unshared wasm memory".into());
            }
        }
        if section == 7 {
            let bytes = &wasm[at..end];
            let mut p = 0;
            for _ in 0..leb(bytes, &mut p)? {
                let len = leb(bytes, &mut p)?;
                let name_end = p.checked_add(len).ok_or("invalid export name length")?;
                let name = std::str::from_utf8(bytes.get(p..name_end).ok_or("truncated export name")?).map_err(|e| e.to_string())?;
                p = name_end;
                let kind = *bytes.get(p).ok_or("missing export kind")?; p += 1;
                leb(bytes, &mut p)?;
                exports.insert(name.to_string(), kind);
            }
        }
        at = end;
    }
    for name in export_references(js)? {
        let kind = if name == "memory" { 2 } else if name == "__indirect_function_table" { 1 } else { 0 };
        if exports.get(&name) != Some(&kind) {
            return Err(format!("packaged glue requires missing wasm export {name} (kind {kind})"));
        }
    }
    Ok(())
}

const ONE_WASM_FETCH_JS: &str = r#"// The wasm's bytes: __WASM__.wasm, raw (the host compresses it on the wire).
async function pack_fetch_wasm(sizes, on_progress) {
    const response = await fetch("__WASM__.wasm");
    if (!response.ok) throw new Error(`__WASM__.wasm: ${response.status}`);
    const bytes = await pack_read(response, got => on_progress(got / sizes.raw));
    if (!pack_is_wasm(bytes)) throw new Error("__WASM__.wasm: not a wasm module");
    return bytes;
}

"#;

/// A JavaScript string literal.
fn js_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            '<' => out.push_str("\\x3c"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}


/// The app's start: fetch the wasm (brotli when the host serves it as
/// such or the browser can decode it; plain otherwise), compile, hand it
/// to the WebGL host, and report progress to the page's loader.
const START_JS: &str = r#"
// ---- the app's start
// @section threads
const pack_audio_worklet_url = URL.createObjectURL(new Blob([MAKEPAD_AUDIO_WORKLET_SOURCE], { type: "text/javascript" }));
globalThis.makepad_audio_worklet_url = pack_audio_worklet_url;
globalThis.makepad_web_worker_url = URL.createObjectURL(new Blob([MAKEPAD_WEB_WORKER_SOURCE], { type: "text/javascript" }));
// @end threads

function pack_is_wasm(bytes) {
    return bytes.length >= 4 && bytes[0] === 0 && bytes[1] === 0x61 && bytes[2] === 0x73 && bytes[3] === 0x6d;
}

function pack_brotli_stream_supported() {
    try { new DecompressionStream("brotli"); return true; } catch (_) { return false; }
}

// Reads a response body to the end, reporting bytes as they arrive.
async function pack_read(response, on_bytes) {
    const reader = response.body.getReader();
    const chunks = [];
    let got = 0;
    for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        chunks.push(value);
        got += value.length;
        on_bytes(got);
    }
    const out = new Uint8Array(got);
    let at = 0;
    for (const c of chunks) { out.set(c, at); at += c.length; }
    return out;
}

// The wasm's bytes: __WASM__.wasm.br decoded by the browser (a host that sends
// it as Content-Encoding: br) or by DecompressionStream, else __WASM__.wasm.
async function pack_fetch_wasm(sizes, on_progress) {
    try {
        const response = await fetch("__WASM__.wasm.br");
        if (response.ok && response.body) {
            const encoded = (response.headers.get("content-encoding") || "").includes("br");
            if (encoded) {
                return await pack_read(response, got => on_progress(got / sizes.raw));
            }
            if (pack_brotli_stream_supported()) {
                const decoded = new Response(response.body.pipeThrough(new DecompressionStream("brotli")));
                const bytes = await pack_read(decoded, got => on_progress(got / sizes.raw));
                if (pack_is_wasm(bytes)) return bytes;
            } else {
                // Some hosts decode without saying so: look at the first bytes.
                const reader = response.body.getReader();
                const first = await reader.read();
                if (!first.done && pack_is_wasm(first.value)) {
                    const rest = await pack_read(new Response(new ReadableStream({
                        start(c) { c.enqueue(first.value); },
                        async pull(c) { const r = await reader.read(); if (r.done) c.close(); else c.enqueue(r.value); },
                    })), got => on_progress(got / sizes.raw));
                    return rest;
                }
                reader.cancel();
            }
        }
    } catch (_) {
    }
    const response = await fetch("__WASM__.wasm");
    if (!response.ok) throw new Error(`__WASM__.wasm: ${response.status}`);
    return await pack_read(response, got => on_progress(got / sizes.raw));
}

export async function start_app(canvas, sizes, on_progress) {
    const bytes = await pack_fetch_wasm(sizes, on_progress);
    const limits = WasmBridge.parse_wasm_memory_limits(bytes);
    const module = await WebAssembly.compile(bytes);
    const wasm = await WasmBridge.instantiate_wasm(module, undefined, { _post_signal: _ => { } }, limits);
    if (!(wasm instanceof WebAssembly.Instance) && !(wasm && wasm.exports)) throw wasm;
    makepad_crash_reporter.set_wasm(wasm);
    return new WasmWebGL(wasm, {}, canvas);
}
"#;


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_strip_whole_and_refuse_unbalanced() {
        let src = "a\n// @section midi\nmidi()\n// @end midi\nb\n// @section xr\nxr()\n// @end xr\n";
        assert_eq!(strip_sections(src, &["midi"], "t.js").unwrap(), "a\nb\n// @section xr\nxr()\n// @end xr\n");
        assert!(strip_sections("// @section midi\nx\n", &["midi"], "t.js").is_err());
    }

    #[test]
    fn the_one_wasm_start_fetches_one_file() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let js = one_wasm_js(&root, &SECTIONS, "app").unwrap();
        assert!(js.contains("export async function start_app") && js.contains("fetch(\"app.wasm\")") && !js.contains("DecompressionStream(\"deflate\")"));
        assert!(!js.contains("app.wasm.br"));
    }

    #[test]
    fn modules_become_one_scope() {
        let src = "import { A,\n  B } from \"./a.js\";\nexport class C {}\nexport function f() {}\nconst x = import(\"y\");\n";
        assert_eq!(unmodule(src), "class C {}\nfunction f() {}\nconst x = import(\"y\");\n");
    }

    #[test]
    fn the_real_runtime_strips_cleanly() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let all = runtime_js(&root, &[], "app").unwrap();
        let stripped = runtime_js(&root, &SECTIONS, "app").unwrap();
        assert!(stripped.len() < all.len());
        assert!(stripped.contains("export async function start_app") && stripped.contains("\"app.wasm.br\""));
    }

    /// A top-level function or constant a section defines is used only
    /// inside that section: stripping the section must not leave a call to
    /// it (a helper added inside a section by mistake).
    #[test]
    fn section_definitions_stay_inside_their_section() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../platform/src/os/web");
        for file in ["web.js", "web_gl.js"] {
            let src = std::fs::read_to_string(root.join(file)).unwrap();
            for section in SECTIONS {
                let begin = format!("// @section {section}");
                let end = format!("// @end {section}");
                let mut inside = String::new();
                let mut rest = src.as_str();
                while let Some(b) = rest.find(&begin) {
                    let after = &rest[b..];
                    let e = after.find(&end).expect("balanced");
                    inside.push_str(&after[..e]);
                    rest = &after[e..];
                }
                let outside = strip_sections(&src, &[section], file).unwrap();
                // Top-level definitions only (column 0): locals of a method
                // are the method's own.
                for line in inside.lines() {
                    let name = line
                        .strip_prefix("function ")
                        .or_else(|| line.strip_prefix("const "))
                        .and_then(|r| r.split(|c: char| !(c.is_alphanumeric() || c == '_')).next())
                        .filter(|n| n.len() > 3);
                    if let Some(name) = name {
                        assert!(!outside.contains(name), "{file}: `{name}` is defined in section {section} but used outside it");
                    }
                }
            }
        }
    }
}
