//! Shader packs: draw shaders compiled ahead of time. A pack holds, per
//! draw shader, its GLSL ES 3.0 code (what the WebGL2 backend runs) and the
//! compiler's reflection of it (the io with its pod types, samplers, uniform
//! buffer bindings, scope uniforms, table constants: everything the draw
//! path builds the shader's mapping from), serialised into versioned bytes
//! and keyed by a hash of the shader's source.
//!
//! - Recording (native): [`ShaderPacks::record_enabled`] makes every draw shader
//!   that is applied also run the web build's GLSL compile and keep the
//!   result; [`ShaderPacks::take_recording`] serialises it.
//! - Use: [`ShaderPacks::install`]. The web build's draw-shader compile
//!   looks a shader's key up first; a hit deserialises the entry and builds
//!   the mapping from it exactly as a compile's reflection is built
//!   (`CxDrawShaderMapping::from_desc`), with no compile. Built with
//!   `--cfg makepad_precompiled_shaders` the web build does not contain the
//!   shader compiler at all: a miss is logged with the shader's name and
//!   source location and draws nothing.
//!
//! The layouts (offsets, strides, uniform packing) are not in the pack: the
//! mapping computes them from the io for the target it runs on, as after a
//! compile, so one pack serves any target that runs its code.
//!
//! The key names a shader by what its code is made of, never by heap
//! indices or code positions, so one shader has one key in every process,
//! on every target and in any load order:
//!
//! - the pipeline state (`color_format`, `blend_op`) and the const-table
//!   mode;
//! - along the shader object's prototype chain, every field in definition
//!   order: a script function by its tokens (whitespace and plain comments
//!   don't count, `/** */` annotations and `#(…)` values do), a shader io
//!   marker by its io type and pod type, a plain value by its kind (an id or
//!   a bool by its value); the Rust instance fields by name and pod type;
//! - for every function so hashed, each identifier in it that its scope
//!   resolves (and `a.b` through a scope object) to a function, hashed the
//!   same way, or to another value, by kind. Two documents with the same
//!   `pixel` text that calls helpers of different bodies get two keys.
//!
//! Assumed equal between the recording and the binary, and not in the key:
//! code reached only through a type or a value at compile time (a pod
//! type's methods such as `Sdf2d.circle`, library functions behind a local),
//! the values (not the kinds) of scope uniforms, and pod type names. Within
//! one build of the code these hold; record again after changing library
//! shader code.
//!
//! Scope uniforms (values a shader reads from its surrounding scope) are
//! found again in the running heap by a path of names from the shader
//! object (its methods, then scope lookups), which needs only heap lookups.
//! Where recording finds no such path, or the running heap no longer has
//! it, the value recorded with the shader is used.
use makepad_relax_api::relax_api;
use crate::{heap::ScriptHeap, pod::*, shader::*, tokenizer::ScriptTokenTag, value::*, shader_reflection::*, *};
use makepad_live_id::*;
use std::collections::{HashMap, HashSet};
/// Bumped whenever the key's recipe or the pack's layout changes; a pack of
/// another version is refused.
const SHADER_PACK_VERSION: u32 = 2;
const SHADER_PACK_MAGIC: &[u8; 4] = b"MPSP";

