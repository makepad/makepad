use crate::{
    gpu::{self, Pipeline},
    Diagnostic, Document, Mode, State,
};
use makepad_hal::{CmdWriter, Id};
use makepad_script::{
    host::ScriptVmHost, id, LiveId, NoTrap, ScriptHeap, ScriptMod, ScriptObject,
    ScriptValue, ScriptVm, ScriptVmBase,
};
use std::collections::HashMap;

pub const ERROR_BAND: usize = 0;
pub const GLYPH: usize = 1;
pub const FIELD: usize = 2;
pub const ICON: usize = 3;
/// the layer pipe's Id in its bank (its index in `pipes` is `Program::layer`)
const LAYER_SLOT: u32 = 4;
pub const PIPE_BANK_SIZE: u32 = 8;

#[derive(Clone, Copy)]
pub struct Layout {
    pub card_padding: f32,
    pub action_height: f32,
    pub action_gap: f32,
    pub action_width: f32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Role {
    App,
    Status,
    Current,
    Location,
    Domain,
    Errors,
    Panic,
    Hints,
}

#[derive(Clone, Copy)]
pub struct Line {
    pub role: Role,
    pub size: f32,
    pub height: f32,
    pub gap: f32,
    pub color: [f32; 4],
    pub align: f32,
}

pub struct Program {
    pub pipes: Vec<Pipeline>,
    /// the index of the `layer` pipe in `pipes` (the running app's frame under the window
    /// layer); a document without it draws no layer
    pub layer: Option<usize>,
    /// the document has the `icon` pipe (at ICON)
    pub icon: bool,
    pub lines: Vec<Line>,
    boot: Layout,
    overlay: Layout,
    hints: HashMap<LiveId, String>,
    pub path: std::path::PathBuf,
}

/// A key's value; an absent key is nil (the heap answers a lookup failure as an error value).
fn get(heap: &ScriptHeap, object: ScriptObject, key: LiveId) -> ScriptValue {
    let value = heap.value(object, key.into(), NoTrap);
    if value.is_err() { ScriptValue::NIL } else { value }
}

fn object(heap: &ScriptHeap, parent: ScriptObject, key: LiveId) -> Result<ScriptObject, String> {
    get(heap, parent, key)
        .as_object()
        .ok_or_else(|| format!("Boot document needs {key}"))
}

fn positive(
    heap: &ScriptHeap,
    object: ScriptObject,
    key: LiveId,
    zero: bool,
) -> Result<f32, String> {
    gpu::number(get(heap, object, key))
        .map(|v| v as f32)
        .filter(|v| v.is_finite() && if zero { *v >= 0.0 } else { *v > 0.0 })
        .ok_or_else(|| {
            format!(
                "Boot document {key} must be a finite {}number",
                if zero { "nonnegative " } else { "positive " }
            )
        })
}

fn layout(heap: &ScriptHeap, root: ScriptObject, name: LiveId) -> Result<Layout, String> {
    let obj = object(heap, root, name)?;
    Ok(Layout {
        card_padding: optional_number(heap, obj, id!(card_padding), 12.0)?,
        action_height: optional_number(heap, obj, id!(action_height), 48.0)?.max(44.0),
        action_gap: optional_number(heap, obj, id!(action_gap), 8.0)?,
        action_width: optional_number(heap, obj, id!(action_width), 152.0)?.max(1.0),
    })
}

fn optional_number(
    heap: &ScriptHeap,
    obj: ScriptObject,
    key: LiveId,
    fallback: f32,
) -> Result<f32, String> {
    if get(heap, obj, key).is_nil() {
        Ok(fallback)
    } else {
        positive(heap, obj, key, true)
    }
}

fn errors(vm: &mut ScriptVm, phase: &'static str) -> Vec<Diagnostic> {
    vm.drain_errors();
    vm.bx
        .captured_errors
        .take()
        .unwrap_or_default()
        .into_iter()
        .map(|error| {
            let (file, line, col) = error.loc().map_or((String::new(), 0, 0), |loc| {
                (loc.file, loc.line + 1, loc.col + 1)
            });
            Diagnostic::new(phase, file, line, col, error.message())
        })
        .collect()
}

impl Program {
    pub fn compile(
        document: &Document,
        dialect: u32,
        bank: u32,
        generation: u32,
    ) -> Result<Self, Vec<Diagnostic>> {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = ScriptVm {
            host: host.host_context(),
            bx: Box::new(ScriptVmBase::new()),
        };
        vm.bx.captured_errors = Some(Vec::new());
        makepad_script::glyph_coverage::script_mod(&mut vm);
        let value = vm.eval(ScriptMod::new(
            document
                .path
                .parent()
                .and_then(|p| p.to_str())
                .unwrap_or(""),
            "relax_boot_document",
            &document.path.to_string_lossy(),
            0,
            0,
            &document.source,
            &[],
        ));
        let mut diagnostics = errors(&mut vm, "parse");
        if !diagnostics.is_empty() {
            return Err(diagnostics);
        }
        vm.bx.captured_errors = Some(Vec::new());
        let parsed = (|| -> Result<_, String> {
            let root = value
                .as_object()
                .ok_or("Boot document must return a text/pass object")?;
            let boot = layout(vm.heap(), root, id!(boot))?;
            let overlay = layout(vm.heap(), root, id!(overlay))?;
            let mut lines = Vec::new();
            let entries = get(vm.heap(), root, id!(lines))
                .as_array()
                .ok_or("Boot document needs a lines array")?;
            for i in 0..vm.heap().array_len(entries) {
                let line = vm
                    .heap()
                    .array_index(entries, i, NoTrap)
                    .as_object()
                    .ok_or("Each boot line must be an object")?;
                let role = match get(vm.heap(), line, id!(role)).as_id() {
                    Some(v) if v == id!(app) => Role::App,
                    Some(v) if v == id!(status) => Role::Status,
                    Some(v) if v == id!(current) => Role::Current,
                    Some(v) if v == id!(location) => Role::Location,
                    Some(v) if v == id!(domain) => Role::Domain,
                    Some(v) if v == id!(errors) => Role::Errors,
                    Some(v) if v == id!(panic) => Role::Panic,
                    Some(v) if v == id!(hints) => Role::Hints,
                    _ => return Err("Unknown boot line role".into()),
                };
                lines.push(Line {
                    role,
                    size: positive(vm.heap(), line, id!(size), false)?,
                    height: positive(vm.heap(), line, id!(height), false)?,
                    gap: positive(vm.heap(), line, id!(gap), true)?,
                    color: gpu::color(vm.heap(), get(vm.heap(), line, id!(color))),
                    align: optional_number(vm.heap(), line, id!(align), 0.0)?.min(1.0),
                });
            }
            for role in [
                Role::App,
                Role::Status,
                Role::Domain,
                Role::Errors,
                Role::Panic,
                Role::Hints,
            ] {
                if lines.iter().filter(|line| line.role == role).count() != 1 {
                    return Err("Boot document needs each line role exactly once".into());
                }
            }
            if lines.last().map(|line| line.role) != Some(Role::Hints) {
                return Err("Boot document key hints must be the last line".into());
            }
            let mut hints = HashMap::new();
            let obj = object(vm.heap(), root, id!(hints))?;
            for key in [
                id!(restart),
                id!(retry),
                id!(copy),
                id!(open),
                id!(dismiss),
                id!(errors),
                id!(scroll),
                id!(return_value),
                id!(submit),
            ] {
                let value = get(vm.heap(), obj, key);
                let text = vm
                    .string_with(value, |_, text| text.to_owned())
                    .ok_or_else(|| format!("Boot document needs hint {key}"))?;
                hints.insert(key, text);
            }
            for (key, fallback) in [
                (id!(tap_restart), "Restart"),
                (id!(tap_copy), "Copy"),
                (id!(tap_open), "Open on dev machine"),
                (id!(tap_dismiss), "Dismiss"),
                (id!(tap_submit), "Submit"),
                (id!(tap_cancel), "Cancel"),
                (id!(tap_resume), "Resume"),
                (id!(input_label), "Return value"),
                (id!(paused), "Paused after a panic"),
                (id!(halted), "Compile halted"),
                (id!(follow_on), "follow-on"),
                (id!(root_cause), "root cause"),
                (id!(resolve), "Resolving dependencies"),
                (id!(parse), "Reading source"),
                (id!(typecheck), "Checking types"),
                (id!(codegen), "Compiling"),
                (id!(link), "Linking"),
                (id!(first_frame), "Opening your app"),
            ] {
                let value = get(vm.heap(), obj, key);
                let label = if value.is_nil() {
                    fallback.to_owned()
                } else {
                    vm.string_with(value, |_, text| text.to_owned())
                        .ok_or_else(|| format!("Boot label {key} must be text"))?
                };
                hints.insert(key, label);
            }
            let mut pipes = Vec::new();
            for (index, key) in [id!(error_band), id!(glyph), id!(ignition)]
                .into_iter()
                .enumerate()
            {
                let shader = object(vm.heap(), root, key)?;
                pipes.push(Pipeline::compile(
                    &mut vm,
                    shader,
                    dialect,
                    Id::new(bank + index as u32, generation),
                )?);
            }
            if !get(vm.heap(), root, id!(icon)).is_nil() {
                let shader = object(vm.heap(), root, id!(icon))?;
                pipes.push(Pipeline::compile(
                    &mut vm,
                    shader,
                    dialect,
                    Id::new(bank + ICON as u32, generation),
                )?);
            }
            let icon = pipes.len() > ICON;
            let mut layer = None;
            if !get(vm.heap(), root, id!(layer)).is_nil() {
                let shader = object(vm.heap(), root, id!(layer))?;
                layer = Some(pipes.len());
                pipes.push(Pipeline::compile(
                    &mut vm,
                    shader,
                    dialect,
                    Id::new(bank + LAYER_SLOT, generation),
                )?);
            }
            Ok((
                boot,
                overlay,
                lines,
                hints,
                pipes,
                layer,
                icon,
            ))
        })();
        diagnostics.extend(errors(&mut vm, "codegen"));
        let (boot, overlay, lines, hints, pipes, layer, icon) =
            match parsed {
                Ok(parsed) if diagnostics.is_empty() => parsed,
                Ok(_) => return Err(diagnostics),
                Err(message) => {
                    diagnostics.push(Diagnostic::new(
                        "codegen",
                        document.path.to_string_lossy(),
                        1,
                        1,
                        message,
                    ));
                    return Err(diagnostics);
                }
            };
        Ok(Self {
            pipes,
            layer,
            icon,
            lines,
            boot,
            overlay,
            hints,
            path: document.path.clone(),
        })
    }

