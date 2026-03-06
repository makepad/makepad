use crate::{
    cx::Cx,
    draw_pass::DrawPassId,
    draw_shader::{CxDrawShader, CxDrawShaderMapping, CxDrawShaderOptions, DrawShaderId},
    draw_vars::{
        DrawVars, DRAW_CALL_DYN_UNIFORMS, DRAW_CALL_TEXTURE_SLOTS,
        DRAW_CALL_UNIFORM_BUFFER_SLOTS,
    },
    geometry::GeometryId,
    id_pool::*,
    makepad_error_log::*,
    makepad_live_id::LiveId,
    makepad_math::*,
    makepad_script::*,
    os::{CxOsDrawCall, CxOsDrawList},
    script::vm::*,
    texture::{Texture, TextureFormat, TextureId, TextureUpdated},
    uniform_buffer::UniformBuffer,
};
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug)]
pub struct DrawList(PoolId);

impl DrawList {
    pub fn new(cx: &mut Cx) -> Self {
        cx.draw_lists.alloc()
    }
}

impl ScriptHook for DrawList {}
impl ScriptApply for DrawList {}
impl ScriptNew for DrawList {
    fn script_new(vm: &mut ScriptVm) -> Self {
        Self::new(vm.cx_mut())
    }
}

#[derive(Clone, Debug, PartialEq, Copy, Hash, Ord, PartialOrd, Eq)]
pub struct DrawListId(usize, u64);

#[derive(Clone, Copy, Debug, Default)]
pub struct GpuPassMetrics {
    pub draw_calls: u64,
    pub instances: u64,
    pub vertices: u64,
    pub instance_bytes: u64,
    pub uniform_bytes: u64,
    pub vertex_buffer_bytes: u64,
    pub texture_bytes: u64,
}

impl Cx {
    pub fn collect_gpu_pass_metrics(&self, draw_pass_id: DrawPassId) -> GpuPassMetrics {
        let mut metrics = GpuPassMetrics::default();
        let mut uploaded_geometries = HashSet::new();
        let mut uploaded_textures = Vec::<TextureId>::new();
        let Some(draw_list_id) = self.passes[draw_pass_id].main_draw_list_id else {
            return metrics;
        };
        self.collect_gpu_metrics_for_draw_list(
            draw_list_id,
            draw_pass_id,
            &mut metrics,
            &mut uploaded_geometries,
            &mut uploaded_textures,
        );
        metrics
    }

    fn estimate_texture_upload_bytes(&self, texture_id: TextureId) -> u64 {
        let cx_texture = &self.textures[texture_id];
        match &cx_texture.format {
            TextureFormat::VecBGRAu8_32 {
                width,
                height,
                updated,
                ..
            } => {
                if let TextureUpdated::Empty = updated {
                    0
                } else {
                    (*width as u64)
                        .saturating_mul(*height as u64)
                        .saturating_mul(4)
                }
            }
            TextureFormat::VecCubeBGRAu8_32 {
                width,
                height,
                updated,
                ..
            } => {
                if let TextureUpdated::Empty = updated {
                    0
                } else {
                    (*width as u64)
                        .saturating_mul(*height as u64)
                        .saturating_mul(4)
                        .saturating_mul(6)
                }
            }
            TextureFormat::VecMipBGRAu8_32 {
                width,
                height,
                updated,
                ..
            } => {
                if let TextureUpdated::Empty = updated {
                    0
                } else {
                    (*width as u64)
                        .saturating_mul(*height as u64)
                        .saturating_mul(4)
                }
            }
            TextureFormat::VecRGBAf32 {
                width,
                height,
                updated,
                ..
            } => {
                if let TextureUpdated::Empty = updated {
                    0
                } else {
                    (*width as u64)
                        .saturating_mul(*height as u64)
                        .saturating_mul(16)
                }
            }
            TextureFormat::VecRu8 {
                width,
                height,
                updated,
                ..
            } => {
                if let TextureUpdated::Empty = updated {
                    0
                } else {
                    (*width as u64).saturating_mul(*height as u64)
                }
            }
            TextureFormat::VecRGu8 {
                width,
                height,
                updated,
                ..
            } => {
                if let TextureUpdated::Empty = updated {
                    0
                } else {
                    (*width as u64)
                        .saturating_mul(*height as u64)
                        .saturating_mul(2)
                }
            }
            TextureFormat::VecRf32 {
                width,
                height,
                updated,
                ..
            } => {
                if let TextureUpdated::Empty = updated {
                    0
                } else {
                    (*width as u64)
                        .saturating_mul(*height as u64)
                        .saturating_mul(4)
                }
            }
            _ => 0,
        }
    }

