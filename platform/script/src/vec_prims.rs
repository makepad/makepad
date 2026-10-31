use crate::apply::*;
use crate::heap::*;
use crate::traits::*;
use crate::value::*;
use crate::vm::*;
use crate::*;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;

impl<T> ScriptHook for Vec<T> where T: ScriptApply + ScriptNew + 'static + ScriptDeriveMarker {}
impl<T> ScriptNew for Vec<T>
where
    T: ScriptApply + ScriptNew + 'static + ScriptDeriveMarker,
{
    fn script_type_id_static() -> ScriptTypeId {
        ScriptTypeId::of_path("alloc::vec::Vec").with_args(&[T::script_type_id_static()])
    }
    fn script_type_check(heap: &ScriptHeap, value: ScriptValue) -> bool {
        if let Some(obj) = value.as_object() {
            let len = heap.vec_len(obj);
            for i in 0..len {
                if let Some(value) = heap.vec_value_if_exist(obj, i) {
                    if !T::script_type_check(heap, value) {
                        return false;
                    }
                }
            }
            return true;
        } else if let Some(arr) = value.as_array() {
            // typed storage (f32/u32/u16/u8) always checks
            if let Some((front, back)) = heap.array_values(arr) {
                for v in front.iter().chain(back) {
                    if !T::script_type_check(heap, *v) {
                        return false;
                    }
                }
            }
            return true;
        }
        value.is_nil() || T::script_type_check(heap, value) || heap.has_apply_transform(value)
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
impl<T> ScriptApply for Vec<T>
where
    T: ScriptApply + ScriptNew + 'static + ScriptDeriveMarker,
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
            let len = vm.heap().vec_len(obj);
            self.resize_with(len, || ScriptNew::script_new(vm));
            for i in 0..len {
                if let Some(value) = vm.heap().vec_value_if_exist(obj, i) {
                    self[i].script_apply(vm, apply, scope, value);
                }
            }
        } else if let Some(arr) = value.as_array() {
            let len = vm.heap().array_len(arr);
            self.resize_with(len, || ScriptNew::script_new(vm));
            for i in 0..len {
                let value = vm.heap().array_index_unchecked(arr, i);
                self[i].script_apply(vm, apply, scope, value);
            }
        } else if value.is_nil() {
            self.clear()
        } else {
            self.clear();
            self.push(ScriptNew::script_from_value(vm, value));
        }
    }
    fn script_to_value(&self, vm: &mut ScriptVm) -> ScriptValue {
        let arr = vm.heap_mut().new_array();
        for value in self {
            let value = value.script_to_value(vm);
            vm.heap_mut().array_push_unchecked(arr, value);
        }
        arr.into()
    }
}

impl ScriptHook for Vec<u8> {}
impl ScriptNew for Vec<u8> {
    fn script_type_id_static() -> ScriptTypeId {
        ScriptTypeId::of_path("alloc::vec::Vec<u8>")
    }
    fn script_type_check(heap: &ScriptHeap, value: ScriptValue) -> bool {
        if let Some(obj) = value.as_object() {
            for kv in heap.vec_ref(obj) {
                if !kv.value.is_number() {
                    return false;
                }
            }
            return true;
        } else if let Some(arr) = value.as_array() {
            // typed storage (f32/u32/u16/u8) always checks
            if let Some((front, back)) = heap.array_values(arr) {
                for v in front.iter().chain(back) {
                    if !v.is_number() {
                        return false;
                    }
                }
            }
            return true;
        }
        value.is_string_like() || value.is_nil() || heap.has_apply_transform(value)
    }
    fn script_default(vm: &mut ScriptVm) -> ScriptValue {
        vm.heap_mut().new_object().into()
    }
    fn script_new(_vm: &mut ScriptVm) -> Self {
        Default::default()
    }
    fn script_proto_build(vm: &mut ScriptVm, _props: &mut ScriptTypeProps) -> ScriptValue {
        vm.heap_mut().new_object().into()
    }
}

impl ScriptApply for Vec<u8> {
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
            for kv in vm.heap().vec_ref(obj) {
                self.push(kv.value.as_f64().unwrap_or(0.0) as _);
            }
        } else if let Some(arr) = value.as_array() {
            self.clear();
            self.resize(vm.array_len(arr), 0);
            vm.heap().copy_array_bytes(arr, self);
        } else if let Some(str) = value.as_string() {
            let str = vm.heap().string(str);
            self.clear();
            self.extend(str.as_bytes());
        } else if value
            .as_inline_string(|s| {
                self.clear();
                self.extend(s.as_bytes());
            })
            .is_some()
        {
        } else if value.is_nil() {
            self.clear();
        } else {
            script_err_type_mismatch!(vm.trap(), "wrong vec type in apply");
        }
    }
    fn script_to_value(&self, vm: &mut ScriptVm) -> ScriptValue {
        vm.heap_mut().new_array_from_slice_u8(self).into()
    }
}

impl ScriptDeriveMarker for Arc<[u8]> {}
impl ScriptHook for Arc<[u8]> {}
impl ScriptNew for Arc<[u8]> {
    fn script_type_id_static() -> ScriptTypeId {
        ScriptTypeId::of_path("alloc::sync::Arc<[u8]>")
    }
    fn script_type_check(heap: &ScriptHeap, value: ScriptValue) -> bool {
        <Vec<u8> as ScriptNew>::script_type_check(heap, value)
    }
    fn script_default(vm: &mut ScriptVm) -> ScriptValue {
        <Vec<u8> as ScriptNew>::script_default(vm)
    }
    fn script_new(_vm: &mut ScriptVm) -> Self {
        Arc::from([])
    }
    fn script_proto_build(vm: &mut ScriptVm, props: &mut ScriptTypeProps) -> ScriptValue {
        <Vec<u8> as ScriptNew>::script_proto_build(vm, props)
    }
}

