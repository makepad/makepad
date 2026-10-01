//! Shader packs: draw shaders compiled ahead of time. A pack holds, per
//! draw shader, its GLSL ES 3.0 code (what the WebGL2 backend runs) and the
//! compiler's reflection of it (the io with its pod types, samplers, uniform
//! buffer bindings, scope uniforms, table constants: everything the draw
//! path builds the shader's mapping from), serialised into versioned bytes
//! and keyed by a hash of the shader's source.
//!
//! - Recording (native): [`Cx::record_shader_pack`] makes every draw shader
//!   that is applied also run the web build's GLSL compile and keep the
//!   result; [`Cx::take_shader_pack`] serialises it.
//! - Use: [`Cx::install_shader_pack`]. The web build's draw-shader compile
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

use {
    crate::{
        cx::Cx,
        draw_shader::{
            DrawShaderDesc, DrawShaderDescIo, DrawShaderDescScope,
            DrawShaderDescScopeUniform,
        },
        makepad_live_id::*,
        makepad_script::heap::ScriptHeap,
        makepad_script::pod::{
            ScriptPodField, ScriptPodMat, ScriptPodPacked, ScriptPodTy, ScriptPodTypeData, ScriptPodTypeInline,
            ScriptPodVec,
        },
        makepad_script::shader::{
            SamplerAddress, SamplerCoord, SamplerFilter, ShaderIoKind, ShaderSampler, ShaderStorageFlags,
            ShaderTableConst, TextureType, UniformBufferBindings,
        },
        makepad_script::tokenizer::ScriptToken,
        makepad_script::value::{ScriptObject, ScriptPodType, ScriptValue},
        makepad_script::*,
    },
    std::{borrow::Cow, collections::HashMap, collections::HashSet},
};

#[cfg(not(target_arch = "wasm32"))]
use crate::{draw_shader::DrawShaderAttrFormat, draw_vars::DrawVars, script::vm::*};

/// Bumped whenever the key's recipe or the pack's layout changes; a pack of
/// another version is refused.
const SHADER_PACK_VERSION: u32 = 1;
const SHADER_PACK_MAGIC: &[u8; 4] = b"MPSP";

/// One draw shader of a pack: its GLSL ES 3.0 code and reflection.
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

/// The installed shader packs and, on native, the recording.
#[derive(Default)]
pub struct CxShaderPacks {
    packs: Vec<Cow<'static, [u8]>>,
    /// Key -> (pack, entry offset).
    index: HashMap<u64, (usize, usize)>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) recorder: Option<ShaderPackRecorder>,
}

