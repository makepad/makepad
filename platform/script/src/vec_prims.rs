use crate::apply::*;
use crate::array::*;
use crate::heap::*;
use crate::object::*;
use crate::traits::*;
use crate::value::*;
use crate::vm::*;
use crate::*;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::hash::Hash;

impl<T> ScriptHook for Vec<T> where T: ScriptApply + ScriptNew + 'static + ScriptDeriveMarker {}
impl<T> ScriptNew for Vec<T>
where
    T: ScriptApply + ScriptNew + 'static + ScriptDeriveMarker,
{
    fn script_type_id_static() -> ScriptTypeId {
        ScriptTypeId::of::<Self>()
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
            match heap.array_storage(arr) {
                ScriptArrayStorage::ScriptValue(vec) => {
                    for v in vec {
                        if !T::script_type_check(heap, *v) {
                            return false;
                        }
                    }
                    return true;
                }
                ScriptArrayStorage::F32(_) => return true,
                ScriptArrayStorage::U32(_) => return true,
                ScriptArrayStorage::U16(_) => return true,
                ScriptArrayStorage::U8(_) => return true,
            }
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
        ScriptTypeId::of::<Self>()
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
            let len = vm.bx.heap.vec_len(obj);
            self.resize_with(len, || ScriptNew::script_new(vm));
            for i in 0..len {
                if let Some(value) = vm.bx.heap.vec_value_if_exist(obj, i) {
                    self[i].script_apply(vm, apply, scope, value);
                }
            }
        } else if let Some(arr) = value.as_array() {
            let len = vm.bx.heap.array_len(arr);
            self.resize_with(len, || ScriptNew::script_new(vm));
            for i in 0..len {
                let value = vm.bx.heap.array_index_unchecked(arr, i);
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
        let arr = vm.bx.heap.new_array();
        for value in self {
            let value = value.script_to_value(vm);
            vm.bx.heap.array_push_unchecked(arr, value);
        }
        arr.into()
    }
}

impl ScriptHook for Vec<u8> {}
impl ScriptNew for Vec<u8> {
    fn script_type_id_static() -> ScriptTypeId {
        ScriptTypeId::of::<Self>()
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
            match heap.array_storage(arr) {
                ScriptArrayStorage::ScriptValue(vec) => {
                    for v in vec {
                        if !v.is_number() {
                            return false;
                        }
                    }
                    return true;
                }
                ScriptArrayStorage::F32(_) => return true,
                ScriptArrayStorage::U32(_) => return true,
                ScriptArrayStorage::U16(_) => return true,
                ScriptArrayStorage::U8(_) => return true,
            }
        }
        value.is_string_like() || value.is_nil() || heap.has_apply_transform(value)
    }
    fn script_default(vm: &mut ScriptVm) -> ScriptValue {
        vm.bx.heap.new_object().into()
    }
    fn script_new(_vm: &mut ScriptVm) -> Self {
        Default::default()
    }
    fn script_proto_build(vm: &mut ScriptVm, _props: &mut ScriptTypeProps) -> ScriptValue {
        vm.bx.heap.new_object().into()
    }
}

impl ScriptApply for Vec<u8> {
    fn script_type_id(&self) -> ScriptTypeId {
        ScriptTypeId::of::<Self>()
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
            for kv in vm.bx.heap.vec_ref(obj) {
                self.push(kv.value.as_f64().unwrap_or(0.0) as _);
            }
        } else if let Some(arr) = value.as_array() {
            self.clear();
            match vm.bx.heap.array_storage(arr) {
                ScriptArrayStorage::ScriptValue(vec) => {
                    for v in vec {
                        self.push((*v).into())
                    }
                }
                ScriptArrayStorage::F32(vec) => {
                    for v in vec {
                        self.push(*v as _)
                    }
                }
                ScriptArrayStorage::U32(vec) => {
                    for v in vec {
                        self.push(*v as _)
                    }
                }
                ScriptArrayStorage::U16(vec) => {
                    for v in vec {
                        self.push(*v as _)
                    }
                }
                ScriptArrayStorage::U8(vec) => {
                    for v in vec {
                        self.push(*v as _)
                    }
                }
            }
        } else if let Some(str) = value.as_string() {
            let str = vm.bx.heap.string(str);
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
            script_err_type_mismatch!(vm.bx.threads.cur_ref().trap, "wrong vec type in apply");
        }
    }
    fn script_to_value(&self, vm: &mut ScriptVm) -> ScriptValue {
        vm.bx.heap.new_array_from_slice_u8(self).into()
    }
}

impl ScriptHook for Vec<ScriptValue> {}
impl ScriptNew for Vec<ScriptValue> {
    fn script_type_id_static() -> ScriptTypeId {
        ScriptTypeId::of::<Self>()
    }
    fn script_type_check(_heap: &ScriptHeap, _value: ScriptValue) -> bool {
        true
    }
    fn script_default(vm: &mut ScriptVm) -> ScriptValue {
        vm.bx.heap.new_object().into()
    }
    fn script_new(_vm: &mut ScriptVm) -> Self {
        Default::default()
    }
    fn script_proto_build(vm: &mut ScriptVm, _props: &mut ScriptTypeProps) -> ScriptValue {
        vm.bx.heap.new_object().into()
    }
}

