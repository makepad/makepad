use crate::{
    area::Area,
    cx::Cx,
    draw_list::DrawListId,
    id_pool::*,
    makepad_math::*,
    //makepad_live_id::*,
    makepad_script::*,
    os::CxOsPass,
    script::vm::*,
    texture::Texture,
    window::WindowId,
};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};

#[derive(Clone, Default)]
pub(crate) struct GpuTimeQuery {
    samples_ms: Arc<Mutex<VecDeque<(u64, f64)>>>,
    /// App-owned label for the pass content, captured at ENCODE time into
    /// each completion sample. A pass replays on every window repaint, not
    /// only when its owner rebuilt it, so completed durations cannot be
    /// matched to submissions by arrival order — the tag travels with the
    /// command buffer instead.
    tag: Arc<AtomicU64>,
}

// Only backends that report command-buffer timing (Metal today) call the
// recording half; the other backends still compile it.
#[allow(dead_code)]
impl GpuTimeQuery {
    pub(crate) fn record_seconds_tagged(&self, tag: u64, seconds: f64) {
        let ms = seconds * 1000.0;
        if !ms.is_finite() || ms < 0.0 {
            return;
        }
        let mut samples = self
            .samples_ms
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if samples.len() == 1024 {
            samples.pop_front();
        }
        samples.push_back((tag, ms));
    }

    pub(crate) fn set_tag(&self, tag: u64) {
        self.tag.store(tag, Ordering::Relaxed);
    }

    pub(crate) fn current_tag(&self) -> u64 {
        self.tag.load(Ordering::Relaxed)
    }

    fn take_samples(&self) -> Vec<f64> {
        self.take_tagged_samples().into_iter().map(|(_, ms)| ms).collect()
    }

    fn take_tagged_samples(&self) -> Vec<(u64, f64)> {
        self.samples_ms
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .drain(..)
            .collect()
    }
}

#[derive(Debug)]
pub struct DrawPass(PoolId);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DrawPassId(pub(crate) usize);

#[derive(Default)]
pub struct CxDrawPassPool(pub(crate) IdPool<CxDrawPass>);
impl CxDrawPassPool {
    fn alloc(&mut self) -> DrawPass {
        DrawPass(self.0.alloc())
    }

    pub fn id_iter(&self) -> DrawPassIterator {
        DrawPassIterator {
            cur: 0,
            len: self.0.pool.len(),
        }
    }
}

pub struct DrawPassIterator {
    cur: usize,
    len: usize,
}

impl Iterator for DrawPassIterator {
    type Item = DrawPassId;
    fn next(&mut self) -> Option<Self::Item> {
        if self.cur >= self.len {
            return None;
        }
        let cur = self.cur;
        self.cur += 1;
        Some(DrawPassId(cur))
    }
}

impl std::ops::Index<DrawPassId> for CxDrawPassPool {
    type Output = CxDrawPass;
    fn index(&self, index: DrawPassId) -> &Self::Output {
        &self.0.pool[index.0].item
    }
}

impl std::ops::IndexMut<DrawPassId> for CxDrawPassPool {
    fn index_mut(&mut self, index: DrawPassId) -> &mut Self::Output {
        &mut self.0.pool[index.0].item
    }
}

impl ScriptHook for DrawPass {}
impl ScriptNew for DrawPass {
    fn script_new(vm: &mut ScriptVm) -> Self {
        Self::new(vm.cx_mut())
    }
}
impl ScriptApply for DrawPass {
    fn script_apply(
        &mut self,
        _vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
    }
}

impl DrawPass {
    pub fn new(cx: &mut Cx) -> Self {
        let uniforms_gen = cx.next_uniform_gen();
        let pass = cx.passes.alloc();
        cx.passes[pass.draw_pass_id()].pass_uniforms_gen = uniforms_gen;
        pass
    }
}

