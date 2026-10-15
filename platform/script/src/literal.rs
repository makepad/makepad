//! Edit mode's literal-site map: every place a literal written in an edited
//! file lands, so an editor's change of that literal reaches all of them for
//! the next frame without a reload or a recompile.
//!
//! A literal is named by its [`LiteralSite`]: the file (by the name its
//! compiled copies give it) and the zero-based line and column of its first
//! character, in the text the running program was loaded from. Its value
//! lands in:
//! - the VM's code (the immediate that pushes it: per-frame code computes
//!   with it again each frame) and what load-time code stored it in as is
//!   (a `let`, a record's field), patched by the host that owns the VM;
//! - shader slots: compiled under edit mode, a shader reads the literal from
//!   a uniform slot instead of a folded constant (`Cx::patch_literal`);
//! - kernel parameters: a compute kernel reads it from a hidden parameter.
//!
//! Each of these is a [`LiteralSink`]; [`patch_sinks`] reaches every live
//! sink. A value computed from the literal while the program loaded (mixed,
//! passed to a call, a loop bound) is not anywhere it can be patched: the
//! sink says so in [`LiteralReach::reload`] and the editor reloads then.
//! Off (no edited files), nothing is lifted, nothing is recorded, and
//! compiled code is byte-identical to a build without the map.
use makepad_relax_api::relax_api;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};

/// Where a literal is written: a file and the zero-based line and column
/// (in characters) of its first character (a number's digits, past any
/// sign; a colour's `#`).
#[relax_api(handle, clone, eq, debug)]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LiteralSite {
    pub file: String,
    pub line: u32,
    pub col: u32,
}

/// A literal's new value, as an editor writes it: a number (the digits as
/// written, without a sign before them) or a colour's four channels (0..1).
#[relax_api(plain)]
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(u8)]
pub enum LiteralValue {
    Number(f64),
    Color([f32; 4]),
}

/// What a patch reached: how many places now hold the new value, and why a
/// reload is still needed when the literal also fed something that cannot
/// be patched (a value computed from it while the program loaded).
#[relax_api(handle, default)]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiteralReach {
    pub places: usize,
    pub reload: Vec<String>,
}

/// What a [`LiteralSite`] holds, as views.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LiteralSiteView<'a> {
    pub file: &'a str,
    pub line: u32,
    pub col: u32,
}

#[relax_api]
impl LiteralSite {
    #[relax_api]
    pub fn new(file: &str, line: u32, col: u32) -> Self { Self { file: file.to_owned(), line, col } }
    #[relax_api]
    pub fn view(&self) -> LiteralSiteView<'_> { LiteralSiteView { file: &self.file, line: self.line, col: self.col } }
}

impl LiteralSite {
    #[inline(always)]
    pub fn file(&self) -> &str { self.view().file }
    #[inline(always)]
    pub fn line(&self) -> u32 { self.view().line }
    #[inline(always)]
    pub fn col(&self) -> u32 { self.view().col }
}

/// What a [`LiteralReach`] holds, as views ([`LiteralReach::view`]).
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LiteralReachView<'a> {
    pub places: usize,
    pub reload_count: usize,
    /// The reload reason at the index asked for ("" past the end).
    pub reason: &'a str,
}

/// What [`LiteralReach::edit`] does.
pub const REACH_SET_PLACES: u32 = 0;
pub const REACH_ADD_RELOAD: u32 = 1;

#[relax_api]
impl LiteralReach {
    #[relax_api]
    pub fn view(&self, reason_index: usize) -> LiteralReachView<'_> {
        LiteralReachView { places: self.places, reload_count: self.reload.len(), reason: self.reload.get(reason_index).map_or("", |s| s.as_str()) }
    }

    /// [`REACH_SET_PLACES`]: the places `n`; [`REACH_ADD_RELOAD`]: the reason `text`, unless it is there.
    #[relax_api]
    pub fn edit(&mut self, op: u32, n: usize, text: &str) {
        match op {
            REACH_SET_PLACES => self.places = n,
            _ => {
                if !self.reload.iter().any(|s| s == text) {
                    self.reload.push(text.to_owned());
                }
            }
        }
    }
}

impl LiteralReach {
    #[inline(always)]
    pub fn places(&self) -> usize { self.view(0).places }
    #[inline(always)]
    pub fn set_places(&mut self, places: usize) { self.edit(REACH_SET_PLACES, places, ""); }
    #[inline(always)]
    pub fn add_reload(&mut self, reason: &str) { self.edit(REACH_ADD_RELOAD, 0, reason); }
    #[inline(always)]
    pub fn reload_count(&self) -> usize { self.view(0).reload_count }
    #[inline(always)]
    pub fn reload_reason(&self, index: usize) -> &str { self.view(index).reason }

    pub fn add(&mut self, other: LiteralReach) {
        self.places += other.places;
        for why in other.reload {
            if !self.reload.contains(&why) {
                self.reload.push(why);
            }
        }
    }

