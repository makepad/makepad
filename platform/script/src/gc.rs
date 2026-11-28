use relax_attr::relax;
use relax_api::relax_api;
use crate::array::*;
use crate::function::*;
use crate::gen_index::{GenRef, GenVec};
use crate::handle::*;
use crate::heap::*;
use crate::makepad_error_log::*;

use crate::object::*;
use crate::object_heap::*;
use crate::pod::*;
use crate::regex::*;
use crate::string::ScriptStringData;
use crate::thread::*;
use crate::trap::*;
use crate::value::*;
use crate::vm::*;

use std::sync::Arc;

/// Tracks heap statistics from the last garbage collection run.
/// Used to determine when to trigger the next GC cycle.
#[relax_api(plain, run)]
#[derive(Default, Clone, Copy)]
pub struct ScriptHeapGcLast {
    pub objects: usize,
    pub strings: usize,
    pub arrays: usize,
    pub pods: usize,
    pub handles: usize,
    pub regexes: usize,
}

// Heap indices occupy 24 bits. Only objects and arrays have worklist entries.
const ARRAY_SLOT: u32 = 1 << 31;
const _: () = assert!(HEAP_INDEX_BITS < 32);

#[derive(Default, PartialEq, Eq)]
pub(crate) enum GcPhase {
    #[default]
    Idle,
    Marked,
    Skipped,
}

/// A mark-walk value whose index is beyond its table came from ANOTHER heap.
/// Marking must survive it (skip + report): panicking here aborts the whole
/// host app over one corrupted isolate value.
#[cold]
#[inline(never)]
fn foreign_value_skip(kind: &str, index: u32, len: usize) {
    eprintln!(
        "script gc: skipping foreign {kind} index {index} (table len {len}) - \
         value was minted by a different heap"
    );
}

/// The object pod type `pt` belongs to: it is collected with that object (`sweep`), so a pod,
/// a pod type value or a type built on it keeps the object, and with it the type, alive.
#[inline]
fn pod_type_owner(pod_types: &[ScriptPodTypeData], pt: ScriptPodType) -> Option<ScriptObject> {
    let owner = pod_types.get(pt.index as usize)?.object;
    (owner != ScriptObject::ZERO).then_some(owner)
}

/// What a live pod type holds: its default and, for a struct, enum or array, each field's
/// default and the owner of each field or element type (an inline type's `self_ref`).
fn pod_type_refs(pod_types: &[ScriptPodTypeData], pt: ScriptPodType, value: &mut dyn FnMut(ScriptValue)) {
    let Some(data) = pod_types.get(pt.index as usize) else { return };
    value(data.default);
    let owner = |t: &ScriptPodTypeInline| pod_type_owner(pod_types, t.self_ref).map_or(NIL, ScriptValue::from);
    let fields = |fields: &[ScriptPodField], value: &mut dyn FnMut(ScriptValue)| {
        for f in fields {
            value(f.default);
            value(owner(&f.ty));
        }
    };
    match &data.ty {
        ScriptPodTy::Struct { fields: f, .. } => fields(f, value),
        ScriptPodTy::Enum { variants, .. } => {
            for v in variants {
                match &v.variant {
                    ScriptPodEnumVariant::Bare => {}
                    ScriptPodEnumVariant::Tuple { items } => items.iter().for_each(|t| value(owner(t))),
                    ScriptPodEnumVariant::Named { fields: f } => fields(f, value),
                }
            }
        }
        ScriptPodTy::FixedArray { ty, .. } | ScriptPodTy::VariableArray { ty, .. } => value(owner(ty)),
        _ => {}
    }
}

// References are checked while their generation is still in hand. The tables
// cannot recycle a slot between queueing its index and tracing its children.
// The mark helpers are always inlined: a call per traced value made a collection about a fifth slower.
#[inline(always)]
fn push_mark<T, R: GenRef>(
    table: &GenVec<T>, ptr: R, kind: u32, name: &str, trace: impl FnOnce(&T) -> bool,
    marks: &mut [u64], work: &mut Vec<u32>,
) {
    let Some(slot) = table.get_checked(ptr) else {
        foreign_value_skip(name, ptr.index(), table.len());
        return;
    };
    if !trace(slot) { return; }
    let _ = &table[ptr]; // Preserve the generation check even on a duplicate edge.
    let index = ptr.index();
    let word = &mut marks[index as usize / 64];
    let bit = 1u64 << (index % 64);
    if *word & bit == 0 {
        *word |= bit;
        debug_assert!(work.len() < work.capacity());
        work.push(kind | index);
    }
}

#[inline]
fn is_marked(marks: &[u64], index: usize) -> bool {
    // A standalone sweep on a fresh heap has no preceding mark.
    marks.get(index / 64).is_some_and(|word| word & (1u64 << (index % 64)) != 0)
}

/// Borrow the traced tables separately from the leaf tags and worklist, so
/// map keys, array elements and pod defaults can be visited in place.
struct Marker<'a, const STATIC: bool> {
    objects: &'a GenVec<ScriptObjectData>,
    arrays: &'a GenVec<ScriptArrayData>,
    pod_types: &'a [ScriptPodTypeData],
    glob_sources: &'a std::collections::HashMap<ScriptObject, Vec<ScriptObject>>,
    strings: &'a mut GenVec<Option<ScriptStringData>>,
    pods: &'a mut GenVec<ScriptPodData>,
    handles: &'a mut GenVec<Option<ScriptHandleData>>,
    regexes: &'a mut GenVec<Option<ScriptRegexData>>,
    object_marks: &'a mut [u64],
    array_marks: &'a mut [u64],
    work: &'a mut Vec<u32>,
}

// Split these borrows so roots and body scratch can still be visited in place.
macro_rules! marker {
    ($heap:ident, $static:literal) => {
        Marker::<$static> {
            objects: &$heap.objects,
            arrays: &$heap.arrays,
            strings: &mut $heap.strings,
            pods: &mut $heap.pods,
            handles: &mut $heap.handles,
            regexes: &mut $heap.regexes,
            pod_types: &$heap.pod_types,
            glob_sources: &$heap.glob_sources,
            object_marks: &mut $heap.object_marks,
            array_marks: &mut $heap.array_marks,
            work: &mut $heap.mark_vec,
        }
    };
}

