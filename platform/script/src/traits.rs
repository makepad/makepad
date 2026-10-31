use makepad_relax_api::relax_api;
use crate::apply::*;
use crate::heap::*;
use crate::value::*;
use crate::vm::*;
use makepad_live_id::*;

// ============================================================================
// Script traits
// ============================================================================

pub trait ScriptDeriveMarker {}

/// A script type's identity: a 128-bit FNV-1a hash of its canonical path (and, for generic
/// types, its arguments' ids). Every compiler computes the same value for the same path, unlike
/// `std::any::TypeId`, so ids made by app code built by Rapid and by the VM built into the
/// makepad loader by rustc agree. The VM's own types and the derive use fixed paths
/// (`module_path!()` + name); a hand-written `ScriptNew` impl without
/// `script_type_id_static` gets the id of `core::any::type_name::<Self>()` (such types exist
/// only in the code of the compiler that names them). Registration checks for collisions
/// (ScriptHeap::register_type).
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ScriptTypeId {
    lo: u64,
    hi: u64,
}

const FNV128_OFFSET: u128 = 0x6c62272e07bb014262b821756295c58d;
const FNV128_PRIME: u128 = 0x0000000001000000000000000000013B;

impl ScriptTypeId {
    pub(crate) const fn from_u128(h: u128) -> Self {
        Self { lo: h as u64, hi: (h >> 64) as u64 }
    }

    const fn to_u128(self) -> u128 {
        (self.lo as u128) | ((self.hi as u128) << 64)
    }

    /// The id of the type at canonical path `path` (`"makepad_widgets_core::view::View"`,
    /// `"f32"`).
    pub const fn of_path(path: &str) -> Self {
        let b = path.as_bytes();
        let mut h = FNV128_OFFSET;
        let mut i = 0;
        while i < b.len() {
            h ^= b[i] as u128;
            h = h.wrapping_mul(FNV128_PRIME);
            i += 1;
        }
        Self::from_u128(h)
    }

    /// The id of this generic type applied to `args` (`Vec` + `[f32]`).
    pub const fn with_args(self, args: &[ScriptTypeId]) -> Self {
        let mut h = self.to_u128();
        // a separator no path byte produces
        h ^= 0x100;
        h = h.wrapping_mul(FNV128_PRIME);
        let mut i = 0;
        while i < args.len() {
            let a = args[i].to_u128();
            let mut k = 0;
            while k < 16 {
                h ^= (a >> (k * 8)) & 0xff;
                h = h.wrapping_mul(FNV128_PRIME);
                k += 1;
            }
            i += 1;
        }
        Self::from_u128(h)
    }

    /// The id of a type without a fixed path (hand-written impls): its name as the compiler
    /// of this code prints it.
    pub fn of_type_name<T: ?Sized>() -> Self {
        Self::of_path(core::any::type_name::<T>())
    }
}

impl std::fmt::Debug for ScriptTypeId {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "ScriptTypeId({:016x}{:016x})", self.hi, self.lo)
    }
}

// sself we implement
pub trait ScriptHook {
    // these are the root entrypoints, and they by default dispatch to simpler lifecycle points
    fn on_before_apply(
        &mut self,
        _vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
    }

    fn on_before_dispatch(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        _value: ScriptValue,
    ) {
        match apply {
            Apply::New => self.on_before_new_scoped(vm, scope),
            // Both LiveEdit (Reload) and request_script_reapply (ScriptReapply)
            // fire the reload hooks — `apply.is_reload()` returns true for
            // both, and widgets that branch on it expect the broader semantic.
            // Widgets that need to differentiate can branch on
            // `apply.is_live_edit_reload()` or `apply.is_script_reapply()`
            // inside the hook.
            Apply::Reload | Apply::Rebake | Apply::ScriptReapply => {
                self.on_before_reload_scoped(vm, scope)
            }
            _ => (),
        }
    }

    fn on_after_apply(
        &mut self,
        _vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
    }

    fn on_after_dispatch(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        _value: ScriptValue,
    ) {
        match apply {
            Apply::New => self.on_after_new_scoped(vm, scope),
            Apply::Reload | Apply::Rebake | Apply::ScriptReapply => {
                self.on_after_reload_scoped(vm, scope)
            }
            _ => (),
        }
        self.on_alive()
    }
    // allows you to provide a custom apply impl, return true to skip generated apply code
    fn on_custom_apply(
        &mut self,
        _vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) -> bool {
        false
    }

    /// Allows a type with custom scalar syntax to override the value emitted
    /// by a derived `ScriptApply::script_to_value` implementation.
    fn on_custom_to_value(&self, _vm: &mut ScriptVm) -> Option<ScriptValue> {
        None
    }

    // implemented by procmacro for reflection into script objects/type cchecking
    fn on_type_check(_heap: &ScriptHeap, _value: ScriptValue) -> bool {
        false
    }
    fn on_proto_build(_vm: &mut ScriptVm, _obj: ScriptObject, _props: &mut ScriptTypeProps) {}
    fn on_proto_methods(_vm: &mut ScriptVm, _obj: ScriptObject) {}

    // Simple signatured lifecyclehooks
    fn on_alive(&self) {} // use this hook to quickly check if your object is alive, useful for debugging
    fn on_before_new(&mut self, _vm: &mut ScriptVm) {}
    fn on_before_reload(&mut self, _vm: &mut ScriptVm) {}
    fn on_after_new(&mut self, _vm: &mut ScriptVm) {}
    fn on_after_reload(&mut self, _vm: &mut ScriptVm) {}

