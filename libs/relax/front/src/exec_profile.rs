//! File demand census under MAKEPAD_RUN_PROFILE. This measures compilation for a call,
//! not instruction coverage: inlining and calls to already-installed ahead code
//! do not enter the lazy stub. Body queries keep those files out of the strict
//! declaration-only bucket. All state is diagnostic; no load decisions use it.
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use relax_session::sync::Mutex;

pub const LOADED: u32 = 1;
pub const PARSED: u32 = 2;
pub const LOWERED: u32 = 4;
pub const CALL: u32 = 8;
pub const AHEAD: u32 = 16;
pub const SPEC: u32 = 32;
pub const BODY: u32 = 64;
pub const CTFE: u32 = 128;
const SHIFT: u32 = 16;
static PRESENT: AtomicBool = AtomicBool::new(false);
static REPORTED_PRESENT: AtomicBool = AtomicBool::new(false);
static REPORTED_EXIT: AtomicBool = AtomicBool::new(false);
static FILES: Mutex<BTreeMap<String, Arc<File>>> = Mutex::new(BTreeMap::new());

#[derive(Default)]
pub struct File {
    // Low bits: before present; high bits: entire run. One fetch_or per note.
    bits: AtomicU32,
    parse_ns: [AtomicU64; 2],
    lower_ns: [AtomicU64; 2],
    before: OnceLock<(u32, [u64; 2])>,
}

pub fn file(path: &str) -> Arc<File> {
    FILES.lock().unwrap().entry(path.into()).or_default().clone()
}

impl File {
    pub fn note(&self, bit: u32) {
        let before = !PRESENT.load(Ordering::Acquire);
        self.bits.fetch_or((bit << SHIFT) | if before { bit } else { 0 }, Ordering::Relaxed);
    }

    pub fn cost(&self, lower: bool, ns: u64) {
        let before = !PRESENT.load(Ordering::Acquire);
        let bit = if lower { LOWERED } else { PARSED };
        let costs = if lower { &self.lower_ns } else { &self.parse_ns };
        costs[1].fetch_add(ns, Ordering::Relaxed);
        if before { costs[0].fetch_add(ns, Ordering::Relaxed); }
        self.bits.fetch_or((bit << SHIFT) | if before { bit } else { 0 }, Ordering::Relaxed);
    }
}

// The exact slot entering rt_compile, not a thread-wide "inside a call" flag:
// linking that slot may recursively install other slots ahead of their calls.
thread_local! {
    static CALL_SLOT: std::cell::Cell<Option<(usize, u32)>> = const { std::cell::Cell::new(None) };
}
pub struct Call(Option<(usize, u32)>);
pub fn call(unit: usize, slot: u32) -> Call {
    Call(CALL_SLOT.with(|c| c.replace(Some((unit, slot)))))
}
impl Drop for Call {
    fn drop(&mut self) { CALL_SLOT.with(|c| c.set(self.0)); }
}
pub fn is_call(unit: usize, slot: u32) -> bool {
    CALL_SLOT.with(|c| c.get() == Some((unit, slot)))
}

pub fn first_present() { PRESENT.store(true, Ordering::Release); }

/// No Unit lock: workers only publish atomic flags/costs. Never wait for a build
/// at the present boundary. Only completed parse/lower spans are attributed.
pub fn report(first_present: bool) {
    let once = if first_present { &REPORTED_PRESENT } else { &REPORTED_EXIT };
    if once.swap(true, Ordering::AcqRel) { return; }
    let files: Vec<_> = FILES.lock().unwrap().iter().map(|(p, f)| (p.clone(), f.clone())).collect();
    if files.is_empty() { return; }
    use std::io::Write;
    let mut out = std::io::BufWriter::new(std::io::stderr().lock());
    let at = if first_present { "first-present" } else { "exit" };
    // The loaded set partitions into call, declaration-only, and other-body.
    // The remaining groups are overlapping views. A speculative/body-only file is
    // not evidence that just its signatures suffice, even when never called.
    let mut totals = [[0u64; 3]; 9];
    for (path, f) in files {
        let bits = f.bits.load(Ordering::Relaxed);
        // Preserve the first-present split verbatim in the exit report.
        let (pre, costs) = *f.before.get_or_init(|| (bits & 0xffff,
            [f.parse_ns[0].load(Ordering::Relaxed), f.lower_ns[0].load(Ordering::Relaxed)]));
        let all = bits >> SHIFT;
        let loaded = pre & LOADED != 0;
        let called = pre & CALL != 0;
        let body = pre & (BODY | CTFE | AHEAD | SPEC) != 0;
        let decl = loaded && !called && !body;
        let _ = writeln!(out, "exec-file: at={at} path={path:?} loaded_boot={} parsed_boot={} lowered_boot={} call_pre={} ahead_pre={} spec_pre={} body_needed_pre={} ctfe_pre={} declarations_only={} no_call_candidate={} parse_cpu_ns={} lower_cpu_ns={} call_total={} ahead_total={} spec_total={} parse_cpu_ns_total={} lower_cpu_ns_total={}",
            u8::from(loaded), u8::from(pre & PARSED != 0), u8::from(pre & LOWERED != 0), u8::from(called), u8::from(pre & AHEAD != 0), u8::from(pre & SPEC != 0), u8::from(pre & BODY != 0), u8::from(pre & CTFE != 0), u8::from(decl), u8::from(loaded && !called), costs[0], costs[1],
            u8::from(all & CALL != 0), u8::from(all & AHEAD != 0), u8::from(all & SPEC != 0), f.parse_ns[1].load(Ordering::Relaxed), f.lower_ns[1].load(Ordering::Relaxed));
        let groups = [loaded, called, decl, loaded && !called, pre & LOWERED != 0,
            loaded && !called && body, pre != 0 && !loaded, pre & AHEAD != 0, pre & SPEC != 0];
        for (i, yes) in groups.into_iter().enumerate() {
            if !yes { continue; }
            for (j, n) in [1, costs[0], costs[1]].into_iter().enumerate() {
                totals[i][j] += n;
            }
        }
    }
    for (i, name) in ["loaded_boot", "compiled_for_call_pre", "declarations_only", "no_call_candidate", "lowered_boot", "other_body_needed", "discovery_only", "ahead_pre", "spec_pre"].into_iter().enumerate() {
        let t = totals[i];
        let _ = writeln!(out, "exec-summary: at={at} group={name} files={} parse_cpu_ns={} lower_cpu_ns={} present_seen={} metric=compile-for-call", t[0], t[1], t[2], u8::from(PRESENT.load(Ordering::Acquire)));
    }
}