impl<const STATIC: bool> Marker<'_, STATIC> {
    #[inline(always)]
    fn object(&mut self, ptr: ScriptObject) {
        push_mark(self.objects, ptr, 0, "object", |o| !o.tag.is_static() && o.tag.is_alloced(),
            self.object_marks, self.work);
    }

    #[inline(always)]
    fn array(&mut self, ptr: ScriptArray) {
        push_mark(self.arrays, ptr, ARRAY_SLOT, "array", |a| !a.tag.is_static() && a.tag.is_alloced(),
            self.array_marks, self.work);
    }

    #[inline(always)]
    fn pod_owner(&mut self, pt: ScriptPodType) {
        if let Some(owner) = pod_type_owner(self.pod_types, pt) { self.object(owner); }
    }

    /// A root, outside the trace loop: one call each keeps `mark` small.
    #[inline(never)]
    fn mark_root(&mut self, value: ScriptValue) {
        self.mark_value(value);
    }

    #[inline(always)]
    fn mark_value(&mut self, value: ScriptValue) {
        if let Some(ptr) = value.as_object() {
            self.object(ptr);
        } else if let Some(ptr) = value.as_string() {
            if let Some(slot) = self.strings.get_checked_mut(ptr) {
                if let Some(data) = slot.as_mut() {
                    if !data.tag.is_static() {
                        if STATIC { data.tag.set_static(); } else { data.tag.set_mark(); }
                    }
                }
            } else {
                foreign_value_skip("string", ptr.index, self.strings.len());
            }
        } else if let Some(ptr) = value.as_array() {
            self.array(ptr);
        } else if let Some(ptr) = value.as_pod() {
            if let Some(pod) = self.pods.get_checked_mut(ptr) {
                if !pod.tag.is_static() && pod.tag.is_alloced() {
                    if STATIC { pod.tag.set_static(); } else { pod.tag.set_mark(); }
                    let ty = pod.ty;
                    let elem = pod.tag.as_array_builder();
                    self.pod_owner(ty);
                    if let Some(elem) = elem { self.pod_owner(elem); }
                }
            } else {
                foreign_value_skip("pod", ptr.index, self.pods.len());
            }
        } else if let Some(ptr) = value.as_handle() {
            if ptr.index != 0 {
                if let Some(slot) = self.handles.get_checked_mut(ptr) {
                    if let Some(data) = slot.as_mut() {
                        if !data.tag.is_static() {
                        if STATIC { data.tag.set_static(); } else { data.tag.set_mark(); }
                    }
                    }
                } else {
                    foreign_value_skip("handle", ptr.index, self.handles.len());
                }
            }
        } else if let Some(ptr) = value.as_regex() {
            if let Some(slot) = self.regexes.get_checked_mut(ptr) {
                if let Some(data) = slot.as_mut() {
                    if !data.tag.is_static() {
                        if STATIC { data.tag.set_static(); } else { data.tag.set_mark(); }
                    }
                }
            } else {
                foreign_value_skip("regex", ptr.index, self.regexes.len());
            }
        } else if let Some(pt) = value.as_pod_type() {
            self.pod_owner(pt);
        }
    }

    fn mark_inner(&mut self, entry: u32) {
        if entry & ARRAY_SLOT != 0 {
            let array = self.arrays.get_at((entry & !ARRAY_SLOT) as usize);
            if let ScriptArrayStorage::ScriptValue(values) = &array.storage {
                for value in values { self.mark_value(*value); }
            }
            return;
        }
        let index = entry as usize;
        let object = self.objects.get_at(index);
        self.mark_value(object.proto);
        for (key, val) in object.map.iter() {
            self.mark_value(*key);
            self.mark_value(val.value);
        }
        for kv in &object.vec {
            self.mark_value(kv.key);
            self.mark_value(kv.value);
        }
        let obj = ScriptObject::new(entry, self.objects.generation(index));
        if object.tag.has_globs() {
            if let Some(sources) = self.glob_sources.get(&obj) {
                for src in sources { self.object(*src); }
            }
        }
        if let Some(pt) = object.tag.as_pod_type() {
            if pod_type_owner(self.pod_types, pt) == Some(obj) {
                pod_type_refs(self.pod_types, pt, &mut |v| self.mark_value(v));
            } else {
                self.pod_owner(pt);
            }
        }
    }
}

#[relax_api(run)]
impl ScriptHeap {
    /// Recursively mark a value and all reachable values as static (permanent).
    /// This walks the object graph similar to GC marking but sets the static flag instead.
    pub fn set_static(&mut self, value: ScriptValue) {
        // Borrowing the bitmaps abandons a host's pending mark. Only a new mark can authorize its sweep.
        let was_idle = self.gc_phase == GcPhase::Idle;
        self.gc_phase = GcPhase::Skipped;
        if !was_idle {
            self.object_marks.fill(0);
            self.array_marks.fill(0);
        }
        self.mark_vec.clear();
        let mut walk = marker!(self, true);
        walk.mark_value(value);
        let mut i = 0;
        while i < walk.work.len() {
            walk.mark_inner(walk.work[i]);
            i += 1;
        }
        for entry in self.mark_vec.drain(..) {
            let i = (entry & !ARRAY_SLOT) as usize;
            let marks = if entry & ARRAY_SLOT == 0 {
                self.objects.get_at_mut(i).tag.set_static();
                &mut self.object_marks
            } else {
                self.arrays.get_at_mut(i).tag.set_static();
                &mut self.array_marks
            };
            marks[i / 64] &= !(1 << (i % 64));
        }
        // A walk that unwinds leaves Skipped, so its partial visit bits are cleared before reuse.
        if was_idle { self.gc_phase = GcPhase::Idle; }
    }

    #[inline]
    pub fn new_object_ref(&mut self, obj: ScriptObject) -> ScriptObjectRef {
        // Mark as reffed so free_object_if_unreffed won't free it
        self.obj_mode(obj, OBJ_MODE_REFFED, 0);
        let roots = self.heap_key() as *const crate::roots::ScriptRoots;
        crate::roots::script_root_retain(roots, crate::roots::ROOT_OBJECT, obj.index);
        ScriptObjectRef { roots, obj }
    }

    #[inline]
    pub fn new_array_ref(&mut self, array: ScriptArray) -> ScriptArrayRef {
        let roots = self.heap_key() as *const crate::roots::ScriptRoots;
        crate::roots::script_root_retain(roots, crate::roots::ROOT_ARRAY, array.index);
        ScriptArrayRef { roots, array }
    }