    // simple with scope
    fn on_before_new_scoped(&mut self, vm: &mut ScriptVm, _scope: &mut Scope) {
        self.on_before_new(vm)
    }
    fn on_before_reload_scoped(&mut self, vm: &mut ScriptVm, _scope: &mut Scope) {
        self.on_before_reload(vm)
    }
    fn on_after_new_scoped(&mut self, vm: &mut ScriptVm, _scope: &mut Scope) {
        self.on_after_new(vm)
    }
    fn on_after_reload_scoped(&mut self, vm: &mut ScriptVm, _scope: &mut Scope) {
        self.on_after_reload(vm)
    }
}

/// `script_type_check` of a derived enum: the value is an object whose root
/// proto is one of the variants' ids.
#[doc(hidden)]
#[inline(never)]
pub fn script_derive_enum_type_check(heap: &ScriptHeap, value: ScriptValue, variants: &[LiveId]) -> bool {
    if let Some(o) = value.as_object() {
        if let Some(id) = heap.root_proto(o).as_id() {
            return variants.contains(&id);
        }
    }
    false
}

// The per-type functions the VM calls (type checks, proto builders) are `extern "C-unwind"`
// trampolines over the trait methods: the VM calls them without knowing which compiler built
// the type (local/plans/loader-core.md section 8). Same cost as a Rust fn pointer.
pub type ScriptTypeCheckFn = extern "C-unwind" fn(&ScriptHeap, ScriptValue) -> bool;
pub type ScriptProtoBuildFn = extern "C-unwind" fn(&mut ScriptVm, &mut ScriptTypeProps) -> ScriptValue;
pub type ScriptProtoPropsFn = extern "C-unwind" fn(&mut ScriptVm, ScriptObject, &mut ScriptTypeProps);
pub type ScriptProtoMethodsFn = extern "C-unwind" fn(&mut ScriptVm, ScriptObject);

#[doc(hidden)]
pub extern "C-unwind" fn script_type_check_c<T: ScriptNew + ?Sized>(heap: &ScriptHeap, value: ScriptValue) -> bool {
    T::script_type_check(heap, value)
}
#[doc(hidden)]
pub extern "C-unwind" fn script_on_type_check_c<T: ScriptHook + ?Sized>(heap: &ScriptHeap, value: ScriptValue) -> bool {
    T::on_type_check(heap, value)
}
#[doc(hidden)]
pub extern "C-unwind" fn script_proto_build_c<T: ScriptNew + ?Sized>(vm: &mut ScriptVm, props: &mut ScriptTypeProps) -> ScriptValue {
    T::script_proto_build(vm, props)
}
#[doc(hidden)]
pub extern "C-unwind" fn script_proto_props_c<T: ScriptNew + ?Sized>(vm: &mut ScriptVm, object: ScriptObject, props: &mut ScriptTypeProps) {
    T::script_proto_props(vm, object, props)
}
#[doc(hidden)]
pub extern "C-unwind" fn script_on_proto_build_c<T: ScriptHook + ?Sized>(vm: &mut ScriptVm, object: ScriptObject, props: &mut ScriptTypeProps) {
    T::on_proto_build(vm, object, props)
}
#[doc(hidden)]
pub extern "C-unwind" fn script_on_proto_methods_c<T: ScriptHook + ?Sized>(vm: &mut ScriptVm, object: ScriptObject) {
    T::on_proto_methods(vm, object)
}

#[relax_api(plain)]
#[derive(Clone, Copy, Debug)]
pub struct ScriptTypeProp {
    pub order: u32,
    pub ty: ScriptTypeId,
}

#[relax_api(handle, default)]
#[derive(Default, Debug)]
pub struct ScriptTypeProps {
    pub props: LiveIdMap<LiveId, ScriptTypeProp>,
    /// Index marking where Rust instance fields begin in the props list.
    /// Fields with order < rust_instance_start are config fields (live fields before #[deref]).
    /// Fields with order >= rust_instance_start are instance fields (deref parent fields + child's fields after deref).
    /// The shader compiler uses iter_rust_instance_ordered() to process only instance fields.
    pub rust_instance_start: u32,
}

/// What [`ScriptTypeProps::edit`] does.
pub const PROPS_INSERT: u32 = 0;
pub const PROPS_MARK_RUST_INSTANCE_START: u32 = 1;

#[relax_api]
impl ScriptTypeProps {
    /// Edits the props by `op`: [`PROPS_INSERT`] adds field `id` of type `ty`;
    /// [`PROPS_MARK_RUST_INSTANCE_START`] marks the current position as where Rust instance
    /// fields begin.
    #[relax_api]
    pub fn edit(&mut self, op: u32, id: LiveId, ty: ScriptTypeId) {
        match op {
            PROPS_INSERT => {
                let order = self.props.len() as u32;
                self.props.insert(id, ScriptTypeProp { order, ty });
            }
            _ => self.rust_instance_start = self.props.len() as u32,
        }
    }

    #[inline(always)]
    pub fn insert(&mut self, id: LiveId, ty: ScriptTypeId) {
        self.edit(PROPS_INSERT, id, ty);
    }

    /// Mark the current position as where Rust instance fields begin.
    /// Called by the derive macro just before processing the #[deref] field.
    /// Config fields (live fields before #[deref]) are added to props before this call,
    /// then parent fields and child's own fields are added after.
    #[inline(always)]
    pub fn mark_rust_instance_start(&mut self) {
        self.edit(PROPS_MARK_RUST_INSTANCE_START, LiveId(0), ScriptTypeId::from_u128(0));
    }

