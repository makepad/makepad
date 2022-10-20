use {
    crate::{
        makepad_error_log::*,
        makepad_math::*,
        makepad_shader_compiler::{
            ShaderTy
        },
        makepad_live_compiler::{
            LiveId,
        },
        draw_list::DrawListId,
        makepad_math::{
            Rect
        },
        cx::Cx
    }
};

#[derive(Clone, Hash, Ord, PartialOrd, Eq, Debug, PartialEq, Copy)]
pub struct InstanceArea {
    pub draw_list_id: DrawListId,
    pub draw_item_id: usize,
    pub instance_offset: usize,
    pub instance_count: usize,
    pub redraw_id: u64
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
    pub redraw_id: u64
}

#[derive(Clone, Debug, Hash, PartialEq, Ord, PartialOrd, Eq, Copy)]
pub enum Area {
    Empty,
    Instance(InstanceArea),
    //DrawList(DrawListArea),
    Rect(RectArea)
}

impl Default for Area {
    fn default() -> Area {
        Area::Empty
    }
}

pub struct DrawReadRef<'a> {
    pub repeat: usize,
    pub stride: usize,
    pub buffer: &'a [f32]
}

pub struct DrawWriteRef<'a> {
    pub repeat: usize,
    pub stride: usize,
    pub buffer: &'a mut [f32]
}

impl Into<Area> for InstanceArea {
    fn into(self) -> Area {
        Area::Instance(self)
    }
}

impl Area {
    
    pub fn redraw(&self, cx: &mut Cx) {
        cx.redraw_area(*self);
    }
    
    
    pub fn valid_instance(&self, cx: &Cx) -> Option<&InstanceArea> {
        if self.is_valid(cx) {
            if let Self::Instance(inst) = self {
                return Some(inst)
            }
        }
        None
    }
    
    pub fn is_empty(&self) -> bool {
        if let Area::Empty = self {
            return true
        }
        false
    }
    
    pub fn draw_list_id(&self) -> Option<DrawListId> {
        return match self {
            Area::Instance(inst) => {
                Some(inst.draw_list_id)
            },
            Area::Rect(list) => {
                Some(list.draw_list_id)
            }
            _ => None
        }
    }
    
    pub fn is_first_instance(&self) -> bool {
        return match self {
            Area::Instance(inst) => {
                inst.instance_offset == 0
            },
            _ => false,
        }
    }
    
    pub fn is_valid(&self, cx: &Cx) -> bool {
        return match self {
            Area::Instance(inst) => {
                if inst.instance_count == 0 {
                    return false
                }
                let draw_list = &cx.draw_lists[inst.draw_list_id];
                if draw_list.redraw_id != inst.redraw_id {
                    return false
                }
                return true
            },
            Area::Rect(list) => {
                let draw_list = &cx.draw_lists[list.draw_list_id];
                if draw_list.redraw_id != list.redraw_id {
                    return false
                }
                return true
            },
            _ => false,
        }
    }

