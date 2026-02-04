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

/// Describes the root source that initiated a GC marking chain
#[derive(Clone, Debug)]
pub enum ScriptGcRoot {
    TypeCheckProto(usize), // type_check[i].object.proto
    TypeDefault(u32),      // type_defaults object index
    PodTypeDefault(usize), // pod_types[i].default
    PodTypeObject(usize),  // pod_types[i].object
    RootObject(u32),       // root_objects entry, object index
    RootArray(u32),        // root_arrays entry, array index
    ThreadStack {
        thread: usize,
        stack_idx: usize,
    },
    ThreadScope {
        thread: usize,
        scope_idx: usize,
    },
    ThreadMe {
        thread: usize,
        me_idx: usize,
        kind: &'static str,
    },
    ThreadLoop {
        thread: usize,
        loop_idx: usize,
    },
    ThreadTrapErr {
        thread: usize,
        err_idx: usize,
    },
    ThreadTrapReturn(usize),
    ScriptBodyValue {
        body_idx: usize,
        value_idx: usize,
    },
    TokenizerString {
        body_idx: usize,
        token_idx: usize,
    },
    NativeTypeTable {
        table_idx: usize,
    },
    Child, // Added as child of another object
}

impl Default for ScriptGcRoot {
    fn default() -> Self {
        ScriptGcRoot::Child
    }
}

#[derive(Clone)]
pub struct ScriptGcMark {
    pub kind: ScriptGcMarkKind,
    pub root: ScriptGcRoot,
    pub parent: Option<u32>, // Parent object index that referenced this
}

#[derive(Copy, Clone)]
pub enum ScriptGcMarkKind {
    Object(ScriptObject),
    Array(ScriptArray),
}

// Queue values for static marking - used inside map_iter closures where we can't check is_static
macro_rules! queue_static_val {
    ($self:ident, $val:expr) => {
        if let Some(ptr) = $val.as_object() {
            $self.mark_vec.push(ScriptGcMark {
                kind: ScriptGcMarkKind::Object(ptr),
                root: ScriptGcRoot::Child,
                parent: None,
            });
        } else if let Some(ptr) = $val.as_string() {
            if let Some(str_data) = $self.strings[ptr].as_mut() {
                str_data.tag.set_static();
            }
        } else if let Some(ptr) = $val.as_array() {
            $self.mark_vec.push(ScriptGcMark {
                kind: ScriptGcMarkKind::Array(ptr),
                root: ScriptGcRoot::Child,
                parent: None,
            });
        } else if let Some(ptr) = $val.as_pod() {
            $self.pods[ptr].tag.set_static();
        } else if let Some(ptr) = $val.as_handle() {
            if let Some(handle_data) = $self.handles[ptr].as_mut() {
                handle_data.tag.set_static();
            }
        }
    };
}

