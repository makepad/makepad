//! Views and owned references used at the compiler's host boundary.
//!
//! Collections stay with the compiler that allocated them. A view borrows
//! only for the call; an owned reference retains and releases in this crate.
use makepad_relax_api::relax_api;

use crate::kernel::{Kernel, KernelBody, Layout, LayoutField};
use crate::{Backend, ShaderError};
use std::sync::Arc;

/// One parameter's metadata, borrowing the compiler-owned name.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ParamInfoView<'a> {
    pub name: &'a str,
    pub default: f32,
    pub min: f32,
    pub max: f32,
}

#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LayoutFieldView<'a> {
    pub name: &'a str,
    pub ty: crate::kernel::FieldTy,
    pub offset: u32,
}

#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LayoutView<'a> {
    pub name: &'a str,
    pub stride: u32,
    pub fields: &'a [LayoutFieldView<'a>],
}

#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ErrorView<'a> {
    pub start: usize,
    pub end: usize,
    pub message: &'a str,
}

/// One buffer's metadata, borrowing only the compiler-owned name.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct BufferInfoView<'a> {
    pub name: &'a str,
    pub access: crate::kernel::Access,
    pub stride: u32,
}

/// One strong reference to a kernel. A handle: it clones and drops through
/// the compiler's own code, once each, whichever side runs it; the kernel's
/// plain face is reached through its deref.
#[relax_api(handle, clone)]
#[derive(Clone)]
pub struct KernelRef(Arc<KernelBody>);

impl KernelRef {
    pub(crate) fn from_arc(kernel: Arc<KernelBody>) -> Self {
        Self(kernel)
    }

    pub fn as_ptr(&self) -> *const Kernel {
        &**self
    }

    pub fn ptr_eq(&self, other: &Self) -> bool {
        std::ptr::eq(&**self, &**other)
    }
}

#[relax_api]
impl std::ops::Deref for KernelRef {
    type Target = Kernel;
    #[relax_api]
    fn deref(&self) -> &Kernel {
        &self.0.head
    }
}

impl AsRef<Kernel> for KernelRef {
    fn as_ref(&self) -> &Kernel {
        self
    }
}

impl std::fmt::Debug for KernelRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&**self, f)
    }
}

/// Extends a borrow of data its owner keeps (and never changes) to the
/// owner's life: a plain view hands out `'static` slices of it, always
/// behind a borrow of the owner.
///
/// # Safety
/// `r` must stay allocated and unchanged for as long as any copy lives.
pub(crate) unsafe fn forever<T: ?Sized>(r: &T) -> &'static T {
    &*(r as *const T)
}

/// A live-literal predicate. The context is borrowed until compilation
/// returns, and only the originating compiler's callback dereferences it.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LiveFilter {
    pub context: *const (),
    pub include: Option<unsafe extern "C-unwind" fn(*const (), usize) -> bool>,
}

/// Where a compiled-in fn is written: file, 1-based first line and column,
/// last line and the column one past its last token.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FnPlaceView<'a> {
    pub file: &'a str,
    pub line: u32,
    pub col: u32,
    pub end_line: u32,
    pub end_col: u32,
}

/// An error's place in the document: the file, 1-based line and column, and
/// the message without the place. An empty file: the error has no place.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ErrorPlaceView<'a> {
    pub file: &'a str,
    pub line: u32,
    pub col: u32,
    pub message: &'a str,
}

/// One host count of a prepared Kernel object (`count`, `repeat`, ..): a
/// number, or a program that computes it.
#[cfg(feature = "vm")]
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CountView<'a> {
    pub field: &'a str,
    pub has_value: bool,
    pub value: f64,
    pub program: Option<&'a crate::object::PreparedProgram>,
    pub records: &'a [&'a str],
}