    /// Whether the editor must reload to show the value everywhere.
    #[inline(always)]
    pub fn needs_reload(&self) -> bool {
        self.reload_count() > 0
    }
}

/// Rows `rows` of a compiled text (zero-based, as its tokenizer or parser
/// counts them) are a copy of `file` from `line`, `col` on: the copy begins
/// at column `code_col` of its first row (text before it there is the
/// host's), and the rows after it are the file's lines as written.
#[relax_api(handle, clone)]
#[derive(Clone, Debug, PartialEq)]
pub struct LiteralOrigin {
    pub rows: std::ops::Range<u32>,
    pub code_col: u32,
    pub file: String,
    pub line: u32,
    pub col: u32,
}

#[relax_api]
impl LiteralOrigin {
    #[relax_api]
    pub fn new(row_start: u32, row_end: u32, code_col: u32, file: &str, line: u32, col: u32) -> Self {
        Self { rows: row_start..row_end, code_col, file: file.to_owned(), line, col }
    }

    /// The origins of `copy` (text written in `file` from zero-based
    /// `line`, `col` on) inside a host's generated `code`, row by row: the
    /// copy's first row ends a row of `code`, and every row after it that
    /// reads as written is the file's next line. Rows the host rewrote (a
    /// substitution) are left out, so their literals stay folded rather
    /// than be named by a place they are not at. Empty when the copy's
    /// first row is not in `code`.
    #[inline(always)]
    pub fn of_copy(code: &str, copy: &str, file: &str, line: u32, col: u32) -> Vec<LiteralOrigin> {
        let mut out = Vec::new();
        Self::of_copy_into(code, copy, file, line, col, &mut ScriptOriginsOut::new(&mut out));
        out
    }
    #[relax_api]
    pub fn of_copy_into(code: &str, copy: &str, file: &str, line: u32, col: u32, sink: &mut ScriptOriginsOut) {
        let rows: Vec<&str> = code.split('\n').collect();
        let lines: Vec<&str> = copy.split('\n').collect();
        let Some(first) = lines.first().filter(|l| !l.trim().is_empty()) else {
            return;
        };
        let Some(r0) = rows.iter().position(|row| row.ends_with(first)) else {
            return;
        };
        let mut out: Vec<LiteralOrigin> = Vec::new();
        for (k, text) in lines.iter().enumerate() {
            let r = r0 + k;
            let Some(row) = rows.get(r) else { break };
            if k > 0 && row != text {
                continue;
            }
            let r = r as u32;
            match out.last_mut() {
                Some(o) if k > 0 && o.rows.end == r && o.line + (r - o.rows.start) == line + k as u32 => o.rows.end = r + 1,
                _ => out.push(LiteralOrigin {
                    rows: r..r + 1,
                    code_col: if k == 0 { (row.chars().count() - first.chars().count()) as u32 } else { 0 },
                    file: file.to_string(),
                    line: line + k as u32,
                    col: if k == 0 { col } else { 0 },
                }),
            }
        }
        for origin in &out { sink.push(origin); }
    }
}

/// Edit mode's state: the files being edited, and the compiled texts (by
/// the module name they are compiled under) whose rows are copies of a
/// file. A literal on such a row of an edited file is live.
#[relax_api(handle, default)]
#[derive(Clone, Debug, Default)]
pub struct LiveLiterals {
    pub edited: Vec<String>,
    pub sources: HashMap<String, Vec<LiteralOrigin>>,
}

impl LiveLiterals {
    pub fn is_edited(&self, file: &str) -> bool { self.edited.iter().any(|f| f == file) }

    /// The site of the literal at `row`, `col` of compiled text `module`,
    /// if that row is a copy of an edited file.
    pub fn site(&self, module: &str, row: u32, col: u32) -> Option<LiteralSite> {
        let origin = self.sources.get(module)?.iter().find(|o| o.rows.contains(&row) && self.is_live(&o.file))?;
        site_in(origin, row, col)
    }

    /// Whether literals copied from `file` are live: an edited file, or a
    /// host's generated text ([`GENERATED`]).
    pub fn is_live(&self, file: &str) -> bool {
        file.starts_with(GENERATED) || self.edited.iter().any(|f| f == file)
    }

    /// The site of the literal at `row`, `col` of a text whose origins are
    /// `origins` (a host's own text, not registered by module), if live.
    pub fn site_in(&self, origins: &[LiteralOrigin], row: u32, col: u32) -> Option<LiteralSite> {
        let origin = origins.iter().find(|o| o.rows.contains(&row) && self.is_live(&o.file))?;
        site_in(origin, row, col)
    }
}

fn site_in(origin: &LiteralOrigin, row: u32, col: u32) -> Option<LiteralSite> {
    let col = if row == origin.rows.start { (col.checked_sub(origin.code_col)?) + origin.col } else { col };
    Some(LiteralSite { file: origin.file.clone(), line: origin.line + (row - origin.rows.start), col })
}

