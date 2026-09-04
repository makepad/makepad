#![allow(clippy::result_unit_err)]

use {
    crate::{
        cx_2d::Cx2d,
        cx_draw::CxDraw,
        makepad_platform::*,
        nav::*,
        turtle::{AlignEntry, Walk},
    },
    std::{ops::Deref, ops::DerefMut},
};

pub trait DrawListExt {
    fn draw_list_id(&self) -> DrawListId;
    fn set_view_transform(&self, cx: &mut Cx, mat: &Mat4f);
    fn set_view_transform_self_only(&self, cx: &mut Cx, mat: &Mat4f);
    fn begin_always(&mut self, cx: &mut CxDraw);
    fn begin_maybe(&mut self, cx: &mut CxDraw, will_redraw: bool) -> Redrawing;
    fn end(&mut self, cx: &mut CxDraw);
    fn get_view_transform(&self, cx: &Cx) -> Mat4f;
    fn map_point_to_local(&self, cx: &Cx, world: DVec2) -> DVec2;
    fn map_point_from_local(&self, cx: &Cx, local: DVec2) -> DVec2;
    fn debug_parent_draw_list_id(&self, cx: &Cx) -> Option<DrawListId>;
    fn debug_child_draw_list_ids(&self, cx: &Cx) -> Vec<DrawListId>;
    fn redraw(&self, cx: &mut Cx);
    fn redraw_self_and_children(&self, cx: &mut Cx);
}

impl DrawListExt for DrawList {
    fn draw_list_id(&self) -> DrawListId {
        self.id()
    }
    fn set_view_transform(&self, cx: &mut Cx, mat: &Mat4f) {
        fn set_view_transform_recur(draw_list_id: DrawListId, cx: &mut Cx, mat: &Mat4f) {
            /*if cx.draw_lists[draw_list_id].locked_view_transform {
                return
            }*/
            let uniforms_gen = cx.next_uniform_gen();
            cx.draw_lists[draw_list_id].set_uniform_view_transform(mat, uniforms_gen);
            let draw_order_len = cx.draw_lists[draw_list_id].draw_item_order_len();
            for order_index in 0..draw_order_len {
                let Some(draw_item_id) =
                    cx.draw_lists[draw_list_id].draw_item_id_at_order_index(order_index)
                else {
                    continue;
                };
                if let Some(sub_list_id) =
                    cx.draw_lists[draw_list_id].draw_items[draw_item_id].sub_list()
                {
                    if cx.draw_lists.is_id_freed(sub_list_id) {
                        continue;
                    }
                    set_view_transform_recur(sub_list_id, cx, mat);
                }
            }
        }
        set_view_transform_recur(self.id(), cx, mat);
    }

    fn set_view_transform_self_only(&self, cx: &mut Cx, mat: &Mat4f) {
        let uniforms_gen = cx.next_uniform_gen();
        cx.draw_lists[self.id()].set_uniform_view_transform(mat, uniforms_gen);
    }

    fn begin_always(&mut self, cx: &mut CxDraw) {
        self.begin_maybe(cx, true).expect_redraw();
    }

    fn begin_maybe(&mut self, cx: &mut CxDraw, will_redraw: bool) -> Redrawing {
        // check if we have a pass id parent
        let pass_id = cx.pass_stack.last().unwrap().pass_id;
        let redraw_id = cx.cx.redraw_id;

        cx.draw_lists[self.id()].draw_pass_id = Some(pass_id);
        // An ordinary list has no depth floor of its own; it inherits whatever
        // the walk is on. Written every begin because draw list ids are pooled
        // and a recycled slot could otherwise carry an overlay's old lift.
        cx.draw_lists[self.id()].overlay_z_lift = 0.0;

        let codeflow_parent_id = cx.draw_list_stack.last().cloned();

        let is_main_draw_list = if cx.passes[pass_id].main_draw_list_id.is_none() {
            cx.passes[pass_id].main_draw_list_id = Some(self.id());
            true
        } else {
            false
        };

        // find the parent draw list id
        if let Some(parent_id) = codeflow_parent_id {
            if !is_main_draw_list {
                let parent = &mut cx.cx.draw_lists[parent_id];
                parent.append_sub_list(redraw_id, self.id());

                cx.nav_list_item_push(parent_id, NavItem::Child(self.id()));
            }
        }

        // set nesting draw list id for incremental repaint scanning
        cx.cx.draw_lists[self.id()].codeflow_parent_id = codeflow_parent_id;

        // check redraw status
        if cx.cx.draw_lists[self.id()].draw_items.len() != 0 && !will_redraw {
            return Redrawing::no();
        }

        if cx.passes[pass_id].main_draw_list_id.unwrap() == self.id() {
            cx.passes[pass_id].paint_dirty = true;
        }

        let recording_gen = cx.cx.next_uniform_gen();
        let uniforms_gen = cx.cx.next_uniform_gen();
        cx.cx.draw_lists[self.id()].clear_draw_items(
            redraw_id,
            recording_gen,
            uniforms_gen,
        );

        cx.nav_list_clear(self.id());

        cx.draw_list_stack.push(self.id());

        Redrawing::yes()
    }

