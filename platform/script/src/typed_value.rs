//! Typed values from data: a value of a registered script type, built from a syntax tree
//! without evaluating anything.
//!
//! An edit that arrives from outside the app (a collaborator changing a property of a running
//! app) must never run anything in it. [`ScriptVm::build_typed`] reads the value's syntax tree
//! ([`crate::syntax::parse_value`]) against the declared type of the property and builds the
//! value directly:
//! - a scalar (number, string, colour, `true`, `false`, `nil`, `@id`) passes when the declared
//!   type's own check accepts it; a `theme.*` path reads a scalar out of `mod.theme`;
//! - `vec2f(..)`, `vec3f(..)` and `vec4f(..)` build float vectors from numeric literals;
//! - `X.Y`, a bare `Y`, `X.Y{..}` and `X.Y(..)` name a variant of the declared enum `X`, read
//!   from that enum's own API object, never from a scope. A named variant's fields and a tuple
//!   variant's arguments are built against their own declared types, the same way; a tuple
//!   variant's constructor is never called: its object is built the way the derive builds it;
//! - `{..}` or `X{..}` builds the declared struct `X`, field by field;
//! - `[a, b, ..]` builds an array for a declared `Vec<T>`. Each element is built against `T`,
//!   including when it contains further typed fields or arrays;
//! - a draw shader's input declared in script (`color: instance(#fff)`, `uniform(1.0)`), which
//!   no Rust type declares, takes the kind it holds (number, colour or float vector of the
//!   same size). A four-component float vector also takes a colour.
//!
//! Anything else is refused with a [`TypedRefusal`].
//!
//! Invariant: every object in the output is either a frozen variant object of a declared enum,
//! or one this builder allocated whose proto is a registered type's proto or one of its
//! variants. Every array is freshly allocated and contains values built against its declared
//! element type. Every leaf is a scalar or a vector of floats. No path reads a name from a scope,
//! calls a function, or returns or stores a function.

use crate::colorhex::hex_bytes_to_u32;
use crate::pod::ScriptPodVec;
use crate::syntax::{Field, Kind, Node};
use crate::vm_api::ScriptTextOut;
use crate::*;
use relax_api::relax_api;

/// Why a value was not built.
#[derive(Clone, Debug, PartialEq)]
pub enum TypedRefusal {
    /// Not data: a call, an expression, a function, a name looked up in a scope.
    NotData(String),
    /// The property has no declared type here (a named child, a key the type does not have).
    UnknownProp(String),
    /// A value the declared type does not accept.
    WrongType(String),
    /// `X.Y` where `Y` is not one of the variants of `X`.
    UnknownVariant(String),
    /// Data this builder cannot build.
    Unsupported(String),
    /// The VM refused an allocation while building the value.
    AllocationLimit(String),
}

impl std::fmt::Display for TypedRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotData(why) => write!(f, "not a data value: {why}"),
            Self::UnknownProp(why) => write!(f, "no such property: {why}"),
            Self::WrongType(why) => write!(f, "wrong type: {why}"),
            Self::UnknownVariant(why) => write!(f, "unknown variant: {why}"),
            Self::Unsupported(why) => write!(f, "not supported: {why}"),
            Self::AllocationLimit(why) => write!(f, "allocation limit: {why}"),
        }
    }
}

/// What the builder needs of a registered type.
struct TypeView {
    id: ScriptTypeId,
    check: ScriptTypeCheckFn,
    proto: ScriptValue,
    name: Option<LiveId>,
    /// Declared fields: a struct, or a named variant.
    fields: bool,
}

/// What an edit writes into.
#[derive(Clone, Copy)]
enum Slot {
    /// A declared field, built against its type.
    Typed(ScriptTypeId),
    /// A draw shader's input declared in script (`color: instance(#fff)`, `uniform(1.0)`),
    /// identified by the kind of value it holds.
    Input(InputKind),
}

/// The kind of value held by a script-declared shader input.
#[derive(Clone, Copy, PartialEq, Eq)]
enum InputKind {
    Number,
    Colour,
    Vec2f,
    Vec3f,
    Vec4f,
}

