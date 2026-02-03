use crate::array::*;
use crate::function::*;
use crate::handle::*;
use crate::heap::*;
use crate::makepad_error_log::*;
use crate::object::*;
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
}

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
            if let Some(str_data) = $self.strings[ptr.index as usize].as_mut() {
                str_data.tag.set_static();
            }
        } else if let Some(ptr) = $val.as_array() {
            $self.mark_vec.push(ScriptGcMark::Array(ptr));
        } else if let Some(ptr) = $val.as_pod() {
            $self.pods[ptr.index as usize].tag.set_static();
        } else if let Some(ptr) = $val.as_handle() {
            if let Some(handle_data) = $self.handles[ptr.index as usize].as_mut() {
                handle_data.tag.set_static();
            }
        }
    };
}

// Set static with check - used outside closures where we can check is_static
macro_rules! set_static_val {
    ($self:ident, $val:expr) => {
        if let Some(ptr) = $val.as_object() {
            if !$self.objects[ptr.index as usize].tag.is_static() {
                $self.mark_vec.push(ScriptGcMark::Object(ptr));
            }
        } else if let Some(ptr) = $val.as_string() {
            if let Some(str_data) = $self.strings[ptr.index as usize].as_mut() {
                str_data.tag.set_static();
            }
        } else if let Some(ptr) = $val.as_array() {
            if !$self.arrays[ptr.index as usize].tag.is_static() {
                $self.mark_vec.push(ScriptGcMark::Array(ptr));
            }
        } else if let Some(ptr) = $val.as_pod() {
            $self.pods[ptr.index as usize].tag.set_static();
        } else if let Some(ptr) = $val.as_handle() {
            if let Some(handle_data) = $self.handles[ptr.index as usize].as_mut() {
                handle_data.tag.set_static();
            }
        }
    };
}