    fn collect_gpu_metrics_for_draw_list(
        &self,
        draw_list_id: DrawListId,
        draw_pass_id: DrawPassId,
        metrics: &mut GpuPassMetrics,
        uploaded_geometries: &mut HashSet<GeometryId>,
        uploaded_textures: &mut Vec<TextureId>,
    ) {
        let draw_list = &self.draw_lists[draw_list_id];
        for order_index in 0..draw_list.draw_item_order_len() {
            let Some(draw_item_id) = draw_list.draw_item_id_at_order_index(order_index) else {
                continue;
            };
            let draw_item = &draw_list.draw_items[draw_item_id];
            if let Some(sub_list_id) = draw_item.kind.sub_list() {
                self.collect_gpu_metrics_for_draw_list(
                    sub_list_id,
                    draw_pass_id,
                    metrics,
                    uploaded_geometries,
                    uploaded_textures,
                );
                continue;
            }
            let Some(draw_call) = draw_item.kind.draw_call() else {
                continue;
            };

            let sh = &self.draw_shaders[draw_call.draw_shader_id.index];
            let instance_slots = sh.mapping.instances.total_slots;
            if instance_slots == 0 {
                continue;
            }
            let instance_count = draw_item
                .instances
                .as_ref()
                .map_or(0usize, |instances| instances.len() / instance_slots);
            if instance_count == 0 {
                continue;
            }

            let Some(geometry_id) = draw_call.geometry_id else {
                continue;
            };
            let geometry = &self.geometries[geometry_id];
            let index_count = geometry.indices.len() as u64;

            metrics.draw_calls = metrics.draw_calls.saturating_add(1);
            metrics.instances = metrics.instances.saturating_add(instance_count as u64);
            metrics.vertices = metrics
                .vertices
                .saturating_add(index_count.saturating_mul(instance_count as u64));

            if draw_call.instance_dirty {
                metrics.instance_bytes = metrics.instance_bytes.saturating_add(
                    (draw_item.instances.as_ref().map_or(0usize, Vec::len) * 4) as u64,
                );
            }

            // OpenGL/Android fallback estimate: count per-draw uniform uploads for VS+FS.
            let uniform_f32s = draw_call.draw_call_uniforms.as_slice().len()
                + self.passes[draw_pass_id].pass_uniforms.as_slice().len()
                + draw_list.draw_list_uniforms.as_slice().len()
                + draw_call.dyn_uniforms.len()
                + sh.mapping.scope_uniforms_buf.len();
            metrics.uniform_bytes = metrics
                .uniform_bytes
                .saturating_add((uniform_f32s as u64).saturating_mul(4).saturating_mul(2));

            if uploaded_geometries.insert(geometry_id) {
                if geometry.dirty_vertices {
                    metrics.vertex_buffer_bytes = metrics
                        .vertex_buffer_bytes
                        .saturating_add((geometry.vertices.len() * 4) as u64);
                }
                if geometry.dirty_indices {
                    metrics.vertex_buffer_bytes = metrics
                        .vertex_buffer_bytes
                        .saturating_add((geometry.indices.len() * 4) as u64);
                }
            }

            for texture in draw_call.texture_slots.iter().flatten() {
                let texture_id = texture.texture_id();
                if uploaded_textures
                    .iter()
                    .any(|existing| *existing == texture_id)
                {
                    continue;
                }
                uploaded_textures.push(texture_id);
                metrics.texture_bytes = metrics
                    .texture_bytes
                    .saturating_add(self.estimate_texture_upload_bytes(texture_id));
            }
        }
    }
}

