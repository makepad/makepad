use crate::recording_buffer::{CpuReservation, RecordingBudget, RecordingBuffer};
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
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

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
        unsafe {
            value
                .as_mut_ptr()
                .cast::<u8>()
                .write_bytes(0, std::mem::size_of::<T>());
        }
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
    items: Vec<(PreparedDrawBox<CxDrawItem>, RecordingBuffer)>,
    pointers: Vec<Box<CxDrawItem>>,
    shader_checks: Vec<u64>,
    child_ids: Vec<DrawListId>,
    count: usize,
    instance_floats: usize,
    metadata: Option<Arc<CpuReservation>>,
    refused: bool,
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
        Self::with_instance_capacity_in(draw_items, floats, &RecordingBudget::default())
    }
    pub fn refused(&self) -> bool {
        self.refused
    }
    pub fn with_instance_capacity_in(
        draw_items: usize,
        floats: usize,
        budget: &RecordingBudget,
    ) -> Self {
        let mut storage = Self {
            list: None,
            items: Vec::new(),
            pointers: Vec::new(),
            shader_checks: Vec::new(),
            child_ids: Vec::new(),
            count: draw_items,
            instance_floats: floats,
            metadata: None,
            refused: false,
        };
        let bytes = std::mem::size_of::<CxDrawList>()
            .saturating_add(256)
            .saturating_add(draw_items.saturating_add(1).saturating_mul(
                std::mem::size_of::<CxDrawItem>()
                    + std::mem::size_of::<RecordingBuffer>()
                    + std::mem::size_of::<DrawListId>()
                    + std::mem::size_of::<u64>()
                    + 128,
            ));
        let Some(credit) = budget.reserve(bytes) else {
            storage.refused = true;
            return storage;
        };
        storage.metadata = Some(Arc::new(credit));
        storage.items.reserve_exact(draw_items + 1);
        storage
            .items
            .push((PreparedDrawBox::new(), RecordingBuffer::new(budget.clone())));
        for _ in 0..draw_items {
            let mut instances = RecordingBuffer::new(budget.clone());
            instances.resize(floats, 1.0);
            if instances.refused() {
                storage.refused = true;
                storage.items.clear();
                return storage;
            }
            std::hint::black_box(instances.as_slice());
            instances.clear();
            storage.items.push((PreparedDrawBox::new(), instances));
        }
        storage.shader_checks = vec![u64::MAX; draw_items];
        std::hint::black_box(storage.shader_checks.as_slice());
        storage.shader_checks.clear();
        storage.child_ids = vec![DrawListId(usize::MAX, u64::MAX); draw_items];
        std::hint::black_box(storage.child_ids.as_slice());
        storage.child_ids.clear();
        storage.pointers.reserve_exact(draw_items);
        storage.list = Some(PreparedDrawBox::new());
        storage
    }

    fn restore_instance_capacity(&mut self, items: &mut CxDrawItems) {
        if self.instance_floats == 0 {
            return;
        }
        items.instance_counters.set(None);
        let mut spares = self.items.iter_mut().rev();
        for item in &mut items.buffer[..self.count] {
            let instances = item.instances.as_mut().unwrap();
            if instances.capacity() >= self.instance_floats {
                continue;
            }
            let Some((_, spare)) =
                spares.find(|(_, buffer)| buffer.capacity() >= self.instance_floats)
            else {
                break;
            };
            // The old allocation travels back in the worker package. No
            // allocation or final disposal occurs while reviving a warm slot.
            std::mem::swap(instances, spare);
        }
    }

    fn install_pointer_capacity(&mut self, items: &mut CxDrawItems) {
        if items.buffer.capacity() < self.pointers.capacity() {
            self.pointers.extend(items.buffer.drain(..));
            std::mem::swap(&mut items.buffer, &mut self.pointers);
            if let Some(credit) = &self.metadata {
                items.inventory_credits.push(credit.clone());
            }
        }
    }

    fn install_inventory_capacity(&mut self, list: &mut CxDrawList) {
        if list.find_appendable_draw_shader_check.capacity() < self.shader_checks.capacity()
            || list.draw_items.child_inventory.capacity() < self.child_ids.capacity()
        {
            if let Some(credit) = &self.metadata {
                list.draw_items.inventory_credits.push(credit.clone());
            }
        }
        self.install_pointer_capacity(&mut list.draw_items);
        if list.find_appendable_draw_shader_check.capacity() < self.shader_checks.capacity() {
            self.shader_checks
                .extend(list.find_appendable_draw_shader_check.drain(..));
            std::mem::swap(
                &mut self.shader_checks,
                &mut list.find_appendable_draw_shader_check,
            );
        }
        if list.draw_items.child_inventory.capacity() < self.child_ids.capacity() {
            self.child_ids
                .extend(list.draw_items.child_inventory.drain(..));
            std::mem::swap(&mut self.child_ids, &mut list.draw_items.child_inventory);
        }
    }

    /// Supplies only the next recording slot. A parent's inventory can be
    /// prepared on a worker without initializing all its items on one frame.
    pub fn prepare_next_item(&mut self, list: &mut CxDrawList) -> bool {
        let count = list.draw_items.used + 1;
        self.prepare_items(list, count)
    }
    /// Install an already allocated small recording inventory before a host
    /// emits its retained streams and nested sub-list links.
    pub fn prepare_items(&mut self, list: &mut CxDrawList, count: usize) -> bool {
        if list.draw_items.buffer.len() >= count {
            return true;
        }
        if self.refused {
            return false;
        }
        self.install_inventory_capacity(list);
        if list.draw_items.buffer.capacity() < count
            || self.items.len() < count.saturating_sub(list.draw_items.buffer.len())
        {
            return false;
        }
        list.draw_items.install_prepared_items(count, self);
        true
    }
}

impl DrawList {
    /// The caller reserves the parent's known pointer inventory once. No
    /// allocation waits occur here; an unavailable package leaves it untouched.
    pub fn new_prepared(
        cx: &mut Cx,
        parent: DrawListId,
        storage: &mut DrawListRecordingStorage,
    ) -> Option<Self> {
        let parent_items = &cx.draw_lists[parent].draw_items;
        let parent_count = parent_items.used + 1;
        let extra_parent = parent_count.saturating_sub(parent_items.buffer.len());
        if parent_items.buffer.capacity() < parent_count
            || (storage.items.len() < storage.count + extra_parent
                && (!storage.refused || extra_parent != 0))
            || (cx.draw_lists.0.free_count() == 0 && storage.list.is_none())
        {
            return None;
        }
        let draw_list = Self::new_detached_prepared(cx, storage)?;
        cx.draw_lists[parent]
            .draw_items
            .install_prepared_items(parent_count, storage);
        Some(draw_list)
    }

    /// Creates a retained list before its future painter parent is known.
    pub fn new_detached_prepared(
        cx: &mut Cx,
        storage: &mut DrawListRecordingStorage,
    ) -> Option<Self> {
        let warm = cx.draw_lists.0.try_alloc_reusing(32, |list| {
            list.draw_items.buffer.len() >= storage.count
                && list.draw_items.child_inventory.capacity() >= storage.count
                && list.find_appendable_draw_shader_check.capacity() >= storage.count
                && (!storage.refused
                    || storage.instance_floats == 0
                    || list.draw_items.buffer[..storage.count].iter().all(|item| {
                        item.instances
                            .as_ref()
                            .is_some_and(|b| b.capacity() >= storage.instance_floats)
                    }))
        });
        let draw_list = if let Some(id) = warm {
            let list = DrawList(id);
            cx.draw_lists.reset_allocated(list.id());
            list
        } else {
            if storage.refused
                || storage.items.len() < storage.count
                || (cx.draw_lists.0.free_count() == 0 && storage.list.is_none())
            {
                return None;
            }
            if cx.draw_lists.0.free_count() != 0 {
                cx.draw_lists.alloc()
            } else {
                let list = storage.list.take().unwrap().initialize(CxDrawList {
                    recording_metadata: storage.metadata.clone(),
                    ..CxDrawList::default()
                });
                let list = DrawList(cx.draw_lists.0.alloc_new(Some(list)));
                cx.draw_lists.reset_allocated(list.id());
                list
            }
        };
        let list = &mut cx.draw_lists[draw_list.id()];
        storage.install_inventory_capacity(list);
        list.draw_items
            .install_prepared_items(storage.count, storage);
        storage.restore_instance_capacity(&mut list.draw_items);
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
        for candidates in &mut scan.candidates {
            candidates.clear();
            candidates.reserve(256);
        }
        scan.allowance = [self.draw_lists.1.limit; 4];
        scan.deferred_bytes = 0;
        scan.critical.clear();
        scan.immediate.clear();
        scan.critical_eligible = 0;
        scan.critical_deferred = 0;
        scan.examined = 0;
        scan.eligible = [0; 4];
        scan.turn = scan.turn.wrapping_add(1);
        self.collect_instance_uploads(
            root,
            crate::retained_instances::UploadCategory::Other,
            2,
            &mut scan,
        );
        let mut available = self.draw_lists.1.limit;
        // Frame ink first, all of it: a draw item's own data is copied whole
        // in the frame that recorded it.
        for index in 0..scan.immediate.len() {
            let request = scan.immediate[index];
            if scan.best[request.list.index()] != request.priority {
                continue;
            }
            let request = self.resolve_upload_category(request);
            scan.requests.push(request);
        }
        // Present-blocking items first: a rotating cursor must never defer a
        // payload the next paint cannot do without. The frame allowance still
        // bounds them; anything it cannot fit is counted, not silently skipped.
        // Up to 256 critical items are served every frame with no rotation at
        // all; only beyond that count does a cursor round-robin among them, so
        // a wide dirty inventory still makes bounded forward progress.
        let critical_count = scan.critical.len();
        let critical_start = if critical_count > 256 { scan.critical_cursor.min(critical_count) } else { 0 };
        let mut served = 0usize;
        let mut next_cursor = 0usize;
        for step in 0..critical_count {
            let index = (critical_start + step) % critical_count;
            let (request, bytes, stride, ordinal) = scan.critical[index];
            if scan.best[request.list.index()] != request.priority {
                continue;
            }
            if served == 256 || (bytes != 0 && available < stride) {
                scan.critical_deferred += 1;
                scan.deferred_bytes = scan.deferred_bytes.saturating_add(bytes.max(1));
                continue;
            }
            available = available.saturating_sub(bytes.min(available) / stride * stride);
            let request = self.resolve_upload_category(request);
            scan.requests.push(request);
            served += 1;
            next_cursor = ordinal + 1;
        }
        scan.critical_cursor = if critical_count > 256 && next_cursor < critical_count { next_cursor } else { 0 };
        // Reserve periodic service for each lower priority, even while a
        // pointer publication changes continuously. Painter order is untouched.
        let first = if scan.turn % 4 == 0 {
            1 + (scan.turn / 4 % 2) as usize
        } else {
            0
        };
        for rank in 0..4 {
            // Ordinary small draws receive service before bulk publications.
            let priority = if rank == 0 { 3 } else { (first + rank - 1) % 3 };
            for index in 0..scan.candidates[priority].len() {
                let (request, bytes, stride, ordinal) = scan.candidates[priority][index];
                // An alias later reached this list through a more urgent
                // ancestor. Its new bucket owns admission, never this old one.
                if scan.best[request.list.index()] != request.priority {
                    continue;
                }
                if scan.requests.len() >= scan.immediate.len() + 256 || (bytes != 0 && available < stride) {
                    scan.deferred_bytes = scan.deferred_bytes.saturating_add(bytes.max(1));
                    continue;
                }
                available = available.saturating_sub(bytes.min(available) / stride * stride);
                let request = self.resolve_upload_category(request);
                scan.cursor[priority] = ordinal + 1;
                scan.requests.push(request);
            }
        }
        for p in 0..4 {
            if scan.cursor[p] >= scan.eligible[p] {
                scan.cursor[p] = 0;
            }
        }
        scan.served = scan.requests.len();
        std::mem::take(&mut scan.requests)
    }

    fn resolve_upload_category(&self, mut request: InstanceUploadRequest) -> InstanceUploadRequest {
        if request.category == crate::retained_instances::UploadCategory::Other {
            use crate::retained_instances::UploadCategory as Category;
            let call = self.draw_lists[request.list].draw_items[request.item]
                .draw_call()
                .unwrap();
            let inputs = &self.draw_shaders[call.draw_shader_id.index]
                .mapping
                .instances
                .inputs;
            let has = |name| inputs.iter().any(|input| input.id == name);
            request.category = if has(crate::id!(roof_height)) && has(crate::id!(start)) {
                Category::Outlines
            } else if has(crate::id!(height)) && has(crate::id!(tint)) {
                Category::Walls
            } else if has(crate::id!(stripe_side)) {
                Category::Roofs
            } else if has(crate::id!(glyph_depth)) {
                Category::Labels
            } else {
                Category::Other
            };
        }
        request
    }

    /// Why the last upload collection did or did not reach one item: the
    /// collector's frame counters plus the per-list proofs it consults
    /// (`clean_leaf`, the exact counters, reachability, demand). O(1); read
    /// by the Metal hole line so a starved draw item names its cause.
    pub fn instance_upload_verdict(&self, root: DrawListId, list: DrawListId, item: usize) -> InstanceUploadVerdict {
        let scan = self.draw_lists[root].upload_collection.borrow();
        let reached = scan.best.get(list.index()).copied().unwrap_or(3);
        let target = &self.draw_lists[list];
        let counters = target.draw_items.instance_counters.get();
        InstanceUploadVerdict {
            served: scan.served,
            critical_eligible: scan.critical_eligible,
            critical_deferred: scan.critical_deferred,
            deferred_bytes: scan.deferred_bytes,
            examined: scan.examined,
            limit: self.draw_lists.1.limit,
            list_reached: reached != 3,
            list_priority: reached,
            clean_leaf: target.draw_items.clean_leaf.get(),
            counters_proof: counters.map(|c| c.map(|c| c.upload_pending).unwrap_or(true)),
            demanded: self.draw_lists.retained_list_demanded(list),
            upload_needed: item < target.draw_items.len() && target.draw_items[item].retained_upload_needed(),
        }
    }

    pub fn instance_upload_collection_critical_deferred(&self, root: DrawListId) -> usize {
        self.draw_lists[root].upload_collection.borrow().critical_deferred
    }

