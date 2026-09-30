//! Building a material program: a base lane's shader object with the
//! material's hooks installed, checked by the shader front end before any
//! pipeline exists, with every error mapped back to the author's source.
use makepad_draw::makepad_platform::makepad_script::shader::{ShaderFnCompiler, ShaderMode, ShaderOutput, ShaderType};
use makepad_draw::makepad_platform::makepad_script::shader_backend::ShaderBackend;
use makepad_draw::*;
use makepad_scene::BaseKind;
use crate::hooks::*;

/// One compile problem, at the author's source position when known.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub message: String,
    /// The Splash source the error points at (a document path, a game file).
    pub file: Option<String>,
    /// 1-based line and column; 0 when unknown.
    pub line: u32,
    pub col: u32,
    /// The hook the error was found in, when it can be told.
    pub hook: Option<Hook>,
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(hook) = self.hook {
            write!(f, "{}: ", hook.spec().key)?;
        }
        write!(f, "{}", self.message)?;
        if let Some(file) = &self.file {
            write!(f, " ({file}:{}:{})", self.line, self.col)?;
        }
        Ok(())
    }
}

/// A material that cannot be built. It is shown as the error material in
/// previews and fails an export; the diagnostics go to the author (and the
/// AI) with their source lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaterialError {
    pub diagnostics: Vec<Diagnostic>,
}

impl MaterialError {
    pub fn new(message: impl Into<String>) -> Self {
        Self { diagnostics: vec![Diagnostic { message: message.into(), file: None, line: 0, col: 0, hook: None }] }
    }
    fn hook(hook: Hook, message: impl Into<String>) -> Self {
        Self { diagnostics: vec![Diagnostic { message: message.into(), file: None, line: 0, col: 0, hook: Some(hook) }] }
    }
}

impl std::fmt::Display for MaterialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, d) in self.diagnostics.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{d}")?;
        }
        Ok(())
    }
}

impl std::error::Error for MaterialError {}

/// Turn one shader front-end message into a diagnostic. The compiler writes
/// `message (compiler.rs:line) at file:line:col` when it knows the source.
pub fn parse_diagnostic(raw: &str) -> Diagnostic {
    let mut d = Diagnostic { message: raw.trim().to_string(), file: None, line: 0, col: 0, hook: None };
    if let Some(at) = raw.rfind(" at ") {
        let loc = &raw[at + 4..];
        let mut parts = loc.rsplitn(3, ':');
        let (col, line, file) = (parts.next(), parts.next(), parts.next());
        if let (Some(col), Some(line), Some(file)) = (col, line, file) {
            if let (Ok(col), Ok(line)) = (col.trim().parse(), line.trim().parse()) {
                d.file = Some(file.to_string());
                d.line = line;
                d.col = col;
                d.message = raw[..at].trim().to_string();
            }
        }
    }
    // The compiler's own origin (`(shader_ops.rs:123)`) helps nobody writing a material.
    if let Some(open) = d.message.rfind(" (") {
        if d.message.ends_with(".rs)") || d.message[open..].contains(".rs:") {
            d.message.truncate(open);
        }
    }
    d
}

/// A material's hooks, checked against a host's mask.
#[derive(Clone, Debug, Default)]
pub struct HookSet {
    hooks: Vec<(Hook, ScriptObject)>,
    /// How far the vertex hook may move geometry, world units (culling and
    /// shadow fitting grow the item's bounds by it).
    pub bounds_pad: f32,
}