impl DrawListId {
    pub fn index(&self) -> usize {
        self.0
    }
    pub fn generation(&self) -> u64 {
        self.1
    }
}

impl DrawList {
    pub fn id(&self) -> DrawListId {
        DrawListId(self.0.id, self.0.generation)
    }
}

#[derive(Default)]
pub struct CxDrawListPool(pub(crate) IdPool<CxDrawList>);
impl CxDrawListPool {
    pub fn alloc(&mut self) -> DrawList {
        DrawList(self.0.alloc())
    }

    pub fn checked_index(&self, index: DrawListId) -> Option<&CxDrawList> {
        let d = &self.0.pool[index.0];
        if d.generation != index.1 {
            return None;
        }
        return Some(&d.item);
    }
}

impl std::ops::Index<DrawListId> for CxDrawListPool {
    type Output = CxDrawList;
    fn index(&self, index: DrawListId) -> &Self::Output {
        let d = &self.0.pool[index.0];
        if d.generation != index.1 {
            error!(
                "Drawlist id generation wrong index: {} current gen:{} in pointer:{}",
                index.0, d.generation, index.1
            )
        }
        &d.item
    }
}

impl std::ops::IndexMut<DrawListId> for CxDrawListPool {
    fn index_mut(&mut self, index: DrawListId) -> &mut Self::Output {
        let d = &mut self.0.pool[index.0];
        if d.generation != index.1 {
            error!(
                "Drawlist id generation wrong {} {} {}",
                index.0, d.generation, index.1
            )
        }
        &mut d.item
    }
}

#[derive(Default, Clone, Script, ScriptHook)]
#[repr(C)]
pub struct DrawCallUniforms {
    #[live]
    pub zbias: f32,
    #[live]
    pub pad1: f32,
    #[live]
    pub pad2: f32,
    #[live]
    pub pad3: f32,
}

impl DrawCallUniforms {
    pub fn as_slice(&self) -> &[f32; std::mem::size_of::<DrawCallUniforms>()] {
        unsafe { std::mem::transmute(self) }
    }
    /*
    pub fn get_local_scroll(&self) -> Vec4f {
        self.draw_scroll
    }*/

    pub fn set_zbias(&mut self, zbias: f32) {
        self.zbias = zbias;
    }
    /*
    pub fn set_clip(&mut self, clip: (Vec2f, Vec2f)) {
        self.draw_clip_x1 = clip.0.x;
        self.draw_clip_y1 = clip.0.y;
        self.draw_clip_x2 = clip.1.x;
        self.draw_clip_y2 = clip.1.y;
    }

    pub fn set_local_scroll(&mut self, scroll: Vec2f, local_scroll: Vec2f, options: &CxDrawShaderOptions) {
        self.draw_scroll.x = scroll.x;
        if !options.no_h_scroll {
            self.draw_scroll.x += local_scroll.x;
        }
        self.draw_scroll.y = scroll.y;
        if !options.no_v_scroll {
            self.draw_scroll.y += local_scroll.y;
        }
        self.draw_scroll.z = local_scroll.x;
        self.draw_scroll.w = local_scroll.y;
    }*/
}

pub enum CxDrawKind {
    SubList(DrawListId),
    DrawCall(CxDrawCall),
    Empty,
}

pub struct CxDrawItem {
    pub redraw_id: u64,
    pub kind: CxDrawKind,
    // these values stick around to reduce buffer churn
    pub draw_item_id: usize,
    pub instances: Option<Vec<f32>>,
    pub os: CxOsDrawCall,
}

impl std::ops::Deref for CxDrawItem {
    type Target = CxDrawKind;
    fn deref(&self) -> &Self::Target {
        &self.kind
    }
}

impl CxDrawKind {
    pub fn is_empty(&self) -> bool {
        match self {
            CxDrawKind::Empty => true,
            _ => false,
        }
    }

