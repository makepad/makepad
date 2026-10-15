use crate::{
    gpu::{self, Pipeline},
    Diagnostic, Document, Mode, State,
};
use makepad_hal::{CmdWriter, Id};
use makepad_script::{
    host::ScriptVmHost, id, LiveId, NoTrap, ScriptHeap, ScriptMod, ScriptObject, ScriptObjectRef,
    ScriptValue, ScriptVm, ScriptVmBase,
};
use std::collections::HashMap;

pub const BACKGROUND: usize = 0;
pub const PROGRESS: usize = 1;
pub const ERROR_BAND: usize = 2;
pub const GLYPH: usize = 3;
pub const ICON: usize = 4;
pub const PIPE_BANK_SIZE: u32 = 8;

#[derive(Clone, Copy)]
pub struct Layout {
    pub margin: f32,
    pub padding: f32,
    pub top: f32,
    pub max_width: f32,
    pub max_height: f32,
    pub center: f32,
    pub columns: f32,
    pub icon_size: f32,
    pub card_padding: f32,
    pub action_height: f32,
    pub action_gap: f32,
    pub action_width: f32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Role {
    App,
    Progress,
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
    pub light_color: [f32; 4],
    pub align: f32,
}

pub struct Program {
    root: ScriptObjectRef,
    vm: Option<Box<ScriptVmBase>>,
    pub pipes: Vec<Pipeline>,
    pub lines: Vec<Line>,
    boot: Layout,
    overlay: Layout,
    landscape: Option<Layout>,
    landscape_height: f32,
    pub settle_seconds: f32,
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
        margin: positive(heap, obj, id!(margin), true)?,
        padding: positive(heap, obj, id!(padding), true)?,
        top: positive(heap, obj, id!(top), true)?,
        max_width: positive(heap, obj, id!(max_width), true)?,
        max_height: positive(heap, obj, id!(max_height), true)?,
        center: optional_number(heap, obj, id!(center), 0.0)?,
        columns: optional_number(heap, obj, id!(column_split), 0.0)?,
        icon_size: optional_number(heap, obj, id!(icon_size), 72.0)?,
        card_padding: optional_number(heap, obj, id!(card_padding), 16.0)?,
        action_height: optional_number(heap, obj, id!(action_height), 52.0)?.max(48.0),
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
            let root_ref = vm.heap_mut().new_object_ref(root);
            let boot = layout(vm.heap(), root, id!(boot))?;
            let overlay = layout(vm.heap(), root, id!(overlay))?;
            let landscape = if get(vm.heap(), root, id!(landscape)).is_nil() {
                None
            } else {
                Some(layout(vm.heap(), root, id!(landscape))?)
            };
            let landscape_height = optional_number(vm.heap(), root, id!(landscape_height), 540.0)?;
            let settle_seconds = optional_number(vm.heap(), root, id!(settle_seconds), 1.5)?;
            for name in [id!(phase_target), id!(ease)] {
                if !vm.heap().is_fn(object(vm.heap(), root, name)?) {
                    return Err(format!("Boot document {name} must be a function"));
                }
            }
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
                    Some(v) if v == id!(progress) => Role::Progress,
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
                    size: if matches!(role, Role::Progress) {
                        0.0
                    } else {
                        positive(vm.heap(), line, id!(size), false)?
                    },
                    height: positive(vm.heap(), line, id!(height), false)?,
                    gap: positive(vm.heap(), line, id!(gap), true)?,
                    color: gpu::color(vm.heap(), get(vm.heap(), line, id!(color))),
                    light_color: gpu::color(vm.heap(), {
                        let light = get(vm.heap(), line, id!(light_color));
                        if light.is_nil() {
                            get(vm.heap(), line, id!(color))
                        } else {
                            light
                        }
                    }),
                    align: optional_number(vm.heap(), line, id!(align), 0.0)?.min(1.0),
                });
            }
            for role in [
                Role::App,
                Role::Progress,
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
            for (index, key) in [id!(background), id!(progress), id!(error_band), id!(glyph)]
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
            Ok((
                root_ref,
                boot,
                overlay,
                landscape,
                landscape_height,
                settle_seconds,
                lines,
                hints,
                pipes,
            ))
        })();
        diagnostics.extend(errors(&mut vm, "codegen"));
        let (root, boot, overlay, landscape, landscape_height, settle_seconds, lines, hints, pipes) =
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
        let mut program = Self {
            vm: Some(vm.bx),
            root,
            pipes,
            lines,
            boot,
            overlay,
            landscape,
            landscape_height,
            settle_seconds,
            hints,
            path: document.path.clone(),
        };
        program.call(id!(phase_target), &[0.0, 0.0, 1.0, 6.0, 0.0, 0.0, 0.0])?;
        program.call(id!(ease), &[0.0, 0.0, 0.0])?;
        Ok(program)
    }

    pub fn layout(&self, mode: Mode, size: [f32; 2]) -> Layout {
        if mode == Mode::Overlay {
            self.overlay
        } else if size[0] > size[1] && size[1] < self.landscape_height {
            self.landscape.unwrap_or(self.boot)
        } else {
            self.boot
        }
    }

    pub fn label(&self, key: LiveId) -> String {
        self.hints[&key].clone()
    }

    pub fn line(&self, role: Role, dark: bool) -> Line {
        let mut line = *self
            .lines
            .iter()
            .find(|l| l.role == role)
            .unwrap_or_else(|| self.lines.iter().find(|l| l.role == Role::Status).unwrap());
        if !dark {
            line.color = line.light_color;
        }
        line
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

    pub fn call(&mut self, name: LiveId, args: &[f64]) -> Result<f32, Vec<Diagnostic>> {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = ScriptVm {
            host: host.host_context(),
            bx: self.vm.take().expect("boot VM"),
        };
        vm.bx.captured_errors = Some(Vec::new());
        let function = get(vm.heap(), self.root.as_object(), name);
        let args: Vec<ScriptValue> = args.iter().map(|v| ScriptValue::from_f64(*v)).collect();
        let value = vm.call(function, &args);
        let mut diagnostics = errors(&mut vm, "run");
        let value = gpu::number(value)
            .map(|v| v as f32)
            .filter(|v| v.is_finite());
        if value.is_none() {
            diagnostics.push(Diagnostic::new(
                "run",
                self.path.to_string_lossy(),
                1,
                1,
                format!("Boot function {name} must return a finite number"),
            ));
        }
        vm.bx.gc_if_needed();
        self.vm = Some(vm.bx);
        if diagnostics.is_empty() {
            Ok(value.unwrap())
        } else {
            Err(diagnostics)
        }
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
