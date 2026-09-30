//! The bounded, pure document contract: every evaluation of document code
//! (a document's load, and each per-frame call of its closures) runs under
//! the VM's instruction, time, heap-allocation, operand-stack and call-frame
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
//! A write into a frozen object is an ordinary script error (`Immutable`),
//! reported with its source location.

use crate::array::ScriptArrayStorage;
use crate::heap::{ScriptAllocationReport, ScriptHeap};
use crate::value::ScriptValue;
use crate::vm::{ScriptRunBudget, ScriptVm};
use std::time::Duration;

/// The limits of one evaluation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EvalLimits {
    pub instructions: usize,
    /// Wall time (a backstop behind the instruction count).
    pub time: Duration,
    /// Heap bytes the evaluation may allocate.
    pub heap_bytes: usize,
    pub stack_values: usize,
    pub call_frames: usize,
}

impl EvalLimits {
    /// A document's load: the top-level evaluation that builds its value.
    pub const DOC_LOAD: EvalLimits = EvalLimits {
        instructions: 200_000_000,
        time: Duration::from_secs(5),
        heap_bytes: 512 << 20,
        stack_values: 1 << 20,
        call_frames: 4096,
    };
    /// One per-frame call of a document closure (a keyable, a property
    /// function, a tick).
    pub const DOC_FRAME: EvalLimits = EvalLimits {
        instructions: 20_000_000,
        time: Duration::from_millis(250),
        heap_bytes: 64 << 20,
        stack_values: 1 << 18,
        call_frames: 1024,
    };

    pub const fn with_instructions(self, instructions: usize) -> EvalLimits {
        EvalLimits { instructions, ..self }
    }
}

/// What a bounded evaluation used, and the limit it hit, if any.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EvalReport {
    pub instructions: usize,
    pub heap: ScriptAllocationReport,
    /// The limit that stopped the evaluation (its script error message).
    pub limit: Option<String>,
}

/// Script errors that come from a limit (uncatchable resource failures).
fn limit_message(error: &str) -> bool {
    [
        "instruction limit",
        "time budget exceeded",
        "allocation",
        "heap limit",
        "stack limit exceeded",
        "call frame limit exceeded",
        "script limit",
    ]
    .iter()
    .any(|k| error.contains(k))
}

impl ScriptVm<'_> {
    /// Runs `f` (one evaluation of document code) under `limits`. Nested
    /// bounds only narrow. Errors are captured, not printed: the report's
    /// `limit` names a limit that was hit, and the evaluation's other
    /// script errors are left in the capture sink for the host
    /// (`take_errors`).
    pub fn eval_bounded<R>(&mut self, limits: &EvalLimits, f: impl FnOnce(&mut ScriptVm) -> R) -> (R, EvalReport) {
        if self.bx.captured_errors.is_none() {
            self.bx.captured_errors = Some(Vec::new());
        }
        let before = self.bx.captured_errors.as_ref().map_or(0, |e| e.len());
        let previous = self.bx.run_budget;
        let strings_after = std::mem::replace(&mut self.bx.heap.charge_native_strings_after, true);
        let hard = ScriptRunBudget::from_durations(limits.time, limits.time, 1024);
        self.bx.run_budget = Some(match previous {
            // Only narrow: keep the earlier deadline.
            Some(p) if p.hard_deadline <= hard.hard_deadline => p,
            _ => hard,
        });
        let ((r, heap), instructions) = {
            let r = self.with_instruction_limit(limits.instructions, |vm| {
                vm.with_stack_value_limit(limits.stack_values, |vm| {
                    vm.with_call_frame_limit(limits.call_frames, |vm| vm.with_heap_allocation_limit(limits.heap_bytes, f))
                })
            });
            (r, self.last_limit_consumed())
        };
        self.bx.run_budget = previous;
        self.bx.heap.charge_native_strings_after = strings_after;
        let mut limit = None;
        if let Some(errors) = self.bx.captured_errors.as_ref() {
            limit = errors.iter().skip(before).find(|e| limit_message(e)).cloned();
        }
        if limit.is_none() && heap.exceeded {
            limit = Some(format!("script heap limit of {} bytes exceeded", limits.heap_bytes));
        }
        if limit.is_none() && instructions >= limits.instructions {
            limit = Some(format!("script instruction limit of {} exceeded", limits.instructions));
        }
        (r, EvalReport { instructions, heap, limit })
    }

    /// Freezes the document reachable from `roots` (its value, its
    /// closures and the scopes they captured) so later calls can read it
    /// but never write it. Host-owned objects are left alone and not
    /// entered: static and already-frozen objects (modules, components,
    /// types, APIs), the module table and the host's injected globals.
    /// Returns how many objects and arrays were frozen.
    pub fn freeze_document(&mut self, roots: &[ScriptValue]) -> (usize, usize) {
        let mut stop: Vec<ScriptValue> = self.bx.injected_globals.values().copied().collect();
        stop.push(self.bx.heap.modules.into());
        self.bx.heap.freeze_reachable(roots, &stop)
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
