use crate::function::*;
use crate::heap::*;
use crate::makepad_live_id::*;
use crate::mod_shader::ShaderIoType;
use crate::native::*;
use crate::traits::*;
use crate::value::*;
use crate::value_map::*;
use crate::*;
use ::std::cell::RefCell;
use ::std::collections::hash_map::Entry;
use ::std::collections::HashMap;
use ::std::fmt;
use ::std::rc::Rc;
//use std::collections::btree_map::BTreeMap;

#[derive(Default)]
pub struct ScriptObjectTag(u64);

pub type ScriptObjectMap = ValueMap<ScriptValue, ScriptMapValue>;

pub struct ScriptObjectRef {
    pub(crate) roots: Option<Rc<RefCell<HashMap<ScriptObject, usize>>>>,
    pub(crate) obj: ScriptObject,
}

impl ScriptObjectRef {
    pub fn is_zero(&self) -> bool {
        self.obj == ScriptObject::ZERO
    }
}

impl Default for ScriptObjectRef {
    fn default() -> Self {
        Self {
            roots: None,
            obj: ScriptObject::ZERO,
        }
    }
}

impl Clone for ScriptObjectRef {
    fn clone(&self) -> Self {
        if let Some(roots) = &self.roots {
            let mut roots = roots.borrow_mut();
            match roots.entry(self.obj) {
                Entry::Occupied(mut occ) => {
                    let value = occ.get_mut();
                    *value += 1;
                }
                Entry::Vacant(_vac) => {
                    eprintln!("ScriptObjectRef root is vacant!");
                }
            }
        }
        Self {
            roots: self.roots.clone(),
            obj: self.obj.clone(),
        }
    }
}

impl From<ScriptObjectRef> for ScriptValue {
    fn from(v: ScriptObjectRef) -> Self {
        ScriptValue::from_object(v.as_object())
    }
}

impl ScriptObjectRef {
    pub fn as_object(&self) -> ScriptObject {
        self.obj
    }
}

pub trait ScriptRefOptionExt {
    fn as_object(&self) -> Option<ScriptObject>;
}
impl ScriptRefOptionExt for Option<ScriptObjectRef> {
    fn as_object(&self) -> Option<ScriptObject> {
        if let Some(x) = self {
            Some(x.as_object())
        } else {
            None
        }
    }
}

