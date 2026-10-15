use makepad_relax_api::relax_api;
use crate::array::*;
use crate::gc::*;
use crate::gen_index::{GenSlot, GenVec};
use crate::handle::*;
use crate::makepad_live_id::*;
use crate::object::*;
use crate::pod::*;
use crate::regex::*;
use crate::string::*;
use crate::traits::*;
use crate::object_heap::*;
use crate::value::*;

use std::collections::HashMap;
use std::fmt::Write;
use std::mem::size_of;

/// Result of one execution-scoped script allocation budget.
#[relax_api(plain)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScriptAllocationReport {
    /// Logical heap bytes charged while the budget was installed.
    pub allocated_bytes: usize,
    /// True when an allocation was refused before growing its container.
    pub exceeded: bool,
}

#[relax_api(handle)]
#[derive(Clone, Debug)]
pub(crate) struct ScriptAllocationBudget {
    remaining: usize,
    allocated: usize,
    exceeded: bool,
    error: Option<String>,
}

impl ScriptAllocationBudget {
    fn new(limit: usize) -> Self {
        Self {
            remaining: limit,
            allocated: 0,
            exceeded: false,
            error: None,
        }
    }

    /// Charge logical payload bytes. Arithmetic is saturating so a hostile
    /// sparse index cannot wrap into a small allocation request. `kind` names
    /// the limit in the recorded diagnostic ("allocation" for a scoped run
    /// budget, "heap allocation" for the persistent retained cap).
    fn charge(&mut self, bytes: usize, operation: &'static str, kind: &'static str) -> bool {
        if self.exceeded {
            return false;
        }
        if bytes > self.remaining {
            self.exceeded = true;
            self.error = Some(format!(
                "script {kind} limit exceeded while {operation}: requested {bytes} bytes, {} remaining",
                self.remaining
            ));
            return false;
        }
        self.remaining -= bytes;
        self.allocated = self.allocated.saturating_add(bytes);
        true
    }
}

// heap_walk kinds
pub const HEAP_WALK_TYPE_ROOTS: u32 = 0;
pub const HEAP_WALK_ALLOCED: u32 = 1;
pub const HEAP_WALK_ROOTED: u32 = 2;

/// The caller's `push(ctx, object)` behind heap_walk's `push(ctx, default, object)`.
struct ObjectWalk {
    ctx: *mut u8,
    push: unsafe extern "C-unwind" fn(*mut u8, ScriptObject),
}

unsafe extern "C-unwind" fn object_walk_push(walk: *mut u8, _default: bool, object: ScriptObject) {
    let walk = &*(walk as *const ObjectWalk);
    (walk.push)(walk.ctx, object)
}

// heap_charge modes
pub const HEAP_CHARGE_BYTES: u32 = 0;
pub const HEAP_CHARGE_MAP_ENTRY: u32 = 1;

// heap_stat counters
pub const HEAP_STAT_KEY: u32 = 0;
pub const HEAP_STAT_TYPE_SLOTS: u32 = 1;
pub const HEAP_STAT_TYPE_DEFAULTS: u32 = 2;
pub const HEAP_STAT_POD_TYPES: u32 = 3;
pub const HEAP_STAT_ROOTED_OBJECTS: u32 = 4;
pub const HEAP_STAT_ROOTED_ARRAYS: u32 = 5;
pub const HEAP_STAT_ROOTED_HANDLES: u32 = 6;
pub const HEAP_STAT_OBJECTS_LEN: u32 = 7;
pub const HEAP_STAT_REUSE_EPOCH: u32 = 8;
pub const HEAP_STAT_LIVE_LEN: u32 = 9;
pub const HEAP_STAT_NEEDS_GC: u32 = 10;
/// the heap's tag (heap_tag.rs)
pub const HEAP_STAT_TAG: u32 = 11;

/// value_text's mode.
pub const TEXT_DEBUG: u32 = 0;
pub const TEXT_JSON: u32 = 1;

/// type_info's mode.
pub const TYPE_REGISTERED: u32 = 0;
pub const TYPE_DEFAULT_FOR_ID: u32 = 1;
pub const TYPE_OBJECT_ID: u32 = 2;
pub const TYPE_NAME_IN_CHAIN: u32 = 3;

/// What type_info answers (by its mode).
#[relax_api(plain)]
#[repr(C)]
pub struct TypeInfo {
    pub check: *const ScriptTypeCheck,
    pub object: ScriptObject,
    pub type_id: ScriptTypeId,
    pub name: LiveId,
    pub found: bool,
    /// TYPE_REGISTERED: the check has an object, and then its prototype and the name its
    /// `script_proto` registered it under (`name_str`; none: `name_ptr` null).
    pub has_object: bool,
    pub proto: ScriptValue,
    pub name_ptr: *const u8,
    pub name_len: usize,
}

impl TypeInfo {
    /// The registering type's name as written in its `script_proto`.
    #[inline]
    pub fn name_str(&self) -> Option<&'static str> {
        // SAFETY: the name is a `&'static str` of the registration
        (!self.name_ptr.is_null()).then(|| unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(self.name_ptr, self.name_len)) })
    }
}

#[relax_api(handle)]
#[derive(Default)]
pub struct ScriptHeap {
    pub modules: ScriptObject,
    pub(crate) gc_last: ScriptHeapGcLast,
    pub(crate) mark_vec: Vec<ScriptGcMark>,
    pub(crate) object_reuse_epoch: u64,

    /// root counts of the Rust-held refs (roots.rs)
    pub(crate) roots: crate::roots::ScriptRootsOwner,
    /// this heap's identity in every ref it mints (heap_tag.rs)
    pub(crate) heap_tag: crate::heap_tag::HeapTag,
    /// lazily expanded system modules (lazy_modules.rs): `mod` keys not yet made, by group
    pub(crate) lazy: crate::lazy_modules::LazyModules,

    pub(crate) type_defaults: HashMap<ScriptTypeIndex, ScriptObject>,

    // GenVec provides generation-checked access via Index<ScriptObject> etc.
    // Use index[obj] for checked access, index[i as usize] for unchecked iteration
    pub(crate) objects: GenVec<ScriptObjectData>,
    pub(crate) objects_free: Vec<ScriptObject>, // Stores refs with incremented generation, ready to reuse

    pub(crate) string_intern: HashMap<ScriptRcString, ScriptString>,
    pub(crate) strings_reuse: Vec<String>,
    pub(crate) strings: GenVec<Option<ScriptStringData>>,
    pub(crate) strings_free: Vec<ScriptString>,

    pub(crate) arrays: GenVec<ScriptArrayData>,
    pub(crate) arrays_free: Vec<ScriptArray>,

    pub(crate) pod_types: Vec<ScriptPodTypeData>,
    pub(crate) shader_types: Vec<std::cell::OnceCell<crate::shader_schema::ShaderTypeData>>,
    pub(crate) pod_types_free: Vec<ScriptPodType>,
    pub(crate) pods: GenVec<ScriptPodData>,
    pub(crate) pods_free: Vec<ScriptPod>,

    pub(crate) type_check: Vec<ScriptTypeCheck>,
    pub(crate) type_index: HashMap<ScriptTypeId, ScriptTypeIndex>,

    pub(crate) handles: GenVec<Option<ScriptHandleData>>,
    pub(crate) handles_free: Vec<ScriptHandle>,

    pub(crate) regex_intern: HashMap<RegexInternKey, ScriptRegex>,
    pub(crate) regexes: GenVec<Option<ScriptRegexData>>,
    pub(crate) regexes_free: Vec<ScriptRegex>,

    /// `None` is the trusted/default behavior. Sandboxed hosts install this
    /// only around one eval or tick, so widget/Studio VMs keep their existing
    /// unrestricted heap semantics.
    pub(crate) allocation_budget: Option<ScriptAllocationBudget>,
    /// Harmless immutable sentinels returned after the budget is exhausted.
    /// They keep parser/native construction paths memory-safe until the VM
    /// observes the pending hard-limit error and bails at the next opcode.
    pub(crate) allocation_poison_object: ScriptObject,
    pub(crate) allocation_poison_array: ScriptArray,

    /// Octoscript's aggregate retained-heap cap, expressed as a persistent
    /// allocation budget that outlives any one `with_heap_allocation_limit`
    /// scope. `None` preserves the inherited unrestricted VM behavior.
    pub(crate) heap_cap: Option<ScriptAllocationBudget>,
    pub(crate) max_heap_bytes: Option<usize>,
    /// Retained-capacity estimate taken at the last reconcile; live accounting
    /// adds the bytes charged to `heap_cap` since then.
    pub(crate) heap_baseline_bytes: usize,
    pub(crate) heap_limit_exceeded: bool,
    /// Per-string construction ceiling (logical bytes of one script string).
    pub(crate) max_string_bytes: Option<usize>,
    pub(crate) string_limit_exceeded: bool,
    /// A refusal raised by the per-string ceiling, surfaced through the same
    /// `take_allocation_error` poll the VM uses for budget refusals.
    pub(crate) pending_string_limit_error: Option<String>,
    /// Set whenever a refusal is recorded (scoped budget, retained cap or
    /// string ceiling). The interpreter asks `take_allocation_error` before
    /// every instruction, so the no-error case must be one flag read.
    pub(crate) allocation_error_pending: bool,
    /// Bounded document evaluation (`eval_bounded`): natives that build a
    /// string without a preflight build it and charge its length after,
    /// instead of being refused. Off for Octoscript, which refuses them.
    pub(crate) charge_native_strings_after: bool,
    /// The module registrations, while a collect run records them
    /// (makepad-script-census; `census`).
    pub census: Option<Box<dyn crate::census::ScriptCensus>>,
}

#[relax_api]
impl ScriptHeap {
    /// Whether `obj` names a live object of this heap.
    pub fn object_alive(&self, obj: ScriptObject) -> bool {
        let slots = self.object_slots();
        slots.is_valid(obj) && slots[obj].tag.is_alloced()
    }

