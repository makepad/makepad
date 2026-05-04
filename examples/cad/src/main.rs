pub use makepad_ai;
pub use makepad_code_editor;
pub use makepad_csg;
pub use makepad_widgets;
pub use makepad_xr;

use makepad_ai::*;
use makepad_code_editor::{
    code_editor::{CodeEditorAction, KeepCursorInView},
    decoration::DecorationSet,
    CodeDocument, CodeEditor, CodeSession,
};
use makepad_csg::Solid;
use makepad_widgets::makepad_platform::{makepad_script::ScriptVmBase, thread::SignalToUI};
use makepad_widgets::*;
use makepad_xr::scene::*;
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Instant;

app_main!(App);

const DEFAULT_CAD_SCRIPT: &str = r#"let base = cube(3.0, 1.1, 1.6, true)
let bore_x = cylinder(0.28, 4.0, 40, true).rotate_z(90.0)
let bore_z = cylinder(0.20, 2.2, 32, true).rotate_x(90.0)
let shell = base.difference(bore_x).difference(bore_z)

let left_boss = cylinder(0.44, 0.36, 40, true)
    .translate(-1.05, 0.0, 0.0)
let right_boss = cylinder(0.44, 0.36, 40, true)
    .translate(1.05, 0.0, 0.0)
let top_rib = cube(2.0, 0.24, 0.30, true)
    .translate(0.0, 0.55, 0.0)

render(shell.merge(left_boss).merge(right_boss).merge(top_rib))"#;

const LIVE_UPDATE_INTERVAL: f64 = 0.08;
const LOCAL_OPENAI_URL: &str = "http://10.0.0.168:8080/v1/chat/completions";
const LOCAL_OPENAI_MODEL: &str = "Gemma4Unlim-31B-ModelOptFullAttn-FullCal128.gguf";
const GENERATED_DIR: &str = "generated";
const GENERATED_SCRIPT_FILE: &str = "current.cad";
const GENERATED_OBJ_FILE: &str = "current.obj";
const DEMO_MAX_CURVE_SEGMENTS: u32 = 16;
const DEMO_MAX_SPHERE_RINGS: u32 = 12;
const DEMO_MAX_TORUS_MINOR_SEGMENTS: u32 = 8;

thread_local! {
    static CAD_SCRIPT_OUTPUT: RefCell<Option<Solid>> = RefCell::new(None);
}

fn cad_manifest_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn cad_generated_dir_path() -> PathBuf {
    cad_manifest_path().join(GENERATED_DIR)
}

fn cad_generated_script_path() -> PathBuf {
    cad_generated_dir_path().join(GENERATED_SCRIPT_FILE)
}

fn cad_generated_obj_path() -> PathBuf {
    cad_generated_dir_path().join(GENERATED_OBJ_FILE)
}

fn load_saved_cad_script() -> Option<String> {
    fs::read_to_string(cad_generated_script_path())
        .ok()
        .filter(|source| !source.trim().is_empty())
}

fn save_cad_state(source: &str, solid: &Solid) -> Result<(), String> {
    let dir = cad_generated_dir_path();
    fs::create_dir_all(&dir)
        .map_err(|err| format!("could not create generated directory: {err}"))?;
    fs::write(dir.join(GENERATED_SCRIPT_FILE), source)
        .map_err(|err| format!("could not save CAD script: {err}"))?;
    solid
        .write_obj(cad_generated_obj_path().to_string_lossy().as_ref())
        .map_err(|err| format!("could not save OBJ mesh: {err}"))?;
    Ok(())
}

fn set_cad_script_output(solid: Solid) {
    CAD_SCRIPT_OUTPUT.with(|slot| {
        *slot.borrow_mut() = Some(solid);
    });
}

fn clear_cad_script_output() {
    CAD_SCRIPT_OUTPUT.with(|slot| {
        *slot.borrow_mut() = None;
    });
}

fn take_cad_script_output() -> Option<Solid> {
    CAD_SCRIPT_OUTPUT.with(|slot| slot.borrow_mut().take())
}

#[derive(Clone, Debug)]
struct CadSolidHandle {
    solid: Solid,
}

impl ScriptHandleGc for CadSolidHandle {
    fn gc(&mut self) {}

    fn debug_fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "cad_solid(vertices={}, triangles={})",
            self.solid.vertex_count(),
            self.solid.triangle_count()
        )
    }
}

fn new_cad_solid(vm: &mut ScriptVm, solid: Solid) -> ScriptValue {
    let ty = vm.handle_type(id!(cad_solid));
    vm.bx
        .heap
        .new_handle(ty, Box::new(CadSolidHandle { solid }))
        .into()
}

fn solid_from_value(vm: &mut ScriptVm, value: ScriptValue) -> Option<Solid> {
    let handle = value.as_handle()?;
    vm.downcast_handle_gc::<CadSolidHandle>(handle)
        .map(|handle| handle.solid.clone())
}

fn args_value(vm: &mut ScriptVm, args: ScriptObject, index: usize) -> ScriptValue {
    vm.bx
        .heap
        .vec_value(args, index, vm.bx.threads.cur_ref().trap.pass())
}

fn self_value(vm: &mut ScriptVm, args: ScriptObject) -> ScriptValue {
    vm.bx
        .heap
        .value(args, id!(self).into(), vm.bx.threads.cur_ref().trap.pass())
}

fn arg_f64(vm: &mut ScriptVm, args: ScriptObject, index: usize, default: f64) -> f64 {
    let value = args_value(vm, args, index);
    if value.is_nil() {
        default
    } else {
        value.as_number().unwrap_or(default)
    }
}

fn arg_u32(vm: &mut ScriptVm, args: ScriptObject, index: usize, default: u32) -> u32 {
    let value = args_value(vm, args, index);
    if value.is_nil() {
        default
    } else {
        value
            .as_number()
            .map(|value| value as u32)
            .unwrap_or(default)
    }
}

fn arg_bool(vm: &mut ScriptVm, args: ScriptObject, index: usize, default: bool) -> bool {
    let value = args_value(vm, args, index);
    if value.is_nil() {
        default
    } else {
        vm.bx.heap.cast_to_bool(value)
    }
}

fn arg_solid(vm: &mut ScriptVm, args: ScriptObject, index: usize) -> Option<Solid> {
    let value = args_value(vm, args, index);
    solid_from_value(vm, value)
}

fn self_solid(vm: &mut ScriptVm, args: ScriptObject) -> Option<Solid> {
    let value = self_value(vm, args);
    solid_from_value(vm, value)
}

fn cad_unary_method(
    vm: &mut ScriptVm,
    args: ScriptObject,
    op: impl FnOnce(Solid, &mut ScriptVm, ScriptObject) -> Solid,
) -> ScriptValue {
    let Some(solid) = self_solid(vm, args) else {
        return NIL;
    };
    let solid = op(solid, vm, args);
    new_cad_solid(vm, solid)
}

fn cad_binary_method(
    vm: &mut ScriptVm,
    args: ScriptObject,
    op: impl FnOnce(&Solid, &Solid) -> Solid,
) -> ScriptValue {
    let Some(left) = self_solid(vm, args) else {
        return NIL;
    };
    let Some(right) = arg_solid(vm, args, 0) else {
        return NIL;
    };
    new_cad_solid(vm, op(&left, &right))
}

fn install_cad_binary_function(
    vm: &mut ScriptVm,
    module: ScriptObject,
    name: LiveId,
    op: fn(&Solid, &Solid) -> Solid,
) {
    vm.add_method(module, name, script_args!(), move |vm, args| {
        let Some(left) = arg_solid(vm, args, 0) else {
            return NIL;
        };
        let Some(right) = arg_solid(vm, args, 1) else {
            return NIL;
        };
        new_cad_solid(vm, op(&left, &right))
    });
}

fn install_cad_binary_handle_method(
    vm: &mut ScriptVm,
    ty: ScriptHandleType,
    name: LiveId,
    op: fn(&Solid, &Solid) -> Solid,
) {
    vm.add_handle_method(ty, name, script_args!(), move |vm, args| {
        cad_binary_method(vm, args, op)
    });
}