/// The file-name prefix of a host's generated text (values a host baked
/// into code it wrote, not a file anyone edits): live whenever edit mode
/// is on, patched by that host as its values change.
pub const GENERATED: &str = "generated:";

static ON: AtomicBool = AtomicBool::new(false);
static LIVE: RwLock<Option<Arc<LiveLiterals>>> = RwLock::new(None);

/// literal's op.
pub const LIT_EDIT_MODE: u32 = 0;
pub const LIT_SET_EDITED: u32 = 1;
pub const LIT_SET_SOURCE: u32 = 2;
pub const LIT_REGISTER_SINK: u32 = 3;
pub const LIT_PATCH: u32 = 4;

/// What [`literal`] works on, by op: LIT_SET_EDITED reads `edited`; LIT_SET_SOURCE `module` and
/// `origins`; LIT_REGISTER_SINK moves in the record at `sink`; LIT_PATCH patches `site` with
/// `value` and writes what it reached to `reach`.
#[relax_api(plain)]
#[repr(C)]
pub struct LiteralReq<'a> {
    pub edited: *const LiveLiterals,
    pub module: &'a str,
    pub origins: &'a [LiteralOrigin],
    pub sink: *const LiteralSinkRegistration,
    pub site: *const LiteralSite,
    pub value: LiteralValue,
    pub reach: *mut LiteralReach,
}

impl<'a> LiteralReq<'a> {
    #[inline(always)]
    pub fn new() -> Self {
        Self {
            edited: std::ptr::null(),
            module: "",
            origins: &[],
            sink: std::ptr::null(),
            site: std::ptr::null(),
            value: LiteralValue::Number(0.0),
            reach: std::ptr::null_mut(),
        }
    }
}

/// The edit-mode state: LIT_EDIT_MODE whether it is on (one atomic load: what hot paths check);
/// LIT_SET_EDITED, LIT_SET_SOURCE whether the state changed; LIT_REGISTER_SINK adds the sink
/// (true); LIT_PATCH patches every live sink (dropped ones are forgotten; true).
#[relax_api]
pub fn literal(op: u32, req: &mut LiteralReq) -> bool {
    match op {
        LIT_EDIT_MODE => ON.load(Ordering::Relaxed),
        LIT_SET_EDITED => {
            // SAFETY: `edited` names live literals for this call
            let edited = unsafe { &*req.edited }.edited.clone();
            update(|live| {
                if live.edited == edited {
                    return false;
                }
                live.edited = edited;
                true
            })
        }
        LIT_SET_SOURCE => {
            let module = req.module;
            let origins = req.origins.to_vec();
            update(|live| {
                if live.sources.get(module).map_or(origins.is_empty(), |old| *old == origins) {
                    return false;
                }
                if origins.is_empty() {
                    live.sources.remove(module);
                } else {
                    live.sources.insert(module.to_string(), origins);
                }
                true
            })
        }
        LIT_REGISTER_SINK => {
            // SAFETY: the record is moved in by the caller
            let entry = Arc::new(LiteralSinkEntry(unsafe { std::ptr::read(req.sink) }));
            if let Ok(mut sinks) = SINKS.lock() {
                sinks.retain(|s| unsafe { (s.0.alive)(s.0.data) });
                sinks.push(entry);
            }
            true
        }
        _ => {
            let sinks: Vec<Arc<LiteralSinkEntry>> = match SINKS.lock() {
                Ok(mut sinks) => {
                    sinks.retain(|s| unsafe { (s.0.alive)(s.0.data) });
                    sinks.clone()
                }
                Err(_) => Vec::new(),
            };
            let mut reach = LiteralReach::default();
            for sink in sinks {
                let mut result = LiteralReach::default();
                // SAFETY: `site` names a live site for this call
                if unsafe { (sink.0.patch)(sink.0.data, req.site, &req.value, &mut result) } {
                    reach.add(result);
                }
            }
            // SAFETY: `reach` is the caller's out slot
            unsafe { *req.reach = reach };
            true
        }
    }
}

/// Whether edit mode is on (one atomic load: what hot paths check).
#[inline(always)]
pub fn edit_mode() -> bool {
    literal(LIT_EDIT_MODE, &mut LiteralReq::new())
}

/// The edit-mode state compilers read (None: off).
#[inline(always)]
pub fn live() -> Option<Arc<LiveLiterals>> {
    let ptr = live_into();
    if ptr.is_null() { None } else { Some(unsafe { Arc::from_raw(ptr) }) }
}
/// Transfers one strong reference to the caller, or null when edit mode is off.
pub fn live_into() -> *const LiveLiterals {
    if !edit_mode() { return std::ptr::null(); }
    LIVE.read().ok().and_then(|s| s.clone()).map_or(std::ptr::null(), Arc::into_raw)
}

