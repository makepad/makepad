use crate::function::*;
use crate::heap::*;
use crate::makepad_live_id::*;
use crate::native::*;
use crate::object::*;
use crate::value::*;
use crate::*;

use std::cell::RefCell;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::rc::Rc;

#[derive(Debug)]
pub struct ScriptArrayRef {
    pub(crate) roots: Rc<RefCell<HashMap<ScriptArray, usize>>>,
    pub(crate) array: ScriptArray,
}

impl From<ScriptArrayRef> for ScriptValue {
    fn from(v: ScriptArrayRef) -> Self {
        ScriptValue::from_array(v.as_array())
    }
}

impl Clone for ScriptArrayRef {
    fn clone(&self) -> Self {
        let mut roots = self.roots.borrow_mut();
        match roots.entry(self.array) {
            Entry::Occupied(mut occ) => {
                let value = occ.get_mut();
                *value += 1;
            }
            Entry::Vacant(_vac) => {
                eprintln!("ScriptObjectRef root is vacant!");
            }
        }
        Self {
            roots: self.roots.clone(),
            array: self.array.clone(),
        }
    }
}

impl ScriptArrayRef {
    pub fn as_array(&self) -> ScriptArray {
        self.array
    }
}

impl Drop for ScriptArrayRef {
    fn drop(&mut self) {
        let mut roots = self.roots.borrow_mut();
        match roots.entry(self.array) {
            Entry::Occupied(mut occ) => {
                let value = occ.get_mut();
                if *value >= 1 {
                    *value -= 1;
                } else {
                    eprintln!("ScriptObjectRef is 0!");
                }
                if *value == 0 {
                    occ.remove();
                }
            }
            Entry::Vacant(_vac) => {
                eprintln!("ScriptObjectRef root is vacant!");
            }
        }
    }
}

#[derive(Default)]
pub struct ScriptArrayTag(u64);

impl ScriptArrayTag {
    pub const MARK: u64 = 0x1 << 40;
    pub const ALLOCED: u64 = 0x2 << 40;
    pub const STATIC: u64 = 0x4 << 40;
    pub const DIRTY: u64 = 0x40 << 40;
    pub const FROZEN: u64 = 0x100 << 40;

    pub const REF_DATA_MASK: u64 = 0xFF_FFFF_FFFF;
    pub const REF_KIND_MASK: u64 = 0xF << 58;
    pub const REF_KIND_APPLY_TRANSFORM: u64 = 0x6 << 58;

    pub fn is_alloced(&self) -> bool {
        return self.0 & Self::ALLOCED != 0;
    }

    pub fn set_apply_transform(&mut self, ni: NativeId) {
        self.0 &= !(Self::REF_DATA_MASK);
        self.0 &= !(Self::REF_KIND_MASK);
        self.0 |= (ni.index as u64) | Self::REF_KIND_APPLY_TRANSFORM;
    }

    pub fn as_apply_transform(&self) -> Option<NativeId> {
        if self.0 & Self::REF_KIND_MASK == Self::REF_KIND_APPLY_TRANSFORM {
            Some(NativeId {
                index: self.0 as u32,
            })
        } else {
            None
        }
    }

    pub fn is_apply_transform(&self) -> bool {
        self.0 & Self::REF_KIND_MASK == Self::REF_KIND_APPLY_TRANSFORM
    }

    pub fn set_alloced(&mut self) {
        self.0 |= Self::ALLOCED
    }

    pub fn clear(&mut self) {
        self.0 = 0;
    }

    pub fn is_marked(&self) -> bool {
        self.0 & Self::MARK != 0
    }

    pub fn set_mark(&mut self) {
        self.0 |= Self::MARK
    }

    pub fn clear_mark(&mut self) {
        self.0 &= !Self::MARK
    }

    pub fn freeze(&mut self) {
        self.0 |= Self::FROZEN
    }

    pub fn is_frozen(&self) -> bool {
        self.0 & Self::FROZEN != 0
    }

    pub fn set_static(&mut self) {
        self.0 |= Self::STATIC
    }

    pub fn is_static(&self) -> bool {
        self.0 & Self::STATIC != 0
    }

    pub fn set_dirty(&mut self) {
        self.0 |= Self::DIRTY
    }

    pub fn check_and_clear_dirty(&mut self) -> bool {
        if self.0 & Self::DIRTY != 0 {
            self.0 &= !Self::DIRTY;
            true
        } else {
            false
        }
    }
}

