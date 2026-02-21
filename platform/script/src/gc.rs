use crate::array::*;
use crate::function::*;
use crate::handle::*;
use crate::heap::*;
use crate::makepad_error_log::*;

use crate::object::*;
use crate::regex::*;
use crate::thread::*;
use crate::trap::*;
use crate::value::*;
use crate::vm::*;

use std::collections::hash_map::Entry;
use std::sync::Arc;

/// Tracks heap statistics from the last garbage collection run.
/// Used to determine when to trigger the next GC cycle.
#[derive(Default, Clone, Copy)]
pub struct ScriptHeapGcLast {
    pub objects: usize,
    pub strings: usize,
    pub arrays: usize,
    pub pods: usize,
    pub handles: usize,
    pub regexes: usize,
}

/// Lightweight mark item for GC work list - just the reference, no debug info
#[derive(Copy, Clone)]
pub enum ScriptGcMark {
    Object(ScriptObject),
    Array(ScriptArray),
}

// Queue values for static marking - used inside map_iter closures where we can't check is_static
macro_rules! queue_static_val {
    ($self:ident, $val:expr) => {
        if let Some(ptr) = $val.as_object() {
            $self.mark_vec.push(ScriptGcMark::Object(ptr));
        } else if let Some(ptr) = $val.as_string() {
            if let Some(str_data) = $self.strings[ptr].as_mut() {
                str_data.tag.set_static();
            }
        } else if let Some(ptr) = $val.as_array() {
            $self.mark_vec.push(ScriptGcMark::Array(ptr));
        } else if let Some(ptr) = $val.as_pod() {
            $self.pods[ptr].tag.set_static();
        } else if let Some(ptr) = $val.as_handle() {
            if let Some(handle_data) = $self.handles[ptr].as_mut() {
                handle_data.tag.set_static();
            }
        } else if let Some(ptr) = $val.as_regex() {
            if let Some(regex_data) = $self.regexes[ptr].as_mut() {
                regex_data.tag.set_static();
            }
        }
    };
}

// Set static with check - used outside closures where we can check is_static
macro_rules! set_static_val {
    ($self:ident, $val:expr) => {
        if let Some(ptr) = $val.as_object() {
            if !$self.objects[ptr].tag.is_static() {
                $self.mark_vec.push(ScriptGcMark::Object(ptr));
            }
        } else if let Some(ptr) = $val.as_string() {
            if let Some(str_data) = $self.strings[ptr].as_mut() {
                str_data.tag.set_static();
            }
        } else if let Some(ptr) = $val.as_array() {
            if !$self.arrays[ptr].tag.is_static() {
                $self.mark_vec.push(ScriptGcMark::Array(ptr));
            }
        } else if let Some(ptr) = $val.as_pod() {
            $self.pods[ptr].tag.set_static();
        } else if let Some(ptr) = $val.as_handle() {
            if let Some(handle_data) = $self.handles[ptr].as_mut() {
                handle_data.tag.set_static();
            }
        } else if let Some(ptr) = $val.as_regex() {
            if let Some(regex_data) = $self.regexes[ptr].as_mut() {
                regex_data.tag.set_static();
            }
        }
    };
}

impl ScriptHeap {
    /// Recursively mark a value and all reachable values as static (permanent).
    /// This walks the object graph similar to GC marking but sets the static flag instead.
    pub fn set_static(&mut self, value: ScriptValue) {
        self.mark_vec.clear();

        // Initial value
        set_static_val!(self, value);

        // Process the work list - use while loop since set_static_inner adds to mark_vec
        let mut i = 0;
        while i < self.mark_vec.len() {
            let mark = self.mark_vec[i];
            self.set_static_inner(mark);
            i += 1;
        }
    }