impl Drop for ScriptObjectRef {
    fn drop(&mut self) {
        if let Some(roots) = &self.roots {
            let mut roots = roots.borrow_mut();
            match roots.entry(self.obj) {
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
}

impl ScriptObjectTag {
    // marked in the mark-sweep gc
    pub const MARK: u64 = 0x1 << 40;
    // object is not 'free'
    pub const ALLOCED: u64 = 0x2 << 40;
    // object is 'deep' aka writes to protochain
    pub const DEEP: u64 = 0x4 << 40;
    // make the map tracked
    pub const TRACKED: u64 = 0x8 << 40;
    // used to quick-free objects if not set
    pub const REFFED: u64 = 0x10 << 40;
    // object is skipped in gc passes
    pub const STATIC: u64 = 0x20 << 40;
    // object dirty
    pub const DIRTY: u64 = 0x40 << 40;

    // set when the object has been first applied
    pub const FIRST_APPLIED: u64 = 0x80 << 40;
    // marks object readonly
    pub const FROZEN: u64 = 0x100 << 40;
    // checks base types against prototype
    pub const VALIDATED: u64 = 0x200 << 40;
    // for read only allow writes only if map item doesnt exist
    pub const MAP_ADD: u64 = 0x400 << 40;
    // vec is frozen
    pub const VEC_FROZEN: u64 = 0x800 << 40;
    // type checked
    pub const TYPE_CHECKED: u64 = 0x1000 << 40;
    // cant be a prototype
    pub const NOTPROTO: u64 = 0x2000 << 40;
    // automatically convert between id and string keys when looking up
    pub const STRING_KEYS: u64 = 0x4000 << 40;

    pub const FROM_EVAL: u64 = 0x8000 << 40;

    pub const FREEZE_MASK: u64 = Self::FROZEN | Self::VALIDATED | Self::MAP_ADD | Self::VEC_FROZEN;

    const PROTO_FWD: u64 = Self::ALLOCED
        | Self::DEEP
        | Self::STORAGE_MASK
        | Self::VALIDATED
        | Self::MAP_ADD
        | Self::VEC_FROZEN
        | Self::TRACKED
        | Self::REF_KIND_MASK
        | Self::REF_DATA_MASK
        | Self::TYPE_CHECKED;

    pub const NEED_CHECK_MASK: u64 = Self::FREEZE_MASK | Self::TYPE_CHECKED;

    pub const FLAG_MASK: u64 = 0x3FFFF << 40;

    pub const REF_KIND_SCRIPT_FN: u64 = 0x1 << 58;
    pub const REF_KIND_NATIVE_FN: u64 = 0x2 << 58;
    pub const REF_KIND_TYPE_INDEX: u64 = 0x3 << 58;
    pub const REF_KIND_POD_TYPE: u64 = 0x4 << 58;
    pub const REF_KIND_SHADER_IO: u64 = 0x5 << 58;
    pub const REF_KIND_APPLY_TRANSFORM: u64 = 0x6 << 58;
    pub const REF_KIND_MASK: u64 = 0xF << 58;
    pub const REF_DATA_MASK: u64 = 0xFF_FFFF_FFFF;

    pub const STORAGE_SHIFT: u64 = 62;
    pub const STORAGE_MASK: u64 = 0x3 << Self::STORAGE_SHIFT;

    pub const STORAGE_AUTO: u64 = 0 << Self::STORAGE_SHIFT;
    pub const STORAGE_VEC2: u64 = 1 << Self::STORAGE_SHIFT;
    pub const STORAGE_MAP: u64 = 2 << Self::STORAGE_SHIFT;

    pub fn proto_fwd(&self) -> u64 {
        self.0 & Self::PROTO_FWD
    }

    pub fn set_proto_fwd(&mut self, fwd: u64) {
        self.0 = (self.0 & !Self::PROTO_FWD) | (fwd & Self::PROTO_FWD)
    }

    // STORAGE

    pub fn is_auto(&self) -> bool {
        self.0 & Self::STORAGE_MASK == Self::STORAGE_AUTO
    }

    pub fn is_vec2(&self) -> bool {
        self.0 & Self::STORAGE_MASK == Self::STORAGE_VEC2
    }

    pub fn is_map(&self) -> bool {
        self.0 & Self::STORAGE_MASK == Self::STORAGE_MAP
    }

    pub fn set_vec2(&mut self) {
        self.0 &= !Self::STORAGE_MASK;
        self.0 |= Self::STORAGE_VEC2;
    }

    pub fn set_auto(&mut self) {
        self.0 &= !Self::STORAGE_AUTO;
        self.0 |= Self::STORAGE_MAP;
    }

    pub fn set_map(&mut self) {
        self.0 &= !Self::STORAGE_MASK;
        self.0 |= Self::STORAGE_MAP;
    }

    // FLAGS

    pub fn set_first_applied_and_clean(&mut self) {
        self.0 &= !Self::DIRTY;
        self.0 |= Self::FIRST_APPLIED;
    }

    pub fn is_first_applied(&self) -> bool {
        self.0 & Self::FIRST_APPLIED != 0
    }

    pub fn set_tracked(&mut self) {
        self.0 |= Self::TRACKED
    }

    pub fn is_tracked(&self) -> bool {
        self.0 & Self::TRACKED != 0
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

    pub fn set_string_keys(&mut self) {
        self.0 |= Self::STRING_KEYS
    }

    pub fn is_string_keys(&self) -> bool {
        self.0 & Self::STRING_KEYS != 0
    }

    pub fn set_from_eval(&mut self) {
        self.0 |= Self::FROM_EVAL
    }

    pub fn is_from_eval(&self) -> bool {
        self.0 & Self::FROM_EVAL != 0
    }

    pub fn set_static(&mut self) {
        self.0 |= Self::STATIC
    }

    pub fn is_static(&self) -> bool {
        self.0 & Self::STATIC != 0
    }

    pub fn is_notproto(&self) -> bool {
        self.0 & Self::NOTPROTO != 0
    }

    pub fn set_notproto(&mut self) {
        self.0 |= Self::NOTPROTO
    }

    pub fn is_frozen(&self) -> bool {
        self.0 & Self::FROZEN != 0
    }

    /// Check if object is immutable (either frozen or static)
    /// Static objects are GC-permanent and should not be modified
    #[inline(always)]
    pub fn is_immutable(&self) -> bool {
        self.0 & (Self::FROZEN | Self::STATIC) != 0
    }

    pub fn is_validated(&self) -> bool {
        self.0 & Self::VALIDATED != 0
    }

    pub fn is_map_add(&self) -> bool {
        self.0 & Self::MAP_ADD != 0
    }

    pub fn set_reffed(&mut self) {
        self.0 |= Self::REFFED
    }

    pub fn is_reffed(&self) -> bool {
        self.0 & Self::REFFED != 0
    }

    pub fn set_deep(&mut self) {
        self.0 |= Self::DEEP
    }

    pub fn clear_deep(&mut self) {
        self.0 &= !Self::DEEP
    }

    pub fn is_deep(&self) -> bool {
        self.0 & Self::DEEP != 0
    }

    pub fn is_alloced(&self) -> bool {
        return self.0 & Self::ALLOCED != 0;
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

    // FREEZE

    pub fn freeze(&mut self) {
        self.0 &= !(Self::FREEZE_MASK);
        self.0 |= Self::FROZEN
    }

    pub fn freeze_type(&mut self) {
        self.0 &= !(Self::FREEZE_MASK);
        self.0 |= Self::FROZEN | Self::VEC_FROZEN
    }

    pub fn freeze_api(&mut self) {
        self.0 &= !(Self::FREEZE_MASK);
        self.0 |= Self::FROZEN | Self::VALIDATED | Self::VEC_FROZEN
    }

    pub fn freeze_module(&mut self) {
        self.0 &= !(Self::FREEZE_MASK);
        self.0 |= Self::MAP_ADD | Self::VEC_FROZEN | Self::NOTPROTO
    }

    pub fn freeze_component(&mut self) {
        self.0 &= !(Self::FREEZE_MASK);
        self.0 |= Self::FROZEN | Self::VALIDATED
    }

    pub fn freeze_shader(&mut self) {
        self.0 &= !(Self::FREEZE_MASK);
        self.0 |= Self::FROZEN | Self::VALIDATED | Self::MAP_ADD | Self::VEC_FROZEN
    }

    pub fn freeze_ext(&mut self) {
        self.0 &= !(Self::FREEZE_MASK);
        self.0 |= Self::FROZEN | Self::VALIDATED | Self::MAP_ADD
    }

    pub fn needs_checking(&self) -> bool {
        self.0 & (Self::NEED_CHECK_MASK) != 0
    }

    pub fn is_vec_frozen(&self) -> bool {
        self.0 & (Self::VEC_FROZEN | Self::FROZEN | Self::STATIC) != 0
    }

    // REF

    pub fn set_type_index(&mut self, ty: ScriptTypeIndex) {
        self.0 &= !(Self::REF_DATA_MASK);
        self.0 &= !(Self::REF_KIND_MASK);
        self.0 |= ty.0 as u64 | Self::REF_KIND_TYPE_INDEX | Self::TYPE_CHECKED;
    }

    pub fn as_type_index(&self) -> Option<ScriptTypeIndex> {
        if self.is_type_index() {
            Some(ScriptTypeIndex(self.0 as u32))
        } else {
            None
        }
    }

    pub fn is_type_index(&self) -> bool {
        self.0 & Self::REF_KIND_MASK == Self::REF_KIND_TYPE_INDEX
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

    pub fn set_fn(&mut self, ptr: ScriptFnPtr) {
        self.0 &= !(Self::REF_DATA_MASK);
        self.0 &= !(Self::REF_KIND_MASK);
        match ptr {
            ScriptFnPtr::Script(ip) => self.0 |= ip.to_u40() | Self::REF_KIND_SCRIPT_FN,
            ScriptFnPtr::Native(ni) => self.0 |= (ni.index as u64) | Self::REF_KIND_NATIVE_FN,
        }
    }

    pub fn as_fn(&self) -> Option<ScriptFnPtr> {
        if self.0 & Self::REF_KIND_MASK == Self::REF_KIND_SCRIPT_FN {
            Some(ScriptFnPtr::Script(ScriptIp::from_u40(self.0)))
        } else if self.0 & Self::REF_KIND_MASK == Self::REF_KIND_NATIVE_FN {
            Some(ScriptFnPtr::Native(NativeId {
                index: self.0 as u32,
            }))
        } else {
            None
        }
    }

    pub fn is_script_fn(&self) -> bool {
        self.0 & Self::REF_KIND_MASK == Self::REF_KIND_SCRIPT_FN
    }

    pub fn is_native_fn(&self) -> bool {
        self.0 & Self::REF_KIND_MASK == Self::REF_KIND_NATIVE_FN
    }

    pub fn is_fn(&self) -> bool {
        self.is_script_fn() || self.is_native_fn()
    }

    pub fn set_pod_type(&mut self, ty: ScriptPodType) {
        self.0 &= !(Self::REF_DATA_MASK);
        self.0 &= !(Self::REF_KIND_MASK);
        self.0 |= ty.index as u64 | Self::REF_KIND_POD_TYPE
    }

    pub fn as_pod_type(&self) -> Option<ScriptPodType> {
        if self.is_pod_type() {
            Some(ScriptPodType {
                index: self.0 as u32,
            })
        } else {
            None
        }
    }

    pub fn is_pod_type(&self) -> bool {
        self.0 & Self::REF_KIND_MASK == Self::REF_KIND_POD_TYPE
    }

    pub fn set_shader_io(&mut self, ty: ShaderIoType) {
        self.0 &= !(Self::REF_DATA_MASK);
        self.0 &= !(Self::REF_KIND_MASK);
        self.0 |= ty.0 as u64 | Self::REF_KIND_SHADER_IO
    }

    pub fn as_shader_io(&self) -> Option<ShaderIoType> {
        if self.is_shader_io() {
            Some(ShaderIoType(self.0 as u32))
        } else {
            None
        }
    }

    pub fn is_shader_io(&self) -> bool {
        self.0 & Self::REF_KIND_MASK == Self::REF_KIND_SHADER_IO
    }
}

impl fmt::Debug for ScriptObjectTag {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for ScriptObjectTag {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "ObjectType(").ok();
        if self.is_vec2() {
            write!(f, "STORAGE_VEC2|").ok();
        }
        if self.is_auto() {
            write!(f, "STORAGE_AUTO|").ok();
        }
        if self.is_map() {
            write!(f, "STORAGE_MAP|").ok();
        }
        if self.is_marked() {
            write!(f, "MARK|").ok();
        }
        if self.is_alloced() {
            write!(f, "ALLOCED|").ok();
        }
        if self.is_deep() {
            write!(f, "DEEP|").ok();
        }
        if self.is_script_fn() {
            write!(f, "SCRIPT_FN({:?})|", self.as_fn().unwrap()).ok();
        }
        if self.is_native_fn() {
            write!(f, "NATIVE_FN({:?})|", self.as_fn().unwrap()).ok();
        }
        if self.is_reffed() {
            write!(f, "REFFED|").ok();
        }
        if self.is_frozen() {
            write!(f, "FROZEN|").ok();
        }
        if self.is_vec_frozen() {
            write!(f, "VEC_FROZEN|").ok();
        }
        if self.is_validated() {
            write!(f, "VALIDATED|").ok();
        }
        if self.is_map_add() {
            write!(f, "MAP_ADD|").ok();
        }
        if self.is_script_fn() {
            write!(f, "SCRIPT_FN|").ok();
        }
        if self.is_native_fn() {
            write!(f, "NATIVE_FN|").ok();
        }

        write!(f, ")")
    }
}

#[derive(Default, Debug, PartialEq, Eq, Clone, Copy, Hash, Ord, PartialOrd)]
pub struct ScriptMapTag(u64);

impl ScriptMapTag {
    // Lower 32 bits store insertion order
    const ORDER_MASK: u64 = 0xFFFF_FFFF;
    // Bit 33 (index 32) stores the dirty flag
    const DIRTY: u64 = 1 << 32;

    #[allow(dead_code)]
    fn dirty() -> Self {
        Self(Self::DIRTY)
    }

    fn dirty_with_order(order: u32) -> Self {
        Self(Self::DIRTY | (order as u64))
    }

    fn get_and_clear_dirty(&mut self) -> bool {
        let ret = self.0 & Self::DIRTY != 0;
        self.0 &= !Self::DIRTY;
        ret
    }

    fn set_dirty(&mut self) {
        self.0 |= Self::DIRTY;
    }

    pub fn order(&self) -> u32 {
        (self.0 & Self::ORDER_MASK) as u32
    }

    #[allow(dead_code)]
    fn set_order(&mut self, order: u32) {
        self.0 = (self.0 & !Self::ORDER_MASK) | (order as u64);
    }

    fn with_order_offset(self, offset: u32) -> Self {
        let new_order = self.order() + offset;
        Self((self.0 & !Self::ORDER_MASK) | (new_order as u64))
    }
}

#[derive(Default, Debug, PartialEq, Eq, Clone, Copy, Hash, Ord, PartialOrd)]
pub struct ScriptMapValue {
    pub tag: ScriptMapTag,
    pub value: ScriptValue,
}

#[derive(Default, Debug, PartialEq, Eq, Clone, Copy, Hash, Ord, PartialOrd)]
pub struct ScriptVecValue {
    pub key: ScriptValue,
    pub value: ScriptValue,
}

#[derive(Default, Debug)]
pub struct ScriptObjectData {
    pub tag: ScriptObjectTag,
    pub proto: ScriptValue,
    pub map: ScriptObjectMap,
    pub vec: Vec<ScriptVecValue>,
}

impl ScriptObjectData {
    pub fn add_type_methods(native: &mut ScriptNative, heap: &mut ScriptHeap) {
        native.add_type_method(
            heap,
            ScriptValueType::REDUX_OBJECT,
            id!(proto),
            &[],
            |vm, args| {
                if let Some(sself) = script_value!(vm, args.self).as_object() {
                    return vm.bx.heap.proto(sself);
                }
                script_err_unexpected!(
                    vm.bx.threads.cur_ref().trap,
                    "proto called on non-object value"
                )
            },
        );

        native.add_type_method(
            heap,
            ScriptValueType::REDUX_OBJECT,
            id!(push),
            &[],
            |vm, args| {
                if let Some(sself) = script_value!(vm, args.self).as_object() {
                    let trap = vm.bx.threads.cur().trap.pass();
                    return vm.bx.heap.vec_push_vec(sself, args, trap);
                }
                script_err_unexpected!(
                    vm.bx.threads.cur_ref().trap,
                    "push called on non-object value"
                )
            },
        );

        native.add_type_method(
            heap,
            ScriptValueType::REDUX_OBJECT,
            id!(pop),
            &[],
            |vm, args| {
                if let Some(sself) = script_value!(vm, args.self).as_object() {
                    let trap = vm.bx.threads.cur().trap.pass();
                    return vm.bx.heap.vec_pop(sself, trap).value;
                }
                script_err_unexpected!(
                    vm.bx.threads.cur_ref().trap,
                    "pop called on non-object value"
                )
            },
        );

        native.add_type_method(
            heap,
            ScriptValueType::REDUX_OBJECT,
            id!(len),
            &[],
            |vm, args| {
                if let Some(sself) = script_value!(vm, args.self).as_object() {
                    return vm.bx.heap.vec_len(sself).into();
                }
                script_err_unexpected!(
                    vm.bx.threads.cur_ref().trap,
                    "len called on non-object value"
                )
            },
        );

        native.add_type_method(
            heap,
            ScriptValueType::REDUX_OBJECT,
            id!(vec_len),
            &[],
            |vm, args| {
                if let Some(sself) = script_value!(vm, args.self).as_object() {
                    return vm.bx.heap.vec_len(sself).into();
                }
                script_err_unexpected!(
                    vm.bx.threads.cur_ref().trap,
                    "vec_len called on non-object value"
                )
            },
        );

        native.add_type_method(
            heap,
            ScriptValueType::REDUX_OBJECT,
            id!(map_len),
            &[],
            |vm, args| {
                if let Some(sself) = script_value!(vm, args.self).as_object() {
                    return vm.bx.heap.map_len(sself).into();
                }
                script_err_unexpected!(
                    vm.bx.threads.cur_ref().trap,
                    "map_len called on non-object value"
                )
            },
        );

        native.add_type_method(
            heap,
            ScriptValueType::REDUX_OBJECT,
            id!(delete),
            script_args!(key = NIL),
            |vm, args| {
                if let Some(sself) = script_value!(vm, args.self).as_object() {
                    let key = script_value!(vm, args.key);
                    if let Some(val) = vm.bx.heap.map_delete(sself, &key) {
                        return val;
                    }
                    return NIL;
                }
                script_err_unexpected!(
                    vm.bx.threads.cur_ref().trap,
                    "delete called on non-object value"
                )
            },
        );

        native.add_type_method(
            heap,
            ScriptValueType::REDUX_OBJECT,
            id!(vec_key),
            script_args!(index = NIL),
            |vm, args| {
                if let Some(sself) = script_value!(vm, args.self).as_object() {
                    let index = script_value!(vm, args.index);
                    let idx = index.as_index();
                    let kv = vm
                        .bx
                        .heap
                        .vec_key_value(sself, idx, vm.bx.threads.cur().trap.pass());
                    if let Some(id) = kv.key.as_id() {
                        return id.escape();
                    }
                    return kv.key;
                }
                script_err_unexpected!(
                    vm.bx.threads.cur_ref().trap,
                    "vec_key called on non-object value"
                )
            },
        );

        native.add_type_method(
            heap,
            ScriptValueType::REDUX_OBJECT,
            id!(gc_id),
            &[],
            |vm, args| {
                if let Some(sself) = script_value!(vm, args.self).as_object() {
                    return sself.index().into();
                }
                script_err_unexpected!(
                    vm.bx.threads.cur_ref().trap,
                    "gc_id called on non-object value"
                )
            },
        );

        native.add_type_method(
            heap,
            ScriptValueType::REDUX_OBJECT,
            id!(extend),
            &[],
            |vm, args| {
                if let Some(sself) = script_value!(vm, args.self).as_object() {
                    let trap = vm.bx.threads.cur().trap.pass();
                    return vm.bx.heap.vec_push_vec_of_vec(sself, args, false, trap);
                }
                script_err_unexpected!(
                    vm.bx.threads.cur_ref().trap,
                    "extend called on non-object value"
                )
            },
        );

        native.add_type_method(
            heap,
            ScriptValueType::REDUX_OBJECT,
            id!(splat),
            &[],
            |vm, args| {
                if let Some(sself) = script_value!(vm, args.self).as_object() {
                    let trap = vm.bx.threads.cur().trap.pass();
                    return vm.bx.heap.vec_push_vec_of_vec(sself, args, true, trap);
                }
                script_err_unexpected!(
                    vm.bx.threads.cur_ref().trap,
                    "import called on non-object value"
                )
            },
        );

        native.add_type_method(
            heap,
            ScriptValueType::REDUX_OBJECT,
            id!(freeze),
            &[],
            |vm, args| {
                if let Some(sself) = script_value!(vm, args.self).as_object() {
                    vm.bx.heap.freeze(sself);
                    return sself.into();
                }
                script_err_unexpected!(
                    vm.bx.threads.cur_ref().trap,
                    "freeze called on non-object value"
                )
            },
        );

        native.add_type_method(
            heap,
            ScriptValueType::REDUX_OBJECT,
            id!(freeze_api),
            &[],
            |vm, args| {
                if let Some(sself) = script_value!(vm, args.self).as_object() {
                    vm.bx.heap.freeze_api(sself);
                    return sself.into();
                }
                script_err_unexpected!(
                    vm.bx.threads.cur_ref().trap,
                    "freeze_api called on non-object value"
                )
            },
        );

        native.add_type_method(
            heap,
            ScriptValueType::REDUX_OBJECT,
            id!(freeze_module),
            &[],
            |vm, args| {
                if let Some(sself) = script_value!(vm, args.self).as_object() {
                    vm.bx.heap.freeze_module(sself);
                    return sself.into();
                }
                script_err_unexpected!(
                    vm.bx.threads.cur_ref().trap,
                    "freeze_module called on non-object value"
                )
            },
        );

        native.add_type_method(
            heap,
            ScriptValueType::REDUX_OBJECT,
            id!(freeze_component),
            &[],
            |vm, args| {
                if let Some(sself) = script_value!(vm, args.self).as_object() {
                    vm.bx.heap.freeze_component(sself);
                    return sself.into();
                }
                script_err_unexpected!(
                    vm.bx.threads.cur_ref().trap,
                    "freeze_component called on non-object value"
                )
            },
        );

        native.add_type_method(
            heap,
            ScriptValueType::REDUX_OBJECT,
            id!(retain),
            script_args!(cb = NIL),
            |vm, args| {
                if let Some(sself) = script_value!(vm, args.self).as_object() {
                    let fnptr = script_value!(vm, args.cb);
                    let mut i = 0;
                    while i < vm.bx.heap.vec_len(sself) {
                        let value = script_value!(vm, sself[i]);
                        let ret = vm.call(fnptr, &[value]);
                        if ret.is_err() {
                            return ret;
                        }
                        if !vm.bx.heap.cast_to_bool(ret) {
                            let trap = vm.bx.threads.cur().trap.pass();
                            vm.bx.heap.vec_remove(sself, i, trap);
                        } else {
                            i += 1
                        }
                    }
                    return NIL;
                }
                script_err_not_impl!(
                    vm.bx.threads.cur_ref().trap,
                    "retain called on non-object value"
                )
            },
        );
    }

    pub fn map_insert(&mut self, key: ScriptValue, value: ScriptValue) {
        if self.tag.is_tracked() {
            let order = self.map.len() as u32;
            match self.map.entry(key) {
                Entry::Occupied(mut occ) => {
                    let old = occ.get_mut();
                    if old.value != value {
                        old.tag.set_dirty();
                        self.tag.set_dirty();
                        old.value = value;
                    }
                    return;
                }
                Entry::Vacant(vac) => {
                    vac.insert(ScriptMapValue {
                        value,
                        tag: ScriptMapTag::dirty_with_order(order),
                    });
                    return;
                }
            }
        } else {
            let order = self.map.len() as u32;
            self.map.insert(
                key,
                ScriptMapValue {
                    value,
                    tag: ScriptMapTag::dirty_with_order(order),
                },
            );
        }
    }

    pub fn map_set_if_exist(&mut self, key: ScriptValue, value: ScriptValue) -> bool {
        if self.tag.is_tracked() {
            match self.map.entry(key) {
                Entry::Occupied(mut occ) => {
                    let old = occ.get_mut();
                    if old.value != value {
                        old.tag.set_dirty();
                        self.tag.set_dirty();
                        old.value = value;
                    }
                    return true;
                }
                Entry::Vacant(_) => {}
            }
        }
        if let Some(val) = self.map.get_mut(&key) {
            val.value = value;
            return true;
        }
        false
    }

    pub fn map_get(&self, key: &ScriptValue) -> Option<ScriptValue> {
        if let Some(val) = self.map.get(key) {
            Some(val.value)
        } else {
            None
        }
    }

    pub fn map_get_if_dirty(&mut self, key: &ScriptValue) -> Option<ScriptValue> {
        if self.tag.is_tracked() {
            match self.map.entry(*key) {
                Entry::Occupied(mut occ) => {
                    let val = occ.get_mut();
                    if val.tag.get_and_clear_dirty() {
                        return Some(val.value);
                    }
                    return None;
                }
                Entry::Vacant(_) => return None,
            };
        }
        self.map_get(key)
    }

    pub fn map_delete(&mut self, key: &ScriptValue) -> Option<ScriptValue> {
        self.map.remove(key).map(|v| v.value)
    }

    pub fn map_len(&self) -> usize {
        self.map.len()
    }

    pub fn map_iter_ret<T, F: FnMut(ScriptValue, ScriptValue) -> Option<T>>(
        &self,
        mut f: F,
    ) -> Option<T> {
        for (key, val) in self.map.iter() {
            let r = f(*key, val.value);
            if r.is_some() {
                return r;
            }
        }
        None
    }

    pub fn map_iter<F: FnMut(ScriptValue, ScriptValue)>(&self, mut f: F) {
        for (key, val) in self.map.iter() {
            f(*key, val.value);
        }
    }

    pub fn map_iter_ordered<F: FnMut(ScriptValue, ScriptValue)>(&self, mut f: F) {
        let mut ordered: Vec<_> = self.map.iter().collect();
        ordered.sort_by_key(|(_, val)| val.tag.order());
        for (key, val) in ordered {
            f(*key, val.value);
        }
    }

    pub fn merge_map_from_other(&mut self, other: &ScriptObjectData) {
        let offset = self.map.len() as u32;
        for (k, v) in other.map.iter() {
            self.map.insert(
                *k,
                ScriptMapValue {
                    value: v.value,
                    tag: v.tag.with_order_offset(offset),
                },
            );
        }
    }

    /// Merge map entries from other, but only if the key doesn't already exist in self.
    /// Used by the splat operator to not overwrite existing values.
    pub fn merge_map_from_other_no_overwrite(&mut self, other: &ScriptObjectData) {
        let offset = self.map.len() as u32;
        for (k, v) in other.map.iter() {
            if !self.map.contains_key(k) {
                self.map.insert(
                    *k,
                    ScriptMapValue {
                        value: v.value,
                        tag: v.tag.with_order_offset(offset),
                    },
                );
            }
        }
    }

    pub fn push_vec_from_other(&mut self, other: &ScriptObjectData) {
        self.vec.extend_from_slice(&other.vec);
    }

    //const DONT_RECYCLE_WHEN: usize = 1000;
    pub fn with_proto(proto: ScriptValue) -> Self {
        Self {
            proto,
            ..Default::default()
        }
    }

    pub fn clear(&mut self) {
        self.proto = NIL;
        self.tag.clear();
        self.map.clear();
        self.vec.clear();
        // Debug: verify clear worked
        debug_assert!(self.map.is_empty(), "map.clear() didn't work!");
    }
}