    /// The draw shader objects reachable from `root` (fields holding an object with `vertex`
    /// and `fragment`/`pixel` methods), appended to `out`: shader prewarming's walk
    /// (platform shader_prewarm.rs), inside the VM so engine code reads no heap internals.
    pub fn draw_objects_into(&self, root: ScriptObject, sink: &mut ScriptObjectsOut) {
    let heap = self;
    let slots = heap.object_slots();
    let valid = |o: ScriptObject| slots.is_valid(o) && slots[o].tag.is_alloced();
    // Module scopes and preludes hold every name: walls.
    let mut seen: std::collections::HashSet<ScriptObject> = std::collections::HashSet::new();
    seen.insert(heap.modules);
    for (key, m) in slots[heap.modules].map.iter() {
        let Some(module) = m.value.as_object() else { continue };
        if slots[module].proto != *key {
            continue;
        }
        seen.insert(module);
        if key.as_id() == Some(id!(prelude)) {
            seen.extend(slots[module].map.iter().filter_map(|(_, v)| v.value.as_object()));
        }
    }
    // A draw object counts when a field holds it (`draw_bg: {..}` in a
    // template the UI reaches); one reached only as another's proto (a base
    // template's, a type default) is not what an instance draws with.
    let mut out = Vec::new();
    let mut candidates: std::collections::HashSet<ScriptObject> = std::collections::HashSet::new();
    let mut stack: Vec<(ScriptObject, bool)> = vec![(root, false)];
    let mut arrays: std::collections::HashSet<ScriptArray> = std::collections::HashSet::new();
    let mut values: Vec<ScriptValue> = Vec::new();
    while let Some((obj, by_field)) = stack.pop() {
        if !valid(obj) {
            continue;
        }
        if by_field && !candidates.contains(&obj) {
            let has = |name: LiveId| heap.object_method(obj, name.into(), crate::trap::NoTrap).as_object().is_some();
            if has(id!(vertex)) && (has(id!(fragment)) || has(id!(pixel))) {
                candidates.insert(obj);
                out.push(obj);
            }
        }
        if !seen.insert(obj) {
            continue;
        }
        // A function is code, not a UI definition: what it builds when it
        // runs (and the scope it closes over, which holds every name its
        // body imported) is left to the realtime path.
        if heap.as_fn(obj).is_some() {
            continue;
        }
        let object = &slots[obj];
        if let Some(p) = object.proto.as_object() {
            stack.push((p, false));
        }
        values.clear();
        values.extend(object.map.iter().map(|(_, e)| e.value));
        values.extend(object.vec.iter().map(|kv| kv.value));
        let mut i = 0;
        while i < values.len() {
            let v = values[i];
            i += 1;
            if let Some(o) = v.as_object() {
                stack.push((o, true));
            } else if let Some(a) = v.as_array() {
                if heap.array_slots().is_valid(a) && arrays.insert(a) {
                    for j in 0..heap.array_len(a) {
                        values.push(heap.array_index_unchecked(a, j));
                    }
                }
            }
        }
    }
    sink.push(&out);
}

    /// Create the refused-allocation sentinels before any budget becomes
    /// active. They are static and frozen, so GC retains them and refused
    /// writes are inert.
    fn ensure_allocation_poison(&mut self) {
        if self.allocation_poison_object == ScriptObject::ZERO {
            let object = self.new_object();
            self.objects[object].tag.set_static();
            self.objects[object].tag.freeze();
            self.allocation_poison_object = object;
        }
        if self.allocation_poison_array == ScriptArray::default() {
            let array = self.new_array();
            self.arrays[array].tag.set_static();
            self.arrays[array].tag.freeze();
            self.allocation_poison_array = array;
        }
    }

    pub(crate) fn begin_allocation_budget(
        &mut self,
        limit: usize,
    ) -> Option<ScriptAllocationBudget> {
        self.ensure_allocation_poison();
        self.allocation_budget
            .replace(ScriptAllocationBudget::new(limit))
    }

    pub(crate) fn end_allocation_budget(
        &mut self,
        previous: Option<ScriptAllocationBudget>,
    ) -> ScriptAllocationReport {
        let current = self.allocation_budget.take();
        self.allocation_budget = previous;
        current.map_or_else(ScriptAllocationReport::default, |budget| {
            ScriptAllocationReport {
                allocated_bytes: budget.allocated,
                exceeded: budget.exceeded,
            }
        })
    }

    /// Charge logical payload bytes before a script-controlled container
    /// grows. Both the scoped run budget and the persistent retained cap are
    /// charged; either one can refuse. Arithmetic is saturating so a hostile
    /// sparse index cannot wrap into a small allocation request.
    #[inline]
    pub(crate) fn charge_allocation(&mut self, bytes: usize, operation: &'static str) -> bool {
        self.heap_charge(HEAP_CHARGE_BYTES, ScriptObject::ZERO, NIL, bytes, operation)
    }

    /// Charges the heap's allocation budget by `mode`: HEAP_CHARGE_BYTES charges `bytes`;
    /// HEAP_CHARGE_MAP_ENTRY charges one new entry of `ptr`'s map for `key` (nothing when the key
    /// is there already). False: refused, nothing to allocate.
    #[inline]
    #[relax_api]
    pub(crate) fn heap_charge(&mut self, mode: u32, ptr: ScriptObject, key: ScriptValue, bytes: usize, operation: &'static str) -> bool {
        match mode {
            HEAP_CHARGE_BYTES => self.charge_allocation_impl(bytes, operation),
            _ => self.charge_object_map_entry_impl(ptr, key, operation),
        }
    }

    #[inline]
    fn charge_allocation_impl(&mut self, bytes: usize, operation: &'static str) -> bool {
        // Every container growth lands here; a VM with no budget and no cap
        // (every Makepad host) pays this one check.
        if self.allocation_budget.is_none() && self.heap_cap.is_none() {
            return true;
        }
        self.charge_limited_allocation(bytes, operation)
    }

    #[inline(never)]
    fn charge_limited_allocation(&mut self, bytes: usize, operation: &'static str) -> bool {
        if let Some(budget) = self.allocation_budget.as_mut() {
            if !budget.charge(bytes, operation, "allocation") {
                self.allocation_error_pending = true;
                return false;
            }
        }
        if let Some(cap) = self.heap_cap.as_mut() {
            if !cap.charge(bytes, operation, "heap allocation") {
                self.heap_limit_exceeded = true;
                self.allocation_error_pending = true;
                return false;
            }
        }
        true
    }

    #[inline]
    pub(crate) fn has_allocation_budget(&self) -> bool {
        self.allocation_budget.is_some() || self.heap_cap.is_some()
    }

    pub(crate) fn allocation_remaining(&self) -> usize {
        let scoped = self
            .allocation_budget
            .as_ref()
            .map_or(usize::MAX, |budget| budget.remaining);
        let cap = self
            .heap_cap
            .as_ref()
            .map_or(usize::MAX, |budget| budget.remaining);
        scoped.min(cap)
    }

    pub(crate) fn allocation_exceeded(&self) -> bool {
        self.allocation_budget
            .as_ref()
            .is_some_and(|budget| budget.exceeded)
            || self.heap_cap.as_ref().is_some_and(|cap| cap.exceeded)
    }

    /// The pending refusal the interpreter turns into an uncatchable bail
    /// before the next opcode: a scoped-budget refusal, a retained-cap
    /// refusal, or a per-string ceiling refusal, in that order.
    #[inline(always)]
    pub(crate) fn take_allocation_error(&mut self) -> Option<String> {
        if !self.allocation_error_pending {
            return None;
        }
        self.take_pending_allocation_error()
    }

    #[inline(never)]
    #[cold]
    fn take_pending_allocation_error(&mut self) -> Option<String> {
        let error = self.take_first_allocation_error();
        self.allocation_error_pending = self
            .allocation_budget
            .as_ref()
            .is_some_and(|budget| budget.error.is_some())
            || self.heap_cap.as_ref().is_some_and(|cap| cap.error.is_some())
            || self.pending_string_limit_error.is_some();
        error
    }

    fn take_first_allocation_error(&mut self) -> Option<String> {
        if let Some(error) = self
            .allocation_budget
            .as_mut()
            .and_then(|budget| budget.error.take())
        {
            return Some(error);
        }
        if let Some(error) = self.heap_cap.as_mut().and_then(|cap| cap.error.take()) {
            return Some(error);
        }
        self.pending_string_limit_error.take()
    }

    // Octoscript retained-heap cap.
    //
    // The inherited `with_heap_allocation_limit` budget is execution scoped:
    // it meters one eval or tick and is discarded afterwards. Octoscript's
    // runtime instead bounds the *retained* VM heap across evaluations. That
    // contract is expressed here as a persistent budget whose headroom is
    // `max_heap_bytes - estimated retained capacity`, re-derived by
    // `reconcile_heap_bytes` at host boundaries (setup, GC, before an eval)
    // and decremented by the same `charge_allocation` calls that feed the
    // scoped budget. Both budgets coexist: a scoped run budget nested inside
    // the cap is charged first, and the cap second.

    /// Applies an aggregate cap to the retained VM heap. `None` removes it and
    /// preserves the inherited Makepad VM behavior. The estimate tracks
    /// retained capacity of script strings, arrays, objects, slots, and intern
    /// tables; it intentionally cannot account for opaque host handles,
    /// adapter-owned Rust allocations, or the process allocator's metadata.
    pub fn set_max_heap_bytes(&mut self, maximum_bytes: Option<usize>) {
        self.max_heap_bytes = maximum_bytes;
        self.heap_limit_exceeded = false;
        if maximum_bytes.is_some() {
            self.ensure_allocation_poison();
        } else {
            self.heap_cap = None;
        }
        self.reconcile_heap_bytes();
    }

    /// Returns the configured aggregate retained-heap cap, if any.
    pub fn max_heap_bytes(&self) -> Option<usize> {
        self.max_heap_bytes
    }

    /// Returns the retained-capacity estimate at the last reconcile plus the
    /// logical bytes charged against the cap since. Without a cap this is the
    /// last reconciled estimate only.
    #[relax_api]
    pub fn accounted_heap_bytes(&self) -> usize {
        let charged = self.heap_cap.as_ref().map_or(0, |cap| cap.allocated);
        self.heap_baseline_bytes.saturating_add(charged)
    }