/// One draw shader of a pack: its GLSL ES 3.0 code and reflection.
#[relax_api(handle, eq)]
#[derive(Clone, Debug, PartialEq)]
pub struct ShaderPackEntry {
    /// [`shader_pack_key`] of the shader objects it serves.
    pub key: u64,
    /// The shader's type name, for diagnostics.
    pub name: String,
    pub vertex: String,
    pub fragment: String,
    /// The pod types `io` refers to by index.
    pub pod_types: Vec<PackPodType>,
    /// The io in the compiler's order.
    pub io: Vec<PackIo>,
    pub samplers: Vec<ShaderSampler>,
    /// In io order.
    pub scope_uniforms: Vec<PackScopeUniform>,
    pub table_consts: Vec<PackTableConst>,
    /// (pod type name, buffer index) of each uniform buffer.
    pub uniform_buffer_bindings: Vec<(u64, u32)>,
    pub scope_uniform_buffer_index: Option<u32>,
    /// The shader reads `draw_pass.time`.
    pub uses_time: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PackIo {
    pub kind: PackIoKind,
    pub name: u64,
    /// Index into the entry's `pod_types`.
    pub ty: u32,
    pub buffer_index: Option<u32>,
    /// Uniform buffers: the block name the code declares.
    pub block_name: String,
    /// Textures: the sampler the code samples it with.
    pub sampler: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PackIoKind {
    StorageBuffer { read: bool, write: bool },
    UniformBuffer,
    Sampler,
    Texture(TextureType),
    Varying,
    VertexBuffer,
    VertexPosition,
    FragmentOutput(u8),
    RustInstance,
    Uniform,
    DynInstance,
    ScopeUniform,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PackPodType {
    /// The type's name, 0 for none.
    pub name: u64,
    pub kind: PackPodKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PackPodKind {
    Void,
    F32,
    F16,
    U32,
    I32,
    Bool,
    AtomicU32,
    AtomicI32,
    Vec(ScriptPodVec),
    Mat(ScriptPodMat),
    Packed(ScriptPodPacked),
    /// Fields as (name, index into the entry's `pod_types`).
    Struct { align_of: u32, size_of: u32, fields: Vec<(u64, u32)> },
    FixedArray { align_of: u32, size_of: u32, len: u32, ty: u32 },
    VariableArray { align_of: u32, ty: u32 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct PackScopeUniform {
    pub shader_name: u64,
    pub slots: u32,
    pub source: PackScopeSource,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PackScopeSource {
    /// `key` looked up from the object reached from the shader object by
    /// looking each name of `path` up in turn (scope lookups: an object's
    /// fields, then its prototypes'); `recorded` is the value it had.
    Path { path: Vec<u64>, key: u64, recorded: Vec<f32> },
    /// The table constant at this index.
    Const(u32),
    /// These slot values (no path to the source was found).
    Fixed(Vec<f32>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct PackTableConst {
    pub shader_name: u64,
    pub doc: String,
    pub value: f64,
}

// ---------------------------------------------------------------------------
// The key.

struct KeyHasher(u64);

impl KeyHasher {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
    fn bytes(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= *b as u64;
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    fn u8(&mut self, v: u8) {
        self.bytes(&[v]);
    }
    fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }
    fn id(&mut self, id: LiveId) {
        self.u64(id.0);
    }
}

// Tags that keep the hashed parts apart.
const K_FIELD: u8 = 1;
const K_FN: u8 = 2;
const K_NATIVE: u8 = 3;
const K_IO: u8 = 4;
const K_POD: u8 = 5;
const K_VALUE: u8 = 6;
const K_OBJECT: u8 = 7;
const K_RUST_FIELDS: u8 = 8;
const K_PROTO: u8 = 9;
const K_SEEN: u8 = 10;
const K_RESOLVED: u8 = 11;
const K_TOKEN: u8 = 12;
const K_DOC: u8 = 13;
const K_NAMESPACE: u8 = 14;

/// `key` looked up from `obj` the way a scope lookup does (the object's
/// map, its vec, then its prototypes), without raising an error on a miss.
pub(crate) fn find_scope_value(heap: &ScriptHeap, obj: ScriptObject, key: LiveId) -> Option<ScriptValue> {
    heap.scope_value_opt(obj, key)
}

struct KeyWalk<'a> {
    bx: &'a ScriptVmBase,
    h: KeyHasher,
    seen: HashSet<ScriptObject>,
    /// Every script function hashed, in order (the recorder's search reads
    /// their identifiers).
    fns: Vec<ScriptObject>,
}

impl<'a> KeyWalk<'a> {
    fn heap(&self) -> &'a ScriptHeap {
        self.bx.heap_ref()
    }

    fn pod(&mut self, ty: &ScriptPodTy) {
        self.h.u8(K_POD);
        match ty.kind() {
            crate::pod::ScriptPodTyTag::Void => self.h.u8(0),
            crate::pod::ScriptPodTyTag::ArrayBuilder => self.h.u8(1),
            crate::pod::ScriptPodTyTag::UndefinedStruct => self.h.u8(2),
            crate::pod::ScriptPodTyTag::F32 => self.h.u8(3),
            crate::pod::ScriptPodTyTag::F16 => self.h.u8(4),
            crate::pod::ScriptPodTyTag::U32 => self.h.u8(5),
            crate::pod::ScriptPodTyTag::I32 => self.h.u8(6),
            crate::pod::ScriptPodTyTag::Bool => self.h.u8(7),
            crate::pod::ScriptPodTyTag::AtomicU32 => self.h.u8(8),
            crate::pod::ScriptPodTyTag::AtomicI32 => self.h.u8(9),
            crate::pod::ScriptPodTyTag::Vec => {
                let v = ty.as_vec().unwrap();
                self.h.u8(10);
                self.h.u8(v as u8);
            }
            crate::pod::ScriptPodTyTag::Mat => {
                let m = ty.as_mat().unwrap();
                self.h.u8(11);
                self.h.u8(m as u8);
            }
            crate::pod::ScriptPodTyTag::Packed => {
                let p = ty.packed().unwrap();
                self.h.u8(12);
                self.h.u8(p as u8);
            }
            crate::pod::ScriptPodTyTag::Struct => {
                let fields = ty.struct_fields().unwrap();
                self.h.u8(13);
                self.h.u64(ty.align_of() as u64);
                self.h.u64(ty.size_of() as u64);
                for field in fields {
                    self.h.id(field.name());
                    self.pod(field.ty().data().ty());
                }
            }
            crate::pod::ScriptPodTyTag::Enum => {
                let variants = ty.enum_variants().unwrap();
                self.h.u8(14);
                self.h.u64(ty.align_of() as u64);
                self.h.u64(ty.size_of() as u64);
                for variant in variants {
                    self.h.id(variant.name());
                }
            }
            crate::pod::ScriptPodTyTag::FixedArray => {
                self.h.u8(15);
                self.h.u64(ty.align_of() as u64);
                self.h.u64(ty.size_of() as u64);
                self.h.u64(ty.array_len().unwrap() as u64);
                self.pod(ty.array_element().unwrap().data().ty());
            }
            crate::pod::ScriptPodTyTag::VariableArray => {
                self.h.u8(16);
                self.h.u64(ty.align_of() as u64);
                self.pod(ty.array_element().unwrap().data().ty());
            }
        }
    }

    /// The pod type a value is or carries, if any.
    fn value_pod(&self, v: ScriptValue) -> Option<&'a ScriptPodTy> {
        let heap = self.heap();
        if let Some(pod) = v.as_pod() {
            return Some(heap.pod_data(pod).0.ty());
        }
        let ty = v.as_pod_type().or_else(|| heap.pod_type(v))?;
        Some(heap.pod_type_ref(ty).ty())
    }

    /// A value by its kind: inline values that can steer code (ids, bools)
    /// by value, pods and pod types by their type, everything else by kind.
    fn value_kind(&mut self, v: ScriptValue) {
        if let Some(ty) = self.value_pod(v) {
            self.pod(ty);
            return;
        }
        self.h.u8(K_VALUE);
        self.h.u8(v.value_type().index());
        if v.as_id().is_some() || v.as_bool().is_some() {
            self.h.u64(v.raw());
        }
    }

    /// A value an inlined `#(…)` puts into code: inline values whole.
    fn inlined_value(&mut self, v: ScriptValue) {
        if v.as_object().is_some() || v.as_string().is_some() || v.as_pod().is_some() {
            self.value_kind(v);
        } else {
            self.h.u8(K_VALUE);
            self.h.u64(v.raw());
        }
    }

    /// A field of the shader object (or of a prototype).
    fn field(&mut self, v: ScriptValue) {
        let heap = self.heap();
        let Some(obj) = v.as_object() else {
            self.value_kind(v);
            return;
        };
        if heap.as_fn(obj).is_some() {
            self.function(obj);
        } else if let Some(io) = heap.as_shader_io(obj) {
            self.h.u8(K_IO);
            self.h.u64(io.index() as u64);
            let proto = heap.proto(obj);
            self.value_kind(proto);
        } else if let Some(ty) = self.value_pod(v) {
            self.pod(ty);
        } else {
            self.h.u8(K_OBJECT);
        }
    }

    /// A function by its tokens, then the functions its names resolve to.
    fn function(&mut self, fnobj: ScriptObject) {
        let bx = self.bx;
        let heap = self.heap();
        let ip = match heap.fn_script_ip(fnobj) {
            Some(ip) => ip,
            _ => {
                self.h.u8(K_NATIVE);
                return;
            }
        };
        if !self.seen.insert(fnobj) {
            self.h.u8(K_SEEN);
            return;
        }
        self.fns.push(fnobj);
        self.h.u8(K_FN);
        // A fn compiled in a namespace reads its names otherwise: the key
        // has the namespace (a fn without one keys as before).
        if let Some(ns) = bx.code_ref().shader_fn_namespaces.borrow().get(&fnobj) {
            self.h.u8(K_NAMESPACE);
            self.h.u64(ns.renames.len() as u64);
            for (from, to) in &ns.renames {
                self.h.id(*from);
                self.h.id(*to);
            }
            if let Some(input) = ns.input {
                for id in [input.field, input.method, input.at, input.param] {
                    self.h.id(id);
                }
            }
        }
        let mut names: Vec<(LiveId, Option<LiveId>)> = Vec::new();
        let mut last: Option<LiveId> = None;
        let mut dot = false;
        let mut inlined = Vec::new();
        let h = &mut self.h;
        bx.code_ref().fn_tokens(ip, |tok, doc, value| {
            if let Some(doc) = doc {
                h.u8(K_DOC);
                h.bytes(doc.as_bytes());
            }
            h.u8(K_TOKEN);
            h.u8(tok.preceded_by_newline() as u8);
            match tok.token().kind() {
                ScriptTokenTag::Identifier => {
                    let id = tok.token().identifier();
                    h.u8(1);
                    h.id(id);
                }
                ScriptTokenTag::Operator => {
                    let id = tok.token().operator();
                    h.u8(2);
                    h.id(id);
                }
                ScriptTokenTag::Separator => {
                    let id = tok.token().separator();
                    h.u8(3);
                    h.id(id);
                }
                ScriptTokenTag::OpenCurly => h.u8(4),
                ScriptTokenTag::CloseCurly => h.u8(5),
                ScriptTokenTag::OpenRound => h.u8(6),
                ScriptTokenTag::CloseRound => h.u8(7),
                ScriptTokenTag::OpenSquare => h.u8(8),
                ScriptTokenTag::CloseSquare => h.u8(9),
                ScriptTokenTag::String => {
                    let s = tok.token().as_string().unwrap();
                    h.u8(10);
                    heap.string_with(s, |_, s| h.bytes(s.as_bytes()));
                }
                ScriptTokenTag::F32 | ScriptTokenTag::F16 => {
                    let v = tok.token().as_f32().or_else(|| tok.token().as_f16()).unwrap();
                    h.u8(11);
                    h.u64(v.to_bits() as u64);
                }
                ScriptTokenTag::F64 => {
                    let v = tok.token().as_f64().unwrap();
                    h.u8(12);
                    h.u64(v.to_bits());
                }
                ScriptTokenTag::U32 => {
                    let v = tok.token().as_u32().unwrap();
                    h.u8(13);
                    h.u64(v as u64);
                }
                ScriptTokenTag::I32 => {
                    let v = tok.token().as_i32().unwrap();
                    h.u8(14);
                    h.u64(v as u64);
                }
                ScriptTokenTag::U40 => {
                    let v = tok.token().as_u40().unwrap();
                    h.u8(15);
                    h.u64(v);
                }
                ScriptTokenTag::Color => {
                    let v = tok.token().as_color().unwrap();
                    h.u8(16);
                    h.u64(v as u64);
                }
                ScriptTokenTag::RustValue => {
                    h.u8(17);
                    inlined.push(value.unwrap_or(ScriptValue::NIL));
                }
                ScriptTokenTag::End | ScriptTokenTag::StreamEnd | ScriptTokenTag::StringUnfinished => h.u8(18),
            }
            // Names (and `a.b` pairs) to resolve once the text is hashed.
            match tok.token().kind() {
                ScriptTokenTag::Identifier => {
                    let id = tok.token().identifier();
                    if let (true, Some(a)) = (dot, last) {
                        names.push((a, Some(id)));
                    }
                    names.push((id, None));
                    last = Some(id);
                    dot = false;
                }
                ScriptTokenTag::Operator if tok.token().operator() == id!(.) => dot = last.is_some(),
                _ => {
                    last = None;
                    dot = false;
                }
            }
        });
        for v in inlined {
            self.inlined_value(v);
        }
        let mut done = HashSet::new();
        for (a, b) in names {
            if !done.insert((a, b)) || a == id!(self) || a == id!(fn) {
                continue;
            }
            let Some(va) = find_scope_value(heap, fnobj, a) else { continue };
            let v = match b {
                None => va,
                Some(b) => {
                    let Some(obj) = va.as_object() else { continue };
                    if heap.as_fn(obj).is_some() {
                        continue;
                    }
                    let Some(vb) = find_scope_value(heap, obj, b) else { continue };
                    vb
                }
            };
            self.h.u8(K_RESOLVED);
            self.h.id(a);
            if let Some(b) = b {
                self.h.id(b);
            }
            match v.as_object() {
                Some(obj) if heap.as_fn(obj).is_some() => self.function(obj),
                Some(_) => {
                    if let Some(ty) = self.value_pod(v) {
                        self.pod(ty);
                    } else {
                        self.h.u8(K_OBJECT);
                    }
                }
                None => self.value_kind(v),
            }
        }
    }
}

fn shader_key_walk<'a>(bx: &'a ScriptVmBase, io_self: ScriptObject, const_table: bool) -> KeyWalk<'a> {
    let mut w = KeyWalk { bx, h: KeyHasher::new(), seen: HashSet::new(), fns: Vec::new() };
    let heap = bx.heap_ref();
    w.h.u64(SHADER_PACK_VERSION as u64);
    w.h.u8(const_table as u8);
    for key in [id!(color_format), id!(blend_op)] {
        w.h.id(key);
        w.h.id(heap.value(io_self, key.into(), NoTrap).as_id().unwrap_or(LiveId(0)));
    }
    let mut cur = Some(io_self);
    while let Some(obj) = cur {
        w.h.u8(K_PROTO);
        if let Some(ty_index) = heap.object_type_index(obj) {
            w.h.u8(K_RUST_FIELDS);
            for (field, type_id) in heap.type_check(ty_index).props().iter_rust_instance_ordered() {
                w.h.id(field);
                match heap.type_id_to_pod_type(type_id, bx.pod_builtins()) {
                    Some(ty) => w.pod(heap.pod_type_ref(ty).ty()),
                    None => w.h.u8(0),
                }
            }
        }
        for e in heap.map_entries_ordered(obj) {
            let (k, v) = (e.key, e.value);
            w.h.u8(K_FIELD);
            match k.as_id() {
                Some(id) => w.h.id(id),
                None => w.h.u8(k.value_type().index()),
            }
            w.field(v);
        }
        cur = heap.proto(obj).as_object();
    }
    w
}

/// The key a draw shader object's pack entry is found by: the same in
/// every process, on every target, in any load order (see the module
/// docs for what it covers).
#[relax_api]
pub fn shader_pack_key(vm: &ScriptVm, io_self: ScriptObject, const_table: bool) -> u64 {
    shader_key_walk(&vm.bx, io_self, const_table).h.0
}

// ---------------------------------------------------------------------------
// Entry <-> reflection.

#[relax_api]
impl ShaderPackEntry {
    /// The reflection of this shader for the shader object `io_self` of
    /// this heap (its scope uniforms are found again from it), for
    /// `CxDrawShaderMapping::from_desc`.
    #[relax_api]
    pub fn to_desc(&self, heap: &ScriptHeap, io_self: ScriptObject) -> DrawShaderDesc {
        let tys: Vec<ScriptPodTy> = (0..self.pod_types.len()).map(|i| pod_from_pack(&self.pod_types, i as u32)).collect();
        let io = self
            .io
            .iter()
            .map(|io| {
                let name = LiveId(io.name);
                let kind = match io.kind {
                    PackIoKind::StorageBuffer { read, write } => ShaderIoKind::storage_buffer(read, write),
                    PackIoKind::UniformBuffer => ShaderIoKind::uniform_buffer(),
                    PackIoKind::Sampler => ShaderIoKind::sampler(),
                    PackIoKind::Texture(t) => ShaderIoKind::texture(t),
                    PackIoKind::Varying => ShaderIoKind::varying(),
                    PackIoKind::VertexBuffer => ShaderIoKind::vertex_buffer(),
                    PackIoKind::VertexPosition => ShaderIoKind::vertex_position(),
                    PackIoKind::FragmentOutput(i) => ShaderIoKind::fragment(i),
                    PackIoKind::RustInstance => ShaderIoKind::rust_instance(),
                    PackIoKind::Uniform => ShaderIoKind::uniform(),
                    PackIoKind::DynInstance => ShaderIoKind::dyn_instance(),
                    PackIoKind::ScopeUniform => ShaderIoKind::scope_uniform(),
                };
                // A custom uniform buffer keeps the heap's pod type: the one
                // its marker on the shader object names.
                let pod_type = match io.kind {
                    PackIoKind::UniformBuffer => find_scope_value(heap, io_self, name)
                        .and_then(|v| v.as_object())
                        .and_then(|marker| {
                            let proto = heap.proto(marker);
                            proto.as_pod_type().or_else(|| heap.pod_type(proto))
                        })
                        .unwrap_or(ScriptPodType::VOID),
                    _ => ScriptPodType::VOID,
                };
                let pod_name = self.pod_types.get(io.ty as usize).map(|t| t.name).unwrap_or(0);
                DrawShaderDescIo {
                    kind,
                    name,
                    ty: Box::new(tys.get(io.ty as usize).cloned().unwrap_or_default()),
                    pod_name: (pod_name != 0).then_some(LiveId(pod_name)),
                    pod_type,
                    buffer_index: io.buffer_index.map(|i| i as usize),
                    block_name: io.block_name.as_str().into(),
                    sampler: io.sampler as usize,
                }
            })
            .collect();
        let scope_uniforms = self
            .scope_uniforms
            .iter()
            .map(|su| DrawShaderDescScopeUniform {
                shader_name: LiveId(su.shader_name),
                slots: su.slots as usize,
                source: match &su.source {
                    PackScopeSource::Const(i) => DrawShaderDescScope::Const(*i as usize),
                    PackScopeSource::Fixed(values) => DrawShaderDescScope::Fixed(values.as_slice().into()),
                    PackScopeSource::Path { path, key, recorded } => {
                        let key = LiveId(*key);
                        match resolve_path(heap, io_self, path) {
                            Some(obj) if find_scope_value(heap, obj, key).is_some() => DrawShaderDescScope::Scope(obj, key),
                            _ => DrawShaderDescScope::Fixed(recorded.as_slice().into()),
                        }
                    }
                },
            })
            .collect();
        DrawShaderDesc {
            io,
            samplers: self.samplers.clone(),
            scope_uniforms,
            table_consts: self
                .table_consts
                .iter()
                .map(|tc| ShaderTableConst::new(LiveId(tc.shader_name), &tc.doc, tc.value))
                .collect(),
            folded_sites: Vec::new(),
            uniform_buffer_bindings: {
                let mut bindings = UniformBufferBindings::new(self.scope_uniform_buffer_index.map(|i| i as usize));
                for (name, index) in &self.uniform_buffer_bindings { bindings.push(LiveId(*name), *index as usize); }
                bindings
            },
            uses_time: self.uses_time,
        }
    }
}

fn resolve_path(heap: &ScriptHeap, io_self: ScriptObject, path: &[u64]) -> Option<ScriptObject> {
    let mut cur = io_self;
    for id in path {
        cur = find_scope_value(heap, cur, LiveId(*id))?.as_object()?;
    }
    Some(cur)
}

/// A pod type into an entry's type list (deduplicated); `None` for a type
/// a draw shader's io cannot carry.
fn pod_to_pack(data: &ScriptPodTypeData, out: &mut Vec<PackPodType>) -> Option<u32> {
    let ty = data.ty();
    let kind = match ty.kind() {
        crate::pod::ScriptPodTyTag::Void => PackPodKind::Void,
        crate::pod::ScriptPodTyTag::F32 => PackPodKind::F32,
        crate::pod::ScriptPodTyTag::F16 => PackPodKind::F16,
        crate::pod::ScriptPodTyTag::U32 => PackPodKind::U32,
        crate::pod::ScriptPodTyTag::I32 => PackPodKind::I32,
        crate::pod::ScriptPodTyTag::Bool => PackPodKind::Bool,
        crate::pod::ScriptPodTyTag::AtomicU32 => PackPodKind::AtomicU32,
        crate::pod::ScriptPodTyTag::AtomicI32 => PackPodKind::AtomicI32,
        crate::pod::ScriptPodTyTag::Vec => PackPodKind::Vec(ty.as_vec().unwrap()),
        crate::pod::ScriptPodTyTag::Mat => PackPodKind::Mat(ty.as_mat().unwrap()),
        crate::pod::ScriptPodTyTag::Packed => PackPodKind::Packed(ty.packed().unwrap()),
        crate::pod::ScriptPodTyTag::Struct => {
            let fields = ty.struct_fields().unwrap();
            let mut packed = Vec::with_capacity(fields.len());
            for field in fields {
                packed.push((field.name().0, pod_to_pack(field.ty().data(), out)?));
            }
            PackPodKind::Struct { align_of: ty.align_of() as u32, size_of: ty.size_of() as u32, fields: packed }
        }
        crate::pod::ScriptPodTyTag::FixedArray => PackPodKind::FixedArray {
            align_of: ty.align_of() as u32,
            size_of: ty.size_of() as u32,
            len: ty.array_len().unwrap() as u32,
            ty: pod_to_pack(ty.array_element().unwrap().data(), out)?,
        },
        crate::pod::ScriptPodTyTag::VariableArray => {
            PackPodKind::VariableArray { align_of: ty.align_of() as u32, ty: pod_to_pack(ty.array_element().unwrap().data(), out)? }
        }
        crate::pod::ScriptPodTyTag::ArrayBuilder | crate::pod::ScriptPodTyTag::UndefinedStruct | crate::pod::ScriptPodTyTag::Enum => return None,
    };
    let entry = PackPodType { name: data.name().map(|n| n.0).unwrap_or(0), kind };
    Some(match out.iter().position(|t| *t == entry) {
        Some(i) => i as u32,
        None => {
            out.push(entry);
            (out.len() - 1) as u32
        }
    })
}

fn pod_from_pack(types: &[PackPodType], index: u32) -> ScriptPodTy {
    let inline = |index: u32| ScriptPodTypeInline::new(ScriptPodType::VOID, ScriptPodTypeData::new(
        types.get(index as usize).and_then(|t| (t.name != 0).then_some(LiveId(t.name))), pod_from_pack(types, index)));
    let Some(ty) = types.get(index as usize) else {
        return ScriptPodTy::scalar(ScriptPodScalar::Void);
    };
    match &ty.kind {
        PackPodKind::Void => ScriptPodTy::scalar(ScriptPodScalar::Void),
        PackPodKind::F32 => ScriptPodTy::scalar(ScriptPodScalar::F32),
        PackPodKind::F16 => ScriptPodTy::scalar(ScriptPodScalar::F16),
        PackPodKind::U32 => ScriptPodTy::scalar(ScriptPodScalar::U32),
        PackPodKind::I32 => ScriptPodTy::scalar(ScriptPodScalar::I32),
        PackPodKind::Bool => ScriptPodTy::scalar(ScriptPodScalar::Bool),
        PackPodKind::AtomicU32 => ScriptPodTy::scalar(ScriptPodScalar::AtomicU32),
        PackPodKind::AtomicI32 => ScriptPodTy::scalar(ScriptPodScalar::AtomicI32),
        PackPodKind::Vec(v) => ScriptPodTy::vector(*v),
        PackPodKind::Mat(m) => ScriptPodTy::matrix(*m),
        PackPodKind::Packed(p) => ScriptPodTy::packed_type(*p),
        PackPodKind::Struct { align_of, size_of, fields } => {
            let mut result = ScriptPodTy::structure(*align_of as usize, *size_of as usize);
            for (name, ty) in fields {
                result.push_field(ScriptPodField::new(LiveId(*name), ScriptValue::NIL, inline(*ty)));
            }
            result
        }
        PackPodKind::FixedArray { align_of, size_of, len, ty } => ScriptPodTy::fixed_array(*align_of as usize, *size_of as usize, *len as usize, inline(*ty)),
        PackPodKind::VariableArray { align_of, ty } => ScriptPodTy::variable_array(*align_of as usize, inline(*ty)),
    }
}

/// A pod type's structure, comparable across heaps (names, fields, sizes):
/// what a pack keeps of it.
fn pod_structure(name: Option<LiveId>, ty: &ScriptPodTy) -> Option<(Vec<PackPodType>, u32)> {
    let mut out = Vec::new();
    let data = ScriptPodTypeData::new(name, ty.clone());
    let index = pod_to_pack(&data, &mut out)?;
    Some((out, index))
}

/// A compile's reflection as a pack entry: scope uniform sources become
/// paths from the shader object.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
pub(crate) fn pack_entry_from_desc(
    vm: &ScriptVm,
    io_self: ScriptObject,
    key: u64,
    name: String,
    vertex: String,
    fragment: String,
    desc: &DrawShaderDesc,
    fns: &[ScriptObject],
) -> Result<ShaderPackEntry, String> {
    let heap = vm.heap();
    let mut pod_types = Vec::new();
    let mut io = Vec::new();
    for d in &desc.io {
        let data = ScriptPodTypeData::new(d.pod_name, (*d.ty).clone());
        let ty = pod_to_pack(&data, &mut pod_types)
            .ok_or_else(|| format!("io `{}` has a pod type a pack cannot carry", d.name))?;
        let kind = match d.kind.kind() {
            crate::shader_output::ShaderIoKindTag::StorageBuffer => PackIoKind::StorageBuffer { read: d.kind.storage_flags().unwrap().is_read(), write: d.kind.storage_flags().unwrap().is_write() },
            crate::shader_output::ShaderIoKindTag::UniformBuffer => PackIoKind::UniformBuffer,
            crate::shader_output::ShaderIoKindTag::Sampler => PackIoKind::Sampler,
            crate::shader_output::ShaderIoKindTag::Texture => PackIoKind::Texture(d.kind.as_texture().unwrap()),
            crate::shader_output::ShaderIoKindTag::Varying => PackIoKind::Varying,
            crate::shader_output::ShaderIoKindTag::VertexBuffer => PackIoKind::VertexBuffer,
            crate::shader_output::ShaderIoKindTag::VertexPosition => PackIoKind::VertexPosition,
            crate::shader_output::ShaderIoKindTag::FragmentOutput => PackIoKind::FragmentOutput(d.kind.fragment_output().unwrap()),
            crate::shader_output::ShaderIoKindTag::RustInstance => PackIoKind::RustInstance,
            crate::shader_output::ShaderIoKindTag::Uniform => PackIoKind::Uniform,
            crate::shader_output::ShaderIoKindTag::DynInstance => PackIoKind::DynInstance,
            crate::shader_output::ShaderIoKindTag::ScopeUniform => PackIoKind::ScopeUniform,
            crate::shader_output::ShaderIoKindTag::Workgroup => return Err(format!("io `{}` is a compute kernel's workgroup array; draw packs have none", d.name)),
        };
        io.push(PackIo {
            kind,
            name: d.name.0,
            ty,
            buffer_index: d.buffer_index.map(|i| i as u32),
            block_name: d.block_name.to_string(),
            sampler: d.sampler as u32,
        });
    }
    let mut paths = ScopePaths::new(&vm.bx, io_self, fns);
    let mut scope_uniforms = Vec::new();
    for su in &desc.scope_uniforms {
        let source = match &su.source {
            DrawShaderDescScope::Const(i) => PackScopeSource::Const(*i as u32),
            DrawShaderDescScope::Fixed(values) => PackScopeSource::Fixed(values.to_vec()),
            DrawShaderDescScope::Scope(obj, key) => {
                let mut recorded = vec![0.0f32; su.slots];
                let value = heap.scope_value(*obj, *key, NoTrap);
                write_pack_value(heap, value, &mut recorded);
                match paths.find(*obj) {
                    Some(path) => PackScopeSource::Path { path, key: key.0, recorded },
                    None => {
                        makepad_error_log::warning!(
                            "shader pack: '{}' scope uniform `{}` has no path from the shader object; its recorded value is used",
                            name,
                            key
                        );
                        PackScopeSource::Fixed(recorded)
                    }
                }
            }
        };
        scope_uniforms.push(PackScopeUniform { shader_name: su.shader_name.0, slots: su.slots as u32, source });
    }
    Ok(ShaderPackEntry {
        key,
        name,
        vertex,
        fragment,
        pod_types,
        io,
        samplers: desc.samplers.clone(),
        scope_uniforms,
        table_consts: desc
            .table_consts
            .iter()
            .map(|tc| PackTableConst { shader_name: tc.shader_name().0, doc: tc.doc().to_owned(), value: tc.value() })
            .collect(),
        uniform_buffer_bindings: (0..desc.uniform_buffer_bindings.len()).map(|i| (desc.uniform_buffer_bindings.binding_name(i).0, desc.uniform_buffer_bindings.binding_index(i) as u32)).collect(),
        scope_uniform_buffer_index: desc.uniform_buffer_bindings.scope_uniform_buffer_index().map(|i| i as u32),
        uses_time: desc.uses_time(),
    })
}

/// Paths of names from a shader object to the objects its scope uniforms
/// are read from: a breadth-first search over scope lookups of the names
/// its functions use.
#[cfg(not(target_arch = "wasm32"))]
struct ScopePaths<'a> {
    heap: &'a ScriptHeap,
    names: Vec<LiveId>,
    /// Objects reached so far, with their paths, in search order.
    reached: Vec<(ScriptObject, Vec<u64>)>,
    seen: HashSet<ScriptObject>,
    /// How far `reached` has been expanded.
    expanded: usize,
}

#[cfg(not(target_arch = "wasm32"))]
impl<'a> ScopePaths<'a> {
    /// Paths at most this many names long.
    const MAX_DEPTH: usize = 8;

    fn new(bx: &'a ScriptVmBase, io_self: ScriptObject, fns: &[ScriptObject]) -> Self {
        let heap = bx.heap_ref();
        let mut names = Vec::new();
        let mut known = HashSet::new();
        for f in fns {
            if let Some(ip) = heap.fn_script_ip(*f) {
                bx.code_ref().fn_tokens(ip, |tok, _, _| {
                    if tok.token().kind() == ScriptTokenTag::Identifier {
                        let id = tok.token().identifier();
                        if known.insert(id) {
                            names.push(id);
                        }
                    }
                });
            }
        }
        // The shader object's own fields (its methods) start every path.
        let mut cur = Some(io_self);
        while let Some(obj) = cur {
            for e in heap.map_entries_ordered(obj) {
                let (k, _) = (e.key, e.value);
                if let Some(id) = k.as_id() {
                    if known.insert(id) {
                        names.push(id);
                    }
                }
            }
            cur = heap.proto(obj).as_object();
        }
        let mut seen = HashSet::new();
        seen.insert(io_self);
        Self { heap, names, reached: vec![(io_self, Vec::new())], seen, expanded: 0 }
    }

    fn find(&mut self, target: ScriptObject) -> Option<Vec<u64>> {
        loop {
            if let Some((_, path)) = self.reached.iter().find(|(obj, _)| *obj == target) {
                return Some(path.clone());
            }
            if self.expanded >= self.reached.len() {
                return None;
            }
            let (obj, path) = self.reached[self.expanded].clone();
            self.expanded += 1;
            if path.len() >= Self::MAX_DEPTH {
                continue;
            }
            for name in &self.names {
                let Some(next) = find_scope_value(self.heap, obj, *name).and_then(|v| v.as_object()) else { continue };
                if self.seen.insert(next) {
                    let mut p = path.clone();
                    p.push(name.0);
                    self.reached.push((next, p));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The pack's bytes.
//
// "MPSP", version u32, string count u32, strings (len u32 + utf-8), entry
// count u32, then per entry: key u64, byte length u32, the entry. Numbers
// are little endian; the GLSL texts are strings shared between entries.

const VECS: [ScriptPodVec; 15] = [
    ScriptPodVec::Vec2f,
    ScriptPodVec::Vec3f,
    ScriptPodVec::Vec4f,
    ScriptPodVec::Vec2h,
    ScriptPodVec::Vec3h,
    ScriptPodVec::Vec4h,
    ScriptPodVec::Vec2u,
    ScriptPodVec::Vec3u,
    ScriptPodVec::Vec4u,
    ScriptPodVec::Vec2i,
    ScriptPodVec::Vec3i,
    ScriptPodVec::Vec4i,
    ScriptPodVec::Vec2b,
    ScriptPodVec::Vec3b,
    ScriptPodVec::Vec4b,
];
const MATS: [ScriptPodMat; 9] = [
    ScriptPodMat::Mat2x2f,
    ScriptPodMat::Mat3x2f,
    ScriptPodMat::Mat4x2f,
    ScriptPodMat::Mat2x3f,
    ScriptPodMat::Mat3x3f,
    ScriptPodMat::Mat4x3f,
    ScriptPodMat::Mat2x4f,
    ScriptPodMat::Mat3x4f,
    ScriptPodMat::Mat4x4f,
];
const PACKEDS: [ScriptPodPacked; 8] = [
    ScriptPodPacked::F16x2,
    ScriptPodPacked::F16x4,
    ScriptPodPacked::U16x2,
    ScriptPodPacked::I16x2,
    ScriptPodPacked::U16x2Norm,
    ScriptPodPacked::I16x2Norm,
    ScriptPodPacked::U8x4Norm,
    ScriptPodPacked::I8x4Norm,
];
const TEXTURES: [TextureType; 11] = [
    TextureType::Texture1d,
    TextureType::Texture1dArray,
    TextureType::Texture2d,
    TextureType::Texture2dArray,
    TextureType::Texture3d,
    TextureType::Texture3dArray,
    TextureType::TextureCube,
    TextureType::TextureCubeArray,
    TextureType::TextureDepth,
    TextureType::TextureDepthArray,
    TextureType::TextureVideo,
];
const FILTERS: [SamplerFilter; 2] = [SamplerFilter::Nearest, SamplerFilter::Linear];
const ADDRESSES: [SamplerAddress; 4] =
    [SamplerAddress::Repeat, SamplerAddress::ClampToEdge, SamplerAddress::ClampToZero, SamplerAddress::MirroredRepeat];
const COORDS: [SamplerCoord; 2] = [SamplerCoord::Normalized, SamplerCoord::Pixel];

fn code_of<T: PartialEq>(table: &[T], v: &T) -> u8 {
    table.iter().position(|t| t == v).unwrap_or(0) as u8
}

struct PackWriter {
    out: Vec<u8>,
}

impl PackWriter {
    fn u8(&mut self, v: u8) {
        self.out.push(v);
    }
    fn u32(&mut self, v: u32) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }
    fn len(&mut self, n: usize) {
        self.u32(n as u32);
    }
    fn opt(&mut self, v: Option<u32>) {
        match v {
            Some(v) => {
                self.u8(1);
                self.u32(v);
            }
            None => self.u8(0),
        }
    }
    fn str(&mut self, s: &str) {
        self.len(s.len());
        self.out.extend_from_slice(s.as_bytes());
    }
    fn floats(&mut self, values: &[f32]) {
        self.len(values.len());
        for v in values {
            self.u32(v.to_bits());
        }
    }

    fn entry(&mut self, e: &ShaderPackEntry, vertex: u32, fragment: u32) {
        self.str(&e.name);
        self.u32(vertex);
        self.u32(fragment);
        self.len(e.pod_types.len());
        for t in &e.pod_types {
            self.u64(t.name);
            match &t.kind {
                PackPodKind::Void => self.u8(0),
                PackPodKind::F32 => self.u8(1),
                PackPodKind::F16 => self.u8(2),
                PackPodKind::U32 => self.u8(3),
                PackPodKind::I32 => self.u8(4),
                PackPodKind::Bool => self.u8(5),
                PackPodKind::AtomicU32 => self.u8(6),
                PackPodKind::AtomicI32 => self.u8(7),
                PackPodKind::Vec(v) => {
                    self.u8(8);
                    self.u8(code_of(&VECS, v));
                }
                PackPodKind::Mat(m) => {
                    self.u8(9);
                    self.u8(code_of(&MATS, m));
                }
                PackPodKind::Packed(p) => {
                    self.u8(10);
                    self.u8(code_of(&PACKEDS, p));
                }
                PackPodKind::Struct { align_of, size_of, fields } => {
                    self.u8(11);
                    self.u32(*align_of);
                    self.u32(*size_of);
                    self.len(fields.len());
                    for (name, ty) in fields {
                        self.u64(*name);
                        self.u32(*ty);
                    }
                }
                PackPodKind::FixedArray { align_of, size_of, len, ty } => {
                    self.u8(12);
                    self.u32(*align_of);
                    self.u32(*size_of);
                    self.u32(*len);
                    self.u32(*ty);
                }
                PackPodKind::VariableArray { align_of, ty } => {
                    self.u8(13);
                    self.u32(*align_of);
                    self.u32(*ty);
                }
            }
        }
        self.len(e.io.len());
        for io in &e.io {
            match io.kind {
                PackIoKind::StorageBuffer { read, write } => {
                    self.u8(0);
                    self.u8(read as u8 | (write as u8) << 1);
                }
                PackIoKind::UniformBuffer => self.u8(1),
                PackIoKind::Sampler => self.u8(2),
                PackIoKind::Texture(t) => {
                    self.u8(3);
                    self.u8(code_of(&TEXTURES, &t));
                }
                PackIoKind::Varying => self.u8(4),
                PackIoKind::VertexBuffer => self.u8(5),
                PackIoKind::VertexPosition => self.u8(6),
                PackIoKind::FragmentOutput(i) => {
                    self.u8(7);
                    self.u8(i);
                }
                PackIoKind::RustInstance => self.u8(8),
                PackIoKind::Uniform => self.u8(9),
                PackIoKind::DynInstance => self.u8(10),
                PackIoKind::ScopeUniform => self.u8(11),
            }
            self.u64(io.name);
            self.u32(io.ty);
            self.opt(io.buffer_index);
            self.str(&io.block_name);
            self.u32(io.sampler);
        }
        self.len(e.samplers.len());
        for s in &e.samplers {
            self.u8(code_of(&FILTERS, &s.filter));
            self.u8(code_of(&ADDRESSES, &s.address));
            self.u8(code_of(&COORDS, &s.coord));
            self.u8(s.is_video as u8);
            self.u8(s.compare as u8);
        }
        self.len(e.scope_uniforms.len());
        for su in &e.scope_uniforms {
            self.u64(su.shader_name);
            self.u32(su.slots);
            match &su.source {
                PackScopeSource::Path { path, key, recorded } => {
                    self.u8(0);
                    self.len(path.len());
                    for id in path {
                        self.u64(*id);
                    }
                    self.u64(*key);
                    self.floats(recorded);
                }
                PackScopeSource::Const(i) => {
                    self.u8(1);
                    self.u32(*i);
                }
                PackScopeSource::Fixed(values) => {
                    self.u8(2);
                    self.floats(values);
                }
            }
        }
        self.len(e.table_consts.len());
        for tc in &e.table_consts {
            self.u64(tc.shader_name);
            self.str(&tc.doc);
            self.u64(tc.value.to_bits());
        }
        self.len(e.uniform_buffer_bindings.len());
        for (name, index) in &e.uniform_buffer_bindings {
            self.u64(*name);
            self.u32(*index);
        }
        self.opt(e.scope_uniform_buffer_index);
        self.u32(e.uses_time as u32);
    }
}

/// Entries as pack bytes.
pub fn write_shader_pack(entries: &[ShaderPackEntry]) -> Vec<u8> {
    fn string<'a>(s: &'a str, strings: &mut Vec<&'a str>) -> u32 {
        match strings.iter().position(|t| *t == s) {
            Some(i) => i as u32,
            None => {
                strings.push(s);
                (strings.len() - 1) as u32
            }
        }
    }
    let mut strings: Vec<&str> = Vec::new();
    let mut codes = Vec::with_capacity(entries.len());
    for e in entries {
        let v = string(&e.vertex, &mut strings);
        let f = string(&e.fragment, &mut strings);
        codes.push((v, f));
    }
    let mut w = PackWriter { out: Vec::new() };
    w.out.extend_from_slice(SHADER_PACK_MAGIC);
    w.u32(SHADER_PACK_VERSION);
    w.len(strings.len());
    for s in &strings {
        w.str(s);
    }
    w.len(entries.len());
    for (e, (v, f)) in entries.iter().zip(codes) {
        w.u64(e.key);
        let at = w.out.len();
        w.u32(0);
        w.entry(e, v, f);
        let len = (w.out.len() - at - 4) as u32;
        w.out[at..at + 4].copy_from_slice(&len.to_le_bytes());
    }
    w.out
}

/// The entries of pack bytes, in pack order.
pub fn read_shader_pack(bytes: &[u8]) -> Result<Vec<ShaderPackEntry>, String> {
    let strings = read_strings(bytes)?;
    let mut entries = Vec::new();
    for (key, offset) in read_index(bytes)? {
        entries.push(PackReader { bytes, at: offset }.entry(key, &strings)?);
    }
    Ok(entries)
}

struct PackReader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> PackReader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).filter(|end| *end <= self.bytes.len()).ok_or("the shader pack ends early")?;
        let s = &self.bytes[self.at..end];
        self.at = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn len(&mut self) -> Result<usize, String> {
        Ok(self.u32()? as usize)
    }
    fn opt(&mut self) -> Result<Option<u32>, String> {
        Ok(match self.u8()? {
            0 => None,
            _ => Some(self.u32()?),
        })
    }
    fn str(&mut self) -> Result<&'a str, String> {
        let n = self.len()?;
        std::str::from_utf8(self.take(n)?).map_err(|_| "a shader pack string is not utf-8".to_string())
    }
    fn floats(&mut self) -> Result<Vec<f32>, String> {
        let n = self.len()?;
        (0..n).map(|_| Ok(f32::from_bits(self.u32()?))).collect()
    }
    fn code<T: Copy>(&mut self, table: &[T]) -> Result<T, String> {
        let i = self.u8()? as usize;
        table.get(i).copied().ok_or_else(|| "a shader pack holds an unknown code".to_string())
    }

    fn entry(&mut self, key: u64, strings: &[&str]) -> Result<ShaderPackEntry, String> {
        let name = self.str()?.to_string();
        let code = |r: &mut Self| -> Result<String, String> {
            let i = r.len()?;
            strings.get(i).map(|s| s.to_string()).ok_or_else(|| "a shader pack entry names no code".to_string())
        };
        let vertex = code(self)?;
        let fragment = code(self)?;
        let n = self.len()?;
        let mut pod_types = Vec::with_capacity(n.min(1024));
        for _ in 0..n {
            let name = self.u64()?;
            let kind = match self.u8()? {
                0 => PackPodKind::Void,
                1 => PackPodKind::F32,
                2 => PackPodKind::F16,
                3 => PackPodKind::U32,
                4 => PackPodKind::I32,
                5 => PackPodKind::Bool,
                6 => PackPodKind::AtomicU32,
                7 => PackPodKind::AtomicI32,
                8 => PackPodKind::Vec(self.code(&VECS)?),
                9 => PackPodKind::Mat(self.code(&MATS)?),
                10 => PackPodKind::Packed(self.code(&PACKEDS)?),
                11 => {
                    let align_of = self.u32()?;
                    let size_of = self.u32()?;
                    let n = self.len()?;
                    let mut fields = Vec::with_capacity(n.min(1024));
                    for _ in 0..n {
                        fields.push((self.u64()?, self.u32()?));
                    }
                    PackPodKind::Struct { align_of, size_of, fields }
                }
                12 => PackPodKind::FixedArray { align_of: self.u32()?, size_of: self.u32()?, len: self.u32()?, ty: self.u32()? },
                13 => PackPodKind::VariableArray { align_of: self.u32()?, ty: self.u32()? },
                _ => return Err("a shader pack holds an unknown pod type".into()),
            };
            pod_types.push(PackPodType { name, kind });
        }
        let n = self.len()?;
        let mut io = Vec::with_capacity(n.min(1024));
        for _ in 0..n {
            let kind = match self.u8()? {
                0 => {
                    let rw = self.u8()?;
                    PackIoKind::StorageBuffer { read: rw & 1 != 0, write: rw & 2 != 0 }
                }
                1 => PackIoKind::UniformBuffer,
                2 => PackIoKind::Sampler,
                3 => PackIoKind::Texture(self.code(&TEXTURES)?),
                4 => PackIoKind::Varying,
                5 => PackIoKind::VertexBuffer,
                6 => PackIoKind::VertexPosition,
                7 => PackIoKind::FragmentOutput(self.u8()?),
                8 => PackIoKind::RustInstance,
                9 => PackIoKind::Uniform,
                10 => PackIoKind::DynInstance,
                11 => PackIoKind::ScopeUniform,
                _ => return Err("a shader pack holds an unknown io kind".into()),
            };
            io.push(PackIo {
                kind,
                name: self.u64()?,
                ty: self.u32()?,
                buffer_index: self.opt()?,
                block_name: self.str()?.to_string(),
                sampler: self.u32()?,
            });
        }
        let n = self.len()?;
        let mut samplers = Vec::with_capacity(n.min(1024));
        for _ in 0..n {
            samplers.push(ShaderSampler {
                filter: self.code(&FILTERS)?,
                address: self.code(&ADDRESSES)?,
                coord: self.code(&COORDS)?,
                is_video: self.u8()? != 0,
                compare: self.u8()? != 0,
            });
        }
        let n = self.len()?;
        let mut scope_uniforms = Vec::with_capacity(n.min(1024));
        for _ in 0..n {
            let shader_name = self.u64()?;
            let slots = self.u32()?;
            let source = match self.u8()? {
                0 => {
                    let n = self.len()?;
                    let mut path = Vec::with_capacity(n.min(1024));
                    for _ in 0..n {
                        path.push(self.u64()?);
                    }
                    PackScopeSource::Path { path, key: self.u64()?, recorded: self.floats()? }
                }
                1 => PackScopeSource::Const(self.u32()?),
                2 => PackScopeSource::Fixed(self.floats()?),
                _ => return Err("a shader pack holds an unknown scope source".into()),
            };
            scope_uniforms.push(PackScopeUniform { shader_name, slots, source });
        }
        let n = self.len()?;
        let mut table_consts = Vec::with_capacity(n.min(1024));
        for _ in 0..n {
            table_consts.push(PackTableConst {
                shader_name: self.u64()?,
                doc: self.str()?.to_string(),
                value: f64::from_bits(self.u64()?),
            });
        }
        let n = self.len()?;
        let mut uniform_buffer_bindings = Vec::with_capacity(n.min(1024));
        for _ in 0..n {
            uniform_buffer_bindings.push((self.u64()?, self.u32()?));
        }
        let scope_uniform_buffer_index = self.opt()?;
        let uses_time = self.u32()? != 0;
        Ok(ShaderPackEntry {
            key,
            name,
            vertex,
            fragment,
            pod_types,
            io,
            samplers,
            scope_uniforms,
            table_consts,
            uniform_buffer_bindings,
            scope_uniform_buffer_index,
            uses_time,
        })
    }
}

/// The header checked and the shared strings read.
fn read_strings(bytes: &[u8]) -> Result<Vec<&str>, String> {
    let mut r = PackReader { bytes, at: 0 };
    if r.take(4)? != SHADER_PACK_MAGIC {
        return Err("not a shader pack".into());
    }
    let version = r.u32()?;
    if version != SHADER_PACK_VERSION {
        return Err(format!("shader pack version {} (this build reads {})", version, SHADER_PACK_VERSION));
    }
    let n = r.len()?;
    let mut strings = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        strings.push(r.str()?);
    }
    Ok(strings)
}

/// Each entry's key and where its bytes start, without reading the entries.
fn read_index(bytes: &[u8]) -> Result<Vec<(u64, usize)>, String> {
    read_strings(bytes)?;
    let mut r = PackReader { bytes, at: 0 };
    r.take(8)?;
    let n = r.len()?;
    for _ in 0..n {
        let len = r.len()?;
        r.take(len)?;
    }
    let n = r.len()?;
    let mut index = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        let key = r.u64()?;
        let len = r.len()?;
        let at = r.at;
        r.take(len)?;
        index.push((key, at));
    }
    Ok(index)
}


#[relax_api(handle)]
#[derive(Default)]
pub struct ShaderPackBytes(Vec<u8>);
#[relax_api]
impl ShaderPackBytes { pub fn bytes(&self) -> &[u8] { &self.0 } }

pub struct ShaderPackLookup(Option<ShaderPackEntry>);
impl ShaderPackLookup { pub fn entry(&self) -> Option<&ShaderPackEntry> { self.0.as_ref() } }

#[relax_api(handle, default)]
#[derive(Default)]
pub struct ShaderPacks {
    packs: Vec<Vec<u8>>,
    index: HashMap<u64, (usize, usize)>,
    error: String,
    installed_count: usize,
    #[cfg(not(target_arch = "wasm32"))]
    recorder: Option<ShaderPackRecorder>,
}
/// What ShaderPacks::view answers.
#[relax_api(plain)]
#[repr(C)]
pub struct ShaderPacksView<'a> {
    pub error: &'a str,
    pub installed_count: usize,
    pub recording: bool,
}

