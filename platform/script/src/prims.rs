use crate::apply::*;
use crate::function::*;
use crate::handle::*;
use crate::heap::*;
use crate::object::*;
use crate::pod::*;
use crate::traits::*;
use crate::value::*;
use crate::vm::*;
use crate::*;
use makepad_live_id::*;

#[macro_export]
macro_rules!script_primitive {
    ( $ ty: ty, $new:item, $ type_check: item, $ apply: item, $ to_value: item) => {
        impl ScriptHook for $ty{}
        impl ScriptNew for $ty{
            fn script_type_id_static()->ScriptTypeId{
                const ID: ScriptTypeId = ScriptTypeId::of_path(concat!("makepad_script::prim::", stringify!($ty)));
                ID
            }
            $ new
            $ type_check
            fn script_proto(vm:&mut ScriptVm)->ScriptValue{
                script_proto_inner(vm, Self::script_type_id_static(), script_proto_build_c::<Self>, script_type_check_c::<Self>, Some(stringify!($ty)), false)
            }
            fn script_default(vm:&mut ScriptVm)->ScriptValue{Self::script_new(vm).script_to_value(vm)}
            fn script_proto_build(vm:&mut ScriptVm, _props:&mut ScriptTypeProps)->ScriptValue{
                 Self::script_default(vm)
            }
        }
        impl ScriptApply for $ty{
            fn script_type_id(&self)->ScriptTypeId{<Self as ScriptNew>::script_type_id_static()}
            $apply
            $to_value
        }
    }
}

trait CompactScriptPrimitive: Copy + Default {
    type Logical: ScriptNew + ScriptApply;

    fn packed_type() -> ScriptPodPacked;
    fn pod_type(vm: &ScriptVm) -> ScriptPodType;
    fn from_words(words: &[u32]) -> Self;
    fn to_words(self) -> Vec<u32>;
    fn from_logical(value: Self::Logical) -> Self;
    fn to_logical(self) -> Self::Logical;
}

macro_rules! script_compact_primitive {
    ($ty:ty) => {
        impl ScriptHook for $ty {}

        impl ScriptNew for $ty {
            fn script_type_id_static() -> ScriptTypeId {
                const ID: ScriptTypeId = ScriptTypeId::of_path(concat!("makepad_script::prim::", stringify!($ty)));
                ID
            }

            fn script_new(_vm: &mut ScriptVm) -> Self {
                Self::default()
            }

            fn script_proto(vm: &mut ScriptVm) -> ScriptValue {
                script_proto_inner(
                    vm,
                    Self::script_type_id_static(),
                    script_proto_build_c::<Self>,
                    script_type_check_c::<Self>,
                    Some(stringify!($ty)),
                    false,
                )
            }

            fn script_type_check(heap: &ScriptHeap, value: ScriptValue) -> bool {
                if let Some(pod) = value.as_pod() {
                    let (pod_type, _) = heap.pod_data(pod);
                    if pod_type.ty().packed() == Some(<Self as CompactScriptPrimitive>::packed_type()) {
                        return true;
                    }
                }
                <<Self as CompactScriptPrimitive>::Logical as ScriptNew>::script_type_check(
                    heap, value,
                )
            }

            fn script_default(vm: &mut ScriptVm) -> ScriptValue {
                Self::default().script_to_value(vm)
            }

            fn script_proto_build(vm: &mut ScriptVm, _props: &mut ScriptTypeProps) -> ScriptValue {
                Self::script_default(vm)
            }
        }

        impl ScriptApply for $ty {
            fn script_type_id(&self) -> ScriptTypeId {
                <Self as ScriptNew>::script_type_id_static()
            }

            fn script_apply(
                &mut self,
                vm: &mut ScriptVm,
                apply: &Apply,
                scope: &mut Scope,
                value: ScriptValue,
            ) {
                if let Some(pod) = value.as_pod() {
                    let view = vm.heap().pod_view(crate::pod_heap::POD_VIEW_DATA, pod, ScriptPodType::default());
                    if view.id == <Self as CompactScriptPrimitive>::pod_type(vm) {
                        // SAFETY: the words are the pod's data, borrowed from the heap
                        *self = <Self as CompactScriptPrimitive>::from_words(unsafe { std::slice::from_raw_parts(view.ptr, view.len) });
                        return;
                    }
                }
                let mut logical = <Self as CompactScriptPrimitive>::to_logical(*self);
                logical.script_apply(vm, apply, scope, value);
                *self = <Self as CompactScriptPrimitive>::from_logical(logical);
            }

            fn script_to_value(&self, vm: &mut ScriptVm) -> ScriptValue {
                let pod_type = <Self as CompactScriptPrimitive>::pod_type(vm);
                let pod = vm.heap_mut().new_pod(pod_type);
                vm.heap_mut().pod_data_mut(pod).copy_from_slice(&<Self as CompactScriptPrimitive>::to_words(*self));
                pod.into()
            }
        }

    };
}

impl CompactScriptPrimitive for makepad_math::F16x2 {
    type Logical = makepad_math::Vec2f;
    fn packed_type() -> ScriptPodPacked { ScriptPodPacked::F16x2 }
    fn pod_type(vm: &ScriptVm) -> ScriptPodType { vm.pod_builtins().pod_f16x2 }
    fn from_words(v: &[u32]) -> Self {
        let v = v.first().copied().unwrap_or(0);
        Self { x: v as u16, y: (v >> 16) as u16 }
    }
    fn to_words(self) -> Vec<u32> { vec![self.x as u32 | ((self.y as u32) << 16)] }
    fn from_logical(v: Self::Logical) -> Self { Self::from_f32(v.x, v.y) }
    fn to_logical(self) -> Self::Logical {
        let (x, y) = self.to_f32();
        makepad_math::Vec2f { x, y }
    }
}

