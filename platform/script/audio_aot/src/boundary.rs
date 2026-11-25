//! Compiler-owned audio programs and diagnostics at the host boundary.
use relax_api::relax_api;

use crate::{AudioShader, AudioShaderBody, Backend, ParamCurve, ParamUnit, ShaderError};
use makepad_script_compute::boundary::ErrorView;
use std::sync::{Arc, atomic::{AtomicU32, Ordering}};

/// Extends a borrow of data its owner keeps (and never changes) to the
/// owner's life: a plain view hands out `'static` slices of it, always
/// behind a borrow of the owner.
///
/// # Safety
/// `r` must stay allocated and unchanged for as long as any copy lives.
pub(crate) unsafe fn forever<T: ?Sized>(r: &T) -> &'static T {
    &*(r as *const T)
}

/// One strong reference to a shader. A handle: it clones and drops through
/// the compiler's own code, once each, whichever side runs it; the shader's
/// plain face is reached through its deref.
#[relax_api(handle, clone)]
#[derive(Clone)]
pub struct AudioShaderRef(Arc<AudioShaderBody>);

impl AudioShaderRef {
    pub(crate) fn from_arc(shader: Arc<AudioShaderBody>) -> Self {
        Self(shader)
    }

    pub fn as_ptr(&self) -> *const AudioShader {
        &**self
    }

    pub fn ptr_eq(&self, other: &Self) -> bool {
        std::ptr::eq(&**self, &**other)
    }
}

#[relax_api]
impl std::ops::Deref for AudioShaderRef {
    type Target = AudioShader;
    #[relax_api]
    fn deref(&self) -> &AudioShader {
        &self.0.head
    }
}

impl AudioShader {
    #[inline]
    pub fn mono(&self) -> bool { self.mono != 0 }

    /// Maximum look-ahead declared by the shader, in frames. Defaults to zero.
    #[inline]
    pub fn latency(&self) -> u32 { self.latency }
}

impl AsRef<AudioShader> for AudioShaderRef {
    fn as_ref(&self) -> &AudioShader {
        self
    }
}

impl std::fmt::Debug for AudioShaderRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&**self, f)
    }
}

/// One state variable's place in the state block, borrowing the
/// compiler-owned names.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct StateVarView<'a> {
    pub name: &'a str,
    /// Type signature: equal signatures have equal layouts.
    pub sig: &'a str,
    pub offset: u32,
    pub words: u32,
}

/// A compiler warning with a byte span in the source of its node.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct WarningView<'a> {
    /// Index in a fused graph, or [`NO_NODE`] for a standalone shader.
    pub node: usize,
    pub start: usize,
    pub end: usize,
    pub message: &'a str,
}

/// Presentation and smoothing information borrowed from a compiled parameter.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ParamMetaView<'a> {
    pub unit: ParamUnit,
    pub curve: ParamCurve,
    pub step: f32,
    /// Seconds, or a negative value for the default block behavior.
    pub smooth: f32,
    pub label: &'a str,
    pub group: &'a str,
    pub choices: &'a [&'a str],
}

#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FuseNodeView<'a> {
    pub name: &'a str,
    pub code: &'a str,
    pub inputs: &'a [crate::Port],
}

/// An interleaved recording, in frames at the engine rate.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SampleView<'a> {
    pub name: &'a str,
    pub channels: u32,
    pub frames: u32,
    pub data: &'a [f32],
}

#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct AudioBindings<'a> {
    pub samples: &'a [SampleView<'a>],
    pub modules: &'a [crate::Module<'a>],
    pub oversample: crate::lower::RateOverride,
    /// Borrowed for this compile, including constant folding and table initialization.
    pub control: Option<&'a AtomicU32>,
}

impl AudioBindings<'_> {
    pub(crate) fn is_cancelled(&self) -> bool {
        self.control.is_some_and(|word| word.load(Ordering::Relaxed) != 0)
    }
}

/// What to compile.
#[relax_api(plain)]
#[repr(C, u8)]
pub enum CompileReq<'a> {
    /// Audio shader text, parsed and compiled on the provided side,
    /// including recording the program.
    Source { code: &'a str, backend: Backend, counting: bool },
    /// A graph fused from borrowed, flat node records.
    Fused { nodes: &'a [FuseNodeView<'a>], outputs: &'a [crate::Port], backend: Backend },
}

/// Everything a compilation holds, borrowed from it.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CompilationView<'a> {
    pub shader: Option<&'a AudioShaderRef>,
    /// The caller cancelled before the shader was published.
    pub cancelled: bool,
    pub errors: &'a [ErrorView<'a>],
    pub warnings: &'a [WarningView<'a>],
    /// The fused graph node an error is in ([`NO_NODE`]: none).
    pub node: usize,
}

/// No node.
pub const NO_NODE: usize = usize::MAX;

/// Owns the shader or the compilation diagnostics until the caller has
/// copied their text. Read it through [`ShaderCompilation::view`].
#[relax_api(handle)]
pub struct ShaderCompilation(Box<CompileData>);

struct CompileData {
    shader: Option<AudioShaderRef>,
    errors: Vec<ShaderError>,
    node: Option<usize>,
    views: std::sync::OnceLock<Vec<ErrorView<'static>>>,
}

