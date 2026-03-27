use crate::{
    cx::Cx,
    //makepad_live_id::{
    //LiveId,
    //},
    draw_list::DrawListId,
    makepad_error_log::*,
    makepad_math::*,
};

#[derive(Clone, Hash, Ord, PartialOrd, Eq, Debug, PartialEq, Copy)]
pub struct InstanceArea {
    pub draw_list_id: DrawListId,
    pub draw_item_id: usize,
    pub instance_offset: usize,
    pub instance_count: usize,
    pub redraw_id: u64,
}
/*
#[derive(Clone, Hash, Ord, PartialOrd, Eq, Debug, PartialEq, Copy)]
pub struct DrawListArea {
    pub draw_list_id: DrawListId,
    pub redraw_id: u64
}*/

#[derive(Clone, Hash, Ord, PartialOrd, Eq, Debug, PartialEq, Copy)]
pub struct RectArea {
    pub draw_list_id: DrawListId,
    pub rect_id: usize,
    pub redraw_id: u64,
}

#[derive(Clone, Debug, Hash, PartialEq, Ord, PartialOrd, Eq, Copy)]
pub enum Area {
    Empty,
    Instance(InstanceArea),
    //DrawList(DrawListArea),
    Rect(RectArea),
}

impl Default for Area {
    fn default() -> Area {
        Area::Empty
    }
}

pub struct _DrawReadRef<'a> {
    pub repeat: usize,
    pub stride: usize,
    pub buffer: &'a [f32],
}

pub struct _DrawWriteRef<'a> {
    pub repeat: usize,
    pub stride: usize,
    pub buffer: &'a mut [f32],
}

impl Into<Area> for InstanceArea {
    fn into(self) -> Area {
        Area::Instance(self)
    }
}

impl Area {
    pub fn area(&self) -> Self {
        self.clone()
    }

    pub fn redraw(&self, cx: &mut Cx) {
        cx.redraw_area(*self);
    }

    pub fn valid_instance(&self, cx: &Cx) -> Option<&InstanceArea> {
        if self.is_valid(cx) {
            if let Self::Instance(inst) = self {
                return Some(inst);
            }
        }
        None
    }

    pub fn is_empty(&self) -> bool {
        if let Area::Empty = self {
            return true;
        }
        false
    }

    pub fn draw_list_id(&self) -> Option<DrawListId> {
        return match self {
            Area::Instance(inst) => Some(inst.draw_list_id),
            Area::Rect(list) => Some(list.draw_list_id),
            _ => None,
        };
    }

    pub fn redraw_id(&self) -> Option<u64> {
        return match self {
            Area::Instance(inst) => Some(inst.redraw_id),
            Area::Rect(list) => Some(list.redraw_id),
            _ => None,
        };
    }

    pub fn is_first_instance(&self) -> bool {
        return match self {
            Area::Instance(inst) => inst.instance_offset == 0,
            _ => false,
        };
    }

    /// Extends this area to include another area if they're in the same draw call.
    /// If self is stale (redraw_id doesn't match Cx), returns new_area.
    /// If self is current, extends to cover both ranges.
    pub fn extend_with(self, _cx: &Cx, new_area: Area) -> Area {
        // If self is empty, just use the new one
        if let Area::Empty = self {
            return new_area;
        }

        // Check if old area is stale by comparing against Cx's redraw_id
        if let Area::Instance(old_inst) = self {
            if let Area::Instance(new_inst) = new_area {
                if new_inst.redraw_id != old_inst.redraw_id
                    || old_inst.draw_list_id != new_inst.draw_list_id
                    || old_inst.draw_item_id != new_inst.draw_item_id
                {
                    return new_area;
                }

                // Extend: keep old offset, expand count to cover both ranges
                return Area::Instance(InstanceArea {
                    draw_list_id: old_inst.draw_list_id,
                    draw_item_id: old_inst.draw_item_id,
                    instance_offset: old_inst.instance_offset,
                    instance_count: old_inst.instance_count + new_inst.instance_count,
                    redraw_id: new_inst.redraw_id,
                });
            }
        }

        // Different draw calls, just use the new one
        new_area
    }