fn update(f: impl FnOnce(&mut LiveLiterals) -> bool) -> bool {
    let Ok(mut guard) = LIVE.write() else { return false };
    let mut next = guard.as_deref().cloned().unwrap_or_default();
    if !f(&mut next) {
        return false;
    }
    ON.store(!next.edited.is_empty(), Ordering::Relaxed);
    *guard = Some(Arc::new(next));
    true
}

/// Edit mode for the files `edited` (none: off). Whether it changed.
pub fn set_edited(edited: Vec<String>) -> bool {
    let mut values = LiveLiterals::default();
    for value in &edited { values.push_edited(value); }
    set_edited_from(&values)
}

#[inline(always)]
pub fn set_edited_from(values: &LiveLiterals) -> bool {
    let mut req = LiteralReq::new();
    req.edited = values;
    literal(LIT_SET_EDITED, &mut req)
}

/// Name the rows of compiled text `module` that are copies of a file
/// (empty: forget it). Whether it changed.
pub fn set_source(module: &str, origins: Vec<LiteralOrigin>) -> bool {
    set_source_from(module, &origins)
}

#[inline(always)]
pub fn set_source_from(module: &str, origins: &[LiteralOrigin]) -> bool {
    let mut req = LiteralReq::new();
    req.module = module;
    req.origins = origins;
    literal(LIT_SET_SOURCE, &mut req)
}

/// A place literals land that a patch can reach (a compiled kernel, a
/// running program's VM): it writes `value` everywhere `site` landed in it
/// and says what it reached.
pub trait LiteralSink: Send + Sync {
    fn patch(&self, site: &LiteralSite, value: LiteralValue) -> LiteralReach;
}

/// A sink as its caller made it: the caller's own data and the operations on it, all of which
/// stay in the compiler that built them. [`register_sink`] takes the record and calls `release`
/// on it exactly once, when the sink is forgotten (it is no longer alive, or the process ends).
#[relax_api(plain)]
#[repr(C)]
pub struct LiteralSinkRegistration {
    data: *mut u8,
    alive: unsafe extern "C-unwind" fn(*mut u8) -> bool,
    /// Patches the sink if it is still alive and says whether it was.
    patch: unsafe extern "C-unwind" fn(*mut u8, *const LiteralSite, *const LiteralValue, *mut LiteralReach) -> bool,
    release: unsafe extern "C-unwind" fn(*mut u8),
}

/// The VM's one owner of a registration: whoever holds the last `Arc` releases the caller's data.
struct LiteralSinkEntry(LiteralSinkRegistration);
// The caller's data is a `Weak` of a `LiteralSink`, which is Send + Sync (see [`add_sink`]).
unsafe impl Send for LiteralSinkEntry {}
unsafe impl Sync for LiteralSinkEntry {}
impl Drop for LiteralSinkEntry {
    fn drop(&mut self) { unsafe { (self.0.release)(self.0.data) } }
}
static SINKS: Mutex<Vec<Arc<LiteralSinkEntry>>> = Mutex::new(Vec::new());

/// Register a sink for as long as it lives. Generic over the sink's own type: the boxes below are
/// made, read and freed by the code that instantiates this for its sink, so they never cross
/// the boundary as anything but the record's pointers.
#[inline(always)]
pub fn add_sink<S: LiteralSink + ?Sized + 'static>(sink: Weak<S>) {
    unsafe extern "C-unwind" fn alive<S: LiteralSink + ?Sized + 'static>(data: *mut u8) -> bool {
        (&*(data as *const Weak<S>)).strong_count() > 0
    }
    unsafe extern "C-unwind" fn patch<S: LiteralSink + ?Sized + 'static>(data: *mut u8, site: *const LiteralSite, value: *const LiteralValue, out: *mut LiteralReach) -> bool {
        match (&*(data as *const Weak<S>)).upgrade() {
            Some(sink) => {
                *out = sink.patch(&*site, *value);
                true
            }
            None => false,
        }
    }
    unsafe extern "C-unwind" fn release<S: LiteralSink + ?Sized + 'static>(data: *mut u8) {
        drop(Box::from_raw(data as *mut Weak<S>));
    }
    register_sink(LiteralSinkRegistration {
        data: Box::into_raw(Box::new(sink)) as *mut u8,
        alive: alive::<S>,
        patch: patch::<S>,
        release: release::<S>,
    });
}

#[inline(always)]
pub fn register_sink(sink: LiteralSinkRegistration) {
    let sink = std::mem::ManuallyDrop::new(sink);
    let mut req = LiteralReq::new();
    req.sink = &*sink;
    literal(LIT_REGISTER_SINK, &mut req);
}

