use makepad_relax_api::relax_api;
use crate::apply::Apply;
use crate::heap::*;
use crate::makepad_live_id::*;
use crate::mod_shader::ShaderIoType;
use crate::suggest::*;
use crate::traits::*;
use crate::trap::*;
use crate::value::*;
use crate::*;

/// An object's header (obj_info): the tag, the prototype, the construction site and the sizes of
/// its vec and map.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ObjInfo {
    pub tag: ScriptObjectTag,
    pub proto: ScriptValue,
    pub made_at: ScriptIp,
    pub vec_len: usize,
    pub map_len: usize,
}

/// What obj_get answers: the value, and whether an optional lookup found anything.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ObjGot {
    pub value: ScriptValue,
    pub found: bool,
}

/// What obj_iter_at answers: the entry (when `found`) and the object's vec and map sizes.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ObjIterAt {
    pub key: ScriptValue,
    pub value: ScriptValue,
    pub found: bool,
    pub vec_len: usize,
    pub map_len: usize,
}

// obj_get modes
/// `value`: the key's value along the prototype chain.
pub const OBJ_GET_VALUE: u32 = 0;
/// `scope_value`: the key (an id) in a scope chain.
pub const OBJ_GET_SCOPE: u32 = 1;
/// `value_for_apply`: map lookup along the chain; `found` false when absent.
pub const OBJ_GET_APPLY: u32 = 2;
/// `value_for_apply` for an eval: the object's own map only.
pub const OBJ_GET_APPLY_EVAL: u32 = 3;
/// The object's own map value; `found` false when absent.
pub const OBJ_GET_MAP_OWN: u32 = 4;
/// The module registered under the key (an id); `obj` unused.
pub const OBJ_GET_MODULE: u32 = 5;
/// The root of the module tree (`mod`).
pub const OBJ_GET_MODULES_ROOT: u32 = 6;
/// A method of `obj` by name, the prototype chain included.
pub const OBJ_GET_METHOD: u32 = 7;

// obj_set modes
pub const OBJ_SET_VALUE: u32 = 0;
pub const OBJ_SET_VEC: u32 = 1;
pub const OBJ_SET_MAP_OWN: u32 = 2;
pub const OBJ_SET_MAP_CONVERTED: u32 = 3;
pub const OBJ_SET_POKE_SCOPE: u32 = 4;
pub const OBJ_SET_REBIND_SCOPE: u32 = 5;
pub const OBJ_SET_VEC_PUSH: u32 = 6;
pub const OBJ_SET_VEC_PUSH_UNCHECKED: u32 = 7;
/// `value`: an object whose vector part is appended to `obj`'s.
pub const OBJ_SET_VEC_PUSH_VEC: u32 = 8;
/// `value`: an object whose fields are copied into `obj`.
pub const OBJ_SET_MERGE: u32 = 9;
/// Write `value` where the name `key` lives: a named child in `obj`'s own vec is replaced in
/// place, anything else is `set_value`.
pub const OBJ_SET_SLOT: u32 = 10;

// obj_new modes
pub const OBJ_NEW: u32 = 0;
pub const OBJ_NEW_WITH_PROTO: u32 = 1;
pub const OBJ_NEW_WITH_PROTO_NO_VEC: u32 = 2;
pub const OBJ_NEW_MODULE: u32 = 3;

// obj_mode modes
pub const OBJ_MODE_FREEZE: u32 = 0;
pub const OBJ_MODE_FREEZE_COMPONENT: u32 = 1;
pub const OBJ_MODE_FREEZE_SHADER: u32 = 2;
pub const OBJ_MODE_FREEZE_EXT: u32 = 3;
pub const OBJ_MODE_FREEZE_API: u32 = 4;
pub const OBJ_MODE_STORAGE_VEC2: u32 = 5;
pub const OBJ_MODE_STORAGE_AUTO: u32 = 6;
pub const OBJ_MODE_CLEAR_DEEP: u32 = 7;
pub const OBJ_MODE_APPLY_TRANSFORM: u32 = 8;
pub const OBJ_MODE_TYPE: u32 = 9;
pub const OBJ_MODE_STRING_KEYS: u32 = 10;
pub const OBJ_MODE_SHADER_IO: u32 = 11;
pub const OBJ_MODE_MADE_AT: u32 = 12;
pub const OBJ_MODE_REFFED: u32 = 13;
pub const OBJ_MODE_TYPE_DEFAULT: u32 = 14;

#[relax_api]
impl ScriptHeap {
    // New objects

    #[inline]
    pub(crate) fn charge_object_vec_entries(
        &mut self,
        entries: usize,
        operation: &'static str,
    ) -> bool {
        let bytes = entries
            .checked_mul(std::mem::size_of::<ScriptVecValue>())
            .unwrap_or(usize::MAX);
        self.charge_allocation(bytes, operation)
    }

    #[inline]
    pub(crate) fn charge_object_map_entry(
        &mut self,
        ptr: ScriptObject,
        key: ScriptValue,
        operation: &'static str,
    ) -> bool {
        self.heap_charge(HEAP_CHARGE_MAP_ENTRY, ptr, key, 0, operation)
    }

    pub(crate) fn charge_object_map_entry_impl(
        &mut self,
        ptr: ScriptObject,
        key: ScriptValue,
        operation: &'static str,
    ) -> bool {
        if self.is_allocation_poison_object(ptr) {
            return false;
        }
        if self.objects[ptr].map.contains_key(&key) {
            return true;
        }
        // ValueMap is a compact Vec for small maps and spills to a HashMap.
        // Charge one key/value pair plus one word each of table/index
        // overhead. This is deliberately conservative across both layouts.
        let bytes = std::mem::size_of::<(ScriptValue, ScriptMapValue)>()
            .saturating_add(2 * std::mem::size_of::<usize>());
        self.charge_allocation(bytes, operation)
    }

    /// Insert an already converted Rust-map entry with the same budget charge
    /// and string-key tag as ScriptApply's map conversion. Collection owners
    /// can stream entries without exposing their Rust storage to the VM.
    #[inline]
    pub fn map_insert_converted(
        &mut self,
        object: ScriptObject,
        key: ScriptValue,
        value: ScriptValue,
    ) {
        self.obj_set(OBJ_SET_MAP_CONVERTED, object, key, value, NoTrap);
    }

    fn map_insert_converted_impl(
        &mut self,
        object: ScriptObject,
        key: ScriptValue,
        value: ScriptValue,
    ) {
        if self.charge_object_map_entry(object, key, "converting a Rust map to a script object") {
            self.objects[object].map.insert(key, ScriptMapValue { tag: Default::default(), value });
        }
        if key.is_string_like() {
            self.objects[object].tag.set_string_keys();
        }
    }

    /// `charge_object_map_entry` then `map_insert`, with one scan of the map
    /// for an untracked object: an existing key is updated in place (no
    /// charge, order kept), an absent one is charged and appended.
    #[inline]
    pub(crate) fn bind_map_entry(
        &mut self,
        ptr: ScriptObject,
        key: ScriptValue,
        value: ScriptValue,
        operation: &'static str,
    ) {
        if self.is_allocation_poison_object(ptr) {
            return;
        }
        let object = &mut self.objects[ptr];
        if object.tag.is_tracked() {
            if self.charge_object_map_entry(ptr, key, operation) {
                self.objects[ptr].map_insert(key, value);
            }
            return;
        }
        if let Some(existing) = object.map.get_mut(&key) {
            existing.value = value;
            return;
        }
        let bytes = std::mem::size_of::<(ScriptValue, ScriptMapValue)>()
            .saturating_add(2 * std::mem::size_of::<usize>());
        if !self.charge_allocation(bytes, operation) {
            return;
        }
        self.objects[ptr].map_insert_absent_untracked(key, value);
    }

    fn new_object_impl(&mut self) -> ScriptObject {
        if let Some(obj) = self.objects_free.pop() {
            // obj already has the correct generation from gc.rs sweep
            let object = &mut self.objects[obj];
            debug_assert!(
                object.map.is_empty() && object.vec.is_empty() && object.proto.is_nil(),
                "reused object slot was not fully cleared before reuse"
            );
            object.tag.set_alloced();
            object.proto = id!(object).into();
            obj
        } else {
            if !self.charge_allocation(
                std::mem::size_of::<ScriptObjectData>(),
                "creating an object",
            ) {
                return self.allocation_poison_object;
            }
            let index = self.objects.len() as u32;
            let mut object = ScriptObjectData::default();
            object.tag.set_alloced();
            object.proto = id!(object).into();
            let (_, generation) = self.objects.push(object);
            ScriptObject::new(index, generation)
        }
    }

    // ---- the object core: the ops over the object store. Everything else on objects is ordinary
    // code over these. Each op dispatches on a mode word to the body the method had; a
    // constant mode folds away when the op is inlined natively, so the call costs what the
    // method did.

    /// Header of `obj`: its tag, proto, made_at and the sizes of its vec and map.
    #[inline]
    #[relax_api]
    pub fn obj_info(&self, obj: ScriptObject) -> ObjInfo {
        let object = &self.objects[obj];
        ObjInfo {
            tag: object.tag,
            proto: object.proto,
            made_at: object.made_at,
            vec_len: object.vec.len(),
            map_len: object.map.len(),
        }
    }

    /// Reads from `obj` by `mode` (OBJ_GET_*): `found` is false when an optional lookup missed.
    #[inline]
    #[relax_api]
    pub fn obj_get(&self, mode: u32, obj: ScriptObject, key: ScriptValue, trap: ScriptTrap) -> ObjGot {
        match mode {
            OBJ_GET_VALUE => ObjGot { value: self.value_impl(obj, key, trap), found: true },
            OBJ_GET_SCOPE => ObjGot { value: self.scope_value_impl(obj, key, trap), found: true },
            OBJ_GET_APPLY | OBJ_GET_APPLY_EVAL => {
                let got = self.value_for_apply_impl(obj, key, mode == OBJ_GET_APPLY_EVAL);
                ObjGot { value: got.unwrap_or(NIL), found: got.is_some() }
            }
            OBJ_GET_MAP_OWN => match self.objects[obj].map.get(&key) {
                Some(v) => ObjGot { value: v.value, found: true },
                None => ObjGot { value: NIL, found: false },
            },
            OBJ_GET_MODULE => ObjGot { value: self.value_impl(self.modules, key, NoTrap), found: true },
            OBJ_GET_MODULES_ROOT => ObjGot { value: self.modules.into(), found: true },
            OBJ_GET_METHOD => ObjGot { value: self.value_deep_map(obj, key, trap), found: true },
            _ => unreachable!("obj_get mode"),
        }
    }