#[derive(PartialEq)]
pub enum ScriptArrayStorage {
    ScriptValue(VecDeque<ScriptValue>),
    F32(Vec<f32>),
    U32(Vec<u32>),
    U16(Vec<u16>),
    U8(Vec<u8>),
}

impl ScriptArrayStorage {
    pub fn clear(&mut self) {
        match self {
            Self::ScriptValue(v) => v.clear(),
            Self::F32(v) => v.clear(),
            Self::U32(v) => v.clear(),
            Self::U16(v) => v.clear(),
            Self::U8(v) => v.clear(),
        }
    }
    pub fn len(&self) -> usize {
        match self {
            Self::ScriptValue(v) => v.len(),
            Self::F32(v) => v.len(),
            Self::U32(v) => v.len(),
            Self::U16(v) => v.len(),
            Self::U8(v) => v.len(),
        }
    }
    pub fn index(&self, index: usize) -> Option<ScriptValue> {
        match self {
            Self::ScriptValue(v) => {
                if let Some(v) = v.get(index) {
                    (*v).into()
                } else {
                    None
                }
            }
            Self::F32(v) => {
                if let Some(v) = v.get(index) {
                    Some((*v).into())
                } else {
                    None
                }
            }
            Self::U32(v) => {
                if let Some(v) = v.get(index) {
                    Some((*v).into())
                } else {
                    None
                }
            }
            Self::U16(v) => {
                if let Some(v) = v.get(index) {
                    Some((*v).into())
                } else {
                    None
                }
            }
            Self::U8(v) => {
                if let Some(v) = v.get(index) {
                    Some((*v).into())
                } else {
                    None
                }
            }
        }
    }
    pub fn set_index(&mut self, index: usize, value: ScriptValue) {
        match self {
            Self::ScriptValue(v) => {
                if index >= v.len() {
                    v.resize(index + 1, NIL);
                }
                v[index] = value;
            }
            Self::F32(v) => {
                if index >= v.len() {
                    v.resize(index + 1, 0.0);
                }
                v[index] = value.as_f64().unwrap_or(0.0) as f32;
            }
            Self::U32(v) => {
                if index >= v.len() {
                    v.resize(index + 1, 0);
                }
                v[index] = value.as_f64().unwrap_or(0.0) as u32;
            }
            Self::U16(v) => {
                if index >= v.len() {
                    v.resize(index + 1, 0);
                }
                v[index] = value.as_f64().unwrap_or(0.0) as u16;
            }
            Self::U8(v) => {
                if index >= v.len() {
                    v.resize(index + 1, 0);
                }
                v[index] = value.as_f64().unwrap_or(0.0) as u8;
            }
        }
    }
    pub fn push(&mut self, value: ScriptValue) {
        match self {
            Self::ScriptValue(v) => v.push_back(value),
            Self::F32(v) => v.push(value.as_f64().unwrap_or(0.0) as f32),
            Self::U32(v) => v.push(value.as_f64().unwrap_or(0.0) as u32),
            Self::U16(v) => v.push(value.as_f64().unwrap_or(0.0) as u16),
            Self::U8(v) => v.push(value.as_f64().unwrap_or(0.0) as u8),
        }
    }
    pub fn push_vec(&mut self, vec: &[ScriptVecValue]) {
        match self {
            Self::ScriptValue(v) => {
                for a in vec {
                    v.push_back(a.value)
                }
            }
            Self::F32(v) => {
                for a in vec {
                    v.push(a.value.as_f64().unwrap_or(0.0) as f32)
                }
            }
            Self::U32(v) => {
                for a in vec {
                    v.push(a.value.as_f64().unwrap_or(0.0) as u32)
                }
            }
            Self::U16(v) => {
                for a in vec {
                    v.push(a.value.as_f64().unwrap_or(0.0) as u16)
                }
            }
            Self::U8(v) => {
                for a in vec {
                    v.push(a.value.as_f64().unwrap_or(0.0) as u8)
                }
            }
        }
    }
    pub fn pop(&mut self) -> Option<ScriptValue> {
        match self {
            Self::ScriptValue(v) => {
                if let Some(v) = v.pop_back() {
                    Some(v.into())
                } else {
                    None
                }
            }
            Self::F32(v) => {
                if let Some(v) = v.pop() {
                    Some(v.into())
                } else {
                    None
                }
            }
            Self::U32(v) => {
                if let Some(v) = v.pop() {
                    Some(v.into())
                } else {
                    None
                }
            }
            Self::U16(v) => {
                if let Some(v) = v.pop() {
                    Some(v.into())
                } else {
                    None
                }
            }
            Self::U8(v) => {
                if let Some(v) = v.pop() {
                    Some(v.into())
                } else {
                    None
                }
            }
        }
    }