    pub fn sub_list(&self) -> Option<DrawListId> {
        match self {
            CxDrawKind::SubList(id) => Some(*id),
            _ => None,
        }
    }
    pub fn draw_call(&self) -> Option<&CxDrawCall> {
        match self {
            CxDrawKind::DrawCall(call) => Some(call),
            _ => None,
        }
    }
    pub fn draw_call_mut(&mut self) -> Option<&mut CxDrawCall> {
        match self {
            CxDrawKind::DrawCall(call) => Some(call),
            _ => None,
        }
    }
}

pub struct CxDrawCall {
    pub draw_shader_id: DrawShaderId, // if shader_id changed, delete gl vao
    pub options: CxDrawShaderOptions,
    pub append_group_id: u64,
    pub total_instance_slots: usize,
    pub draw_call_uniforms: DrawCallUniforms, // draw uniforms
    pub geometry_id: Option<GeometryId>,
    pub dyn_uniforms: [f32; DRAW_CALL_DYN_UNIFORMS], // user uniforms
    pub texture_slots: [Option<Texture>; DRAW_CALL_TEXTURE_SLOTS],
    pub uniform_buffer_slots: [Option<UniformBuffer>; DRAW_CALL_UNIFORM_BUFFER_SLOTS],
    pub instance_dirty: bool,
    pub uniforms_dirty: bool,
}

impl CxDrawCall {
    pub fn new(mapping: &CxDrawShaderMapping, draw_vars: &DrawVars) -> Self {
        CxDrawCall {
            geometry_id: draw_vars.geometry_id,
            options: draw_vars.options.clone(),
            append_group_id: draw_vars.append_group_id,
            draw_shader_id: draw_vars.draw_shader_id.unwrap(),
            total_instance_slots: mapping.instances.total_slots,
            draw_call_uniforms: DrawCallUniforms::default(),
            dyn_uniforms: draw_vars.dyn_uniforms,
            texture_slots: draw_vars.texture_slots.clone(),
            uniform_buffer_slots: draw_vars.uniform_buffer_slots.clone(),
            instance_dirty: true,
            uniforms_dirty: true,
        }
    }
}

#[derive(Clone, Script, ScriptHook)]
#[repr(C)]
pub struct DrawListUniforms {
    #[live]
    pub view_transform: Mat4f,
    #[live]
    pub view_clip: Vec4f,
    #[live]
    pub view_shift: Vec2f,
    #[live]
    pub pad1: f32,
    #[live]
    pub pad2: f32,
}

impl Default for DrawListUniforms {
    fn default() -> Self {
        Self {
            view_transform: Mat4f::identity(),
            view_clip: vec4(-100000.0, -100000.0, 100000.0, 100000.0),
            view_shift: vec2(0.0, 0.0),
            pad1: 0.0,
            pad2: 0.0,
        }
    }
}

impl DrawListUniforms {
    pub fn as_slice(&self) -> &[f32; std::mem::size_of::<DrawListUniforms>()] {
        unsafe { std::mem::transmute(self) }
    }
}

#[derive(Default)]
pub struct CxDrawItems {
    pub(crate) buffer: Vec<CxDrawItem>,
    used: usize,
}

impl std::ops::Index<usize> for CxDrawItems {
    type Output = CxDrawItem;
    fn index(&self, index: usize) -> &Self::Output {
        &self.buffer[index]
    }
}

impl std::ops::IndexMut<usize> for CxDrawItems {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.buffer[index]
    }
}

impl CxDrawItems {
    pub fn len(&self) -> usize {
        self.used
    }
    pub fn clear(&mut self) {
        self.used = 0
    }
    pub fn push_item(&mut self, redraw_id: u64, kind: CxDrawKind) -> &mut CxDrawItem {
        let draw_item_id = self.used;
        if self.used >= self.buffer.len() {
            self.buffer.push(CxDrawItem {
                draw_item_id,
                redraw_id,
                instances: Some(Vec::new()),
                os: CxOsDrawCall::default(),
                kind: kind,
            });
        } else {
            // reuse an older one, keeping all GPU resources attached
            let draw_item = &mut self.buffer[draw_item_id];
            draw_item.instances.as_mut().unwrap().clear();
            draw_item.kind = kind;
            draw_item.redraw_id = redraw_id;
        }
        self.used += 1;
        &mut self.buffer[draw_item_id]
    }
}

