use crate::apply::*;
use crate::heap::*;
use crate::value::*;
use crate::vm::*;
use makepad_live_id::*;

// ============================================================================
// Script traits
// ============================================================================

pub trait ScriptDeriveMarker {}

pub type ScriptTypeId = std::any::TypeId;

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

pub trait ScriptHookDeref {
    fn on_deref_before_apply(
        &mut self,
        _vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
    }
    fn on_deref_after_apply(
        &mut self,
        _vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ScriptTypeProp {
    pub order: u32,
    pub ty: ScriptTypeId,
}

#[derive(Default, Debug)]
pub struct ScriptTypeProps {
    pub props: LiveIdMap<LiveId, ScriptTypeProp>,
    /// Index marking where Rust instance fields begin in the props list.
    /// Fields with order < rust_instance_start are config fields (live fields before #[deref]).
    /// Fields with order >= rust_instance_start are instance fields (deref parent fields + child's fields after deref).
    /// The shader compiler uses iter_rust_instance_ordered() to process only instance fields.
    pub rust_instance_start: u32,
}

impl ScriptTypeProps {
    pub fn insert(&mut self, id: LiveId, ty: ScriptTypeId) {
        let order = self.props.len() as u32;
        self.props.insert(id, ScriptTypeProp { order, ty });
    }

    /// Mark the current position as where Rust instance fields begin.
    /// Called by the derive macro just before processing the #[deref] field.
    /// Config fields (live fields before #[deref]) are added to props before this call,
    /// then parent fields and child's own fields are added after.
    pub fn mark_rust_instance_start(&mut self) {
        self.rust_instance_start = self.props.len() as u32;
    }

    pub fn iter_ordered(&self) -> impl Iterator<Item = (LiveId, ScriptTypeId)> + '_ {
        let mut ordered: Vec<_> = self.props.iter().map(|(k, v)| (*k, *v)).collect();
        ordered.sort_by_key(|(_, prop)| prop.order);
        ordered.into_iter().map(|(id, prop)| (id, prop.ty))
    }

    /// Iterate over props that are part of the Rust instance data.
    /// Skips config fields (live fields before #[deref]) and returns instance fields in order:
    /// deref parent fields first, then child's own fields after deref.
    /// Used by the shader compiler to build the RustInstance struct layout.
    pub fn iter_rust_instance_ordered(&self) -> impl Iterator<Item = (LiveId, ScriptTypeId)> + '_ {
        let rust_instance_start = self.rust_instance_start;
        let mut ordered: Vec<_> = self
            .props
            .iter()
            .filter(|(_, prop)| prop.order >= rust_instance_start)
            .map(|(k, v)| (*k, *v))
            .collect();
        ordered.sort_by_key(|(_, prop)| prop.order);
        ordered.into_iter().map(|(id, prop)| (id, prop.ty))
    }
}

pub struct ScriptTypeObject {
    pub(crate) type_id: ScriptTypeId,
    pub(crate) check: fn(&ScriptHeap, ScriptValue) -> bool, // Function pointer instead of boxed closure
    pub(crate) proto: ScriptValue,
    pub(crate) name: Option<LiveId>,
}

pub struct ScriptTypeCheck {
    pub props: ScriptTypeProps,
    pub object: Option<ScriptTypeObject>,
    /// If true, this type is a `repr(u32)` enum and should be treated as `u32` in shaders.
    pub is_repr_u32_enum: bool,
}

#[derive(Copy, Clone, Hash, Eq, PartialEq)]
pub struct ScriptTypeIndex(pub(crate) u32);

// Non-generic helper to reduce monomorphization in script_proto
#[inline(never)]
fn register_type_inner(
    vm: &mut ScriptVm,
    type_id: ScriptTypeId,
    proto: ScriptValue,
    props: ScriptTypeProps,
    check: fn(&ScriptHeap, ScriptValue) -> bool,
    name: Option<LiveId>,
    is_repr_u32_enum: bool,
) -> ScriptValue {
    let ty_check = ScriptTypeCheck {
        object: Some(ScriptTypeObject {
            type_id,
            proto,
            check,
            name,
        }),
        props,
        is_repr_u32_enum,
    };
    let ty_index = vm.bx.heap.register_type(Some(type_id), ty_check);
    if let Some(obj) = proto.as_object() {
        vm.bx.heap.set_type(obj, ty_index);
    }
    proto
}

