use crate::{
    cx::Cx,
    draw_pass::DrawPassId,
    draw_shader::{CxDrawShader, CxDrawShaderMapping, CxDrawShaderOptions, DrawShaderId},
    draw_vars::{
        DrawVars, DRAW_CALL_DYN_UNIFORMS, DRAW_CALL_TEXTURE_SLOTS, DRAW_CALL_UNIFORM_BUFFER_SLOTS,
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

fn texture_slots_neq(a: &Option<Texture>, b: &Option<Texture>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.texture_id() != b.texture_id(),
        (None, None) => false,
        _ => true,
    }
}

#[derive(Debug)]
pub struct DrawList(PoolId);

// Only uninitialized allocation storage crosses threads. T is constructed
// after handoff; a dropped spare never runs T's destructor on a worker.
struct PreparedDrawBox<T>(Box<std::mem::MaybeUninit<T>>);
unsafe impl<T> Send for PreparedDrawBox<T> {}
impl<T> PreparedDrawBox<T> {
    fn new() -> Self {
        let mut value = Box::<T>::new_uninit();
        // Touch every page on the allocating worker without constructing T.
        unsafe { value.as_mut_ptr().cast::<u8>().write_bytes(0, std::mem::size_of::<T>()); }
        std::hint::black_box(&value);
        Self(value)
    }
    fn initialize(mut self, value: T) -> Box<T> {
        self.0.write(value);
        // Exactly one fully initialized T was written above; ownership now
        // belongs to the UI's ordinary typed Box.
        unsafe { self.0.assume_init() }
    }
}

/// Worker-prepared allocation for a retained child and its parent link.
/// Live drawing resources never enter this package. Return unused storage
/// to a worker when the caller no longer needs it.
pub struct DrawListRecordingStorage {
    list: Option<PreparedDrawBox<CxDrawList>>,
    items: Vec<(PreparedDrawBox<CxDrawItem>, Vec<f32>)>,
    pointers: Vec<Box<CxDrawItem>>,
    shader_checks: Vec<u64>,
    child_ids: Vec<DrawListId>,
    count: usize,
}
// `pointers` is always empty outside new_prepared's exclusive borrow. Its
// capacity may be exchanged with an emptied UI inventory, never live items.
// All other fields contain scalars, float storage or uninitialized boxes.
unsafe impl Send for DrawListRecordingStorage {}
impl DrawListRecordingStorage {
    pub fn new(draw_items: usize) -> Self {
        Self::with_instance_capacity(draw_items, 256)
    }
    pub fn with_instance_capacity(draw_items: usize, floats: usize) -> Self {
        let mut items = Vec::with_capacity(draw_items + 1);
        // The extra parent link has no instance stream and is consumed last.
        items.push((PreparedDrawBox::new(), Vec::new()));
        for _ in 0..draw_items {
            let mut instances = vec![1.0; floats];
            std::hint::black_box(instances.as_slice());
            instances.clear();
            items.push((PreparedDrawBox::new(), instances));
        }
        let mut shader_checks = vec![u64::MAX; draw_items];
        std::hint::black_box(shader_checks.as_slice());
        shader_checks.clear();
        let mut child_ids = vec![DrawListId(usize::MAX, u64::MAX); draw_items];
        std::hint::black_box(child_ids.as_slice());
        child_ids.clear();
        Self { list: Some(PreparedDrawBox::new()), items,
            pointers: Vec::with_capacity(draw_items), shader_checks, child_ids, count: draw_items }
    }

    fn install_pointer_capacity(&mut self, items: &mut CxDrawItems) {
        if items.buffer.capacity() < self.pointers.capacity() {
            self.pointers.extend(items.buffer.drain(..));
            std::mem::swap(&mut items.buffer, &mut self.pointers);
        }
    }

    fn install_inventory_capacity(&mut self, list: &mut CxDrawList) {
        self.install_pointer_capacity(&mut list.draw_items);
        if list.find_appendable_draw_shader_check.capacity() < self.shader_checks.capacity() {
            self.shader_checks.extend(list.find_appendable_draw_shader_check.drain(..));
            std::mem::swap(&mut self.shader_checks, &mut list.find_appendable_draw_shader_check);
        }
        if list.draw_items.child_inventory.capacity() < self.child_ids.capacity() {
            self.child_ids.extend(list.draw_items.child_inventory.drain(..));
            std::mem::swap(&mut self.child_ids, &mut list.draw_items.child_inventory);
        }
    }

    /// Supplies only the next recording slot. A parent's inventory can be
    /// prepared on a worker without initializing all its items on one frame.
    pub fn prepare_next_item(&mut self, list: &mut CxDrawList) -> bool {
        self.install_inventory_capacity(list);
        let count = list.draw_items.used + 1;
        if list.draw_items.buffer.capacity() < count
            || self.items.len() < count.saturating_sub(list.draw_items.buffer.len()) {
            return false;
        }
        list.draw_items.install_prepared_items(count, self);
        true
    }
}

impl DrawList {
    /// The caller reserves the parent's known pointer inventory once. No
    /// allocation waits occur here; an unavailable package leaves it untouched.
    pub fn new_prepared(cx: &mut Cx, parent: DrawListId, storage: &mut DrawListRecordingStorage) -> Option<Self> {
        let parent_items = &cx.draw_lists[parent].draw_items;
        let parent_count = parent_items.used + 1;
        let extra_parent = parent_count.saturating_sub(parent_items.buffer.len());
        if parent_items.buffer.capacity() < parent_count
            || storage.items.len() < storage.count + extra_parent
            || (cx.draw_lists.0.free_count() == 0 && storage.list.is_none()) {
            return None;
        }
        let draw_list = Self::new_detached_prepared(cx, storage)?;
        cx.draw_lists[parent].draw_items.install_prepared_items(parent_count, storage);
        Some(draw_list)
    }

    /// Creates a retained list before its future painter parent is known.
    pub fn new_detached_prepared(cx: &mut Cx, storage: &mut DrawListRecordingStorage) -> Option<Self> {
        if storage.items.len() < storage.count
            || (cx.draw_lists.0.free_count() == 0 && storage.list.is_none()) {
            return None;
        }
        let draw_list = if cx.draw_lists.0.free_count() != 0 {
            cx.draw_lists.alloc()
        } else {
            let list = storage.list.take().unwrap().initialize(CxDrawList::default());
            let list = DrawList(cx.draw_lists.0.alloc_new(Some(list)));
            cx.draw_lists.reset_allocated(list.id());
            list
        };
        let list = &mut cx.draw_lists[draw_list.id()];
        storage.install_inventory_capacity(list);
        list.draw_items.install_prepared_items(storage.count, storage);
        let recording_gen = cx.next_uniform_gen();
        let uniforms_gen = cx.next_uniform_gen();
        let list = &mut cx.draw_lists[draw_list.id()];
        list.recording_gen = recording_gen;
        list.uniforms_gen = uniforms_gen;
        Some(draw_list)
    }
    pub fn new(cx: &mut Cx) -> Self {
        let recording_gen = cx.next_uniform_gen();
        let uniforms_gen = cx.next_uniform_gen();
        let draw_list = cx.draw_lists.alloc();
        let cx_draw_list = &mut cx.draw_lists[draw_list.id()];
        cx_draw_list.recording_gen = recording_gen;
        cx_draw_list.uniforms_gen = uniforms_gen;
        draw_list
    }

    pub fn set_reset_zbias(&self, cx: &mut Cx, reset_zbias: bool) {
        cx.draw_lists[self.id()].reset_zbias = reset_zbias;
    }

    pub fn reset_zbias(&self, cx: &Cx) -> bool {
        cx.draw_lists[self.id()].reset_zbias
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
    /// A single inventory traversal feeds three bounded priority buckets.
    /// Only one frame's byte allowance and at most256 requests leave here.
    pub fn pending_instance_uploads(&self, root: DrawListId) -> Vec<InstanceUploadRequest> {
        let mut scan = self.draw_lists[root].upload_collection.borrow_mut();
        scan.best.resize(self.draw_lists.0.slot_count(), 3);
        scan.best.fill(3);
        scan.requests.clear();
        scan.requests.reserve(256);
        for candidates in &mut scan.candidates { candidates.clear(); candidates.reserve(256); }
        scan.allowance = [self.draw_lists.1.limit; 3];
        scan.deferred_bytes = 0;
        scan.examined = 0;
        self.collect_instance_uploads(root, crate::retained_instances::UploadCategory::Other, 2, &mut scan);
        let mut available = self.draw_lists.1.limit;
        for priority in 0..3 {
            for index in 0..scan.candidates[priority].len() {
                let (mut request, bytes, stride) = scan.candidates[priority][index];
                // An alias later reached this list through a more urgent
                // ancestor. Its new bucket owns admission, never this old one.
                if scan.best[request.list.index()] != request.priority { continue; }
                if scan.requests.len() == 256 || (bytes != 0 && available < stride) {
                    scan.deferred_bytes = scan.deferred_bytes.saturating_add(bytes.max(1));
                    continue;
                }
                available = available.saturating_sub(bytes.min(available) / stride * stride);
                if request.category == crate::retained_instances::UploadCategory::Other {
                    use crate::retained_instances::UploadCategory as Category;
                    let call = self.draw_lists[request.list].draw_items[request.item].draw_call().unwrap();
                    let inputs = &self.draw_shaders[call.draw_shader_id.index].mapping.instances.inputs;
                    let has = |name| inputs.iter().any(|input| input.id == name);
                    request.category = if has(crate::id!(roof_height)) && has(crate::id!(start)) { Category::Outlines }
                        else if has(crate::id!(height)) && has(crate::id!(tint)) { Category::Walls }
                        else if has(crate::id!(stripe_side)) { Category::Roofs }
                        else if has(crate::id!(glyph_depth)) { Category::Labels }
                        else { Category::Other };
                }
                scan.requests.push(request);
            }
        }
        std::mem::take(&mut scan.requests)
    }

    pub fn instance_upload_collection_deferred(&self, root: DrawListId) -> usize {
        self.draw_lists[root].upload_collection.borrow().deferred_bytes
    }

    pub fn instance_upload_collection_examined(&self, root: DrawListId) -> usize {
        self.draw_lists[root].upload_collection.borrow().examined
    }

    pub fn recycle_instance_uploads(&self, root: DrawListId, mut requests: Vec<InstanceUploadRequest>) {
        requests.clear();
        self.draw_lists[root].upload_collection.borrow_mut().requests = requests;
    }

    fn collect_instance_uploads(
        &self, id: DrawListId, inherited: crate::retained_instances::UploadCategory,
        priority: u8, scan: &mut InstanceUploadCollection,
    ) {
        use crate::retained_instances::UploadCategory as Category;
        if self.draw_lists.is_id_freed(id) { return; }
        let list = &self.draw_lists[id];
        let priority = priority.min(list.upload_priority.min(2));
        if scan.best[id.index()] <= priority { return; }
        scan.best[id.index()] = priority;
        if list.draw_items.clean_leaf.get() { return; }
        let category = match list.debug_id {
            id if id == crate::id!(atlas_labels) || id == crate::id!(atlas_label) => Category::Labels,
            id if id == crate::id!(atlas_scene) => Category::Roofs,
            id if id == crate::id!(atlas_walls) => Category::Walls,
            id if id == crate::id!(atlas_overlay) => Category::Outlines,
            id if id == crate::id!(atlas_background) => Category::Background,
            id if id == crate::id!(atlas_structure_batch) => Category::Structure,
            id if id == crate::id!(atlas_code_file) => Category::Code,
            _ => inherited,
        };
        if list.draw_items.child_inventory_valid {
            // Pure parent inventories are contiguous IDs. Avoid a cache miss
            // through each large boxed draw item just to retrieve its child.
            scan.examined += list.draw_items.len();
            for &child in &list.draw_items.child_inventory {
                self.collect_instance_uploads(child, category, priority, scan);
            }
            return;
        }
        let mut clean_leaf = true;
        for item_id in 0..list.draw_items.len() {
            scan.examined += 1;
            let item = &list.draw_items[item_id];
            if let Some(child) = item.sub_list() {
                clean_leaf = false;
                self.collect_instance_uploads(child, category, priority, scan);
                continue;
            }
            let Some(call) = item.draw_call() else { continue };
            if (!call.instance_dirty && !item.instance_upload_pending) || call.total_instance_slots == 0 { continue; }
            clean_leaf = false;
            let bytes = item.retained_instances.as_ref().map_or_else(
                || item.instances.as_ref().map_or(0, |v| v.len() * 4), |p| p.byte_len());
            let stride = call.total_instance_slots * 4;
            let p = priority as usize;
            if scan.candidates[p].len() == 256 || (bytes != 0 && scan.allowance[p] < stride) {
                scan.deferred_bytes = scan.deferred_bytes.saturating_add(bytes.max(1));
                continue;
            }
            scan.allowance[p] = scan.allowance[p].saturating_sub(bytes.min(scan.allowance[p]) / stride * stride);
            scan.candidates[p].push((InstanceUploadRequest { list: id, item: item_id, category, priority }, bytes, stride));
        }
        list.draw_items.clean_leaf.set(clean_leaf);
    }

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
                data,
                updated,
                ..
            } => {
                if let TextureUpdated::Empty = updated {
                    0
                } else {
                    // The data buffer holds the full mip chain (~4/3 of level 0), so use its
                    // actual length when present rather than just level-0 (width*height*4).
                    data.as_ref()
                        .map(|d| (d.len() as u64).saturating_mul(4))
                        .unwrap_or_else(|| (*width as u64).saturating_mul(*height as u64).saturating_mul(4))
                }
            }
            TextureFormat::VecMipRGBAf32 {
                width,
                height,
                data,
                updated,
                ..
            } => {
                if let TextureUpdated::Empty = updated {
                    0
                } else {
                    data.as_ref()
                        .map(|d| (d.len() as u64).saturating_mul(16))
                        .unwrap_or_else(|| (*width as u64).saturating_mul(*height as u64).saturating_mul(16))
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
                if self.draw_lists.is_id_freed(sub_list_id) {
                    continue;
                }
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
            let instance_count = draw_item.retained_instances.as_ref().map_or_else(
                || draw_item.instances.as_ref().map_or(0, |instances| instances.len() / instance_slots),
                |_| draw_item.retained_instance_count,
            );
            if instance_count == 0 {
                continue;
            }

            let Some(geometry_id) = draw_call.geometry_id else {
                continue;
            };
            let geometry = &self.geometries[geometry_id];
            let index_count = geometry.index_count as u64;

            metrics.draw_calls = metrics.draw_calls.saturating_add(1);
            metrics.instances = metrics.instances.saturating_add(instance_count as u64);
            metrics.vertices = metrics
                .vertices
                .saturating_add(index_count.saturating_mul(instance_count as u64));

            if draw_call.instance_dirty {
                metrics.instance_bytes = metrics.instance_bytes.saturating_add(
                    (draw_item.retained_instances.as_ref().map_or_else(
                        || draw_item.instances.as_ref().map_or(0usize, Vec::len),
                        |_| draw_item.retained_upload_range.len(),
                    ) * 4) as u64,
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
                        .saturating_add(geometry.vertices.byte_len() as u64);
                }
                if geometry.dirty_indices {
                    metrics.vertex_buffer_bytes = metrics
                        .vertex_buffer_bytes
                        .saturating_add((geometry.index_count * geometry.index_width) as u64);
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

#[derive(Default)]
struct InstanceUploadCollection {
    best: Vec<u8>,
    candidates: [Vec<(InstanceUploadRequest, usize, usize)>; 3],
    allowance: [usize; 3],
    requests: Vec<InstanceUploadRequest>,
    deferred_bytes: usize,
    examined: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct InstanceUploadRequest {
    pub list: DrawListId,
    pub item: usize,
    pub category: crate::retained_instances::UploadCategory,
    pub priority: u8,
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
pub struct CxDrawListPool(
    // Keep uniforms, backend state and inventory scratch at stable addresses.
    // Growing the pool then moves only handles, not every live draw list.
    pub(crate) IdPool<Box<CxDrawList>>,
    pub crate::retained_instances::RetainedUploadBudget,
);
impl CxDrawListPool {
    pub fn alloc(&mut self) -> DrawList {
        let draw_list = DrawList(self.0.alloc());
        self.reset_allocated(draw_list.id());
        draw_list
    }

    fn reset_allocated(&mut self, id: DrawListId) {
        // A recycled slot keeps the GPU resources of its draw items, never
        // the previous owner's list uniforms: a stale view_clip from a
        // dropped list would clip the new owner's instances.
        self[id].draw_list_uniforms = Default::default();
        self[id].zbias_hold = None;
        self[id].reset_zbias = false;
        self[id].upload_priority = 2;
        self[id].reset_draw_item_uniform_caches();
    }

    /// Every live draw list id (the tweaker's colour pulse walks all
    /// draw calls in place).
    pub fn id_iter(&self) -> impl Iterator<Item = DrawListId> + '_ {
        self.0.pool.iter().enumerate().filter_map(|(i, d)| {
            let id = DrawListId(i, d.generation);
            if self.is_id_freed(id) {
                None
            } else {
                Some(id)
            }
        })
    }

    pub fn checked_index(&self, index: DrawListId) -> Option<&CxDrawList> {
        let d = &self.0.pool[index.0];
        if d.generation != index.1 {
            return None;
        }
        return Some(&d.item);
    }

    /// True if this draw list's slot has been handed back to the free pool (its owning widget
    /// was dropped) but not yet reused. A freed slot keeps its generation until realloc, so
    /// `checked_index` still returns it — this distinguishes "dropped, awaiting reuse" so callers
    /// (e.g. the overlay flush) can drop a stale sub-list whose widget no longer exists.
    pub fn is_id_freed(&self, index: DrawListId) -> bool {
        let d = &self.0.pool[index.0];
        if d.generation != index.1 {
            return true;
        }
        self.0.is_free(index.0)
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
    // The array length is in f32 elements, not bytes, hence the >> 2.
    pub fn as_slice(&self) -> &[f32; std::mem::size_of::<DrawCallUniforms>() >> 2] {
        unsafe { std::mem::transmute(self) }
    }
    /*
    pub fn get_local_scroll(&self) -> Vec4f {
        self.draw_scroll
    }*/

    /// Sets the zbias, returning `true` if it actually changed.
    ///
    /// The zbias is recomputed from the global draw order on every frame, so it shifts whenever
    /// a sibling draw list grows or shrinks -- including for a cached draw call that was not
    /// redrawn and therefore has `uniforms_dirty` unset. Backends that only upload
    /// `DrawCallUniforms` when `uniforms_dirty` is set must also upload when this returns `true`,
    /// or the GPU keeps a stale depth for that draw call and the depth test rejects it.
    pub fn set_zbias(&mut self, zbias: f32) -> bool {
        let changed = self.zbias != zbias;
        self.zbias = zbias;
        changed
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

#[repr(C)]
pub enum CxDrawKind {
    SubList(DrawListId),
    DrawCall(CxDrawCall),
    Empty,
}

// Collection reads only this header for clean calls/child links. Keep it
// together instead of faulting pages containing the large uniform payload.
#[repr(C)]
pub struct CxDrawItem {
    /// A partial backend copy owes another frame even after recording stops.
    pub instance_upload_pending: bool,
    pub redraw_id: u64,
    pub kind: CxDrawKind,
    // these values stick around to reduce buffer churn
    pub draw_item_id: usize,
    pub instances: Option<Vec<f32>>,
    /// Immutable worker payload; old callers continue using `instances`.
    pub retained_instances: Option<crate::retained_instances::RetainedInstances>,
    pub retained_instance_id: u64,
    /// Recording may vary draw count without changing the immutable publication.
    pub retained_instance_count: usize,
    pub retained_upload_range: std::ops::Range<usize>,
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

#[repr(C)]
pub struct CxDrawCall {
    pub instance_dirty: bool,
    pub uniforms_dirty: bool,
    pub total_instance_slots: usize,
    pub draw_shader_id: DrawShaderId, // if shader_id changed, delete gl vao
    pub options: CxDrawShaderOptions,
    pub append_group_id: u64,
    pub draw_call_uniforms: DrawCallUniforms, // draw uniforms
    pub geometry_id: Option<GeometryId>,
    pub dyn_uniforms: [f32; DRAW_CALL_DYN_UNIFORMS], // user uniforms
    pub texture_slots: [Option<Texture>; DRAW_CALL_TEXTURE_SLOTS],
    pub uniform_buffer_slots: [Option<UniformBuffer>; DRAW_CALL_UNIFORM_BUFFER_SLOTS],
    /// Replaced with a process-wide generation whenever either uniform block
    /// owned by this draw call changes.
    pub uniforms_gen: u64,
    /// Component nesting depth (`Cx::nesting_depth`) at the moment this call
    /// was created. The exploded z-layer view hands this to the shader in
    /// place of the paint-order zbias, so one plane = one nesting level.
    /// Stamped always (one f32 write per call creation); read only while the
    /// mode is up.
    pub turtle_depth: f32,
}

impl CxDrawCall {
    pub fn new(
        mapping: &CxDrawShaderMapping,
        draw_vars: &DrawVars,
        turtle_depth: f32,
        uniforms_gen: u64,
    ) -> Self {
        debug_assert_ne!(uniforms_gen, 0);
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
            uniforms_gen,
            turtle_depth,
        }
    }

    #[inline]
    pub fn mark_uniforms_dirty(&mut self, uniforms_gen: u64) {
        debug_assert_ne!(uniforms_gen, 0);
        self.uniforms_dirty = true;
        self.uniforms_gen = uniforms_gen;
    }

    /// The z the shader sees in `world.z`. Paint order normally; the emitting
    /// component's nesting depth while the pass is exploded — deeper nesting
    /// is a larger z, and the ortho maps larger z nearer the viewer, so
    /// children lift toward you and parents stay at the bottom of the stack.
    pub fn resolve_zbias(
        &mut self,
        paint_order: f32,
        sploded: bool,
        uniforms_gen: u64,
    ) -> bool {
        let z = if sploded {
            // One level is worth far more than any widget's own `draw_depth`,
            // so those stay an in-plane tie-break instead of whole planes of
            // separation. See `sploded::SPLODED_DEPTH_UNIT`.
            self.turtle_depth * crate::sploded::SPLODED_DEPTH_UNIT
        } else {
            paint_order
        };
        let changed = self.draw_call_uniforms.set_zbias(z);
        if changed {
            debug_assert_ne!(uniforms_gen, 0);
            self.uniforms_gen = uniforms_gen;
        }
        changed
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
    // The array length is in f32 elements, not bytes, hence the >> 2.
    pub fn as_slice(&self) -> &[f32; std::mem::size_of::<DrawListUniforms>() >> 2] {
        unsafe { std::mem::transmute(self) }
    }
}

#[derive(Default)]
pub struct CxDrawItems {
    // A leaf proven clean by the collector needs no draw-call metadata walk.
    // Every mutable public item access invalidates this proof, including
    // patches that bypass ordinary recording. Parent lists always recurse.
    clean_leaf: std::cell::Cell<bool>,
    // Stable records: growing a parent's inventory moves only pointers, never
    // the large uniform/texture/backend payload of every preceding child.
    pub(crate) buffer: Vec<Box<CxDrawItem>>,
    used: usize,
    instance_capacity_hint: usize,
    first_instance_spare: Vec<f32>,
    child_inventory: Vec<DrawListId>,
    child_inventory_valid: bool,
}

impl std::ops::Index<usize> for CxDrawItems {
    type Output = CxDrawItem;
    fn index(&self, index: usize) -> &Self::Output {
        &self.buffer[index]
    }
}

impl std::ops::IndexMut<usize> for CxDrawItems {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        self.clean_leaf.set(false);
        self.child_inventory_valid = false;
        &mut self.buffer[index]
    }
}

impl CxDrawItems {
    fn install_prepared_items(&mut self, count: usize, storage: &mut DrawListRecordingStorage) {
        assert!(self.buffer.capacity() >= count);
        while self.buffer.len() < count {
            let (allocation, instances) = storage.items.pop().unwrap();
            self.buffer.push(allocation.initialize(CxDrawItem {
                instance_upload_pending: false, redraw_id: 0, kind: CxDrawKind::Empty,
                draw_item_id: self.buffer.len(), instances: Some(instances),
                retained_instances: None, retained_instance_id: 0,
                retained_instance_count: 0, retained_upload_range: 0..0,
                os: CxOsDrawCall::default(),
            }));
        }
    }
    /// Storage may be prepared by a worker before this list records its first
    /// draw. These helpers are for single-stream retained lists; the caller
    /// re-records immediately after swapping and owns retirement of the spare.
    pub fn first_instance_buffer_capacity(&self) -> usize {
        self.buffer.first().and_then(|item| item.instances.as_ref())
            .map_or(self.first_instance_spare.capacity(), Vec::capacity)
    }
    pub fn swap_first_instance_buffer(&mut self, spare: &mut Vec<f32>) {
        if let Some(item) = self.buffer.first_mut() {
            std::mem::swap(item.instances.as_mut().unwrap(), spare);
        } else {
            std::mem::swap(&mut self.first_instance_spare, spare);
        }
        self.clean_leaf.set(false);
    }
    pub fn len(&self) -> usize {
        self.used
    }
    pub fn clear(&mut self) {
        self.clean_leaf.set(false);
        self.child_inventory.clear();
        self.child_inventory_valid = true;
        self.used = 0
    }
    /// Binding changes backend residency/uniform stamps, never instance
    /// dirtiness or child topology. Upload/recording mutations use IndexMut.
    #[cfg(all(not(headless), any(target_os = "macos", target_os = "ios", target_os = "tvos")))]
    pub(crate) fn binding_mut(&mut self, index: usize) -> &mut CxDrawItem {
        &mut self.buffer[index]
    }
    pub fn push_item(&mut self, redraw_id: u64, kind: CxDrawKind) -> &mut CxDrawItem {
        self.clean_leaf.set(false);
        // The returned mutable item may have its kind replaced by the caller.
        self.child_inventory_valid = false;
        let draw_item_id = self.used;
        if self.used >= self.buffer.len() {
            let capacity = kind.draw_call().map_or(0, |call|
                call.total_instance_slots.saturating_mul(self.instance_capacity_hint));
            let mut instances = if draw_item_id == 0 {
                std::mem::take(&mut self.first_instance_spare)
            } else { Vec::new() };
            instances.reserve(capacity);
            self.buffer.push(Box::new(CxDrawItem {
                draw_item_id,
                redraw_id,
                instances: Some(instances),
                retained_instances: None,
                retained_instance_id: 0,
                retained_instance_count: 0,
                retained_upload_range: 0..0,
                instance_upload_pending: false,
                os: CxOsDrawCall::default(),
                kind: kind,
            }));
        } else {
            // reuse an older one, keeping all GPU resources attached
            let draw_item = &mut self.buffer[draw_item_id];
            draw_item.instances.as_mut().unwrap().clear();
            if draw_item.retained_instances.is_none() {
                draw_item.retained_instance_id = 0;
            }
            draw_item.retained_instances = None;
            draw_item.kind = kind;
            draw_item.redraw_id = redraw_id;
        }
        self.used += 1;
        &mut self.buffer[draw_item_id]
    }
}

#[derive(Default)]
pub struct CxDrawList {
    upload_collection: std::cell::RefCell<InstanceUploadCollection>,
    pub debug_id: LiveId,
    pub debug_dump: bool,
    pub debug_dump_count: u32,
    /// Copy admission order, independent of painter order: pointer, visible,
    /// then background. Zero is the pointer owner's highest priority.
    pub upload_priority: u8,
    pub reset_zbias: bool,

    /// Depth floor for this draw list and everything drawn under it, in
    /// `world.z` units. `0.0` — every ordinary list — costs one compare and
    /// changes nothing.
    ///
    /// A 2D pass has ONE depth buffer shared by every draw list in it, and a
    /// 2D vertex lands at `world.z = draw_depth + draw_call.zbias`. The
    /// `zbias` half is the paint-order counter the backend walk accumulates
    /// (`zbias_step`, 0.001), so "drawn later" only outranks "drawn earlier"
    /// by a thousandth — nothing at all against a widget that spends whole
    /// units of `draw_depth` to order its own ink. That is why an overlay,
    /// which by construction paints after the body, could still be rejected by
    /// the depth test and disappear behind it.
    ///
    /// A list with a floor raises the running counter to it on entry
    /// (`raise_zbias_to_floor`), so every draw call beneath it — including
    /// nested plain draw lists, which inherit the same counter — starts above
    /// the band the body was using. Set by `DrawList2d::begin_overlay_inner`;
    /// see the constants there for the scheme and its headroom.
    pub overlay_z_lift: f32,

    pub codeflow_parent_id: Option<DrawListId>, // the id of the parent we nest in, codeflow wise

    pub redraw_id: u64,
    /// Replaced on every recording, including repeated recordings in one redraw.
    pub recording_gen: u64,
    pub draw_pass_id: Option<DrawPassId>,

    pub draw_items: CxDrawItems,
    pub draw_item_reorder: Option<Vec<usize>>,
    draw_item_reorder_spare: Vec<usize>,

    /// For a draw list registered as a sub-list of the window Overlay: the
    /// position in which it was BEGUN this frame. Overlay slots are handed out
    /// first-come and kept for the life of the process, so without this the
    /// paint order of every glass surface is the order they were first
    /// created rather than the order they are drawn. `Overlay::end` turns
    /// these into `draw_item_reorder`.
    pub overlay_order: u64,

    pub draw_list_uniforms: DrawListUniforms,
    /// Replaced when this list is recorded or its uniform block is changed.
    pub uniforms_gen: u64,
    pub draw_list_has_clip: bool,

    /// Paint order for a RETAINED sub-list. `None` — every ordinary list —
    /// lets the backend walk hand each draw call its own step of the
    /// running paint-order counter, as always. `Some(steps)` makes the list
    /// one unit of that counter: every draw call in it resolves its zbias
    /// to the counter value at entry, the list's internal layering is
    /// authored by its owner in the instances' `draw_depth` (in units of
    /// `zbias_step`), and the counter advances by `steps` on exit.
    ///
    /// A list recorded once and re-attached on later frames cannot know
    /// how many calls precede it, so its baked depths must be relative to
    /// its entry; the map's tile lists bake the same layering the
    /// immediate path produced (one layer per stream, faces with their
    /// casing) and report the layers used. Every backend's `render_view`
    /// honours this in exactly one place — the sub-list branch, beside
    /// `reset_zbias` — by walking the child with a zero step and a copy of
    /// the counter.
    pub zbias_hold: Option<u32>,

    pub os: CxOsDrawList,
    pub rect_areas: Vec<CxRectArea>,
    pub find_appendable_draw_shader_check: Vec<u64>,
}

pub struct CxRectArea {
    pub rect: Rect,
    pub draw_clip: (Vec2d, Vec2d),
}

impl CxDrawList {
    /// Parent inventories retain one stable pointer per known child.
    pub fn reserve_sub_list_inventory(&mut self, count: usize) {
        self.draw_items.buffer.reserve(count.saturating_sub(self.draw_items.buffer.len()));
        self.draw_items.child_inventory.reserve(count.saturating_sub(self.draw_items.child_inventory.len()));
        self.find_appendable_draw_shader_check.reserve(count.saturating_sub(self.find_appendable_draw_shader_check.len()));
    }

    /// A retained scene knows its inventory before the first quad. Reserve
    /// once at publication so aligned-instance pushes never repeatedly move a
    /// growing multi-megabyte stream during a camera/fit frame.
    pub fn reserve_instance_inventory(&mut self, count: usize) {
        if count <= self.draw_items.instance_capacity_hint { return; }
        self.draw_items.instance_capacity_hint = count;
        for item in &mut self.draw_items.buffer {
            let Some(call) = item.kind.draw_call() else { continue };
            let capacity = count.saturating_mul(call.total_instance_slots);
            if let Some(instances) = &mut item.instances {
                instances.reserve(capacity.saturating_sub(instances.len()));
            }
        }
        self.find_appendable_draw_shader_check.reserve(16);
    }

    #[inline]
    pub fn set_uniform_view_transform(&mut self, transform: &Mat4f, uniforms_gen: u64) {
        debug_assert_ne!(uniforms_gen, 0);
        self.draw_list_uniforms.view_transform = *transform;
        self.uniforms_gen = uniforms_gen;
    }

    #[inline]
    fn reset_draw_item_uniform_caches(&mut self) {
        #[cfg(any(target_arch = "wasm32", test))]
        for item in &mut self.draw_items.buffer {
            item.os.uniforms_recording_gen = None;
            item.os.draw_call_uniforms_gen = None;
            item.os.user_uniforms_gen = None;
        }
    }

    /// Raise a backend walk's running paint-order depth counter to this
    /// sub-list's floor, on the way into it. See [`CxDrawList::overlay_z_lift`].
    ///
    /// It raises and never lowers, so the counter stays monotone across the
    /// whole pass: two overlays sharing a floor still order by draw position,
    /// and a nested overlay's higher floor is not undone by a sibling drawn
    /// after it. Every backend's `render_view` calls this at exactly one
    /// place — the sub-list branch, beside the `reset_zbias` check — and for
    /// an ordinary list (`overlay_z_lift == 0.0`) it is one compare that
    /// changes nothing.
    #[inline]
    pub fn raise_zbias_to_floor(&self, zbias: &mut f32) {
        if *zbias < self.overlay_z_lift {
            *zbias = self.overlay_z_lift;
        }
    }

    fn append_trace_enabled() -> bool {
        false
    }

    fn append_trace_log(message: impl FnOnce() -> String) {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        if !Self::append_trace_enabled() {
            return;
        }
        let n = COUNT.fetch_add(1, Ordering::Relaxed);
        if n < 200 {
            log!("{}", message());
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

    /// `depth_target` is `Some` only while the exploded z-layer view is up:
    /// batches must then stay depth-homogeneous, because the whole call shares
    /// one z. `None` — the ordinary case — leaves batching exactly as it was.
    pub fn find_appendable_drawcall(
        &mut self,
        sh: &CxDrawShader,
        draw_vars: &DrawVars,
        depth_target: Option<f32>,
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

            // Exploded view: a call carries ONE z, so it may only hold
            // instances from one nesting level. A depth mismatch is treated
            // like any other uniform difference.
            if let Some(depth) = depth_target {
                if draw_call.turtle_depth != depth {
                    if can_cross {
                        continue;
                    }
                    break;
                }
            }

            if self.find_appendable_draw_shader_check[i] == draw_shader_check {
                // TODO! figure out why this can happen
                if draw_call.draw_shader_id != draw_vars.draw_shader_id.unwrap() {
                    Self::append_trace_log(|| format!(
                        "append_miss shader_mismatch call_shader={} vars_shader={}",
                        draw_call.draw_shader_id.index,
                        draw_vars.draw_shader_id.unwrap().index
                    ));
                } else if draw_call.append_group_id == target_group {
                    // lets compare uniforms and textures..
                    if !sh.mapping.flags.draw_call_nocompare {
                        if draw_call.geometry_id != draw_vars.geometry_id {
                            Self::append_trace_log(|| format!(
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
                            Self::append_trace_log(|| format!(
                                "append_barrier uniform_diff shader={} at_draw_item={}",
                                draw_call.draw_shader_id.index, i
                            ));
                            if can_cross {
                                continue;
                            }
                            break;
                        }

                        for i in 0..sh.mapping.textures.len() {
                            if texture_slots_neq(
                                &draw_call.texture_slots[i],
                                &draw_vars.texture_slots[i],
                            ) {
                                diff = true;
                                break;
                            }
                        }
                        if diff {
                            Self::append_trace_log(|| format!(
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
                            Self::append_trace_log(|| format!(
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
                        Self::append_trace_log(|| format!(
                            "append_barrier options_diff shader={} at_draw_item={}",
                            draw_call.draw_shader_id.index, i
                        ));
                        if can_cross {
                            continue;
                        }
                        break;
                    }
                    Self::append_trace_log(|| format!(
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
                Self::append_trace_log(|| format!(
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
        turtle_depth: f32,
        uniforms_gen: u64,
    ) -> &mut CxDrawItem {
        Self::append_trace_log(|| format!(
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
            CxDrawKind::DrawCall(CxDrawCall::new(
                &sh.mapping,
                draw_vars,
                turtle_depth,
                uniforms_gen,
            )),
        )
    }

    pub fn draw_item_order_len(&self) -> usize {
        self.draw_item_reorder
            .as_ref()
            .map(|reorder| reorder.len())
            .unwrap_or_else(|| self.draw_items.len())
    }

    /// Keep painter-order capacity across both presentation and re-recording.
    pub fn take_draw_item_reorder(&mut self) -> Vec<usize> {
        self.draw_item_reorder.take()
            .unwrap_or_else(|| std::mem::take(&mut self.draw_item_reorder_spare))
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

    pub fn clear_draw_items(
        &mut self,
        redraw_id: u64,
        recording_gen: u64,
        uniforms_gen: u64,
    ) {
        debug_assert_ne!(recording_gen, 0);
        debug_assert_ne!(uniforms_gen, 0);
        self.redraw_id = redraw_id;
        self.recording_gen = recording_gen;
        self.uniforms_gen = uniforms_gen;
        self.reset_draw_item_uniform_caches();
        self.draw_items.clear();
        if let Some(mut order) = self.draw_item_reorder.take() {
            order.clear();
            self.draw_item_reorder_spare = order;
        }
        self.rect_areas.clear();
        self.find_appendable_draw_shader_check.clear();
    }

    pub fn append_sub_list(&mut self, redraw_id: u64, sub_list_id: DrawListId) {
        // see if we need to add a new one
        let children_only = self.draw_items.child_inventory_valid;
        self.draw_items
            .push_item(redraw_id, CxDrawKind::SubList(sub_list_id));
        if children_only {
            self.draw_items.child_inventory.push(sub_list_id);
            self.draw_items.child_inventory_valid = true;
        }
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

impl Cx {
    /// Every draw list reachable from `pass_id`'s main list through SubList
    /// items — the lists that are actually on screen this frame. Retained
    /// lists a container has stopped referencing (a Dock's hidden pages, a
    /// closed StackNavigation view) keep their items but are not here.
    pub fn attached_draw_lists(&self, pass_id: DrawPassId) -> std::collections::HashSet<DrawListId> {
        self.attached_draw_lists_from(self.passes[pass_id].main_draw_list_id)
    }

    /// Same walk from any set of roots. A list links into its parent only
    /// when it ENDS, so mid-draw (an overlay drawing while its ancestors
    /// are still open) the pass root does not yet reach the open chain —
    /// seed the walk with the open lists too (`Cx2d::open_draw_lists`).
    pub fn attached_draw_lists_from(
        &self,
        roots: impl IntoIterator<Item = DrawListId>,
    ) -> std::collections::HashSet<DrawListId> {
        let mut out = std::collections::HashSet::new();
        let mut stack: Vec<DrawListId> = roots.into_iter().collect();
        while let Some(list_id) = stack.pop() {
            if self.draw_lists.is_id_freed(list_id) || !out.insert(list_id) {
                continue;
            }
            let draw_list = &self.draw_lists[list_id];
            for order_index in 0..draw_list.draw_item_order_len() {
                let Some(item_id) = draw_list.draw_item_id_at_order_index(order_index) else {
                    continue;
                };
                if let CxDrawKind::SubList(sub) = &draw_list.draw_items[item_id].kind {
                    stack.push(*sub);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::texture::CxTexturePool;

    #[test]
    fn texture_slot_batching_comparison_is_symmetric() {
        let mut textures = CxTexturePool::default();
        let none: Option<Texture> = None;
        let texture_t = Some(textures.alloc(TextureFormat::Unknown));
        let texture_u = Some(textures.alloc(TextureFormat::Unknown));
        let merges = |existing: &Option<Texture>, new: &Option<Texture>| {
            !texture_slots_neq(existing, new)
        };

        assert!(merges(&none, &none));
        assert!(merges(&texture_t, &texture_t));
        assert!(!merges(&none, &texture_t));
        assert!(!merges(&texture_t, &none));
        assert!(!merges(&texture_t, &texture_u));
    }
}

#[cfg(test)]
mod retained_sub_list_tests {
    use super::*;

    /// The retained-sub-list contract: a parent list may keep naming a child
    /// list that its owner dropped (a map tile evicted at event time, a hidden
    /// page). The dropped id stays dead — before AND after the pool hands its
    /// slot to a new list, whose id differs by generation — and every tree
    /// walk skips it rather than landing on whatever list now holds the slot.
    #[test]
    fn a_dropped_sub_list_stays_dead_across_slot_reuse() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let parent = DrawList::new(&mut cx);
        let child = DrawList::new(&mut cx);
        let child_id = child.id();
        cx.draw_lists[parent.id()].append_sub_list(1, child_id);
        assert!(!cx.draw_lists.is_id_freed(child_id));
        assert!(cx.attached_draw_lists_from([parent.id()]).contains(&child_id));

        drop(child);
        assert!(cx.draw_lists.is_id_freed(child_id));
        assert!(cx.draw_lists.checked_index(child_id).is_some(), "slot kept until reuse");
        assert!(!cx.attached_draw_lists_from([parent.id()]).contains(&child_id));

        let reused = DrawList::new(&mut cx);
        assert_eq!(reused.id().index(), child_id.index(), "the freed slot is reused");
        assert_ne!(reused.id().generation(), child_id.generation());
        assert!(cx.draw_lists.is_id_freed(child_id));
        assert!(cx.draw_lists.checked_index(child_id).is_none());
        assert!(!cx.draw_lists.is_id_freed(reused.id()));
        let attached = cx.attached_draw_lists_from([parent.id()]);
        assert!(attached.contains(&parent.id()));
        assert!(!attached.contains(&child_id));
        assert!(!attached.contains(&reused.id()), "the stale entry must not reach the new list");
    }
}

#[cfg(test)]
mod uniform_generation_tests {
    use super::*;

    fn test_draw_call(uniforms_gen: u64) -> CxDrawCall {
        CxDrawCall {
            draw_shader_id: DrawShaderId { index: 0 },
            options: CxDrawShaderOptions::default(),
            append_group_id: 0,
            total_instance_slots: 0,
            draw_call_uniforms: DrawCallUniforms::default(),
            geometry_id: None,
            dyn_uniforms: [0.0; DRAW_CALL_DYN_UNIFORMS],
            texture_slots: Default::default(),
            uniform_buffer_slots: Default::default(),
            instance_dirty: true,
            uniforms_dirty: true,
            uniforms_gen,
            turtle_depth: 0.0,
        }
    }

    #[test]
    fn pooled_draw_call_gets_a_globally_unique_generation() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let first_list = DrawList::new(&mut cx);
        let first_id = first_list.id();
        let first_gen = cx.next_uniform_gen();
        let first_item = cx.draw_lists[first_id]
            .draw_items
            .push_item(1, CxDrawKind::DrawCall(test_draw_call(first_gen)));
        first_item.os.uniforms_recording_gen = Some(first_gen);
        first_item.os.draw_call_uniforms_gen = Some(first_gen);
        first_item.os.user_uniforms_gen = Some(first_gen);

        drop(first_list);
        let reused_list = DrawList::new(&mut cx);
        let reused_id = reused_list.id();
        assert_eq!(first_id.index(), reused_id.index());
        assert_ne!(first_id.generation(), reused_id.generation());
        let retained_item = &cx.draw_lists[reused_id].draw_items.buffer[0];
        assert_eq!(retained_item.os.uniforms_recording_gen, None);
        assert_eq!(retained_item.os.draw_call_uniforms_gen, None);
        assert_eq!(retained_item.os.user_uniforms_gen, None);

        let recording_gen = cx.next_uniform_gen();
        let list_uniforms_gen = cx.next_uniform_gen();
        cx.draw_lists[reused_id].clear_draw_items(2, recording_gen, list_uniforms_gen);
        let second_gen = cx.next_uniform_gen();
        cx.draw_lists[reused_id]
            .draw_items
            .push_item(2, CxDrawKind::DrawCall(test_draw_call(second_gen)));

        let issued = [first_gen, recording_gen, list_uniforms_gen, second_gen];
        assert!(issued.iter().all(|gen| *gen != 0));
        let unique: HashSet<_> = issued.into_iter().collect();
        assert_eq!(unique.len(), issued.len());
        assert_eq!(
            cx.draw_lists[reused_id].draw_items[0]
                .kind
                .draw_call()
                .unwrap()
                .uniforms_gen,
            second_gen
        );
    }

    #[test]
    fn clear_draw_items_resets_uniform_upload_generations() {
        let mut draw_list = CxDrawList::default();
        let item = draw_list
            .draw_items
            .push_item(1, CxDrawKind::DrawCall(test_draw_call(1)));
        item.os.uniforms_recording_gen = Some(2);
        item.os.draw_call_uniforms_gen = Some(3);
        item.os.user_uniforms_gen = Some(4);

        draw_list.clear_draw_items(2, 5, 6);

        let item = &draw_list.draw_items.buffer[0];
        assert_eq!(item.os.uniforms_recording_gen, None);
        assert_eq!(item.os.draw_call_uniforms_gen, None);
        assert_eq!(item.os.user_uniforms_gen, None);
        assert_eq!(draw_list.recording_gen, 5);
        assert_eq!(draw_list.uniforms_gen, 6);
    }
}