#[derive(Default)]
pub struct CxDrawList {
    pub debug_id: LiveId,
    pub debug_dump: bool,
    pub debug_dump_count: u32,

    pub codeflow_parent_id: Option<DrawListId>, // the id of the parent we nest in, codeflow wise

    pub redraw_id: u64,
    pub draw_pass_id: Option<DrawPassId>,

    pub draw_items: CxDrawItems,
    pub draw_item_reorder: Option<Vec<usize>>,

    pub draw_list_uniforms: DrawListUniforms,
    pub draw_list_has_clip: bool,

    pub os: CxOsDrawList,
    pub rect_areas: Vec<CxRectArea>,
    pub find_appendable_draw_shader_check: Vec<u64>,
}

pub struct CxRectArea {
    pub rect: Rect,
    pub draw_clip: (Vec2d, Vec2d),
}

impl CxDrawList {
    fn append_trace_enabled() -> bool {
        false
    }

    fn append_trace_log(message: String) {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        if !Self::append_trace_enabled() {
            return;
        }
        let n = COUNT.fetch_add(1, Ordering::Relaxed);
        if n < 200 {
            log!("{}", message);
        } else if n == 200 {
            log!("append_trace: log limit reached, suppressing further output");
        }
    }

    #[inline]
    fn group_base(group: u64) -> u64 {
        group >> 8
    }

    #[inline]
    fn group_lane(group: u64) -> u8 {
        (group & 0xff) as u8
    }

    #[inline]
    fn can_cross_group_barrier(
        target_group: u64,
        target_draw_call_group: u64,
        barrier_group: u64,
        barrier_draw_call_group: u64,
    ) -> bool {
        let target_base = Self::group_base(target_group);
        let barrier_base = Self::group_base(barrier_group);
        if target_base != barrier_base {
            return false;
        }
        let target_lane = Self::group_lane(target_group);
        let barrier_lane = Self::group_lane(barrier_group);
        // Only background lane draws may cross content lane barriers.
        // Letting content lane cross background barriers can reorder text under
        // newly-created background drawcalls when background batching splits.
        if target_lane == 0 && barrier_lane == 1 {
            return true;
        }
        // Explicit non-default draw_call_group layers (seeded via new_draw_call)
        // may cross other background-lane groups to find their own anchor call.
        // This preserves explicit layer lock-in without re-enabling broad
        // background/content reordering.
        target_lane == 0
            && barrier_lane == 0
            && target_draw_call_group != 0
            && target_draw_call_group != barrier_draw_call_group
    }