#[derive(Script)]
pub struct ScriptDrawPass {
    #[rust(DrawPass::new(vm.cx_mut()))]
    pub handle: DrawPass,
    #[live]
    pub clear_color: Vec4f,
    #[live]
    pub dont_clear: bool,
    #[live]
    pub keep_camera_matrix: bool,
}

impl std::ops::Deref for ScriptDrawPass {
    type Target = DrawPass;
    fn deref(&self) -> &Self::Target {
        &self.handle
    }
}

impl ScriptHook for ScriptDrawPass {
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        vm.host.cx_mut().passes[self.handle.draw_pass_id()].clear_color = self.clear_color;
        vm.host.cx_mut().passes[self.handle.draw_pass_id()].dont_clear = self.dont_clear;
        vm.host.cx_mut().passes[self.handle.draw_pass_id()].keep_camera_matrix =
            self.keep_camera_matrix;
    }
}

/*
impl LiveHook for DrawPass {}
impl LiveNew for DrawPass {
    fn live_design_with(_cx:&mut Cx){}
    fn new(cx: &mut Cx) -> Self {
        let pass = cx.passes.alloc();
        pass
    }

    fn live_type_info(_cx: &mut Cx) -> LiveTypeInfo {
        LiveTypeInfo {
            module_id: LiveModuleId::from_str(&module_path!()).unwrap(),
            live_type: LiveType::of::<Self>(),
            fields: Vec::new(),
            live_ignore: true,
            //kind: LiveTypeKind::Object,
            type_name: id_lut!(DrawPass)
        }
    }
}

impl LiveApply for DrawPass {

    fn apply(&mut self, cx: &mut Cx, apply: &Apply, start_index: usize, nodes: &[LiveNode]) -> usize {

        if !nodes[start_index].value.is_structy_type() {
            cx.apply_error_wrong_type_for_struct(live_error_origin!(), start_index, nodes, live_id!(View));
            return nodes.skip_node(start_index);
        }

        let mut index = start_index + 1;
        loop {
            if nodes[index].value.is_close() {
                index += 1;
                break;
            }
            match nodes[index].id {
                live_id!(clear_color) => cx.passes[self.draw_pass_id()].clear_color = LiveNew::new_apply_mut_index(cx, apply, &mut index, nodes),
                live_id!(dont_clear) => cx.passes[self.draw_pass_id()].dont_clear = LiveNew::new_apply_mut_index(cx, apply, &mut index, nodes),
                _ => {
                    cx.apply_error_no_matching_field(live_error_origin!(), index, nodes);
                    index = nodes.skip_node(index);
                }
            }
        }
        return index;
    }
}*/

impl DrawPass {
    pub fn id_equals(&self, id: usize) -> bool {
        self.0.id == id
    }

    pub fn new_with_name(cx: &mut Cx, name: &str) -> Self {
        let pass = Self::new(cx);
        pass.set_pass_name(cx, name);
        pass
    }

    pub fn draw_pass_id(&self) -> DrawPassId {
        DrawPassId(self.0.id)
    }

    pub fn set_as_xr_pass(&self, cx: &mut Cx) {
        let cxpass = &mut cx.passes[self.draw_pass_id()];
        cxpass.parent = CxDrawPassParent::Xr;
    }

    pub fn set_pass_parent(&self, cx: &mut Cx, pass: &DrawPass) {
        let cxpass = &mut cx.passes[self.draw_pass_id()];
        cxpass.parent = CxDrawPassParent::DrawPass(pass.draw_pass_id());
    }

    pub fn set_pass_name(&self, cx: &mut Cx, name: &str) {
        let cxpass = &mut cx.passes[self.draw_pass_id()];
        cxpass.debug_name = name.to_string();
    }