    pub fn iter_ordered(&self) -> impl Iterator<Item = (LiveId, ScriptTypeId)> + '_ {
        self.collect_ordered(false).into_iter()
    }

    /// Instance fields in declaration order, excluding configuration fields.
    pub fn iter_rust_instance_ordered(&self) -> impl Iterator<Item = (LiveId, ScriptTypeId)> + '_ {
        self.collect_ordered(true).into_iter()
    }

    fn collect_ordered(&self, rust_only: bool) -> Vec<(LiveId, ScriptTypeId)> {
        unsafe extern "C-unwind" fn push(ctx: *mut u8, name: LiveId, ty: ScriptTypeId) {
            (&mut *(ctx as *mut Vec<(LiveId, ScriptTypeId)>)).push((name, ty));
        }
        let mut out = Vec::new();
        unsafe { self.ordered_with(rust_only, &mut out as *mut Vec<(LiveId, ScriptTypeId)> as *mut u8, push); }
        out
    }

    /// # Safety
    /// `ctx` must remain valid for each synchronous invocation of `push`.
    #[relax_api]
    pub unsafe fn ordered_with(&self, rust_only: bool, ctx: *mut u8, push: unsafe extern "C-unwind" fn(*mut u8, LiveId, ScriptTypeId)) {
        let mut ordered: Vec<_> = self.props.iter().filter(|(_, prop)| !rust_only || prop.order >= self.rust_instance_start).collect();
        ordered.sort_by_key(|(_, prop)| prop.order);
        for (id, prop) in ordered { push(ctx, *id, prop.ty); }
    }
}

#[relax_api(plain)]
pub struct ScriptTypeObject {
    pub(crate) type_id: ScriptTypeId,
    pub(crate) check: ScriptTypeCheckFn,
    pub(crate) proto: ScriptValue,
    pub(crate) name: Option<LiveId>,
    /// the registering type's name as written in its `script_proto` (collision check)
    pub(crate) name_str: Option<&'static str>,
}

/// The same registering name (pointer first: one literal per type in practice).
#[inline]
pub(crate) fn same_type_name(a: Option<&'static str>, b: Option<&'static str>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => std::ptr::eq(a, b) || a == b,
        (None, None) => true,
        _ => false,
    }
}

/// Two different types with one ScriptTypeId (a 128-bit path-hash collision, or two types
/// registered under one fixed path): never silent.
#[cold]
#[inline(never)]
pub(crate) fn script_type_id_collision(id: ScriptTypeId, have: Option<&str>, new: Option<&str>) -> ! {
    panic!("ScriptTypeId collision: {:?} is registered for `{}` and now for `{}`", id, have.unwrap_or("?"), new.unwrap_or("?"))
}

#[relax_api(marked)]
impl ScriptTypeCheck {
    /// A check holding `props` (taken), `object` (moved in; null: none) and `is_repr_u32_enum`.
    ///
    /// # Safety
    /// `object` is null or a readable `ScriptTypeObject` the caller relinquishes.
    #[relax_api]
    pub unsafe fn build(props: &mut ScriptTypeProps, object: *const ScriptTypeObject, is_repr_u32_enum: bool) -> Self {
        let object = if object.is_null() { None } else { Some(std::ptr::read(object)) };
        Self { props: std::mem::take(props), object, is_repr_u32_enum }
    }
    #[inline]
    pub fn from_props(props: &mut ScriptTypeProps) -> Self {
        // SAFETY: no object
        unsafe { Self::build(props, std::ptr::null(), false) }
    }
    #[relax_api]
    pub fn props(&self) -> &ScriptTypeProps { &self.props }
}

#[relax_api(handle)]
pub struct ScriptTypeCheck {
    pub props: ScriptTypeProps,
    pub object: Option<ScriptTypeObject>,
    /// If true, this type is a `repr(u32)` enum and should be treated as `u32` in shaders.
    pub is_repr_u32_enum: bool,
}

#[relax_api(plain)]
#[derive(Copy, Clone, Hash, Eq, PartialEq)]
pub struct ScriptTypeIndex(pub(crate) u32);

// Non-generic helper to reduce monomorphization in script_proto
#[inline(never)]
pub fn register_type_inner(
    vm: &mut ScriptVm,
    type_id: ScriptTypeId,
    proto: ScriptValue,
    mut props: ScriptTypeProps,
    check: ScriptTypeCheckFn,
    name: Option<LiveId>,
    name_str: Option<&'static str>,
    is_repr_u32_enum: bool,
) -> ScriptValue {
    let object = std::mem::ManuallyDrop::new(ScriptTypeObject {
        type_id,
        proto,
        check,
        name,
        name_str,
    });
    // SAFETY: the object is moved into the check (ManuallyDrop here)
    let ty_check = unsafe { ScriptTypeCheck::build(&mut props, &*object, is_repr_u32_enum) };
    let ty_index = vm.heap_mut().register_type(Some(type_id), ty_check);
    if let Some(obj) = proto.as_object() {
        vm.heap_mut().set_type(obj, ty_index);
    }
    proto
}