    /// Recomputes the retained-capacity estimate from the current heap state
    /// and re-arms the cap's headroom from it. Appropriate after trusted raw
    /// VM configuration, host-side value injection, or garbage collection.
    /// Normal script allocation paths update the cached amount in constant
    /// time instead of scanning the heap on every instruction.
    #[relax_api]
    pub fn reconcile_heap_bytes(&mut self) {
        self.heap_baseline_bytes = self.estimated_heap_bytes();
        let Some(maximum) = self.max_heap_bytes else {
            return;
        };
        let mut cap = ScriptAllocationBudget::new(maximum.saturating_sub(self.heap_baseline_bytes));
        if self.heap_baseline_bytes > maximum {
            self.heap_limit_exceeded = true;
            cap.exceeded = true;
        }
        self.heap_cap = Some(cap);
    }

    /// Recomputes accounting for uncommon allocation paths whose backing
    /// storage is owned by a lower-level helper.
    pub(crate) fn reconcile_heap_bytes_if_limited(&mut self) {
        if self.max_heap_bytes.is_some() {
            self.reconcile_heap_bytes();
        }
    }

    /// Returns and clears an aggregate heap-cap failure raised by a script
    /// allocation path. Clearing also re-opens the cap for later charges; a
    /// host that wants the refusal to persist re-checks `accounted_heap_bytes`
    /// against its limit before running more script.
    pub fn take_heap_limit_exceeded(&mut self) -> bool {
        let exceeded = std::mem::take(&mut self.heap_limit_exceeded);
        if let Some(cap) = self.heap_cap.as_mut() {
            cap.exceeded = false;
            cap.error = None;
        }
        exceeded
    }

    fn estimated_heap_bytes(&self) -> usize {
        fn bytes_for<T>(capacity: usize) -> usize {
            capacity.saturating_mul(size_of::<T>())
        }

        fn map_bytes<K, V, S>(map: &HashMap<K, V, S>) -> usize {
            // HashMap keeps bucket control data in addition to key/value
            // payload. Charge two machine words plus one control byte per
            // usable bucket to avoid treating map capacity as free.
            let entry_bytes = size_of::<(K, V)>()
                .saturating_add(size_of::<usize>().saturating_mul(2))
                .saturating_add(1);
            map.capacity().saturating_mul(entry_bytes)
        }

        let mut bytes = size_of::<Self>();

        bytes = bytes.saturating_add(bytes_for::<ScriptGcMark>(self.mark_vec.capacity()));
        bytes = bytes.saturating_add(self.roots.get().bytes());
        bytes = bytes.saturating_add(map_bytes(&self.type_defaults));

        bytes = bytes.saturating_add(bytes_for::<GenSlot<ScriptObjectData>>(
            self.objects.capacity(),
        ));
        bytes = bytes.saturating_add(bytes_for::<ScriptObject>(self.objects_free.capacity()));
        for object in self.objects.iter() {
            bytes = bytes.saturating_add(object.retained_bytes());
        }

        bytes = bytes.saturating_add(map_bytes(&self.string_intern));
        bytes = bytes.saturating_add(bytes_for::<String>(self.strings_reuse.capacity()));
        for string in &self.strings_reuse {
            bytes = bytes.saturating_add(string.capacity());
        }
        bytes = bytes.saturating_add(bytes_for::<GenSlot<Option<ScriptStringData>>>(
            self.strings.capacity(),
        ));
        bytes = bytes.saturating_add(bytes_for::<ScriptString>(self.strings_free.capacity()));
        for string in self.strings.iter().flatten() {
            bytes = bytes.saturating_add(string.string.0.capacity());
        }

        bytes = bytes.saturating_add(bytes_for::<GenSlot<ScriptArrayData>>(
            self.arrays.capacity(),
        ));
        bytes = bytes.saturating_add(bytes_for::<ScriptArray>(self.arrays_free.capacity()));
        for array in self.arrays.iter() {
            bytes = bytes.saturating_add(array.storage.retained_bytes());
        }

        bytes = bytes.saturating_add(bytes_for::<std::cell::OnceCell<crate::shader_schema::ShaderTypeData>>(self.shader_types.capacity()));
        bytes = bytes.saturating_add(bytes_for::<ScriptPodTypeData>(self.pod_types.capacity()));
        bytes = bytes.saturating_add(bytes_for::<ScriptPodType>(self.pod_types_free.capacity()));
        bytes = bytes.saturating_add(bytes_for::<GenSlot<ScriptPodData>>(self.pods.capacity()));
        bytes = bytes.saturating_add(bytes_for::<ScriptPod>(self.pods_free.capacity()));
        for pod in self.pods.iter() {
            bytes = bytes.saturating_add(bytes_for::<u32>(pod.data.capacity()));
        }

        bytes = bytes.saturating_add(bytes_for::<ScriptTypeCheck>(self.type_check.capacity()));
        bytes = bytes.saturating_add(map_bytes(&self.type_index));

        // Handle payloads are intentionally opaque Rust-owned allocations;
        // count their slot bookkeeping but not the adapter's object graph.
        bytes = bytes.saturating_add(bytes_for::<GenSlot<Option<ScriptHandleData>>>(
            self.handles.capacity(),
        ));
        bytes = bytes.saturating_add(bytes_for::<ScriptHandle>(self.handles_free.capacity()));

        bytes = bytes.saturating_add(map_bytes(&self.regex_intern));
        bytes = bytes.saturating_add(bytes_for::<GenSlot<Option<ScriptRegexData>>>(
            self.regexes.capacity(),
        ));
        bytes = bytes.saturating_add(bytes_for::<ScriptRegex>(self.regexes_free.capacity()));
        for regex in self.regexes.iter().flatten() {
            // The compiled regex engine is opaque, but the pattern's retained
            // bytes and all VM-level indexes are charged here.
            bytes = bytes.saturating_add(regex.pattern.capacity());
        }
        for key in self.regex_intern.keys() {
            bytes = bytes.saturating_add(key.pattern.capacity());
        }

        bytes
    }

    #[inline]
    pub(crate) fn is_allocation_poison_object(&self, object: ScriptObject) -> bool {
        object == self.allocation_poison_object && object != ScriptObject::ZERO
    }

    #[inline]
    pub(crate) fn is_allocation_poison_array(&self, array: ScriptArray) -> bool {
        array == self.allocation_poison_array && array != ScriptArray::default()
    }

    /// A stable, unique identity for this heap for its lifetime, matching
    /// [`ScriptObjectRef::heap_key`] for every ref minted here. Used to map a
    /// heap to its owning script VM so a widget's objects are always resolved
    /// against the heap that created them.
    #[inline]
    pub fn heap_key(&self) -> usize {
        self.heap_stat(HEAP_STAT_KEY) as usize
    }

    /// Cap on the number of cleared `String` buffers kept around for reuse. Each one holds a
    /// heap allocation; a big script burst can stash thousands of them in `strings_reuse`,
    /// which would otherwise never be released.
    const MAX_STRINGS_REUSE: usize = 1024;

    /// Release memory the heap is holding purely for reuse / over-allocation, called after a
    /// GC sweep. This is safe because it never removes or moves any live slot — it only:
    ///   - drops excess pooled-for-reuse `String` buffers beyond a cap (re-allocated lazily),
    ///   - returns over-allocated spare capacity in the slot arrays and free lists.
    /// The slot arrays keep their high-water `len` (slots are reused via the free lists), so
    /// every existing index / reference stays valid.
    pub fn shrink_to_fit(&mut self) {
        if self.strings_reuse.len() > Self::MAX_STRINGS_REUSE {
            self.strings_reuse.truncate(Self::MAX_STRINGS_REUSE);
        }
        self.strings_reuse.shrink_to_fit();

        self.objects.shrink_to_fit();
        self.strings.shrink_to_fit();
        self.arrays.shrink_to_fit();
        self.pods.shrink_to_fit();
        self.handles.shrink_to_fit();
        self.regexes.shrink_to_fit();

        self.objects_free.shrink_to_fit();
        self.strings_free.shrink_to_fit();
        self.arrays_free.shrink_to_fit();
        self.pods_free.shrink_to_fit();
        self.pod_types_free.shrink_to_fit();
        self.handles_free.shrink_to_fit();
        self.regexes_free.shrink_to_fit();
        self.reconcile_heap_bytes_if_limited();
    }

    /// This heap's tag, the high byte of every generation it mints (heap_tag.rs).
    pub fn heap_tag(&self) -> u8 {
        self.heap_stat(HEAP_STAT_TAG) as u8
    }

    pub fn empty() -> Self {
        let mut objects = GenVec::new();
        let mut arrays = GenVec::new();
        let mut pods = GenVec::new();
        let mut handles = GenVec::new();
        let mut strings = GenVec::new();
        let mut regexes = GenVec::new();
        let heap_tag = crate::heap_tag::HeapTag::alloc();
        objects.set_heap_tag(heap_tag.get());
        arrays.set_heap_tag(heap_tag.get());
        pods.set_heap_tag(heap_tag.get());
        handles.set_heap_tag(heap_tag.get());
        strings.set_heap_tag(heap_tag.get());
        regexes.set_heap_tag(heap_tag.get());

        // Push slot 0 for each (reserved/null slot)
        objects.push(Default::default());
        arrays.push(Default::default());
        pods.push(Default::default());
        handles.push(None);
        strings.push(None); // slot 0 for strings too
        regexes.push(None); // slot 0 for regexes too

        let mut v = Self {
            modules: ScriptObject::ZERO,
            objects,
            arrays,
            pods,
            handles,
            strings,
            regexes,
            heap_tag,
            ..Default::default()
        };
        // Initialize slot 0 (reserved null slot) - use get_at_mut for internal init
        v.objects.get_at_mut(0).tag.set_alloced();
        v.objects.get_at_mut(0).tag.set_static();
        v.objects.get_at_mut(0).tag.freeze();
        v.arrays.get_at_mut(0).tag.set_alloced();
        v.arrays.get_at_mut(0).tag.freeze();

        v.modules = v.new_with_proto(id!(mod).into());
        v.roots.add_static(crate::roots::ROOT_OBJECT, v.modules.index);

        v
    }

