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
macro_rules! script_value_f64 {
    ($ctx:ident, $args:ident.$id: ident) => {
        $ctx.heap.cast_to_f64(
            $ctx.heap.value($args, id!($id).into(), &$ctx.thread.trap),
            $ctx.thread.trap.ip,
        )
    };
    ($ctx:ident, $obj:ident[$index: expr]) => {
        $ctx.heap.cast_to_f64(
            $ctx.heap.vec_value($obj, ($index) as usize),
            $ctx.thread.ip(),
        )
    };
}

#[macro_export]
macro_rules! script_value_bool {
    ($ctx:ident, $args:ident.$id: ident) => {
        $ctx.heap.cast_to_bool(
            $ctx.heap.value($args, id!($id).into(), NIL),
            $ctx.thread.ip(),
        )
    };
    ($ctx:ident, $obj:ident[$index: expr]) => {
        $ctx.heap.cast_to_bool(
            $ctx.heap.vec_value($obj, ($index) as usize),
            $ctx.thread.ip(),
        )
    };
}

#[macro_export]
macro_rules! script_value {
    ($vm:ident, $obj:ident.$id: ident) => {
        $vm.bx.heap.value(
            ($obj).into(),
            id!($id).into(),
            $vm.bx.threads.cur_ref().trap.pass(),
        )
    };
    ($vm:ident, $obj:ident.$id:ident.$id2:ident) => {
        $vm.bx.heap.value(
            $vm.bx
                .heap
                .value(
                    ($obj).into(),
                    id!($id).into(),
                    $vm.bx.threads.cur_ref().trap.pass(),
                )
                .into(),
            id!($id2).into(),
            $vm.bx.threads.cur_ref().trap.pass(),
        )
    };
    ($vm:ident, $obj:ident[$index: expr]) => {
        $vm.bx.heap.vec_value(
            ($obj).into(),
            ($index) as usize,
            $vm.bx.threads.cur_ref().trap.pass(),
        )
    };
    ($vm:ident, $obj:ident as array[$index: expr]) => {
        $vm.bx.heap.array_index(
            ($obj).into(),
            ($index) as usize,
            $vm.bx.threads.cur_ref().trap.pass(),
        )
    };
}

#[macro_export]
macro_rules! script_has_proto {
    ($vm:ident, $what:ident, $obj:ident.$id: ident) => {{
        let proto = $vm.bx.heap.value(
            ($obj).into(),
            id!($id).into(),
            $vm.bx.threads.cur_ref().trap.pass(),
        );
        $vm.bx.heap.has_proto(($what).into(), proto)
    }};
}

#[macro_export]
macro_rules! script_is_fn {
    ($vm:ident, $what:ident, $obj:expr) => {{
        $vm.bx.heap.is_fn(($obj).into())
    }};
}

#[macro_export]
macro_rules! script_array_index {
    ($vm:ident, $obj:ident[$index: expr]) => {{
        let trap = $vm.bx.threads.cur().trap.pass();
        $vm.bx
            .heap
            .array_index(($obj).into(), ($index) as usize, trap)
    }};
}

#[macro_export]
macro_rules! set_script_value {
    ($vm:ident, $obj:ident.$id: ident=$value:expr) => {{
        let trap = $vm.bx.threads.cur().trap.pass();
        $vm.bx
            .heap
            .set_value($obj, id!($id).into(), ($value).into(), trap)
    }};
    ($vm:ident, $obj:ident[$index: expr]=$value:expr) => {{
        let trap = $vm.bx.threads.cur().trap.pass();
        $vm.bx
            .heap
            .set_vec_value($obj, ($index) as usize, ($value).into(), trap)
    }};
}

#[macro_export]
macro_rules! set_script_value_to_api {
    ($vm:ident, $obj:ident.$id: ident=$val:expr) => {{
        let v = $val::script_api($vm);
        let trap = $vm.bx.threads.cur().trap.pass();
        $vm.bx
            .heap
            .set_value(($obj).into(), id_lut!($id).into(), v, trap);
    }};
    ($vm:ident, $obj:ident.$id: ident) => {{
        let v = $id::script_api($vm);
        let trap = $vm.bx.threads.cur().trap.pass();
        $vm.bx
            .heap
            .set_value(($obj).into(), id_lut!($id).into(), v, trap);
    }};
}