fn cad_script_mod(vm: &mut ScriptVm) -> ScriptValue {
    let ty = vm.new_handle_type(id!(cad_solid));
    let cad = vm.new_module(id!(cad));

    vm.add_method(cad, id!(empty), script_args!(), |vm, _args| {
        new_cad_solid(vm, Solid::empty())
    });
    vm.add_method(cad, id!(cube), script_args!(), |vm, args| {
        let sx = arg_f64(vm, args, 0, 1.0).abs().max(0.001);
        let sy = arg_f64(vm, args, 1, 1.0).abs().max(0.001);
        let sz = arg_f64(vm, args, 2, 1.0).abs().max(0.001);
        let center = arg_bool(vm, args, 3, true);
        new_cad_solid(vm, Solid::cube(sx, sy, sz, center))
    });
    vm.add_method(cad, id!(cube_uniform), script_args!(), |vm, args| {
        let size = arg_f64(vm, args, 0, 1.0).abs().max(0.001);
        let center = arg_bool(vm, args, 1, true);
        new_cad_solid(vm, Solid::cube_uniform(size, center))
    });
    vm.add_method(cad, id!(sphere), script_args!(), |vm, args| {
        let radius = arg_f64(vm, args, 0, 1.0).abs().max(0.001);
        let segments = arg_u32(vm, args, 1, 32).clamp(8, DEMO_MAX_CURVE_SEGMENTS);
        let rings = arg_u32(vm, args, 2, 16).clamp(4, DEMO_MAX_SPHERE_RINGS);
        new_cad_solid(vm, Solid::sphere(radius, segments, rings))
    });
    vm.add_method(cad, id!(cylinder), script_args!(), |vm, args| {
        let radius = arg_f64(vm, args, 0, 1.0).abs().max(0.001);
        let height = arg_f64(vm, args, 1, 1.0).abs().max(0.001);
        let segments = arg_u32(vm, args, 2, 32).clamp(3, DEMO_MAX_CURVE_SEGMENTS);
        let center = arg_bool(vm, args, 3, true);
        new_cad_solid(vm, Solid::cylinder(radius, height, segments, center))
    });
    vm.add_method(cad, id!(cone), script_args!(), |vm, args| {
        let radius = arg_f64(vm, args, 0, 1.0).abs().max(0.001);
        let height = arg_f64(vm, args, 1, 1.0).abs().max(0.001);
        let segments = arg_u32(vm, args, 2, 32).clamp(3, DEMO_MAX_CURVE_SEGMENTS);
        let center = arg_bool(vm, args, 3, true);
        new_cad_solid(vm, Solid::cone(radius, height, segments, center))
    });
    vm.add_method(cad, id!(torus), script_args!(), |vm, args| {
        let major_radius = arg_f64(vm, args, 0, 1.0).abs().max(0.001);
        let minor_radius = arg_f64(vm, args, 1, 0.2).abs().max(0.001);
        let major_segments = arg_u32(vm, args, 2, 48).clamp(3, DEMO_MAX_CURVE_SEGMENTS);
        let minor_segments = arg_u32(vm, args, 3, 16).clamp(3, DEMO_MAX_TORUS_MINOR_SEGMENTS);
        new_cad_solid(
            vm,
            Solid::torus(major_radius, minor_radius, major_segments, minor_segments),
        )
    });
    vm.add_method(cad, id!(tapered_cylinder), script_args!(), |vm, args| {
        let r1 = arg_f64(vm, args, 0, 1.0).abs();
        let r2 = arg_f64(vm, args, 1, 0.5).abs();
        let height = arg_f64(vm, args, 2, 1.0).abs().max(0.001);
        let segments = arg_u32(vm, args, 3, 32).clamp(3, DEMO_MAX_CURVE_SEGMENTS);
        let center = arg_bool(vm, args, 4, true);
        new_cad_solid(
            vm,
            Solid::tapered_cylinder(r1, r2, height, segments, center),
        )
    });
    vm.add_method(cad, id!(render), script_args!(), |vm, args| {
        let Some(solid) = arg_solid(vm, args, 0) else {
            return NIL;
        };
        set_cad_script_output(solid.clone());
        new_cad_solid(vm, solid)
    });
    vm.add_method(cad, id!(preview), script_args!(), |vm, args| {
        let Some(solid) = arg_solid(vm, args, 0) else {
            return NIL;
        };
        set_cad_script_output(solid.clone());
        new_cad_solid(vm, solid)
    });

    install_cad_binary_function(vm, cad, id!(merge), Solid::merge);
    install_cad_binary_function(vm, cad, id!(union), Solid::union);
    install_cad_binary_function(vm, cad, id!(difference), Solid::difference);
    install_cad_binary_function(vm, cad, id!(intersection), Solid::intersection);

    install_cad_binary_handle_method(vm, ty, id!(merge), Solid::merge);
    install_cad_binary_handle_method(vm, ty, id!(union), Solid::union);
    install_cad_binary_handle_method(vm, ty, id!(difference), Solid::difference);
    install_cad_binary_handle_method(vm, ty, id!(intersection), Solid::intersection);

    vm.add_handle_method(ty, id!(translate), script_args!(), |vm, args| {
        cad_unary_method(vm, args, |solid, vm, args| {
            solid.translate(
                arg_f64(vm, args, 0, 0.0),
                arg_f64(vm, args, 1, 0.0),
                arg_f64(vm, args, 2, 0.0),
            )
        })
    });
    vm.add_handle_method(ty, id!(rotate_x), script_args!(), |vm, args| {
        cad_unary_method(vm, args, |solid, vm, args| {
            solid.rotate_x(arg_f64(vm, args, 0, 0.0))
        })
    });
    vm.add_handle_method(ty, id!(rotate_y), script_args!(), |vm, args| {
        cad_unary_method(vm, args, |solid, vm, args| {
            solid.rotate_y(arg_f64(vm, args, 0, 0.0))
        })
    });
    vm.add_handle_method(ty, id!(rotate_z), script_args!(), |vm, args| {
        cad_unary_method(vm, args, |solid, vm, args| {
            solid.rotate_z(arg_f64(vm, args, 0, 0.0))
        })
    });
    vm.add_handle_method(ty, id!(scale), script_args!(), |vm, args| {
        cad_unary_method(vm, args, |solid, vm, args| {
            solid.scale(
                arg_f64(vm, args, 0, 1.0),
                arg_f64(vm, args, 1, 1.0),
                arg_f64(vm, args, 2, 1.0),
            )
        })
    });
    vm.add_handle_method(ty, id!(scale_uniform), script_args!(), |vm, args| {
        cad_unary_method(vm, args, |solid, vm, args| {
            solid.scale_uniform(arg_f64(vm, args, 0, 1.0))
        })
    });
    vm.add_handle_method(ty, id!(render), script_args!(), |vm, args| {
        let Some(solid) = self_solid(vm, args) else {
            return NIL;
        };
        set_cad_script_output(solid.clone());
        new_cad_solid(vm, solid)
    });
    vm.add_handle_method(ty, id!(preview), script_args!(), |vm, args| {
        let Some(solid) = self_solid(vm, args) else {
            return NIL;
        };
        set_cad_script_output(solid.clone());
        new_cad_solid(vm, solid)
    });
    NIL
}

fn script_value_to_string(vm: &mut ScriptVm, value: ScriptValue) -> String {
    let mut out = String::new();
    let mut recur = Vec::new();
    vm.bx
        .heap
        .to_debug_string(value, &mut recur, &mut out, false, 0);
    if let Some(err) = value.as_err() {
        if let Some(loc) = vm.bx.code.ip_to_loc(err.ip) {
            if !out.is_empty() {
                return format!("{}: {}", loc, out);
            }
            return loc.to_string();
        }
    }
    out
}

fn line_let_identifier(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix("let ")?;
    let ident_len = rest
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
        .map(char::len_utf8)
        .sum::<usize>();
    if ident_len == 0 {
        return None;
    }
    let ident = &rest[..ident_len];
    let after_ident = rest[ident_len..].trim_start();
    after_ident.starts_with('=').then(|| ident.to_string())
}

fn line_has_output_call(line: &str) -> bool {
    line.contains("render(")
        || line.contains(".render(")
        || line.contains("preview(")
        || line.contains(".preview(")
}

fn is_standalone_output_call(line: &str) -> bool {
    let line = line.trim();
    (line.starts_with("render(")
        || line.starts_with("preview(")
        || line.ends_with(".render()")
        || line.ends_with(".preview()"))
        && !line.starts_with("let ")
}

fn source_without_intermediate_previews(source: &str) -> String {
    let last_output_line = source
        .lines()
        .enumerate()
        .filter_map(|(index, line)| is_standalone_output_call(line).then_some(index))
        .last();

    let Some(last_output_line) = last_output_line else {
        return source.to_string();
    };

    let mut output = String::new();
    for (index, line) in source.lines().enumerate() {
        if index != last_output_line && is_standalone_output_call(line) {
            continue;
        }
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(line);
    }
    if source.ends_with('\n') {
        output.push('\n');
    }
    output
}