    pub fn pop_front(&mut self) -> Option<ScriptValue> {
        match self {
            Self::ScriptValue(v) => {
                if let Some(v) = v.pop_front() {
                    Some(v.into())
                } else {
                    None
                }
            }
            Self::F32(v) => {
                if v.len() > 0 {
                    Some(v.remove(0).into())
                } else {
                    None
                }
            }
            Self::U32(v) => {
                if v.len() > 0 {
                    Some(v.remove(0).into())
                } else {
                    None
                }
            }
            Self::U16(v) => {
                if v.len() > 0 {
                    Some(v.remove(0).into())
                } else {
                    None
                }
            }
            Self::U8(v) => {
                if v.len() > 0 {
                    Some(v.remove(0).into())
                } else {
                    None
                }
            }
        }
    }
    pub fn remove(&mut self, index: usize) -> ScriptValue {
        match self {
            Self::ScriptValue(v) => {
                if let Some(value) = v.remove(index) {
                    value
                } else {
                    NIL
                }
            }
            Self::F32(v) => v.remove(index).into(),
            Self::U32(v) => v.remove(index).into(),
            Self::U16(v) => v.remove(index).into(),
            Self::U8(v) => v.remove(index).into(),
        }
    }
    pub fn to_string(&self, heap: &ScriptHeap, s: &mut String) {
        match self {
            Self::U8(bytes) => {
                let v = String::from_utf8_lossy(bytes);
                s.push_str(v.as_ref());
            }
            Self::ScriptValue(vec) => {
                for v in vec {
                    heap.cast_to_string(*v, s);
                }
            }
            Self::F32(v) => {
                for v in v {
                    if let Some(c) = std::char::from_u32(*v as _) {
                        s.push(c)
                    }
                }
            }
            Self::U32(v) => {
                for v in v {
                    if let Some(c) = std::char::from_u32(*v) {
                        s.push(c)
                    }
                }
            }
            Self::U16(v) => {
                for v in v {
                    if let Some(c) = std::char::from_u32(*v as _) {
                        s.push(c)
                    }
                }
            }
        }
    }
}

pub struct ScriptArrayData {
    pub tag: ScriptArrayTag,
    pub storage: ScriptArrayStorage,
}

impl Default for ScriptArrayData {
    fn default() -> Self {
        Self {
            tag: ScriptArrayTag::default(),
            storage: ScriptArrayStorage::ScriptValue(Default::default()),
        }
    }
}