    pub fn find_appendable_drawcall(
        &mut self,
        sh: &CxDrawShader,
        draw_vars: &DrawVars,
    ) -> Option<usize> {
        // find our drawcall to append to the current layer
        if draw_vars.draw_shader_id.is_none() {
            return None;
        }
        let draw_shader_check = draw_vars
            .draw_shader_id
            .as_ref()
            .unwrap()
            .false_compare_check();
        let target_group = draw_vars.append_group_id;
        let target_draw_call_group = draw_vars.options.draw_call_group.0;

        // Walk backward in draw order and stop at hard barriers.
        // Only sibling parent lanes (background <-> content) may be crossed.
        for i in (0..self.draw_items.len()).rev() {
            let draw_item = &mut self.draw_items[i];
            let Some(draw_call) = &draw_item.draw_call() else {
                break;
            };
            let can_cross = Self::can_cross_group_barrier(
                target_group,
                target_draw_call_group,
                draw_call.append_group_id,
                draw_call.options.draw_call_group.0,
            );

            if self.find_appendable_draw_shader_check[i] == draw_shader_check {
                // TODO! figure out why this can happen
                if draw_call.draw_shader_id != draw_vars.draw_shader_id.unwrap() {
                    Self::append_trace_log(format!(
                        "append_miss shader_mismatch call_shader={} vars_shader={}",
                        draw_call.draw_shader_id.index,
                        draw_vars.draw_shader_id.unwrap().index
                    ));
                } else if draw_call.append_group_id == target_group {
                    // lets compare uniforms and textures..
                    if !sh.mapping.flags.draw_call_nocompare {
                        if draw_call.geometry_id != draw_vars.geometry_id {
                            Self::append_trace_log(format!(
                                "append_miss geom_mismatch shader={} at_draw_item={}",
                                draw_call.draw_shader_id.index, i
                            ));
                            if can_cross {
                                continue;
                            }
                            break;
                        }
                        let mut diff = false;
                        for i in 0..sh.mapping.dyn_uniforms.total_slots {
                            if draw_call.dyn_uniforms[i] != draw_vars.dyn_uniforms[i] {
                                diff = true;
                                break;
                            }
                        }
                        if diff {
                            Self::append_trace_log(format!(
                                "append_barrier uniform_diff shader={} at_draw_item={}",
                                draw_call.draw_shader_id.index, i
                            ));
                            if can_cross {
                                continue;
                            }
                            break;
                        }

                        for i in 0..sh.mapping.textures.len() {
                            fn neq(a: &Option<Texture>, b: &Option<Texture>) -> bool {
                                if let Some(a) = a {
                                    if let Some(b) = b {
                                        return a.texture_id() != b.texture_id();
                                    }
                                    return true;
                                }
                                return false;
                            }
                            if neq(&draw_call.texture_slots[i], &draw_vars.texture_slots[i]) {
                                diff = true;
                                break;
                            }
                        }
                        if diff {
                            Self::append_trace_log(format!(
                                "append_barrier texture_diff shader={} at_draw_item={}",
                                draw_call.draw_shader_id.index, i
                            ));
                            if can_cross {
                                continue;
                            }
                            break;
                        }

                        for i in 0..sh.mapping.uniform_buffers.len() {
                            fn neq(a: &Option<UniformBuffer>, b: &Option<UniformBuffer>) -> bool {
                                if let Some(a) = a {
                                    if let Some(b) = b {
                                        return a.uniform_buffer_id() != b.uniform_buffer_id();
                                    }
                                    return true;
                                }
                                b.is_some()
                            }
                            if neq(
                                &draw_call.uniform_buffer_slots[i],
                                &draw_vars.uniform_buffer_slots[i],
                            ) {
                                diff = true;
                                break;
                            }
                        }
                        if diff {
                            Self::append_trace_log(format!(
                                "append_barrier uniform_buffer_diff shader={} at_draw_item={}",
                                draw_call.draw_shader_id.index, i
                            ));
                            if can_cross {
                                continue;
                            }
                            break;
                        }
                    }
                    if !draw_call.options._appendable_drawcall(&draw_vars.options) {
                        Self::append_trace_log(format!(
                            "append_barrier options_diff shader={} at_draw_item={}",
                            draw_call.draw_shader_id.index, i
                        ));
                        if can_cross {
                            continue;
                        }
                        break;
                    }
                    Self::append_trace_log(format!(
                        "append_hit shader={} draw_item={} group={} draw_call_group={}",
                        draw_call.draw_shader_id.index,
                        i,
                        draw_call.append_group_id,
                        draw_call.options.draw_call_group.0
                    ));
                    return Some(i);
                }
            }

            if !can_cross {
                Self::append_trace_log(format!(
                    "append_barrier group target={} target_draw_call_group={} barrier={} barrier_draw_call_group={} at_draw_item={}",
                    target_group,
                    target_draw_call_group,
                    draw_call.append_group_id,
                    draw_call.options.draw_call_group.0,
                    i
                ));
                break;
            }
        }
        None
    }

    pub fn append_draw_call(
        &mut self,
        redraw_id: u64,
        sh: &CxDrawShader,
        draw_vars: &DrawVars,
    ) -> &mut CxDrawItem {
        Self::append_trace_log(format!(
            "append_new shader={} group={} draw_call_group={} items_before={}",
            draw_vars
                .draw_shader_id
                .map(|v| v.index)
                .unwrap_or(usize::MAX),
            draw_vars.append_group_id,
            draw_vars.options.draw_call_group.0,
            self.draw_items.len()
        ));
        if let Some(ds) = &draw_vars.draw_shader_id {
            self.find_appendable_draw_shader_check
                .push(ds.false_compare_check());
        } else {
            self.find_appendable_draw_shader_check.push(0);
        }
        self.draw_items.push_item(
            redraw_id,
            CxDrawKind::DrawCall(CxDrawCall::new(&sh.mapping, draw_vars)),
        )
    }

