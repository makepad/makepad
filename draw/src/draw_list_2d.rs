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
            cx.draw_lists[draw_list_id]
                .draw_list_uniforms
                .view_transform = *mat;
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
                    set_view_transform_recur(sub_list_id, cx, mat);
                }
            }
        }
        set_view_transform_recur(self.id(), cx, mat);
    }

    fn set_view_transform_self_only(&self, cx: &mut Cx, mat: &Mat4f) {
        cx.draw_lists[self.id()].draw_list_uniforms.view_transform = *mat;
    }

    fn begin_always(&mut self, cx: &mut CxDraw) {
        self.begin_maybe(cx, true).expect_redraw();
    }

    fn begin_maybe(&mut self, cx: &mut CxDraw, will_redraw: bool) -> Redrawing {
        // check if we have a pass id parent
        let pass_id = cx.pass_stack.last().unwrap().pass_id;
        let redraw_id = cx.cx.redraw_id;

        cx.draw_lists[self.id()].draw_pass_id = Some(pass_id);

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

        cx.cx.draw_lists[self.id()].clear_draw_items(redraw_id);

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

impl DrawList2d {
    pub fn new(cx: &mut Cx) -> Self {
        let draw_list = cx.draw_lists.alloc();
        Self {
            dirty_check_rect: Default::default(),
            draw_list,
        }
    }

    pub fn begin_overlay_last(&mut self, cx: &mut Cx2d) {
        self.begin_overlay_inner(cx, true)
    }

    pub fn begin_overlay_reuse(&mut self, cx: &mut Cx2d) {
        self.begin_overlay_inner(cx, false)
    }

    pub fn begin_overlay_inner(&mut self, cx: &mut Cx2d, always_last: bool) {
        let pass_id = cx.pass_stack.last().unwrap().pass_id;
        let redraw_id = cx.cx.redraw_id;

        cx.draw_lists[self.draw_list.id()].draw_pass_id = Some(pass_id);

        let codeflow_parent_id = cx.draw_list_stack.last().cloned().unwrap();

        let overlay_id = cx.overlay_id.unwrap();
        if always_last {
            cx.draw_lists[overlay_id].store_sub_list_last(redraw_id, self.draw_list.id());
        } else {
            cx.draw_lists[overlay_id].store_sub_list(redraw_id, self.draw_list.id());
        }

        cx.nav_list_item_push(codeflow_parent_id, NavItem::Child(self.draw_list.id()));

        cx.cx.draw_lists[self.draw_list.id()].codeflow_parent_id = Some(codeflow_parent_id);
        if cx.passes[pass_id].main_draw_list_id.unwrap() == self.draw_list.id() {
            cx.passes[pass_id].paint_dirty = true;
        }

        cx.cx.draw_lists[self.draw_list.id()].clear_draw_items(redraw_id);

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

        let sh = &self.cx.draw_shaders[draw_shader.index];

        let current_draw_list_id = *self.draw_list_stack.last().unwrap();
        let draw_list = &mut self.cx.draw_lists[current_draw_list_id];

        if append && !sh.mapping.flags.draw_call_always {
            if let Some(index) = draw_list.find_appendable_drawcall(sh, draw_vars) {
                return Some(&mut draw_list.draw_items[index]);
            }
        }

        Some(draw_list.append_draw_call(self.cx.redraw_id, sh, draw_vars))
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
    use super::{DrawList2d, DrawListExt};
    use crate::makepad_platform::Cx;
    use makepad_math::{dvec2, vec4f, Mat4f};

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
}