macro_rules! mark {
    ($self:ident, $val:expr) => {
        if let Some(ptr) = $val.as_object() {
            $self.mark_vec.push(ScriptGcMark::Object(ptr));
        } else if let Some(ptr) = $val.as_string() {
            if let Some(str_data) = $self.strings[ptr.index as usize].as_mut() {
                if !str_data.tag.is_static() {
                    str_data.tag.set_mark();
                }
            } else {
                error!(
                    "GC: dangling reference to unallocated string at index {}",
                    ptr.index
                );
            }
        } else if let Some(ptr) = $val.as_array() {
            $self.mark_vec.push(ScriptGcMark::Array(ptr));
        } else if let Some(ptr) = $val.as_pod() {
            let pod = &mut $self.pods[ptr.index as usize];
            if pod.tag.is_static() {
                // skip
            } else if !pod.tag.is_alloced() {
                error!(
                    "GC: dangling reference to unallocated pod at index {}",
                    ptr.index
                );
            } else {
                pod.tag.set_mark();
            }
        } else if let Some(ptr) = $val.as_handle() {
            if let Some(handle_data) = $self.handles[ptr.index as usize].as_mut() {
                if !handle_data.tag.is_static() {
                    handle_data.tag.set_mark();
                }
            } else {
                error!(
                    "GC: dangling reference to unallocated handle at index {}",
                    ptr.index
                );
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
            self.set_static_inner(self.mark_vec[i]);
            i += 1;
        }
    }

    fn set_static_inner(&mut self, value: ScriptGcMark) {
        match value {
            ScriptGcMark::Object(obj) => {
                let object = &mut self.objects[obj.index as usize];
                // Skip if already static or not allocated
                if object.tag.is_static() {
                    return;
                }
                if !object.tag.is_alloced() {
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
                    let object = &self.objects[obj.index as usize];
                    let key = object.vec[j].key;
                    let val = object.vec[j].value;
                    set_static_val!(self, key);
                    set_static_val!(self, val);
                }
            }
            ScriptGcMark::Array(arr) => {
                let array = &mut self.arrays[arr.index as usize];
                // Skip if already static or not allocated
                if array.tag.is_static() || !array.tag.is_alloced() {
                    return;
                }
                array.tag.set_static();

                // Queue all referenced values
                if let ScriptArrayStorage::ScriptValue(values) =
                    &self.arrays[arr.index as usize].storage
                {
                    for v in values {
                        set_static_val!(self, v);
                    }
                }
            }
        }
    }

    pub fn new_object_ref(&mut self, obj: ScriptObject) -> ScriptObjectRef {
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

    pub fn mark_inner(&mut self, value: ScriptGcMark) {
        match value {
            ScriptGcMark::Object(obj) => {
                let object = &mut self.objects[obj.index as usize];
                // Skip if already marked or static (permanent)
                if object.tag.is_marked() || object.tag.is_static() {
                    return;
                }
                // Dangling reference to unallocated object is a hard bug
                if !object.tag.is_alloced() {
                    error!(
                        "GC: dangling reference to unallocated object at index {}",
                        obj.index
                    );
                    return;
                }
                object.tag.set_mark();
                // Mark proto chain
                let proto = object.proto;
                mark!(self, proto);
                object.map_iter(|key, value| {
                    mark!(self, key);
                    mark!(self, value);
                });
                let len = object.vec.len();
                for i in 0..len {
                    let object = &self.objects[obj.index as usize];
                    let kv = &object.vec[i];
                    mark!(self, kv.key);
                    mark!(self, kv.value);
                }
            }
            ScriptGcMark::Array(arr) => {
                let tag = &self.arrays[arr.index as usize].tag;
                // Skip if already marked or static (permanent)
                if tag.is_marked() || tag.is_static() {
                    return;
                }
                // Dangling reference to unallocated array is a hard bug
                if !tag.is_alloced() {
                    error!(
                        "GC: dangling reference to unallocated array at index {}",
                        arr.index
                    );
                    return;
                }
                self.arrays[arr.index as usize].tag.set_mark();
                if let ScriptArrayStorage::ScriptValue(values) =
                    &self.arrays[arr.index as usize].storage
                {
                    for v in values {
                        mark!(self, v);
                    }
                }
            }
        }
    }

    pub fn mark(&mut self, threads: &ScriptThreads, code: &ScriptCode) {
        self.mark_vec.clear();
        for i in 0..self.type_check.len() {
            if let Some(object) = &self.type_check[i].object {
                if let Some(object) = object.proto.as_object() {
                    self.mark_inner(ScriptGcMark::Object(object));
                }
            }
        }
        // Mark type_defaults objects
        for obj in self.type_defaults.values() {
            self.mark_vec.push(ScriptGcMark::Object(*obj));
        }
        // Mark pod_types default values and objects
        for pod_type in &self.pod_types {
            mark!(self, pod_type.default);
            if pod_type.object != ScriptObject::ZERO {
                self.mark_vec.push(ScriptGcMark::Object(pod_type.object));
            }
        }
        let roots = self.root_objects.borrow();
        for item in roots.keys() {
            self.mark_vec.push(ScriptGcMark::Object(*item));
        }
        drop(roots);
        let roots = self.root_arrays.borrow();
        for item in roots.keys() {
            self.mark_vec.push(ScriptGcMark::Array(*item));
        }
        drop(roots);
        let roots = self.root_handles.borrow();
        for item in roots.keys() {
            self.handles[item.index as usize]
                .as_mut()
                .unwrap()
                .tag
                .set_mark();
        }
        drop(roots);

        // Mark thread stacks directly
        for i in 0..threads.len() {
            if let Some(thread) = threads.get(i) {
                // Stack values
                for value in &thread.stack {
                    mark!(self, value);
                }
                // Scopes
                for scope in &thread.scopes {
                    self.mark_vec.push(ScriptGcMark::Object(*scope));
                }
                // Method call contexts
                for me in &thread.mes {
                    match me {
                        ScriptMe::Object(obj) => self.mark_vec.push(ScriptGcMark::Object(*obj)),
                        ScriptMe::Call { sself, args } => {
                            if let Some(s) = sself {
                                mark!(self, s);
                            }
                            self.mark_vec.push(ScriptGcMark::Object(*args));
                        }
                        ScriptMe::Pod { pod, .. } => {
                            self.pods[pod.index as usize].tag.set_mark();
                        }
                        ScriptMe::Array(arr) => self.mark_vec.push(ScriptGcMark::Array(*arr)),
                    }
                }
                // Loop sources
                for loop_frame in &thread.loops {
                    if let Some(loop_values) = &loop_frame.values {
                        mark!(self, loop_values.source);
                    }
                }
                // Trap error values and return values
                for err in thread.trap.err.borrow().iter() {
                    mark!(self, err.value);
                }
                if let Some(ScriptTrapOn::Return(v)) = thread.trap.on.get() {
                    mark!(self, v);
                }
            }
        }

        // Mark ScriptBody scope and me objects
        for body in code.bodies.borrow().iter() {
            self.mark_vec.push(ScriptGcMark::Object(body.scope));
            self.mark_vec.push(ScriptGcMark::Object(body.me));
            // ScriptMod values contain ScriptValues passed from Rust code via #(...)
            if let ScriptSource::Mod(script_mod) = &body.source {
                for v in &script_mod.values {
                    mark!(self, v);
                }
            }
        }

        // Mark ScriptNative type_table objects
        for type_map in code.native.borrow().type_table.iter() {
            for (_, obj) in type_map.iter() {
                self.mark_vec.push(ScriptGcMark::Object(*obj));
            }
        }

        // Use while loop since mark_inner adds to mark_vec
        let mut i = 0;
        while i < self.mark_vec.len() {
            self.mark_inner(self.mark_vec[i]);
            i += 1;
        }
    }

    pub fn sweep(&mut self, start: std::time::Instant) {
        // GC stats: (static, alive, removed)
        let (mut obj_static, mut obj_alive, mut obj_removed) = (0usize, 0usize, 0usize);
        let (mut arr_static, mut arr_alive, mut arr_removed) = (0usize, 0usize, 0usize);
        let (mut str_static, mut str_alive, mut str_removed) = (0usize, 0usize, 0usize);
        let (mut hdl_static, mut hdl_alive, mut hdl_removed) = (0usize, 0usize, 0usize);
        let (mut pod_static, mut pod_alive, mut pod_removed) = (0usize, 0usize, 0usize);

        for i in 1..self.objects.len() {
            let obj = &mut self.objects[i];
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
                self.objects_free.push(ScriptObject { index: i as _ });
                obj_removed += 1;
            } else {
                if obj.tag.is_alloced() {
                    obj_alive += 1;
                }
                obj.tag.clear_mark();
            }
        }
        for i in 1..self.arrays.len() {
            let array = &mut self.arrays[i];
            // Skip static arrays - they are permanent
            if array.tag.is_static() {
                arr_static += 1;
                continue;
            }
            if !array.tag.is_marked() && array.tag.is_alloced() {
                array.clear();
                self.arrays_free.push(ScriptArray { index: i as _ });
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
            if let Some(str) = &mut self.strings[i] {
                // Skip static strings - they are permanent
                if str.tag.is_static() {
                    str_static += 1;
                    continue;
                }
                if !str.tag.is_marked() {
                    if let Some((k, _)) = self.string_intern.remove_entry(&str.string) {
                        self.strings[i] = None;
                        if let Some(s) = Arc::into_inner(k.0) {
                            self.strings_reuse.push(s);
                        }
                        self.strings_free.push(ScriptString { index: i as _ });
                        str_removed += 1;
                    }
                } else {
                    str_alive += 1;
                    str.tag.clear_mark();
                }
            }
        }
        for i in 1..self.handles.len() {
            if let Some(handle) = &mut self.handles[i] {
                // Skip static handles - they are permanent
                if handle.tag.is_static() {
                    hdl_static += 1;
                    continue;
                }
                if !handle.tag.is_marked() {
                    let handle = self.handles[i].take().unwrap();
                    handle.gc();
                    hdl_removed += 1;
                } else {
                    hdl_alive += 1;
                    handle.tag.clear_mark();
                }
            }
        }
        for i in 1..self.pods.len() {
            let pod = &mut self.pods[i];
            // Skip static pods - they are permanent
            if pod.tag.is_static() {
                pod_static += 1;
                continue;
            }
            if !pod.tag.is_marked() && pod.tag.is_alloced() {
                pod.clear();
                self.pods_free.push(ScriptPod { index: i as _ });
                pod_removed += 1;
            } else {
                if pod.tag.is_alloced() {
                    pod_alive += 1;
                }
                pod.tag.clear_mark();
            }
        }

        // Print compact GC stats: S=static A=alive R=removed
        log!("GC {}us: obj[S:{} A:{} R:{}] arr[S:{} A:{} R:{}] str[S:{} A:{} R:{}] hdl[S:{} A:{} R:{}] pod[S:{} A:{} R:{}]",
            start.elapsed().as_micros(),
            obj_static, obj_alive, obj_removed,
            arr_static, arr_alive, arr_removed,
            str_static, str_alive, str_removed,
            hdl_static, hdl_alive, hdl_removed,
            pod_static, pod_alive, pod_removed);

        // Record heap statistics after GC for triggering next cycle
        self.gc_last = ScriptHeapGcLast {
            objects: self.objects.len() - self.objects_free.len(),
            strings: self.strings.len() - self.strings_free.len(),
            arrays: self.arrays.len() - self.arrays_free.len(),
            pods: self.pods.len() - self.pods_free.len(),
            handles: self.handles.len() - self.handles_free.len(),
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

        false
    }

    pub fn free_object_if_unreffed(&mut self, ptr: ScriptObject) {
        let obj = &mut self.objects[ptr.index as usize];
        if !obj.tag.is_reffed() {
            if let Some(pod_ty) = obj.tag.as_pod_type() {
                self.pod_types_free.push(pod_ty);
            }
            obj.clear();
            self.objects_free.push(ptr);
        }
    }
}