/// Patch every live sink (dropped ones are forgotten).
#[inline(always)]
pub fn patch_sinks(site: &LiteralSite, value: LiteralValue) -> LiteralReach {
    let mut reach = LiteralReach::default();
    let mut req = LiteralReq::new();
    req.site = site;
    req.value = value;
    req.reach = &mut reach;
    literal(LIT_PATCH, &mut req);
    reach
}

/// The zero-based row and column (characters) of byte `at` of `text`.
pub fn row_col(text: &str, at: usize) -> (u32, u32) {
    let at = at.min(text.len());
    let line_start = text[..at].rfind('\n').map_or(0, |n| n + 1);
    (text[..line_start].matches('\n').count() as u32, text[line_start..at].chars().count() as u32)
}

/// The literal immediates a program's load ran (body, opcode index): what
/// the VM records while [`crate::vm::ScriptVmBase::literal_trace`] is set.
#[relax_api(handle, default)]
#[derive(Clone, Debug, Default)]
pub struct LoadTrace {
    pub ran: std::collections::HashSet<(u16, u32)>,
    /// The names the load read, and where (body, instruction): a `let`
    /// read while loading may have been copied away.
    pub name_reads: std::collections::HashSet<(crate::makepad_live_id::LiveId, u16, u32)>,
    /// The record fields the load read (`look.BONE`): where the record
    /// was made, the field, and where it was read (the FIELD instruction).
    /// A value read out of a record while loading may have been copied
    /// away. (The record by where it was made: the record itself may be
    /// collected after the load.)
    pub field_reads: std::collections::HashSet<(crate::value::ScriptIp, crate::makepad_live_id::LiveId, u16, u32)>,
    /// The sites `ran` has (a load's loops run the same literals over and
    /// over: a bit test instead of hashing each run).
    pub(crate) ran_seen: SiteSeen,
}

#[relax_api]
impl LoadTrace {
    /// Visits matching reads synchronously in the trace's iteration order.
    /// # Safety
    /// `ctx` must be valid for `visit` until this call returns.
    #[relax_api]
    pub unsafe fn read_sites_with(&self, key: crate::makepad_live_id::LiveId, site: Option<crate::value::ScriptIp>, ctx: *mut u8, visit: unsafe extern "C-unwind" fn(*mut u8, u16, u32)) {
        if let Some(site) = site {
            for &(made, k, body, at) in &self.field_reads { if k == key && made == site { visit(ctx, body, at); } }
        } else {
            for &(id, body, at) in &self.name_reads { if id == key { visit(ctx, body, at); } }
        }
    }
    #[inline(always)]
    pub fn read_sites(&self, key: crate::makepad_live_id::LiveId, site: Option<crate::value::ScriptIp>) -> Vec<(u16, u32)> {
        unsafe extern "C-unwind" fn push(ctx: *mut u8, body: u16, at: u32) { (&mut *(ctx as *mut Vec<(u16, u32)>)).push((body, at)); }
        let mut out = Vec::new();
        unsafe { self.read_sites_with(key, site, &mut out as *mut _ as *mut u8, push); }
        out
    }
}

/// Sites (body, instruction) seen while tracing a load: one bit each.
#[relax_api(handle)]
#[derive(Clone, Debug, Default)]
pub(crate) struct SiteSeen {
    bodies: Vec<Vec<u64>>,
}

impl SiteSeen {
    /// Whether this is the first time the site is seen (and marks it).
    #[inline]
    pub(crate) fn first(&mut self, body: u16, index: u32) -> bool {
        let (body, index) = (body as usize, index as usize);
        if body >= self.bodies.len() {
            self.bodies.resize_with(body + 1, Vec::new);
        }
        let words = &mut self.bodies[body];
        let (word, bit) = (index / 64, 1u64 << (index % 64));
        if word >= words.len() {
            words.resize(word + 1, 0);
        }
        let first = words[word] & bit == 0;
        words[word] |= bit;
        first
    }
}

/// The names a load read, and where, as the thread records them: the set,
/// and per site the name last recorded there (a site read again for the
/// same name is not hashed again).
#[relax_api(handle)]
#[derive(Clone, Debug, Default)]
pub(crate) struct NameReads {
    pub(crate) set: std::collections::HashSet<(crate::makepad_live_id::LiveId, u16, u32)>,
    last: Vec<Vec<crate::makepad_live_id::LiveId>>,
}

impl NameReads {
    #[inline]
    pub(crate) fn record(&mut self, id: crate::makepad_live_id::LiveId, body: u16, index: u32) {
        let (b, i) = (body as usize, index as usize);
        if b >= self.last.len() {
            self.last.resize_with(b + 1, Vec::new);
        }
        let sites = &mut self.last[b];
        if i >= sites.len() {
            sites.resize(i + 1, crate::makepad_live_id::LiveId(0));
        }
        if sites[i] != id {
            sites[i] = id;
            self.set.insert((id, body, index));
        }
    }
}