    #[inline]
    pub fn registered_type(&self, id: ScriptTypeId) -> Option<&ScriptTypeCheck> {
        let r = self.type_info(TYPE_REGISTERED, id, ScriptObject::ZERO);
        // SAFETY: the check is a slot of this heap's type table, borrowed from `self`
        unsafe { r.check.as_ref() }
    }

    /// Reads the type table by `mode`: TYPE_REGISTERED the check registered for `id` (`check`);
    /// TYPE_DEFAULT_FOR_ID the default object of the type `id` (`object`, `found`); TYPE_OBJECT_ID
    /// the id of the type of `obj` (`type_id`, `found`); TYPE_NAME_IN_CHAIN the registered name of
    /// `obj` or of its nearest typed prototype (`name`, `found`).
    #[relax_api]
    pub fn type_info(&self, mode: u32, id: ScriptTypeId, obj: ScriptObject) -> TypeInfo {
        let mut r = TypeInfo { check: std::ptr::null(), object: ScriptObject::ZERO, type_id: id, name: LiveId(0), found: false, has_object: false, proto: NIL, name_ptr: std::ptr::null(), name_len: 0 };
        match mode {
            TYPE_REGISTERED => {
                if let Some(index) = self.type_index.get(&id) {
                    let check = &self.type_check[index.0 as usize];
                    r.check = check;
                    r.found = true;
                    if let Some(object) = &check.object {
                        r.has_object = true;
                        r.proto = object.proto;
                        if let Some(n) = object.name_str {
                            r.name_ptr = n.as_ptr();
                            r.name_len = n.len();
                        }
                    }
                }
            }
            TYPE_DEFAULT_FOR_ID => {
                let obj = self.type_index.get(&id).and_then(|ty_index| self.type_defaults.get(ty_index).copied());
                self.census_default_used(obj);
                if let Some(obj) = obj {
                    r.object = obj;
                    r.found = true;
                }
            }
            TYPE_OBJECT_ID => {
                if let Some(ti) = self.objects[obj].tag.as_type_index() {
                    if let Some(object) = &self.type_check[ti.0 as usize].object {
                        r.type_id = object.type_id;
                        r.found = true;
                    }
                }
            }
            _ => {
                let mut cur = Some(obj);
                while let Some(obj) = cur {
                    if let Some(type_id) = self.object_type_id(obj) {
                        if let Some(name) = self.type_name_by_id(type_id) {
                            r.name = name;
                            r.found = true;
                            break;
                        }
                    }
                    cur = self.proto(obj).as_object();
                }
            }
        }
        r
    }

    /// A type registered again -- every module run registers every
    /// scriptable type, so a reload does this for all of them -- takes the
    /// slot it had. A slot's proto is a collector root, and so is the
    /// default filed under the slot's index; a fresh slot per run kept every
    /// previous run's template tree alive for the life of the process, some
    /// thousands of objects a reload, and nothing ever let them go.
    ///
    /// A type with an id takes its slot back by the id. One without --
    /// the derive registers an enum's named variants that way -- takes the
    /// slot of any earlier registration with the same props: those carry no
    /// object, so nothing about them differs between runs but the slot.
    #[relax_api]
    pub fn register_type(
        &mut self,
        type_id: Option<ScriptTypeId>,
        ty_check: ScriptTypeCheck,
    ) -> ScriptTypeIndex {
        if let Some(type_id) = type_id {
            if let Some(index) = self.type_index.get(&type_id).copied() {
                // a re-registration (reload) of the same type takes its slot back; another
                // type with the same id is a collision
                if let (Some(have), Some(new)) = (&self.type_check[index.0 as usize].object, &ty_check.object) {
                    if !crate::traits::same_type_name(have.name_str, new.name_str) {
                        crate::traits::script_type_id_collision(type_id, have.name_str, new.name_str);
                    }
                }
                self.type_check[index.0 as usize] = ty_check;
                return index;
            }
        } else if ty_check.object.is_none() {
            let same = self.type_check.iter().position(|have| {
                have.object.is_none()
                    && have.is_repr_u32_enum == ty_check.is_repr_u32_enum
                    && have.props.rust_instance_start == ty_check.props.rust_instance_start
                    && have.props.props.len() == ty_check.props.props.len()
                    && have.props.iter_ordered().eq(ty_check.props.iter_ordered())
            });
            if let Some(index) = same {
                return ScriptTypeIndex(index as _);
            }
        }
        let index = ScriptTypeIndex(self.type_check.len() as _);
        if let Some(type_id) = type_id {
            self.type_index.insert(type_id, index);
        }
        self.type_check.push(ty_check);
        index
    }

    /// How many type slots exist. A test counts these across module runs.
    pub fn registered_type_count(&self) -> usize {
        self.type_check.len()
    }

    /// The objects every type slot roots: each slot's proto, and the
    /// default filed under it. A leak hunt walks what hangs off them.
    #[inline(always)]
    pub fn type_root_ids(&self) -> (Vec<ScriptObject>, Vec<ScriptObject>) {
        unsafe extern "C-unwind" fn push(ctx: *mut u8, default: bool, object: ScriptObject) {
            let (protos, defaults) = &mut *(ctx as *mut (Vec<ScriptObject>, Vec<ScriptObject>));
            if default { defaults.push(object) } else { protos.push(object) }
        }
        let mut out = (Vec::new(), Vec::new());
        // SAFETY: the callback writes only to out during this synchronous call.
        unsafe { self.type_root_ids_with(&mut out as *mut _ as *mut u8, push); }
        out
    }

    /// [`Self::type_root_ids`] through `push`: `default` false for a type
    /// proto, true for a type default.
    ///
    /// # Safety
    /// `ctx` must remain valid for `push` throughout this synchronous walk.
    #[inline]
    pub unsafe fn type_root_ids_with(&self, ctx: *mut u8, push: unsafe extern "C-unwind" fn(*mut u8, bool, ScriptObject)) {
        self.heap_walk(HEAP_WALK_TYPE_ROOTS, ctx, push);
    }

    /// Walks object ids of the heap by `kind` (HEAP_WALK_*) through `push(ctx, default, object)`:
    /// the type slots' protos (`default` false) and defaults (true), every allocated object, or
    /// every object a Rust-held reference roots (`default` false for the last two).
    ///
    /// # Safety
    /// `ctx` must remain valid for `push` throughout this synchronous walk.
    #[relax_api]
    pub unsafe fn heap_walk(&self, kind: u32, ctx: *mut u8, push: unsafe extern "C-unwind" fn(*mut u8, bool, ScriptObject)) {
        match kind {
            HEAP_WALK_TYPE_ROOTS => {
                for check in self.type_check.iter() {
                    if let Some(proto) = check.object.as_ref().and_then(|object| object.proto.as_object()) {
                        push(ctx, false, proto);
                    }
                }
                for object in self.type_defaults.values() {
                    push(ctx, true, *object);
                }
            }
            HEAP_WALK_ALLOCED => {
                for i in 1..self.objects.len() {
                    if self.objects.get_at(i).tag.is_alloced() {
                        push(ctx, false, ScriptObject::new(i as u32, self.objects.generation(i)));
                    }
                }
            }
            HEAP_WALK_ROOTED => {
                let roots = self.roots.get();
                for i in roots.rooted_slots(crate::roots::ROOT_OBJECT) {
                    push(ctx, false, ScriptObject::new(i, self.objects.generation(i as usize)));
                }
            }
            _ => unreachable!("heap_walk kind"),
        }
    }

    /// Every allocated object slot, as a handle a reader can use. A leak
    /// hunt walks these after a collection to see what survived, by kind.
    #[inline(always)]
    pub fn alloced_object_ids(&self) -> Vec<ScriptObject> {
        unsafe extern "C-unwind" fn push(ctx: *mut u8, object: ScriptObject) {
            (&mut *(ctx as *mut Vec<ScriptObject>)).push(object);
        }
        let mut out = Vec::new();
        // SAFETY: the callback writes only to out during this synchronous call.
        unsafe { self.alloced_object_ids_with(&mut out as *mut _ as *mut u8, push); }
        out
    }

    /// # Safety
    /// `ctx` must remain valid for `push` throughout this synchronous walk.
    #[inline]
    pub unsafe fn alloced_object_ids_with(&self, ctx: *mut u8, push: unsafe extern "C-unwind" fn(*mut u8, ScriptObject)) {
        let mut walk = ObjectWalk { ctx, push };
        self.heap_walk(HEAP_WALK_ALLOCED, &mut walk as *mut ObjectWalk as *mut u8, object_walk_push);
    }

    /// Every object a Rust-held reference is keeping alive. A leak hunt
    /// diffs two readings to see what was newly pinned.
    #[inline(always)]
    pub fn root_object_ids(&self) -> Vec<ScriptObject> {
        unsafe extern "C-unwind" fn push(ctx: *mut u8, object: ScriptObject) {
            (&mut *(ctx as *mut Vec<ScriptObject>)).push(object);
        }
        let mut out = Vec::new();
        // SAFETY: the callback writes only to out during this synchronous call.
        unsafe { self.root_object_ids_with(&mut out as *mut _ as *mut u8, push); }
        out
    }

    /// # Safety
    /// `ctx` must remain valid for `push` throughout this synchronous root walk.
    #[inline]
    pub unsafe fn root_object_ids_with(&self, ctx: *mut u8, push: unsafe extern "C-unwind" fn(*mut u8, ScriptObject)) {
        let mut walk = ObjectWalk { ctx, push };
        self.heap_walk(HEAP_WALK_ROOTED, &mut walk as *mut ObjectWalk as *mut u8, object_walk_push);
    }