    pub fn pass_name<'a>(&self, cx: &'a mut Cx) -> &'a str {
        let cxpass = &mut cx.passes[self.draw_pass_id()];
        &cxpass.debug_name
    }

    /// This pass re-encodes whenever its consumer repaints (see
    /// `CxDrawPass::live_with_parent`).
    pub fn set_live_with_parent(&self, cx: &mut Cx, on: bool) {
        cx.passes[self.draw_pass_id()].live_with_parent = on;
    }

    pub fn set_size(&self, cx: &mut Cx, pass_size: Vec2d) {
        let mut pass_size = pass_size;
        if pass_size.x < 1.0 {
            pass_size.x = 1.0
        };
        if pass_size.y < 1.0 {
            pass_size.y = 1.0
        };
        let cxpass = &mut cx.passes[self.draw_pass_id()];
        cxpass.pass_rect = Some(CxDrawPassRect::Size(pass_size));
    }

    pub fn size(&self, cx: &mut Cx) -> Option<Vec2d> {
        let cxpass = &mut cx.passes[self.draw_pass_id()];
        if let Some(CxDrawPassRect::Size(size)) = &cxpass.pass_rect {
            return Some(*size);
        }
        None
    }

    pub fn set_window_clear_color(&self, cx: &mut Cx, clear_color: Vec4f) {
        let cxpass = &mut cx.passes[self.draw_pass_id()];
        cxpass.clear_color = clear_color;
    }

    pub fn clear_color_textures(&self, cx: &mut Cx) {
        let cxpass = &mut cx.passes[self.draw_pass_id()];
        cxpass.color_textures.truncate(0);
    }

    pub fn add_color_texture(
        &self,
        cx: &mut Cx,
        texture: &Texture,
        clear_color: DrawPassClearColor,
    ) {
        let cxpass = &mut cx.passes[self.draw_pass_id()];
        cxpass.color_textures.push(CxDrawPassColorTexture {
            texture: texture.clone(),
            clear_color: clear_color,
            cube_face: None,
        })
    }

    pub fn add_color_texture_face(
        &self,
        cx: &mut Cx,
        texture: &Texture,
        cube_face: u32,
        clear_color: DrawPassClearColor,
    ) {
        let cxpass = &mut cx.passes[self.draw_pass_id()];
        cxpass.color_textures.push(CxDrawPassColorTexture {
            texture: texture.clone(),
            clear_color,
            cube_face: Some(cube_face),
        })
    }

    pub fn set_color_texture(
        &self,
        cx: &mut Cx,
        texture: &Texture,
        clear_color: DrawPassClearColor,
    ) {
        let cxpass = &mut cx.passes[self.draw_pass_id()];
        if cxpass.color_textures.len() != 0 {
            cxpass.color_textures[0] = CxDrawPassColorTexture {
                texture: texture.clone(),
                clear_color: clear_color,
                cube_face: None,
            }
        } else {
            cxpass.color_textures.push(CxDrawPassColorTexture {
                texture: texture.clone(),
                clear_color: clear_color,
                cube_face: None,
            })
        }
    }

    pub fn set_color_texture_face(
        &self,
        cx: &mut Cx,
        texture: &Texture,
        cube_face: u32,
        clear_color: DrawPassClearColor,
    ) {
        let cxpass = &mut cx.passes[self.draw_pass_id()];
        if cxpass.color_textures.len() != 0 {
            cxpass.color_textures[0] = CxDrawPassColorTexture {
                texture: texture.clone(),
                clear_color,
                cube_face: Some(cube_face),
            }
        } else {
            cxpass.color_textures.push(CxDrawPassColorTexture {
                texture: texture.clone(),
                clear_color,
                cube_face: Some(cube_face),
            })
        }
    }

    pub fn set_depth_texture(
        &self,
        cx: &mut Cx,
        texture: &Texture,
        clear_depth: DrawPassClearDepth,
    ) {
        let cxpass = &mut cx.passes[self.draw_pass_id()];
        cxpass.depth_texture = Some(texture.clone());
        cxpass.clear_depth = clear_depth;
    }

    pub fn set_debug(&mut self, cx: &mut Cx, debug: bool) {
        let cxpass = &mut cx.passes[self.draw_pass_id()];
        cxpass.debug = debug;
    }

    pub fn set_dpi_factor(&mut self, cx: &mut Cx, dpi: f64) {
        let cxpass = &mut cx.passes[self.draw_pass_id()];
        cxpass.dpi_factor = Some(dpi);
    }

    /// Enable asynchronous backend GPU timing for this pass. Metal records
    /// the command buffer's GPUStartTime/GPUEndTime; unsupported backends
    /// simply leave the sample queue empty.
    pub fn set_gpu_timing_enabled(&self, cx: &mut Cx, enabled: bool) {
        let pass = &mut cx.passes[self.draw_pass_id()];
        if enabled {
            pass.gpu_time_query.get_or_insert_with(GpuTimeQuery::default);
        } else {
            pass.gpu_time_query = None;
        }
    }

    /// Drain completed command-buffer durations without blocking for work
    /// that is still in flight.
    pub fn take_gpu_times_ms(&self, cx: &Cx) -> Vec<f64> {
        cx.passes[self.draw_pass_id()]
            .gpu_time_query
            .as_ref()
            .map_or_else(Vec::new, GpuTimeQuery::take_samples)
    }

    /// Label the pass's CURRENT content; every completion sample encoded
    /// from now on carries this tag (`take_gpu_time_samples`). Repaints
    /// replay a pass without its owner rebuilding it, so arrival order can
    /// never identify what a duration measured — the tag can.
    pub fn set_gpu_time_tag(&self, cx: &Cx, tag: u64) {
        if let Some(q) = cx.passes[self.draw_pass_id()].gpu_time_query.as_ref() {
            q.set_tag(tag);
        }
    }

    /// Drain completed (tag, duration ms) samples.
    pub fn take_gpu_time_samples(&self, cx: &Cx) -> Vec<(u64, f64)> {
        cx.passes[self.draw_pass_id()]
            .gpu_time_query
            .as_ref()
            .map_or_else(Vec::new, GpuTimeQuery::take_tagged_samples)
    }
}

