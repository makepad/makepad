use makepad_relax_api::relax_api;
use crate::function::*;
use crate::heap::*;
pub use crate::host::*;
use crate::makepad_error_log::*;
use crate::makepad_live_id::*;
use crate::mod_gc::*;
use crate::mod_html::*;
use crate::mod_math::*;
use crate::mod_pod::*;
use crate::mod_regex::*;
use crate::mod_shader::*;
use crate::mod_std::*;
use crate::native::*;
use crate::object::*;
use crate::opcode::*;
use crate::parser::*;
use crate::thread::*;
use crate::tokenizer::*;
use crate::trap::*;
use crate::value::*;
use crate::vm_ctl::*;
use crate::vm_query::*;
use crate::*;
use std::cell::{Cell, Ref, RefCell, RefMut};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

#[relax_api(handle, clone, eq, hash, debug)]
#[derive(Clone, Default, Eq, PartialEq)]
pub struct ScriptModKey {
    pub file: String,
    pub line: usize,
    pub column: usize,
}

impl std::fmt::Debug for ScriptModKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptModKey").field("file", &self.file()).field("line", &self.line()).field("column", &self.column()).finish()
    }
}

impl std::hash::Hash for ScriptModKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.file().hash(state); self.line().hash(state); self.column().hash(state);
    }
}

/// What a [`ScriptModKey`] holds, as views.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScriptModKeyView<'a> {
    pub file: &'a str,
    pub line: usize,
    pub column: usize,
}

#[relax_api]
impl ScriptModKey {
    #[relax_api]
    pub fn new(file: &str, line: usize, column: usize) -> Self { Self { file: file.to_owned(), line, column } }
    #[relax_api]
    pub fn view(&self) -> ScriptModKeyView<'_> { ScriptModKeyView { file: &self.file, line: self.line, column: self.column } }
}

impl ScriptModKey {
    #[inline(always)]
    pub fn file(&self) -> &str { self.view().file }
    #[inline(always)]
    pub fn line(&self) -> usize { self.view().line }
    #[inline(always)]
    pub fn column(&self) -> usize { self.view().column }
    #[inline(always)]
    pub fn from_script_mod(script_mod: &ScriptMod) -> Self {
        let view = script_mod.view();
        Self::new(view.file, view.line, view.column)
    }
}

#[relax_api(handle)]
#[derive(Default, Debug)]
pub struct ScriptMod {
    pub cargo_manifest_path: String,
    pub module_path: String,
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub code: String,
    pub values: Vec<ScriptValue>,
}

/// A pinned heap string's text (ScriptVm::string_pin): valid until its string_unpin.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScriptStrPin {
    ptr: *const u8,
    len: usize,
    depth: u32,
}

impl ScriptStrPin {
    /// # Safety
    /// Only between string_pin and string_unpin.
    #[inline(always)]
    pub unsafe fn as_str<'a>(self) -> &'a str {
        std::str::from_utf8_unchecked(std::slice::from_raw_parts(self.ptr, self.len))
    }
}

/// A script module's source as plain views, what crosses into the VM: `&str`/`&[ScriptValue]`
/// fat pointers and scalars (C-shaped: in the makepad loader the VM is rustc-built and the
/// ScriptMod's owner Rapid-built, local/plans/loader-core.md section 8). The VM copies what it
/// keeps.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ScriptModView<'a> {
    pub cargo_manifest_path: &'a str,
    pub module_path: &'a str,
    pub file: &'a str,
    pub line: usize,
    pub column: usize,
    pub code: &'a str,
    pub values: &'a [ScriptValue],
}

#[relax_api]
impl ScriptMod {
    #[relax_api]
    pub fn new(manifest: &str, module: &str, file: &str, line: usize, column: usize, code: &str, values: &[ScriptValue]) -> Self {
        Self { cargo_manifest_path: manifest.to_owned(), module_path: module.to_owned(), file: file.to_owned(), line, column, code: code.to_owned(), values: values.to_vec() }
    }

    /// Where the block is reported to start (a macro that wraps `script!` names its own call site).
    #[relax_api]
    pub fn set_line_column(&mut self, line: usize, column: usize) {
        self.line = line;
        self.column = column;
    }

    /// What `script_mod!` expands to (one call, so engine code never lays the struct out:
    /// under `makepad run` this crate is built by rustc into the loader). `manifest_dir` loses a
    /// Windows `\\?\` prefix, `file` uses `/`.
    #[inline(always)]
    pub fn from_parts(manifest_dir: &str, module_path: &str, file: &str, line: usize, column: usize, code: &str, values: &[ScriptValue]) -> ScriptMod {
        Self::new(manifest_dir.trim_start_matches("\\\\?\\"), module_path, &file.replace('\\', "/"), line, column, code, values)
    }

    #[inline]
    #[relax_api]
    pub fn view(&self) -> ScriptModView<'_> {
        ScriptModView {
            cargo_manifest_path: &self.cargo_manifest_path,
            module_path: &self.module_path,
            file: &self.file,
            line: self.line,
            column: self.column,
            code: &self.code,
            values: &self.values,
        }
    }
}

impl<'a> ScriptModView<'a> {
    /// No module: for a request that evaluates no source of its own.
    pub fn empty() -> ScriptModView<'static> {
        ScriptModView { cargo_manifest_path: "", module_path: "", file: "", line: 0, column: 0, code: "", values: &[] }
    }
}

/// `ScriptVm::vm_call`: call `fnobj` with `args` (positional, bound to its parameters in order)
/// and `me`.
pub const CALL_ARGS: u32 = 0;
/// Call `fnobj` with `args_obj`: its unnamed entries are positional, its named ones bind by name,
/// its map entries are copied into the call's scope.
pub const CALL_OBJECT: u32 = 1;
/// `fnobj` is a value: run its apply transform if it has one (`found` tells).
pub const CALL_TRANSFORM: u32 = 2;
/// Run the paused thread on.
pub const CALL_RESUME: u32 = 3;

#[relax_api(plain)]
#[repr(C)]
pub struct ScriptCallReq<'a> {
    pub mode: u32,
    /// out (CALL_TRANSFORM): whether the value had a transform
    pub found: bool,
    pub fnobj: ScriptValue,
    pub args: &'a [ScriptValue],
    pub args_obj: ScriptObject,
    pub me: ScriptValue,
}

impl<'a> ScriptCallReq<'a> {
    #[inline(always)]
    pub fn new(mode: u32, fnobj: ScriptValue) -> Self {
        Self { mode, found: false, fnobj, args: &[], args_obj: ScriptObject::ZERO, me: NIL }
    }
}

/// `ScriptVm::vm_eval` adds the module as a body and runs it, as `mode` says.
pub const EVAL_ADD: u32 = 0;
/// `scope`: the object named `__script_source__` in the module's scope (zero: none)
pub const EVAL_SOURCE: u32 = 1;
/// A body the GC reclaims once nothing made from it lives.
pub const EVAL_TRANSIENT: u32 = 2;
/// `scope`: the parent behind the module's own scope.
pub const EVAL_IN_SCOPE: u32 = 3;
/// A file people read: `uses` are brought into its scope first; `body` tells which body it ran.
pub const EVAL_MODULE: u32 = 4;
/// `code`: the full growing source; the module names the body it appends to. `scope` as EVAL_SOURCE.
pub const EVAL_APPEND: u32 = 5;
/// `image`: another VM's fn, `scope` its parent; the result is the fn object or nil.
pub const EVAL_ADOPT: u32 = 6;

#[relax_api(plain)]
#[repr(C)]
pub struct ScriptEvalReq<'a> {
    pub mode: u32,
    /// out: the body the module is
    pub body: u16,
    pub module: ScriptModView<'a>,
    pub scope: ScriptObject,
    pub uses: &'a [ScriptObject],
    pub code: &'a str,
    pub image: *const ScriptFnImage,
}

impl<'a> ScriptEvalReq<'a> {
    #[inline(always)]
    pub fn new(mode: u32, module: ScriptModView<'a>) -> Self {
        Self { mode, body: 0, module, scope: ScriptObject::ZERO, uses: &[], code: "", image: std::ptr::null() }
    }
}

impl ScriptModView<'_> {
    pub fn to_owned(&self) -> ScriptMod {
        ScriptMod {
            cargo_manifest_path: self.cargo_manifest_path.to_string(),
            module_path: self.module_path.to_string(),
            file: self.file.to_string(),
            line: self.line,
            column: self.column,
            code: self.code.to_string(),
            values: self.values.to_vec(),
        }
    }
}

pub enum ScriptSource {
    Mod(ScriptMod),
    Streaming { code: String },
    /// A transient body the GC reclaimed (see [`ScriptVm::eval_transient`]):
    /// its slot is reused by the next module added.
    Free,
}

pub struct ScriptBody {
    pub source: ScriptSource,
    pub effective_code: String,
    pub tokenizer: ScriptTokenizer,
    pub parser: ScriptParser,
    pub scope: ScriptObjectRef,
    pub me: ScriptObjectRef,
    /// The scope the body ended in, once it has run: `let`/`fn` that
    /// shadow a name open child scopes below `scope`.
    pub end_scope: Option<ScriptObjectRef>,
    pub checkpoint: Option<ParserCheckpoint>,
    pub source_len: usize,
    /// The text this body was last parsed from (and its injected values),
    /// hashed: equal content compiles to equal opcodes, so a fn is
    /// identified by this and its opcode index, whatever body slot or file
    /// name holds it (see [`ScriptCode::fn_content_key`]).
    pub content_hash: u64,
    /// Evaluated with [`ScriptVm::eval_transient`]: its scope is not a GC
    /// root, and the body is freed once nothing made from it is alive (no
    /// fn defined in it, no object built by it, no thread running it).
    pub transient: bool,
    /// The file the body's text is really written in, when it is named
    /// otherwise (an adopted fn's body is named by its hash); empty: the
    /// source's own file. Locations and errors report this file.
    pub real_file: String,
    /// A module whose text is a file people read: the file name its
    /// messages carry (the body's own name carries a revision), with its
    /// first line numbered `ScriptMod::line` (1: the usual first line) and
    /// its columns exact (`ScriptMod::column` only tells bodies apart).
    /// Parse errors and runtime locations then name the same line.
    pub shown: Option<String>,
}

impl ScriptBody {
    /// The body's text as its tokens were read: the module's code, or for a
    /// body fed by appending (a game's `game.splash`, whose module carries
    /// no code) what the tokenizer took in. Token spans index it by char.
    pub(crate) fn code_text(&self) -> &str {
        if self.effective_code.is_empty() {
            &self.tokenizer.original
        } else {
            &self.effective_code
        }
    }

    /// The file a location in this body names (see [`Self::shown`]).
    fn site_file(&self, script_mod: &ScriptMod) -> String {
        self.shown.clone().unwrap_or_else(|| script_mod.file.clone())
    }

    /// The zero-based line the body's text starts at.
    fn site_line0(&self, script_mod: &ScriptMod) -> u32 {
        if self.shown.is_some() { script_mod.line.saturating_sub(1) as u32 } else { script_mod.line as u32 }
    }

    /// The file and (line, column) offsets the body's parser reports.
    fn parse_site(&self, script_mod: &ScriptMod) -> (String, (usize, usize)) {
        match &self.shown {
            Some(file) => (file.clone(), (script_mod.line.saturating_sub(1), 0)),
            None => (script_mod.file.clone(), (script_mod.line, script_mod.column)),
        }
    }
}

/// A module borrowed from the body's table. The borrow guard stays opaque so callers
/// can inspect source views without copying the entire module or laying out RefCell state;
/// [`Self::view`] is the one door, the readers below are inline code over it.
#[relax_api(handle)]
pub struct ScriptModuleRead<'a> {
    bodies: Ref<'a, Vec<ScriptBody>>,
    index: usize,
}

/// What a [`ScriptModuleRead`] shows of its body, as views into it.
#[relax_api(plain)]
#[repr(C)]
pub struct ScriptModuleView<'a> {
    pub exists: bool,
    /// How many bodies there are.
    pub count: usize,
    /// Number of original source characters retained by the tokenizer.
    pub source_chars: usize,
    /// The code the body runs.
    pub code: &'a str,
    pub opcodes: &'a [ScriptValue],
    pub source_map: &'a [Option<u32>],
    pub tokens: &'a [ScriptTokenPos],
    /// The scope the body ended in, else its own (zero when the body does not exist).
    pub end_scope: ScriptObject,
    /// Null when the body is no script module.
    pub module: *const ScriptMod,
}

/// The view of body `index` of `bodies`.
fn module_view(bodies: &[ScriptBody], index: usize) -> ScriptModuleView<'_> {
    match bodies.get(index) {
        Some(b) => ScriptModuleView {
            exists: true,
            count: bodies.len(),
            source_chars: b.tokenizer.original.len(),
            code: b.effective_code.as_str(),
            opcodes: b.parser.opcodes.as_slice(),
            source_map: b.parser.source_map.as_slice(),
            tokens: b.tokenizer.tokens.as_slice(),
            end_scope: b.end_scope.as_ref().map(|s| s.as_object()).unwrap_or(b.scope.as_object()),
            module: match &b.source {
                ScriptSource::Mod(m) => m as *const ScriptMod,
                _ => std::ptr::null(),
            },
        },
        None => ScriptModuleView { exists: false, count: bodies.len(), source_chars: 0, code: "", opcodes: &[], source_map: &[], tokens: &[], end_scope: ScriptObject::ZERO, module: std::ptr::null() },
    }
}

#[relax_api]
impl ScriptModuleRead<'_> {
    #[relax_api]
    pub fn view(&self) -> ScriptModuleView<'_> {
        module_view(&self.bodies, self.index)
    }
}

impl ScriptModuleRead<'_> {
    #[inline(always)]
    pub fn exists(&self) -> bool { self.view().exists }
    /// Number of original source characters retained by the tokenizer.
    #[inline(always)]
    pub fn source_char_count(&self) -> usize { self.view().source_chars }
    #[inline(always)]
    pub fn opcode(&self, index: usize) -> Option<crate::opcode::Opcode> { self.view().opcodes.get(index)?.as_opcode().map(|(op, _)| op) }
    #[inline(always)]
    pub fn source_map(&self) -> &[Option<u32>] { self.view().source_map }
    #[inline(always)]
    pub fn end_or_initial_scope(&self) -> Option<ScriptObject> {
        let view = self.view();
        view.exists.then_some(view.end_scope)
    }
    #[inline(always)]
    pub fn tokens(&self) -> &[ScriptTokenPos] { self.view().tokens }
    #[inline(always)]
    pub fn token_index(&self, instruction: usize) -> Option<u32> { self.view().source_map.get(instruction).copied().flatten() }
    #[inline(always)]
    pub fn module(&self) -> Option<&ScriptMod> {
        // SAFETY: the view's module points into the body table this read borrows
        unsafe { self.view().module.as_ref() }
    }
}

/// Mutable body-table borrow whose representation stays inside the VM: [`Self::view`] shows a
/// body, [`Self::set_opcode_value`] changes one of its opcodes.
#[relax_api(handle)]
pub struct ScriptModulesMut<'a> { bodies: std::cell::RefMut<'a, Vec<ScriptBody>> }

#[relax_api]
impl ScriptModulesMut<'_> {
    #[relax_api]
    pub fn view(&self, index: usize) -> ScriptModuleView<'_> { module_view(&self.bodies, index) }
    #[relax_api]
    pub fn set_opcode_value(&mut self, index: usize, at: usize, value: ScriptValue) { self.bodies[index].parser.opcodes[at] = value; }
}

impl ScriptModulesMut<'_> {
    #[inline(always)]
    pub fn len(&self) -> usize { self.view(0).count }
    #[inline(always)]
    pub fn module(&self, index: usize) -> Option<&ScriptMod> {
        // SAFETY: the view's module points into the body table this borrow holds
        unsafe { self.view(index).module.as_ref() }
    }
    #[inline(always)]
    pub fn effective_code(&self, index: usize) -> &str { self.view(index).code }
    #[inline(always)]
    pub fn tokens(&self, index: usize) -> &[ScriptTokenPos] { self.view(index).tokens }
    #[inline(always)]
    pub fn source_map(&self, index: usize) -> &[Option<u32>] { self.view(index).source_map }
    #[inline(always)]
    pub fn opcode_value(&self, index: usize, at: usize) -> ScriptValue { self.view(index).opcodes[at] }
    #[inline(always)]
    pub fn end_or_initial_scope(&self, index: usize) -> ScriptObject { self.view(index).end_scope }
}

#[relax_api]
impl ScriptCode {
    #[relax_api]
    pub fn modules_mut(&self) -> ScriptModulesMut<'_> { ScriptModulesMut { bodies: self.bodies.borrow_mut() } }
}

impl ScriptBody {
    fn content_hash_of(code: &str, values: &[ScriptValue]) -> u64 {
        let mut id = LiveId::from_str(code);
        for v in values {
            id = id.bytes_append(&v.raw().to_le_bytes());
        }
        id.0
    }
}

#[relax_api(plain)]
#[derive(Default)]
pub struct ScriptBuiltins {
    pub range: ScriptObject,
    pub pod: ScriptPodBuiltins,
    /// The function a call of `mod` runs (`crate::mod_math::define_mod_call`).
    pub mod_call: ScriptObject,
}

impl ScriptBuiltins {
    pub fn new(heap: &mut ScriptHeap, pod: ScriptPodBuiltins, mod_call: ScriptObject) -> Self {
        Self {
            range: heap
                .value_path(heap.modules, ids!(std.Range), NoTrap)
                .as_object()
                .unwrap(),
            pod,
            mod_call,
        }
    }
}

/// The script bodies, behind the same `borrow()` / `borrow_mut()` surface a
/// `RefCell` has. Every mutable borrow advances `epoch`: a mutation is the only
/// thing that can move or free a body's opcode buffer (a native call can
/// re-enter `eval` and reload the very body that is running), and `run_core`
/// compares the epoch before it trusts its cached pointer into that buffer.
#[relax_api(handle)]
#[derive(Default)]
pub struct ScriptBodies {
    bodies: RefCell<Vec<ScriptBody>>,
    epoch: Cell<u64>,
}

impl ScriptBodies {
    #[inline(always)]
    pub fn borrow(&self) -> Ref<'_, Vec<ScriptBody>> {
        self.bodies.borrow()
    }

    pub fn borrow_mut(&self) -> RefMut<'_, Vec<ScriptBody>> {
        self.epoch.set(self.epoch.get().wrapping_add(1));
        self.bodies.borrow_mut()
    }

    #[inline(always)]
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch.get()
    }
}

#[relax_api(handle)]
#[derive(Default)]
pub struct ScriptCode {
    pub builtins: ScriptBuiltins,
    pub native: RefCell<ScriptNative>,
    pub bodies: ScriptBodies,
    /// Slots of reclaimed transient bodies, reused before the body list
    /// grows (a body id is part of every ip, so the list is finite).
    pub free_bodies: RefCell<Vec<u16>>,
    pub crate_manifests: Rc<RefCell<HashMap<String, String>>>,
    pub script_mod_overrides: Rc<RefCell<HashMap<ScriptModKey, String>>>,
    /// Shader fns compiled in a namespace ([`ScriptVm::set_shader_fn_namespace`]).
    pub(crate) shader_fn_namespaces: RefCell<HashMap<ScriptObject, Rc<ShaderFnNamespace>>>,
}

/// How a shader fn reads its `self` when several fns share one shader under
/// other names (a host placing a fn's own fields under prefixed names):
/// `self.<from>` is read as `self.<to>` (fields, methods, assignments), and,
/// with an input, the fn takes one more first parameter that stands for
/// one read it makes (`self.<field>.<method>(self.<at>())`, e.g. the colour
/// at its own pixel). Set by the host on the fn object; the compiler emits
/// exactly what it would for the fn written with the names and the
/// parameter in place.
#[derive(Clone, Debug, Default)]
pub(crate) struct ShaderFnNamespace {
    pub renames: Vec<(LiveId, LiveId)>,
    pub input: Option<ShaderFnInput>,
}

/// One rename of a [`ShaderFnNamespace`]: `self.<from>` is `self.<to>`.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ShaderFnRename {
    pub from: LiveId,
    pub to: LiveId,
}

/// See [`ShaderFnNamespace::input`].
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ShaderFnInput {
    /// The read's texture field, method and coordinate method.
    pub field: LiveId,
    pub method: LiveId,
    pub at: LiveId,
    /// The parameter that stands for it (its type is the argument's).
    pub param: LiveId,
}

impl ShaderFnNamespace {
    /// `id` as the fn reads it.
    pub(crate) fn rename(&self, id: LiveId) -> LiveId {
        self.renames.iter().find(|(from, _)| *from == id).map_or(id, |(_, to)| *to)
    }
}