impl CompactScriptPrimitive for makepad_math::F16x4 {
    type Logical = makepad_math::Vec4f;
    fn packed_type() -> ScriptPodPacked { ScriptPodPacked::F16x4 }
    fn pod_type(vm: &ScriptVm) -> ScriptPodType { vm.pod_builtins().pod_f16x4 }
    fn from_words(v: &[u32]) -> Self {
        let a = v.first().copied().unwrap_or(0);
        let b = v.get(1).copied().unwrap_or(0);
        Self { x: a as u16, y: (a >> 16) as u16, z: b as u16, w: (b >> 16) as u16 }
    }
    fn to_words(self) -> Vec<u32> {
        vec![self.x as u32 | ((self.y as u32) << 16), self.z as u32 | ((self.w as u32) << 16)]
    }
    fn from_logical(v: Self::Logical) -> Self { Self::from_f32(v.x, v.y, v.z, v.w) }
    fn to_logical(self) -> Self::Logical {
        let (x, y, z, w) = self.to_f32();
        makepad_math::Vec4f { x, y, z, w }
    }
}

macro_rules! impl_compact_u16x2 {
    ($ty:ty, $packed:expr, $builtin:ident, $from:expr, $to:expr) => {
        impl CompactScriptPrimitive for $ty {
            type Logical = makepad_math::Vec2f;
            fn packed_type() -> ScriptPodPacked { $packed }
            fn pod_type(vm: &ScriptVm) -> ScriptPodType { vm.pod_builtins().$builtin }
            fn from_words(v: &[u32]) -> Self {
                let word = v.first().copied().unwrap_or(0);
                ($from)(word)
            }
            fn to_words(self) -> Vec<u32> { vec![($to)(self)] }
            fn from_logical(v: Self::Logical) -> Self { Self::from_f32(v.x, v.y) }
            fn to_logical(self) -> Self::Logical {
                let (x, y) = self.to_f32();
                makepad_math::Vec2f { x, y }
            }
        }
    };
}

impl_compact_u16x2!(
    makepad_math::UNorm16x2,
    ScriptPodPacked::U16x2Norm,
    pod_unorm16x2,
    |v: u32| makepad_math::UNorm16x2 { x: v as u16, y: (v >> 16) as u16 },
    |v: makepad_math::UNorm16x2| v.x as u32 | ((v.y as u32) << 16)
);
impl_compact_u16x2!(
    makepad_math::SNorm16x2,
    ScriptPodPacked::I16x2Norm,
    pod_snorm16x2,
    |v: u32| makepad_math::SNorm16x2 { x: v as u16 as i16, y: (v >> 16) as u16 as i16 },
    |v: makepad_math::SNorm16x2| v.x as u16 as u32 | ((v.y as u16 as u32) << 16)
);

impl CompactScriptPrimitive for makepad_math::U16x2 {
    type Logical = makepad_math::Vec2f;
    fn packed_type() -> ScriptPodPacked { ScriptPodPacked::U16x2 }
    fn pod_type(vm: &ScriptVm) -> ScriptPodType { vm.pod_builtins().pod_u16x2 }
    fn from_words(v: &[u32]) -> Self {
        let v = v.first().copied().unwrap_or(0);
        Self { x: v as u16, y: (v >> 16) as u16 }
    }
    fn to_words(self) -> Vec<u32> { vec![self.x as u32 | ((self.y as u32) << 16)] }
    fn from_logical(v: Self::Logical) -> Self { Self::from_f32(v.x, v.y) }
    fn to_logical(self) -> Self::Logical { makepad_math::Vec2f { x: self.x as f32, y: self.y as f32 } }
}

impl CompactScriptPrimitive for makepad_math::I16x2 {
    type Logical = makepad_math::Vec2f;
    fn packed_type() -> ScriptPodPacked { ScriptPodPacked::I16x2 }
    fn pod_type(vm: &ScriptVm) -> ScriptPodType { vm.pod_builtins().pod_i16x2 }
    fn from_words(v: &[u32]) -> Self {
        let v = v.first().copied().unwrap_or(0);
        Self { x: v as u16 as i16, y: (v >> 16) as u16 as i16 }
    }
    fn to_words(self) -> Vec<u32> { vec![self.x as u16 as u32 | ((self.y as u16 as u32) << 16)] }
    fn from_logical(v: Self::Logical) -> Self { Self::from_f32(v.x, v.y) }
    fn to_logical(self) -> Self::Logical { makepad_math::Vec2f { x: self.x as f32, y: self.y as f32 } }
}

macro_rules! impl_compact_i8x4 {
    ($ty:ty, $packed:expr, $builtin:ident) => {
        impl CompactScriptPrimitive for $ty {
            type Logical = makepad_math::Vec4f;
            fn packed_type() -> ScriptPodPacked { $packed }
            fn pod_type(vm: &ScriptVm) -> ScriptPodType { vm.pod_builtins().$builtin }
            fn from_words(v: &[u32]) -> Self {
                let bytes = v.first().copied().unwrap_or(0).to_le_bytes();
                Self(bytes.map(|v| v as _))
            }
            fn to_words(self) -> Vec<u32> {
                vec![u32::from_le_bytes(self.0.map(|v| v as u8))]
            }
            fn from_logical(v: Self::Logical) -> Self { Self::from_f32(v.x, v.y, v.z, v.w) }
            fn to_logical(self) -> Self::Logical {
                let (x, y, z, w) = self.to_f32();
                makepad_math::Vec4f { x, y, z, w }
            }
        }
    };
}

