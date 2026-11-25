//! The bounded, pure document contract: every evaluation of document code
//! (a document's load, and each per-frame call of its closures) runs under
//! the VM's instruction, heap-allocation, operand-stack and call-frame
//! limits, and after its load the document's own objects are frozen, so a
//! per-frame call cannot keep state between calls and its result cannot
//! depend on the order frames are evaluated in.
//!
//! ```ignore
//! let (root, report) = vm.eval_bounded(&EvalLimits::DOC_LOAD, |vm| vm.eval(script_mod));
//! vm.freeze_document(&[root]);               // the document is now read-only
//! let (v, report) = vm.eval_bounded(&EvalLimits::DOC_FRAME, |vm| vm.call(f, &args));
//! if let Some(limit) = report.limit { /* a diagnostic: the frame failed */ }
//! ```
//!
//! A limit is an uncatchable script error (`try` does not swallow it): the
//! evaluation bails and the host reports it; nothing is silently cut short.
//! Every limit counts work, never wall time: a correct document does the same
//! work on a busy machine as on an idle one, so it never fails because other
//! work ran beside it, and a runaway loop still stops at its instruction count.
//! A write into a frozen object is an ordinary script error (`Immutable`),
//! reported with its source location.
use relax_api::relax_api;
pub(crate) use makepad_stack_budget::native_stack_budget;

use crate::array::ScriptArrayStorage;
use crate::heap::{ScriptAllocationReport, ScriptHeap};
use crate::value::ScriptValue;
use crate::vm::{ScriptBody, ScriptErrorLoc, ScriptErrorRecord, ScriptEvalReq, ScriptModKey, ScriptModView,
    ScriptSource, ScriptVm, EVAL_APPEND};
use crate::parser::{ParserCheckpoint, ScriptParser};
use crate::tokenizer::ScriptTokenizer;
use crate::units::UnitTable;

/// Admission applies to the whole source presented by one evaluation, including a
/// streaming prefix. The default preserves unrestricted trusted-source parsing.
#[relax_api(plain, run)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdmissionLimits {
    pub source_bytes: usize,
    pub tokens: usize,
    pub parser_states: usize,
    /// One credit per source byte, then credits for parser transitions and EOF reductions.
    /// Credits include the state and bytecode each transition can scan or move.
    pub parse_work: usize,
}

impl AdmissionLimits {
    pub const UNLIMITED: Self = Self {
        source_bytes: usize::MAX, tokens: usize::MAX,
        parser_states: usize::MAX, parse_work: usize::MAX,
    };

    pub(crate) fn for_text(mut self, source: &str, site: AdmissionSite) -> Result<Self, ScriptErrorRecord> {
        if source.len() > self.source_bytes.min(u32::MAX as usize) {
            return Err(site.limit(0, 0, "source too large to admit"));
        }
        if self.parse_work != usize::MAX {
            self.parse_work = self.parse_work.checked_sub(source.len())
                .ok_or_else(|| site.limit(0, 0, "total parse work exceeded"))?;
        }
        Ok(self)
    }

    fn narrow(self, other: Self) -> Self {
        Self {
            source_bytes: self.source_bytes.min(other.source_bytes),
            tokens: self.tokens.min(other.tokens),
            parser_states: self.parser_states.min(other.parser_states),
            parse_work: self.parse_work.min(other.parse_work),
        }
    }
}

impl Default for AdmissionLimits {
    fn default() -> Self { Self::UNLIMITED }
}

#[derive(Clone, Copy)]
pub(crate) struct AdmissionSite<'a> {
    pub(crate) file: &'a str,
    pub(crate) line: usize,
    pub(crate) column: usize,
    pub(crate) first_col: u32,
}

impl AdmissionSite<'_> {
    pub(crate) fn limit(self, row: u32, col: u32, why: &str) -> ScriptErrorRecord {
        let line = ScriptBody::line0_of(self.line) as u32 + row;
        let col = self.column as u32 + col + if row == 0 { self.first_col } else { 0 };
        self.record(line, col, format!("source admission limit: {why}"))
    }

    fn record(self, line: u32, column: u32, message: String) -> ScriptErrorRecord {
        let text = format!("{}:{}:{}: {message}", self.file, line + 1, column + 1);
        let loc = ScriptErrorLoc { file: self.file.to_owned(), line: line + u32::from(self.line != 0), col: column };
        ScriptErrorRecord { loc: Some(loc), message, text }
    }
}