    fn end(&mut self, cx: &mut CxDraw) {
        let draw_list_id = cx.draw_list_stack.pop().unwrap();
        if draw_list_id != self.id() {
            panic!("Mismatch in drawlist id in view.end, check your begin/end pairs");
        }
        if cx.cx.draw_lists[draw_list_id].redraw_id != cx.cx.redraw_id {
            panic!("calling end on a view that didnt get begin called this redraw cycle");
        }
    }

    fn get_view_transform(&self, cx: &Cx) -> Mat4f {
        let cxview = &cx.draw_lists[self.id()];
        cxview.draw_list_uniforms.view_transform
    }

    fn map_point_to_local(&self, cx: &Cx, world: DVec2) -> DVec2 {
        let inverse = self.get_view_transform(cx).invert();
        let mapped = inverse.transform_vec4(vec4f(world.x as f32, world.y as f32, 0.0, 1.0));
        if mapped.w.abs() > 1e-6 {
            dvec2((mapped.x / mapped.w) as f64, (mapped.y / mapped.w) as f64)
        } else {
            dvec2(mapped.x as f64, mapped.y as f64)
        }
    }

    fn map_point_from_local(&self, cx: &Cx, local: DVec2) -> DVec2 {
        let mapped = self.get_view_transform(cx).transform_vec4(vec4f(
            local.x as f32,
            local.y as f32,
            0.0,
            1.0,
        ));
        if mapped.w.abs() > 1e-6 {
            dvec2((mapped.x / mapped.w) as f64, (mapped.y / mapped.w) as f64)
        } else {
            dvec2(mapped.x as f64, mapped.y as f64)
        }
    }

    fn debug_parent_draw_list_id(&self, cx: &Cx) -> Option<DrawListId> {
        cx.draw_lists[self.id()].codeflow_parent_id
    }

    fn debug_child_draw_list_ids(&self, cx: &Cx) -> Vec<DrawListId> {
        let draw_list = &cx.draw_lists[self.id()];
        let mut children = Vec::new();
        for order_index in 0..draw_list.draw_item_order_len() {
            let Some(draw_item_id) = draw_list.draw_item_id_at_order_index(order_index) else {
                continue;
            };
            if let Some(sub_list_id) = draw_list.draw_items[draw_item_id].sub_list() {
                children.push(sub_list_id);
            }
        }
        children
    }

    fn redraw(&self, cx: &mut Cx) {
        cx.redraw_list(self.id());
    }

    fn redraw_self_and_children(&self, cx: &mut Cx) {
        cx.redraw_list_and_children(self.id());
    }
}

#[derive(Debug)]
pub struct DrawList2d {
    // draw info per UI element
    pub(crate) draw_list: DrawList,
    pub(crate) dirty_check_rect: Rect,
    overlay_active: bool,
}

impl ScriptHook for DrawList2d {}
impl ScriptApply for DrawList2d {}
impl ScriptNew for DrawList2d {
    fn script_new(vm: &mut ScriptVm) -> Self {
        Self::new(vm.cx_mut())
    }
}

impl Deref for DrawList2d {
    type Target = DrawList;
    fn deref(&self) -> &Self::Target {
        &self.draw_list
    }
}
impl DerefMut for DrawList2d {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.draw_list
    }
}