    pub fn new_fn_ref(&mut self, obj: ScriptObject) -> ScriptFnRef {
        ScriptFnRef(self.new_object_ref(obj))
    }

    #[inline]
    pub fn new_handle_ref(&mut self, handle: ScriptHandle) -> ScriptHandleRef {
        let roots = self.heap_key() as *const crate::roots::ScriptRoots;
        crate::roots::script_root_retain(roots, crate::roots::ROOT_HANDLE, handle.index);
        ScriptHandleRef { roots, handle }
    }

    /// How many objects the given seeds reach, following what the marker
    /// follows: the proto, glob sources, every map key and value, every vec key and value,
    /// and every element of every array met on the way. `visited` is
    /// shared in and out, so a caller can mark a tree once and then ask
    /// what each further root reaches beyond it. A leak hunt's reading.
    #[inline(always)]
    pub fn reach_count(&self, seeds: &[ScriptObject], visited: &mut std::collections::HashSet<ScriptObject>) -> usize {
        unsafe extern "C-unwind" fn insert(ctx: *mut u8, object: ScriptObject) -> bool {
            (&mut *(ctx as *mut std::collections::HashSet<ScriptObject>)).insert(object)
        }
        // SAFETY: the callback touches only visited during this synchronous walk.
        unsafe { self.reach_count_with(seeds, visited as *mut _ as *mut u8, insert) }
    }

    /// What `seeds` (objects and arrays) reach as the marker follows them, beyond what the
    /// shared visited sets already hold: inspect `vm roots`, one root kind after another.
    pub(crate) fn reach_values(&self, seeds: Vec<ScriptValue>, objects: &mut std::collections::HashSet<ScriptObject>, arrays: &mut std::collections::HashSet<ScriptArray>) -> usize {
        let mut count = 0usize;
        let mut stack = seeds;
        while let Some(val) = stack.pop() {
            if let Some(obj) = val.as_object() {
                if !self.objects.is_valid(obj) || !objects.insert(obj) {
                    continue;
                }
                count += 1;
                let object = &self.objects[obj];
                stack.push(object.proto);
                for (key, entry) in object.map.iter() {
                    stack.push(*key);
                    stack.push(entry.value);
                }
                for kv in object.vec.iter() {
                    stack.push(kv.key);
                    stack.push(kv.value);
                }
                if object.tag.has_globs() {
                    if let Some(sources) = self.glob_sources.get(&obj) {
                        stack.extend(sources.iter().map(|src| ScriptValue::from(*src)));
                    }
                }
                self.pod_type_reach(obj, &mut stack);
            } else if let Some(arr) = val.as_array() {
                if !self.arrays.is_valid(arr) || !arrays.insert(arr) {
                    continue;
                }
                for i in 0..self.array_len(arr) {
                    stack.push(self.array_index_unchecked(arr, i));
                }
            } else {
                self.pod_value_reach(val, &mut stack);
            }
        }
        count
    }

    /// [`Self::reach_count`] with the visited set behind `insert`, which
    /// answers whether an object was newly added.
    ///
    /// # Safety
    /// `ctx` must remain valid for `insert` throughout this synchronous walk.
    #[relax_api(run)]
    pub unsafe fn reach_count_with(&self, seeds: &[ScriptObject], ctx: *mut u8, insert: unsafe extern "C-unwind" fn(*mut u8, ScriptObject) -> bool) -> usize {
        let mut arrays_seen: std::collections::HashSet<ScriptArray> = Default::default();
        let mut count = 0usize;
        let mut stack: Vec<ScriptValue> = seeds.iter().map(|o| ScriptValue::from(*o)).collect();
        while let Some(val) = stack.pop() {
            if let Some(obj) = val.as_object() {
                if !self.objects.is_valid(obj) || !insert(ctx, obj) {
                    continue;
                }
                count += 1;
                let object = &self.objects[obj];
                stack.push(object.proto);
                for (key, entry) in object.map.iter() {
                    stack.push(*key);
                    stack.push(entry.value);
                }
                for kv in object.vec.iter() {
                    stack.push(kv.key);
                    stack.push(kv.value);
                }
                if object.tag.has_globs() {
                    if let Some(sources) = self.glob_sources.get(&obj) {
                        stack.extend(sources.iter().map(|src| ScriptValue::from(*src)));
                    }
                }
                self.pod_type_reach(obj, &mut stack);
            } else if let Some(arr) = val.as_array() {
                if !arrays_seen.insert(arr) {
                    continue;
                }
                let len = self.array_len(arr);
                for i in 0..len {
                    stack.push(self.array_index_unchecked(arr, i));
                }
            } else {
                self.pod_value_reach(val, &mut stack);
            }
        }
        count
    }

    /// The marker's pod type edges for a reach walk: object `obj` carrying a pod type reaches
    /// what the type holds (its owner) or its owner (an object made from the owner).
    fn pod_type_reach(&self, obj: ScriptObject, stack: &mut Vec<ScriptValue>) {
        if let Some(pt) = self.objects[obj].tag.as_pod_type() {
            match pod_type_owner(&self.pod_types, pt) {
                Some(owner) if owner == obj => pod_type_refs(&self.pod_types, pt, &mut |v| stack.push(v)),
                Some(owner) => stack.push(owner.into()),
                None => {}
            }
        }
    }

    /// The marker's pod type edges for a reach walk: a pod or a pod type value reaches the
    /// owner of its type.
    fn pod_value_reach(&self, val: ScriptValue, stack: &mut Vec<ScriptValue>) {
        let types = if let Some(pod) = val.as_pod() {
            match self.pods.get_checked(pod) {
                Some(p) => [Some(p.ty), p.tag.as_array_builder()],
                None => return,
            }
        } else if let Some(pt) = val.as_pod_type() {
            [Some(pt), None]
        } else {
            return;
        };
        for pt in types.into_iter().flatten() {
            if let Some(owner) = pod_type_owner(&self.pod_types, pt) {
                stack.push(owner.into());
            }
        }
    }

