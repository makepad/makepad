//! Immutable document metadata shared by loaders, writers, hosts and runs.
//! Build a registry before sharing it in an `Arc`; runtime executor factories
//! are registered separately and never travel in this serializable snapshot.

use super::types::{TypeRegistry, TypeSchema};
use crate::{Literal, Port, PortType, MAX_SEQUENCE};
use makepad_micro_serde::*;
use std::collections::{BTreeMap, HashSet};

pub const BUILTIN_OWNER: &str = "core.jobs";
const MAX_TYPES: usize = 512;
const MAX_DESCRIPTORS: usize = 256;
const MAX_FIELDS: usize = 64;
const MAX_TEXT: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, SerJson, DeJson)]
pub enum ParamType {
    String, Literal, Number, NonNegativeInteger, PortTypeNoBytes,
    PortTypeWithBytes, HttpOut, HttpMethod, LiteralArray, FanIn,
}

/// How a parameter may be written while its node runs in a live partition.
/// Absent, a parameter is authored: changing it is a source edit and a new
/// revision. Anything else is an endpoint a prepared plan can admit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, SerJson, DeJson)]
pub enum EndpointClass {
    /// A finite number inside the parameter's `range`.
    Control,
    /// `0` or `1`.
    Toggle,
    /// An ordered edge (a note, a reset): never coalesced, refusable.
    Trigger,
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct ParamDescriptor {
    pub name: String,
    pub expected: ParamType,
    pub default: Literal,
    /// What the parameter does, for the catalog, the inspector and tools.
    pub doc: Option<String>,
    /// `(min, max, step)`; a step of `1` makes the parameter an integer.
    pub range: Option<(f64, f64, Option<f64>)>,
    pub endpoint: Option<EndpointClass>,
    /// The scalar signal type a wire into this control or toggle carries.
    /// `Some` makes the parameter also a real, same-named node input: the
    /// authored literal stays the fallback, a wire in `in: {..}` drives it.
    /// Scalar values are range/step normalized; toggles switch at 0.5.
    /// Non-finite signals hold the last effective value.
    /// `None` (authored-only parameters, triggers) is a parameter only.
    pub signal: Option<PortType>,
}

impl ParamDescriptor {
    pub fn authored(name: impl Into<String>, expected: ParamType, default: Literal, doc: &str) -> Self {
        Self { name: name.into(), expected, default, doc: Some(doc.into()), range: None, endpoint: None, signal: None }
    }

    pub fn control(name: impl Into<String>, default: f64, min: f64, max: f64, doc: &str) -> Self {
        Self {
            name: name.into(), expected: ParamType::Number, default: Literal::Num(default),
            doc: Some(doc.into()), range: Some((min, max, None)), endpoint: Some(EndpointClass::Control), signal: None,
        }
    }

    pub fn toggle(name: impl Into<String>, default: bool, doc: &str) -> Self {
        Self {
            name: name.into(), expected: ParamType::Number, default: Literal::Num(if default { 1.0 } else { 0.0 }),
            doc: Some(doc.into()), range: Some((0.0, 1.0, Some(1.0))), endpoint: Some(EndpointClass::Toggle), signal: None,
        }
    }

    /// This control or toggle also takes a wire of the scalar type `ty`.
    pub fn with_signal(mut self, ty: PortType) -> Self {
        self.signal = Some(ty);
        self
    }
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct InputDescriptor {
    pub name: String,
    pub ty: PortType,
    pub many: Option<u32>,
    pub default: Literal,
    pub flexible: bool,
}

/// Builtin language policies. Provider nodes use fixed descriptor fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq, SerJson, DeJson)]
pub enum NodeCodec { Fields, Input, Output, Fn, Http, Ask, Gen, Map, Choose }

/// Canonical field ordering and default suppression are descriptor data.
#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub enum WriteField {
    Param { name: String, force: bool },
    Input { name: String, force: bool },
    Ports, Domain, Params, Inputs, FnBody,
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct TypeDescriptor {
    pub type_name: String,
    pub kind: String,
    pub owner: String,
    pub params: Vec<ParamDescriptor>,
    pub inputs: Vec<InputDescriptor>,
    pub outputs: Vec<Port>,
    pub codec: NodeCodec,
    pub write: Vec<WriteField>,
    pub domain: Option<String>,
    pub face: String,
    pub doc: String,
}

impl TypeDescriptor {
    /// The control inputs: one per parameter with `signal`, named as the
    /// parameter, one wire at most, unwired (`null`) by default. They are not
    /// registered as `inputs`; the parameter is their one declaration.
    pub fn control_inputs(&self) -> impl Iterator<Item = InputDescriptor> + '_ {
        self.params.iter().filter_map(|param| param.signal.clone().map(|ty| InputDescriptor {
            name: param.name.clone(), ty, many: None, default: Literal::Null, flexible: false,
        }))
    }