    /// The size of every root the collector marks from, in the order it
    /// marks them: type slots, type defaults, pod types, Rust-held object
    /// refs, Rust-held array refs, Rust-held handles. Two readings apart in
    /// time say which root a leak hangs off.
    pub fn root_counts(&self) -> [usize; 6] {
        [
            self.heap_stat(HEAP_STAT_TYPE_SLOTS) as usize,
            self.heap_stat(HEAP_STAT_TYPE_DEFAULTS) as usize,
            self.heap_stat(HEAP_STAT_POD_TYPES) as usize,
            self.heap_stat(HEAP_STAT_ROOTED_OBJECTS) as usize,
            self.heap_stat(HEAP_STAT_ROOTED_ARRAYS) as usize,
            self.heap_stat(HEAP_STAT_ROOTED_HANDLES) as usize,
        ]
    }

    /// Counts of the heap, by `which` (HEAP_STAT_*).
    #[inline]
    #[relax_api]
    pub fn heap_stat(&self, which: u32) -> u64 {
        match which {
            HEAP_STAT_KEY => self.roots.ptr() as usize as u64,
            HEAP_STAT_TYPE_SLOTS => self.type_check.len() as u64,
            HEAP_STAT_TYPE_DEFAULTS => self.type_defaults.len() as u64,
            HEAP_STAT_POD_TYPES => self.pod_types.len() as u64,
            HEAP_STAT_ROOTED_OBJECTS => self.roots.get().rooted_len(crate::roots::ROOT_OBJECT) as u64,
            HEAP_STAT_ROOTED_ARRAYS => self.roots.get().rooted_len(crate::roots::ROOT_ARRAY) as u64,
            HEAP_STAT_ROOTED_HANDLES => self.roots.get().rooted_len(crate::roots::ROOT_HANDLE) as u64,
            HEAP_STAT_OBJECTS_LEN => self.objects.len() as u64,
            HEAP_STAT_REUSE_EPOCH => self.object_reuse_epoch,
            HEAP_STAT_LIVE_LEN => self.gc_live_len_impl() as u64,
            HEAP_STAT_NEEDS_GC => self.needs_gc_impl() as u64,
            HEAP_STAT_TAG => self.heap_tag.get() as u64,
            _ => unreachable!("heap_stat"),
        }
    }

    #[inline]
    pub fn type_matches_id(&self, ptr: ScriptObject, type_id: ScriptTypeId) -> bool {
        self.object_type_id(ptr) == Some(type_id)
    }

    /// Returns the TypeId for an object if it has a registered type.
    #[inline]
    pub fn object_type_id(&self, ptr: ScriptObject) -> Option<ScriptTypeId> {
        let r = self.type_info(TYPE_OBJECT_ID, ScriptTypeId::from_u128(0), ptr);
        r.found.then_some(r.type_id)
    }

    /// Returns the registered script name for a given TypeId, if any.
    pub fn type_name_by_id(&self, type_id: ScriptTypeId) -> Option<LiveId> {
        if let Some(index) = self.type_index.get(&type_id) {
            if let Some(object) = &self.type_check[index.0 as usize].object {
                return object.name;
            }
        }
        None
    }

    /// Best-effort human name for an object: the registered Rust type name
    /// of the object or of its nearest typed prototype. Used for
    /// diagnostics — e.g. naming a draw shader in compile-error reports.
    #[inline]
    pub fn object_type_name_in_chain(&self, ptr: ScriptObject) -> Option<LiveId> {
        let r = self.type_info(TYPE_NAME_IN_CHAIN, ScriptTypeId::from_u128(0), ptr);
        r.found.then_some(r.name)
    }


    // Accessors

    pub fn has_proto(&mut self, ptr: ScriptObject, rhs: ScriptValue) -> bool {
        let mut ptr = ptr;
        loop {
            let object = &mut self.objects[ptr];
            if object.proto == rhs {
                return true;
            }
            if let Some(object) = object.proto.as_object() {
                ptr = object
            } else {
                return false;
            }
        }
    }

    #[inline]
    pub fn proto(&self, ptr: ScriptObject) -> ScriptValue {
        self.obj_info(ptr).proto
    }

    pub fn root_proto(&self, ptr: ScriptObject) -> ScriptValue {
        let mut ptr = ptr;
        loop {
            let proto = self.obj_info(ptr).proto;
            if let Some(next_ptr) = proto.as_object() {
                ptr = next_ptr
            } else {
                return proto;
            }
        }
    }

    /// Monotonic counter bumped when object slots are freed/reused.
    /// Used by higher layers to evict caches keyed by ScriptObject identity.
    #[inline]
    pub fn object_reuse_epoch(&self) -> u64 {
        self.heap_stat(HEAP_STAT_REUSE_EPOCH)
    }

    #[inline]
    pub(crate) fn bump_object_reuse_epoch(&mut self) {
        self.object_reuse_epoch = self.object_reuse_epoch.wrapping_add(1);
    }

    #[inline]
    pub fn type_check(&self, index: ScriptTypeIndex) -> &ScriptTypeCheck {
        &self.type_check[index.0 as usize]
    }

    #[inline]
    pub fn set_type_default(&mut self, obj: ScriptObject) -> bool {
        self.obj_mode(obj, OBJ_MODE_TYPE_DEFAULT, 0) != 0
    }

    pub fn type_default(&self, ty_index: ScriptTypeIndex) -> Option<ScriptObject> {
        let obj = self.type_defaults.get(&ty_index).copied();
        self.census_default_used(obj);
        obj
    }

    #[inline]
    pub fn type_default_for_id(&self, type_id: ScriptTypeId) -> Option<ScriptObject> {
        let r = self.type_info(TYPE_DEFAULT_FOR_ID, type_id, ScriptObject::ZERO);
        r.found.then_some(r.object)
    }

    /// A Rust type made from its registered default while a census records:
    /// a use of the module that registered it (`census`).
    #[inline]
    fn census_default_used(&self, obj: Option<ScriptObject>) {
        if let (Some(census), Some(obj)) = (&self.census, obj) {
            census.default_used(obj);
        }
    }

    /// Every object slot, for heap walkers outside the VM (the module
    /// census): `is_valid`, `get_at`, `generation`, indexing.
    pub fn object_slots(&self) -> &GenVec<ScriptObjectData> {
        &self.objects
    }

    /// Every array slot, for heap walkers outside the VM.
    pub fn array_slots(&self) -> &GenVec<ScriptArrayData> {
        &self.arrays
    }

    /// The objects Rust holds (rooted by a `ScriptObjectRef`).
    pub fn rooted_objects(&self) -> Vec<ScriptObject> {
        self.root_object_ids()
    }

    /// The registered type defaults.
    pub fn type_default_objects(&self) -> Vec<ScriptObject> {
        unsafe extern "C-unwind" fn push(ctx: *mut u8, object: ScriptObject) {
            (&mut *(ctx as *mut Vec<ScriptObject>)).push(object);
        }
        let mut out = Vec::new();
        // SAFETY: the callback writes only to out during this synchronous call.
        unsafe { self.type_default_objects_with(&mut out as *mut _ as *mut u8, push); }
        out
    }

    /// # Safety
    /// `ctx` must remain valid for `push` throughout this synchronous walk.
    pub unsafe fn type_default_objects_with(&self, ctx: *mut u8, push: unsafe extern "C-unwind" fn(*mut u8, ScriptObject)) {
        for object in self.type_defaults.values() { push(ctx, *object); }
    }

    /// Look up a field's ScriptTypeId from the type-check structure of an object.
    /// This is used when the field value isn't on the prototype but the type is registered.
    pub fn field_type_from_type_check(
        &self,
        obj: ScriptObject,
        field_id: LiveId,
    ) -> Option<ScriptTypeId> {
        let object = &self.objects[obj];
        if let Some(ty_index) = object.tag.as_type_index() {
            let type_check = &self.type_check[ty_index.0 as usize];
            if let Some(prop) = type_check.props.props.get(&field_id) {
                return Some(prop.ty);
            }
        }
        // Also check the prototype chain
        if let Some(proto_obj) = object.proto.as_object() {
            return self.field_type_from_type_check(proto_obj, field_id);
        }
        None
    }

    #[inline]
    #[relax_api]
    pub fn cast_to_f64(&self, v: ScriptValue, ip: ScriptIp) -> f64 {
        if let Some(v) = v.as_f64() {
            return v;
        }
        if let Some(v) = v.as_u40() {
            return v as _;
        }
        // Numbers are the hot case (every arithmetic opcode lands here) and
        // stay inlinable; everything else converts out of line.
        self.cast_non_number_to_f64(v, ip)
    }

    #[inline(never)]
    fn cast_non_number_to_f64(&self, v: ScriptValue, ip: ScriptIp) -> f64 {
        // Inline and heap strings convert alike; text that is not a number
        // is NaN rather than 0, so `"abc" * 2` cannot masquerade as zero.
        if let Some(number) = self.string_with(v, |_, text| text.parse::<f64>()) {
            return number.unwrap_or_else(|_| {
                ScriptValue::from_f64_traced_nan(f64::NAN, ip)
                    .as_f64()
                    .unwrap()
            });
        }
        if let Some(v) = v.as_bool() {
            return if v { 1.0 } else { 0.0 };
        }
        if let Some(v) = v.as_f32() {
            return v as f64;
        }
        if let Some(v) = v.as_f16() {
            return v as f64;
        }
        if let Some(v) = v.as_u32() {
            return v as f64;
        }
        if let Some(v) = v.as_i32() {
            return v as f64;
        }
        if let Some(v) = v.as_color() {
            return v as f64;
        }
        if v.is_nil() {
            return 0.0;
        }
        if let Some(v) = self.pod_scalar_number(v) {
            return v;
        }
        ScriptValue::from_f64_traced_nan(f64::NAN, ip)
            .as_f64()
            .unwrap()
    }

    pub fn cast_to_bool(&self, v: ScriptValue) -> bool {
        if let Some(b) = v.as_bool() {
            return b;
        }
        if v.is_nil() {
            return false;
        }
        if let Some(v) = v.as_f64() {
            return v != 0.0;
        }
        if let Some(v) = v.as_u40() {
            return v != 0;
        }
        if let Some(v) = v.as_f32() {
            return v != 0.0;
        }
        if let Some(v) = v.as_f16() {
            return v != 0.0;
        }
        if let Some(v) = v.as_u32() {
            return v != 0;
        }
        if let Some(v) = v.as_i32() {
            return v != 0;
        }
        if let Some(_v) = v.as_object() {
            return true;
        }
        if v.inline_string_not_empty() {
            return true;
        }
        if let Some(v) = v.as_string() {
            return self.string(v).len() != 0;
        }
        if let Some(_v) = v.as_id() {
            return true;
        }
        if let Some(_v) = v.as_color() {
            return true;
        }
        if v.is_opcode() {
            return true;
        }
        false
    }