/// Non-generic body of `ScriptNew::script_proto`: registers the type on its
/// first call. `name` (the type's name for error messages, entered in the id
/// table) and `is_repr_u32_enum` (a `repr(u32)` enum, which the shader
/// compiler treats as `u32`) are passed as values: the derive writes them
/// into its `script_proto`, so a script type costs no extra functions for
/// them.
#[doc(hidden)]
#[inline(never)]
pub fn script_proto_inner(
    vm: &mut ScriptVm,
    type_id: ScriptTypeId,
    build: ScriptProtoBuildFn,
    check: ScriptTypeCheckFn,
    name: Option<&'static str>,
    is_repr_u32_enum: bool,
) -> ScriptValue {
    let mut info = vm.heap().type_info(TYPE_REGISTERED, type_id, ScriptObject::ZERO);
    if !info.found && expand_on_type_miss(vm) {
        info = vm.heap().type_info(TYPE_REGISTERED, type_id, ScriptObject::ZERO);
    }
    if info.found {
        assert!(info.has_object);
        if !same_type_name(info.name_str(), name) {
            script_type_id_collision(type_id, info.name_str(), name);
        }
        return info.proto;
    }
    let mut props = ScriptTypeProps::default();
    let proto = build(vm, &mut props);
    let name_id = match name {
        Some(name) => Some(LiveId::from_str_with_lut(name).unwrap()),
        None => None,
    };
    register_type_inner(vm, type_id, proto, props, check, name_id, name, is_repr_u32_enum)
}

// Non-generic body of the default `ScriptNew::script_proto_build`.
#[inline(never)]
pub fn script_proto_build_inner(
    vm: &mut ScriptVm,
    props: &mut ScriptTypeProps,
    proto_props: ScriptProtoPropsFn,
    on_proto_build: ScriptProtoPropsFn,
    on_proto_methods: ScriptProtoMethodsFn,
) -> ScriptValue {
    let proto = vm.new_object();
    // build prototype here
    proto_props(vm, proto, props);
    on_proto_build(vm, proto, props);
    on_proto_methods(vm, proto);
    proto.into()
}

// Non-generic body of the default `ScriptNew::script_reload_default`.
#[inline(never)]
pub fn script_reload_default_inner(vm: &mut ScriptVm, type_id: ScriptTypeId) -> ScriptValue {
    if let Some(default_obj) = type_default_expanding(vm, type_id) {
        default_obj.into()
    } else {
        NIL
    }
}

// Non-generic body of the default `ScriptNew::script_type_check`.
#[inline(never)]
pub fn script_type_check_inner(
    heap: &ScriptHeap,
    value: ScriptValue,
    on_type_check: ScriptTypeCheckFn,
    type_id: ScriptTypeId,
) -> bool {
    if on_type_check(heap, value) {
        return true;
    }
    if let Some(o) = value.as_object() {
        heap.type_matches_id(o, type_id)
    } else {
        false
    }
}