impl_compact_i8x4!(makepad_math::UNorm8x4, ScriptPodPacked::U8x4Norm, pod_unorm8x4);
impl_compact_i8x4!(makepad_math::SNorm8x4, ScriptPodPacked::I8x4Norm, pod_snorm8x4);

script_compact_primitive!(makepad_math::F16x2);
script_compact_primitive!(makepad_math::F16x4);
script_compact_primitive!(makepad_math::U16x2);
script_compact_primitive!(makepad_math::I16x2);
script_compact_primitive!(makepad_math::UNorm16x2);
script_compact_primitive!(makepad_math::SNorm16x2);
script_compact_primitive!(makepad_math::UNorm8x4);
script_compact_primitive!(makepad_math::SNorm8x4);

script_primitive!(
    f32,
    fn script_new(_vm: &mut ScriptVm) -> Self {
        Default::default()
    },
    fn script_type_check(heap: &ScriptHeap, value: ScriptValue) -> bool {
        value.is_number() || value.is_bool() || heap.has_apply_transform(value)
    },
    fn script_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
    ) {
        if let Some(v) = value.as_number() {
            *self = v as _;
            return;
        }
        if let Some(v) = value.as_bool() {
            *self = if v { 1.0 } else { 0.0 };
            return;
        }
        if let Some(transformed) = vm.call_apply_transform(value) {
            return self.script_apply(vm, apply, scope, transformed);
        }
    },
    fn script_to_value(&self, _vm: &mut ScriptVm) -> ScriptValue {
        ScriptValue::from_f64(*self as _)
    }
);

script_primitive!(
    f64,
    fn script_new(_vm: &mut ScriptVm) -> Self {
        Default::default()
    },
    fn script_type_check(heap: &ScriptHeap, value: ScriptValue) -> bool {
        value.is_number() || value.is_bool() || heap.has_apply_transform(value)
    },
    fn script_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
    ) {
        if let Some(v) = value.as_number() {
            *self = v;
            return;
        }
        if let Some(v) = value.as_bool() {
            *self = if v { 1.0 } else { 0.0 };
            return;
        }
        if let Some(transformed) = vm.call_apply_transform(value) {
            return self.script_apply(vm, apply, scope, transformed);
        }
    },
    fn script_to_value(&self, _vm: &mut ScriptVm) -> ScriptValue {
        ScriptValue::from_f64(*self)
    }
);

script_primitive!(
    u64,
    fn script_new(_vm: &mut ScriptVm) -> Self {
        Default::default()
    },
    fn script_type_check(heap: &ScriptHeap, value: ScriptValue) -> bool {
        value.is_number() || value.is_bool() || heap.has_apply_transform(value)
    },
    fn script_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
    ) {
        if let Some(v) = value.as_number() {
            *self = v as u64;
            return;
        }
        if let Some(v) = value.as_bool() {
            *self = if v { 1 } else { 0 };
            return;
        }
        if let Some(transformed) = vm.call_apply_transform(value) {
            return self.script_apply(vm, apply, scope, transformed);
        }
    },
    fn script_to_value(&self, _vm: &mut ScriptVm) -> ScriptValue {
        ScriptValue::from_f64(*self as f64)
    }
);

script_primitive!(
    usize,
    fn script_new(_vm: &mut ScriptVm) -> Self {
        Default::default()
    },
    fn script_type_check(heap: &ScriptHeap, value: ScriptValue) -> bool {
        value.is_number() || value.is_bool() || heap.has_apply_transform(value)
    },
    fn script_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
    ) {
        if let Some(v) = value.as_number() {
            *self = v as usize;
            return;
        }
        if let Some(v) = value.as_bool() {
            *self = if v { 1 } else { 0 };
            return;
        }
        if let Some(transformed) = vm.call_apply_transform(value) {
            return self.script_apply(vm, apply, scope, transformed);
        }
    },
    fn script_to_value(&self, _vm: &mut ScriptVm) -> ScriptValue {
        ScriptValue::from_f64(*self as f64)
    }
);

script_primitive!(
    ScriptObjectRef,
    fn script_new(vm: &mut ScriptVm) -> Self {
        vm.heap_mut().new_object_ref(ScriptObject::ZERO)
    },
    fn script_type_check(_heap: &ScriptHeap, value: ScriptValue) -> bool {
        value.is_object()
    },
    fn script_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) {
        if let Some(obj) = value.as_object() {
            *self = vm.heap_mut().new_object_ref(obj)
        }
    },
    fn script_to_value(&self, _vm: &mut ScriptVm) -> ScriptValue {
        self.as_object().into()
    }
);

script_primitive!(
    ScriptFnRef,
    fn script_new(vm: &mut ScriptVm) -> Self {
        vm.heap_mut().new_fn_ref(ScriptObject::ZERO)
    },
    fn script_type_check(heap: &ScriptHeap, value: ScriptValue) -> bool {
        if let Some(obj) = value.as_object() {
            heap.is_fn(obj)
        } else {
            false
        }
    },
    fn script_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) {
        if let Some(obj) = value.as_object() {
            if vm.heap_mut().is_fn(obj) {
                *self = vm.heap_mut().new_fn_ref(obj)
            }
        }
    },
    fn script_to_value(&self, _vm: &mut ScriptVm) -> ScriptValue {
        self.as_object().into()
    }
);