    fn set_static_inner(&mut self, value: ScriptGcMark) {
        match value {
            ScriptGcMark::Object(obj) => {
                let object = &mut self.objects[obj];
                // Skip if already static or not allocated
                if object.tag.is_static() || !object.tag.is_alloced() {
                    return;
                }
                object.tag.set_static();

                // Queue all referenced values using macro (no is_static check inside closure)
                // Also queues proto chain
                let proto = object.proto;
                queue_static_val!(self, proto);
                object.map_iter(|key, val| {
                    queue_static_val!(self, key);
                    queue_static_val!(self, val);
                });
                let len = object.vec.len();
                for j in 0..len {
                    let object = &self.objects[obj];
                    let key = object.vec[j].key;
                    let val = object.vec[j].value;
                    set_static_val!(self, key);
                    set_static_val!(self, val);
                }
            }
            ScriptGcMark::Array(arr) => {
                let array = &mut self.arrays[arr];
                // Skip if already static or not allocated
                if array.tag.is_static() || !array.tag.is_alloced() {
                    return;
                }
                array.tag.set_static();

                // Queue all referenced values
                if let ScriptArrayStorage::ScriptValue(values) = &self.arrays[arr].storage {
                    for v in values {
                        set_static_val!(self, v);
                    }
                }
            }
        }
    }

    pub fn new_object_ref(&mut self, obj: ScriptObject) -> ScriptObjectRef {
        // Mark as reffed so free_object_if_unreffed won't free it
        self.objects[obj].tag.set_reffed();

        let mut roots = self.root_objects.borrow_mut();
        match roots.entry(obj) {
            Entry::Occupied(mut occ) => {
                *occ.get_mut() += 1;
                ScriptObjectRef {
                    roots: Some(self.root_objects.clone()),
                    obj: obj,
                }
            }
            Entry::Vacant(vac) => {
                vac.insert(1);
                ScriptObjectRef {
                    roots: Some(self.root_objects.clone()),
                    obj: obj,
                }
            }
        }
    }

    pub fn new_array_ref(&mut self, array: ScriptArray) -> ScriptArrayRef {
        let mut roots = self.root_arrays.borrow_mut();
        match roots.entry(array) {
            Entry::Occupied(mut occ) => {
                *occ.get_mut() += 1;
                ScriptArrayRef {
                    roots: self.root_arrays.clone(),
                    array: array,
                }
            }
            Entry::Vacant(vac) => {
                vac.insert(1);
                ScriptArrayRef {
                    roots: self.root_arrays.clone(),
                    array: array,
                }
            }
        }
    }

    pub fn new_fn_ref(&mut self, obj: ScriptObject) -> ScriptFnRef {
        ScriptFnRef(self.new_object_ref(obj))
    }

    pub fn new_handle_ref(&mut self, handle: ScriptHandle) -> ScriptHandleRef {
        let mut roots = self.root_handles.borrow_mut();
        match roots.entry(handle) {
            Entry::Occupied(mut occ) => {
                *occ.get_mut() += 1;
                ScriptHandleRef {
                    roots: self.root_handles.clone(),
                    handle: handle,
                }
            }
            Entry::Vacant(vac) => {
                vac.insert(1);
                ScriptHandleRef {
                    roots: self.root_handles.clone(),
                    handle: handle,
                }
            }
        }
    }