    pub fn is_valid(&self, cx: &Cx) -> bool {
        return match self {
            Area::Instance(inst) => {
                if inst.instance_count == 0 {
                    return false;
                }
                if let Some(draw_list) = cx.draw_lists.checked_index(inst.draw_list_id) {
                    if draw_list.redraw_id != inst.redraw_id {
                        return false;
                    }
                    return true;
                }
                return false;
            }
            Area::Rect(list) => {
                if let Some(draw_list) = cx.draw_lists.checked_index(list.draw_list_id) {
                    if draw_list.redraw_id != list.redraw_id {
                        return false;
                    }
                    return true;
                }
                return false;
            }
            _ => false,
        };
    }

    fn draw_list_transform(cx: &Cx, draw_list_id: DrawListId) -> Mat4f {
        cx.draw_lists[draw_list_id].draw_list_uniforms.view_transform
    }

    fn transform_point(mat: &Mat4f, point: Vec2d) -> Vec2d {
        let transformed = mat.transform_vec4(vec4f(point.x as f32, point.y as f32, 0.0, 1.0));
        if transformed.w.abs() > 1e-6 {
            dvec2(
                (transformed.x / transformed.w) as f64,
                (transformed.y / transformed.w) as f64,
            )
        } else {
            dvec2(transformed.x as f64, transformed.y as f64)
        }
    }

    fn transform_rect_aabb(mat: &Mat4f, rect: Rect) -> Rect {
        let corners = [
            rect.pos,
            dvec2(rect.pos.x + rect.size.x, rect.pos.y),
            dvec2(rect.pos.x, rect.pos.y + rect.size.y),
            dvec2(rect.pos.x + rect.size.x, rect.pos.y + rect.size.y),
        ];
        let mut min = dvec2(f64::INFINITY, f64::INFINITY);
        let mut max = dvec2(f64::NEG_INFINITY, f64::NEG_INFINITY);
        for corner in corners {
            let point = Self::transform_point(mat, corner);
            min.x = min.x.min(point.x);
            min.y = min.y.min(point.y);
            max.x = max.x.max(point.x);
            max.y = max.y.max(point.y);
        }
        Rect {
            pos: min,
            size: dvec2((max.x - min.x).max(0.0), (max.y - min.y).max(0.0)),
        }
    }

    fn draw_list_id_unchecked(&self) -> Option<DrawListId> {
        match self {
            Area::Instance(inst) => Some(inst.draw_list_id),
            Area::Rect(rect) => Some(rect.draw_list_id),
            Area::Empty => None,
        }
    }

    /// Returns the area rect after local item clip and local draw-list clip.
    ///
    /// This stays in draw-list local coordinates and matches the pre-transform clip model used by
    /// ordinary 2D shaders.
    pub fn local_clipped_rect(&self, cx: &Cx) -> Rect {
        return match self {
            Area::Instance(inst) => {
                if inst.instance_count == 0 {
                    error!("get_rect called on instance_count ==0 area pointer, use mark/sweep correctly!");
                    return Rect::default();
                }
                let draw_list = &cx.draw_lists[inst.draw_list_id];
                if draw_list.redraw_id != inst.redraw_id {
                    return Rect::default();
                }
                let draw_item = &draw_list.draw_items[inst.draw_item_id];
                let draw_call = draw_item.draw_call().unwrap();

                if draw_item.instances.as_ref().unwrap().len() == 0 {
                    error!("No instances but everything else valid?");
                    return Rect::default();
                }
                let sh = &cx.draw_shaders[draw_call.draw_shader_id.index];
                let buf = draw_item.instances.as_ref().unwrap();
                if let Some(rect_pos) = sh.mapping.rect_pos {
                    let pos = dvec2(
                        buf[inst.instance_offset + rect_pos] as f64,
                        buf[inst.instance_offset + rect_pos + 1] as f64,
                    );
                    if let Some(rect_size) = sh.mapping.rect_size {
                        let size = dvec2(
                            buf[inst.instance_offset + rect_size] as f64,
                            buf[inst.instance_offset + rect_size + 1] as f64,
                        );
                        let mut rect = Rect { pos, size };
                        if let Some(draw_clip) = sh.mapping.draw_clip {
                            let p1 = dvec2(
                                buf[inst.instance_offset + draw_clip] as f64,
                                buf[inst.instance_offset + draw_clip + 1] as f64,
                            );
                            let p2 = dvec2(
                                buf[inst.instance_offset + draw_clip + 2] as f64,
                                buf[inst.instance_offset + draw_clip + 3] as f64,
                            );
                            rect = rect.clip((p1, p2));
                        }
                        return rect;
                    }
                }
                Rect::default()
            }
            Area::Rect(ra) => {
                let draw_list = &cx.draw_lists[ra.draw_list_id];
                let rect_area = &draw_list.rect_areas[ra.rect_id];
                rect_area.rect.clip(rect_area.draw_clip)
            }
            _ => Rect::default(),
        };
    }