script_primitive!(
    ScriptHandleRef,
    fn script_new(vm: &mut ScriptVm) -> Self {
        vm.heap_mut().new_handle_ref(ScriptHandle::ZERO)
    },
    fn script_type_check(_heap: &ScriptHeap, value: ScriptValue) -> bool {
        value.as_handle().is_some()
    },
    fn script_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) {
        if let Some(handle) = value.as_handle() {
            *self = vm.heap_mut().new_handle_ref(handle)
        }
    },
    fn script_to_value(&self, _vm: &mut ScriptVm) -> ScriptValue {
        self.as_handle().into()
    }
);

script_primitive!(
    u32,
    fn script_new(_vm: &mut ScriptVm) -> Self {
        Default::default()
    },
    fn script_type_check(heap: &ScriptHeap, value: ScriptValue) -> bool {
        value.is_number() || value.is_bool() || heap.has_apply_transform(value)
    },
    fn script_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
    ) {
        if let Some(v) = value.as_number() {
            *self = v as u32;
            return;
        }
        if let Some(v) = value.as_bool() {
            *self = if v { 1 } else { 0 };
            return;
        }
        if let Some(transformed) = vm.call_apply_transform(value) {
            return self.script_apply(vm, apply, scope, transformed);
        }
    },
    fn script_to_value(&self, _vm: &mut ScriptVm) -> ScriptValue {
        ScriptValue::from_f64(*self as f64)
    }
);

script_primitive!(
    u16,
    fn script_new(_vm: &mut ScriptVm) -> Self {
        Default::default()
    },
    fn script_type_check(heap: &ScriptHeap, value: ScriptValue) -> bool {
        value.is_number() || value.is_bool() || heap.has_apply_transform(value)
    },
    fn script_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
    ) {
        if let Some(v) = value.as_number() {
            *self = v as u16;
            return;
        }
        if let Some(v) = value.as_bool() {
            *self = if v { 1 } else { 0 };
            return;
        }
        if let Some(transformed) = vm.call_apply_transform(value) {
            return self.script_apply(vm, apply, scope, transformed);
        }
    },
    fn script_to_value(&self, _vm: &mut ScriptVm) -> ScriptValue {
        ScriptValue::from_f64(*self as f64)
    }
);

script_primitive!(
    bool,
    fn script_new(_vm: &mut ScriptVm) -> Self {
        Default::default()
    },
    fn script_type_check(_heap: &ScriptHeap, value: ScriptValue) -> bool {
        value.is_bool()
    },
    fn script_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) {
        *self = vm.heap_mut().cast_to_bool(value);
    },
    fn script_to_value(&self, _vm: &mut ScriptVm) -> ScriptValue {
        ScriptValue::from_bool(*self)
    }
);

script_primitive!(
    String,
    fn script_new(_vm: &mut ScriptVm) -> Self {
        Default::default()
    },
    fn script_type_check(_heap: &ScriptHeap, value: ScriptValue) -> bool {
        value.is_string_like()
    },
    fn script_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) {
        // Same rationale as `ArcStringMut::script_apply`: text-bearing fields
        // (`TextInput.text`, `TextInput.empty_text`, etc.) are canonically
        // mutated through imperative setters at runtime. A re-walk that
        // carries no authored change (`cx.request_script_reapply()` preference
        // broadcast, or the `script_mod` re-run a safe-area inset change
        // forces) would otherwise clobber that runtime value with the stale
        // DSL literal. Strings are not part of the shared-heap-object
        // propagation pipeline (which uses `Size`/numerics), so bailing here
        // is safe.
        if apply.preserves_runtime_state() {
            return;
        }
        self.clear();
        vm.heap().cast_to_string(value, self);
    },
    fn script_to_value(&self, vm: &mut ScriptVm) -> ScriptValue {
        if let Some(val) = ScriptValue::from_inline_string(&self) {
            return val;
        } else {
            vm.new_string(self).into()
        }
    }
);
impl ScriptDeriveMarker for String {}

script_primitive!(
    &'static str,
    fn script_new(_vm: &mut ScriptVm) -> Self {
        Default::default()
    },
    fn script_type_check(_heap: &ScriptHeap, value: ScriptValue) -> bool {
        value.is_string_like()
    },
    fn script_apply(
        &mut self,
        _vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
    },
    fn script_to_value(&self, vm: &mut ScriptVm) -> ScriptValue {
        if let Some(val) = ScriptValue::from_inline_string(&self) {
            return val;
        } else {
            vm.new_string(self).into()
        }
    }
);
impl ScriptDeriveMarker for &'static str {}

script_primitive!(
    LiveId,
    fn script_new(_vm: &mut ScriptVm) -> Self {
        Default::default()
    },
    // `nil` is the empty id, so an id a script set can be taken back the
    // way an Option is: `container_id: nil`.
    fn script_type_check(_heap: &ScriptHeap, value: ScriptValue) -> bool {
        value.is_id() || value.is_nil()
    },
    fn script_apply(
        &mut self,
        _vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) {
        if value.is_nil() {
            *self = LiveId(0)
        } else if let Some(id) = value.as_id() {
            *self = id
        }
    },
    fn script_to_value(&self, _vm: &mut ScriptVm) -> ScriptValue {
        self.into()
    }
);
impl ScriptDeriveMarker for LiveId {}

script_primitive!(
    ScriptObject,
    fn script_new(_vm: &mut ScriptVm) -> Self {
        Default::default()
    },
    fn script_type_check(_heap: &ScriptHeap, value: ScriptValue) -> bool {
        value.is_object()
    },
    fn script_apply(
        &mut self,
        _vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) {
        if let Some(object) = value.as_object() {
            *self = object
        }
    },
    fn script_to_value(&self, _vm: &mut ScriptVm) -> ScriptValue {
        (*self).into()
    }
);
//impl ScriptDeriveMarker for ScriptObject{}

