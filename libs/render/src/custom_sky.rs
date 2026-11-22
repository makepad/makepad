//! A host's pixel function over the colour of a stock sky background.

use crate::renderer::ibl::{self, DrawEnvBackground};
use crate::shaders::{DrawSceneSky, DrawSceneSkyAnalytic};
use makepad_draw::*;
use makepad_render_material::{diagnose, HookSet, MaterialError};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.shared.*

    mod.draw.SkyPaint = {
        params: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        // The first three floats point to the sun. The fourth selects the scene's colour encoding.
        sky_frame: uniform(vec4(0.0, 1.0, 0.0, 1.0))
        sky_sun: fn() -> vec3 { return self.sky_frame.xyz }
        sky_time: fn() -> float { return self.draw_pass.time }
        sky_ray: fn() -> vec3 { return normalize(self.v_dir) }

        sky_color: fn(dir: vec3, stock: vec3) -> vec3 {
            var base = stock
            if self.sky_frame.w < 0.5 { base = srgb_to_linear(stock) }
            let painted = self.sky_paint(dir, base)
            if self.sky_frame.w < 0.5 { return linear_to_srgb(painted) }
            return painted
        }
        pixel: fn() {
            return vec4(self.sky_color(self.sky_ray(), self.sky_stock().xyz), 1.0)
        }
        sky_hatch: fn() {
            let dir = self.sky_ray()
            let stripe = step(0.5, fract((dir.x + dir.y + dir.z) * 32.0))
            return vec4(mix(vec3(1.0, 0.0, 1.0), vec3(0.12, 0.0, 0.12), stripe), 1.0)
        }
    }

    mod.draw.SkyGradientPaint = mod.draw.DrawSceneSky {
        ..mod.draw.SkyPaint
        sky_stock: mod.draw.DrawSceneSky.pixel
    }
    mod.draw.SkyAnalyticPaint = mod.draw.DrawSceneSkyAnalytic {
        ..mod.draw.SkyPaint
        sky_stock: mod.draw.DrawSceneSkyAnalytic.pixel
    }
    mod.draw.SkyEnvironmentPaint = mod.draw.DrawEnvBackground {
        ..mod.draw.SkyPaint
        sky_stock: mod.draw.DrawEnvBackground.pixel
        sky_ray: fn() -> vec3 {
            // The map rotates its lookup after reconstructing this world ray.
            let v = vec4(self.v_ndc.x * self.bg.z, self.v_ndc.y * self.bg.w, -1.0, 0.0)
            return normalize((self.draw_pass.camera_inv * v).xyz)
        }
    }
}

/// A background a host wants to paint. Each uses its own stock geometry and bindings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkyKind {
    Gradient,
    Analytic,
    Environment,
}

struct SkyVariant<T> {
    draw: T,
    hatch: T,
    // Both objects belong to the VM that compiled them. Keep their functions alive.
    _program: ScriptObjectRef,
    _hatch_program: ScriptObjectRef,
}

fn build_draw<T: ScriptNew>(
    vm: &mut ScriptVm,
    base: ScriptObject,
    method: LiveId,
    function: ScriptObject,
    vars: fn(&mut T) -> &mut DrawVars,
) -> Result<(T, ScriptObjectRef), MaterialError> {
    let obj = vm.heap_mut().new_with_proto_no_vec(base.into());
    let result = vm.set_value(obj, method, function);
    if !result.is_nil() || vm.object_method(obj, method).as_object() != Some(function) {
        return Err(MaterialError::new(format!("could not install sky method `{method}`")));
    }
    let program = vm.new_object_ref(obj);
    let mut draw = T::script_new(vm);
    // Construction can apply the stock shader. Only the candidate may satisfy this check.
    vars(&mut draw).draw_shader_id = None;
    vars(&mut draw).geometry_id = None;
    draw.script_apply(vm, &Apply::New, &mut Scope::empty(), obj.into());
    if !vars(&mut draw).can_instance() {
        let detail = makepad_draw::makepad_platform::shader_error::take();
        diagnose(vm, obj, &HookSet::new())?;
        return Err(MaterialError::new(detail.unwrap_or_else(|| "sky shader did not build".into())));
    }
    Ok((draw, program))
}

fn build_variant<T: ScriptNew>(
    vm: &mut ScriptVm,
    module: ScriptObject,
    name: LiveId,
    pixel: ScriptObject,
    vars: fn(&mut T) -> &mut DrawVars,
) -> Result<Box<SkyVariant<T>>, MaterialError> {
    let base = vm.value(module, name).as_object()
        .ok_or_else(|| MaterialError::new(format!("missing sky shader `{name}`")))?;
    let hatch = vm.object_method(base, id!(sky_hatch)).as_object()
        .ok_or_else(|| MaterialError::new("missing sky hatch"))?;
    let (draw, program) = build_draw(vm, base, id!(sky_paint), pixel, vars)?;
    let (hatch, hatch_program) = build_draw(vm, base, id!(pixel), hatch, vars)?;
    Ok(Box::new(SkyVariant { draw, hatch, _program: program, _hatch_program: hatch_program }))
}