/// Everything a compilation holds, borrowed from it: the kernel, the
/// diagnostics (with their document places), what a kernel made from
/// Splash values compiled in, and, for a prepared Kernel object, its
/// programs and counts.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CompilationView<'a> {
    pub kernel: Option<&'a KernelRef>,
    pub errors: &'a [ErrorView<'a>],
    /// Per error, its document place (kernels compiled from Splash values).
    pub places: &'a [ErrorPlaceView<'a>],
    pub fn_places: &'a [FnPlaceView<'a>],
    /// The document names it compiled in as constants.
    pub constants: &'a [&'a str],
    /// Names the compiled-in fns use without binding them.
    pub free: &'a [&'a str],
    #[cfg(feature = "vm")]
    pub per_frame: bool,
    #[cfg(feature = "vm")]
    pub program: Option<&'a crate::object::PreparedProgram>,
    #[cfg(feature = "vm")]
    pub counts: &'a [CountView<'a>],
}

/// Compiler-owned result storage, including all diagnostic strings. Read
/// it through [`KernelCompilation::view`].
#[relax_api(handle)]
pub struct KernelCompilation(Box<CompileData>);

pub(crate) struct CompileData {
    kernel: Option<KernelRef>,
    errors: Vec<ShaderError>,
    /// Per error (when compiled from Splash values): its message without
    /// the place, and the document place (file, line, column).
    places: Vec<Option<(String, String, u32, u32)>>,
    /// Names the compiled-in fns use without binding them (kernels from
    /// Splash values).
    free: Vec<String>,
    /// Where its compiled-in fns are written and the document names it
    /// compiled in as constants (kernels from Splash values): what an
    /// editor's edit there changes only by compiling again.
    fn_places: Vec<(String, u32, u32, u32, u32)>,
    constants: Vec<String>,
    #[cfg(feature = "vm")]
    object: Option<crate::object::ObjectData>,
    views: std::sync::OnceLock<Views>,
}

/// The slices of a [`CompilationView`], built on first look.
struct Views {
    errors: Vec<ErrorView<'static>>,
    places: Vec<ErrorPlaceView<'static>>,
    fn_places: Vec<FnPlaceView<'static>>,
    constants: Vec<&'static str>,
    free: Vec<&'static str>,
    #[cfg(feature = "vm")]
    counts: Vec<CountView<'static>>,
    /// Owns the record names the counts borrow.
    #[cfg(feature = "vm")]
    #[allow(dead_code)]
    count_records: Vec<Vec<&'static str>>,
}

impl CompileData {
    pub(crate) fn new(kernel: Option<KernelRef>, errors: Vec<ShaderError>) -> Self {
        Self {
            kernel,
            errors,
            places: Vec::new(),
            free: Vec::new(),
            fn_places: Vec::new(),
            constants: Vec::new(),
            #[cfg(feature = "vm")]
            object: None,
            views: std::sync::OnceLock::new(),
        }
    }

    fn views(&self) -> &Views {
        self.views.get_or_init(|| {
            // SAFETY: the views borrow this data, which is not changed once
            // built (the OnceLock is filled by the first look) and which
            // outlives them: a CompilationView borrows the compilation.
            unsafe {
                let none = ErrorPlaceView { file: "", line: 0, col: 0, message: "" };
                #[cfg(feature = "vm")]
                let count_records: Vec<Vec<&'static str>> = self.object.iter().flat_map(|o| o.counts.iter()).map(|c| c.records.iter().map(|r| forever(r.as_str())).collect()).collect();
                Views {
                    errors: self.errors.iter().map(|e| ErrorView { start: e.start, end: e.end, message: forever(e.message.as_str()) }).collect(),
                    places: (0..self.errors.len())
                        .map(|k| match self.places.get(k) {
                            Some(Some((message, file, line, col))) => ErrorPlaceView { file: forever(file.as_str()), line: *line, col: *col, message: forever(message.as_str()) },
                            _ => none,
                        })
                        .collect(),
                    fn_places: self.fn_places.iter().map(|(file, line, col, end_line, end_col)| FnPlaceView { file: forever(file.as_str()), line: *line, col: *col, end_line: *end_line, end_col: *end_col }).collect(),
                    constants: self.constants.iter().map(|c| forever(c.as_str())).collect(),
                    free: self.free.iter().map(|f| forever(f.as_str())).collect(),
                    #[cfg(feature = "vm")]
                    counts: self
                        .object
                        .iter()
                        .flat_map(|o| o.counts.iter())
                        .zip(&count_records)
                        .map(|(c, records)| CountView { field: forever(c.field.as_str()), has_value: c.value.is_some(), value: c.value.unwrap_or(0.0), program: c.program.as_ref().map(|p| forever(p)), records: forever(records.as_slice()) })
                        .collect(),
                    #[cfg(feature = "vm")]
                    count_records,
                }
            }
        })
    }
}