    /// Declared inputs followed by the control inputs.
    pub fn all_inputs(&self) -> Vec<InputDescriptor> {
        self.inputs.iter().cloned().chain(self.control_inputs()).collect()
    }

    /// The parameter behind control input `name`, if it is one.
    pub fn control_input(&self, name: &str) -> Option<&ParamDescriptor> {
        self.params.iter().find(|param| param.name == name && param.signal.is_some())
    }

    pub fn has_control_inputs(&self) -> bool {
        self.params.iter().any(|param| param.signal.is_some())
    }

    pub fn provider(type_name: impl Into<String>, owner: impl Into<String>, inputs: Vec<InputDescriptor>, outputs: Vec<Port>) -> Self {
        Self {
            type_name: type_name.into(), kind: "provider".into(), owner: owner.into(),
            params: Vec::new(), inputs, outputs, codec: NodeCodec::Fields,
            write: vec![WriteField::Params, WriteField::Inputs],
            domain: None, face: String::new(), doc: String::new(),
        }
    }
}

/// A complete registry snapshot, including every builtin schema and node
/// descriptor. This is not a patch: omissions, duplicates and changes to the
/// local builtin baseline are rejected by `Registry::from_data`.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct RegistryData {
    pub types: Vec<TypeSchema>,
    pub descriptors: Vec<TypeDescriptor>,
}

#[derive(Clone, Debug)]
pub struct Registry {
    types: TypeRegistry,
    descriptors: BTreeMap<String, TypeDescriptor>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistryError(pub String);
impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.0) }
}
impl std::error::Error for RegistryError {}

impl Default for Registry { fn default() -> Self { Self::with_builtins() } }

impl Registry {
    /// The shipped builtins. They come from the compiled-in preludes alone,
    /// so they are read once per process: reading them evaluates both
    /// preludes in a fresh VM and resolves every doc comment, and every cut
    /// or deck load asked for them again (a DECK slide reorder spent ~75% of
    /// its edit in this, once per slide, twice per edit).
    pub fn with_builtins() -> Self {
        static BUILTINS: std::sync::OnceLock<Registry> = std::sync::OnceLock::new();
        BUILTINS.get_or_init(|| {
            let types = TypeRegistry::with_builtins();
            let descriptors = crate::graph::builtin_descriptors(&types)
                .expect("the shipped flow prelude must have valid descriptors")
                .into_iter().map(|descriptor| (descriptor.type_name.clone(), descriptor)).collect();
            Self { types, descriptors }
        }).clone()
    }

    pub fn from_data(data: RegistryData) -> Result<Self, RegistryError> {
        if data.types.len() > MAX_TYPES || data.descriptors.len() > MAX_DESCRIPTORS {
            return Err(RegistryError("registry metadata exceeds its type or descriptor count limit".into()));
        }
        let mut registry = Self::with_builtins();
        let mut types = HashSet::new();
        for schema in &data.types {
            if !types.insert(schema.key.as_str()) {
                return Err(RegistryError(format!("registry snapshot repeats type `{}`", schema.key.as_str())));
            }
        }
        for schema in registry.types.schemas() {
            if !types.contains(schema.key.as_str()) {
                return Err(RegistryError(format!("registry snapshot omits builtin type `{}`", schema.key.as_str())));
            }
        }
        let mut descriptors = HashSet::new();
        for descriptor in &data.descriptors {
            if !descriptors.insert(descriptor.type_name.as_str()) {
                return Err(RegistryError(format!("registry snapshot repeats node type `{}`", descriptor.type_name)));
            }
        }
        for descriptor in registry.descriptors() {
            if !descriptors.contains(descriptor.type_name.as_str()) {
                return Err(RegistryError(format!("registry snapshot omits builtin node type `{}`", descriptor.type_name)));
            }
        }
        for schema in data.types { registry.register_type(schema)?; }
        for descriptor in data.descriptors { registry.register(descriptor)?; }
        Ok(registry)
    }