impl HookSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, hook: Hook, function: ScriptObject) -> Self {
        self.set(hook, function);
        self
    }

    pub fn set(&mut self, hook: Hook, function: ScriptObject) {
        self.hooks.retain(|(h, _)| *h != hook);
        self.hooks.push((hook, function));
        self.hooks.sort_by_key(|(h, _)| *h);
    }

    pub fn get(&self, hook: Hook) -> Option<ScriptObject> {
        self.hooks.iter().find(|(h, _)| *h == hook).map(|(_, f)| *f)
    }

    pub fn has(&self, hook: Hook) -> bool {
        self.get(hook).is_some()
    }

    pub fn iter(&self) -> impl Iterator<Item = (Hook, ScriptObject)> + '_ {
        self.hooks.iter().copied()
    }

    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    /// Read the hooks out of a material spec object (`{surface: fn(..){..},
    /// finish: fn(..){..}, bounds_pad: 0.2}`). A function under any other
    /// key, a hook outside `mask` or a non-function hook is refused.
    pub fn from_spec(vm: &mut ScriptVm, spec: ScriptObject, mask: HookMask) -> Result<Self, MaterialError> {
        let mut set = HookSet::new();
        let entries: Vec<(ScriptValue, ScriptValue)> = vm.bx.heap.map_ref(spec).iter().map(|(k, v)| (*k, v.value)).collect();
        for (key, value) in entries {
            let Some(id) = key.as_id() else { continue };
            let name = id.to_string();
            if id == id!(bounds_pad) {
                let pad = value.as_number().unwrap_or(f64::NAN);
                if !(pad.is_finite() && (0.0..=1.0e4).contains(&pad)) {
                    return Err(MaterialError::new("bounds_pad must be a number in 0..10000 (world units)"));
                }
                set.bounds_pad = pad as f32;
                continue;
            }
            let is_fn = value.as_object().is_some_and(|o| vm.bx.heap.is_fn(o));
            match HOOKS.iter().find(|s| LiveId::from_str(s.key) == id).map(|s| s.hook) {
                Some(hook) => {
                    if !mask.allows(hook) {
                        return Err(MaterialError::hook(hook, format!("this host does not accept the `{}` hook; allowed: {}", hook.spec().key, mask.keys().join(", "))));
                    }
                    let Some(function) = value.as_object().filter(|_| is_fn) else {
                        return Err(MaterialError::hook(hook, format!("`{}` must be a shader function {}", hook.spec().key, hook.spec().signature)));
                    };
                    set.set(hook, function);
                }
                None if is_fn => {
                    let why = if ENGINE_OWNED.iter().any(|k| LiveId::from_str(k) == id) { "is engine-owned" } else { "is not a material hook" };
                    return Err(MaterialError::new(format!("`{name}` {why}; hooks are {}", mask.keys().join(", "))));
                }
                None => {}
            }
        }
        set.validate(vm, mask)?;
        Ok(set)
    }

    /// Every hook allowed, a function, and declaring the hook's parameters.
    pub fn validate(&self, vm: &ScriptVm, mask: HookMask) -> Result<(), MaterialError> {
        for (hook, function) in self.iter() {
            let spec = hook.spec();
            if !mask.allows(hook) {
                return Err(MaterialError::hook(hook, format!("this host does not accept the `{}` hook", spec.key)));
            }
            if !vm.bx.heap.is_fn(function) {
                return Err(MaterialError::hook(hook, format!("must be a shader function {}", spec.signature)));
            }
            let declared = vm.bx.heap.vec_ref(function).iter().filter(|v| !v.key.is_nil()).count();
            if declared != spec.params.len() {
                return Err(MaterialError::hook(hook, format!("takes {} parameter(s), found {declared}: {}", spec.params.len(), spec.signature)));
            }
        }
        if !self.bounds_pad.is_finite() || self.bounds_pad < 0.0 {
            return Err(MaterialError::new("bounds_pad must be finite and non-negative"));
        }
        Ok(())
    }
}

/// What a program is built from: the base lane, the author's hooks, and
/// the engine's own overrides (the built-in kinds and derived variants).
pub struct ProgramRequest<'a> {
    /// The base lane's shader type default (the render crate's Pbr lane).
    pub base: ScriptObject,
    pub kind: BaseKind,
    pub hooks: &'a HookSet,
    pub mask: HookMask,
    /// Engine-owned methods to install after the hooks (compose, IBL).
    pub overrides: &'a [(LiveId, ScriptObject)],
}

/// Install the hooks on a fresh object whose prototype is the base lane and
/// check it through the shader front end (Metal lowering, which is the
/// strictest of the backends about types). Returns the object to apply to
/// the base lane's draw struct.
pub fn build_program(vm: &mut ScriptVm, req: &ProgramRequest) -> Result<ScriptObject, MaterialError> {
    let obj = install_program(vm, req)?;
    diagnose(vm, obj, req.hooks)?;
    Ok(obj)
}

/// [`build_program`] without the front-end check: for a host that compiles
/// the object right away and calls [`diagnose`] only when that fails, so a
/// good material is lowered once, not twice.
pub fn install_program(vm: &mut ScriptVm, req: &ProgramRequest) -> Result<ScriptObject, MaterialError> {
    req.hooks.validate(vm, req.mask)?;
    if req.kind == BaseKind::Unlit && (req.hooks.has(Hook::Light) || req.hooks.has(Hook::Lighting)) {
        return Err(MaterialError::new("an Unlit material has no lighting to hook: use a Pbr base for `light` or `lighting`"));
    }
    let obj = vm.bx.heap.new_with_proto_no_vec(req.base.into());
    let installs = req.hooks.iter().map(|(h, f)| (LiveId::from_str(h.spec().method), f, Some(h)))
        .chain(req.overrides.iter().map(|(id, f)| (*id, *f, None)));
    for (method, function, hook) in installs {
        let result = vm.bx.heap.set_value(obj, method.into(), function.into(), NoTrap);
        let installed = vm.bx.heap.object_method(obj, method.into(), NoTrap).as_object() == Some(function);
        if !result.is_nil() || !installed {
            let message = format!("`{method}` could not be installed on the base shader");
            return Err(match hook {
                Some(h) => MaterialError::hook(h, message),
                None => MaterialError::new(message),
            });
        }
    }
    Ok(obj)
}