    /// Returns an axis-aligned approximation of the visible area after local clipping and
    /// `view_transform`.
    pub fn clipped_rect(&self, cx: &Cx) -> Rect {
        let Some(draw_list_id) = self.draw_list_id_unchecked() else {
            return Rect::default();
        };
        let local = self.local_clipped_rect(cx);
        Self::transform_rect_aabb(&Self::draw_list_transform(cx, draw_list_id), local)
    }

    /// Returns the stored item rect in draw-list local coordinates without clipping or
    /// draw-list-wide transforms.
    pub fn rect(&self, cx: &Cx) -> Rect {
        return match self {
            Area::Instance(inst) => {
                if inst.instance_count == 0 {
                    error!("get_rect called on instance_count ==0 area pointer, use mark/sweep correctly!");
                    return Rect::default();
                }
                let draw_list = &cx.draw_lists[inst.draw_list_id];
                if draw_list.redraw_id != inst.redraw_id {
                    return Rect::default();
                }
                let draw_item = &draw_list.draw_items[inst.draw_item_id];
                let draw_call = draw_item.draw_call().unwrap();

                if draw_item.instances.as_ref().unwrap().len() == 0 {
                    error!("No instances but everything else valid?");
                    return Rect::default();
                }
                let sh = &cx.draw_shaders[draw_call.draw_shader_id.index];
                // ok now we have to patch x/y/w/h into it
                let buf = draw_item.instances.as_ref().unwrap();
                if let Some(rect_pos) = sh.mapping.rect_pos {
                    let pos = dvec2(
                        buf[inst.instance_offset + rect_pos + 0] as f64,
                        buf[inst.instance_offset + rect_pos + 1] as f64,
                    );
                    if let Some(rect_size) = sh.mapping.rect_size {
                        let size = dvec2(
                            buf[inst.instance_offset + rect_size + 0] as f64,
                            buf[inst.instance_offset + rect_size + 1] as f64,
                        );
                        return Rect { pos, size };
                    }
                }
                Rect::default()
            }
            Area::Rect(ra) => {
                let draw_list = &cx.draw_lists[ra.draw_list_id];
                if draw_list.redraw_id == ra.redraw_id {
                    let rect_area = &draw_list.rect_areas[ra.rect_id];
                    return rect_area.rect;
                }
                Rect::default()
            }
            _ => Rect::default(),
        };
    }

    /// Maps an absolute point back into draw-list local coordinates through the inverse
    /// `view_transform`.
    pub fn abs_to_local(&self, cx: &Cx, abs: Vec2d) -> Vec2d {
        let Some(draw_list_id) = self.draw_list_id_unchecked() else {
            return abs;
        };
        let inverse = Self::draw_list_transform(cx, draw_list_id).invert();
        Self::transform_point(&inverse, abs)
    }

    /// Converts an absolute point to item-relative coordinates.
    pub fn abs_to_rel(&self, cx: &Cx, abs: Vec2d) -> Vec2d {
        let local = self.abs_to_local(cx, abs);
        let rect = self.rect(cx);
        Vec2d {
            x: local.x - rect.pos.x,
            y: local.y - rect.pos.y,
        }
    }