script_primitive!(
    ScriptValue,
    fn script_new(_vm: &mut ScriptVm) -> Self {
        Default::default()
    },
    fn script_type_check(_heap: &ScriptHeap, _value: ScriptValue) -> bool {
        true
    },
    fn script_apply(
        &mut self,
        _vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) {
        *self = value
    },
    fn script_to_value(&self, _vm: &mut ScriptVm) -> ScriptValue {
        *self
    }
);
//impl ScriptDeriveMarker for ScriptValue{}

// LiveIdMap<K, V> implementation
impl<K, V> ScriptHook for LiveIdMap<K, V>
where
    K: ScriptApply
        + ScriptNew
        + 'static
        + std::cmp::Eq
        + std::hash::Hash
        + Copy
        + From<LiveId>
        + std::fmt::Debug,
    V: ScriptApply + ScriptNew + 'static,
{
}

impl<K, V> ScriptNew for LiveIdMap<K, V>
where
    K: ScriptApply
        + ScriptNew
        + 'static
        + std::cmp::Eq
        + std::hash::Hash
        + Copy
        + From<LiveId>
        + std::fmt::Debug,
    V: ScriptApply + ScriptNew + 'static,
{
    fn script_type_id_static() -> ScriptTypeId {
        ScriptTypeId::of_path("makepad_live_id::LiveIdMap").with_args(&[K::script_type_id_static(), V::script_type_id_static()])
    }
    fn script_type_check(heap: &ScriptHeap, value: ScriptValue) -> bool {
        if let Some(obj) = value.as_object() {
            for e in heap.map_entries(obj) {
                if !K::script_type_check(heap, e.key) {
                    return false;
                }
                if !V::script_type_check(heap, e.value) {
                    return false;
                }
            }
            return true;
        }
        value.is_nil() || heap.has_apply_transform(value)
    }
    fn script_default(_vm: &mut ScriptVm) -> ScriptValue {
        NIL
    }
    fn script_new(_vm: &mut ScriptVm) -> Self {
        Default::default()
    }
    fn script_proto_build(_vm: &mut ScriptVm, _props: &mut ScriptTypeProps) -> ScriptValue {
        NIL
    }
}

impl<K, V> ScriptApply for LiveIdMap<K, V>
where
    K: ScriptApply
        + ScriptNew
        + 'static
        + std::cmp::Eq
        + std::hash::Hash
        + Copy
        + From<LiveId>
        + std::fmt::Debug,
    V: ScriptApply + ScriptNew + 'static,
{
    fn script_type_id(&self) -> ScriptTypeId {
        <Self as ScriptNew>::script_type_id_static()
    }
    fn script_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
    ) {
        // Check for apply transform
        if let Some(transformed) = vm.call_apply_transform(value) {
            return self.script_apply(vm, apply, scope, transformed);
        }
        if let Some(obj) = value.as_object() {
            self.clear();
            for e in vm.heap().map_entries(obj) {
                let key = K::script_from_value(vm, e.key);
                let value = V::script_from_value(vm, e.value);
                self.insert(key, value);
            }
        } else if value.is_nil() {
            self.clear()
        } else {
            script_err_type_mismatch!(vm.trap(), "wrong type in apply");
        }
    }
    fn script_to_value(&self, vm: &mut ScriptVm) -> ScriptValue {
        let obj = vm.heap_mut().new_object();
        for (key, value) in self.iter() {
            let key = key.script_to_value(vm);
            let value = value.script_to_value(vm);
            if !vm.heap_mut().charge_object_map_entry(
                obj,
                key,
                "converting a Rust map to a script object",
            ) {
                continue;
            }
            vm.heap_mut().map_insert_own(obj, key, value);
        }
        obj.into()
    }
}

script_primitive!(
    makepad_math::Vec2d,
    fn script_new(_vm: &mut ScriptVm) -> Self {
        Default::default()
    },
    fn script_type_check(heap: &ScriptHeap, value: ScriptValue) -> bool {
        if value.is_number() {
            return true;
        }
        if let Some(pod) = value.as_pod() {
            let (pod_type, _) = heap.pod_data(pod);
            if let Some(v) = pod_type.ty().as_vec() {
                return v.dims() == 2;
            }
        }
        heap.has_apply_transform(value)
    },
    fn script_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
    ) {
        if let Some(v) = value.as_number() {
            self.x = v;
            self.y = v;
            return;
        }
        if let Some(pod) = value.as_pod() {
            let (pod_type, pod_words) = vm.heap_mut().pod_data(pod);
            if let Some(v) = pod_type.ty().as_vec() {
                if v.dims() == 2 {
                    match v {
                        ScriptPodVec::Vec2f => {
                            self.x = f32::from_bits(pod_words[0]) as f64;
                            self.y = f32::from_bits(pod_words[1]) as f64;
                        }
                        ScriptPodVec::Vec2i => {
                            self.x = pod_words[0] as i32 as f64;
                            self.y = pod_words[1] as i32 as f64;
                        }
                        ScriptPodVec::Vec2u => {
                            self.x = pod_words[0] as f64;
                            self.y = pod_words[1] as f64;
                        }
                        ScriptPodVec::Vec2b => {
                            self.x = if pod_words[0] != 0 { 1.0 } else { 0.0 };
                            self.y = if pod_words[1] != 0 { 1.0 } else { 0.0 };
                        }
                        ScriptPodVec::Vec2h => {
                            self.x = crate::pod_heap::f16_to_f32(pod_words[0] as u16) as f64;
                            self.y =
                                crate::pod_heap::f16_to_f32((pod_words[0] >> 16) as u16) as f64;
                        }
                        _ => (),
                    }
                    return;
                }
            }
        }
        // Try apply transform at the bottom
        if let Some(transformed) = vm.call_apply_transform(value) {
            return self.script_apply(vm, apply, scope, transformed);
        }
    },
    fn script_to_value(&self, vm: &mut ScriptVm) -> ScriptValue {
        let pod_type = vm.pod_builtins().pod_vec2f;
        let pod = vm.heap_mut().new_pod(pod_type);
        let pod_words = vm.heap_mut().pod_data_mut(pod);
        pod_words[0] = (self.x as f32).to_bits();
        pod_words[1] = (self.y as f32).to_bits();
        pod.into()
    }
);