    pub fn draw_item_order_len(&self) -> usize {
        self.draw_item_reorder
            .as_ref()
            .map(|reorder| reorder.len())
            .unwrap_or_else(|| self.draw_items.len())
    }

    pub fn draw_item_id_at_order_index(&self, order_index: usize) -> Option<usize> {
        let draw_item_id = if let Some(reorder) = self.draw_item_reorder.as_ref() {
            *reorder.get(order_index)?
        } else if order_index < self.draw_items.len() {
            order_index
        } else {
            return None;
        };
        (draw_item_id < self.draw_items.len()).then_some(draw_item_id)
    }

    pub fn clear_draw_items(&mut self, redraw_id: u64) {
        self.redraw_id = redraw_id;
        self.draw_items.clear();
        self.rect_areas.clear();
        self.find_appendable_draw_shader_check.clear();
    }

    pub fn append_sub_list(&mut self, redraw_id: u64, sub_list_id: DrawListId) {
        // see if we need to add a new one
        self.draw_items
            .push_item(redraw_id, CxDrawKind::SubList(sub_list_id));
        self.find_appendable_draw_shader_check.push(0);
    }

    pub fn store_sub_list_last(&mut self, redraw_id: u64, sub_list_id: DrawListId) {
        // use an empty slot if we have them to insert our subview
        let len = self.draw_items.len();
        for i in 0..len {
            let item = &mut self.draw_items[i];
            if let Some(id) = item.kind.sub_list() {
                if id == sub_list_id {
                    item.kind = CxDrawKind::Empty;
                    break;
                }
            }
        }
        if len > 0 {
            let item = &mut self.draw_items[len - 1];
            if item.kind.is_empty() {
                item.redraw_id = redraw_id;
                item.kind = CxDrawKind::SubList(sub_list_id);
                return;
            }
            if let CxDrawKind::SubList(id) = item.kind {
                if id == sub_list_id {
                    item.redraw_id = redraw_id;
                    return;
                }
            }
        }
        self.append_sub_list(redraw_id, sub_list_id);
    }

    pub fn store_sub_list(&mut self, redraw_id: u64, sub_list_id: DrawListId) {
        // use an empty slot if we have them to insert our subview
        for i in 0..self.draw_items.len() {
            let item = &mut self.draw_items[i];
            if let Some(id) = item.kind.sub_list() {
                if id == sub_list_id {
                    return;
                }
            }
        }
        for i in 0..self.draw_items.len() {
            let item = &mut self.draw_items[i];
            if item.kind.is_empty() {
                item.redraw_id = redraw_id;
                item.kind = CxDrawKind::SubList(sub_list_id);
                return;
            }
        }
        self.append_sub_list(redraw_id, sub_list_id);
    }

    pub fn clear_sub_list(&mut self, sub_list_id: DrawListId) {
        // set our subview to empty
        for i in 0..self.draw_items.len() {
            let item = &mut self.draw_items[i];
            if let Some(check_id) = item.kind.sub_list() {
                if check_id == sub_list_id {
                    item.kind = CxDrawKind::Empty;
                }
            }
        }
    }
    /*
    pub fn get_local_scroll(&self) -> Vec2f {
        let xs = if self.no_v_scroll {0.} else {self.snapped_scroll.x};
        let ys = if self.no_h_scroll {0.} else {self.snapped_scroll.y};
        Vec2f {x: xs, y: ys}
    }*/
    /*
    pub fn uniform_view_transform(&mut self, v: &Mat4f) {
        //dump in uniforms
        self.draw_list_uniforms.view_transform = *v;
    }

    pub fn get_view_transform(&self) -> Mat4f {
        self.draw_list_uniforms.view_transform
    }*/
}
