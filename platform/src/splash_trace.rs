//! The record of the Splash a run used (local/agent_state/wasm/DESIGN.md
//! "Splash shaking"): a web build's under the trace host
//! (`makepad-web-trace`), or a native run's with
//! `MAKEPAD_SPLASH_TRACE=<file>` (written at shutdown, or by
//! [`Cx::splash_trace_exit`] before a process exits on its own).
//!
//! The host calls `wasm_record_splash(1)` before it creates the app, so the
//! module census records from the first `script_mod` on, and
//! `wasm_take_splash_use(cx)` after the run: every `script_mod!` block and
//! its top-level definitions, each named by a stable id (the registering
//! module's path and the definition's path, `makepad_widgets::button::
//! mod.widgets.Button`; a statement that defines nothing by its place
//! among its module's statements, `makepad_widgets::button#0.3`), with its
//! bytes, its byte range in the block's text (a whole top-level statement:
//! blanking it to spaces leaves a block that still parses), the block's text
//! hash (FNV-1a 64), and whether the run used it ([`makepad_script_census::top_use`]),
//! and the modules the census saw registered and used. Nothing is
//! rewritten: the record names the original source.

use crate::cx::Cx;
use std::sync::atomic::{AtomicBool, Ordering};

static RECORD: AtomicBool = AtomicBool::new(false);

/// Whether the record was asked for (read as the Cx is made).
pub(crate) fn requested() -> bool {
    RECORD.load(Ordering::Relaxed) || native_path().is_some()
}

/// `MAKEPAD_SPLASH_TRACE=<file>` (native runs only).
fn native_path() -> Option<std::path::PathBuf> {
    if cfg!(target_arch = "wasm32") {
        return None;
    }
    std::env::var_os("MAKEPAD_SPLASH_TRACE").filter(|p| !p.is_empty()).map(Into::into)
}

static WRITTEN: AtomicBool = AtomicBool::new(false);

impl Cx {
    /// A native run with `MAKEPAD_SPLASH_TRACE=<file>`: write the record there (once). Runs at
    /// shutdown; a program that exits on its own calls it first.
    pub fn splash_trace_exit(&mut self) {
        let Some(path) = native_path() else { return };
        if WRITTEN.swap(true, Ordering::Relaxed) {
            return;
        }
        let json = splash_use_json(self);
        if let Err(e) = std::fs::write(&path, json) {
            crate::error!("MAKEPAD_SPLASH_TRACE {}: {e}", path.display());
        }
    }
}

fn fnv64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The record as JSON:
/// `{"modules": {"registered": [..], "used": {module: by}}, "blocks": [{"module", "crate", "file",
/// "line", "column", "bytes", "hash", "items": [{"id", "path", "start", "end", "bytes", "used": true|false|null}]}]}`
/// (`start`/`end`: byte offsets in the block's text; `hash`: FNV-1a 64 of that text; `used` null: the
/// statement defines no object, a `use` or a registration, and stays).
pub fn splash_use_json(cx: &mut Cx) -> String {
    let (blocks, modules) = cx.with_vm(|vm| (makepad_script_census::top_use::top_use(vm), makepad_script_census::census_used_from(vm, &[])));
    let mut out = String::from("{\"modules\": ");
    match modules {
        Some(m) => {
            out.push_str("{\"registered\": [");
            out.push_str(&m.registered.iter().map(|s| esc(s)).collect::<Vec<_>>().join(", "));
            out.push_str("], \"used\": {");
            out.push_str(&m.used.iter().map(|(k, v)| format!("{}: {}", esc(k), esc(v))).collect::<Vec<_>>().join(", "));
            out.push_str("}}");
        }
        None => out.push_str("null"),
    }
    out.push_str(", \"blocks\": [");
    let mut ordinal: std::collections::HashMap<String, usize> = Default::default();
    for (bi, b) in blocks.iter().enumerate() {
        // What names the block: its module, else its file.
        let owner = if b.module_path.is_empty() { b.file.clone() } else { b.module_path.clone() };
        let n = ordinal.entry(owner.clone()).or_default();
        let block_no = *n;
        *n += 1;
        // A block registered without a module path (a lazy group's text)
        // is named by its file.
        let krate = match b.module_path.split("::").next() {
            Some(k) if !k.is_empty() => k.to_string(),
            _ => format!("(file) {}", b.file),
        };
        if bi > 0 {
            out.push_str(", ");
        }
        out.push_str(&format!("{{\"module\": {}, \"crate\": {}, \"file\": {}, \"line\": {}, \"column\": {}, \"bytes\": {}, \"hash\": \"{:016x}\", \"items\": [", esc(&b.module_path), esc(&krate), esc(&b.file), b.line, b.column, b.code.len(), fnv64(b.code.as_bytes())));
        for (si, (s, used)) in b.statements.iter().enumerate() {
            if si > 0 {
                out.push_str(", ");
            }
            let path = s.path.join(".");
            let id = if s.name().is_some() { format!("{owner}::{path}") } else { format!("{owner}#{block_no}.{si}") };
            let used = match used {
                None => "null",
                Some(true) => "true",
                Some(false) => "false",
            };
            out.push_str(&format!("{{\"id\": {}, \"path\": {}, \"start\": {}, \"end\": {}, \"bytes\": {}, \"used\": {}}}", esc(&id), esc(&path), s.start, s.end, s.end - s.start, used));
        }
        out.push_str("]}");
    }
    out.push_str("]}");
    out
}

/// The trace host's switch: record the Splash use from the next Cx on.
#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn wasm_record_splash(on: u32) {
    RECORD.store(on != 0, Ordering::Relaxed);
}

/// The trace host's read: `[bytes, utf-8 JSON...]` as u64 words, the JSON
/// of [`splash_use_json`] (the host keeps it).
///
/// # Safety
/// `cx_ptr` is the Cx `wasm_create_app` returned.
#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn wasm_take_splash_use(cx_ptr: u32) -> u32 {
    let cx = &mut *(cx_ptr as *mut Cx);
    let json = splash_use_json(cx);
    let mut words = vec![json.len() as u64];
    words.extend(json.as_bytes().chunks(8).map(|c| {
        let mut w = [0u8; 8];
        w[..c.len()].copy_from_slice(c);
        u64::from_le_bytes(w)
    }));
    Box::leak(words.into_boxed_slice()).as_ptr() as usize as u32
}