    pub fn data(&self) -> RegistryData {
        RegistryData { types: self.types.schemas().cloned().collect(), descriptors: self.descriptors.values().cloned().collect() }
    }
    pub fn types(&self) -> &TypeRegistry { &self.types }
    pub fn descriptor(&self, name: &str) -> Option<&TypeDescriptor> { self.descriptors.get(name) }
    pub fn descriptors(&self) -> impl Iterator<Item = &TypeDescriptor> { self.descriptors.values() }
    pub fn register_type(&mut self, schema: TypeSchema) -> Result<(), RegistryError> {
        if schema.doc.len() > MAX_TEXT || (self.types.resolve(schema.key.as_str()).is_err() && self.types.schemas().count() >= MAX_TYPES) {
            return Err(RegistryError("registry type metadata exceeds its count or text limit".into()));
        }
        self.types.register(schema).map_err(|error| RegistryError(error.to_string()))
    }

    pub fn register(&mut self, descriptor: TypeDescriptor) -> Result<(), RegistryError> {
        if descriptor.params.len() > MAX_FIELDS || descriptor.inputs.len() > MAX_FIELDS || descriptor.outputs.len() > MAX_FIELDS
            || descriptor.doc.len() > MAX_TEXT || descriptor.face.len() > 128 || descriptor.write.len() > MAX_FIELDS * 2
        {
            return Err(RegistryError("node descriptor exceeds its field or text limit".into()));
        }
        if let Some(existing) = self.descriptor(&descriptor.type_name) {
            return if existing == &descriptor { Ok(()) } else {
                Err(RegistryError(format!("node type `{}` is already registered differently", descriptor.type_name)))
            };
        }
        if self.descriptors.len() >= MAX_DESCRIPTORS {
            return Err(RegistryError("registry exceeds its descriptor count limit".into()));
        }
        if !identifier(&descriptor.type_name) || matches!(descriptor.type_name.as_str(), "Node" | "Flow" | "Component" | "App") {
            return Err(RegistryError(format!("invalid or reserved node type `{}`", descriptor.type_name)));
        }
        if descriptor.owner.is_empty() || descriptor.owner.len() > 128 || descriptor.owner == BUILTIN_OWNER
            || !descriptor.owner.split('.').all(|part| part.bytes().next().is_some_and(|byte| byte.is_ascii_lowercase())
                && part.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'))
        {
            return Err(RegistryError(format!("invalid or reserved execution owner `{}`", descriptor.owner)));
        }
        if descriptor.kind != "provider" || descriptor.codec != NodeCodec::Fields
            || descriptor.write != [WriteField::Params, WriteField::Inputs] || descriptor.domain.is_some()
        {
            return Err(RegistryError("provider descriptors require kind `provider`, fixed Fields, Params/Inputs writing and no hub domain".into()));
        }
        let mut fields = HashSet::new();
        for name in descriptor.params.iter().map(|param| &param.name).chain(descriptor.inputs.iter().map(|input| &input.name)) {
            if !identifier(name) || reserved(name) || !fields.insert(name) {
                return Err(RegistryError(format!("node `{}` has an invalid, reserved or duplicate field `{name}`", descriptor.type_name)));
            }
        }
        let mut outputs = HashSet::new();
        for output in &descriptor.outputs {
            if !identifier(&output.name) || !outputs.insert(&output.name) {
                return Err(RegistryError(format!("node `{}` has an invalid or duplicate output `{}`", descriptor.type_name, output.name)));
            }
        }
        for (ty, many) in descriptor.inputs.iter().map(|port| (&port.ty, port.many))
            .chain(descriptor.outputs.iter().map(|port| (&port.ty, port.many)))
        {
            self.types.resolve_port(ty).map_err(|error| RegistryError(error.to_string()))?;
            if many.is_some_and(|bound| !(1..=MAX_SEQUENCE).contains(&bound)) {
                return Err(RegistryError(format!("node `{}` has an invalid ordered bound", descriptor.type_name)));
            }
        }
        for param in &descriptor.params {
            validate_default(&param.default)?;
            if param.doc.as_ref().is_some_and(|doc| doc.len() > MAX_TEXT) {
                return Err(RegistryError(format!("parameter `{}` has an oversized description", param.name)));
            }
            if let Some((min, max, step)) = param.range {
                if !min.is_finite() || !max.is_finite() || min > max || step.is_some_and(|step| !step.is_finite() || step <= 0.0) {
                    return Err(RegistryError(format!("parameter `{}` has an invalid range", param.name)));
                }
            }
            match (param.endpoint, param.range, &param.default) {
                (None, _, _) | (Some(EndpointClass::Trigger), _, _) => {}
                (Some(_), Some((min, max, _)), Literal::Num(default)) if (min..=max).contains(default) => {}
                _ => return Err(RegistryError(format!(
                    "endpoint parameter `{}` needs a range and a numeric default inside it", param.name
                ))),
            }
            crate::graph::validate_descriptor_param(param, &param.default, &self.types).map_err(RegistryError)?;
            if let Some(signal) = &param.signal {
                if !matches!(param.endpoint, Some(EndpointClass::Control | EndpointClass::Toggle)) {
                    return Err(RegistryError(format!("only a control or toggle parameter takes a signal; `{}` does not", param.name)));
                }
                let schema = self.types.resolve_port(signal).map_err(|error| RegistryError(error.to_string()))?;
                if schema.carrier != crate::document::types::Carrier::Control {
                    return Err(RegistryError(format!("the signal of `{}` must be a scalar control type", param.name)));
                }
            }
        }
        for input in &descriptor.inputs {
            validate_default(&input.default)?;
            if input.flexible {
                return Err(RegistryError(format!("provider input `{}` must keep its declared type", input.name)));
            }
            match (&input.default, input.many) {
                (Literal::Null, _) => {}
                (Literal::Arr(values), Some(_)) if values.is_empty() => {}
                (_, Some(_)) => return Err(RegistryError(format!("ordered provider input `{}` has a nonempty literal default", input.name))),
                (value, None) => {
                    crate::Value::from_literal(input.ty.clone(), value).map_err(RegistryError)?;
                }
            }
        }
        self.descriptors.insert(descriptor.type_name.clone(), descriptor);
        Ok(())
    }
}

fn identifier(name: &str) -> bool {
    !name.is_empty() && name.len() <= 128
        && name.bytes().next().is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}
fn reserved(name: &str) -> bool {
    matches!(name, "kind" | "type_name" | "at" | "size" | "flip" | "ui" | "on_fail" | "label" | "out" | "port" | "ports" | "domain")
}

fn validate_default(value: &Literal) -> Result<(), RegistryError> {
    fn count(value: &Literal, depth: usize, left: &mut usize) -> bool {
        if depth > 16 || *left == 0 { return false }
        *left -= 1;
        match value {
            Literal::Num(number) => number.is_finite(),
            Literal::Id(text) => identifier(text),
            Literal::Str(text) => {
                if text.len() > *left { return false }
                *left -= text.len(); true
            }
            Literal::Arr(values) => values.iter().all(|value| count(value, depth + 1, left)),
            Literal::Obj(values) => values.iter().all(|(name, value)| {
                if !identifier(name) || name.len() > *left { return false }
                *left -= name.len(); count(value, depth + 1, left)
            }),
            _ => true,
        }
    }
    let mut left = MAX_TEXT;
    if count(value, 0, &mut left) { Ok(()) }
    else { Err(RegistryError("descriptor default exceeds its depth/size limit or contains a nonfinite number".into())) }
}
