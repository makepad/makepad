//! Bounded Splash model programs for the exact polygonal CAD engine.
//!
//! The script surface is the frozen verbs in `apps/sandbox/CSG_API_REVIEW.md`
//! plus its one reviewed follow-up, `csg.implicit`. Calls only build immutable
//! nodes; all geometry and validation remain in `libs/csg`.

use crate::{
    difference_all_with, dvec3, intersection_all_with, union_all_with, FinishParams, Solid, Vec3d,
};

/// Finishing for LocalGen documents. LocalGen authors in metres and
/// legitimate detail routinely reaches the sub-millimetre range (a 1e-4
/// altitude cutoff deleted one closing face from the reviewed 35 mm dog legs
/// after the fourth union), so the finishing tolerances here are finer than
/// the unit-scale library defaults. Topology validation in `mesh_document`
/// stays the final authority.
const LOCALGEN_FINISH: FinishParams = FinishParams {
    tolerance: 1e-5,
    min_altitude: 0.0,
};
use makepad_csg_math::thread_pool;
use makepad_csg_mesh::{mesh::TriMesh, validate::validate_mesh};
use makepad_csg_sdf::{sdf_to_mesh, sdf_to_mesh_ref, Sdf3, SdfSplashExpr};
use makepad_script::{
    math_aot::{MathAot, MathAotParam, MathAotValue},
    numeric::NumericValue,
    *,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

const DEFAULT_COLOR: [f32; 4] = [0.8, 0.8, 0.8, 1.0];
const MIN_DIMENSION: f64 = 0.001;
const MAX_DIMENSION: f64 = 50.0;

#[derive(Clone, Copy, Debug)]
pub struct CsgBudgets {
    pub max_source_bytes: usize,
    pub max_nodes: usize,
    pub max_parts: usize,
    pub max_triangles: usize,
    /// Largest power-of-two grid resolution accepted by `csg.implicit`.
    pub max_implicit_resolution: usize,
    /// One deadline shared by eval and exact meshing.
    pub max_eval_time: Duration,
    pub max_instructions: usize,
    pub max_heap_bytes: usize,
}

impl Default for CsgBudgets {
    fn default() -> Self {
        Self {
            max_source_bytes: 12_000,
            max_nodes: 2_000,
            max_parts: 32,
            max_triangles: 150_000,
            max_implicit_resolution: 128,
            max_eval_time: Duration::from_secs(30),
            max_instructions: 4_000_000,
            max_heap_bytes: 4 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum CsgError {
    Cancelled,
    SourceTooLarge { found: usize, limit: usize },
    Eval(String),
    Budget { what: &'static str, found: usize, limit: usize },
    Invalid(String),
}

impl std::fmt::Display for CsgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("cancelled"),
            Self::SourceTooLarge { found, limit } => write!(f, "source is {found} bytes; maximum is {limit}"),
            Self::Eval(message) | Self::Invalid(message) => f.write_str(message),
            Self::Budget { what, found, limit } => write!(f, "{what} budget exceeded: {found}; maximum is {limit}"),
        }
    }
}
impl std::error::Error for CsgError {}

type NodeId = usize;

#[derive(Clone, Debug)]
enum Primitive {
    Box { size: [f64; 3] },
    Sphere { r: f64, seg: u32 },
    Cylinder { r: f64, r2: Option<f64>, h: f64, seg: u32 },
    Torus { r: f64, tube: f64, seg: u32 },
    Extrude { points: Vec<[f64; 2]>, h: f64, twist: f64, taper: f64, seg: u32 },
    Lathe { profile: Vec<[f64; 2]>, angle: f64, seg: u32 },
    ImplicitPending(usize),
    Implicit { mesh: TriMesh },
}

#[derive(Clone, Copy, Debug)]
enum BooleanOp { Union, Difference, Intersect }

#[derive(Clone, Debug)]
enum NodeKind {
    Primitive(Primitive),
    Boolean { op: BooleanOp, children: Vec<NodeId> },
    Move { child: NodeId, by: [f64; 3] },
    Rotate { child: NodeId, degrees: [f64; 3] },
    Scale { child: NodeId, by: [f64; 3] },
    Mirror { child: NodeId, axis: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CsgAnimKind { Swing, Spin, Bob }
impl CsgAnimKind {
    pub fn as_str(self) -> &'static str {
        match self { Self::Swing => "swing", Self::Spin => "spin", Self::Bob => "bob" }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CsgAxis { X, Y, Z }
impl CsgAxis {
    pub fn as_str(self) -> &'static str {
        match self { Self::X => "x", Self::Y => "y", Self::Z => "z" }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CsgAnimation {
    pub kind: CsgAnimKind,
    pub axis: CsgAxis,
    pub degrees: f32,
    pub hz: f32,
    pub amp: f32,
}

#[derive(Clone, Debug)]
struct Part {
    name: String,
    root: NodeId,
    color: [f32; 4],
    parent: Option<usize>,
    pivot: Option<[f64; 3]>,
    animation: Option<CsgAnimation>,
}

#[derive(Clone, Debug)]
pub struct CsgDocument {
    nodes: Vec<NodeKind>,
    parts: Vec<Part>,
    warnings: Vec<String>,
    budgets: CsgBudgets,
    deadline: Instant,
}
impl CsgDocument {
    pub fn node_count(&self) -> usize { self.nodes.len() }
    pub fn part_count(&self) -> usize { self.parts.len() }
    pub fn part_names(&self) -> impl Iterator<Item = &str> { self.parts.iter().map(|p| p.name.as_str()) }
    pub fn parent_edges(&self) -> impl Iterator<Item = (&str, &str)> {
        self.parts.iter().filter_map(|p| p.parent.map(|i| (p.name.as_str(), self.parts[i].name.as_str())))
    }
    pub fn animations(&self) -> impl Iterator<Item = (&str, CsgAnimation)> {
        self.parts.iter().filter_map(|p| p.animation.map(|a| (p.name.as_str(), a)))
    }
    pub fn warnings(&self) -> &[String] { &self.warnings }
}

#[derive(Clone, Debug)]
pub struct MeshedPart {
    pub name: String,
    pub pivot: [f32; 3],
    pub color: [f32; 4],
    /// Index in `MeshedModel::parts`; parents always precede children.
    pub parent: Option<usize>,
    pub animation: Option<CsgAnimation>,
    pub mesh: TriMesh,
}
#[derive(Clone, Debug, Default)]
pub struct MeshedModel {
    pub parts: Vec<MeshedPart>,
    pub triangles: usize,
    pub warnings: Vec<String>,
}
#[derive(Clone, Debug)]
pub struct PartPreview { pub completed: usize, pub total: usize, pub model: MeshedModel }
#[derive(Clone, Debug)]
pub struct Thumbnail { pub width: u32, pub height: u32, pub rgba: Vec<u8> }

#[derive(Default)]
struct EvalState {
    nodes: Vec<NodeKind>,
    parts: Vec<Part>,
    warnings: Vec<String>,
    error: Option<String>,
    budgets: CsgBudgets,
    implicit: Vec<ImplicitSpec>,
}

#[derive(Clone)]
struct ImplicitSpec {
    function: ScriptFnRef,
    bounds: ([f64; 3], [f64; 3]),
    resolution: usize,
    uniforms: Vec<MathAotValue>,
}
impl EvalState {
    fn fail(&mut self, message: impl Into<String>) -> ScriptValue {
        if self.error.is_none() { self.error = Some(message.into()) }
        NIL
    }
    fn push_node(&mut self, node: NodeKind) -> ScriptValue {
        if self.nodes.len() >= self.budgets.max_nodes {
            return self.fail(format!("csg operation budget exceeded: maximum is {}", self.budgets.max_nodes));
        }
        self.nodes.push(node);
        ScriptValue::from_f64(self.nodes.len() as f64)
    }
    fn node(&self, value: ScriptValue) -> Option<NodeId> {
        let n = value.as_f64()? as usize;
        (n > 0 && n <= self.nodes.len()).then_some(n - 1)
    }
}

fn arg(vm: &mut ScriptVm, args: ScriptObject, index: usize) -> ScriptValue {
    let v = vm.bx.heap.vec_value(args, index, NoTrap);
    if v.is_err() { NIL } else { v }
}
fn value_string(vm: &mut ScriptVm, value: ScriptValue) -> String {
    vm.bx.heap.cast_to_owned_string(value, "copying a CSG string argument").unwrap_or_default()
}
fn value_f64(vm: &mut ScriptVm, value: ScriptValue) -> f64 {
    vm.bx.heap.cast_to_f64(value, vm.bx.threads.cur_ref().trap.ip)
}
fn field(vm: &mut ScriptVm, object: ScriptObject, key: LiveId) -> ScriptValue {
    let v = vm.bx.heap.value(object, key.into(), NoTrap);
    if v.is_err() { NIL } else { v }
}
fn numeric(vm: &mut ScriptVm, value: ScriptValue) -> NumericValue {
    NumericValue::from_script_value_heap(&vm.bx.heap, value, vm.bx.threads.cur_ref().trap.ip)
}
fn vec2(vm: &mut ScriptVm, value: ScriptValue) -> Option<[f64; 2]> {
    match numeric(vm, value) { NumericValue::Vec2(v) => Some([v.x as f64, v.y as f64]), _ => None }
}
fn vec3(vm: &mut ScriptVm, value: ScriptValue) -> Option<[f64; 3]> {
    match numeric(vm, value) { NumericValue::Vec3(v) => Some([v.x as f64, v.y as f64, v.z as f64]), _ => None }
}
fn color(vm: &mut ScriptVm, value: ScriptValue) -> Option<[f32; 4]> {
    match numeric(vm, value) { NumericValue::Color(v) => Some([v.x, v.y, v.z, v.w]), _ => None }
}
fn list_len(vm: &ScriptVm, value: ScriptValue) -> usize {
    value.as_array().map(|a| vm.bx.heap.array_len(a))
        .or_else(|| value.as_object().map(|o| vm.bx.heap.vec_len(o))).unwrap_or(0)
}
fn list_value(vm: &mut ScriptVm, value: ScriptValue, index: usize) -> ScriptValue {
    if let Some(a) = value.as_array() { vm.bx.heap.array_index(a, index, NoTrap) }
    else if let Some(o) = value.as_object() { vm.bx.heap.vec_value(o, index, NoTrap) }
    else { NIL }
}
fn points2(vm: &mut ScriptVm, value: ScriptValue) -> Option<Vec<[f64; 2]>> {
    let len = list_len(vm, value);
    if !(3..=256).contains(&len) { return None }
    (0..len).map(|i| { let v = list_value(vm, value, i); vec2(vm, v) }).collect()
}
fn uniform_value(vm: &mut ScriptVm, value: ScriptValue) -> Option<(MathAotParam, MathAotValue)> {
    match numeric(vm, value) {
        NumericValue::F64(v) if v.is_finite() => {
            Some((MathAotParam::Scalar, MathAotValue::Scalar(v)))
        }
        NumericValue::Vec2(v) if v.x.is_finite() && v.y.is_finite() => Some((
            MathAotParam::Vec2,
            MathAotValue::Vec2([v.x, v.y]),
        )),
        NumericValue::Vec3(v) if v.x.is_finite() && v.y.is_finite() && v.z.is_finite() => Some((
            MathAotParam::Vec3,
            MathAotValue::Vec3([v.x, v.y, v.z]),
        )),
        NumericValue::Vec4(v)
            if v.x.is_finite() && v.y.is_finite() && v.z.is_finite() && v.w.is_finite() =>
        {
            Some((MathAotParam::Vec4, MathAotValue::Vec4([v.x, v.y, v.z, v.w])))
        }
        _ => None,
    }
}
fn warn_unknown_keys(vm: &mut ScriptVm, state: &Rc<RefCell<EvalState>>, verb: &str, opts: ScriptObject, allowed: &[LiveId]) {
    for index in 0..vm.bx.heap.iter_len(opts) {
        let kv = vm.bx.heap.iter_key_value(opts, index, NoTrap);
        let Some(key) = kv.key.as_id() else { continue };
        if !allowed.contains(&key) {
            state.borrow_mut().warnings.push(format!("csg.{verb}: unknown option `{key}` (ignored)"));
        }
    }
}
fn opts(vm: &mut ScriptVm, state: &Rc<RefCell<EvalState>>, args: ScriptObject, index: usize, verb: &str) -> Option<ScriptObject> {
    arg(vm, args, index).as_object().or_else(|| {
        state.borrow_mut().fail(format!("csg.{verb}: expected options object"));
        None
    })
}
fn option_f64(vm: &mut ScriptVm, opts: ScriptObject, key: LiveId, default: f64) -> f64 {
    let v = field(vm, opts, key); if v.is_nil() { default } else { value_f64(vm, v) }
}
fn option_seg(vm: &mut ScriptVm, opts: ScriptObject) -> u32 {
    option_f64(vm, opts, id!(seg), 24.0).clamp(3.0, 64.0) as u32
}
fn dimension(v: f64) -> bool { v.is_finite() && (MIN_DIMENSION..=MAX_DIMENSION).contains(&v) }
fn finite3(v: [f64; 3]) -> bool { v.into_iter().all(f64::is_finite) }
fn valid_name(name: &str) -> bool {
    (1..=24).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
fn push_primitive(state: &Rc<RefCell<EvalState>>, p: Primitive) -> ScriptValue {
    state.borrow_mut().push_node(NodeKind::Primitive(p))
}

fn c_box(vm: &mut ScriptVm, state: &Rc<RefCell<EvalState>>, args: ScriptObject) -> ScriptValue {
    let Some(o) = opts(vm, state, args, 0, "box") else { return NIL };
    warn_unknown_keys(vm, state, "box", o, &[id!(size)]);
    let size_v = field(vm, o, id!(size));
    let Some(size) = vec3(vm, size_v) else { return state.borrow_mut().fail("csg.box: size must be vec3 metres") };
    if !size.into_iter().all(dimension) { return state.borrow_mut().fail("csg.box: size components must be 0.001..50 metres") }
    push_primitive(state, Primitive::Box { size })
}
fn c_sphere(vm: &mut ScriptVm, state: &Rc<RefCell<EvalState>>, args: ScriptObject) -> ScriptValue {
    let Some(o) = opts(vm, state, args, 0, "sphere") else { return NIL };
    warn_unknown_keys(vm, state, "sphere", o, &[id!(r), id!(seg)]);
    let r = option_f64(vm, o, id!(r), 0.5);
    if !dimension(r) { return state.borrow_mut().fail("csg.sphere: r must be 0.001..50 metres") }
    let seg = option_seg(vm, o);
    push_primitive(state, Primitive::Sphere { r, seg })
}
fn c_cylinder(vm: &mut ScriptVm, state: &Rc<RefCell<EvalState>>, args: ScriptObject) -> ScriptValue {
    let Some(o) = opts(vm, state, args, 0, "cylinder") else { return NIL };
    warn_unknown_keys(vm, state, "cylinder", o, &[id!(r), id!(h), id!(r2), id!(seg)]);
    let (r, h) = (option_f64(vm, o, id!(r), 0.5), option_f64(vm, o, id!(h), 1.0));
    let r2v = field(vm, o, id!(r2));
    let r2 = (!r2v.is_nil()).then(|| value_f64(vm, r2v));
    if !dimension(r) || !dimension(h) || r2.is_some_and(|v| v != 0.0 && !dimension(v)) {
        return state.borrow_mut().fail("csg.cylinder: r/h/r2 must be 0.001..50 metres (r2 may be 0)");
    }
    let seg = option_seg(vm, o);
    push_primitive(state, Primitive::Cylinder { r, r2, h, seg })
}
fn c_torus(vm: &mut ScriptVm, state: &Rc<RefCell<EvalState>>, args: ScriptObject) -> ScriptValue {
    let Some(o) = opts(vm, state, args, 0, "torus") else { return NIL };
    warn_unknown_keys(vm, state, "torus", o, &[id!(r), id!(tube), id!(seg)]);
    let (r, tube) = (option_f64(vm, o, id!(r), 0.5), option_f64(vm, o, id!(tube), 0.1));
    if !dimension(r) || !dimension(tube) || tube >= r { return state.borrow_mut().fail("csg.torus: require 0.001 <= tube < r <= 50 metres") }
    let seg = option_seg(vm, o);
    push_primitive(state, Primitive::Torus { r, tube, seg })
}
fn c_extrude(vm: &mut ScriptVm, state: &Rc<RefCell<EvalState>>, args: ScriptObject) -> ScriptValue {
    let points_v = arg(vm, args, 0);
    let Some(points) = points2(vm, points_v) else { return state.borrow_mut().fail("csg.extrude: first argument must be 3..256 vec2(x,z) points") };
    let Some(o) = opts(vm, state, args, 1, "extrude") else { return NIL };
    warn_unknown_keys(vm, state, "extrude", o, &[id!(h), id!(twist), id!(taper), id!(seg)]);
    let (h, twist, taper) = (option_f64(vm, o, id!(h), 1.0), option_f64(vm, o, id!(twist), 0.0), option_f64(vm, o, id!(taper), 1.0));
    if !dimension(h) || !twist.is_finite() || !dimension(taper) || points.iter().flatten().any(|v| !v.is_finite() || v.abs() > MAX_DIMENSION) {
        return state.borrow_mut().fail("csg.extrude: invalid h/twist/taper/profile dimensions");
    }
    let seg = option_seg(vm, o);
    push_primitive(state, Primitive::Extrude { points, h, twist, taper, seg })
}
fn c_lathe(vm: &mut ScriptVm, state: &Rc<RefCell<EvalState>>, args: ScriptObject) -> ScriptValue {
    let profile_v = arg(vm, args, 0);
    let Some(profile) = points2(vm, profile_v) else { return state.borrow_mut().fail("csg.lathe: first argument must be 3..256 vec2(r,y) points") };
    let Some(o) = opts(vm, state, args, 1, "lathe") else { return NIL };
    warn_unknown_keys(vm, state, "lathe", o, &[id!(angle), id!(seg)]);
    let angle = option_f64(vm, o, id!(angle), 360.0);
    if !(0.001..=360.0).contains(&angle) || profile.iter().any(|p| p[0] < 0.0 || p[0] > MAX_DIMENSION || !p[1].is_finite()) {
        return state.borrow_mut().fail("csg.lathe: invalid non-negative profile or angle");
    }
    let seg = option_seg(vm, o);
    push_primitive(state, Primitive::Lathe { profile, angle, seg })
}
fn c_implicit(vm: &mut ScriptVm, state: &Rc<RefCell<EvalState>>, args: ScriptObject) -> ScriptValue {
    let function_value = arg(vm, args, 0);
    let Some(function_object) = function_value.as_object().filter(|o| vm.bx.heap.is_fn(*o)) else {
        return state.borrow_mut().fail("csg.implicit: first argument must be a function");
    };
    let Some(o) = opts(vm, state, args, 1, "implicit") else { return NIL };
    warn_unknown_keys(vm, state, "implicit", o, &[id!(bounds), id!(res), id!(uniforms)]);

    let bounds_value = field(vm, o, id!(bounds));
    if list_len(vm, bounds_value) != 2 {
        return state.borrow_mut().fail("csg.implicit: bounds must be [vec3(min), vec3(max)]");
    }
    let min_value = list_value(vm, bounds_value, 0);
    let max_value = list_value(vm, bounds_value, 1);
    let Some(min) = vec3(vm, min_value).filter(|v| finite3(*v)) else {
        return state.borrow_mut().fail("csg.implicit: bounds min must be a finite vec3");
    };
    let Some(max) = vec3(vm, max_value).filter(|v| finite3(*v)) else {
        return state.borrow_mut().fail("csg.implicit: bounds max must be a finite vec3");
    };
    if (0..3).any(|axis| {
        min[axis] >= max[axis]
            || min[axis].abs() > MAX_DIMENSION
            || max[axis].abs() > MAX_DIMENSION
            || max[axis] - min[axis] < MIN_DIMENSION
    }) {
        return state
            .borrow_mut()
            .fail("csg.implicit: bounds must be ordered, non-degenerate, and within +/-50 metres");
    }

    let res_value = field(vm, o, id!(res));
    let res_number = value_f64(vm, res_value);
    let resolution = res_number as usize;
    let max_resolution = state.borrow().budgets.max_implicit_resolution;
    if !res_number.is_finite()
        || res_number != resolution as f64
        || resolution < 8
        || !resolution.is_power_of_two()
        || resolution > max_resolution
    {
        return state.borrow_mut().fail(format!(
            "csg.implicit: res must be a power of two from 8 through {max_resolution}"
        ));
    }

    let mut uniforms = Vec::new();
    let uniforms_value = field(vm, o, id!(uniforms));
    if !uniforms_value.is_nil() {
        let count = list_len(vm, uniforms_value);
        if count > 16 {
            return state.borrow_mut().fail("csg.implicit: at most 16 uniforms are allowed");
        }
        for index in 0..count {
            let value = list_value(vm, uniforms_value, index);
            let Some((_ty, value)) = uniform_value(vm, value) else {
                return state.borrow_mut().fail(
                    "csg.implicit: uniforms must be finite numbers or vec2/vec3/vec4 values",
                );
            };
            uniforms.push(value);
        }
    }

    let function = vm.bx.heap.new_fn_ref(function_object);
    let mut state = state.borrow_mut();
    let pending = state.implicit.len();
    state.implicit.push(ImplicitSpec {
        function,
        bounds: (min, max),
        resolution,
        uniforms,
    });
    state.push_node(NodeKind::Primitive(Primitive::ImplicitPending(pending)))
}
fn c_boolean(vm: &mut ScriptVm, state: &Rc<RefCell<EvalState>>, args: ScriptObject, op: BooleanOp, verb: &str) -> ScriptValue {
    let len = vm.bx.heap.vec_len(args);
    if !(2..=256).contains(&len) { return state.borrow_mut().fail(format!("csg.{verb}: expected 2..256 solids")) }
    let mut children = Vec::with_capacity(len);
    for index in 0..len {
        let value = arg(vm, args, index);
        let Some(child) = state.borrow().node(value) else { return state.borrow_mut().fail(format!("csg.{verb}: argument {} is not a solid", index + 1)) };
        children.push(child);
    }
    state.borrow_mut().push_node(NodeKind::Boolean { op, children })
}
fn c_move(vm: &mut ScriptVm, state: &Rc<RefCell<EvalState>>, args: ScriptObject) -> ScriptValue {
    let first = arg(vm, args, 0);
    let Some(child) = state.borrow().node(first) else { return state.borrow_mut().fail("csg.move: first argument is not a solid") };
    let second = arg(vm, args, 1);
    let Some(by) = vec3(vm, second).filter(|v| finite3(*v)) else { return state.borrow_mut().fail("csg.move: second argument must be a finite vec3") };
    state.borrow_mut().push_node(NodeKind::Move { child, by })
}
fn c_rotate(vm: &mut ScriptVm, state: &Rc<RefCell<EvalState>>, args: ScriptObject) -> ScriptValue {
    let first = arg(vm, args, 0);
    let Some(child) = state.borrow().node(first) else { return state.borrow_mut().fail("csg.rotate: first argument is not a solid") };
    let Some(o) = opts(vm, state, args, 1, "rotate") else { return NIL };
    warn_unknown_keys(vm, state, "rotate", o, &[id!(x), id!(y), id!(z)]);
    let degrees = [option_f64(vm, o, id!(x), 0.0), option_f64(vm, o, id!(y), 0.0), option_f64(vm, o, id!(z), 0.0)];
    if !finite3(degrees) { return state.borrow_mut().fail("csg.rotate: x/y/z must be finite degrees") }
    state.borrow_mut().push_node(NodeKind::Rotate { child, degrees })
}
fn c_scale(vm: &mut ScriptVm, state: &Rc<RefCell<EvalState>>, args: ScriptObject) -> ScriptValue {
    let first = arg(vm, args, 0);
    let Some(child) = state.borrow().node(first) else { return state.borrow_mut().fail("csg.scale: first argument is not a solid") };
    let value = arg(vm, args, 1);
    let by = value.as_f64().map(|n| [n; 3]).or_else(|| vec3(vm, value));
    let Some(by) = by.filter(|v| v.iter().copied().all(dimension)) else { return state.borrow_mut().fail("csg.scale: scale must be positive number or vec3") };
    state.borrow_mut().push_node(NodeKind::Scale { child, by })
}
fn c_mirror(vm: &mut ScriptVm, state: &Rc<RefCell<EvalState>>, args: ScriptObject) -> ScriptValue {
    let first = arg(vm, args, 0);
    let Some(child) = state.borrow().node(first) else { return state.borrow_mut().fail("csg.mirror: first argument is not a solid") };
    let axis_value = arg(vm, args, 1);
    let axis = match value_string(vm, axis_value).as_str() { "x" => 0, "y" => 1, "z" => 2, _ => return state.borrow_mut().fail("csg.mirror: axis must be x, y, or z") };
    state.borrow_mut().push_node(NodeKind::Mirror { child, axis })
}
fn c_part(vm: &mut ScriptVm, state: &Rc<RefCell<EvalState>>, args: ScriptObject) -> ScriptValue {
    let name_value = arg(vm, args, 0);
    let name = value_string(vm, name_value);
    if !valid_name(&name) { return state.borrow_mut().fail("csg.part: name must match [a-z0-9-]{1,24}") }
    let solid_value = arg(vm, args, 1);
    let Some(root) = state.borrow().node(solid_value) else { return state.borrow_mut().fail("csg.part: second argument is not a solid") };
    if state.borrow().parts.len() >= state.borrow().budgets.max_parts {
        let limit = state.borrow().budgets.max_parts;
        return state.borrow_mut().fail(format!("csg part budget exceeded: maximum is {limit}"));
    }
    if state.borrow().parts.iter().any(|p| p.name == name) { return state.borrow_mut().fail(format!("csg.part: duplicate part '{name}'")) }
    let mut color_value = DEFAULT_COLOR;
    let (mut parent, mut pivot) = (None, None);
    if let Some(o) = arg(vm, args, 2).as_object() {
        warn_unknown_keys(vm, state, "part", o, &[id!(color), id!(parent), id!(pivot)]);
        let cv = field(vm, o, id!(color));
        if !cv.is_nil() {
            let Some(c) = color(vm, cv) else { return state.borrow_mut().fail("csg.part: color must be #rrggbb") };
            color_value = c;
        }
        let pv = field(vm, o, id!(parent));
        if !pv.is_nil() {
            let parent_name = value_string(vm, pv);
            parent = state.borrow().parts.iter().position(|p| p.name == parent_name);
            if parent.is_none() { return state.borrow_mut().fail(format!("csg.part: parent '{parent_name}' must be declared first")) }
        }
        let qv = field(vm, o, id!(pivot));
        if !qv.is_nil() {
            let Some(p) = vec3(vm, qv).filter(|v| finite3(*v)) else { return state.borrow_mut().fail("csg.part: pivot must be a finite vec3") };
            pivot = Some(p);
        }
    }
    state.borrow_mut().parts.push(Part { name, root, color: color_value, parent, pivot, animation: None });
    NIL
}
fn c_anim(vm: &mut ScriptVm, state: &Rc<RefCell<EvalState>>, args: ScriptObject) -> ScriptValue {
    let name_value = arg(vm, args, 0);
    let name = value_string(vm, name_value);
    let Some(index) = state.borrow().parts.iter().position(|p| p.name == name) else { return state.borrow_mut().fail(format!("csg.anim: unknown part '{name}'")) };
    let Some(o) = opts(vm, state, args, 1, "anim") else { return NIL };
    warn_unknown_keys(vm, state, "anim", o, &[id!(kind), id!(axis), id!(degrees), id!(hz), id!(amp)]);
    let kind_value = field(vm, o, id!(kind));
    let kind = match value_string(vm, kind_value).as_str() { "swing" => CsgAnimKind::Swing, "spin" => CsgAnimKind::Spin, "bob" => CsgAnimKind::Bob, _ => return state.borrow_mut().fail("csg.anim: kind must be swing, spin, or bob") };
    let av = field(vm, o, id!(axis));
    let axis_name = if av.is_nil() { "x".into() } else { value_string(vm, av) };
    let axis = match axis_name.as_str() { "x" => CsgAxis::X, "y" => CsgAxis::Y, "z" => CsgAxis::Z, _ => return state.borrow_mut().fail("csg.anim: axis must be x, y, or z") };
    let (degrees, hz, amp) = (option_f64(vm, o, id!(degrees), 25.0), option_f64(vm, o, id!(hz), 2.0), option_f64(vm, o, id!(amp), 0.1));
    if !degrees.is_finite() || !hz.is_finite() || hz <= 0.0 || !amp.is_finite() || amp < 0.0 { return state.borrow_mut().fail("csg.anim: degrees/amp must be finite and hz positive") }
    state.borrow_mut().parts[index].animation = Some(CsgAnimation { kind, axis, degrees: degrees as f32, hz: hz as f32, amp: amp as f32 });
    NIL
}

struct CsgApiGc;
impl ScriptHandleGc for CsgApiGc { fn gc(&mut self) {} }

struct DeadlineSdf<T> {
    inner: T,
    deadline: Instant,
    stopped: Arc<AtomicBool>,
}

impl<T: Sdf3> Sdf3 for DeadlineSdf<T> {
    fn distance(&self, point: Vec3d) -> f64 {
        if thread_pool::cancelled() || Instant::now() >= self.deadline {
            self.stopped.store(true, Ordering::Relaxed);
            f64::INFINITY
        } else {
            self.inner.distance(point)
        }
    }
}

struct InterpreterSdf<'vm, 'host> {
    vm: RefCell<&'vm mut ScriptVm<'host>>,
    function: ScriptValue,
    uniforms: Vec<MathAotValue>,
    deadline: Instant,
    instructions: Cell<usize>,
    heap_limit: usize,
    failed: Cell<bool>,
}

fn math_value_to_script(vm: &mut ScriptVm, value: MathAotValue) -> ScriptValue {
    let numeric = match value {
        MathAotValue::Scalar(value) => NumericValue::F64(value),
        MathAotValue::Vec2(value) => NumericValue::Vec2(makepad_script::makepad_math::Vec2f {
            x: value[0],
            y: value[1],
        }),
        MathAotValue::Vec3(value) => NumericValue::Vec3(makepad_script::makepad_math::Vec3f {
            x: value[0],
            y: value[1],
            z: value[2],
        }),
        MathAotValue::Vec4(value) => NumericValue::Vec4(makepad_script::makepad_math::Vec4f {
            x: value[0],
            y: value[1],
            z: value[2],
            w: value[3],
        }),
    };
    let bx = &mut *vm.bx;
    numeric.to_script_value_heap(&mut bx.heap, &bx.code)
}

impl Sdf3 for InterpreterSdf<'_, '_> {
    fn distance(&self, point: Vec3d) -> f64 {
        let remaining = self.instructions.get();
        if remaining == 0 || thread_pool::cancelled() || Instant::now() >= self.deadline {
            self.failed.set(true);
            return f64::INFINITY;
        }
        let mut vm = self.vm.borrow_mut();
        let mut args = Vec::with_capacity(self.uniforms.len() + 1);
        args.push(math_value_to_script(
            &mut vm,
            MathAotValue::Vec3([point.x as f32, point.y as f32, point.z as f32]),
        ));
        args.extend(
            self.uniforms
                .iter()
                .copied()
                .map(|value| math_value_to_script(&mut vm, value)),
        );
        let function = self.function;
        let ((result, _allocation), consumed) = {
            let (result, allocation) = vm.with_heap_allocation_limit(self.heap_limit, |vm| {
                vm.with_instruction_limit(remaining, |vm| vm.call(function, &args))
            });
            let consumed = vm.last_limit_consumed();
            ((result, allocation), consumed)
        };
        self.instructions.set(remaining.saturating_sub(consumed));
        for value in args {
            vm.release_transient(value);
        }
        let Some(value) = result.as_number().filter(|value| value.is_finite()) else {
            self.failed.set(true);
            return f64::INFINITY;
        };
        value
    }
}

fn math_param(value: MathAotValue) -> MathAotParam {
    match value {
        MathAotValue::Scalar(_) => MathAotParam::Scalar,
        MathAotValue::Vec2(_) => MathAotParam::Vec2,
        MathAotValue::Vec3(_) => MathAotParam::Vec3,
        MathAotValue::Vec4(_) => MathAotParam::Vec4,
    }
}

fn mesh_implicit(
    vm: &mut ScriptVm,
    spec: &ImplicitSpec,
    budgets: CsgBudgets,
    deadline: Instant,
    interpreter_instructions: usize,
) -> Result<TriMesh, CsgError> {
    if thread_pool::cancelled() {
        return Err(CsgError::Cancelled);
    }
    if Instant::now() >= deadline {
        return Err(CsgError::Budget { what: "eval-time", found: 1, limit: 0 });
    }
    let min = dvec3(spec.bounds.0[0], spec.bounds.0[1], spec.bounds.0[2]);
    let max = dvec3(spec.bounds.1[0], spec.bounds.1[1], spec.bounds.1[2]);
    let depth = spec.resolution.ilog2() as usize;
    let function: ScriptValue = spec.function.clone().into();
    let uniform_types: Vec<_> = spec.uniforms.iter().copied().map(math_param).collect();
    let aot = MathAot::new(vm);
    let mesh = if let Some(compiled) =
        aot.compile(vm, function, &[MathAotParam::Vec3], &uniform_types)
    {
        let mut field = SdfSplashExpr::new(compiled.into_inner());
        field.set_uniforms(spec.uniforms.clone());
        let stopped = Arc::new(AtomicBool::new(false));
        let field = DeadlineSdf { inner: field, deadline, stopped: stopped.clone() };
        let mesh = sdf_to_mesh(field, min, max, depth);
        if stopped.load(Ordering::Relaxed) {
            if thread_pool::cancelled() {
                return Err(CsgError::Cancelled);
            }
            return Err(CsgError::Budget { what: "eval-time", found: 1, limit: 0 });
        }
        mesh
    } else {
        let field = InterpreterSdf {
            vm: RefCell::new(vm),
            function,
            uniforms: spec.uniforms.clone(),
            deadline,
            instructions: Cell::new(interpreter_instructions),
            heap_limit: budgets.max_heap_bytes,
            failed: Cell::new(false),
        };
        let mesh = sdf_to_mesh_ref(&field, min, max, depth);
        if field.failed.get() {
            if thread_pool::cancelled() {
                return Err(CsgError::Cancelled);
            }
            if Instant::now() >= deadline || field.instructions.get() == 0 {
                return Err(CsgError::Budget { what: "eval-time", found: 1, limit: 0 });
            }
            return Err(CsgError::Eval("csg.implicit: interpreter function returned a non-finite number".into()));
        }
        mesh
    };
    if mesh.triangle_count() > budgets.max_triangles {
        return Err(CsgError::Budget {
            what: "triangle",
            found: mesh.triangle_count(),
            limit: budgets.max_triangles,
        });
    }
    Ok(mesh)
}

/// Evaluate in a new worker-local VM. Only Splash core/math and `csg` are
/// registered; filesystem, network, process, world and game capabilities do
/// not exist in this VM.
pub fn evaluate_program(source: &str, budgets: CsgBudgets) -> Result<CsgDocument, CsgError> {
    if source.len() > budgets.max_source_bytes { return Err(CsgError::SourceTooLarge { found: source.len(), limit: budgets.max_source_bytes }) }
    if thread_pool::cancelled() { return Err(CsgError::Cancelled) }
    let started = Instant::now();
    let deadline = started.checked_add(budgets.max_eval_time).unwrap_or(started);
    let state = Rc::new(RefCell::new(EvalState { budgets, ..Default::default() }));
    let (mut host, mut std) = ((), ());
    let mut vm = ScriptVm { host: &mut host, std: &mut std, bx: Box::new(ScriptVmBase::new()) };
    let api_type = vm.new_handle_type(id_lut!(csg));
    let dispatch = state.clone();
    vm.set_handle_call(api_type, move |vm, args, method| match method {
        id if id == id!(box) => c_box(vm, &dispatch, args),
        id if id == id!(sphere) => c_sphere(vm, &dispatch, args),
        id if id == id!(cylinder) => c_cylinder(vm, &dispatch, args),
        id if id == id!(torus) => c_torus(vm, &dispatch, args),
        id if id == id!(extrude) => c_extrude(vm, &dispatch, args),
        id if id == id!(lathe) => c_lathe(vm, &dispatch, args),
        id if id == id!(implicit) => c_implicit(vm, &dispatch, args),
        id if id == id!(union) => c_boolean(vm, &dispatch, args, BooleanOp::Union, "union"),
        id if id == id!(difference) => c_boolean(vm, &dispatch, args, BooleanOp::Difference, "difference"),
        id if id == id!(intersect) => c_boolean(vm, &dispatch, args, BooleanOp::Intersect, "intersect"),
        id if id == id!(move) => c_move(vm, &dispatch, args),
        id if id == id!(rotate) => c_rotate(vm, &dispatch, args),
        id if id == id!(scale) => c_scale(vm, &dispatch, args),
        id if id == id!(mirror) => c_mirror(vm, &dispatch, args),
        id if id == id!(part) => c_part(vm, &dispatch, args),
        id if id == id!(anim) => c_anim(vm, &dispatch, args),
        _ => dispatch.borrow_mut().fail(format!("unknown csg verb '{method}'")),
    });
    let handle = vm.bx.heap.new_handle(api_type, Box::new(CsgApiGc));
    vm.set_injected_global(id!(csg), handle.into());
    vm.bx.captured_errors = Some(Vec::new());
    vm.bx.run_budget = Some(ScriptRunBudget {
        soft_deadline: deadline,
        hard_deadline: deadline,
        sample_interval_instructions: 1_024,
        instructions_until_sample: 1_024,
    });
    let script = ScriptMod { file: "model.csg.splash".into(), line: 0, column: 1, code: format!("use mod.std.*\nuse mod.math.*\nuse mod.pod.*\n{source}\n;"), ..Default::default() };
    let _ = vm.with_heap_allocation_limit(budgets.max_heap_bytes, |vm| vm.with_instruction_limit(budgets.max_instructions, |vm| vm.eval(script)));
    let interpreter_instructions = budgets
        .max_instructions
        .saturating_sub(vm.last_limit_consumed());
    let mut errors = vm.take_errors();
    vm.bx.captured_errors = Some(Vec::new());
    let mut implicit_error = None;
    if errors.is_empty() && state.borrow().error.is_none() {
        let implicit = state.borrow().implicit.clone();
        for (pending, spec) in implicit.iter().enumerate() {
            match mesh_implicit(&mut vm, spec, budgets, deadline, interpreter_instructions) {
                Ok(mesh) => {
                    let mut state = state.borrow_mut();
                    if let Some(node) = state.nodes.iter_mut().find(|node| {
                        matches!(node, NodeKind::Primitive(Primitive::ImplicitPending(index)) if *index == pending)
                    }) {
                        *node = NodeKind::Primitive(Primitive::Implicit { mesh });
                    }
                }
                Err(error) => {
                    implicit_error = Some(error);
                    break;
                }
            }
        }
    }
    errors.extend(vm.take_errors());
    vm.bx.run_budget = None;
    drop(vm);
    let mut state = Rc::try_unwrap(state).map_err(|_| CsgError::Eval("CSG evaluator retained its state".into()))?.into_inner();
    if thread_pool::cancelled() { return Err(CsgError::Cancelled) }
    if let Some(error) = implicit_error { return Err(error) }
    if let Some(error) = state.error.take() { return Err(CsgError::Eval(error)) }
    if !errors.is_empty() { return Err(CsgError::Eval(errors.join("\n"))) }
    if state.parts.is_empty() { return Err(CsgError::Invalid("program declared no csg.part".into())) }
    if Instant::now() >= deadline { return Err(CsgError::Budget { what: "eval-time", found: 1, limit: 0 }) }
    Ok(CsgDocument { nodes: state.nodes, parts: state.parts, warnings: state.warnings, budgets, deadline })
}

fn check_running(document: &CsgDocument) -> Result<(), CsgError> {
    if thread_pool::cancelled() { Err(CsgError::Cancelled) }
    else if Instant::now() >= document.deadline { Err(CsgError::Budget { what: "eval-time", found: 1, limit: 0 }) }
    else { Ok(()) }
}
fn primitive_solid(p: &Primitive) -> Solid {
    match p {
        Primitive::Box { size } => Solid::cube(size[0], size[1], size[2], false).translate(-size[0] * 0.5, 0.0, -size[2] * 0.5),
        Primitive::Sphere { r, seg } => Solid::sphere(*r, *seg, (*seg / 2).max(6)),
        Primitive::Cylinder { r, r2, h, seg } => match r2 { None => Solid::cylinder(*r, *h, *seg, false), Some(0.0) => Solid::cone(*r, *h, *seg, false), Some(r2) => Solid::tapered_cylinder(*r, *r2, *h, *seg, false) },
        Primitive::Torus { r, tube, seg } => Solid::torus(*r, *tube, *seg, (*seg / 2).max(8)),
        Primitive::Extrude { points, h, twist, taper, seg } => if *twist == 0.0 && *taper == 1.0 { Solid::extrude(points, *h) } else { Solid::linear_extrude(points, *h, *twist, *taper, (*seg / 2).max(2)) },
        Primitive::Lathe { profile, angle, seg } => Solid::rotate_extrude(profile, *angle, *seg),
        Primitive::Implicit { mesh } => Solid::from_mesh(mesh.clone()),
        Primitive::ImplicitPending(_) => Solid::empty(),
    }
}
fn mesh_node(document: Arc<CsgDocument>, id: NodeId) -> Result<Solid, CsgError> {
    check_running(&document)?;
    let node = document.nodes.get(id).cloned().ok_or_else(|| CsgError::Invalid("bad CSG node".into()))?;
    let solid = match node {
        NodeKind::Primitive(p) => primitive_solid(&p),
        NodeKind::Boolean { op, children } => {
            let tasks = children.into_iter().map(|child| { let document = document.clone(); move || mesh_node(document, child) }).collect();
            let solids = thread_pool::parallel_for(tasks).into_iter().collect::<Result<Vec<_>, _>>()?;
            check_running(&document)?;
            match op { BooleanOp::Union => union_all_with(&solids, LOCALGEN_FINISH), BooleanOp::Difference => difference_all_with(&solids, LOCALGEN_FINISH), BooleanOp::Intersect => intersection_all_with(&solids, LOCALGEN_FINISH) }
        }
        NodeKind::Move { child, by } => mesh_node(document.clone(), child)?.translate(by[0], by[1], by[2]),
        NodeKind::Rotate { child, degrees } => mesh_node(document.clone(), child)?.rotate_x(degrees[0]).rotate_y(degrees[1]).rotate_z(degrees[2]),
        NodeKind::Scale { child, by } => mesh_node(document.clone(), child)?.scale(by[0], by[1], by[2]),
        NodeKind::Mirror { child, axis } => mesh_node(document.clone(), child)?.mirror(axis),
    };
    check_running(&document)?;
    Ok(solid)
}

/// Exact-mesh named parts in declaration order. Boolean children fan out on
/// the shared CAD pool; every completed top-level part is a preview stage.
pub fn mesh_document(document: CsgDocument, mut preview: impl FnMut(PartPreview)) -> Result<MeshedModel, CsgError> {
    let document = Arc::new(document);
    let mut model = MeshedModel { warnings: document.warnings.clone(), ..Default::default() };
    let total = document.parts.len();
    for part in document.parts.iter().cloned() {
        check_running(&document)?;
        let solid = mesh_node(document.clone(), part.root)?;
        if solid.is_empty() { return Err(CsgError::Invalid(format!("csg.part '{}': boolean result is empty", part.name))) }
        let found = model.triangles.saturating_add(solid.triangle_count());
        if found > document.budgets.max_triangles { return Err(CsgError::Budget { what: "triangle", found, limit: document.budgets.max_triangles }) }
        let mesh = solid.into_mesh();
        let report = validate_mesh(&mesh);
        if !report.is_closed || !report.is_manifold || !report.is_consistently_oriented {
            return Err(CsgError::Invalid(format!("csg.part '{}': mesh is not a closed consistently-oriented manifold (boundary {}, non-manifold {})", part.name, report.boundary_edges, report.non_manifold_edges)));
        }
        let pivot = part.pivot.unwrap_or_else(|| { let b = mesh.bounding_box(); let c = (b.min + b.max) * 0.5; [c.x, c.y, c.z] });
        model.triangles = found;
        model.parts.push(MeshedPart { name: part.name, pivot: [pivot[0] as f32, pivot[1] as f32, pivot[2] as f32], color: part.color, parent: part.parent, animation: part.animation, mesh });
        preview(PartPreview { completed: model.parts.len(), total, model: model.clone() });
    }
    Ok(model)
}

fn bounds(model: &MeshedModel) -> Option<(Vec3d, Vec3d)> {
    let (mut min, mut max, mut any) = (dvec3(f64::INFINITY, f64::INFINITY, f64::INFINITY), dvec3(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY), false);
    for part in &model.parts { for &v in &part.mesh.vertices { min.x=min.x.min(v.x); min.y=min.y.min(v.y); min.z=min.z.min(v.z); max.x=max.x.max(v.x); max.y=max.y.max(v.y); max.z=max.z.max(v.z); any=true; } }
    any.then_some((min, max))
}

/// Deterministic CAD-engine thumbnail, with no ground/debug rig.
pub fn render_thumbnail(model: &MeshedModel, size: u32) -> Result<Thumbnail, CsgError> {
    let size = size.clamp(256, 1024);
    let Some((min, max)) = bounds(model) else { return Err(CsgError::Invalid("cannot thumbnail an empty model".into())) };
    let center = (min + max) * 0.5;
    let right = dvec3(0.70710678, 0.0, -0.70710678);
    let up = dvec3(-0.29883624, 0.90630779, -0.29883624);
    let forward = right.cross(up);
    let mut projected = Vec::new(); let mut extent: f64 = 1e-9;
    for part in &model.parts {
        projected.push(part.mesh.vertices.iter().map(|&v| { let p=v-center; let q=[p.dot(right),p.dot(up),p.dot(forward)]; extent=extent.max(q[0].abs()).max(q[1].abs()); q }).collect::<Vec<_>>());
    }
    let scale=size as f64*0.42/extent; let count=size as usize*size as usize;
    let mut rgba=vec![0u8;count*4]; let mut zbuf=vec![f64::NEG_INFINITY;count];
    for pixel in rgba.chunks_exact_mut(4) { pixel.copy_from_slice(&[25,31,43,255]) }
    let light=dvec3(-0.35,0.8,0.48).normalize();
    for (pi,part) in model.parts.iter().enumerate() { let points=&projected[pi];
        for (ti,&[ia,ib,ic]) in part.mesh.triangles.iter().enumerate() {
            if thread_pool::cancelled() { return Err(CsgError::Cancelled) }
            let (a,b,c)=(points[ia as usize],points[ib as usize],points[ic as usize]);
            let sx=|p:[f64;3]|size as f64*0.5+p[0]*scale; let sy=|p:[f64;3]|size as f64*0.52-p[1]*scale;
            let (ax,ay,bx,by,cx,cy)=(sx(a),sy(a),sx(b),sy(b),sx(c),sy(c)); let area=(bx-ax)*(cy-ay)-(by-ay)*(cx-ax); if area.abs()<1e-12 {continue}
            let (x0,x1,y0,y1)=(ax.min(bx).min(cx).floor().max(0.0) as u32,ax.max(bx).max(cx).ceil().min((size-1) as f64) as u32,ay.min(by).min(cy).floor().max(0.0) as u32,ay.max(by).max(cy).ceil().min((size-1) as f64) as u32);
            let (va,vb,vc)=part.mesh.triangle_vertices(ti); let n=(vb-va).cross(vc-va).normalize(); let shade=(0.28+0.72*n.dot(light).abs()).clamp(0.0,1.0) as f32;
            let color=[(part.color[0].clamp(0.0,1.0)*shade*255.0)as u8,(part.color[1].clamp(0.0,1.0)*shade*255.0)as u8,(part.color[2].clamp(0.0,1.0)*shade*255.0)as u8,255];
            for y in y0..=y1 { for x in x0..=x1 { let(px,py)=(x as f64+0.5,y as f64+0.5); let w0=((bx-px)*(cy-py)-(by-py)*(cx-px))/area; let w1=((cx-px)*(ay-py)-(cy-py)*(ax-px))/area; let w2=1.0-w0-w1; if w0< -1e-8||w1< -1e-8||w2< -1e-8{continue} let depth=a[2]*w0+b[2]*w1+c[2]*w2; let index=y as usize*size as usize+x as usize; if depth>zbuf[index]{zbuf[index]=depth;rgba[index*4..index*4+4].copy_from_slice(&color)} } }
        }
    }
    Ok(Thumbnail { width:size, height:size, rgba })
}

#[cfg(test)]
mod tests {
    use super::*;
    const MUG: &str = r#"
let outer = csg.cylinder({r: 0.045, h: 0.09})
let handle = csg.move(csg.rotate(csg.torus({r: 0.03, tube: 0.008}), {x: 90}), vec3(0.055, 0.045, 0))
let bore = csg.move(csg.cylinder({r: 0.038, h: 0.09}), vec3(0, 0.008, 0))
csg.part("mug", csg.difference(csg.union(outer, handle), bore), {color: #4477aa})
"#;
    const DOG: &str = r#"
let torso = csg.move(csg.box({size: vec3(0.5, 0.22, 0.24)}), vec3(0, 0.18, 0))
let leg = csg.cylinder({r: 0.035, h: 0.2})
let body = csg.union(torso, csg.move(leg, vec3(0.17, 0, 0.08)), csg.move(leg, vec3(0.17, 0, -0.08)), csg.move(leg, vec3(-0.17, 0, 0.08)), csg.move(leg, vec3(-0.17, 0, -0.08)))
csg.part("body", body, {color: #8b5a2b})
let skull = csg.move(csg.sphere({r: 0.11}), vec3(0.3, 0.42, 0))
let muzzle = csg.move(csg.box({size: vec3(0.12, 0.08, 0.1)}), vec3(0.38, 0.34, 0))
csg.part("head", csg.union(skull, muzzle), {color: #8b5a2b, parent: "body", pivot: vec3(0.26, 0.4, 0)})
csg.part("nose", csg.move(csg.sphere({r: 0.025, seg: 12}), vec3(0.45, 0.4, 0)), {color: #1a1a1a, parent: "head"})
let ear = csg.box({size: vec3(0.03, 0.1, 0.05)})
csg.part("ear-l", csg.move(ear, vec3(0.27, 0.44, 0.09)), {color: #5a351d, parent: "head", pivot: vec3(0.27, 0.54, 0.09)})
csg.part("ear-r", csg.move(ear, vec3(0.27, 0.44, -0.09)), {color: #5a351d, parent: "head", pivot: vec3(0.27, 0.54, -0.09)})
let tail = csg.rotate(csg.cylinder({r: 0.025, r2: 0.008, h: 0.18}), {z: 40})
csg.part("tail", csg.move(tail, vec3(-0.25, 0.36, 0)), {color: #8b5a2b, parent: "body", pivot: vec3(-0.25, 0.36, 0)})
csg.anim("ear-l", {kind: "swing", axis: "x", degrees: 25, hz: 1.2})
csg.anim("ear-r", {kind: "swing", axis: "x", degrees: 25, hz: 1.2})
csg.anim("tail", {kind: "swing", axis: "y", degrees: 40, hz: 3})
"#;
    #[test] fn reviewed_mug_is_one_valid_polygonal_part() {
        let document=evaluate_program(MUG,CsgBudgets::default()).unwrap(); assert_eq!(document.part_names().collect::<Vec<_>>(),["mug"]);
        let model=mesh_document(document,|_|{}).unwrap(); assert_eq!(model.parts.len(),1); assert!((500..=8_000).contains(&model.triangles),"{}",model.triangles); assert!(validate_mesh(&model.parts[0].mesh).is_manifold);
        let b=model.parts[0].mesh.bounding_box(); assert!((b.min.y-0.0).abs()<0.002,"{:?}",b); assert!((b.max.y-0.09).abs()<0.002,"{:?}",b);
    }
    #[test] fn reviewed_dog_keeps_hierarchy_colors_and_animation_manifest() {
        let document=evaluate_program(DOG,CsgBudgets::default()).unwrap();
        assert_eq!(document.part_names().collect::<Vec<_>>(),["body","head","nose","ear-l","ear-r","tail"]);
        assert_eq!(document.parent_edges().collect::<Vec<_>>(),[("head","body"),("nose","head"),("ear-l","head"),("ear-r","head"),("tail","body")]); assert_eq!(document.animations().count(),3);
        // The exact reviewed mapping currently produces 1,494 triangles.
        // Keep a narrow lower guard without adding geometry solely to hit a
        // round estimate in the review document.
        let model=mesh_document(document,|_|{}).unwrap(); assert!((1_450..=20_000).contains(&model.triangles),"{}",model.triangles); assert!(model.parts.iter().all(|p|validate_mesh(&p.mesh).is_manifold));
    }
    #[test] fn exact_union_closes_each_reviewed_leg_stage() {
        for count in 1..=4 {
            let moves = [
                "csg.move(leg, vec3(0.17, 0, 0.08))",
                "csg.move(leg, vec3(0.17, 0, -0.08))",
                "csg.move(leg, vec3(-0.17, 0, 0.08))",
                "csg.move(leg, vec3(-0.17, 0, -0.08))",
            ];
            let source = format!("let torso=csg.move(csg.box({{size:vec3(0.5,0.22,0.24)}}),vec3(0,0.18,0))\nlet leg=csg.cylinder({{r:0.035,h:0.2}})\ncsg.part(\"body\",csg.union(torso,{}))", moves[..count].join(","));
            let document = evaluate_program(&source, CsgBudgets::default()).unwrap();
            if let Err(error) = mesh_document(document, |_| {}) { panic!("stage {count}: {error}") }
        }
    }
    #[test] fn budgets_timeout_and_cancel_fail_closed() {
        let mut b=CsgBudgets::default();b.max_nodes=1;assert!(matches!(evaluate_program(DOG,b),Err(CsgError::Eval(_))));
        let mut b=CsgBudgets::default();b.max_triangles=10;let d=evaluate_program(DOG,b).unwrap();assert!(matches!(mesh_document(d,|_|{}),Err(CsgError::Budget{what:"triangle",..})));
        let mut b=CsgBudgets::default();b.max_eval_time=Duration::from_millis(2);assert!(matches!(evaluate_program("while true {}",b),Err(CsgError::Eval(_))));
        let token=thread_pool::CancelToken::new();token.cancel();let result=thread_pool::with_cancel(&token,||evaluate_program(MUG,CsgBudgets::default()));assert_eq!(result.unwrap_err(),CsgError::Cancelled);
    }
    #[test] fn warnings_and_thumbnail_are_real_results() {
        let source=MUG.replace("{r: 0.045, h: 0.09}","{r: 0.045, h: 0.09, typo: 1}");let d=evaluate_program(&source,CsgBudgets::default()).unwrap();assert_eq!(d.warnings().len(),1);
        let model=mesh_document(d,|_|{}).unwrap();let t=render_thumbnail(&model,256).unwrap();assert_eq!(t.rgba.len(),256*256*4);assert!(t.rgba.chunks_exact(4).any(|p|p != [25,31,43,255]));
    }
    #[test]
    fn implicit_smooth_blend_is_a_regular_colored_part() {
        let source = r#"
let field = |p, c, k| {
    let a = length(p - c) - 0.55
    let b = length(p + c) - 0.55
    let h = clamp(0.5 + 0.5 * (b - a) / k, 0.0, 1.0)
    mix(b, a, h) - k * h * (1.0 - h)
}
let blob = csg.implicit(field, {bounds: [vec3(-1, -0.8, -0.8), vec3(1, 0.8, 0.8)], res: 32, uniforms: [vec3(0.32, 0, 0), 0.22]})
csg.part("blend", blob, {color: #55aadd})
csg.part("plinth", csg.move(csg.box({size: vec3(1.4, 0.08, 0.7)}), vec3(0, -0.72, 0)), {color: #334455})
"#;
        let document = evaluate_program(source, CsgBudgets::default()).unwrap();
        assert_eq!(document.part_names().collect::<Vec<_>>(), ["blend", "plinth"]);
        let model = mesh_document(document, |_| {}).unwrap();
        assert_eq!(model.parts.len(), 2);
        assert!(model.parts[0].mesh.triangle_count() > 100);
        assert_eq!(model.parts[0].color, [0x55 as f32 / 255.0, 0xaa as f32 / 255.0, 0xdd as f32 / 255.0, 1.0]);
        assert!(model.parts.iter().all(|part| validate_mesh(&part.mesh).is_manifold));
    }

    #[test]
    fn implicit_rejected_by_aot_falls_back_to_the_splash_interpreter() {
        let source = r#"
let radius = |p| length(p)
let field = |p| radius(p) - 0.5
let ball = csg.implicit(field, {bounds: [vec3(-0.7, -0.7, -0.7), vec3(0.7, 0.7, 0.7)], res: 16})
csg.part("ball", ball, {})
"#;
        let model = mesh_document(
            evaluate_program(source, CsgBudgets::default()).unwrap(),
            |_| {},
        )
        .unwrap();
        assert!(model.triangles > 100);
        assert!(validate_mesh(&model.parts[0].mesh).is_manifold);
    }

    #[test]
    fn implicit_resolution_uses_the_document_budget() {
        let source = r#"
let field = |p| length(p) - 0.5
csg.part("ball", csg.implicit(field, {bounds: [vec3(-1), vec3(1)], res: 64}), {})
"#;
        let mut budgets = CsgBudgets::default();
        budgets.max_implicit_resolution = 32;
        let error = evaluate_program(source, budgets).unwrap_err().to_string();
        assert!(error.contains("res must be a power of two from 8 through 32"), "{error}");

        let mut budgets = CsgBudgets::default();
        budgets.max_triangles = 10;
        assert!(matches!(
            evaluate_program(source, budgets),
            Err(CsgError::Budget { what: "triangle", found, limit: 10 }) if found > 10
        ));
    }
}
