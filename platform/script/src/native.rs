use makepad_relax_api::relax_api;
use crate::array::*;
use crate::function::*;
use crate::heap::*;
use crate::makepad_live_id::live_id::*;
use crate::makepad_live_id::*;
use crate::object::*;
use crate::string::*;
use crate::value::*;
use crate::vm::*;
use crate::*;

#[macro_export]
macro_rules! script_value {
    ($vm:ident, $obj:ident.$id: ident) => {
        $vm.value_trap(($obj).into(), id!($id))
    };
    ($vm:ident, $obj:ident.$id:ident.$id2:ident) => {{
        let __script_value_inner = $vm.value_trap(($obj).into(), id!($id));
        $vm.value_trap(__script_value_inner.into(), id!($id2))
    }};
    ($vm:ident, $obj:ident[$index: expr]) => {
        $vm.vec_value_trap(($obj).into(), ($index) as usize)
    };
    ($vm:ident, $obj:ident as array[$index: expr]) => {
        $vm.heap().array_index(($obj).into(), ($index) as usize, $vm.trap())
    };
}

#[macro_export]
macro_rules! script_has_proto {
    ($vm:ident, $what:ident, $obj:ident.$id: ident) => {{
        let proto = $vm.value_trap(($obj).into(), id!($id));
        $vm.heap_mut().has_proto(($what).into(), proto)
    }};
}

#[macro_export]
macro_rules! script_is_fn {
    ($vm:ident, $what:ident, $obj:expr) => {{
        $vm.heap().is_fn(($obj).into())
    }};
}

#[macro_export]
macro_rules! script_array_index {
    ($vm:ident, $obj:ident[$index: expr]) => {{
        $vm.heap().array_index(($obj).into(), ($index) as usize, $vm.trap())
    }};
}

#[macro_export]
macro_rules! set_script_value {
    ($vm:ident, $obj:ident.$id: ident=$value:expr) => {{
        $vm.set_value_trap($obj, id!($id), $value)
    }};
}

#[macro_export]
macro_rules! set_script_value_to_api {
    ($vm:ident, $obj:ident.$id: ident=$val:expr) => {{
        let v = $val::script_api($vm);
        $vm.set_value_trap(($obj).into(), id_lut!($id), v);
    }};
    ($vm:ident, $obj:ident.$id: ident) => {{
        let v = $id::script_api($vm);
        $vm.set_value_trap(($obj).into(), id_lut!($id), v);
    }};
}

#[macro_export]
macro_rules! set_script_value_to_pod {
    ($vm:ident, $obj:ident.$id: ident=$val:expr) => {{
        let v = $val::script_pod($vm).expect("Cant make a pod type");
        $vm.heap_mut().pod_type_name_set(v, id_lut!($id));
        $vm.set_value_trap(($obj).into(), id_lut!($id), v);
    }};
    ($vm:ident, $obj:ident.$id: ident) => {{
        let v = $id::script_pod($vm).expect("Cant make a pod type");
        $vm.heap_mut().pod_type_name_set(v, id_lut!($id));
        $vm.set_value_trap(($obj).into(), id_lut!($id), v);
    }};
}

#[macro_export]
macro_rules! script_args{
    ($($id:ident=$val:expr),*)=>{
        &[$((id!($id), ($val).into()),)*]
    }
}

#[macro_export]
macro_rules! script_args_def{
    ($($id:ident=$val:expr),*)=>{
        &[$((id_lut!($id), ($val).into()),)*]
    }
}