    #[relax_api(run)]
    #[relax(keep_reachable)]
    pub fn mark(&mut self, threads: &ScriptThreads, code: &ScriptCode) {
        if !self.preflight_gc(code) {
            self.gc_phase = GcPhase::Skipped;
            return;
        }
        self.gc_phase = GcPhase::Marked;
        self.mark_vec.clear();
        self.object_marks.fill(0);
        self.array_marks.fill(0);
        let mut marker = marker!(self, false);

        // Mark type_check protos
        for i in 0..self.type_check.len() {
            if let Some(object) = &self.type_check[i].object {
                marker.mark_root(object.proto);
            }
        }

        // Mark type_defaults objects
        for obj in self.type_defaults.values().copied() {
            marker.object(obj);
        }

        // Mark the Rust-held refs' roots (roots.rs): slot indices, at their current generation
        {
            let roots = self.roots.get();
            for index in roots.rooted_slots(crate::roots::ROOT_OBJECT) {
                let generation = self.objects.generation(index as usize);
                marker.object(ScriptObject::new(index, generation));
            }
            for index in roots.rooted_slots(crate::roots::ROOT_ARRAY) {
                let generation = self.arrays.generation(index as usize);
                marker.array(ScriptArray::new(index, generation));
            }
            let handles = &mut marker.handles;
            for index in roots.rooted_slots(crate::roots::ROOT_HANDLE) {
                if let Some(handle_data) = handles.get_at_mut(index as usize).as_mut() {
                    handle_data.tag.set_mark();
                }
            }
        }

        // Mark thread stacks
        for thread_idx in 0..threads.len() {
            if let Some(thread) = threads.get(thread_idx) {
                // Stack values
                for value in thread.stack.iter() {
                    marker.mark_root(*value);
                }
                // Frame-local slot values
                for value in thread.slots.iter() {
                    marker.mark_root(*value);
                }
                // Scopes
                for scope in thread.scopes.iter() {
                    marker.mark_root((*scope).into());
                }
                // Method call contexts
                for me in thread.mes.iter() {
                    match me {
                        ScriptMe::Object(obj) => {
                            marker.mark_root((*obj).into());
                        }
                        ScriptMe::Call { sself, args, .. } => {
                            if let Some(s) = sself {
                                marker.mark_root(*s);
                            }
                            marker.mark_root((*args).into());
                        }
                        ScriptMe::Pod { pod, .. } => {
                            marker.mark_root((*pod).into());
                        }
                        ScriptMe::Array(arr) => {
                            marker.mark_root((*arr).into());
                        }
                    }
                }
                // Loop sources
                for loop_frame in thread.loops.iter() {
                    if let Some(loop_values) = &loop_frame.values {
                        marker.mark_root(loop_values.source);
                    }
                }
                for value in thread.continuation_roots() {
                    marker.mark_root(value);
                }
                // Trap error values
                for err in thread.trap.err_borrow().iter() {
                    marker.mark_root(err.value);
                }
                // Trap return/bail values
                match thread.trap.get_on() {
                    Some(ScriptTrapOn::CallBack { scope, .. }) => marker.mark_root(scope.into()),
                    Some(ScriptTrapOn::Return(v) | ScriptTrapOn::Uncaught(v) | ScriptTrapOn::Bail(v)) => {
                        marker.mark_root(v);
                    }
                    _ => {}
                }
            }
        }

        // Mark ScriptBody scope and me objects (a transient body's only
        // while it is alive: `mark_transient_bodies`)
        for body in code.bodies.borrow().iter() {
            if body.transient || matches!(body.source, ScriptSource::Free) {
                continue;
            }
            if let ScriptSource::Mod(script_mod) = &body.source {
                for v in script_mod.values.iter() {
                    marker.mark_root(*v);
                }
            }

            // Mark tokenizer string literals as roots
            for str_val in body.tokenizer.iter_strings() {
                if let Some(ptr) = str_val.as_string() {
                    if let Some(str_data) = marker.strings[ptr].as_mut() {
                        if !str_data.tag.is_static() {
                            str_data.tag.set_mark();
                        }
                    }
                }
            }
        }

        // Mark ScriptNative type_table objects
        for type_map in code.native.borrow().type_table.iter() {
            for (_, obj) in type_map.iter() {
                marker.mark_root((*obj).into());
            }
        }

        // Process the work list - use while loop since mark_inner adds to mark_vec
        marker.drain_mark_vec();
        marker.mark_transient_bodies(threads, code, &mut self.transient_bodies,
            &mut self.live_bodies, &mut self.newly_live_bodies);
    }
}

impl<const STATIC: bool> Marker<'_, STATIC> {
    fn drain_mark_vec(&mut self) {
        let mut i = 0;
        while i < self.work.len() {
            let mark = self.work[i];
            self.mark_inner(mark);
            i += 1;
        }
        self.work.clear();
    }

    /// Transient bodies ([`crate::vm::ScriptVm::eval_transient`]) live while
    /// a thread runs in them, a live fn is defined in them or a live object
    /// was built by them; a live body keeps its scope, `me`, values and
    /// string literals, which can make more bodies live (to a fixpoint).
    /// The rest are freed here: their objects are unmarked, and the sweep
    /// that follows takes them.
    fn mark_transient_bodies(&mut self, threads: &ScriptThreads, code: &ScriptCode,
        transient: &mut Vec<bool>, live: &mut Vec<bool>, newly: &mut Vec<usize>,
    ) {
        transient.clear();
        transient.extend(code.bodies.borrow().iter().map(|b| b.transient));
        if !transient.contains(&true) {
            return;
        }
        live.clear();
        live.resize(transient.len(), false);
        newly.clear();
        let note = |ip: ScriptIp, live: &mut Vec<bool>, newly: &mut Vec<usize>| {
            let b = ip.body as usize;
            if !ip.is_unknown() && transient.get(b) == Some(&true) && !live[b] {
                live[b] = true;
                newly.push(b);
            }
        };
        for thread_idx in 0..threads.len() {
            if let Some(thread) = threads.get(thread_idx) {
                note(thread.trap.ip, live, newly);
                for call in thread.calls.iter() {
                    if let Some(ip) = call.return_ip {
                        note(ip, live, newly);
                    }
                }
            }
        }
        loop {
            for i in 1..self.objects.len() {
                let obj = self.objects.get_at(i);
                if !obj.tag.is_alloced() || !(is_marked(self.object_marks, i) || obj.tag.is_static()) {
                    continue;
                }
                if let Some(ScriptFnPtr::Script(ip)) = obj.tag.as_fn() {
                    note(ip, live, newly);
                }
                note(obj.made_at, live, newly);
            }
            if newly.is_empty() {
                break;
            }
            {
                let bodies = code.bodies.borrow();
                for b in newly.drain(..) {
                    let body = &bodies[b];
                    self.object(body.scope.as_object());
                    self.object(body.me.as_object());
                    if let Some(end) = &body.end_scope {
                        self.object(end.as_object());
                    }
                    if let Some(script_mod) = body.source.written() {
                        for v in script_mod.values.iter() {
                            self.mark_value(*v);
                        }
                    }
                    for str_val in body.tokenizer.iter_strings() {
                        if let Some(ptr) = str_val.as_string() {
                            if let Some(str_data) = self.strings[ptr].as_mut() {
                                if !str_data.tag.is_static() {
                                    str_data.tag.set_mark();
                                }
                            }
                        }
                    }
                }
            }
            self.drain_mark_vec();
        }
        let mut bodies = code.bodies.borrow_mut();
        let mut free = code.free_bodies.borrow_mut();
        for (b, body) in bodies.iter_mut().enumerate() {
            if body.transient && !live[b] {
                *body = ScriptBody {
                    source: ScriptSource::Free,
                    effective_code: String::new(),
                    tokenizer: crate::tokenizer::ScriptTokenizer::vacant(),
                    parser: crate::parser::ScriptParser::vacant(),
                    scope: Default::default(),
                    me: Default::default(),
                    end_scope: None,
                    checkpoint: None,
                    source_len: 0,
                    content_hash: 0,
                    transient: false,
                    is_script_mod: false,
                    real_file: String::new(),
                    shown: None,
                    first_col: 0,
                };
                free.push(b as u16);
            }
        }
    }
}