#[derive(Clone)]
pub enum DrawPassClearColor {
    InitWith(Vec4f),
    ClearWith(Vec4f),
}

impl Default for DrawPassClearColor {
    fn default() -> Self {
        Self::ClearWith(Vec4f::default())
    }
}

#[derive(Clone)]
pub enum DrawPassClearDepth {
    InitWith(f32),
    ClearWith(f32),
}

#[derive(Clone)]
pub struct CxDrawPassColorTexture {
    pub clear_color: DrawPassClearColor,
    pub texture: Texture,
    pub cube_face: Option<u32>,
}

#[derive(Default, Clone, Script, ScriptHook)]
#[repr(C)]
pub struct DrawPassUniforms {
    #[live]
    pub camera_projection: Mat4f,
    #[live]
    pub camera_projection_r: Mat4f,
    #[live]
    pub camera_view: Mat4f,
    #[live]
    pub camera_view_r: Mat4f,
    #[live]
    pub depth_projection: Mat4f,
    #[live]
    pub depth_projection_r: Mat4f,
    #[live]
    pub depth_view: Mat4f,
    #[live]
    pub depth_view_r: Mat4f,
    #[live]
    pub camera_inv: Mat4f,
    #[live]
    pub camera_inv_r: Mat4f,
    #[live]
    pub dpi_factor: f32,
    #[live]
    pub dpi_dilate: f32,
    #[live]
    pub time: f32,
    /// App-controlled clock shared by every map draw in this pass. This is
    /// deliberately separate from `time`: reading that field makes the
    /// shader's static `uses_time` scan repaint the pass at display rate.
    #[live]
    pub shiny_time: f32,
}

impl DrawPassUniforms {
    pub fn as_slice(&self) -> &[f32; std::mem::size_of::<DrawPassUniforms>() >> 2] {
        unsafe { std::mem::transmute(self) }
    }
}