/// Private candidate, built from the complete borrowed prefix under this entry's limits.
/// A tighter limit therefore also covers input previously admitted by a trusted editor.
pub(crate) struct AdmittedSource {
    tokenizer: ScriptTokenizer,
    parser: ScriptParser,
    checkpoint: Option<ParserCheckpoint>,
    len: usize,
    hash: u64,
}

impl AdmittedSource {
    fn parse(source: &str, values: &[ScriptValue], streaming: bool, site: AdmissionSite, limits: AdmissionLimits,
             units: &UnitTable, keep_numbers: bool, heap: &mut ScriptHeap) -> Result<Box<Self>, ScriptErrorRecord> {
        let limits = limits.for_text(source, site)?;
        let mut tokenizer = ScriptTokenizer::with_units(units.clone());
        tokenizer.tokenize_up_to(source, heap, limits.tokens);
        let provisional = if tokenizer.tokens.len() > limits.tokens {
            false
        } else if streaming {
            tokenizer.push_pending_token(heap)
        } else {
            tokenizer.finish(heap);
            false
        };
        if tokenizer.tokens.len() > limits.tokens {
            let (row, col) = tokenizer.token_start_row_col(limits.tokens as u32).unwrap_or_default();
            return Err(site.limit(row, col, "token count exceeded"));
        }
        let pending = streaming.then(|| (tokenizer.intern_unfinished_string(heap), provisional));
        Self::from_tokens(source, values, tokenizer, pending, site, limits, keep_numbers)
    }

    pub(crate) fn from_tokens(
        source: &str, values: &[ScriptValue], tokenizer: ScriptTokenizer,
        pending: Option<(Option<ScriptValue>, bool)>, site: AdmissionSite, limits: AdmissionLimits, keep_numbers: bool,
    ) -> Result<Box<Self>, ScriptErrorRecord> {
        let mut parser = ScriptParser::keeping_numbers(keep_numbers);
        parser.set_emit_errors(false);
        parser.admission = Some(limits);
        parser.first_col = site.first_col;
        let offsets = (ScriptBody::line0_of(site.line), site.column);
        let checkpoint = if let Some((unfinished, provisional)) = pending {
            Some(parser.parse_streaming(&tokenizer, site.file, offsets, values, unfinished, provisional))
        } else {
            parser.parse(&tokenizer, site.file, offsets, values);
            None
        };
        if let Some(d) = parser.admission_error.take() {
            return Err(site.record(d.line, d.column, format!("source admission limit: {}", d.message)));
        }
        if let Some(d) = parser.diagnostics.first() {
            return Err(site.record(d.line, d.column, d.message.clone()));
        }
        parser.admission = None;
        parser.set_emit_errors(true);
        Ok(Box::new(Self { tokenizer, parser, checkpoint, len: source.len(), hash: ScriptBody::content_hash_of(source, values) }))
    }

    pub(crate) fn install(self: Box<Self>, body: &mut ScriptBody) {
        body.tokenizer = self.tokenizer;
        body.parser = self.parser;
        body.checkpoint = self.checkpoint;
        body.source_len = self.len;
        body.content_hash = self.hash;
    }
}

/// The limits of one evaluation.
#[relax_api(plain, run)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EvalLimits {
    pub instructions: u64,
    /// Heap bytes the evaluation may allocate.
    pub heap_bytes: usize,
    pub stack_values: usize,
    pub call_frames: usize,
    pub admission: AdmissionLimits,
    /// Additional cap on live interpreter entries, including the root. The physical stack guard always applies.
    pub native_depth: u32,
}

// A release probe sampled the address of the same local at five successive
// levels of [n-1].map(visit) on a 2 MiB thread. The stride was 1,040 bytes
// (arm64 macOS), 1,136 (x64 Linux), and 1,408 (x64 Windows). Round up to
// 4 KiB per interpreter entry and reserve at least half of the *remaining*
// stack. A host cap narrows that physical ceiling. Also check the actual
// descent, since another native or an unoptimized build can have a larger frame.
pub(crate) const NATIVE_ENTRY_BYTES: usize = 4096;

impl EvalLimits {
    /// A document's load: the top-level evaluation that builds its value.
    pub const DOC_LOAD: EvalLimits = EvalLimits {
        instructions: 200_000_000,
        heap_bytes: 512 << 20,
        stack_values: 1 << 20,
        call_frames: 4096,
        admission: AdmissionLimits::UNLIMITED,
        native_depth: u32::MAX,
    };
    /// One per-frame call of a document closure (a keyable, a property
    /// function, a tick).
    pub const DOC_FRAME: EvalLimits = EvalLimits {
        instructions: 20_000_000,
        heap_bytes: 64 << 20,
        stack_values: 1 << 18,
        call_frames: 1024,
        admission: AdmissionLimits::UNLIMITED,
        native_depth: u32::MAX,
    };