#[relax_api(run)]
impl ScriptHeap {
    #[relax_api(run)]
    #[relax(keep_reachable)]
    pub fn sweep(&mut self, log_stats: bool) {
        if self.gc_phase == GcPhase::Skipped { return; }
        let start = crate::clock::monotonic_now();

        // GC stats: (static, alive, removed)
        let (mut obj_static, mut obj_alive, mut obj_removed) = (0usize, 0usize, 0usize);
        let (mut arr_static, mut arr_alive, mut arr_removed) = (0usize, 0usize, 0usize);
        let (mut str_static, mut str_alive, mut str_removed) = (0usize, 0usize, 0usize);
        let (mut hdl_static, mut hdl_alive, mut hdl_removed) = (0usize, 0usize, 0usize);
        let (mut pod_static, mut pod_alive, mut pod_removed) = (0usize, 0usize, 0usize);
        let (mut rex_static, mut rex_alive, mut rex_removed) = (0usize, 0usize, 0usize);

        for i in 1..self.objects.len() {
            let obj = &mut self.objects.get_at_mut(i);
            // Skip static objects - they are permanent
            if obj.tag.is_static() {
                obj_static += 1;
                continue;
            }
            if !is_marked(&self.object_marks, i) && obj.tag.is_alloced() {
                let pod_ty = obj.tag.as_pod_type();
                let has_globs = obj.tag.has_globs();
                obj.clear();
                if has_globs {
                    self.glob_sources.remove(&ScriptObject::new(i as u32, self.objects.generation(i)));
                }
                if let Some(pt) = pod_ty {
                    self.free_pod_type_of(ScriptObject::new(i as u32, self.objects.generation(i)), pt);
                }
                // Increment generation so stale references will be detected
                self.objects.free_slot(i as u32);
                // Push ref with NEW generation to free list - ready to reuse
                let new_gen = self.objects.generation(i);
                self.objects_free.push(ScriptObject::new(i as u32, new_gen));
                obj_removed += 1;
            } else {
                if obj.tag.is_alloced() {
                    obj_alive += 1;
                }
            }
        }
        if obj_removed != 0 {
            self.bump_object_reuse_epoch();
        }
        for i in 1..self.arrays.len() {
            let array = &mut self.arrays.get_at_mut(i);
            // Skip static arrays - they are permanent
            if array.tag.is_static() {
                arr_static += 1;
                continue;
            }
            if !is_marked(&self.array_marks, i) && array.tag.is_alloced() {
                array.clear();
                // Increment generation, then push ref with new generation
                self.arrays.free_slot(i as u32);
                let new_gen = self.arrays.generation(i);
                self.arrays_free.push(ScriptArray::new(i as u32, new_gen));
                arr_removed += 1;
            } else {
                if array.tag.is_alloced() {
                    arr_alive += 1;
                }
            }
        }
        // always leave the empty null string at 0
        for i in 1..self.strings.len() {
            if let Some(str) = &mut self.strings.get_at_mut(i) {
                // Skip static strings - they are permanent
                if str.tag.is_static() {
                    str_static += 1;
                    continue;
                }
                if !str.tag.is_marked() {
                    if let Some((k, _)) = self.string_intern.remove_entry(&str.string) {
                        self.strings.set_at(i, None);
                        // Normal string construction may grow the reuse pool. A sweep only
                        // returns buffers that fit the pool's existing capacity.
                        if self.strings_reuse.len() < self.strings_reuse.capacity() {
                            if let Some(s) = Arc::into_inner(k.0) {
                                self.recycle_string(s);
                            }
                        }
                        // Increment generation, then push ref with new generation
                        self.strings.free_slot(i as u32);
                        let new_gen = self.strings.generation(i);
                        self.strings_free.push(ScriptString::new(i as u32, new_gen));
                        str_removed += 1;
                    }
                } else {
                    str_alive += 1;
                    str.tag.clear_mark();
                }
            }
        }
        for i in 1..self.handles.len() {
            if let Some(handle) = &mut self.handles.get_at_mut(i) {
                // Skip static handles - they are permanent
                if handle.tag.is_static() {
                    hdl_static += 1;
                    continue;
                }
                if !handle.tag.is_marked() {
                    let handle_data = self.handles.get_at_mut(i).take().unwrap();
                    handle_data.gc();
                    // Increment generation, then push ref with new generation
                    self.handles.free_slot(i as u32);
                    let new_gen = self.handles.generation(i);
                    // Note: ScriptHandle also needs a type, but for free list we use type 0
                    self.handles_free.push(ScriptHandle::new(
                        ScriptHandleType(0),
                        i as u32,
                        new_gen,
                    ));
                    hdl_removed += 1;
                } else {
                    hdl_alive += 1;
                    handle.tag.clear_mark();
                }
            }
        }
        for i in 1..self.pods.len() {
            let pod = &mut self.pods.get_at_mut(i);
            // Skip static pods - they are permanent
            if pod.tag.is_static() {
                pod_static += 1;
                continue;
            }
            if !pod.tag.is_marked() && pod.tag.is_alloced() {
                pod.clear();
                // Increment generation, then push ref with new generation
                self.pods.free_slot(i as u32);
                let new_gen = self.pods.generation(i);
                self.pods_free.push(ScriptPod::new(i as u32, new_gen));
                pod_removed += 1;
            } else {
                if pod.tag.is_alloced() {
                    pod_alive += 1;
                }
                pod.tag.clear_mark();
            }
        }

        for i in 1..self.regexes.len() {
            if let Some(re) = &mut self.regexes.get_at_mut(i) {
                if re.tag.is_static() {
                    rex_static += 1;
                    continue;
                }
                if !re.tag.is_marked() {
                    // Build the intern key to remove from intern table
                    let key = RegexInternKey {
                        pattern: std::mem::take(&mut re.pattern),
                        flags: re.flags,
                    };
                    self.regex_intern.remove(&key);
                    self.regexes.set_at(i, None);
                    self.regexes.free_slot(i as u32);
                    let new_gen = self.regexes.generation(i);
                    self.regexes_free.push(ScriptRegex::new(i as u32, new_gen));
                    rex_removed += 1;
                } else {
                    rex_alive += 1;
                    re.tag.clear_mark();
                }
            }
        }

        self.object_marks.fill(0);
        self.array_marks.fill(0);
        self.gc_phase = GcPhase::Idle;

        // Record heap statistics after GC for triggering next cycle
        self.gc_last = ScriptHeapGcLast {
            objects: self.objects.len() - self.objects_free.len(),
            strings: self.strings.len() - self.strings_free.len(),
            arrays: self.arrays.len() - self.arrays_free.len(),
            pods: self.pods.len() - self.pods_free.len(),
            handles: self.handles.len() - self.handles_free.len(),
            regexes: self.regexes.len() - self.regexes_free.len(),
        };
        // A collection is the one point where retained capacity can shrink;
        // re-derive the retained-heap cap's headroom from the swept state.
        self.reconcile_heap_bytes_if_limited();
        let gc_ns = ((crate::clock::monotonic_now() - start) * 1e9) as u64;
        // Print compact GC stats: S=static A=alive R=removed
        let elapsed_us = ((crate::clock::monotonic_now() - start) * 1_000_000.0) as u128;
        if log_stats {
            log!("GC {}us: obj[S:{} A:{} R:{}] arr[S:{} A:{} R:{}] str[S:{} A:{} R:{}] hdl[S:{} A:{} R:{}] pod[S:{} A:{} R:{}] rex[S:{} A:{} R:{}]",
                elapsed_us,
                obj_static, obj_alive, obj_removed,
                arr_static, arr_alive, arr_removed,
                str_static, str_alive, str_removed,
                hdl_static, hdl_alive, hdl_removed,
                pod_static, pod_alive, pod_removed,
                rex_static, rex_alive, rex_removed);
        }

        // the flight recorder's trace of the script VM's collections (makepad run)
        if makepad_error_log::flight::on() {
            use std::io::Write;
            // Seven integer fields, each at most 20 digits, plus the literal text.
            const SIZE: usize = 224;
            let mut buf = [0u8; SIZE];
            let mut out = &mut buf[..];
            write!(out, "GC {} us: objects {} alive {} removed, arrays {} / {}, strings {} / {}",
                gc_ns / 1000, obj_alive, obj_removed, arr_alive, arr_removed, str_alive, str_removed)
                .expect("the collection statistics fit the stack buffer");
            let len = SIZE - out.len();
            let message = std::str::from_utf8(&buf[..len]).unwrap();
            makepad_error_log::flight::mark_ab("script.gc", message, gc_ns, (obj_removed + arr_removed + str_removed) as u64);
        }
        crate::inspect_registry::gc_done(self, gc_ns);
    }