#[relax_api]
impl ShaderCompilation {
    pub fn view(&self) -> CompilationView<'_> {
        let data = &*self.0;
        // SAFETY: the views borrow the messages of this data, which is not
        // changed once built and outlives them: a CompilationView borrows
        // the compilation.
        let errors = data.views.get_or_init(|| data.errors.iter().map(|e| ErrorView { start: e.start, end: e.end, message: unsafe { forever(e.message.as_str()) } }).collect());
        let warnings = data.shader.as_ref().map_or(&[][..], |s| s.warnings());
        CompilationView { warnings, shader: data.shader.as_ref(), cancelled: data.shader.is_none() && errors.is_empty(), errors, node: data.node.unwrap_or(NO_NODE) }
    }
}

impl ShaderCompilation {
    fn new(result: Result<Arc<AudioShaderBody>, Vec<ShaderError>>, node: Option<usize>) -> Self {
        let (shader, errors) = match result {
            Ok(shader) => (Some(AudioShaderRef::from_arc(shader)), Vec::new()),
            Err(errors) => (None, errors),
        };
        Self(Box::new(CompileData { shader, errors, node, views: std::sync::OnceLock::new() }))
    }

    /// The compiled shader (another reference to it: the compilation keeps
    /// its own until dropped).
    pub fn take_shader(&mut self) -> Option<AudioShaderRef> {
        self.view().shader.cloned()
    }

    pub fn is_cancelled(&self) -> bool { self.view().cancelled }

    pub fn error_count(&self) -> usize {
        self.view().errors.len()
    }

    pub fn error(&self, index: usize) -> ErrorView<'_> {
        self.view().errors[index]
    }

    pub fn node(&self) -> Option<usize> {
        let node = self.view().node;
        (node != NO_NODE).then_some(node)
    }

    #[inline(always)]
    pub fn into_result(mut self) -> Result<AudioShaderRef, Vec<ShaderError>> {
        if let Some(shader) = self.take_shader() {
            return Ok(shader);
        }
        Err((0..self.error_count())
            .map(|index| {
                let error = self.error(index);
                ShaderError::new(error.start, error.end, error.message.to_string())
            })
            .collect())
    }
}

// Check on the caller's side of the op. In a JIT run the UI thread is marked
// by the running application's core_util, before control enters the native compiler.
#[relax_api(per_side)]
static SHADER_COMPILE: makepad_core_util::ui_thread::WorkerOnly =
    makepad_core_util::ui_thread::WorkerOnly::new("a score shader compile");

/// Parses, fuses and compiles on the provided side.
#[inline(always)]
pub fn audio_compile(req: &CompileReq<'_>) -> ShaderCompilation {
    match req {
        CompileReq::Source { code, backend, counting } => compile_audio(code, *backend, *counting),
        CompileReq::Fused { nodes, outputs, backend } => compile_fused(nodes, outputs, *backend),
    }
}

/// Parses and compiles on the provided side, including recording the program.
#[inline(always)]
pub fn compile_audio(code: &str, backend: Backend, counting: bool) -> ShaderCompilation {
    compile_audio_with(code, backend, counting, &AudioBindings::default())
}

#[inline(always)]
pub fn compile_audio_with(code: &str, backend: Backend, counting: bool, bind: &AudioBindings<'_>) -> ShaderCompilation {
    SHADER_COMPILE.check();
    compile_audio_with_impl(code, backend, counting, bind)
}

#[relax_api]
pub(crate) fn compile_audio_with_impl(code: &str, backend: Backend, counting: bool, bind: &AudioBindings<'_>) -> ShaderCompilation {
    let result = (|| {
        if bind.is_cancelled() { return Err(Vec::new()); }
        let (imports, items) = crate::AudioImports::source(code, bind).map_err(|error| vec![error])?;
        crate::compile_items_with(items, imports, backend, counting, bind)
    })();
    ShaderCompilation::new(result, None)
}

/// Fuses a graph from borrowed, flat node records and compiles it on the provided side.
#[inline(always)]
pub fn compile_fused(nodes: &[FuseNodeView<'_>], outputs: &[crate::Port], backend: Backend) -> ShaderCompilation {
    compile_fused_with(nodes, outputs, backend, &AudioBindings::default())
}

#[inline(always)]
pub fn compile_fused_with(nodes: &[FuseNodeView<'_>], outputs: &[crate::Port], backend: Backend, bindings: &AudioBindings<'_>) -> ShaderCompilation {
    SHADER_COMPILE.check();
    compile_fused_with_impl(nodes, outputs, backend, bindings)
}

#[relax_api]
pub(crate) fn compile_fused_with_impl(nodes: &[FuseNodeView<'_>], outputs: &[crate::Port], backend: Backend, bindings: &AudioBindings<'_>) -> ShaderCompilation {
    let nodes: Vec<_> = nodes.iter().map(|node| crate::FuseNode {
        name: node.name, code: node.code, inputs: node.inputs.to_vec(),
    }).collect();
    match crate::fuse::fuse_impl(&nodes, outputs, backend, bindings) {
        Ok(shader) => ShaderCompilation::new(Ok(shader), None),
        Err(crate::fuse::FuseFailure::Cancelled) => ShaderCompilation::new(Err(Vec::new()), None),
        Err(crate::fuse::FuseFailure::Diagnostic(error)) => ShaderCompilation::new(Err(vec![error.error]), error.node),
    }
}