/// ShaderPacks::control's mode.
pub const SHADER_PACKS_INSTALL: u32 = 0;
pub const SHADER_PACKS_RECORD_ENABLED: u32 = 1;

#[relax_api(marked)]
impl ShaderPacks {
    pub fn new() -> Self { Self::default() }
    pub fn is_empty(&self) -> bool { self.index.is_empty() }
    #[relax_api]
    pub fn view(&self) -> ShaderPacksView<'_> {
        #[cfg(not(target_arch = "wasm32"))]
        let recording = self.recorder.is_some();
        #[cfg(target_arch = "wasm32")]
        let recording = false;
        ShaderPacksView { error: &self.error, installed_count: self.installed_count, recording }
    }
    #[inline]
    pub fn error(&self) -> &str { self.view().error }
    #[inline]
    pub fn installed_count(&self) -> usize { self.view().installed_count }
    #[cfg(not(target_arch = "wasm32"))]
    #[inline]
    pub fn is_recording(&self) -> bool { self.view().recording }
    #[inline]
    pub fn install(&mut self, bytes: &[u8]) -> bool { self.control(SHADER_PACKS_INSTALL, bytes, false) }
    #[cfg(not(target_arch = "wasm32"))]
    #[inline]
    pub fn record_enabled(&mut self, on: bool) { self.control(SHADER_PACKS_RECORD_ENABLED, &[], on); }
    /// Changes the packs by `mode`: SHADER_PACKS_INSTALL adds the pack `bytes` (false: it does not
    /// read, see `view().error`); SHADER_PACKS_RECORD_ENABLED turns recording `on` or off.
    #[relax_api]
    pub fn control(&mut self, mode: u32, bytes: &[u8], on: bool) -> bool {
        if mode == SHADER_PACKS_RECORD_ENABLED {
            #[cfg(not(target_arch = "wasm32"))]
            match (on, self.recorder.is_some()) {
                (true, false) => self.recorder = Some(ShaderPackRecorder::default()),
                (false, true) => self.recorder = None,
                _ => {}
            }
            #[cfg(target_arch = "wasm32")]
            let _ = on;
            return true;
        }
        self.error.clear();
        let index = match read_index(bytes) { Ok(index) => index, Err(error) => { self.error = error; return false; } };
        let at = self.packs.len();
        self.installed_count = index.len();
        for (key, offset) in index { self.index.insert(key, (at, offset)); }
        self.packs.push(bytes.to_vec());
        true
    }
    pub fn lookup(&self, key: u64) -> ShaderPackLookup {
        let entry = (|| {
            let (pack, offset) = *self.index.get(&key)?;
            let bytes = &self.packs[pack];
            let strings = read_strings(bytes).ok()?;
            let mut r = PackReader { bytes, at: offset };
            match r.entry(key, &strings) {
                Ok(entry) => Some(entry),
                Err(e) => { makepad_error_log::error!("shader pack: entry {:016x} does not read: {}", key, e); None }
            }
        })();
        ShaderPackLookup(entry)
    }
    #[cfg(not(target_arch = "wasm32"))]
    #[relax_api]
    pub fn take_recording(&mut self) -> ShaderPackBytes {
        let entries = match &mut self.recorder {
            Some(recorder) => { recorder.keys.clear(); std::mem::take(&mut recorder.entries) }
            None => Vec::new(),
        };
        ShaderPackBytes(write_shader_pack(&entries))
    }
    #[cfg(not(target_arch = "wasm32"))]
    #[relax_api]
    pub fn record(&mut self, vm: &mut ScriptVm, shader: ScriptObject) {
        self.error.clear();
        let Some(recorder) = self.recorder.as_mut() else { return; };
        let epoch = vm.heap().object_reuse_epoch();
        if recorder.epoch != epoch { recorder.objects.clear(); recorder.epoch = epoch; }
        if !recorder.objects.insert((vm.heap_key(), shader)) { return; }
        let walk = shader_key_walk(&vm.bx, shader, false);
        let key = walk.h.0;
        let fns = walk.fns;
        if recorder.keys.contains(&key) { return; }
        let result = record_shader_with_key(vm, shader, key, fns);
        match result.0 {
            Ok(entry) => { recorder.keys.insert(key); recorder.entries.push(entry); }
            Err(error) => { self.error = error; },
        }
    }
}
#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
struct ShaderPackRecorder {
    entries: Vec<ShaderPackEntry>, keys: HashSet<u64>, objects: HashSet<(usize, ScriptObject)>, epoch: u64,
}