    /// Check if garbage collection should be triggered.
    ///
    /// Objects and pods are summed: collect when their sum has doubled since
    /// the last collection and reached their combined minimum thresholds,
    /// because a mark costs in proportion to the live set. Strings, arrays,
    /// handles and regexes double on their own, each with a minimum threshold
    /// to avoid GC thrashing on small heaps.

    #[inline]
    pub fn needs_gc(&self) -> bool {
        self.heap_stat(HEAP_STAT_NEEDS_GC) != 0
    }

    pub(crate) fn needs_gc_impl(&self) -> bool {
        if GC_STRESS.load(std::sync::atomic::Ordering::Relaxed) {
            return true;
        }
        // Minimum thresholds before GC can trigger (avoid thrashing on small heaps)
        const MIN_OBJECTS: usize = 1024;
        const MIN_STRINGS: usize = 256;
        const MIN_ARRAYS: usize = 128;
        const MIN_PODS: usize = 128;
        const MIN_HANDLES: usize = 64;

        // Growth factor - trigger GC when heap doubles (2x)
        // This is similar to Lua's default and V8's heuristics
        const GROWTH_FACTOR: usize = 2;

        let objects = self.objects.len() - self.objects_free.len();
        let strings = self.strings.len() - self.strings_free.len();
        let arrays = self.arrays.len() - self.arrays_free.len();
        let pods = self.pods.len() - self.pods_free.len();
        let handles = self.handles.len() - self.handles_free.len();

        // Objects and pods grow together; other categories grow independently.
        let grown = objects + pods;
        if grown >= MIN_OBJECTS + MIN_PODS
            && grown >= (self.gc_last.objects + self.gc_last.pods) * GROWTH_FACTOR
        {
            return true;
        }
        if strings >= MIN_STRINGS && strings >= self.gc_last.strings * GROWTH_FACTOR {
            return true;
        }
        if arrays >= MIN_ARRAYS && arrays >= self.gc_last.arrays * GROWTH_FACTOR {
            return true;
        }
        if handles >= MIN_HANDLES && handles >= self.gc_last.handles * GROWTH_FACTOR {
            return true;
        }

        // Regexes are interned so unlikely to grow fast, but check anyway
        let regexes = self.regexes.len() - self.regexes_free.len();
        if regexes >= MIN_HANDLES && regexes >= self.gc_last.regexes * GROWTH_FACTOR {
            return true;
        }

        false
    }