/// Compiled on the host's VM. Only requested backgrounds get a draw and an error hatch.
pub struct SkyHook {
    gradient: Option<Box<SkyVariant<DrawSceneSky>>>,
    analytic: Option<Box<SkyVariant<DrawSceneSkyAnalytic>>>,
    environment: Option<Box<SkyVariant<DrawEnvBackground>>>,
    params: Vec4f,
    failed: Option<DrawShaderId>,
    reported: bool,
}

impl SkyHook {
    /// Compile a function taking the unit world direction and linear stock RGB.
    /// Its result is linear RGB in both the HDR and display-space scene paths.
    pub fn build(
        vm: &mut ScriptVm,
        pixel: ScriptObject,
        kinds: &[SkyKind],
        params: Vec4f,
    ) -> Result<Self, MaterialError> {
        if !vm.is_fn(pixel) || vm.heap().vec_ref(pixel).iter().filter(|v| !v.key.is_nil()).count() != 2 {
            return Err(MaterialError::new("pixel must be fn(dir: vec3, base: vec3) -> vec3"));
        }
        let function = vm.new_object_ref(pixel);
        let module = vm.module(id!(draw));
        if vm.value(module, id!(SkyEnvironmentPaint)).as_object().is_none() {
            DrawSceneSky::register_domes(vm);
            makepad_render_graph::pass_stdlib(vm);
            if vm.heap().type_default_for_id(DrawEnvBackground::script_type_id_static()).is_none() {
                makepad_render_material::builtin::register(vm);
                ibl::script_mod(vm);
            }
            script_mod(vm);
        }
        let pixel = function.as_object();
        let mut hook = Self { gradient: None, analytic: None, environment: None, params, failed: None, reported: false };
        if kinds.contains(&SkyKind::Gradient) {
            hook.gradient = Some(build_variant(vm, module, id!(SkyGradientPaint), pixel, |d: &mut DrawSceneSky| &mut d.cube.draw_vars)?);
        }
        if kinds.contains(&SkyKind::Analytic) {
            hook.analytic = Some(build_variant(vm, module, id!(SkyAnalyticPaint), pixel, |d: &mut DrawSceneSkyAnalytic| &mut d.cube.draw_vars)?);
        }
        if kinds.contains(&SkyKind::Environment) {
            hook.environment = Some(build_variant(vm, module, id!(SkyEnvironmentPaint), pixel, |d: &mut DrawEnvBackground| &mut d.draw_vars)?);
        }
        Ok(hook)
    }

    pub(crate) fn set_params(&mut self, params: Vec4f) {
        self.params = params;
    }

    pub(crate) fn take_failure<'a>(&mut self, cx: &'a Cx) -> Option<&'a str> {
        if self.reported { return None; }
        if self.failed.is_none() {
            let shaders = [
                self.gradient.as_ref().and_then(|v| v.draw.cube.draw_vars.draw_shader_id),
                self.analytic.as_ref().and_then(|v| v.draw.cube.draw_vars.draw_shader_id),
                self.environment.as_ref().and_then(|v| v.draw.draw_vars.draw_shader_id),
            ];
            self.failed = shaders.into_iter().flatten().find(|&id| cx.draw_shader_failure(id).is_some());
        }
        let log = cx.draw_shader_failure(self.failed?)?;
        self.reported = true;
        Some(log)
    }

    pub(crate) fn gradient(&mut self, cx: &Cx, frame: Vec4f) -> Option<&mut DrawSceneSky> {
        Some(self.gradient.as_mut()?.select(cx, self.params, frame, &mut self.failed, |d| &mut d.cube.draw_vars))
    }

    pub(crate) fn analytic(&mut self, cx: &Cx, frame: Vec4f) -> Option<&mut DrawSceneSkyAnalytic> {
        Some(self.analytic.as_mut()?.select(cx, self.params, frame, &mut self.failed, |d| &mut d.cube.draw_vars))
    }

    pub(crate) fn environment(&mut self, cx: &Cx, frame: Vec4f) -> Option<&mut DrawEnvBackground> {
        Some(self.environment.as_mut()?.select(cx, self.params, frame, &mut self.failed, |d| &mut d.draw_vars))
    }
}

impl<T> SkyVariant<T> {
    fn select(
        &mut self, cx: &Cx, params: Vec4f, frame: Vec4f, failed: &mut Option<DrawShaderId>,
        vars: fn(&mut T) -> &mut DrawVars,
    ) -> &mut T {
        let vars = vars(&mut self.draw);
        if failed.is_none() {
            *failed = vars.draw_shader_id.filter(|&id| cx.draw_shader_failure(id).is_some());
        }
        if failed.is_some() { return &mut self.hatch; }
        vars.set_uniform(cx, live_id!(params), &[params.x, params.y, params.z, params.w]);
        vars.set_uniform(cx, live_id!(sky_frame), &[frame.x, frame.y, frame.z, frame.w]);
        &mut self.draw
    }
}
