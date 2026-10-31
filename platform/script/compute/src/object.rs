//! Complete compilation preparation for a VM kernel object. Application rules
//! arrive as descriptors; declarations, dependency discovery and source maps
//! never leave this owner. Backend compilation can be deferred to a worker.
use makepad_relax_api::relax_api;
use crate::vm_kernel::{self, Decl, Entry, KernelProgram, VmKernel};
use crate::boundary::{kernel_compile, CompileReq, KernelCompilation};
use crate::kernel::MathMode;
use std::sync::Arc;
use crate::{Backend, module::Module};
use makepad_script::*;

#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ImplicitParam<'a> {
    pub name: &'a str,
    pub default: f32,
    pub min: f32,
    pub max: f32,
    pub per_frame: bool,
}

/// The host's scalar helper convention, including its entry function. The
/// compiler knows neither the meaning of these fields nor when they execute.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CountPolicy<'a> {
    pub fields: &'a [&'a str],
    pub entry: ScriptObject,
    pub output: &'a str,
    pub binding: &'a str,
    pub record_suffix: &'a str,
}

/// Compiler-owned syntax tree and provenance, independent of the originating
/// VM. It contains no VM references and may move to the host's compiler worker.
/// A handle: it clones and drops through the compiler's own code.
#[relax_api(handle, clone)]
#[derive(Clone)]
pub struct PreparedProgram(Arc<KernelProgram>);

// SAFETY: the handle shares an immutable syntax tree containing only owned
// strings and offsets. Compilation borrows it without mutation.
unsafe impl Send for PreparedProgram {}
unsafe impl Sync for PreparedProgram {}

impl PreparedProgram {
    fn new(program: KernelProgram) -> Self { Self(Arc::new(program)) }
    pub(crate) fn program(&self) -> &KernelProgram { &self.0 }