fn progressive_cad_preview_source(source: &str) -> Option<String> {
    let mut depth = 0i32;
    let mut last_good_end = 0usize;
    let mut cursor = 0usize;

    for line in source.split_inclusive('\n') {
        cursor += line.len();
        for ch in line.chars() {
            match ch {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth -= 1,
                _ => {}
            }
        }
        if depth < 0 {
            break;
        }
        let trimmed = line.trim();
        if depth == 0
            && !trimmed.is_empty()
            && !trimmed.ends_with('.')
            && !trimmed.ends_with(',')
            && !trimmed.ends_with('=')
        {
            last_good_end = cursor;
        }
    }

    if last_good_end == 0 {
        return None;
    }

    let stripped = source_without_intermediate_previews(&source[..last_good_end]);
    let prefix = stripped.trim_end();
    if prefix.is_empty() {
        return None;
    }
    if prefix
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .is_some_and(line_has_output_call)
    {
        return Some(prefix.to_string());
    }

    let last_ident = prefix.lines().filter_map(line_let_identifier).last()?;
    Some(format!("{}\npreview({})", prefix, last_ident))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progressive_preview_tracks_newest_complete_let_after_prior_preview() {
        let source = "\
let outer = cube(3.2, 6.4, 0.55, true)
preview(outer)
let shell = outer.difference(cube(2.8, 5.9, 0.5, true))
preview(shell)
let final_case = shell.difference(cube(0.85, 0.26, 1.2, true))
";

        let preview_source = progressive_cad_preview_source(source).unwrap();

        assert!(preview_source.ends_with("preview(final_case)"));
    }

    #[test]
    fn progressive_preview_does_not_duplicate_latest_explicit_preview() {
        let source = "\
let final_case = cube(1.0, 1.0, 1.0, true)
preview(final_case)
";

        let preview_source = progressive_cad_preview_source(source).unwrap();

        assert_eq!(preview_source.matches("preview(final_case)").count(), 1);
        assert!(preview_source.ends_with("preview(final_case)"));
    }

    #[test]
    fn progressive_preview_keeps_latest_explicit_render() {
        let source = "\
let final_case = cube(1.0, 1.0, 1.0, true)
render(final_case)
";

        let preview_source = progressive_cad_preview_source(source).unwrap();

        assert!(preview_source.ends_with("render(final_case)"));
        assert!(!preview_source.contains("preview(final_case)"));
    }

    #[test]
    fn phone_case_cutters_change_mesh() {
        let outer = Solid::cube(3.2, 6.4, 0.55, true);
        let phone_void = Solid::cube(2.8, 5.9, 0.50, true).translate(0.0, -0.08, 0.18);
        let shell = outer.difference(&phone_void);
        let camera_plate = Solid::cube(1.6, 1.6, 0.18, true).translate(-0.7, 2.3, -0.36);
        let shell_with_plate = shell.merge(&camera_plate);
        let camera_a = Solid::cylinder(0.24, 1.4, 56, true)
            .rotate_x(90.0)
            .translate(-0.7, 2.3, 0.0);
        let camera_b = Solid::cylinder(0.24, 1.4, 56, true)
            .rotate_x(90.0)
            .translate(-0.15, 2.3, 0.0);
        let camera_c = Solid::cylinder(0.24, 1.4, 56, true)
            .rotate_x(90.0)
            .translate(-0.7, 1.75, 0.0);
        let charge_port = Solid::cube(0.85, 0.26, 1.2, true).translate(0.0, -3.15, 0.05);
        let speaker_left = Solid::cube(0.55, 0.16, 1.2, true).translate(-0.85, -3.15, 0.05);
        let speaker_right = Solid::cube(0.55, 0.16, 1.2, true).translate(0.85, -3.15, 0.05);
        let mute_switch = Solid::cube(0.5, 0.22, 1.2, true)
            .rotate_y(90.0)
            .translate(-1.65, 2.6, 0.05);
        let volume_up = Solid::cube(0.5, 0.55, 1.2, true)
            .rotate_y(90.0)
            .translate(-1.65, 1.7, 0.05);
        let volume_down = Solid::cube(0.5, 0.55, 1.2, true)
            .rotate_y(90.0)
            .translate(-1.65, 0.95, 0.05);
        let power_btn = Solid::cube(0.5, 0.7, 1.2, true)
            .rotate_y(90.0)
            .translate(1.65, 1.6, 0.05);

        let after_camera_a = shell_with_plate.difference(&camera_a);
        let after_camera_b = after_camera_a.difference(&camera_b);
        let after_camera_c = after_camera_b.difference(&camera_c);
        let after_charge_port = after_camera_c.difference(&charge_port);
        let after_speaker_left = after_charge_port.difference(&speaker_left);
        let after_speaker_right = after_speaker_left.difference(&speaker_right);
        let after_mute_switch = after_speaker_right.difference(&mute_switch);
        let after_volume_up = after_mute_switch.difference(&volume_up);
        let after_volume_down = after_volume_up.difference(&volume_down);
        let final_case = after_volume_down.difference(&power_btn);

        assert_ne!(
            shell_with_plate.triangle_count(),
            final_case.triangle_count(),
            "cutters should change triangle count"
        );
    }

    #[test]
    fn phone_case_script_evaluates_to_final_case() {
        let source = "\
let outer = cube(3.2, 6.4, 0.55, true)
preview(outer)
let phone_void = cube(2.8, 5.9, 0.50, true).translate(0.0, -0.08, 0.18)
let shell = outer.difference(phone_void)
preview(shell)
let camera_plate = cube(1.6, 1.6, 0.18, true).translate(-0.7, 2.3, -0.36)
let shell_with_plate = shell.merge(camera_plate)
preview(shell_with_plate)
let camera_a = cylinder(0.24, 1.4, 56, true).rotate_x(90.0).translate(-0.7, 2.3, 0.0)
let camera_b = cylinder(0.24, 1.4, 56, true).rotate_x(90.0).translate(-0.15, 2.3, 0.0)
let camera_c = cylinder(0.24, 1.4, 56, true).rotate_x(90.0).translate(-0.7, 1.75, 0.0)
let charge_port = cube(0.85, 0.26, 1.2, true).translate(0.0, -3.15, 0.05)
let speaker_left = cube(0.55, 0.16, 1.2, true).translate(-0.85, -3.15, 0.05)
let speaker_right = cube(0.55, 0.16, 1.2, true).translate(0.85, -3.15, 0.05)
let mute_switch = cube(0.5, 0.22, 1.2, true).rotate_y(90.0).translate(-1.65, 2.6, 0.05)
let volume_up = cube(0.5, 0.55, 1.2, true).rotate_y(90.0).translate(-1.65, 1.7, 0.05)
let volume_down = cube(0.5, 0.55, 1.2, true).rotate_y(90.0).translate(-1.65, 0.95, 0.05)
let power_btn = cube(0.5, 0.7, 1.2, true).rotate_y(90.0).translate(1.65, 1.6, 0.05)
let final_case = shell_with_plate.difference(camera_a).difference(camera_b).difference(camera_c).difference(charge_port).difference(speaker_left).difference(speaker_right).difference(mute_switch).difference(volume_up).difference(volume_down).difference(power_btn)
render(final_case)
";
        let solid = eval_cad_script(&source, false).unwrap();

        assert!(
            solid.triangle_count() > 1000,
            "full script should render the final cut case, not the early shell preview"
        );
    }

    #[test]
    fn preview_output_is_overwritten_after_cylinder_line() {
        let source = "\
let base = cube(3.2, 6.4, 0.55, true)
preview(base)
let cutter = cylinder(0.24, 1.4, 56, true).rotate_x(90.0).translate(-0.7, 2.3, 0.0)
let final_case = base.difference(cutter)
preview(final_case)
";
        let solid = eval_cad_script(source, false).unwrap();

        assert!(solid.triangle_count() > base_cube_triangle_count());
    }

    #[test]
    fn cylinder_constructor_renders() {
        let source = "render(cylinder(0.24, 1.4, 56, true))";
        let solid = eval_cad_script(source, false).unwrap();

        assert_eq!(solid.triangle_count(), 64);
    }

    #[test]
    fn chained_cylinder_transform_renders() {
        let source = "render(cylinder(0.24, 1.4, 56, true).rotate_x(90.0).translate(-0.7, 2.3, 0.0))";
        let solid = eval_cad_script(source, false).unwrap();

        assert_eq!(solid.triangle_count(), 64);
    }

    #[test]
    fn difference_with_bound_cutter_renders() {
        let source = "\
let base = cube(3.2, 6.4, 0.55, true)
let cutter = cylinder(0.24, 1.4, 56, true).rotate_x(90.0).translate(-0.7, 2.3, 0.0)
let final_case = base.difference(cutter)
render(final_case)
";
        let solid = eval_cad_script(source, false).unwrap();

        assert!(solid.triangle_count() > base_cube_triangle_count());
    }

    fn base_cube_triangle_count() -> usize {
        Solid::cube(3.2, 6.4, 0.55, true).triangle_count()
    }
}

