pub use makepad_code_editor;
pub use makepad_csg;
pub use makepad_widgets;
pub use makepad_xr;

use makepad_code_editor::{
    code_editor::{CodeEditorAction, KeepCursorInView},
    decoration::DecorationSet,
    CodeDocument, CodeEditor, CodeSession,
};
use makepad_csg::Solid;
use makepad_widgets::*;
use makepad_xr::scene::*;
use std::cell::RefCell;

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

thread_local! {
    static CAD_SCRIPT_OUTPUT: RefCell<Option<Solid>> = RefCell::new(None);
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
        let segments = arg_u32(vm, args, 1, 32).clamp(8, 128);
        let rings = arg_u32(vm, args, 2, 16).clamp(4, 64);
        new_cad_solid(vm, Solid::sphere(radius, segments, rings))
    });
    vm.add_method(cad, id!(cylinder), script_args!(), |vm, args| {
        let radius = arg_f64(vm, args, 0, 1.0).abs().max(0.001);
        let height = arg_f64(vm, args, 1, 1.0).abs().max(0.001);
        let segments = arg_u32(vm, args, 2, 32).clamp(3, 128);
        let center = arg_bool(vm, args, 3, true);
        new_cad_solid(vm, Solid::cylinder(radius, height, segments, center))
    });
    vm.add_method(cad, id!(cone), script_args!(), |vm, args| {
        let radius = arg_f64(vm, args, 0, 1.0).abs().max(0.001);
        let height = arg_f64(vm, args, 1, 1.0).abs().max(0.001);
        let segments = arg_u32(vm, args, 2, 32).clamp(3, 128);
        let center = arg_bool(vm, args, 3, true);
        new_cad_solid(vm, Solid::cone(radius, height, segments, center))
    });
    vm.add_method(cad, id!(torus), script_args!(), |vm, args| {
        let major_radius = arg_f64(vm, args, 0, 1.0).abs().max(0.001);
        let minor_radius = arg_f64(vm, args, 1, 0.2).abs().max(0.001);
        let major_segments = arg_u32(vm, args, 2, 48).clamp(3, 160);
        let minor_segments = arg_u32(vm, args, 3, 16).clamp(3, 96);
        new_cad_solid(
            vm,
            Solid::torus(major_radius, minor_radius, major_segments, minor_segments),
        )
    });
    vm.add_method(cad, id!(tapered_cylinder), script_args!(), |vm, args| {
        let r1 = arg_f64(vm, args, 0, 1.0).abs();
        let r2 = arg_f64(vm, args, 1, 0.5).abs();
        let height = arg_f64(vm, args, 2, 1.0).abs().max(0.001);
        let segments = arg_u32(vm, args, 3, 32).clamp(3, 128);
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

fn eval_cad_script(cx: &mut Cx, source: &str) -> Result<Solid, String> {
    let code = format!("use mod.std.*\nuse mod.cad.*\n{}", source);
    let script_mod = ScriptMod {
        cargo_manifest_path: String::new(),
        module_path: String::new(),
        file: "cad_editor".to_string(),
        line: 1,
        column: 0,
        code: String::new(),
        values: Vec::new(),
    };
    clear_cad_script_output();
    cx.with_vm(|vm| {
        let value = vm.eval_with_append_source(script_mod, &code, NIL.into());
        if value.is_err() {
            return Err(script_value_to_string(vm, value));
        }
        if let Some(solid) = take_cad_script_output() {
            if !solid.is_empty() {
                return Ok(solid);
            }
        }
        solid_from_value(vm, value).ok_or_else(|| {
            let value = script_value_to_string(vm, value);
            if value.is_empty() || value == "nil" {
                "script did not call render(solid) or return a CAD solid".to_string()
            } else {
                format!("script returned {}, expected a CAD solid", value)
            }
        })
    })
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
                            height: 42.0
                            flow: Right
                            align: Align{x: 0.0 y: 0.5}
                            padding: Inset{left: 14.0 right: 14.0}
                            spacing: 16.0
                            draw_bg +: {color: #x171d24}

                            title_label := Label{
                                width: Fit
                                height: Fit
                                text: "CAD"
                                draw_text +: {
                                    color: #xf3f6f8
                                    text_style +: {font_size: 13.5}
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

fn update_geometry_from_solid(
    cx: &mut Cx,
    geometry: &mut Option<Geometry>,
    solid: &Solid,
) -> CadStats {
    let mesh = solid.mesh();
    let mut stats = CadStats {
        vertices: solid.vertex_count(),
        triangles: solid.triangle_count(),
        max_dimension: 0.0,
    };

    if mesh.vertices.is_empty() || mesh.triangles.is_empty() {
        *geometry = None;
        return stats;
    }

    let bbox = mesh.bounding_box();
    if bbox.is_empty() {
        *geometry = None;
        return stats;
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
        *geometry = None;
    } else {
        let geometry = geometry.get_or_insert_with(|| Geometry::new(cx));
        geometry.update(cx, indices, vertices);
    }

    stats
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

    fn set_solid(&mut self, cx: &mut Cx, solid: &Solid) -> CadStats {
        let stats = update_geometry_from_solid(cx, &mut self.mesh_geometry, solid);
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
}

impl App {
    fn regenerate(&mut self, cx: &mut Cx) {
        let source = self.ui.widget(cx, ids!(cad_editor)).text();
        if source == self.last_source {
            return;
        }
        self.last_source = source.clone();

        match eval_cad_script(cx, &source) {
            Ok(solid) => {
                let stats = if let Some(mut viewport) = self
                    .ui
                    .widget(cx, ids!(cad_viewport))
                    .borrow_mut::<CadViewport>()
                {
                    viewport.set_solid(cx, &solid)
                } else {
                    CadStats::default()
                };
                self.ui.label(cx, ids!(status_label)).set_text(
                    cx,
                    &format!(
                        "{} triangles, {} vertices, bounds {:.2}",
                        stats.triangles, stats.vertices, stats.max_dimension
                    ),
                );
                self.ui.redraw(cx);
            }
            Err(err) => {
                self.ui
                    .label(cx, ids!(status_label))
                    .set_text(cx, &format!("Error: {}", err));
                self.ui.redraw(cx);
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
        self.ui
            .widget(cx, ids!(cad_editor))
            .set_text(cx, DEFAULT_CAD_SCRIPT);
        self.regenerate(cx);
    }

    fn handle_timer(&mut self, cx: &mut Cx, event: &TimerEvent) {
        if self.live_update_timer.is_timer(event).is_some() {
            self.regenerate(cx);
        }
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for action in actions {
            if matches!(action.cast(), CadCodeEditorAction::TextDidChange) {
                self.regenerate(cx);
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
            self.regenerate(cx);
        }
    }
}