// Helpers called by the `#[derive(Script)]` expansion. Each derived field is
// one call here instead of an inline block, so the per-field work is compiled
// once per field TYPE (or not generic at all) rather than once per struct and
// field. Not meant to be called by hand.
#[relax_api]
impl ScriptVm<'_> {
    /// The `#[live]` / `#[apply_default]` fields of one derived `script_apply`, resolved in one
    /// call (ScriptHeap::resolve_fields): `fields` sorted by id with each field's slot, `out` the
    /// slots in declaration order; a field set nowhere holds `ScriptValue::NOT_FOUND`. Eval applies
    /// read only the object's own map.
    #[doc(hidden)]
    #[relax_api]
    pub fn script_derive_resolve(&mut self, apply: &Apply, value: ScriptValue, fields: &[(LiveId, u32)], out: &mut [ScriptValue]) {
        self.heap().resolve_fields(value, apply.is_eval(), fields, out)
    }

    /// One resolved field of `script_apply`: apply `slot` to the field; a field set nowhere gets,
    /// on a reload, the type's registered default (`script_reload_default`), else stays untouched.
    #[doc(hidden)]
    #[inline(never)]
    pub fn script_derive_apply_slot<T: ScriptNew>(
        &mut self,
        apply: &Apply,
        scope: &mut Scope,
        slot: ScriptValue,
        field: &mut T,
    ) {
        let v = if slot == ScriptValue::NOT_FOUND {
            if !apply.is_reload() {
                return;
            }
            let default_value = <T as ScriptNew>::script_reload_default(self);
            if default_value.is_nil() {
                return;
            }
            default_value
        } else {
            slot
        };
        <T as ScriptApply>::script_apply(field, self, apply, scope, v);
    }

    /// Like [`Self::script_derive_apply_slot`], but for a `#[live(expr)]`
    /// field: when a reload falls back to a default, the field gets its own
    /// `declared_default` back (the same as a new one gets), not the type's.
    #[doc(hidden)]
    #[inline(never)]
    pub fn script_derive_apply_slot_or_default<T: ScriptNew>(
        &mut self,
        apply: &Apply,
        scope: &mut Scope,
        slot: ScriptValue,
        field: &mut T,
        declared_default: fn() -> T,
    ) {
        if slot != ScriptValue::NOT_FOUND {
            <T as ScriptApply>::script_apply(field, self, apply, scope, slot);
        } else if apply.is_reload() && !<T as ScriptNew>::script_reload_default(self).is_nil() {
            *field = declared_default();
        }
    }

    /// `script_to_value` of a derived struct: the custom hook, or a new object
    /// with the type's proto holding the fields.
    #[doc(hidden)]
    pub fn script_derive_to_value<T: ScriptNew>(&mut self, this: &T) -> ScriptValue {
        if let Some(value) = <T as ScriptHook>::on_custom_to_value(this, self) {
            return value;
        }
        let proto = T::script_proto(self).into();
        let obj = self.new_with_proto_of(proto);
        this.script_to_value_props(self, obj);
        obj.into()
    }

    /// A run of `#[live]` / `#[apply_default]` fields of `script_to_value_props`: `obj.ids[i] =
    /// values[i]`, in order, in one call.
    #[doc(hidden)]
    pub fn script_derive_set_props(&mut self, obj: ScriptObject, ids: &[LiveId], values: &[ScriptValue]) {
        for (id, value) in ids.iter().zip(values) {
            self.set_value_trap_of(obj, ScriptValue::from_id(*id), *value);
        }
    }

    #[inline(never)]
    #[track_caller]
    pub fn script_derive_set_prop(&mut self, obj: ScriptObject, name: &str, value: ScriptValue) {
        self.set_value_trap_of(obj, ScriptValue::from_id(LiveId::from_str_with_lut(name).unwrap()), value);
    }

    /// A `#[live]` / `#[apply_default]` field of `script_proto_props`.
    #[doc(hidden)]
    #[inline(never)]
    #[track_caller]
    pub fn script_derive_proto_field<T: ScriptNew>(
        &mut self,
        props: &mut ScriptTypeProps,
        name: &str,
    ) {
        <T as ScriptNew>::script_proto(self);
        props.insert(
            LiveId::from_str_with_lut(name).unwrap(),
            <T as ScriptNew>::script_type_id_static(),
        );
    }

    /// A bare variant of a derived enum's `script_proto_build`.
    /// `proto_name` is the variant's name, `key` its `id!`, `enum_name` the
    /// enum's name; `repr_value` is the discriminant of a `repr(u32)` enum.
    #[doc(hidden)]
    #[inline(never)]
    #[track_caller]
    pub fn script_derive_enum_bare_variant(
        &mut self,
        enum_object: ScriptObject,
        proto_name: &str,
        key: LiveId,
        enum_name: &str,
        repr_value: Option<f64>,
    ) {
        let bare = self.new_with_proto_of(LiveId::from_str_with_lut(proto_name).unwrap().into());
        if let Some(repr_value) = repr_value {
            self.set_value_trap_of(bare, id!(_repr_u32_enum_value).into(), repr_value.into());
        }
        self.set_value_trap_of(enum_object, key.into(), bare.into());
        self.set_value_trap_of(bare, LiveId::from_str_with_lut("__enum").unwrap().into(), LiveId::from_str_with_lut(enum_name).unwrap().into());
        self.heap_mut().freeze(bare);
    }

    /// The constructor method of a tuple variant of a derived enum: builds
    /// the variant object from `args`, reporting a wrong argument count and
    /// any argument `checks[i]` rejects as errors at `file`:`line`.
    #[doc(hidden)]
    #[inline(never)]
    pub fn script_derive_enum_tuple_new(
        &mut self,
        args: ScriptObject,
        key: LiveId,
        enum_name: &str,
        checks: &[ScriptTypeCheckFn],
        file: &str,
        line: u32,
    ) -> ScriptValue {
        let tuple = self.new_with_proto_of(key.into());
        self.set_value_trap_of(
            tuple,
            LiveId::from_str_with_lut("__enum").unwrap().into(),
            LiveId::from_str_with_lut(enum_name).unwrap().into(),
        );
        if self.heap().vec_len(args) != checks.len() {
            self.script_derive_push_err(
                ScriptValue::script_err_invalid_args,
                "wrong argument count",
                file,
                line,
            );
        }
        for (i, check) in checks.iter().enumerate() {
            if let Some(a) = self.heap().vec_value_if_exist(args, i) {
                if !check(self.heap(), a) {
                    self.script_derive_push_err(
                        ScriptValue::script_err_type_mismatch,
                        "argument type mismatch",
                        file,
                        line,
                    );
                }
            }
        }
        let p = self.bx.parts_mut();
        p.heap.vec_push_vec(tuple, args, p.trap.pass());
        tuple.into()
    }

    /// A derived enum's `script_apply` met an object whose root id names no
    /// variant.
    #[doc(hidden)]
    #[inline(never)]
    pub fn script_derive_enum_unknown_variant(
        &mut self,
        enum_name: &str,
        other: LiveId,
        object: ScriptObject,
        file: &str,
        line: u32,
    ) {
        let obj_desc = self.format_object_for_error(object);
        self.script_derive_push_err(
            ScriptValue::script_err_unknown_type,
            &format!(
                "unknown variant '{}' for enum {}, object: {}",
                other, enum_name, obj_desc
            ),
            file,
            line,
        );
    }

    /// A derived enum's `script_apply` met an object without a variant id.
    #[doc(hidden)]
    #[inline(never)]
    pub fn script_derive_enum_not_variant(
        &mut self,
        enum_name: &str,
        object: ScriptObject,
        file: &str,
        line: u32,
    ) {
        let obj_desc = self.format_object_for_error(object);
        self.script_derive_push_err(
            ScriptValue::script_err_unknown_type,
            &format!(
                "expected variant id for enum {}, got object: {}",
                enum_name, obj_desc
            ),
            file,
            line,
        );
    }

    /// A derived enum's `script_apply` met a value that is not an object.
    #[doc(hidden)]
    #[inline(never)]
    pub fn script_derive_enum_bad_value(
        &mut self,
        enum_name: &str,
        value: ScriptValue,
        file: &str,
        line: u32,
    ) {
        let value_desc = self.format_enum_variant_error(value);
        self.script_derive_push_err(
            ScriptValue::script_err_unknown_type,
            &format!("expected variant for enum {}, got {}", enum_name, value_desc),
            file,
            line,
        );
    }

    // What the `script_err_*!` macros do, with the origin passed in: the
    // derive hands over the `file!()` / `line!()` of its own expansion.
    pub fn script_derive_push_err(
        &mut self,
        err: fn(ScriptIp) -> ScriptValue,
        message: &str,
        file: &str,
        line: u32,
    ) {
        if let crate::trap::ScriptTrap::Inner(trap) = self.trap() {
            trap.push_err(err(trap.cur_ip()), &message, file, line);
        }
    }
}