/// Where the first (outermost) overlay's depth floor sits, in `world.z`.
///
/// The budget: a 2D pass is orthographic with `near = 100`, `far = -100`, so
/// `ndc_z = 0.5 - z/200` and only `z < 100` survives the near plane. Body
/// content is well under 32 — the largest `draw_depth` anywhere in this repo
/// is the dock's dragged ghost tab at 20, then tab bars and reorder lists at
/// 10, charts and score engraving at 0..8; the map parks a backdrop at -50.
/// 32 clears all of it with room to spare, and is the value the score app
/// proved by hand before this moved into the framework.
pub const OVERLAY_Z_BASE: f32 = 32.0;

/// What each further level of overlay nesting adds — a modal's dropdown over
/// the modal, a tooltip over that.
///
/// It has to beat the `draw_depth` band the *enclosing overlay's* content
/// spends. Every overlay in the repo that something can nest inside — menus,
/// dialogs, popups, tooltips — is flat, so 8 is a wide margin; the one deep
/// overlay, the dock's dragged ghost tab at 20, is a drag ghost that nothing
/// opens a popup over. 8 also keeps the ladder short enough to stay far from
/// the near plane; see [`OVERLAY_Z_MAX`].
pub const OVERLAY_Z_STEP: f32 = 8.0;

/// The ceiling on the ladder, so a pathological nesting depth cannot walk into
/// the near plane. 64 + the worst content `draw_depth` (20) + the paint-order
/// counter (0.001 per draw call, so ~2 for a very busy frame) is ~86, leaving
/// 14 units of headroom below 100.
pub const OVERLAY_Z_MAX: f32 = 64.0;

/// The depth floor for an overlay at `nesting`, counted from 1 for the
/// outermost. See [`OVERLAY_Z_BASE`], [`OVERLAY_Z_STEP`], [`OVERLAY_Z_MAX`].
pub fn overlay_z_lift(nesting: usize) -> f32 {
    let level = nesting.max(1) - 1;
    (OVERLAY_Z_BASE + OVERLAY_Z_STEP * level as f32).min(OVERLAY_Z_MAX)
}

impl DrawList2d {
    pub fn new(cx: &mut Cx) -> Self {
        let draw_list = DrawList::new(cx);
        Self {
            dirty_check_rect: Default::default(),
            draw_list,
            overlay_active: false,
        }
    }

    pub fn end(&mut self, cx: &mut Cx2d) {
        if self.overlay_active {
            self.overlay_active = false;
            cx.overlay_draw_depth = cx.overlay_draw_depth.saturating_sub(1);
        }
        self.draw_list.end(cx);
    }

    pub fn begin_overlay_last(&mut self, cx: &mut Cx2d) {
        self.begin_overlay_inner(cx, true)
    }

    pub fn begin_overlay_reuse(&mut self, cx: &mut Cx2d) {
        self.begin_overlay_inner(cx, false)
    }