/// The one door to the compiler's results.
#[relax_api]
impl KernelCompilation {
    pub fn view(&self) -> CompilationView<'_> {
        let data = &*self.0;
        let views = data.views();
        CompilationView {
            kernel: data.kernel.as_ref(),
            errors: &views.errors,
            places: &views.places,
            fn_places: &views.fn_places,
            constants: &views.constants,
            free: &views.free,
            #[cfg(feature = "vm")]
            per_frame: data.object.as_ref().is_some_and(|o| o.per_frame),
            #[cfg(feature = "vm")]
            program: data.object.as_ref().and_then(|o| o.program.as_ref()),
            #[cfg(feature = "vm")]
            counts: &views.counts,
        }
    }
}

impl KernelCompilation {
    /// The compiled kernel (another reference to it: the compilation keeps
    /// its own until dropped).
    pub fn take_kernel(&mut self) -> Option<KernelRef> {
        self.view().kernel.cloned()
    }

    pub fn error_count(&self) -> usize {
        self.view().errors.len()
    }

    pub fn error(&self, index: usize) -> ErrorView<'_> {
        self.view().errors[index]
    }

    /// Every error's message, joined.
    pub fn messages(&self) -> String {
        self.view().errors.iter().map(|e| e.message).collect::<Vec<_>>().join("; ")
    }

    /// Error `index`'s document place, when it points into a fn of the
    /// document (kernels compiled from Splash values).
    pub fn error_place(&self, index: usize) -> Option<ErrorPlaceView<'_>> {
        let place = *self.view().places.get(index)?;
        (!place.file.is_empty()).then_some(place)
    }

    /// The kernel's compiled-in fns, where written (kernels compiled from
    /// Splash values).
    pub fn fn_place_count(&self) -> usize {
        self.view().fn_places.len()
    }

    pub fn fn_place(&self, index: usize) -> FnPlaceView<'_> {
        self.view().fn_places[index]
    }

    /// The document names it compiled in as constants.
    pub fn constant_count(&self) -> usize {
        self.view().constants.len()
    }

    pub fn constant(&self, index: usize) -> &str {
        self.view().constants[index]
    }

    /// Whether the kernel's fns use `name` without binding it (`time`: it
    /// runs every frame). Kernels compiled from Splash values.
    pub fn reads(&self, name: &str) -> bool {
        self.view().free.iter().any(|n| *n == name)
    }
}

/// A declaration's kind (see [`DeclView`]).
#[relax_api(plain)]
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeclKind {
    Input,
    Output,
    EmitBuffer,
    Param,
    /// A record type (`record`: its fields and zero values, in order).
    Record,
}

/// One field of a record declaration and its zero value: a number
/// (`lanes` 1), a float vector (`lanes` 2 to 4) or a colour (`lanes` 0).
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RecordFieldView<'a> {
    pub name: &'a str,
    pub lanes: u32,
    pub values: [f64; 4],
    pub color: u32,
}