/// How a variant was written.
enum Shape<'n> {
    Bare,
    Tuple(&'n [Node]),
    Fields(&'n [Field]),
}

/// The stack a build may use: as the parser does (`syntax::parse`), half of what is left where
/// the build starts. A value nests no deeper than its declared type, so only a recursive type
/// gets near it; a value that would go deeper is refused, never a stack overflow.
struct Descent<'a> {
    src: &'a str,
    origin: usize,
    budget: usize,
}

impl<'a> Descent<'a> {
    fn here(src: &'a str) -> Self {
        let marker = 0u8;
        let origin = std::hint::black_box(&marker) as *const u8 as usize;
        Self { src, origin, budget: makepad_stack_budget::native_stack_budget(origin) }
    }

    fn check(&self) -> Result<(), TypedRefusal> {
        let marker = 0u8;
        let here = std::hint::black_box(&marker) as *const u8 as usize;
        if self.origin.abs_diff(here) >= self.budget {
            return Err(TypedRefusal::Unsupported("a value nested too deeply to build".into()));
        }
        Ok(())
    }
}

// The ops a JIT-compiled caller reaches the builder through. The builder reads the heap's own
// tables (type checks, objects, the tuple-args table), whose bytes are rustc's, so it runs on the
// provided side whole. No owned string crosses: a refusal comes back through a `ScriptTextOut`.
#[relax_api(run)]
impl ScriptVm<'_> {
    /// [`Self::build_typed_edit`] from the value's text, read as data (`syntax::parse_value`):
    /// the edit's container, or NIL with the refusal written to `why`.
    pub fn typed_edit_text_into(&mut self, target: ScriptObject, path: &[LiveId], text: &str, why: &mut ScriptTextOut) -> ScriptValue {
        let built = match crate::syntax::parse_value(text) {
            Ok(node) => self.build_typed_edit(target, path, text, &node).map_err(|refusal| refusal.to_string()),
            Err(error) => Err(format!("not a value: {error:?}")),
        };
        match built {
            Ok(container) => container.into(),
            Err(refusal) => {
                why.push(&refusal);
                NIL
            }
        }
    }

    /// [`Self::typed_edit_with`] as an op: the edit's container, or NIL with the refusal in `why`.
    pub fn typed_edit_value_into(&mut self, target: ScriptObject, path: &[LiveId], value: ScriptValue, why: &mut ScriptTextOut) -> ScriptValue {
        match self.typed_edit_with(target, path, value) {
            Ok(container) => container.into(),
            Err(refusal) => {
                why.push(&refusal.to_string());
                NIL
            }
        }
    }
}

impl ScriptVm<'_> {
    /// A typed edit of `path` on `target` from the value's text ([`Self::typed_edit_text_into`]):
    /// the edit's container, or why it was refused.
    #[inline(always)]
    pub fn typed_edit_text(&mut self, target: ScriptObject, path: &[LiveId], text: &str) -> Result<ScriptObject, String> {
        let mut why = String::new();
        let container = self.typed_edit_text_into(target, path, text, &mut ScriptTextOut::new(&mut why));
        container.as_object().ok_or(why)
    }

    /// A typed edit of `path` on `target` to a value already in hand ([`Self::typed_edit_value_into`]).
    #[inline(always)]
    pub fn typed_edit_value(&mut self, target: ScriptObject, path: &[LiveId], value: ScriptValue) -> Result<ScriptObject, String> {
        let mut why = String::new();
        let container = self.typed_edit_value_into(target, path, value, &mut ScriptTextOut::new(&mut why));
        container.as_object().ok_or(why)
    }
}

fn text(src: &str, node: &Node) -> String {
    node.text(src).chars().take(80).collect()
}