fn eval_cad_script_in_vm(
    vm: &mut ScriptVm,
    source: &str,
    allow_progressive_preview: bool,
) -> Result<Solid, String> {
    let source = if allow_progressive_preview {
        progressive_cad_preview_source(source).unwrap_or_else(|| source.to_string())
    } else {
        source_without_intermediate_previews(source)
    };
    let code = format!("use mod.std.*\nuse mod.cad.*\n{}", source);
    let script_mod = ScriptMod {
        cargo_manifest_path: String::new(),
        module_path: String::new(),
        file: "cad_editor".to_string(),
        line: 1,
        column: 0,
        code,
        values: Vec::new(),
    };
    clear_cad_script_output();
    let previous_silence_errors = vm.bx.silence_errors;
    vm.bx.silence_errors = previous_silence_errors || allow_progressive_preview;
    let value = vm.eval(script_mod);
    let result = if value.is_err() {
        Err(script_value_to_string(vm, value))
    } else if let Some(solid) = take_cad_script_output() {
        if !solid.is_empty() {
            Ok(solid)
        } else {
            solid_from_value(vm, value).ok_or_else(|| {
                let value = script_value_to_string(vm, value);
                if value.is_empty() || value == "nil" {
                    "script did not call render(solid) or return a CAD solid".to_string()
                } else {
                    format!("script returned {}, expected a CAD solid", value)
                }
            })
        }
    } else {
        solid_from_value(vm, value).ok_or_else(|| {
            let value = script_value_to_string(vm, value);
            if value.is_empty() || value == "nil" {
                "script did not call render(solid) or return a CAD solid".to_string()
            } else {
                format!("script returned {}, expected a CAD solid", value)
            }
        })
    };
    vm.drain_errors();
    vm.bx.silence_errors = previous_silence_errors;
    result
}