script_primitive!(
    makepad_math::Vec2f,
    fn script_new(_vm: &mut ScriptVm) -> Self {
        Default::default()
    },
    fn script_type_check(heap: &ScriptHeap, value: ScriptValue) -> bool {
        if value.is_number() {
            return true;
        }
        if let Some(pod) = value.as_pod() {
            let (pod_type, _) = heap.pod_data(pod);
            if let Some(v) = pod_type.ty().as_vec() {
                return v.dims() == 2;
            }
        }
        heap.has_apply_transform(value)
    },
    fn script_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
    ) {
        if let Some(v) = value.as_number() {
            self.x = v as f32;
            self.y = v as f32;
            return;
        }
        if let Some(pod) = value.as_pod() {
            let (pod_type, pod_words) = vm.heap_mut().pod_data(pod);
            if let Some(v) = pod_type.ty().as_vec() {
                if v.dims() == 2 {
                    match v {
                        ScriptPodVec::Vec2f => {
                            self.x = f32::from_bits(pod_words[0]);
                            self.y = f32::from_bits(pod_words[1]);
                        }
                        ScriptPodVec::Vec2i => {
                            self.x = pod_words[0] as i32 as f32;
                            self.y = pod_words[1] as i32 as f32;
                        }
                        ScriptPodVec::Vec2u => {
                            self.x = pod_words[0] as f32;
                            self.y = pod_words[1] as f32;
                        }
                        ScriptPodVec::Vec2b => {
                            self.x = if pod_words[0] != 0 { 1.0 } else { 0.0 };
                            self.y = if pod_words[1] != 0 { 1.0 } else { 0.0 };
                        }
                        ScriptPodVec::Vec2h => {
                            self.x = crate::pod_heap::f16_to_f32(pod_words[0] as u16);
                            self.y = crate::pod_heap::f16_to_f32((pod_words[0] >> 16) as u16);
                        }
                        _ => (),
                    }
                    return;
                }
            }
        }
        // Try apply transform at the bottom
        if let Some(transformed) = vm.call_apply_transform(value) {
            return self.script_apply(vm, apply, scope, transformed);
        }
    },
    fn script_to_value(&self, vm: &mut ScriptVm) -> ScriptValue {
        let pod_type = vm.pod_builtins().pod_vec2f;
        let pod = vm.heap_mut().new_pod(pod_type);
        let pod_words = vm.heap_mut().pod_data_mut(pod);
        pod_words[0] = (self.x).to_bits();
        pod_words[1] = (self.y).to_bits();
        pod.into()
    }
);

script_primitive!(
    makepad_math::Vec3f,
    fn script_new(_vm: &mut ScriptVm) -> Self {
        Default::default()
    },
    fn script_type_check(heap: &ScriptHeap, value: ScriptValue) -> bool {
        if value.is_number() {
            return true;
        }
        if value.is_color() {
            return true;
        }
        if let Some(pod) = value.as_pod() {
            let (pod_type, _) = heap.pod_data(pod);
            if let Some(v) = pod_type.ty().as_vec() {
                return v.dims() == 3;
            }
        }
        heap.has_apply_transform(value)
    },
    fn script_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
    ) {
        if let Some(v) = value.as_number() {
            self.x = v as f32;
            self.y = v as f32;
            self.z = v as f32;
            return;
        }
        if let Some(c) = value.as_color() {
            let v = makepad_math::Vec4f::from_u32(c);
            self.x = v.x;
            self.y = v.y;
            self.z = v.z;
            return;
        }
        if let Some(pod) = value.as_pod() {
            let (pod_type, pod_words) = vm.heap_mut().pod_data(pod);
            if let Some(v) = pod_type.ty().as_vec() {
                if v.dims() == 3 {
                    match v {
                        ScriptPodVec::Vec3f => {
                            self.x = f32::from_bits(pod_words[0]);
                            self.y = f32::from_bits(pod_words[1]);
                            self.z = f32::from_bits(pod_words[2]);
                        }
                        ScriptPodVec::Vec3i => {
                            self.x = pod_words[0] as i32 as f32;
                            self.y = pod_words[1] as i32 as f32;
                            self.z = pod_words[2] as i32 as f32;
                        }
                        ScriptPodVec::Vec3u => {
                            self.x = pod_words[0] as f32;
                            self.y = pod_words[1] as f32;
                            self.z = pod_words[2] as f32;
                        }
                        ScriptPodVec::Vec3b => {
                            self.x = if pod_words[0] != 0 { 1.0 } else { 0.0 };
                            self.y = if pod_words[1] != 0 { 1.0 } else { 0.0 };
                            self.z = if pod_words[2] != 0 { 1.0 } else { 0.0 };
                        }
                        ScriptPodVec::Vec3h => {
                            self.x = crate::pod_heap::f16_to_f32(pod_words[0] as u16);
                            self.y = crate::pod_heap::f16_to_f32((pod_words[0] >> 16) as u16);
                            self.z = crate::pod_heap::f16_to_f32(pod_words[1] as u16);
                        }
                        _ => (),
                    }
                    return;
                }
            }
        }
        // Try apply transform at the bottom
        if let Some(transformed) = vm.call_apply_transform(value) {
            return self.script_apply(vm, apply, scope, transformed);
        }
    },
    fn script_to_value(&self, vm: &mut ScriptVm) -> ScriptValue {
        let pod_type = vm.pod_builtins().pod_vec3f;
        let pod = vm.heap_mut().new_pod(pod_type);
        let pod_words = vm.heap_mut().pod_data_mut(pod);
        pod_words[0] = (self.x).to_bits();
        pod_words[1] = (self.y).to_bits();
        pod_words[2] = (self.z).to_bits();
        pod.into()
    }
);