    // returns the final screen rect
    pub fn get_clipped_rect(&self, cx: &Cx) -> Rect {
        
        return match self {
            Area::Instance(inst) => {
                if inst.instance_count == 0 {
                    //panic!();
                    error!("get_rect called on instance_count ==0 area pointer, use mark/sweep correctly!");
                    return Rect::default()
                }
                let draw_list = &cx.draw_lists[inst.draw_list_id];
                if draw_list.redraw_id != inst.redraw_id {
                    return Rect::default();
                }
                let draw_item = &draw_list.draw_items[inst.draw_item_id];
                let draw_call = draw_item.draw_call().unwrap();
                
                if draw_item.instances.as_ref().unwrap().len() == 0 {
                    error!("No instances but everything else valid?");
                    return Rect::default()
                }
                if cx.draw_shaders.generation != draw_call.draw_shader.draw_shader_generation {
                    error!("Generation invalid get_rect {} {:?} {} {}", draw_list.debug_id, inst, cx.draw_shaders.generation, draw_call.draw_shader.draw_shader_generation);
                    return Rect::default()
                }
                let sh = &cx.draw_shaders[draw_call.draw_shader.draw_shader_id];
                // ok now we have to patch x/y/w/h into it
                let buf = draw_item.instances.as_ref().unwrap();
                if let Some(rect_pos) = sh.mapping.rect_pos {
                    let pos = dvec2(buf[inst.instance_offset + rect_pos + 0] as f64, buf[inst.instance_offset + rect_pos + 1] as f64);
                    if let Some(rect_size) = sh.mapping.rect_size {
                        let size = dvec2(buf[inst.instance_offset + rect_size + 0] as f64, buf[inst.instance_offset + rect_size + 1] as f64);
                        if let Some(draw_clip) = sh.mapping.draw_clip {
                            let p1= dvec2(
                                buf[inst.instance_offset + draw_clip + 0] as f64,
                                buf[inst.instance_offset + draw_clip + 1] as f64,
                            );
                            let p2 = dvec2(
                                buf[inst.instance_offset + draw_clip + 2] as f64,
                                buf[inst.instance_offset + draw_clip + 3] as f64
                            );
                            return Rect{pos,size}.clip((p1,p2));
                        }
                    }
                }
                Rect::default()
            },
            Area::Rect(ra) => {
                // we need to clip this drawlist too
                let draw_list = &cx.draw_lists[ra.draw_list_id];
                let rect_area = &draw_list.rect_areas[ra.rect_id];
                return rect_area.rect.clip(rect_area.draw_clip);                
            },
            _ => Rect::default(),
        }
    }
    
    pub fn get_rect(&self, cx: &Cx) -> Rect {
        
        return match self {
            Area::Instance(inst) => {
                if inst.instance_count == 0 {
                    //panic!();
                    error!("get_rect called on instance_count ==0 area pointer, use mark/sweep correctly!");
                    return Rect::default()
                }
                let draw_list = &cx.draw_lists[inst.draw_list_id];
                if draw_list.redraw_id != inst.redraw_id {
                    return Rect::default();
                }
                let draw_item = &draw_list.draw_items[inst.draw_item_id];
                let draw_call = draw_item.draw_call().unwrap();
                
                if draw_item.instances.as_ref().unwrap().len() == 0 {
                    error!("No instances but everything else valid?");
                    return Rect::default()
                }
                if cx.draw_shaders.generation != draw_call.draw_shader.draw_shader_generation {
                    error!("Generation invalid get_rect {} {:?} {} {}", draw_list.debug_id, inst, cx.draw_shaders.generation, draw_call.draw_shader.draw_shader_generation);
                    return Rect::default()
                }
                let sh = &cx.draw_shaders[draw_call.draw_shader.draw_shader_id];
                // ok now we have to patch x/y/w/h into it
                let buf = draw_item.instances.as_ref().unwrap();
                if let Some(rect_pos) = sh.mapping.rect_pos {
                    let pos = dvec2(buf[inst.instance_offset + rect_pos + 0] as f64, buf[inst.instance_offset + rect_pos + 1] as f64);
                    if let Some(rect_size) = sh.mapping.rect_size {
                        let size = dvec2(buf[inst.instance_offset + rect_size + 0] as f64, buf[inst.instance_offset + rect_size + 1] as f64);
                        return Rect{pos,size};
                    }
                }
                Rect::default()
            },
            Area::Rect(ra) => {
                let draw_list = &cx.draw_lists[ra.draw_list_id];
                let rect_area = &draw_list.rect_areas[ra.rect_id];
                return rect_area.rect;                
            },
            _ => Rect::default(),
        }
    }
    