    /// Reuse the collector's dense reachability inventories. Unchanged passes
    /// keep their last inventory; a pass without one is conservatively protected.
    /// This avoids a second walk through cold draw-item allocations each frame.
    pub fn prepare_retained_working_set(&mut self, root: DrawListId) {
        let count = self.draw_lists.0.slot_count();
        let mut next = std::mem::take(&mut self.draw_lists.1.working_set);
        next.resize(count, true);
        let mut obsolete_tail = false;
        let root_scan = self.draw_lists[root].upload_collection.borrow();
        for index in 0..count {
            let mut demanded = root_scan.best.get(index).is_none_or(|rank| *rank != 3);
            if !demanded {
                for pass in self.passes.id_iter() {
                    if let Some(other) = self.passes[pass]
                        .main_draw_list_id
                        .filter(|other| *other != root)
                    {
                        if self.draw_lists[other]
                            .upload_collection
                            .borrow()
                            .best
                            .get(index)
                            .is_none_or(|rank| *rank != 3)
                        {
                            demanded = true;
                            break;
                        }
                    }
                }
            }
            next[index] = demanded;
            let list = &self.draw_lists.0.pool[index];
            if demanded { list.gpu_cache_last_used.set(self.repaint_id); }
            let used = list.draw_items.used;
            let previous = list.gpu_cache_recorded_len.replace(used).min(list.draw_items.buffer.len());
            obsolete_tail |= previous > used && list.draw_items.buffer[used..previous].iter()
                .any(|item| item.draw_call().is_some());
        }
        drop(root_scan);
        self.draw_lists.1.working_set = next;
        self.draw_lists.1.working_set_valid = true;
        // Camera membership is no longer retirement debt. Only a recording
        // that actually discarded backend slots needs the bounded tail scan.
        if obsolete_tail { self.draw_lists.1.working_set_scan_passes = 2; }
    }

    /// Protect the complete current pass graph, including clean leaves that
    /// need no upload and lists reached by more than one pass.
    pub fn prepare_retained_demand(&mut self, root: DrawListId, epoch: u64) {
        let epoch = epoch.saturating_add(1);
        if self.draw_lists.1.demand_epoch != epoch {
            self.draw_lists.1.demand_epoch = epoch;
            for pass in self.passes.id_iter() {
                if let Some(root) = self.passes[pass].main_draw_list_id {
                    self.mark_retained_demand(root, epoch);
                }
            }
        }
        self.mark_retained_demand(root, epoch);
    }
    fn mark_retained_demand(&self, id: DrawListId, epoch: u64) {
        if self.draw_lists.is_id_freed(id) {
            return;
        }
        let list = &self.draw_lists[id];
        if list.gpu_demand_epoch.replace(epoch) == epoch {
            return;
        }
        if let Some(children) = list.draw_items.child_inventory() {
            for &child in children {
                self.mark_retained_demand(child, epoch);
            }
        } else {
            for order in 0..list.draw_item_order_len() {
                if let Some(index) = list.draw_item_id_at_order_index(order) {
                    if let Some(child) = list.draw_items[index].sub_list() {
                        self.mark_retained_demand(child, epoch);
                    }
                }
            }
        }
    }

    pub fn instance_upload_collection_deferred(&self, root: DrawListId) -> usize {
        self.draw_lists[root]
            .upload_collection
            .borrow()
            .deferred_bytes
    }

    pub fn instance_upload_collection_examined(&self, root: DrawListId) -> usize {
        self.draw_lists[root].upload_collection.borrow().examined
    }

    pub fn recycle_instance_uploads(
        &self,
        root: DrawListId,
        mut requests: Vec<InstanceUploadRequest>,
    ) {
        requests.clear();
        self.draw_lists[root]
            .upload_collection
            .borrow_mut()
            .requests = requests;
    }