// Non-generic body of the default `ScriptNew::script_proto`.
#[inline(never)]
fn script_proto_inner(
    vm: &mut ScriptVm,
    type_id: ScriptTypeId,
    build: fn(&mut ScriptVm, &mut ScriptTypeProps) -> ScriptValue,
    check: fn(&ScriptHeap, ScriptValue) -> bool,
    name: fn() -> Option<LiveId>,
    is_repr_u32_enum: fn() -> bool,
) -> ScriptValue {
    if let Some(check) = vm.bx.heap.registered_type(type_id) {
        return check.object.as_ref().unwrap().proto;
    }
    let mut props = ScriptTypeProps::default();
    let proto = build(vm, &mut props);
    register_type_inner(vm, type_id, proto, props, check, name(), is_repr_u32_enum())
}

// Non-generic body of the default `ScriptNew::script_proto_build`.
#[inline(never)]
fn script_proto_build_inner(
    vm: &mut ScriptVm,
    props: &mut ScriptTypeProps,
    proto_props: fn(&mut ScriptVm, ScriptObject, &mut ScriptTypeProps),
    on_proto_build: fn(&mut ScriptVm, ScriptObject, &mut ScriptTypeProps),
    on_proto_methods: fn(&mut ScriptVm, ScriptObject),
) -> ScriptValue {
    let proto = vm.bx.heap.new_object();
    // build prototype here
    proto_props(vm, proto, props);
    on_proto_build(vm, proto, props);
    on_proto_methods(vm, proto);
    proto.into()
}

// Non-generic body of the default `ScriptNew::script_reload_default`.
#[inline(never)]
fn script_reload_default_inner(vm: &mut ScriptVm, type_id: ScriptTypeId) -> ScriptValue {
    if let Some(default_obj) = vm.bx.heap.type_default_for_id(type_id) {
        default_obj.into()
    } else {
        NIL
    }
}

// Non-generic body of the default `ScriptNew::script_type_check`.
#[inline(never)]
fn script_type_check_inner(
    heap: &ScriptHeap,
    value: ScriptValue,
    on_type_check: fn(&ScriptHeap, ScriptValue) -> bool,
    type_id: fn() -> ScriptTypeId,
) -> bool {
    if on_type_check(heap, value) {
        return true;
    }
    if let Some(o) = value.as_object() {
        heap.type_matches_id(o, type_id())
    } else {
        false
    }
}

// Helpers called by the `#[derive(Script)]` expansion. Each derived field is
// one call here instead of an inline block, so the per-field work is compiled
// once per field TYPE (or not generic at all) rather than once per struct and
// field. Not meant to be called by hand.
impl ScriptVm<'_> {
    /// A `#[live]` / `#[apply_default]` field of `script_apply`: apply the
    /// field's value from `value` (the prototype chain included), or on a
    /// reload the type's registered default when the object doesn't set it.
    #[doc(hidden)]
    #[inline(never)]
    pub fn script_derive_apply_field<T: ScriptNew>(
        &mut self,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
        id: LiveId,
        field: &mut T,
    ) {
        let mut field_value = self.bx.heap.value_for_apply(value, id.into(), apply);
        if field_value.is_none() && apply.is_reload() {
            let default_value = <T as ScriptNew>::script_reload_default(self);
            if !default_value.is_nil() {
                field_value = Some(default_value);
            }
        }
        if let Some(v) = field_value {
            <T as ScriptApply>::script_apply(field, self, apply, scope, v);
        }
    }

    /// A `#[live]` / `#[apply_default]` field of `script_to_value_props`.
    #[doc(hidden)]
    #[inline(never)]
    #[track_caller]
    pub fn script_derive_field_to_value<T: ScriptApply + ?Sized>(
        &mut self,
        obj: ScriptObject,
        name: &str,
        field: &T,
    ) {
        let value: ScriptValue = <T as ScriptApply>::script_to_value(field, self);
        self.script_derive_set_prop(obj, name, value);
    }

    #[inline(never)]
    #[track_caller]
    fn script_derive_set_prop(&mut self, obj: ScriptObject, name: &str, value: ScriptValue) {
        self.bx.heap.set_value(
            obj,
            ScriptValue::from_id(LiveId::from_str_with_lut(name).unwrap()),
            value,
            self.bx.threads.cur().trap.pass(),
        );
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
        let bare = self
            .bx
            .heap
            .new_with_proto(LiveId::from_str_with_lut(proto_name).unwrap().into());
        if let Some(repr_value) = repr_value {
            self.bx.heap.set_value(
                bare,
                id!(_repr_u32_enum_value).into(),
                ScriptValue::from(repr_value),
                self.bx.threads.cur().trap.pass(),
            );
        }
        self.bx.heap.set_value(
            enum_object,
            key.into(),
            bare.into(),
            self.bx.threads.cur().trap.pass(),
        );
        self.bx.heap.set_value(
            bare,
            LiveId::from_str_with_lut("__enum").unwrap().into(),
            LiveId::from_str_with_lut(enum_name).unwrap().into(),
            self.bx.threads.cur().trap.pass(),
        );
        self.bx.heap.freeze(bare);
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
        checks: &[fn(&ScriptHeap, ScriptValue) -> bool],
        file: &str,
        line: u32,
    ) -> ScriptValue {
        let tuple = self.bx.heap.new_with_proto(key.into());
        self.bx.heap.set_value(
            tuple,
            LiveId::from_str_with_lut("__enum").unwrap().into(),
            LiveId::from_str_with_lut(enum_name).unwrap().into(),
            self.bx.threads.cur().trap.pass(),
        );
        if self.bx.heap.vec_len(args) != checks.len() {
            self.script_derive_push_err(
                ScriptValue::script_err_invalid_args,
                "wrong argument count".to_string(),
                file,
                line,
            );
        }
        for (i, check) in checks.iter().enumerate() {
            if let Some(a) = self.bx.heap.vec_value_if_exist(args, i) {
                if !check(&self.bx.heap, a) {
                    self.script_derive_push_err(
                        ScriptValue::script_err_type_mismatch,
                        "argument type mismatch".to_string(),
                        file,
                        line,
                    );
                }
            }
        }
        self.bx
            .heap
            .vec_push_vec(tuple, args, self.bx.threads.cur().trap.pass());
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
            format!(
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
            format!(
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
            format!("expected variant for enum {}, got {}", enum_name, value_desc),
            file,
            line,
        );
    }

    // What the `script_err_*!` macros do, with the origin passed in: the
    // derive hands over the `file!()` / `line!()` of its own expansion.
    fn script_derive_push_err(
        &mut self,
        err: fn(ScriptIp) -> ScriptValue,
        message: String,
        file: &str,
        line: u32,
    ) {
        if let crate::trap::ScriptTrap::Inner(trap) = self.bx.threads.cur().trap.pass() {
            trap.push_err(err(trap.ip), message, file.into(), line);
        }
    }
}