    pub fn abs_to_rel(&self, cx: &Cx, abs: DVec2) -> DVec2 {
        return match self {
            Area::Instance(inst) => {
                if inst.instance_count == 0 {
                    error!("abs_to_rel_scroll called on instance_count ==0 area pointer, use mark/sweep correctly!");
                    return abs
                }
                let draw_list = &cx.draw_lists[inst.draw_list_id];
                if draw_list.redraw_id != inst.redraw_id {
                    return abs;
                }
                let draw_item = &draw_list.draw_items[inst.draw_item_id];
                let draw_call = draw_item.draw_call().unwrap();
                if cx.draw_shaders.generation != draw_call.draw_shader.draw_shader_generation {
                    error!("Generation invalid abs_to_rel {} {:?} {} {}", draw_list.debug_id, inst, cx.draw_shaders.generation, draw_call.draw_shader.draw_shader_generation);
                    return abs;
                }
                
                let sh = &cx.draw_shaders[draw_call.draw_shader.draw_shader_id];
                // ok now we have to patch x/y/w/h into it
                if let Some(rect_pos) = sh.mapping.rect_pos {
                    let buf = draw_item.instances.as_ref().unwrap();
                    let x = buf[inst.instance_offset + rect_pos + 0] as f64; 
                    let y = buf[inst.instance_offset + rect_pos + 1] as f64;
                    return DVec2 {
                        x: abs.x - x,
                        y: abs.y - y
                    }
                }
                abs
            },
            Area::Rect(ra) => {
                let draw_list = &cx.draw_lists[ra.draw_list_id];
                let rect_area = &draw_list.rect_areas[ra.rect_id];
                DVec2 {
                    x: abs.x - rect_area.rect.pos.x,
                    y: abs.y - rect_area.rect.pos.y
                }
            },
            _ => abs,
        }
    }
    
    pub fn set_rect(&self, cx: &mut Cx, rect: &Rect) {
        match self {
            Area::Instance(inst) => {
                let cxview = &mut cx.draw_lists[inst.draw_list_id];
                if cxview.redraw_id != inst.redraw_id {
                    //println!("set_rect called on invalid area pointer, use mark/sweep correctly!");
                    return;
                }
                let draw_item = &mut cxview.draw_items[inst.draw_item_id];
                //log!("{:?}", draw_item.kind.sub_list().is_some());
                let draw_call = draw_item.kind.draw_call().unwrap();
                if cx.draw_shaders.generation != draw_call.draw_shader.draw_shader_generation {
                    return;
                }
                let sh = &cx.draw_shaders[draw_call.draw_shader.draw_shader_id]; // ok now we have to patch x/y/w/h into it
                let buf = draw_item.instances.as_mut().unwrap();
                if let Some(rect_pos) = sh.mapping.rect_pos {
                    buf[inst.instance_offset + rect_pos + 0] = rect.pos.x as f32;
                    buf[inst.instance_offset + rect_pos + 1] = rect.pos.y as f32;
                }
                if let Some(rect_size) = sh.mapping.rect_size {
                    buf[inst.instance_offset + rect_size + 0] = rect.size.x as f32;
                    buf[inst.instance_offset + rect_size + 1] = rect.size.y as f32;
                }
            },
            Area::Rect(ra) => {
                let draw_list = &mut cx.draw_lists[ra.draw_list_id];
                let rect_area = &mut draw_list.rect_areas[ra.rect_id];
                rect_area.rect = *rect
            },
            _ => ()
        }
    }
    
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
                if let Some(input) = sh.mapping.user_uniforms.inputs.iter().find( | input | input.id == id) {
                    if input.ty != ty {
                        panic!("get_read_ref wrong uniform type, expected {:?} got: {:?}!", input.ty, ty);
                    }
                    return Some(
                        DrawReadRef {
                            repeat: 1,
                            stride: 0,
                            buffer: &draw_call.user_uniforms[input.offset..]
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
                
                if let Some(input) = sh.mapping.user_uniforms.inputs.iter().find( | input | input.id == id) {
                    if input.ty != ty {
                        panic!("get_write_ref {} wrong uniform type, expected {:?} got: {:?}!", name, input.ty, ty);
                    }
                    
                    cx.passes[draw_list.pass_id.unwrap()].paint_dirty = true;
                    draw_call.uniforms_dirty = true;
                    
                    return Some(
                        DrawWriteRef {
                            repeat: 1,
                            stride: 0,
                            buffer: &mut draw_call.user_uniforms[input.offset..]
                        }
                    )
                }
                if let Some(input) = sh.mapping.instances.inputs.iter().find( | input | input.id == id) {
                    if input.ty != ty {
                        panic!("get_write_ref {} wrong instance type, expected {:?} got: {:?}!", name, input.ty, ty);
                    }
                    
                    cx.passes[draw_list.pass_id.unwrap()].paint_dirty = true;
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
    }
}