    // Debug and utility

    pub fn println(&self, value: ScriptValue) {
        let mut out = String::new();
        let mut recur = Vec::new();
        self.to_debug_string(value, &mut recur, &mut out, true, 0);
        println!("{out}");
    }

    pub fn to_debug_string(&self, value: ScriptValue, recur: &mut Vec<ScriptValue>, out: &mut String, formatted: bool, depth: usize) {
        self.debug_string_into(value, recur, &mut crate::vm_api::ScriptTextOut::new(out), formatted, depth);
    }

    /// Format inside the owning compiler. Recursion guards are unchanged on return.
    #[inline]
    pub fn debug_string_into(&self, value: ScriptValue, recur: &[ScriptValue], out: &mut crate::vm_api::ScriptTextOut, formatted: bool, depth: usize) {
        self.value_text(TEXT_DEBUG, value, recur, formatted, depth, out);
    }

    /// Writes `value` as text into `out` by `mode`: TEXT_DEBUG its debug format (`recur` the
    /// recursion guards, unchanged on return; `formatted`, `depth` as to_debug_string), TEXT_JSON
    /// its JSON.
    #[relax_api]
    pub fn value_text(&self, mode: u32, value: ScriptValue, recur: &[ScriptValue], formatted: bool, depth: usize, out: &mut crate::vm_api::ScriptTextOut) {
        let mut text = String::new();
        if mode == TEXT_DEBUG {
            self.to_debug_string_native(value, &mut recur.to_vec(), &mut text, formatted, depth);
        } else {
            self.to_json_depth(value, &mut text, 0);
        }
        out.push(&text);
    }

    fn to_debug_string_native(
        &self,
        value: ScriptValue,
        recur: &mut Vec<ScriptValue>,
        out: &mut String,
        formatted: bool,
        depth: usize,
    ) {
        fn write_indent(out: &mut String, depth: usize) {
            for _ in 0..depth {
                out.push_str("- - ");
            }
        }

        fn write_separator(out: &mut String, formatted: bool, depth: usize, first: bool) {
            if !first {
                if formatted {
                    out.push_str(",\n");
                    write_indent(out, depth);
                } else {
                    out.push_str(", ");
                }
            }
        }

        if let Some(obj) = value.as_object() {
            if self.is_fn(obj) {
                write!(out, "<fn {}>", obj.index()).ok();
                return;
            }
            if recur.iter().any(|v| *v == value) {
                write!(out, "<recur>").ok();
                return;
            }
            recur.push(value);

            let object = &self.objects[obj];
            if object.tag.is_script_fn() {
                write!(out, "Fn").ok();
            } else if object.tag.is_native_fn() {
                write!(out, "Native").ok();
            }
            let mut ptr = obj;
            // scan up the chain to set the proto value
            write!(out, "<{}>{{", obj.index()).ok();

            // Check if object has any content (for formatted output)
            let has_content = {
                let obj_data = &self.objects[obj];
                obj_data.map_len() > 0
                    || !obj_data.vec.is_empty()
                    || obj_data.tag.as_type_index().is_some()
            };

            if formatted && has_content {
                out.push('\n');
                write_indent(out, depth + 1);
            }

            let mut first = true;

            // if we have a type index, output type checked base properties first
            if let Some(ty_index) = object.tag.as_type_index() {
                write!(out, "<type ").ok();
                let type_check = &self.type_check[ty_index.0 as usize];
                for (prop_id, _prop_ty) in type_check.props.iter_ordered() {
                    if !first {
                        write!(out, ", ").ok();
                    }
                    write!(out, "{}", prop_id).ok();
                    first = false;
                }
                write!(out, ">").ok();
                if formatted {
                    out.push('\n');
                    write_indent(out, depth + 1);
                }
                first = true;
            }

            loop {
                let object = &self.objects[ptr];

                object.map_iter_ordered(|key, value| {
                    write_separator(out, formatted, depth + 1, first);
                    if key != NIL {
                        self.to_debug_string_native(key, recur, out, formatted, depth + 1);
                        write!(out, ": ").ok();
                    }
                    self.to_debug_string_native(value, recur, out, formatted, depth + 1);
                    first = false;
                });
                for kv in object.vec.iter() {
                    write_separator(out, formatted, depth + 1, first);
                    if kv.key != NIL {
                        write!(out, "{}: ", kv.key).ok();
                    }
                    self.to_debug_string_native(kv.value, recur, out, formatted, depth + 1);
                    first = false;
                }
                if let Some(next_ptr) = object.proto.as_object() {
                    if formatted {
                        if !first {
                            out.push_str(",\n");
                            write_indent(out, depth + 1);
                        }
                        write!(out, "^<{}>", next_ptr.index()).ok();
                    } else {
                        if !first {
                            write!(out, ",").ok();
                        }
                        write!(out, "^<{}>", next_ptr.index()).ok();
                    }
                    ptr = next_ptr
                } else {
                    if formatted && has_content {
                        out.push('\n');
                        write_indent(out, depth);
                    }
                    write!(out, "/{}", object.proto).ok();
                    break;
                }
            }
            write!(out, "}}").ok();
            recur.pop();
        } else if let Some(arr) = value.as_array() {
            if recur.iter().any(|v| *v == value) {
                write!(out, "<recur>").ok();
                return;
            }
            recur.push(value);
            let array = &self.arrays[arr];
            let len = array.storage.len();
            write!(out, "<{}>[", arr.index()).ok();

            if formatted && len > 0 {
                out.push('\n');
                write_indent(out, depth + 1);
            }

            for i in 0..len {
                if i != 0 {
                    if formatted {
                        out.push_str(",\n");
                        write_indent(out, depth + 1);
                    } else {
                        out.push_str(", ");
                    }
                }
                self.to_debug_string_native(
                    array.storage.index(i).unwrap(),
                    recur,
                    out,
                    formatted,
                    depth + 1,
                );
            }

            if formatted && len > 0 {
                out.push('\n');
                write_indent(out, depth);
            }

            write!(out, "]").ok();
            recur.pop();
        } else if let Some(s) = value.as_string() {
            let s = if let Some(s) = &self.strings[s] {
                &s.string.0
            } else {
                ""
            };
            write!(out, "\"").ok();
            write!(out, "{}", s).ok();
            write!(out, "\"").ok();
        } else if value
            .as_inline_string(|s| {
                write!(out, "\"").ok();
                write!(out, "{}", s).ok();
                write!(out, "\"").ok();
            })
            .is_some()
        {
        } else if let Some(pod) = value.as_pod() {
            let pod = &self.pods[pod];
            let pod_type = &self.pod_types[pod.ty.index as usize];
            self.pod_debug(out, pod_type, 0, &pod.data);
        } else if let Some(handle_box) = value.as_handle().and_then(|h| self.handle_box_ref(h)) {
            // The host's own text for its handle (`ScriptHandleGc::debug_fmt`).
            write!(out, "{:?}", handle_box).ok();
        } else {
            write!(out, "{}", value).ok();
        }
    }

    #[relax_api]
    pub fn to_json(&mut self, value: ScriptValue) -> ScriptValue {
        if !self.has_allocation_budget() {
            return self.new_string_with(|heap, s| {
                heap.to_json_inner(value, s);
            });
        }
        let cap = self.allocation_remaining();
        let Some(len) = self.to_json_len_bounded(value, &mut Vec::new(), cap) else {
            let _ = self.charge_allocation(usize::MAX, "serializing JSON");
            return NIL;
        };
        self.new_string_with_preflight(len, "serializing JSON", |heap, out| {
            heap.to_json_inner(value, out);
        })
    }

    fn to_json_len_bounded(
        &self,
        value: ScriptValue,
        recur: &mut Vec<ScriptValue>,
        cap: usize,
    ) -> Option<usize> {
        fn add(len: &mut usize, additional: usize, cap: usize) -> Option<()> {
            *len = len.checked_add(additional)?;
            (*len <= cap).then_some(())
        }

        fn escaped_len(value: &str, cap: usize) -> Option<usize> {
            let mut len = 0usize;
            for ch in value.chars() {
                let additional = match ch {
                    '\x08' | '\x0c' | '\n' | '\r' | '"' | '\\' => 2,
                    ch => ch.len_utf8(),
                };
                add(&mut len, additional, cap)?;
            }
            Some(len)
        }

        if recur.len() >= 128 || recur.contains(&value) {
            return None;
        }
        if let Some(obj) = value.as_object() {
            recur.push(value);
            let mut len = 1usize; // {
            let mut first = true;
            let mut ptr = obj;
            let mut proto_seen = Vec::new();
            loop {
                if proto_seen.contains(&ptr) {
                    return None;
                }
                proto_seen.push(ptr);
                let object = &self.objects[ptr];
                for (key, map_value) in object.map.iter() {
                    if !first {
                        add(&mut len, 1, cap)?;
                    }
                    let key_len = self.to_json_len_bounded(*key, recur, cap - len)?;
                    add(&mut len, key_len, cap)?;
                    add(&mut len, 1, cap)?; // :
                    let value_len =
                        self.to_json_len_bounded(map_value.value, recur, cap - len)?;
                    add(&mut len, value_len, cap)?;
                    first = false;
                }
                for item in &object.vec {
                    if !first {
                        add(&mut len, 1, cap)?;
                    }
                    let key_len = self.to_json_len_bounded(item.key, recur, cap - len)?;
                    add(&mut len, key_len, cap)?;
                    add(&mut len, 1, cap)?;
                    let value_len = self.to_json_len_bounded(item.value, recur, cap - len)?;
                    add(&mut len, value_len, cap)?;
                    first = false;
                }
                if let Some(next) = object.proto.as_object() {
                    ptr = next;
                } else {
                    break;
                }
            }
            add(&mut len, 1, cap)?; // }
            recur.pop();
            return Some(len);
        }
        if let Some(array) = value.as_array() {
            recur.push(value);
            let storage = &self.arrays[array].storage;
            let mut len = 1usize; // [
            let mut first = true;
            for index in 0..storage.len() {
                if let Some(item) = storage.index(index) {
                    if !first {
                        add(&mut len, 1, cap)?;
                    }
                    let item_len = self.to_json_len_bounded(item, recur, cap - len)?;
                    add(&mut len, item_len, cap)?;
                    first = false;
                }
            }
            add(&mut len, 1, cap)?; // ]
            recur.pop();
            return Some(len);
        }
        if let Some(id) = value.as_id() {
            let inner = id.as_string(|value| {
                value.and_then(|value| escaped_len(value, cap.saturating_sub(2)))
            })?;
            return inner.checked_add(2).filter(|len| *len <= cap);
        }
        if let Some(string) = value.as_string() {
            let inner = escaped_len(self.string(string), cap.saturating_sub(2))?;
            return inner.checked_add(2).filter(|len| *len <= cap);
        }
        if let Some(inner) = value.as_inline_string(|string| {
            escaped_len(string, cap.saturating_sub(2))
        }) {
            return inner?.checked_add(2).filter(|len| *len <= cap);
        }
        if let Some(value) = value.as_bool() {
            return Some(if value { 4 } else { 5 }).filter(|len| *len <= cap);
        }
        if let Some(value) = value.as_number() {
            let len = format!("{}", value).len();
            return (len <= cap).then_some(len);
        }
        if let Some(value) = value.as_handle() {
            let len = format!("Handle{:?}", value).len();
            return (len <= cap).then_some(len);
        }
        (4 <= cap).then_some(4) // null
    }