fn eval_cad_script(source: &str, allow_progressive_preview: bool) -> Result<Solid, String> {
    let mut host = ();
    let mut std = ();
    let mut vm = ScriptVm {
        host: &mut host,
        std: &mut std,
        bx: Box::new(ScriptVmBase::new()),
    };
    cad_script_mod(&mut vm);
    eval_cad_script_in_vm(&mut vm, source, allow_progressive_preview)
}

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom

    mod.draw.DrawCadMesh = mod.std.set_type_default() do #(DrawCadMesh::script_shader(vm)){
        alpha_blend: false
        backface_culling: true
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.IcoVertex, geom.IcoGeom)
        u_light_dir: uniform(vec3(-0.32, 0.86, 0.40))
        u_fill_dir: uniform(vec3(0.62, 0.42, -0.58))
        v_world_clip: varying(vec4f)
        v_world: varying(vec3f)
        v_normal: varying(vec3f)

        active_camera_world_pos: fn() -> vec3f {
            let camera_world = self.draw_pass.camera_inv * vec4(0.0, 0.0, 0.0, 1.0)
            return vec3(
                camera_world.x / max(camera_world.w, 0.00001),
                camera_world.y / max(camera_world.w, 0.00001),
                camera_world.z / max(camera_world.w, 0.00001)
            )
        }

        vertex: fn() {
            let local_pos = vec3(self.geom.pos.x, self.geom.pos.y, self.geom.pos.z)
            let local_normal = normalize(vec3(self.geom.normal.x, self.geom.normal.y, self.geom.normal.z))
            let model_view = self.draw_list.view_transform * self.transform
            let world = model_view * vec4(local_pos.x, local_pos.y, local_pos.z, 1.0)
            let world_normal = normalize((model_view * vec4(local_normal.x, local_normal.y, local_normal.z, 0.0)).xyz)
            self.v_world = world.xyz
            self.v_normal = world_normal
            self.v_world_clip = vec4(world.x, world.y, world.z, 1.0)
            let view_pos = self.draw_pass.camera_view * world
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
        }

        pixel: fn() {
            let normal = normalize(self.v_normal)
            let view_dir = normalize(self.active_camera_world_pos() - self.v_world)
            let key = max(dot(normal, normalize(self.u_light_dir)), 0.0)
            let fill = max(dot(normal, normalize(self.u_fill_dir)), 0.0)
            let rim = pow(max(1.0 - max(dot(normal, view_dir), 0.0), 0.0), 2.5)
            let lit = 0.16 + key * 0.70 + fill * 0.22 + rim * 0.26
            let color = self.color.xyz * lit + vec3(0.05, 0.07, 0.08) * rim
            return vec4(color, self.color.w)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.v_world_clip, self.pixel(), self.depth_clip)
        }
    }

    mod.widgets.CadCodeEditorBase = #(CadCodeEditor::register_widget(vm))
    mod.widgets.CadCodeEditor = set_type_default() do mod.widgets.CadCodeEditorBase{
        width: Fill
        height: Fill
        editor +: {
            width: Fill
            height: Fill
            pad_left_top: vec2(14.0, 12.0)
            empty_page_at_end: false
            read_only: false
            show_gutter: false
            word_wrap: false
            scroll_bars: mod.widgets.ScrollBars {}
            draw_bg +: {
                color: #x10151b
            }
            draw_gutter +: {
                color: #x697784
            }
            draw_text +: {
                text_style: theme.font_code
            }
        }
    }

    mod.widgets.CadViewportBase = #(CadViewport::register_widget(vm))
    mod.widgets.CadViewport = set_type_default() do mod.widgets.CadViewportBase{
        width: Fill
        height: Fill
        clear_color: #x0a0f14
        color: vec4(0.34, 0.74, 0.86, 1.0)
        ground_color: vec4(0.09, 0.13, 0.16, 1.0)
        draw_bg: mod.draw.DrawXrSceneTexture{}
        draw_mesh: mod.draw.DrawCadMesh{
            backface_culling: true
        }
        draw_ground: mod.draw.DrawCadMesh{
            backface_culling: false
        }
        camera: mod.widgets.XrCamera{
            fov_y: 42.0
            desktop_target: vec3(0.0, 0.02, 0.0)
            distance: 5.5
            distance_min: 1.2
            distance_max: 18.0
            wheel_zoom_step: 0.08
        }
    }

    load_all_resources() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1280, 760)
                body +: {
                    app_view := SolidView{
                        width: Fill
                        height: Fill
                        flow: Down
                        draw_bg +: {color: #x0d1116}

                        header := SolidView{
                            width: Fill
                            height: 58.0
                            flow: Down
                            padding: Inset{left: 14.0 top: 7.0 right: 14.0 bottom: 5.0}
                            spacing: 2.0
                            draw_bg +: {color: #x171d24}

                            title_row := View{
                                width: Fill
                                height: Fit
                                flow: Right
                                spacing: 16.0
                                align: Align{x: 0.0 y: 0.5}

                                title_label := Label{
                                    width: Fit
                                    height: Fit
                                    text: "CAD"
                                    draw_text +: {
                                        color: #xf3f6f8
                                        text_style +: {font_size: 13.5}
                                    }
                                }

                                cad_busy_spinner := LoadingSpinner{
                                    width: 16
                                    height: 16
                                    visible: false
                                    draw_bg +: {
                                        color: #x7dd3fc
                                        stroke_width: 2.0
                                        rotation_speed: 1.6
                                    }
                                }

                                status_label := Label{
                                    width: Fill
                                    height: Fit
                                    text: ""
                                    draw_text +: {
                                        color: #x9aa8b5
                                        text_style +: {font_size: 11.5}
                                    }
                                }
                            }

                            prompt_title_label := Label{
                                width: Fill
                                height: Fit
                                padding: 0.0
                                text: ""
                                draw_text +: {
                                    color: #x7f8d9a
                                    font_scale: 0.92
                                    text_style +: {font_size: 9.5}
                                }
                            }
                        }

                        split_view := Splitter{
                            width: Fill
                            height: Fill
                            axis: SplitterAxis.Horizontal
                            align: SplitterAlign.FromA(510.0)
                            size: 7.0
                            min_vertical: 300.0
                            max_vertical: 240.0
                            draw_bg +: {
                                color: #x2a333c
                                color_hover: #x3f4b56
                                color_drag: #x587083
                                splitter_pad: 2.0
                                bar_size: 120.0
                            }

                            a: SolidView{
                                width: Fill
                                height: Fill
                                flow: Overlay
                                draw_bg +: {color: #x10151b}

                                cad_editor := mod.widgets.CadCodeEditor{}
                            }

                            b: SolidView{
                                width: Fill
                                height: Fill
                                flow: Overlay
                                draw_bg +: {color: #x0a0f14}

                                cad_viewport := mod.widgets.CadViewport{}
                            }
                        }

                        prompt_panel := SolidView{
                            width: Fill
                            height: Fit
                            flow: Down
                            padding: Inset{left: 12.0 top: 8.0 right: 12.0 bottom: 8.0}
                            spacing: 6.0
                            draw_bg +: {color: #x171d24}

                            prompt_row := View{
                                width: Fill
                                height: Fit
                                flow: Right
                                spacing: 8.0
                                align: Align{y: 0.5}

                                backend_dropdown := DropDown{
                                    width: 150.0
                                    labels: ["Claude Splash" "Local OpenAI"]
                                    draw_text +: {
                                        text_style +: {font_size: 11.0}
                                    }
                                }

                                cad_prompt_input := TextInput{
                                    width: Fill
                                    height: Fit
                                    empty_text: "Prompt CAD changes... (Enter to generate)"
                                }

                                ai_generate_button := Button{
                                    width: 90.0
                                    text: "Generate"
                                }

                                ai_cancel_button := Button{
                                    width: 78.0
                                    text: "Cancel"
                                    visible: false
                                }
                            }

                            ai_status_label := Label{
                                width: Fill
                                height: Fit
                                text: ""
                                draw_text +: {
                                    color: #x9aa8b5
                                    text_style +: {font_size: 10.5}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawCadMesh {
    #[rust(vec3(-0.32, 0.86, 0.40))]
    light_dir: Vec3f,
    #[rust(vec3(0.62, 0.42, -0.58))]
    fill_dir: Vec3f,
    #[deref]
    draw_vars: DrawVars,
    #[live]
    color: Vec4f,
    #[live]
    transform: Mat4f,
    #[live(1.0)]
    depth_clip: f32,
}

impl DrawCadMesh {
    fn apply_uniforms(&mut self, cx: &mut CxDraw) {
        let light_dir = if self.light_dir.length() > 0.000_01 {
            self.light_dir.normalize()
        } else {
            vec3(0.0, 1.0, 0.0)
        };
        let fill_dir = if self.fill_dir.length() > 0.000_01 {
            self.fill_dir.normalize()
        } else {
            vec3(1.0, 0.0, 0.0)
        };
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_light_dir),
            &[light_dir.x, light_dir.y, light_dir.z],
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_fill_dir),
            &[fill_dir.x, fill_dir.y, fill_dir.z],
        );
    }

    fn draw(&mut self, cx: &mut CxDraw, geometry_id: GeometryId) {
        self.draw_vars.geometry_id = Some(geometry_id);
        self.apply_uniforms(cx);
        if self.draw_vars.can_instance() {
            let new_area = cx.add_instance(&self.draw_vars);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
    }
}

#[derive(Clone, Copy, Default)]
struct CadStats {
    vertices: usize,
    triangles: usize,
    max_dimension: f64,
}

#[derive(Default)]
struct CadMeshData {
    indices: Vec<u32>,
    vertices: Vec<f32>,
    stats: CadStats,
}

fn cad_mesh_data_from_solid(solid: &Solid) -> CadMeshData {
    let mesh = solid.mesh();
    let mut stats = CadStats {
        vertices: solid.vertex_count(),
        triangles: solid.triangle_count(),
        max_dimension: 0.0,
    };

    if mesh.vertices.is_empty() || mesh.triangles.is_empty() {
        return CadMeshData {
            stats,
            ..Default::default()
        };
    }

    let bbox = mesh.bounding_box();
    if bbox.is_empty() {
        return CadMeshData {
            stats,
            ..Default::default()
        };
    }

    let center = bbox.center();
    let size = bbox.size();
    stats.max_dimension = size.x.max(size.y).max(size.z);
    let scale = 1.75 / stats.max_dimension.max(0.000_001);

    let mut vertices = Vec::with_capacity(mesh.triangles.len() * 3 * 8);
    let mut indices = Vec::with_capacity(mesh.triangles.len() * 3);

    for tri in &mesh.triangles {
        let Some(a) = mesh.vertices.get(tri[0] as usize) else {
            continue;
        };
        let Some(b) = mesh.vertices.get(tri[1] as usize) else {
            continue;
        };
        let Some(c) = mesh.vertices.get(tri[2] as usize) else {
            continue;
        };

        let p0 = vec3(
            ((a.x - center.x) * scale) as f32,
            ((a.y - center.y) * scale) as f32,
            ((a.z - center.z) * scale) as f32,
        );
        let p1 = vec3(
            ((b.x - center.x) * scale) as f32,
            ((b.y - center.y) * scale) as f32,
            ((b.z - center.z) * scale) as f32,
        );
        let p2 = vec3(
            ((c.x - center.x) * scale) as f32,
            ((c.y - center.y) * scale) as f32,
            ((c.z - center.z) * scale) as f32,
        );
        let mut normal = Vec3f::cross(p1 - p0, p2 - p0);
        if normal.length() <= 0.000_001 {
            normal = vec3(0.0, 1.0, 0.0);
        } else {
            normal = normal.normalize();
        }

        for p in [p0, p1, p2] {
            vertices.extend_from_slice(&[p.x, p.y, p.z, 1.0, normal.x, normal.y, normal.z, 0.0]);
            indices.push(indices.len() as u32);
        }
    }

    if vertices.is_empty() || indices.is_empty() {
        CadMeshData {
            stats,
            ..Default::default()
        }
    } else {
        CadMeshData {
            indices,
            vertices,
            stats,
        }
    }
}

fn update_geometry_from_mesh(
    cx: &mut Cx,
    geometry: &mut Option<Geometry>,
    mesh_data: CadMeshData,
) -> CadStats {
    let stats = mesh_data.stats;
    if mesh_data.vertices.is_empty() || mesh_data.indices.is_empty() {
        *geometry = None;
    } else {
        let geometry = geometry.get_or_insert_with(|| Geometry::new(cx));
        geometry.update(cx, mesh_data.indices, mesh_data.vertices);
    }
    stats
}

struct CadRebuildRequest {
    seq: u64,
    source: String,
    allow_progressive_preview: bool,
    save_output: bool,
}

enum CadRebuildPayload {
    Mesh {
        mesh_data: CadMeshData,
        saved: bool,
        save_error: Option<String>,
    },
    Error(String),
}

struct CadRebuildResult {
    seq: u64,
    payload: CadRebuildPayload,
}

struct CadRebuildWorker {
    request_tx: Sender<CadRebuildRequest>,
    result_rx: Receiver<CadRebuildResult>,
}

impl CadRebuildWorker {
    fn new(cx: &mut Cx) -> Self {
        let (request_tx, request_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        cx.spawn_thread(move || cad_rebuild_worker_loop(request_rx, result_tx));
        Self {
            request_tx,
            result_rx,
        }
    }

    fn request(&self, request: CadRebuildRequest) {
        let _ = self.request_tx.send(request);
    }

    fn try_recv(&self) -> Option<CadRebuildResult> {
        self.result_rx.try_recv().ok()
    }
}

fn cad_rebuild_worker_loop(
    request_rx: Receiver<CadRebuildRequest>,
    result_tx: Sender<CadRebuildResult>,
) {
    while let Ok(mut request) = request_rx.recv() {
        while let Ok(newer_request) = request_rx.try_recv() {
            request = newer_request;
        }

        let payload = match eval_cad_script(&request.source, request.allow_progressive_preview) {
            Ok(solid) => {
                let save_error = if request.save_output {
                    save_cad_state(&request.source, &solid).err()
                } else {
                    None
                };
                CadRebuildPayload::Mesh {
                    mesh_data: cad_mesh_data_from_solid(&solid),
                    saved: request.save_output && save_error.is_none(),
                    save_error,
                }
            }
            Err(err) => CadRebuildPayload::Error(err),
        };
        if result_tx
            .send(CadRebuildResult {
                seq: request.seq,
                payload,
            })
            .is_err()
        {
            return;
        }
        SignalToUI::set_ui_signal();
    }
}

fn ensure_ground_geometry(cx: &mut Cx, geometry: &mut Option<Geometry>) -> GeometryId {
    let geometry = geometry.get_or_insert_with(|| {
        let geometry = Geometry::new(cx);
        let y = -1.02;
        let size = 2.45;
        let vertices = vec![
            -size, y, -size, 1.0, 0.0, 1.0, 0.0, 0.0, size, y, -size, 1.0, 0.0, 1.0, 0.0, 0.0,
            size, y, size, 1.0, 0.0, 1.0, 0.0, 0.0, -size, y, size, 1.0, 0.0, 1.0, 0.0, 0.0,
        ];
        geometry.update(cx, vec![0, 1, 2, 0, 2, 3], vertices);
        geometry
    });
    geometry.geometry_id()
}

fn set_pass_camera(cx: &mut Cx, pass: &DrawPass, scene: &SceneState3D) {
    let camera_inv = scene.view.invert();
    let pass_uniforms = &mut cx.passes[pass.draw_pass_id()].pass_uniforms;
    pass_uniforms.camera_projection = scene.projection;
    pass_uniforms.camera_projection_r = scene.projection;
    pass_uniforms.camera_view = scene.view;
    pass_uniforms.camera_view_r = scene.view;
    pass_uniforms.depth_projection = scene.projection;
    pass_uniforms.depth_projection_r = scene.projection;
    pass_uniforms.depth_view = scene.view;
    pass_uniforms.depth_view_r = scene.view;
    pass_uniforms.camera_inv = camera_inv;
    pass_uniforms.camera_inv_r = camera_inv;
}

#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct CadViewport {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_bg: DrawXrSceneTexture,
    #[live]
    draw_mesh: DrawCadMesh,
    #[live]
    draw_ground: DrawCadMesh,
    #[live(vec4(0.043, 0.063, 0.086, 1.0))]
    clear_color: Vec4f,
    #[live(vec4(0.34, 0.74, 0.86, 1.0))]
    color: Vec4f,
    #[live(vec4(0.09, 0.13, 0.16, 1.0))]
    ground_color: Vec4f,
    #[live]
    camera: XrCamera,
    #[new]
    pass: DrawPass,
    #[new]
    draw_list: DrawList,
    #[new]
    color_texture: Texture,
    #[new]
    depth_texture: Texture,
    #[rust]
    area: Area,
    #[rust(false)]
    initialized: bool,
    #[rust(false)]
    camera_pose_initialized: bool,
    #[rust]
    mesh_geometry: Option<Geometry>,
    #[rust]
    ground_geometry: Option<Geometry>,
    #[rust]
    stats: CadStats,
}

impl CadViewport {
    fn ensure_initialized(&mut self, cx: &mut Cx) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        if !self.camera_pose_initialized {
            self.camera.orbit_yaw = 0.58;
            self.camera.orbit_pitch = -0.24;
            self.camera_pose_initialized = true;
        }
        self.color_texture = Texture::new_with_format(
            cx,
            TextureFormat::RenderBGRAu8 {
                size: TextureSize::Auto,
                initial: true,
            },
        );
        self.depth_texture = Texture::new_with_format(
            cx,
            TextureFormat::DepthD32 {
                size: TextureSize::Auto,
                initial: true,
            },
        );
        self.pass.set_color_texture(
            cx,
            &self.color_texture,
            DrawPassClearColor::ClearWith(self.clear_color),
        );
        self.pass
            .set_depth_texture(cx, &self.depth_texture, DrawPassClearDepth::ClearWith(1.0));
        cx.passes[self.pass.draw_pass_id()].keep_camera_matrix = true;
    }

    fn set_mesh(&mut self, cx: &mut Cx, mesh_data: CadMeshData) -> CadStats {
        let stats = update_geometry_from_mesh(cx, &mut self.mesh_geometry, mesh_data);
        self.stats = stats;
        self.area.redraw(cx);
        cx.redraw_all();
        stats
    }

    fn draw_scene(&mut self, cx: &mut Cx3d, scene_state: SceneState3D) {
        self.draw_list.begin_always(cx);
        cx.begin_scene_3d(scene_state);
        let previous_world = cx.set_scene_world_transform_3d(Mat4f::identity());

        let ground_id = ensure_ground_geometry(cx.cx, &mut self.ground_geometry);
        self.draw_ground.transform = Mat4f::identity();
        self.draw_ground.color = self.ground_color;
        self.draw_ground.depth_clip = 0.0;
        self.draw_ground.draw(cx, ground_id);

        if let Some(geometry) = &self.mesh_geometry {
            self.draw_mesh.transform = Mat4f::identity();
            self.draw_mesh.color = self.color;
            self.draw_mesh.depth_clip = 0.0;
            self.draw_mesh.draw(cx, geometry.geometry_id());
        }

        if let Some(previous_world) = previous_world {
            let _ = cx.set_scene_world_transform_3d(previous_world);
        }
        cx.end_scene_3d();
        self.draw_list.end(cx);
    }
}

impl WidgetNode for CadViewport {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }

    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn area(&self) -> Area {
        self.area
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
}

impl Widget for CadViewport {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        self.camera.handle_desktop_interaction(cx, event);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        if rect.size.x <= 1.0 || rect.size.y <= 1.0 {
            return DrawStep::done();
        }

        self.ensure_initialized(cx.cx);
        self.camera.set_desktop_viewport_rect(rect);
        self.pass.set_size(cx, rect.size);
        self.pass.set_color_texture(
            cx,
            &self.color_texture,
            DrawPassClearColor::ClearWith(self.clear_color),
        );
        self.pass
            .set_depth_texture(cx, &self.depth_texture, DrawPassClearDepth::ClearWith(1.0));

        cx.make_child_pass(&self.pass);
        cx.begin_pass(&self.pass, None);
        if let Some(scene_state) = self.camera.desktop_scene_state(rect, cx.time()) {
            set_pass_camera(cx.cx, &self.pass, &scene_state);
            let cx3d = &mut Cx3d::new(cx.cx);
            self.draw_scene(cx3d, scene_state);
        }
        cx.end_pass(&self.pass);

        self.draw_bg.set_scene_texture(&self.color_texture);
        self.draw_bg.draw_abs(cx, rect);
        self.area = self.draw_bg.area();
        cx.set_pass_area(&self.pass, self.area);
        DrawStep::done()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum CadCodeEditorAction {
    TextDidChange,
    #[default]
    None,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum BackendType {
    #[default]
    ClaudeSplash,
    LocalOpenAi,
}

const BACKENDS: [BackendType; 2] = [BackendType::ClaudeSplash, BackendType::LocalOpenAi];

impl BackendType {
    fn to_index(self) -> usize {
        BACKENDS
            .iter()
            .position(|&backend| backend == self)
            .unwrap_or(0)
    }

    fn from_index(index: usize) -> Option<Self> {
        BACKENDS.get(index).copied()
    }

    fn status_label(self) -> &'static str {
        match self {
            Self::ClaudeSplash => "Ready: Claude Splash",
            Self::LocalOpenAi => "Ready: Local OpenAI stream at 10.0.0.168:8080",
        }
    }
}

#[derive(Script, ScriptHook, WidgetRef, WidgetSet, WidgetRegister)]
pub struct CadCodeEditor {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[live]
    editor: CodeEditor,
    #[rust]
    session: Option<CodeSession>,
    #[live]
    text: ArcStringMut,
}

impl CadCodeEditor {
    fn lazy_init_session(&mut self) {
        if self.session.is_none() {
            let doc = CodeDocument::new(self.text.as_ref().into(), DecorationSet::new());
            let mut session = CodeSession::new(doc);
            session.handle_changes();
            self.session = Some(session);
            self.editor.keep_cursor_in_view = KeepCursorInView::Once;
        }
    }
}

impl WidgetNode for CadCodeEditor {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }

    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn area(&self) -> Area {
        self.editor.area()
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.editor.redraw(cx)
    }

    fn find_widgets_from_point(&self, cx: &Cx, point: DVec2, found: &mut dyn FnMut(&WidgetRef)) {
        self.editor.find_widgets_from_point(cx, point, found)
    }

    fn visible(&self) -> bool {
        self.editor.visible()
    }

    fn set_visible(&mut self, cx: &mut Cx, visible: bool) {
        self.editor.set_visible(cx, visible)
    }
}

impl Widget for CadCodeEditor {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.lazy_init_session();
        let session = self.session.as_mut().unwrap();
        self.editor.draw_walk_editor(cx, session, walk);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        self.lazy_init_session();
        let session = self.session.as_mut().unwrap();
        let old_text = self.text.as_ref().to_string();
        for action in self
            .editor
            .handle_event(cx, event, &mut Scope::empty(), session)
        {
            if matches!(action, CodeEditorAction::TextDidChange) {
                session.handle_changes();
            }
        }

        session.handle_changes();
        let text = session.document().as_text().to_string();
        if text != old_text {
            self.text.set(&text);
            cx.widget_action(self.uid, CadCodeEditorAction::TextDidChange);
        }
    }

    fn text(&self) -> String {
        self.text.as_ref().to_string()
    }

    fn set_text(&mut self, cx: &mut Cx, value: &str) {
        if self.text.as_ref() != value {
            self.text.set(value);
            self.session = None;
            self.redraw(cx);
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust(false)]
    initialized: bool,
    #[rust]
    live_update_timer: Timer,
    #[rust]
    last_source: String,
    #[rust]
    cad_worker: Option<CadRebuildWorker>,
    #[rust]
    rebuild_seq: u64,
    #[rust(false)]
    rebuild_pending: bool,
    #[rust]
    current_prompt_title: String,
    #[rust]
    agent: Option<Box<dyn Agent>>,
    #[rust]
    session_id: Option<SessionId>,
    #[rust]
    current_prompt: Option<PromptId>,
    #[rust]
    active_backend: BackendType,
    #[rust]
    backend_available: bool,
    #[rust]
    ai_response_buffer: String,
    #[rust]
    ai_prompt_started_at: Option<Instant>,
}

impl App {
    fn set_rebuild_pending(&mut self, cx: &mut Cx, pending: bool) {
        if self.rebuild_pending == pending {
            return;
        }
        self.rebuild_pending = pending;
        self.ui
            .view(cx, ids!(cad_busy_spinner))
            .set_visible(cx, pending);
        self.ui.redraw(cx);
    }

    fn update_prompt_title(&self, cx: &mut Cx) {
        let title = if self.current_prompt_title.trim().is_empty() {
            "Prompt: default CAD script".to_string()
        } else {
            format!("Prompt: {}", self.current_prompt_title.trim())
        };
        self.ui
            .label(cx, ids!(prompt_title_label))
            .set_text(cx, &title);
    }

    fn cad_system_prompt() -> String {
        let prompt_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("system_prompt.md");
        std::fs::read_to_string(&prompt_path)
            .unwrap_or_else(|_| include_str!("../system_prompt.md").to_string())
    }

    fn create_backend_session(&mut self, cx: &mut Cx, backend: BackendType) {
        self.cancel_ai_prompt(cx);
        self.agent = None;
        self.session_id = None;
        self.current_prompt = None;
        self.ai_response_buffer.clear();
        self.ai_prompt_started_at = None;
        self.active_backend = backend;

        let agent = match backend {
            BackendType::ClaudeSplash => {
                self.backend_available = ClaudeCodeAgent::is_available();
                self.backend_available
                    .then(|| Box::new(ClaudeCodeAgent::new()) as Box<dyn Agent>)
            }
            BackendType::LocalOpenAi => {
                self.backend_available = true;
                Some(
                    Box::new(StatelessBackendAdapter::new(Box::new(OpenAiBackend::new(
                        BackendConfig::OpenAI {
                            api_key: String::new(),
                            model: LOCAL_OPENAI_MODEL.to_string(),
                            base_url: Some(LOCAL_OPENAI_URL.to_string()),
                            reasoning_effort: None,
                        },
                    )))) as Box<dyn Agent>,
                )
            }
        };

        let Some(agent) = agent else {
            self.update_ai_status(cx);
            return;
        };

        let config = SessionConfig {
            cwd: Some(env!("CARGO_MANIFEST_DIR").to_string()),
            system_prompt: Some(Self::cad_system_prompt()),
            ..Default::default()
        };
        self.agent = Some(agent);
        if let Some(agent) = &mut self.agent {
            self.session_id = Some(agent.create_session(cx, config));
        }
        self.update_ai_status(cx);
    }

    fn update_ai_status(&self, cx: &mut Cx) {
        let status = if self.current_prompt.is_some() {
            let chars = self.ai_response_buffer.len();
            let elapsed_ms = self
                .ai_prompt_started_at
                .map(|started| started.elapsed().as_millis())
                .unwrap_or(0);
            let dots = ".".repeat(((elapsed_ms / 350) % 4) as usize);
            let backend = match self.active_backend {
                BackendType::ClaudeSplash => "Claude Splash",
                BackendType::LocalOpenAi => "Local OpenAI",
            };
            if chars == 0 {
                let elapsed = (elapsed_ms / 1000) as u64;
                if elapsed < 2 {
                    format!("Starting {}{}", backend, dots)
                } else {
                    format!("Thinking{} Waiting for {} ({elapsed}s)", dots, backend)
                }
            } else {
                format!("Streaming code{} {} chars", dots, chars)
            }
        } else if self.backend_available {
            self.active_backend.status_label().to_string()
        } else {
            match self.active_backend {
                BackendType::ClaudeSplash => {
                    "Claude Code not found. Set CLAUDE_CODE_PATH or install claude.".to_string()
                }
                BackendType::LocalOpenAi => "Local OpenAI backend unavailable".to_string(),
            }
        };
        self.ui
            .label(cx, ids!(ai_status_label))
            .set_text(cx, &status);
    }

    fn set_ai_busy(&mut self, cx: &mut Cx, busy: bool) {
        self.ui
            .view(cx, ids!(ai_cancel_button))
            .set_visible(cx, busy);
        self.ui
            .view(cx, ids!(ai_generate_button))
            .set_visible(cx, !busy);
    }

    fn send_ai_prompt(&mut self, cx: &mut Cx) {
        if self.current_prompt.is_some() {
            return;
        }
        let prompt_input = self.ui.text_input(cx, ids!(cad_prompt_input));
        let prompt = prompt_input.text();
        if prompt.trim().is_empty() {
            return;
        }
        self.current_prompt_title = prompt.trim().to_string();
        self.update_prompt_title(cx);

        let (agent, session_id) = match (&mut self.agent, self.session_id) {
            (Some(agent), Some(session_id)) => (agent, session_id),
            _ => {
                self.update_ai_status(cx);
                return;
            }
        };
        if !agent.is_session_ready(session_id) {
            self.ui
                .label(cx, ids!(ai_status_label))
                .set_text(cx, "AI backend is still starting");
            self.ui.redraw(cx);
            return;
        }

        let current_script = self.ui.widget(cx, ids!(cad_editor)).text();
        let request = format!(
            "User request:\n{}\n\nCurrent CAD script:\n```cad\n{}\n```\n\nReturn the complete replacement CAD script only.",
            prompt.trim(),
            current_script
        );

        self.ai_response_buffer.clear();
        self.current_prompt = Some(agent.send_prompt(cx, session_id, &request));
        self.ai_prompt_started_at = Some(Instant::now());
        prompt_input.set_text(cx, "");
        self.set_ai_busy(cx, true);
        self.update_ai_status(cx);
        self.ui.redraw(cx);
    }

    fn cancel_ai_prompt(&mut self, cx: &mut Cx) {
        let was_busy = self.current_prompt.is_some();
        if let (Some(agent), Some(prompt_id)) = (&mut self.agent, self.current_prompt.take()) {
            agent.cancel_prompt(cx, prompt_id);
        }
        self.ai_response_buffer.clear();
        self.ai_prompt_started_at = None;
        self.set_ai_busy(cx, false);
        if was_busy {
            self.ui
                .label(cx, ids!(ai_status_label))
                .set_text(cx, "Generation canceled");
        } else {
            self.update_ai_status(cx);
        }
        self.ui.redraw(cx);
    }

    fn extract_cad_script(text: &str) -> String {
        let trimmed = text.trim();
        if let Some(start) = trimmed.find("```") {
            let after_open = &trimmed[start + 3..];
            let code_start = after_open.find('\n').map(|idx| idx + 1).unwrap_or(0);
            let after_lang = &after_open[code_start..];
            if let Some(end) = after_lang.find("```") {
                return after_lang[..end].trim().to_string();
            }
        }
        trimmed.to_string()
    }

    fn extract_streaming_cad_script(text: &str) -> String {
        let trimmed = text.trim_start();
        if let Some(start) = trimmed.find("```") {
            let after_open = &trimmed[start + 3..];
            let code_start = after_open.find('\n').map(|idx| idx + 1).unwrap_or(0);
            let after_lang = &after_open[code_start..];
            if let Some(end) = after_lang.find("```") {
                return after_lang[..end].trim_start().to_string();
            }
            return after_lang.trim_start().to_string();
        }

        let mut first_code = None;
        for needle in [
            "let ",
            "render(",
            "empty()",
            "cube(",
            "cube_uniform(",
            "sphere(",
            "cylinder(",
            "cone(",
            "torus(",
            "tapered_cylinder(",
        ] {
            if let Some(index) = trimmed.find(needle) {
                first_code = Some(first_code.map_or(index, |first: usize| first.min(index)));
            }
        }

        if let Some(first_code) = first_code {
            trimmed[first_code..].to_string()
        } else {
            trimmed.to_string()
        }
    }

    fn stream_ai_response_to_editor(&mut self, cx: &mut Cx) {
        let script = Self::extract_streaming_cad_script(&self.ai_response_buffer);
        if script.trim().is_empty() {
            return;
        }
        let editor = self.ui.widget(cx, ids!(cad_editor));
        if editor.text() != script {
            editor.set_text(cx, &script);
            self.request_rebuild(cx, false, false);
            self.ui.redraw(cx);
            cx.redraw_all();
        }
    }

    fn apply_ai_response(&mut self, cx: &mut Cx) {
        let script = Self::extract_cad_script(&self.ai_response_buffer);
        self.current_prompt = None;
        self.ai_prompt_started_at = None;
        self.set_ai_busy(cx, false);
        if script.trim().is_empty() {
            self.ui
                .label(cx, ids!(ai_status_label))
                .set_text(cx, "AI returned an empty CAD script");
            return;
        }

        self.ui.widget(cx, ids!(cad_editor)).set_text(cx, &script);
        self.request_rebuild(cx, true, true);
        self.ui
            .label(cx, ids!(ai_status_label))
            .set_text(cx, "Generated CAD script applied");
        self.ai_response_buffer.clear();
        self.ui.redraw(cx);
    }

    fn request_rebuild(&mut self, cx: &mut Cx, force: bool, save_output: bool) {
        if self.cad_worker.is_none() {
            self.cad_worker = Some(CadRebuildWorker::new(cx));
        }
        let source = self.ui.widget(cx, ids!(cad_editor)).text();
        if !force && source == self.last_source {
            return;
        }
        self.last_source = source.clone();
        self.rebuild_seq = self.rebuild_seq.wrapping_add(1);
        let seq = self.rebuild_seq;

        if let Some(worker) = &self.cad_worker {
            worker.request(CadRebuildRequest {
                seq,
                source,
                allow_progressive_preview: self.current_prompt.is_some(),
                save_output,
            });
        }

        let status = if self.current_prompt.is_some() {
            "Computing streamed 3D model...".to_string()
        } else {
            "Computing 3D model...".to_string()
        };
        self.set_rebuild_pending(cx, true);
        self.ui.label(cx, ids!(status_label)).set_text(cx, &status);
        self.ui.redraw(cx);
    }

    fn drain_rebuild_results(&mut self, cx: &mut Cx) {
        let mut latest = None;
        if let Some(worker) = &self.cad_worker {
            while let Some(result) = worker.try_recv() {
                latest = Some(result);
            }
        }

        let Some(result) = latest else {
            return;
        };
        if result.seq != self.rebuild_seq {
            return;
        }

        self.set_rebuild_pending(cx, false);
        match result.payload {
            CadRebuildPayload::Mesh {
                mesh_data,
                saved,
                save_error,
            } => {
                let stats = if let Some(mut viewport) = self
                    .ui
                    .widget(cx, ids!(cad_viewport))
                    .borrow_mut::<CadViewport>()
                {
                    viewport.set_mesh(cx, mesh_data)
                } else {
                    CadStats::default()
                };
                let save_status = if let Some(save_error) = save_error {
                    format!("; save failed: {}", save_error)
                } else if saved {
                    "; saved".to_string()
                } else {
                    String::new()
                };
                self.ui.label(cx, ids!(status_label)).set_text(
                    cx,
                    &format!(
                        "{} triangles, {} vertices, bounds {:.2}{}",
                        stats.triangles, stats.vertices, stats.max_dimension, save_status
                    ),
                );
                self.ui.redraw(cx);
            }
            CadRebuildPayload::Error(err) => {
                let status = if self.current_prompt.is_some() {
                    "Streaming CAD script...".to_string()
                } else {
                    format!("Error: {}", err)
                };
                self.ui.label(cx, ids!(status_label)).set_text(cx, &status);
                self.ui.redraw(cx);
            }
        }
    }

    fn drain_agent_events(&mut self, cx: &mut Cx, event: &Event) {
        let events = if let Some(agent) = &mut self.agent {
            agent.handle_event(cx, event)
        } else {
            Vec::new()
        };

        for event in events {
            match event {
                AgentEvent::SessionReady { .. } => {
                    self.update_ai_status(cx);
                }
                AgentEvent::SessionError { error, .. } => {
                    self.backend_available = false;
                    self.current_prompt = None;
                    self.ai_prompt_started_at = None;
                    self.set_ai_busy(cx, false);
                    self.ui
                        .label(cx, ids!(ai_status_label))
                        .set_text(cx, &format!("Error: {}", error));
                }
                AgentEvent::TextDelta { text, .. } => {
                    self.ai_response_buffer.push_str(&text);
                    self.stream_ai_response_to_editor(cx);
                    self.update_ai_status(cx);
                    self.ui.redraw(cx);
                    cx.redraw_all();
                }
                AgentEvent::TurnComplete { .. } => {
                    self.apply_ai_response(cx);
                }
                AgentEvent::PromptError { error, .. } => {
                    self.current_prompt = None;
                    self.ai_prompt_started_at = None;
                    self.set_ai_busy(cx, false);
                    self.ui
                        .label(cx, ids!(ai_status_label))
                        .set_text(cx, &format!("Error: {}", error));
                    self.ui.redraw(cx);
                }
                AgentEvent::ToolRequest { .. } => {}
            }
        }
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        self.live_update_timer = cx.start_interval(LIVE_UPDATE_INTERVAL);
        let (startup_source, prompt_title, save_loaded_source) =
            if let Some(saved_source) = load_saved_cad_script() {
                (saved_source, "saved CAD script", true)
            } else {
                (DEFAULT_CAD_SCRIPT.to_string(), "default CAD script", false)
            };
        self.ui
            .widget(cx, ids!(cad_editor))
            .set_text(cx, &startup_source);
        self.current_prompt_title = prompt_title.to_string();
        self.update_prompt_title(cx);
        self.ui
            .drop_down(cx, ids!(backend_dropdown))
            .set_selected_item(cx, self.active_backend.to_index());
        self.create_backend_session(cx, self.active_backend);
        self.request_rebuild(cx, true, save_loaded_source);
    }

    fn handle_timer(&mut self, cx: &mut Cx, event: &TimerEvent) {
        if self.live_update_timer.is_timer(event).is_some() {
            self.request_rebuild(cx, false, self.current_prompt.is_none());
            self.drain_rebuild_results(cx);
            self.drain_agent_events(cx, &Event::Signal);
            if self.rebuild_pending {
                self.ui.redraw(cx);
            }
            if self.current_prompt.is_some() {
                self.update_ai_status(cx);
                self.ui.redraw(cx);
            }
        }
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self
            .ui
            .button(cx, ids!(ai_generate_button))
            .clicked(actions)
        {
            self.send_ai_prompt(cx);
        }
        if self.ui.button(cx, ids!(ai_cancel_button)).clicked(actions) {
            self.cancel_ai_prompt(cx);
        }
        if self
            .ui
            .text_input(cx, ids!(cad_prompt_input))
            .returned(actions)
            .is_some()
        {
            self.send_ai_prompt(cx);
        }
        if self
            .ui
            .text_input(cx, ids!(cad_prompt_input))
            .escaped(actions)
        {
            self.cancel_ai_prompt(cx);
        }
        if let Some(index) = self
            .ui
            .drop_down(cx, ids!(backend_dropdown))
            .selected(actions)
        {
            if let Some(backend) = BackendType::from_index(index) {
                if backend != self.active_backend {
                    self.create_backend_session(cx, backend);
                }
            }
        }
        for action in actions {
            if matches!(action.cast(), CadCodeEditorAction::TextDidChange) {
                self.request_rebuild(cx, false, self.current_prompt.is_none());
            }
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        crate::makepad_code_editor::script_mod(vm);
        crate::makepad_xr::script_mod(vm);
        cad_script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());

        if self.initialized && !matches!(event, Event::Startup) {
            self.drain_rebuild_results(cx);
        }

        self.drain_agent_events(cx, event);
    }
}