impl CxShaderPacks {
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// The entry for `key`, deserialised from its pack.
    pub fn entry(&self, key: u64) -> Option<ShaderPackEntry> {
        let (pack, offset) = *self.index.get(&key)?;
        let bytes = &self.packs[pack];
        let strings = read_strings(bytes).ok()?;
        let mut r = PackReader { bytes, at: offset };
        match r.entry(key, &strings) {
            Ok(entry) => Some(entry),
            Err(e) => {
                crate::error!("shader pack: entry {:016x} does not read: {}", key, e);
                None
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
pub(crate) struct ShaderPackRecorder {
    entries: Vec<ShaderPackEntry>,
    keys: HashSet<u64>,
    /// Shader objects already recorded (by heap and object), valid for
    /// `epoch` (the heap's object reuse epoch).
    objects: HashSet<(usize, ScriptObject)>,
    epoch: u64,
}

impl Cx {
    /// Make every draw shader applied from now on also compile to GLSL ES
    /// 3.0, as the web build does, and keep it for the pack. Turn it on
    /// before the shaders to record are applied; off drops what was
    /// recorded and not taken.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn record_shader_pack(&mut self, on: bool) {
        let packs = &mut self.draw_shaders.packs;
        match (on, packs.recorder.is_some()) {
            (true, false) => packs.recorder = Some(ShaderPackRecorder::default()),
            (false, true) => packs.recorder = None,
            _ => {}
        }
    }

    /// The shaders recorded since recording started or since the last take,
    /// as a pack (in the order they were first applied).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn take_shader_pack(&mut self) -> Vec<u8> {
        let entries = match &mut self.draw_shaders.packs.recorder {
            Some(recorder) => {
                recorder.keys.clear();
                std::mem::take(&mut recorder.entries)
            }
            None => Vec::new(),
        };
        write_shader_pack(&entries)
    }

    /// Install a shader pack (bytes from [`Cx::take_shader_pack`]; usually
    /// embedded in the binary): draw shaders whose key it holds bind from it
    /// without a compile. Install before the first shader is applied; a
    /// later pack's entry wins over an earlier one's. Returns the number of
    /// entries. The web backend consults packs; native backends compile
    /// their own code.
    pub fn install_shader_pack(&mut self, pack: impl Into<Cow<'static, [u8]>>) -> Result<usize, String> {
        let pack = pack.into();
        let index = read_index(&pack)?;
        let packs = &mut self.draw_shaders.packs;
        let at = packs.packs.len();
        let count = index.len();
        for (key, offset) in index {
            packs.index.insert(key, (at, offset));
        }
        packs.packs.push(pack);
        Ok(count)
    }
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

/// `key` looked up from `obj` the way a scope lookup does (the object's
/// map, its vec, then its prototypes), without raising an error on a miss.
pub(crate) fn find_scope_value(heap: &ScriptHeap, obj: ScriptObject, key: LiveId) -> Option<ScriptValue> {
    let key: ScriptValue = key.into();
    let mut cur = obj;
    loop {
        let data = heap.object_data(cur);
        if let Some(v) = data.map.get(&key) {
            return Some(v.value);
        }
        if data.tag.is_vec2() {
            if let Some(kv) = data.vec.iter().rev().find(|kv| kv.key == key) {
                return Some(kv.value);
            }
        }
        cur = data.proto.as_object()?;
    }
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
        &self.bx.heap
    }

    fn pod(&mut self, ty: &ScriptPodTy) {
        self.h.u8(K_POD);
        match ty {
            ScriptPodTy::Void => self.h.u8(0),
            ScriptPodTy::ArrayBuilder => self.h.u8(1),
            ScriptPodTy::UndefinedStruct => self.h.u8(2),
            ScriptPodTy::F32 => self.h.u8(3),
            ScriptPodTy::F16 => self.h.u8(4),
            ScriptPodTy::U32 => self.h.u8(5),
            ScriptPodTy::I32 => self.h.u8(6),
            ScriptPodTy::Bool => self.h.u8(7),
            ScriptPodTy::AtomicU32 => self.h.u8(8),
            ScriptPodTy::AtomicI32 => self.h.u8(9),
            ScriptPodTy::Vec(v) => {
                self.h.u8(10);
                self.h.u8(*v as u8);
            }
            ScriptPodTy::Mat(m) => {
                self.h.u8(11);
                self.h.u8(*m as u8);
            }
            ScriptPodTy::Packed(p) => {
                self.h.u8(12);
                self.h.u8(*p as u8);
            }
            ScriptPodTy::Struct { align_of, size_of, fields } => {
                self.h.u8(13);
                self.h.u64(*align_of as u64);
                self.h.u64(*size_of as u64);
                for field in fields {
                    self.h.id(field.name);
                    self.pod(&field.ty.data.ty);
                }
            }
            ScriptPodTy::Enum { align_of, size_of, variants } => {
                self.h.u8(14);
                self.h.u64(*align_of as u64);
                self.h.u64(*size_of as u64);
                for variant in variants {
                    self.h.id(variant.name);
                }
            }
            ScriptPodTy::FixedArray { align_of, size_of, len, ty } => {
                self.h.u8(15);
                self.h.u64(*align_of as u64);
                self.h.u64(*size_of as u64);
                self.h.u64(*len as u64);
                self.pod(&ty.data.ty);
            }
            ScriptPodTy::VariableArray { align_of, ty } => {
                self.h.u8(16);
                self.h.u64(*align_of as u64);
                self.pod(&ty.data.ty);
            }
        }
    }

    /// The pod type a value is or carries, if any.
    fn value_pod(&self, v: ScriptValue) -> Option<&'a ScriptPodTy> {
        let heap = self.heap();
        if let Some(pod) = v.as_pod() {
            return Some(&heap.pod_data(pod).0.ty);
        }
        let ty = v.as_pod_type().or_else(|| heap.pod_type(v))?;
        Some(&heap.pod_type_ref(ty).ty)
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
        let ip = match heap.as_fn(fnobj) {
            Some(ScriptFnPtr::Script(ip)) => ip,
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
        let mut names: Vec<(LiveId, Option<LiveId>)> = Vec::new();
        let mut last: Option<LiveId> = None;
        let mut dot = false;
        let mut inlined = Vec::new();
        let h = &mut self.h;
        bx.code.fn_tokens(ip, |tok, doc, value| {
            if let Some(doc) = doc {
                h.u8(K_DOC);
                h.bytes(doc.as_bytes());
            }
            h.u8(K_TOKEN);
            h.u8(tok.preceded_by_newline as u8);
            match &tok.token {
                ScriptToken::Identifier(id) => {
                    h.u8(1);
                    h.id(*id);
                }
                ScriptToken::Operator(id) => {
                    h.u8(2);
                    h.id(*id);
                }
                ScriptToken::Separator(id) => {
                    h.u8(3);
                    h.id(*id);
                }
                ScriptToken::OpenCurly => h.u8(4),
                ScriptToken::CloseCurly => h.u8(5),
                ScriptToken::OpenRound => h.u8(6),
                ScriptToken::CloseRound => h.u8(7),
                ScriptToken::OpenSquare => h.u8(8),
                ScriptToken::CloseSquare => h.u8(9),
                ScriptToken::String(s) => {
                    h.u8(10);
                    heap.string_with(*s, |_, s| h.bytes(s.as_bytes()));
                }
                ScriptToken::F32(v) | ScriptToken::F16(v) => {
                    h.u8(11);
                    h.u64(v.to_bits() as u64);
                }
                ScriptToken::F64(v) => {
                    h.u8(12);
                    h.u64(v.to_bits());
                }
                ScriptToken::U32(v) => {
                    h.u8(13);
                    h.u64(*v as u64);
                }
                ScriptToken::I32(v) => {
                    h.u8(14);
                    h.u64(*v as u64);
                }
                ScriptToken::U40(v) => {
                    h.u8(15);
                    h.u64(*v);
                }
                ScriptToken::Color(v) => {
                    h.u8(16);
                    h.u64(*v as u64);
                }
                ScriptToken::RustValue(_) => {
                    h.u8(17);
                    inlined.push(value.unwrap_or(ScriptValue::NIL));
                }
                ScriptToken::End | ScriptToken::StreamEnd | ScriptToken::StringUnfinished => h.u8(18),
            }
            // Names (and `a.b` pairs) to resolve once the text is hashed.
            match tok.token {
                ScriptToken::Identifier(id) => {
                    if let (true, Some(a)) = (dot, last) {
                        names.push((a, Some(id)));
                    }
                    names.push((id, None));
                    last = Some(id);
                    dot = false;
                }
                ScriptToken::Operator(op) if op == id!(.) => dot = last.is_some(),
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
    let heap = &bx.heap;
    w.h.u64(SHADER_PACK_VERSION as u64);
    w.h.u8(const_table as u8);
    for key in [id!(color_format), id!(blend_op)] {
        w.h.id(key);
        w.h.id(heap.value(io_self, key.into(), NoTrap).as_id().unwrap_or(LiveId(0)));
    }
    let mut cur = Some(io_self);
    while let Some(obj) = cur {
        w.h.u8(K_PROTO);
        let data = heap.object_data(obj);
        if let Some(ty_index) = data.tag.as_type_index() {
            w.h.u8(K_RUST_FIELDS);
            for (field, type_id) in heap.type_check(ty_index).props.iter_rust_instance_ordered() {
                w.h.id(field);
                match heap.type_id_to_pod_type(type_id, &bx.code.builtins.pod) {
                    Some(ty) => w.pod(&heap.pod_type_ref(ty).ty),
                    None => w.h.u8(0),
                }
            }
        }
        let mut fields = Vec::new();
        data.map_iter_ordered(|k, v| fields.push((k, v)));
        for (k, v) in fields {
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
pub fn shader_pack_key(vm: &ScriptVm, io_self: ScriptObject, const_table: bool) -> u64 {
    shader_key_walk(&vm.bx, io_self, const_table).h.0
}

// ---------------------------------------------------------------------------
// Entry <-> reflection.

impl ShaderPackEntry {
    /// The reflection of this shader for the shader object `io_self` of
    /// this heap (its scope uniforms are found again from it), for
    /// `CxDrawShaderMapping::from_desc`.
    pub fn to_desc(&self, heap: &ScriptHeap, io_self: ScriptObject) -> DrawShaderDesc {
        let tys: Vec<ScriptPodTy> = (0..self.pod_types.len()).map(|i| pod_from_pack(&self.pod_types, i as u32)).collect();
        let io = self
            .io
            .iter()
            .map(|io| {
                let name = LiveId(io.name);
                let kind = match io.kind {
                    PackIoKind::StorageBuffer { read, write } => {
                        let mut flags = ShaderStorageFlags::default();
                        if read {
                            flags.set_read();
                        }
                        if write {
                            flags.set_write();
                        }
                        ShaderIoKind::StorageBuffer(flags)
                    }
                    PackIoKind::UniformBuffer => ShaderIoKind::UniformBuffer,
                    PackIoKind::Sampler => ShaderIoKind::Sampler(Default::default()),
                    PackIoKind::Texture(t) => ShaderIoKind::Texture(t),
                    PackIoKind::Varying => ShaderIoKind::Varying,
                    PackIoKind::VertexBuffer => ShaderIoKind::VertexBuffer,
                    PackIoKind::VertexPosition => ShaderIoKind::VertexPosition,
                    PackIoKind::FragmentOutput(i) => ShaderIoKind::FragmentOutput(i),
                    PackIoKind::RustInstance => ShaderIoKind::RustInstance,
                    PackIoKind::Uniform => ShaderIoKind::Uniform,
                    PackIoKind::DynInstance => ShaderIoKind::DynInstance,
                    PackIoKind::ScopeUniform => ShaderIoKind::ScopeUniform,
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
                    ty: tys.get(io.ty as usize).cloned().unwrap_or_default(),
                    pod_name: (pod_name != 0).then_some(LiveId(pod_name)),
                    pod_type,
                    buffer_index: io.buffer_index.map(|i| i as usize),
                    block_name: io.block_name.clone(),
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
                    PackScopeSource::Fixed(values) => DrawShaderDescScope::Fixed(values.clone()),
                    PackScopeSource::Path { path, key, recorded } => {
                        let key = LiveId(*key);
                        match resolve_path(heap, io_self, path) {
                            Some(obj) if find_scope_value(heap, obj, key).is_some() => DrawShaderDescScope::Scope(obj, key),
                            _ => DrawShaderDescScope::Fixed(recorded.clone()),
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
                .map(|tc| ShaderTableConst {
                    shader_name: LiveId(tc.shader_name),
                    doc: tc.doc.clone(),
                    value: tc.value,
                    ip: Default::default(),
                })
                .collect(),
            uniform_buffer_bindings: UniformBufferBindings {
                bindings: self.uniform_buffer_bindings.iter().map(|(name, index)| (LiveId(*name), *index as usize)).collect(),
                scope_uniform_buffer_index: self.scope_uniform_buffer_index.map(|i| i as usize),
            },
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
    let kind = match &data.ty {
        ScriptPodTy::Void => PackPodKind::Void,
        ScriptPodTy::F32 => PackPodKind::F32,
        ScriptPodTy::F16 => PackPodKind::F16,
        ScriptPodTy::U32 => PackPodKind::U32,
        ScriptPodTy::I32 => PackPodKind::I32,
        ScriptPodTy::Bool => PackPodKind::Bool,
        ScriptPodTy::AtomicU32 => PackPodKind::AtomicU32,
        ScriptPodTy::AtomicI32 => PackPodKind::AtomicI32,
        ScriptPodTy::Vec(v) => PackPodKind::Vec(*v),
        ScriptPodTy::Mat(m) => PackPodKind::Mat(*m),
        ScriptPodTy::Packed(p) => PackPodKind::Packed(*p),
        ScriptPodTy::Struct { align_of, size_of, fields } => {
            let mut packed = Vec::with_capacity(fields.len());
            for field in fields {
                packed.push((field.name.0, pod_to_pack(&field.ty.data, out)?));
            }
            PackPodKind::Struct { align_of: *align_of as u32, size_of: *size_of as u32, fields: packed }
        }
        ScriptPodTy::FixedArray { align_of, size_of, len, ty } => PackPodKind::FixedArray {
            align_of: *align_of as u32,
            size_of: *size_of as u32,
            len: *len as u32,
            ty: pod_to_pack(&ty.data, out)?,
        },
        ScriptPodTy::VariableArray { align_of, ty } => {
            PackPodKind::VariableArray { align_of: *align_of as u32, ty: pod_to_pack(&ty.data, out)? }
        }
        ScriptPodTy::ArrayBuilder | ScriptPodTy::UndefinedStruct | ScriptPodTy::Enum { .. } => return None,
    };
    let entry = PackPodType { name: data.name.map(|n| n.0).unwrap_or(0), kind };
    Some(match out.iter().position(|t| *t == entry) {
        Some(i) => i as u32,
        None => {
            out.push(entry);
            (out.len() - 1) as u32
        }
    })
}

fn pod_from_pack(types: &[PackPodType], index: u32) -> ScriptPodTy {
    let inline = |index: u32| ScriptPodTypeInline {
        self_ref: ScriptPodType::VOID,
        data: ScriptPodTypeData {
            name: types.get(index as usize).and_then(|t| (t.name != 0).then_some(LiveId(t.name))),
            ty: pod_from_pack(types, index),
            ..Default::default()
        },
    };
    let Some(ty) = types.get(index as usize) else {
        return ScriptPodTy::Void;
    };
    match &ty.kind {
        PackPodKind::Void => ScriptPodTy::Void,
        PackPodKind::F32 => ScriptPodTy::F32,
        PackPodKind::F16 => ScriptPodTy::F16,
        PackPodKind::U32 => ScriptPodTy::U32,
        PackPodKind::I32 => ScriptPodTy::I32,
        PackPodKind::Bool => ScriptPodTy::Bool,
        PackPodKind::AtomicU32 => ScriptPodTy::AtomicU32,
        PackPodKind::AtomicI32 => ScriptPodTy::AtomicI32,
        PackPodKind::Vec(v) => ScriptPodTy::Vec(*v),
        PackPodKind::Mat(m) => ScriptPodTy::Mat(*m),
        PackPodKind::Packed(p) => ScriptPodTy::Packed(*p),
        PackPodKind::Struct { align_of, size_of, fields } => ScriptPodTy::Struct {
            align_of: *align_of as usize,
            size_of: *size_of as usize,
            fields: fields
                .iter()
                .map(|(name, ty)| ScriptPodField { name: LiveId(*name), default: ScriptValue::NIL, ty: inline(*ty) })
                .collect(),
        },
        PackPodKind::FixedArray { align_of, size_of, len, ty } => ScriptPodTy::FixedArray {
            align_of: *align_of as usize,
            size_of: *size_of as usize,
            len: *len as usize,
            ty: Box::new(inline(*ty)),
        },
        PackPodKind::VariableArray { align_of, ty } => {
            ScriptPodTy::VariableArray { align_of: *align_of as usize, ty: Box::new(inline(*ty)) }
        }
    }
}

/// A pod type's structure, comparable across heaps (names, fields, sizes):
/// what a pack keeps of it.
pub fn pod_structure(name: Option<LiveId>, ty: &ScriptPodTy) -> Option<(Vec<PackPodType>, u32)> {
    let mut out = Vec::new();
    let data = ScriptPodTypeData { name, ty: ty.clone(), ..Default::default() };
    let index = pod_to_pack(&data, &mut out)?;
    Some((out, index))
}

// ---------------------------------------------------------------------------
// Recording (native).

#[cfg(not(target_arch = "wasm32"))]
impl DrawVars {
    /// While a shader pack is recorded: record the shader object `io_self`
    /// if its key is new (the web build's GLSL compile with its
    /// reflection). Runs beside, not instead of, this backend's compile.
    pub(crate) fn record_shader_pack_entry(vm: &mut ScriptVm, io_self: ScriptObject) {
        let heap_key = vm.bx.heap.heap_key();
        let epoch = vm.bx.heap.object_reuse_epoch();
        {
            let Some(recorder) = vm.host.cx_mut().draw_shaders.packs.recorder.as_mut() else {
                return;
            };
            if recorder.epoch != epoch {
                recorder.objects.clear();
                recorder.epoch = epoch;
            }
            if !recorder.objects.insert((heap_key, io_self)) {
                return;
            }
        }
        // Recorded as the web build compiles: no remote bridge there, so
        // the const table is off.
        let walk = shader_key_walk(&vm.bx, io_self, false);
        let key = walk.h.0;
        let mut fns = walk.fns;
        if vm.host.cx().draw_shaders.packs.recorder.as_ref().is_some_and(|r| r.keys.contains(&key)) {
            return;
        }
        let Some((output, code)) = DrawVars::compile_glsl(vm, io_self, false) else {
            return;
        };
        let desc = DrawShaderDesc::from_output(&vm.bx.heap, &output);
        let crate::draw_shader::CxDrawShaderCode::Separate { vertex, fragment } = code else {
            return;
        };
        let name = vm
            .bx
            .heap
            .object_type_name_in_chain(io_self)
            .map(|id| id.to_string())
            .unwrap_or_default();
        // The functions the compile reached name the scope-uniform paths.
        for f in &output.functions {
            if !fns.contains(&f.fnobj) {
                fns.push(f.fnobj);
            }
        }
        match pack_entry_from_desc(vm, io_self, key, name.clone(), vertex, fragment, &desc, &fns) {
            Ok(entry) => {
                if let Some(recorder) = vm.host.cx_mut().draw_shaders.packs.recorder.as_mut() {
                    recorder.keys.insert(key);
                    recorder.entries.push(entry);
                }
            }
            Err(e) => crate::error!("shader pack: '{}' cannot be recorded: {}", name, e),
        }
    }
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
    let heap = &vm.bx.heap;
    let mut pod_types = Vec::new();
    let mut io = Vec::new();
    for d in &desc.io {
        let data = ScriptPodTypeData { name: d.pod_name, ty: d.ty.clone(), ..Default::default() };
        let ty = pod_to_pack(&data, &mut pod_types)
            .ok_or_else(|| format!("io `{}` has a pod type a pack cannot carry", d.name))?;
        let kind = match &d.kind {
            ShaderIoKind::StorageBuffer(flags) => PackIoKind::StorageBuffer { read: flags.is_read(), write: flags.is_write() },
            ShaderIoKind::UniformBuffer => PackIoKind::UniformBuffer,
            ShaderIoKind::Sampler(_) => PackIoKind::Sampler,
            ShaderIoKind::Texture(t) => PackIoKind::Texture(*t),
            ShaderIoKind::Varying => PackIoKind::Varying,
            ShaderIoKind::VertexBuffer => PackIoKind::VertexBuffer,
            ShaderIoKind::VertexPosition => PackIoKind::VertexPosition,
            ShaderIoKind::FragmentOutput(i) => PackIoKind::FragmentOutput(*i),
            ShaderIoKind::RustInstance => PackIoKind::RustInstance,
            ShaderIoKind::Uniform => PackIoKind::Uniform,
            ShaderIoKind::DynInstance => PackIoKind::DynInstance,
            ShaderIoKind::ScopeUniform => PackIoKind::ScopeUniform,
        };
        io.push(PackIo {
            kind,
            name: d.name.0,
            ty,
            buffer_index: d.buffer_index.map(|i| i as u32),
            block_name: d.block_name.clone(),
            sampler: d.sampler as u32,
        });
    }
    let mut paths = ScopePaths::new(&vm.bx, io_self, fns);
    let mut scope_uniforms = Vec::new();
    for su in &desc.scope_uniforms {
        let source = match &su.source {
            DrawShaderDescScope::Const(i) => PackScopeSource::Const(*i as u32),
            DrawShaderDescScope::Fixed(values) => PackScopeSource::Fixed(values.clone()),
            DrawShaderDescScope::Scope(obj, key) => {
                let mut recorded = vec![0.0f32; su.slots];
                let value = heap.scope_value(*obj, *key, NoTrap);
                DrawVars::write_value_to_f32_slots(heap, value, &mut recorded, 0, su.slots, DrawShaderAttrFormat::Float);
                match paths.find(*obj) {
                    Some(path) => PackScopeSource::Path { path, key: key.0, recorded },
                    None => {
                        crate::warning!(
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
            .map(|tc| PackTableConst { shader_name: tc.shader_name.0, doc: tc.doc.clone(), value: tc.value })
            .collect(),
        uniform_buffer_bindings: desc.uniform_buffer_bindings.bindings.iter().map(|(n, i)| (n.0, *i as u32)).collect(),
        scope_uniform_buffer_index: desc.uniform_buffer_bindings.scope_uniform_buffer_index.map(|i| i as u32),
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
        let heap = &bx.heap;
        let mut names = Vec::new();
        let mut known = HashSet::new();
        for f in fns {
            if let Some(ScriptFnPtr::Script(ip)) = heap.as_fn(*f) {
                bx.code.fn_tokens(ip, |tok, _, _| {
                    if let ScriptToken::Identifier(id) = tok.token {
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
            heap.object_data(obj).map_iter_ordered(|k, _| {
                if let Some(id) = k.as_id() {
                    if known.insert(id) {
                        names.push(id);
                    }
                }
            });
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

// ---------------------------------------------------------------------------
// Verification (native).

/// Check that a shader pack reproduces a compile for the shader object
/// `io_self`: compile it to GLSL as the web build does, put the result
/// through a pack (bytes and back), build the mapping from that and compare
/// it, field by field, with the mapping the compile itself gives. Returns
/// the shader's key, or what differs.
#[cfg(not(target_arch = "wasm32"))]
pub fn verify_shader_pack_entry(vm: &mut ScriptVm, io_self: ScriptObject) -> Result<u64, String> {
    use crate::draw_shader::{CxDrawShaderCode, CxDrawShaderMapping};
    let walk = shader_key_walk(&vm.bx, io_self, false);
    let key = walk.h.0;
    let mut fns = walk.fns;
    let (output, code) = DrawVars::compile_glsl(vm, io_self, false).ok_or("the shader does not compile")?;
    for f in &output.functions {
        if !fns.contains(&f.fnobj) {
            fns.push(f.fnobj);
        }
    }
    let source = vm.bx.heap.new_object_ref(io_self);
    let live = CxDrawShaderMapping::from_shader_output(source, code.clone(), &vm.bx.heap, &output, None);
    let desc = DrawShaderDesc::from_output(&vm.bx.heap, &output);
    let CxDrawShaderCode::Separate { vertex, fragment } = code.clone() else {
        return Err("not separate code".into());
    };
    let entry = pack_entry_from_desc(vm, io_self, key, String::new(), vertex, fragment, &desc, &fns)?;
    let bytes = write_shader_pack(std::slice::from_ref(&entry));
    let read = read_shader_pack(&bytes)?;
    let [back] = &read[..] else {
        return Err("the pack does not hold one entry".into());
    };
    if *back != entry {
        return Err("the entry does not read back as written".into());
    }
    let back_code = CxDrawShaderCode::Separate { vertex: back.vertex.clone(), fragment: back.fragment.clone() };
    if back_code != code {
        return Err("the pack's code differs from the compile's".into());
    }
    let source = vm.bx.heap.new_object_ref(io_self);
    let mut packed = CxDrawShaderMapping::from_desc(source, back_code, &vm.bx.heap, &back.to_desc(&vm.bx.heap, io_self), None);
    let mut live = live;
    let trap = NoTrap;
    live.fill_scope_uniforms_buffer(&vm.bx.heap, &trap);
    packed.fill_scope_uniforms_buffer(&vm.bx.heap, &trap);
    mapping_difference(&live, &packed).map_or(Ok(key), Err)
}

/// The first field two mappings differ in (pod types compared by
/// structure, table constants without their source position).
#[cfg(not(target_arch = "wasm32"))]
fn mapping_difference(a: &crate::draw_shader::CxDrawShaderMapping, b: &crate::draw_shader::CxDrawShaderMapping) -> Option<String> {
    fn same<T: std::fmt::Debug>(field: &str, a: T, b: T) -> Result<(), String> {
        let (a, b) = (format!("{:?}", a), format!("{:?}", b));
        if a == b {
            Ok(())
        } else {
            Err(format!("`{}` differs:\n  compile: {}\n  pack:    {}", field, a, b))
        }
    }
    let reflected = |ios: &[crate::draw_shader_layout::ReflectedIo]| -> Vec<(LiveId, Option<(Vec<PackPodType>, u32)>)> {
        ios.iter().map(|io| (io.name, pod_structure(None, &io.ty))).collect()
    };
    let check = || -> Result<(), String> {
        same("code", a.code == b.code, true)?;
        same("flags", a.flags, b.flags)?;
        same("instances", &a.instances, &b.instances)?;
        same("dyn_instances", &a.dyn_instances, &b.dyn_instances)?;
        same("dyn_uniforms", &a.dyn_uniforms, &b.dyn_uniforms)?;
        same("geometries", &a.geometries, &b.geometries)?;
        same(
            "textures",
            a.textures.iter().map(|t| (t.id, t.tex_type)).collect::<Vec<_>>(),
            b.textures.iter().map(|t| (t.id, t.tex_type)).collect::<Vec<_>>(),
        )?;
        same("uniform_buffers", &a.uniform_buffers, &b.uniform_buffers)?;
        same("samplers", &a.samplers, &b.samplers)?;
        same("texture_sampler_indices", &a.texture_sampler_indices, &b.texture_sampler_indices)?;
        same("uses_time", a.uses_time, b.uses_time)?;
        same("rect_pos", a.rect_pos, b.rect_pos)?;
        same("rect_size", a.rect_size, b.rect_size)?;
        same("draw_clip", a.draw_clip, b.draw_clip)?;
        same("draw_depth", a.draw_depth, b.draw_depth)?;
        same("uniform_buffer_bindings", &a.uniform_buffer_bindings, &b.uniform_buffer_bindings)?;
        same("scope_uniforms", &a.scope_uniforms, &b.scope_uniforms)?;
        same("scope_uniform_sources", &a.scope_uniform_sources, &b.scope_uniform_sources)?;
        same("scope_uniforms_buf", &a.scope_uniforms_buf, &b.scope_uniforms_buf)?;
        same(
            "table_consts",
            a.table_consts.iter().map(|t| (t.shader_name, &t.doc, &t.name, t.min, t.max, t.step, t.initial, t.value, t.input)).collect::<Vec<_>>(),
            b.table_consts.iter().map(|t| (t.shader_name, &t.doc, &t.name, t.min, t.max, t.step, t.initial, t.value, t.input)).collect::<Vec<_>>(),
        )?;
        same("geometry_id", a.geometry_id, b.geometry_id)?;
        same("varying_total_slots", a.varying_total_slots, b.varying_total_slots)?;
        same("color_format", a.color_format, b.color_format)?;
        same("blend_op", a.blend_op, b.blend_op)?;
        same("fragment_outputs", a.fragment_outputs, b.fragment_outputs)?;
        same("reflection.instance", reflected(&a.reflection.instance), reflected(&b.reflection.instance))?;
        same("reflection.vertex", reflected(&a.reflection.vertex), reflected(&b.reflection.vertex))?;
        Ok(())
    };
    check().err()
}