    /// Writes to `obj` by `mode` (OBJ_SET_*). The result is the mode's: an error value or NIL, or
    /// a bool for the poke and rebind modes.
    #[inline]
    #[relax_api]
    pub fn obj_set(&mut self, mode: u32, obj: ScriptObject, key: ScriptValue, value: ScriptValue, trap: ScriptTrap) -> ScriptValue {
        match mode {
            OBJ_SET_VALUE => self.set_value_impl(obj, key, value, trap),
            OBJ_SET_VEC => self.set_value_vec_impl(obj, key, value, trap),
            OBJ_SET_SLOT => self.set_value_slot_impl(obj, key, value, trap),
            OBJ_SET_MAP_OWN => {
                self.objects[obj].map.insert(key, ScriptMapValue { tag: Default::default(), value });
                NIL
            }
            OBJ_SET_MAP_CONVERTED => {
                self.map_insert_converted_impl(obj, key, value);
                NIL
            }
            OBJ_SET_POKE_SCOPE => ScriptValue::from_bool(self.poke_scope_value_impl(obj, key, value)),
            OBJ_SET_REBIND_SCOPE => ScriptValue::from_bool(self.rebind_scope_value_impl(obj, key, value)),
            OBJ_SET_VEC_PUSH => self.vec_push_impl(obj, key, value, trap),
            OBJ_SET_VEC_PUSH_VEC => self.vec_push_vec_impl(obj, value.as_object().expect("vec_push_vec of an object"), trap),
            OBJ_SET_MERGE => self.merge_object_impl(obj, value.as_object().expect("merge_object of an object"), trap),
            OBJ_SET_VEC_PUSH_UNCHECKED => {
                self.vec_push_unchecked_impl(obj, key, value);
                NIL
            }
            _ => unreachable!("obj_set mode"),
        }
    }

    /// The `index`-th entry of `obj` across its vec then its map (a `for k, v in obj` step).
    #[inline]
    #[relax_api]
    pub fn obj_iter_at(&self, obj: ScriptObject, index: usize) -> ObjIterAt {
        let object = &self.objects[obj];
        let vec_len = object.vec.len();
        if index < vec_len {
            let kv = object.vec[index];
            return ObjIterAt { key: kv.key, value: kv.value, found: true, vec_len, map_len: object.map.len() };
        }
        match object.map.iter().nth(index - vec_len) {
            Some((key, map_val)) => ObjIterAt { key: *key, value: map_val.value, found: true, vec_len, map_len: object.map.len() },
            None => ObjIterAt { key: NIL, value: NIL, found: false, vec_len, map_len: object.map.len() },
        }
    }

    /// A new object by `mode` (OBJ_NEW*). `proto` is the prototype, or for OBJ_NEW_MODULE the
    /// module's name as an id value.
    #[inline]
    #[relax_api]
    pub fn obj_new(&mut self, mode: u32, proto: ScriptValue) -> ScriptObject {
        match mode {
            OBJ_NEW => self.new_object_impl(),
            OBJ_NEW_WITH_PROTO => self.new_with_proto_impl(proto, true),
            OBJ_NEW_WITH_PROTO_NO_VEC => self.new_with_proto_impl(proto, false),
            OBJ_NEW_MODULE => {
                let md = self.new_with_proto_impl(proto, true);
                self.set_value_impl(self.modules, proto, md.into(), NoTrap);
                md
            }
            _ => unreachable!("obj_new mode"),
        }
    }

    /// Changes the tag or header of `obj` by `mode` (OBJ_MODE_*); `arg` is the mode's operand.
    /// OBJ_MODE_TYPE_DEFAULT answers whether `obj` has a type (1) to be the default of.
    #[inline]
    #[relax_api]
    pub fn obj_mode(&mut self, obj: ScriptObject, mode: u32, arg: u64) -> u64 {
        let object = &mut self.objects[obj];
        match mode {
            OBJ_MODE_FREEZE => object.tag.freeze(),
            OBJ_MODE_FREEZE_COMPONENT => object.tag.freeze_component(),
            OBJ_MODE_FREEZE_SHADER => object.tag.freeze_shader(),
            OBJ_MODE_FREEZE_EXT => object.tag.freeze_ext(),
            OBJ_MODE_FREEZE_API => object.tag.freeze_api(),
            OBJ_MODE_STORAGE_VEC2 => object.tag.set_vec2(),
            OBJ_MODE_STORAGE_AUTO => object.tag.set_auto(),
            OBJ_MODE_CLEAR_DEEP => object.tag.clear_deep(),
            OBJ_MODE_APPLY_TRANSFORM => object.tag.set_apply_transform(NativeId { index: arg as u32 }),
            OBJ_MODE_TYPE => object.tag.set_type_index(ScriptTypeIndex(arg as u32)),
            OBJ_MODE_STRING_KEYS => object.tag.set_string_keys(),
            OBJ_MODE_SHADER_IO => object.tag.set_shader_io(ShaderIoType(arg as u32)),
            OBJ_MODE_MADE_AT => object.made_at = ScriptIp { body: (arg >> 32) as u16, index: arg as u32 },
            OBJ_MODE_REFFED => object.tag.set_reffed(),
            OBJ_MODE_TYPE_DEFAULT => {
                if let Some(ty_index) = object.tag.as_type_index() {
                    // Add to type_defaults mapping (GC will scan this table)
                    self.type_defaults.insert(ty_index, obj);
                    return 1;
                }
                return 0;
            }
            _ => unreachable!("obj_mode mode"),
        }
        0
    }

    /// The root of the module tree (`mod`).
    #[inline]
    pub fn modules_root(&self) -> ScriptObject {
        self.obj_get(OBJ_GET_MODULES_ROOT, ScriptObject::ZERO, NIL, NoTrap).value.into()
    }

    #[inline]
    pub fn module(&self, id: LiveId) -> ScriptObject {
        self.obj_get(OBJ_GET_MODULE, ScriptObject::ZERO, id.into(), NoTrap).value.into()
    }

    #[inline]
    pub fn new_module(&mut self, id: LiveId) -> ScriptObject {
        self.obj_new(OBJ_NEW_MODULE, id.into())
    }

    #[inline]
    pub fn value(&self, ptr: ScriptObject, key: ScriptValue, trap: ScriptTrap) -> ScriptValue {
        self.obj_get(OBJ_GET_VALUE, ptr, key, trap).value
    }

    #[inline]
    pub fn scope_value(&self, ptr: ScriptObject, key: LiveId, trap: ScriptTrap) -> ScriptValue {
        self.obj_get(OBJ_GET_SCOPE, ptr, key.into(), trap).value
    }

    #[inline]
    pub fn value_for_apply(&mut self, obj: ScriptValue, key: ScriptValue, apply: &Apply) -> Option<ScriptValue> {
        let ptr = obj.as_object()?;
        let mode = if apply.is_eval() { OBJ_GET_APPLY_EVAL } else { OBJ_GET_APPLY };
        let got = self.obj_get(mode, ptr, key, NoTrap);
        got.found.then_some(got.value)
    }

    #[inline]
    pub fn set_value(&mut self, ptr: ScriptObject, key: ScriptValue, value: ScriptValue, trap: ScriptTrap) -> ScriptValue {
        self.obj_set(OBJ_SET_VALUE, ptr, key, value, trap)
    }

    #[inline]
    pub fn set_value_vec(&mut self, ptr: ScriptObject, key: ScriptValue, value: ScriptValue, trap: ScriptTrap) -> ScriptValue {
        self.obj_set(OBJ_SET_VEC, ptr, key, value, trap)
    }

    /// Replace the value an existing binding holds, found as
    /// [`Self::set_scope_value`] finds it, even in a frozen object: a host
    /// editing the running code's values in place (an editor dragging a
    /// constant). Whether the binding was found.
    #[inline]
    pub fn poke_scope_value(&mut self, ptr: ScriptObject, key: LiveId, value: ScriptValue) -> bool {
        self.obj_set(OBJ_SET_POKE_SCOPE, ptr, key.into(), value, NoTrap).as_bool().unwrap_or(false)
    }

    /// The host rebinding `key` where a scope chain from `ptr` binds it (the
    /// nearest scope that has it), past a frozen document: what a script
    /// cannot do, a host does between runs (a value it recomputes per frame).
    /// False when no scope in the chain binds `key`.
    #[inline]
    pub fn rebind_scope_value(&mut self, ptr: ScriptObject, key: LiveId, value: ScriptValue) -> bool {
        self.obj_set(OBJ_SET_REBIND_SCOPE, ptr, key.into(), value, NoTrap).as_bool().unwrap_or(false)
    }

    pub fn new_with_proto_checked(&mut self, proto: ScriptValue, trap: ScriptTrap) -> ScriptObject {
        if let Some(ptr) = proto.as_object() {
            let object = &mut self.objects[ptr];
            if object.tag.is_notproto() {
                script_err_wrong_value!(trap, "object marked as not usable as prototype");
                return ScriptObject::ZERO;
            }
        }
        self.new_with_proto(proto)
    }

    #[inline]
    pub fn new_object(&mut self) -> ScriptObject {
        self.obj_new(OBJ_NEW, NIL)
    }

    #[inline]
    pub fn new_with_proto(&mut self, proto: ScriptValue) -> ScriptObject {
        self.obj_new(OBJ_NEW_WITH_PROTO, proto)
    }

    #[inline]
    pub fn new_with_proto_no_vec(&mut self, proto: ScriptValue) -> ScriptObject {
        self.obj_new(OBJ_NEW_WITH_PROTO_NO_VEC, proto)
    }