script_primitive!(
    makepad_math::Vec4f,
    fn script_new(_vm: &mut ScriptVm) -> Self {
        Default::default()
    },
    fn script_type_check(heap: &ScriptHeap, value: ScriptValue) -> bool {
        if value.is_number() {
            return true;
        }
        if value.is_color() {
            return true;
        }
        if let Some(pod) = value.as_pod() {
            let (pod_type, _) = heap.pod_data(pod);
            if let Some(v) = pod_type.ty().as_vec() {
                return v.dims() == 4;
            }
        }
        heap.has_apply_transform(value)
    },
    fn script_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
    ) {
        if let Some(v) = value.as_number() {
            self.x = v as f32;
            self.y = v as f32;
            self.z = v as f32;
            self.w = v as f32;
            return;
        }
        if let Some(c) = value.as_color() {
            let v = makepad_math::Vec4f::from_u32(c);
            *self = v;
            return;
        }
        if let Some(pod) = value.as_pod() {
            let (pod_type, pod_words) = vm.heap_mut().pod_data(pod);
            if let Some(v) = pod_type.ty().as_vec() {
                if v.dims() == 4 {
                    match v {
                        ScriptPodVec::Vec4f => {
                            self.x = f32::from_bits(pod_words[0]);
                            self.y = f32::from_bits(pod_words[1]);
                            self.z = f32::from_bits(pod_words[2]);
                            self.w = f32::from_bits(pod_words[3]);
                        }
                        ScriptPodVec::Vec4i => {
                            self.x = pod_words[0] as i32 as f32;
                            self.y = pod_words[1] as i32 as f32;
                            self.z = pod_words[2] as i32 as f32;
                            self.w = pod_words[3] as i32 as f32;
                        }
                        ScriptPodVec::Vec4u => {
                            self.x = pod_words[0] as f32;
                            self.y = pod_words[1] as f32;
                            self.z = pod_words[2] as f32;
                            self.w = pod_words[3] as f32;
                        }
                        ScriptPodVec::Vec4b => {
                            self.x = if pod_words[0] != 0 { 1.0 } else { 0.0 };
                            self.y = if pod_words[1] != 0 { 1.0 } else { 0.0 };
                            self.z = if pod_words[2] != 0 { 1.0 } else { 0.0 };
                            self.w = if pod_words[3] != 0 { 1.0 } else { 0.0 };
                        }
                        ScriptPodVec::Vec4h => {
                            self.x = crate::pod_heap::f16_to_f32(pod_words[0] as u16);
                            self.y = crate::pod_heap::f16_to_f32((pod_words[0] >> 16) as u16);
                            self.z = crate::pod_heap::f16_to_f32(pod_words[1] as u16);
                            self.w = crate::pod_heap::f16_to_f32((pod_words[1] >> 16) as u16);
                        }
                        _ => (),
                    }
                    return;
                }
            }
        }
        // Try apply transform at the bottom
        if let Some(transformed) = vm.call_apply_transform(value) {
            return self.script_apply(vm, apply, scope, transformed);
        }
    },
    fn script_to_value(&self, vm: &mut ScriptVm) -> ScriptValue {
        let pod_type = vm.pod_builtins().pod_vec4f;
        let pod = vm.heap_mut().new_pod(pod_type);
        let pod_words = vm.heap_mut().pod_data_mut(pod);
        pod_words[0] = (self.x).to_bits();
        pod_words[1] = (self.y).to_bits();
        pod_words[2] = (self.z).to_bits();
        pod_words[3] = (self.w).to_bits();
        pod.into()
    }
);

script_primitive!(
    makepad_math::Mat4f,
    fn script_new(_vm: &mut ScriptVm) -> Self {
        Default::default()
    },
    fn script_type_check(heap: &ScriptHeap, value: ScriptValue) -> bool {
        if value.is_number() {
            return true;
        }
        if let Some(pod) = value.as_pod() {
            let (pod_type, _) = heap.pod_data(pod);
            if let Some(m) = pod_type.ty().as_mat() {
                return m.dims() == (4, 4);
            }
        }
        heap.has_apply_transform(value)
    },
    fn script_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
    ) {
        if let Some(v) = value.as_number() {
            for i in 0..16 {
                self.v[i] = v as f32;
            }
            return;
        }
        if let Some(pod) = value.as_pod() {
            let (pod_type, pod_words) = vm.heap_mut().pod_data(pod);
            if let Some(m) = pod_type.ty().as_mat() {
                if m.dims() == (4, 4) {
                    match m {
                        ScriptPodMat::Mat4x4f => {
                            for i in 0..16 {
                                self.v[i] = f32::from_bits(pod_words[i]);
                            }
                        }
                        _ => (),
                    }
                    return;
                }
            }
        }
        // Try apply transform at the bottom
        if let Some(transformed) = vm.call_apply_transform(value) {
            return self.script_apply(vm, apply, scope, transformed);
        }
    },
    fn script_to_value(&self, vm: &mut ScriptVm) -> ScriptValue {
        let pod_type = vm.pod_builtins().pod_mat4x4f;
        let pod = vm.heap_mut().new_pod(pod_type);
        let pod_words = vm.heap_mut().pod_data_mut(pod);
        for i in 0..16 {
            pod_words[i] = self.v[i].to_bits();
        }
        pod.into()
    }
);