/// Non-generic body of the default `ScriptNew::script_pod`: the pod struct type of a registered
/// type (`type_name` for the layout assertions' messages).
#[inline(never)]
#[relax_api]
pub fn script_pod_inner(vm: &mut ScriptVm, type_id: ScriptTypeId, type_name: &str) -> Option<ScriptPodType> {
    use crate::pod::*;
    use makepad_math::{
        F16x2, F16x4, I16x2, Mat4f, Quat, SNorm16x2, SNorm8x4, U16x2, UNorm16x2, UNorm8x4,
        Vec2f, Vec3f, Vec4f,
    };

    fn align_up(offset: usize, align: usize) -> usize {
        if align == 0 {
            return offset;
        }
        let rem = offset % align;
        if rem == 0 {
            offset
        } else {
            offset + (align - rem)
        }
    }

    fn rust_repr_layout_for_type_id(
        heap: &ScriptHeap,
        type_id: ScriptTypeId,
    ) -> Option<(usize, usize)> {
        if type_id == <f32 as ScriptNew>::script_type_id_static() {
            return Some((std::mem::size_of::<f32>(), std::mem::align_of::<f32>()));
        }
        if type_id == <f64 as ScriptNew>::script_type_id_static() {
            return Some((std::mem::size_of::<f64>(), std::mem::align_of::<f64>()));
        }
        if type_id == <u32 as ScriptNew>::script_type_id_static() {
            return Some((std::mem::size_of::<u32>(), std::mem::align_of::<u32>()));
        }
        if type_id == ({ const ID: ScriptTypeId = ScriptTypeId::of_path("makepad_script::prim::i32"); ID }) {
            return Some((std::mem::size_of::<i32>(), std::mem::align_of::<i32>()));
        }
        if type_id == <bool as ScriptNew>::script_type_id_static() {
            return Some((std::mem::size_of::<bool>(), std::mem::align_of::<bool>()));
        }
        if type_id == <Vec2f as ScriptNew>::script_type_id_static() {
            return Some((std::mem::size_of::<Vec2f>(), std::mem::align_of::<Vec2f>()));
        }
        if type_id == <Vec3f as ScriptNew>::script_type_id_static() {
            return Some((std::mem::size_of::<Vec3f>(), std::mem::align_of::<Vec3f>()));
        }
        if type_id == <Vec4f as ScriptNew>::script_type_id_static() {
            return Some((std::mem::size_of::<Vec4f>(), std::mem::align_of::<Vec4f>()));
        }
        if type_id == <Mat4f as ScriptNew>::script_type_id_static() {
            return Some((std::mem::size_of::<Mat4f>(), std::mem::align_of::<Mat4f>()));
        }
        if type_id == ({ const ID: ScriptTypeId = ScriptTypeId::of_path("makepad_script::prim::Quat"); ID }) {
            return Some((std::mem::size_of::<Quat>(), std::mem::align_of::<Quat>()));
        }
        if type_id == <F16x2 as ScriptNew>::script_type_id_static() {
            return Some((std::mem::size_of::<F16x2>(), std::mem::align_of::<F16x2>()));
        }
        if type_id == <F16x4 as ScriptNew>::script_type_id_static() {
            return Some((std::mem::size_of::<F16x4>(), std::mem::align_of::<F16x4>()));
        }
        if type_id == <U16x2 as ScriptNew>::script_type_id_static() {
            return Some((std::mem::size_of::<U16x2>(), std::mem::align_of::<U16x2>()));
        }
        if type_id == <I16x2 as ScriptNew>::script_type_id_static() {
            return Some((std::mem::size_of::<I16x2>(), std::mem::align_of::<I16x2>()));
        }
        if type_id == <UNorm16x2 as ScriptNew>::script_type_id_static() {
            return Some((
                std::mem::size_of::<UNorm16x2>(),
                std::mem::align_of::<UNorm16x2>(),
            ));
        }
        if type_id == <SNorm16x2 as ScriptNew>::script_type_id_static() {
            return Some((
                std::mem::size_of::<SNorm16x2>(),
                std::mem::align_of::<SNorm16x2>(),
            ));
        }
        if type_id == <UNorm8x4 as ScriptNew>::script_type_id_static() {
            return Some((
                std::mem::size_of::<UNorm8x4>(),
                std::mem::align_of::<UNorm8x4>(),
            ));
        }
        if type_id == <SNorm8x4 as ScriptNew>::script_type_id_static() {
            return Some((
                std::mem::size_of::<SNorm8x4>(),
                std::mem::align_of::<SNorm8x4>(),
            ));
        }

        let type_check = heap.registered_type(type_id)?;
        let mut offset = 0usize;
        let mut align = 1usize;
        for (_, field_type_id) in type_check.props.iter_rust_instance_ordered() {
            let (field_size, field_align) = rust_repr_layout_for_type_id(heap, field_type_id)?;
            offset = align_up(offset, field_align);
            offset += field_size;
            align = align.max(field_align);
        }
        Some((align_up(offset, align), align))
    }


    let type_check = vm.bx.heap.registered_type(type_id)?;

    // Build pod fields from the type props
    // Use iter_rust_instance_ordered to skip config fields (live fields before #[deref])
    let mut fields = Vec::new();
    let mut ordered_layout = Vec::new();

    for (field_name, field_type_id) in type_check.props.iter_rust_instance_ordered() {
        // Try to get the pod type for this field's type
        if let Some(pod_type) = vm
            .bx
            .heap
            .type_id_to_pod_type(field_type_id, &vm.bx.code.builtins.pod)
        {
            let pod_type_data = vm.bx.heap.pod_type_ref(pod_type);

            fields.push(ScriptPodField {
                name: field_name,
                ty: ScriptPodTypeInline {
                    self_ref: pod_type,
                    data: pod_type_data.clone(),
                },
                default: pod_type_data.default,
            });
            ordered_layout.push((field_name, field_type_id, pod_type_data.ty.clone()));
        } else {
            // Field type doesn't have a corresponding pod type
            return None;
        }
    }

    // Create the pod type using the centralized layout calculation
    let pod_obj = vm.bx.heap.new_with_proto(id!(pod_struct).into());
    vm.bx.heap.set_object_storage_vec2(pod_obj);
    vm.bx.heap.set_notproto(pod_obj);

    let mut rust_offset = 0usize;
    let mut shader_offset = 0usize;
    for (field_name, field_type_id, shader_ty) in &ordered_layout {
        let (rust_size, rust_align) =
            rust_repr_layout_for_type_id(&vm.bx.heap, *field_type_id)?;
        rust_offset = align_up(rust_offset, rust_align);
        shader_offset = align_up(shader_offset, shader_ty.align_of());
        assert!(
            rust_offset == shader_offset,
            "Rust POD field offset mismatch for {}.{}: Rust repr(C) offset is {}, shader POD offset is {}. Add explicit padding fields for std140 compatibility.",
            type_name,
            field_name,
            rust_offset,
            shader_offset
        );
        rust_offset += rust_size;
        shader_offset += shader_ty.size_of();
    }

    let pod_ty = ScriptPodTy::new_struct(fields);
    let rust_size = rust_repr_layout_for_type_id(&vm.bx.heap, type_id)?.0;
    let pod_size = pod_ty.size_of();
    assert!(
        rust_size == pod_size,
        "Rust POD size mismatch for {}: Rust repr(C) size is {}, shader POD size is {}. Add explicit padding fields for std140 compatibility.",
        type_name,
        rust_size,
        pod_size
    );

    let pt = vm.bx.heap.new_pod_type(pod_obj, None, pod_ty, NIL);
    vm.bx.heap.set_object_pod_type(pod_obj, pt);
    vm.bx.heap.freeze(pod_obj);

    Some(pt)
}