    /// Begin an overlay draw list: it composites after the body of the pass,
    /// **and** above it in depth.
    ///
    /// The second half is not free. A 2D pass has one depth buffer that every
    /// draw list in it shares, and a vertex lands at
    /// `world.z = draw_depth + draw_call.zbias`. Painting later only buys a
    /// `zbias_step` (0.001) of z, so any widget that spends a real band of
    /// `draw_depth` to order its own ink — a chart, a dock's dragged tab, a
    /// score's engraving — sits in front of an overlay that was drawn after
    /// it, and the depth test throws the overlay away. Ordering alone does not
    /// put a modal over a page; this is what does.
    ///
    /// The list is therefore given a depth FLOOR ([`OVERLAY_Z_BASE`] +
    /// [`OVERLAY_Z_STEP`] per level of overlay nesting), which the backend
    /// walk raises its paint-order counter to on the way in. Only z moves: x
    /// and y are untouched, so hit testing, `map_point_to_local` and
    /// `map_point_from_local` are exactly as they were. Nothing here writes
    /// `view_transform`, so a caller that sets its own after beginning an
    /// overlay keeps the lift.
    pub fn begin_overlay_inner(&mut self, cx: &mut Cx2d, always_last: bool) {
        let pass_id = cx
            .overlay_pass_id
            .unwrap_or_else(|| cx.pass_stack.last().unwrap().pass_id);
        let redraw_id = cx.cx.redraw_id;

        cx.draw_lists[self.draw_list.id()].draw_pass_id = Some(pass_id);

        let codeflow_parent_id = cx.draw_list_stack.last().cloned().unwrap();

        let overlay_id = cx.overlay_id.unwrap();
        if always_last {
            cx.draw_lists[overlay_id].store_sub_list_last(redraw_id, self.draw_list.id());
        } else {
            cx.draw_lists[overlay_id].store_sub_list(redraw_id, self.draw_list.id());
        }

        // Stamp the draw ORDER. `store_sub_list` above only decides which slot
        // this list occupies (first free, kept forever) — it says nothing about
        // paint order, which is what a caller drawing one glass surface over
        // another actually means. `Overlay::end` sorts by this.
        cx.overlay_seq += 1;
        let seq = cx.overlay_seq;
        cx.draw_lists[self.draw_list.id()].overlay_order = seq;

        if !self.overlay_active {
            self.overlay_active = true;
            cx.overlay_draw_depth += 1;
        }

        // Lift this list out of the body's depth band. `overlay_draw_depth` is
        // the nesting count including this list, so the outermost overlay is 1.
        cx.cx.draw_lists[self.draw_list.id()].overlay_z_lift =
            overlay_z_lift(cx.overlay_draw_depth);

        cx.nav_list_item_push(codeflow_parent_id, NavItem::Child(self.draw_list.id()));

        cx.cx.draw_lists[self.draw_list.id()].codeflow_parent_id = Some(codeflow_parent_id);
        if cx.passes[pass_id].main_draw_list_id.unwrap() == self.draw_list.id() {
            cx.passes[pass_id].paint_dirty = true;
        }

        let recording_gen = cx.cx.next_uniform_gen();
        let uniforms_gen = cx.cx.next_uniform_gen();
        cx.cx.draw_lists[self.draw_list.id()].clear_draw_items(
            redraw_id,
            recording_gen,
            uniforms_gen,
        );

        cx.nav_list_clear(self.draw_list.id());

        cx.draw_list_stack.push(self.draw_list.id());
    }

    pub fn begin(&mut self, cx: &mut Cx2d, walk: Walk) -> Redrawing {
        let will_redraw = cx.will_redraw(self, walk);
        self.begin_maybe(cx, will_redraw)
    }
}

impl<'a> CxDraw<'a> {
    pub fn new_draw_call(&mut self, draw_vars: &DrawVars) -> Option<&mut CxDrawItem> {
        self.get_draw_call(false, draw_vars)
    }

    pub fn append_to_draw_call(&mut self, draw_vars: &DrawVars) -> Option<&mut CxDrawItem> {
        self.get_draw_call(true, draw_vars)
    }

    pub fn get_current_draw_list_id(&self) -> Option<DrawListId> {
        self.draw_list_stack.last().cloned()
    }

    pub fn get_draw_call(&mut self, append: bool, draw_vars: &DrawVars) -> Option<&mut CxDrawItem> {
        draw_vars.draw_shader_id?;
        let draw_shader = draw_vars.draw_shader_id.unwrap();

        // Issued before borrowing the draw-list fields; unused only when this
        // request appends to an existing draw call.
        let uniforms_gen = self.cx.next_uniform_gen();

        let sh = &self.cx.draw_shaders[draw_shader.index];

        // The nesting depth this call belongs to. `depth_target` is Some only
        // while the exploded view is up, which is what keeps batching — and so
        // the whole render — byte-identical when the mode is off.
        let turtle_depth = self.cx.nesting_depth as f32;
        let depth_target = self.cx.sploded_depth_target();

        let current_draw_list_id = *self.draw_list_stack.last().unwrap();
        let draw_list = &mut self.cx.draw_lists[current_draw_list_id];

        if append && !sh.mapping.flags.draw_call_always {
            if let Some(index) = draw_list.find_appendable_drawcall(sh, draw_vars, depth_target) {
                return Some(&mut draw_list.draw_items[index]);
            }
        }

        Some(draw_list.append_draw_call(
            self.cx.redraw_id,
            sh,
            draw_vars,
            turtle_depth,
            uniforms_gen,
        ))
    }