    fn collect_instance_uploads(
        &self,
        id: DrawListId,
        inherited: crate::retained_instances::UploadCategory,
        priority: u8,
        scan: &mut InstanceUploadCollection,
    ) {
        use crate::retained_instances::UploadCategory as Category;
        if self.draw_lists.is_id_freed(id) {
            return;
        }
        let list = &self.draw_lists[id];
        let priority = priority.min(list.upload_priority.min(2));
        if scan.best[id.index()] <= priority {
            return;
        }
        scan.best[id.index()] = priority;
        if list.draw_items.clean_leaf.get() {
            return;
        }
        if list
            .draw_items
            .instance_counters
            .get()
            .is_some_and(|counts| counts.is_ok_and(|counts| !counts.upload_pending))
        {
            // The exact counters just consumed this leaf during drawing. A
            // clean publication proof avoids rereading its cold item payloads.
            list.draw_items.clean_leaf.set(true);
            return;
        }
        let category = match list.debug_id {
            id if id == crate::id!(atlas_labels) || id == crate::id!(atlas_label) => {
                Category::Labels
            }
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
            let Some(call) = item.draw_call() else {
                continue;
            };
            if !item.retained_upload_needed() || call.total_instance_slots == 0 {
                continue;
            }
            clean_leaf = false;
            // The frame allowance is charged what this request copies: a
            // publication continuing from its resident prefix owes its tail,
            // not its whole length (charging the whole length let one large
            // file's tail take the frame's allowance and served the rest of
            // the inventory one item per frame — thousands of frames).
            let bytes = item.retained_instances.as_ref().map_or_else(
                || item.instances.as_ref().map_or(0, |v| v.len() * 4),
                |p| (item.retained_upload_range.len() * 4).min(p.byte_len()),
            );
            let stride = call.total_instance_slots * 4;
            // A payload the next paint cannot present without: a fresh immediate
            // publication, a first retained publication, or a GPU-evicted one
            // (mirrors CxDrawItem::blocks_present without backend residency).
            let wanted = item.retained_instances.as_ref().map_or_else(
                || item.instances.as_ref().map_or(0, |v| v.len() / call.total_instance_slots),
                |_| item.retained_instance_count,
            );
            if item.retained_instances.is_none() {
                scan.immediate.push(InstanceUploadRequest { list: id, item: item_id, category, priority });
                continue;
            }
            let critical = wanted != 0
                && !item.retained_progressive
                && (item.retained_gpu_evicted || item.retained_instance_id == 0);
            if critical {
                let ordinal = scan.critical_eligible;
                scan.critical_eligible += 1;
                scan.critical.push((
                    InstanceUploadRequest { list: id, item: item_id, category, priority },
                    bytes,
                    stride,
                    ordinal,
                ));
                continue;
            }
            let p = if bytes <= 64 * 1024 && item.retained_instances.is_none() {
                3
            } else {
                priority as usize
            };
            let ordinal = scan.eligible[p];
            scan.eligible[p] += 1;
            if ordinal < scan.cursor[p] {
                scan.deferred_bytes = scan.deferred_bytes.saturating_add(bytes.max(1));
                continue;
            }
            if scan.candidates[p].len() == 256 || (bytes != 0 && scan.allowance[p] < stride) {
                scan.deferred_bytes = scan.deferred_bytes.saturating_add(bytes.max(1));
                continue;
            }
            scan.allowance[p] =
                scan.allowance[p].saturating_sub(bytes.min(scan.allowance[p]) / stride * stride);
            scan.candidates[p].push((
                InstanceUploadRequest {
                    list: id,
                    item: item_id,
                    category,
                    priority,
                },
                bytes,
                stride,
                ordinal,
            ));
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
                        .unwrap_or_else(|| {
                            (*width as u64)
                                .saturating_mul(*height as u64)
                                .saturating_mul(4)
                        })
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
                        .unwrap_or_else(|| {
                            (*width as u64)
                                .saturating_mul(*height as u64)
                                .saturating_mul(16)
                        })
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
                || {
                    draw_item
                        .instances
                        .as_ref()
                        .map_or(0, |instances| instances.len() / instance_slots)
                },
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
                        || draw_item.instances.as_ref().map_or(0usize, |v| v.len()),
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
    candidates: [Vec<(InstanceUploadRequest, usize, usize, usize)>; 4],
    cursor: [usize; 4],
    eligible: [usize; 4],
    turn: u64,
    allowance: [usize; 4],
    requests: Vec<InstanceUploadRequest>,
    deferred_bytes: usize,
    examined: usize,
    /// Items whose absence this frame would block the present (a fresh
    /// immediate payload, a first or GPU-evicted publication). They are
    /// served before every rotating class, bounded only by the frame allowance.
    critical: Vec<(InstanceUploadRequest, usize, usize, usize)>,
    /// Frame ink owed this frame: served whole, every frame, before anything
    /// else — no cap, no allowance, no rotation (residency by construction).
    immediate: Vec<InstanceUploadRequest>,
    critical_cursor: usize,
    critical_eligible: usize,
    critical_deferred: usize,
    /// Requests handed to the backend by the last collection (a diagnostic
    /// for the hole line: "served N of the critical M this frame").
    served: usize,
}

/// The upload collector's view of one draw item after its last run.
#[derive(Clone, Copy, Debug, Default)]
pub struct InstanceUploadVerdict {
    pub served: usize,
    pub critical_eligible: usize,
    pub critical_deferred: usize,
    pub deferred_bytes: usize,
    pub examined: usize,
    pub limit: usize,
    /// The collector's walk from the root reached this list at all.
    pub list_reached: bool,
    pub list_priority: u8,
    /// The list's cached "nothing to upload here" proof — a stale one hides
    /// an item from every collection until the list re-records.
    pub clean_leaf: bool,
    /// The exact counters' `upload_pending` (None: no counters cached).
    pub counters_proof: Option<bool>,
    pub demanded: bool,
    pub upload_needed: bool,
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

type RetiredItem<P> = (
    Option<P>,
    Option<crate::retained_instances::RetainedInstances>,
    Option<RecordingBuffer>,
    Option<Arc<crate::retained_instances::RetainedAllocationRecord>>,
);
struct RetirementBatch<P> {
    items: [Option<RetiredItem<P>>; 64],
}
struct RetirementBatches<P: Send + 'static> {
    available: Vec<Box<RetirementBatch<P>>>,
    preparing: Option<crate::thread::TaskHandle<Vec<Box<RetirementBatch<P>>>>>,
    initialized: bool,
    returned_tx: std::sync::mpsc::SyncSender<Box<RetirementBatch<P>>>,
    returned_rx: std::sync::mpsc::Receiver<Box<RetirementBatch<P>>>,
}
impl<P: Send + 'static> RetirementBatches<P> {
    fn new() -> Self {
        let (returned_tx, returned_rx) = std::sync::mpsc::sync_channel(16);
        Self {
            available: Vec::new(),
            preparing: None,
            initialized: false,
            returned_tx,
            returned_rx,
        }
    }
    fn poll(&mut self, pool: &crate::thread::TaskPool) -> bool {
        if let Some(result) = self
            .preparing
            .as_mut()
            .and_then(crate::thread::TaskHandle::try_take)
        {
            self.preparing = None;
            if let Ok(available) = result {
                self.available = available;
                self.initialized = true;
            }
        }
        if !self.initialized {
            if self.preparing.is_none() {
                if let Ok(slot) = pool.reserve(crate::thread::Lane::Heavy) {
                    self.preparing = Some(slot.submit_named("retirement batch storage", || {
                        (0..16)
                            .map(|_| {
                                PreparedDrawBox::new().initialize(RetirementBatch {
                                    items: std::array::from_fn(|_| None),
                                })
                            })
                            .collect()
                    }));
                }
            }
            return false;
        }
        for _ in 0..16 {
            let Ok(batch) = self.returned_rx.try_recv() else {
                break;
            };
            // Exactly sixteen envelopes circulate among this vector, workers,
            // and the bounded return channel. Returned envelopes are empty.
            debug_assert!(self.available.len() < 16);
            self.available.push(batch);
        }
        true
    }
}

struct RetiredDrawInstances {
    // UI-owned staging, consumed only after the bounded worker queue accepts
    // a batch. The payloads contain no UI handles and travel as Arc leases.
    values: std::cell::RefCell<Vec<crate::retained_instances::RetainedInstances>>,
    recording_releases: std::cell::RefCell<std::collections::VecDeque<(DrawListId, usize)>>,
    pending: Arc<std::sync::atomic::AtomicBool>,
}
pub struct CxDrawListPool(
    // Keep uniforms, backend state and inventory scratch at stable addresses.
    // Growing the pool then moves only handles, not every live draw list.
    pub(crate) IdPool<Box<CxDrawList>>,
    pub crate::retained_instances::RetainedUploadBudget,
    Option<(DrawListId, usize)>,
    std::rc::Rc<RetiredDrawInstances>,
    std::collections::HashMap<std::any::TypeId, Box<dyn std::any::Any>>,
    (usize, usize),
);
impl Default for CxDrawListPool {
    fn default() -> Self {
        let budget = crate::retained_instances::RetainedUploadBudget::default();
        let retired = std::rc::Rc::new(RetiredDrawInstances {
            values: std::cell::RefCell::new(Vec::with_capacity(4096)),
            recording_releases: std::cell::RefCell::new(std::collections::VecDeque::with_capacity(4096)),
            pending: budget.retirement_queued.clone(),
        });
        Self(
            Default::default(),
            budget,
            None,
            retired,
            Default::default(),
            (0, 0),
        )
    }
}
impl CxDrawListPool {
    pub fn prepared_metadata_leases(&self) -> usize {
        self.0
            .pool
            .iter()
            .filter(|s| s.recording_metadata.is_some())
            .count()
    }
    /// Free slots keep reusable metadata, but their old backing allocations
    /// are not residents. Reserve worker service before detaching storage, so
    /// a full queue preserves both the payload and this bounded retry cursor.
    pub fn retire_free_items<P: Send + 'static>(
        &mut self,
        pool: &crate::thread::TaskPool,
        frame: u64,
        mut take_backend: impl FnMut(&mut CxOsDrawCall) -> P,
    ) -> bool {
        self.retire_free_items_with_ids(pool, frame, |_, _, os| take_backend(os))
    }
    /// The software queue model keeps backend allocations outside CxOsDrawCall.
    /// Give it the exact physical item being retired, under the same reserved
    /// worker service and generation checks as a native backend.
    pub fn retire_free_items_with_ids<P: Send + 'static>(
        &mut self,
        pool: &crate::thread::TaskPool,
        frame: u64,
        mut take_backend: impl FnMut(DrawListId, usize, &mut CxOsDrawCall) -> P,
    ) -> bool {
        self.publish_instance_retirement_pending();
        if self.1.retirement_frame == Some(frame) {
            return self.has_pending_instance_retirements();
        }
        self.1.retirement_frame = Some(frame);
        if !self.has_pending_instance_retirements() {
            self.1.retirement_queued.store(false, Ordering::Release);
            return false;
        }
        let batches = self
            .4
            .entry(std::any::TypeId::of::<P>())
            .or_insert_with(|| Box::new(RetirementBatches::<P>::new()))
            .downcast_mut::<RetirementBatches<P>>()
            .unwrap();
        if !batches.poll(pool) {
            self.1.retirement_queued.store(true, Ordering::Release);
            return true;
        }
        let started = std::time::Instant::now();
        let mut examined = 0;
        // At most 256 payloads / 512 metadata visits / 200 us, with four
        // bounded worker jobs. Larger prepared envelopes drain retired scene
        // inventories without increasing UI time or flooding completion queues.
        for _ in 0..4 {
            let Ok(slot) = pool.reserve(crate::thread::Lane::Heavy) else {
                break;
            };
            let Some(mut batch) = batches.available.pop() else {
                break;
            };
            let mut count = 0;
            while count < batch.items.len() && examined < 512 && started.elapsed().as_micros() < 200
            {
                examined += 1;
                // Alternate account metadata and payloads while both are
                // pending. All envelopes were allocated on the worker.
                let payloads_pending = self.2.is_some()
                    || self.0.has_pending_retirements()
                    || !self.3.values.borrow().is_empty();
                if examined % 2 == 0 || !payloads_pending {
                    if let Some(record) = self.1.allocations.take_metadata_disposal() {
                        batch.items[count] = Some((None, None, None, Some(record)));
                        count += 1;
                        continue;
                    }
                }
                let retired = self.3.values.borrow_mut().pop();
                if let Some(retired) = retired {
                    batch.items[count] = Some((None, Some(retired), None, None));
                    count += 1;
                    continue;
                }
                let release = self.3.recording_releases.borrow_mut().pop_front();
                if let Some((id, index)) = release {
                    if self.0.is_live_generation(id.0, id.1) {
                        let items = &mut self.0.pool[id.0].draw_items;
                        if let Some(item) = items.buffer.get_mut(index) {
                            item.recording_release_owner = None;
                            if item.uploaded_recording_scratch() {
                                let storage = item.instances.replace(RecordingBuffer::new(items.recording_budget.clone()));
                                batch.items[count] = Some((None, None, storage, None));
                                count += 1;
                            }
                        }
                    }
                    // An unfinished copy is not retirement debt. A later
                    // backend mutation queues its next check; a paused/evicted
                    // owner cannot keep the event loop alive through this queue.
                    continue;
                }
                let (id, index) = match self.2 {
                    Some((id, index))
                        if !self.0.is_live_generation(id.0, id.1)
                            && self.0.pool[id.0].generation == id.1
                            && index < self.0.pool[id.0].draw_items.buffer.len() =>
                    {
                        (id, index)
                    }
                    _ => {
                        self.2 = None;
                        let Some(index) = self.0.take_free_retirement() else {
                            if self.0.has_pending_retirements() {
                                continue;
                            }
                            break;
                        };
                        self.2 = Some((DrawListId(index, self.0.pool[index].generation), 0));
                        continue;
                    }
                };
                let item = &mut self.0.pool[id.0].draw_items.buffer[index];
                batch.items[count] = Some((
                    Some(take_backend(id, index, &mut item.os)),
                    item.retained_instances.take(),
                    item.instances.take(),
                    None,
                ));
                count += 1;
                item.instances = Some(RecordingBuffer::default());
                item.kind = CxDrawKind::Empty;
                item.instance_upload_pending = false;
                self.2 = Some((id, index + 1));
            }
            if count == 0 {
                batches.available.push(batch);
                break;
            }
            let returned = batches.returned_tx.clone();
            let counter = self.1.retirements.clone();
            counter.fetch_add(1, Ordering::AcqRel);
            slot.submit_named("retained draw storage retirement", move || {
                for item in &mut batch.items {
                    drop(item.take());
                }
                // On context shutdown the receiver can disappear; disposal of
                // that empty envelope still occurs here on the worker.
                let _ = returned.try_send(batch);
                counter.fetch_sub(1, Ordering::AcqRel);
                crate::thread::SignalToUI::set_ui_signal();
            })
            .detach();
        }
        // Settlement includes rotating scans and completion/metadata debt,
        // not just the queue of freed CPU publications. Otherwise a view can
        // announce rest while the backend still evicts on later paint beats.
        self.publish_instance_retirement_pending();
        self.has_pending_instance_retirements()
    }
    /// Snapshot only non-demanded owners. Callers supply world/camera distance
    /// on the list; unknown positions sort after known nearby cached owners.
    /// No publication in any current pass can be selected, even if it is clean.
    pub fn backend_item_count(&self, id: DrawListId) -> usize {
        self[id].draw_items.buffer.len()
    }
    pub fn retained_eviction_candidates(&self) -> Vec<DrawListId> {
        let mut candidates: Vec<_> = self
            .0
            .pool
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.gpu_demand_epoch.get() != self.1.demand_epoch)
            .map(|(index, slot)| DrawListId(index, slot.generation))
            .collect();
        candidates.sort_unstable_by(|a, b| {
            self[*b]
                .gpu_eviction_distance
                .total_cmp(&self[*a].gpu_eviction_distance)
                .then_with(|| {
                    self[*a]
                        .gpu_cache_last_used
                        .get()
                        .cmp(&self[*b].gpu_cache_last_used.get())
                })
        });
        candidates
    }
    /// Bounded rotating inventory, ordered farthest first within each batch.
    /// Large cold inventories cannot turn pressure into an all-owner sort on
    /// the paint path. Repeated frames eventually visit every physical slot.
    pub fn retained_eviction_batch(&mut self, max_lists: usize) -> (Vec<(DrawListId, std::ops::Range<usize>)>, bool) {
        if self.1.eviction_cursor >= self.0.pool.len() {
            self.1.eviction_cursor = 0;
            self.1.eviction_item_cursor = 0;
        }
        if self.1.eviction_cursor == 0 && self.1.eviction_item_cursor == 0 {
            self.1.eviction_cycle_has_victims = false;
        }
        let max_lists = max_lists.min(128usize.saturating_sub(self.1.eviction_scanned));
        let mut candidates = Vec::with_capacity(max_lists);
        for _ in 0..max_lists {
            if self.1.eviction_cursor == self.0.pool.len() || self.1.eviction_items_scanned == 128 { break; }
            let index = self.1.eviction_cursor;
            let slot = &self.0.pool[index];
            self.1.eviction_scanned += 1;
            {
                // Resume within a large physical list as well as between owners.
                // Otherwise its first 128 empty slots can strand every later
                // buffer forever and falsely certify a victim-free sweep.
                let len = slot.draw_items.buffer.len();
                let start = self.1.eviction_item_cursor.min(len);
                let end = len.min(start + 128 - self.1.eviction_items_scanned);
                self.1.eviction_items_scanned += end - start;
                candidates.push((DrawListId(index, slot.generation), start..end));
                if end < len {
                    self.1.eviction_item_cursor = end;
                    break;
                }
            }
            self.1.eviction_cursor += 1;
            self.1.eviction_item_cursor = 0;
        }
        candidates.sort_unstable_by(|(a,_),(b,_)| self[*b].gpu_eviction_distance.total_cmp(&self[*a].gpu_eviction_distance)
            .then_with(|| self[*a].gpu_cache_last_used.get().cmp(&self[*b].gpu_cache_last_used.get()))
            .then_with(|| a.index().cmp(&b.index())));
        let complete = self.1.eviction_cursor == self.0.pool.len();
        (candidates, complete)
    }
    pub fn retained_list_demanded(&self, id: DrawListId) -> bool {
        !self.is_id_freed(id) && self[id].gpu_demand_epoch.get() == self.1.demand_epoch
    }
    /// Attach a ready shared-instance block to a draw item: the residency
    /// lease of the DL-1 contract. Refuses an unready, failed or released
    /// block and a range or stride that does not fit — the item keeps what
    /// it had. O(1): an `Arc` clone and a receipt read. Through the adapter
    /// the block also becomes the item's retained publication (the old draw
    /// path copies it under its allowance until DL-3's per-publication
    /// backing lands); `instance_ranges` limits the draw to the slice.
    pub fn attach_shared(
        &mut self,
        id: DrawListId,
        index: usize,
        block: &crate::shared_instances::SharedInstances,
        first: usize,
        count: usize,
    ) -> Result<(), AttachError> {
        if self.is_id_freed(id) || index >= self[id].draw_items.len() {
            return Err(AttachError::NoDrawCall);
        }
        let receipt = block.receipt();
        if receipt.released() { return Err(AttachError::Released); }
        if receipt.failed() { return Err(AttachError::Failed); }
        if !receipt.upload_ready() { return Err(AttachError::NotReady); }
        let Some(range) = block.slice(first, count) else { return Err(AttachError::RangeOutOfBounds); };
        let slots = self[id].draw_items[index]
            .kind
            .draw_call()
            .map(|call| call.total_instance_slots)
            .ok_or(AttachError::NoDrawCall)?;
        if slots == 0 || block.slots() != slots { return Err(AttachError::StrideMismatch); }
        let publication = block.retained();
        self.set_retained_publication(id, index, &publication);
        let item = &mut self[id].draw_items[index];
        item.retained_instance_count = count;
        item.retained_prefetched = true;
        item.instance_ranges = if first == 0 && count == block.count() {
            Vec::new()
        } else {
            vec![range.start as u32..range.end as u32]
        };
        item.shared = Some((block.clone(), range));
        let items = &self[id].draw_items;
        items.clean_leaf.set(false);
        items.instance_counters.set(None);
        Ok(())
    }

    /// Release a draw item's lease: the item draws nothing until the next
    /// attach; the block's backing is retired by the backend once its last
    /// reader completes (`retire_complete`).
    pub fn detach_shared(&mut self, id: DrawListId, index: usize) -> bool {
        if self.is_id_freed(id) || index >= self[id].draw_items.len() { return false; }
        let item = &mut self[id].draw_items[index];
        if item.shared.take().is_none() { return false; }
        // The lease goes with the adapter publication: the item becomes an
        // immediate with zero bytes, the collector serves it, and the backend
        // retires its buffer through the completed-reader path (contract §3.4).
        item.retained_instances = None;
        item.retained_instance_count = 0;
        item.retained_prefetched = false;
        item.instance_ranges.clear();
        item.instance_upload_pending = true;
        if let Some(call) = item.kind.draw_call_mut() {
            call.instance_dirty = true;
        }
        let items = &self[id].draw_items;
        items.clean_leaf.set(false);
        items.instance_counters.set(None);
        true
    }

    /// A list re-recorded through a publication replacement (a detached
    /// list among them) is demanded until the next working-set walk says
    /// otherwise: the O(1) counterpart of `reset_allocated` for a list that
    /// keeps its slot.
    pub fn set_retained_publication(&mut self, id: DrawListId, index: usize, publication: &crate::retained_instances::RetainedInstances) -> bool {
        let replaced = self[id].draw_items.set_retained_publication(index, publication);
        if replaced {
            if let Some(slot) = self.1.working_set.get_mut(id.index()) {
                *slot = true;
            }
        }
        replaced
    }

    /// Detach a completed cached backend publication. Keep the CPU recording
    /// and invalidate all upload/consumption proofs so re-entry retries it.
    pub fn evict_retained_item(&mut self, id: DrawListId, index: usize) {
        self.1.evictions += 1;
        // The list's cached "nothing to upload" proofs no longer hold: an
        // evicted item in a leaf whose proof said clean was never collected
        // again until the list re-recorded (the reclaim path resets both).
        self[id].draw_items.clean_leaf.set(false);
        self[id].draw_items.instance_counters.set(None);
        let item = &mut self[id].draw_items[index];
        item.retained_gpu_evicted = true;
        item.retained_instance_id = 0;
        item.resident_schema = 0;
        item.consumed_instance_id = 0;
        item.consumed_serial = 0;
        item.consumed_uniforms_gen = 0;
        item.instance_upload_pending = true;
        if let Some(publication) = &item.retained_instances {
            item.retained_upload_range = 0..publication.data().len();
        }
        if let Some(call) = item.kind.draw_call_mut() {
            call.instance_dirty = true;
        }
    }

    /// Reclaim backend-owned spare storage under physical allocation pressure.
    /// The backend may detach only completed, undisplayed allocations. Reuse
    /// the worker-prepared retirement envelopes and keep a bounded scan cursor.
    pub fn reclaim_backend_storage<P: Send + 'static>(
        &mut self,
        pool: &crate::thread::TaskPool,
        mut take_backend: impl FnMut(&mut CxOsDrawCall) -> Option<P>,
    ) {
        self.reclaim_backend_storage_with_usage(pool, |_, _, _, os| take_backend(os));
    }
    /// Bounded rotating service also exposes unused tail items of live lists.
    /// Recording fewer calls does not free the pooled slot or its GPU storage.
    pub fn reclaim_backend_storage_with_usage<P: Send + 'static>(
        &mut self,
        pool: &crate::thread::TaskPool,
        mut take_backend: impl FnMut(DrawListId, usize, bool, &mut CxOsDrawCall) -> Option<P>,
    ) {
        let batches = self
            .4
            .entry(std::any::TypeId::of::<P>())
            .or_insert_with(|| Box::new(RetirementBatches::<P>::new()))
            .downcast_mut::<RetirementBatches<P>>()
            .unwrap();
        if !batches.poll(pool) {
            return;
        }
        let Ok(slot) = pool.reserve(crate::thread::Lane::Heavy) else {
            return;
        };
        let Some(mut batch) = batches.available.pop() else {
            return;
        };
        let started = std::time::Instant::now();
        let mut count = 0;
        for _ in 0..128 {
            if count == batch.items.len()
                || started.elapsed().as_micros() >= 200
                || self.0.pool.is_empty()
            {
                break;
            }
            if self.5 .0 >= self.0.pool.len() {
                self.5 = (0, 0);
                self.1.working_set_scan_passes = self.1.working_set_scan_passes.saturating_sub(1);
            }
            let list = &mut self.0.pool[self.5 .0];
            if self.5 .1 >= list.draw_items.buffer.len() {
                self.5 .0 += 1;
                self.5 .1 = 0;
                continue;
            }
            let index = self.5 .1;
            let in_recording = index < list.draw_items.used;
            let id = DrawListId(self.5 .0, list.generation);
            let item = &mut list.draw_items.buffer[index];
            // Initial zero-count uploads can intentionally prepare an ink
            // stage. Only retire zero ink after its publication became resident.
            let hidden_resident = item.retained_zero_ink()
                && !item.instance_upload_pending
                && item
                    .retained_instances
                    .as_ref()
                    .is_some_and(|p| p.id() == item.retained_instance_id);
            let off_demand = self.1.working_set_valid
                && !self.1.working_set.get(id.index()).copied().unwrap_or(true);
            // A live recording is a cache entry, including hidden LOD slots
            // and offscreen owners. Only obsolete tails are garbage here.
            // Resident victims go through the pressure-ranked eviction path.
            let used = in_recording;
            self.5 .1 += 1;
            if let Some(payload) = take_backend(id, index, used, &mut item.os) {
                if !used && hidden_resident {
                    item.retained_gpu_evicted = true;
                    // Zero instances need no new receipt. Preserve the logical
                    // publication for CodeView's zero-ink delivery contract;
                    // nonzero re-entry is blocked until actual backing returns.
                    item.instance_upload_pending = false;
                    item.retained_upload_range =
                        0..item.retained_instances.as_ref().unwrap().data().len();
                    if let Some(call) = item.kind.draw_call_mut() {
                        call.instance_dirty = false;
                    }
                    list.draw_items.clean_leaf.set(false);
                    list.draw_items.instance_counters.set(None);
                } else if !used && off_demand {
                    item.retained_gpu_evicted = true;
                    item.retained_instance_id = 0;
                    item.resident_schema = 0;
                    item.consumed_serial = 0;
                    item.instance_upload_pending = true;
                    if let Some(publication) = &item.retained_instances {
                        item.retained_upload_range = 0..publication.data().len();
                    }
                    if let Some(call) = item.kind.draw_call_mut() {
                        call.instance_dirty = true;
                    }
                    list.draw_items.clean_leaf.set(false);
                    list.draw_items.instance_counters.set(None);
                }
                batch.items[count] = Some((Some(payload), None, None, None));
                count += 1;
            }
        }
        if count == 0 {
            batches.available.push(batch);
            return;
        }
        let returned = batches.returned_tx.clone();
        let counter = self.1.retirements.clone();
        counter.fetch_add(1, Ordering::AcqRel);
        slot.submit_named("backend spare reclamation", move || {
            for item in &mut batch.items {
                drop(item.take());
            }
            let _ = returned.try_send(batch);
            counter.fetch_sub(1, Ordering::AcqRel);
            crate::thread::SignalToUI::set_ui_signal();
        })
        .detach();
    }

    #[cfg(headless)]
    pub fn retirement_diagnostics<P: Send + 'static>(&self) -> String {
        let batches = self
            .4
            .get(&std::any::TypeId::of::<P>())
            .and_then(|b| b.downcast_ref::<RetirementBatches<P>>());
        let cursor = self.2.map(|(id, index)| {
            (
                id,
                index,
                self.0.is_live_generation(id.0, id.1),
                self.0.pool[id.0].generation,
                self.0.pool[id.0].draw_items.buffer.len(),
            )
        });
        format!(
            "frame={:?} cursor={cursor:?} pending_slots={} staged={} batches={:?} workers={} scan_passes={} allocation_pending={} published={}",
            self.1.retirement_frame,
            self.0.has_pending_retirements(),
            self.3.values.borrow().len(),
            batches.map(|b| (b.initialized, b.preparing.is_some(), b.available.len())),
            self.1.retirements.load(Ordering::Acquire),
            self.1.working_set_scan_passes,
            self.1.allocations.has_pending_retirements(),
            self.1.retirement_queued.load(Ordering::Acquire)
        )
    }

    /// Publish a current receipt, including after the last bounded backend
    /// service or on a frame with no dirty passes. The previous publication
    /// is never itself evidence of outstanding work.
    pub fn publish_instance_retirement_pending(&self) -> bool {
        let pending = self.has_pending_instance_retirements();
        self.1.retirement_queued.store(pending, Ordering::Release);
        pending
    }

    /// The terms of `has_pending_instance_retirements`, named (diagnostics:
    /// which maintenance a rest is still waiting for).
    pub fn instance_retirement_terms(&self) -> String {
        let terms = [
            ("allocations", self.1.allocations.has_pending_retirements()),
            ("batch", self.2.is_some()),
            ("pool", self.0.has_pending_retirements()),
            ("values", !self.3.values.borrow().is_empty()),
            ("releases", !self.3.recording_releases.borrow().is_empty()),
            ("retirements", self.1.retirements.load(Ordering::Acquire) != 0),
        ];
        terms.iter().filter(|(_, on)| *on).map(|(name, _)| *name).collect::<Vec<_>>().join(",")
    }

    pub fn has_pending_instance_retirements(&self) -> bool {
        // The rotating working-set scan is a paint-time census of unused
        // tail items: it advances with paints and finds nothing an idle app
        // needs, so it never holds the app's maintenance wake (it held it
        // forever at rest: 25 frames/s asking for a paint nothing wanted).
        self.1.allocations.has_pending_retirements()
            || self.2.is_some()
            || self.0.has_pending_retirements()
            || !self.3.values.borrow().is_empty()
            || !self.3.recording_releases.borrow().is_empty()
            || self.1.retirements.load(Ordering::Acquire) != 0
    }
    pub fn list_and_upload_budget(
        &mut self,
        id: DrawListId,
    ) -> (
        &mut CxDrawList,
        &mut crate::retained_instances::RetainedUploadBudget,
    ) {
        let slot = &mut self.0.pool[id.0];
        assert_eq!(slot.generation, id.1, "stale draw-list generation");
        (&mut slot.item, &mut self.1)
    }
    pub fn alloc(&mut self) -> DrawList {
        let draw_list = DrawList(self.0.alloc());
        self.reset_allocated(draw_list.id());
        draw_list
    }

    fn reset_allocated(&mut self, id: DrawListId) {
        // A recycled slot keeps the GPU resources of its draw items, never
        // the previous owner's list uniforms: a stale view_clip from a
        // dropped list would clip the new owner's instances.
        let recording_budget = self.1.recordings.clone();
        self[id].draw_items.recording_budget = recording_budget;
        self[id].draw_items.list_id = Some(id);
        self[id].draw_items.retired_instances = Some(self.3.clone());
        self[id].draw_items.clean_leaf.set(false);
        self[id].draw_items.instance_counters.set(None);
        self[id].draw_list_uniforms = Default::default();
        self[id].zbias_hold = None;
        self[id].reset_zbias = false;
        self[id].upload_priority = 2;
        self[id].gpu_demand_epoch.set(0);
        self[id].gpu_eviction_distance = f64::INFINITY;
        self[id].reset_draw_item_uniform_caches();
        // A slot allocated (or reused) since the last working-set walk is
        // demanded until that walk says otherwise — O(1) here, never a walk
        // over every list per allocation (a 160 K-item wall allocates lists
        // every frame of its load; walking them all made it 10× slower). A
        // reused slot's stale `false` once evicted a wall's fresh recordings
        // as off-demand (holes of 650 ms). A slot beyond the vector reads
        // `true` already (`unwrap_or(true)`).
        if let Some(slot) = self.1.working_set.get_mut(id.index()) {
            *slot = true;
        }
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
        !self.0.is_live_generation(index.0, index.1)
    }
    /// Physical backend storage may outlive a handle generation. Query the
    /// current slot without allocating an inventory of all live handles.
    pub fn is_slot_freed(&self, index: usize) -> bool {
        self.0.is_free(index)
    }
}