    pub fn set_rect(&self, cx: &mut Cx, rect: &Rect) {
        match self {
            Area::Instance(inst) => {
                if inst.instance_count == 0 {
                    error!("set_rect called on instance_count ==0 area pointer, use mark/sweep correctly!");
                    return;
                }
                let cxview = &mut cx.draw_lists[inst.draw_list_id];
                if cxview.redraw_id != inst.redraw_id {
                    //println!("set_rect called on invalid area pointer, use mark/sweep correctly!");
                    return;
                }
                let draw_item = &mut cxview.draw_items[inst.draw_item_id];
                //log!("{:?}", draw_item.kind.sub_list().is_some());
                let draw_call = draw_item.kind.draw_call().unwrap();
                let sh = &cx.draw_shaders[draw_call.draw_shader_id.index]; // ok now we have to patch x/y/w/h into it
                let buf = draw_item.instances.as_mut().unwrap();
                if let Some(rect_pos) = sh.mapping.rect_pos {
                    let x_index = inst.instance_offset + rect_pos;
                    let y_index = inst.instance_offset + rect_pos + 1;
                    if y_index >= buf.len() {
                        error!(
                            "set_rect rect_pos out of bounds: offset={} rect_pos={} len={}",
                            inst.instance_offset,
                            rect_pos,
                            buf.len()
                        );
                        return;
                    }
                    buf[x_index] = rect.pos.x as f32;
                    buf[y_index] = rect.pos.y as f32;
                }
                if let Some(rect_size) = sh.mapping.rect_size {
                    let w_index = inst.instance_offset + rect_size;
                    let h_index = inst.instance_offset + rect_size + 1;
                    if h_index >= buf.len() {
                        error!(
                            "set_rect rect_size out of bounds: offset={} rect_size={} len={}",
                            inst.instance_offset,
                            rect_size,
                            buf.len()
                        );
                        return;
                    }
                    buf[w_index] = rect.size.x as f32;
                    buf[h_index] = rect.size.y as f32;
                }
            }
            Area::Rect(ra) => {
                let draw_list = &mut cx.draw_lists[ra.draw_list_id];
                let rect_area = &mut draw_list.rect_areas[ra.rect_id];
                rect_area.rect = *rect
            }
            _ => (),
        }
    }
    /*
    pub fn get_read_ref<'a>(&self, cx: &'a Cx, id: LiveId, ty: ShaderTy) -> Option<DrawReadRef<'a >> {
        match self {
            Area::Instance(inst) => {
                let draw_list = &cx.draw_lists[inst.draw_list_id];
                let draw_item = &draw_list.draw_items[inst.draw_item_id];
                let draw_call = draw_item.draw_call().unwrap();
                if draw_list.redraw_id != inst.redraw_id {
                    error!("get_instance_read_ref called on invalid area pointer, use mark/sweep correctly!");
                    return None;
                }
                if cx.draw_shaders.generation != draw_call.draw_shader.draw_shader_generation {
                    return None;
                }
                let sh = &cx.draw_shaders[draw_call.draw_shader.draw_shader_id];
                if let Some(input) = sh.mapping.draw_call_uniforms.inputs.iter().find( | input | input.id == id) {
                    if input.ty != ty {
                        panic!("get_read_ref wrong uniform type, expected {:?} got: {:?}!", input.ty, ty);
                    }
                    return Some(
                        DrawReadRef {
                            repeat: 1,
                            stride: 0,
                            buffer: &draw_call.draw_call_uniforms[input.offset..]
                        }
                    )
                }
                if let Some(input) = sh.mapping.instances.inputs.iter().find( | input | input.id == id) {
                    if input.ty != ty {
                        panic!("get_read_ref wrong instance type, expected {:?} got: {:?}!", input.ty, ty);
                    }
                    if inst.instance_count == 0 {
                        return None
                    }
                    return Some(
                        DrawReadRef {
                            repeat: inst.instance_count,
                            stride: sh.mapping.instances.total_slots,
                            buffer: &draw_item.instances.as_ref().unwrap()[(inst.instance_offset + input.offset)..],
                        }
                    )
                }
                panic!("get_read_ref property not found! {}", id);
            }
            _ => (),
        }
        None
    }

    pub fn get_write_ref<'a>(&self, cx: &'a mut Cx, id: LiveId, ty: ShaderTy, name: &str) -> Option<DrawWriteRef<'a >> {
        match self {
            Area::Instance(inst) => {
                let draw_list = &mut cx.draw_lists[inst.draw_list_id];
                if draw_list.redraw_id != inst.redraw_id {
                    return None;
                }
                let draw_item = &mut draw_list.draw_items[inst.draw_item_id];
                let draw_call = draw_item.kind.draw_call_mut().unwrap();
                if cx.draw_shaders.generation != draw_call.draw_shader.draw_shader_generation {
                    return None;
                }
                let sh = &cx.draw_shaders[draw_call.draw_shader.draw_shader_id];

                if let Some(input) = sh.mapping.draw_call_uniforms.inputs.iter().find( | input | input.id == id) {
                    if input.ty != ty {
                        panic!("get_write_ref {} wrong uniform type, expected {:?} got: {:?}!", name, input.ty, ty);
                    }

                    cx.passes[draw_list.draw_pass_id.unwrap()].paint_dirty = true;
                    draw_call.uniforms_dirty = true;

                    return Some(
                        DrawWriteRef {
                            repeat: 1,
                            stride: 0,
                            buffer: &mut draw_call.draw_call_uniforms[input.offset..]
                        }
                    )
                }
                if let Some(input) = sh.mapping.instances.inputs.iter().find( | input | input.id == id) {
                    if input.ty != ty {
                        panic!("get_write_ref {} wrong instance type, expected {:?} got: {:?}!", name, input.ty, ty);
                    }

                    cx.passes[draw_list.draw_pass_id.unwrap()].paint_dirty = true;
                    draw_call.instance_dirty = true;
                    if inst.instance_count == 0 {
                        return None
                    }
                    return Some(
                        DrawWriteRef {
                            repeat: inst.instance_count,
                            stride: sh.mapping.instances.total_slots,
                            buffer: &mut draw_item.instances.as_mut().unwrap()[(inst.instance_offset + input.offset)..]
                        }
                    )
                }
                panic!("get_write_ref {} property not found!", name);
            }
            _ => (),
        }
        None
    }*/
}