impl ScriptArrayData {
    pub fn add_type_methods(native: &mut ScriptNative, heap: &mut ScriptHeap) {
        native.add_type_method(
            heap,
            ScriptValueType::REDUX_ARRAY,
            id!(to_string),
            &[],
            |vm, args| {
                if let Some(arr) = script_value!(vm, args.self).as_array() {
                    return vm
                        .bx
                        .heap
                        .new_string_with(|heap, s| {
                            heap.array_storage(arr).to_string(heap, s);
                        })
                        .into();
                }
                script_err_unexpected!(vm.bx.threads.cur_ref().trap, "unexpected array type")
            },
        );

        native.add_type_method(heap, ScriptValueType::REDUX_ARRAY, id!(parse_json), &[], |vm, args|{
            if let Some(array) = script_value!(vm, args.self).as_array(){
                // Take json_parser out to avoid borrow conflict
                let mut json_parser = std::mem::take(&mut vm.bx.threads.cur().json_parser);
                let result = vm.bx.heap.array_mut_self_with(array, |heap, storage|{
                    match storage{
                        ScriptArrayStorage::U8(bytes)=>{
                             let v = String::from_utf8_lossy(bytes);
                            json_parser.read_json(v.as_ref(), heap)
                        }
                        _=>{
                            NIL // Error handled below
                        }
                    }
                });
                vm.bx.threads.cur().json_parser = json_parser;
                if result == NIL {
                    // Check if it was due to wrong storage type
                    let storage = vm.bx.heap.array_storage(array);
                    if !matches!(storage, ScriptArrayStorage::U8(_)) {
                        return script_err_type_mismatch!(vm.bx.threads.cur_ref().trap, "parse_json requires U8 byte array, got different array storage type");
                    }
                }
                return result.into()
            }
            script_err_unexpected!(vm.bx.threads.cur_ref().trap, "parse_json called on non-array value")
        });

        native.add_type_method(
            heap,
            ScriptValueType::REDUX_STRING,
            id!(parse_json),
            &[],
            |vm, args| {
                if let Some(arr) = script_value!(vm, args.self).as_array() {
                    // Take json_parser out to avoid borrow conflict
                    let mut json_parser = std::mem::take(&mut vm.bx.threads.cur().json_parser);
                    let result = vm.bx.heap.temp_string_with(|heap, temp| {
                        let storage = heap.array_storage(arr);
                        storage.to_string(heap, temp);
                        json_parser.read_json(temp, heap)
                    });
                    vm.bx.threads.cur().json_parser = json_parser;
                    return result;
                }
                script_err_unexpected!(
                    vm.bx.threads.cur_ref().trap,
                    "to_string called on non-array value"
                )
            },
        );

        native.add_type_method(
            heap,
            ScriptValueType::REDUX_ARRAY,
            id!(push),
            &[],
            |vm, args| {
                if let Some(sself) = script_value!(vm, args.self).as_array() {
                    let trap = vm.bx.threads.cur().trap.pass();
                    vm.bx.heap.array_push_vec(sself, args, trap);
                    return NIL;
                }
                script_err_unexpected!(
                    vm.bx.threads.cur_ref().trap,
                    "push called on non-array value"
                )
            },
        );

        native.add_type_method(
            heap,
            ScriptValueType::REDUX_ARRAY,
            id!(pop),
            &[],
            |vm, args| {
                if let Some(sself) = script_value!(vm, args.self).as_array() {
                    let trap = vm.bx.threads.cur().trap.pass();
                    return vm.bx.heap.array_pop(sself, trap);
                }
                script_err_unexpected!(
                    vm.bx.threads.cur_ref().trap,
                    "pop called on non-array value"
                )
            },
        );

        native.add_type_method(
            heap,
            ScriptValueType::REDUX_ARRAY,
            id!(clear),
            &[],
            |vm, args| {
                if let Some(sself) = script_value!(vm, args.self).as_array() {
                    let trap = vm.bx.threads.cur().trap.pass();
                    vm.bx.heap.array_clear(sself, trap);
                    return NIL;
                }
                script_err_unexpected!(
                    vm.bx.threads.cur_ref().trap,
                    "clear called on non-array value"
                )
            },
        );

        native.add_type_method(
            heap,
            ScriptValueType::REDUX_ARRAY,
            id!(len),
            &[],
            |vm, args| {
                if let Some(sself) = script_value!(vm, args.self).as_array() {
                    return vm.bx.heap.array_len(sself).into();
                }
                script_err_unexpected!(
                    vm.bx.threads.cur_ref().trap,
                    "len called on non-array value"
                )
            },
        );

        native.add_type_method(
            heap,
            ScriptValueType::REDUX_ARRAY,
            id!(freeze),
            &[],
            |vm, args| {
                if let Some(sself) = script_value!(vm, args.self).as_array() {
                    vm.bx.heap.freeze_array(sself);
                    return sself.into();
                }
                script_err_unexpected!(
                    vm.bx.threads.cur_ref().trap,
                    "freeze called on non-array value"
                )
            },
        );

        native.add_type_method(
            heap,
            ScriptValueType::REDUX_ARRAY,
            id!(retain),
            script_args!(cb = NIL),
            |vm, args| {
                if let Some(sself) = script_value!(vm, args.self).as_array() {
                    let fnptr = script_value!(vm, args.cb);
                    let mut i = 0;
                    while i < vm.bx.heap.array_len(sself) {
                        let value = script_array_index!(vm, sself[i]);
                        let ret = vm.call(fnptr, &[value]);
                        if ret.is_err() {
                            return ret;
                        }
                        if !vm.bx.heap.cast_to_bool(ret) {
                            let trap = vm.bx.threads.cur().trap.pass();
                            vm.bx.heap.array_remove(sself, i, trap);
                        } else {
                            i += 1
                        }
                    }
                    return NIL;
                }
                script_err_not_impl!(
                    vm.bx.threads.cur_ref().trap,
                    "retain called on non-array value"
                )
            },
        );
    }

    pub fn clear(&mut self) {
        self.storage.clear();
        self.tag.clear()
    }

    pub fn is_value_array(&self) -> bool {
        if let ScriptArrayStorage::ScriptValue(_) = &self.storage {
            true
        } else {
            false
        }
    }
}