impl ScriptVm<'_> {
    fn check_allocation(&mut self) -> Result<(), TypedRefusal> {
        if self.heap().allocation_error_pending {
            let message = if self.heap().in_vm_entry() {
                // The enclosing entry still needs to deliver this refusal.
                self.heap().peek_allocation_error().unwrap_or("script allocation limit exceeded").to_owned()
            } else {
                match self.heap_mut().take_allocation_error() {
                    Some(AllocationError::Limit(message)) => message,
                    Some(AllocationError::Memory) => self.heap().peek_allocation_error().unwrap_or("out of memory").to_owned(),
                    None => "script allocation limit exceeded".into(),
                }
            };
            return Err(TypedRefusal::AllocationLimit(message));
        }
        // Raw host allocations record physical exhaustion outside a VM entry.
        if !self.heap().in_vm_entry() {
            if let Some(refusal) = self.heap_mut().take_memory_refusal() {
                return Err(TypedRefusal::AllocationLimit(refusal.reason.into()));
            }
        }
        Ok(())
    }

    /// A value of the registered type `ty`, built from the data tree `node` parsed from `src`.
    /// Never evaluates, never calls.
    pub fn build_typed(&mut self, ty: ScriptTypeId, src: &str, node: &Node) -> Result<ScriptValue, TypedRefusal> {
        self.typed_build(ty, node, &Descent::here(src))
    }

    fn typed_build(&mut self, ty: ScriptTypeId, node: &Node, descent: &Descent<'_>) -> Result<ScriptValue, TypedRefusal> {
        descent.check()?;
        let src = descent.src;
        let view = self.type_view(ty)?;
        match &node.kind {
            Kind::Error => Err(TypedRefusal::Unsupported("a value nested too deeply to read".into())),
            Kind::Number(_) | Kind::Str(_) | Kind::Color(_) | Kind::Name(_) | Kind::Unary { .. } => {
                let value = self.typed_scalar(src, node)?;
                self.typed_check(&view, value, src, node)
            }
            Kind::Ident(word) if matches!(word.as_str(), "true" | "false" | "nil") => {
                let value = self.typed_scalar(src, node)?;
                self.typed_check(&view, value, src, node)
            }
            Kind::Ident(path) if path.starts_with("theme.") => {
                let value = self.theme_scalar(path)?;
                self.typed_check(&view, value, src, node)
            }
            Kind::Ident(written) => self.typed_variant(&view, written, Shape::Bare, descent),
            Kind::Call { callee, args, .. } => match &callee.kind {
                Kind::Ident(written) => match self.typed_vector(written, src, args)? {
                    Some(value) => self.typed_check(&view, value, src, node),
                    None => self.typed_variant(&view, written, Shape::Tuple(args), descent),
                },
                _ => Err(TypedRefusal::NotData(text(src, node))),
            },
            Kind::Object { ty: Some(written), fields, .. } if written.contains('.') || !view.fields => {
                self.typed_variant(&view, written, Shape::Fields(fields), descent)
            }
            Kind::Object { ty: written, fields, .. } => {
                if let Some(written) = written {
                    if view.name != Some(LiveId::from_str(written)) {
                        return Err(TypedRefusal::WrongType(format!("`{written}` is not the declared type")));
                    }
                }
                let Some(proto) = view.proto.as_object().filter(|_| view.fields) else {
                    return Err(TypedRefusal::WrongType(format!("{} has no fields", self.type_label(&view))));
                };
                let object = self.heap_mut().new_with_proto(proto.into());
                self.check_allocation()?;
                self.typed_fields(object, fields, descent)?;
                self.typed_check(&view, object.into(), src, node)
            }
            Kind::Array { items, .. } => {
                let element_ty = self.heap().vec_element_type(ty)
                    .ok_or_else(|| TypedRefusal::WrongType(format!("{} does not take an array", self.type_label(&view))))?;
                let array = self.heap_mut().new_array();
                self.check_allocation()?;
                for item in items {
                    let value = self.typed_build(element_ty, item, descent)?;
                    self.heap_mut().array_push(array, value, NoTrap);
                    self.check_allocation()?;
                }
                // Allocation limits can refuse the array itself or a push.
                let built = self.heap().arr_view(array);
                if built.tag.is_immutable() || built.len_all() != items.len() {
                    return Err(TypedRefusal::Unsupported("could not allocate the complete array".into()));
                }
                Ok(array.into())
            }
            _ => Err(TypedRefusal::NotData(text(src, node))),
        }
    }

    /// `new_with_proto(target)` holding only `path: value`, with the value built against the
    /// declared type of `path` on `target`: what `__script_source__{a.b: value}` evaluates to.
    /// `node` is parsed from `src`. Each parent segment is the instance copy the VM's own `a.b: value` builds on.
    pub fn build_typed_edit(&mut self, target: ScriptObject, path: &[LiveId], src: &str, node: &Node) -> Result<ScriptObject, TypedRefusal> {
        let (container, at, last, slot) = self.typed_edit_slot(target, path)?;
        let value = match slot {
            Slot::Typed(ty) => {
                let heap = self.heap();
                let current = heap.value(at, last.into(), NoTrap).as_object();
                let is_draw = |obj| heap.is_draw_object(obj);
                if current.is_some_and(is_draw) || heap.type_default_for_id(ty).is_some_and(is_draw) {
                    let why = format!("{last} is a draw shader: set its inputs one by one");
                    return Err(match node.kind {
                        Kind::Array { .. } => TypedRefusal::WrongType(why),
                        _ => TypedRefusal::Unsupported(why),
                    });
                }
                self.build_typed(ty, src, node)?
            }
            Slot::Input(_) if matches!(node.kind, Kind::Array { .. }) => {
                return Err(TypedRefusal::WrongType(format!("{last} does not take an array")));
            }
            Slot::Input(kind) => {
                let value = if let Some(path) = node.ident().filter(|path| path.starts_with("theme.")) {
                    self.theme_scalar(path)?
                } else if let Some((name, args)) = node.as_call() {
                    self.typed_vector(name, src, args)?.ok_or_else(|| TypedRefusal::NotData(text(src, node)))?
                } else {
                    self.typed_scalar(src, node)?
                };
                self.input_takes(kind, value, last)?;
                value
            }
        };
        self.typed_edit_store(at, last, value)?;
        Ok(container)
    }

    /// [`Self::build_typed_edit`] with the value already in hand, checked against the declared
    /// type the same way: a property's own captured value, put back after a failed edit.
    pub fn typed_edit_with(&mut self, target: ScriptObject, path: &[LiveId], value: ScriptValue) -> Result<ScriptObject, TypedRefusal> {
        let (container, at, last, slot) = self.typed_edit_slot(target, path)?;
        match slot {
            Slot::Typed(ty) => {
                let view = self.type_view(ty)?;
                if !(view.check)(self.heap(), value) {
                    return Err(TypedRefusal::WrongType(format!("{last} does not take the value it had")));
                }
            }
            Slot::Input(kind) => self.input_takes(kind, value, last)?,
        }
        self.typed_edit_store(at, last, value)?;
        Ok(container)
    }

    /// The edit's container, the object that takes the last segment, that segment and its slot.
    fn typed_edit_slot(&mut self, target: ScriptObject, path: &[LiveId]) -> Result<(ScriptObject, ScriptObject, LiveId, Slot), TypedRefusal> {
        let Some((last, parents)) = path.split_last() else {
            return Err(TypedRefusal::UnknownProp("an empty path".into()));
        };
        // The proto `eval_body` gives `__script_source__`: an evaluated source stands for its proto.
        let target = match self.heap().is_from_eval(target) {
            true => self.heap().proto(target).as_object().unwrap_or(target),
            false => target,
        };
        let container = self.heap_mut().new_with_proto(target.into());
        self.check_allocation()?;
        self.heap_mut().set_from_eval(container);
        let mut at = container;
        for segment in parents {
            // Only into a declared struct: never a copy of a function, a variant or a plain value.
            let Some(ty) = self.heap().field_type_from_type_check(at, *segment) else {
                return Err(TypedRefusal::UnknownProp(format!("{segment}")));
            };
            if !self.type_view(ty).is_ok_and(|view| view.fields) {
                return Err(TypedRefusal::UnknownProp(format!("{segment} is not a struct")));
            }
            let mut sub = self.heap_mut().proto_field_from_value(at, (*segment).into(), NoTrap);
            self.check_allocation()?;
            if sub.is_nil() || sub.is_err() {
                sub = self.heap_mut().proto_field_from_type_check(at, *segment, NoTrap);
                self.check_allocation()?;
            }
            at = sub.as_object().ok_or_else(|| TypedRefusal::UnknownProp(format!("{segment} has no value to edit (no type default)")))?;
        }
        if let Some(ty) = self.heap().field_type_from_type_check(at, *last) {
            return Ok((container, at, *last, Slot::Typed(ty)));
        }
        match self.shader_input(at, *last) {
            Some(Some(kind)) => Ok((container, at, *last, Slot::Input(kind))),
            Some(None) => Err(TypedRefusal::Unsupported(format!("{last} is a shader input of an unsupported kind"))),
            None => Err(TypedRefusal::UnknownProp(format!("{last}"))),
        }
    }

    /// Whether `key` is a script-declared instance or uniform input, using the highest marker
    /// in the proto chain as the shader compiler does (`find_highest_shader_io`). For an input,
    /// report the kind of value it holds, or `Some(None)` for an unsupported value.
    fn shader_input(&self, at: ScriptObject, key: LiveId) -> Option<Option<InputKind>> {
        let heap = self.heap();
        let mut highest = None;
        let mut current = Some(at);
        while let Some(object) = current {
            if let Some(io) = heap.map_own_value(object, key.into()).and_then(|own| own.as_object()).and_then(|own| heap.as_shader_io(own)) {
                highest = Some(io);
            }
            current = heap.proto(object).as_object();
        }
        if !matches!(highest, Some(crate::mod_shader::SHADER_IO_DYN_INSTANCE | crate::mod_shader::SHADER_IO_DYN_UNIFORM)) {
            return None;
        }
        let mut value = heap.value(at, key.into(), NoTrap);
        if let Some(input) = value.as_object().filter(|object| heap.as_shader_io(*object).is_some()) {
            value = heap.proto(input);
        }
        Some(self.input_kind(value))
    }

    fn input_kind(&self, value: ScriptValue) -> Option<InputKind> {
        if value.is_color() {
            return Some(InputKind::Colour);
        }
        if value.as_number().is_some() {
            return Some(InputKind::Number);
        }
        let pod = value.as_pod()?;
        match self.heap().pod_data(pod).0.ty().as_vec()? {
            ScriptPodVec::Vec2f => Some(InputKind::Vec2f),
            ScriptPodVec::Vec3f => Some(InputKind::Vec3f),
            ScriptPodVec::Vec4f => Some(InputKind::Vec4f),
            _ => None,
        }
    }

    fn input_takes(&self, kind: InputKind, value: ScriptValue, key: LiveId) -> Result<(), TypedRefusal> {
        let actual = self.input_kind(value);
        if actual == Some(kind) || (kind == InputKind::Vec4f && actual == Some(InputKind::Colour)) {
            return Ok(());
        }
        let takes = match kind {
            InputKind::Number => "a number",
            InputKind::Colour => "a colour",
            InputKind::Vec2f => "vec2f(..)",
            InputKind::Vec3f => "vec3f(..)",
            InputKind::Vec4f => "a colour or vec4f(..)",
        };
        Err(TypedRefusal::WrongType(format!("{key} takes {takes}")))
    }

    fn typed_edit_store(&mut self, at: ScriptObject, last: LiveId, value: ScriptValue) -> Result<(), TypedRefusal> {
        let stored = self.heap_mut().set_value(at, last.into(), value, NoTrap);
        self.check_allocation()?;
        if stored.is_err() {
            return Err(TypedRefusal::WrongType(format!("{last} does not take that value")));
        }
        Ok(())
    }

    fn type_view(&self, ty: ScriptTypeId) -> Result<TypeView, TypedRefusal> {
        let check = self.heap().registered_type(ty).ok_or_else(|| TypedRefusal::Unsupported("an unregistered type".into()))?;
        let object = check.object.as_ref().ok_or_else(|| TypedRefusal::Unsupported("a type without a registration".into()))?;
        Ok(TypeView { id: ty, check: object.check, proto: object.proto, name: object.name, fields: !check.props.props.is_empty() })
    }

    fn type_label(&self, view: &TypeView) -> String {
        view.name.map_or_else(|| "the declared type".into(), |name| format!("{name}"))
    }

    /// The declared type's own check, the one a typed `set_value` runs.
    fn typed_check(&self, view: &TypeView, value: ScriptValue, src: &str, node: &Node) -> Result<ScriptValue, TypedRefusal> {
        if (view.check)(self.heap(), value) {
            Ok(value)
        } else {
            Err(TypedRefusal::WrongType(format!("{} does not take {}", self.type_label(view), text(src, node))))
        }
    }

    /// Build a float vector directly from numeric literals. Other names fall through to the caller.
    fn typed_vector(&mut self, name: &str, src: &str, args: &[Node]) -> Result<Option<ScriptValue>, TypedRefusal> {
        let (n, ty) = match name {
            "vec2f" => (2, self.pod_builtins().pod_vec2f),
            "vec3f" => (3, self.pod_builtins().pod_vec3f),
            "vec4f" => (4, self.pod_builtins().pod_vec4f),
            _ => return Ok(None),
        };
        if args.len() != n {
            return Err(TypedRefusal::WrongType(format!("{name} takes {n} numbers, separated by commas")));
        }
        let mut words = [0; 4];
        for (index, arg) in args.iter().enumerate() {
            let number = self.typed_scalar(src, arg)?.as_number().ok_or_else(|| TypedRefusal::NotData(text(src, arg)))?;
            words[index] = (number as f32).to_bits();
        }
        let pod = self.heap_mut().new_pod(ty);
        self.check_allocation()?;
        self.heap_mut().pod_data_mut(pod)[..n].copy_from_slice(&words[..n]);
        Ok(Some(pod.into()))
    }

    fn typed_scalar(&mut self, src: &str, node: &Node) -> Result<ScriptValue, TypedRefusal> {
        Ok(match &node.kind {
            Kind::Number(n) => (*n).into(),
            Kind::Unary { op, operand } if op == "-" => match operand.kind {
                Kind::Number(n) => (-n).into(),
                _ => return Err(TypedRefusal::NotData(text(src, node))),
            },
            Kind::Str(s) => {
                let value = self.heap_mut().new_string_from_str(s);
                self.check_allocation()?;
                value
            }
            Kind::Color(hex) => ScriptValue::from_color(
                hex_bytes_to_u32(hex.as_bytes()).map_err(|_| TypedRefusal::NotData(format!("#{hex}")))?,
            ),
            Kind::Name(id) => LiveId::from_str(id.trim_start_matches('@')).into(),
            Kind::Ident(word) if word == "true" => true.into(),
            Kind::Ident(word) if word == "false" => false.into(),
            Kind::Ident(word) if word == "nil" => NIL,
            _ => return Err(TypedRefusal::NotData(text(src, node))),
        })
    }

    /// A scalar read out of `mod.theme`, segment by segment. Never an object: a theme entry
    /// that holds one (a function among them) is refused.
    fn theme_scalar(&mut self, path: &str) -> Result<ScriptValue, TypedRefusal> {
        // A heap read: a lazy module is not expanded for an edit.
        let mut value: ScriptValue = self.heap().module(id!(theme)).into();
        if value.as_object().is_none_or(|theme| theme == ScriptObject::ZERO) {
            return Err(TypedRefusal::UnknownProp(path.into()));
        }
        for segment in path.split('.').skip(1) {
            let Some(object) = value.as_object() else {
                return Err(TypedRefusal::UnknownProp(path.into()));
            };
            value = self.heap().value(object, LiveId::from_str(segment).into(), NoTrap);
        }
        if value.is_err() || value.is_nil() || value.as_object().is_some() || value.as_array().is_some() {
            return Err(TypedRefusal::NotData(format!("{path} is not a theme value")));
        }
        Ok(value)
    }

    /// `X.Y`, `Y`, `X.Y(..)` or `X.Y{..}`: a variant of the declared enum, read from its API.
    fn typed_variant(&mut self, view: &TypeView, written: &str, shape: Shape, descent: &Descent<'_>) -> Result<ScriptValue, TypedRefusal> {
        let (head, variant) = match written.split_once('.') {
            Some((head, variant)) => (Some(head), variant),
            None => (None, written),
        };
        if variant.contains('.') {
            return Err(TypedRefusal::NotData(format!("`{written}` is a path, not a variant")));
        }
        let (Some(name), Some(api)) = (view.name, view.proto.as_object().filter(|api| !self.heap().is_fn(*api))) else {
            return Err(TypedRefusal::WrongType(format!("`{written}` for a type without variants")));
        };
        if head.is_some_and(|head| LiveId::from_str(head) != name) {
            return Err(TypedRefusal::WrongType(format!("`{written}` is not a {name}")));
        }
        let key = LiveId::from_str(variant);
        let unknown = || TypedRefusal::UnknownVariant(format!("{variant} is not a {name}"));
        let entry = self.heap().value(api, key.into(), NoTrap).as_object().ok_or_else(unknown)?;
        if self.heap().is_fn(entry) {
            // A tuple variant's constructor. Never called: the variant object is built the way
            // the derive builds it, each argument against the type the derive recorded for it.
            let Shape::Tuple(args) = shape else {
                return Err(TypedRefusal::WrongType(format!("{name}.{variant} takes arguments")));
            };
            // The table is rustc's: read through the heap's ops, never its field.
            let Some(count) = self.heap().tuple_arg_count(view.id, key) else {
                return Err(unknown());
            };
            if count != args.len() {
                return Err(TypedRefusal::WrongType(format!("{name}.{variant} takes {count} arguments")));
            }
            let mut values = Vec::with_capacity(args.len());
            for (index, arg) in args.iter().enumerate() {
                let ty = self.heap().tuple_arg_type(view.id, key, index).ok_or_else(unknown)?;
                values.push(self.typed_build(ty, arg, descent)?);
            }
            let tuple = self.heap_mut().new_with_proto(key.into());
            self.check_allocation()?;
            self.heap_mut().set_value(tuple, id!(__enum).into(), name.into(), NoTrap);
            self.check_allocation()?;
            for value in values {
                self.heap_mut().vec_push(tuple, NIL, value, NoTrap);
                self.check_allocation()?;
            }
            return if (view.check)(self.heap(), tuple.into()) { Ok(tuple.into()) } else { Err(unknown()) };
        }
        let heap = self.heap();
        if heap.root_proto(entry).as_id() != Some(key) || heap.value(entry, id!(__enum).into(), NoTrap).as_id() != Some(name) {
            return Err(unknown());
        }
        let named = heap.objects[entry].tag.as_type_index().is_some();
        match shape {
            Shape::Bare => Ok(entry.into()),
            Shape::Fields(fields) if named => {
                let object = self.heap_mut().new_with_proto(entry.into());
                self.check_allocation()?;
                self.typed_fields(object, fields, descent)?;
                if (view.check)(self.heap(), object.into()) { Ok(object.into()) } else { Err(unknown()) }
            }
            Shape::Fields(_) => Err(TypedRefusal::WrongType(format!("{name}.{variant} has no fields"))),
            Shape::Tuple(_) => Err(TypedRefusal::WrongType(format!("{name}.{variant} takes no arguments"))),
        }
    }

    /// `name: value` fields onto a typed object, each built against its declared type.
    fn typed_fields(&mut self, object: ScriptObject, fields: &[Field], descent: &Descent<'_>) -> Result<(), TypedRefusal> {
        for field in fields {
            let (Some(name), ":") = (&field.name, field.op.as_str()) else {
                return Err(TypedRefusal::NotData("only `name: value` fields".into()));
            };
            let key = LiveId::from_str(name);
            let ty = self.heap().field_type_from_type_check(object, key)
                .ok_or_else(|| TypedRefusal::UnknownProp(name.clone()))?;
            let value = self.typed_build(ty, &field.value, descent)?;
            let stored = self.heap_mut().set_value(object, key.into(), value, NoTrap);
            self.check_allocation()?;
            if stored.is_err() {
                return Err(TypedRefusal::WrongType(format!("{name} does not take {}", text(descent.src, &field.value))));
            }
        }
        Ok(())
    }
}