    pub fn layout(&self, mode: Mode) -> Layout {
        if mode == Mode::Overlay {
            self.overlay
        } else {
            self.boot
        }
    }

    pub fn label(&self, key: LiveId) -> String {
        self.hints[&key].clone()
    }

    pub fn line(&self, role: Role) -> Line {
        *self
            .lines
            .iter()
            .find(|l| l.role == role)
            .unwrap_or_else(|| self.lines.iter().find(|l| l.role == Role::Status).unwrap())
    }

    pub fn hints(&self, state: &State, input: Option<&str>) -> String {
        if input.is_some() {
            // The value is laid out once in the scrollable editor; keep its
            // submit/cancel hints reachable even for a long return expression.
            return self.hints[&id!(submit)].clone();
        }
        let mut hints = Vec::new();
        if state.can_retry {
            hints.push(self.hints[&id!(retry)].clone());
        } else if state.can_restart {
            hints.push(self.hints[&id!(restart)].clone());
        }
        if !state.diagnostics.is_empty() || state.parked.is_some() {
            hints.push(self.hints[&id!(copy)].clone());
        }
        if !state.diagnostics.is_empty() {
            hints.push(self.hints[&id!(open)].clone());
            hints.push(self.hints[&id!(errors)].clone());
        }
        if let Some(panic) = &state.parked {
            hints.extend(
                panic
                    .choices
                    .iter()
                    .enumerate()
                    .map(|(i, choice)| format!("{} {choice}", i + 1)),
            );
        }
        hints.push(self.hints[&id!(scroll)].clone());
        if state.mode == Mode::Overlay {
            hints.push(self.hints[&id!(dismiss)].clone());
        }
        hints.join(" · ")
    }

    pub fn create(&self, writer: &mut CmdWriter, dialect: u32) {
        for pipe in &self.pipes {
            pipe.create(writer, dialect);
        }
    }

    pub fn destroy(&self, writer: &mut CmdWriter) {
        for pipe in &self.pipes {
            writer.pipeline_destroy(pipe.id);
        }
    }

    pub fn ready(&self) -> bool {
        self.pipes.iter().all(|pipe| pipe.ready)
    }
}

#[cfg(test)]
mod tests {
    /// The bundled document compiles for every dialect, with the window layer's pipe.
    #[test]
    fn bundled_document_compiles_with_its_layer() {
        for dialect in [makepad_hal::dialect::MSL, makepad_hal::dialect::SPIRV, makepad_hal::dialect::HLSL] {
            let program = super::Program::compile(&crate::Document::bundled(), dialect, 0, 1)
                .unwrap_or_else(|e| panic!("{:?}", e.iter().map(|d| d.message.clone()).collect::<Vec<_>>()));
            assert!(program.layer.is_some() && program.icon);
        }
    }
}