    pub fn mark_inner(&mut self, mark: ScriptGcMark) {
        match mark {
            ScriptGcMark::Object(obj) => {
                // Check flags and set mark
                let object = &mut self.objects[obj];
                // Skip if already marked or static (permanent) or not allocated
                if object.tag.is_marked() || object.tag.is_static() || !object.tag.is_alloced() {
                    return;
                }
                object.tag.set_mark();

                // Mark proto chain - get proto without holding borrow
                let proto = self.objects[obj].proto;
                self.mark_value(proto);

                // Mark map entries - queue objects/arrays for later, mark primitives directly
                for (key, map_val) in self.objects[obj].map.iter() {
                    // Queue key if object/array (keys are typically IDs, not heap refs)
                    if let Some(ptr) = key.as_object() {
                        self.mark_vec.push(ScriptGcMark::Object(ptr));
                    } else if let Some(ptr) = key.as_array() {
                        self.mark_vec.push(ScriptGcMark::Array(ptr));
                    }
                    // Queue/mark value
                    let val = map_val.value;
                    if let Some(ptr) = val.as_object() {
                        self.mark_vec.push(ScriptGcMark::Object(ptr));
                    } else if let Some(ptr) = val.as_array() {
                        self.mark_vec.push(ScriptGcMark::Array(ptr));
                    } else if let Some(ptr) = val.as_string() {
                        if let Some(str_data) = self.strings[ptr].as_mut() {
                            if !str_data.tag.is_static() {
                                str_data.tag.set_mark();
                            }
                        }
                    } else if let Some(ptr) = val.as_pod() {
                        let pod = &mut self.pods[ptr];
                        if !pod.tag.is_static() && pod.tag.is_alloced() {
                            pod.tag.set_mark();
                        }
                    } else if let Some(ptr) = val.as_handle() {
                        if ptr.index != 0 {
                            if let Some(handle_data) = self.handles[ptr].as_mut() {
                                if !handle_data.tag.is_static() {
                                    handle_data.tag.set_mark();
                                }
                            }
                        }
                    } else if let Some(ptr) = val.as_regex() {
                        if let Some(regex_data) = self.regexes[ptr].as_mut() {
                            if !regex_data.tag.is_static() {
                                regex_data.tag.set_mark();
                            }
                        }
                    }
                }

                // Mark vec entries
                let vec_len = self.objects[obj].vec.len();
                for i in 0..vec_len {
                    let object = &self.objects[obj];
                    let kv = &object.vec[i];
                    let key = kv.key;
                    let value = kv.value;
                    self.mark_value(key);
                    self.mark_value(value);
                }
            }
            ScriptGcMark::Array(arr) => {
                let tag = &self.arrays[arr].tag;
                // Skip if already marked or static (permanent) or not allocated
                if tag.is_marked() || tag.is_static() || !tag.is_alloced() {
                    return;
                }
                self.arrays[arr].tag.set_mark();

                // Mark array values by iterating indices to avoid Vec allocation
                if let ScriptArrayStorage::ScriptValue(values) = &self.arrays[arr].storage {
                    let len = values.len();
                    for i in 0..len {
                        if let ScriptArrayStorage::ScriptValue(values) = &self.arrays[arr].storage {
                            let v = values[i];
                            self.mark_value(v);
                        }
                    }
                }
            }
        }
    }

    /// Mark a single value - adds objects/arrays to work list, marks primitives directly
    #[inline]
    fn mark_value(&mut self, val: ScriptValue) {
        if let Some(ptr) = val.as_object() {
            self.mark_vec.push(ScriptGcMark::Object(ptr));
        } else if let Some(ptr) = val.as_string() {
            if let Some(str_data) = self.strings[ptr].as_mut() {
                if !str_data.tag.is_static() {
                    str_data.tag.set_mark();
                }
            }
        } else if let Some(ptr) = val.as_array() {
            self.mark_vec.push(ScriptGcMark::Array(ptr));
        } else if let Some(ptr) = val.as_pod() {
            let pod = &mut self.pods[ptr];
            if !pod.tag.is_static() && pod.tag.is_alloced() {
                pod.tag.set_mark();
            }
        } else if let Some(ptr) = val.as_handle() {
            // Skip handle index 0 - it's the "null" handle (ScriptHandle::ZERO)
            if ptr.index != 0 {
                if let Some(handle_data) = self.handles[ptr].as_mut() {
                    if !handle_data.tag.is_static() {
                        handle_data.tag.set_mark();
                    }
                }
            }
        } else if let Some(ptr) = val.as_regex() {
            if let Some(regex_data) = self.regexes[ptr].as_mut() {
                if !regex_data.tag.is_static() {
                    regex_data.tag.set_mark();
                }
            }
        }
    }