    fn new_with_proto_impl(
        &mut self,
        proto: ScriptValue,
        copy_vec_from_auto_proto: bool,
    ) -> ScriptObject {
        let (proto_fwd, proto_ptr) = if let Some(ptr) = proto.as_object() {
            // Use checked access via ScriptObject. Do not mutate the
            // prototype until all script-controlled growth is preflighted.
            (self.objects[ptr].tag.proto_fwd(), ptr)
        } else {
            let ptr = self.new_object_impl();
            if self.is_allocation_poison_object(ptr) {
                return ptr;
            }
            self.objects[ptr].proto = proto;
            return ptr;
        };

        let copy_count = if copy_vec_from_auto_proto && self.objects[proto_ptr].tag.is_auto() {
            self.objects[proto_ptr].vec.len()
        } else {
            0
        };
        let metadata_bytes = if self.objects_free.is_empty() {
            std::mem::size_of::<ScriptObjectData>()
        } else {
            0
        };
        let copy_bytes = copy_count
            .checked_mul(std::mem::size_of::<ScriptVecValue>())
            .unwrap_or(usize::MAX);
        if !self.charge_allocation(
            metadata_bytes.checked_add(copy_bytes).unwrap_or(usize::MAX),
            "creating an object with a prototype",
        ) {
            return self.allocation_poison_object;
        }
        self.objects[proto_ptr].tag.set_reffed();

        if let Some(obj) = self.objects_free.pop() {
            // obj already has the correct generation from gc.rs sweep
            let index = obj.index;

            // Use raw index for split_at_mut (internal operation)
            let (object, proto_object) = if index > proto_ptr.index {
                let (o1, o2) = self.objects.slots_split_at_mut(index as usize);
                (&mut o2[0].data, &mut o1[proto_ptr.index as usize].data)
            } else {
                let (o1, o2) = self.objects.slots_split_at_mut(proto_ptr.index as usize);
                (&mut o1[index as usize].data, &mut o2[0].data)
            };
            debug_assert!(
                object.map.is_empty() && object.vec.is_empty() && object.proto.is_nil(),
                "reused object slot was not fully cleared before proto allocation"
            );
            object.tag.set_alloced();
            object.tag.set_proto_fwd(proto_fwd);
            object.proto = proto;
            // only copy vec if we are 'auto' otherwise we proto inherit normally
            if copy_vec_from_auto_proto && proto_object.tag.is_auto() {
                object.vec.extend_from_slice(&proto_object.vec);
            }
            obj
        } else {
            let index = self.objects.len() as u32;
            let mut object = ScriptObjectData::with_proto(proto);
            object.tag.set_alloced();
            object.tag.set_proto_fwd(proto_fwd);
            let proto_object = &self.objects[proto_ptr];
            if copy_vec_from_auto_proto && proto_object.tag.is_auto() {
                object.vec.extend_from_slice(&proto_object.vec);
            }
            let (_, generation) = self.objects.push(object);
            ScriptObject::new(index, generation)
        }
    }

    /// Record the ip of the BEGIN opcode that constructed this object.
    /// Set by handle_begin_proto / handle_begin_bare; stays
    /// ScriptIp::UNKNOWN for Rust-built objects.
    #[inline]
    pub fn set_made_at(&mut self, ptr: ScriptObject, ip: ScriptIp) {
        self.obj_mode(ptr, OBJ_MODE_MADE_AT, ((ip.body as u64) << 32) | ip.index as u64);
    }

    /// The construction-site ip of this object (ScriptIp::UNKNOWN if it
    /// was not built by a script object literal).
    #[inline]
    pub fn made_at(&self, ptr: ScriptObject) -> ScriptIp {
        self.obj_info(ptr).made_at
    }

    #[inline]
    pub fn new_if_reffed(&mut self, ptr: ScriptObject) -> ScriptObject {
        let obj = &self.objects[ptr];
        if obj.tag.is_reffed() {
            let proto = obj.proto;
            return self.new_with_proto(proto);
        }
        return ptr;
    }

    // Object flagv

    #[inline]
    pub fn set_object_deep(&mut self, ptr: ScriptObject) {
        self.objects[ptr].tag.set_deep()
    }

    #[inline]
    pub fn set_object_storage_vec2(&mut self, ptr: ScriptObject) {
        self.obj_mode(ptr, OBJ_MODE_STORAGE_VEC2, 0);
    }

    #[inline]
    pub fn set_object_storage_auto(&mut self, ptr: ScriptObject) {
        self.obj_mode(ptr, OBJ_MODE_STORAGE_AUTO, 0);
    }

    #[inline]
    pub fn set_object_pod_type(&mut self, ptr: ScriptObject, pt: ScriptPodType) {
        self.objects[ptr].tag.set_pod_type(pt)
    }

    #[inline]
    pub fn set_first_applied_and_clean(&mut self, ptr: ScriptObject) {
        self.objects[ptr].tag.set_first_applied_and_clean()
    }

    #[inline]
    pub fn clear_object_deep(&mut self, ptr: ScriptObject) {
        self.obj_mode(ptr, OBJ_MODE_CLEAR_DEEP, 0);
    }

    #[inline]
    pub fn freeze(&mut self, ptr: ScriptObject) {
        self.obj_mode(ptr, OBJ_MODE_FREEZE, 0);
    }

    #[inline]
    pub fn set_notproto(&mut self, ptr: ScriptObject) {
        self.objects[ptr].tag.set_notproto()
    }

    #[inline]
    pub fn set_from_eval(&mut self, ptr: ScriptObject) {
        self.objects[ptr].tag.set_from_eval()
    }

    #[inline]
    pub fn is_from_eval(&self, ptr: ScriptObject) -> bool {
        self.objects[ptr].tag.is_from_eval()
    }

    #[inline]
    pub fn freeze_module(&mut self, ptr: ScriptObject) {
        self.objects[ptr].tag.freeze_module()
    }

    #[inline]
    pub fn freeze_component(&mut self, ptr: ScriptObject) {
        self.obj_mode(ptr, OBJ_MODE_FREEZE_COMPONENT, 0);
    }

    #[inline]
    pub fn freeze_shader(&mut self, ptr: ScriptObject) {
        self.obj_mode(ptr, OBJ_MODE_FREEZE_SHADER, 0);
    }

    #[inline]
    pub fn freeze_ext(&mut self, ptr: ScriptObject) {
        self.obj_mode(ptr, OBJ_MODE_FREEZE_EXT, 0);
    }

    #[inline]
    pub fn freeze_api(&mut self, ptr: ScriptObject) {
        self.obj_mode(ptr, OBJ_MODE_FREEZE_API, 0);
    }

    #[inline]
    pub fn set_object_apply_transform(&mut self, ptr: ScriptObject, ni: NativeId) {
        self.obj_mode(ptr, OBJ_MODE_APPLY_TRANSFORM, ni.index as u64);
    }

    #[inline]
    pub fn set_type(&mut self, obj: ScriptObject, ty: ScriptTypeIndex) {
        self.obj_mode(obj, OBJ_MODE_TYPE, ty.0 as u64);
    }

    #[inline]
    pub fn set_string_keys(&mut self, obj: ScriptObject) {
        self.obj_mode(obj, OBJ_MODE_STRING_KEYS, 0);
    }

    #[inline]
    pub fn set_shader_io(&mut self, obj: ScriptObject, io: ShaderIoType) {
        self.obj_mode(obj, OBJ_MODE_SHADER_IO, io.0 as u64);
    }

    #[inline]
    pub fn as_shader_io(&self, obj: ScriptObject) -> Option<ShaderIoType> {
        self.obj_info(obj).tag.as_shader_io()
    }

    // Writing object values

    pub(crate) fn force_value_in_map(
        &mut self,
        ptr: ScriptObject,
        key: ScriptValue,
        sself: ScriptValue,
    ) {
        if !self.charge_object_map_entry(ptr, key, "adding an object property") {
            return;
        }
        self.escape_value(sself);
        let object = &mut self.objects[ptr];
        object.map_insert(key, sself);
    }

    fn set_value_index(
        &mut self,
        ptr: ScriptObject,
        index: ScriptValue,
        value: ScriptValue,
        trap: ScriptTrap,
    ) -> ScriptValue {
        if self.is_allocation_poison_object(ptr) || self.objects[ptr].tag.is_vec_frozen() {
            // has rw flags
            return script_err_immutable!(trap, "cannot set index on frozen vec");
        }

        let index = index.as_index();
        let len = self.objects[ptr].vec.len();
        if index >= len {
            let additional = index
                .checked_add(1)
                .and_then(|target| target.checked_sub(len))
                .unwrap_or(usize::MAX);
            if !self.charge_object_vec_entries(additional, "growing a sparse object index") {
                return NIL;
            }
        }
        let object = &mut self.objects[ptr];
        if index >= object.vec.len() {
            object.vec.resize(index + 1, ScriptVecValue::default());
        }
        object.vec[index].value = value;
        return NIL;
    }

    fn set_value_slot_impl(
        &mut self,
        ptr: ScriptObject,
        key: ScriptValue,
        value: ScriptValue,
        trap: ScriptTrap,
    ) -> ScriptValue {
        // an instance holds its own copy of the template's vec, so its own vec is where a
        // named child lives; a typed object's set_value routes a proto's vec child itself
        if self.objects[ptr].vec.iter().any(|kv| kv.key == key) {
            return self.set_value_vec_impl(ptr, key, value, trap);
        }
        self.set_value_impl(ptr, key, value, trap)
    }

    #[inline]
    pub fn set_value_slot(&mut self, ptr: ScriptObject, key: ScriptValue, value: ScriptValue, trap: ScriptTrap) -> ScriptValue {
        self.obj_set(OBJ_SET_SLOT, ptr, key, value, trap)
    }

    fn set_value_vec_impl(
        &mut self,
        ptr: ScriptObject,
        key: ScriptValue,
        value: ScriptValue,
        trap: ScriptTrap,
    ) -> ScriptValue {
        self.escape_value(value);
        let object = &mut self.objects[ptr];
        if object.tag.is_vec_frozen() {
            return script_err_immutable!(trap, "cannot set vec key on frozen vec");
        }
        for kv in object.vec.iter_mut().rev() {
            if kv.key == key {
                kv.value = value;
                return NIL;
            }
        }
        // just append it
        if !self.charge_object_vec_entries(1, "adding an object vector property") {
            return NIL;
        }
        let object = &mut self.objects[ptr];
        object.vec.push(ScriptVecValue { key, value });
        NIL
    }

    fn set_value_deep(
        &mut self,
        ptr: ScriptObject,
        key: ScriptValue,
        value: ScriptValue,
        trap: ScriptTrap,
    ) -> ScriptValue {
        let mut ptr = ptr;
        loop {
            let object = &mut self.objects[ptr];
            let is_immutable = object.tag.is_immutable();
            // Check vec for key
            for kv in object.vec.iter_mut().rev() {
                if kv.key == key {
                    if is_immutable {
                        return script_err_immutable!(trap, "cannot modify immutable object");
                    }
                    kv.value = value;
                    return NIL;
                }
            }
            // Check map for key
            if object.map_get(&key).is_some() {
                if is_immutable {
                    return script_err_immutable!(trap, "cannot modify immutable object");
                }
                object.map_set_if_exist(key, value);
                return NIL;
            }
            if let Some(next_ptr) = object.proto.as_object() {
                ptr = next_ptr
            } else {
                break;
            }
        }
        // alright nothing found
        if self.objects[ptr].tag.is_immutable() {
            return script_err_immutable!(trap, "cannot modify immutable object");
        }
        if self.objects[ptr].tag.is_vec2() {
            if !self.charge_object_vec_entries(1, "adding a deep object vector property") {
                return NIL;
            }
            let object = &mut self.objects[ptr];
            object.vec.push(ScriptVecValue { key, value });
        } else {
            if !self.charge_object_map_entry(ptr, key, "adding a deep object property") {
                return NIL;
            }
            let object = &mut self.objects[ptr];
            object.map_insert(key, value);
        }
        NIL
    }

