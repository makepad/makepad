use relax_api::relax_api;
use crate::array::*;
use crate::gc::*;
use crate::gen_index::{GenSlot, GenVec, SlotRefusal};
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
#[relax_api(plain, run)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScriptAllocationReport {
    /// Logical heap bytes charged while the budget was installed.
    pub allocated_bytes: usize,
    /// True when an allocation was refused before growing its container.
    pub exceeded: bool,
}

/// The refusal reason when a heap table already holds as many values as a
/// value's index can name (16.7 M per kind): a limit of the representation,
/// reported like a refused allocation.
pub const TOO_MANY_LIVE_VALUES: &str = "too many live values";

const COLLECTING_GARBAGE: &str = "out of memory while collecting garbage";
pub(crate) const NAMING_A_PROPERTY: &str = "out of memory while naming a property";
pub(crate) const STARTING_A_LOOP: &str = "out of memory while starting a loop";
pub(crate) const RAN_LOW_ON_MEMORY: &str = "out of memory: the system refused an allocation";

/// Capacity for an entire walk, irrespective of the vector's current length.
pub(crate) fn reserve_to<T>(v: &mut Vec<T>, required: usize) -> bool {
    required <= v.capacity() || v.try_reserve(required - v.len()).is_ok()
}

fn size_marks(marks: &mut Vec<u64>, slots: usize) -> bool {
    let words = slots.div_ceil(64);
    if words > marks.len() {
        if !reserve_to(marks, words) { return false; }
        marks.resize(words, 0);
    }
    true
}