    pub const fn with_instructions(self, instructions: u64) -> EvalLimits {
        EvalLimits { instructions, ..self }
    }
}

/// What a bounded evaluation used, and the limit it hit, if any.
#[relax_api(handle, run)]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EvalReport {
    pub instructions: u64,
    pub heap: ScriptAllocationReport,
    /// The limit that stopped the evaluation (its script error message).
    pub limit: Option<String>,
}

#[relax_api(run)]
impl EvalReport {
    #[relax_api(run)]
    pub fn limit(&self) -> Option<&str> { self.limit.as_deref() }
}

/// Script errors that come from a limit (uncatchable resource failures), by their message
/// (a source file whose name says `allocation` is no limit).
fn limit_message(error: &str) -> bool {
    [
        "instruction limit",
        "time budget exceeded",
        "allocation",
        "heap limit",
        "stack limit exceeded",
        "call frame limit exceeded",
        "native re-entry limit exceeded",
        "source admission limit",
        "script limit",
    ]
    .iter()
    .any(|k| error.contains(k))
}

#[relax_api(run)]
impl ScriptVm<'_> {
    /// Runs `f` (one evaluation of document code) under `limits`. Nested
    /// bounds only narrow. Errors are captured, not printed: the report's
    /// `limit` names a limit that was hit, and the evaluation's other
    /// script errors are left in the capture sink for the host
    /// (`take_errors`).
    pub fn eval_bounded<R>(&mut self, limits: &EvalLimits, f: impl FnOnce(&mut ScriptVm) -> R) -> (R, EvalReport) {
        unsafe extern "C-unwind" fn run<R, F: FnOnce(&mut ScriptVm) -> R>(ctx: *mut u8, vm: *mut ScriptVm) {
            let (f, result) = &mut *(ctx as *mut (Option<F>, Option<R>));
            *result = Some(f.take().unwrap()(&mut *vm));
        }
        fn call<R, F: FnOnce(&mut ScriptVm) -> R>(vm: &mut ScriptVm, limits: &EvalLimits, f: F) -> (R, EvalReport) {
            let mut state = (Some(f), None::<R>);
            let report = unsafe { vm.eval_bounded_with(limits, &mut state as *mut _ as *mut u8, run::<R, F>) };
            (state.1.unwrap(), report)
        }
        call(self, limits, f)
    }

    /// Runs the callback synchronously under the same limits as `eval_bounded`.
    /// # Safety
    /// `ctx` must be valid for `run`, which is called exactly once and must not retain the VM pointer.
    #[relax_api(run)]
    pub unsafe fn eval_bounded_with(&mut self, limits: &EvalLimits, ctx: *mut u8, run: unsafe extern "C-unwind" fn(*mut u8, *mut ScriptVm)) -> EvalReport {
        if self.bx.captured_errors.is_none() {
            self.bx.captured_errors = Some(Vec::new());
        }
        let before = self.bx.captured_errors.as_ref().map_or(0, |e| e.len());
        let strings_after = std::mem::replace(&mut self.bx.heap.charge_native_strings_after, true);
        let previous_admission = self.bx.admission;
        let previous_depth = self.bx.native_depth_limit;
        self.bx.admission = previous_admission.narrow(limits.admission);
        self.bx.native_depth_limit = previous_depth.min(limits.native_depth);
        let ((outcome, heap), instructions) = {
            let r = self.with_instruction_limit(limits.instructions, |vm| {
                vm.with_stack_value_limit(limits.stack_values, |vm| {
                    vm.with_call_frame_limit(limits.call_frames, |vm| {
                        vm.with_heap_allocation_limit(limits.heap_bytes, |vm| {
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { run(ctx, vm) }))
                        })
                    })
                })
            });
            (r, self.last_limit_consumed())
        };
        self.bx.admission = previous_admission;
        self.bx.native_depth_limit = previous_depth;
        self.bx.heap.charge_native_strings_after = strings_after;
        if let Err(panic) = outcome { std::panic::resume_unwind(panic); }
        let mut limit = None;
        if let Some(errors) = self.bx.captured_errors.as_ref() {
            limit = errors.iter().skip(before).find(|e| limit_message(&e.message)).map(|e| e.text.clone());
        }
        if limit.is_none() && heap.exceeded {
            limit = Some(format!("script heap limit of {} bytes exceeded", limits.heap_bytes));
        }
        if limit.is_none() && instructions >= limits.instructions {
            limit = Some(format!("script instruction limit of {} exceeded", limits.instructions));
        }
        EvalReport { instructions, heap, limit }
    }

    /// Freezes the document reachable from `roots` (its value, its
    /// closures and the scopes they captured) so later calls can read it
    /// but never write it. Host-owned objects are left alone and not
    /// entered: static and already-frozen objects (components, types,
    /// APIs), the module table, every module in it and the host's injected
    /// globals. A module stays open when the document holds it (`use
    /// mod.draw` puts `mod.draw` itself in the scope its closures capture):
    /// the host goes on registering into its modules after a document has
    /// loaded (a kit built on first use adds its shaders to `mod.draw`),
    /// and a module frozen here would refuse that write.
    /// Returns how many objects and arrays were frozen.
    #[relax_api(run)]
    pub fn freeze_document(&mut self, roots: &[ScriptValue]) -> (usize, usize) {
        let mut stop: Vec<ScriptValue> = self.bx.injected_globals.values().copied().collect();
        let heap = &self.bx.heap;
        stop.push(heap.modules.into());
        heap.objects[heap.modules].map_iter(|_, module| stop.push(module));
        self.bx.heap.freeze_reachable(roots, &stop)
    }
}