/// The front end's errors for a program, located and attributed to hooks.
pub fn diagnose(vm: &mut ScriptVm, obj: ScriptObject, hooks: &HookSet) -> Result<(), MaterialError> {
    let errors = frontend_errors(vm, obj);
    if errors.is_empty() {
        return Ok(());
    }
    let mut diagnostics: Vec<Diagnostic> = errors.iter().map(|e| parse_diagnostic(e)).collect();
    attribute_hooks(vm, hooks, &mut diagnostics);
    Err(MaterialError { diagnostics })
}

/// The shader front end's messages for a draw shader object (empty = it
/// lowers). The same lowering a pipeline build runs, without building one.
pub fn frontend_errors(vm: &mut ScriptVm, io_self: ScriptObject) -> Vec<String> {
    frontend_errors_for(vm, io_self, ShaderBackend::Metal)
}

/// [`frontend_errors`] lowering for one backend.
pub fn frontend_errors_for(vm: &mut ScriptVm, io_self: ScriptObject, backend: ShaderBackend) -> Vec<String> {
    let mut output = ShaderOutput::default();
    output.backend = backend;
    output.use_vulkan = false;
    output.pre_collect_rust_instance_io(vm, io_self);
    output.pre_collect_shader_io(vm, io_self);
    for (entry, mode) in [(id!(vertex), ShaderMode::Vertex), (id!(fragment), ShaderMode::Fragment)] {
        let Some(fnobj) = vm.bx.heap.object_method(io_self, entry.into(), NoTrap).as_object() else { continue };
        output.mode = mode;
        ShaderFnCompiler::compile_shader_def(vm, &mut output, NoTrap, entry, fnobj, ShaderType::IoSelf(io_self), vec![]);
    }
    if output.has_errors {
        let report = output.error_report();
        report.lines().map(|l| l.to_string()).filter(|l| !l.trim().is_empty()).collect()
    } else {
        Vec::new()
    }
}

/// Name the hook each located diagnostic falls in: the hook whose function
/// body spans the error's line in the same file.
fn attribute_hooks(vm: &ScriptVm, hooks: &HookSet, diagnostics: &mut [Diagnostic]) {
    let spans: Vec<(Hook, String, u32)> = hooks.iter().filter_map(|(hook, function)| {
        let makepad_draw::makepad_platform::makepad_script::ScriptFnPtr::Script(ip) = vm.bx.heap.as_fn(function)? else { return None };
        let loc = vm.bx.code.ip_to_loc(ip)?;
        Some((hook, loc.file, loc.line))
    }).collect();
    for d in diagnostics.iter_mut().filter(|d| d.hook.is_none()) {
        let Some(file) = &d.file else { continue };
        // The nearest hook starting at or before the error's line.
        d.hook = spans.iter().filter(|(_, f, line)| f == file && *line <= d.line).max_by_key(|(_, _, line)| *line).map(|(h, _, _)| *h);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn located_messages_become_spans_and_compiler_origins_are_dropped() {
        let d = parse_diagnostic("undefined function missing_fn (shader_calls.rs:812) at game://level.splash:14:22");
        assert_eq!(d.file.as_deref(), Some("game://level.splash"));
        assert_eq!((d.line, d.col), (14, 22));
        assert_eq!(d.message, "undefined function missing_fn");
        let plain = parse_diagnostic("shader loops too costly");
        assert_eq!(plain.file, None);
        assert_eq!(plain.message, "shader loops too costly");
        let windows = parse_diagnostic("bad (x.rs:1) at C:/docs/a.splash:3:4");
        assert_eq!(windows.file.as_deref(), Some("C:/docs/a.splash"));
        assert_eq!((windows.line, windows.col), (3, 4));
    }

    #[test]
    fn errors_print_with_their_hook_and_line() {
        let e = MaterialError { diagnostics: vec![Diagnostic { message: "type mismatch".into(), file: Some("doc.splash".into()), line: 7, col: 3, hook: Some(Hook::Finish) }] };
        assert_eq!(e.to_string(), "finish: type mismatch (doc.splash:7:3)");
    }
}