    pub fn to_json_inner(&self, value: ScriptValue, out: &mut String) {
        self.json_into(value, &mut crate::vm_api::ScriptTextOut::new(out));
    }

    #[inline]
    pub fn json_into(&self, value: ScriptValue, out: &mut crate::vm_api::ScriptTextOut) {
        self.value_text(TEXT_JSON, value, &[], false, 0, out);
    }

    fn to_json_depth(&self, value: ScriptValue, out: &mut String, depth: usize) {
        // A cyclic object/array graph recurses forever here (scripts can
        // build one, and hosts serialize script-supplied values); a depth cap
        // turns both cycles and absurd nesting into a null leaf instead of a
        // host stack overflow.
        const MAX_JSON_DEPTH: usize = 24;
        if depth > MAX_JSON_DEPTH {
            out.push_str("null");
            return;
        }
        fn escape_str(inp: &str, out: &mut String) {
            for c in inp.chars() {
                match c {
                    '\x08' => out.push_str("\\b"),
                    '\x0c' => out.push_str("\\f"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    c if (c as u32) < 0x20 => {
                        write!(out, "\\u{:04x}", c as u32).ok();
                    }
                    c => {
                        out.push(c);
                    }
                }
            }
        }
        if let Some(obj) = value.as_object() {
            let mut ptr = obj;
            // scan up the chain to set the proto value
            out.push('{');
            let mut first = true;
            loop {
                let object = &self.objects[ptr];
                object.map_iter(|key, value| {
                    if !first {
                        out.push(',')
                    }
                    self.to_json_depth(key, out, depth + 1);
                    out.push(':');
                    self.to_json_depth(value, out, depth + 1);
                    first = false;
                });
                for kv in object.vec.iter() {
                    if !first {
                        out.push(',')
                    }
                    first = false;
                    self.to_json_depth(kv.key, out, depth + 1);
                    out.push(':');
                    self.to_json_depth(kv.value, out, depth + 1);
                }
                if let Some(next_ptr) = object.proto.as_object() {
                    ptr = next_ptr
                } else {
                    break;
                }
            }
            out.push('}');
        } else if let Some(arr) = value.as_array() {
            let array = &self.arrays[arr];
            let len = array.storage.len();
            let mut first = true;
            out.push('[');
            for i in 0..len {
                if let Some(value) = array.storage.index(i) {
                    if !first {
                        out.push(',')
                    }
                    first = false;
                    self.to_json_depth(value, out, depth + 1);
                }
            }
            out.push(']');
        } else if let Some(id) = value.as_id() {
            out.push('"');
            id.as_string(|s| {
                if let Some(s) = s {
                    escape_str(s, out);
                }
            });
            out.push('"');
            // alright. sself is json eh. so.
        } else if let Some(s) = value.as_string() {
            let s = if let Some(s) = &self.strings[s] {
                &s.string.0
            } else {
                ""
            };
            out.push('"');
            escape_str(s, out);
            out.push('"');
        } else if value
            .as_inline_string(|s| {
                out.push('"');
                escape_str(s, out);
                out.push('"');
            })
            .is_some()
        {
        } else if let Some(v) = value.as_bool() {
            if v {
                out.push_str("true")
            } else {
                out.push_str("false")
            }
        } else if let Some(v) = value.as_number() {
            write!(out, "{}", v).ok();
        } else if let Some(v) = value.as_handle() {
            // A handle has no JSON form; null is honest and stays parseable.
            let _ = v;
            out.push_str("null");
        } else {
            out.push_str("null");
        }
    }

    // memory  usage
    #[inline]
    pub fn objects_len(&self) -> usize {
        self.heap_stat(HEAP_STAT_OBJECTS_LEN) as usize
    }

    /// Checks if a value has an apply transform without calling it.
    /// Used by type_check to be permissive when a transform exists.
    #[inline]
    pub fn has_apply_transform(&self, value: ScriptValue) -> bool {
        if let Some(obj) = value.as_object() {
            return self.obj_info(obj).tag.as_apply_transform().is_some();
        }
        if let Some(arr) = value.as_array() {
            return self.arr_view(arr).tag.as_apply_transform().is_some();
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trap::*;
    use crate::vm::{ScriptMod, ScriptVm, ScriptVmBase, ScriptVmHost};

    fn eval(vm: &mut ScriptVm, file: &str, code: &str) -> ScriptValue {
        vm.eval(ScriptMod {
            file: file.to_owned(),
            code: format!("{code}\n;"),
            ..Default::default()
        })
    }

    fn string_of(vm: &ScriptVm, value: ScriptValue) -> Option<String> {
        vm.bx.heap.string_with(value, |_, text| text.to_owned())
    }

    #[test]
    fn numeric_string_conversion_handles_inline_and_heap_strings_and_yields_nan() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = ScriptVm {
            host: host.host_context(),
            bx: Box::new(ScriptVmBase::new()),
        };
        let ip = ScriptIp::default();
        let inline = vm.bx.heap.new_string_from_str("12.5");
        assert!(inline.as_inline_string(|_| ()).is_some());
        assert_eq!(vm.bx.heap.cast_to_f64(inline, ip), 12.5);

        let heap = vm.bx.heap.new_string_from_str("1234567890123456.5");
        assert!(heap.as_string().is_some());
        assert_eq!(vm.bx.heap.cast_to_f64(heap, ip), 1234567890123456.5);

        for text in ["abc", "", "12abc", "a very long string that is not a number"] {
            let value = vm.bx.heap.new_string_from_str(text);
            assert!(vm.bx.heap.cast_to_f64(value, ip).is_nan(), "{text:?} must be NaN");
        }
    }

    #[test]
    fn array_opcodes_reject_invalid_indexes_before_touching_storage() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = ScriptVm {
            host: host.host_context(),
            bx: Box::new(ScriptVmBase::new()),
        };
        vm.bx.captured_errors = Some(Vec::new());
        let array = vm.bx.heap.new_array();
        for item in [1.0, 2.0, 3.0] {
            vm.bx
                .heap
                .array_push(array, ScriptValue::from_f64(item), ScriptTrap::NoTrap);
        }
        vm.set_injected_global(id!(values), array.into());

        for (file, code) in [
            ("neg-write", "values[-1] = 9"),
            ("frac-write", "values[1.5] = 9"),
            ("nan-write", "values[0 / 0] = 9"),
            ("inf-write", "values[1 / 0] = 9"),
            ("huge-write", "values[1e30] = 9"),
            ("string-write", "values[\"1\"] = 9"),
            ("nil-write", "values[nil] = 9"),
            ("neg-compound", "values[-1] += 9"),
            ("neg-read", "values[-1]"),
            ("frac-read", "values[0.5]"),
            ("string-read", "values[\"0\"]"),
            ("nan-read", "values[0 / 0]"),
        ] {
            let result = eval(&mut vm, &format!("{file}.octoscript"), code);
            assert!(result.is_err(), "{file}: {code} must error, got {result:?}");
            let _ = vm.take_errors();
            let storage = vm.bx.heap.array_storage(array);
            assert_eq!(storage.len(), 3, "{file}: storage length changed");
            let items: Vec<_> = (0..3)
                .map(|index| storage.index(index).and_then(|value| value.as_f64()))
                .collect();
            assert_eq!(items, vec![Some(1.0), Some(2.0), Some(3.0)], "{file}");
        }

        // Valid integral indexes, including float-typed integers, still work.
        let result = eval(&mut vm, "valid-write.octoscript", "values[3.0] = 4\nvalues[3]");
        assert_eq!(result.as_number(), Some(4.0), "{result:?} {:?}", vm.take_errors());
        assert_eq!(vm.bx.heap.array_len(array), 4);
    }

    #[test]
    fn updating_an_untracked_field_keeps_its_insertion_order() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = ScriptVm {
            host: host.host_context(),
            bx: Box::new(ScriptVmBase::new()),
        };
        let result = eval(
            &mut vm,
            "insertion-order.octoscript",
            "let o = {a: 1, b: 2, c: 3}\no.a = 10\no.b = 20\no.to_json()",
        );
        assert_eq!(
            string_of(&vm, result).as_deref(),
            Some(r#"{"a":10,"b":20,"c":3}"#)
        );

        // The raw storage path: re-inserting an existing key keeps its slot.
        let mut object = ScriptObjectData::default();
        assert!(!object.tag.is_tracked());
        object.map_insert(id!(a).into(), ScriptValue::from_f64(1.0));
        object.map_insert(id!(b).into(), ScriptValue::from_f64(2.0));
        object.map_insert(id!(a).into(), ScriptValue::from_f64(3.0));
        let mut keys = Vec::new();
        object.map_iter_ordered(|key, value| keys.push((key, value)));
        assert_eq!(
            keys,
            vec![
                (id!(a).into(), ScriptValue::from_f64(3.0)),
                (id!(b).into(), ScriptValue::from_f64(2.0)),
            ]
        );
    }

    #[test]
    fn sparse_array_growth_past_the_heap_cap_bails_without_touching_storage() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = ScriptVm {
            host: host.host_context(),
            bx: Box::new(ScriptVmBase::new()),
        };
        vm.bx.captured_errors = Some(Vec::new());
        let array = vm.bx.heap.new_array();
        vm.set_injected_global(id!(values), array.into());

        vm.bx.heap.reconcile_heap_bytes();
        let baseline = vm.bx.heap.accounted_heap_bytes();
        vm.bx.heap.set_max_heap_bytes(Some(baseline + 256 * 1024));
        assert_eq!(vm.bx.heap.accounted_heap_bytes(), baseline);

        let result = eval(
            &mut vm,
            "heap-limit.octoscript",
            "try { values[268435456] = 1 } { \"ok\" }",
        );
        assert!(result.is_err(), "the cap refusal is uncatchable: {result:?}");
        assert!(vm.bx.threads.cur_ref().trap.err_is_empty());
        assert!(vm
            .take_errors()
            .iter()
            .any(|diagnostic| diagnostic.contains("script heap allocation limit exceeded")));
        assert_eq!(vm.bx.heap.array_len(array), 0);
        assert!(vm.bx.heap.take_heap_limit_exceeded());
        assert!(!vm.bx.heap.take_heap_limit_exceeded());

        // Once the flag is taken the cap re-opens for ordinary work.
        let result = eval(&mut vm, "after-cap.octoscript", "values[2] = 1\nvalues.len()");
        assert_eq!(result.as_number(), Some(3.0));
        assert!(vm.bx.heap.accounted_heap_bytes() > baseline);
        assert!(vm.bx.heap.accounted_heap_bytes() <= baseline + 256 * 1024);
    }