#[derive(Clone, Debug)]
pub enum CxDrawPassRect {
    Area(Area),
    AreaOrigin(Area, Vec2d),
    Size(Vec2d),
}

#[derive(Clone)]
pub struct CxDrawPass {
    pub debug: bool,
    pub debug_name: String,
    pub color_textures: Vec<CxDrawPassColorTexture>,
    pub depth_texture: Option<Texture>,
    pub clear_depth: DrawPassClearDepth,
    pub dont_clear: bool,
    pub keep_camera_matrix: bool,
    pub depth_init: f64,
    pub clear_color: Vec4f,
    pub dpi_factor: Option<f64>,
    pub main_draw_list_id: Option<DrawListId>,
    pub parent: CxDrawPassParent,
    pub paint_dirty: bool,
    /// Opt-in liveness: when this pass's CONSUMER (its parent) repaints,
    /// this pass re-encodes too. The gauss blur chain lives on it — glass
    /// blurs the world in realtime instead of holding the last rebuild —
    /// while texture caches, which exist to NOT re-render, stay untouched.
    pub live_with_parent: bool,
    /// The draw list that last declared this pass a dependency through
    /// `make_child_pass`, with that list's redraw id at the time. The parent
    /// link above outlives the frame that made it, but the pass's output is
    /// only consumed while the list that attached it still stands: once that
    /// list is recorded again without re-attaching — the window stopped
    /// capturing the gauss scene, the map stopped baking its shadow mask —
    /// the pass is orphaned and must not be painted (see
    /// `Cx::pass_attachment_is_stale`). `None` for a pass parented by a window
    /// or by hand (`set_pass_parent`, a hand-built chain); those never go
    /// stale.
    pub attached_by: Option<(DrawListId, u64)>,
    /// Set by `Cx::repaint_pass`: the caller asked for this pass by name, so
    /// it paints once even while orphaned (a thumbnail sheet re-executed for
    /// a texture readback). Cleared when the repaint order is computed.
    pub repaint_requested: bool,
    pub pass_rect: Option<CxDrawPassRect>,
    pub view_shift: Vec2d,
    pub view_scale: Vec2d,
    pub pass_uniforms: DrawPassUniforms,
    /// Replaced with a process-wide generation whenever the pass block changes.
    pub pass_uniforms_gen: u64,
    pub zbias_step: f32,
    /// Set while the exploded z-layer view is up on this pass; `None` is
    /// ordinary flat 2D and leaves `camera_view` the identity it always was.
    pub sploded: Option<crate::sploded::SplodedParams>,
    pub os: CxOsPass,
    pub(crate) gpu_time_query: Option<GpuTimeQuery>,
}