    /// Current allocated entries by category, excluding free slots.
    #[relax_api(run)]
    pub fn gc_live_counts(&self) -> ScriptHeapGcLast {
        ScriptHeapGcLast {
            objects: self.objects.len() - self.objects_free.len(),
            strings: self.strings.len() - self.strings_free.len(),
            arrays: self.arrays.len() - self.arrays_free.len(),
            pods: self.pods.len() - self.pods_free.len(),
            handles: self.handles.len() - self.handles_free.len(),
            regexes: self.regexes.len() - self.regexes_free.len(),
        }
    }

    #[inline]
    pub fn gc_live_len(&self) -> usize {
        self.heap_stat(HEAP_STAT_LIVE_LEN) as usize
    }

    pub(crate) fn gc_live_len_impl(&self) -> usize {
        let objects = self.objects.len() - self.objects_free.len();
        let strings = self.strings.len() - self.strings_free.len();
        let arrays = self.arrays.len() - self.arrays_free.len();
        let pods = self.pods.len() - self.pods_free.len();
        let handles = self.handles.len() - self.handles_free.len();
        let regexes = self.regexes.len() - self.regexes_free.len();
        objects + strings + arrays + pods + handles + regexes
    }

    /// Conservative escape barrier. Called by every *checked* store funnel
    /// (`set_value`, scope def/set, `vec_push`, `array_push`, splat merges,
    /// `force_value_in_map`): any object VALUE stored into a script-reachable
    /// container is tagged REFFED, so the eager-free paths
    /// ([`Self::free_object_if_unreffed`], scope frees at call exit,
    /// `ScriptVm::release_transient`) can never free an object the script may
    /// have retained. REFFED does NOT block normal GC — an unreachable REFFED
    /// object is still swept — it only disables *eager* frees. The
    /// `*_unchecked` store variants skip the barrier on purpose: they are the
    /// host fast path for building containers the host itself will release.
    #[inline]
    pub fn escape_value(&mut self, v: ScriptValue) {
        if let Some(obj) = v.as_object() {
            if self.objects.is_valid(obj) {
                self.ref_object_chain(obj);
            }
        }
    }

    /// Tag `obj` REFFED, and its proto chain up to the first object that already is. Keeps the
    /// invariant that a REFFED object's protos are all REFFED: a loop scope does not tag its proto
    /// (the enclosing scope, `OBJ_NEW_LOOP_SCOPE`), so when a loop scope escapes (a closure made in
    /// the loop body, `scope` stored as a value) the scopes it reads through are pinned with it
    /// and no eager free at loop or call exit can take them. Stops at the first REFFED object: by
    /// the invariant everything above it is REFFED already.
    #[inline]
    pub(crate) fn ref_object_chain(&mut self, obj: ScriptObject) {
        let mut cur = obj;
        while self.objects.is_valid(cur) {
            let object = &mut self.objects[cur];
            if !object.tag.is_alloced() || object.tag.is_reffed() {
                return;
            }
            object.tag.set_reffed();
            match object.proto.as_object() {
                Some(proto) => cur = proto,
                None => return,
            }
        }
    }

    /// Diagnostic/test accessor for the escape barrier.
    #[relax_api(run)]
    pub fn is_object_reffed(&self, obj: ScriptObject) -> bool {
        self.objects.is_valid(obj) && self.objects[obj].tag.is_reffed()
    }

    /// Live (allocated, non-free) object slot count — the flat-heap metric
    /// for host transient tests.
    #[relax_api(run)]
    pub fn live_object_len(&self) -> usize {
        self.objects.len() - self.objects_free.len()
    }

    /// Free a host-transient value now if it never escaped into script
    /// structures (see [`Self::escape_value`]). Safe by construction: a value
    /// the script retained is REFFED and this is a no-op (normal GC collects
    /// it when it actually dies). Non-object values are ignored.
    pub fn free_value_if_unreffed(&mut self, v: ScriptValue) {
        if let Some(obj) = v.as_object() {
            self.free_object_if_unreffed(obj);
        }
    }

    pub fn free_object_if_unreffed(&mut self, ptr: ScriptObject) {
        // Free lists can grow. A physical refusal drops the scope roots during
        // the bail and leaves their slots to the next ordinary collection.
        if self.memory_refusal_pending() || self.is_allocation_poison_object(ptr) { return; }
        // Check if reference is still valid (may have been freed by GC)
        if !self.objects.is_valid(ptr) {
            return;
        }
        let obj = &mut self.objects[ptr];
        // Must check is_alloced to avoid double-freeing
        if obj.tag.is_alloced() && !obj.tag.is_reffed() && !obj.tag.is_static() {
            let pod_ty = obj.tag.as_pod_type();
            if obj.tag.has_globs() {
                self.glob_sources.remove(&ptr);
            }
            obj.clear();
            if let Some(pt) = pod_ty {
                self.free_pod_type_of(ptr, pt);
            }
            // Increment generation so stale references will be detected
            self.objects.free_slot(ptr.index);
            // Push ref with NEW generation to free list - ready to reuse
            let new_gen = self.objects.generation(ptr.index as usize);
            self.objects_free
                .push(ScriptObject::new(ptr.index, new_gen));
            self.bump_object_reuse_epoch();
        }
    }