    #[inline]
    fn validate_type(&self, lhs: ScriptValue, rhs: ScriptValue) -> bool {
        lhs.value_type().to_redux() == rhs.value_type().to_redux()
    }

    fn set_value_shallow_checked(
        &mut self,
        top_ptr: ScriptObject,
        key: ScriptValue,
        key_id: LiveId,
        value: ScriptValue,
        trap: ScriptTrap,
    ) -> ScriptValue {
        let object = &self.objects[top_ptr];
        if object.tag.is_immutable() {
            return script_err_immutable!(trap, "cannot set property on immutable object");
        }

        if let Some(ty) = object.tag.as_type_index() {
            let check = &self.type_check[ty.0 as usize];
            if let Some(type_prop) = check.props.props.get(&key_id) {
                if let Some(ty_index) = self.type_index.get(&type_prop.ty) {
                    let check_prop = &self.type_check[ty_index.0 as usize];
                    if let Some(type_object) = &check_prop.object {
                        if !(type_object.check)(self, value) {
                            let expected = format_expected_type(self, type_object);
                            return script_err_type_mismatch!(
                                trap,
                                "type mismatch for property {:?}: expected {}, got {}",
                                key_id,
                                expected,
                                format_value_type(self, value)
                            );
                        }
                    }
                } else {
                    println!(
                        "Trying to check a type that hasnt been registered yet for {} {}",
                        key, value
                    );
                    return script_err_unknown_type!(
                        trap,
                        "type not registered for property {:?}",
                        key_id
                    );
                }
            } else if !object.tag.is_map_add() {
                // check if the key exists in the vec (for := defined properties)
                let mut found_in_vec = false;
                let mut ptr = top_ptr;
                loop {
                    let obj = &self.objects[ptr];
                    for kv in obj.vec.iter().rev() {
                        if kv.key == key {
                            found_in_vec = true;
                            break;
                        }
                    }
                    if found_in_vec {
                        break;
                    }
                    if let Some(next_ptr) = obj.proto.as_object() {
                        ptr = next_ptr;
                    } else {
                        break;
                    }
                }
                if found_in_vec {
                    return self.set_value_vec_impl(top_ptr, key, value, trap);
                }
                let type_name = check
                    .object
                    .as_ref()
                    .and_then(|o| o.name)
                    .and_then(|name| name.as_string(|s| s.map(|s| format!(" {s}"))))
                    .unwrap_or_default();
                return script_err_not_found!(
                    trap,
                    "property {:?} not defined on type{}{}",
                    key_id,
                    type_name,
                    suggest_property(self, top_ptr, key)
                );
            }
            if !self.charge_object_map_entry(top_ptr, key, "adding a checked object property") {
                return NIL;
            }
            let object = &mut self.objects[top_ptr];
            object.map_insert(key, value);
            return NIL;
        }
        // check against prototype or type
        if object.tag.is_validated() {
            let mut ptr = top_ptr;
            loop {
                let object = &self.objects[ptr];
                if object.tag.is_vec2() {
                    for kv in object.vec.iter().rev() {
                        if kv.key == key {
                            if !self.validate_type(kv.value, value) {
                                return script_err_type_mismatch!(
                                    trap,
                                    "type mismatch assigning to property {:?}: expected {}, got {}",
                                    key,
                                    format_value_type(self, kv.value),
                                    format_value_type(self, value)
                                );
                            }
                            return self.set_value_shallow(top_ptr, key, value, trap);
                        }
                    }
                }
                if let Some(set_value) = object.map_get(&key) {
                    if !self.validate_type(set_value, value) {
                        return script_err_type_mismatch!(
                            trap,
                            "type mismatch assigning to property {:?}: expected {}, got {}",
                            key,
                            format_value_type(self, set_value),
                            format_value_type(self, value)
                        );
                    }
                    return self.set_value_shallow(top_ptr, key, value, trap);
                }
                // also check vec for := defined properties on non-vec2 objects
                if !object.tag.is_vec2() {
                    for kv in object.vec.iter().rev() {
                        if kv.key == key {
                            return self.set_value_vec_impl(top_ptr, key, value, trap);
                        }
                    }
                }
                if let Some(next_ptr) = object.proto.as_object() {
                    ptr = next_ptr
                } else if !object.tag.is_map_add() {
                    // not found
                    return script_err_not_found!(
                        trap,
                        "property {:?} not found and object does not allow adding",
                        key_id
                    );
                }
            }
        }
        if self.objects[top_ptr].tag.is_map_add() {
            if self.objects[top_ptr].tag.is_vec2() {
                for kv in self.objects[top_ptr].vec.iter().rev() {
                    if kv.key == key {
                        return script_err_duplicate!(trap, "key {:?} already exists in vec", key);
                    }
                }
                if !self.charge_object_vec_entries(1, "adding a checked object vector property") {
                    return NIL;
                }
                let object = &mut self.objects[top_ptr];
                object.vec.push(ScriptVecValue { key, value });
                return NIL;
            }
            if self.objects[top_ptr].map_get(&key).is_some() {
                return script_err_duplicate!(trap, "key {:?} already exists in map", key);
            } else {
                if !self.charge_object_map_entry(
                    top_ptr,
                    key,
                    "adding a checked object property",
                ) {
                    return NIL;
                }
                let object = &mut self.objects[top_ptr];
                object.map_insert(key, value);
                return NIL;
            }
        }
        script_err_unexpected!(trap, "unexpected state in set_value_shallow_checked")
    }

    fn set_value_shallow(
        &mut self,
        ptr: ScriptObject,
        key: ScriptValue,
        value: ScriptValue,
        trap: ScriptTrap,
    ) -> ScriptValue {
        if self.is_allocation_poison_object(ptr) || self.objects[ptr].tag.is_immutable() {
            return script_err_immutable!(trap, "cannot set property on immutable object");
        }
        if self.objects[ptr].tag.is_vec2() {
            for kv in self.objects[ptr].vec.iter_mut().rev() {
                if kv.key == key {
                    kv.value = value;
                    return NIL;
                }
            }
            if !self.charge_object_vec_entries(1, "adding an object vector property") {
                return NIL;
            }
            let object = &mut self.objects[ptr];
            object.vec.push(ScriptVecValue { key, value });
            return NIL;
        }
        if !self.charge_object_map_entry(ptr, key, "adding an object property") {
            return NIL;
        }
        let object = &mut self.objects[ptr];
        object.map_insert(key, value);
        NIL
    }

    /// A loop variable bound into its iteration scope: `set_value_def`,
    /// with the plain-scope case (an id key into an untracked map-storage
    /// object with no type checks) taken without set_value's dispatch.
    #[inline]
    pub(crate) fn set_loop_binding(&mut self, ptr: ScriptObject, key: ScriptValue, value: ScriptValue) {
        let tag = &self.objects[ptr].tag;
        if key.as_id().is_none()
            || key.is_escaped_id()
            || tag.is_deep()
            || tag.needs_checking()
            || tag.is_string_keys()
            || tag.is_vec2()
            || tag.is_immutable()
        {
            self.set_value_def(ptr, key, value);
            return;
        }
        self.escape_value(value);
        self.bind_map_entry(ptr, key, value, "adding an object property");
    }

    #[inline]
    pub fn set_value_def(&mut self, ptr: ScriptObject, key: ScriptValue, value: ScriptValue) {
        self.set_value(ptr, key, value, NoTrap);
    }

    fn set_value_impl(
        &mut self,
        ptr: ScriptObject,
        key: ScriptValue,
        value: ScriptValue,
        trap: ScriptTrap,
    ) -> ScriptValue {
        self.escape_value(value);
        let key = key.unescape_id();
        if let Some(key_id) = key.as_id() {
            let object = &self.objects[ptr];
            if !object.tag.is_deep() {
                if object.tag.needs_checking() {
                    return self.set_value_shallow_checked(ptr, key, key_id, value, trap);
                }
                if object.tag.is_string_keys() {
                    if let Some(skey) = key_id.as_string(|s| {
                        if let Some(s) = s {
                            // Try to get existing interned string
                            if let Some(existing) = self.check_intern_string(s) {
                                Some(existing)
                            } else {
                                // Not interned yet - intern it now to maintain consistency
                                Some(self.new_string_from_str(s))
                            }
                        } else {
                            None
                        }
                    }) {
                        return self.set_value_shallow(ptr, skey, value, trap);
                    }
                    // LiveId couldn't be converted to string - fall through to use LiveId
                    // This happens for hashed IDs that lost their string representation
                }
                return self.set_value_shallow(ptr, key, value, trap);
            } else {
                return self.set_value_deep(ptr, key, value, trap);
            }
        }
        if key.is_index() {
            // use vector
            return self.set_value_index(ptr, key, value, trap);
        }
        if key.is_string_like() || key.is_object() || key.is_color() || key.is_bool() {
            // scan protochain for object
            let object = &mut self.objects[ptr];
            if !object.tag.is_deep() {
                if object.tag.needs_checking() {
                    return script_err_type_mismatch!(trap, "invalid key type for checked object");
                }
                return self.set_value_shallow(ptr, key, value, trap);
            } else {
                return self.set_value_deep(ptr, key, value, trap);
            }
        }
        script_err_type_mismatch!(trap, "unsupported key type for set_value")
    }

    // scope specific value get/set

    pub fn set_scope_value(
        &mut self,
        ptr: ScriptObject,
        key: LiveId,
        value: ScriptValue,
        trap: ScriptTrap,
    ) -> ScriptValue {
        self.escape_value(value);
        let root_ptr = ptr;
        let mut ptr = ptr;
        loop {
            let object = &mut self.objects[ptr];
            if let Some(set) = object.map.get_mut(&key.into()) {
                if object.tag.is_immutable() {
                    return script_err_immutable!(trap, "cannot modify immutable object");
                }
                set.value = value;
                return NIL;
            }
            if let Some(next_ptr) = object.proto.as_object() {
                ptr = next_ptr
            } else {
                break;
            }
        }
        // alright nothing found
        script_err_not_found!(
            trap,
            "variable {} not found in scope{}",
            key,
            suggest_scope_var(self, root_ptr, key)
        )
    }

    fn poke_scope_value_impl(&mut self, ptr: ScriptObject, key: ScriptValue, value: ScriptValue) -> bool {
        self.escape_value(value);
        let mut ptr = ptr;
        loop {
            let object = &mut self.objects[ptr];
            if let Some(set) = object.map.get_mut(&key) {
                set.value = value;
                return true;
            }
            match object.proto.as_object() {
                Some(next) => ptr = next,
                None => return false,
            }
        }
    }