impl CxDrawListPool {
    /// Clear every retained draw call's binding of the given textures and
    /// uniform buffers, wherever it lives. A hidden retained slot keeps its
    /// bindings until it activates again, so a resource its owner released
    /// can only be freed once those slots let go. No pass is marked for
    /// repaint: a call binding a released resource draws nothing until its
    /// owner rebinds it. Returns the number of bindings cleared.
    pub fn release_bindings(
        &mut self,
        textures: &[crate::texture::TextureId],
        uniform_buffers: &[crate::uniform_buffer::UniformBufferId],
    ) -> usize {
        let mut cleared = 0;
        for list in 0..self.0.pool.len() {
            let generation = self.0.pool[list].generation;
            if !self.0.is_live_generation(list, generation) {
                continue;
            }
            let items = &mut self.0.pool[list].item.draw_items;
            for item in items.buffer.iter_mut() {
                let Some(call) = item.kind.draw_call_mut() else {
                    continue;
                };
                for slot in call.texture_slots.iter_mut() {
                    if slot.as_ref().is_some_and(|t| textures.contains(&t.texture_id())) {
                        *slot = None;
                        cleared += 1;
                    }
                }
                for slot in call.uniform_buffer_slots.iter_mut() {
                    if slot.as_ref().is_some_and(|u| uniform_buffers.contains(&u.uniform_buffer_id())) {
                        *slot = None;
                        cleared += 1;
                    }
                }
            }
        }
        cleared
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
    /// An intentionally hidden retained stream whose physical backing was reclaimed.
    pub retained_gpu_evicted: bool,
    /// Explicitly prefetched LOD backing stays resident while this list is demanded.
    /// Off-demand and unused recording slots remain eligible for reclamation.
    pub retained_prefetched: bool,
    /// Optional detail over an already painted surface. Missing backing may
    /// defer this item, but must never hold the containing window's present.
    pub retained_progressive: bool,
    pub redraw_id: u64,
    pub kind: CxDrawKind,
    // these values stick around to reduce buffer churn
    pub draw_item_id: usize,
    pub instances: Option<RecordingBuffer>,
    recording_release_owner: Option<DrawListId>,
    recording_metadata: Option<Arc<CpuReservation>>,
    /// Immutable worker payload; old callers continue using `instances`.
    pub retained_instances: Option<crate::retained_instances::RetainedInstances>,
    pub retained_instance_id: u64,
    /// Content hash of the last immediate payload a backend fully uploaded;
    /// an identical re-record is then a no-op upload. Zero = none.
    pub immediate_hash: u64,
    /// Immutable layout/font interpretation of the wanted and resident bytes.
    /// Zero preserves the ordinary immediate/retained API contract.
    pub retained_schema: u64,
    pub resident_schema: u64,
    /// Backend consumption, distinct from recording and upload admission.
    pub consumed_instance_id: u64,
    pub consumed_schema: u64,
    pub consumed_serial: u64,
    pub consumed_uniforms_gen: u64,
    /// Recording may vary draw count without changing the immutable publication.
    pub retained_instance_count: usize,
    pub retained_upload_range: std::ops::Range<usize>,
    /// The instances submitted when non-empty: absolute instance indices,
    /// each range clamped to what is resident. An owner whose one recording
    /// holds a spatially sorted inventory (the map's scene lists) or a
    /// row-ordered publication (a code view) presents the part a camera or
    /// a tile sheet needs without re-recording or re-uploading anything;
    /// the backends draw one call per range. Empty: every instance.
    /// Per backend: Metal draws one `drawIndexedPrimitives … baseInstance`
    /// per range; the headless raster submits the ranged instances; OpenGL,
    /// D3D11, WebGL and Vulkan draw the whole item (ranges ignored: more
    /// work, the same pixels).
    pub instance_ranges: Vec<std::ops::Range<u32>>,
    /// The residency lease of the DL-1 contract: a ready shared-instance
    /// block and the instance range this item draws from it (`attach_shared`
    /// refuses an unready block, so a draw of one is impossible by
    /// construction). Until DL-5 deletes the retained path, the attached
    /// block also drives the old fields through its adapter publication.
    pub shared: Option<(crate::shared_instances::SharedInstances, std::ops::Range<usize>)>,
    pub os: CxOsDrawCall,
}

/// Why `CxDrawListPool::attach_shared` refused a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachError {
    /// The block's upload has not completed (`receipt().upload_ready()` is
    /// false): draw the previous ready block, or nothing, and attach later.
    NotReady,
    /// The block's upload failed; the producer republishes.
    Failed,
    /// The block was released; a new publication is needed.
    Released,
    /// `first + count` exceeds the block's instance count.
    RangeOutOfBounds,
    /// The block's stride is not the draw call's instance stride.
    StrideMismatch,
    /// The list is dead, the item index is out of range, or the item is not a draw call.
    NoDrawCall,
}

impl CxDrawItem {
    fn uploaded_recording_scratch(&self) -> bool {
        !self.instance_upload_pending && !self.retained_gpu_evicted
            && self.kind.draw_call().is_some_and(|call| !call.instance_dirty)
            && self.retained_instances.as_ref().is_some_and(|p| p.id() == self.retained_instance_id)
            && self.instances.as_ref().is_some_and(|v| v.is_empty() && v.capacity() != 0)
    }
    /// Shared presentation policy, also used by the software recorder. Only
    /// hosts that supply a stable underlying surface opt into progressive ink.
    /// Ordinary surfaces/texture producers keep their complete-frame contract.
    /// Only an item with NO resident backing holds the present. A replacement
    /// still copying keeps its previous complete payload on screen (both the
    /// Metal and the software copy paths replace out of place or whole), which
    /// is exactly what a skipped present would have shown anyway — only later.
    pub fn blocks_present(&self, resident_instances: usize) -> bool {
        let Some(call) = self.kind.draw_call() else { return false; };
        let slots = call.total_instance_slots;
        if slots == 0 { return false; }
        let wanted = self.retained_instances.as_ref().map_or_else(
            || self.instances.as_ref().map_or(0, |v| v.len() / slots),
            |_| self.retained_instance_count);
        wanted != 0 && !(self.retained_progressive && self.retained_instances.is_some())
            && resident_instances == 0
    }
    pub fn retained_zero_ink(&self) -> bool {
        self.retained_instance_count == 0 && self.retained_instances.is_some()
    }
    pub fn retained_upload_suspended(&self) -> bool {
        self.retained_zero_ink() && !self.retained_prefetched
    }
    pub fn retained_upload_needed(&self) -> bool {
        !self.retained_upload_suspended()
            && (self.retained_gpu_evicted
                || self.instance_upload_pending
                || self
                    .kind
                    .draw_call()
                    .is_some_and(|call| call.instance_dirty))
    }

    /// Exact ordinary/retained draw delivery. A binding change cannot inherit a
    /// previous command's receipt; camera matrices in the parent stay separate.
    pub fn draw_consumption_complete(&self, completed: u64) -> bool {
        self.kind.draw_call().is_some_and(|call| {
            !call.instance_dirty && call.uniforms_gen == self.consumed_uniforms_gen
        }) && !self.instance_upload_pending
            && self.retained_consumption_complete(completed)
    }
    pub fn retained_consumption_complete(&self, completed: u64) -> bool {
        self.retained_binding_ready()
            && self.consumed_instance_id == self.retained_instance_id
            && self.consumed_schema == self.retained_schema
            && self.consumed_serial != 0
            && self.consumed_serial <= completed
    }
    /// A progressive replacement may keep drawing its previous complete
    /// backing under the same instance/uniform schema. This is presentation
    /// only: upload and consumption receipts still require the new identity.
    pub fn retained_presentable(&self, resident_instances: usize) -> bool {
        self.retained_binding_ready()
            || (self.retained_progressive
                && self.instance_upload_pending
                && !self.retained_gpu_evicted
                && self.retained_instances.is_some()
                && self.retained_instance_id != 0
                && self.retained_schema == self.resident_schema
                && resident_instances != 0)
    }