impl ScriptApply for Vec<ScriptValue> {
    fn script_type_id(&self) -> ScriptTypeId {
        ScriptTypeId::of::<Self>()
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
            for kv in vm.bx.heap.vec_ref(obj) {
                self.push(kv.value);
            }
        } else if let Some(arr) = value.as_array() {
            self.clear();
            match vm.bx.heap.array_storage(arr) {
                ScriptArrayStorage::ScriptValue(vec) => {
                    for v in vec {
                        self.push((*v).into())
                    }
                }
                ScriptArrayStorage::F32(vec) => {
                    for v in vec {
                        self.push((*v).into())
                    }
                }
                ScriptArrayStorage::U32(vec) => {
                    for v in vec {
                        self.push((*v).into())
                    }
                }
                ScriptArrayStorage::U16(vec) => {
                    for v in vec {
                        self.push((*v).into())
                    }
                }
                ScriptArrayStorage::U8(vec) => {
                    for v in vec {
                        self.push((*v).into())
                    }
                }
            }
        } else if value.is_nil() {
            self.clear();
        } else {
            self.clear();
            self.push(value);
        }
    }
    fn script_to_value(&self, vm: &mut ScriptVm) -> ScriptValue {
        let arr = vm.bx.heap.new_array();
        for value in self {
            vm.bx.heap.array_push_unchecked(arr, *value);
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
        ScriptTypeId::of::<Self>()
    }
    fn script_type_check(heap: &ScriptHeap, value: ScriptValue) -> bool {
        if let Some(obj) = value.as_object() {
            let map = heap.map_ref(obj);
            for (key, value) in map.iter() {
                if !K::script_type_check(heap, *key) {
                    return false;
                }
                if !V::script_type_check(heap, value.value) {
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
        ScriptTypeId::of::<Self>()
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
            vm.map_mut_with(obj, |vm, obj_map| {
                for (key, value) in obj_map.iter_mut() {
                    let key = K::script_from_value(vm, *key);
                    let value = V::script_from_value(vm, value.value);
                    self.insert(key, value);
                }
            })
        } else if value.is_nil() {
            self.clear()
        } else {
            script_err_type_mismatch!(vm.bx.threads.cur_ref().trap, "wrong vec type in apply");
        }
    }
    fn script_to_value(&self, vm: &mut ScriptVm) -> ScriptValue {
        let obj = vm.bx.heap.new_object();
        let mut has_string_keys = false;
        vm.map_mut_with(obj, |vm, obj_map| {
            for (key, value) in self.iter() {
                let key = key.script_to_value(vm);
                if key.is_string_like() {
                    has_string_keys = true;
                }
                let value = value.script_to_value(vm);
                if !vm.bx.heap.charge_object_map_entry(
                    obj,
                    key,
                    "converting a Rust map to a script object",
                ) {
                    continue;
                }
                obj_map.insert(
                    key,
                    ScriptMapValue {
                        tag: Default::default(),
                        value,
                    },
                );
            }
        });
        if has_string_keys {
            vm.bx.heap.set_string_keys(obj);
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
        ScriptTypeId::of::<Self>()
    }
    fn script_type_check(heap: &ScriptHeap, value: ScriptValue) -> bool {
        if let Some(obj) = value.as_object() {
            let map = heap.map_ref(obj);
            for (key, value) in map.iter() {
                if !K::script_type_check(heap, *key) {
                    return false;
                }
                if !V::script_type_check(heap, value.value) {
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
        ScriptTypeId::of::<Self>()
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
            vm.map_mut_with(obj, |vm, obj_map| {
                for (key, value) in obj_map.iter_mut() {
                    let key = K::script_from_value(vm, *key);
                    let value = V::script_from_value(vm, value.value);
                    self.insert(key, value);
                }
            })
        } else if value.is_nil() {
            self.clear()
        } else {
            script_err_type_mismatch!(vm.bx.threads.cur_ref().trap, "wrong vec type in apply");
        }
    }
    fn script_to_value(&self, vm: &mut ScriptVm) -> ScriptValue {
        let obj = vm.bx.heap.new_object();
        let mut has_string_keys = false;
        vm.map_mut_with(obj, |vm, obj_map| {
            for (key, value) in self.iter() {
                let key = key.script_to_value(vm);
                if key.is_string_like() {
                    has_string_keys = true;
                }
                let value = value.script_to_value(vm);
                if !vm.bx.heap.charge_object_map_entry(
                    obj,
                    key,
                    "converting a Rust map to a script object",
                ) {
                    continue;
                }
                obj_map.insert(
                    key,
                    ScriptMapValue {
                        tag: Default::default(),
                        value,
                    },
                );
            }
        });
        if has_string_keys {
            vm.bx.heap.set_string_keys(obj);
        }
        obj.into()
    }
}