#[relax_api(handle)]
pub struct ShaderPackRecord(Result<ShaderPackEntry, String>);
/// What ShaderPackRecord::view answers: the entry, or the error.
#[relax_api(plain)]
#[repr(C)]
pub struct ShaderPackRecordView<'a> {
    pub entry: *const ShaderPackEntry,
    pub error: &'a str,
    pub failed: bool,
}

#[relax_api(marked)]
impl ShaderPackRecord {
    #[relax_api]
    pub fn view(&self) -> ShaderPackRecordView<'_> {
        match &self.0 {
            Ok(entry) => ShaderPackRecordView { entry, error: "", failed: false },
            Err(error) => ShaderPackRecordView { entry: std::ptr::null(), error, failed: true },
        }
    }
    #[inline]
    pub fn entry(&self) -> Option<&ShaderPackEntry> {
        // SAFETY: the entry is the record's own, borrowed from `self`
        unsafe { self.view().entry.as_ref() }
    }
    #[inline]
    pub fn error(&self) -> Option<&str> { let v = self.view(); v.failed.then_some(v.error) }
    #[relax_api]
    pub fn bytes(&self) -> ShaderPackBytes {
        ShaderPackBytes(match self.0.as_ref().ok() { Some(entry) => write_shader_pack(std::slice::from_ref(entry)), None => Vec::new() })
    }
}
#[cfg(not(target_arch = "wasm32"))]
#[relax_api]
pub fn record_shader(vm: &mut ScriptVm, shader: ScriptObject) -> ShaderPackRecord {
    let walk = shader_key_walk(&vm.bx, shader, false);
    let key = walk.h.0;
    record_shader_with_key(vm, shader, key, walk.fns)
}
#[cfg(not(target_arch = "wasm32"))]
fn record_shader_with_key(vm: &mut ScriptVm, shader: ScriptObject, key: u64, mut fns: Vec<ScriptObject>) -> ShaderPackRecord {
    use crate::shader_compile::{compile_shader, ShaderCompileOptions};
    let opts = ShaderCompileOptions::new(crate::shader_backend::ShaderBackend::Glsl, false, false, true, false);
    let output = compile_shader(vm, shader, &opts);
    if output.has_errors() {
        let name = vm.heap().object_type_name_in_chain(shader).map(|id| id.to_string()).unwrap_or_else(|| format!("<script object {}>", shader.index()));
        return ShaderPackRecord(Err(format!("draw shader '{}' failed to compile and will NOT be drawn:\n{}", name, output.error_report())));
    }
    output.warn_inline_size(vm, shader);
    for object in output.reached_functions() { if !fns.contains(&object) { fns.push(object); } }
    let src = output.glsl().unwrap();
    let desc = DrawShaderDesc::from_output(vm.heap(), &output);
    let name = vm.heap().object_type_name_in_chain(shader).map(|id| id.to_string()).unwrap_or_default();
    ShaderPackRecord(pack_entry_from_desc(vm, shader, key, name, src.vertex().to_owned(), src.fragment().to_owned(), &desc, &fns))
}
/// What ShaderPackEntry::view answers.
#[relax_api(plain)]
#[repr(C)]
pub struct ShaderPackEntryView<'a> {
    pub key: u64,
    pub name: &'a str,
    pub vertex: &'a str,
    pub fragment: &'a str,
}
#[relax_api(marked)]
impl ShaderPackEntry {
    #[relax_api]
    pub fn view(&self) -> ShaderPackEntryView<'_> {
        ShaderPackEntryView { key: self.key, name: &self.name, vertex: &self.vertex, fragment: &self.fragment }
    }
    #[inline]
    pub fn key(&self) -> u64 { self.view().key }
    #[inline]
    pub fn name(&self) -> &str { self.view().name }
    #[inline]
    pub fn vertex(&self) -> &str { self.view().vertex }
    #[inline]
    pub fn fragment(&self) -> &str { self.view().fragment }
}