    /// The construction sites (`Self::made_at`) in code body `body` that
    /// live objects were built at, each once, in order.
    #[inline(always)]
    pub fn made_at_sites(&self, body: u16) -> Vec<ScriptIp> {
        let mut out = Vec::new();
        self.made_at_sites_into(body, &mut ScriptIpsOut::new(&mut out));
        out
    }
    #[relax_api]
    pub fn made_at_sites_into(&self, body: u16, sink: &mut ScriptIpsOut) {
        let mut out: Vec<ScriptIp> = (0..self.objects.len())
            .map(|i| self.objects.get_at(i))
            .filter(|o| o.tag.is_alloced() && !o.made_at.is_unknown() && o.made_at.body == body)
            .map(|o| o.made_at)
            .collect();
        out.sort();
        out.dedup();
        for ip in &out { sink.push(ip); }
    }

    /// Set own field `key` to `value`, in place (even in a frozen object),
    /// in every live object built at `made_at` whose field holds `was` now:
    /// a host editing the running code's literals (an editor's drag on a
    /// record's field written as a literal), where the records that literal
    /// built already exist. A field that holds something else (set again
    /// since) is left. How many changed.
    #[relax_api]
    pub fn poke_made_at_field(&mut self, made_at: ScriptIp, key: LiveId, was: ScriptValue, value: ScriptValue) -> usize {
        self.escape_value(value);
        let key: ScriptValue = key.into();
        let mut changed = 0;
        for i in 0..self.objects.len() {
            let object = self.objects.get_at_mut(i);
            if !object.tag.is_alloced() || object.made_at != made_at {
                continue;
            }
            if let Some(set) = object.map.get_mut(&key) {
                if set.value == was {
                    set.value = value;
                    changed += 1;
                }
                continue;
            }
            for kv in object.vec.iter_mut() {
                if kv.key == key && kv.value == was {
                    kv.value = value;
                    changed += 1;
                }
            }
        }
        changed
    }

    /// Every field, entry and list item that holds `was` now: where a value
    /// was copied as data (`{sung: BONE}` holds BONE's colour, not BONE),
    /// for a host to set in place with [`Self::set_places`] (a running
    /// document recoloured as a colour is dragged, without a load). Plain
    /// values compare by their bits.
    #[inline(always)]
    pub fn places_holding(&self, was: ScriptValue) -> Vec<ScriptValuePlace> {
        let mut out = Vec::new();
        self.places_holding_into(was, &mut ScriptPlacesOut::new(&mut out));
        out
    }
    #[relax_api]
    pub fn places_holding_into(&self, was: ScriptValue, out: &mut ScriptPlacesOut) {
        for i in 0..self.objects.len() {
            let object = self.objects.get_at(i);
            if !object.tag.is_alloced() {
                continue;
            }
            for (key, set) in object.map.iter() {
                if set.value == was {
                    out.push(&ScriptValuePlace::Field { object: i, made_at: object.made_at, key: *key });
                }
            }
            for (slot, kv) in object.vec.iter().enumerate() {
                if kv.value == was {
                    out.push(&ScriptValuePlace::Entry { object: i, made_at: object.made_at, slot });
                }
            }
        }
        for i in 0..self.arrays.len() {
            let array = self.arrays.get_at(i);
            if !array.tag.is_alloced() {
                continue;
            }
            if let crate::array::ScriptArrayStorage::ScriptValue(items) = &array.storage {
                for (slot, item) in items.iter().enumerate() {
                    if *item == was {
                        out.push(&ScriptValuePlace::Item { array: i, slot });
                    }
                }
            }
        }
    }

    /// Set each of `places` ([`Self::places_holding`]) that still holds
    /// `was` (and is still the object built where it was) to `value`, in
    /// place, frozen objects too. A place that holds something else now
    /// (set again, or its slot reused) is left. How many changed.
    #[relax_api]
    pub fn set_places(&mut self, places: &[ScriptValuePlace], was: ScriptValue, value: ScriptValue) -> usize {
        self.escape_value(value);
        let mut changed = 0;
        for place in places {
            match place {
                ScriptValuePlace::Field { object, made_at, key } => {
                    if *object >= self.objects.len() {
                        continue;
                    }
                    let o = self.objects.get_at_mut(*object);
                    if !o.tag.is_alloced() || o.made_at != *made_at {
                        continue;
                    }
                    if let Some(set) = o.map.get_mut(key) {
                        if set.value == was {
                            set.value = value;
                            changed += 1;
                        }
                    }
                }
                ScriptValuePlace::Entry { object, made_at, slot } => {
                    if *object >= self.objects.len() {
                        continue;
                    }
                    let o = self.objects.get_at_mut(*object);
                    if !o.tag.is_alloced() || o.made_at != *made_at {
                        continue;
                    }
                    if let Some(kv) = o.vec.get_mut(*slot) {
                        if kv.value == was {
                            kv.value = value;
                            changed += 1;
                        }
                    }
                }
                ScriptValuePlace::Item { array, slot } => {
                    if *array >= self.arrays.len() {
                        continue;
                    }
                    let a = self.arrays.get_at_mut(*array);
                    if !a.tag.is_alloced() {
                        continue;
                    }
                    if let crate::array::ScriptArrayStorage::ScriptValue(items) = &mut a.storage {
                        if let Some(item) = items.get_mut(*slot) {
                            if *item == was {
                                *item = value;
                                changed += 1;
                            }
                        }
                    }
                }
            }
        }
        changed
    }

    /// A name's value as seen from scope `ptr`, or `None` when no scope in
    /// its chain has it (no error raised).
    pub fn scope_value_opt(&self, ptr: ScriptObject, key: LiveId) -> Option<ScriptValue> {
        let mut ptr = ptr;
        let key_sv: ScriptValue = key.into();
        loop {
            let object = &self.objects[ptr];
            if let Some(set) = object.map.get(&key_sv) {
                return Some(set.value);
            }
            if object.tag.is_vec2() {
                for kv in object.vec.iter().rev() {
                    if kv.key == key_sv {
                        return Some(kv.value);
                    }
                }
            }
            ptr = object.proto.as_object()?;
        }
    }

    fn rebind_scope_value_impl(&mut self, ptr: ScriptObject, key: ScriptValue, value: ScriptValue) -> bool {
        self.escape_value(value);
        let mut ptr = ptr;
        loop {
            let object = &mut self.objects[ptr];
            if let Some(set) = object.map.get_mut(&key) {
                set.value = value;
                return true;
            }
            match object.proto.as_object() {
                Some(next) => ptr = next,
                None => return false,
            }
        }
    }

    fn scope_value_impl(&self, ptr: ScriptObject, key_sv: ScriptValue, trap: ScriptTrap) -> ScriptValue {
        let root_ptr = ptr;
        let mut ptr = ptr;
        let key = key_sv.as_id().unwrap_or_default();
        loop {
            let object = &self.objects[ptr];
            if let Some(set) = object.map.get(&key_sv) {
                return set.value;
            }
            if object.tag.is_vec2() {
                for kv in object.vec.iter().rev() {
                    if kv.key == key_sv {
                        return kv.value;
                    }
                }
            }
            if let Some(next_ptr) = object.proto.as_object() {
                ptr = next_ptr
            } else {
                break;
            }
        }
        self.scope_value_not_found(root_ptr, key, trap)
    }

    #[cold]
    #[inline(never)]
    fn scope_value_not_found(&self, root_ptr: ScriptObject, key: LiveId, trap: ScriptTrap) -> ScriptValue {
        script_err_not_found!(
            trap,
            "variable {} not found in scope{}{}",
            key,
            suggest_scope_var(self, root_ptr, key),
            self.import_hint(key)
        )
    }

    /// Where a name that is not in scope lives among the system modules, as the `use` to add:
    /// a document's domain brings some modules in by default, any other one is a `use` away.
    /// Guidance only (an error path): it walks `mod` and one level below it.
    #[cold]
    pub fn import_hint(&self, key: LiveId) -> String {
        let key_value: ScriptValue = key.into();
        let modules = &self.objects[self.modules];
        let mut found = Vec::new();
        for (name, entry) in modules.map.iter() {
            let (Some(name), Some(module)) = (name.as_id(), entry.value.as_object()) else { continue };
            if !self.objects.is_valid(module) {
                continue;
            }
            let module_data = &self.objects[module];
            if module_data.map_get(&key_value).is_some() {
                found.push(format!("`use mod.{name}.{key}`"));
                continue;
            }
            for (sub, sub_entry) in module_data.map.iter() {
                let (Some(sub), Some(sub_obj)) = (sub.as_id(), sub_entry.value.as_object()) else { continue };
                if self.objects.is_valid(sub_obj) && self.objects[sub_obj].map_get(&key_value).is_some() {
                    found.push(format!("`use mod.{name}.{sub}.*`"));
                }
            }
            if found.len() >= 3 {
                break;
            }
        }
        if found.is_empty() {
            String::new()
        } else {
            let domain = self.lazy.domain.map_or(String::new(), |d| format!(" ({} get {} by default; anything else is a `use` away)", d.what, if d.default_uses.is_empty() { "no `use` lines".to_string() } else { d.default_uses.join(", ") }));
            format!(". It is a system module's: import it with {}{domain}", found.join(" or "))
        }
    }

    /// Single chain-walk locate for compound scope assignment (`x += v`).
    /// Returns the object whose MAP holds `key` plus the current value (the
    /// slot both the read and the write of a compound assign target).
    /// Returns Err(true) when the key is only visible through a vec2 vec —
    /// the caller must take the slow read+write path to reproduce its
    /// (read ok / write error) semantics. Err(false) means not found.
    #[inline]
    pub fn scope_map_owner(
        &self,
        ptr: ScriptObject,
        key: ScriptValue,
    ) -> Result<(ScriptObject, ScriptValue), bool> {
        let mut ptr = ptr;
        loop {
            let object = &self.objects[ptr];
            if let Some(set) = object.map.get(&key) {
                return Ok((ptr, set.value));
            }
            if object.tag.is_vec2() {
                for kv in object.vec.iter().rev() {
                    if kv.key == key {
                        return Err(true);
                    }
                }
            }
            if let Some(next_ptr) = object.proto.as_object() {
                ptr = next_ptr
            } else {
                return Err(false);
            }
        }
    }

    /// Write a compound-assign result into a slot located by
    /// [`Self::scope_map_owner`]. Mirrors the write half of
    /// `set_scope_value` (escape barrier, immutable check, in-place store).
    #[inline]
    pub fn scope_write_in_map(
        &mut self,
        owner: ScriptObject,
        key: ScriptValue,
        value: ScriptValue,
        trap: ScriptTrap,
    ) -> ScriptValue {
        self.escape_value(value);
        let object = &mut self.objects[owner];
        if object.tag.is_immutable() {
            return script_err_immutable!(trap, "cannot modify immutable object");
        }
        if let Some(set) = object.map.get_mut(&key) {
            set.value = value;
        }
        NIL
    }