// Option

impl<T> ScriptHook for Option<T> where T: ScriptApply + ScriptNew + 'static {}
impl<T> ScriptNew for Option<T>
where
    T: ScriptApply + ScriptNew + 'static,
{
    fn script_type_id_static() -> ScriptTypeId {
        ScriptTypeId::of_path("core::option::Option").with_args(&[T::script_type_id_static()])
    }
    fn script_type_check(heap: &ScriptHeap, value: ScriptValue) -> bool {
        value.is_nil() || T::script_type_check(heap, value)
    }
    fn script_default(_vm: &mut ScriptVm) -> ScriptValue {
        NIL
    }
    fn script_new(_vm: &mut ScriptVm) -> Self {
        Default::default()
    }
    fn script_proto_build(_vm: &mut ScriptVm, _props: &mut ScriptTypeProps) -> ScriptValue {
        NIL
    }
}
impl<T> ScriptApply for Option<T>
where
    T: ScriptApply + ScriptNew + 'static,
{
    fn script_type_id(&self) -> ScriptTypeId {
        <Self as ScriptNew>::script_type_id_static()
    }
    fn script_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
    ) {
        if let Some(v) = self {
            if value.is_nil() {
                *self = None
            } else {
                v.script_apply(vm, apply, scope, value);
            }
        } else {
            if !value.is_nil() {
                let mut inner = T::script_new(vm);
                inner.script_apply(vm, apply, scope, value);
                *self = Some(inner);
            }
        }
    }
    fn script_to_value(&self, vm: &mut ScriptVm) -> ScriptValue {
        if let Some(s) = self {
            s.script_to_value(vm)
        } else {
            NIL
        }
    }
}

// Box: the same component as T, stored on the heap. A #[live] field of a
// large T (a set of draw shaders) boxes it so the struct that holds it, and
// every constructor frame that builds that struct by value, stays small; a
// 1 MB Windows main stack does not fit several nested copies of it.

impl<T> ScriptHook for Box<T> where T: ScriptApply + ScriptNew + 'static {}
impl<T> ScriptNew for Box<T>
where
    T: ScriptApply + ScriptNew + 'static,
{
    fn script_type_id_static() -> ScriptTypeId {
        T::script_type_id_static()
    }
    fn script_type_check(heap: &ScriptHeap, value: ScriptValue) -> bool {
        T::script_type_check(heap, value)
    }
    fn script_default(vm: &mut ScriptVm) -> ScriptValue {
        T::script_default(vm)
    }
    fn script_reload_default(vm: &mut ScriptVm) -> ScriptValue {
        T::script_reload_default(vm)
    }
    fn script_new(vm: &mut ScriptVm) -> Self {
        Box::new(T::script_new(vm))
    }
    fn script_new_with_default(vm: &mut ScriptVm) -> Self {
        Box::new(T::script_new_with_default(vm))
    }
    fn script_proto(vm: &mut ScriptVm) -> ScriptValue {
        T::script_proto(vm)
    }
    fn script_proto_props(vm: &mut ScriptVm, object: ScriptObject, props: &mut ScriptTypeProps) {
        T::script_proto_props(vm, object, props)
    }
}
impl<T> ScriptApply for Box<T>
where
    T: ScriptApply + ScriptNew + 'static,
{
    fn script_type_id(&self) -> ScriptTypeId {
        (**self).script_type_id()
    }
    fn script_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
    ) {
        (**self).script_apply(vm, apply, scope, value)
    }
    fn script_to_value(&self, vm: &mut ScriptVm) -> ScriptValue {
        (**self).script_to_value(vm)
    }
    fn script_source(&self) -> ScriptObject {
        (**self).script_source()
    }
}

#[cfg(test)]
mod compact_primitive_tests {
    use super::*;

    fn round_trip<T>(vm: &mut ScriptVm, value: T)
    where
        T: ScriptNew + ScriptApply + Copy + PartialEq + std::fmt::Debug,
    {
        let script_value = value.script_to_value(vm);
        assert_eq!(T::script_from_value(vm, script_value), value);
    }

    #[test]
    fn compact_primitives_round_trip_through_pod_values() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = ScriptVm {
            host: host.host_context(),
            bx: Box::new(ScriptVmBase::new()),
        };
        round_trip(&mut vm, makepad_math::F16x2::from_f32(0.5, -2.0));
        round_trip(&mut vm, makepad_math::F16x4::from_f32(0.5, -2.0, 3.0, 4.0));
        round_trip(&mut vm, makepad_math::U16x2::from_u16(1, u16::MAX));
        round_trip(&mut vm, makepad_math::I16x2::from_i16(i16::MIN, i16::MAX));
        round_trip(&mut vm, makepad_math::UNorm16x2 { x: 1, y: u16::MAX });
        round_trip(&mut vm, makepad_math::SNorm16x2 { x: i16::MIN, y: i16::MAX });
        round_trip(&mut vm, makepad_math::UNorm8x4([1, 2, 3, 255]));
        round_trip(&mut vm, makepad_math::SNorm8x4([i8::MIN, -1, 0, i8::MAX]));
    }
}