    pub fn mark(&mut self, threads: &ScriptThreads, code: &ScriptCode) {
        self.mark_vec.clear();

        // Mark type_check protos
        for i in 0..self.type_check.len() {
            if let Some(object) = &self.type_check[i].object {
                self.mark_value(object.proto);
            }
        }

        // Mark type_defaults objects
        for (_, obj) in &self.type_defaults {
            self.mark_vec.push(ScriptGcMark::Object(*obj));
        }

        // Mark pod_types default values and objects
        let pod_type_data: Vec<_> = self
            .pod_types
            .iter()
            .map(|pt| (pt.default, pt.object))
            .collect();
        for (default, pod_obj) in pod_type_data {
            self.mark_value(default);
            if pod_obj != ScriptObject::ZERO {
                self.mark_vec.push(ScriptGcMark::Object(pod_obj));
            }
        }

        // Mark root_objects
        let roots: Vec<_> = self.root_objects.borrow().keys().copied().collect();
        for item in roots {
            self.mark_vec.push(ScriptGcMark::Object(item));
        }

        // Mark root_arrays
        let roots: Vec<_> = self.root_arrays.borrow().keys().copied().collect();
        for item in roots {
            self.mark_vec.push(ScriptGcMark::Array(item));
        }

        // Mark root_handles directly
        let roots: Vec<_> = self.root_handles.borrow().keys().copied().collect();
        for item in roots {
            if let Some(handle_data) = self.handles[item].as_mut() {
                handle_data.tag.set_mark();
            }
        }

        // Mark thread stacks
        for thread_idx in 0..threads.len() {
            if let Some(thread) = threads.get(thread_idx) {
                // Stack values
                for value in thread.stack.iter() {
                    self.mark_value(*value);
                }
                // Scopes
                for scope in thread.scopes.iter() {
                    self.mark_vec.push(ScriptGcMark::Object(*scope));
                }
                // Method call contexts
                for me in thread.mes.iter() {
                    match me {
                        ScriptMe::Object(obj) => {
                            self.mark_vec.push(ScriptGcMark::Object(*obj));
                        }
                        ScriptMe::Call { sself, args, .. } => {
                            if let Some(s) = sself {
                                self.mark_value(*s);
                            }
                            self.mark_vec.push(ScriptGcMark::Object(*args));
                        }
                        ScriptMe::Pod { pod, .. } => {
                            self.pods[*pod].tag.set_mark();
                        }
                        ScriptMe::Array(arr) => {
                            self.mark_vec.push(ScriptGcMark::Array(*arr));
                        }
                    }
                }
                // Loop sources
                for loop_frame in thread.loops.iter() {
                    if let Some(loop_values) = &loop_frame.values {
                        self.mark_value(loop_values.source);
                    }
                }
                // Trap error values
                for err in thread.trap.err.borrow().iter() {
                    self.mark_value(err.value);
                }
                // Trap return/bail values
                match thread.trap.on.get() {
                    Some(ScriptTrapOn::Return(v)) | Some(ScriptTrapOn::Bail(v)) => {
                        self.mark_value(v);
                    }
                    _ => {}
                }
            }
        }

        // Mark ScriptBody scope and me objects
        for body in code.bodies.borrow().iter() {
            if let ScriptSource::Mod(script_mod) = &body.source {
                for v in script_mod.values.iter() {
                    self.mark_value(*v);
                }
            }

            // Mark tokenizer string literals as roots
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

        // Mark ScriptNative type_table objects
        for type_map in code.native.borrow().type_table.iter() {
            for (_, obj) in type_map.iter() {
                self.mark_vec.push(ScriptGcMark::Object(*obj));
            }
        }

        // Process the work list - use while loop since mark_inner adds to mark_vec
        let mut i = 0;
        while i < self.mark_vec.len() {
            let mark = self.mark_vec[i];
            self.mark_inner(mark);
            i += 1;
        }
    }

    pub fn sweep(&mut self, log_stats: bool) {
        #[cfg(not(target_arch = "wasm32"))]
        let start = std::time::Instant::now();

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
            if !obj.tag.is_marked() && obj.tag.is_alloced() {
                if let Some(pod_ty) = obj.tag.as_pod_type() {
                    self.pod_types_free.push(pod_ty);
                }
                obj.clear();
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
                obj.tag.clear_mark();
            }
        }
        for i in 1..self.arrays.len() {
            let array = &mut self.arrays.get_at_mut(i);
            // Skip static arrays - they are permanent
            if array.tag.is_static() {
                arr_static += 1;
                continue;
            }
            if !array.tag.is_marked() && array.tag.is_alloced() {
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
                array.tag.clear_mark();
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
                        if let Some(mut s) = Arc::into_inner(k.0) {
                            s.clear();
                            self.strings_reuse.push(s);
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
                        pattern: re.pattern.clone(),
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

        // Print compact GC stats: S=static A=alive R=removed
        #[cfg(not(target_arch = "wasm32"))]
        let elapsed_us = start.elapsed().as_micros();
        #[cfg(target_arch = "wasm32")]
        let elapsed_us = 0u128;
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

        // Record heap statistics after GC for triggering next cycle
        self.gc_last = ScriptHeapGcLast {
            objects: self.objects.len() - self.objects_free.len(),
            strings: self.strings.len() - self.strings_free.len(),
            arrays: self.arrays.len() - self.arrays_free.len(),
            pods: self.pods.len() - self.pods_free.len(),
            handles: self.handles.len() - self.handles_free.len(),
            regexes: self.regexes.len() - self.regexes_free.len(),
        };
    }

    /// Check if garbage collection should be triggered.
    ///
    /// Uses a growth-based heuristic similar to Lua and V8:
    /// - Trigger when any heap category has grown by 2x since last GC
    /// - Use minimum thresholds to avoid GC thrashing on small heaps
    /// - Objects are weighted more heavily as they're the primary allocation type
    pub fn needs_gc(&self) -> bool {
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

        // Check each category: must exceed minimum AND have grown by factor
        if objects >= MIN_OBJECTS && objects >= self.gc_last.objects * GROWTH_FACTOR {
            return true;
        }
        if strings >= MIN_STRINGS && strings >= self.gc_last.strings * GROWTH_FACTOR {
            return true;
        }
        if arrays >= MIN_ARRAYS && arrays >= self.gc_last.arrays * GROWTH_FACTOR {
            return true;
        }
        if pods >= MIN_PODS && pods >= self.gc_last.pods * GROWTH_FACTOR {
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

    pub fn gc_live_len(&self) -> usize {
        let objects = self.objects.len() - self.objects_free.len();
        let strings = self.strings.len() - self.strings_free.len();
        let arrays = self.arrays.len() - self.arrays_free.len();
        let pods = self.pods.len() - self.pods_free.len();
        let handles = self.handles.len() - self.handles_free.len();
        let regexes = self.regexes.len() - self.regexes_free.len();
        objects + strings + arrays + pods + handles + regexes
    }

    pub fn free_object_if_unreffed(&mut self, ptr: ScriptObject) {
        // Check if reference is still valid (may have been freed by GC)
        if !self.objects.is_valid(ptr) {
            return;
        }
        let obj = &mut self.objects[ptr];
        // Must check is_alloced to avoid double-freeing
        if obj.tag.is_alloced() && !obj.tag.is_reffed() {
            if let Some(pod_ty) = obj.tag.as_pod_type() {
                self.pod_types_free.push(pod_ty);
            }
            obj.clear();
            // Increment generation so stale references will be detected
            self.objects.free_slot(ptr.index);
            // Push ref with NEW generation to free list - ready to reuse
            let new_gen = self.objects.generation(ptr.index as usize);
            self.objects_free
                .push(ScriptObject::new(ptr.index, new_gen));
        }
    }
}