    pub fn retained_binding_ready(&self) -> bool {
        if self.retained_gpu_evicted && !self.retained_zero_ink() {
            return false;
        }
        if self.retained_instances.is_none()
            && self
                .instances
                .as_ref()
                .is_some_and(RecordingBuffer::refused)
        {
            return false;
        }
        (self.retained_schema == 0 || self.retained_schema == self.resident_schema)
            && self
                .retained_instances
                .as_ref()
                .is_none_or(|p| p.id() == self.retained_instance_id)
    }
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
    fn update_from(
        &mut self,
        mapping: &CxDrawShaderMapping,
        draw_vars: &DrawVars,
        turtle_depth: f32,
        uniforms_gen: u64,
    ) {
        debug_assert_ne!(uniforms_gen, 0);
        self.geometry_id = draw_vars.geometry_id;
        self.options.clone_from(&draw_vars.options);
        self.append_group_id = draw_vars.append_group_id;
        self.draw_shader_id = draw_vars.draw_shader_id.unwrap();
        self.total_instance_slots = mapping.instances.total_slots;
        self.draw_call_uniforms = DrawCallUniforms::default();
        self.dyn_uniforms.copy_from_slice(&draw_vars.dyn_uniforms);
        self.texture_slots.clone_from(&draw_vars.texture_slots);
        self.uniform_buffer_slots
            .clone_from(&draw_vars.uniform_buffer_slots);
        self.instance_dirty = true;
        self.uniforms_dirty = true;
        self.uniforms_gen = uniforms_gen;
        self.turtle_depth = turtle_depth;
    }

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
    pub fn resolve_zbias(&mut self, paint_order: f32, sploded: bool, uniforms_gen: u64) -> bool {
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

/// Counts of the current instance recording; uniforms and backend receipts
/// alone do not change them. Retained zero-count draws can still upload bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DrawItemInstanceCounters {
    pub calls: usize,
    pub instances: usize,
    pub dirty_instances: usize,
    pub dirty_bytes: usize,
    pub upload_pending: bool,
    pub recording_refused: bool,
}

#[derive(Default)]
pub struct CxDrawItems {
    list_id: Option<DrawListId>,
    retired_instances: Option<std::rc::Rc<RetiredDrawInstances>>,
    // A leaf proven clean by the collector needs no draw-call metadata walk.
    // Every mutable public item access invalidates this proof, including
    // patches that bypass ordinary recording. Parent lists always recurse.
    clean_leaf: std::cell::Cell<bool>,
    instance_counters: std::cell::Cell<Option<Result<DrawItemInstanceCounters, ()>>>,
    // Stable records: growing a parent's inventory moves only pointers, never
    // the large uniform/texture/backend payload of every preceding child.
    pub(crate) buffer: Vec<Box<CxDrawItem>>,
    used: usize,
    instance_capacity_hint: usize,
    first_instance_spare: RecordingBuffer,
    recording_budget: RecordingBudget,
    inventory_credits: Vec<Arc<CpuReservation>>,
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
        self.queue_recording_release(index);
        self.clean_leaf.set(false);
        self.instance_counters.set(None);
        self.child_inventory_valid = false;
        &mut self.buffer[index]
    }
}

impl CxDrawItems {
    /// Upload and binding mutations announce only their own spare storage.
    /// The worker service checks the resulting receipt after this borrow ends;
    /// no full-corpus scan follows an unrelated widget's upload.
    fn queue_recording_release(&mut self, index: usize) {
        let Some(id) = self.list_id else { return; };
        let Some(retired) = &self.retired_instances else { return; };
        let item = &mut self.buffer[index];
        if item.recording_release_owner == Some(id)
            || item.retained_instances.is_none()
            || !item.instances.as_ref().is_some_and(|v| v.is_empty() && v.capacity() != 0)
        { return; }
        let mut queue = retired.recording_releases.borrow_mut();
        if queue.len() == queue.capacity() { return; }
        queue.push_back((id, index));
        item.recording_release_owner = Some(id);
        retired.pending.store(true, Ordering::Release);
    }
    /// Replace a screen-derived immutable view without recording its parent.
    /// The producer still owns the complete CPU publication; backend storage
    /// follows its ordinary asynchronous replacement/retirement path.
    pub fn set_retained_publication(&mut self, index: usize, publication: &crate::retained_instances::RetainedInstances) -> bool {
        let item = &mut self[index];
        if item.retained_instances.as_ref().is_some_and(|p|p.id()==publication.id()) {return false;}
        item.retained_upload_range = publication.upload_since(item.retained_instance_id);
        item.retained_instances = Some(publication.clone());
        item.retained_gpu_evicted = false;
        item.instance_upload_pending = true;
        item.kind.draw_call_mut().unwrap().instance_dirty = true;
        true
    }

    /// Hide a retained stage without discarding its backing or real receipts.
    /// A cold/pending publication pauses copying until it can submit ink again.
    pub fn suspend_retained(&mut self, index: usize) -> bool {
        let item = &self.buffer[index];
        if item.retained_instances.is_none()
            || (item.retained_instance_count == 0 && !item.retained_prefetched) { return false; }
        let item = &mut self[index];
        let changed = item.retained_instance_count != 0;
        item.retained_instance_count = 0;
        item.retained_prefetched = false;
        changed
    }
    /// Presentation bindings do not change instance counts or upload ranges.
    /// Preserve those cached proofs unless the actual retained count changes.
    pub fn update_retained_presentation(
        &mut self,
        index: usize,
        count: usize,
        vars: &DrawVars,
        generation: u64,
    ) {
        let item = &mut self.buffer[index];
        if item.retained_instance_count != count {
            self.clean_leaf.set(false);
            self.instance_counters.set(None);
            item.retained_instance_count = count;
        }
        let call = item
            .kind
            .draw_call_mut()
            .expect("retained presentation requires a draw call");
        assert_eq!(Some(call.draw_shader_id), vars.draw_shader_id);
        if call.dyn_uniforms != vars.dyn_uniforms
            || call.texture_slots != vars.texture_slots
            || call.uniform_buffer_slots != vars.uniform_buffer_slots
        {
            call.dyn_uniforms = vars.dyn_uniforms;
            call.texture_slots = vars.texture_slots.clone();
            call.uniform_buffer_slots = vars.uniform_buffer_slots.clone();
            call.mark_uniforms_dirty(generation);
        }
    }
    /// Patch only the given dyn-uniform ranges of a retained item's draw call
    /// from `vars` (the shared, camera-dependent uniforms), leaving the item's
    /// own per-owner values untouched. A camera-only frame therefore never
    /// rebinds per-owner uniforms nor copies the whole block. The call is
    /// marked dirty only when a value actually changed.
    pub fn patch_retained_uniforms(
        &mut self,
        index: usize,
        ranges: &[(usize, usize)],
        vars: &DrawVars,
        generation: u64,
    ) {
        let item = &mut self.buffer[index];
        let call = item
            .kind
            .draw_call_mut()
            .expect("retained presentation requires a draw call");
        assert_eq!(Some(call.draw_shader_id), vars.draw_shader_id);
        let mut changed = false;
        for &(offset, slots) in ranges {
            for i in offset..(offset + slots).min(call.dyn_uniforms.len()) {
                if call.dyn_uniforms[i] != vars.dyn_uniforms[i] {
                    call.dyn_uniforms[i] = vars.dyn_uniforms[i];
                    changed = true;
                }
            }
        }
        if changed {
            call.mark_uniforms_dirty(generation);
        }
    }
    /// Reinterpret the same resident publication without inventing copy or
    /// consumption receipts. This changes no instance-count/upload predicate.
    pub fn stamp_retained_schema(&mut self, index: usize, schema: u64, prefetched: bool) {
        let item = &mut self.buffer[index];
        item.retained_schema = schema;
        if prefetched && !item.retained_prefetched {
            self.clean_leaf.set(false);
            item.retained_prefetched = true;
        }
        if item
            .retained_instances
            .as_ref()
            .is_some_and(|p| p.id() == item.retained_instance_id)
        {
            item.resident_schema = schema;
        }
    }
    /// Exact leaf counts, cached until any mutable item access, recording, or
    /// buffer replacement. Parents return None and keep their ordered walk.
    pub fn leaf_instance_counters(&self) -> Option<DrawItemInstanceCounters> {
        if let Some(cached) = self.instance_counters.get() {
            return cached.ok();
        }
        let mut counts = DrawItemInstanceCounters::default();
        for item in &self.buffer[..self.used] {
            counts.recording_refused |= item.retained_instances.is_none()
                && item
                    .instances
                    .as_ref()
                    .is_some_and(RecordingBuffer::refused);
            if item.sub_list().is_some() {
                self.instance_counters.set(Some(Err(())));
                return None;
            }
            let Some(call) = item.draw_call() else {
                continue;
            };
            let slots = call.total_instance_slots;
            if slots == 0 {
                continue;
            }
            counts.upload_pending |= item.retained_upload_needed();
            let instances = item.retained_instances.as_ref().map_or_else(
                || item.instances.as_ref().map_or(0, |v| v.len() / slots),
                |_| item.retained_instance_count,
            );
            counts.calls += usize::from(instances != 0);
            counts.instances += instances;
            if (call.instance_dirty && !item.retained_upload_suspended())
                || (item.retained_gpu_evicted && !item.retained_zero_ink())
            {
                let floats = item.retained_instances.as_ref().map_or_else(
                    || item.instances.as_ref().map_or(0, |v| v.len()),
                    |_| item.retained_upload_range.len(),
                );
                counts.dirty_bytes += floats * 4;
                counts.dirty_instances += floats / slots;
            }
        }
        self.instance_counters.set(Some(Ok(counts)));
        Some(counts)
    }
    /// Read a retained recording's backend receipt while the parent has begun
    /// its next recording. Reusing this slot invalidates the old redraw id.
    pub fn item_at_redraw(&self, index: usize, redraw_id: u64) -> Option<&CxDrawItem> {
        self.buffer
            .get(index)
            .map(Box::as_ref)
            .filter(|item| item.redraw_id == redraw_id)
    }
    fn install_prepared_items(&mut self, count: usize, storage: &mut DrawListRecordingStorage) {
        assert!(self.buffer.capacity() >= count);
        while self.buffer.len() < count {
            let (allocation, mut instances) = storage.items.pop().unwrap();
            instances.bind_budget(&self.recording_budget);
            self.buffer.push(allocation.initialize(CxDrawItem {
                instance_upload_pending: false,
                immediate_hash: 0,
                shared: None,
                redraw_id: 0,
                kind: CxDrawKind::Empty,
                draw_item_id: self.buffer.len(),
                instances: Some(instances),
                recording_metadata: storage.metadata.clone(),
                retained_instances: None,
                retained_instance_id: 0,
                retained_gpu_evicted: false,
                retained_prefetched: false,
                retained_progressive: false,
                recording_release_owner: None,
                retained_schema: 0,
                resident_schema: 0,
                consumed_instance_id: 0,
                consumed_schema: 0,
                consumed_serial: 0,
                consumed_uniforms_gen: 0,
                retained_instance_count: 0,
                retained_upload_range: 0..0,
                instance_ranges: Vec::new(),
                os: CxOsDrawCall::default(),
            }));
        }
    }
    /// Storage may be prepared by a worker before this list records its first
    /// draw. These helpers are for single-stream retained lists; the caller
    /// re-records immediately after swapping and owns retirement of the spare.
    pub fn prepared_metadata_bytes(&self) -> usize {
        self.inventory_credits
            .iter()
            .map(|c| c.bytes())
            .sum::<usize>()
            + self
                .buffer
                .iter()
                .filter_map(|i| i.recording_metadata.as_ref())
                .map(|c| c.bytes())
                .sum::<usize>()
    }
    pub fn recording_refused(&self) -> bool {
        if let Some(Ok(counts)) = self.instance_counters.get() {
            return counts.recording_refused;
        }
        self.buffer[..self.used].iter().any(|item| {
            item.retained_instances.is_none()
                && item
                    .instances
                    .as_ref()
                    .is_some_and(RecordingBuffer::refused)
        })
    }
    pub fn first_instance_buffer_capacity(&self) -> usize {
        self.buffer
            .first()
            .and_then(|item| item.instances.as_ref())
            .map_or(
                self.first_instance_spare.capacity(),
                RecordingBuffer::capacity,
            )
    }
    pub fn item_capacity(&self) -> usize {
        self.buffer.len()
    }
    pub fn swap_first_instance_buffer(&mut self, spare: &mut RecordingBuffer) -> bool {
        if !spare.bind_budget(&self.recording_budget) {
            return false;
        }
        if let Some(item) = self.buffer.first_mut() {
            std::mem::swap(item.instances.as_mut().unwrap(), spare);
        } else {
            std::mem::swap(&mut self.first_instance_spare, spare);
        }
        self.clean_leaf.set(false);
        self.instance_counters.set(None);
        true
    }
    pub fn len(&self) -> usize {
        self.used
    }
    /// Dense child-only recording inventory. Transform walks are independent
    /// of painter order, including temporarily hidden retained children.
    pub fn child_inventory(&self) -> Option<&[DrawListId]> {
        self.child_inventory_valid.then_some(&self.child_inventory)
    }
    /// Pipeline options do not mutate instance data or child topology.
    pub fn set_depth_write(&mut self, index: usize, enabled: bool) {
        if let Some(call) = self.buffer[index].kind.draw_call_mut() {
            call.options.depth_write = enabled;
        }
    }
    /// The submitted instance ranges of an item (see
    /// `CxDrawItem::instance_ranges`); presentation only, never a
    /// recording or upload change. Returns whether they changed.
    pub fn set_instance_ranges(&mut self, index: usize, ranges: &[std::ops::Range<u32>]) -> bool {
        let item = &mut self.buffer[index];
        if item.instance_ranges.as_slice() == ranges {
            return false;
        }
        item.instance_ranges.clear();
        item.instance_ranges.extend_from_slice(ranges);
        true
    }
    /// The instances an item submits: its ranges clamped to `resident`, or
    /// `0..resident` when it has none.
    pub fn submitted_instances(&self, index: usize, resident: usize) -> usize {
        let item = &self.buffer[index];
        if item.instance_ranges.is_empty() {
            return resident;
        }
        item.instance_ranges
            .iter()
            .map(|r| (r.end.min(resident as u32)).saturating_sub(r.start.min(resident as u32)) as usize)
            .sum()
    }
    /// Record backend consumption without changing the recording's topology
    /// or instance upload state.
    pub fn record_consumption(&mut self, index: usize, serial: u64) {
        self.queue_recording_release(index);
        let item = &mut self.buffer[index];
        if item.retained_binding_ready() && !item.instance_upload_pending {
            item.consumed_instance_id = item.retained_instance_id;
            item.consumed_schema = item.resident_schema;
            item.consumed_serial = serial;
            item.consumed_uniforms_gen = item.kind.draw_call().map_or(0, |call| call.uniforms_gen);
        }
    }
    pub fn clear(&mut self) {
        self.clean_leaf.set(false);
        self.instance_counters.set(None);
        self.child_inventory.clear();
        self.child_inventory_valid = true;
        self.used = 0
    }
    /// Binding changes backend residency/uniform stamps, never instance
    /// dirtiness or child topology. Upload/recording mutations use IndexMut.
    #[cfg(all(
        not(headless),
        any(target_os = "macos", target_os = "ios", target_os = "tvos")
    ))]
    pub(crate) fn binding_mut(&mut self, index: usize) -> &mut CxDrawItem {
        self.queue_recording_release(index);
        &mut self.buffer[index]
    }
    pub fn push_item(&mut self, redraw_id: u64, kind: CxDrawKind) -> &mut CxDrawItem {
        let slots = kind.draw_call().map_or(0, |call| call.total_instance_slots);
        let item = self.push_slot(redraw_id, slots);
        item.kind = kind;
        item
    }

    // A child link must not pass a draw-call-sized enum through the stack.
    // Keep each existing call in its stable box so re-recording can update
    // its fields directly, without moving its uniform/resource payload twice.
    fn push_slot(&mut self, redraw_id: u64, slots: usize) -> &mut CxDrawItem {
        self.clean_leaf.set(false);
        self.instance_counters.set(None);
        // The returned mutable item may have its kind replaced by the caller.
        self.child_inventory_valid = false;
        let draw_item_id = self.used;
        if self.used >= self.buffer.len() {
            let capacity = slots.saturating_mul(self.instance_capacity_hint);
            let mut instances = if draw_item_id == 0 {
                std::mem::take(&mut self.first_instance_spare)
            } else {
                RecordingBuffer::new(self.recording_budget.clone())
            };
            instances.bind_budget(&self.recording_budget);
            instances.reserve(capacity);
            self.buffer.push(Box::new(CxDrawItem {
                draw_item_id,
                redraw_id,
                instances: Some(instances),
                recording_metadata: None,
                retained_instances: None,
                retained_instance_id: 0,
                retained_gpu_evicted: false,
                retained_prefetched: false,
                retained_progressive: false,
                recording_release_owner: None,
                retained_schema: 0,
                resident_schema: 0,
                consumed_instance_id: 0,
                consumed_schema: 0,
                consumed_serial: 0,
                consumed_uniforms_gen: 0,
                retained_instance_count: 0,
                retained_upload_range: 0..0,
                instance_ranges: Vec::new(),
                instance_upload_pending: false,
                immediate_hash: 0,
                shared: None,
                os: CxOsDrawCall::default(),
                kind: CxDrawKind::Empty,
            }));
        } else {
            // reuse an older one, keeping all GPU resources attached
            let draw_item = &mut self.buffer[draw_item_id];
            draw_item.retained_gpu_evicted = false;
            draw_item.retained_prefetched = false;
            draw_item.retained_progressive = false;
            draw_item.instances.as_mut().unwrap().clear();
            draw_item
                .instances
                .as_mut()
                .unwrap()
                .bind_budget(&self.recording_budget);
            if draw_item.retained_instances.is_none() {
                draw_item.retained_instance_id = 0;
            }
            if let Some(retired) = draw_item.retained_instances.take() {
                if let Some(queue) = &self.retired_instances {
                    queue.values.borrow_mut().push(retired);
                    queue.pending.store(true, Ordering::Release);
                }
            }
            draw_item.retained_schema = 0;
            draw_item.consumed_serial = 0;
            draw_item.consumed_uniforms_gen = 0;
            draw_item.instance_ranges.clear();
            // A reused slot never keeps a previous record's lease.
            draw_item.shared = None;
            draw_item.redraw_id = redraw_id;
        }
        self.used += 1;
        &mut self.buffer[draw_item_id]
    }
}