impl ScriptLoc {
    #[inline(always)]
    pub fn new(file: &str, line: u32, col: u32) -> Self { Self { file: file.to_owned(), line, col } }
    #[inline(always)]
    pub fn file(&self) -> &str { &self.file }
    #[inline(always)]
    pub fn line(&self) -> u32 { self.line }
    #[inline(always)]
    pub fn col(&self) -> u32 { self.col }
}

/// Caller-owned source location. All methods stay on the constructing side.
#[relax_api(plain)]
pub struct ScriptLoc {
    pub file: String,
    pub col: u32,
    pub line: u32,
}

/// A script fn's tokens with their document places (see
/// [`ScriptCode::fn_tokens_placed`]).
pub struct ScriptFnTokens {
    pub file: String,
    /// A closure (`|a| ..`, from its opening `|`), not `fn (..) { .. }`
    /// (from the parameter list's `(`).
    pub closure: bool,
    pub tokens: Vec<ScriptFnToken>,
}

/// [`ScriptFnTokens`] in flight across the relax boundary: its String and Vec as the words of
/// their buffers (`makepad_relax_api::Words`); `ScriptVm::fn_tokens_placed` rebuilds them.
#[relax_api(plain)]
#[repr(C)]
pub struct ScriptFnTokensRaw {
    pub file: [usize; 3],
    pub tokens: [usize; 3],
    pub closure: bool,
}

/// One token of a script fn: as the VM read it, whether a newline
/// preceded it, its characters in its body's code (`start` first, `end`
/// one past the last) and where it is written (1-based line and column).
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScriptFnToken {
    pub token: ScriptToken,
    pub newline: bool,
    pub start: usize,
    pub end: usize,
    pub line: u32,
    pub col: u32,
}

/// A script fn moved between VMs: its tokens as the VM that evaluated it
/// read them, the characters they were read from (the fn as written) and
/// where it is written (file, 0-based line and column of its first
/// character). It holds no VM references: one VM makes it
/// ([`ScriptCode::fn_image`]), another adopts it as a fn of its own
/// ([`crate::vm::ScriptVm::adopt_fn`]); nothing is re-read from text.
/// To Relax-compiled code it is an opaque handle: the crate drops, clones and
/// reads it (relax_api.txt ops); an empty image ([`Self::is_empty`]) stands
/// for "no fn" where a handle crosses by value.
#[relax_api(handle, clone)]
#[repr(transparent)]
pub struct ScriptFnImage(Option<Box<ScriptFnImageData>>);

struct ScriptFnImageData {
    file: String,
    line: u32,
    col: u32,
    text: String,
    tokens: Vec<ScriptTokenPos>,
    /// The string tokens' text, by token index (strings live in a heap).
    strings: Vec<(usize, String)>,
    hash: u64,
}

#[relax_api]
impl ScriptFnImage {
    fn new(file: String, line: u32, col: u32, text: String, tokens: Vec<ScriptTokenPos>, strings: Vec<(usize, String)>) -> Self {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        let mut eat = |bytes: &[u8]| {
            for b in bytes {
                h = (h ^ *b as u64).wrapping_mul(0x100_0000_01b3);
            }
        };
        eat(file.as_bytes());
        eat(&line.to_le_bytes());
        eat(&col.to_le_bytes());
        eat(text.as_bytes());
        Self(Some(Box::new(ScriptFnImageData { file, line, col, text, tokens, strings, hash: h })))
    }

    pub(crate) fn empty() -> Self { Self(None) }

    /// What the image holds, as views.
    #[relax_api]
    pub fn view(&self) -> ScriptFnImageView<'_> {
        match self.data() {
            Some(d) => ScriptFnImageView { empty: false, file: &d.file, text: &d.text, line: d.line, col: d.col, hash: d.hash, tokens: &d.tokens },
            None => ScriptFnImageView { empty: true, file: "", text: "", line: 0, col: 0, hash: 0, tokens: &[] },
        }
    }
    fn data(&self) -> Option<&ScriptFnImageData> { self.0.as_deref() }
}

/// What a [`ScriptFnImage`] holds, as views ([`ScriptFnImage::view`]).
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScriptFnImageView<'a> {
    pub empty: bool,
    pub file: &'a str,
    pub text: &'a str,
    pub line: u32,
    pub col: u32,
    pub hash: u64,
    pub tokens: &'a [ScriptTokenPos],
}

impl ScriptFnImage {
    /// No fn (what [`ScriptVm::fn_image_of`] gives for a value that is none).
    #[inline(always)]
    pub fn is_empty(&self) -> bool { self.view().empty }
    /// The file the fn is written in.
    #[inline(always)]
    pub fn file(&self) -> &str { self.view().file }
    /// The 0-based line and column of its first character.
    #[inline(always)]
    pub fn line(&self) -> u32 { self.view().line }
    #[inline(always)]
    pub fn col(&self) -> u32 { self.view().col }
    /// The characters its tokens were read from (the fn as written).
    #[inline(always)]
    pub fn text(&self) -> &str { self.view().text }
    /// Its identity: the place and the characters.
    #[inline(always)]
    pub fn hash(&self) -> u64 { self.view().hash }
    /// Its tokens in order, each with the characters it was written in (a
    /// slice of [`Self::text`]; a string token's includes its quotes).
    #[inline(always)]
    pub fn tokens(&self) -> ScriptFnImageTokens<'_> {
        let view = self.view();
        ScriptFnImageTokens { tokens: view.tokens.iter(), rest: view.text, char_at: 0 }
    }
}

/// The tokens of a [`ScriptFnImage`] with their text ([`ScriptFnImage::tokens`]).
pub struct ScriptFnImageTokens<'a> {
    tokens: std::slice::Iter<'a, ScriptTokenPos>,
    /// The text from character index `char_at` on.
    rest: &'a str,
    char_at: usize,
}

impl<'a> Iterator for ScriptFnImageTokens<'a> {
    type Item = (ScriptTokenPos, &'a str);
    fn next(&mut self) -> Option<(ScriptTokenPos, &'a str)> {
        let t = *self.tokens.next()?;
        let span = t.span();
        // Spans are in order: walk the text forward by characters.
        let skip = span.start.saturating_sub(self.char_at);
        let from = self.rest.char_indices().nth(skip).map_or(self.rest.len(), |(b, _)| b);
        let rest = &self.rest[from..];
        let len = span.end.saturating_sub(span.start);
        let to = rest.char_indices().nth(len).map_or(rest.len(), |(b, _)| b);
        self.rest = rest;
        self.char_at = span.start.max(self.char_at);
        Some((t, &rest[..to]))
    }
}

impl Clone for ScriptFnImage {
    fn clone(&self) -> Self {
        Self(self.0.as_ref().map(|d| Box::new(ScriptFnImageData { file: d.file.clone(), line: d.line, col: d.col, text: d.text.clone(), tokens: d.tokens.clone(), strings: d.strings.clone(), hash: d.hash })))
    }
}

impl std::fmt::Debug for ScriptFnImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ScriptFnImage({}:{}:{} {:016x})", self.file(), self.line(), self.col(), self.hash())
    }
}

/// Rustc-owned location storage inside an error record. Rapid callers obtain a
/// caller-owned `ScriptLoc` through `ScriptErrorRecord::loc`, never this layout.
#[relax_api(handle)]
#[derive(Clone)]
pub struct ScriptErrorLoc {
    pub file: String,
    pub col: u32,
    pub line: u32,
}

/// An error a script raised (or a parse error), with where it is: its
/// source location (zero-based line and column, like [`ScriptLoc`]) and
/// its message apart, and the text the VM has always reported it as
/// (`file:line:col: message (origin)`), which `Display` prints.
#[relax_api(handle, clone)]
#[derive(Clone)]
pub struct ScriptErrorRecord {
    pub loc: Option<ScriptErrorLoc>,
    /// The message without its location or the interpreter's origin.
    pub message: String,
    pub text: String,
}

impl std::fmt::Display for ScriptErrorRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

/// A host's own message, at no place in the source.
impl From<String> for ScriptErrorRecord {
    fn from(message: String) -> Self {
        Self::plain(message)
    }
}

impl std::fmt::Debug for ScriptErrorRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.text, f)
    }
}

#[relax_api]
impl ScriptErrorRecord {
    #[inline(always)]
    pub fn new(loc: Option<&ScriptLoc>, message: &str, text: &str) -> Self {
        let at = crate::vm_api::ScriptLineCol { line: loc.map_or(0, |l| l.line), col: loc.map_or(0, |l| l.col) };
        Self::from_parts(loc.map(|l| l.file.as_str()), &at, message, text)
    }

    #[relax_api]
    pub fn from_parts(file: Option<&str>, at: &crate::vm_api::ScriptLineCol, message: &str, text: &str) -> Self {
        Self { loc: file.map(|file| ScriptErrorLoc { file: file.to_owned(), line: at.line, col: at.col }), message: message.to_owned(), text: text.to_owned() }
    }

    /// What the record holds, as views.
    #[relax_api]
    pub fn view(&self) -> ScriptErrorRecordView<'_> {
        match &self.loc {
            Some(loc) => ScriptErrorRecordView { text: &self.text, message: &self.message, has_loc: true, file: &loc.file, line: loc.line, col: loc.col },
            None => ScriptErrorRecordView { text: &self.text, message: &self.message, has_loc: false, file: "", line: 0, col: 0 },
        }
    }
}

/// What a [`ScriptErrorRecord`] holds, as views ([`ScriptErrorRecord::view`]).
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScriptErrorRecordView<'a> {
    pub text: &'a str,
    pub message: &'a str,
    pub has_loc: bool,
    pub file: &'a str,
    pub line: u32,
    pub col: u32,
}

impl ScriptErrorRecord {
    #[inline(always)]
    pub fn text(&self) -> &str { self.view().text }
    #[inline(always)]
    pub fn message(&self) -> &str { self.view().message }
    #[inline(always)]
    pub fn loc(&self) -> Option<ScriptLoc> {
        let view = self.view();
        view.has_loc.then(|| ScriptLoc::new(view.file, view.line, view.col))
    }

    #[inline(always)]
    pub fn loc_into(&self, file: &mut crate::vm_api::ScriptTextOut, at: &mut crate::vm_api::ScriptLineCol) -> bool {
        let view = self.view();
        if view.has_loc {
            file.push(view.file);
            at.line = view.line;
            at.col = view.col;
        }
        view.has_loc
    }

    /// An error a host raises itself, at no place in the source.
    pub fn plain(message: String) -> Self {
        Self::plain_str(&message)
    }

    #[inline(always)]
    pub fn plain_str(message: &str) -> Self {
        Self::from_parts(None, &crate::vm_api::ScriptLineCol::default(), message, message)
    }

    /// A parse error: its formatted text and, when the parser kept it, its
    /// structured diagnostic.
    pub fn parse(text: String, diagnostic: Option<&crate::parser::ScriptParserDiagnostic>, file: &str) -> Self {
        let loc = diagnostic.map(|d| ScriptErrorLoc { file: file.to_string(), line: d.line, col: d.column });
        let message = diagnostic.map_or_else(|| text.clone(), |d| d.message.clone());
        Self { loc, message, text }
    }
}

impl Clone for ScriptLoc {
    #[inline(always)]
    fn clone(&self) -> Self { Self { file: self.file.clone(), line: self.line, col: self.col } }
}

impl PartialEq for ScriptLoc {
    #[inline(always)]
    fn eq(&self, other: &Self) -> bool { self.file == other.file && self.col == other.col && self.line == other.line }
    #[inline(always)]
    fn ne(&self, other: &Self) -> bool { !self.eq(other) }
}

impl std::fmt::Debug for ScriptLoc {
    #[inline(always)]
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

impl std::fmt::Display for ScriptLoc {
    #[inline(always)]
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}:{}:{}", self.file, self.line, self.col)
    }
}

impl ScriptCode {
    /// What a fn's code is, for caches keyed by it (the draw shader cache):
    /// its body, the content that body was last parsed from and its opcode
    /// index. Unlike the ip alone it changes when a body is re-evaluated
    /// with new text. The body is part of it: the same text in another body
    /// can name other things (its scope, its imports), so it is another fn.
    pub fn fn_content_key(&self, ip: ScriptIp) -> u64 {
        let content = self.bodies.borrow().get(ip.body as usize).map_or(0, |b| b.content_hash);
        LiveId(content).bytes_append(&ip.to_u40().to_le_bytes()).0
    }

    /// The source text of the fn whose body starts at `ip` — the `fn` token's
    /// line through the matching closing brace — plus where it lives. What
    /// the design tweaker shows under a material well: the pixel/vertex
    /// function as written, docs included, for a code-only rewrite.
    #[inline(always)]
    pub fn fn_source_text(&self, ip: ScriptIp) -> Option<(ScriptLoc, String)> {
        let (mut file, mut at, mut text) = (String::new(), crate::vm_api::ScriptLineCol::default(), String::new());
        self.fn_source_text_into(ip, &mut crate::vm_api::ScriptTextOut::new(&mut file), &mut at, &mut crate::vm_api::ScriptTextOut::new(&mut text))
            .then(|| (ScriptLoc::new(&file, at.line, at.col), text))
    }

    pub fn fn_source_text_into(&self, ip: ScriptIp, file: &mut crate::vm_api::ScriptTextOut, at: &mut crate::vm_api::ScriptLineCol, output: &mut crate::vm_api::ScriptTextOut) -> bool {
        let mut sink = ScriptQuerySink::new();
        sink.text0(file);
        sink.text1(output);
        let answer = self.query(QUERY_FN_TEXT, &mut sink, |req| {
            req.ip = ip;
            req.flag = true;
        });
        *at = answer.at;
        answer.found
    }