    /// Complete backend compilation; input modules borrow only for this call.
    #[inline(always)]
    pub fn compile(&self, backend: Backend, modules: &[Module<'_>]) -> KernelCompilation {
        kernel_compile(&CompileReq::Prepared { program: self, backend, modules })
    }
}

pub(crate) struct CountProgram { pub(crate) field: String, pub(crate) value: Option<f64>, pub(crate) program: Option<PreparedProgram>, pub(crate) records: Vec<String> }

/// What preparing a Kernel object made: its program, the host's counts and
/// whether it runs every frame.
pub(crate) struct ObjectData {
    pub(crate) program: Option<PreparedProgram>,
    pub(crate) counts: Vec<CountProgram>,
    pub(crate) per_frame: bool,
}

impl KernelCompilation {
    /// The prepared object's program ([`prepare_object`]).
    pub fn take_program(&mut self) -> Option<PreparedProgram> { self.view().program.cloned() }
    pub fn per_frame(&self) -> bool { self.view().per_frame }
    pub fn count_index(&self, name: &str) -> Option<usize> { self.view().counts.iter().position(|c| c.field == name) }
    pub fn count_value(&self, index: usize) -> Option<f64> {
        let c = self.view().counts[index];
        c.has_value.then_some(c.value)
    }
    pub fn take_count_program(&mut self, index: usize) -> Option<PreparedProgram> { self.view().counts[index].program.cloned() }
    pub fn record_count(&self, index: usize) -> usize { self.view().counts[index].records.len() }
    pub fn record(&self, index: usize, record: usize) -> &str { self.view().counts[index].records[record] }
}

/// Read a VM Kernel object, find its complete lexical closure, add host-supplied
/// implicit parameters, and prepare all scalar helper programs in one operation.
/// Host fields other than the supplied count fields are ignored by the compiler.
#[inline(always)]
pub fn prepare_object(vm: &ScriptVm, object: ScriptObject, name: &str, host_fields: &[&str], implicit: &[ImplicitParam<'_>], dynamic_names: &[&str], counts: CountPolicy<'_>, math: MathMode) -> KernelCompilation {
    kernel_compile(&CompileReq::Object { vm, object, name, host_fields, implicit, dynamic_names, counts, math })
}
pub(crate) fn prepare(vm: &ScriptVm, object: ScriptObject, name: &str, host_fields: &[&str], implicit: &[ImplicitParam<'_>], dynamic_names: &[&str], counts: CountPolicy<'_>, math: MathMode) -> Result<ObjectData, String> {
    let at = |field: &str, error: String| format!("Kernel `{name}`.{field}: {error}");
    let mut fields = Vec::new();
    for entry in vm.heap().map_entries_ordered(object) {
        if let Some(key) = entry.key.as_id() { fields.push((key.to_string(), entry.value)); }
    }
    let mut decls = Vec::new();
    let mut entry = None;
    let mut bind = Vec::new();
    let mut count_fields = Vec::new();
    for (key, value) in fields {
        if host_fields.contains(&key.as_str()) {
            if counts.fields.contains(&key.as_str()) { count_fields.push((key, value)); }
            continue;
        }
        let function = value.as_object().filter(|f| vm.heap().as_fn(*f).is_some());
        if let Some(kind) = Entry::from_name(&key) {
            let f = function.ok_or_else(|| at(&key, "the kernel's entry is a fn(i)".into()))?;
            if entry.is_some() { return Err(format!("Kernel `{name}`: one entry (element, vertex, instance, primitive, reduce_*)")); }
            entry = Some((kind, f));
        } else if let Some(decl) = vm_kernel::read_decl(vm, &key, value).map_err(|e| at(&key, e))? {
            decls.push(decl);
        } else if let Some(f) = function {
            bind.push((key, f));
        } else {
            return Err(at(&key, format!("a kernel field is a declaration (input, output, emit_buffer, param, record, table), a fn, or {}", host_fields.join("/"))));
        }
    }
    let (entry, entry_fn) = entry.ok_or_else(|| format!("Kernel `{name}`: no entry (`element: fn(i) {{ .. }}`)"))?;
    let declared: Vec<String> = decls.iter().map(|d| d.name().to_string()).collect();
    let mut kernel = VmKernel { decls, entry, entry_fn, math, uses: Vec::new(), bind };
    let first = vm_kernel::kernel_program(vm, &kernel, false).map_err(|e| format!("Kernel `{name}`: {}", e.message))?;
    for param in implicit {
        if first.free.iter().any(|f| f == param.name) && !declared.iter().any(|d| d == param.name) {
            kernel.decls.push(Decl::Param { name: param.name.into(), default: param.default, range: Some((param.min, param.max)) });
        }
    }
    let program = if kernel.decls.len() > declared.len() { vm_kernel::kernel_program(vm, &kernel, false).map_err(|e| format!("Kernel `{name}`: {}", e.message))? } else { first };
    let per_frame = program.free.iter().any(|f| dynamic_names.contains(&f.as_str()) || implicit.iter().any(|p| p.per_frame && p.name == f) && !declared.contains(f));
    let params: Vec<Decl> = kernel.decls.iter().filter(|d| matches!(d, Decl::Param { .. })).cloned().collect();
    let mut buffers = Vec::new();
    for decl in &kernel.decls {
        let (name, named) = match decl {
            Decl::Input { name, buffer, .. } | Decl::Output { name, buffer, .. } | Decl::EmitBuffer { name, buffer, .. } => (name, buffer),
            _ => continue,
        };
        for buffer in std::iter::once(name).chain(named.iter()) {
            if !buffers.contains(buffer) { buffers.push(buffer.clone()); }
        }
    }
    let mut programs = Vec::new();
    for (field, value) in count_fields {
        if let Some(value) = value.as_number() {
            programs.push(CountProgram { field, value: Some(value), program: None, records: Vec::new() });
            continue;
        }
        let function = value.as_object().filter(|f| vm.heap().as_fn(*f).is_some()).ok_or_else(|| at(&field, format!("a number, or a fn() of the kernel's params and <buffer>{}", counts.record_suffix)))?;
        let mut decls = vec![Decl::Output { name: counts.output.into(), ty: "f32".into(), stride: None, offset: None, buffer: None }];
        decls.extend(params.iter().cloned());
        for buffer in &buffers {
            let name = format!("{buffer}{}", counts.record_suffix);
            if !params.iter().any(|p| p.name() == name) { decls.push(Decl::Param { name, default: 0.0, range: None }); }
        }
        let k = VmKernel { decls, entry: Entry::Element, entry_fn: counts.entry, math, uses: Vec::new(), bind: vec![(counts.binding.into(), function)] };
        let program = vm_kernel::kernel_program(vm, &k, false).map_err(|e| at(&field, e.message))?;
        programs.push(CountProgram { field, value: None, program: Some(PreparedProgram::new(program)), records: buffers.clone() });
    }
    Ok(ObjectData { program: Some(PreparedProgram::new(program)), counts: programs, per_frame })
}