// Set static with check - used outside closures where we can check is_static
macro_rules! set_static_val {
    ($self:ident, $val:expr) => {
        if let Some(ptr) = $val.as_object() {
            if !$self.objects[ptr].tag.is_static() {
                $self.mark_vec.push(ScriptGcMark {
                    kind: ScriptGcMarkKind::Object(ptr),
                    root: ScriptGcRoot::Child,
                    parent: None,
                });
            }
        } else if let Some(ptr) = $val.as_string() {
            if let Some(str_data) = $self.strings[ptr].as_mut() {
                str_data.tag.set_static();
            }
        } else if let Some(ptr) = $val.as_array() {
            if !$self.arrays[ptr].tag.is_static() {
                $self.mark_vec.push(ScriptGcMark {
                    kind: ScriptGcMarkKind::Array(ptr),
                    root: ScriptGcRoot::Child,
                    parent: None,
                });
            }
        } else if let Some(ptr) = $val.as_pod() {
            $self.pods[ptr].tag.set_static();
        } else if let Some(ptr) = $val.as_handle() {
            if let Some(handle_data) = $self.handles[ptr].as_mut() {
                handle_data.tag.set_static();
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
            let mark = self.mark_vec[i].clone();
            self.set_static_inner(mark);
            i += 1;
        }
    }

    fn set_static_inner(&mut self, value: ScriptGcMark) {
        match value.kind {
            ScriptGcMarkKind::Object(obj) => {
                let object = &mut self.objects[obj];
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
                    let object = &self.objects[obj];
                    let key = object.vec[j].key;
                    let val = object.vec[j].value;
                    set_static_val!(self, key);
                    set_static_val!(self, val);
                }
            }
            ScriptGcMarkKind::Array(arr) => {
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
        // Update tracking context from the mark item
        if !matches!(mark.root, ScriptGcRoot::Child) {
            // This is a root item - start fresh tracking
            self.gc_root_source = format!("{:?}", mark.root);
            self.gc_parent_chain.clear();
        }
        if let Some(parent) = mark.parent {
            // Add parent to chain if not already there
            if self.gc_parent_chain.last() != Some(&parent) {
                self.gc_parent_chain.push(parent);
            }
        }

        match mark.kind {
            ScriptGcMarkKind::Object(obj) => {
                // First check flags and set mark - release borrow immediately
                {
                    let object = &mut self.objects[obj];
                    // Skip if already marked or static (permanent)
                    if object.tag.is_marked() || object.tag.is_static() {
                        return;
                    }
                    // Dangling reference to unallocated object is a hard bug
                    if !object.tag.is_alloced() {
                        self.log_dangling_error("object", obj.index as usize);
                        return;
                    }
                    object.tag.set_mark();
                }

                // Set current parent for debugging child references
                let prev_parent = self.current_parent;
                self.current_parent = Some(obj);

                // Mark proto chain - get proto without holding borrow
                let proto = self.objects[obj].proto;
                self.mark_value(proto);

                // Collect map entries first to avoid borrow conflict
                let mut map_values: Vec<(ScriptValue, ScriptValue)> = Vec::new();
                self.objects[obj].map_iter(|key, value| {
                    map_values.push((key, value));
                });
                // Now mark them
                for (key, value) in map_values {
                    self.mark_value(key);
                    self.mark_value(value);
                }

                // Mark vec entries
                let len = self.objects[obj].vec.len();
                for i in 0..len {
                    let object = &self.objects[obj];
                    let kv = &object.vec[i];
                    let key = kv.key;
                    let value = kv.value;
                    self.mark_value(key);
                    self.mark_value(value);
                }

                // Restore previous parent
                self.current_parent = prev_parent;
            }
            ScriptGcMarkKind::Array(arr) => {
                let tag = &self.arrays[arr].tag;
                // Skip if already marked or static (permanent)
                if tag.is_marked() || tag.is_static() {
                    return;
                }
                // Dangling reference to unallocated array is a hard bug
                if !tag.is_alloced() {
                    self.log_dangling_error("array", arr.index as usize);
                    return;
                }
                self.arrays[arr].tag.set_mark();

                // Collect array values first to avoid borrow conflict
                let values: Vec<ScriptValue> =
                    if let ScriptArrayStorage::ScriptValue(values) = &self.arrays[arr].storage {
                        values.iter().copied().collect()
                    } else {
                        Vec::new()
                    };

                for v in values {
                    self.mark_value(v);
                }
            }
        }
    }

    /// Mark a single value - extracted to avoid macro issues with borrows
    #[inline]
    fn mark_value(&mut self, val: ScriptValue) {
        if let Some(ptr) = val.as_object() {
            self.mark_vec.push(ScriptGcMark {
                kind: ScriptGcMarkKind::Object(ptr),
                root: ScriptGcRoot::Child,
                parent: self.current_parent.map(|p| p.index),
            });
        } else if let Some(ptr) = val.as_string() {
            if let Some(str_data) = self.strings[ptr].as_mut() {
                if !str_data.tag.is_static() {
                    str_data.tag.set_mark();
                }
            } else {
                self.log_dangling_error("string", ptr.index as usize);
            }
        } else if let Some(ptr) = val.as_array() {
            self.mark_vec.push(ScriptGcMark {
                kind: ScriptGcMarkKind::Array(ptr),
                root: ScriptGcRoot::Child,
                parent: self.current_parent.map(|p| p.index),
            });
        } else if let Some(ptr) = val.as_pod() {
            let pod = &mut self.pods[ptr];
            if pod.tag.is_static() {
                // skip
            } else if !pod.tag.is_alloced() {
                self.log_dangling_error("pod", ptr.index as usize);
            } else {
                pod.tag.set_mark();
            }
        } else if let Some(ptr) = val.as_handle() {
            // Skip handle index 0 - it's the "null" handle (ScriptHandle::ZERO)
            if ptr.index == 0 {
                return;
            }
            if let Some(handle_data) = self.handles[ptr].as_mut() {
                if !handle_data.tag.is_static() {
                    handle_data.tag.set_mark();
                }
            } else {
                self.log_dangling_error("handle", ptr.index as usize);
            }
        }
    }

    /// Log a comprehensive error when a dangling reference is found during GC.
    /// This shows the root source, the full parent chain, and details about each object.
    pub fn log_dangling_error(&self, kind: &str, index: usize) {
        let mut msg = format!(
            "\n========== GC DANGLING REFERENCE ERROR ==========\n\
             Dangling {} reference at index {}\n\n\
             Root source: {}\n\n\
             Parent chain ({} objects):\n",
            kind,
            index,
            self.gc_root_source,
            self.gc_parent_chain.len()
        );

        for (i, &parent_idx) in self.gc_parent_chain.iter().enumerate() {
            msg.push_str(&format!("  [{}] ", i));
            msg.push_str(&self.format_object_debug(parent_idx));
            msg.push('\n');
        }

        if let Some(current) = self.current_parent {
            msg.push_str(&format!(
                "\nImmediate parent (current object being processed):\n  "
            ));
            msg.push_str(&self.format_object_debug(current.index));
            msg.push('\n');
        }

        msg.push_str("==================================================\n");
        error!("{}", msg);
    }

    /// Format debug info for a single object by index
    fn format_object_debug(&self, index: u32) -> String {
        if (index as usize) >= self.objects.len() {
            return format!("Object {} (out of bounds!)", index);
        }

        let object = self.objects.get_at(index as usize);
        let mut info = format!("Object {}", index);

        // Add allocation status
        if !object.tag.is_alloced() {
            info.push_str(" [NOT ALLOCATED]");
            return info;
        }
        if object.tag.is_static() {
            info.push_str(" [STATIC]");
        }

        // Add type info if available
        if let Some(ty_index) = object.tag.as_type_index() {
            if let Some(type_check) = self.type_check.get(ty_index.0 as usize) {
                if let Some(obj_info) = &type_check.object {
                    info.push_str(&format!(" type={:?}", obj_info.type_id));
                }
            }
        }

        // Add proto info
        if let Some(proto_id) = object.proto.as_id() {
            proto_id.as_string(|s| {
                if let Some(s) = s {
                    info.push_str(&format!(" proto={}", s));
                }
            });
        } else if let Some(proto_obj) = object.proto.as_object() {
            info.push_str(&format!(" proto=Object({})", proto_obj.index));
        }

        // Add first few keys to help identify the object
        let mut keys: Vec<String> = Vec::new();
        object.map_iter(|key, _value| {
            if keys.len() < 8 {
                if let Some(id) = key.as_id() {
                    id.as_string(|s| {
                        if let Some(s) = s {
                            keys.push(s.to_string());
                        }
                    });
                } else if key.is_string_like() {
                    key.as_inline_string(|s| {
                        keys.push(format!("\"{}\"", s));
                    });
                } else {
                    keys.push(format!("{}", key));
                }
            }
        });
        if !keys.is_empty() {
            info.push_str(&format!(" keys=[{}]", keys.join(", ")));
        }

        // Add vec info
        if !object.vec.is_empty() {
            info.push_str(&format!(" vec_len={}", object.vec.len()));
            // Show first few vec keys
            let mut vec_keys: Vec<String> = Vec::new();
            for kv in object.vec.iter().take(5) {
                if let Some(id) = kv.key.as_id() {
                    id.as_string(|s| {
                        if let Some(s) = s {
                            vec_keys.push(format!("${}", s));
                        }
                    });
                }
            }
            if !vec_keys.is_empty() {
                info.push_str(&format!(" vec_keys=[{}]", vec_keys.join(", ")));
            }
        }

        info
    }

    pub fn mark(&mut self, threads: &ScriptThreads, code: &ScriptCode) {
        self.mark_vec.clear();
        self.current_parent = None;
        self.gc_root_source.clear();
        self.gc_parent_chain.clear();

        // Helper macro to push with root tracking
        macro_rules! push_root {
            ($kind:expr, $root:expr) => {
                self.mark_vec.push(ScriptGcMark {
                    kind: $kind,
                    root: $root,
                    parent: None,
                });
            };
        }

        // Mark type_check protos
        for i in 0..self.type_check.len() {
            if let Some(object) = &self.type_check[i].object {
                let proto = object.proto;
                if let Some(ptr) = proto.as_object() {
                    push_root!(
                        ScriptGcMarkKind::Object(ptr),
                        ScriptGcRoot::TypeCheckProto(i)
                    );
                } else if let Some(ptr) = proto.as_array() {
                    push_root!(
                        ScriptGcMarkKind::Array(ptr),
                        ScriptGcRoot::TypeCheckProto(i)
                    );
                } else {
                    // Handle non-object/array protos (strings, pods, handles) directly
                    self.mark_value(proto);
                }
            }
        }

        // Mark type_defaults objects
        for (ty_idx, obj) in &self.type_defaults {
            push_root!(
                ScriptGcMarkKind::Object(*obj),
                ScriptGcRoot::TypeDefault(ty_idx.0)
            );
        }

        // Mark pod_types default values and objects
        // Collect first to avoid borrow issues
        let pod_type_data: Vec<_> = self
            .pod_types
            .iter()
            .enumerate()
            .map(|(i, pt)| (i, pt.default, pt.object))
            .collect();
        for (i, default, pod_obj) in pod_type_data {
            if let Some(ptr) = default.as_object() {
                push_root!(
                    ScriptGcMarkKind::Object(ptr),
                    ScriptGcRoot::PodTypeDefault(i)
                );
            } else if let Some(ptr) = default.as_array() {
                push_root!(
                    ScriptGcMarkKind::Array(ptr),
                    ScriptGcRoot::PodTypeDefault(i)
                );
            } else {
                // Handle non-object/array defaults directly
                self.mark_value(default);
            }
            if pod_obj != ScriptObject::ZERO {
                push_root!(
                    ScriptGcMarkKind::Object(pod_obj),
                    ScriptGcRoot::PodTypeObject(i)
                );
            }
        }

        // Mark root_objects
        let roots: Vec<_> = self.root_objects.borrow().keys().copied().collect();
        for item in roots {
            push_root!(
                ScriptGcMarkKind::Object(item),
                ScriptGcRoot::RootObject(item.index)
            );
        }

        // Mark root_arrays
        let roots: Vec<_> = self.root_arrays.borrow().keys().copied().collect();
        for item in roots {
            push_root!(
                ScriptGcMarkKind::Array(item),
                ScriptGcRoot::RootArray(item.index)
            );
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
                for (stack_idx, value) in thread.stack.iter().enumerate() {
                    if let Some(ptr) = value.as_object() {
                        push_root!(
                            ScriptGcMarkKind::Object(ptr),
                            ScriptGcRoot::ThreadStack {
                                thread: thread_idx,
                                stack_idx
                            }
                        );
                    } else if let Some(ptr) = value.as_array() {
                        push_root!(
                            ScriptGcMarkKind::Array(ptr),
                            ScriptGcRoot::ThreadStack {
                                thread: thread_idx,
                                stack_idx
                            }
                        );
                    } else {
                        self.mark_value(*value);
                    }
                }
                // Scopes
                for (scope_idx, scope) in thread.scopes.iter().enumerate() {
                    push_root!(
                        ScriptGcMarkKind::Object(*scope),
                        ScriptGcRoot::ThreadScope {
                            thread: thread_idx,
                            scope_idx
                        }
                    );
                }
                // Method call contexts
                for (me_idx, me) in thread.mes.iter().enumerate() {
                    match me {
                        ScriptMe::Object(obj) => {
                            push_root!(
                                ScriptGcMarkKind::Object(*obj),
                                ScriptGcRoot::ThreadMe {
                                    thread: thread_idx,
                                    me_idx,
                                    kind: "Object"
                                }
                            );
                        }
                        ScriptMe::Call { sself, args } => {
                            if let Some(s) = sself {
                                if let Some(ptr) = s.as_object() {
                                    push_root!(
                                        ScriptGcMarkKind::Object(ptr),
                                        ScriptGcRoot::ThreadMe {
                                            thread: thread_idx,
                                            me_idx,
                                            kind: "Call.sself"
                                        }
                                    );
                                } else if let Some(ptr) = s.as_array() {
                                    push_root!(
                                        ScriptGcMarkKind::Array(ptr),
                                        ScriptGcRoot::ThreadMe {
                                            thread: thread_idx,
                                            me_idx,
                                            kind: "Call.sself"
                                        }
                                    );
                                } else {
                                    self.mark_value(*s);
                                }
                            }
                            push_root!(
                                ScriptGcMarkKind::Object(*args),
                                ScriptGcRoot::ThreadMe {
                                    thread: thread_idx,
                                    me_idx,
                                    kind: "Call.args"
                                }
                            );
                        }
                        ScriptMe::Pod { pod, .. } => {
                            self.pods[*pod].tag.set_mark();
                        }
                        ScriptMe::Array(arr) => {
                            push_root!(
                                ScriptGcMarkKind::Array(*arr),
                                ScriptGcRoot::ThreadMe {
                                    thread: thread_idx,
                                    me_idx,
                                    kind: "Array"
                                }
                            );
                        }
                    }
                }
                // Loop sources
                for (loop_idx, loop_frame) in thread.loops.iter().enumerate() {
                    if let Some(loop_values) = &loop_frame.values {
                        let source = loop_values.source;
                        if let Some(ptr) = source.as_object() {
                            push_root!(
                                ScriptGcMarkKind::Object(ptr),
                                ScriptGcRoot::ThreadLoop {
                                    thread: thread_idx,
                                    loop_idx
                                }
                            );
                        } else if let Some(ptr) = source.as_array() {
                            push_root!(
                                ScriptGcMarkKind::Array(ptr),
                                ScriptGcRoot::ThreadLoop {
                                    thread: thread_idx,
                                    loop_idx
                                }
                            );
                        } else {
                            self.mark_value(source);
                        }
                    }
                }
                // Trap error values
                for (err_idx, err) in thread.trap.err.borrow().iter().enumerate() {
                    let val = err.value;
                    if let Some(ptr) = val.as_object() {
                        push_root!(
                            ScriptGcMarkKind::Object(ptr),
                            ScriptGcRoot::ThreadTrapErr {
                                thread: thread_idx,
                                err_idx
                            }
                        );
                    } else if let Some(ptr) = val.as_array() {
                        push_root!(
                            ScriptGcMarkKind::Array(ptr),
                            ScriptGcRoot::ThreadTrapErr {
                                thread: thread_idx,
                                err_idx
                            }
                        );
                    } else {
                        self.mark_value(val);
                    }
                }
                // Trap return values
                if let Some(ScriptTrapOn::Return(v)) = thread.trap.on.get() {
                    if let Some(ptr) = v.as_object() {
                        push_root!(
                            ScriptGcMarkKind::Object(ptr),
                            ScriptGcRoot::ThreadTrapReturn(thread_idx)
                        );
                    } else if let Some(ptr) = v.as_array() {
                        push_root!(
                            ScriptGcMarkKind::Array(ptr),
                            ScriptGcRoot::ThreadTrapReturn(thread_idx)
                        );
                    } else {
                        self.mark_value(v);
                    }
                }
            }
        }

        // Mark ScriptBody scope and me objects
        for (body_idx, body) in code.bodies.borrow().iter().enumerate() {
            if let ScriptSource::Mod(script_mod) = &body.source {
                for (value_idx, v) in script_mod.values.iter().enumerate() {
                    if let Some(ptr) = v.as_object() {
                        push_root!(
                            ScriptGcMarkKind::Object(ptr),
                            ScriptGcRoot::ScriptBodyValue {
                                body_idx,
                                value_idx
                            }
                        );
                    } else if let Some(ptr) = v.as_array() {
                        push_root!(
                            ScriptGcMarkKind::Array(ptr),
                            ScriptGcRoot::ScriptBodyValue {
                                body_idx,
                                value_idx
                            }
                        );
                    } else {
                        self.mark_value(*v);
                    }
                }
            }

            // Mark tokenizer string literals as roots
            // These are strings parsed from the source code and stored in the token stream
            for (token_idx, str_val) in body.tokenizer.iter_strings().enumerate() {
                // Strings are marked directly since they don't contain references
                if let Some(ptr) = str_val.as_string() {
                    if let Some(str_data) = self.strings[ptr].as_mut() {
                        if !str_data.tag.is_static() {
                            str_data.tag.set_mark();
                        }
                    }
                }
                // Handle edge case: inline strings don't need marking (they're not heap-allocated)
                let _ = token_idx; // Silence unused warning, kept for debugging root tracking
            }
        }

        // Mark ScriptNative type_table objects
        for (table_idx, type_map) in code.native.borrow().type_table.iter().enumerate() {
            for (_, obj) in type_map.iter() {
                push_root!(
                    ScriptGcMarkKind::Object(*obj),
                    ScriptGcRoot::NativeTypeTable { table_idx }
                );
            }
        }

        // Process the work list - use while loop since mark_inner adds to mark_vec
        let mut i = 0;
        while i < self.mark_vec.len() {
            let mark = self.mark_vec[i].clone();
            self.mark_inner(mark);
            i += 1;
        }
    }

    pub fn sweep(&mut self, start: std::time::Instant, log_stats: bool) {
        // GC stats: (static, alive, removed)
        let (mut obj_static, mut obj_alive, mut obj_removed) = (0usize, 0usize, 0usize);
        let (mut arr_static, mut arr_alive, mut arr_removed) = (0usize, 0usize, 0usize);
        let (mut str_static, mut str_alive, mut str_removed) = (0usize, 0usize, 0usize);
        let (mut hdl_static, mut hdl_alive, mut hdl_removed) = (0usize, 0usize, 0usize);
        let (mut pod_static, mut pod_alive, mut pod_removed) = (0usize, 0usize, 0usize);

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
                        if let Some(s) = Arc::into_inner(k.0) {
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

        // Print compact GC stats: S=static A=alive R=removed
        if log_stats {
            log!("GC {}us: obj[S:{} A:{} R:{}] arr[S:{} A:{} R:{}] str[S:{} A:{} R:{}] hdl[S:{} A:{} R:{}] pod[S:{} A:{} R:{}]",
                start.elapsed().as_micros(),
                obj_static, obj_alive, obj_removed,
                arr_static, arr_alive, arr_removed,
                str_static, str_alive, str_removed,
                hdl_static, hdl_alive, hdl_removed,
                pod_static, pod_alive, pod_removed);
        }

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