impl Default for CxDrawPass {
    fn default() -> Self {
        CxDrawPass {
            debug: false,
            dont_clear: false,
            keep_camera_matrix: false,
            debug_name: String::new(),
            zbias_step: 0.001,
            sploded: None,
            pass_uniforms: DrawPassUniforms::default(),
            pass_uniforms_gen: 0,
            color_textures: Vec::new(),
            depth_texture: None,
            dpi_factor: None,
            clear_depth: DrawPassClearDepth::ClearWith(1.0),
            clear_color: Vec4f::default(),
            depth_init: 1.0,
            main_draw_list_id: None,
            view_shift: dvec2(0.0, 0.0),
            view_scale: dvec2(1.0, 1.0),
            parent: CxDrawPassParent::None,
            paint_dirty: false,
            live_with_parent: false,
            attached_by: None,
            repaint_requested: false,
            pass_rect: None,
            os: CxOsPass::default(),
            gpu_time_query: None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum CxDrawPassParent {
    Xr,
    Window(WindowId),
    DrawPass(DrawPassId),
    None,
}

impl CxDrawPass {
    #[inline]
    pub fn mark_pass_uniforms_dirty(&mut self, uniforms_gen: u64) {
        debug_assert_ne!(uniforms_gen, 0);
        self.pass_uniforms_gen = uniforms_gen;
    }

    pub fn set_time(&mut self, time: f32, uniforms_gen: u64) {
        self.pass_uniforms.time = time;
        self.mark_pass_uniforms_dirty(uniforms_gen);
    }

    pub fn set_dpi_factor(&mut self, dpi_factor: f64, uniforms_gen: u64) {
        let dpi_dilate = (2. - dpi_factor).max(0.).min(1.);
        self.pass_uniforms.dpi_factor = dpi_factor as f32;
        self.pass_uniforms.dpi_dilate = dpi_dilate as f32;
        self.mark_pass_uniforms_dirty(uniforms_gen);
    }

    pub fn set_ortho_matrix(&mut self, offset: Vec2d, size: Vec2d, uniforms_gen: u64) {
        let offset = offset + self.view_shift;
        let size = size * self.view_scale;
        let zero = Mat4f { v: [0.0; 16] };

        let ortho = Mat4f::ortho(
            offset.x as f32,
            (offset.x + size.x) as f32,
            offset.y as f32,
            (offset.y + size.y) as f32,
            100.,
            -100.,
            1.0,
            1.0,
        );
        self.pass_uniforms.camera_projection = ortho;
        // The exploded z-layer view is exactly this one substitution: every 2D
        // vertex ends in `camera_projection * (camera_view * world)`, so a
        // non-identity `camera_view` tilts the whole window's draw-call stack
        // without a single shader edit. See `crate::sploded`.
        self.pass_uniforms.camera_view = match &self.sploded {
            Some(params) => params.camera_view(offset, size),
            None => Mat4f::identity(),
        };
        // Regular 2D passes don't participate in XR scene-depth clipping.
        self.pass_uniforms.depth_projection = zero;
        self.pass_uniforms.depth_projection_r = zero;
        self.pass_uniforms.depth_view = zero;
        self.pass_uniforms.depth_view_r = zero;
        self.pass_uniforms.camera_inv = Mat4f::identity();
        self.pass_uniforms.camera_inv_r = Mat4f::identity();
        self.mark_pass_uniforms_dirty(uniforms_gen);
    }
}

/// The size of a pass's depth attachment, in device pixels.
///
/// A pass rendering into a texture the caller chose takes that texture's
/// allocation: the WM hands its clients power-of-two shared textures larger
/// than the pass. A pass drawing into its own targets, or a window, sizes the
/// depth buffer like its colour buffers, from the pass rect.
// Only the D3D11 backend binds strict-size depth views; the other backends
// clip to the smallest attachment and size depth from the pass.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn depth_attachment_size(
    target_alloc: Option<(usize, usize)>,
    pass_size: DVec2,
    dpi_factor: f64,
) -> (usize, usize) {
    target_alloc.unwrap_or_else(|| {
        let size = pass_size * dpi_factor;
        (size.x as usize, size.y as usize)
    })
}

#[cfg(test)]
mod depth_attachment_size_tests {
    use super::*;

    #[test]
    fn a_window_pass_sizes_depth_from_the_pass_rect() {
        assert_eq!(depth_attachment_size(None, dvec2(1126.0, 680.0), 2.0), (2252, 1360));
        assert_eq!(depth_attachment_size(None, dvec2(800.0, 600.0), 1.0), (800, 600));
    }

    #[test]
    fn a_hosted_pass_sizes_depth_from_the_shared_texture_allocation() {
        // The WM's Windows allocation for a 1126x680 tile at dpi 2.
        assert_eq!(
            depth_attachment_size(Some((4096, 2048)), dvec2(1126.0, 680.0), 2.0),
            (4096, 2048)
        );
    }

    #[test]
    fn the_target_allocation_wins_even_when_smaller_than_the_pass() {
        assert_eq!(
            depth_attachment_size(Some((1024, 512)), dvec2(1126.0, 680.0), 2.0),
            (1024, 512)
        );
    }
}