/// A type the registry misses may be one a lazily expanded module group registers (a Splash
/// isolate's widget library registers its types when it expands): expand the heap's pending
/// groups, once, before concluding it is not there. A miss only (registration is the rare
/// path), so a registered type costs nothing more. True when something was expanded.
#[inline(never)]
pub fn expand_on_type_miss(vm: &mut ScriptVm) -> bool {
    vm.vm_lazy(crate::lazy_modules::LAZY_EXPAND_ALL, LiveId(0), "")
}

/// The default object of type `type_id`, expanding pending lazy groups on a miss.
#[inline(never)]
pub fn type_default_expanding(vm: &mut ScriptVm, type_id: ScriptTypeId) -> Option<ScriptObject> {
    match vm.heap().type_default_for_id(type_id) {
        Some(object) => Some(object),
        None if expand_on_type_miss(vm) => vm.heap().type_default_for_id(type_id),
        None => None,
    }
}

/// Non-generic body of the default `ScriptNew::script_enum_lookup_variant`.
#[inline(never)]
pub fn script_enum_lookup_variant_inner(vm: &mut ScriptVm, type_id: ScriptTypeId, variant: LiveId) -> ScriptValue {
    let mut info = vm.heap().type_info(TYPE_REGISTERED, type_id, ScriptObject::ZERO);
    if !(info.found && info.has_object) && expand_on_type_miss(vm) {
        info = vm.heap().type_info(TYPE_REGISTERED, type_id, ScriptObject::ZERO);
    }
    assert!(info.found && info.has_object);
    let obj = info.proto.into();
    let p = vm.bx.parts_mut();
    p.heap.value(obj, variant.into(), crate::trap::ScriptTrap::Inner(p.trap))
}