#[cfg(test)]
mod tests {
    use super::{Area, RectArea};
    use crate::{cx::Cx, draw_list::CxRectArea, makepad_math::*};

    fn setup_rect_area(
        cx: &mut Cx,
        rect: Rect,
        draw_clip: (Vec2d, Vec2d),
        view_transform: Mat4f,
    ) -> Area {
        let draw_list = cx.draw_lists.alloc();
        let draw_list_id = draw_list.id();
        cx.draw_lists[draw_list_id].redraw_id = cx.redraw_id;
        cx.draw_lists[draw_list_id].draw_list_uniforms.view_transform = view_transform;
        cx.draw_lists[draw_list_id].rect_areas.push(CxRectArea { rect, draw_clip });
        Area::Rect(RectArea {
            draw_list_id,
            rect_id: 0,
            redraw_id: cx.redraw_id,
        })
    }

    #[test]
    fn rect_helpers_split_local_and_transformed_semantics() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let area = setup_rect_area(
            &mut cx,
            rect(0.0, 0.0, 50.0, 50.0),
            (dvec2(5.0, 5.0), dvec2(30.0, 25.0)),
            Mat4f::translation(vec3(10.0, 20.0, 0.0)),
        );

        assert_eq!(area.rect(&cx), rect(0.0, 0.0, 50.0, 50.0));
        assert_eq!(area.local_clipped_rect(&cx), rect(5.0, 5.0, 25.0, 20.0));
        assert_eq!(area.clipped_rect(&cx), rect(15.0, 25.0, 25.0, 20.0));
    }

    #[test]
    fn abs_to_local_and_rel_follow_view_transform() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let area = setup_rect_area(
            &mut cx,
            rect(4.0, 6.0, 20.0, 10.0),
            (dvec2(-1000.0, -1000.0), dvec2(1000.0, 1000.0)),
            Mat4f::translation(vec3(10.0, 20.0, 0.0)),
        );

        assert_eq!(area.abs_to_local(&cx, dvec2(17.0, 29.0)), dvec2(7.0, 9.0));
        assert_eq!(area.abs_to_rel(&cx, dvec2(17.0, 29.0)), dvec2(3.0, 3.0));
    }
}