// implementation is procmacro generated
pub trait ScriptNew: ScriptApply + ScriptHook
where
    Self: 'static,
{
    /// Returns the LiveId name of this type for error messages.
    /// Override this in derive macro to provide meaningful type names.
    fn script_type_name() -> Option<LiveId> {
        None
    }

    /// Returns true if this type is a `repr(u32)` enum.
    /// Override in derive macro for enums with discriminants.
    /// Used by shader compiler to treat the enum as `u32`.
    fn is_repr_u32_enum() -> bool {
        false
    }

    fn script_type_check(heap: &ScriptHeap, value: ScriptValue) -> bool {
        script_type_check_inner(
            heap,
            value,
            <Self as ScriptHook>::on_type_check,
            Self::script_type_id_static,
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
        use crate::pod::*;
        use makepad_math::{
            F16x2, F16x4, I16x2, Mat4f, Quat, SNorm16x2, SNorm8x4, U16x2, UNorm16x2, UNorm8x4,
            Vec2f, Vec3f, Vec4f,
        };
        use std::any::TypeId;

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
            if type_id == TypeId::of::<f32>() {
                return Some((std::mem::size_of::<f32>(), std::mem::align_of::<f32>()));
            }
            if type_id == TypeId::of::<f64>() {
                return Some((std::mem::size_of::<f64>(), std::mem::align_of::<f64>()));
            }
            if type_id == TypeId::of::<u32>() {
                return Some((std::mem::size_of::<u32>(), std::mem::align_of::<u32>()));
            }
            if type_id == TypeId::of::<i32>() {
                return Some((std::mem::size_of::<i32>(), std::mem::align_of::<i32>()));
            }
            if type_id == TypeId::of::<bool>() {
                return Some((std::mem::size_of::<bool>(), std::mem::align_of::<bool>()));
            }
            if type_id == TypeId::of::<Vec2f>() {
                return Some((std::mem::size_of::<Vec2f>(), std::mem::align_of::<Vec2f>()));
            }
            if type_id == TypeId::of::<Vec3f>() {
                return Some((std::mem::size_of::<Vec3f>(), std::mem::align_of::<Vec3f>()));
            }
            if type_id == TypeId::of::<Vec4f>() {
                return Some((std::mem::size_of::<Vec4f>(), std::mem::align_of::<Vec4f>()));
            }
            if type_id == TypeId::of::<Mat4f>() {
                return Some((std::mem::size_of::<Mat4f>(), std::mem::align_of::<Mat4f>()));
            }
            if type_id == TypeId::of::<Quat>() {
                return Some((std::mem::size_of::<Quat>(), std::mem::align_of::<Quat>()));
            }
            if type_id == TypeId::of::<F16x2>() {
                return Some((std::mem::size_of::<F16x2>(), std::mem::align_of::<F16x2>()));
            }
            if type_id == TypeId::of::<F16x4>() {
                return Some((std::mem::size_of::<F16x4>(), std::mem::align_of::<F16x4>()));
            }
            if type_id == TypeId::of::<U16x2>() {
                return Some((std::mem::size_of::<U16x2>(), std::mem::align_of::<U16x2>()));
            }
            if type_id == TypeId::of::<I16x2>() {
                return Some((std::mem::size_of::<I16x2>(), std::mem::align_of::<I16x2>()));
            }
            if type_id == TypeId::of::<UNorm16x2>() {
                return Some((
                    std::mem::size_of::<UNorm16x2>(),
                    std::mem::align_of::<UNorm16x2>(),
                ));
            }
            if type_id == TypeId::of::<SNorm16x2>() {
                return Some((
                    std::mem::size_of::<SNorm16x2>(),
                    std::mem::align_of::<SNorm16x2>(),
                ));
            }
            if type_id == TypeId::of::<UNorm8x4>() {
                return Some((
                    std::mem::size_of::<UNorm8x4>(),
                    std::mem::align_of::<UNorm8x4>(),
                ));
            }
            if type_id == TypeId::of::<SNorm8x4>() {
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

        // First ensure the proto is built so type reflection is available
        Self::script_proto(vm);

        let type_id = Self::script_type_id_static();
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
                std::any::type_name::<Self>(),
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
            std::any::type_name::<Self>(),
            rust_size,
            pod_size
        );

        let pt = vm.bx.heap.new_pod_type(pod_obj, None, pod_ty, NIL);
        vm.bx.heap.set_object_pod_type(pod_obj, pt);
        vm.bx.heap.freeze(pod_obj);

        Some(pt)
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

    fn script_type_id_static() -> ScriptTypeId {
        ScriptTypeId::of::<Self>()
    }
    fn script_new(vm: &mut ScriptVm) -> Self;

    fn script_new_with_default(vm: &mut ScriptVm) -> Self
    where
        Self: Sized,
    {
        let type_id = Self::script_type_id_static();
        if let Some(default_obj) = vm.bx.heap.type_default_for_id(type_id) {
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

    fn script_proto(vm: &mut ScriptVm) -> ScriptValue {
        // The body lives in a non-generic function; this per-type copy only
        // passes the type's functions along.
        script_proto_inner(
            vm,
            Self::script_type_id_static(),
            Self::script_proto_build,
            Self::script_type_check,
            Self::script_type_name,
            Self::is_repr_u32_enum,
        )
    }

    fn script_proto_build(vm: &mut ScriptVm, props: &mut ScriptTypeProps) -> ScriptValue {
        script_proto_build_inner(
            vm,
            props,
            Self::script_proto_props,
            <Self as ScriptHook>::on_proto_build,
            <Self as ScriptHook>::on_proto_methods,
        )
    }

    fn script_proto_props(_vm: &mut ScriptVm, _object: ScriptObject, _props: &mut ScriptTypeProps) {
    }

    fn script_api(vm: &mut ScriptVm) -> ScriptValue {
        let val = Self::script_proto(vm);
        vm.bx.heap.freeze_api(val.into());
        val
    }

    fn script_component(vm: &mut ScriptVm) -> ScriptValue {
        let val = Self::script_proto(vm);
        vm.bx.heap.freeze_component(val.into());
        val
    }

    fn script_shader(vm: &mut ScriptVm) -> ScriptValue {
        let val = Self::script_proto(vm);
        vm.bx.heap.freeze_shader(val.into());
        val
    }

    fn script_ext(vm: &mut ScriptVm) -> ScriptValue {
        let val = Self::script_proto(vm);
        vm.bx.heap.freeze_ext(val.into());
        val
    }

    fn script_enum_lookup_variant(vm: &mut ScriptVm, variant: LiveId) -> ScriptValue {
        let rt = vm
            .bx
            .heap
            .registered_type(Self::script_type_id_static())
            .unwrap();
        let obj = rt.object.as_ref().unwrap().proto.into();
        vm.bx
            .heap
            .value(obj, variant.into(), vm.bx.threads.cur_ref().trap.pass())
    }
}

pub trait ScriptApply {
    fn script_type_id(&self) -> ScriptTypeId
    where
        Self: 'static,
    {
        ScriptTypeId::of::<Self>()
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