// implementation is procmacro generated
pub trait ScriptNew: ScriptApply + ScriptHook
where
    Self: 'static,
{
    fn script_type_check(heap: &ScriptHeap, value: ScriptValue) -> bool {
        script_type_check_inner(
            heap,
            value,
            script_on_type_check_c::<Self>,
            Self::script_type_id_static(),
        )
    }

    /// Builds a pod struct type from the macro-generated type reflection.
    /// This iterates through the ScriptTypeProps in order and generates
    /// a ScriptPodTy::Struct with fields matching the struct's layout.
    /// Uses iter_rust_instance_ordered() to skip config fields before #[deref].
    fn script_pod(vm: &mut ScriptVm) -> Option<ScriptPodType>
    where
        Self: Sized,
    {
        // First ensure the proto is built so type reflection is available
        Self::script_proto(vm);
        script_pod_inner(vm, Self::script_type_id_static(), std::any::type_name::<Self>())
    }

    fn script_default(vm: &mut ScriptVm) -> ScriptValue
    where
        Self: Sized,
    {
        Self::script_proto(vm);
        Self::script_new(vm).script_to_value(vm)
    }

    fn script_reload_default(vm: &mut ScriptVm) -> ScriptValue
    where
        Self: Sized,
    {
        script_reload_default_inner(vm, Self::script_type_id_static())
    }

    /// Derived and the VM's own types return a fixed-path id (const); this default is for
    /// hand-written impls (ScriptTypeId docs).
    fn script_type_id_static() -> ScriptTypeId {
        ScriptTypeId::of_type_name::<Self>()
    }
    fn script_new(vm: &mut ScriptVm) -> Self;

    fn script_new_with_default(vm: &mut ScriptVm) -> Self
    where
        Self: Sized,
    {
        let type_id = Self::script_type_id_static();
        if let Some(default_obj) = type_default_expanding(vm, type_id) {
            Self::script_from_value(vm, default_obj.into())
        } else {
            Self::script_new(vm)
        }
    }

    fn from_script_mod(vm: &mut ScriptVm, f: fn(&mut ScriptVm) -> ScriptValue) -> Self
    where
        Self: Sized,
    {
        let value = f(vm);
        if value.is_nil() {
            panic!(
                "script_mod! returned nil — the script block must end with an expression \
                 that evaluates to the app value (e.g. add `app` as the last line after \
                 `let app = startup() do ...{{ }}`)."
            );
        }
        Self::script_from_value(vm, value)
    }

    // default impls

    fn script_from_value(vm: &mut ScriptVm, value: ScriptValue) -> Self
    where
        Self: Sized,
    {
        let mut s = Self::script_new(vm);
        s.script_apply(vm, &Apply::New, &mut Scope::empty(), value);
        s
    }

    fn script_from_value_scoped(vm: &mut ScriptVm, scope: &mut Scope, value: ScriptValue) -> Self
    where
        Self: Sized,
    {
        let mut s = Self::script_new(vm);
        s.script_apply(vm, &Apply::New, scope, value);
        s
    }

    /// The type's prototype, registered on first use. The derive (and the
    /// primitive impls) override it to pass the type's name; this default
    /// registers an unnamed type.
    fn script_proto(vm: &mut ScriptVm) -> ScriptValue {
        script_proto_inner(
            vm,
            Self::script_type_id_static(),
            script_proto_build_c::<Self>,
            script_type_check_c::<Self>,
            None,
            false,
        )
    }

    fn script_proto_build(vm: &mut ScriptVm, props: &mut ScriptTypeProps) -> ScriptValue {
        script_proto_build_inner(
            vm,
            props,
            script_proto_props_c::<Self>,
            script_on_proto_build_c::<Self>,
            script_on_proto_methods_c::<Self>,
        )
    }

    fn script_proto_props(_vm: &mut ScriptVm, _object: ScriptObject, _props: &mut ScriptTypeProps) {
    }

    fn script_api(vm: &mut ScriptVm) -> ScriptValue {
        let val = Self::script_proto(vm);
        vm.heap_mut().freeze_api(val.into());
        val
    }

    fn script_component(vm: &mut ScriptVm) -> ScriptValue {
        let val = Self::script_proto(vm);
        vm.heap_mut().freeze_component(val.into());
        val
    }

    fn script_shader(vm: &mut ScriptVm) -> ScriptValue {
        let val = Self::script_proto(vm);
        vm.heap_mut().freeze_shader(val.into());
        val
    }

    fn script_ext(vm: &mut ScriptVm) -> ScriptValue {
        let val = Self::script_proto(vm);
        vm.heap_mut().freeze_ext(val.into());
        val
    }

    fn script_enum_lookup_variant(vm: &mut ScriptVm, variant: LiveId) -> ScriptValue {
        script_enum_lookup_variant_inner(vm, Self::script_type_id_static(), variant)
    }
}

pub trait ScriptApply {
    /// Equals `ScriptNew::script_type_id_static` for types that implement both (the derive
    /// and the VM's own impls forward to it).
    fn script_type_id(&self) -> ScriptTypeId
    where
        Self: 'static,
    {
        ScriptTypeId::of_type_name::<Self>()
    }
    fn script_apply(
        &mut self,
        _vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
    }
    fn script_to_value(&self, _vm: &mut ScriptVm) -> ScriptValue {
        NIL
    }
    fn script_to_value_props(&self, _vm: &mut ScriptVm, _obj: ScriptObject) {}
    fn script_source(&self) -> ScriptObject {
        ScriptObject::ZERO
    }
    /// The heap key (`ScriptObjectRef::heap_key`) of the object
    /// [`ScriptApply::script_source`] names: which VM's heap it lives in. 0
    /// when unknown (no `#[source]`, or not set yet).
    fn script_source_heap_key(&self) -> usize {
        0
    }

    /// Evaluates a ScriptMod and applies the result to self.
    /// The ScriptMod is deduplicated by file/line/column so calling this repeatedly
    /// with the same source location won't create multiple code blocks.
    /// The script code should be wrapped as `__script_source__{...}` and this method
    /// sets `__script_source__` on the scope to `self.script_source()` before evaluation.
    fn script_apply_eval(&mut self, vm: &mut ScriptVm, script_mod: ScriptMod) {
        let source = self.script_source();
        let value = vm.eval_with_source(script_mod, source);
        self.script_apply(vm, &Apply::Eval, &mut Scope::default(), value);
    }
}

pub trait ScriptApplyDefault {
    fn script_apply_default(
        &mut self,
        _vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) -> Option<ScriptValue> {
        None
    }
}

pub trait ScriptReset {
    fn script_reset(&mut self, vm: &mut ScriptVm, apply: &Apply, value: ScriptValue);
}