#[relax_api]
impl crate::vm::ScriptVm<'_> {
    /// Start tracing a load (edit mode): the literals it runs, the names
    /// and record fields it reads.
    #[inline(always)]
    pub fn begin_literal_trace(&mut self) {
        let mut trace = LoadTrace::default();
        self.literal_trace(false, &mut trace);
    }

    /// The trace of the load since [`Self::begin_literal_trace`].
    #[inline(always)]
    pub fn end_literal_trace(&mut self) -> Option<LoadTrace> {
        let mut trace = LoadTrace::default();
        self.literal_trace(true, &mut trace).then_some(trace)
    }

    /// Begins a load trace, or (`end`) ends it into `out`: whether there was one.
    #[relax_api]
    pub fn literal_trace(&mut self, end: bool, out: &mut LoadTrace) -> bool {
        if !end {
            self.bx.literal_trace = Some(Box::default());
            self.bx.threads.cur().name_reads = Some(Box::default());
            return true;
        }
        let Some(trace) = self.bx.literal_trace.take() else { return false };
        let mut trace = *trace;
        if let Some(reads) = self.bx.threads.cur().name_reads.take() {
            trace.name_reads = reads.set;
        }
        *out = trace;
        true
    }
}

/// Whether the name `id` read at `body`, `index` was a record's field of the
/// same name built from it (`{BONE: BONE}`, a module's exports): a copy a
/// patch of the `let` finds by its name.
pub fn named_copy(code: &crate::vm::ScriptCode, id: crate::makepad_live_id::LiveId, body: u16, index: u32) -> bool {
    use crate::tokenizer::ScriptToken;
    let bodies = code.bodies.borrow();
    let Some(b) = bodies.get(body as usize) else { return false };
    let Some((op, _)) = b.parser.opcodes.get(index as usize).and_then(|o| o.as_opcode()) else { return false };
    if op != crate::opcode::Opcode::ASSIGN_ME {
        return false;
    }
    let Some(Some(tok)) = b.parser.source_map.get(index as usize) else { return false };
    let toks = &b.tokenizer.tokens;
    let tok = *tok as usize;
    // `key: value`: the `:` with the name before and after it.
    let is = |t: Option<&crate::tokenizer::ScriptTokenPos>| matches!(t.map(|t| &t.token), Some(ScriptToken::Identifier(x)) if *x == id);
    (tok > 0 && is(toks.get(tok - 1)) && is(toks.get(tok + 1))) || (is(toks.get(tok)) && is(toks.get(tok + 2)))
}

/// Where a literal's value went in a program that has loaded.
#[relax_api(plain)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiteralUse {
    /// Its code did not run at load: per-frame code pushes it each time it
    /// runs, so patching the code is all it takes.
    Code,
    /// Load bound it to a `let` as is.
    Let,
    /// Load stored it as is in a record's field (`{alpha: 0.55}`).
    Field,
    /// Load stored it as is in a list (`[#ff0000, 0.5]`).
    ListItem,
    /// Load computed with it (an operator, a call, a condition): the value
    /// it made is not the literal, and only a reload makes it again.
    Computed,
}

impl LiteralUse {
    /// Why a reload is needed, when it is.
    pub fn reload_reason(self) -> Option<&'static str> {
        match self {
            LiteralUse::Computed => Some("computed with while the program loaded"),
            LiteralUse::ListItem => Some("copied into a list while the program loaded"),
            _ => None,
        }
    }
}

/// What became of the literal immediate at `index` of code body `body`, given
/// what the load ran: its next instruction says (a `-` before it is applied
/// after the immediate and still counts as the literal as written).
#[relax_api]
pub fn load_use(code: &crate::vm::ScriptCode, trace: &LoadTrace, body: u16, index: u32) -> LiteralUse {
    use crate::opcode::Opcode;
    if !trace.ran.contains(&(body, index)) {
        return LiteralUse::Code;
    }
    let bodies = code.bodies.borrow();
    let Some(b) = bodies.get(body as usize) else { return LiteralUse::Computed };
    let ops = &b.parser.opcodes;
    let mut k = index as usize + 1;
    if matches!(ops.get(k).and_then(|o| o.as_opcode()), Some((Opcode::NEG, _))) {
        k += 1;
    }
    match ops.get(k).and_then(|o| o.as_opcode()) {
        Some((Opcode::LET_DYN | Opcode::LET_TYPED | Opcode::LET_SLOT, _)) => LiteralUse::Let,
        Some((op, _)) if op == Opcode::ASSIGN_ME || op == Opcode::ASSIGN_ME_VEC => LiteralUse::Field,
        Some((Opcode::POP_TO_ME, _)) => LiteralUse::ListItem,
        _ => LiteralUse::Computed,
    }
}

/// One literal an edit changed: where it is in the text before (zero-based
/// line and column of its first character, the digits past a sign or the
/// `#`) and what it says now (a number in its unit's base unit).
#[relax_api(plain)]
#[derive(Clone, Debug, PartialEq)]
pub struct LiteralEdit {
    pub line: u32,
    pub col: u32,
    pub value: LiteralValue,
}