/// One host declaration of a kernel made from Splash values, borrowed for
/// one call. `u32::MAX` is an absent stride or offset, `""` an absent
/// buffer; an emit buffer's record is `ty`, or `words` wide when `ty` is
/// empty; a param's range is `min..max` when `ranged`; a record's fields
/// are `record`.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct DeclView<'a> {
    pub kind: DeclKind,
    pub name: &'a str,
    pub ty: &'a str,
    pub stride: u32,
    pub offset: u32,
    pub buffer: &'a str,
    pub words: u32,
    pub capacity: u32,
    pub default: f32,
    pub min: f32,
    pub max: f32,
    pub ranged: bool,
    pub record: &'a [RecordFieldView<'a>],
}

/// A kernel made from Splash values (`vm_kernel::VmKernel`), borrowed for
/// one call: the host's declarations, the entry fn object, the modules it
/// uses and the fns it binds by name.
#[relax_api(plain)]
#[cfg(feature = "vm")]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VmKernelView<'a> {
    pub decls: &'a [DeclView<'a>],
    pub entry: crate::vm_kernel::Entry,
    pub entry_fn: makepad_script::ScriptObject,
    pub math: crate::kernel::MathMode,
    pub uses: &'a [&'a str],
    pub bind_names: &'a [&'a str],
    pub bind_fns: &'a [makepad_script::ScriptObject],
    /// The document's values as it wrote them: whole numbers stay
    /// integers, arrays and plain objects are constants too (a document
    /// kernel host); else only floats, colours and vectors.
    pub whole_values: bool,
}

/// What to compile: one door for every way into the compiler.
#[relax_api(plain)]
#[repr(C, u8)]
pub enum CompileReq<'a> {
    /// Splash text. The live filter's context is borrowed until compilation
    /// returns, and only the originating compiler's callback dereferences it
    /// (it must accept every source byte offset).
    Source { src: &'a str, layouts: &'a [LayoutView<'a>], backend: Backend, modules: &'a [crate::module::Module<'a>], live: LiveFilter },
    /// A kernel made from Splash values: its syntax tree comes from the
    /// declarations and the fns the entry reaches through the VM. The errors
    /// name the document place.
    #[cfg(feature = "vm")]
    Vm { vm: &'a makepad_script::ScriptVm<'a>, kernel: &'a VmKernelView<'a>, layouts: &'a [LayoutView<'a>], backend: Backend, modules: &'a [crate::module::Module<'a>] },
    /// A VM Kernel object: its complete lexical closure, with the host's
    /// implicit parameters, and every scalar helper program prepared in one
    /// operation (compile the programs with `Prepared`).
    #[cfg(feature = "vm")]
    Object {
        vm: &'a makepad_script::ScriptVm<'a>,
        object: makepad_script::ScriptObject,
        name: &'a str,
        host_fields: &'a [&'a str],
        implicit: &'a [crate::object::ImplicitParam<'a>],
        dynamic_names: &'a [&'a str],
        counts: crate::object::CountPolicy<'a>,
        math: crate::kernel::MathMode,
    },
    /// A prepared program, completely compiled by a backend.
    #[cfg(feature = "vm")]
    Prepared { program: &'a crate::object::PreparedProgram, backend: Backend, modules: &'a [crate::module::Module<'a>] },
}

/// The kernel compiles running now, as `kind [name] on thread` (the host's quit trace names
/// what an app's stop waited for: a compile that has begun runs to its end).
pub fn kernels_compiling() -> Vec<String> {
    COMPILING.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(_, what)| what.clone()).collect()
}

static COMPILING: std::sync::Mutex<Vec<(u64, String)>> = std::sync::Mutex::new(Vec::new());

/// One running compile in [`COMPILING`], removed when it ends.
struct Compiling(u64);