#[relax_api(handle)]
pub struct ShaderPackContents(Result<Vec<ShaderPackEntry>, String>);
/// What ShaderPackContents::view answers.
#[relax_api(plain)]
#[repr(C)]
pub struct ShaderPackContentsView<'a> {
    pub entries: &'a [ShaderPackEntry],
    pub error: &'a str,
    pub failed: bool,
}
#[relax_api(marked)]
impl ShaderPackContents {
    #[relax_api]
    pub fn view(&self) -> ShaderPackContentsView<'_> {
        match &self.0 {
            Ok(entries) => ShaderPackContentsView { entries, error: "", failed: false },
            Err(error) => ShaderPackContentsView { entries: &[], error, failed: true },
        }
    }
    #[inline]
    pub fn len(&self) -> usize { self.view().entries.len() }
    #[inline]
    pub fn entry(&self, index: usize) -> Option<&ShaderPackEntry> { self.view().entries.get(index) }
    #[inline]
    pub fn error(&self) -> Option<&str> { let v = self.view(); v.failed.then_some(v.error) }
}
#[relax_api]
pub fn inspect_shader_pack(bytes: &[u8]) -> ShaderPackContents { ShaderPackContents(read_shader_pack(bytes)) }

#[cfg(not(target_arch = "wasm32"))]
fn write_value_to_f32_slots(
        heap: &ScriptHeap,
        value: ScriptValue,
        output: &mut [f32],
        offset: usize,
        slots: usize,
    ) {
        // Try f64 first (most common for abstract numbers)
        if let Some(v) = value.as_f64() {
            let v = v as f32;
            for i in 0..slots {
                output[offset + i] = v;
            }
            return;
        }

        // Try u40 (common integer format in script)
        if let Some(v) = value.as_u40() {
            let v = v as f32;
            for i in 0..slots {
                output[offset + i] = v;
            }
            return;
        }

        // Try f32
        if let Some(v) = value.as_f32() {
            let v = v;
            for i in 0..slots {
                output[offset + i] = v;
            }
            return;
        }

        // Try f16
        if let Some(v) = value.as_f16() {
            let v = v;
            for i in 0..slots {
                output[offset + i] = v;
            }
            return;
        }

        // Try u32/i32
        if let Some(v) = value.as_u32() {
            let v = v as f32;
            for i in 0..slots {
                output[offset + i] = v;
            }
            return;
        }
        if let Some(v) = value.as_i32() {
            let v = v as f32;
            for i in 0..slots {
                output[offset + i] = v;
            }
            return;
        }

        // Try bool
        if let Some(v) = value.as_bool() {
            let v = {
                    if v {
                        1.0
                    } else {
                        0.0
                    }
                };
            for i in 0..slots {
                output[offset + i] = v;
            }
            return;
        }

        // Try color (u32 RGBA)
        if let Some(c) = value.as_color() {
            let v = makepad_math::Vec4f::from_u32(c);
            if slots >= 1 {
                output[offset + 0] = v.x;
            }
            if slots >= 2 {
                output[offset + 1] = v.y;
            }
            if slots >= 3 {
                output[offset + 2] = v.z;
            }
            if slots >= 4 {
                output[offset + 3] = v.w;
            }
            return;
        }

        // An animator state's `snap(..)`: an object standing for the value
        // it carries. A state the animator plays is resolved before it gets
        // here, but a state applied as a widget's default (`hover: {default:
        // @down}`) is handed over as written, and a slot that skipped it
        // left the widget in the state before.
        if let Some(obj) = value.as_object() {
            if heap.has_apply_transform(value) {
                let carried = heap.value(obj, live_id!(value).into(), NoTrap);
                if !carried.is_nil() && !carried.is_err() && carried.as_object() != Some(obj) {
                    write_value_to_f32_slots(heap, carried, output, offset, slots);
                    return;
                }
            }
        }

        // Try repr(u32) enum variant objects used by script APIs.
        // These carry the numeric payload in `_repr_u32_enum_value`.
        if let Some(obj) = value.as_object() {
            let enum_value = heap.value(obj, live_id!(_repr_u32_enum_value).into(), NoTrap);
            if let Some(v) = enum_value.as_f64() {
                let v = v as f32;
                for i in 0..slots {
                    output[offset + i] = v;
                }
                return;
            }
            if let Some(v) = enum_value.as_u32() {
                let v = v as f32;
                for i in 0..slots {
                    output[offset + i] = v;
                }
                return;
            }
        }

        // Try pod (Vec2f, Vec3f, Vec4f, etc.)
        if let Some(pod) = value.as_pod() {
            let (pod_type, data) = heap.pod_data(pod);

            match pod_type.ty().kind() {
                crate::pod::ScriptPodTyTag::F32 => {
                    let v = f32::from_bits(data[0]);
                    for i in 0..slots {
                        output[offset + i] = v;
                    }
                }
                crate::pod::ScriptPodTyTag::F16 => {
                    let v = pod_heap::f16_to_f32(data[0] as u16);
                    let v = v;
                    for i in 0..slots {
                        output[offset + i] = v;
                    }
                }
                crate::pod::ScriptPodTyTag::U32 | crate::pod::ScriptPodTyTag::AtomicU32 => {
                    let v = data[0] as f32;
                    for i in 0..slots {
                        output[offset + i] = v;
                    }
                }
                crate::pod::ScriptPodTyTag::I32 | crate::pod::ScriptPodTyTag::AtomicI32 => {
                    let v = data[0] as i32 as f32;
                    for i in 0..slots {
                        output[offset + i] = v;
                    }
                }
                crate::pod::ScriptPodTyTag::Bool => {
                    let v = {
                            if data[0] != 0 {
                                1.0
                            } else {
                                0.0
                            }
                        };
                    for i in 0..slots {
                        output[offset + i] = v;
                    }
                }
                crate::pod::ScriptPodTyTag::Vec => {
                    let vec_ty = pod_type.ty().as_vec().unwrap();
                    let dims = vec_ty.dims();
                    match vec_ty {
                        ScriptPodVec::Vec2f | ScriptPodVec::Vec3f | ScriptPodVec::Vec4f => {
                            for i in 0..dims.min(slots) {
                                output[offset + i] = f32::from_bits(data[i]);
                            }
                        }
                        ScriptPodVec::Vec2h | ScriptPodVec::Vec3h | ScriptPodVec::Vec4h => {
                            for i in 0..dims.min(slots) {
                                if i & 1 == 1 {
                                    output[offset + i] =
                                        pod_heap::f16_to_f32((data[i >> 1] >> 16) as u16);
                                } else {
                                    output[offset + i] = pod_heap::f16_to_f32(data[i >> 1] as u16);
                                }
                            }
                        }
                        ScriptPodVec::Vec2u | ScriptPodVec::Vec3u | ScriptPodVec::Vec4u => {
                            for i in 0..dims.min(slots) {
                                output[offset + i] = data[i] as f32;
                            }
                        }
                        ScriptPodVec::Vec2i | ScriptPodVec::Vec3i | ScriptPodVec::Vec4i => {
                            for i in 0..dims.min(slots) {
                                output[offset + i] = data[i] as i32 as f32;
                            }
                        }
                        ScriptPodVec::Vec2b | ScriptPodVec::Vec3b | ScriptPodVec::Vec4b => {
                            for i in 0..dims.min(slots) {
                                output[offset + i] = {
                                        if data[i] != 0 {
                                            1.0
                                        } else {
                                            0.0
                                        }
                                    };
                            }
                        }
                    }
                }
                crate::pod::ScriptPodTyTag::Mat => {
                    let mat_ty = pod_type.ty().as_mat().unwrap();
                    // Matrices are stored as f32 arrays (column-major order)
                    let dim = mat_ty.dim();
                    for i in 0..dim.min(slots) {
                        output[offset + i] = f32::from_bits(data[i]);
                    }
                }
                _ => {
                    // For other pod types, default to 0
                    for i in 0..slots {
                        output[offset + i] = 0.0;
                    }
                }
            }
            return;
        }

        // Default: fill with zeros
        for i in 0..slots {
            output[offset + i] = 0.0;
        }
    }
#[cfg(not(target_arch = "wasm32"))]
fn write_pack_value(heap: &ScriptHeap, value: ScriptValue, output: &mut [f32]) { write_value_to_f32_slots(heap, value, output, 0, output.len()); }

/// Compare reflected POD layouts without exporting the pack serialization model.
#[relax_api]
pub fn same_pod_structure(a: &ScriptPodTy, b: &ScriptPodTy) -> bool { pod_structure(None, a) == pod_structure(None, b) }