// Natives are C-ABI callables: `{data, call, drop}` where `call` is an `extern "C-unwind"` fn
// made per closure type by a generic trampoline. The VM calls them without knowing which
// compiler built the closure: in the makepad loader the VM is rustc-built and app natives are
// Rapid-built (local/plans/loader-core.md section 8). Under one compiler this is the same
// indirect call a `Box<dyn Fn>` makes. "C-unwind": a panicking native unwinds through the VM
// as before.
macro_rules! native_callable {
    ($(#[$m:meta])* $name:ident, $view:ident, ($($arg:ident: $ty:ty),*)) => {
        $(#[$m])*
        #[relax_api(plain)]
        #[repr(C)]
        pub struct $name {
            data: *mut u8,
            call: unsafe extern "C-unwind" fn(*const u8, &mut ScriptVm $(, $ty)*) -> ScriptValue,
            drop: unsafe extern "C-unwind" fn(*mut u8),
        }

        /// A copy of a native's entry that outlives the borrow of the native table (the
        /// closure lives as long as its table entry).
        #[relax_api(plain)]
        #[derive(Clone, Copy)]
        #[repr(C)]
        pub struct $view {
            data: *const u8,
            call: unsafe extern "C-unwind" fn(*const u8, &mut ScriptVm $(, $ty)*) -> ScriptValue,
        }

        impl $name {
            #[inline(always)]
            pub fn new<F>(f: F) -> Self
            where
                F: Fn(&mut ScriptVm $(, $ty)*) -> ScriptValue + 'static,
            {
                unsafe extern "C-unwind" fn call<F>(data: *const u8, vm: &mut ScriptVm $(, $arg: $ty)*) -> ScriptValue
                where
                    F: Fn(&mut ScriptVm $(, $ty)*) -> ScriptValue + 'static,
                {
                    (*(data as *const F))(vm $(, $arg)*)
                }
                unsafe extern "C-unwind" fn drop_box<F>(data: *mut u8) {
                    drop(Box::from_raw(data as *mut F));
                }
                Self {
                    data: Box::into_raw(Box::new(f)) as *mut u8,
                    call: call::<F>,
                    drop: drop_box::<F>,
                }
            }

            #[inline(always)]
            pub fn view(&self) -> $view {
                $view { data: self.data, call: self.call }
            }

            /// The native as `{data, call, drop}`, its ownership passing to the raw record
            /// (what `ScriptVmBase::vm_register` takes).
            #[inline(always)]
            pub fn into_raw(self) -> crate::vm_ctl::ScriptNativeRaw {
                let me = std::mem::ManuallyDrop::new(self);
                crate::vm_ctl::ScriptNativeRaw { data: me.data, call: me.call as *const (), drop: me.drop }
            }

            /// # Safety
            /// `raw` must come from `into_raw` of this very type and not be used again.
            #[inline(always)]
            pub unsafe fn from_raw(raw: crate::vm_ctl::ScriptNativeRaw) -> Self {
                Self { data: raw.data, call: std::mem::transmute(raw.call), drop: raw.drop }
            }
        }

        impl $view {
            /// # Safety
            /// The native's table entry must still exist (natives are never removed while
            /// the VM runs).
            #[inline(always)]
            pub unsafe fn call(self, vm: &mut ScriptVm $(, $arg: $ty)*) -> ScriptValue {
                (self.call)(self.data, vm $(, $arg)*)
            }
        }

        impl Drop for $name {
            fn drop(&mut self) {
                unsafe { (self.drop)(self.data) }
            }
        }
    };
}

native_callable!(
    /// A field read on a value type: (vm, value, field).
    NativeGetterFn, NativeGetterView, (value: ScriptValue, field: LiveId)
);
native_callable!(
    /// A field write on a value type: (vm, value, field, new value).
    NativeSetterFn, NativeSetterView, (value: ScriptValue, field: LiveId, to: ScriptValue)
);
native_callable!(
    /// Generic method dispatch: called with (vm, args_object, method).
    /// The args object has `self` set and all call arguments collected.
    /// Used when a type has no specific method registered but has a catch-all call handler.
    NativeCallFn, NativeCallView, (args: ScriptObject, method: LiveId)
);
native_callable!(
    /// A native function: (vm, args_object).
    NativeFn, NativeFnView, (args: ScriptObject)
);

#[derive(Default)]
pub struct ScriptNative {
    pub(crate) functions: Vec<NativeFn>,
    pub(crate) type_table: Vec<LiveIdMap<LiveId, ScriptObject>>,
    pub(crate) handle_type: LiveIdMap<LiveId, ScriptHandleType>,
    pub(crate) getters: Vec<NativeGetterFn>,
    pub(crate) setters: Vec<NativeSetterFn>,
    pub(crate) calls: Vec<Option<NativeCallFn>>,
}

impl ScriptNative {
    pub fn new(h: &mut ScriptHeap) -> Self {
        let mut native = Self::default();
        native.add_shared(h);
        ScriptObjectData::add_type_methods(&mut native, h);
        ScriptArrayData::add_type_methods(&mut native, h);
        ScriptStringData::add_type_methods(&mut native, h);
        native
    }

    /// Generic entry point - only boxes the closure, delegates to non-generic helper
    #[inline(always)]
    pub fn add_fn<F>(
        &mut self,
        heap: &mut ScriptHeap,
        args: &[(LiveId, ScriptValue)],
        f: F,
    ) -> ScriptObject
    where
        F: Fn(&mut ScriptVm, ScriptObject) -> ScriptValue + 'static,
    {
        let boxed = NativeFn::new(f);
        self.add_fn_boxed(heap, args, boxed)
    }

    /// Non-generic helper that does the actual work - reduces monomorphization
    #[inline(never)]
    fn add_fn_boxed(
        &mut self,
        heap: &mut ScriptHeap,
        args: &[(LiveId, ScriptValue)],
        f: NativeFn,
    ) -> ScriptObject {
        let fn_index = self.functions.len();
        let fn_obj = heap.new_with_proto(id!(native).into());
        heap.set_object_storage_vec2(fn_obj);
        heap.set_fn(
            fn_obj,
            ScriptFnPtr::Native(NativeId {
                index: fn_index as u32,
            }),
        );

        for (arg, def) in args {
            heap.set_value_def(fn_obj, (*arg).into(), *def);
        }

        self.functions.push(f);

        fn_obj
    }

    /// Registers a native function to be used as an apply_transform and returns its NativeId.
    /// This is used for creating objects that transform to a computed value when applied.
    #[inline(always)]
    pub fn add_apply_transform_fn<F>(&mut self, f: F) -> NativeId
    where
        F: Fn(&mut ScriptVm, ScriptObject) -> ScriptValue + 'static,
    {
        self.add_apply_transform_native(NativeFn::new(f))
    }

    pub fn add_apply_transform_native(&mut self, f: NativeFn) -> NativeId {
        let fn_index = self.functions.len();
        self.functions.push(f);
        NativeId {
            index: fn_index as u32,
        }
    }

    /// Generic entry point - only boxes the closure, delegates to non-generic helper
    #[inline(always)]
    pub fn add_method<F>(
        &mut self,
        heap: &mut ScriptHeap,
        module: ScriptObject,
        method: LiveId,
        args: &[(LiveId, ScriptValue)],
        f: F,
    ) where
        F: Fn(&mut ScriptVm, ScriptObject) -> ScriptValue + 'static,
    {
        self.add_method_boxed(heap, module, method, args, NativeFn::new(f))
    }

    #[inline(never)]
    pub fn add_method_boxed(
        &mut self,
        heap: &mut ScriptHeap,
        module: ScriptObject,
        method: LiveId,
        args: &[(LiveId, ScriptValue)],
        f: NativeFn,
    ) {
        let fn_obj = self.add_fn_boxed(heap, args, f);
        heap.set_value_def(module, method.into(), fn_obj.into());
    }

    pub fn new_handle_type(&mut self, heap: &mut ScriptHeap, id: LiveId) -> ScriptHandleType {
        let ht = self.type_table.len() - ScriptValueType::REDUX_HANDLE_FIRST.to_index();
        if ht >= ScriptValueType::REDUX_HANDLE_MAX as usize {
            panic!(
                "Too many handle types (max {})",
                ScriptValueType::REDUX_HANDLE_MAX
            );
        }
        let ty = ScriptHandleType(ht as u8);
        self.handle_type.insert(id, ty);
        self.add_type_method(heap, ty.to_redux(), id!(ty), &[], move |_, _| id.escape());
        ty
    }

    #[inline(always)]
    pub fn set_type_getter<F>(&mut self, ty_redux: ScriptTypeRedux, f: F)
    where
        F: Fn(&mut ScriptVm, ScriptValue, LiveId) -> ScriptValue + 'static,
    {
        self.set_type_getter_native(ty_redux, NativeGetterFn::new(f))
    }

    pub fn set_type_getter_native(&mut self, ty_redux: ScriptTypeRedux, f: NativeGetterFn) {
        self.getters[ty_redux.to_index()] = f
    }

    #[inline(always)]
    pub fn set_type_setter<F>(&mut self, ty_redux: ScriptTypeRedux, f: F)
    where
        F: Fn(&mut ScriptVm, ScriptValue, LiveId, ScriptValue) -> ScriptValue + 'static,
    {
        self.set_type_setter_native(ty_redux, NativeSetterFn::new(f))
    }

    pub fn set_type_setter_native(&mut self, ty_redux: ScriptTypeRedux, f: NativeSetterFn) {
        self.setters[ty_redux.to_index()] = f
    }

    #[inline(always)]
    pub fn set_type_call<F>(&mut self, ty_redux: ScriptTypeRedux, f: F)
    where
        F: Fn(&mut ScriptVm, ScriptObject, LiveId) -> ScriptValue + 'static,
    {
        self.set_type_call_native(ty_redux, NativeCallFn::new(f))
    }

    pub fn set_type_call_native(&mut self, ty_redux: ScriptTypeRedux, f: NativeCallFn) {
        self.ensure_type_table_capacity(ty_redux);
        self.calls[ty_redux.to_index()] = Some(f);
    }

    /// The methods registered on a value type (`set_type_method`), by name.
    pub fn type_methods(&self, ty_redux: ScriptTypeRedux) -> Option<&LiveIdMap<LiveId, ScriptObject>> {
        self.type_table.get(ty_redux.to_index())
    }

    /// Ensures capacity for type tables - non-generic to reduce monomorphization
    #[inline(never)]
    fn ensure_type_table_capacity(&mut self, ty_redux: ScriptTypeRedux) {
        if ty_redux.to_index() as usize >= self.type_table.len() {
            self.type_table
                .resize_with(ty_redux.to_index() + 1, || Default::default());
            self.getters.resize_with(ty_redux.to_index() + 1, || {
                NativeGetterFn::new(|vm: &mut ScriptVm, value: ScriptValue, field: LiveId| {
                    script_err_not_found!(
                        vm.bx.threads.cur_ref().trap,
                        "no getter for field {:?} on type {:?}",
                        field,
                        value.value_type()
                    )
                })
            });
            self.setters.resize_with(ty_redux.to_index() + 1, || {
                NativeSetterFn::new(|vm: &mut ScriptVm, value: ScriptValue, field: LiveId, _: ScriptValue| {
                    script_err_not_found!(
                        vm.bx.threads.cur_ref().trap,
                        "no setter for field {:?} on type {:?}",
                        field,
                        value.value_type()
                    )
                })
            });
            self.calls.resize_with(ty_redux.to_index() + 1, || None);
        }
    }

    /// Generic entry point - only boxes the closure, delegates to non-generic helper
    #[inline(always)]
    pub fn add_type_method<F>(
        &mut self,
        heap: &mut ScriptHeap,
        ty_redux: ScriptTypeRedux,
        method: LiveId,
        args: &[(LiveId, ScriptValue)],
        f: F,
    ) where
        F: Fn(&mut ScriptVm, ScriptObject) -> ScriptValue + 'static,
    {
        self.add_type_method_native(heap, ty_redux, method, args, NativeFn::new(f))
    }

    #[inline(never)]
    pub fn add_type_method_native(
        &mut self,
        heap: &mut ScriptHeap,
        ty_redux: ScriptTypeRedux,
        method: LiveId,
        args: &[(LiveId, ScriptValue)],
        f: NativeFn,
    ) {
        let fn_obj = self.add_fn_boxed(heap, args, f);
        self.ensure_type_table_capacity(ty_redux);
        self.type_table[ty_redux.to_index()].insert(method, fn_obj);
    }

    /// Removes every native method registered for one value type.
    ///
    /// A host that embeds the raw VM normally leaves this surface intact. A
    /// restricted embedding can clear it during trusted setup, then install a
    /// smaller owned method surface before any script is evaluated.
    pub fn clear_type_methods(&mut self, ty_redux: ScriptTypeRedux) -> usize {
        let Some(methods) = self.type_table.get_mut(ty_redux.to_index()) else {
            return 0;
        };
        let removed = methods.len();
        methods.clear();
        removed
    }

    pub fn add_shared(&mut self, heap: &mut ScriptHeap) {
        self.add_type_method(heap, ScriptValueType::REDUX_NUMBER, id!(ty), &[], |_, _| {
            id!(number).escape()
        });
        self.add_type_method(heap, ScriptValueType::REDUX_NAN, id!(ty), &[], |_, _| {
            id!(nan).escape()
        });
        self.add_type_method(heap, ScriptValueType::REDUX_BOOL, id!(ty), &[], |_, _| {
            id!(bool).escape()
        });
        self.add_type_method(heap, ScriptValueType::REDUX_NIL, id!(ty), &[], |_, _| {
            id!(nil).escape()
        });
        self.add_type_method(heap, ScriptValueType::REDUX_COLOR, id!(ty), &[], |_, _| {
            id!(color).escape()
        });
        self.add_type_method(heap, ScriptValueType::REDUX_STRING, id!(ty), &[], |_, _| {
            id!(string).escape()
        });
        self.add_type_method(heap, ScriptValueType::REDUX_OBJECT, id!(ty), &[], |_, _| {
            id!(object).escape()
        });
        self.add_type_method(heap, ScriptValueType::REDUX_ARRAY, id!(ty), &[], |_, _| {
            id!(rsid).escape()
        });
        self.add_type_method(heap, ScriptValueType::REDUX_REGEX, id!(ty), &[], |_, _| {
            id!(regex).escape()
        });
        self.add_type_method(heap, ScriptValueType::REDUX_OPCODE, id!(ty), &[], |_, _| {
            id!(opcode).escape()
        });
        self.add_type_method(heap, ScriptValueType::REDUX_ERR, id!(ty), &[], |_, _| {
            id!(err).escape()
        });
        self.add_type_method(heap, ScriptValueType::REDUX_ID, id!(ty), &[], |_, _| {
            id!(id).escape()
        });

        let types = [
            (ScriptValueType::REDUX_NUMBER, id!(is_number)),
            (ScriptValueType::REDUX_NAN, id!(is_nan)),
            (ScriptValueType::REDUX_BOOL, id!(is_bool)),
            (ScriptValueType::REDUX_NIL, id!(is_nil)),
            (ScriptValueType::REDUX_COLOR, id!(is_color)),
            (ScriptValueType::REDUX_STRING, id!(is_string)),
            (ScriptValueType::REDUX_OBJECT, id!(is_object)),
            (ScriptValueType::REDUX_ARRAY, id!(is_array)),
            (ScriptValueType::REDUX_REGEX, id!(is_regex)),
            (ScriptValueType::REDUX_OPCODE, id!(is_opcode)),
            (ScriptValueType::REDUX_ERR, id!(is_err)),
            (ScriptValueType::REDUX_ID, id!(is_id)),
        ];

        for (ty, _) in types {
            self.add_type_method(heap, ty, id!(to_json), &[], |vm, args| {
                let sself = script_value!(vm, args.self);
                vm.bx.heap.to_json(sself)
            });
            self.add_type_method(heap, ty, id!(to_number), &[], |vm, args| {
                let sself = script_value!(vm, args.self);
                vm.bx
                    .heap
                    .cast_to_f64(sself, vm.bx.threads.cur_ref().trap.ip)
                    .into()
            });
            if ty != ScriptValueType::REDUX_ARRAY {
                self.add_type_method(heap, ty, id!(to_string), &[], |vm, args| {
                    let sself = script_value!(vm, args.self);
                    if sself.is_string_like() {
                        return sself;
                    }
                    let len = vm.bx.heap.cast_to_string_len(sself);
                    vm.bx.heap.new_string_with_preflight(len, "converting a value to a string", |heap, out| {
                        heap.cast_to_string(sself, out);
                    })
                });
            }
        }

        // `is_fn()`: whether a value is a function (on every type).
        for ty in types.iter().map(|t| t.0).chain([ScriptValueType::REDUX_POD, ScriptValueType::REDUX_POD_TYPE]) {
            self.add_type_method(heap, ty, id_lut!(is_fn), &[], |vm, args| {
                let sself = script_value!(vm, args.self);
                sself.as_object().is_some_and(|o| vm.bx.heap.is_fn(o)).into()
            });
        }

        // The type predicates (`is_number()`, …) on every type, pods too,
        // each true only on its own type.
        let predicates = types.iter().copied().chain([(ScriptValueType::REDUX_POD, id_lut!(is_pod)), (ScriptValueType::REDUX_POD_TYPE, id_lut!(is_pod_type))]).collect::<Vec<_>>();
        for (ty, id) in predicates.iter().copied() {
            for (receiver, _) in predicates.iter().copied() {
                self.add_type_method(heap, receiver, id, &[], move |_, _| (ty == receiver).into());
            }
        }
    }
}