    pub fn def_scope_value(
        &mut self,
        ptr: ScriptObject,
        key: LiveId,
        value: ScriptValue,
    ) -> Option<ScriptObject> {
        self.escape_value(value);
        // if we already have sself value we have to shadow the scope
        let key: ScriptValue = key.into();
        if self.objects[ptr].map.contains_key(&key) {
            let new_scope = self.new_with_proto(ptr.into());
            if !self.charge_object_map_entry(new_scope, key, "defining a scope value") {
                return Some(new_scope);
            }
            let object = &mut self.objects[new_scope];
            object.map.insert(
                key,
                ScriptMapValue {
                    tag: Default::default(),
                    value,
                },
            );
            return Some(new_scope);
        } else {
            if !self.charge_object_map_entry(ptr, key, "defining a scope value") {
                return None;
            }
            let object = &mut self.objects[ptr];
            object.map.insert(
                key,
                ScriptMapValue {
                    tag: Default::default(),
                    value,
                },
            );
            return None;
        }
    }

    // Reading object values

    fn value_index(&self, ptr: ScriptObject, index: ScriptValue, trap: ScriptTrap) -> ScriptValue {
        let object = &self.objects[ptr];
        // most used path
        let index = index.as_index();
        if let Some(kv) = object.vec.get(index) {
            return kv.value;
        }
        script_err_not_found!(
            trap,
            "index {} out of bounds (len={})",
            index,
            object.vec.len()
        )
    }

    fn value_deep_map(
        &self,
        obj_ptr: ScriptObject,
        key: ScriptValue,
        trap: ScriptTrap,
    ) -> ScriptValue {
        let mut ptr = obj_ptr;
        loop {
            let object = &self.objects[ptr];
            if let Some(value) = object.map_get(&key) {
                return value;
            }
            if let Some(next_ptr) = object.proto.as_object() {
                ptr = next_ptr
            } else {
                break;
            }
        }
        if self.lazy.miss(obj_ptr, self.modules, key) {
            return NIL;
        }
        script_err_not_found!(
            trap,
            "key {:?} not found in prototype chain{}",
            key,
            suggest_property(self, obj_ptr, key)
        )
    }