impl Compiling {
    fn begin(req: &CompileReq<'_>) -> Compiling {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let kind = match req {
            CompileReq::Source { .. } => "source kernel".to_string(),
            #[cfg(feature = "vm")]
            CompileReq::Vm { .. } => "splash kernel".to_string(),
            #[cfg(feature = "vm")]
            CompileReq::Object { name, .. } => format!("kernel object {name}"),
            #[cfg(feature = "vm")]
            CompileReq::Prepared { .. } => "prepared kernel".to_string(),
        };
        let what = format!("{kind} on {}", std::thread::current().name().unwrap_or("?"));
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        COMPILING.lock().unwrap_or_else(|e| e.into_inner()).push((id, what));
        Compiling(id)
    }
}

impl Drop for Compiling {
    fn drop(&mut self) {
        COMPILING.lock().unwrap_or_else(|e| e.into_inner()).retain(|(id, _)| *id != self.0);
    }
}

/// Compiles wholly inside the provided crate. All input views expire on
/// return; the output owns its kernel or diagnostics independently.
#[relax_api]
pub fn kernel_compile(req: &CompileReq<'_>) -> KernelCompilation {
    let _compiling = Compiling::begin(req);
    let data = match req {
        CompileReq::Source { src, layouts, backend, modules, live } => compile_source(src, layouts, *backend, modules, *live),
        #[cfg(feature = "vm")]
        CompileReq::Vm { vm, kernel, layouts, backend, modules } => compile_vm(vm, kernel, layouts, *backend, modules),
        #[cfg(feature = "vm")]
        CompileReq::Object { vm, object, name, host_fields, implicit, dynamic_names, counts, math } => match crate::object::prepare(vm, *object, name, host_fields, implicit, dynamic_names, *counts, *math) {
            Ok(object) => {
                let mut data = CompileData::new(None, Vec::new());
                data.object = Some(object);
                data
            }
            Err(error) => CompileData::new(None, vec![ShaderError::new(0, 0, error)]),
        },
        #[cfg(feature = "vm")]
        CompileReq::Prepared { program, backend, modules } => match program.program().compile(&[], *backend, modules) {
            Ok(kernel) => CompileData::new(Some(kernel), Vec::new()),
            Err(errors) => CompileData::new(None, errors),
        },
    };
    KernelCompilation(Box::new(data))
}

/// Compiles Splash text.
///
/// # Safety
/// If `live.include` is present, its context and code must remain valid
/// during this call, and the callback must accept every source byte offset.
#[inline(always)]
pub unsafe fn compile_kernel(src: &str, layouts: &[LayoutView<'_>], backend: Backend, modules: &[crate::module::Module<'_>], live: LiveFilter) -> KernelCompilation {
    kernel_compile(&CompileReq::Source { src, layouts, backend, modules, live })
}

/// Builds and compiles a kernel made from Splash values wholly inside the
/// provided crate.
#[cfg(feature = "vm")]
#[inline(always)]
pub fn compile_vm_kernel(vm: &makepad_script::ScriptVm<'_>, kernel: &VmKernelView<'_>, layouts: &[LayoutView<'_>], backend: Backend, modules: &[crate::module::Module<'_>]) -> KernelCompilation {
    kernel_compile(&CompileReq::Vm { vm, kernel, layouts, backend, modules })
}

fn compile_source(src: &str, layouts: &[LayoutView<'_>], backend: Backend, modules: &[crate::module::Module<'_>], live: LiveFilter) -> CompileData {
    let mismatches = layout_mismatches(layouts);
    if !mismatches.is_empty() {
        return CompileData::new(None, mismatches);
    }
    let layouts = owned_layouts(layouts);
    let filter = |offset| match live.include {
        // SAFETY: the request's contract (see `compile_kernel`).
        Some(include) => unsafe { include(live.context, offset) },
        None => false,
    };
    let filter = live.include.map(|_| &filter as &dyn Fn(usize) -> bool);
    match crate::kernel::compile_live_impl(src, &layouts, backend, modules, filter) {
        Ok(kernel) => CompileData::new(Some(KernelRef::from_arc(kernel)), Vec::new()),
        Err(errors) => CompileData::new(None, errors),
    }
}