// One room check for the next slot: its bitmap bit and its share of the common worklist.
#[inline]
fn reserve_trace_slot(work: &mut Vec<u32>, marks: &mut Vec<u64>, slots: usize, other: usize,
    reason: &'static str) -> Result<(), &'static str> {
    let covered = (marks.len() * 64).min(work.capacity().saturating_sub(other));
    if slots < covered || (reserve_to(work, slots + 1 + other) && size_marks(marks, slots + 1)) {
        Ok(())
    } else {
        Err(reason)
    }
}

const STORING_A_STRING: &str = "out of memory while storing a string";

/// Room for one more slot in `table`, and for `free` to hold every slot of
/// the grown table, so that sweep and unwind never grow a free list.
#[inline]
fn reserve_table_slot<T, R>(table: &mut GenVec<T>, free: &mut Vec<R>, reason: &'static str) -> Result<(), &'static str> {
    match table.try_reserve_slot() {
        Ok(()) => {}
        Err(SlotRefusal::Full) => return Err(TOO_MANY_LIVE_VALUES),
        Err(SlotRefusal::Memory) => return Err(reason),
    }
    let slots = table.len() + 1;
    if free.capacity() < slots && free.try_reserve(slots - free.len()).is_err() {
        return Err(reason);
    }
    Ok(())
}

/// Grow `free` so it can hold every slot of a table about to reach `slots`,
/// for the tables whose new slots are not guarded (pods, pod types, handles,
/// regexes: a document cannot make one of their requests large). Their sweep
/// then never grows a free list either.
#[inline]
pub(crate) fn reserve_free_list<R>(free: &mut Vec<R>, slots: usize) {
    if free.capacity() < slots {
        free.reserve(slots - free.len());
    }
}

/// The first physical allocation refusal in an evaluation. The reason is static
/// so the host can read it even while the allocator is still refusing requests.
#[relax_api(plain, run)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScriptMemoryRefusal {
    pub reason: &'static str,
    pub ip: ScriptIp,
}

pub(crate) enum AllocationError {
    Memory,
    Limit(String),
}

#[relax_api(handle, run)]
#[derive(Clone, Debug)]
pub(crate) struct ScriptAllocationBudget {
    remaining: usize,
    allocated: usize,
    exceeded: bool,
    /// First charge that did not fit, used to settle an enclosing scope.
    refused_bytes: usize,
    error: Option<String>,
}

impl ScriptAllocationBudget {
    fn new(limit: usize) -> Self {
        Self {
            remaining: limit,
            allocated: 0,
            exceeded: false,
            refused_bytes: 0,
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
            self.refused_bytes = bytes;
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
pub const HEAP_STAT_ALLOCATION_ERROR_PENDING: u32 = 12;

/// value_text's mode.
pub const TEXT_DEBUG: u32 = 0;
pub const TEXT_JSON: u32 = 1;

/// type_info's mode.
pub const TYPE_REGISTERED: u32 = 0;
pub const TYPE_DEFAULT_FOR_ID: u32 = 1;
pub const TYPE_OBJECT_ID: u32 = 2;
pub const TYPE_NAME_IN_CHAIN: u32 = 3;
pub const TYPE_NAME_BY_ID: u32 = 4;

/// What type_info answers (by its mode).
#[relax_api(plain, run)]
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

#[relax_api(handle, run, default)]
#[derive(Default)]
pub struct ScriptHeap {
    pub modules: ScriptObject,
    pub(crate) gc_last: ScriptHeapGcLast,
    pub(crate) mark_vec: Vec<u32>,
    pub(crate) object_marks: Vec<u64>,
    pub(crate) array_marks: Vec<u64>,
    pub(crate) gc_phase: GcPhase,
    pub(crate) transient_bodies: Vec<bool>,
    pub(crate) live_bodies: Vec<bool>,
    pub(crate) newly_live_bodies: Vec<usize>,
    pub(crate) object_reuse_epoch: u64,
    /// A guest under an entry policy asked for a collection (`gc.run`); the host collects on its
    /// own schedule and clears this ([`Self::take_gc_request`]).
    pub(crate) gc_requested: bool,

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
    /// The argument types of each derived enum's tuple variants, by enum type and variant
    /// (`script_derive_enum_tuple_args`): what lets `typed_value` build `Size.Fixed(200)` from
    /// data, each argument against its own type, without calling the variant's constructor.
    pub(crate) tuple_arg_types: HashMap<(ScriptTypeId, LiveId), Box<[ScriptTypeId]>>,
    /// The element type recorded when a generic `Vec<T>` builds its prototype.
    vec_element_types: HashMap<ScriptTypeId, ScriptTypeId>,
    /// The pod type of each Rust type (`ScriptNew::script_pod`). A Rust type's layout is fixed
    /// for the process, so it has one pod type, static (never collected). A module's
    /// `#(T::script_pod(vm))` then gives the same value on every run: a changed `#(..)` value
    /// parses the module again with new content keys for its functions, and a design re-run
    /// compiled every shader of the module again (13 of a large app's on each edit, 11-04).
    pub(crate) rust_pod_types: HashMap<ScriptTypeId, ScriptPodType>,
    /// Handles their host interned under a key (`intern_handle`): one per key, static.
    pub(crate) interned_handles: HashMap<u64, ScriptHandle>,
    /// Whether a name is a module's private key (`__forms`, `__tag`: it starts with `__`), by id.
    /// A name's text never changes, and a glob `use` asks for every key it brings in: the
    /// interner's lookup per key was 7% of a design re-run's main thread (a large app, 11-04).
    pub(crate) private_names: HashMap<LiveId, bool, LiveIdHasherBuilder>,
    /// Each scope's glob `use` sources in use order, kept outside its visible bindings.
    pub(crate) glob_sources: HashMap<ScriptObject, Vec<ScriptObject>>,

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
    /// Set whenever a limit or physical refusal is recorded.
    /// The interpreter asks `take_allocation_error` before
    /// every instruction, so the no-error case must be one flag read.
    pub(crate) allocation_error_pending: bool,
    memory_refusal: Option<ScriptMemoryRefusal>,
    memory_refusal_pending: bool,
    /// Includes host call and eval setup, direct natives, and nested interpreters.
    vm_entry_depth: u32,
    /// Bounded document evaluation (`eval_bounded`): natives that build a
    /// string without a preflight build it and charge its length after,
    /// instead of being refused. Off for Octoscript, which refuses them.
    pub(crate) charge_native_strings_after: bool,
    /// The module registrations, while a collect run records them
    /// (makepad-script-census; `census`).
    pub census: Option<Box<dyn crate::census::ScriptCensus>>,
    /// The definitions a run reads, while a trace records them (touch.rs).
    pub(crate) touch: Option<Box<crate::touch::ScriptTouch>>,
}

// Report a refusal no interpreter owns without asking for memory or taking a std lock:
// on macOS the first lock of std::io::stderr() allocates its mutex, which fails here.
#[cfg(unix)]
#[cold]
fn report_refusal(reason: &str) {
    unsafe extern "C" {
        fn write(fd: std::ffi::c_int, buf: *const std::ffi::c_void, count: usize) -> isize;
    }
    let write_all = |mut bytes: &[u8]| {
        while !bytes.is_empty() {
            let n = unsafe { write(2, bytes.as_ptr().cast(), bytes.len()) };
            if n > 0 {
                bytes = &bytes[n as usize..];
            } else if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            } else {
                break;
            }
        }
    };
    // One write when it fits, so concurrent reports do not interleave.
    let mut line = [0u8; 256];
    let bytes = reason.as_bytes();
    if bytes.len() < line.len() {
        line[..bytes.len()].copy_from_slice(bytes);
        line[bytes.len()] = b'\n';
        write_all(&line[..bytes.len() + 1]);
    } else {
        write_all(bytes);
        write_all(b"\n");
    }
}

#[cfg(not(unix))]
#[cold]
fn report_refusal(reason: &str) {
    use std::io::Write;
    let mut stderr = std::io::stderr().lock();
    let _ = stderr.write_all(reason.as_bytes());
    let _ = stderr.write_all(b"\n");
}

#[relax_api(run)]
impl ScriptHeap {
    /// How many arguments a derived enum's tuple variant takes, when the derive recorded its
    /// argument types (`typed_value` builds each from data). An op: the table is rustc's.
    #[relax_api(run)]
    pub fn tuple_arg_count(&self, enum_ty: ScriptTypeId, variant: LiveId) -> Option<usize> {
        self.tuple_arg_types.get(&(enum_ty, variant)).map(|args| args.len())
    }

    /// The recorded type of argument `index` of a derived enum's tuple variant.
    #[relax_api(run)]
    pub fn tuple_arg_type(&self, enum_ty: ScriptTypeId, variant: LiveId, index: usize) -> Option<ScriptTypeId> {
        self.tuple_arg_types.get(&(enum_ty, variant)).and_then(|args| args.get(index)).copied()
    }

    /// Returns true on first registration, before the vector registers its element's prototype.
    #[relax_api(run)]
    pub fn register_vec_element_type(&mut self, vec_ty: ScriptTypeId, element_ty: ScriptTypeId) -> bool {
        self.vec_element_types.insert(vec_ty, element_ty).is_none()
    }

    #[relax_api(run)]
    pub fn vec_element_type(&self, vec_ty: ScriptTypeId) -> Option<ScriptTypeId> {
        self.vec_element_types.get(&vec_ty).copied()
    }

    /// Declare metadata accepted by a registered component without making it a
    /// child in the object's ordered vector. The metadata has no Rust field. An op of the
    /// provided crate: JIT'd widget registration calls it, never reads the heap's fields.
    #[relax_api(run)]
    pub fn define_type_property(&mut self, object: ScriptObject, name: LiveId, ty: ScriptTypeId) {
        let Some(index) = self.object_type_index(object) else { return };
        let props = &mut self.type_check[index.0 as usize].props;
        if !props.props.contains_key(&name) { props.insert(name, ty); }
    }

    /// Whether `obj` names a live object of this heap.
    pub fn object_alive(&self, obj: ScriptObject) -> bool {
        let slots = self.object_slots();
        slots.is_valid(obj) && slots[obj].tag.is_alloced()
    }

    /// The shader compiler's test for a draw object: `vertex` with `fragment` or `pixel`.
    pub(crate) fn is_draw_object(&self, obj: ScriptObject) -> bool {
        let has = |name: LiveId| self.object_method(obj, name.into(), crate::trap::NoTrap).as_object().is_some();
        has(id!(vertex)) && (has(id!(fragment)) || has(id!(pixel)))
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
        if by_field && !candidates.contains(&obj) && heap.is_draw_object(obj) {
            candidates.insert(obj);
            out.push(obj);
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
            self.objects[object].tag.set_reffed();
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
        // Only the innermost counter is hot. Its headroom already includes
        // every ancestor, and its accepted charges transfer back on exit.
        let mut child = ScriptAllocationBudget::new(limit);
        if let Some(parent) = &self.allocation_budget {
            child.remaining = child.remaining.min(parent.remaining);
            child.exceeded = parent.exceeded;
        }
        self.allocation_budget.replace(child)
    }

    pub(crate) fn end_allocation_budget(
        &mut self,
        previous: Option<ScriptAllocationBudget>,
    ) -> ScriptAllocationReport {
        let current = self.allocation_budget.take();
        self.allocation_budget = previous;
        current.map_or_else(ScriptAllocationReport::default, |mut budget| {
            if let Some(parent) = &mut self.allocation_budget {
                parent.remaining -= budget.allocated;
                parent.allocated = parent.allocated.saturating_add(budget.allocated);
                if !parent.exceeded && budget.refused_bytes > parent.remaining {
                    parent.exceeded = true;
                    parent.refused_bytes = budget.refused_bytes;
                    parent.error = budget.error.take();
                }
            }
            ScriptAllocationReport {
                allocated_bytes: budget.allocated,
                exceeded: budget.exceeded,
            }
        })
    }

    /// Reserve the container before mutating it. A physical refusal is independent
    /// of the host's logical allocation budget.
    #[inline]
    pub(crate) fn reserve_for(
        &mut self,
        reason: &'static str,
        trap: crate::trap::ScriptTrap,
        reserve: impl FnOnce(&mut Self) -> Result<(), std::collections::TryReserveError>,
    ) -> bool {
        if self.memory_refusal_pending { return false; }
        if reserve(self).is_ok() { return true; }
        self.refuse_memory(reason, trap);
        false
    }

    #[cold]
    pub(crate) fn refuse_memory(&mut self, reason: &'static str, trap: crate::trap::ScriptTrap) {
        if !self.memory_refusal_pending {
            let ip = match trap {
                crate::trap::ScriptTrap::Inner(trap) => trap.ip,
                crate::trap::NoTrap => ScriptIp::UNKNOWN,
            };
            self.memory_refusal = Some(ScriptMemoryRefusal { reason, ip });
            if self.in_vm_entry() {
                self.memory_refusal_pending = true;
                self.allocation_error_pending = true;
            } else {
                // No VM entry owns this failure. Report it to the host right here.
                report_refusal(reason);
            }
            if let Some(budget) = &mut self.allocation_budget { budget.exceeded = true; }
        }
    }

    #[inline]
    pub(crate) fn memory_refusal_pending(&self) -> bool { self.memory_refusal_pending }

    /// Marking and reclamation only borrow storage acquired before the first mark.
    pub(crate) fn preflight_gc(&mut self, code: &crate::vm::ScriptCode) -> bool {
        let bodies = code.bodies.borrow().len();
        if reserve_to(&mut self.mark_vec, self.objects.len() + self.arrays.len())
            && size_marks(&mut self.object_marks, self.objects.len())
            && size_marks(&mut self.array_marks, self.arrays.len())
            && reserve_to(&mut self.transient_bodies, bodies)
            && reserve_to(&mut self.live_bodies, bodies)
            && reserve_to(&mut self.newly_live_bodies, bodies)
            && reserve_to(&mut code.free_bodies.borrow_mut(), bodies)
        {
            true
        } else {
            self.refuse_memory(COLLECTING_GARBAGE, crate::trap::NoTrap);
            false
        }
    }

    /// Room for one more object slot (see [`reserve_table_slot`]). False after
    /// recording the refusal; the caller returns its refused value.
    #[inline]
    pub(crate) fn reserve_object_slot(&mut self) -> bool {
        if self.memory_refusal_pending { return false; }
        const REASON: &str = "out of memory while creating an object";
        let result = reserve_table_slot(&mut self.objects, &mut self.objects_free, REASON)
            .and_then(|()| reserve_trace_slot(&mut self.mark_vec, &mut self.object_marks,
                self.objects.len(), self.arrays.len(), REASON));
        self.table_slot_result(result)
    }

    #[inline]
    pub(crate) fn reserve_array_slot(&mut self) -> bool {
        if self.memory_refusal_pending { return false; }
        const REASON: &str = "out of memory while creating an array";
        let result = reserve_table_slot(&mut self.arrays, &mut self.arrays_free, REASON)
            .and_then(|()| reserve_trace_slot(&mut self.mark_vec, &mut self.array_marks,
                self.arrays.len(), self.objects.len(), REASON));
        self.table_slot_result(result)
    }

    /// A new string slot and its intern-table entry.
    #[inline]
    pub(crate) fn reserve_string_slot(&mut self) -> bool {
        if self.memory_refusal_pending { return false; }
        let result = reserve_table_slot(&mut self.strings, &mut self.strings_free, STORING_A_STRING);
        self.table_slot_result(result) && self.reserve_string_intern()
    }

    /// The intern-table entry for a string stored in a reused slot.
    #[inline]
    pub(crate) fn reserve_string_intern(&mut self) -> bool {
        if self.memory_refusal_pending { return false; }
        let room = self.string_intern.len() < self.string_intern.capacity() || self.string_intern.try_reserve(1).is_ok();
        self.table_slot_result(if room { Ok(()) } else { Err(STORING_A_STRING) })
    }

    #[inline]
    fn table_slot_result(&mut self, result: Result<(), &'static str>) -> bool {
        match result {
            Ok(()) => true,
            Err(reason) => {
                self.refuse_memory(reason, crate::trap::NoTrap);
                false
            }
        }
    }

    pub(crate) fn memory_error_value(&mut self, ip: ScriptIp) -> ScriptValue {
        let refusal = self.memory_refusal.as_mut().expect("pending memory refusal");
        if refusal.ip.is_unknown() { refusal.ip = ip; }
        ScriptValue::script_err_limit(refusal.ip)
    }

    pub(crate) fn finish_memory_refusal(&mut self) {
        self.memory_refusal_pending = false;
        self.allocation_error_pending = self.first_limit_error().is_some();
    }

    #[cold]
    fn clear_superseded_limit_errors(&mut self) {
        if let Some(budget) = &mut self.allocation_budget { budget.error = None; }
        if let Some(cap) = &mut self.heap_cap { cap.error = None; }
        self.pending_string_limit_error = None;
    }

    #[inline]
    pub(crate) fn enter_vm_entry(&mut self) {
        if self.vm_entry_depth == 0 {
            self.memory_refusal = None;
            self.finish_memory_refusal();
        }
        self.vm_entry_depth += 1;
    }

    #[inline]
    pub(crate) fn leave_vm_entry(&mut self) {
        debug_assert!(self.vm_entry_depth > 0);
        self.vm_entry_depth -= 1;
        if self.vm_entry_depth == 0 && self.memory_refusal_pending {
            self.clear_superseded_limit_errors();
            self.finish_memory_refusal();
        }
    }

    #[inline]
    pub(crate) fn in_vm_entry(&self) -> bool {
        self.vm_entry_depth != 0
    }

    /// Takes the last completed physical refusal once, including its
    /// instruction pointer when known. Reading cannot disarm an active unwind.
    #[inline]
    pub fn take_memory_refusal(&mut self) -> Option<ScriptMemoryRefusal> {
        let mut out = ScriptMemoryRefusal { reason: "", ip: ScriptIp::UNKNOWN };
        self.read_memory_refusal(&mut out).then_some(out)
    }

    #[relax_api(run)]
    fn read_memory_refusal(&mut self, out: &mut ScriptMemoryRefusal) -> bool {
        if self.memory_refusal_pending { return false; }
        if let Some(refusal) = self.memory_refusal.take() {
            *out = refusal;
            return true;
        }
        false
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
    #[relax_api(run)]
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
    /// before the next opcode. Physical exhaustion takes precedence and
    /// stays pending until the outermost VM entry has unwound.
    #[inline(always)]
    pub(crate) fn take_allocation_error(&mut self) -> Option<AllocationError> {
        if !self.allocation_error_pending {
            return None;
        }
        if self.memory_refusal_pending {
            return Some(AllocationError::Memory);
        }
        self.take_pending_allocation_error().map(AllocationError::Limit)
    }

    #[inline(never)]
    #[cold]
    fn take_pending_allocation_error(&mut self) -> Option<String> {
        let error = self.take_first_allocation_error();
        self.allocation_error_pending = self.peek_allocation_error().is_some();
        error
    }

    pub(crate) fn peek_allocation_error(&self) -> Option<&str> {
        if !self.allocation_error_pending { return None; }
        if self.memory_refusal_pending {
            return Some(self.memory_refusal.as_ref().map_or("out of memory", |r| r.reason));
        }
        self.first_limit_error()
    }

    fn first_limit_error(&self) -> Option<&str> {
        self.allocation_budget.as_ref().and_then(|budget| budget.error.as_deref())
            .or_else(|| self.heap_cap.as_ref().and_then(|cap| cap.error.as_deref()))
            .or(self.pending_string_limit_error.as_deref())
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
    #[relax_api(run)]
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
    #[relax_api(run)]
    pub fn accounted_heap_bytes(&self) -> usize {
        let charged = self.heap_cap.as_ref().map_or(0, |cap| cap.allocated);
        self.heap_baseline_bytes.saturating_add(charged)
    }

    /// Recomputes the retained-capacity estimate from the current heap state
    /// and re-arms the cap's headroom from it. Appropriate after trusted raw
    /// VM configuration, host-side value injection, or garbage collection.
    /// Normal script allocation paths update the cached amount in constant
    /// time instead of scanning the heap on every instruction.
    #[relax_api(run)]
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
    #[relax_api(run)]
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

        bytes = bytes.saturating_add(bytes_for::<u32>(self.mark_vec.capacity()));
        bytes = bytes.saturating_add(bytes_for::<u64>(self.object_marks.capacity()));
        bytes = bytes.saturating_add(bytes_for::<u64>(self.array_marks.capacity()));
        bytes = bytes.saturating_add(bytes_for::<bool>(self.transient_bodies.capacity()));
        bytes = bytes.saturating_add(bytes_for::<bool>(self.live_bodies.capacity()));
        bytes = bytes.saturating_add(bytes_for::<usize>(self.newly_live_bodies.capacity()));
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
        if self.gc_phase == GcPhase::Skipped { return; }
        self.mark_vec.shrink_to(self.objects.len() + self.arrays.len());
        self.object_marks.shrink_to(self.objects.len().div_ceil(64));
        self.array_marks.shrink_to(self.arrays.len().div_ceil(64));
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

        // A later sweep can return every collectible slot without growing these lists.
        self.objects_free.shrink_to(self.objects.len());
        self.strings_free.shrink_to(self.strings.len());
        self.arrays_free.shrink_to(self.arrays.len());
        self.pods_free.shrink_to(self.pods.len());
        self.pod_types_free.shrink_to(self.pod_types.len());
        self.handles_free.shrink_to(self.handles.len());
        self.regexes_free.shrink_to(self.regexes.len());
        self.reconcile_heap_bytes_if_limited();
    }

    /// This heap's tag, the high byte of every generation it mints (heap_tag.rs).
    pub fn heap_tag(&self) -> u8 {
        self.heap_stat(HEAP_STAT_TAG) as u8
    }

    /// Whether the VM has an allocation refusal to deliver.
    #[inline]
    pub fn allocation_error_pending(&self) -> bool {
        self.heap_stat(HEAP_STAT_ALLOCATION_ERROR_PENDING) != 0
    }

    pub(crate) fn empty() -> Self {
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
            touch: crate::touch::ScriptTouch::new_if_recording(),
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
        // A refused table slot returns these, so they must exist before any refusal can.
        v.ensure_allocation_poison();

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
    /// `obj` or of its nearest typed prototype (`name`, `found`); TYPE_NAME_BY_ID the script name
    /// the type `id` is registered under (`name`, `found`).
    #[relax_api(run)]
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
            TYPE_NAME_BY_ID => {
                if let Some(name) = self.type_name_of(id) {
                    r.name = name;
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
                        if let Some(name) = self.type_name_of(type_id) {
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
    #[relax_api(run)]
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
    #[relax_api(run)]
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
    /// marks them: type slots, type defaults, the live pod types (not a
    /// root: a pod type lives while its owner object is reached), Rust-held
    /// object refs, Rust-held array refs, Rust-held handles. Two readings
    /// apart in time say which root a leak hangs off.
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
    #[relax_api(run)]
    pub fn heap_stat(&self, which: u32) -> u64 {
        match which {
            HEAP_STAT_KEY => self.roots.ptr() as usize as u64,
            HEAP_STAT_TYPE_SLOTS => self.type_check.len() as u64,
            HEAP_STAT_TYPE_DEFAULTS => self.type_defaults.len() as u64,
            HEAP_STAT_POD_TYPES => (self.pod_types.len() - self.pod_types_free.len()) as u64,
            HEAP_STAT_ROOTED_OBJECTS => self.roots.get().rooted_len(crate::roots::ROOT_OBJECT) as u64,
            HEAP_STAT_ROOTED_ARRAYS => self.roots.get().rooted_len(crate::roots::ROOT_ARRAY) as u64,
            HEAP_STAT_ROOTED_HANDLES => self.roots.get().rooted_len(crate::roots::ROOT_HANDLE) as u64,
            HEAP_STAT_OBJECTS_LEN => self.objects.len() as u64,
            HEAP_STAT_REUSE_EPOCH => self.object_reuse_epoch,
            HEAP_STAT_LIVE_LEN => self.gc_live_len_impl() as u64,
            HEAP_STAT_NEEDS_GC => self.needs_gc_impl() as u64,
            HEAP_STAT_TAG => self.heap_tag.get() as u64,
            HEAP_STAT_ALLOCATION_ERROR_PENDING => self.allocation_error_pending as u64,
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
    #[inline]
    pub fn type_name_by_id(&self, type_id: ScriptTypeId) -> Option<LiveId> {
        let r = self.type_info(TYPE_NAME_BY_ID, type_id, ScriptObject::ZERO);
        r.found.then_some(r.name)
    }

    /// type_name_by_id inside the type table's op (it reads rustc's tables)
    fn type_name_of(&self, type_id: ScriptTypeId) -> Option<LiveId> {
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

    /// Whether a guest asked for a collection since the last call, clearing the request. The host
    /// that pumps the VM collects it at most once per turn, so a loop of `gc.run()` costs fuel like
    /// any loop instead of a collection each.
    pub fn take_gc_request(&mut self) -> bool {
        std::mem::take(&mut self.gc_requested)
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
        if let Some(obj) = obj {
            self.touch_obj(obj);
        }
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
    #[relax_api(run)]
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
    #[relax_api(run)]
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

    #[relax_api(run)]
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

    fn refusal_vm(host: &mut ScriptVmHost) -> ScriptVm<'_> {
        let mut bx = Box::new(ScriptVmBase::new());
        bx.bail_on_uncaught_error = true;
        bx.captured_errors = Some(Vec::new());
        ScriptVm { host: host.host_context(), bx }
    }

    fn cap_object_growth(heap: &mut ScriptHeap, headroom: usize) {
        while !heap.objects_free.is_empty() {
            heap.new_object();
        }
        heap.reconcile_heap_bytes();
        heap.set_max_heap_bytes(Some(heap.accounted_heap_bytes() + headroom));
    }

    fn assert_limit_record(vm: &mut ScriptVm, message: &str) {
        let errors = vm.take_error_records();
        assert_eq!(errors.len(), 1, "expected a single refusal: {errors:?}");
        assert!(errors[0].message().contains(message), "wrong refusal: {errors:?}");
        vm.capture_errors();
    }

    #[test]
    fn array_map_filter_retain_stop_callback_after_heap_cap() {
        fn warm_free_objects_drain_arrays(heap: &mut ScriptHeap) {
            while !heap.arrays_free.is_empty() { heap.new_array(); }
            if heap.objects_free.len() < 8 {
                let slots: [ScriptObject; 8] = std::array::from_fn(|_| heap.new_object());
                for slot in slots { heap.free_object_if_unreffed(slot); }
            }
        }

        let mut host = ScriptVmHost::new((), ());
        let mut vm = refusal_vm(&mut host);
        let calls = std::rc::Rc::new(std::cell::Cell::new(0usize));
        let probe = vm.new_module(id!(probe));
        let counter = calls.clone();
        vm.add_method(probe, id_lut!(bump), &[], move |_, _| {
            counter.set(counter.get() + 1);
            NIL
        });
        let counter = calls.clone();
        vm.add_method(probe, id_lut!(fail_str), &[(id!(x), NIL)], move |vm, _| {
            counter.set(counter.get() + 1);
            vm.bx.heap.new_string_from_str("01234");
            true.into()
        });
        let arr = vm.bx.heap.new_array();
        let obj = vm.bx.heap.new_object();
        vm.set_injected_global(id!(arr), arr.into());
        vm.set_injected_global(id!(obj), obj.into());
        let nil_calls = ["map", "filter"].map(|m| eval(&mut vm, &format!("{m}_nil"), &format!("fn() {{ arr.{m}(nil) }}")));
        let string_calls = ["map", "filter", "find", "any"].map(|m|
            eval(&mut vm, m, &format!("fn() {{ arr.{m}(mod.probe.fail_str) }}")));
        let retains = ["arr", "obj"].map(|receiver| eval(&mut vm, receiver,
            &format!("let cb = fn(x) {{ mod.probe.bump()\n return true }}\nfn() {{ {receiver}.retain(cb) }}")));

        let empty_costs = nil_calls.map(|f| {
            warm_free_objects_drain_arrays(&mut vm.bx.heap);
            let (value, report) = vm.with_heap_allocation_limit(usize::MAX, |vm| vm.call(f, &[]));
            assert!(value.is_array() && !report.exceeded, "empty map/filter must return a result array");
            report.allocated_bytes
        });
        let refused_array_growth = |vm: &mut ScriptVm| [0, 1].map(|i| {
            let heap = &mut vm.bx.heap;
            warm_free_objects_drain_arrays(heap);
            heap.reconcile_heap_bytes();
            heap.set_max_heap_bytes(Some(heap.accounted_heap_bytes() + empty_costs[i] - 1));
            let before = heap.gc_live_counts().objects;
            assert!(vm.call(nil_calls[i], &[]).is_err(), "refused result array must stop the call");
            assert!(vm.bx.heap.take_heap_limit_exceeded(), "result array must hit the heap cap");
            assert_limit_record(vm, "while creating an array");
            vm.bx.heap.set_max_heap_bytes(None);
            vm.bx.heap.gc_live_counts().objects - before
        });
        let refused_string_growth = |vm: &mut ScriptVm| {
            vm.bx.heap.set_max_string_bytes(Some(4));
            let growth = string_calls.map(|f| {
                calls.set(0);
                let before = vm.bx.heap.gc_live_counts().objects;
                assert!(vm.call(f, &[]).is_err(), "callback string refusal must end the call");
                assert!(vm.bx.heap.take_string_limit_exceeded(), "callback must hit the string cap");
                assert_limit_record(vm, "while creating a string");
                assert_eq!(calls.get(), 1, "only the first native callback may execute");
                vm.bx.heap.gc_live_counts().objects - before
            });
            vm.bx.heap.set_max_string_bytes(None);
            growth
        };
        let empty_growth = refused_array_growth(&mut vm);
        vm.bx.heap.array_push(arr, 1.into(), NoTrap);
        vm.bx.heap.vec_push(obj, NIL, 1.into(), NoTrap);
        let single_growth = refused_string_growth(&mut vm);
        let retain_costs = retains.map(|f| {
            while !vm.bx.heap.arrays_free.is_empty() { vm.bx.heap.new_array(); }
            while !vm.bx.heap.objects_free.is_empty() { vm.bx.heap.new_object(); }
            calls.set(0);
            let (value, report) = vm.with_heap_allocation_limit(usize::MAX, |vm| vm.call(f, &[]));
            assert!(!value.is_err() && !report.exceeded, "one-element retain must complete");
            assert_eq!(calls.get(), 1, "one-element retain must call its predicate");
            report.allocated_bytes
        });
        for n in 2..=4 {
            vm.bx.heap.array_push(arr, n.into(), NoTrap);
            vm.bx.heap.vec_push(obj, NIL, n.into(), NoTrap);
        }
        let array_growth = refused_array_growth(&mut vm);
        let string_growth = refused_string_growth(&mut vm);
        for (f, cost) in retains.into_iter().zip(retain_costs) {
            while !vm.bx.heap.arrays_free.is_empty() { vm.bx.heap.new_array(); }
            cap_object_growth(&mut vm.bx.heap, cost);
            calls.set(0);
            assert!(vm.call(f, &[]).is_err(), "a refused retain callback must bail");
            assert!(vm.bx.heap.take_heap_limit_exceeded(), "retain must reach the heap cap");
            assert_limit_record(&mut vm, "script heap allocation limit exceeded while");
            assert_eq!(calls.get(), 1, "retain must stop before the second callback");
            vm.bx.heap.set_max_heap_bytes(None);
        }
        let lengths = (vm.bx.heap.array_len(arr), vm.bx.heap.vec_len(obj));
        assert_eq!((array_growth, string_growth, lengths), (empty_growth, single_growth, (4, 4)),
            "result-array and callback refusals must skip later scopes, and retain must preserve both receivers");
    }

    #[test]
    fn regex_methods_and_split_stop_on_allocation_refusal() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = refusal_vm(&mut host);
        for (id, pattern, flags) in [
            (id!(comma), ",", "g"), (id!(no_match), "z", "g"),
            (id!(words), "[a-z]+", "g"), (id!(pair), "(a)(b)", ""),
            (id!(left_pair), "(aaaaaaa)(ffffff)", ""), (id!(right_pair), "(aaaaaaa)(gggggg)", ""),
        ] {
            let re = vm.bx.heap.new_regex(pattern, flags).expect("compile fixture regex");
            vm.set_injected_global(id, re);
        }
        let loops = [
            ("aaaaaaa,bbbbbb", "split(\",\")", "bbbbbb"),
            ("aaaaaaa,cccccc", "split(comma)", "cccccc"),
            ("aaaaaaa,dddddd", "match_str(words)", "dddddd"),
            ("aaaaaaa,eeeeee", "match_all(words)", "eeeeee"),
        ].map(|(s, method, tail)| {
            let f = eval(&mut vm, tail, &format!("fn() {{ \"{s}\".{method} }}"));
            (f, tail)
        });
        let [replace, noop, exec, first, test, left, right] = [
            r#""a,b,c,d".replace(comma, "-")"#, r#""a,b,c,d".replace(no_match, "-")"#,
            r#"pair.exec("zz")"#, r#""zz".match_str(pair)"#, r#"pair.test("zz")"#,
            r#"left_pair.exec("aaaaaaaffffff")"#, r#""aaaaaaagggggg".match_str(right_pair)"#,
        ].map(|body| eval(&mut vm, body, &format!("fn() {{ {body} }}")));

        vm.bx.heap.set_max_string_bytes(Some(6));
        let interned = loops.map(|(f, tail)| {
            assert!(vm.bx.heap.check_intern_string(tail).is_none(), "tail {tail} must start outside the string table");
            assert!(vm.call(f, &[]).is_err(), "the first long token before {tail} must end the call");
            assert!(vm.bx.heap.take_string_limit_exceeded(), "token before {tail} must reach the string cap");
            assert_limit_record(&mut vm, "while creating a string");
            vm.bx.heap.check_intern_string(tail).is_some()
        });
        let captures = [(left, "ffffff"), (right, "gggggg")].map(|(f, tail)| {
            assert!(vm.bx.heap.check_intern_string(tail).is_none(), "capture {tail} must start outside the string table");
            let before = vm.bx.heap.gc_live_counts().arrays;
            assert!(vm.call(f, &[]).is_err(), "the long full match must stop capture construction");
            assert!(vm.bx.heap.take_string_limit_exceeded(), "the full match must reach the string cap");
            assert_limit_record(&mut vm, "while creating a string");
            (vm.bx.heap.gc_live_counts().arrays - before, vm.bx.heap.check_intern_string(tail).is_some())
        });
        vm.bx.heap.set_max_string_bytes(None);

        let mut measure = |f| {
            while !vm.bx.heap.objects_free.is_empty() { vm.bx.heap.new_object(); }
            let (value, report) = vm.with_heap_allocation_limit(usize::MAX, |vm| vm.call(f, &[]));
            assert!(!value.is_err() && !report.exceeded, "the uncapped no-match prefix must complete");
            report.allocated_bytes
        };
        let cost_noop = measure(noop);
        let cost_test = measure(test);
        cap_object_growth(&mut vm.bx.heap, cost_noop);
        vm.bx.heap.set_max_string_bytes(Some(4));
        assert!(vm.call(replace, &[]).is_err(), "the first refused match must stop replacement");
        assert!(vm.bx.heap.take_heap_limit_exceeded(), "collecting the first match must hit the heap cap");
        assert_limit_record(&mut vm, "while collecting regex matches");
        let pending = vm.bx.heap.allocation_error_pending;
        let string_limit = vm.bx.heap.take_string_limit_exceeded();
        vm.bx.heap.set_max_string_bytes(None);
        vm.bx.heap.set_max_heap_bytes(None);

        let slots = [exec, first].map(|f| {
            cap_object_growth(&mut vm.bx.heap, cost_test);
            let value = vm.call(f, &[]);
            let exceeded = vm.bx.heap.take_heap_limit_exceeded();
            if exceeded { assert_limit_record(&mut vm, "while running a regex"); }
            vm.bx.heap.set_max_heap_bytes(None);
            (value.is_err(), exceeded)
        });
        assert_eq!((interned, captures, (pending, string_limit), slots),
            ([false; 4], [(0, false); 2], (false, false), [(true, true); 2]),
            "split/match loops must skip later tokens, exec must skip captures, replace must deliver only the heap limit, and both exec paths must charge slots");
    }

    #[test]
    fn html_query_array_and_parse_json_stop_on_heap_cap() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = refusal_vm(&mut host);
        let sel = eval(&mut vm, "selection", r#""<a>1</a><a>2</a><a>3</a>".parse_html().query("a")"#);
        let doc = eval(&mut vm, "document", r#""<a href='aaaaaaa'>aaaaaaa</a><a href='bbbbbb'>cccccc</a>".parse_html()"#);
        let raw = vm.bx.heap.new_string_from_str("[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]");
        vm.set_injected_global(id!(sel), sel);
        vm.set_injected_global(id!(doc), doc);
        vm.set_injected_global(id!(raw), raw);
        let [array, attr, text, json] = ["sel.array()", r#"doc.query("a@href")"#, r#"doc.query("a.text")"#, "raw.parse_json()"]
            .map(|body| eval(&mut vm, body, &format!("fn() {{ {body} }}")));
        let refuse_result_array = |vm: &mut ScriptVm, f| {
            let entry = size_of::<(ScriptValue, ScriptMapValue)>() + 2 * size_of::<usize>();
            while !vm.bx.heap.arrays_free.is_empty() { vm.bx.heap.new_array(); }
            cap_object_growth(&mut vm.bx.heap, 2 * size_of::<ScriptObjectData>() + entry);
            assert!(vm.call(f, &[]).is_err(), "refusing the result array must end the call");
            assert!(vm.bx.heap.take_heap_limit_exceeded(), "result array must hit the retained cap");
            assert_limit_record(vm, "while creating an array");
            vm.bx.heap.set_max_heap_bytes(None);
        };
        let before = vm.bx.heap.handles.len();
        refuse_result_array(&mut vm, array);
        let handles = vm.bx.heap.handles.len() - before;

        vm.bx.heap.set_max_string_bytes(Some(6));
        let interned = [(attr, "bbbbbb"), (text, "cccccc")].map(|(f, tail)| {
            assert!(vm.bx.heap.check_intern_string(tail).is_none(), "trailing query value {tail} must start uninterned");
            assert!(vm.call(f, &[]).is_err(), "the first seven-byte query value must stop the call");
            assert!(vm.bx.heap.take_string_limit_exceeded(), "query must report the string limit");
            assert_limit_record(&mut vm, "while creating a string");
            vm.bx.heap.check_intern_string(tail).is_some()
        });
        vm.bx.heap.set_max_string_bytes(None);

        refuse_result_array(&mut vm, json);
        let parser = &vm.bx.threads.cur_ref().json_parser.parser;
        let root = (parser.index, parser.root.is_nil());

        vm.bx.heap.new_array();
        vm.gc();
        while vm.bx.heap.arrays_free.len() > 1 { vm.bx.heap.new_array(); }
        let outer = *vm.bx.heap.arrays_free.last().expect("one free slot for the outer JSON array");
        cap_object_growth(&mut vm.bx.heap, 0);
        let mut parser = crate::json::JsonParserThread::default();
        let nested = parser.read_json("[{}]", &mut vm.bx.heap);
        assert!(vm.bx.heap.take_heap_limit_exceeded(), "the nested object must hit the retained cap");
        assert_eq!(vm.bx.heap.array_len(outer), 0, "a refused child must leave the outer array empty");
        assert_eq!((handles, interned, root, (parser.parser.index, nested.is_nil())),
            (0, [false; 2], (1, true), (2, true)),
            "HTML must skip trailing handles and strings, and JSON must stop at the refusing root or child with NIL");
    }

    #[test]
    fn eval_scope_setup_and_tokenize_refusal_report_limit() {
        use crate::vm::{ScriptEvalReq, EVAL_ADD};

        let mut host = ScriptVmHost::new((), ());
        let mut vm = refusal_vm(&mut host);
        let parent = vm.bx.heap.new_object();
        let poison = vm.bx.heap.allocation_poison_object;
        let proto = vm.bx.heap.objects[poison].proto;
        cap_object_growth(&mut vm.bx.heap, 0);
        let scoped = ScriptMod::new("", "", "scoped", 1, 0, "42", &[]);
        let value = vm.eval_in_scope(scoped, parent);
        assert_eq!(vm.bx.heap.objects[poison].proto, proto, "a refused scope must preserve the sentinel prototype");
        {
            let bodies = vm.bx.code.bodies.borrow();
            let body = bodies.last().expect("registered body");
            assert!(body.scope.is_unrooted() && body.me.is_unrooted(), "refused body objects must be unrooted");
        }
        assert!(value.is_err(), "a refused eval scope must return an error");
        assert!(vm.bx.heap.take_heap_limit_exceeded(), "scope construction must reach the heap limit");
        assert_limit_record(&mut vm, "script heap allocation limit exceeded");
        vm.bx.heap.set_max_heap_bytes(None);

        vm.set_injected_global(id!(g1), 1.into());
        vm.set_injected_global(id!(g2), 2.into());
        let entry = size_of::<(ScriptValue, ScriptMapValue)>() + 2 * size_of::<usize>();
        cap_object_growth(&mut vm.bx.heap, size_of::<ScriptObjectData>() + 2 * entry);
        let added = ScriptMod::new("", "", "added", 1, 0, "42", &[]);
        let mut req = ScriptEvalReq::new(EVAL_ADD, added.view());
        let value = vm.vm_eval(&mut req);
        assert!(value.is_err(), "adding a body must report a refused global");
        assert!(vm.bx.heap.take_heap_limit_exceeded(), "injecting a global must reach the heap limit");
        assert_limit_record(&mut vm, "while adding an object property");
        {
            let bodies = vm.bx.code.bodies.borrow();
            let body = &bodies[req.body as usize];
            assert!(!body.scope.is_unrooted() && body.me.is_unrooted(), "only the refused body object must be unrooted");
            assert_eq!(vm.bx.heap.map_len(body.scope.obj), 2, "the scope must contain mod and one completed global");
        }
        vm.bx.heap.set_max_heap_bytes(None);

        for append in [false, true] {
            let code = "\"12345\" + (";
            let module = || ScriptMod::new("", "", if append { "stream" } else { "eval" }, 1, 0, code, &[]);
            vm.bx.heap.set_max_string_bytes(Some(4));
            let result = if append {
                vm.eval_with_append_source(module(), code, ScriptObject::ZERO)
            } else { vm.eval(module()) };
            assert!(result.is_err(), "tokenization refusal must end evaluation (append={append})");
            assert!(vm.bx.heap.take_string_limit_exceeded(), "tokenization must hit the string limit (append={append})");
            assert_limit_record(&mut vm, "script string allocation limit exceeded");
            vm.bx.heap.set_max_string_bytes(None);
            let result = if append {
                vm.eval_with_append_source(module(), "\"12345\" + (\"6\")", ScriptObject::ZERO)
            } else { eval(&mut vm, "eval", "\"123456\"") };
            assert_eq!(string_of(&vm, result).as_deref(), Some("123456"), "tokenization must retry cleanly (append={append})");
        }
    }

    #[test]
    fn array_apply_transform_refusal_never_invokes_native_without_self() {
        use std::cell::Cell;
        use std::rc::Rc;

        let mut host = ScriptVmHost::new((), ());
        let mut vm = refusal_vm(&mut host);
        let array = vm.bx.heap.new_array();
        let calls = Rc::new(Cell::new(0));
        let count = calls.clone();
        let native = vm.add_apply_transform_fn(move |vm, args| {
            count.set(count.get() + 1);
            vm.value(args, id!(self))
        });
        vm.bx.heap.arrays[array].tag.set_apply_transform(native);
        while !vm.bx.heap.objects_free.is_empty() { vm.bx.heap.new_object(); }
        let obj = size_of::<ScriptObjectData>();
        for (budget, operation) in [(obj - 1, "creating an object"), (obj, "adding an object property")] {
            let (result, report) = vm.with_heap_allocation_limit(budget, |vm| vm.call_apply_transform(array.into()));
            assert!(report.exceeded, "the transform must reach its budget while {operation}");
            assert!(result.is_some_and(|v| v.is_err()), "the refused transform must return an error while {operation}");
            assert_eq!(calls.get(), 0, "the transform must await a complete receiver");
            assert_limit_record(&mut vm, operation);
        }
        assert_eq!(vm.call_apply_transform(array.into()), Some(array.into()), "a successful transform must receive its array");
        assert_eq!(calls.get(), 1, "a complete receiver must dispatch once");
    }

    #[test]
    fn string_keys_set_value_string_limit_never_inserts_nil_key() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = refusal_vm(&mut host);
        let obj = vm.bx.heap.new_object();
        vm.bx.heap.set_string_keys(obj);
        vm.set_injected_global(id!(o), obj.into());
        vm.bx.heap.set_max_string_bytes(Some(4));
        let result = eval(&mut vm, "string-key", "o.long_property_name = 42");
        assert!(result.is_err(), "a refused property name must end the evaluation");
        assert!(vm.bx.heap.take_string_limit_exceeded(), "the property name must reach the string cap");
        assert_limit_record(&mut vm, "script string allocation limit exceeded");
        assert!(vm.bx.heap.map_ref(obj).is_empty(), "a refused name must leave the property map empty");
    }

    #[test]
    fn proto_field_refusal_never_stores_poison_or_reports_not_found() {
        use crate::opcode::{Opcode, OpcodeArgs};
        use crate::thread::ScriptMe;

        for op in [Opcode::PROTO_INHERIT_READ, Opcode::FIELD_SLOT_READ, Opcode::PROTO_FIELD] {
            let mut host = ScriptVmHost::new((), ());
            let mut vm = refusal_vm(&mut host);
            let template = eval(&mut vm, "proto-child", "{ child := { x: 1 } }");
            let instance = vm.bx.heap.new_with_proto(template);
            let child = vm.bx.heap.vec_key_value(instance, 0, NoTrap);
            assert!(child.value.is_object(), "the instance must own the named child slot");
            let thread = vm.bx.threads.cur();
            if op == Opcode::PROTO_INHERIT_READ {
                thread.push_me(ScriptMe::Object(instance));
            } else {
                thread.push_stack_unchecked(instance.into());
            }
            thread.push_stack_unchecked(child.key);
            let ip = thread.trap.ip;
            cap_object_growth(&mut vm.bx.heap, 0);
            vm.opcode(op, OpcodeArgs::NONE);
            assert!(vm.bx.heap.take_heap_limit_exceeded(), "{op:?} must refuse the derived child");
            assert_eq!(vm.bx.heap.vec_key_value(instance, 0, NoTrap).value, child.value,
                "{op:?} must retain the original child");
            let thread = vm.bx.threads.cur_ref();
            assert!(thread.trap.err_is_empty(), "{op:?} must preserve the refusal without a missing-field error");
            assert_eq!(thread.trap.ip, ip, "{op:?} must stop at the refused field");
        }

        let mut heap = ScriptHeap::empty();
        let ty = ScriptTypeId::from_u128(1);
        let child_type = heap.register_type(Some(ty), ScriptTypeCheck::from_props(&mut ScriptTypeProps::default()));
        let child = heap.new_object();
        heap.type_defaults.insert(child_type, child);
        let mut fields = ScriptTypeProps::default();
        fields.insert(id!(child), ty);
        let parent_type = heap.register_type(None, ScriptTypeCheck::from_props(&mut fields));
        let parent = heap.new_object();
        heap.objects[parent].tag.set_type_index(parent_type);
        let trap = ScriptTrapInner::default();
        cap_object_growth(&mut heap, 0);
        let result = heap.proto_field_from_type_check(parent, id!(child), trap.pass());
        assert!(heap.take_heap_limit_exceeded(), "instantiating the field default must reach the heap cap");
        assert!(result.is_nil(), "a refused default must publish no child");
        assert!(trap.err_is_empty(), "the declared field must produce no lookup error");
        assert!(heap.map_ref(parent).is_empty(), "a refused default must leave the parent empty");

        for (base, field, levels) in [("{child := {x: 1}}", "child", 1), ("{a := {child := {x: 1}}}", "a.child", 2)] {
            let mut host = ScriptVmHost::new((), ());
            let mut vm = refusal_vm(&mut host);
            let template = eval(&mut vm, "template", base).as_object().expect("named-child template");
            let out = vm.bx.heap.new_object();
            vm.bx.heap.set_value_def(out, id!(obj).into(), NIL);
            vm.set_injected_global(id!(base), template.into());
            vm.set_injected_global(id!(out), out.into());
            let code = format!("fn() {{ base {{\n out.obj = me\n {field} +: {{x: 2}}\n }} }}");
            let f = eval(&mut vm, "derived-child", &code);
            let bytes = size_of::<ScriptObjectData>() + levels * (size_of::<ScriptObjectData>() + size_of::<ScriptVecValue>());
            cap_object_growth(&mut vm.bx.heap, bytes);
            assert!(vm.call(f, &[]).is_err(), "{field}: a refused child must bail");
            assert!(vm.bx.heap.take_heap_limit_exceeded(), "{field}: the child must reach the retained cap");
            assert_limit_record(&mut vm, "while creating an object with a prototype");
            let mut instance = vm.bx.heap.value(out, id!(obj).into(), NoTrap).as_object().expect("published instance");
            let mut base = template;
            if levels == 2 {
                instance = vm.bx.heap.vec_key_value(instance, 0, NoTrap).value.as_object().expect("derived parent");
                base = vm.bx.heap.vec_key_value(base, 0, NoTrap).value.as_object().expect("template parent");
            }
            assert_ne!(instance, base, "{field}: the containing instance must exist before the refusal");
            assert_eq!(vm.bx.heap.vec_key_value(instance, 0, NoTrap).value, vm.bx.heap.vec_key_value(base, 0, NoTrap).value,
                "{field}: the instance must retain the template child");
        }
    }

    #[test]
    fn use_glob_and_shadow_let_refusal_never_push_or_tag_poison_scope() {
        for headroom in [0, size_of::<ScriptObjectData>()] {
            let mut host = ScriptVmHost::new((), ());
            let mut vm = refusal_vm(&mut host);
            let scope = vm.bx.heap.new_object();
            let module = vm.bx.heap.new_object();
            vm.bx.heap.def_scope_value(scope, id!(x), 1.into());
            vm.bx.heap.set_value_def(module, id!(x).into(), 2.into());
            vm.bx.heap.set_value_def(module, id!(y).into(), 3.into());
            let thread = vm.bx.threads.cur();
            thread.push_scope(scope);
            thread.push_stack_unchecked(module.into());
            thread.push_stack_unchecked(id!(*).into());
            cap_object_growth(&mut vm.bx.heap, headroom);
            vm.handle_use();
            assert!(vm.bx.heap.take_heap_limit_exceeded(), "shadowing x must reach the heap cap");
            assert_eq!(vm.bx.threads.cur_ref().scopes.as_slice(), &[scope], "a refused shadow scope must stay off the stack");
            let poison = vm.bx.heap.allocation_poison_object;
            for obj in [scope, poison] {
                assert!(!vm.bx.heap.objects[obj].tag.has_globs(), "a refused use must leave the glob tag clear");
                assert!(!vm.bx.heap.glob_sources.contains_key(&obj), "a refused use must publish no glob source");
            }
            assert_eq!(vm.bx.heap.map_len(scope), 1, "later glob entries must remain unbound");
            assert_eq!(vm.bx.heap.value(scope, id!(x).into(), NoTrap), 1.into(), "the shadowed value must survive");
        }
        let mut heap = ScriptHeap::empty();
        let scope = heap.new_object();
        let later = heap.new_object();
        cap_object_growth(&mut heap, 0);
        let items = [(id!(a), 1.into()), (id!(b), later.into())];
        assert_eq!(heap.def_scope_values(scope, &items), (2, None), "a failed batch must stop the caller's binding loop");
        assert!(heap.take_heap_limit_exceeded(), "the first binding must hit the retained cap");
        assert!(!heap.objects[later].tag.is_reffed(), "the value after a refused binding must never be visited");
        assert!(heap.map_ref(scope).is_empty(), "a refused first binding must leave the scope empty");

        let mut host = ScriptVmHost::new((), ());
        let mut vm = refusal_vm(&mut host);
        let closures = eval(&mut vm, "glob-shadow",
            "let prefix = {a: 1}\nlet m = {a: 1, b: 2, c: 3}\n{\n prefix: fn() {let b = 0\n let keep = || b\n use prefix.*}\n shadow: fn() {let b = 0\n let keep = || b\n use m.*}\n}").as_object().expect("glob fixtures");
        let prefix = vm.bx.heap.value(closures, id!(prefix).into(), NoTrap);
        let shadow = vm.bx.heap.value(closures, id!(shadow).into(), NoTrap);
        while !vm.bx.heap.objects_free.is_empty() { vm.bx.heap.new_object(); }
        let (value, full) = vm.with_heap_allocation_limit(usize::MAX, |vm| vm.call(prefix, &[]));
        assert!(!value.is_err() && !full.exceeded, "the prefix must complete before the shared name");
        cap_object_growth(&mut vm.bx.heap, full.allocated_bytes);
        assert!(vm.call(shadow, &[]).is_err(), "the glob must bail at the shadowed name");
        assert!(vm.bx.heap.take_heap_limit_exceeded(), "creating the shadow scope must reach the heap cap");
        assert_limit_record(&mut vm, "while creating an object with a prototype");
        let poison = vm.bx.heap.allocation_poison_object;
        assert!(!vm.bx.heap.objects[poison].tag.has_globs() && !vm.bx.heap.glob_sources.contains_key(&poison),
            "a refused glob must publish no metadata for the sentinel");
    }

    #[test]
    fn splat_call_args_refusal_stops_before_type_shift() {
        use crate::opcode::{Opcode, OpcodeArgs};
        use crate::thread::{CallFrame, CallTarget, ScriptMe};

        for (source, call) in [("[1, \"hi\"]", true), ("{1, \"hi\"}", true), ("[1, 2]", false)] {
            let mut host = ScriptVmHost::new((), ());
            let mut vm = refusal_vm(&mut host);
            let f = eval(&mut vm, "parameters", "fn(a = 0, b = \"ok\") { a }");
            let values = eval(&mut vm, "values", source);
            assert!(f.is_object() && (values.is_object() || values.is_array()), "the splat fixtures must compile");
            let args = vm.bx.heap.new_with_proto(f);
            vm.bx.heap.clear_object_deep(args);
            let dst = vm.bx.heap.new_object();
            let thread = vm.bx.threads.cur();
            thread.push_call_frame(CallFrame {
                bases: thread.new_bases(), args: OpcodeArgs::NONE, return_ip: None,
                prev_slot_base: thread.slot_base, body_root: false, native_cont: false,
            });
            thread.push_me(if call {
                ScriptMe::Call { args, sself: None, target: CallTarget::Resolve(None) }
            } else { ScriptMe::Object(dst) });
            thread.push_stack_unchecked(values);
            let ip = thread.trap.ip;
            cap_object_growth(&mut vm.bx.heap, 0);
            vm.opcode(Opcode::ME_SPLAT, OpcodeArgs::NONE);
            assert!(vm.bx.heap.take_heap_limit_exceeded(), "{source} must reach the heap cap");
            let thread = vm.bx.threads.cur_ref();
            assert!(thread.trap.err_is_empty(), "{source} must produce no secondary type error");
            assert_eq!(thread.trap.ip, ip, "{source} must stop at the refused element");
            assert_eq!(vm.bx.heap.map_len(args), 0, "a refused argument must stay unbound");
            assert_eq!(vm.bx.heap.vec_len(dst), 0, "a refused splat must leave the destination empty");
        }

        let mut host = ScriptVmHost::new((), ());
        let mut vm = refusal_vm(&mut host);
        let f = eval(&mut vm, "splat", "fn f(a = 0, b = \"ok\") { a }\nfn() { f(..[1, \"hi\"]) }");
        assert!(f.is_object(), "the splatting closure must compile before enabling the cap");
        while !vm.bx.heap.arrays_free.is_empty() { vm.bx.heap.new_array(); }
        let bytes = 2 * size_of::<ScriptObjectData>() + size_of::<ScriptArrayData>() + 2 * size_of::<ScriptValue>();
        cap_object_growth(&mut vm.bx.heap, bytes);
        assert!(vm.call(f, &[]).is_err(), "a refused splat must end the enclosing call");
        assert!(vm.bx.heap.take_heap_limit_exceeded(), "binding the first positional argument must reach the heap cap");
        assert_limit_record(&mut vm, "while binding a function argument");
    }

    #[test]
    fn compound_string_assign_refusal_preserves_target_and_slot() {
        use crate::opcode::{Opcode, OpcodeArgs};

        for (setup, target) in [
            ("let s = \"abc\"", "s"),
            ("let o = {s: \"abc\"}", "o.s"),
            ("let o = {s: \"abc\"}", "o[\"s\"]"),
            ("let a = [\"abc\"]", "a[0]"),
        ] {
            let mut host = ScriptVmHost::new((), ());
            let mut vm = refusal_vm(&mut host);
            let code = format!("{setup}\n{{get: fn() {{ {target} }}, bump: fn() {{ {target} += \"123456\" }} }}");
            let obj = eval(&mut vm, "compound", &code).as_object().expect("accessor pair");
            let bump = vm.bx.heap.value(obj, id!(bump).into(), NoTrap);
            let get = vm.bx.heap.value(obj, id!(get).into(), NoTrap);
            vm.bx.heap.set_max_string_bytes(Some(8));
            assert!(vm.call(bump, &[]).is_err(), "{target} must bail");
            assert!(vm.bx.heap.take_string_limit_exceeded(), "{target} must hit the string cap");
            assert_limit_record(&mut vm, "while concatenating strings");
            vm.bx.heap.set_max_string_bytes(None);
            let value = vm.call(get, &[]);
            assert_eq!(string_of(&vm, value).as_deref(), Some("abc"), "{target} must retain its original value");
            cap_object_growth(&mut vm.bx.heap, size_of::<ScriptObjectData>());
            assert!(vm.call(bump, &[]).is_err(), "{target} must also bail on a retained-heap refusal");
            assert!(vm.bx.heap.take_heap_limit_exceeded(), "{target} must hit the retained cap");
            assert_limit_record(&mut vm, "script heap allocation limit exceeded while concatenating strings");
            vm.bx.heap.set_max_heap_bytes(None);
            let value = vm.call(get, &[]);
            assert_eq!(string_of(&vm, value).as_deref(), Some("abc"), "{target} must survive a retained-heap refusal");
        }

        let mut host = ScriptVmHost::new((), ());
        let mut vm = refusal_vm(&mut host);
        let old = vm.bx.heap.new_string_from_str("abc");
        let rhs = vm.bx.heap.new_string_from_str("123456");
        let t = vm.bx.threads.cur();
        t.slots.push(old);
        t.push_stack_unchecked(id!(x).into());
        t.push_stack_unchecked(rhs);
        let ip = t.trap.ip;
        vm.bx.heap.set_max_string_bytes(Some(8));
        vm.opcode(Opcode::ASSIGN_SLOT_ADD, OpcodeArgs::from_u32(0));
        let t = vm.bx.threads.cur();
        assert_eq!(t.slot(0), old, "the refused concatenation must preserve its slot");
        assert!(t.stack.is_empty(), "a refused assignment must publish no operand");
        assert_eq!(t.trap.ip, ip, "a refused assignment must keep its instruction pointer");
        assert!(vm.bx.heap.take_string_limit_exceeded(), "slot concatenation must reach the cap");

        assert_concat_refusal("local-add.splash",
            "fn f() {\n    var x = \"abc\"\n    x += \"123456\"\n    x\n}\nf()", 3);
    }

    fn assert_concat_refusal(file: &str, code: &str, line: u32) {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = refusal_vm(&mut host);
        vm.bx.heap.set_max_string_bytes(Some(8));
        let result = vm.eval(ScriptMod::new("", "", file, 1, 0, code, &[]));
        let errors = vm.take_error_records();
        assert!(result.is_err(), "{file} must bail on the refused expression: {result:?}");
        assert!(vm.bx.heap.take_string_limit_exceeded(), "{file} missed the string limit: {errors:?}");
        assert_eq!(errors.len(), 1, "{file} must report only the refusal: {errors:?}");
        let error = errors[0].view();
        assert!(error.message.contains("script string allocation limit exceeded while concatenating strings"),
            "{file} reported the wrong allocation: {errors:?}");
        assert_eq!((error.file, error.line), (file, line), "refusal must point to the expression");
    }

    #[test]
    fn same_opcode_error_after_refusal_still_reports_heap_limit() {
        assert_concat_refusal("assign.splash",
            "let o = {s: \"12345\"}\no.freeze()\no.s += \"6789\"\n99", 3);
        assert_concat_refusal("caught-assign.splash",
            "let o = {s: \"12345\"}\no.freeze()\ntry {\n    o.s += \"6789\"\n} catch {\n    99\n}", 4);
    }

    #[test]
    fn refused_expr_in_pod_or_typed_arg_pop_to_me_reports_limit() {
        assert_concat_refusal("pod-arg.splash",
            "use mod.pod.*\nvec2(\"12345\" + \"6789\", 0)", 2);
        assert_concat_refusal("typed-arg.splash",
            "fn f(x = 0) { x }\nf(\"12345\" + \"6789\")", 2);
    }

    #[test]
    fn logical_cap_loop_scope_refusal_never_frees_poison_sentinel() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = refusal_vm(&mut host);
        let parent = vm.bx.heap.new_object();
        while !vm.bx.heap.objects_free.is_empty() {
            vm.bx.heap.new_object();
        }
        vm.bx.heap.reconcile_heap_bytes();
        let baseline = vm.bx.heap.accounted_heap_bytes();
        let poison = vm.bx.heap.allocation_poison_object;
        for _ in 0..2 {
            vm.bx.heap.set_max_heap_bytes(Some(baseline));
            let scope = vm.bx.heap.new_loop_scope(parent);
            assert_eq!(scope, poison, "refused scope must use the shared sentinel");
            assert!(vm.bx.heap.take_allocation_error().is_some(), "scope must report the refusal");
            assert!(vm.bx.heap.take_heap_limit_exceeded(), "retained heap cap must be reported");
            let thread = vm.bx.threads.cur();
            let bases = thread.new_bases();
            assert!(thread.push_scope(scope), "scope must be present during unwind");
            thread.truncate_bases(bases, &mut vm.bx.heap);
            assert!(vm.bx.heap.objects.is_valid(poison), "unwind must preserve the sentinel generation");
            assert!(vm.bx.heap.objects_free.iter().all(|slot| slot.index != poison.index),
                "the sentinel slot must stay off the free list");
        }
        vm.bx.heap.set_max_heap_bytes(None);
        assert_eq!(eval(&mut vm, "after-scope.splash", "6 * 7").as_number(), Some(42.0),
            "the VM must accept work after lifting the cap");
    }

    #[test]
    fn refused_object_mode_and_tag_mutations_leave_poison_clean() {
        let mut heap = ScriptHeap::empty();
        let obj = heap.allocation_poison_object;
        let tag = heap.objects[obj].tag;
        let proto = heap.objects[obj].proto;
        let made_at = heap.objects[obj].made_at;
        let ip = ScriptIp { body: 1, index: 2 };
        heap.set_object_deep(obj);
        assert_eq!(heap.objects[obj].tag, tag, "setting deep must preserve the tag");
        heap.clear_object_deep(obj);
        heap.set_object_storage_vec2(obj);
        heap.set_object_pod_type(obj, ScriptPodType::VOID);
        heap.set_first_applied_and_clean(obj);
        heap.set_string_keys(obj);
        heap.set_from_eval(obj);
        heap.set_notproto(obj);
        heap.freeze_module(obj);
        heap.set_made_at(obj, ip);
        heap.set_fn(obj, crate::function::ScriptFnPtr::Script(ip));
        assert_eq!(heap.objects[obj].tag, tag, "object modes and function metadata must stay inert");
        assert_eq!(heap.objects[obj].proto, proto, "the sentinel must retain its prototype");
        assert_eq!(heap.objects[obj].made_at, made_at, "the sentinel must retain its origin");
    }

    #[test]
    fn refused_receiver_leaves_call_frame_for_unwind() {
        use crate::opcode::OpcodeArgs;
        use crate::thread::{CallTarget, ScriptMe};

        let mut host = ScriptVmHost::new((), ());
        let mut vm = refusal_vm(&mut host);
        let args = vm.bx.heap.new_object();
        let receiver = vm.bx.heap.new_array();
        let thread = vm.bx.threads.cur();
        let ip = thread.trap.ip;
        let stack_depth = thread.stack.len();
        assert!(thread.push_me(ScriptMe::Call {
            args, sself: Some(receiver.into()), target: CallTarget::Resolve(None),
        }), "call arguments must be present before binding the receiver");
        vm.bx.heap.reconcile_heap_bytes();
        vm.bx.heap.set_max_heap_bytes(Some(vm.bx.heap.accounted_heap_bytes()));
        assert!(!vm.handle_call_exec(OpcodeArgs::NONE), "a refused receiver suppresses pop_to_me");
        assert!(vm.bx.heap.take_heap_limit_exceeded(), "binding the receiver must hit the heap cap");
        let thread = vm.bx.threads.cur_ref();
        assert_eq!(thread.trap.ip, ip, "the refusal must stay on the call");
        assert_eq!(thread.stack.len(), stack_depth, "the refused call must publish no operand");
        assert!(matches!(thread.mes.last(), Some(ScriptMe::Call { args: frame, .. }) if *frame == args),
            "the call builder must be left for the unwind");
        assert!(vm.bx.heap.objects.is_valid(args), "the call arguments must survive until unwind");
        assert_eq!(vm.bx.heap.array_len(receiver), 0, "the receiver must retain its contents");
        assert!(thread.trap.err_is_empty(), "the refused call must emit no secondary error");

        let obj = size_of::<ScriptObjectData>();
        let pod = size_of::<ScriptPodData>() + 8;
        let item = size_of::<ScriptVecValue>();
        for (file, code, bytes, line) in [
            ("pod-receiver", "use mod.pod.*\nfn() {\n    vec2(\n        a.push(1),\n        0\n    )\n}", 2 * obj + pod + item, 4),
            ("typed-receiver", "fn f(x = 0) { x }\nfn() {\n    f(\n        a.push(1)\n    )\n}", 3 * obj + item, 4),
            ("caught-receiver", "use mod.pod.*\nfn() {\n    try {\n        vec2(\n            a.push(1),\n            0\n        )\n    } catch {\n        99\n    }\n}", 2 * obj + pod + item, 5),
        ] {
            let mut host = ScriptVmHost::new((), ());
            let mut vm = refusal_vm(&mut host);
            let array = vm.bx.heap.new_array();
            vm.set_injected_global(id!(a), array.into());
            let f = vm.eval(ScriptMod::new("", "", file, 1, 0, code, &[]));
            assert!(f.is_object(), "{file} must compile before enabling the cap");
            cap_object_growth(&mut vm.bx.heap, bytes);
            let value = vm.call(f, &[]);
            let errors = vm.take_error_records();
            assert!(value.is_err(), "{file}: the nested call must bail");
            assert!(vm.bx.heap.take_heap_limit_exceeded(), "{file}: the receiver must reach the heap cap");
            assert_eq!(vm.bx.heap.array_len(array), 0, "{file}: the receiver must stay empty");
            assert_eq!(errors.len(), 1, "{file}: expected a single refusal: {errors:?}");
            assert!(errors[0].message().contains("script heap allocation limit exceeded while adding an object property"),
                "{file}: the receiver binding must own the refusal: {errors:?}");
            assert_eq!(errors[0].view().line, line, "{file}: the reported line must be the inner call");
        }
    }

    #[test]
    fn poison_container_writes_are_inert_without_immutable_error() {
        let mut heap = ScriptHeap::empty();
        let errors = ScriptTrapInner::default();
        let trap = errors.pass();
        let obj = heap.allocation_poison_object;
        let arr = heap.allocation_poison_array;
        let source = heap.new_object();
        let nested = heap.new_object();
        let values = heap.new_array_from_vec_u8(vec![1]);
        heap.vec_push(source, NIL, nested.into(), NoTrap);
        let key = id!(name).into();
        let text = heap.new_string_from_str("name");
        let value = ScriptValue::from_f64(1.0);
        for k in [key, text, source.into(), value] {
            assert_eq!(heap.set_value(obj, k, value, trap), NIL, "set_value on sentinel for {k:?}");
        }
        assert_eq!(heap.set_value_slot(obj, key, value, trap), NIL, "slot write on sentinel");
        assert_eq!(heap.set_inherit_slot(obj, text, value, trap), NIL, "literal slot write on sentinel");
        assert_eq!(heap.set_value_vec(obj, key, value, trap), NIL, "vec write on sentinel");
        assert_eq!(heap.vec_push(obj, key, value, trap), NIL, "vec push on sentinel");
        for from in [source, obj] {
            assert_eq!(heap.vec_push_vec(obj, from, trap), NIL, "vec extension on sentinel");
            assert_eq!(heap.merge_object(obj, from, trap), NIL, "object merge on sentinel");
        }
        assert_eq!(heap.vec_push_vec_of_vec(obj, source, true, trap), NIL, "nested vec push on sentinel");
        heap.array_push(arr, value, trap);
        heap.array_push_vec(arr, source, trap);
        heap.merge_array(arr, values, trap);
        assert_eq!(heap.set_array_index(arr, 0, value, trap), NIL, "array write on sentinel");
        assert!(errors.err_is_empty(), "sentinel writes must leave the trap queue empty");
        assert!(heap.objects[obj].map.is_empty() && heap.objects[obj].vec.is_empty(),
            "sentinel writes must preserve empty object storage");
        assert_eq!(heap.array_len(arr), 0, "sentinel writes must preserve empty array storage");
    }

    #[test]
    fn shrinking_keeps_room_to_sweep_each_table() {
        let mut heap = ScriptHeap::empty();
        for name in ["kept pattern", "dead pattern"] {
            let values = [
                heap.new_object().into(),
                heap.new_array().into(),
                heap.new_string_from_str(name),
                heap.new_regex(name, "").unwrap(),
            ];
            if name == "kept pattern" {
                for value in values { heap.set_static(value); }
            }
        }
        heap.sweep(false);
        heap.shrink_to_fit();
        for (table, free, len) in [
            ("objects", heap.objects_free.capacity(), heap.objects.len()),
            ("strings", heap.strings_free.capacity(), heap.strings.len()),
            ("arrays", heap.arrays_free.capacity(), heap.arrays.len()),
            ("pods", heap.pods_free.capacity(), heap.pods.len()),
            ("pod types", heap.pod_types_free.capacity(), heap.pod_types.len()),
            ("handles", heap.handles_free.capacity(), heap.handles.len()),
            ("regexes", heap.regexes_free.capacity(), heap.regexes.len()),
        ] {
            assert!(free + 1 >= len, "{table}: shrunk free list has capacity {free} for {len} table slots");
        }
    }

    #[test]
    fn a_full_table_refuses_as_too_many_live_values() {
        use crate::gen_index::TEST_SLOT_LIMIT;
        let mut host = ScriptVmHost::new((), ());
        let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
        let heap = &vm.bx.heap;
        for (table, len, free) in [
            ("objects", heap.objects.len(), heap.objects_free.capacity()),
            ("arrays", heap.arrays.len(), heap.arrays_free.capacity()),
            ("strings", heap.strings.len(), heap.strings_free.capacity()),
        ] {
            // Slot 0 is never freed.
            assert!(free + 1 >= len, "{table}: the free list can take every slot, so a sweep never grows it");
        }
        let poison = heap.allocation_poison_object;
        assert!(poison != ScriptObject::ZERO, "the refused-value sentinels exist before any refusal can");

        // Room for 64 more values of each kind than the heap holds now.
        let limit = heap.objects.len().max(heap.arrays.len()).max(heap.strings.len()) + 64;
        TEST_SLOT_LIMIT.with(|l| l.set(Some(limit)));
        vm.capture_errors();
        let chain = vm.with_instruction_limit(100_000, |vm| eval(vm, "full_table", "let head = nil\nloop { head = {next: head} }"));
        let refusal = vm.bx.heap.take_memory_refusal();
        let errors = vm.take_errors();
        // A host call outside any run gets its refusal at once (after any free slots are reused).
        let refused = (0..=limit).map(|_| vm.bx.heap.new_object()).find(|object| *object == poison);
        let host_refusal = vm.bx.heap.take_memory_refusal();
        TEST_SLOT_LIMIT.with(|l| l.set(None));

        assert!(chain.is_err(), "the chain that filled the table ended as an error");
        assert_eq!(refusal.map(|r| r.reason), Some(TOO_MANY_LIVE_VALUES), "the run names the full table");
        assert!(errors.is_empty(), "the refusal formats no diagnostic: {errors:?}");
        assert_eq!(refused, Some(poison), "a refused host object is the inert sentinel");
        assert_eq!(host_refusal.map(|r| r.reason), Some(TOO_MANY_LIVE_VALUES), "the host call gets its refusal");
        assert_eq!(eval(&mut vm, "after_full", "6 * 7").as_number(), Some(42.0), "the VM evaluates again");
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
    fn object_fields_keep_insertion_order_through_iteration_and_json() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = ScriptVm {
            host: host.host_context(),
            bx: Box::new(ScriptVmBase::new()),
        };
        // Cross the spill threshold on the same object, preserving its surviving fields.
        let code = r#"
            fn snapshot(o) {
                let pairs = []
                for k, v in o { pairs.push([k.to_string(), v]) }
                [pairs.to_json(), o.to_json()]
            }
            let o = {a: 1, b: 2, c: 3, d: 4}
            let states = [snapshot(o)]
            o.a = 10
            o.b = 20
            states.push(snapshot(o))
            o.delete(@b)
            o.delete(@c)
            o.e = 5
            o.b = 6
            states.push(snapshot(o))
            for i in 20 { o["key" + i] = i }
            o.key2 = 42
            o.delete(@key0)
            o.delete(@a)
            o.f = 7
            o.key0 = 8
            states.push(snapshot(o))
            let big = {}
            for i in 4096 { big["field" + i] = i }
            big.delete(@field1)
            big.field1 = 777
            states.push(snapshot(big))

            let changing = {}
            for i in 32 { changing["f" + i] = i }
            let keys = for k, v in changing { k }
            let seen = []
            for k, v in changing {
                seen.push(v)
                changing.delete(k)
                if v == 2 {
                    for i in 3..20 { changing.delete(keys[i]) }
                    changing.extra = 32
                }
            }
            states.push(seen.to_json())
            for i in 32 { changing[keys[i]] = i }
            let inserts = []
            let nested = []
            for k, v in changing {
                inserts.push(v)
                if v == 2 {
                    for nk, nv in changing {
                        nested.push(nv)
                        if nv == 0 {
                            for i in 4..24 { changing.delete(keys[i]) }
                            changing.extra = 32
                            for i in 25..32 { changing.delete(keys[i]) }
                            changing.more = 33
                        }
                    }
                }
            }
            states.push(inserts.to_json())
            states.push(nested.to_json())
            states
        "#;
        vm.capture_errors();
        let result = eval(&mut vm, "insertion-order.octoscript", code);
        assert_eq!(vm.take_errors(), Vec::<String>::new(), "order script must run");
        let states = result.as_array().unwrap();
        let heap = &vm.bx.heap;
        let check = |i, fields: &[(&str, usize)]| {
            let state = heap.array_index(states, i, NoTrap).as_array().unwrap();
            let pairs = fields.iter().map(|(k, v)| format!(r#"["{k}",{v}]"#)).collect::<Vec<_>>().join(",");
            let json = fields.iter().map(|(k, v)| format!(r#""{k}":{v}"#)).collect::<Vec<_>>().join(",");
            assert_eq!(string_of(&vm, heap.array_index(state, 0, NoTrap)).unwrap(), format!("[{pairs}]"), "for at step {i}");
            assert_eq!(string_of(&vm, heap.array_index(state, 1, NoTrap)).unwrap(), format!("{{{json}}}"), "JSON at step {i}");
        };
        check(0, &[("a", 1), ("b", 2), ("c", 3), ("d", 4)]);
        check(1, &[("a", 10), ("b", 20), ("c", 3), ("d", 4)]);
        check(2, &[("a", 10), ("d", 4), ("e", 5), ("b", 6)]);
        let names: Vec<_> = (1..20).map(|i| format!("key{i}")).collect();
        let mut spilled = vec![("d", 4), ("e", 5), ("b", 6)];
        spilled.extend(names.iter().zip(1..20).map(|(name, i)| (name.as_str(), if i == 2 { 42 } else { i })));
        spilled.extend([("f", 7), ("key0", 8)]);
        check(3, &spilled);

        let names: Vec<_> = (0..4096).map(|i| format!("field{i}")).collect();
        let mut big: Vec<_> = names.iter().enumerate().filter(|(i, _)| *i != 1)
            .map(|(i, name)| (name.as_str(), i)).collect();
        big.push(("field1", 777));
        check(4, &big);
        assert_eq!(string_of(&vm, heap.array_index(states, 5, NoTrap)).unwrap(),
            "[0,1,2,20,21,22,23,24,25,26,27,28,29,30,31,32]",
            "deleting the current entry and compacting must keep the next survivor");
        assert_eq!(string_of(&vm, heap.array_index(states, 6, NoTrap)).unwrap(),
            "[0,1,2,3,24,32,33]",
            "a suspended outer cursor must survive two compactions and visit both appends");
        assert_eq!(string_of(&vm, heap.array_index(states, 7, NoTrap)).unwrap(),
            "[0,1,2,3,24,32,33]", "nested cursors keep their own positions");

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

        let mut map = ScriptObjectMap::default();
        for i in 0..32 {
            map.insert(ScriptValue::from_f64(i as f64), ScriptMapValue::default());
        }
        for i in 0..24 { map.remove(&ScriptValue::from_f64(i as f64)); }
        assert!(map.hole_count() > map.len(), "deletes leave holes until an append");
        for i in 0..24 {
            map.insert_absent(ScriptValue::from_f64(i as f64), ScriptMapValue::default());
            assert!(map.hole_count() <= map.len(), "hole budget after insertion {i}");
        }
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