    #[test]
    fn heap_cap_accounting_reconciles_and_reports_an_overfull_heap() {
        let mut heap = ScriptHeap::empty();
        heap.reconcile_heap_bytes();
        let baseline = heap.accounted_heap_bytes();
        assert!(baseline > 0);
        assert_eq!(heap.max_heap_bytes(), None);

        // An already-overfull heap reports immediately and refuses growth.
        heap.set_max_heap_bytes(Some(baseline - 1));
        assert!(heap.take_heap_limit_exceeded());
        assert_eq!(heap.accounted_heap_bytes(), baseline);

        // Removing the cap restores the unrestricted VM.
        heap.set_max_heap_bytes(None);
        assert!(!heap.has_allocation_budget());
        assert!(!heap.take_heap_limit_exceeded());

        // Charges accumulate on top of the reconciled baseline until the next
        // reconcile re-derives the estimate from the actual retained state.
        heap.set_max_heap_bytes(Some(usize::MAX));
        let baseline = heap.accounted_heap_bytes();
        let value = heap.new_string_from_str("a string long enough to live on the heap");
        assert!(value.as_string().is_some());
        assert!(heap.accounted_heap_bytes() > baseline);
        heap.reconcile_heap_bytes();
        assert!(heap.accounted_heap_bytes() > baseline);
        assert!(!heap.take_heap_limit_exceeded());
    }

    #[test]
    fn scoped_allocation_budgets_nest_inside_the_retained_cap() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = ScriptVm {
            host: host.host_context(),
            bx: Box::new(ScriptVmBase::new()),
        };
        vm.bx.captured_errors = Some(Vec::new());
        vm.bx.heap.set_max_heap_bytes(Some(usize::MAX));
        let before = vm.bx.heap.accounted_heap_bytes();
        let (_, report) = vm.with_heap_allocation_limit(64, |vm| {
            eval(vm, "scoped.octoscript", "let values = []\nvalues[4096] = 1")
        });
        assert!(report.exceeded);
        let _ = vm.take_errors();
        // The scoped refusal is not a retained-cap refusal.
        assert!(!vm.bx.heap.take_heap_limit_exceeded());
        assert_eq!(vm.bx.heap.accounted_heap_bytes(), before);
    }

    #[test]
    fn string_limit_bails_instead_of_entering_a_try_fallback() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = ScriptVm {
            host: host.host_context(),
            bx: Box::new(ScriptVmBase::new()),
        };
        vm.bx.heap.set_max_string_bytes(Some(8));
        vm.bx.captured_errors = Some(Vec::new());

        let result = eval(
            &mut vm,
            "string-limit.octoscript",
            "let payload = \"x\"\n\
             let index = 0\n\
             while (index < 3) {\n\
                 payload += payload\n\
                 index += 1\n\
             }\n\
             try { payload + payload } { \"ok\" }",
        );

        assert!(result.is_err());
        assert!(vm.bx.threads.cur_ref().trap.err_is_empty());
        assert!(vm
            .take_errors()
            .iter()
            .any(|diagnostic| diagnostic.contains("script string allocation limit exceeded")));
        assert!(vm.bx.heap.take_string_limit_exceeded());
        assert!(!vm.bx.heap.take_string_limit_exceeded());
    }

    #[test]
    fn byte_array_string_conversion_stops_at_the_string_limit() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = ScriptVm {
            host: host.host_context(),
            bx: Box::new(ScriptVmBase::new()),
        };
        vm.bx.heap.set_max_string_bytes(Some(8));
        vm.bx.captured_errors = Some(Vec::new());

        let bytes = vm.bx.heap.new_array_from_vec_u8(vec![0xFF; 16]);
        vm.set_injected_global(id!(bytes), bytes.into());

        let result = eval(
            &mut vm,
            "byte-array-string-limit.octoscript",
            "try { bytes.to_string() } { \"ok\" }",
        );

        assert!(result.is_err());
        assert!(vm
            .take_errors()
            .iter()
            .any(|diagnostic| diagnostic.contains("script string allocation limit exceeded")));
    }

    #[test]
    fn byte_array_string_conversion_preserves_lossy_utf8_without_a_limit() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = ScriptVm {
            host: host.host_context(),
            bx: Box::new(ScriptVmBase::new()),
        };

        let bytes = vm.bx.heap.new_array_from_vec_u8(vec![b'a', 0xFF, b'b']);
        vm.set_injected_global(id!(bytes), bytes.into());

        let result = eval(&mut vm, "byte-array-lossy-utf8.octoscript", "bytes.to_string()");
        assert_eq!(string_of(&vm, result), Some("a\u{FFFD}b".to_owned()));
    }

    #[test]
    fn bounded_string_helpers_preserve_their_normal_results() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = ScriptVm {
            host: host.host_context(),
            bx: Box::new(ScriptVmBase::new()),
        };
        vm.bx.heap.set_max_string_bytes(Some(64));
        vm.bx.heap.set_max_heap_bytes(Some(usize::MAX));

        for (file, code, expected) in [
            (
                "string-replace.octoscript",
                "\"abcd\".replace(\"b\", \"XX\")",
                "aXXcd",
            ),
            (
                "string-url-encode.octoscript",
                "\"a b!\".url_encode()",
                "a%20b%21",
            ),
            (
                "string-url-decode.octoscript",
                "\"a%20b%21\".url_decode()",
                "a b!",
            ),
            (
                "string-concat.octoscript",
                "\"hello \" + \"world, this is a heap string\"",
                "hello world, this is a heap string",
            ),
            (
                "json-roundtrip.octoscript",
                "{a: [1, 2, \"three\"], b: {c: true}}.to_json()",
                r#"{"a":[1,2,"three"],"b":{"c":true}}"#,
            ),
        ] {
            let result = eval(&mut vm, file, code);
            assert_eq!(string_of(&vm, result), Some(expected.to_owned()), "{file}");
        }
        assert!(!vm.bx.heap.take_string_limit_exceeded());
        assert!(!vm.bx.heap.take_heap_limit_exceeded());
    }

    #[test]
    fn byte_array_json_input_stops_at_the_string_limit() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = ScriptVm {
            host: host.host_context(),
            bx: Box::new(ScriptVmBase::new()),
        };
        vm.bx.heap.set_max_string_bytes(Some(8));
        vm.bx.captured_errors = Some(Vec::new());

        let bytes = vm
            .bx
            .heap
            .new_array_from_vec_u8(br#"{"value": 12345}"#.to_vec());
        vm.set_injected_global(id!(bytes), bytes.into());

        let result = eval(
            &mut vm,
            "byte-array-json-limit.octoscript",
            "try { bytes.parse_json() } { \"ok\" }",
        );

        assert!(result.is_err());
        assert!(vm
            .take_errors()
            .iter()
            .any(|diagnostic| diagnostic.contains("script string allocation limit exceeded")));
    }

    #[test]
    fn byte_array_json_input_parses_within_the_string_limit() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = ScriptVm {
            host: host.host_context(),
            bx: Box::new(ScriptVmBase::new()),
        };
        vm.bx.heap.set_max_string_bytes(Some(64));

        let bytes = vm
            .bx
            .heap
            .new_array_from_vec_u8(br#"{"value": 12345}"#.to_vec());
        vm.set_injected_global(id!(bytes), bytes.into());

        let result = eval(&mut vm, "byte-array-json.octoscript", "bytes.parse_json().value");
        assert_eq!(result.as_number(), Some(12345.0));
        assert!(!vm.bx.heap.take_string_limit_exceeded());
    }
}

impl ScriptHeap {
    pub fn object_type_index(&self, obj: ScriptObject) -> Option<crate::traits::ScriptTypeIndex> { self.objects[obj].tag.as_type_index() }
}