#[cfg(feature = "vm")]
fn compile_vm(vm: &makepad_script::ScriptVm<'_>, kernel: &VmKernelView<'_>, layouts: &[LayoutView<'_>], backend: Backend, modules: &[crate::module::Module<'_>]) -> CompileData {
    let mismatches = layout_mismatches(layouts);
    if !mismatches.is_empty() {
        let mut data = CompileData::new(None, mismatches);
        data.places = data.errors.iter().map(|_| None).collect();
        return data;
    }
    let layouts = owned_layouts(layouts);
    let k = from_view(kernel);
    let (mut fn_places, mut constants) = (Vec::new(), Vec::new());
    let (kernel, errors, places, free) = match crate::vm_kernel::kernel_program(vm, &k, kernel.whole_values) {
        Err(e) => (None, vec![e], vec![None], Vec::new()),
        Ok(program) => {
            let free = program.free.clone();
            fn_places = program.fn_places();
            constants = program.constants.clone();
            match program.compile_placed(&layouts, backend, modules) {
                Ok(kernel) => (Some(kernel), Vec::new(), Vec::new(), free),
                Err(errors) => {
                    let places = errors.iter().map(|(e, place)| place.as_ref().map(|(file, line, col)| (e.message.clone(), file.clone(), *line, *col))).collect();
                    let errors = errors.into_iter().map(|(e, place)| match place {
                        Some((file, line, col)) => ShaderError { message: format!("{} ({file}:{line}:{col})", e.message), ..e },
                        None => e,
                    }).collect();
                    (None, errors, places, free)
                }
            }
        }
    };
    let mut data = CompileData::new(kernel, errors);
    data.places = places;
    data.free = free;
    data.fn_places = fn_places;
    data.constants = constants;
    data
}

#[cfg(feature = "vm")]
/// The kernel a view describes, as the compiler's own declarations.
fn from_view(k: &VmKernelView<'_>) -> crate::vm_kernel::VmKernel {
    use crate::vm_kernel::{Decl, Record, VmKernel};
    let opt = |x: u32| (x != u32::MAX).then_some(x);
    let decls = k.decls.iter().map(|d| {
        let name = d.name.to_string();
        let buffer = (!d.buffer.is_empty()).then(|| d.buffer.to_string());
        match d.kind {
            DeclKind::Input => Decl::Input { name, ty: d.ty.to_string(), stride: opt(d.stride), offset: opt(d.offset), buffer },
            DeclKind::Output => Decl::Output { name, ty: d.ty.to_string(), stride: opt(d.stride), offset: opt(d.offset), buffer },
            DeclKind::EmitBuffer => Decl::EmitBuffer { name, record: if d.ty.is_empty() { Record::Words(d.words) } else { Record::Type(d.ty.to_string()) }, capacity: d.capacity, buffer },
            DeclKind::Param => Decl::Param { name, default: d.default, range: d.ranged.then_some((d.min, d.max)) },
            DeclKind::Record => Decl::Struct {
                name,
                fields: d.record.iter().map(|f| (f.name.to_string(), match f.lanes {
                    0 => crate::vm_kernel::Const::Color(f.color),
                    1 => crate::vm_kernel::Const::Num(f.values[0]),
                    n => crate::vm_kernel::Const::Vec(n.min(4) as u8, [f.values[0] as f32, f.values[1] as f32, f.values[2] as f32, f.values[3] as f32]),
                })).collect(),
            },
        }
    }).collect();
    VmKernel {
        decls,
        entry: k.entry,
        entry_fn: k.entry_fn,
        math: k.math,
        uses: k.uses.iter().map(|u| u.to_string()).collect(),
        bind: k.bind_names.iter().zip(k.bind_fns).map(|(n, f)| (n.to_string(), *f)).collect(),
    }
}