impl ScriptVm<'_> {
    pub(crate) fn admit_source(&mut self, req: &ScriptEvalReq, module: &ScriptModView, site: AdmissionSite) -> Result<Box<AdmittedSource>, ScriptErrorRecord> {
        let limits = self.bx.admission;
        let key = ScriptModKey::new(module.file, module.line, module.column);
        let overrides = self.bx.code.script_mod_overrides.borrow();
        let effective = overrides.get(&key).map_or(module.code, String::as_str);
        if effective.len() > limits.source_bytes {
            return Err(site.limit(0, 0, "source too large to admit"));
        }
        let streaming = req.mode == EVAL_APPEND;
        let mut values = module.values;
        let bodies = self.bx.code.bodies.borrow();
        let mut site = site;
        if streaming {
            if let Some((body, previous)) = bodies.iter().find_map(|body| match &body.source {
                ScriptSource::Mod(m) if m.file == module.file && m.line == module.line && m.column == module.column => Some((body, m)),
                _ => None,
            }) {
                values = &previous.values;
                if let Some(shown) = &body.shown {
                    site = AdmissionSite { file: shown, column: 0, first_col: body.first_col, ..site };
                }
            }
        }
        let source = if streaming { req.code } else { effective };
        AdmittedSource::parse(source, values, streaming, site, limits, &self.bx.units, self.bx.keep_numbers, &mut self.bx.heap)
    }
}

impl ScriptHeap {
    /// Freezes every object and array reachable from `roots`, not entering
    /// `stop` values or objects that are static or frozen in any mode.
    pub fn freeze_reachable(&mut self, roots: &[ScriptValue], stop: &[ScriptValue]) -> (usize, usize) {
        let stop_objects: std::collections::HashSet<_> = stop.iter().filter_map(|v| v.as_object()).collect();
        let stop_arrays: std::collections::HashSet<_> = stop.iter().filter_map(|v| v.as_array()).collect();
        let mut seen_objects = std::collections::HashSet::new();
        let mut seen_arrays = std::collections::HashSet::new();
        let (mut objects, mut arrays) = (0, 0);
        let mut work: Vec<ScriptValue> = roots.to_vec();
        while let Some(v) = work.pop() {
            if let Some(obj) = v.as_object() {
                if stop_objects.contains(&obj) || !self.objects.is_valid(obj) || !seen_objects.insert(obj) {
                    continue;
                }
                let data = &self.objects[obj];
                if data.tag.is_static() || data.tag.is_frozen() || data.tag.is_vec_frozen() {
                    continue;
                }
                work.push(data.proto);
                for (key, entry) in data.map.iter() {
                    work.push(*key);
                    work.push(entry.value);
                }
                for kv in data.vec.iter() {
                    work.push(kv.key);
                    work.push(kv.value);
                }
                self.objects[obj].tag.freeze();
                objects += 1;
            } else if let Some(arr) = v.as_array() {
                if stop_arrays.contains(&arr) || !self.arrays.is_valid(arr) || !seen_arrays.insert(arr) {
                    continue;
                }
                let tag = &self.arrays[arr].tag;
                if tag.is_static() || tag.is_frozen() {
                    continue;
                }
                if let ScriptArrayStorage::ScriptValue(values) = &self.arrays[arr].storage {
                    work.extend(values.iter().copied());
                }
                self.arrays[arr].tag.freeze();
                arrays += 1;
            }
        }
        (objects, arrays)
    }
}