impl ScriptApply for Arc<[u8]> {
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
        let mut bytes = self.to_vec();
        bytes.script_apply(vm, apply, scope, value);
        *self = Arc::from(bytes);
    }
    fn script_to_value(&self, vm: &mut ScriptVm) -> ScriptValue {
        vm.heap_mut().new_array_from_slice_u8(self).into()
    }
}

impl ScriptHook for Vec<ScriptValue> {}
impl ScriptNew for Vec<ScriptValue> {
    fn script_type_id_static() -> ScriptTypeId {
        ScriptTypeId::of_path("alloc::vec::Vec<makepad_script::ScriptValue>")
    }
    fn script_type_check(_heap: &ScriptHeap, _value: ScriptValue) -> bool {
        true
    }
    fn script_default(vm: &mut ScriptVm) -> ScriptValue {
        vm.heap_mut().new_object().into()
    }
    fn script_new(_vm: &mut ScriptVm) -> Self {
        Default::default()
    }
    fn script_proto_build(vm: &mut ScriptVm, _props: &mut ScriptTypeProps) -> ScriptValue {
        vm.heap_mut().new_object().into()
    }
}

impl ScriptApply for Vec<ScriptValue> {
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
            for kv in vm.heap().vec_ref(obj) {
                self.push(kv.value);
            }
        } else if let Some(arr) = value.as_array() {
            self.clear();
            let heap = vm.heap();
            if let Some((front, back)) = heap.array_values(arr) {
                self.extend(front.iter().chain(back).copied());
            } else if let Some(vec) = heap.array_f32(arr) {
                self.extend(vec.iter().map(|v| ScriptValue::from(*v)));
            } else if let Some(vec) = heap.array_u32(arr) {
                self.extend(vec.iter().map(|v| ScriptValue::from(*v)));
            } else if let Some(vec) = heap.array_u16(arr) {
                self.extend(vec.iter().map(|v| ScriptValue::from(*v)));
            } else if let Some(vec) = heap.array_u8(arr) {
                self.extend(vec.iter().map(|v| ScriptValue::from(*v)));
            }
        } else if value.is_nil() {
            self.clear();
        } else {
            self.clear();
            self.push(value);
        }
    }
    fn script_to_value(&self, vm: &mut ScriptVm) -> ScriptValue {
        let arr = vm.heap_mut().new_array();
        for value in self {
            vm.heap_mut().array_push_unchecked(arr, *value);
        }
        arr.into()
    }
}

impl<K, V> ScriptHook for HashMap<K, V>
where
    K: ScriptApply + ScriptNew + 'static + Eq + Hash,
    V: ScriptApply + ScriptNew + 'static,
{
}
impl<K, V> ScriptNew for HashMap<K, V>
where
    K: ScriptApply + ScriptNew + 'static + Eq + Hash,
    V: ScriptApply + ScriptNew + 'static,
{
    fn script_type_id_static() -> ScriptTypeId {
        ScriptTypeId::of_path("std::collections::HashMap").with_args(&[K::script_type_id_static(), V::script_type_id_static()])
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
impl<K, V> ScriptApply for HashMap<K, V>
where
    K: ScriptApply + ScriptNew + 'static + Eq + Hash,
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
            // hashmaps and btreemaps are cleared and copied fresh we can optimise later
            self.clear();
            for e in vm.heap().map_entries(obj) {
                let key = K::script_from_value(vm, e.key);
                let value = V::script_from_value(vm, e.value);
                self.insert(key, value);
            }
        } else if value.is_nil() {
            self.clear()
        } else {
            script_err_type_mismatch!(vm.trap(), "wrong vec type in apply");
        }
    }
    fn script_to_value(&self, vm: &mut ScriptVm) -> ScriptValue {
        let obj = vm.heap_mut().new_object();
        let mut has_string_keys = false;
        for (key, value) in self.iter() {
            let key = key.script_to_value(vm);
            if key.is_string_like() {
                has_string_keys = true;
            }
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
        if has_string_keys {
            vm.heap_mut().set_string_keys(obj);
        }
        obj.into()
    }
}

impl<K, V> ScriptHook for BTreeMap<K, V>
where
    K: ScriptApply + ScriptNew + 'static + Ord,
    V: ScriptApply + ScriptNew + 'static,
{
}
impl<K, V> ScriptNew for BTreeMap<K, V>
where
    K: ScriptApply + ScriptNew + 'static + Ord,
    V: ScriptApply + ScriptNew + 'static,
{
    fn script_type_id_static() -> ScriptTypeId {
        ScriptTypeId::of_path("alloc::collections::BTreeMap").with_args(&[K::script_type_id_static(), V::script_type_id_static()])
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
impl<K, V> ScriptApply for BTreeMap<K, V>
where
    K: ScriptApply + ScriptNew + 'static + Ord,
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
            // hashmaps and btreemaps are cleared and copied fresh we can optimise later
            self.clear();
            for e in vm.heap().map_entries(obj) {
                let key = K::script_from_value(vm, e.key);
                let value = V::script_from_value(vm, e.value);
                self.insert(key, value);
            }
        } else if value.is_nil() {
            self.clear()
        } else {
            script_err_type_mismatch!(vm.trap(), "wrong vec type in apply");
        }
    }
    fn script_to_value(&self, vm: &mut ScriptVm) -> ScriptValue {
        let obj = vm.heap_mut().new_object();
        let mut has_string_keys = false;
        for (key, value) in self.iter() {
            let key = key.script_to_value(vm);
            if key.is_string_like() {
                has_string_keys = true;
            }
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
        if has_string_keys {
            vm.heap_mut().set_string_keys(obj);
        }
        obj.into()
    }
}