/// The errors of `layouts` that differ from the tables registered under
/// their names ([`register_layout`]); empty when every one matches.
fn layout_mismatches(layouts: &[LayoutView<'_>]) -> Vec<ShaderError> {
    layouts.iter().filter_map(registered_mismatch).map(|message| ShaderError::new(0, 1, message)).collect()
}

fn owned_layouts(layouts: &[LayoutView<'_>]) -> Vec<Layout> {
    layouts.iter().map(owned).collect()
}

/// A zero-copy capability derived from a borrowed host buffer. The two words
/// are the backend's buffer table entry, so execution reads this table directly.
/// Copying a descriptor grants no additional safe mutable access. Only the unsafe
/// range operation dereferences it; its caller proves disjoint writes.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct BorrowedBuffer<'a> {
    address: u64,
    words: u64,
    borrow: std::marker::PhantomData<&'a mut [u32]>,
}
// SAFETY: safe methods never access the pointed-to words. Execution requires
// disjoint writes and live parent borrows on all workers.
unsafe impl Send for BorrowedBuffer<'_> {}
unsafe impl Sync for BorrowedBuffer<'_> {}
impl<'a> BorrowedBuffer<'a> {
    #[inline]
    pub fn input(words: &'a [u32]) -> Self { Self { address: words.as_ptr() as u64, words: words.len() as u64, borrow: std::marker::PhantomData } }
    #[inline]
    pub fn output(words: &'a mut [u32]) -> Self { Self { address: words.as_mut_ptr() as u64, words: words.len() as u64, borrow: std::marker::PhantomData } }
    /// # Safety
    /// The pointer must name a live AtomicU32 for the descriptor's lifetime.
    /// It is used only for the kernel's read-only control buffer.
    pub unsafe fn control(cancel: *mut u32) -> Self { Self { address: cancel as u64, words: 1, borrow: std::marker::PhantomData } }
    pub fn empty() -> Self { Self { address: 0, words: 0, borrow: std::marker::PhantomData } }
    pub fn len(&self) -> usize { self.words as usize }
}

/// The record layouts apps registered as their own tables, by name.
fn registry() -> &'static std::sync::Mutex<Vec<Layout>> {
    static REGISTRY: std::sync::OnceLock<std::sync::Mutex<Vec<Layout>>> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

fn owned(layout: &LayoutView<'_>) -> Layout {
    Layout {
        name: layout.name.into(),
        stride: layout.stride,
        fields: layout.fields.iter().map(|field| LayoutField { name: field.name.into(), ty: field.ty, offset: field.offset }).collect(),
    }
}

/// How `layout` differs from the table registered under its name, if one is.
fn registered_mismatch(layout: &LayoutView<'_>) -> Option<String> {
    let registry = registry().lock().unwrap_or_else(|e| e.into_inner());
    let known = registry.iter().find(|known| known.name == layout.name)?;
    let wanted = owned(layout);
    if *known == wanted {
        return None;
    }
    let describe = |l: &Layout| {
        let fields: Vec<String> = l.fields.iter().map(|f| format!("{} {:?} @{}", f.name, f.ty, f.offset)).collect();
        format!("stride {} [{}]", l.stride, fields.join(", "))
    };
    Some(format!("layout `{}` is not the registered one: passed {}, registered {}", layout.name, describe(&wanted), describe(known)))
}

/// Registers an app's own record table (a `const` LayoutView) as the one
/// every kernel compiled with a layout of that name must use: a compile
/// passing a different table under the name fails with a mismatch error.
/// Registering the same table again is a no-op; false when another table
/// was already registered under the name (it stays registered).
#[relax_api]
pub fn register_layout(layout: &LayoutView<'_>) -> bool {
    if registered_mismatch(layout).is_some() {
        return false;
    }
    let mut registry = registry().lock().unwrap_or_else(|e| e.into_inner());
    if !registry.iter().any(|known| known.name == layout.name) {
        registry.push(owned(layout));
    }
    true
}
