use crate::function::*;
use crate::heap::*;
use crate::makepad_error_log::*;
use crate::makepad_live_id::*;
use crate::mod_gc::*;
use crate::mod_math::*;
use crate::mod_pod::*;
use crate::mod_shader::*;
use crate::mod_std::*;
use crate::native::*;
use crate::object::*;
use crate::opcode::*;
use crate::parser::*;
use crate::thread::*;
use crate::tokenizer::*;
use crate::trap::*;
use crate::value::*;
use crate::*;
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;

#[derive(Default, Debug)]
pub struct ScriptMod {
    pub cargo_manifest_path: String,
    pub module_path: String,
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub code: String,
    pub values: Vec<ScriptValue>,
}

pub enum ScriptSource {
    Mod(ScriptMod),
    Streaming { code: String },
}

pub struct ScriptBody {
    pub source: ScriptSource,
    pub tokenizer: ScriptTokenizer,
    pub parser: ScriptParser,
    pub scope: ScriptObject,
    pub me: ScriptObject,
}

#[derive(Default)]
pub struct ScriptBuiltins {
    pub range: ScriptObject,
    pub pod: ScriptPodBuiltins,
}

impl ScriptBuiltins {
    pub fn new(heap: &mut ScriptHeap, pod: ScriptPodBuiltins) -> Self {
        Self {
            range: heap
                .value_path(heap.modules, ids!(std.Range), NoTrap)
                .as_object()
                .unwrap(),
            pod,
        }
    }
}

#[derive(Default)]
pub struct ScriptCode {
    pub builtins: ScriptBuiltins,
    pub native: RefCell<ScriptNative>,
    pub bodies: RefCell<Vec<ScriptBody>>,
    pub crate_manifests: RefCell<HashMap<String, String>>,
}

pub struct ScriptLoc {
    pub file: String,
    pub col: u32,
    pub line: u32,
}

impl std::fmt::Debug for ScriptLoc {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

impl std::fmt::Display for ScriptLoc {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}:{}:{}", self.file, self.line, self.col)
    }
}

impl ScriptCode {
    pub fn ip_to_loc(&self, ip: ScriptIp) -> Option<ScriptLoc> {
        if let Some(body) = self.bodies.borrow().get(ip.body as usize) {
            if let Some(Some(index)) = body.parser.source_map.get(ip.index as usize) {
                if let Some(rc) = body.tokenizer.token_index_to_row_col(*index) {
                    if let ScriptSource::Mod(script_mod) = &body.source {
                        return Some(ScriptLoc {
                            file: script_mod.file.clone(),
                            line: rc.0 + script_mod.line as u32,
                            col: rc.1,
                        });
                    } else {
                        return Some(ScriptLoc {
                            file: "generated".into(),
                            line: rc.0,
                            col: rc.1,
                        });
                    };
                }
            }
        }
        return Some(ScriptLoc {
            file: "unknown".into(),
            line: ip.body as _,
            col: ip.index as _,
        });
    }
}

pub struct ScriptVm<'a> {
    pub host: &'a mut dyn Any,
    pub bx: Box<ScriptVmBase>,
}

impl<'a> ScriptVm<'a> {
    pub fn heap(&self) -> &ScriptHeap {
        &self.bx.heap
    }

    pub fn heap_mut(&mut self) -> &mut ScriptHeap {
        &mut self.bx.heap
    }

    /// Run garbage collection (mark and sweep).
    pub fn gc(&mut self) {
        let start = std::time::Instant::now();
        self.bx.heap.mark(&self.bx.threads, &self.bx.code);
        self.bx.heap.sweep(start);
    }

    pub fn thread(&self) -> &ScriptThread {
        self.bx.threads.cur_ref()
    }

    pub fn thread_mut(&mut self) -> &mut ScriptThread {
        self.bx.threads.cur()
    }