    pub fn begin_many_instances(&mut self, draw_vars: &DrawVars) -> Option<ManyInstances> {
        let draw_list_id = self.get_current_draw_list_id().unwrap();
        let draw_item = self.append_to_draw_call(draw_vars);
        draw_item.as_ref()?;
        let draw_item = draw_item.unwrap();
        //let draw_call = draw_item.kind.draw_call().unwrap();
        let mut instances = None;

        std::mem::swap(&mut instances, &mut draw_item.instances);
        Some(ManyInstances {
            instance_area: InstanceArea {
                draw_list_id,
                draw_item_id: draw_item.draw_item_id,
                instance_count: 0,
                instance_offset: instances.as_ref().unwrap().len(),
                redraw_id: draw_item.redraw_id,
            },
            aligned: None,
            instances: instances.unwrap(),
        })
    }

    pub fn end_many_instances(&mut self, many_instances: ManyInstances) -> Area {
        let mut ia = many_instances.instance_area;
        let draw_list = &mut self.draw_lists[ia.draw_list_id];
        let draw_item = &mut draw_list.draw_items[ia.draw_item_id];
        let draw_call = draw_item.kind.draw_call().unwrap();

        let mut instances = Some(many_instances.instances);
        std::mem::swap(&mut instances, &mut draw_item.instances);
        ia.instance_count = (draw_item.instances.as_ref().unwrap().len() - ia.instance_offset)
            / draw_call.total_instance_slots;
        ia.into()
    }

    pub fn add_instance(&mut self, draw_vars: &DrawVars) -> Area {
        let data = draw_vars.as_slice();
        let draw_list_id = self.get_current_draw_list_id().unwrap();
        let draw_item = self.append_to_draw_call(draw_vars);
        if draw_item.is_none() {
            return Area::Empty;
        }
        let draw_item = draw_item.unwrap();
        let draw_call = draw_item.draw_call().unwrap();
        if draw_call.total_instance_slots == 0 {
            error!("Draw shader {:?} has no instance slots; nothing drawn", draw_call.draw_shader_id);
            return Area::Empty;
        }
        let instance_count = data.len() / draw_call.total_instance_slots;
        let check = data.len() % draw_call.total_instance_slots;
        if check > 0 {
            panic!("Data not multiple of total slots");
        }
        let ia = InstanceArea {
            draw_list_id,
            draw_item_id: draw_item.draw_item_id,
            instance_count,
            instance_offset: draw_item.instances.as_ref().unwrap().len(),
            redraw_id: draw_item.redraw_id,
        };
        draw_item
            .instances
            .as_mut()
            .unwrap()
            .extend_from_slice(data);
        ia.into()
    }
}

impl<'a, 'b> Cx2d<'a, 'b> {
    pub fn begin_many_aligned_instances(&mut self, draw_vars: &DrawVars) -> Option<ManyInstances> {
        let mut li = self.begin_many_instances(draw_vars);
        li.as_ref()?;
        li.as_mut().unwrap().aligned = Some(self.align_list.len());
        self.align_list.push(AlignEntry::Unset);
        li
    }

    pub fn end_many_instances(&mut self, many_instances: ManyInstances) -> Area {
        let mut ia = many_instances.instance_area;
        let draw_list = &mut self.draw_lists[ia.draw_list_id];
        let draw_item = &mut draw_list.draw_items[ia.draw_item_id];
        let draw_call = draw_item.kind.draw_call().unwrap();

        let mut instances = Some(many_instances.instances);
        std::mem::swap(&mut instances, &mut draw_item.instances);
        ia.instance_count = (draw_item.instances.as_ref().unwrap().len() - ia.instance_offset)
            / draw_call.total_instance_slots;
        if let Some(aligned) = many_instances.aligned {
            self.align_list[aligned] = AlignEntry::Area(ia.into());
        }
        ia.into()
    }

    pub fn add_aligned_instance(&mut self, draw_vars: &DrawVars) -> Area {
        let data = draw_vars.as_slice();
        let draw_list_id = self.get_current_draw_list_id().unwrap();
        let draw_item = self.append_to_draw_call(draw_vars);
        if draw_item.is_none() {
            return Area::Empty;
        }
        let draw_item = draw_item.unwrap();
        let draw_call = draw_item.draw_call().unwrap();
        if draw_call.total_instance_slots == 0 {
            error!("Draw shader {:?} has no instance slots; nothing drawn", draw_call.draw_shader_id);
            return Area::Empty;
        }
        let instance_count = data.len() / draw_call.total_instance_slots;
        let check = data.len() % draw_call.total_instance_slots;
        if check > 0 {
            error!("Data not multiple of total slots");
            return Area::Empty;
        }
        let ia: Area = (InstanceArea {
            draw_list_id,
            draw_item_id: draw_item.draw_item_id,
            instance_count,
            instance_offset: draw_item.instances.as_ref().unwrap().len(),
            redraw_id: draw_item.redraw_id,
        })
        .into();
        draw_item
            .instances
            .as_mut()
            .unwrap()
            .extend_from_slice(data);
        self.align_list.push(AlignEntry::Area(ia));
        ia
    }