impl LiteralEdit {
    pub fn line(&self) -> u32 { self.line }
    pub fn col(&self) -> u32 { self.col }
    pub fn value(&self) -> LiteralValue { self.value }
}

/// A piece of Splash text: a number or colour literal (with its value), or
/// anything else, compared as written.
#[derive(Debug, PartialEq)]
enum Piece<'a> {
    Text(&'a str),
    Literal { at: usize, text: &'a str, value: LiteralValue },
}

/// The literals an edit from `old` to `new` changed, when that is all it
/// changed: the same text apart from number and colour literals (a sign,
/// a unit suffix, a word or a bracket that changed is a structural edit:
/// `None`). Every site is `old`'s, the text the running program was loaded
/// from, so a literal that grows does not move the others. Strings and
/// comments are text (a number in them is not a literal).
#[inline(always)]
pub fn literal_edits(old: &str, new: &str) -> Option<Vec<LiteralEdit>> {
    let mut out = Vec::new();
    literal_edits_into(old, new, &mut ScriptLiteralEditsOut::new(&mut out)).then_some(out)
}
pub fn literal_edits_into(old: &str, new: &str, sink: &mut ScriptLiteralEditsOut) -> bool {
    let a = pieces(old);
    let b = pieces(new);
    if a.len() != b.len() {
        return false;
    }
    let mut out = Vec::new();
    for (x, y) in a.iter().zip(&b) {
        match (x, y) {
            (Piece::Text(p), Piece::Text(q)) if p == q => {}
            (Piece::Literal { at, text: p, .. }, Piece::Literal { text: q, value, .. }) => {
                if p != q {
                    let (line, col) = row_col(old, *at);
                    out.push(LiteralEdit { line, col, value: *value });
                }
            }
            _ => return false,
        }
    }
    for edit in &out { sink.push(edit); }
    true
}

/// The value of `text` when it is exactly one literal (a number as
/// written, with its unit; a `#colour`).
pub fn literal_value(text: &str) -> Option<LiteralValue> {
    match pieces(text).as_slice() {
        [Piece::Literal { at: 0, text: t, value }] if t.len() == text.len() => Some(*value),
        _ => None,
    }
}

/// Splits Splash text into literals and the text between them (strings,
/// raw strings and comments skipped whole). Adjacent text is one piece.
fn pieces(src: &str) -> Vec<Piece<'_>> {
    let b = src.as_bytes();
    let mut out: Vec<Piece> = Vec::new();
    let mut text_start = 0;
    let mut i = 0;
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    while i < b.len() {
        let c = b[i];
        // Comments.
        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(b.len());
            continue;
        }
        // Raw strings r"…", r#"…"#.
        if c == b'r' && (i == 0 || !ident(b[i - 1])) {
            let mut j = i + 1;
            while j < b.len() && b[j] == b'#' {
                j += 1;
            }
            if b.get(j) == Some(&b'"') {
                let hashes = j - i - 1;
                let mut k = j + 1;
                loop {
                    if k >= b.len() {
                        break;
                    }
                    if b[k] == b'"' && b[k + 1..].iter().take(hashes).filter(|&&h| h == b'#').count() == hashes && b.len() >= k + 1 + hashes {
                        k += 1 + hashes;
                        break;
                    }
                    k += 1;
                }
                i = k.min(b.len());
                continue;
            }
        }
        if c == b'"' {
            i += 1;
            while i < b.len() && b[i] != b'"' {
                i += if b[i] == b'\\' { 2 } else { 1 };
            }
            i = (i + 1).min(b.len());
            continue;
        }
        if ident(c) && !c.is_ascii_digit() {
            while i < b.len() && ident(b[i]) {
                i += 1;
            }
            continue;
        }
        // A colour: #rgb, #rgba, #rrggbb, #rrggbbaa.
        if c == b'#' {
            let mut j = i + 1;
            while j < b.len() && b[j].is_ascii_hexdigit() {
                j += 1;
            }
            let digits = &src[i + 1..j];
            if !(j < b.len() && ident(b[j])) {
                if let Some(rgba) = hex_color(digits) {
                    if text_start < i {
                        out.push(Piece::Text(&src[text_start..i]));
                    }
                    out.push(Piece::Literal { at: i, text: &src[i..j], value: LiteralValue::color(rgba) });
                    i = j;
                    text_start = j;
                    continue;
                }
            }
            i = j.max(i + 1);
            continue;
        }
        // A number: digits, a fraction, an exponent, a unit suffix.
        if c.is_ascii_digit() && (i == 0 || !(ident(b[i - 1]) || b[i - 1] == b'.')) {
            let mut j = i;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j < b.len() && b[j] == b'.' && b.get(j + 1).is_none_or(|d| d.is_ascii_digit() || !ident(*d)) {
                j += 1;
                while j < b.len() && b[j].is_ascii_digit() {
                    j += 1;
                }
            }
            if j < b.len() && (b[j] == b'e' || b[j] == b'E') {
                let mut k = j + 1;
                if k < b.len() && (b[k] == b'+' || b[k] == b'-') {
                    k += 1;
                }
                if k < b.len() && b[k].is_ascii_digit() {
                    while k < b.len() && b[k].is_ascii_digit() {
                        k += 1;
                    }
                    j = k;
                }
            }
            let digits_end = j;
            while j < b.len() && ident(b[j]) {
                j += 1;
            }
            let suffix = &src[digits_end..j];
            let Ok(v) = src[i..digits_end].parse::<f64>() else {
                i = j;
                continue;
            };
            let value = if suffix.is_empty() {
                Some(v)
            } else {
                crate::tokenizer::ScriptUnit::parse(suffix).map(|u| u.to_base(v))
            };
            if let Some(v) = value {
                if text_start < i {
                    out.push(Piece::Text(&src[text_start..i]));
                }
                // The unit is part of what it says: a changed unit changes
                // the text piece after the literal.
                out.push(Piece::Literal { at: i, text: &src[i..digits_end], value: LiteralValue::number(v) });
                text_start = digits_end;
            }
            i = j;
            continue;
        }
        i += 1;
    }
    if text_start < src.len() {
        out.push(Piece::Text(&src[text_start..]));
    }
    out
}