#[macro_export]
macro_rules! set_script_value_to_pod {
    ($vm:ident, $obj:ident.$id: ident=$val:expr) => {{
        let v = $val::script_pod($vm).expect("Cant make a pod type");
        $vm.bx.heap.pod_type_name_set(v, id_lut!($id));
        $vm.bx.heap.set_value(
            ($obj).into(),
            id_lut!($id).into(),
            v.into(),
            $vm.bx.threads.cur_ref().trap.pass(),
        );
    }};
    ($vm:ident, $obj:ident.$id: ident) => {{
        let v = $id::script_pod($vm).expect("Cant make a pod type");
        $vm.bx.heap.pod_type_name_set(v, id_lut!($id));
        $vm.bx.heap.set_value(
            ($obj).into(),
            id_lut!($id).into(),
            v.into(),
            $vm.bx.threads.cur_ref().trap.pass(),
        );
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

pub type NativeGetterFn = Box<dyn Fn(&mut ScriptVm, ScriptValue, LiveId) -> ScriptValue + 'static>;
pub type NativeSetterFn =
    Box<dyn Fn(&mut ScriptVm, ScriptValue, LiveId, ScriptValue) -> ScriptValue + 'static>;
pub type NativeFn = Box<dyn Fn(&mut ScriptVm, ScriptObject) -> ScriptValue + 'static>;

#[derive(Default)]
pub struct ScriptNative {
    pub(crate) functions: Vec<NativeFn>,
    pub(crate) type_table: Vec<LiveIdMap<LiveId, ScriptObject>>,
    pub(crate) handle_type: LiveIdMap<LiveId, ScriptHandleType>,
    pub(crate) getters: Vec<NativeGetterFn>,
    pub(crate) setters: Vec<NativeSetterFn>,
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
        let boxed: NativeFn = Box::new(f);
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
    pub fn add_apply_transform_fn<F>(&mut self, f: F) -> NativeId
    where
        F: Fn(&mut ScriptVm, ScriptObject) -> ScriptValue + 'static,
    {
        let fn_index = self.functions.len();
        self.functions.push(Box::new(f));
        NativeId {
            index: fn_index as u32,
        }
    }

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
        // lets get the
        let fn_obj = self.add_fn(heap, args, f);
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

    pub fn set_type_getter<F>(&mut self, ty_redux: ScriptTypeRedux, f: F)
    where
        F: Fn(&mut ScriptVm, ScriptValue, LiveId) -> ScriptValue + 'static,
    {
        self.getters[ty_redux.to_index()] = Box::new(f)
    }

    pub fn set_type_setter<F>(&mut self, ty_redux: ScriptTypeRedux, f: F)
    where
        F: Fn(&mut ScriptVm, ScriptValue, LiveId, ScriptValue) -> ScriptValue + 'static,
    {
        self.setters[ty_redux.to_index()] = Box::new(f)
    }

    /// Ensures capacity for type tables - non-generic to reduce monomorphization
    #[inline(never)]
    fn ensure_type_table_capacity(&mut self, ty_redux: ScriptTypeRedux) {
        if ty_redux.to_index() as usize >= self.type_table.len() {
            self.type_table
                .resize_with(ty_redux.to_index() + 1, || Default::default());
            self.getters.resize_with(ty_redux.to_index() + 1, || {
                Box::new(|vm, value, field| {
                    script_err_not_found!(
                        vm.bx.threads.cur_ref().trap,
                        "no getter for field {:?} on type {:?}",
                        field,
                        value.value_type()
                    )
                })
            });
            self.setters.resize_with(ty_redux.to_index() + 1, || {
                Box::new(|vm, value, field, _| {
                    script_err_not_found!(
                        vm.bx.threads.cur_ref().trap,
                        "no setter for field {:?} on type {:?}",
                        field,
                        value.value_type()
                    )
                })
            });
        }
    }

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
        let fn_obj = self.add_fn(heap, args, f);
        self.ensure_type_table_capacity(ty_redux);
        self.type_table[ty_redux.to_index()].insert(method, fn_obj);
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
                    vm.bx.heap.new_string_with(|heap, out| {
                        heap.cast_to_string(sself, out);
                    })
                });
            }
        }

        for (ty, id) in types {
            self.add_type_method(heap, ScriptValueType::REDUX_NUMBER, id, &[], move |_, _| {
                (ty == ScriptValueType::REDUX_NUMBER).into()
            });
            self.add_type_method(heap, ScriptValueType::REDUX_NAN, id, &[], move |_, _| {
                (ty == ScriptValueType::REDUX_NAN).into()
            });
            self.add_type_method(heap, ScriptValueType::REDUX_BOOL, id, &[], move |_, _| {
                (ty == ScriptValueType::REDUX_BOOL).into()
            });
            self.add_type_method(heap, ScriptValueType::REDUX_NIL, id, &[], move |_, _| {
                (ty == ScriptValueType::REDUX_NIL).into()
            });
            self.add_type_method(heap, ScriptValueType::REDUX_COLOR, id, &[], move |_, _| {
                (ty == ScriptValueType::REDUX_COLOR).into()
            });
            self.add_type_method(heap, ScriptValueType::REDUX_STRING, id, &[], move |_, _| {
                (ty == ScriptValueType::REDUX_STRING).into()
            });
            self.add_type_method(heap, ScriptValueType::REDUX_OBJECT, id, &[], move |_, _| {
                (ty == ScriptValueType::REDUX_OBJECT).into()
            });
            self.add_type_method(heap, ScriptValueType::REDUX_ARRAY, id, &[], move |_, _| {
                (ty == ScriptValueType::REDUX_ARRAY).into()
            });
            self.add_type_method(heap, ScriptValueType::REDUX_REGEX, id, &[], move |_, _| {
                (ty == ScriptValueType::REDUX_REGEX).into()
            });
            self.add_type_method(heap, ScriptValueType::REDUX_OPCODE, id, &[], move |_, _| {
                (ty == ScriptValueType::REDUX_OPCODE).into()
            });
            self.add_type_method(heap, ScriptValueType::REDUX_ERR, id, &[], move |_, _| {
                (ty == ScriptValueType::REDUX_ERR).into()
            });
            self.add_type_method(heap, ScriptValueType::REDUX_ID, id, &[], move |_, _| {
                (ty == ScriptValueType::REDUX_ID).into()
            });
        }
    }
}