    fn value_deep(&self, obj_ptr: ScriptObject, key: ScriptValue, trap: ScriptTrap) -> ScriptValue {
        let mut ptr = obj_ptr;
        loop {
            let object = &self.objects[ptr];
            if let Some(value) = object.map_get(&key) {
                return value;
            }
            // handle auto conversion from string to id and back for json interop
            if object.tag.is_string_keys() {
                if let Some(id) = key.as_id() {
                    if let Some(value) = id.as_string(|s| {
                        if let Some(s) = s {
                            if let Some(idx) = self.check_intern_string(s) {
                                object.map_get(&idx)
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    }) {
                        return value;
                    }
                } else if key.is_string_like() {
                    // string key on string_keys object: resolve via intern to canonical ScriptValue
                    if let Some(Some(value)) = self.string_with(key, |heap, s| {
                        if let Some(idx) = heap.check_intern_string(s) {
                            if idx != key {
                                object.map_get(&idx)
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    }) {
                        return value;
                    }
                }
            } else if key.is_string_like() {
                let id = if let Some(s) = key.as_string() {
                    if let Some(s) = &self.strings[s] {
                        LiveId::from_str(&s.string.0)
                    } else {
                        LiveId(0)
                    }
                } else {
                    key.as_inline_string(|s| LiveId::from_str(s)).unwrap()
                };
                if let Some(value) = object.map_get(&id.into()) {
                    return value;
                }
            }
            for kv in object.vec.iter().rev() {
                if kv.key == key {
                    return kv.value;
                }
            }
            if let Some(next_ptr) = object.proto.as_object() {
                ptr = next_ptr;
            } else {
                break;
            }
        }
        if self.lazy.miss(obj_ptr, self.modules, key) {
            return NIL;
        }
        if self.objects[obj_ptr].tag.is_api() {
            return script_err_not_found!(
                trap,
                "no field `{}`{}",
                key.as_id().and_then(|id| id.as_string(|s| s.map(|s| s.to_string()))).unwrap_or_else(|| format!("{:?}", key)),
                api_own_keys(self, obj_ptr)
            );
        }
        script_err_not_found!(
            trap,
            "property {:?} not found in prototype chain{}",
            key,
            suggest_property(self, obj_ptr, key)
        )
    }

    #[inline]
    pub fn object_method(
        &self,
        ptr: ScriptObject,
        key: ScriptValue,
        trap: ScriptTrap,
    ) -> ScriptValue {
        self.obj_get(OBJ_GET_METHOD, ptr, key, trap).value
    }

    pub fn value_path(&self, ptr: ScriptObject, keys: &[LiveId], trap: ScriptTrap) -> ScriptValue {
        let mut value: ScriptValue = ptr.into();
        for key in keys {
            if let Some(obj) = value.as_object() {
                value = self.value(obj, (*key).into(), trap);
            } else {
                return script_err_not_found!(
                    trap,
                    "path segment {:?} requires object but got {:?}",
                    key,
                    value.value_type()
                );
            }
        }
        value
    }

    fn value_impl(&self, ptr: ScriptObject, key: ScriptValue, trap: ScriptTrap) -> ScriptValue {
        let key = key.unescape_id();
        if key.is_id() {
            return self.value_deep(ptr, key, trap);
        }
        if key.is_index() {
            return self.value_index(ptr, key, trap);
        }
        if key.is_string_like() || key.is_object() || key.is_color() || key.is_bool() {
            // scan protochain for object
            return self.value_deep(ptr, key, trap);
        }
        // TODO implement string lookup
        script_err_not_found!(
            trap,
            "unsupported key type {:?} for value lookup",
            key.value_type()
        )
    }

    /// Create a default instance for a type-checked field that doesn't exist on the prototype.
    /// This is used for deep prototypical inheritance - when accessing obj.field where field
    /// only exists in the type-check structure, we create a new instance and set it on obj.
    pub fn proto_field_from_type_check(
        &mut self,
        obj: ScriptObject,
        field_id: LiveId,
        trap: ScriptTrap,
    ) -> ScriptValue {
        // Get the field's type_id from the type-check structure
        if let Some(field_type_id) = self.field_type_from_type_check(obj, field_id) {
            // Look up the type_default for this type
            if let Some(default_obj) = self.type_default_for_id(field_type_id) {
                // Create a new object with the default as prototype
                let new_obj = self.new_with_proto(default_obj.into());
                // Set it on the parent object
                self.set_value(obj, field_id.into(), new_obj.into(), trap);
                return new_obj.into();
            }
        }
        script_err_not_found!(
            trap,
            "{}",
            slot_not_found_message(self, obj, field_id)
        )
    }

    /// Handle proto_field access for a value that exists on the prototype chain.
    /// If the value is an object that comes from a prototype (not directly on obj),
    /// create a new object with it as prototype and set it on obj.
    pub fn proto_field_from_value(
        &mut self,
        obj: ScriptObject,
        field: ScriptValue,
        trap: ScriptTrap,
    ) -> ScriptValue {
        // First check if the field exists directly on this object
        let obj_data = &self.objects[obj];
        if let Some(value) = obj_data.map_get(&field) {
            // Field exists directly on object, return as-is
            return value;
        }
        // Handle is_string_keys: convert LiveId to string key
        if obj_data.tag.is_string_keys() {
            if let Some(id) = field.as_id() {
                if let Some(value) = id.as_string(|s| {
                    if let Some(s) = s {
                        if let Some(idx) = self.check_intern_string(s) {
                            self.objects[obj].map_get(&idx)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }) {
                    return value;
                }
            }
        }

        // Field doesn't exist directly - get from prototype chain. Both callers drop a miss and
        // fall back to proto_field_from_type_check for id fields, so a miss never reaches `trap`.
        let value = self.value(obj, field, NoTrap);

        // If it's an object from prototype, create a new instance
        if let Some(value_obj) = value.as_object() {
            // Create a new object with the prototype value as its proto
            // Note: new_with_proto automatically copies type_index from prototype chain
            let new_obj = self.new_with_proto(value_obj.into());
            // Set it on the current object, where the name lives (a named child keeps its vec position)
            self.set_value_slot(obj, field, new_obj.into(), trap);
            return new_obj.into();
        }

        // Not an object (primitive or nil) - return as-is
        value
    }

    /// The fields of one derived struct apply, resolved in one pass: `obj`'s own map, then (not
    /// for `eval_only`) each prototype in turn, each object visited once. `fields` is sorted by id,
    /// `.1` the slot in `out` (declaration order). A field takes the first value found along the
    /// chain (value_for_apply's precedence); a field found nowhere keeps `ScriptValue::NOT_FOUND`,
    /// which every slot starts as. A map with more entries than fields still open is probed by id
    /// instead of walked, so a small apply block on a big struct walks the block.
    pub fn resolve_fields(&self, obj: ScriptValue, eval_only: bool, fields: &[(LiveId, u32)], out: &mut [ScriptValue]) {
        for v in out.iter_mut() {
            *v = ScriptValue::NOT_FOUND;
        }
        let Some(mut ptr) = obj.as_object() else { return };
        let mut open = fields.len();
        loop {
            let object = &self.objects[ptr];
            if object.map.len() <= open {
                for (key, entry) in object.map.iter() {
                    let Some(id) = key.as_id() else { continue };
                    if let Ok(i) = fields.binary_search_by(|f| f.0.cmp(&id)) {
                        let slot = &mut out[fields[i].1 as usize];
                        if *slot == ScriptValue::NOT_FOUND && ScriptValue::from_id(id) == *key {
                            *slot = entry.value;
                            open -= 1;
                        }
                    }
                }
            } else {
                for (id, slot) in fields {
                    let slot = &mut out[*slot as usize];
                    if *slot == ScriptValue::NOT_FOUND {
                        if let Some(v) = object.map_get(&ScriptValue::from_id(*id)) {
                            *slot = v;
                            open -= 1;
                        }
                    }
                }
            }
            if open == 0 || eval_only {
                return;
            }
            match object.proto.as_object() {
                Some(next) => ptr = next,
                None => return,
            }
        }
    }

    fn value_for_apply_impl(
        &self,
        ptr: ScriptObject,
        key: ScriptValue,
        eval: bool,
    ) -> Option<ScriptValue> {
        // only do top level if dirty
        let object = &self.objects[ptr];
        if let Some(value) = object.map_get(&key) {
            return Some(value);
        }
        // For eval, only check the object's own map (no prototype chain)
        if eval {
            return None;
        }
        // if we havent been applied before apply prototype chain too
        let mut ptr = object.proto.as_object()?;
        loop {
            let object = &self.objects[ptr];
            if let Some(value) = object.map_get(&key) {
                return Some(value);
            }
            ptr = object.proto.as_object()?;
        }
    }

    #[inline]
    pub(crate) fn map_ref(&self, object: ScriptObject) -> &ScriptObjectMap {
        let object = &self.objects[object];
        &object.map
    }

    /// `object`'s own map entries (no prototype chain) in map order, appended to `out`;
    /// `ordered`: in definition order (the map tag's order) instead.
    #[relax_api]
    pub fn map_entries_into(&self, object: ScriptObject, ordered: bool, out: &mut ScriptEntriesOut) {
        let map = &self.objects[object].map;
        let mut entries: Vec<(u32, ScriptMapEntry)> =
            map.iter().map(|(k, v)| (v.tag.order(), ScriptMapEntry { key: *k, value: v.value })).collect();
        if ordered {
            entries.sort_by_key(|(order, _)| *order);
        }
        let entries: Vec<ScriptMapEntry> = entries.into_iter().map(|(_, e)| e).collect();
        out.push(&entries);
    }

    /// `object`'s own map entries in map order.
    #[inline(always)]
    pub fn map_entries(&self, object: ScriptObject) -> Vec<ScriptMapEntry> {
        let mut out = Vec::new();
        self.map_entries_into(object, false, &mut ScriptEntriesOut::new(&mut out));
        out
    }

    /// `object`'s own map entries in definition order.
    #[inline(always)]
    pub fn map_entries_ordered(&self, object: ScriptObject) -> Vec<ScriptMapEntry> {
        let mut out = Vec::new();
        self.map_entries_into(object, true, &mut ScriptEntriesOut::new(&mut out));
        out
    }

    /// Visits `object`'s own map entries in definition order.
    #[inline]
    pub fn object_map_iter_ordered<F: FnMut(ScriptValue, ScriptValue)>(&self, object: ScriptObject, mut f: F) {
        for entry in self.map_entries_ordered(object) {
            f(entry.key, entry.value);
        }
    }

    /// `object`'s own map entry for `key` (no prototype chain, no vec).
    #[inline]
    pub fn object_own_value(&self, object: ScriptObject, key: &ScriptValue) -> Option<ScriptValue> {
        let got = self.obj_get(OBJ_GET_MAP_OWN, object, *key, NoTrap);
        got.found.then_some(got.value)
    }

    /// The map entries of `object`'s prototype chain from the root (oldest ancestor) to
    /// `object` itself, each object's in map order: a later entry overrides an earlier one.
    pub fn proto_map_entries_into(&self, object: ScriptObject, out: &mut ScriptEntriesOut) {
        let mut chain = vec![object];
        while let Some(proto) = self.obj_info(*chain.last().unwrap()).proto.as_object() {
            chain.push(proto);
        }
        for object in chain.into_iter().rev() {
            self.map_entries_into(object, false, out);
        }
    }

    /// See [`Self::proto_map_entries_into`].
    #[inline(always)]
    pub fn proto_map_entries(&self, object: ScriptObject) -> Vec<ScriptMapEntry> {
        let mut out = Vec::new();
        self.proto_map_entries_into(object, &mut ScriptEntriesOut::new(&mut out));
        out
    }

    /// `object`'s own map value at `key` (no prototype chain) into `out`; false when absent.
    pub fn map_own_value_into(&self, object: ScriptObject, key: ScriptValue, out: &mut ScriptValue) -> bool {
        let got = self.obj_get(OBJ_GET_MAP_OWN, object, key, NoTrap);
        if got.found {
            *out = got.value;
        }
        got.found
    }

    /// `object`'s own map value at `key` (no prototype chain); None when absent.
    #[inline(always)]
    pub fn map_own_value(&self, object: ScriptObject, key: ScriptValue) -> Option<ScriptValue> {
        let mut out = NIL;
        self.map_own_value_into(object, key, &mut out).then_some(out)
    }

    /// Inserts `key: value` into `object`'s own map as is: no prototype, type or freeze checks.
    #[inline]
    pub fn map_insert_own(&mut self, object: ScriptObject, key: ScriptValue, value: ScriptValue) {
        self.obj_set(OBJ_SET_MAP_OWN, object, key, value, NoTrap);
    }

    /// Whether `object` is frozen immutable.
    #[inline]
    pub fn is_immutable(&self, object: ScriptObject) -> bool {
        self.obj_info(object).tag.is_immutable()
    }

    // Vec Reading

    #[inline]
    pub fn vec_key_value(
        &self,
        ptr: ScriptObject,
        index: usize,
        trap: ScriptTrap,
    ) -> ScriptVecValue {
        let vec = self.vec_ref(ptr);
        if let Some(value) = vec.get(index) {
            return *value;
        }
        ScriptVecValue {
            key: NIL,
            value: script_err_out_of_bounds!(
                trap,
                "vec index {} out of bounds (len={})",
                index,
                vec.len()
            ),
        }
    }

    #[inline]
    pub fn vec_value(&self, ptr: ScriptObject, index: usize, trap: ScriptTrap) -> ScriptValue {
        let vec = self.vec_ref(ptr);
        if let Some(kv) = vec.get(index) {
            return kv.value;
        }
        script_err_out_of_bounds!(
            trap,
            "vec index {} out of bounds (len={})",
            index,
            vec.len()
        )
    }

    #[inline]
    pub fn vec_value_if_exist(&self, ptr: ScriptObject, index: usize) -> Option<ScriptValue> {
        self.vec_ref(ptr).get(index).map(|kv| kv.value)
    }

    #[inline]
    pub fn vec_len(&self, ptr: ScriptObject) -> usize {
        self.vec_ref(ptr).len()
    }

    #[inline]
    pub fn map_delete(&mut self, ptr: ScriptObject, key: &ScriptValue) -> Option<ScriptValue> {
        let object = &mut self.objects[ptr];
        object.map_delete(&key.unescape_id())
    }

    #[inline]
    pub fn map_len(&self, ptr: ScriptObject) -> usize {
        self.obj_info(ptr).map_len
    }

    /// Total number of iterable entries: vec entries + map entries.
    /// Used by `for k, v in obj` to iterate all entries.
    #[inline]
    pub fn iter_len(&self, ptr: ScriptObject) -> usize {
        let info = self.obj_info(ptr);
        info.vec_len + info.map_len
    }

    /// Get key-value at an iteration index spanning both vec and map.
    /// Indices 0..vec_len come from vec, indices vec_len..vec_len+map_len come from map.
    #[inline]
    pub fn iter_key_value(
        &self,
        ptr: ScriptObject,
        index: usize,
        trap: ScriptTrap,
    ) -> ScriptVecValue {
        let at = self.obj_iter_at(ptr, index);
        if at.found {
            return ScriptVecValue { key: at.key, value: at.value };
        }
        ScriptVecValue {
            key: NIL,
            value: script_err_out_of_bounds!(
                trap,
                "iter index {} out of bounds (vec_len={}, map_len={})",
                index,
                at.vec_len,
                at.map_len
            ),
        }
    }

    /// The vec part of `ptr`, a view into the heap.
    #[inline]
    #[relax_api]
    pub fn vec_ref(&self, ptr: ScriptObject) -> &[ScriptVecValue] {
        let object = &self.objects[ptr];
        &object.vec
    }

    // Vec Writing

    pub fn vec_insert_value_at(
        &mut self,
        _ptr: ScriptObject,
        _key: ScriptValue,
        _value: ScriptValue,
        _before: bool,
        _ip: ScriptTrap,
    ) -> ScriptValue {
        NIL
    }

    pub fn vec_insert_value_begin(
        &mut self,
        _ptr: ScriptObject,
        _key: ScriptValue,
        _value: ScriptValue,
        _ip: ScriptTrap,
    ) -> ScriptValue {
        NIL
    }

    #[inline]
    pub fn vec_push_vec(&mut self, target: ScriptObject, source: ScriptObject, trap: ScriptTrap) -> ScriptValue {
        self.obj_set(OBJ_SET_VEC_PUSH_VEC, target, NIL, source.into(), trap)
    }

    fn vec_push_vec_impl(
        &mut self,
        target: ScriptObject,
        source: ScriptObject,
        trap: ScriptTrap,
    ) -> ScriptValue {
        if target == source {
            return script_err_invalid_args!(
                trap,
                "cannot push object vec onto itself (object gc_id={})",
                target.index
            );
        }
        if self.is_allocation_poison_object(target) || self.objects[target].tag.is_vec_frozen() {
            return script_err_immutable!(trap, "cannot push to frozen vec");
        }
        let source_len = self.objects[source].vec.len();
        if !self.charge_object_vec_entries(source_len, "extending an object vector") {
            return NIL;
        }
        let (target_obj, source_obj) = if target.index > source.index {
            let (o1, o2) = self.objects.slots_split_at_mut(target.index as usize);
            (&mut o2[0].data, &mut o1[source.index as usize].data)
        } else {
            let (o1, o2) = self.objects.slots_split_at_mut(source.index as usize);
            (&mut o1[target.index as usize].data, &mut o2[0].data)
        };
        target_obj.push_vec_from_other(source_obj);
        NIL
    }

    pub fn vec_push_vec_of_vec(
        &mut self,
        target: ScriptObject,
        source: ScriptObject,
        map: bool,
        trap: ScriptTrap,
    ) -> ScriptValue {
        let len = self.objects[source].vec.len();
        for i in 0..len {
            if let Some(source) = self.objects[source].vec[i].value.as_object() {
                if target == source {
                    return script_err_invalid_args!(
                        trap,
                        "cannot push object vec onto itself in nested push (object gc_id={})",
                        target.index
                    );
                }
                if self.is_allocation_poison_object(target)
                    || self.objects[target].tag.is_vec_frozen()
                {
                    return script_err_immutable!(
                        trap,
                        "cannot push to frozen vec in nested push"
                    );
                }
                let vec_entries = self.objects[source].vec.len();
                let map_entries = if map {
                    self.objects[source].map.len()
                } else {
                    0
                };
                let vec_bytes = vec_entries
                    .checked_mul(std::mem::size_of::<ScriptVecValue>())
                    .unwrap_or(usize::MAX);
                let map_bytes = map_entries
                    .checked_mul(
                        std::mem::size_of::<(ScriptValue, ScriptMapValue)>()
                            .saturating_add(2 * std::mem::size_of::<usize>()),
                    )
                    .unwrap_or(usize::MAX);
                if !self.charge_allocation(
                    vec_bytes.checked_add(map_bytes).unwrap_or(usize::MAX),
                    "merging nested object containers",
                ) {
                    return NIL;
                }
                let (target_obj, source_obj) = if target.index > source.index {
                    let (o1, o2) = self.objects.slots_split_at_mut(target.index as usize);
                    (&mut o2[0].data, &mut o1[source.index as usize].data)
                } else {
                    let (o1, o2) = self.objects.slots_split_at_mut(source.index as usize);
                    (&mut o1[target.index as usize].data, &mut o2[0].data)
                };
                target_obj.push_vec_from_other(source_obj);
                if map {
                    target_obj.merge_map_from_other(source_obj);
                }
            }
        }
        NIL
    }

    /// Merges the vec and map parts of a source object into a target object.
    /// Used by the splat operator (..) to spread one object into another.
    /// Map entries from source are only added if they don't already exist in target.
    #[inline]
    pub fn merge_object(&mut self, target: ScriptObject, source: ScriptObject, trap: ScriptTrap) -> ScriptValue {
        self.obj_set(OBJ_SET_MERGE, target, NIL, source.into(), trap)
    }

    fn merge_object_impl(
        &mut self,
        target: ScriptObject,
        source: ScriptObject,
        trap: ScriptTrap,
    ) -> ScriptValue {
        if target == source {
            return script_err_invalid_args!(
                trap,
                "cannot merge object into itself (object gc_id={})",
                target.index
            );
        }
        if self.is_allocation_poison_object(target) || self.objects[target].tag.is_immutable() {
            return script_err_immutable!(trap, "cannot merge into immutable object");
        }
        let vec_entries = if self.objects[target].tag.is_vec_frozen() {
            0
        } else {
            self.objects[source].vec.len()
        };
        let missing_map_entries = self.objects[source]
            .map
            .iter()
            .filter(|(key, _)| !self.objects[target].map.contains_key(key))
            .count();
        let temp_bytes = self.objects[source]
            .map
            .len()
            .checked_mul(std::mem::size_of::<ScriptValue>())
            .unwrap_or(usize::MAX);
        let vec_bytes = vec_entries
            .checked_mul(std::mem::size_of::<ScriptVecValue>())
            .unwrap_or(usize::MAX);
        let map_bytes = missing_map_entries
            .checked_mul(
                std::mem::size_of::<(ScriptValue, ScriptMapValue)>()
                    .saturating_add(2 * std::mem::size_of::<usize>()),
            )
            .unwrap_or(usize::MAX);
        let bytes = temp_bytes
            .checked_add(vec_bytes)
            .and_then(|bytes| bytes.checked_add(map_bytes))
            .unwrap_or(usize::MAX);
        if !self.charge_allocation(bytes, "merging object containers") {
            return NIL;
        }
        // escape barrier: source's values become reachable from target
        let vec_len = self.objects[source].vec.len();
        for i in 0..vec_len {
            let v = self.objects[source].vec[i].value;
            self.escape_value(v);
        }
        let map_vals: Vec<ScriptValue> = self.objects[source]
            .map
            .iter()
            .map(|(_, v)| v.value)
            .collect();
        for v in map_vals {
            self.escape_value(v);
        }
        let (target_obj, source_obj) = if target.index > source.index {
            let (o1, o2) = self.objects.slots_split_at_mut(target.index as usize);
            (&mut o2[0].data, &mut o1[source.index as usize].data)
        } else {
            let (o1, o2) = self.objects.slots_split_at_mut(source.index as usize);
            (&mut o1[target.index as usize].data, &mut o2[0].data)
        };
        if !target_obj.tag.is_vec_frozen() {
            target_obj.push_vec_from_other(source_obj);
        }
        // Only add map entries that don't already exist in target
        target_obj.merge_map_from_other_no_overwrite(source_obj);
        NIL
    }

    #[inline]
    pub fn vec_push(
        &mut self,
        ptr: ScriptObject,
        key: ScriptValue,
        value: ScriptValue,
        trap: ScriptTrap,
    ) -> ScriptValue {
        self.obj_set(OBJ_SET_VEC_PUSH, ptr, key, value, trap)
    }

    fn vec_push_impl(
        &mut self,
        ptr: ScriptObject,
        key: ScriptValue,
        value: ScriptValue,
        trap: ScriptTrap,
    ) -> ScriptValue {
        if self.is_allocation_poison_object(ptr) || self.objects[ptr].tag.is_vec_frozen() {
            return script_err_immutable!(trap, "cannot push to frozen vec");
        }
        if !self.charge_object_vec_entries(1, "pushing an object vector value") {
            return NIL;
        }
        self.escape_value(value);
        let object = &mut self.objects[ptr];
        object.vec.push(ScriptVecValue { key, value });
        NIL
    }

    #[inline]
    pub fn vec_push_unchecked(&mut self, ptr: ScriptObject, key: ScriptValue, value: ScriptValue) {
        self.obj_set(OBJ_SET_VEC_PUSH_UNCHECKED, ptr, key, value, NoTrap);
    }

    fn vec_push_unchecked_impl(&mut self, ptr: ScriptObject, key: ScriptValue, value: ScriptValue) {
        if self.is_allocation_poison_object(ptr) || self.allocation_exceeded() {
            return;
        }
        if !self.charge_object_vec_entries(1, "pushing an object vector value") {
            return;
        }
        let object = &mut self.objects[ptr];
        object.vec.push(ScriptVecValue { key, value });
    }

    pub fn vec_remove(
        &mut self,
        ptr: ScriptObject,
        index: usize,
        trap: ScriptTrap,
    ) -> ScriptVecValue {
        let object = &mut self.objects[ptr];
        if object.tag.is_vec_frozen() {
            return ScriptVecValue {
                key: NIL,
                value: script_err_immutable!(trap, "cannot remove from frozen vec"),
            };
        }
        if index >= object.vec.len() {
            return ScriptVecValue {
                key: NIL,
                value: script_err_out_of_bounds!(
                    trap,
                    "vec remove index {} out of bounds (len={})",
                    index,
                    object.vec.len()
                ),
            };
        }
        object.vec.remove(index)
    }

    pub fn vec_pop(&mut self, ptr: ScriptObject, trap: ScriptTrap) -> ScriptVecValue {
        let object = &mut self.objects[ptr];
        if object.tag.is_vec_frozen() {
            return ScriptVecValue {
                key: NIL,
                value: script_err_immutable!(trap, "cannot pop from frozen vec"),
            };
        }
        object.vec.pop().unwrap_or_else(|| ScriptVecValue {
            key: NIL,
            value: script_err_out_of_bounds!(trap, "cannot pop from empty vec"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capped_map_insert_is_refused_before_mutation() {
        let mut heap = ScriptHeap::empty();
        let object = heap.new_object();
        heap.set_max_heap_bytes(Some(usize::MAX));
        let baseline = heap.accounted_heap_bytes();
        heap.set_max_heap_bytes(Some(baseline + 1));

        let trap = ScriptTrap::NoTrap;
        let result = heap.set_value(object, id!(entry).into(), NIL, trap.pass());
        assert!(result.is_nil(), "{result:?}");
        assert!(heap.take_heap_limit_exceeded());
        assert!(heap.objects[object].map.is_empty());

        // Headroom re-opens after the host takes the flag.
        heap.set_max_heap_bytes(Some(baseline + 64 * 1024));
        let result = heap.set_value(object, id!(entry).into(), NIL, trap.pass());
        assert!(result.is_nil(), "{result:?}");
        assert!(!heap.take_heap_limit_exceeded());
        assert_eq!(heap.objects[object].map.len(), 1);
    }

    #[test]
    fn capped_sparse_vec_index_is_refused_before_mutation() {
        let mut heap = ScriptHeap::empty();
        let object = heap.new_object();
        heap.objects[object].tag.set_vec2();
        heap.set_max_heap_bytes(Some(usize::MAX));
        let baseline = heap.accounted_heap_bytes();
        heap.set_max_heap_bytes(Some(baseline + 1024));

        let trap = ScriptTrap::NoTrap;
        let result = heap.set_value(object, ScriptValue::from_f64(1_000_000.0), NIL, trap.pass());
        assert!(result.is_nil(), "{result:?}");
        assert!(heap.take_heap_limit_exceeded());
        assert!(heap.objects[object].vec.is_empty());
    }
}

/// A place in the heap a value is held: an object's field (by key) or
/// entry (by slot), or a list's item. See [`ScriptHeap::places_holding`].
#[relax_api(plain)]
#[derive(Clone, Debug)]
#[repr(C, u8)]
pub enum ScriptValuePlace {
    Field { object: usize, made_at: ScriptIp, key: ScriptValue },
    Entry { object: usize, made_at: ScriptIp, slot: usize },
    Item { array: usize, slot: usize },
}

impl ScriptValuePlace {
    pub fn field_key(&self) -> Option<ScriptValue> { match self { Self::Field { key, .. } => Some(*key), _ => None } }
}

/// Synchronous output into a caller-owned vector; opaque elements are cloned natively.
#[relax_api(plain)]
#[repr(C)]
pub struct ScriptPlacesOut<'a> {
    ctx: *mut u8,
    push: unsafe extern "C-unwind" fn(*mut u8, *const ScriptValuePlace),
    _out: std::marker::PhantomData<&'a mut Vec<ScriptValuePlace>>,
}
impl<'a> ScriptPlacesOut<'a> {
    #[inline(always)]
    pub fn new(out: &'a mut Vec<ScriptValuePlace>) -> Self {
        unsafe extern "C-unwind" fn push(ctx: *mut u8, value: *const ScriptValuePlace) {
            (&mut *(ctx as *mut Vec<ScriptValuePlace>)).push((&*value).clone());
        }
        Self { ctx: out as *mut _ as *mut u8, push, _out: std::marker::PhantomData }
    }
    pub fn push(&mut self, value: &ScriptValuePlace) {
        unsafe { (self.push)(self.ctx, value) }
    }
}

/// Synchronous output into a caller-owned vector; opaque elements are cloned natively.
#[relax_api(plain)]
#[repr(C)]
pub struct ScriptIpsOut<'a> {
    ctx: *mut u8,
    push: unsafe extern "C-unwind" fn(*mut u8, *const ScriptIp),
    _out: std::marker::PhantomData<&'a mut Vec<ScriptIp>>,
}
impl<'a> ScriptIpsOut<'a> {
    #[inline(always)]
    pub fn new(out: &'a mut Vec<ScriptIp>) -> Self {
        unsafe extern "C-unwind" fn push(ctx: *mut u8, value: *const ScriptIp) {
            (&mut *(ctx as *mut Vec<ScriptIp>)).push((&*value).clone());
        }
        Self { ctx: out as *mut _ as *mut u8, push, _out: std::marker::PhantomData }
    }
    pub fn push(&mut self, value: &ScriptIp) {
        unsafe { (self.push)(self.ctx, value) }
    }
}