/// A colour's hex digits (3, 4, 6 or 8) as its channels.
fn hex_color(d: &str) -> Option<[f32; 4]> {
    let n = |s: &str| u8::from_str_radix(s, 16).ok().map(|v| v as f32 / 255.0);
    let dup = |c: char| format!("{c}{c}");
    let c: Vec<char> = d.chars().collect();
    match c.len() {
        3 | 4 => {
            let ch = |k: usize| n(&dup(c[k]));
            Some([ch(0)?, ch(1)?, ch(2)?, if c.len() == 4 { ch(3)? } else { 1.0 }])
        }
        6 | 8 => Some([n(&d[0..2])?, n(&d[2..4])?, n(&d[4..6])?, if c.len() == 8 { n(&d[6..8])? } else { 1.0 }]),
        _ => None,
    }
}

impl LiteralValue {
    pub fn number(value: f64) -> Self { Self::Number(value) }
    pub fn color(value: [f32; 4]) -> Self { Self::Color(value) }
    pub fn as_number(&self) -> Option<f64> { if let Self::Number(v) = self { Some(*v) } else { None } }
    pub fn as_color(&self) -> Option<[f32; 4]> { if let Self::Color(v) = self { Some(*v) } else { None } }
}

#[relax_api]
impl LiveLiterals { pub fn push_edited(&mut self, file: &str) { self.edited.push(file.to_owned()); } }

/// Synchronous output into a caller-owned vector; opaque elements are cloned natively.
#[relax_api(plain)]
#[repr(C)]
pub struct ScriptOriginsOut<'a> {
    ctx: *mut u8,
    push: unsafe extern "C-unwind" fn(*mut u8, *const LiteralOrigin),
    _out: std::marker::PhantomData<&'a mut Vec<LiteralOrigin>>,
}
impl<'a> ScriptOriginsOut<'a> {
    #[inline(always)]
    pub fn new(out: &'a mut Vec<LiteralOrigin>) -> Self {
        unsafe extern "C-unwind" fn push(ctx: *mut u8, value: *const LiteralOrigin) {
            (&mut *(ctx as *mut Vec<LiteralOrigin>)).push((&*value).clone());
        }
        Self { ctx: out as *mut _ as *mut u8, push, _out: std::marker::PhantomData }
    }
    pub fn push(&mut self, value: &LiteralOrigin) {
        unsafe { (self.push)(self.ctx, value) }
    }
}

/// Synchronous output into a caller-owned vector; opaque elements are cloned natively.
#[relax_api(plain)]
#[repr(C)]
pub struct ScriptLiteralEditsOut<'a> {
    ctx: *mut u8,
    push: unsafe extern "C-unwind" fn(*mut u8, *const LiteralEdit),
    _out: std::marker::PhantomData<&'a mut Vec<LiteralEdit>>,
}
impl<'a> ScriptLiteralEditsOut<'a> {
    #[inline(always)]
    pub fn new(out: &'a mut Vec<LiteralEdit>) -> Self {
        unsafe extern "C-unwind" fn push(ctx: *mut u8, value: *const LiteralEdit) {
            (&mut *(ctx as *mut Vec<LiteralEdit>)).push((&*value).clone());
        }
        Self { ctx: out as *mut _ as *mut u8, push, _out: std::marker::PhantomData }
    }
    pub fn push(&mut self, value: &LiteralEdit) {
        unsafe { (self.push)(self.ctx, value) }
    }
}