    pub fn trap(&'a self) -> ScriptTrap<'a> {
        self.bx.threads.cur_ref().trap.pass()
    }

    /// Format an enum variant error with descriptive information about the value.
    /// Used by generated code from derive macros for better error messages.
    pub fn format_enum_variant_error(&self, value: ScriptValue) -> String {
        crate::suggest::format_enum_variant_error(&self.bx.heap, value)
    }

    /// Format a ScriptObject for error messages with a brief debug representation.
    /// Shows the object's proto chain and key properties.
    pub fn format_object_for_error(&self, obj: ScriptObject) -> String {
        let mut out = String::new();
        let mut recur = Vec::new();
        // Use the heap's debug string but limit depth to keep it concise
        self.bx
            .heap
            .to_debug_string(obj.into(), &mut recur, &mut out, false, 0);
        // Truncate if too long
        if out.len() > 200 {
            out.truncate(197);
            out.push_str("...");
        }
        out
    }

    pub fn set_thread(&mut self, id: usize) {
        self.bx.threads.set_current(id);
    }

    pub fn with_vm<R, F: FnOnce(&mut ScriptVm) -> R>(&mut self, f: F) -> R {
        f(self)
    }

    pub fn call(&mut self, fnobj: ScriptValue, args: &[ScriptValue]) -> ScriptValue {
        let scope = self.bx.heap.new_with_proto(fnobj);

        self.bx.heap.clear_object_deep(scope);
        if fnobj.is_err() {
            return fnobj;
        }

        let trap = self.bx.threads.cur().trap.pass();
        let err = self.bx.heap.push_all_fn_args(scope, args, trap);
        if err.is_err() {
            return err;
        }

        self.bx.heap.set_object_deep(scope);
        self.bx.heap.set_object_storage_auto(scope);

        if let Some(fnptr) = self.bx.heap.parent_as_fn(scope) {
            match fnptr {
                ScriptFnPtr::Native(ni) => {
                    // Get the function pointer and drop the borrow before calling
                    let func_ptr: *const dyn Fn(&mut ScriptVm, ScriptObject) -> ScriptValue = {
                        let native = self.bx.code.native.borrow();
                        &*native.functions[ni.index as usize] as *const _
                    };
                    // Pause thread before native call so re-entrant calls get a different thread
                    self.bx.threads.cur().is_paused = true;
                    // SAFETY: The function pointer is valid as long as native functions aren't removed during execution
                    let result = unsafe { (*func_ptr)(self, scope) };
                    // Only unpause if native didn't explicitly pause (via pause() which sets trap.on to Pause)
                    if !matches!(
                        self.bx.threads.cur().trap.on.get(),
                        Some(ScriptTrapOn::Pause)
                    ) {
                        self.bx.threads.cur().is_paused = false;
                    }
                    return result;
                }
                ScriptFnPtr::Script(sip) => {
                    let call = CallFrame {
                        bases: self.bx.threads.cur_ref().new_bases(),
                        args: OpcodeArgs::default(),
                        return_ip: None,
                    };
                    self.bx.threads.cur().scopes.push(scope);
                    self.bx.threads.cur().calls.push(call);
                    self.bx.threads.cur().trap.ip = sip;
                    return self.run_core();
                }
            }
        } else {
            return script_err_wrong_value!(
                self.bx.threads.cur_ref().trap,
                "call target is not a function (got {:?})",
                self.bx.heap.proto(scope).value_type()
            );
        }
    }

    /// Drain and log any pending errors in the error queue.
    /// Call this after operations that may produce errors outside of run_core
    /// (e.g., script_apply calls from Rust code).
    pub fn drain_errors(&mut self) {
        loop {
            let err = self.bx.threads.cur().trap.err.borrow_mut().pop_front();
            if let Some(err) = err {
                if let Some(ptr) = err.value.as_err() {
                    if let Some(loc2) = self.bx.code.ip_to_loc(ptr.ip) {
                        log_with_level(
                            &loc2.file,
                            loc2.line,
                            loc2.col,
                            loc2.line,
                            loc2.col,
                            format!("{} ({}:{})", err.message, err.origin_file, err.origin_line),
                            LogLevel::Error,
                        );
                    } else {
                        // No location info, still log the error
                        log_with_level(
                            &err.origin_file,
                            err.origin_line,
                            0,
                            err.origin_line,
                            0,
                            err.message.clone(),
                            LogLevel::Error,
                        );
                    }
                } else {
                    // Error without IP, still log
                    log_with_level(
                        &err.origin_file,
                        err.origin_line,
                        0,
                        err.origin_line,
                        0,
                        err.message.clone(),
                        LogLevel::Error,
                    );
                }
            } else {
                break;
            }
        }
    }

    #[inline(never)]
    #[cold]
    fn handle_errors(&mut self) {
        if self.bx.threads.cur().call_has_try() {
            // pop all errors
            self.bx.threads.cur().trap.err.borrow_mut().clear();
            let try_frame = self.bx.threads.cur().tries.pop().unwrap();
            self.bx
                .threads
                .cur()
                .truncate_bases(try_frame.bases, &mut self.bx.heap);
            if try_frame.push_nil {
                self.bx.threads.cur().push_stack_unchecked(NIL)
            }
            self.bx
                .threads
                .cur()
                .trap
                .goto(try_frame.start_ip + try_frame.jump);
        } else {
            self.drain_errors();
        }
    }

    pub fn run_core(&mut self) -> ScriptValue {
        // Cache opcodes pointer to avoid RefCell borrow on every iteration
        let mut cached_body_index: usize = usize::MAX;
        let mut opcodes_ptr: *const ScriptValue = std::ptr::null();
        let mut opcodes_len: usize = 0;

        loop {
            let thread = self.bx.threads.cur();
            let body_index = thread.trap.ip.body as usize;
            let ip_index = thread.trap.ip.index as usize;

            // Only re-borrow bodies when body changes
            if body_index != cached_body_index {
                let bodies = self.bx.code.bodies.borrow();
                let body = &bodies[body_index];
                opcodes_ptr = body.parser.opcodes.as_ptr();
                opcodes_len = body.parser.opcodes.len();
                cached_body_index = body_index;
            }

            if ip_index >= opcodes_len {
                // If there's a value on the stack, return it (for expression-style scripts)
                let stack_len = self.bx.threads.cur().stack.len();
                if stack_len > 0 {
                    log!("run_core: returning stack value, stack_len={}", stack_len);
                    return self.bx.threads.cur().pop_stack_value();
                }
                log!("run_core: stack empty, returning NIL");
                return NIL;
            }

            // SAFETY: opcodes_ptr is valid as long as bodies isn't mutated during execution
            let opcode = unsafe { *opcodes_ptr.add(ip_index) };

            if let Some((opcode, args)) = opcode.as_opcode() {
                self.opcode(opcode, args);
                // if exception tracing - is_empty() is faster than len()>0
                if !self.bx.threads.cur().trap.err.borrow().is_empty() {
                    self.handle_errors();
                }
                // Check with get() first to avoid unnecessary write in common case (None)
                if self.bx.threads.cur().trap.on.get().is_some() {
                    match self.bx.threads.cur().trap.on.take().unwrap() {
                        ScriptTrapOn::Pause => return NIL,
                        ScriptTrapOn::Return(value) => return value,
                    }
                }
            } else {
                // its a direct value-to-stack
                self.bx.threads.cur().push_stack_value(opcode);
                self.bx.threads.cur().trap.goto_next();
            }
        }
    }

    pub fn run_root(&mut self, body_id: u16) -> ScriptValue {
        // Extract values from bodies before modifying thread state
        let (scope, me) = {
            let bodies = self.bx.code.bodies.borrow();
            (bodies[body_id as usize].scope, bodies[body_id as usize].me)
        };

        self.bx.threads.cur().calls.push(CallFrame {
            bases: StackBases {
                tries: 0,
                loops: 0,
                stack: 0,
                scope: 0,
                mes: 0,
            },
            args: Default::default(),
            return_ip: None,
        });

        self.bx.threads.cur().scopes.push(scope);
        self.bx.threads.cur().mes.push(ScriptMe::Object(me));

        self.bx.threads.cur().trap.ip.body = body_id;
        self.bx.threads.cur().trap.ip.index = 0;

        // the main interpreter loop
        self.run_core()
    }

    /// Checks if the value has an apply transform and calls it, returning the transformed value.
    /// Returns None if no transform exists, Some(transformed) if a transform was applied.
    pub fn call_apply_transform(&mut self, value: ScriptValue) -> Option<ScriptValue> {
        if let Some(obj) = value.as_object() {
            if let Some(ni) = self.bx.heap.objects[obj.index as usize]
                .tag
                .as_apply_transform()
            {
                let func_ptr: *const dyn Fn(&mut ScriptVm, ScriptObject) -> ScriptValue = {
                    let native = self.bx.code.native.borrow();
                    &*native.functions[ni.index as usize] as *const _
                };
                // Pause thread before native call so re-entrant calls get a different thread
                self.bx.threads.cur().is_paused = true;
                let result = unsafe { (*func_ptr)(self, obj) };
                // Only unpause if native didn't explicitly pause
                if !matches!(
                    self.bx.threads.cur().trap.on.get(),
                    Some(ScriptTrapOn::Pause)
                ) {
                    self.bx.threads.cur().is_paused = false;
                }
                return Some(result);
            }
        } else if let Some(arr) = value.as_array() {
            if let Some(ni) = self.bx.heap.arrays[arr.index as usize]
                .tag
                .as_apply_transform()
            {
                // For arrays, we need to create a temporary args object
                let args_obj = self.bx.heap.new_object();
                self.bx
                    .heap
                    .set_value_def(args_obj, id!(self).into(), value);
                let func_ptr: *const dyn Fn(&mut ScriptVm, ScriptObject) -> ScriptValue = {
                    let native = self.bx.code.native.borrow();
                    &*native.functions[ni.index as usize] as *const _
                };
                // Pause thread before native call so re-entrant calls get a different thread
                self.bx.threads.cur().is_paused = true;
                let result = unsafe { (*func_ptr)(self, args_obj) };
                // Only unpause if native didn't explicitly pause
                if !matches!(
                    self.bx.threads.cur().trap.on.get(),
                    Some(ScriptTrapOn::Pause)
                ) {
                    self.bx.threads.cur().is_paused = false;
                }
                return Some(result);
            }
        }
        None
    }

    pub fn resume(&mut self) -> ScriptValue {
        self.bx.threads.cur().is_paused = false;
        self.run_core()
    }

    pub fn cast_to_f64(&self, v: ScriptValue) -> f64 {
        self.bx
            .heap
            .cast_to_f64(v, self.bx.threads.cur_ref().trap.ip)
    }

    pub fn handle_type(&self, id: LiveId) -> ScriptHandleType {
        *self.bx.code.native.borrow().handle_type.get(&id).unwrap()
    }

    pub fn new_handle_type(&mut self, id: LiveId) -> ScriptHandleType {
        self.bx
            .code
            .native
            .borrow_mut()
            .new_handle_type(&mut self.bx.heap, id)
    }

    pub fn add_handle_method<F>(
        &mut self,
        ht: ScriptHandleType,
        method: LiveId,
        args: &[(LiveId, ScriptValue)],
        f: F,
    ) where
        F: Fn(&mut ScriptVm, ScriptObject) -> ScriptValue + 'static,
    {
        self.bx.code.native.borrow_mut().add_type_method(
            &mut self.bx.heap,
            ht.to_redux(),
            method,
            args,
            f,
        )
    }

    pub fn set_handle_setter<F>(&mut self, ht: ScriptHandleType, f: F)
    where
        F: Fn(&mut ScriptVm, ScriptValue, LiveId, ScriptValue) -> ScriptValue + 'static,
    {
        self.bx
            .code
            .native
            .borrow_mut()
            .set_type_setter(ht.to_redux(), f)
    }

    pub fn set_handle_getter<F>(&mut self, ht: ScriptHandleType, f: F)
    where
        F: Fn(&mut ScriptVm, ScriptValue, LiveId) -> ScriptValue + 'static,
    {
        self.bx
            .code
            .native
            .borrow_mut()
            .set_type_getter(ht.to_redux(), f)
    }

    pub fn new_module(&mut self, id: LiveId) -> ScriptObject {
        self.bx.heap.new_module(id)
    }

    pub fn module(&mut self, id: LiveId) -> ScriptObject {
        self.bx.heap.module(id)
    }

    pub fn map_mut_with<R, F: FnOnce(&mut Self, &mut ScriptObjectMap) -> R>(
        &mut self,
        object: ScriptObject,
        f: F,
    ) -> R {
        let mut map = ScriptObjectMap::default();
        std::mem::swap(
            &mut map,
            &mut self.bx.heap.objects[object.index as usize].map,
        );
        let r = f(self, &mut map);
        std::mem::swap(
            &mut map,
            &mut self.bx.heap.objects[object.index as usize].map,
        );
        r
    }

    /// Walk the prototype chain from root (oldest ancestor) to leaf (the object itself),
    /// calling the closure for each object's map. This is useful for collecting inherited
    /// properties where child properties should override parent properties.
    pub fn proto_map_iter_mut_with<F: FnMut(&mut Self, &mut ScriptObjectMap)>(
        &mut self,
        object: ScriptObject,
        f: &mut F,
    ) {
        // First recurse to the prototype (if any), so we process from root to leaf
        if let Some(proto) = self.bx.heap.objects[object.index as usize]
            .proto
            .as_object()
        {
            self.proto_map_iter_mut_with(proto, f);
        }
        // Then process this object's map
        let mut map = ScriptObjectMap::default();
        std::mem::swap(
            &mut map,
            &mut self.bx.heap.objects[object.index as usize].map,
        );
        f(self, &mut map);
        std::mem::swap(
            &mut map,
            &mut self.bx.heap.objects[object.index as usize].map,
        );
    }

    pub fn vec_with<R, F: FnOnce(&mut Self, &[ScriptVecValue]) -> R>(
        &mut self,
        object: ScriptObject,
        f: F,
    ) -> R {
        let mut vec = Vec::new();
        std::mem::swap(
            &mut vec,
            &mut self.bx.heap.objects[object.index as usize].vec,
        );
        let r = f(self, &vec);
        std::mem::swap(
            &mut vec,
            &mut self.bx.heap.objects[object.index as usize].vec,
        );
        r
    }

    pub fn vec_mut_with<R, F: FnOnce(&mut Self, &mut Vec<ScriptVecValue>) -> R>(
        &mut self,
        object: ScriptObject,
        f: F,
    ) -> R {
        let mut vec = Vec::new();
        std::mem::swap(
            &mut vec,
            &mut self.bx.heap.objects[object.index as usize].vec,
        );
        let r = f(self, &mut vec);
        std::mem::swap(
            &mut vec,
            &mut self.bx.heap.objects[object.index as usize].vec,
        );
        r
    }

    pub fn string_with<R, F: FnOnce(&mut Self, &str) -> R>(
        &mut self,
        value: ScriptValue,
        f: F,
    ) -> Option<R> {
        if let Some(s) = value.as_string() {
            if let Some(s) = &self.bx.heap.strings[s.index as usize] {
                let s = s.string.clone();
                return Some(f(self, &s.0));
            }
            return None;
        }
        if let Some(r) = value.as_inline_string(|s| f(self, s)) {
            return Some(r);
        }
        None
    }

    pub fn new_string_with<F: FnOnce(&mut Self, &mut String)>(&mut self, f: F) -> ScriptValue {
        let mut out = if let Some(s) = self.bx.heap.strings_reuse.pop() {
            s
        } else {
            String::new()
        };
        f(self, &mut out);
        self.bx.heap.intern_or_store_string(out)
    }

    pub fn add_method<F>(
        &mut self,
        module: ScriptObject,
        method: LiveId,
        args: &[(LiveId, ScriptValue)],
        f: F,
    ) where
        F: Fn(&mut ScriptVm, ScriptObject) -> ScriptValue + 'static,
    {
        self.bx
            .code
            .native
            .borrow_mut()
            .add_method(&mut self.bx.heap, module, method, args, f)
    }

    /// Registers a native function to be used as an apply_transform and returns its NativeId.
    /// This is used for creating objects that transform to a computed value when applied.
    pub fn add_apply_transform_fn<F>(&mut self, f: F) -> NativeId
    where
        F: Fn(&mut ScriptVm, ScriptObject) -> ScriptValue + 'static,
    {
        self.bx.code.native.borrow_mut().add_apply_transform_fn(f)
    }

    pub fn add_script_mod(&mut self, new_mod: ScriptMod) -> u16 {
        // Register this crate's manifest path for crate path resolution
        let crate_name = new_mod.module_path.split("::").next().unwrap_or("");
        if !crate_name.is_empty() {
            self.bx.code.crate_manifests.borrow_mut().insert(
                crate_name.replace('-', "_"),
                new_mod.cargo_manifest_path.clone(),
            );
        }

        let scope = self.bx.heap.new_with_proto(id!(scope).into());
        self.bx.heap.set_object_deep(scope);
        self.bx
            .heap
            .set_value_def(scope, id!(mod).into(), self.bx.heap.modules.into());
        let me = self.bx.heap.new_with_proto(id!(root_me).into());

        let new_body = ScriptBody {
            source: ScriptSource::Mod(new_mod),
            tokenizer: ScriptTokenizer::default(),
            parser: ScriptParser::default(),
            scope,
            me,
        };
        let mut bodies = self.bx.code.bodies.borrow_mut();
        for (i, body) in bodies.iter_mut().enumerate() {
            if let ScriptSource::Mod(script_mod) = &body.source {
                if let ScriptSource::Mod(new_mod) = &new_body.source {
                    if script_mod.file == new_mod.file
                        && script_mod.line == new_mod.line
                        && script_mod.column == new_mod.column
                    {
                        *body = new_body;
                        return i as u16;
                    }
                }
            }
        }
        let i = bodies.len();
        bodies.push(new_body);
        i as u16
    }

    pub fn eval(&mut self, script_mod: ScriptMod) -> ScriptValue {
        self.eval_with_source(script_mod, ScriptObject::ZERO)
    }

    pub fn eval_with_source(&mut self, script_mod: ScriptMod, source: ScriptObject) -> ScriptValue {
        let body_id = self.add_script_mod(script_mod);

        // Set __script_source__ on the scope if source is provided
        // If source has FROM_EVAL flag, use its prototype instead
        if source != ScriptObject::ZERO {
            let actual_source = if self.bx.heap.is_from_eval(source) {
                // Use the prototype of the FROM_EVAL object
                if let Some(proto) = self.bx.heap.proto(source).as_object() {
                    proto
                } else {
                    source
                }
            } else {
                source
            };
            let scope = self.bx.code.bodies.borrow()[body_id as usize].scope;
            self.bx
                .heap
                .set_value_def(scope, id!(__script_source__).into(), actual_source.into());
        }

        let mut bodies = self.bx.code.bodies.borrow_mut();
        let body = &mut bodies[body_id as usize];

        if let ScriptSource::Mod(script_mod) = &body.source {
            // Only log short scripts (likely test scripts)
            body.tokenizer.tokenize(&script_mod.code, &mut self.bx.heap);
            body.parser.parse(
                &body.tokenizer,
                &script_mod.file,
                (script_mod.line, script_mod.column),
                &script_mod.values,
            );
            drop(bodies);
            // lets point our thread to it

            let result = self.run_root(body_id);

            // Mark the result object with FROM_EVAL flag
            if let Some(result_obj) = result.as_object() {
                self.bx.heap.set_from_eval(result_obj);
            }

            result
        } else {
            NIL
        }
    }
}

pub struct ScriptVmBase {
    pub void: usize,
    pub code: ScriptCode,
    pub heap: ScriptHeap,
    pub threads: ScriptThreads,
    pub debug_trace: bool,
}

impl ScriptVmBase {
    pub fn empty() -> Self {
        Self {
            void: 0,
            code: ScriptCode::default(),
            threads: ScriptThreads::empty(),
            heap: ScriptHeap::empty(),
            debug_trace: false,
        }
    }

    pub fn new() -> Self {
        let mut heap = ScriptHeap::empty();
        let mut native = ScriptNative::new(&mut heap);
        define_math_module(&mut heap, &mut native);
        define_std_module(&mut heap, &mut native);
        define_shader_module(&mut heap, &mut native);
        define_gc_module(&mut heap, &mut native);
        let pod_builtins = define_pod_module(&mut heap, &mut native);

        let builtins = ScriptBuiltins::new(&mut heap, pod_builtins);

        Self {
            void: 0,
            code: ScriptCode {
                builtins,
                native: RefCell::new(native),
                bodies: Default::default(),
                crate_manifests: Default::default(),
            },
            threads: ScriptThreads::new(),
            heap: heap,
            debug_trace: false,
        }
    }
}