    pub(crate) fn fn_source_text_native(&self, ip: ScriptIp, file: &mut crate::vm_api::ScriptTextOut, at: &mut crate::vm_api::ScriptLineCol, output: &mut crate::vm_api::ScriptTextOut) -> bool {
        let found = (|| -> Option<(ScriptLoc, String)> {
            let loc = self.ip_to_loc_plain(ip)?;
            let bodies = self.bodies.borrow();
            let body = bodies.get(ip.body as usize)?;
            let source_map = &body.parser.source_map;
            // Synthetic opcodes map to no token; take the nearest mapped one on
            // either side, as `ip_to_loc` does.
            let ip_index = (ip.index as usize).min(source_map.len().saturating_sub(1));
            let token_index = (0..=ip_index)
                .rev()
                .find_map(|i| source_map.get(i).and_then(|slot| *slot))
                .or_else(|| {
                    ((ip_index + 1)..source_map.len()).find_map(|i| source_map.get(i).and_then(|slot| *slot))
                })?;
            let (row, _col) = body.tokenizer.token_index_to_row_col(token_index)?;
            let code = body.code_text();
            let lines: Vec<&str> = code.split_inclusive('\n').collect();
            // The ip maps to a token INSIDE the fn; walk back to the header line
            // (the nearest line above holding `fn`), then take from there to the
            // matching closing brace.
            let mut header = (row as usize).min(lines.len().saturating_sub(1));
            while header > 0 && !lines[header].contains("fn") {
                header -= 1;
            }
            let start: usize = lines[..header].iter().map(|l| l.len()).sum();
            let rest = &code[start.min(code.len())..];
            // From the first `{` after the fn header to its matching `}`.
            let open = rest.find('{')?;
            let mut depth = 0i32;
            let mut end = None;
            for (i, ch) in rest[open..].char_indices() {
                match ch {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(open + i + 1);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let end = end?;
            // Include the header line (e.g. `pixel: fn() {`) from its own start.
            let line_start = rest[..open].rfind('\n').map(|i| i + 1).unwrap_or(0);
            let text = rest[line_start..end].to_string();
            Some((loc, text))
        })();
        if let Some((loc, text)) = found {
            file.push(&loc.file); at.line = loc.line; at.col = loc.col;
            output.push(&text);
            true
        } else { false }
    }

    /// The exact text of the fn whose body starts at `ip`, from its `fn`
    /// keyword to its closing brace (`fn(uv: vec2) -> vec4 { ... }`), and
    /// where the `fn` keyword is. For hosts that take a document's function
    /// as code of their own (a shader pass compiled elsewhere): they keep the
    /// text and report its errors at the document's lines.
    ///
    /// A closure (`|a, b| expr`, `|p| { .. }`) gives its own text, from the
    /// opening `|` through its body (an expression body ends where its
    /// statement does).
    #[inline(always)]
    pub fn fn_text(&self, ip: ScriptIp) -> Option<(ScriptLoc, String)> {
        let (mut file, mut at, mut text) = (String::new(), crate::vm_api::ScriptLineCol::default(), String::new());
        self.fn_text_into(ip, &mut crate::vm_api::ScriptTextOut::new(&mut file), &mut at, &mut crate::vm_api::ScriptTextOut::new(&mut text))
            .then(|| (ScriptLoc::new(&file, at.line, at.col), text))
    }

    pub fn fn_text_into(&self, ip: ScriptIp, file: &mut crate::vm_api::ScriptTextOut, at: &mut crate::vm_api::ScriptLineCol, output: &mut crate::vm_api::ScriptTextOut) -> bool {
        let mut sink = ScriptQuerySink::new();
        sink.text0(file);
        sink.text1(output);
        let answer = self.query(QUERY_FN_TEXT, &mut sink, |req| req.ip = ip);
        *at = answer.at;
        answer.found
    }

    pub(crate) fn fn_text_native(&self, ip: ScriptIp, file: &mut crate::vm_api::ScriptTextOut, at: &mut crate::vm_api::ScriptLineCol, output: &mut crate::vm_api::ScriptTextOut) -> bool {
        let found = (|| -> Option<()> {
            let bodies = self.bodies.borrow();
            let body = bodies.get(ip.body as usize)?;
            let source_map = &body.parser.source_map;
            // The body opcode just before `ip` maps to the body's first token;
            // a closure's parameter list closes with `|` right before it.
            let body_tok = (ip.index as usize).checked_sub(1).and_then(|s| source_map.get(s).copied().flatten());
            if let Some(t) = body_tok {
                if Self::closure_text_into(body, t as usize, file, at, output) {
                    return Some(());
                }
            }
            let ip_index = (ip.index as usize).min(source_map.len().saturating_sub(1));
            let token_index = (0..=ip_index)
                .rev()
                .find_map(|i| source_map.get(i).and_then(|slot| *slot))
                .or_else(|| ((ip_index + 1)..source_map.len()).find_map(|i| source_map.get(i).and_then(|slot| *slot)))? as usize;
            let tokens = &body.tokenizer.tokens;
            // Back to the header's `fn` (a parameter list holds none).
            let mut k = token_index.min(tokens.len().checked_sub(1)?);
            while !matches!(tokens[k].token, ScriptToken::Identifier(id) if id == id!(fn)) {
                k = k.checked_sub(1)?;
            }
            // From the `fn` keyword (a token's position may sit a character into
            // it) through the body's matching `}` (comments skipped).
            let chars: Vec<char> = body.code_text().chars().collect();
            let mut start = tokens[k].pos().min(chars.len());
            while start > 0 && (chars[start - 1].is_alphanumeric() || chars[start - 1] == '_') {
                start -= 1;
            }
            let mut i = start;
            let mut depth = 0i32;
            let mut end = None;
            while i < chars.len() {
                match chars[i] {
                    '/' if chars.get(i + 1) == Some(&'/') => {
                        while i < chars.len() && chars[i] != '\n' {
                            i += 1;
                        }
                        continue;
                    }
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(i + 1);
                            break;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            let end = end?;
            let text: String = chars[start..end].iter().collect();
            // Where the `fn` keyword is written (its first character).
            let (row, col) = body.tokenizer.token_start_row_col(k as u32)?;
            let loc = match &body.source {
                ScriptSource::Mod(script_mod) => ScriptLoc { file: body.site_file(script_mod), line: row + script_mod.line as u32, col },
                _ => ScriptLoc { file: "generated".into(), line: row, col },
            };
            file.push(&loc.file); at.line = loc.line; at.col = loc.col;
            output.push(&text);
            Some(())
        })();
        found.is_some()
    }

    /// The token range (opening `|` .. last body token) of the closure whose
    /// body starts at token `body_tok`, or None when that body is not a
    /// closure's.
    fn closure_tokens(body: &ScriptBody, body_tok: usize) -> Option<(usize, usize)> {
        let tokens = &body.tokenizer.tokens;
        let mut k = body_tok.checked_sub(1)?;
        // `|a| -> Type { .. }`
        if tokens[k].token.is_identifier() && k >= 1 && tokens[k - 1].token.operator() == id!(->) {
            k = k.checked_sub(2)?;
        }
        let open = match tokens[k].token.operator() {
            o if o == id!(||) => k,
            o if o == id!(|) => {
                // Back over the parameters (names, `:` types, `,`) to the
                // opening `|`.
                let mut j = k;
                loop {
                    j = j.checked_sub(1)?;
                    let t = &tokens[j].token;
                    if t.operator() == id!(|) {
                        break j;
                    }
                    if !(t.is_identifier() || t.separator() == id!(,) || t.separator() == id!(:) || t.operator() == id!(:)) {
                        return None;
                    }
                }
            }
            _ => return None,
        };
        // The body's last token: a block's matching `}`, or an expression's
        // last token before a statement end at its own depth (a newline not
        // continuing an operator, `,`, `;`, or a bracket it did not open).
        let mut depth = 0i32;
        let mut j = body_tok;
        let last = loop {
            let t = &tokens[j];
            if j > body_tok && depth == 0 && !tokens[body_tok].token.is_open_curly() {
                let ends = (t.preceded_by_newline && !tokens[j - 1].token.is_operator())
                    || t.token.separator() == id!(,)
                    || t.token.separator() == id!(;)
                    || t.token.is_close_curly()
                    || t.token.is_close_round()
                    || t.token.is_close_square();
                if ends {
                    break j - 1;
                }
            }
            if t.token.is_open_curly() || t.token.is_open_round() || t.token.is_open_square() {
                depth += 1;
            } else if t.token.is_close_curly() || t.token.is_close_round() || t.token.is_close_square() {
                depth -= 1;
                if depth == 0 && tokens[body_tok].token.is_open_curly() {
                    break j;
                }
            }
            if j + 1 >= tokens.len() {
                break j;
            }
            j += 1;
        };
        Some((open, last))
    }

    /// The text of the closure whose body starts at token `body_tok`, or
    /// false when that body is not a closure's.
    fn closure_text_into(body: &ScriptBody, body_tok: usize, file: &mut crate::vm_api::ScriptTextOut, at: &mut crate::vm_api::ScriptLineCol, output: &mut crate::vm_api::ScriptTextOut) -> bool {
        let found = (|| -> Option<(ScriptLoc, String)> {
            let tokens = &body.tokenizer.tokens;
            let (open, last) = Self::closure_tokens(body, body_tok)?;
            let chars: Vec<char> = body.code_text().chars().collect();
            let start = tokens[open].span().start.min(chars.len());
            let end = tokens[last].span().end.min(chars.len());
            if start >= end {
                return None;
            }
            let text: String = chars[start..end].iter().collect();
            let (row, col) = body.tokenizer.token_start_row_col(open as u32)?;
            let loc = match &body.source {
                ScriptSource::Mod(script_mod) => ScriptLoc { file: body.site_file(script_mod), line: row + script_mod.line as u32, col },
                _ => ScriptLoc { file: "generated".into(), line: row, col },
            };
            Some((loc, text))
        })();
        if let Some((loc, text)) = found {
            file.push(&loc.file); at.line = loc.line; at.col = loc.col;
            output.push(&text);
            true
        } else { false }
    }

    /// The tokens of a script fn as the VM read them: from the parameter
    /// list's `(` through the body's closing `}` (`fn name(a) { .. }`,
    /// `fn(a) { .. }`), or a closure from its opening `|` through its body.
    /// Each token carries its document place, so a compiler that takes the
    /// fn as code of its own (the kernel compiler) builds its syntax tree
    /// from the VM's tokens and reports errors at the document's lines.
    pub fn fn_tokens_placed(&self, ip: ScriptIp) -> Option<ScriptFnTokens> {
        let bodies = self.bodies.borrow();
        let body = bodies.get(ip.body as usize)?;
        let tokens = &body.tokenizer.tokens;
        let (_, first, last, closure) = Self::fn_token_range(body, ip)?;
        let (file, line0) = match &body.source {
            ScriptSource::Mod(script_mod) => (body.site_file(script_mod), body.site_line0(script_mod)),
            _ => ("generated".to_string(), 0),
        };
        let mut out = Vec::with_capacity(last + 1 - first);
        for i in first..=last {
            let (row, col) = body.tokenizer.token_start_row_col(i as u32)?;
            let t = &tokens[i];
            out.push(ScriptFnToken { token: t.token, newline: t.preceded_by_newline, start: t.start(), end: t.end(), line: row + line0 + 1, col: col + 1 });
        }
        Some(ScriptFnTokens { file, closure, tokens: out })
    }

    /// The token range of the script fn whose code holds `ip`: its header
    /// (the `fn` keyword, or a closure's opening `|`), where its parameters
    /// start (the `(`, or the `|`), its last token (the body's `}`, or a
    /// closure's last expression token) and whether it is a closure.
    fn fn_token_range(body: &ScriptBody, ip: ScriptIp) -> Option<(usize, usize, usize, bool)> {
        let source_map = &body.parser.source_map;
        let tokens = &body.tokenizer.tokens;
        let body_tok = (ip.index as usize).checked_sub(1).and_then(|s| source_map.get(s).copied().flatten()).map(|t| t as usize);
        if let Some((open, last)) = body_tok.and_then(|t| Self::closure_tokens(body, t)) {
            return Some((open, open, last, true));
        }
        // The body's `{` (the opcode before `ip`), else the nearest
        // mapped token; back from there to the header's `fn` (a
        // parameter list holds none; a body may start with a fn of
        // its own, so not from inside the body).
        let token_index = match body_tok.filter(|t| tokens.get(*t).is_some_and(|t| t.token.is_open_curly())) {
            Some(open) => open,
            None => {
                let ip_index = (ip.index as usize).min(source_map.len().saturating_sub(1));
                (0..=ip_index)
                    .rev()
                    .find_map(|i| source_map.get(i).and_then(|slot| *slot))
                    .or_else(|| ((ip_index + 1)..source_map.len()).find_map(|i| source_map.get(i).and_then(|slot| *slot)))? as usize
            }
        };
        let mut k = token_index.min(tokens.len().checked_sub(1)?);
        while !matches!(tokens[k].token, ScriptToken::Identifier(id) if id == id!(fn)) {
            k = k.checked_sub(1)?;
        }
        // `fn name(..)` as well as `fn(..)`.
        let mut open = k + 1;
        if tokens.get(open)?.token.is_identifier() {
            open += 1;
        }
        if !tokens.get(open)?.token.is_open_round() {
            return None;
        }
        // Through the body's matching `}`.
        let mut depth = 0i32;
        let mut j = open;
        loop {
            let t = &tokens.get(j)?.token;
            if t.is_open_curly() {
                depth += 1;
            } else if t.is_close_curly() {
                depth -= 1;
                if depth == 0 {
                    return Some((k, open, j, false));
                }
            }
            j += 1;
        }
    }

    /// The script fn whose code holds `ip` as a [`ScriptFnImage`]: its tokens
    /// as the VM read them (from its `fn` keyword, or a closure's `|`,
    /// through its last token) with the characters they were read from and
    /// where they are written. Another VM adopts it as a fn of its own
    /// ([`ScriptVm::adopt_fn`]). None when `ip` is no script fn's, or the fn
    /// holds a value injected from Rust (`#(..)`), which only this VM has.
    pub fn fn_image(&self, heap: &ScriptHeap, ip: ScriptIp) -> Option<ScriptFnImage> {
        let bodies = self.bodies.borrow();
        let body = bodies.get(ip.body as usize)?;
        let (first, _, last, _) = Self::fn_token_range(body, ip)?;
        let tokens = &body.tokenizer.tokens;
        let chars: Vec<char> = body.code_text().chars().collect();
        let start = tokens[first].span().start;
        let end = tokens[last].span().end.min(chars.len());
        if start >= end {
            return None;
        }
        let text: String = chars[start..end].iter().collect();
        let (row, col) = body.tokenizer.token_start_row_col(first as u32)?;
        let (file, line0) = match &body.source {
            ScriptSource::Mod(script_mod) => (body.site_file(script_mod), body.site_line0(script_mod)),
            _ => ("generated".to_string(), 0),
        };
        let mut out = Vec::with_capacity(last + 1 - first);
        let mut strings = Vec::new();
        for (k, t) in tokens[first..=last].iter().enumerate() {
            let mut t = *t;
            match t.token {
                ScriptToken::RustValue(_) | ScriptToken::StringUnfinished => return None,
                ScriptToken::String(v) => {
                    strings.push((k, heap.string_with(v, |_, s| s.to_string())?));
                    t.token = ScriptToken::String(ScriptValue::EMPTY_STRING);
                }
                _ => {}
            }
            t.rebase(start);
            out.push(t);
        }
        Some(ScriptFnImage::new(file, line0 + row, col, text, out, strings))
    }

    /// Where `field` is written (`field: …`, `field := …`, `field +: …`) in
    /// the object literal constructed at `made_at` (an object's
    /// `ScriptHeap::made_at`), at that literal's own depth: the first such
    /// key, read from the body's tokens. `None` when the literal does not
    /// write the field (it inherits it, or it was set from Rust).
    #[inline(always)]
    pub fn field_loc(&self, made_at: ScriptIp, field: LiveId) -> Option<ScriptLoc> {
        let mut file = String::new();
        let mut at = crate::vm_api::ScriptLineCol::default();
        self.field_loc_into(made_at, field, &mut crate::vm_api::ScriptTextOut::new(&mut file), &mut at)
            .then(|| ScriptLoc::new(&file, at.line, at.col))
    }
    pub fn field_loc_into(&self, made_at: ScriptIp, field: LiveId, file: &mut crate::vm_api::ScriptTextOut, at: &mut crate::vm_api::ScriptLineCol) -> bool {
        let mut sink = ScriptQuerySink::new();
        sink.text0(file);
        let answer = self.query(QUERY_FIELD_LOC, &mut sink, |req| {
            req.ip = made_at;
            req.id = field;
        });
        *at = answer.at;
        answer.found
    }

    pub(crate) fn field_loc_native(&self, made_at: ScriptIp, field: LiveId, file: &mut crate::vm_api::ScriptTextOut, at: &mut crate::vm_api::ScriptLineCol) -> bool {
        let found = (|| -> Option<ScriptLoc> {
            if made_at.is_unknown() {
                return None;
            }
            let bodies = self.bodies.borrow();
            let body = bodies.get(made_at.body as usize)?;
            let source_map = &body.parser.source_map;
            let ip_index = (made_at.index as usize).min(source_map.len().saturating_sub(1));
            let token_index = (0..=ip_index)
                .rev()
                .find_map(|i| source_map.get(i).and_then(|slot| *slot))
                .or_else(|| ((ip_index + 1)..source_map.len()).find_map(|i| source_map.get(i).and_then(|slot| *slot)))? as usize;
            let tokens = &body.tokenizer.tokens;
            let mut k = tokens[token_index..].iter().position(|t| matches!(t.token, ScriptToken::OpenCurly))? + token_index;
            let mut depth = 0usize;
            while k < tokens.len() {
                match &tokens[k].token {
                    ScriptToken::OpenCurly | ScriptToken::OpenRound | ScriptToken::OpenSquare => depth += 1,
                    ScriptToken::CloseCurly | ScriptToken::CloseRound | ScriptToken::CloseSquare => {
                        depth = depth.checked_sub(1)?;
                        if depth == 0 {
                            return None;
                        }
                    }
                    ScriptToken::Identifier(id) if depth == 1 && *id == field => {
                        let op = match tokens.get(k + 1).map(|t| &t.token) {
                            Some(ScriptToken::Operator(op)) | Some(ScriptToken::Separator(op)) => *op,
                            _ => LiveId(0),
                        };
                        if op == id!(:) || op == id!(:=) || op == id!(+:) {
                            // (The identifier's first character: a token's
                            // position may sit a character into it.)
                            let (row, col) = body.tokenizer.token_start_row_col(k as u32)?;
                            return Some(match &body.source {
                                ScriptSource::Mod(script_mod) => ScriptLoc { file: body.site_file(script_mod), line: row + script_mod.line as u32, col },
                                _ => ScriptLoc { file: "generated".into(), line: row, col },
                            });
                        }
                    }
                    _ => {}
                }
                k += 1;
            }
            None
        })();
        if let Some(loc) = found {
            file.push(&loc.file); at.line = loc.line; at.col = loc.col; true
        } else { false }
    }

    /// Visit the tokens of the fn whose body starts at `ip`, from its `fn`
    /// keyword through the body's closing `}`: each token with the `/** */`
    /// annotation in front of it and, for a `#(…)` token, the value it
    /// interpolates. What a host hashes to know a function by its text in
    /// any process (heap indices and ips differ between processes, the
    /// tokens do not). False when `ip` maps to no fn.
    pub fn fn_tokens<F: FnMut(&ScriptTokenPos, Option<&str>, Option<ScriptValue>)>(
        &self, ip: ScriptIp, mut f: F,
    ) -> bool {
        unsafe extern "C-unwind" fn visit<F: FnMut(&ScriptTokenPos, Option<&str>, Option<ScriptValue>)>(
            ctx: *mut u8, token: *const ScriptTokenPos, doc: *const u8, doc_len: usize, value: *const ScriptValue,
        ) {
            let f = &mut *(ctx as *mut F);
            let doc = if doc.is_null() { None } else { Some(std::str::from_utf8_unchecked(std::slice::from_raw_parts(doc, doc_len))) };
            f(&*token, doc, value.as_ref().copied());
        }
        unsafe { self.fn_tokens_with(ip, &mut f as *mut F as *mut u8, visit::<F>) }
    }

    /// Visit tokens while their backing source is borrowed inside the VM.
    /// # Safety
    /// `ctx` must be valid for synchronous calls to `visit`. Token, doc and value
    /// pointers are borrowed for that callback only; absent doc/value are null.
    pub unsafe fn fn_tokens_with(
        &self, ip: ScriptIp, ctx: *mut u8,
        visit: unsafe extern "C-unwind" fn(*mut u8, *const ScriptTokenPos, *const u8, usize, *const ScriptValue),
    ) -> bool {
        let bodies = self.bodies.borrow();
        let Some(body) = bodies.get(ip.body as usize) else {
            return false;
        };
        let source_map = &body.parser.source_map;
        let ip_index = (ip.index as usize).min(source_map.len().saturating_sub(1));
        let Some(token_index) = (0..=ip_index)
            .rev()
            .find_map(|i| source_map.get(i).and_then(|slot| *slot))
            .or_else(|| ((ip_index + 1)..source_map.len()).find_map(|i| source_map.get(i).and_then(|slot| *slot)))
        else {
            return false;
        };
        let tokens = &body.tokenizer.tokens;
        let Some(last) = tokens.len().checked_sub(1) else {
            return false;
        };
        let mut k = (token_index as usize).min(last);
        while !matches!(tokens[k].token, ScriptToken::Identifier(id) if id == id!(fn)) {
            let Some(prev) = k.checked_sub(1) else {
                return false;
            };
            k = prev;
        }
        let values = match &body.source {
            ScriptSource::Mod(script_mod) => &script_mod.values[..],
            _ => &[],
        };
        let docs = &body.tokenizer.docs;
        let mut doc = docs.partition_point(|d| (d.next_token as usize) < k);
        let mut depth = 0i32;
        for (i, tok) in tokens.iter().enumerate().skip(k) {
            let mut text = None;
            while doc < docs.len() && docs[doc].next_token as usize <= i {
                if docs[doc].next_token as usize == i {
                    text = Some(docs[doc].text.as_str());
                }
                doc += 1;
            }
            let value = tok.token.as_rust_value().and_then(|v| values.get(v as usize).copied());
            let (doc, doc_len) = text.map(|s| (s.as_ptr(), s.len())).unwrap_or((std::ptr::null(), 0));
            visit(ctx, tok, doc, doc_len, value.as_ref().map_or(std::ptr::null(), |v| v));
            match tok.token {
                ScriptToken::OpenCurly => depth += 1,
                ScriptToken::CloseCurly => {
                    depth -= 1;
                    if depth == 0 {
                        return true;
                    }
                }
                _ => {}
            }
        }
        true
    }

    /// The text of the line `ip` is on, as the code was evaluated (a host
    /// whose source was rewritten before evaluation finds the same line in
    /// what was written).
    #[inline(always)]
    pub fn ip_line_text(&self, ip: ScriptIp) -> Option<String> {
        let mut text = String::new();
        self.ip_line_text_into(ip, &mut crate::vm_api::ScriptTextOut::new(&mut text)).then_some(text)
    }
    pub fn ip_line_text_into(&self, ip: ScriptIp, out: &mut crate::vm_api::ScriptTextOut) -> bool {
        let mut sink = ScriptQuerySink::new();
        sink.text0(out);
        self.query(QUERY_IP_LINE_TEXT, &mut sink, |req| req.ip = ip).found
    }

    pub(crate) fn ip_line_text_native(&self, ip: ScriptIp, out: &mut crate::vm_api::ScriptTextOut) -> bool {
        let found = (|| -> Option<String> {
            let loc = self.ip_to_loc_plain(ip)?;
            let bodies = self.bodies.borrow();
            let body = bodies.get(ip.body as usize)?;
            let base = match &body.source {
                ScriptSource::Mod(script_mod) => script_mod.line as u32,
                _ => 0,
            };
            body.code_text().lines().nth(loc.line.checked_sub(base)? as usize).map(str::to_string)
        })();
        if let Some(text) = found { out.push(&text); true } else { false }
    }

    /// True when `ip` is in the body of an adopted fn ([`ScriptVm::adopt_fn`]).
    pub fn ip_is_adopted(&self, ip: ScriptIp) -> bool {
        self.bodies.borrow().get(ip.body as usize).is_some_and(|b| !b.real_file.is_empty())
    }

    #[inline(always)]
    pub fn ip_to_loc(&self, ip: ScriptIp) -> Option<ScriptLoc> {
        let mut file = String::new();
        let mut at = crate::vm_api::ScriptLineCol::default();
        self.ip_to_loc_into(ip, &mut crate::vm_api::ScriptTextOut::new(&mut file), &mut at)
            .then(|| ScriptLoc::new(&file, at.line, at.col))
    }
    pub fn ip_to_loc_into(&self, ip: ScriptIp, file: &mut crate::vm_api::ScriptTextOut, at: &mut crate::vm_api::ScriptLineCol) -> bool {
        let mut sink = ScriptQuerySink::new();
        sink.text0(file);
        let answer = self.query(QUERY_IP_TO_LOC, &mut sink, |req| req.ip = ip);
        *at = answer.at;
        answer.found
    }

    /// [`Self::ip_to_loc`] for the VM-side code that already is the VM.
    pub(crate) fn ip_to_loc_plain(&self, ip: ScriptIp) -> Option<ScriptLoc> {
        let (mut file, mut at) = (String::new(), crate::vm_api::ScriptLineCol::default());
        self.ip_to_loc_native(ip, &mut crate::vm_api::ScriptTextOut::new(&mut file), &mut at)
            .then(|| ScriptLoc::new(&file, at.line, at.col))
    }

    pub(crate) fn ip_to_loc_native(&self, ip: ScriptIp, file: &mut crate::vm_api::ScriptTextOut, at: &mut crate::vm_api::ScriptLineCol) -> bool {
        let found = (|| -> Option<ScriptLoc> {
            if let Some(body) = self.bodies.borrow().get(ip.body as usize) {
                let source_map = &body.parser.source_map;
                let ip_index = ip.index as usize;

                let direct_token = source_map.get(ip_index).and_then(|slot| *slot);
                // Some opcodes are synthetic and have `None` in source_map.
                // For error reporting, fall back to the nearest mapped token so we still
                // surface a real file/line instead of "unknown".
                let nearest_token = if direct_token.is_some() {
                    direct_token
                } else {
                    let left = ip_index.min(source_map.len().saturating_sub(1));
                    let left_token = (0..=left)
                        .rev()
                        .find_map(|idx| source_map.get(idx).and_then(|slot| *slot));
                    if left_token.is_some() {
                        left_token
                    } else {
                        ((ip_index + 1)..source_map.len())
                            .find_map(|idx| source_map.get(idx).and_then(|slot| *slot))
                    }
                };

                if let Some(token_index) = nearest_token {
                    if let Some(rc) = body.tokenizer.token_index_to_row_col(token_index) {
                        if let ScriptSource::Mod(script_mod) = &body.source {
                            return Some(ScriptLoc {
                                file: match (&body.shown, body.real_file.is_empty()) {
                                    (Some(shown), _) => shown.clone(),
                                    (None, true) => script_mod.file.clone(),
                                    (None, false) => body.real_file.clone(),
                                },
                                line: rc.0 + script_mod.line as u32,
                                col: rc.1,
                            });
                        }
                        return Some(ScriptLoc {
                            file: "generated".into(),
                            line: rc.0,
                            col: rc.1,
                        });
                    }
                }
            }
            return Some(ScriptLoc {
                file: "unknown".into(),
                line: ip.body as _,
                col: ip.index as _,
            });
        })();
        if let Some(loc) = found {
            file.push(&loc.file); at.line = loc.line; at.col = loc.col; true
        } else { false }
    }
}

#[relax_api(plain)]
#[repr(C)]
pub struct ScriptVm<'a> {
    pub host: ScriptHostContext<'a>,
    pub bx: Box<ScriptVmBase>,
}

#[relax_api(plain)]
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct ScriptRunBudget {
    pub soft_deadline: f64,
    pub hard_deadline: f64,
    pub sample_interval_instructions: u32,
    pub instructions_until_sample: u32,
}

impl ScriptRunBudget {
    pub fn from_durations(soft: Duration, hard: Duration, sample_interval_instructions: u32) -> Self {
        let now = crate::clock::monotonic_now();
        let sample_interval_instructions = sample_interval_instructions.max(1);
        Self {
            soft_deadline: now + soft.as_secs_f64(),
            hard_deadline: now + hard.as_secs_f64(),
            sample_interval_instructions,
            instructions_until_sample: sample_interval_instructions,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ScriptRunBudgetHit {
    Soft,
    Hard,
}

#[relax_api]
impl<'a> ScriptVm<'a> {
    /// Where the running code is: the current instruction, then the return
    /// sites of the calls it is inside, innermost first, at most `max`. A
    /// native reads where it was called from, and through which calls (a
    /// host that records what code drew what).
    /// The loops running now on this thread, outermost first: each loop
    /// variable's name and value (`for li in 0..n` gives `li` and the
    /// iteration's number; a key or index variable too). For a host that
    /// records which iteration drew something (an editor's traced frame).
    #[inline(always)]
    pub fn loop_values(&self, out: &mut Vec<(LiveId, f64)>) {
        unsafe extern "C-unwind" fn push(ctx: *mut u8, bytes: *const u8, _len: usize) {
            let value = std::ptr::read_unaligned(bytes as *const ScriptLoopValue);
            (&mut *(ctx as *mut Vec<(LiveId, f64)>)).push((value.id, value.value));
        }
        out.clear();
        let mut sink = ScriptQuerySink::new();
        sink.records = Some((out as *mut Vec<(LiveId, f64)> as *mut u8, push));
        self.bx.query(QUERY_LOOP_VALUES, &mut sink, |_| {});
    }

    /// Where the running code is: the instruction now, then each caller's
    /// call, innermost first (at most `max`). A caller's frame keeps where
    /// it returns to, the instruction after its call; its site is the call
    /// itself, so a caller maps to the line it called from.
    #[inline(always)]
    pub fn call_sites(&self, out: &mut Vec<ScriptIp>, max: usize) {
        unsafe extern "C-unwind" fn push(ctx: *mut u8, bytes: *const u8, _len: usize) {
            (&mut *(ctx as *mut Vec<ScriptIp>)).push(std::ptr::read_unaligned(bytes as *const ScriptIp));
        }
        out.clear();
        let mut sink = ScriptQuerySink::new();
        sink.records = Some((out as *mut Vec<ScriptIp> as *mut u8, push));
        self.bx.query(QUERY_CALL_SITES, &mut sink, |req| req.index = max);
    }

    /// Bail out of the interpreter with a script error.
    /// Use this when a stack (mes, scopes, loops, calls) is unexpectedly empty,
    /// indicating corrupted bytecode (e.g. from incomplete streaming input).
    /// Sets trap.on to Return(err) so run_core exits cleanly.
    pub(crate) fn bail(&mut self, msg: &str) {
        let err = script_err_unexpected!(self.bx.threads.cur_ref().trap, "{}", msg);
        self.bx
            .threads
            .cur()
            .trap
            .set_on(Some(ScriptTrapOn::Bail(err)));
    }

    #[inline(always)]
    pub fn with_instruction_limit<R>(
        &mut self,
        instruction_limit: usize,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let (previous_remaining, applied) = self.instruction_limit_begin(instruction_limit);
        let result = f(self);
        self.instruction_limit_end(previous_remaining, applied);
        result
    }

    /// The first half of [`Self::with_instruction_limit`]: applies the limit, returns
    /// (the thread's previous remaining budget, the applied limit) for
    /// [`Self::instruction_limit_end`].
    #[inline(always)]
    pub fn instruction_limit_begin(&mut self, instruction_limit: usize) -> (Option<usize>, usize) {
        let previous = self.bx.vm_stat(&mut ScriptCtlReq::new(STAT_LIMIT_REMAINING));
        let previous_remaining = if previous == u64::MAX { None } else { Some(previous as usize) };
        let applied = previous_remaining
            .map(|remaining| remaining.min(instruction_limit))
            .unwrap_or(instruction_limit);
        self.bx.vm_ctl(&mut ScriptCtlReq::with(CTL_LIMIT_SET, applied as u64));
        (previous_remaining, applied)
    }

    /// The second half of [`Self::with_instruction_limit`].
    #[inline(always)]
    pub fn instruction_limit_end(&mut self, previous_remaining: Option<usize>, applied: usize) {
        let mut req = ScriptCtlReq::with(CTL_LIMIT_END, previous_remaining.map_or(u64::MAX, |n| n as u64));
        req.b = applied as u64;
        self.bx.vm_ctl(&mut req);
    }

    /// Runs an operation with a cap on live VM operand values.
    ///
    /// Nested caps can only narrow the current limit. A paused continuation
    /// retains the cap that began its evaluation, matching instruction-limit
    /// behavior.
    pub fn with_stack_value_limit<R>(
        &mut self,
        stack_value_limit: usize,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let previous_limit = self.bx.threads.cur_ref().stack_limit;
        self.bx.threads.cur().stack_limit = previous_limit.min(stack_value_limit);
        let result = f(self);
        if !self.bx.threads.cur_ref().is_paused() {
            self.bx.threads.cur().stack_limit = previous_limit;
        }
        result
    }

    /// Runs an operation with a cap on active VM call frames.
    ///
    /// The count includes a root evaluation frame. The raw Makepad VM has no
    /// call-frame cap, while nested bounded executions retain the narrower
    /// cap. A paused continuation retains its starting cap.
    pub fn with_call_frame_limit<R>(
        &mut self,
        call_frame_limit: usize,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let previous_limit = self.bx.threads.cur_ref().call_frame_limit;
        self.bx.threads.cur().call_frame_limit = Some(
            previous_limit
                .map(|previous| previous.min(call_frame_limit))
                .unwrap_or(call_frame_limit),
        );
        let result = f(self);
        if !self.bx.threads.cur_ref().is_paused() {
            self.bx.threads.cur().call_frame_limit = previous_limit;
        }
        result
    }

    /// Clears resource-limit signals that did not belong to an active VM
    /// instruction. Hosts normally do not need this: `run_core` consumes its
    /// own failures, and Octoscript clears stale signals before a fresh eval.
    pub fn clear_execution_limit_failures(&mut self) {
        self.bx.threads.cur().take_stack_limit_exceeded();
        self.bx.threads.cur().take_call_frame_limit_exceeded();
    }

    /// Run one untrusted evaluation/callback under a logical heap-allocation
    /// ceiling. The default VM has no ceiling; callers opt in explicitly, so
    /// trusted widget, shader and Studio script execution is unchanged.
    ///
    /// Container growth is charged before allocation. If a script exceeds
    /// the allowance, the current run bails with a captured `script limit`
    /// error instead of attempting the allocation. The report lets a host
    /// share one cumulative allowance across several callbacks in a tick.
    #[inline(always)]
    pub fn with_heap_allocation_limit<R>(&mut self, allocation_limit: usize, f: impl FnOnce(&mut Self) -> R) -> (R, ScriptAllocationReport) {
        self.bx.vm_ctl(&mut ScriptCtlReq::with(CTL_ALLOC_BEGIN, allocation_limit as u64));
        let result = f(self);
        let mut req = ScriptCtlReq::new(CTL_ALLOC_END);
        let allocated_bytes = self.bx.vm_ctl(&mut req) as usize;
        (result, ScriptAllocationReport { allocated_bytes, exceeded: req.b != 0 })
    }

    /// Instructions charged by the most recent `with_instruction_limit` call.
    #[inline(always)]
    pub fn last_limit_consumed(&self) -> usize {
        self.bx.vm_stat(&mut ScriptCtlReq::new(STAT_LAST_LIMIT_CONSUMED)) as usize
    }

    #[inline(always)]
    pub fn heap(&self) -> &ScriptHeap {
        self.bx.parts().heap
    }

    #[inline(always)]
    pub fn heap_mut(&mut self) -> &mut ScriptHeap {
        self.bx.parts_mut().heap
    }

    /// Print a script value to stdout with debug formatting.
    pub fn println(&self, value: impl Into<ScriptValue>) {
        self.bx.heap.println(value.into());
    }

    /// Run garbage collection (mark and sweep). Deliberately does NOT return
    /// backing capacity to the allocator: routine GC (paint loop, isolate
    /// round-robin) refills the free lists at the same rate, and repeated
    /// shrink/regrow churn costs more than the high-water memory it saves.
    #[inline(always)]
    pub fn gc(&mut self) {
        self.bx.vm_ctl(&mut ScriptCtlReq::new(CTL_GC));
    }

    /// [`Self::gc`] plus returning over-allocated backing capacity (safe: no
    /// live slot is moved or removed). Call after a known allocation spike —
    /// a world teardown, an eval reset — not on the routine GC cadence.
    #[inline(always)]
    pub fn gc_and_compact(&mut self) {
        self.bx.vm_ctl(&mut ScriptCtlReq::new(CTL_GC_AND_COMPACT));
    }

    /// Free a host-created transient value (a per-call args container, a
    /// per-tick input object) immediately instead of leaving it for GC. Safe
    /// by construction: if the script retained the value, the escape barrier
    /// (`ScriptHeap::escape_value`) tagged it REFFED and this is a no-op —
    /// normal GC handles it. Two caveats for hosts: (1) values bound as fn
    /// args of a call that PAUSED are still live in the paused scope without
    /// being REFFED — only release after the call completed unpaused (check
    /// `vm.thread().is_paused()`); (2) values stored via the `*_unchecked`
    /// heap fns bypass the barrier — releasing those is the host's own
    /// responsibility.
    #[inline(always)]
    pub fn release_transient(&mut self, v: ScriptValue) {
        let mut req = ScriptCtlReq::new(CTL_RELEASE_TRANSIENT);
        req.value = v;
        self.bx.vm_ctl(&mut req);
    }

    /// Run garbage collection with status logging.
    #[inline(always)]
    pub fn gc_with_status(&mut self) {
        self.bx.vm_ctl(&mut ScriptCtlReq::new(CTL_GC_WITH_STATUS));
    }

    #[inline(always)]
    pub fn thread(&self) -> &ScriptThread {
        self.bx.parts().thread
    }

    pub fn thread_mut(&mut self) -> &mut ScriptThread {
        self.bx.threads.cur()
    }

    /// Pushes a type-mismatch error carrying `message` onto the current thread's trap.
    #[inline(always)]
    pub fn push_type_mismatch_error(&mut self, message: &str) {
        let mut req = ScriptErrorsReq::new(ERR_PUSH_TYPE_MISMATCH);
        req.text = message;
        self.bx.vm_errors(&mut req);
    }

    #[inline(always)]
    pub fn trap(&'a self) -> ScriptTrap<'a> {
        ScriptTrap::Inner(self.bx.parts().trap)
    }

    /// Format an enum variant error with descriptive information about the value.
    /// Used by generated code from derive macros for better error messages.
    #[inline(always)]
    pub fn format_enum_variant_error(&self, value: ScriptValue) -> String {
        let mut out = String::new();
        self.format_enum_variant_error_into(value, &mut crate::vm_api::ScriptTextOut::new(&mut out));
        out
    }

    #[inline(always)]
    pub fn format_enum_variant_error_into(&self, value: ScriptValue, out: &mut crate::vm_api::ScriptTextOut) {
        let mut sink = ScriptQuerySink::new();
        sink.text0(out);
        self.bx.query(QUERY_FORMAT_ENUM_VARIANT, &mut sink, |req| req.value = value);
    }

    /// Format a ScriptObject for error messages with a brief debug representation.
    /// Shows the object's proto chain and key properties.
    #[inline(always)]
    pub fn format_object_for_error(&self, obj: ScriptObject) -> String {
        let mut out = String::new();
        self.format_object_for_error_into(obj, &mut crate::vm_api::ScriptTextOut::new(&mut out));
        out
    }

    #[inline(always)]
    pub fn format_object_for_error_into(&self, obj: ScriptObject, out: &mut crate::vm_api::ScriptTextOut) {
        let mut sink = ScriptQuerySink::new();
        sink.text0(out);
        self.bx.query(QUERY_FORMAT_OBJECT, &mut sink, |req| req.value = obj.into());
    }

    pub fn set_thread(&mut self, id: usize) {
        self.bx.threads.set_current(id);
    }

    pub fn with_vm<R, F: FnOnce(&mut ScriptVm) -> R>(&mut self, f: F) -> R {
        f(self)
    }

    #[inline(always)]
    pub fn is_reload(&self) -> bool {
        self.bx.vm_stat(&mut ScriptCtlReq::new(STAT_RELOAD)) != 0
    }

    #[inline(always)]
    pub fn with_reload<R, F: FnOnce(&mut ScriptVm) -> R>(&mut self, f: F) -> R {
        let was_reload = self.set_reload(true);
        let out = f(self);
        self.set_reload(was_reload);
        out
    }

    /// Sets whether applies are reloads; returns the previous setting.
    #[inline(always)]
    pub fn set_reload(&mut self, is_reload: bool) -> bool {
        self.bx.vm_ctl(&mut ScriptCtlReq::with(CTL_SET_RELOAD, is_reload as u64)) != 0
    }

    fn script_me_from_value(&mut self, me: ScriptValue) -> Option<ScriptMe> {
        if me.is_nil() {
            return None;
        }
        if let Some(obj) = me.as_object() {
            return Some(ScriptMe::Object(obj));
        }
        if let Some(arr) = me.as_array() {
            return Some(ScriptMe::Array(arr));
        }
        if let Some(pod) = me.as_pod() {
            return Some(ScriptMe::Pod {
                pod,
                offset: Default::default(),
            });
        }
        None
    }

    fn call_with_scope(&mut self, scope: ScriptObject, me: ScriptValue) -> ScriptValue {
        if let Some(fnptr) = self.bx.heap.parent_as_fn(scope) {
            match fnptr {
                ScriptFnPtr::Native(ni) => {
                    // Get the function pointer and drop the borrow before calling
                    let func = self.bx.code.native.borrow().functions[ni.index as usize].view();
                    // Pause thread before native call so re-entrant calls get a different thread
                    self.bx.threads.cur().is_paused = true;
                    // SAFETY: The function pointer is valid as long as native functions aren't removed during execution
                    let result = unsafe { func.call(self, scope) };
                    // Only unpause if native didn't explicitly pause (via pause() which sets trap.on to Pause)
                    if !matches!(
                        self.bx.threads.cur().trap.get_on(),
                        Some(ScriptTrapOn::Pause)
                    ) {
                        self.bx.threads.cur().is_paused = false;
                        // Eager-free the call scope (same contract as
                        // handle_call_exec): a native that stored or returned
                        // it left it REFFED / guarded.
                        if result.as_object() != Some(scope) {
                            self.bx.heap.free_object_if_unreffed(scope);
                        }
                    }
                    return result;
                }
                ScriptFnPtr::Script(sip) => {
                    let call = CallFrame {
                        bases: self.bx.threads.cur_ref().new_bases(),
                        args: OpcodeArgs::default(),
                        return_ip: None,
                        prev_slot_base: self.bx.threads.cur_ref().slot_base,
                        body_root: false,
                    };
                    if !self.bx.threads.cur().push_call_frame(call) {
                        return self
                            .handle_execution_limit_failure()
                            .expect("a rejected call frame raises a limit failure");
                    }
                    self.bx.threads.cur().scopes.push(scope);
                    if let Some(me) = self.script_me_from_value(me) {
                        self.bx.threads.cur().mes.push(me);
                    }
                    self.bx.threads.cur().trap.ip = sip;
                    self.drain_stale_errors();
                    return self.run_core();
                }
            }
        } else {
            return script_err_wrong_value!(
                self.bx.threads.cur_ref().trap,
                "call target is not a function (got {:?})",
                self.bx.heap.proto(scope).value_type()
            );
        }
    }

    #[inline(always)]
    pub fn call(&mut self, fnobj: ScriptValue, args: &[ScriptValue]) -> ScriptValue {
        self.call_with_me(fnobj, args, NIL)
    }

    #[inline(always)]
    pub fn call_with_me(&mut self, fnobj: ScriptValue, args: &[ScriptValue], me: ScriptValue) -> ScriptValue {
        let mut req = ScriptCallReq::new(CALL_ARGS, fnobj);
        req.args = args;
        req.me = me;
        self.vm_call(&mut req)
    }

    /// The one entry into running script from the host (the op `call`, `resume` and
    /// the like are inline decoders of): `req.mode` says what, see [`ScriptCallReq`].
    #[relax_api]
    pub fn vm_call(&mut self, req: &mut ScriptCallReq) -> ScriptValue {
        match req.mode {
            CALL_ARGS => self.call_with_me_impl(req.fnobj, req.args, req.me),
            CALL_OBJECT => self.call_with_args_object_with_me_impl(req.fnobj, req.args_obj, req.me),
            CALL_TRANSFORM => match self.call_apply_transform_impl(req.fnobj) {
                Some(value) => {
                    req.found = true;
                    value
                }
                None => {
                    req.found = false;
                    NIL
                }
            },
            CALL_RESUME => {
                self.bx.threads.cur().is_paused = false;
                self.run_core()
            }
            mode => panic!("vm_call: unknown mode {mode}"),
        }
    }

    fn call_with_me_impl(
        &mut self,
        fnobj: ScriptValue,
        args: &[ScriptValue],
        me: ScriptValue,
    ) -> ScriptValue {
        let scope = self.bx.heap.new_with_proto(fnobj);

        self.bx.heap.clear_object_deep(scope);
        if fnobj.is_err() {
            return fnobj;
        }

        let trap = self.bx.threads.cur().trap.pass();
        let err = self.bx.heap.push_all_fn_args(scope, args, trap);
        if err.is_err() {
            return err;
        }

        self.bx.heap.set_object_deep(scope);
        self.bx.heap.set_object_storage_auto(scope);
        self.call_with_scope(scope, me)
    }

    pub fn call_with_args_object(
        &mut self,
        fnobj: ScriptValue,
        args_obj: ScriptObject,
    ) -> ScriptValue {
        self.call_with_args_object_with_me(fnobj, args_obj, NIL)
    }

    #[inline(always)]
    pub fn call_with_args_object_with_me(&mut self, fnobj: ScriptValue, args_obj: ScriptObject, me: ScriptValue) -> ScriptValue {
        let mut req = ScriptCallReq::new(CALL_OBJECT, fnobj);
        req.args_obj = args_obj;
        req.me = me;
        self.vm_call(&mut req)
    }

    fn call_with_args_object_with_me_impl(
        &mut self,
        fnobj: ScriptValue,
        args_obj: ScriptObject,
        me: ScriptValue,
    ) -> ScriptValue {
        if fnobj.is_err() {
            return fnobj;
        }
        if fnobj.as_object().is_none() {
            return script_err_wrong_value!(
                self.bx.threads.cur_ref().trap,
                "call target is not a function (got {:?})",
                fnobj.value_type()
            );
        }

        let scope = self.bx.heap.new_with_proto(fnobj);
        self.bx.heap.set_object_storage_vec2(scope);
        self.bx.heap.clear_object_deep(scope);

        let trap = self.bx.threads.cur().trap.pass();
        // Map positional (unnamed) vec args to named function parameters,
        // and merge named map args directly.
        let vec_len = self.bx.heap.vec_len(args_obj);
        for i in 0..vec_len {
            let kv = self.bx.heap.vec_key_value(args_obj, i, trap);
            if kv.key.is_nil() {
                // Unnamed positional arg — map to named parameter via unnamed_fn_arg
                self.bx.heap.unnamed_fn_arg(scope, kv.value, trap);
            } else {
                // Named arg — insert directly
                self.bx.heap.vec_push(scope, kv.key, kv.value, trap);
            }
        }
        // Copy map entries (like `self`, `ui`) from args_obj to scope
        let map_entries: Vec<_> = self
            .bx
            .heap
            .map_ref(args_obj)
            .iter()
            .map(|(k, v)| (*k, v.value))
            .collect();
        for (k, v) in map_entries {
            self.bx.heap.force_value_in_map(scope, k, v);
        }

        self.bx.heap.set_object_deep(scope);
        self.bx.heap.set_object_storage_auto(scope);
        self.call_with_scope(scope, me)
    }

    /// Drain pending errors into formatted strings instead of logging them.
    /// Note that errors raised DURING execution are drained by `run_core`
    /// itself (into the log, or into the captured-error sink when one is
    /// installed) — this only sees errors still queued afterwards. Hosts that
    /// need reliable capture install a sink: `vm.bx.captured_errors =
    /// Some(Vec::new())` before running, then take it after.
    pub fn take_errors(&mut self) -> Vec<String> {
        self.take_error_records().into_iter().map(|e| e.text().to_owned()).collect()
    }

    /// [`Self::take_errors`] with each error's location and message apart.
    pub fn take_error_records(&mut self) -> Vec<ScriptErrorRecord> {
        unsafe extern "C-unwind" fn push(ctx: *mut u8, record: *const ScriptErrorRecord) {
            (&mut *(ctx as *mut Vec<ScriptErrorRecord>)).push((&*record).clone());
        }
        let mut out = Vec::new();
        let mut req = ScriptErrorsReq::new(ERR_TAKE);
        req.ctx = &mut out as *mut Vec<ScriptErrorRecord> as *mut u8;
        req.push = push;
        self.bx.vm_errors(&mut req);
        out
    }

    /// Drain and log any pending errors in the error queue.
    /// Call this after operations that may produce errors outside of run_core
    /// (e.g., script_apply calls from Rust code).
    ///
    /// When a captured-error sink is installed (`bx.captured_errors`), errors
    /// go there instead of the log — even while `silence_errors` is set, so a
    /// host can collect diagnostics from streaming/incremental evals that
    /// would otherwise be dropped as meaningless-mid-stream.
    #[inline(always)]
    pub fn drain_errors(&mut self) {
        self.bx.vm_errors(&mut ScriptErrorsReq::new(ERR_DRAIN));
    }

    #[inline(never)]
    #[cold]
    fn handle_errors(&mut self) {
        if self.bx.threads.cur_ref().call_stack_has_try() {
            // Discard younger script calls until the active frame owns a try
            // frame, restoring that frame's instruction body. Hard VM bails
            // (ScriptTrapOn::Bail) never enter this path.
            while !self.bx.threads.cur_ref().call_has_try() {
                let Some(call) = self.bx.threads.cur().calls.pop() else {
                    self.bail("calls empty while unwinding to a try frame");
                    return;
                };
                let return_ip = call.return_ip;
                self.bx.threads.cur().slot_base = call.prev_slot_base;
                self.bx
                    .threads
                    .cur()
                    .truncate_bases(call.bases, &mut self.bx.heap);
                let Some(return_ip) = return_ip else {
                    self.bail("root call reached while unwinding to a try frame");
                    return;
                };
                self.bx.threads.cur().trap.ip = return_ip;
            }
            // pop all errors
            self.bx.threads.cur().trap.err_clear();
            let try_frame = self.bx.threads.cur().tries.pop().unwrap();
            self.bx
                .threads
                .cur()
                .truncate_bases(try_frame.bases, &mut self.bx.heap);
            if try_frame.push_nil {
                self.bx.threads.cur().push_stack_unchecked(NIL)
            }
            self.bx
                .threads
                .cur()
                .trap
                .goto(try_frame.start_ip + try_frame.jump);
        } else {
            // An uncaught error is reported and the evaluation continues with
            // the error value in hand: one bad statement in a module must not
            // take the rest of the module with it, a live edit is broken half
            // the time, and scripts inspect returned error values (`?`).
            //
            // A host that runs script it did not write opts into
            // `bail_on_uncaught_error`: the error then terminates the
            // evaluation before another instruction (and therefore another
            // host effect) can execute, and rides the Bail back as its result.
            // Streaming evals (`silence_errors`) never bail: incomplete source
            // inevitably raises errors that mean nothing until the rest arrives.
            if !self.bx.bail_on_uncaught_error || self.bx.silence_errors {
                self.drain_errors();
                return;
            }
            let error = self
                .bx
                .threads
                .cur_ref()
                .trap
                .err_borrow()
                .front()
                .map(|e| e.value);
            self.drain_errors();
            if let Some(error) = error {
                self.bx
                    .threads
                    .cur()
                    .trap
                    .set_on(Some(ScriptTrapOn::Bail(error)));
            }
        }
    }

    fn check_run_budget(&mut self) -> Option<ScriptRunBudgetHit> {
        let budget = self.bx.run_budget.as_mut()?;
        budget.instructions_until_sample = budget.instructions_until_sample.saturating_sub(1);
        if budget.instructions_until_sample > 0 {
            return None;
        }
        budget.instructions_until_sample = budget.sample_interval_instructions;

        let now = crate::clock::monotonic_now();
        if now >= budget.hard_deadline {
            return Some(ScriptRunBudgetHit::Hard);
        }
        if now >= budget.soft_deadline {
            return Some(ScriptRunBudgetHit::Soft);
        }
        None
    }

    #[inline(never)]
    #[cold]
    fn bail_resource_limit(&mut self, message: &'static str) -> ScriptValue {
        let err = script_err_limit!(self.bx.threads.cur_ref().trap, "{message}");
        // Resource-limit failures are uncatchable. Drain pre-existing script
        // errors before the Bail unwinds to a clean root state.
        self.drain_errors();
        self.bx
            .threads
            .cur()
            .trap
            .set_on(Some(ScriptTrapOn::Bail(err)));
        self.handle_trap_on()
            .expect("resource-limit Bail must return a VM value")
    }

    /// Errors raised while no script was running (a host apply, a host call of
    /// a value that is not a function) belong to no evaluation. They are
    /// reported here, at the Rust -> script boundary, so the run that starts
    /// now can neither be caught by them nor be ended by them.
    #[inline(always)]
    fn drain_stale_errors(&mut self) {
        if self.bx.threads.cur_ref().trap.has_err() {
            self.drain_errors();
        }
    }

    #[inline(never)]
    #[cold]
    fn handle_execution_limit_failure(&mut self) -> Option<ScriptValue> {
        let stack_limit_exceeded = self.bx.threads.cur().take_stack_limit_exceeded();
        let call_frame_limit_exceeded = self.bx.threads.cur().take_call_frame_limit_exceeded();
        if stack_limit_exceeded {
            return Some(self.bail_resource_limit("script operand stack limit exceeded"));
        }
        if call_frame_limit_exceeded {
            return Some(self.bail_resource_limit("script call frame limit exceeded"));
        }
        None
    }

    fn handle_trap_on(&mut self) -> Option<ScriptValue> {
        if self.bx.threads.cur().trap.on_is_none() {
            return None;
        }
        Some(match self.bx.threads.cur().trap.take_on().unwrap() {
            ScriptTrapOn::Pause | ScriptTrapOn::TimeBudgetYield => NIL,
            ScriptTrapOn::Return(value) => {
                // Preserve the remaining allowance for consumption accounting
                // (with_instruction_limit reads it after the None wipe).
                if let Some(rem) = self.bx.threads.cur().instruction_limit_remaining.take() {
                    self.bx.last_limit_exit_remaining = rem;
                }
                value
            }
            ScriptTrapOn::Bail(value) => {
                // Stack corruption or hard failure: unwind calls to find our root frame
                // and truncate all stacks back to clean state.
                loop {
                    if let Some(call) = self.bx.threads.cur().calls.pop() {
                        self.bx.threads.cur().slot_base = call.prev_slot_base;
                        self.bx
                            .threads
                            .cur()
                            .truncate_bases(call.bases, &mut self.bx.heap);
                        if call.return_ip.is_none() {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                if let Some(rem) = self.bx.threads.cur().instruction_limit_remaining.take() {
                    self.bx.last_limit_exit_remaining = rem;
                }
                value
            }
        })
    }

    pub fn run_core(&mut self) -> ScriptValue {
        // Cached pointer into the active body's opcode buffer, so the loop
        // pays no RefCell borrow per instruction. It is only trusted while
        // `bodies.epoch()` is unchanged: see `ScriptBodies`.
        let mut cached_body_index: usize = usize::MAX;
        let mut cached_epoch: u64 = 0;
        let mut opcodes_ptr: *const ScriptValue = std::ptr::null();
        let mut opcodes_len: usize = 0;

        // A limit signal raised before this run started (a rejected frame, a
        // stale flag) fails it before the first instruction.
        if self.bx.threads.cur_ref().has_execution_limit_exceeded() {
            if let Some(value) = self.handle_execution_limit_failure() {
                return value;
            }
        }

        loop {
            // Heap growth paths cannot own the interpreter trap (many are
            // shared with parsers/native bindings), so they record one hard
            // refusal on the heap. Turn it into the same uncatchable Bail as
            // the instruction ceiling before executing another opcode.
            if let Some(message) = self.bx.heap.take_allocation_error() {
                let err = script_err_limit!(
                    self.bx.threads.cur_ref().trap,
                    "{}",
                    message
                );
                self.drain_errors();
                self.bx
                    .threads
                    .cur()
                    .trap
                    .set_on(Some(ScriptTrapOn::Bail(err)));
                if let Some(value) = self.handle_trap_on() {
                    return value;
                }
            }
            let instruction_limit_exceeded = if let Some(remaining) =
                self.bx.threads.cur().instruction_limit_remaining.as_mut()
            {
                if *remaining == 0 {
                    true
                } else {
                    *remaining -= 1;
                    false
                }
            } else {
                false
            };
            if instruction_limit_exceeded {
                let err = script_err_limit!(
                    self.bx.threads.cur_ref().trap,
                    "script instruction limit exceeded"
                );
                // drain_errors routes to the captured-error sink, the log, or
                // the void depending on host configuration.
                self.drain_errors();
                self.bx
                    .threads
                    .cur()
                    .trap
                    .set_on(Some(ScriptTrapOn::Bail(err)));
                if let Some(value) = self.handle_trap_on() {
                    return value;
                }
            }

            if let Some(hit) = self.check_run_budget() {
                match hit {
                    ScriptRunBudgetHit::Soft => {
                        self.bx.threads.cur().is_paused = true;
                        self.bx
                            .threads
                            .cur()
                            .trap
                            .set_on(Some(ScriptTrapOn::TimeBudgetYield));
                    }
                    ScriptRunBudgetHit::Hard => {
                        let err = script_err_limit!(
                            self.bx.threads.cur().trap.pass(),
                            "script time budget exceeded"
                        );
                        // A hard budget hit is an uncatchable VM bail. Move its
                        // diagnostic out of the thread queue before unwinding so
                        // a later run cannot observe it as a script error.
                        self.drain_errors();
                        self.bx
                            .threads
                            .cur()
                            .trap
                            .set_on(Some(ScriptTrapOn::Bail(err)));
                    }
                }
                if let Some(value) = self.handle_trap_on() {
                    return value;
                }
            }

            let thread = self.bx.threads.cur();
            let body_index = thread.trap.ip.body as usize;
            let ip_index = thread.trap.ip.index as usize;

            // Re-derive the pointer when the body changes or when anything
            // mutated the bodies since it was taken: a native call may have
            // re-entered the VM and reloaded the very body that is running.
            let epoch = self.bx.code.bodies.epoch();
            if body_index != cached_body_index || epoch != cached_epoch {
                let bodies = self.bx.code.bodies.borrow();
                let opcodes = &bodies[body_index].parser.opcodes;
                opcodes_ptr = opcodes.as_ptr();
                opcodes_len = opcodes.len();
                cached_body_index = body_index;
                cached_epoch = epoch;
            }

            if ip_index >= opcodes_len {
                // If there's a value on the stack, return it (for expression-style scripts)
                let stack_len = self.bx.threads.cur().stack.len();
                if stack_len > 0 {
                    return self.bx.threads.cur().pop_stack_value();
                }
                return NIL;
            }

            // SAFETY: the buffer can only move or be freed through
            // `bodies.borrow_mut()`, which advances the epoch checked above.
            let opcode = unsafe { *opcodes_ptr.add(ip_index) };

            if self.bx.debug_trace {
                let stack_len = self.bx.threads.cur_ref().stack.len();
                if let Some((op, a)) = opcode.as_opcode() {
                    eprintln!("TRACE b{body_index} ip{ip_index} stack{stack_len} {op:?} {a:?}");
                } else {
                    eprintln!("TRACE b{body_index} ip{ip_index} stack{stack_len} PUSH {opcode:?}");
                }
            }

            if let Some((opcode, args)) = opcode.as_opcode() {
                self.opcode(opcode, args);
                if self.bx.threads.cur_ref().has_execution_limit_exceeded() {
                    if let Some(value) = self.handle_execution_limit_failure() {
                        return value;
                    }
                }
                // single-load poll for both interrupt sources (errors + traps)
                let pending = self.bx.threads.cur().trap.pending();
                if pending != 0 {
                    if pending & crate::trap::TRAP_PENDING_ERR != 0 {
                        self.handle_errors();
                    }
                    if let Some(value) = self.handle_trap_on() {
                        return value;
                    }
                }
            } else {
                // its a direct value-to-stack
                if let Some(trace) = &mut self.bx.literal_trace {
                    if trace.ran_seen.first(body_index as u16, ip_index as u32) {
                        trace.ran.insert((body_index as u16, ip_index as u32));
                    }
                }
                self.bx.threads.cur().push_stack_value(opcode);
                self.bx.threads.cur().trap.goto_next();
                if self.bx.threads.cur_ref().has_execution_limit_exceeded() {
                    if let Some(value) = self.handle_execution_limit_failure() {
                        return value;
                    }
                }
            }
        }
    }

    pub fn run_root(&mut self, body_id: u16) -> ScriptValue {
        // Extract values from bodies before modifying thread state
        let (scope, me) = {
            let bodies = self.bx.code.bodies.borrow();
            (
                bodies[body_id as usize].scope.obj,
                bodies[body_id as usize].me.obj,
            )
        };

        let root_slots = self.bx.threads.cur_ref().slots.len();
        let root_slot_base = self.bx.threads.cur_ref().slot_base;
        if !self.bx.threads.cur().push_call_frame(CallFrame {
            bases: StackBases {
                tries: 0,
                loops: 0,
                stack: 0,
                scope: 0,
                mes: 0,
                // unlike the other zeroed bases, never truncate slots below
                // what an enclosing (paused/re-entrant) frame allocated
                slots: root_slots,
            },
            args: Default::default(),
            return_ip: None,
            prev_slot_base: root_slot_base,
            body_root: true,
        }) {
            return self
                .handle_execution_limit_failure()
                .expect("a rejected root frame raises a limit failure");
        }

        self.bx.threads.cur().scopes.push(scope);
        self.bx.threads.cur().mes.push(ScriptMe::Object(me));

        self.bx.threads.cur().trap.ip.body = body_id;
        self.bx.threads.cur().trap.ip.index = 0;

        // the main interpreter loop
        self.drain_stale_errors();
        let value = self.run_core();
        if let Some(end) = self.bx.threads.cur().root_end_scope.take() {
            let mut bodies = self.bx.code.bodies.borrow_mut();
            let body = &mut bodies[body_id as usize];
            body.end_scope = Some(if body.transient { ScriptObjectRef::unrooted(end.as_object()) } else { end });
        }
        value
    }

    /// Checks if the value has an apply transform and calls it, returning the transformed value.
    /// Returns None if no transform exists, Some(transformed) if a transform was applied.
    #[inline(always)]
    pub fn call_apply_transform(&mut self, value: ScriptValue) -> Option<ScriptValue> {
        let mut req = ScriptCallReq::new(CALL_TRANSFORM, value);
        let result = self.vm_call(&mut req);
        if req.found { Some(result) } else { None }
    }

    fn call_apply_transform_impl(&mut self, value: ScriptValue) -> Option<ScriptValue> {
        if let Some(obj) = value.as_object() {
            if let Some(ni) = self.bx.heap.objects[obj].tag.as_apply_transform() {
                let func = self.bx.code.native.borrow().functions[ni.index as usize].view();
                // Pause thread before native call so re-entrant calls get a different thread
                self.bx.threads.cur().is_paused = true;
                let result = unsafe { func.call(self, obj) };
                // Only unpause if native didn't explicitly pause
                if !matches!(
                    self.bx.threads.cur().trap.get_on(),
                    Some(ScriptTrapOn::Pause)
                ) {
                    self.bx.threads.cur().is_paused = false;
                }
                return Some(result);
            }
        } else if let Some(arr) = value.as_array() {
            if let Some(ni) = self.bx.heap.arrays[arr].tag.as_apply_transform() {
                // For arrays, we need to create a temporary args object
                let args_obj = self.bx.heap.new_object();
                self.bx
                    .heap
                    .set_value_def(args_obj, id!(self).into(), value);
                let func = self.bx.code.native.borrow().functions[ni.index as usize].view();
                // Pause thread before native call so re-entrant calls get a different thread
                self.bx.threads.cur().is_paused = true;
                let result = unsafe { func.call(self, args_obj) };
                // Only unpause if native didn't explicitly pause
                if !matches!(
                    self.bx.threads.cur().trap.get_on(),
                    Some(ScriptTrapOn::Pause)
                ) {
                    self.bx.threads.cur().is_paused = false;
                }
                return Some(result);
            }
        }
        None
    }

    #[inline(always)]
    pub fn resume(&mut self) -> ScriptValue {
        self.vm_call(&mut ScriptCallReq::new(CALL_RESUME, NIL))
    }

    pub fn cast_to_f64(&self, v: ScriptValue) -> f64 {
        self.bx
            .heap
            .cast_to_f64(v, self.bx.threads.cur_ref().trap.ip)
    }

    #[inline(always)]
    pub fn handle_type(&self, id: LiveId) -> ScriptHandleType {
        ScriptHandleType(self.bx.vm_stat(&mut ScriptCtlReq::with(STAT_HANDLE_TYPE, id.0)) as u8)
    }

    #[inline(always)]
    pub fn new_handle_type(&mut self, id: LiveId) -> ScriptHandleType {
        let mut req = ScriptRegReq::new(REG_HANDLE_TYPE);
        req.id = id;
        ScriptHandleType(self.bx.vm_register(&req) as u8)
    }

    pub fn downcast_handle_gc<T: ScriptHandleGc + 'static>(
        &self,
        handle: ScriptHandle,
    ) -> Option<&T> {
        self.heap().handle_ref::<T>(handle)
    }

    #[inline(always)]
    pub fn add_handle_method<F>(
        &mut self,
        ht: ScriptHandleType,
        method: LiveId,
        args: &[(LiveId, ScriptValue)],
        f: F,
    ) where
        F: Fn(&mut ScriptVm, ScriptObject) -> ScriptValue + 'static,
    {
        self.add_handle_method_native(ht, method, args, NativeFn::new(f))
    }

    #[inline(always)]
    pub fn add_handle_method_native(&mut self, ht: ScriptHandleType, method: LiveId, args: &[(LiveId, ScriptValue)], f: NativeFn) {
        let mut req = ScriptRegReq::new(REG_HANDLE_METHOD);
        req.handle_type = ht;
        req.id = method;
        req.args = args;
        req.native = f.into_raw();
        self.bx.vm_register(&req);
    }

    #[inline(always)]
    pub fn set_handle_setter<F>(&mut self, ht: ScriptHandleType, f: F)
    where
        F: Fn(&mut ScriptVm, ScriptValue, LiveId, ScriptValue) -> ScriptValue + 'static,
    {
        self.set_handle_setter_native(ht, NativeSetterFn::new(f))
    }

    #[inline(always)]
    pub fn set_handle_setter_native(&mut self, ht: ScriptHandleType, f: NativeSetterFn) {
        let mut req = ScriptRegReq::new(REG_HANDLE_SETTER);
        req.handle_type = ht;
        req.native = f.into_raw();
        self.bx.vm_register(&req);
    }

    #[inline(always)]
    pub fn set_handle_getter<F>(&mut self, ht: ScriptHandleType, f: F)
    where
        F: Fn(&mut ScriptVm, ScriptValue, LiveId) -> ScriptValue + 'static,
    {
        self.set_handle_getter_native(ht, NativeGetterFn::new(f))
    }

    #[inline(always)]
    pub fn set_handle_getter_native(&mut self, ht: ScriptHandleType, f: NativeGetterFn) {
        let mut req = ScriptRegReq::new(REG_HANDLE_GETTER);
        req.handle_type = ht;
        req.native = f.into_raw();
        self.bx.vm_register(&req);
    }

    /// Register a catch-all method dispatcher for a handle type.
    /// When a method call is made on a handle that has no specific method
    /// registered for that name, this call function is invoked with
    /// (vm, args_object, method). The args object has `self` set
    /// and all call arguments collected, just like a normal native method.
    #[inline(always)]
    pub fn set_handle_call<F>(&mut self, ht: ScriptHandleType, f: F)
    where
        F: Fn(&mut ScriptVm, ScriptObject, LiveId) -> ScriptValue + 'static,
    {
        self.set_handle_call_native(ht, NativeCallFn::new(f))
    }

    #[inline(always)]
    pub fn set_handle_call_native(&mut self, ht: ScriptHandleType, f: NativeCallFn) {
        let mut req = ScriptRegReq::new(REG_HANDLE_CALL);
        req.handle_type = ht;
        req.native = f.into_raw();
        self.bx.vm_register(&req);
    }

    #[inline(always)]
    pub fn new_module(&mut self, id: LiveId) -> ScriptObject {
        self.heap_mut().new_module(id)
    }

    #[inline(always)]
    pub fn module(&mut self, id: LiveId) -> ScriptObject {
        let m = self.heap_mut().module(id);
        if m == ScriptObject::ZERO && self.expand_lazy_pending() {
            return self.heap_mut().module(id);
        }
        m
    }



    /// `f(vm, elements)` over `object`'s vector part, taken out of the object while `f` runs
    /// (as before; the object reads as empty meanwhile).
    #[inline(always)]
    pub fn vec_with<R, F: FnOnce(&mut Self, &[ScriptVecValue]) -> R>(
        &mut self,
        object: ScriptObject,
        f: F,
    ) -> R {
        let taken = self.vec_take(object);
        let (ptr, len) = (taken.as_ptr(), taken.len());
        // SAFETY: the taken vector stays in the VM's stash, unchanged, until vec_restore
        let r = f(self, unsafe { std::slice::from_raw_parts(ptr, len) });
        self.vec_restore(object);
        r
    }

    /// Moves `object`'s vector part into the VM's stash (vec_restore puts it back) and returns
    /// its elements.
    #[inline(always)]
    pub fn vec_take(&mut self, object: ScriptObject) -> &[ScriptVecValue] {
        let mut req = ScriptCtlReq::new(CTL_VEC_TAKE);
        req.value = object.into();
        self.bx.vm_ctl(&mut req);
        // SAFETY: the taken vector stays in the VM's stash, unchanged, until vec_restore
        unsafe { std::slice::from_raw_parts(req.a as *const ScriptVecValue, req.b as usize) }
    }

    /// Puts back the vector part [`Self::vec_take`] took from `object`.
    #[inline(always)]
    pub fn vec_restore(&mut self, object: ScriptObject) {
        let mut req = ScriptCtlReq::new(CTL_VEC_RESTORE);
        req.value = object.into();
        self.bx.vm_ctl(&mut req);
    }

    pub fn vec_mut_with<R, F: FnOnce(&mut Self, &mut Vec<ScriptVecValue>) -> R>(
        &mut self,
        object: ScriptObject,
        f: F,
    ) -> R {
        let mut vec = Vec::new();
        std::mem::swap(&mut vec, &mut self.bx.heap.objects[object].vec);
        let r = f(self, &mut vec);
        std::mem::swap(&mut vec, &mut self.bx.heap.objects[object].vec);
        r
    }

    /// `f(vm, text)` for a string value (heap or inline); the text stays valid while `f` runs
    /// even when `f` changes the heap (the string is pinned).
    #[inline(always)]
    pub fn string_with<R, F: FnOnce(&mut Self, &str) -> R>(
        &mut self,
        value: ScriptValue,
        f: F,
    ) -> Option<R> {
        if value.as_string().is_some() {
            let pin = self.string_pin(value)?;
            // SAFETY: the pinned string lives until string_unpin
            let r = f(self, unsafe { pin.as_str() });
            self.string_unpin(pin);
            return Some(r);
        }
        value.as_inline_string(|s| f(self, s))
    }

    /// Keeps heap string `value` alive until [`Self::string_unpin`] and returns its text.
    #[inline(always)]
    pub fn string_pin(&mut self, value: ScriptValue) -> Option<ScriptStrPin> {
        let mut req = ScriptCtlReq::new(CTL_STRING_PIN);
        req.value = value;
        let depth = self.bx.vm_ctl(&mut req);
        if depth == u64::MAX { None } else { Some(ScriptStrPin { ptr: req.text.as_ptr(), len: req.text.len(), depth: depth as u32 }) }
    }

    /// Releases a [`Self::string_pin`] (and any pin a panic left above it).
    #[inline(always)]
    pub fn string_unpin(&mut self, pin: ScriptStrPin) {
        self.bx.vm_ctl(&mut ScriptCtlReq::with(CTL_STRING_UNPIN, pin.depth as u64));
    }

    #[inline(always)]
    pub fn new_string_with<F: FnOnce(&mut Self, &mut String)>(&mut self, f: F) -> ScriptValue {
        // the text is built in the caller's own String (it may be another compiler's: the VM
        // is the rustc-built part of the makepad loader), then copied into the heap
        if !self.native_string_preflight() {
            return NIL;
        }
        let mut out = String::new();
        f(self, &mut out);
        self.store_native_string(&out)
    }

    /// Whether a host native may build a string now: under a strict allocation budget (one
    /// that does not charge native strings when stored) it is refused and charged as
    /// unbounded.
    #[inline(always)]
    pub fn native_string_preflight(&mut self) -> bool {
        self.bx.vm_ctl(&mut ScriptCtlReq::new(CTL_NATIVE_STRING_PREFLIGHT)) != 0
    }

    /// Stores a host native's text (charged on its real length under a budget).
    #[inline(always)]
    pub fn store_native_string(&mut self, text: &str) -> ScriptValue {
        let mut req = ScriptCtlReq::new(CTL_STORE_NATIVE_STRING);
        req.text = text;
        self.bx.vm_ctl(&mut req);
        req.value
    }

    /// Only boxes the closure; the registration is the non-generic
    /// `ScriptNative::add_method_boxed`.
    #[inline(always)]
    pub fn add_method<F>(
        &mut self,
        module: ScriptObject,
        method: LiveId,
        args: &[(LiveId, ScriptValue)],
        f: F,
    ) where
        F: Fn(&mut ScriptVm, ScriptObject) -> ScriptValue + 'static,
    {
        self.add_method_native(module, method, args, NativeFn::new(f))
    }

    /// [`Self::add_method`] with the closure already a [`NativeFn`].
    #[inline(always)]
    pub fn add_method_native(&mut self, module: ScriptObject, method: LiveId, args: &[(LiveId, ScriptValue)], f: NativeFn) {
        let mut req = ScriptRegReq::new(REG_METHOD);
        req.module = module;
        req.id = method;
        req.args = args;
        req.native = f.into_raw();
        self.bx.vm_register(&req);
    }

    #[inline(always)]
    pub fn set_injected_global(&mut self, key: LiveId, value: ScriptValue) {
        let mut req = ScriptRegReq::new(REG_INJECTED_GLOBAL);
        req.id = key;
        req.value = value;
        self.bx.vm_register(&req);
    }

    /// Registers a native function to be used as an apply_transform and returns its NativeId.
    /// This is used for creating objects that transform to a computed value when applied.
    #[inline(always)]
    pub fn add_apply_transform_fn<F>(&mut self, f: F) -> NativeId
    where
        F: Fn(&mut ScriptVm, ScriptObject) -> ScriptValue + 'static,
    {
        self.add_apply_transform_native(NativeFn::new(f))
    }

    #[inline(always)]
    pub fn add_apply_transform_native(&mut self, f: NativeFn) -> NativeId {
        let mut req = ScriptRegReq::new(REG_APPLY_TRANSFORM);
        req.native = f.into_raw();
        NativeId { index: self.bx.vm_register(&req) as u32 }
    }

    #[inline(always)]
    pub fn add_script_mod(&mut self, new_mod: ScriptMod) -> u16 {
        self.add_script_mod_view(&new_mod.view())
    }

    #[inline(always)]
    pub fn add_script_mod_view(&mut self, new_mod: &ScriptModView) -> u16 {
        let mut req = ScriptEvalReq::new(EVAL_ADD, *new_mod);
        self.vm_eval(&mut req);
        req.body
    }

    fn add_script_mod_with(&mut self, new_mod: ScriptMod, transient: bool) -> u16 {
        // Register this crate's manifest path for crate path resolution
        let crate_name = new_mod.module_path.split("::").next().unwrap_or("");
        if !crate_name.is_empty() {
            self.bx.code.crate_manifests.borrow_mut().insert(
                crate_name.replace('-', "_"),
                new_mod.cargo_manifest_path.clone(),
            );
        }

        let scope_obj = self.bx.heap.new_with_proto(id!(scope).into());
        self.bx.heap.set_object_deep(scope_obj);
        self.bx
            .heap
            .set_value_def(scope_obj, id!(mod).into(), self.bx.heap.modules.into());
        self.bx.apply_injected_globals_to_scope(scope_obj);
        let me_obj = self.bx.heap.new_with_proto(id!(root_me).into());
        // A transient body's scope and `me` are kept by the body only while
        // it is alive (see `ScriptHeap::mark_transient_bodies`), not as roots.
        let (scope, me) = if transient {
            self.bx.heap.set_reffed(scope_obj);
            self.bx.heap.set_reffed(me_obj);
            (ScriptObjectRef::unrooted(scope_obj), ScriptObjectRef::unrooted(me_obj))
        } else {
            (self.bx.heap.new_object_ref(scope_obj), self.bx.heap.new_object_ref(me_obj))
        };
        let key = ScriptModKey::from_script_mod(&new_mod);
        let override_code = self
            .bx
            .code
            .script_mod_overrides
            .borrow()
            .get(&key)
            .cloned();
        let effective_code = override_code
            .clone()
            .unwrap_or_else(|| new_mod.code.clone());

        let new_body = ScriptBody {
            source: ScriptSource::Mod(new_mod),
            effective_code,
            tokenizer: ScriptTokenizer::default(),
            parser: ScriptParser::keeping_numbers(self.bx.keep_numbers),
            scope,
            me,
            end_scope: None,
            checkpoint: None,
            source_len: 0,
            content_hash: 0,
            transient,
            real_file: String::new(),
            shown: None,
        };
        let mut bodies = self.bx.code.bodies.borrow_mut();
        for (i, body) in bodies.iter_mut().enumerate() {
            if let ScriptSource::Mod(script_mod) = &body.source {
                if let ScriptSource::Mod(new_mod) = &new_body.source {
                    if script_mod.file == new_mod.file
                        && script_mod.line == new_mod.line
                        && script_mod.column == new_mod.column
                    {
                        let values_changed = script_mod.values != new_mod.values;
                        body.source = new_body.source;
                        body.scope = new_body.scope;
                        body.me = new_body.me;
                        body.transient = transient;
                        body.real_file = String::new();
                        body.shown = None;
                        if body.effective_code != new_body.effective_code || values_changed {
                            body.effective_code = new_body.effective_code;
                            body.tokenizer = ScriptTokenizer::default();
                            body.parser = ScriptParser::keeping_numbers(self.bx.keep_numbers);
                            body.checkpoint = None;
                            body.source_len = 0;
                        }
                        return i as u16;
                    }
                }
            }
        }
        if let Some(i) = self.bx.code.free_bodies.borrow_mut().pop() {
            bodies[i as usize] = new_body;
            return i;
        }
        let i = bodies.len();
        // A body id past the packing would alias another body's functions
        // (see `ScriptIp::BODY_BITS`): stop here rather than run the wrong code.
        assert!(i < ScriptIp::MAX_BODIES, "script body limit reached: {} bodies fit in ScriptIp", ScriptIp::MAX_BODIES);
        bodies.push(new_body);
        i as u16
    }

    #[inline(always)]
    pub fn eval(&mut self, script_mod: ScriptMod) -> ScriptValue {
        self.eval_with_source_view(&script_mod.view(), ScriptObject::ZERO)
    }

    /// [`Self::eval`] for source made at runtime (a generated shader, a
    /// document, a UI built from data): the body is reclaimed by the GC once
    /// nothing made from it is alive, and its slot reused, so a host can
    /// evaluate new source for as long as it runs. Name the module after its
    /// content (`file`): the same text then reuses its body, and a body is
    /// never re-evaluated with other text while what it made still lives.
    #[inline(always)]
    pub fn eval_transient(&mut self, script_mod: ScriptMod) -> ScriptValue {
        self.eval_transient_view(&script_mod.view())
    }

    #[inline(always)]
    pub fn eval_transient_view(&mut self, script_mod: &ScriptModView) -> ScriptValue {
        self.vm_eval(&mut ScriptEvalReq::new(EVAL_TRANSIENT, *script_mod))
    }

    /// [`Self::eval`] with `parent` behind the module's own scope: a name the
    /// module does not define is read from `parent` and its scopes (another
    /// body's [`Self::end_scope_of`]), so code compiled later reads a loaded
    /// document's bindings as if written in it. A host uses it to compile
    /// functions over a document after it has run.
    #[inline(always)]
    pub fn eval_in_scope(&mut self, script_mod: ScriptMod, parent: ScriptObject) -> ScriptValue {
        self.eval_in_scope_view(&script_mod.view(), parent)
    }

    #[inline(always)]
    pub fn eval_in_scope_view(&mut self, script_mod: &ScriptModView, parent: ScriptObject) -> ScriptValue {
        let mut req = ScriptEvalReq::new(EVAL_IN_SCOPE, *script_mod);
        req.scope = parent;
        self.vm_eval(&mut req)
    }

    /// [`Self::eval`] for a module that is a file people read (a library):
    /// `script_mod.file` names the file in messages and its first line is
    /// numbered `script_mod.line`; the names of each object in `uses` are in
    /// its scope before it runs, as `use <object>.*` lines at the top of its
    /// text would put them there, without the lines. Each distinct text
    /// is a body of its own (functions of an earlier text keep their code).
    /// Returns the value and the body, for [`Self::scope_exports`].
    #[inline(always)]
    pub fn eval_module(&mut self, script_mod: ScriptMod, uses: &[ScriptObject]) -> (ScriptValue, u16) {
        let mut req = ScriptEvalReq::new(EVAL_MODULE, script_mod.view());
        req.uses = uses;
        let value = self.vm_eval(&mut req);
        (value, req.body)
    }

    fn eval_module_impl(&mut self, mut script_mod: ScriptMod, uses: &[ScriptObject]) -> (ScriptValue, u16) {
        let shown = std::mem::take(&mut script_mod.file);
        script_mod.line = script_mod.line.max(1);
        script_mod.file = format!("{shown}#{:016x}", ScriptBody::content_hash_of(&script_mod.code, &script_mod.values));
        // Transient: a text that is edited again and again must not pile up bodies; the
        // GC reclaims a body once nothing made from it (the exported fns) lives.
        let body_id = self.add_script_mod_with(script_mod, true);
        self.bx.set_body_shown(body_id, &shown);
        for from in uses {
            self.bx.body_use_all(body_id, *from);
        }
        (self.eval_body(body_id, ScriptObject::ZERO), body_id)
    }

    /// Name `body`'s file in its messages and count its first line from
    /// `ScriptMod::line` (see [`ScriptBody::shown`]).
    #[inline(always)]
    pub fn set_body_shown(&mut self, body: u16, file: &str) {
        let mut req = ScriptBodyReq::new(BODY_SET_SHOWN, body);
        req.text = file;
        self.bx.vm_body(&mut req);
    }

    /// Bring every name of `from` into the scope of `body` (a module made
    /// by [`Self::add_script_mod`] that has not run yet), as
    /// `use <from>.*` does. A name already there is replaced; a module's
    /// private keys (`__forms`, `__tag`) stay behind.
    #[inline(always)]
    pub fn body_use_all(&mut self, body: u16, from: ScriptObject) {
        let mut req = ScriptBodyReq::new(BODY_USE_ALL, body);
        req.scope = from;
        self.bx.vm_body(&mut req);
    }

    /// The names a module defined at its top level (its `let`s and `fn`s) as
    /// an object, once `body` has run: what a library exports without
    /// writing an export list. Names that arrived by `use` (the entries of
    /// `uses`, the VM's injected globals, `mod`) are not part of it.
    #[inline(always)]
    pub fn scope_exports(&mut self, body: u16, uses: &[ScriptObject]) -> Option<ScriptObject> {
        let mut req = ScriptBodyReq::new(BODY_EXPORTS, body);
        req.uses = uses;
        if self.bx.vm_body(&mut req) { req.value.as_object() } else { None }
    }

    /// Compile shader fn `f` in a namespace (see [`ShaderFnNamespace`]):
    /// `renames` (`self.<from>` read as `self.<to>`) and an optional input.
    /// Empty renames and no input compile it as written again.
    #[inline(always)]
    pub fn set_shader_fn_namespace(&mut self, f: ScriptObject, renames: &[ShaderFnRename], input: Option<ShaderFnInput>) {
        let mut req = ScriptRegReq::new(REG_SHADER_FN_NAMESPACE);
        req.module = f;
        req.renames = renames;
        req.input = input.as_ref().map_or(std::ptr::null(), |input| input as *const ShaderFnInput);
        self.bx.vm_register(&req);
    }

    /// A fn of this VM made from `image` (another VM's fn, see
    /// [`ScriptCode::fn_image`]): its tokens become a body of this VM, read
    /// as they were, and the fn it evaluates to has `parent` behind its own
    /// scope (the names it reads that it does not bind). The body is named
    /// `adopted://<hash>` (the same fn adopted again reuses it) and reports
    /// the fn's own file, line and column in errors and live literals.
    /// None when the tokens do not evaluate to a fn.
    #[inline(always)]
    pub fn adopt_fn(&mut self, image: &ScriptFnImage, parent: ScriptObject) -> Option<ScriptObject> {
        let mut req = ScriptEvalReq::new(EVAL_ADOPT, ScriptModView::empty());
        req.image = image;
        req.scope = parent;
        self.vm_eval(&mut req).as_object()
    }

    fn adopt_fn_impl(&mut self, image: &ScriptFnImage, parent: ScriptObject) -> Option<ScriptObject> {
        let d = image.data()?;
        let file = format!("adopted://{:016x}", d.hash);
        let script_mod = ScriptMod { cargo_manifest_path: String::new(), module_path: String::new(), file: file.clone(), line: d.line as usize, column: d.col as usize, code: d.text.clone(), values: Vec::new() };
        let body_id = self.add_script_mod_with(script_mod, false);
        {
            let mut bodies = self.bx.code.bodies.borrow_mut();
            let body = &mut bodies[body_id as usize];
            body.real_file = d.file.clone();
            if body.source_len == 0 {
                let mut tokens = d.tokens.clone();
                for (k, text) in &d.strings {
                    tokens[*k].token = ScriptToken::String(self.bx.heap.new_string_from_str(text));
                }
                body.tokenizer = ScriptTokenizer::adopted(&d.text, tokens);
                body.parser = ScriptParser::keeping_numbers(self.bx.keep_numbers);
                body.parser.parse(&body.tokenizer, &file, (d.line as usize, d.col as usize), &[]);
                body.source_len = d.text.len();
                body.content_hash = ScriptBody::content_hash_of(&d.text, &[]);
            }
        }
        let scope = self.bx.code.bodies.borrow()[body_id as usize].scope.obj;
        self.bx.heap.objects[scope].proto = parent.into();
        let v = self.eval_body(body_id, ScriptObject::ZERO);
        v.as_object().filter(|o| self.bx.heap.as_fn(*o).is_some())
    }

    /// The scope the last body evaluated from `file` ended in (its top-level
    /// `let`s, `use`d names and the scopes they shadow), once it has run.
    #[inline(always)]
    pub fn end_scope_of(&self, file: &str) -> Option<ScriptObject> {
        let answer = self.bx.code_ref().query(QUERY_END_SCOPE_OF, &mut ScriptQuerySink::new(), |req| req.text = file);
        if answer.found { answer.value.as_object() } else { None }
    }

    #[inline(always)]
    pub fn eval_with_source(&mut self, script_mod: ScriptMod, source: ScriptObject) -> ScriptValue {
        self.eval_with_source_view(&script_mod.view(), source)
    }

    #[inline(always)]
    pub fn eval_with_source_view(&mut self, script_mod: &ScriptModView, source: ScriptObject) -> ScriptValue {
        let mut req = ScriptEvalReq::new(EVAL_SOURCE, *script_mod);
        req.scope = source;
        self.vm_eval(&mut req)
    }

    /// The one entry into evaluating source (the op the `eval_*` methods are inline
    /// decoders of): `req.mode` picks what to do with the module, see [`ScriptEvalReq`].
    #[relax_api]
    pub fn vm_eval(&mut self, req: &mut ScriptEvalReq) -> ScriptValue {
        match req.mode {
            EVAL_ADD => {
                req.body = self.add_script_mod_with(req.module.to_owned(), false);
                NIL
            }
            EVAL_SOURCE => {
                req.body = self.add_script_mod_with(req.module.to_owned(), false);
                self.eval_body(req.body, req.scope)
            }
            EVAL_TRANSIENT => {
                req.body = self.add_script_mod_with(req.module.to_owned(), true);
                self.eval_body(req.body, ScriptObject::ZERO)
            }
            EVAL_IN_SCOPE => {
                req.body = self.add_script_mod_with(req.module.to_owned(), false);
                let scope = self.bx.code.bodies.borrow()[req.body as usize].scope.obj;
                self.bx.heap.objects[scope].proto = req.scope.into();
                self.eval_body(req.body, ScriptObject::ZERO)
            }
            EVAL_MODULE => {
                let (value, body) = self.eval_module_impl(req.module.to_owned(), req.uses);
                req.body = body;
                value
            }
            EVAL_APPEND => self.eval_with_append_source_impl(req.module.to_owned(), req.code, req.scope),
            EVAL_ADOPT => {
                // SAFETY: the request's image points at the caller's fn image for this call
                let image = unsafe { &*req.image };
                match self.adopt_fn_impl(image, req.scope) {
                    Some(obj) => obj.into(),
                    None => NIL,
                }
            }
            mode => panic!("vm_eval: unknown mode {mode}"),
        }
    }

    fn eval_body(&mut self, body_id: u16, source: ScriptObject) -> ScriptValue {
        let _inspect_eval = crate::inspect_registry::eval_guard(&self.bx);

        // Set __script_source__ on the scope if source is provided
        // If source has FROM_EVAL flag, use its prototype instead
        if source != ScriptObject::ZERO {
            let actual_source = if self.bx.heap.is_from_eval(source) {
                // Use the prototype of the FROM_EVAL object
                if let Some(proto) = self.bx.heap.proto(source).as_object() {
                    proto
                } else {
                    source
                }
            } else {
                source
            };
            let scope = self.bx.code.bodies.borrow()[body_id as usize].scope.obj;
            self.bx
                .heap
                .set_value_def(scope, id!(__script_source__).into(), actual_source.into());
        }

        let mut bodies = self.bx.code.bodies.borrow_mut();
        let body = &mut bodies[body_id as usize];

        if let ScriptSource::Mod(script_mod) = &body.source {
            if body.source_len == 0 {
                body.tokenizer.clear();
                body.parser = ScriptParser::keeping_numbers(self.bx.keep_numbers);
                body.tokenizer
                    .tokenize(&body.effective_code, &mut self.bx.heap);
                body.tokenizer.finish(&mut self.bx.heap);
                let (file, offsets) = body.parse_site(script_mod);
                body.parser.parse(&body.tokenizer, &file, offsets, &script_mod.values);
                body.source_len = body.effective_code.len();
                body.content_hash = ScriptBody::content_hash_of(&body.effective_code, &script_mod.values);
            }
            // Parse errors never enter the trap queue (the parser recovers);
            // surface them to a captured-diagnostics sink here or a validating
            // host reports success for a script that failed to parse.
            // (A file's errors stay with its body: the same text evaluated again reports them again.)
            let parse_errors = if body.shown.is_some() { body.parser.parse_errors.clone() } else { std::mem::take(&mut body.parser.parse_errors) };
            let parse_records: Vec<ScriptErrorRecord> = parse_errors.into_iter().enumerate()
                .map(|(i, text)| ScriptErrorRecord::parse(text, body.parser.diagnostics.get(i), &body.parser.file)).collect();
            drop(bodies);
            if let Some(sink) = self.bx.captured_errors.as_mut() {
                sink.extend(parse_records);
            }
            // lets point our thread to it
            let result = self.run_root(body_id);
            // Mark the result object with FROM_EVAL flag
            if let Some(result_obj) = result.as_object() {
                self.bx.heap.set_from_eval(result_obj);
            }

            result
        } else {
            NIL
        }
    }

    /// Evaluate script incrementally by appending new source to an existing body.
    ///
    /// Pass the full growing source code string each time. On first call, creates
    /// the body and tokenizes/parses everything. On subsequent calls, computes the
    /// delta (new chars since last call), restores the parser checkpoint (removing
    /// auto-close opcodes), tokenizes only the new chars, continues parsing, then
    /// auto-closes again for execution. Always re-executes from opcode 0.
    #[inline(always)]
    pub fn eval_with_append_source(&mut self, script_mod: ScriptMod, code: &str, source: ScriptObject) -> ScriptValue {
        let mut req = ScriptEvalReq::new(EVAL_APPEND, script_mod.view());
        req.code = code;
        req.scope = source;
        self.vm_eval(&mut req)
    }

    fn eval_with_append_source_impl(
        &mut self,
        script_mod: ScriptMod,
        code: &str,
        source: ScriptObject,
    ) -> ScriptValue {
        // Look for an existing body with matching file/line/column
        let existing_body_id = {
            let bodies = self.bx.code.bodies.borrow();
            let mut found = None;
            for (i, body) in bodies.iter().enumerate() {
                if let ScriptSource::Mod(existing_mod) = &body.source {
                    if existing_mod.file == script_mod.file
                        && existing_mod.line == script_mod.line
                        && existing_mod.column == script_mod.column
                    {
                        found = Some(i as u16);
                        break;
                    }
                }
            }
            found
        };

        let body_id = match existing_body_id {
            Some(id) => id,
            None => self.add_script_mod_with(script_mod, false),
        };

        // Set __script_source__ on the scope if source is provided
        if source != ScriptObject::ZERO {
            let actual_source = if self.bx.heap.is_from_eval(source) {
                if let Some(proto) = self.bx.heap.proto(source).as_object() {
                    proto
                } else {
                    source
                }
            } else {
                source
            };
            let scope = self.bx.code.bodies.borrow()[body_id as usize].scope.obj;
            self.bx
                .heap
                .set_value_def(scope, id!(__script_source__).into(), actual_source.into());
        }

        let mut bodies = self.bx.code.bodies.borrow_mut();
        let body = &mut bodies[body_id as usize];

        if let ScriptSource::Mod(existing_mod) = &body.source {
            // Restore checkpoint (removes auto-close opcodes from previous run)
            if let Some(cp) = body.checkpoint.take() {
                body.parser.restore_checkpoint(cp);
            }

            let prev_len = body.source_len;
            // Check if the content has diverged (not just appended to).
            // Compare the new code's prefix against what the tokenizer already has.
            let content_changed = prev_len > 0
                && (code.len() < prev_len
                    || code[..prev_len] != body.tokenizer.original[..prev_len]);

            if content_changed {
                // Content changed entirely — reset and re-tokenize from scratch
                body.tokenizer.clear();
                body.parser = ScriptParser::keeping_numbers(self.bx.keep_numbers);
                body.checkpoint = None;
                body.source_len = code.len();
                body.tokenizer.tokenize(code, &mut self.bx.heap);
            } else if code.len() >= prev_len {
                body.source_len = code.len();
                let new_chars = &code[prev_len..];
                if !new_chars.is_empty() {
                    body.tokenizer.tokenize(new_chars, &mut self.bx.heap);
                }
            }

            // If we stopped mid-string, intern the partial content so the parser
            // can emit the real string value into opcodes for incremental rendering.
            let unfinished = body.tokenizer.intern_unfinished_string(&mut self.bx.heap);
            // A number/identifier/operator the source stops on is still in
            // the tokenizer: parse it provisionally (the parser checkpoints
            // before it), the next append re-lexes it.
            let provisional = body.tokenizer.push_pending_token(&mut self.bx.heap);

            // Incremental parse: continue from checkpoint, auto-close for execution
            let errors_before = body.parser.parse_errors.len();
            let (site_file, site_offsets) = body.parse_site(existing_mod);
            let cp = body.parser.parse_streaming(
                &body.tokenizer,
                &site_file,
                site_offsets,
                &existing_mod.values,
                unfinished,
                provisional,
            );

            body.checkpoint = Some(cp);
            body.content_hash = ScriptBody::content_hash_of(code, &existing_mod.values);

            // A host that installed a captured-error sink is running a GAME
            // eval and needs structural parse errors to FAIL it — the
            // tolerant recovery otherwise runs something else entirely
            // (`let loop` recovered into an infinite empty loop and burned
            // the instruction budget). Live-typing paths install no sink and
            // keep the log-only tolerance.
            let from = errors_before.min(body.parser.parse_errors.len());
            let new_parse_errors: Vec<ScriptErrorRecord> = body.parser.parse_errors[from..].iter().enumerate()
                .map(|(i, text)| ScriptErrorRecord::parse(text.clone(), body.parser.diagnostics.get(from + i), &body.parser.file)).collect();

            drop(bodies);
            if let Some(sink) = &mut self.bx.captured_errors {
                sink.extend(new_parse_errors);
            }
            // Silence runtime errors during incremental eval — incomplete code
            // will inevitably produce errors that are meaningless until the
            // source is fully received.
            self.bx.silence_errors = true;
            let result = self.run_root(body_id);
            self.bx.silence_errors = false;
            if let Some(result_obj) = result.as_object() {
                self.bx.heap.set_from_eval(result_obj);
            }
            result
        } else {
            NIL
        }
    }
}

/// The arithmetic operators a host may give meaning to for its own objects
/// (see [`ScriptVmBase::object_op_hook`]).
#[relax_api(plain)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptBinOp {
    Add,
    Sub,
    Mul,
    Div,
}

/// A host's arithmetic on objects it made: `(heap, op, a, b, ip)`, called
/// for `a op b` when an operand is an object and the VM would otherwise
/// produce NaN (it has no arithmetic on objects). `Some(value)` is the
/// result; `None` keeps the VM's own. `ip` is the operator's place (for the
/// result's `made_at`).
pub type ScriptObjectOpHook = fn(&mut ScriptHeap, ScriptBinOp, ScriptValue, ScriptValue, ScriptIp) -> Option<ScriptValue>;

/// Called as a `let` or `var` binds a value: `(heap, ip, name, value)`,
/// `ip` the binding's place. A host that shows what code computed (an
/// editor's live values) installs it for one evaluation.
pub type ScriptLetHook = fn(&ScriptHeap, ScriptIp, LiveId, ScriptValue);

/// Caller-owned function pointers are only interpreted by their caller-side trampoline.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScriptLetHookAbi {
    pub data: *const (),
    pub call: unsafe extern "C-unwind" fn(*const (), *const ScriptHeap, ScriptIp, LiveId, ScriptValue),
}

#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScriptObjectOpHookAbi {
    pub data: *const (),
    pub call: unsafe extern "C-unwind" fn(*const (), *mut ScriptHeap, *const ScriptBinOp, ScriptValue, ScriptValue, ScriptIp, *mut ScriptValue) -> bool,
}

#[relax_api(handle)]
pub struct ScriptVmBase {
    pub void: usize,
    pub code: ScriptCode,
    pub heap: ScriptHeap,
    pub threads: ScriptThreads,
    pub injected_globals: std::collections::HashMap<LiveId, ScriptValue>,
    /// heap strings a running string_with keeps alive (string_pin / string_unpin)
    pub(crate) string_pins: Vec<crate::string::ScriptRcString>,
    /// vector parts vec_with took out of their objects (vec_take / vec_restore)
    pub(crate) vec_stash: Vec<(ScriptObject, Vec<ScriptVecValue>)>,
    pub is_reload: bool,
    pub debug_trace: bool,
    /// Edit mode, while a program loads: the literal immediates that ran
    /// (`crate::literal::LoadTrace`), so an editor knows which literals fed
    /// load-time values. None (always, outside edit mode): nothing recorded.
    pub literal_trace: Option<Box<crate::literal::LoadTrace>>,
    pub silence_errors: bool,
    /// Errors already logged, by site and message: a script error that repeats
    /// every frame is logged once, not tens of thousands of times a second.
    pub(crate) logged_errors: std::collections::HashSet<u64>,
    /// Whether script-directed debug output (the `~` LOG operator and
    /// `ScriptVm::log`) may reach the host log. Raw Makepad hosts keep it on;
    /// a standalone sandboxed runtime turns it off before any source runs,
    /// which makes `~` a catchable script error instead of an output path.
    pub allow_debug_output: bool,
    /// Whether an uncaught script error ends the evaluation (a Bail carrying
    /// the error) instead of being reported while the evaluation continues.
    /// Off for Makepad hosts: a module keeps evaluating past one bad statement
    /// and scripts may inspect returned error values. A host that runs script
    /// it did not write turns it on before any source runs.
    pub bail_on_uncaught_error: bool,
    /// When Some, drained errors are pushed here (formatted) instead of being
    /// logged or dropped — even under `silence_errors`. Install before an
    /// eval/call, take after, to feed diagnostics back to a host (e.g. an AI
    /// agent editing the script live).
    pub captured_errors: Option<Vec<ScriptErrorRecord>>,
    /// the captures capture_errors_nested put aside
    pub captured_stash: Vec<Option<Vec<ScriptErrorRecord>>>,
    pub run_budget: Option<ScriptRunBudget>,
    /// Instructions charged by the most recent with_instruction_limit call
    /// (see ScriptVm::last_limit_consumed).
    pub last_limit_consumed: usize,
    /// The thread's remaining allowance at Return/Bail, stashed because
    /// handle_trap_on wipes instruction_limit_remaining to None on exit.
    pub last_limit_exit_remaining: usize,
    /// Arithmetic on host objects (none by default): a host whose values
    /// are objects (animation descriptions, handles) can make `a + b` build
    /// a derived value. Only reached where an operand is an object, so no
    /// program on numbers, vectors, colours or strings changes meaning.
    pub(crate) object_op_hook: Option<ScriptObjectOpHookAbi>,
    /// What `let`s bind, told to the host (none by default).
    pub(crate) let_hook: Option<ScriptLetHookAbi>,
    /// Code parsed from now on keeps its number literals as values of their
    /// own ([`ScriptParser::keep_numbers`]).
    pub keep_numbers: bool,
    /// the allocation budgets with_heap_allocation_limit put aside (CTL_ALLOC_BEGIN / CTL_ALLOC_END)
    pub(crate) alloc_budgets: Vec<Option<ScriptAllocationBudget>>,
}

impl Drop for ScriptVmBase {
    fn drop(&mut self) { crate::inspect_registry::unregister(self); }
}

#[relax_api]
impl ScriptVmBase {
    pub(crate) fn notify_let_hook(&self, id: LiveId, value: ScriptValue) {
        if let Some(hook) = self.let_hook {
            unsafe { (hook.call)(hook.data, &self.heap, self.threads.cur_ref().trap.ip, id, value); }
        }
    }

    #[inline(always)]
    pub fn heap_mut(&mut self) -> &mut ScriptHeap { self.parts_mut().heap }
    #[inline(always)]
    pub fn select_first_unpaused_thread(&mut self) { self.vm_ctl(&mut ScriptCtlReq::new(CTL_SELECT_FIRST_UNPAUSED_THREAD)); }
    pub fn select_thread(&mut self, id: ScriptThreadId) { self.threads.set_current_thread_id(id); }
    #[inline(always)]
    pub fn gc_if_needed(&mut self) { self.vm_ctl(&mut ScriptCtlReq::new(CTL_GC_IF_NEEDED)); }
    #[inline(always)]
    pub fn body_count(&self) -> usize { self.vm_stat(&mut ScriptCtlReq::new(STAT_BODY_COUNT)) as usize }
    #[relax_api]
    pub fn body_module_read(&self, index: usize) -> ScriptModuleRead<'_> {
        ScriptModuleRead { bodies: self.code.bodies.borrow(), index }
    }
    pub fn thread_is_paused(&self, index: usize) -> Option<bool> { self.threads.get(index).map(|t| t.is_paused()) }

    #[inline(always)]
    pub fn validate_script(&mut self, code: &str, file: &str, line: usize, column: usize, values: &[ScriptValue]) -> bool {
        let mut req = ScriptBodyReq::new(BODY_VALIDATE, 0);
        req.module = ScriptModView { cargo_manifest_path: "", module_path: "", file, line, column, code, values };
        self.vm_body(&mut req)
    }

    /// The heap (a pointer: the VM's state is laid out by rustc, loader-core.md section 8).
    #[inline(always)]
    pub fn heap_ref(&self) -> &ScriptHeap {
        self.parts().heap
    }

    /// The compiled code (a pointer: rustc lays it out, loader-core.md section 8).
    #[inline(always)]
    pub fn code_ref(&self) -> &ScriptCode {
        self.parts().code
    }

    #[relax_api]
    pub fn empty() -> Self {
        Self {
            void: 0,
            code: ScriptCode::default(),
            threads: ScriptThreads::empty(),
            heap: ScriptHeap::empty(),
            injected_globals: Default::default(),
            string_pins: Vec::new(),
            vec_stash: Vec::new(),
            is_reload: false,
            debug_trace: false,
            literal_trace: None,
            silence_errors: false,
            logged_errors: Default::default(),
            allow_debug_output: true,
            bail_on_uncaught_error: false,
            captured_errors: None,
            captured_stash: Vec::new(),
            run_budget: None,
            last_limit_consumed: 0,
            last_limit_exit_remaining: 0,
            object_op_hook: None,
            let_hook: None,
            keep_numbers: false,
            alloc_budgets: Vec::new(),
        }
    }

    #[relax_api]
    pub fn new() -> Self {
        let mut heap = ScriptHeap::empty();
        let mut native = ScriptNative::new(&mut heap);
        define_math_module(&mut heap, &mut native);
        define_std_module(&mut heap, &mut native);
        define_regex_module(&mut heap, &mut native);
        define_html_module(&mut heap, &mut native);
        define_shader_module(&mut heap, &mut native);
        define_gc_module(&mut heap, &mut native);
        let pod_builtins = define_pod_module(&mut heap, &mut native);
        let mod_call = define_mod_call(&mut heap, &mut native);

        let builtins = ScriptBuiltins::new(&mut heap, pod_builtins, mod_call);

        Self {
            void: 0,
            code: ScriptCode {
                builtins,
                native: RefCell::new(native),
                bodies: Default::default(),
                free_bodies: Default::default(),
                crate_manifests: Default::default(),
                script_mod_overrides: Default::default(),
                shader_fn_namespaces: Default::default(),
            },
            threads: ScriptThreads::new(),
            heap: heap,
            injected_globals: Default::default(),
            string_pins: Vec::new(),
            vec_stash: Vec::new(),
            is_reload: false,
            debug_trace: false,
            literal_trace: None,
            silence_errors: false,
            logged_errors: Default::default(),
            allow_debug_output: true,
            bail_on_uncaught_error: false,
            captured_errors: None,
            captured_stash: Vec::new(),
            run_budget: None,
            last_limit_consumed: 0,
            last_limit_exit_remaining: 0,
            object_op_hook: None,
            let_hook: None,
            keep_numbers: false,
            alloc_budgets: Vec::new(),
        }
    }
}


// moved from ScriptVm: they use nothing but the base
impl ScriptVmBase {
    /// Name `body`'s file in its messages and count its first line from
    /// `ScriptMod::line` (see [`ScriptBody::shown`]).
    pub(crate) fn set_body_shown(&mut self, body: u16, file: &str) {
        self.code.bodies.borrow_mut()[body as usize].shown = Some(file.to_string());
    }

    /// Bring every name of `from` into the scope of `body` (a module made
    /// by [`Self::add_script_mod`] that has not run yet), as
    /// `use <from>.*` does. A name already there is replaced; a module's
    /// private keys (`__forms`, `__tag`) stay behind.
    pub(crate) fn body_use_all(&mut self, body: u16, from: ScriptObject) {
        let scope = self.code.bodies.borrow()[body as usize].scope.obj;
        for (key, value) in self.object_entries(from) {
            let Some(id) = key.as_id() else { continue };
            let mut head = [0u8; 2];
            if LiveId::lut_name_copy(id, &mut head) == 2 && head == *b"__" {
                continue;
            }
            self.heap.set_value_def(scope, key, value);
        }
    }

    /// The keyed entries of `object`'s own map and vector part.
    pub(crate) fn object_entries(&self, object: ScriptObject) -> Vec<(ScriptValue, ScriptValue)> {
        let data = &self.heap.objects[object];
        let mut items: Vec<(ScriptValue, ScriptValue)> = data.map.iter().map(|(k, v)| (*k, v.value)).collect();
        items.extend(data.vec.iter().map(|item| (item.key, item.value)));
        items
    }

    /// The names a module defined at its top level (its `let`s and `fn`s) as
    /// an object, once `body` has run: what a library exports without
    /// writing an export list. Names that arrived by `use` (the entries of
    /// `uses`, the VM's injected globals, `mod`) are not part of it.
    pub(crate) fn scope_exports(&mut self, body: u16, uses: &[ScriptObject]) -> Option<ScriptObject> {
        let end = {
            let bodies = self.code.bodies.borrow();
            let body = bodies.get(body as usize)?;
            body.end_scope.as_ref()?.as_object()
        };
        let mut taken: Vec<ScriptValue> = Vec::new();
        let mut entries: Vec<(ScriptValue, ScriptValue)> = Vec::new();
        let mut at = Some(end);
        while let Some(scope) = at {
            let data = &self.heap.objects[scope];
            for (k, v) in data.map.iter() {
                if !taken.contains(k) {
                    taken.push(*k);
                    entries.push((*k, v.value));
                }
            }
            at = self.heap.proto(scope).as_object();
        }
        let used: Vec<(ScriptValue, ScriptValue)> = uses.iter().flat_map(|u| self.object_entries(*u)).collect();
        let exports = self.heap.new_object();
        for (key, value) in entries {
            let Some(id) = key.as_id() else { continue };
            let mut head = [0u8; 2];
            if id == id!(mod) || (LiveId::lut_name_copy(id, &mut head) == 2 && head == *b"__") {
                continue;
            }
            if self.injected_globals.get(&id) == Some(&value) {
                continue;
            }
            if used.iter().any(|(k, v)| *k == key && *v == value) {
                continue;
            }
            self.heap.set_value_def(exports, key, value);
        }
        Some(exports)
    }

    pub(crate) fn format_error(&self, err: &crate::trap::ScriptError) -> ScriptErrorRecord {
        let loc = err
            .value
            .as_err()
            .and_then(|ptr| self.code.ip_to_loc(ptr.ip));
        // A name an adopted fn reads but its own scope does not bind came from the
        // VM that made it: say how a value gets in.
        // The general "Did you mean" list over every name in scope is dropped
        // there: none of those names is what the fn's author meant to read.
        let adopted_miss = err.message.starts_with("variable ")
            && err.message.contains(" not found in scope")
            && err.value.as_err().is_some_and(|ptr| self.code.ip_is_adopted(ptr.ip));
        let (message, hint) = if adopted_miss {
            let cut = err.message.find(". Did you mean").unwrap_or(err.message.len());
            (&err.message[..cut], "; values from the game come in as inputs")
        } else {
            (err.message.as_str(), "")
        };
        let text = if let Some(loc) = &loc {
            format!(
                "{}:{}:{}: {}{} ({}:{})",
                loc.file, loc.line, loc.col, message, hint, err.origin_file, err.origin_line
            )
        } else {
            format!("{}: {}", err.origin_file, message)
        };
        ScriptErrorRecord { loc: loc.map(|loc| ScriptErrorLoc { file: loc.file, line: loc.line, col: loc.col }), message: message.to_string(), text }
    }

    pub(crate) fn drain_errors(&mut self) {
        loop {
            let err = self.threads.cur().trap.err_pop_front();
            if let Some(err) = err {
                if self.captured_errors.is_some() {
                    let formatted = self.format_error(&err);
                    crate::inspect_registry::error(self, || formatted.text().to_owned());
                    if let Some(sink) = self.captured_errors.as_mut() {
                        sink.push(formatted);
                    }
                    continue;
                }
                if self.silence_errors {
                    continue;
                }
                {
                    use std::hash::{Hash, Hasher};
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    (&err.origin_file, err.origin_line, &err.message).hash(&mut h);
                    if self.logged_errors.len() < 4096 && !self.logged_errors.insert(h.finish()) {
                        continue;
                    }
                }
                crate::inspect_registry::error(self, || self.format_error(&err).text().to_owned());
                if let Some(ptr) = err.value.as_err() {
                    if let Some(loc2) = self.code.ip_to_loc(ptr.ip) {
                        log_with_level(
                            &loc2.file,
                            loc2.line,
                            loc2.col,
                            loc2.line,
                            loc2.col,
                            format!("{} ({}:{})", err.message, err.origin_file, err.origin_line),
                            LogLevel::Error,
                        );
                    } else {
                        // No location info, still log the error
                        log_with_level(
                            &err.origin_file,
                            err.origin_line,
                            0,
                            err.origin_line,
                            0,
                            err.message.clone(),
                            LogLevel::Error,
                        );
                    }
                } else {
                    // Error without IP, still log
                    log_with_level(
                        &err.origin_file,
                        err.origin_line,
                        0,
                        err.origin_line,
                        0,
                        err.message.clone(),
                        LogLevel::Error,
                    );
                }
            } else {
                break;
            }
        }
    }

    pub(crate) fn apply_injected_globals_to_scope(&mut self, scope_obj: ScriptObject) {
        if self.injected_globals.is_empty() {
            return;
        }
        let globals: Vec<(LiveId, ScriptValue)> = self
            .injected_globals
            .iter()
            .map(|(key, value)| (*key, *value))
            .collect();
        for (key, value) in globals {
            self.heap
                .force_value_in_map(scope_obj, key.into(), value);
        }
    }

    pub(crate) fn apply_injected_globals_to_all_scopes(&mut self) {
        if self.injected_globals.is_empty() {
            return;
        }
        let scope_objects: Vec<ScriptObject> = {
            let bodies = self.code.bodies.borrow();
            bodies.iter().map(|body| body.scope.as_object()).collect()
        };
        for scope_obj in scope_objects {
            self.apply_injected_globals_to_scope(scope_obj);
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Script, ScriptHook, Default)]
    struct ApplyEvalParityTest {
        #[source]
        source: ScriptObjectRef,
        #[live]
        is_even: f32,
    }

    #[test]
    fn script_apply_eval_refreshes_interpolated_values_on_reused_callsite() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = ScriptVm {
            host: host.host_context(),
            bx: Box::new(ScriptVmBase::new()),
        };

        let mut item = ApplyEvalParityTest::default();
        let obj = vm.heap_mut().new_object();
        item.source = vm.heap_mut().new_object_ref(obj);

        for idx in 0..6 {
            let is_even_f = if idx % 2 == 0 { 1.0f32 } else { 0.0f32 };
            script_apply_eval!(vm, item, {
                is_even: #(is_even_f)
            });
            assert_eq!(
                item.is_even, is_even_f,
                "reused script_apply_eval callsite kept stale value at iteration {}",
                idx
            );
        }
    }

    static LAZY_RUNS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    unsafe extern "C-unwind" fn expand_demo(vm: *mut ScriptVm) {
        let vm = &mut *vm;
        LAZY_RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        vm.eval(script! { mod.demo = {x: 5, y: 7} mod.demo_extra = {z: 9} true });
    }

    /// A lazy system module is absent until first read, made once by its expander, then
    /// ordinary: field reads, `use`, `use *`, optional reads and `vm.module` all see it.
    #[test]
    fn a_lazy_system_module_expands_once_on_first_touch() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
        vm.declare_lazy_modules("demo", &[id!(demo), id!(demo_extra)], expand_demo);
        let before = LAZY_RUNS.load(std::sync::atomic::Ordering::Relaxed);
        // untouched: absent to a heap-only reader, nothing ran
        assert_eq!(vm.heap().module(id!(demo)), ScriptObject::ZERO);
        assert!(vm.has_lazy_modules(), "a heap-only read expands nothing");
        assert_eq!(LAZY_RUNS.load(std::sync::atomic::Ordering::Relaxed), before);
        // first touch through a field read
        assert_eq!(vm.eval(script! { mod.demo.x }).as_number(), Some(5.0));
        assert_eq!(LAZY_RUNS.load(std::sync::atomic::Ordering::Relaxed), before + 1);
        // the whole group came with it, once
        assert_eq!(vm.eval(script! { mod.demo_extra.z }).as_number(), Some(9.0));
        assert_eq!(vm.eval(script! { use mod.demo.* y }).as_number(), Some(7.0));
        assert_eq!(LAZY_RUNS.load(std::sync::atomic::Ordering::Relaxed), before + 1);
        assert_ne!(vm.module(id!(demo)), ScriptObject::ZERO);

        // a fresh VM touched first by `use`
        let mut vm2 = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
        vm2.declare_lazy_modules("demo", &[id!(demo), id!(demo_extra)], expand_demo);
        assert_eq!(vm2.eval(script! { use mod.demo.x x + 1 }).as_number(), Some(6.0));
        // and through vm.module
        let mut vm3 = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
        vm3.declare_lazy_modules("demo", &[id!(demo), id!(demo_extra)], expand_demo);
        let demo = vm3.module(id!(demo));
        assert_eq!(vm3.value(demo, id!(y)).as_number(), Some(7.0));
        // an unknown module is still an ordinary miss
        let mut vm4 = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
        vm4.declare_lazy_modules("demo", &[id!(demo)], expand_demo);
        assert_eq!(vm4.eval(script! { mod.nothing_here ?? 3 }).as_number(), Some(3.0));
        assert_eq!(vm4.eval(script! { mod.demo.x ?? 3 }).as_number(), Some(5.0), "an optional read expands too");
    }

    unsafe extern "C-unwind" fn expand_capturing(vm: *mut ScriptVm) {
        let vm = &mut *vm;
        vm.capture_errors();
        vm.eval(script! { mod.cap = {x: 1} true });
        let _ = vm.take_errors();
    }

    /// An expansion inside a load that captures its errors leaves that capture in place: the
    /// registration's own capture/take does not end the caller's.
    #[test]
    fn a_lazy_expansion_keeps_the_callers_error_capture() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
        vm.declare_lazy_modules("cap", &[id!(cap)], expand_capturing);
        vm.capture_errors();
        vm.eval(script! { let a = mod.cap.x  nothing_here_at_all + 1 });
        let errors = vm.take_errors();
        assert!(errors.iter().any(|e| e.contains("nothing_here_at_all")), "the caller still captured: {errors:?}");
    }

    /// A name used without its system module in scope fails with the `use` that brings it in.
    #[test]
    fn a_missing_import_names_the_use_to_add() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
        vm.eval(script! { mod.kits = {Glow: 1} true });
        vm.capture_errors();
        vm.eval(script! { Glow + 1 });
        let errors = vm.take_errors();
        assert!(errors.iter().any(|e| e.contains("use mod.kits.Glow")), "{errors:?}");
        // and with the import it resolves
        assert_eq!(vm.eval(script! { use mod.kits.Glow Glow + 1 }).as_number(), Some(2.0));
    }

    fn parse_reports_error(code: &str) -> bool {
        let mut bx = ScriptVmBase::new();
        let mut tokenizer = ScriptTokenizer::default();
        // Production eval appends "\n;" so the tail statement finalizes
        // (the streaming parser only closes a statement on the next token);
        // mirror that here or emit-time checks never run for the last line.
        let code = format!("{}\n;", code);
        tokenizer.tokenize(&code, &mut bx.heap);
        let mut parser = ScriptParser::default();
        parser.parse(&tokenizer, "reserved_binding_test", (0, 0), &[]);
        parser.had_error
    }

    #[test]
    fn reserved_words_cannot_be_bound() {
        // let / var
        assert!(parse_reports_error("let me = 1"));
        assert!(parse_reports_error("let self = 1"));
        assert!(parse_reports_error("let scope = 1"));
        assert!(parse_reports_error("let nil = 1"));
        assert!(parse_reports_error("let true = 1"));
        assert!(parse_reports_error("let for = 1"));
        assert!(parse_reports_error("var me = 1"));
        // fn / closure argument names
        assert!(parse_reports_error("let f = |me| me"));
        assert!(parse_reports_error("let f = |x, self| x"));
        assert!(parse_reports_error("fn f(me) { }"));
        // for-loop variables
        assert!(parse_reports_error("for me in [1] { }"));
        assert!(parse_reports_error("for k, self in [1] { }"));
        // destructuring patterns
        assert!(parse_reports_error("let [me] = [1]"), "array simple");
        assert!(parse_reports_error("let {a, me} = {x: 1}"), "object multi");
        assert!(parse_reports_error("let {me} = {x: 1}"), "object single");
        assert!(parse_reports_error("let [nil] = [1]"));
        assert!(parse_reports_error("let [a, [me]] = [1, [2]]"));
    }

    #[test]
    fn reserved_words_still_read_and_normal_names_bind() {
        // Reading the implicits stays legal — only BINDING them errors.
        assert!(!parse_reports_error("let hero = 1"));
        assert!(!parse_reports_error("let x = me"));
        assert!(!parse_reports_error("let f = |dt, input| dt"));
        assert!(!parse_reports_error("for k, v in [1] { }"));
        assert!(!parse_reports_error("let {x} = {x: 1}"));
        // `me:` as an OBJECT KEY is data, not a binding.
        assert!(!parse_reports_error("let o = {me: 1}"));
    }

    #[test]
    fn reentrant_reload_of_the_active_body_does_not_keep_an_opcode_pointer() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = ScriptVm {
            host: host.host_context(),
            bx: Box::new(ScriptVmBase::new()),
        };

        let reentrant = vm.bx.heap.new_module(id!(reentrant));
        vm.add_method(reentrant, id!(reload), &[], |vm, _| {
            vm.eval(ScriptMod {
                file: "reentrant-reload.octoscript".to_owned(),
                code: "41\n;".to_owned(),
                ..Default::default()
            })
        });

        let _ = vm.eval(ScriptMod {
            file: "reentrant-reload.octoscript".to_owned(),
            code: "use mod.reentrant\nreentrant.reload()\n;".to_owned(),
            ..Default::default()
        });

        // The replacement body intentionally makes the outer VM result
        // unspecified. Under Miri or ASan this path used to dereference the
        // old parser's freed opcode allocation before it could return.
    }