    pub fn add_aligned_rect_area(&mut self, area: &mut Area, rect: Rect) {
        let draw_list_id = *self.draw_list_stack.last().unwrap();
        let draw_list = &mut self.cx.draw_lists[draw_list_id];
        // ok so we have to add
        let rect_id = draw_list.rect_areas.len();
        draw_list.rect_areas.push(CxRectArea {
            rect,
            draw_clip: Default::default(),
        });

        let new_area = Area::Rect(RectArea {
            draw_list_id,
            redraw_id: self.redraw_id,
            rect_id,
        });
        self.align_list.push(AlignEntry::Area(new_area));
        self.update_area_refs(*area, new_area);
        *area = new_area;
    }
}

#[derive(Debug)]
pub struct ManyInstances {
    pub instance_area: InstanceArea,
    pub aligned: Option<usize>,
    pub instances: Vec<f32>,
}

#[derive(Clone)]
pub struct AlignedInstance {
    pub inst: InstanceArea,
    pub index: usize,
}

pub type Redrawing = Result<(), ()>;

pub trait RedrawingApi {
    fn no() -> Redrawing {
        Result::Err(())
    }
    fn yes() -> Redrawing {
        Result::Ok(())
    }
    fn is_redrawing(&self) -> bool;
    fn is_not_redrawing(&self) -> bool;
    fn expect_redraw(&self);
}

impl RedrawingApi for Redrawing {
    fn is_redrawing(&self) -> bool {
        (*self).is_ok()
    }
    fn is_not_redrawing(&self) -> bool {
        (*self).is_err()
    }
    fn expect_redraw(&self) {
        if !self.is_redrawing() {
            panic!("assume_redraw_yes it should redraw")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        overlay_z_lift, DrawList2d, DrawListExt, OVERLAY_Z_BASE, OVERLAY_Z_MAX, OVERLAY_Z_STEP,
    };
    use crate::makepad_platform::Cx;
    use makepad_math::{dvec2, vec4f, Mat4f};

    /// The largest `draw_depth` any widget in this repo spends: the dock's
    /// dragged ghost tab. An overlay has to clear it.
    const WORST_CONTENT_DEPTH: f32 = 20.0;

    /// The near plane: `Mat4f::ortho(.., near = 100, far = -100, ..)` maps
    /// `world.z` to `0.5 - z/200`, so `z >= 100` is clipped away entirely.
    const NEAR_PLANE_Z: f32 = 100.0;

    fn translation(tx: f32, ty: f32) -> Mat4f {
        Mat4f {
            v: [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, tx, ty, 0.0, 1.0,
            ],
        }
    }

    #[test]
    fn self_only_transform_does_not_touch_children() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let parent = DrawList2d::new(&mut cx);
        let child = DrawList2d::new(&mut cx);

        cx.draw_lists[parent.id()].append_sub_list(cx.redraw_id, child.id());
        cx.draw_lists[child.id()].codeflow_parent_id = Some(parent.id());

        let child_mat = translation(3.0, 4.0);
        child.set_view_transform_self_only(&mut cx, &child_mat);

        let parent_mat = translation(10.0, 20.0);
        parent.set_view_transform_self_only(&mut cx, &parent_mat);

        assert_eq!(parent.get_view_transform(&cx).v, parent_mat.v);
        assert_eq!(child.get_view_transform(&cx).v, child_mat.v);
    }

    #[test]
    fn recursive_transform_still_updates_children() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let parent = DrawList2d::new(&mut cx);
        let child = DrawList2d::new(&mut cx);

        cx.draw_lists[parent.id()].append_sub_list(cx.redraw_id, child.id());
        cx.draw_lists[child.id()].codeflow_parent_id = Some(parent.id());