    /// The object `owner`, which carried pod type `pt`, is freed: when it is the type's owner
    /// the type goes too (an object made from the owner carries its type but does not own it).
    /// The slot is emptied, so a scan for an equal type (`new_pod_array_type`) never finds it.
    fn free_pod_type_of(&mut self, owner: ScriptObject, pt: ScriptPodType) {
        let index = pt.index as usize;
        if pod_type_owner(&self.pod_types, pt) != Some(owner) {
            return;
        }
        self.pod_types[index] = ScriptPodTypeData::default();
        self.shader_types[index].take();
        self.pod_types_free.push(pt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_collection_after_an_abandoned_mark_traces_the_current_graph() {
        use crate::makepad_live_id::*;
        let mut heap = ScriptHeap::empty();
        let threads = ScriptThreads::empty();
        let code = ScriptCode::default();
        let parent = heap.new_object();
        let _parent_root = heap.new_object_ref(parent);
        let array = heap.new_array();
        heap.set_value_def(parent, id!(items).into(), array.into());
        let removed = heap.new_object();
        let root = heap.new_object_ref(removed);
        heap.mark(&threads, &code); // Abandon this collection before its sweep.

        let child = heap.new_object();
        heap.set_value_def(parent, id!(child).into(), child.into());
        let element = heap.new_object();
        heap.array_push(array, element.into(), NoTrap);
        drop(root);
        heap.mark(&threads, &code);
        heap.sweep(false);
        assert!(heap.object_alive(child), "the marked parent was retraced after gaining a child");
        assert!(heap.object_alive(element), "the marked array was retraced after gaining an element");
        assert!(!heap.object_alive(removed), "the abandoned mark did not keep the dropped root alive");
    }

    /// The mark never traces a static object or array, so nothing may store a collectable
    /// value into one: every store path refuses, and a collection leaves the static graph whole.
    #[test]
    fn a_store_into_a_static_value_is_refused_so_collection_loses_nothing() {
        use crate::makepad_live_id::*;
        use crate::vm::{ScriptMod, ScriptVm, ScriptVmBase, ScriptVmHost};
        let eval = |vm: &mut ScriptVm, code: &str| {
            vm.eval(ScriptMod { file: "static_store".to_owned(), code: format!("{code}\n;"), ..Default::default() })
        };
        let mut host = ScriptVmHost::new((), ());
        let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
        let api = vm.new_module(id!(api));
        vm.capture_errors();
        eval(&mut vm, "mod.api.store = {list: [1], items: {}}");
        vm.bx.heap.set_static(api.into());
        let store = eval(&mut vm, "mod.api.store").as_object().expect("the store object");
        assert!(vm.bx.heap.objects[store].tag.is_static(), "set_static reaches the store");
        for (case, write, read) in [
            ("property", "mod.api.store.cache = {x: 41}", "mod.api.store.cache"),
            ("local property", "let s = mod.api.store\ns.cache = {x: 41}", "mod.api.store.cache"),
            ("array push", "mod.api.store.list.push({x: 41})", "mod.api.store.list[1]"),
            ("array index", "mod.api.store.list[0] = {x: 41}", "mod.api.store.list[0]"),
            ("vector index", "mod.api.store.items[0] = {x: 41}", "mod.api.store.items[0]"),
            ("vector push", "mod.api.store.items.push({x: 41})", "mod.api.store.items[0]"),
        ] {
            eval(&mut vm, write);
            assert!(!eval(&mut vm, read).is_object(), "{case}: the store was refused");
            vm.gc();
            assert!(!eval(&mut vm, read).is_object(), "{case}: still nothing there after a collection");
            assert_eq!(eval(&mut vm, "mod.api.store.list[0]").as_number(), Some(1.0), "{case}: the static graph is whole");
        }
        vm.take_errors();
    }

    /// A stale handle -- an object collected, its slot taken by a new one --
    /// reports dead and resolves to nothing; it never aliases the newcomer.
    /// The generation that tells them apart is unconditional (no feature).
    #[test]
    fn a_collected_objects_handle_stays_dead_after_its_slot_is_reused() {
        let mut heap = ScriptHeap::empty();
        let proto = heap.new_object();
        let old = heap.new_with_proto(proto.into());
        assert!(heap.object_alive(old));
        // Nothing roots either: a collection with nothing marked takes both.
        heap.sweep(false);
        assert!(!heap.object_alive(old) && !heap.object_alive(proto));
        let mut reused = None;
        for _ in 0..64 {
            let o = heap.new_object();
            if o.index() == old.index() {
                reused = Some(o);
                break;
            }
        }
        let reused = reused.expect("the freed slot is handed out again");
        assert_ne!(reused, old, "the newcomer carries another generation");
        assert!(heap.object_alive(reused));
        assert!(!heap.object_alive(old), "the old handle must not alias the newcomer");
        assert!(!heap.objects.is_valid(old));
        // A prototype walk through the old handle stops at once.
        let mut walk = Some(old);
        let mut steps = 0;
        while let Some(o) = walk {
            if !heap.object_alive(o) {
                break;
            }
            steps += 1;
            walk = heap.proto(o).as_object();
        }
        assert_eq!(steps, 0);
        // Reading through it panics loudly rather than returning the newcomer's data.
        let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| heap.objects[old].proto));
        assert!(read.is_err());
    }

    /// A value minted by one heap and resolved in another fails the generation compare every
    /// access already does, and the panic names both heaps; liveness asks say "not here".
    #[test]
    fn a_foreign_handle_is_refused_with_both_heaps_named() {
        let mut a = ScriptHeap::empty();
        let mut b = ScriptHeap::empty();
        assert_ne!(a.heap_tag(), b.heap_tag());
        for _ in 0..8 {
            b.new_object();
        }
        let from_a = a.new_object();
        assert!(a.object_alive(from_a));
        assert!(!b.object_alive(from_a), "another heap's ref is never alive here");
        assert!(b.objects.get_checked(from_a).is_none(), "the mark walk skips it");
        let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| b.objects[from_a].proto));
        let msg = read.err().and_then(|p| p.downcast::<String>().ok()).map(|s| *s).unwrap_or_default();
        assert!(msg.contains(&format!("from heap {} used in heap {}", a.heap_tag(), b.heap_tag())), "{msg}");
        // The null value is every heap's.
        assert!(b.objects.is_valid(ScriptObject::ZERO) && a.objects.is_valid(ScriptObject::ZERO));
        // Through a ScriptValue too: the tag rides in the value's payload.
        let v: ScriptValue = from_a.into();
        assert_eq!(v.as_object(), Some(from_a));
    }
}

/// Stress mode (a trace host's run that must reach the collector's paths):
/// every safepoint collects ([`set_gc_stress`]).
static GC_STRESS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// (`crate::end_run`) The next run starts by the growth rule.
pub(crate) fn end_run() {
    GC_STRESS.store(false, std::sync::atomic::Ordering::Relaxed);
}

/// Collect at every safepoint from now on (`on`), or by the growth rule again. The trace host's
/// switch below is its one caller.
#[cfg(target_arch = "wasm32")]
pub(crate) fn set_gc_stress(on: bool) {
    GC_STRESS.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// The trace host's stress switch (`makepad-web-trace --stress`).
#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn wasm_script_gc_stress(on: u32) {
    set_gc_stress(on != 0);
}