    fn plain_vm(host: &mut ScriptVmHost<(), ()>) -> ScriptVm<'_> {
        ScriptVm {
            host: host.host_context(),
            bx: Box::new(ScriptVmBase::new()),
        }
    }

    #[test]
    fn streaming_try_catch_restores_the_contextual_separator_state() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = plain_vm(&mut host);
        let script_mod = || ScriptMod {
            file: "streaming-try-catch.octoscript".to_owned(),
            ..Default::default()
        };
        // The trailing space makes the tokenizer emit the separator in the
        // first pass, so the checkpoint must retain that it was consumed and
        // the appended identifier `catch` must parse as the fallback.
        let prefix = "use mod.std.assert\n\
                      let catch = 41\n\
                      try {\n\
                          assert(false)\n\
                      } catch ";

        let _ = vm.eval_with_append_source(script_mod(), prefix, ScriptObject::ZERO);
        let result = vm.eval_with_append_source(
            script_mod(),
            &format!("{prefix}catch + 1\n;"),
            ScriptObject::ZERO,
        );

        assert_eq!(result.as_f64(), Some(42.0));
    }

    #[test]
    fn streaming_logical_precedence_repatches_pending_short_circuits() {
        for (prefix, suffix, expected) in [
            ("true || false ", "&& false", true),
            ("false && true ", "|| true", true),
            ("true == 2 ", "> 1", true),
        ] {
            let mut host = ScriptVmHost::new((), ());
            let mut vm = plain_vm(&mut host);
            let module = || ScriptMod {
                file: "streaming-precedence.octoscript".into(),
                ..Default::default()
            };
            let partial = vm.with_instruction_limit(1000, |vm| {
                vm.eval_with_append_source(module(), prefix, ScriptObject::ZERO)
            });
            assert!(
                !partial.is_err(),
                "unfinished logical expression must terminate: {prefix}"
            );
            let result = vm.with_instruction_limit(1000, |vm| {
                vm.eval_with_append_source(
                    module(),
                    &format!("{prefix}{suffix}\n;"),
                    ScriptObject::ZERO,
                )
            });
            assert_eq!(result.as_bool(), Some(expected), "{prefix}{suffix}");
        }
    }

    #[test]
    fn streaming_legacy_try_ok_repatches_the_success_jump() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = plain_vm(&mut host);
        let script_mod = || ScriptMod {
            file: "streaming-try-ok.octoscript".to_owned(),
            ..Default::default()
        };
        let prefix = "let marker = 0\ntry { 7 } { marker = 1 }";

        let _ = vm.eval_with_append_source(script_mod(), prefix, ScriptObject::ZERO);
        let result = vm.eval_with_append_source(
            script_mod(),
            &format!("{prefix} ok {{ marker = 2 }}\nreturn marker\n;"),
            ScriptObject::ZERO,
        );

        assert_eq!(result.as_u40(), Some(2));
    }

    #[test]
    fn hard_time_budget_drains_its_uncatchable_error() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = plain_vm(&mut host);
        vm.bx.captured_errors = Some(Vec::new());
        vm.bx.run_budget = Some(ScriptRunBudget::from_durations(
            Duration::ZERO,
            Duration::ZERO,
            1,
        ));

        let result = vm.eval(ScriptMod {
            file: "hard-time-budget.octoscript".to_owned(),
            code: "try { loop {} } catch { 42 }\n;".to_owned(),
            ..Default::default()
        });

        assert!(result.is_err());
        assert!(vm.bx.threads.cur_ref().trap.err_is_empty());
        assert!(vm
            .take_errors()
            .iter()
            .any(|diagnostic| diagnostic.contains("script time budget exceeded")));
    }

    #[test]
    fn return_does_not_pop_to_me_after_an_operand_stack_limit() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = plain_vm(&mut host);
        let receiver = vm.heap_mut().new_object();
        let pop_to_me = OpcodeArgs(OpcodeArgs::POP_TO_ME_FLAG);

        {
            let thread = vm.thread_mut();
            thread.stack.push(7.into());
            thread.mes.push(ScriptMe::Object(receiver));
            assert!(thread.push_call_frame(CallFrame {
                bases: StackBases::default(),
                args: OpcodeArgs::NONE,
                return_ip: None,
                prev_slot_base: 0,
                body_root: false,
            }));
            assert!(thread.push_call_frame(CallFrame {
                bases: StackBases {
                    stack: 1,
                    mes: 1,
                    ..Default::default()
                },
                args: pop_to_me,
                return_ip: Some(ScriptIp::default()),
                prev_slot_base: 0,
                body_root: false,
            }));
        }

        vm.with_stack_value_limit(1, |vm| {
            vm.opcode(
                Opcode::RETURN,
                OpcodeArgs(OpcodeArgs::NIL.0 | OpcodeArgs::POP_TO_ME_FLAG),
            );
        });

        assert!(vm.thread().has_execution_limit_exceeded());
        assert_eq!(vm.thread().stack.len(), 1);
        assert_eq!(vm.thread().stack[0].as_f64(), Some(7.0));
        assert_eq!(vm.bx.heap.vec_len(receiver), 0);
    }

    #[test]
    fn uncaught_error_terminates_an_eval_only_when_the_host_opts_in() {
        // `f()` on a number raises an uncaught script error; `42` follows it.
        let source = "let f = 1\nf()\n42";
        let plain_eval = |vm: &mut ScriptVm| {
            vm.bx.captured_errors = Some(Vec::new());
            vm.eval(ScriptMod {
                file: "uncaught-plain.octoscript".to_owned(),
                code: format!("{source}\n;"),
                ..Default::default()
            })
        };

        // Default: the error is reported and the module keeps evaluating.
        let mut host = ScriptVmHost::new((), ());
        let mut vm = plain_vm(&mut host);
        let plain = plain_eval(&mut vm);
        assert_eq!(plain.as_number(), Some(42.0), "{plain:?}");
        assert!(!vm.take_errors().is_empty());

        // Opted in: the error ends the evaluation and is its result.
        let mut host = ScriptVmHost::new((), ());
        let mut vm = plain_vm(&mut host);
        vm.bx.bail_on_uncaught_error = true;
        let bailed = plain_eval(&mut vm);
        assert!(bailed.is_err(), "{bailed:?}");
        assert!(!vm.take_errors().is_empty());

        // Streaming evals never bail, opted in or not.
        let mut host = ScriptVmHost::new((), ());
        let mut vm = plain_vm(&mut host);
        vm.bx.bail_on_uncaught_error = true;
        let streaming = vm.eval_with_append_source(
            ScriptMod {
                file: "uncaught-streaming.octoscript".to_owned(),
                ..Default::default()
            },
            &format!("{source}\n;"),
            ScriptObject::ZERO,
        );
        assert_eq!(streaming.as_number(), Some(42.0), "{streaming:?}");
    }

    #[test]
    fn malformed_ok_end_bails_instead_of_ignoring_the_missing_try_frame() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = plain_vm(&mut host);

        vm.handle_ok_end();

        assert!(matches!(
            vm.bx.threads.cur_ref().trap.get_on(),
            Some(ScriptTrapOn::Bail(_))
        ));
    }
}

impl ScriptVmBase {
    pub fn pod_builtins(&self) -> &crate::mod_pod::ScriptPodBuiltins { &self.code.builtins.pod }
}