#[derive(Default)]
pub struct CxDrawList {
    recording_metadata: Option<Arc<CpuReservation>>,
    upload_collection: std::cell::RefCell<InstanceUploadCollection>,
    pub debug_id: LiveId,
    pub debug_dump: bool,
    pub debug_dump_count: u32,
    /// Copy admission order, independent of painter order: pointer, visible,
    /// then background. Zero is the pointer owner's highest priority.
    pub upload_priority: u8,
    gpu_demand_epoch: std::cell::Cell<u64>,
    gpu_cache_last_used: std::cell::Cell<u64>,
    gpu_cache_recorded_len: std::cell::Cell<usize>,
    /// Squared distance from the owner's current bounds to the demand viewport.
    /// Owners with a custom projection update this with their demand census.
    pub gpu_eviction_distance: f64,
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
        self.draw_items
            .buffer
            .reserve(count.saturating_sub(self.draw_items.buffer.len()));
        self.draw_items
            .child_inventory
            .reserve(count.saturating_sub(self.draw_items.child_inventory.len()));
        self.find_appendable_draw_shader_check
            .reserve(count.saturating_sub(self.find_appendable_draw_shader_check.len()));
    }

    /// A retained scene knows its inventory before the first quad. Reserve
    /// once at publication so aligned-instance pushes never repeatedly move a
    /// growing multi-megabyte stream during a camera/fit frame.
    pub fn reserve_instance_inventory(&mut self, count: usize) {
        if count <= self.draw_items.instance_capacity_hint {
            return;
        }
        self.draw_items.instance_capacity_hint = count;
        self.draw_items.instance_counters.set(None);
        for item in &mut self.draw_items.buffer {
            let Some(call) = item.kind.draw_call() else {
                continue;
            };
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
                    Self::append_trace_log(|| {
                        format!(
                            "append_miss shader_mismatch call_shader={} vars_shader={}",
                            draw_call.draw_shader_id.index,
                            draw_vars.draw_shader_id.unwrap().index
                        )
                    });
                } else if draw_call.append_group_id == target_group {
                    // lets compare uniforms and textures..
                    if !sh.mapping.flags.draw_call_nocompare {
                        if draw_call.geometry_id != draw_vars.geometry_id {
                            Self::append_trace_log(|| {
                                format!(
                                    "append_miss geom_mismatch shader={} at_draw_item={}",
                                    draw_call.draw_shader_id.index, i
                                )
                            });
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
                            Self::append_trace_log(|| {
                                format!(
                                    "append_barrier uniform_diff shader={} at_draw_item={}",
                                    draw_call.draw_shader_id.index, i
                                )
                            });
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
                            Self::append_trace_log(|| {
                                format!(
                                    "append_barrier texture_diff shader={} at_draw_item={}",
                                    draw_call.draw_shader_id.index, i
                                )
                            });
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
                            Self::append_trace_log(|| {
                                format!(
                                    "append_barrier uniform_buffer_diff shader={} at_draw_item={}",
                                    draw_call.draw_shader_id.index, i
                                )
                            });
                            if can_cross {
                                continue;
                            }
                            break;
                        }
                    }
                    if !draw_call.options._appendable_drawcall(&draw_vars.options) {
                        Self::append_trace_log(|| {
                            format!(
                                "append_barrier options_diff shader={} at_draw_item={}",
                                draw_call.draw_shader_id.index, i
                            )
                        });
                        if can_cross {
                            continue;
                        }
                        break;
                    }
                    Self::append_trace_log(|| {
                        format!(
                            "append_hit shader={} draw_item={} group={} draw_call_group={}",
                            draw_call.draw_shader_id.index,
                            i,
                            draw_call.append_group_id,
                            draw_call.options.draw_call_group.0
                        )
                    });
                    return Some(i);
                }
            }

            if !can_cross {
                Self::append_trace_log(|| {
                    format!(
                    "append_barrier group target={} target_draw_call_group={} barrier={} barrier_draw_call_group={} at_draw_item={}",
                    target_group,
                    target_draw_call_group,
                    draw_call.append_group_id,
                    draw_call.options.draw_call_group.0,
                    i
                )
                });
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
        Self::append_trace_log(|| {
            format!(
                "append_new shader={} group={} draw_call_group={} items_before={}",
                draw_vars
                    .draw_shader_id
                    .map(|v| v.index)
                    .unwrap_or(usize::MAX),
                draw_vars.append_group_id,
                draw_vars.options.draw_call_group.0,
                self.draw_items.len()
            )
        });
        if let Some(ds) = &draw_vars.draw_shader_id {
            self.find_appendable_draw_shader_check
                .push(ds.false_compare_check());
        } else {
            self.find_appendable_draw_shader_check.push(0);
        }
        let item = self
            .draw_items
            .push_slot(redraw_id, sh.mapping.instances.total_slots);
        if let Some(call) = item.kind.draw_call_mut() {
            call.update_from(&sh.mapping, draw_vars, turtle_depth, uniforms_gen);
        } else {
            item.kind = CxDrawKind::DrawCall(CxDrawCall::new(
                &sh.mapping,
                draw_vars,
                turtle_depth,
                uniforms_gen,
            ));
        }
        item
    }

    pub fn draw_item_order_len(&self) -> usize {
        self.draw_item_reorder
            .as_ref()
            .map(|reorder| reorder.len())
            .unwrap_or_else(|| self.draw_items.len())
    }

    /// Keep painter-order capacity across both presentation and re-recording.
    pub fn take_draw_item_reorder(&mut self) -> Vec<usize> {
        self.draw_item_reorder
            .take()
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

    pub fn clear_draw_items(&mut self, redraw_id: u64, recording_gen: u64, uniforms_gen: u64) {
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
        self.draw_items.push_slot(redraw_id, 0).kind = CxDrawKind::SubList(sub_list_id);
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
    pub fn attached_draw_lists(
        &self,
        pass_id: DrawPassId,
    ) -> std::collections::HashSet<DrawListId> {
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
        let merges =
            |existing: &Option<Texture>, new: &Option<Texture>| !texture_slots_neq(existing, new);

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

    #[test]
    fn retirement_receipt_clears_after_the_final_scan_without_another_upload() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let list = DrawList::new(&mut cx);
        cx.draw_lists.1.working_set_valid = true;
        cx.draw_lists.1.working_set_scan_passes = 2;
        assert!(cx.draw_lists.publish_instance_retirement_pending());
        let pool = cx.task_pool();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while cx.draw_lists.1.working_set_scan_passes != 0 {
            cx.draw_lists.reclaim_backend_storage(&pool, |_| None::<()>);
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        // No new upload/retire_free_items call: this is the maintenance-only
        // beat following the final content draw, as used by macOS.
        assert!(!cx.draw_lists.publish_instance_retirement_pending());
        assert!(!cx.draw_lists.1.retirement_queued.load(Ordering::Acquire));
        assert!(!cx.draw_lists.is_id_freed(list.id()));
        // A bounded deferred backend queue remains real debt until drained.
        cx.draw_lists.1.allocations.set_pending_backend_retirements(1);
        assert!(cx.draw_lists.publish_instance_retirement_pending());
        cx.draw_lists.1.allocations.set_pending_backend_retirements(0);
        assert!(!cx.draw_lists.publish_instance_retirement_pending());
        // A stale published bit cannot perpetuate itself on an empty beat.
        cx.draw_lists.1.retirement_queued.store(true, Ordering::Release);
        assert!(!cx.draw_lists.publish_instance_retirement_pending());
    }

    /// A pressure eviction detaches an item's GPU backing; the list's cached
    /// "nothing to upload here" proofs (`clean_leaf`, the exact counters) must
    /// fall with it, or the collector skips the leaf and the item is never
    /// uploaded again until the list happens to re-record (a hole that only a
    /// re-record could end — the Source Library wall, 2026-09-11).
    #[test]
    fn a_pressure_eviction_drops_the_leafs_clean_proofs() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let list = DrawList::new(&mut cx);
        let retained =
            crate::retained_instances::RetainedInstances::new(4, vec![1.0; 64].into()).unwrap();
        let item = cx.draw_lists[list.id()].draw_items.push_item(1, CxDrawKind::Empty);
        item.retained_instances = Some(retained);
        item.retained_instance_count = 16;
        let items = &cx.draw_lists[list.id()].draw_items;
        items.clean_leaf.set(true);
        items.instance_counters.set(Some(Ok(DrawItemInstanceCounters {
            calls: 1, instances: 16, dirty_instances: 0, dirty_bytes: 0,
            upload_pending: false, recording_refused: false,
        })));
        cx.draw_lists.evict_retained_item(list.id(), 0);
        let items = &cx.draw_lists[list.id()].draw_items;
        assert!(!items.clean_leaf.get(), "an evicted item is collectable again");
        assert!(items.instance_counters.get().is_none(), "the exact counters no longer prove anything");
        assert!(items[0].retained_gpu_evicted && items[0].instance_upload_pending);
        assert!(items[0].retained_upload_needed());
    }

    /// The retained-sub-list contract: a parent list may keep naming a child
    /// list that its owner dropped (a map tile evicted at event time, a hidden
    /// page). The dropped id stays dead — before AND after the pool hands its
    /// slot to a new list, whose id differs by generation — and every tree
    /// walk skips it rather than landing on whatever list now holds the slot.
    #[test]
    fn a_dropped_sub_list_stays_dead_across_slot_reuse() {
        let accounting = RecordingBudget::new(32);
        let refused = DrawListRecordingStorage::with_instance_capacity_in(2, 256, &accounting);
        assert!(
            refused.refused(),
            "admission precedes recording-package allocation"
        );
        assert_eq!(accounting.bytes(), 0);
        accounting.configure(1024 * 1024);
        let prepared = DrawListRecordingStorage::with_instance_capacity_in(2, 256, &accounting);
        assert!(!prepared.refused());
        assert!(
            accounting.bytes() > 2 * 256 * 4,
            "metadata and float capacities share the credit account"
        );
        drop(prepared);
        assert_eq!(accounting.bytes(), 0);
        {
            let mut warm = Cx::new(Box::new(|_, _| {}));
            warm.draw_lists.1.recordings = accounting.clone();
            let mut storage =
                DrawListRecordingStorage::with_instance_capacity_in(2, 256, &accounting);
            let first = DrawList::new_detached_prepared(&mut warm, &mut storage).unwrap();
            let id = first.id();
            drop(first);
            drop(storage);
            let charged = accounting.bytes();
            accounting.configure(charged);
            let mut refused =
                DrawListRecordingStorage::with_instance_capacity_in(2, 256, &accounting);
            assert!(refused.refused());
            let reused = DrawList::new_detached_prepared(&mut warm, &mut refused)
                .expect("capacity refusal still permits already-charged warm recording slots");
            assert_eq!(reused.id().index(), id.index());
            assert_ne!(reused.id().generation(), id.generation());
            assert_eq!(
                accounting.bytes(),
                charged,
                "warm reuse requires no replacement package"
            );
        }
        assert_eq!(accounting.bytes(), 0);
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let parent = DrawList::new(&mut cx);
        let child = DrawList::new(&mut cx);
        let child_id = child.id();
        cx.draw_lists[parent.id()].draw_items.clear();
        cx.draw_lists[parent.id()].append_sub_list(1, child_id);
        assert_eq!(
            cx.draw_lists[parent.id()].draw_items.child_inventory(),
            Some([child_id].as_slice())
        );
        cx.draw_lists[parent.id()]
            .draw_items
            .record_consumption(0, 1);
        assert_eq!(
            cx.draw_lists[parent.id()].draw_items.child_inventory(),
            Some([child_id].as_slice()),
            "backend consumption must preserve the retained child inventory"
        );
        assert!(!cx.draw_lists.is_id_freed(child_id));
        assert!(cx
            .attached_draw_lists_from([parent.id()])
            .contains(&child_id));

        drop(child);
        assert!(cx.draw_lists.is_id_freed(child_id));
        assert!(
            cx.draw_lists.checked_index(child_id).is_some(),
            "slot kept until reuse"
        );
        assert!(!cx
            .attached_draw_lists_from([parent.id()])
            .contains(&child_id));

        let reused = DrawList::new(&mut cx);
        assert_eq!(
            reused.id().index(),
            child_id.index(),
            "the freed slot is reused"
        );
        assert_ne!(reused.id().generation(), child_id.generation());
        assert!(cx.draw_lists.is_id_freed(child_id));
        assert!(cx.draw_lists.checked_index(child_id).is_none());
        assert!(!cx.draw_lists.is_id_freed(reused.id()));
        let attached = cx.attached_draw_lists_from([parent.id()]);
        assert!(attached.contains(&parent.id()));
        assert!(!attached.contains(&child_id));
        assert!(
            !attached.contains(&reused.id()),
            "the stale entry must not reach the new list"
        );
        let retained =
            crate::retained_instances::RetainedInstances::new(4, vec![1.0; 4096].into()).unwrap();
        let weak = retained.downgrade();
        cx.draw_lists[reused.id()]
            .draw_items
            .push_item(1, CxDrawKind::Empty)
            .retained_instances = Some(retained);
        // Two multi-slot hosts can disappear on each atlas frame. Retirement
        // must drain this small backlog without hundreds of idle presents.
        for _ in 1..8192 {
            cx.draw_lists[reused.id()]
                .draw_items
                .push_item(1, CxDrawKind::Empty);
        }
        let retired_id = reused.id();
        let backend =
            crate::retained_instances::RetainedInstances::new(4, vec![2.0; 4096].into()).unwrap();
        let backend_weak = backend.downgrade();
        let mut backend_slots = std::collections::HashMap::from([((retired_id, 0), backend)]);
        drop(reused);
        let pool = cx.task_pool();
        for frame in 0..100 {
            #[cfg(headless)]
            let pending = {
                let serial = cx.headless_simulated_submit();
                cx.headless_simulated_complete_with_storage(serial, |id, item| {
                    backend_slots.remove(&(id, item))
                });
                cx.draw_lists.has_pending_instance_retirements()
            };
            #[cfg(not(headless))]
            let pending = cx
                .draw_lists
                .retire_free_items_with_ids(&pool, frame, |id, item, _| {
                    backend_slots.remove(&(id, item))
                });
            let _ = (&pool, frame);
            if !pending {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(
            weak.live_bytes(),
            0,
            "free slots release independent CPU recordings through retirement"
        );
        assert!(
            !cx.draw_lists.has_pending_instance_retirements(),
            "bounded retirement backlog must drain within 100 frames: workers={} scans={} allocations={} freed={} pool={}",
            cx.draw_lists.1.retirements.load(Ordering::Acquire), cx.draw_lists.1.working_set_scan_passes,
            cx.draw_lists.1.allocations.has_pending_retirements(), cx.draw_lists.0.has_pending_retirements(), pool.summary()
        );
        assert!(
            backend_slots.is_empty(),
            "the exact freed backend item is detached"
        );
        assert_eq!(
            backend_weak.live_bytes(),
            0,
            "backend copies retire with their CPU recordings"
        );
        assert!(
            !cx.draw_lists.is_id_freed(parent.id()),
            "live demand is never retired"
        );
        let mut restored_storage = DrawListRecordingStorage::with_instance_capacity_in(
            1,
            1024,
            &cx.draw_lists.1.recordings,
        );
        let restored = DrawList::new_detached_prepared(&mut cx, &mut restored_storage).unwrap();
        assert_eq!(restored.id().index(), retired_id.index());
        assert!(
            cx.draw_lists[restored.id()]
                .draw_items
                .first_instance_buffer_capacity()
                >= 1024,
            "warm metadata must adopt worker-prepared instance capacity after retirement"
        );
        let old_publication =
            crate::retained_instances::RetainedInstances::new(4, vec![3.0; 4096].into()).unwrap();
        let old_weak = old_publication.downgrade();
        cx.draw_lists[restored.id()].draw_items.clear();
        cx.draw_lists[restored.id()]
            .draw_items
            .push_item(1, CxDrawKind::Empty)
            .retained_instances = Some(old_publication);
        cx.draw_lists[restored.id()].draw_items.clear();
        cx.draw_lists[restored.id()]
            .draw_items
            .push_item(2, CxDrawKind::Empty);
        assert_ne!(
            old_weak.live_bytes(),
            0,
            "re-recording must hand off the last old publication before disposing it"
        );
        for frame in 100..200 {
            if !cx.draw_lists.retire_free_items(&pool, frame, |_| ()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(
            old_weak.live_bytes(),
            0,
            "old live-list publications also retire on worker service"
        );
        let spare =
            crate::retained_instances::RetainedInstances::new(4, vec![4.0; 4096].into()).unwrap();
        let spare_weak = spare.downgrade();
        let os = &cx.draw_lists[restored.id()].draw_items[0].os as *const _ as usize;
        let mut spare = Some(spare);
        for _ in 0..100 {
            let mut visits = 0;
            cx.draw_lists.reclaim_backend_storage(&pool, |item| {
                visits += 1;
                if item as *const _ as usize == os {
                    spare.take()
                } else {
                    None
                }
            });
            assert!(visits <= 128, "pressure reclamation bounds metadata visits");
            if spare_weak.live_bytes() == 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(
            spare_weak.live_bytes(),
            0,
            "live slots can retire spare backend storage on a worker"
        );
        assert!(
            !cx.draw_lists.is_id_freed(restored.id()),
            "spare reclamation preserves the live recording"
        );
        // Every live recording remains cache storage below pressure, whether
        // hidden, prefetched or offscreen. Obsolete tails still retire above.
        let publication =
            crate::retained_instances::RetainedInstances::new(4, vec![1.0; 16].into()).unwrap();
        let item = &mut cx.draw_lists[restored.id()].draw_items[0];
        item.retained_instance_id = publication.id();
        item.retained_instances = Some(publication);
        item.retained_instance_count = 0;
        item.instance_upload_pending = false;
        cx.draw_lists
            .1
            .working_set
            .resize(cx.draw_lists.0.pool.len(), true);
        cx.draw_lists.1.working_set_valid = true;
        for (prefetched, demanded, expected_used) in [
            (true, true, true),
            (false, true, true),
            (true, false, true),
        ] {
            cx.draw_lists[restored.id()].draw_items[0].retained_prefetched = prefetched;
            cx.draw_lists.1.working_set[restored.id().index()] = demanded;
            let mut observed = false;
            for _ in 0..100 {
                cx.draw_lists
                    .reclaim_backend_storage_with_usage(&pool, |id, index, used, _| {
                        if id == restored.id() && index == 0 {
                            assert_eq!(used, expected_used);
                            observed = true;
                        }
                        None::<()>
                    });
                if observed {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            assert!(observed, "bounded collector must visit the retained LOD");
        }
    }
}

#[cfg(test)]
mod uniform_generation_tests {
    use super::*;
    use std::sync::Arc;

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
    fn inactive_retained_slots_preserve_real_upload_receipts() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let list = DrawList::new(&mut cx);
        let publication = crate::retained_instances::RetainedInstances::new(3, Arc::from([1.0,2.0,3.0])).unwrap();
        let items = &mut cx.draw_lists[list.id()].draw_items;
        items.push_item(1, CxDrawKind::DrawCall(test_draw_call(1)));
        let item = &mut items[0];
        item.retained_instances = Some(publication.clone());
        item.retained_instance_id = publication.id();
        item.retained_instance_count = 1;
        item.retained_schema = 7;
        item.resident_schema = 7;
        item.consumed_instance_id = publication.id();
        item.consumed_schema = 7;
        item.consumed_serial = 12;
        item.instance_upload_pending = false;
        item.kind.draw_call_mut().unwrap().instance_dirty = false;
        assert!(items.suspend_retained(0));
        assert!(!items[0].retained_gpu_evicted);
        assert_eq!(items[0].retained_instance_id, publication.id());
        assert_eq!(items[0].consumed_serial,12);
        assert!(!items[0].retained_upload_needed());
        items[0].retained_instance_count = 1;
        assert!(!items[0].retained_upload_needed(), "returning to a resident stage needs no copy");
        assert!(items[0].retained_consumption_complete(12));
        items[0].retained_gpu_evicted = true;
        items[0].instance_upload_pending = true;
        items.suspend_retained(0);
        assert!(!items[0].retained_upload_needed(), "cold hidden content pauses admission");
        items[0].retained_instance_count = 1;
        assert!(items[0].retained_upload_needed(), "pressure-evicted visible content must retry");
        assert!(!items[0].retained_consumption_complete(12), "never forge a resident receipt");
    }

    #[test]
    fn uploaded_retained_scratch_retires_without_losing_residency() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.draw_lists.1.recordings.configure(1 << 20);
        let list = DrawList::new(&mut cx);
        let publication = crate::retained_instances::RetainedInstances::new(3, Arc::from([1.0, 2.0, 3.0])).unwrap();
        let mut call = test_draw_call(1);
        call.total_instance_slots = 3;
        let item = cx.draw_lists[list.id()].draw_items.push_item(1, CxDrawKind::DrawCall(call));
        item.instances.as_mut().unwrap().reserve(16 << 10);
        item.retained_instances = Some(publication.clone());
        item.retained_instance_count = 1;
        // A partial copy does not release its staging storage.
        item.instance_upload_pending = true;
        let bytes = cx.draw_lists.1.recordings.bytes();
        let pool = cx.task_pool();
        cx.draw_lists.1.copied(12, 12, crate::retained_instances::UploadCategory::Code, std::time::Duration::ZERO);
        for frame in 0..20 {
            cx.draw_lists.retire_free_items(&pool, frame, |_| ());
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(cx.draw_lists.1.recordings.bytes(), bytes);
        let item = &mut cx.draw_lists[list.id()].draw_items[0];
        item.instance_upload_pending = false;
        item.kind.draw_call_mut().unwrap().instance_dirty = false;
        item.retained_instance_id = publication.id();
        cx.draw_lists[list.id()].draw_items.record_consumption(0, 12);
        cx.draw_lists.1.copied(12, 12, crate::retained_instances::UploadCategory::Code, std::time::Duration::ZERO);
        for frame in 20..120 {
            if !cx.draw_lists.retire_free_items(&pool, frame, |_| ()) { break; }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(!cx.draw_lists.has_pending_instance_retirements());
        assert_eq!(cx.draw_lists.1.recordings.bytes(), 0, "storage actually freed, not just uncharged");
        let item = &cx.draw_lists[list.id()].draw_items[0];
        assert_eq!(item.instances.as_ref().unwrap().capacity(), 0);
        assert!(item.retained_consumption_complete(12));
        assert_eq!(item.retained_instances.as_ref().unwrap().data(), publication.data());
        cx.draw_lists.evict_retained_item(list.id(), 0);
        assert!(cx.draw_lists[list.id()].draw_items[0].retained_upload_needed());
        assert_eq!(cx.draw_lists[list.id()].draw_items[0].retained_instances.as_ref().unwrap().data(), publication.data());
    }

    #[test]
    fn retained_binding_waits_for_matching_bytes_and_backend_consumption() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let list = DrawList::new(&mut cx);
        let mut call = test_draw_call(1);
        call.total_instance_slots = 3;
        let resource =
            crate::retained_instances::RetainedInstances::new(3, Arc::from([1.0, 2.0, 3.0]))
                .unwrap();
        let item = cx.draw_lists[list.id()]
            .draw_items
            .push_item(1, CxDrawKind::DrawCall(call));
        item.retained_instances = Some(resource.clone());
        item.retained_schema = 7;
        item.retained_instance_count = 1;
        assert!(item.blocks_present(0), "ordinary surface must wait for its first backing");
        item.retained_progressive = true;
        assert!(!item.blocks_present(0), "optional detail must not hold the window");
        assert!(!item.retained_binding_ready());
        item.retained_instance_id = resource.id();
        item.resident_schema = 7;
        assert!(item.retained_binding_ready());
        assert_ne!(
            item.consumed_instance_id,
            resource.id(),
            "upload is not presentation"
        );
        item.consumed_instance_id = resource.id();
        item.consumed_schema = 7;
        item.consumed_serial = 12;
        assert!(
            !item.retained_consumption_complete(11),
            "a delayed fence keeps presentation outstanding"
        );
        assert!(item.retained_consumption_complete(12));
        item.retained_schema = 8;
        item.resident_schema = 8;
        assert!(
            !item.retained_consumption_complete(12),
            "the same bytes with new bindings require a new consumption receipt"
        );
        let next = crate::retained_instances::RetainedInstances::new(3, Arc::from([4.0, 5.0, 6.0]))
            .unwrap();
        item.retained_instances = Some(next);
        item.retained_schema = 8;
        item.instance_upload_pending = true;
        assert!(item.retained_presentable(1), "progressive replacement keeps same-schema resident ink");
        assert!(!item.retained_presentable(0), "no fabricated backing");
        item.retained_schema = 9;
        assert!(!item.retained_presentable(1), "layout/stage change cannot reuse stale structure bindings");
        item.retained_schema = 8;
        item.retained_gpu_evicted = true;
        assert!(!item.retained_presentable(1), "an evicted allocation is not resident ink");
        item.retained_gpu_evicted = false;
        assert!(!item.draw_consumption_complete(100), "kept ink does not acknowledge the new publication");
        item.instance_upload_pending = false;
        assert!(
            !item.retained_binding_ready(),
            "old bytes cannot use new layout/font metadata"
        );
        assert!(
            !item.retained_consumption_complete(100),
            "a later fence cannot validate a different generation"
        );
        cx.draw_lists[list.id()].draw_items.clear();
        assert!(
            cx.draw_lists[list.id()]
                .draw_items
                .item_at_redraw(0, 1)
                .is_some(),
            "beginning the next list does not erase a backend receipt"
        );
        cx.draw_lists[list.id()]
            .draw_items
            .push_item(2, CxDrawKind::DrawCall(test_draw_call(2)));
        assert!(
            cx.draw_lists[list.id()]
                .draw_items
                .item_at_redraw(0, 1)
                .is_none(),
            "a reused slot is a different recording"
        );
        let item = &mut cx.draw_lists[list.id()].draw_items[0];
        assert!(!item.retained_progressive, "slot reuse must reset the optional-detail policy");
        assert!(
            !item.draw_consumption_complete(100),
            "ordinary replacement cannot inherit the previous receipt"
        );
        item.retained_instance_id = 0;
        item.consumed_instance_id = 0;
        item.consumed_schema = 0;
        item.consumed_serial = 13;
        item.consumed_uniforms_gen = 2;
        item.kind.draw_call_mut().unwrap().instance_dirty = false;
        assert!(!item.draw_consumption_complete(12));
        assert!(item.draw_consumption_complete(13));
        item.kind.draw_call_mut().unwrap().uniforms_gen = 3;
        assert!(
            !item.draw_consumption_complete(100),
            "a changed structure mask needs actual consumption"
        );
        item.retained_schema = 9;
        assert!(
            !item.retained_binding_ready(),
            "a replacement structure cannot interpret old instance bytes"
        );
        item.resident_schema = 9;
        assert!(
            item.retained_binding_ready(),
            "ordinary structure bytes also have a recording schema"
        );
    }

    /// The DL-1 lease: attaching a shared block is refused while its upload
    /// is pending and accepted once ready; the range must fit, the stride
    /// must match, and the item then draws exactly the attached slice.
    #[test]
    fn attach_shared_refuses_unready_blocks_and_leases_ready_ones() {
        use crate::draw_list::AttachError;
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let list = DrawList::new(&mut cx);
        let mut call = test_draw_call(1);
        call.total_instance_slots = 4;
        cx.draw_lists[list.id()].draw_items.push_item(1, CxDrawKind::DrawCall(call));
        let block = cx
            .publish_instances(4, vec![1.0; 64].into(), crate::shared_instances::PublishHints::default())
            .expect("a 64-float block publishes");
        assert_eq!(
            cx.draw_lists.attach_shared(list.id(), 0, &block, 0, 16),
            Err(AttachError::NotReady),
            "an unready block is refused; the item keeps what it had"
        );
        assert!(cx.draw_lists[list.id()].draw_items[0].shared.is_none());
        block.receipt().mark_ready();
        assert_eq!(cx.draw_lists.attach_shared(list.id(), 0, &block, 0, 17), Err(AttachError::RangeOutOfBounds));
        assert_eq!(cx.draw_lists.attach_shared(list.id(), 0, &block, 4, 8), Ok(()));
        let item = &cx.draw_lists[list.id()].draw_items[0];
        assert_eq!(item.shared.as_ref().map(|(_, r)| r.clone()), Some(4..12));
        assert_eq!(item.instance_ranges, vec![4..12]);
        assert_eq!(item.retained_instance_count, 8);
        assert!(item.retained_instances.is_some(), "the adapter publication drives the old draw path");
        let stride_mismatch = cx
            .publish_instances(3, vec![1.0; 9].into(), crate::shared_instances::PublishHints::default())
            .unwrap();
        stride_mismatch.receipt().mark_ready();
        assert_eq!(cx.draw_lists.attach_shared(list.id(), 0, &stride_mismatch, 0, 3), Err(AttachError::StrideMismatch));
        assert!(cx.draw_lists.detach_shared(list.id(), 0));
        let item = &cx.draw_lists[list.id()].draw_items[0];
        assert!(item.shared.is_none() && item.retained_instance_count == 0 && item.instance_ranges.is_empty());
        assert!(item.retained_instances.is_none(), "detach releases the adapter publication");
        assert!(item.retained_upload_needed(), "the zero-byte immediate is collected, so the backend retires the buffer");
        assert!(!cx.draw_lists.detach_shared(list.id(), 0), "detaching twice is a no-op");
    }

    #[test]
    fn upload_collector_services_beyond_256_and_lower_priorities() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let root = DrawList::new(&mut cx);
        let mut children = Vec::new();
        for index in 0..900 {
            let child = DrawList::new(&mut cx);
            let list = &mut cx.draw_lists[child.id()];
            list.debug_id = crate::id!(atlas_code_file);
            list.upload_priority = (index % 3) as u8;
            let mut call = test_draw_call(1);
            call.total_instance_slots = 3;
            call.instance_dirty = true;
            list.draw_items.push_item(1, CxDrawKind::DrawCall(call));
            list.draw_items[0].instances = Some(vec![1.0; 3].into());
            cx.draw_lists[root.id()].append_sub_list(1, child.id());
            children.push(child);
        }
        // Frame ink is served whole every frame: all 900 dirty recordings
        // are requested in the first collection, no cap, no rotation.
        let requests = cx.pending_instance_uploads(root.id());
        assert_eq!(requests.len(), children.len(), "every owed immediate item is served in its frame");
        let seen: std::collections::HashSet<_> = requests.iter().map(|r| r.list).collect();
        cx.recycle_instance_uploads(root.id(), requests);
        assert_eq!(
            seen.len(),
            children.len(),
            "continuous higher-priority work must not strand later requests"
        );
        // A wide physical list needs the same bounded forward progress when
        // evicting. Empty early slots must not hide later backing forever.
        cx.draw_lists.1.demand_epoch = 1;
        let mut retired_slots = std::collections::HashSet::new();
        let mut complete = false;
        for frame in 1..=40 {
            cx.draw_lists.1.begin_frame(frame);
            let (batch, done) = cx.draw_lists.retained_eviction_batch(128);
            assert!(batch.len() <= 128);
            assert!(batch.iter().map(|(_,range)|range.len()).sum::<usize>() <= 128);
            for (id, range) in batch {
                if id == root.id() { retired_slots.extend(range); }
            }
            if done { complete = true; break; }
        }
        assert!(complete, "bounded eviction must finish its physical inventory");
        assert_eq!(retired_slots.len(), 900, "eviction must resume beyond the first 128 slots");
        // A publication larger than the frame cap remains admitted after a
        // partial copy. Exercise the collector, physical copies and receipt,
        // rather than only calling the allowance arithmetic.
        {
            use crate::retained_instances::{
                RetainedInstances, RetainedUploadBudget, UploadCategory,
            };
            let large = DrawList::new(&mut cx);
            cx.draw_lists[large.id()].debug_id = crate::id!(atlas_code_file);
            let mut call = test_draw_call(1);
            call.total_instance_slots = 16;
            let publication =
                RetainedInstances::new(16, vec![0.25; 6 * 1024 * 1024 / 4].into()).unwrap();
            let item = cx.draw_lists[large.id()]
                .draw_items
                .push_item(1, CxDrawKind::DrawCall(call));
            item.retained_instances = Some(publication.clone());
            item.retained_instance_count = publication.count();
            item.retained_upload_range = 0..publication.data().len();
            item.retained_schema = 7;
            let mut copied = vec![0.0; publication.data().len()];
            let mut offset = 0;
            cx.draw_lists.1.limit = 4 * 1024 * 1024;
            for frame in 0..2 {
                cx.draw_lists.1.begin_frame(frame);
                let requests = cx.pending_instance_uploads(large.id());
                assert_eq!(
                    requests.len(),
                    1,
                    "the oversized publication stays schedulable"
                );
                let range = cx.draw_lists.1.range(offset..publication.byte_len(), 64);
                assert_eq!(range.len(), if frame == 0 { 4 << 20 } else { 2 << 20 });
                copied[range.start / 4..range.end / 4]
                    .copy_from_slice(&publication.data()[range.start / 4..range.end / 4]);
                cx.draw_lists.1.copied(
                    range.len(),
                    64,
                    UploadCategory::Code,
                    std::time::Duration::ZERO,
                );
                offset = range.end;
                cx.draw_lists.1.pending_bytes = publication.byte_len() - offset;
                let item = &mut cx.draw_lists[large.id()].draw_items[0];
                item.kind.draw_call_mut().unwrap().instance_dirty = false;
                item.instance_upload_pending = offset != publication.byte_len();
                if !item.instance_upload_pending {
                    item.retained_instance_id = publication.id();
                    item.resident_schema = 7;
                    item.consumed_instance_id = publication.id();
                    item.consumed_schema = 7;
                    item.consumed_serial = 2;
                }
                assert_eq!(item.retained_consumption_complete(2), frame == 1);
                cx.recycle_instance_uploads(large.id(), requests);
            }
            assert_eq!(copied.as_slice(), publication.data());
            assert_eq!(cx.draw_lists.1.pending_bytes, 0);
            assert!(cx.pending_instance_uploads(large.id()).is_empty());
            cx.draw_lists.1 = RetainedUploadBudget::default();
        }
        let items = &mut cx.draw_lists[children[0].id()].draw_items;
        let counts = items.leaf_instance_counters().unwrap();
        assert_eq!(
            (counts.calls, counts.instances, counts.dirty_bytes),
            (1, 1, 12)
        );
        assert_eq!(items.leaf_instance_counters(), Some(counts));
        items[0].kind.draw_call_mut().unwrap().instance_dirty = false;
        assert_eq!(items.leaf_instance_counters().unwrap().dirty_bytes, 0);
        assert!(!items.leaf_instance_counters().unwrap().upload_pending);
        items[0].instance_upload_pending = true;
        assert!(
            items.leaf_instance_counters().unwrap().upload_pending,
            "a clean instance can still owe a backend copy"
        );
        items[0].instance_upload_pending = false;
        items[0].retained_instances = Some(
            crate::retained_instances::RetainedInstances::new(3, Arc::from([1.0, 2.0, 3.0]))
                .unwrap(),
        );
        items[0].retained_instance_count = 0;
        // Explicit prefetch remains schedulable with zero draw calls. Ordinary
        // hidden cache slots are covered by the no-upload regression above.
        items[0].retained_prefetched = true;
        items[0].retained_upload_range = 0..3;
        items[0].kind.draw_call_mut().unwrap().instance_dirty = true;
        let zero = items.leaf_instance_counters().unwrap();
        assert_eq!((zero.calls, zero.instances, zero.dirty_bytes), (0, 0, 12));
        let call = items[0].draw_call().unwrap();
        let mut vars = DrawVars {
            area: crate::area::Area::Empty,
            dyn_instance_start: 0,
            dyn_instance_slots: 0,
            options: Default::default(),
            append_group_id: 0,
            draw_shader_id: Some(call.draw_shader_id),
            geometry_id: None,
            dyn_uniforms: call.dyn_uniforms,
            texture_slots: call.texture_slots.clone(),
            uniform_buffer_slots: call.uniform_buffer_slots.clone(),
            dyn_instances: [0.0; crate::draw_vars::DRAW_CALL_DYN_INSTANCES],
        };
        vars.dyn_uniforms[0] += 1.0;
        items.update_retained_presentation(0, 0, &vars, 701);
        items.stamp_retained_schema(0, 701, false);
        assert_eq!(
            items.instance_counters.get(),
            Some(Ok(zero)),
            "uniform/schema changes must retain exact instance counters"
        );
        assert_eq!(items[0].draw_call().unwrap().uniforms_gen, 701);
        assert_ne!(
            items[0].consumed_uniforms_gen, 701,
            "a presentation update cannot fabricate backend consumption"
        );
        items.update_retained_presentation(0, 1, &vars, 702);
        assert!(
            items.instance_counters.get().is_none(),
            "a real retained-count change invalidates cached accounting"
        );
        let active = items.leaf_instance_counters().unwrap();
        assert_eq!(
            (active.calls, active.instances, active.dirty_bytes),
            (1, 1, 12)
        );
        items.clear();
        assert_eq!(
            items.leaf_instance_counters(),
            Some(DrawItemInstanceCounters::default())
        );
        items.push_item(2, CxDrawKind::SubList(root.id()));
        assert_eq!(items.leaf_instance_counters(), None);
        items[0].kind = CxDrawKind::Empty;
        assert_eq!(
            items.leaf_instance_counters(),
            Some(DrawItemInstanceCounters::default())
        );
        items[0].instances = Some(RecordingBuffer::new(RecordingBudget::new(0)));
        items[0].instances.as_mut().unwrap().reserve(4);
        assert!(items.leaf_instance_counters().unwrap().recording_refused);
        assert!(
            items.recording_refused(),
            "cached leaf proof retains ordinary refusal"
        );
        items[0].instances.as_mut().unwrap().clear();
        assert!(!items.leaf_instance_counters().unwrap().recording_refused);
        assert!(
            !items.recording_refused(),
            "real buffer mutation refreshes refusal proof"
        );
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

/// Content hash of an immediate instance payload, for skipping the upload of
/// a re-record whose bytes did not change (a camera move re-emitting the same
/// world-space geometry). Bounded: payloads above 256 KiB are never hashed
/// (returns 0, which matches nothing), so the hash cost stays under the
/// budget of a small copy. Never returns 0 for a hashed payload.
pub fn immediate_payload_hash(data: &[f32]) -> u64 {
    if data.is_empty() || data.len() * 4 > 256 * 1024 {
        return 0;
    }
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for chunk in data.chunks(64) {
        for v in chunk {
            for b in v.to_bits().to_le_bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
    }
    h.max(1)
}