        let mat = translation(7.0, 9.0);
        parent.set_view_transform(&mut cx, &mat);

        assert_eq!(parent.get_view_transform(&cx).v, mat.v);
        assert_eq!(child.get_view_transform(&cx).v, mat.v);
    }

    #[test]
    fn point_mapping_round_trips_translation() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let draw_list = DrawList2d::new(&mut cx);
        draw_list.set_view_transform_self_only(&mut cx, &translation(10.0, 20.0));

        let world = draw_list.map_point_from_local(&cx, dvec2(5.0, 6.0));
        assert_eq!(world, dvec2(15.0, 26.0));

        let local = draw_list.map_point_to_local(&cx, world);
        assert_eq!(local, dvec2(5.0, 6.0));
    }

    #[test]
    fn debug_helpers_report_parent_children_and_transform() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let parent = DrawList2d::new(&mut cx);
        let child_a = DrawList2d::new(&mut cx);
        let child_b = DrawList2d::new(&mut cx);

        cx.draw_lists[parent.id()].append_sub_list(cx.redraw_id, child_a.id());
        cx.draw_lists[parent.id()].append_sub_list(cx.redraw_id, child_b.id());
        cx.draw_lists[child_a.id()].codeflow_parent_id = Some(parent.id());
        cx.draw_lists[child_b.id()].codeflow_parent_id = Some(parent.id());

        let mat = Mat4f {
            v: [
                2.0, 0.0, 0.0, 0.0, 0.0, 3.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 4.0, 5.0, 0.0, 1.0,
            ],
        };
        parent.set_view_transform_self_only(&mut cx, &mat);

        assert_eq!(parent.get_view_transform(&cx).v, mat.v);
        assert_eq!(child_a.debug_parent_draw_list_id(&cx), Some(parent.id()));
        assert_eq!(child_b.debug_parent_draw_list_id(&cx), Some(parent.id()));
        assert_eq!(
            parent.debug_child_draw_list_ids(&cx),
            vec![child_a.id(), child_b.id()]
        );

        let world = parent.map_point_from_local(&cx, dvec2(1.0, 1.0));
        let expected = mat.transform_vec4(vec4f(1.0, 1.0, 0.0, 1.0));
        assert_eq!(world, dvec2(expected.x as f64, expected.y as f64));
    }

    /// The step must clear any depth band an overlay's own content spends, and
    /// the ladder must stay clear of the near plane even fully extended and
    /// carrying the worst content depth on top of it.
    #[test]
    fn the_overlay_depth_ladder_has_headroom_at_both_ends() {
        assert!(
            OVERLAY_Z_BASE > WORST_CONTENT_DEPTH,
            "the first overlay level ({OVERLAY_Z_BASE}) does not clear the \
             deepest content in the repo ({WORST_CONTENT_DEPTH})",
        );
        // A very busy frame is a couple of thousand draw calls at 0.001 each.
        let paint_order_slack = 4.0;
        assert!(
            OVERLAY_Z_MAX + WORST_CONTENT_DEPTH + paint_order_slack < NEAR_PLANE_Z,
            "the ladder tops out at {OVERLAY_Z_MAX} and can reach the near \
             plane at {NEAR_PLANE_Z}",
        );
        assert!(OVERLAY_Z_STEP > 0.0 && OVERLAY_Z_STEP < OVERLAY_Z_BASE);
    }

    /// Nesting climbs, one level at a time, and then stops climbing.
    #[test]
    fn overlay_z_lift_climbs_with_nesting_and_is_capped() {
        assert_eq!(overlay_z_lift(1), OVERLAY_Z_BASE);
        assert_eq!(overlay_z_lift(2), OVERLAY_Z_BASE + OVERLAY_Z_STEP);
        assert_eq!(overlay_z_lift(3), OVERLAY_Z_BASE + 2.0 * OVERLAY_Z_STEP);
        for n in 1..64 {
            assert!(overlay_z_lift(n) <= overlay_z_lift(n + 1));
            assert!(overlay_z_lift(n) <= OVERLAY_Z_MAX);
        }
        assert_eq!(overlay_z_lift(1000), OVERLAY_Z_MAX);
        // A caller that somehow asks for level 0 still gets a real lift rather
        // than dropping back into the body's band.
        assert_eq!(overlay_z_lift(0), OVERLAY_Z_BASE);
    }

    /// The whole guarantee, in the arithmetic the backend walk actually does:
    /// `world.z = draw_depth + zbias`, with `zbias` a counter that
    /// `raise_zbias_to_floor` lifts on the way into each overlay.
    ///
    /// Body content spends real `draw_depth`; overlays must still land in
    /// front of it, nested overlays in front of their parents, and two
    /// overlays at the same level must still order by draw position.
    #[test]
    fn an_overlays_depth_beats_body_content_that_uses_draw_depth() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let body = DrawList2d::new(&mut cx);
        let overlay = DrawList2d::new(&mut cx);
        let sibling = DrawList2d::new(&mut cx);
        let nested = DrawList2d::new(&mut cx);

        cx.draw_lists[body.id()].overlay_z_lift = 0.0;
        cx.draw_lists[overlay.id()].overlay_z_lift = overlay_z_lift(1);
        cx.draw_lists[sibling.id()].overlay_z_lift = overlay_z_lift(1);
        cx.draw_lists[nested.id()].overlay_z_lift = overlay_z_lift(2);

        let step = 0.001f32; // CxDrawPass::default().zbias_step
        let mut zbias = 0.0f32;

        // The body draws first, deep in its own band.
        cx.draw_lists[body.id()].raise_zbias_to_floor(&mut zbias);
        let body_z = WORST_CONTENT_DEPTH + zbias;
        zbias += step;

        // Then the overlays, in draw order.
        cx.draw_lists[overlay.id()].raise_zbias_to_floor(&mut zbias);
        let overlay_z = zbias;
        zbias += step;

        cx.draw_lists[sibling.id()].raise_zbias_to_floor(&mut zbias);
        let sibling_z = zbias;
        zbias += step;

        cx.draw_lists[nested.id()].raise_zbias_to_floor(&mut zbias);
        let nested_z = zbias;

        assert!(
            overlay_z > body_z,
            "an overlay at {overlay_z} is behind body content at {body_z}",
        );
        assert!(
            sibling_z > overlay_z,
            "the later of two overlays at the same nesting level must be in \
             front: {sibling_z} vs {overlay_z}",
        );
        assert!(
            nested_z > sibling_z,
            "an overlay nested inside another must be in front of it: \
             {nested_z} vs {sibling_z}",
        );
        assert!(nested_z < NEAR_PLANE_Z);
    }

    /// The floor raises the counter and never lowers it — that monotonicity is
    /// what keeps paint order intact once an overlay has run.
    #[test]
    fn the_depth_floor_only_ever_raises_the_counter() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let overlay = DrawList2d::new(&mut cx);
        cx.draw_lists[overlay.id()].overlay_z_lift = overlay_z_lift(1);

        let mut low = 0.5f32;
        cx.draw_lists[overlay.id()].raise_zbias_to_floor(&mut low);
        assert_eq!(low, OVERLAY_Z_BASE);

        // Already above the floor (a deeper overlay ran first): left alone.
        let mut high = OVERLAY_Z_MAX + 1.0;
        cx.draw_lists[overlay.id()].raise_zbias_to_floor(&mut high);
        assert_eq!(high, OVERLAY_Z_MAX + 1.0);

        // An ordinary list never moves the counter at all.
        let plain = DrawList2d::new(&mut cx);
        let mut z = 7.0f32;
        cx.draw_lists[plain.id()].raise_zbias_to_floor(&mut z);
        assert_eq!(z, 7.0);
    }

    /// The lift is depth only: an overlay's `view_transform` is untouched, so
    /// hit testing and `map_point_*` are exactly what they were.
    #[test]
    fn the_depth_floor_does_not_touch_the_view_transform() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let overlay = DrawList2d::new(&mut cx);
        cx.draw_lists[overlay.id()].overlay_z_lift = overlay_z_lift(1);

        assert_eq!(
            overlay.get_view_transform(&cx).v,
            Mat4f::identity().v,
            "the overlay depth floor must not be spent on the view transform",
        );
        assert_eq!(overlay.map_point_from_local(&cx, dvec2(5.0, 6.0)), dvec2(5.0, 6.0));
        assert_eq!(overlay.map_point_to_local(&cx, dvec2(5.0, 6.0)), dvec2(5.0, 6.0));
    }
}
