pub use {
    std::{
        rc::Rc,
        cell::RefCell
    },
    crate::{
        makepad_shader_compiler::{
            ShaderTy
        },
        makepad_live_compiler::{
            LiveId,
        },
        makepad_math::{
            Vec2,
            Rect
        },
        cx::Cx
    }
};

#[derive(Clone, Default, Hash, Ord, PartialOrd, Eq, Debug, PartialEq, Copy)]
pub struct InstanceArea {
    pub draw_list_id: usize,
    pub draw_item_id: usize,
    pub instance_offset: usize,
    pub instance_count: usize,
    pub redraw_id: u64
}

#[derive(Clone, Default, Hash, Ord, PartialOrd, Eq, Debug, PartialEq, Copy)]
pub struct DrawListArea {
    pub draw_list_id: usize,
    pub redraw_id: u64
}

#[derive(Clone, Debug, Hash, PartialEq, Ord, PartialOrd, Eq, Copy)]
pub enum Area {
    Empty,
    Instance(InstanceArea),
    DrawList(DrawListArea)
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
    
    pub fn draw_list_id(&self) -> Option<usize> {
        return match self {
            Area::Instance(inst) => {
                Some(inst.draw_list_id)
            },
            Area::DrawList(list) => {
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
            Area::DrawList(list) => {
                let draw_list = &cx.draw_lists[list.draw_list_id];
                if draw_list.redraw_id != list.redraw_id {
                    return false
                }
                return true
            },
            _ => false,
        }
    }
    
    pub fn get_local_scroll_pos(&self, cx: &Cx) -> Vec2 {
        return match self {
            Area::Instance(inst) => {
                let draw_list = &cx.draw_lists[inst.draw_list_id];
                if draw_list.redraw_id != inst.redraw_id {
                    Vec2::default()
                }
                else {
                    draw_list.unsnapped_scroll
                }
            },
            Area::DrawList(list) => {
                let draw_list = &cx.draw_lists[list.draw_list_id];
                draw_list.unsnapped_scroll
            },
            _ => Vec2::default(),
        }
    }
    
    pub fn get_scroll_pos(&self, cx: &Cx) -> Vec2 {
        return match self {
            Area::Instance(inst) => {
                let draw_list = &cx.draw_lists[inst.draw_list_id];
                if draw_list.redraw_id != inst.redraw_id {
                    Vec2::default()
                }
                else {
                    let draw_call = &draw_list.draw_items[inst.draw_item_id].draw_call.as_ref().unwrap();
                    Vec2 {
                        x: draw_call.draw_uniforms.draw_scroll.x,
                        y: draw_call.draw_uniforms.draw_scroll.y
                    }
                }
            },
            Area::DrawList(list) => {
                let draw_list = &cx.draw_lists[list.draw_list_id];
                draw_list.parent_scroll
            },
            _ => Vec2::default(),
        }
    }
    
    pub fn set_no_scroll(&self, cx: &mut Cx, hor: bool, ver: bool) {
        return match self {
            Area::Instance(inst) => {
                let draw_list = &mut cx.draw_lists[inst.draw_list_id];
                if draw_list.redraw_id != inst.redraw_id {
                    return
                }
                else {
                    let draw_call = draw_list.draw_items[inst.draw_item_id].draw_call.as_mut().unwrap();
                    draw_call.options.no_h_scroll = hor;
                    draw_call.options.no_v_scroll = ver;
                }
            },
            Area::DrawList(list) => {
                let draw_list = &mut cx.draw_lists[list.draw_list_id];
                draw_list.no_h_scroll = hor;
                draw_list.no_v_scroll = ver;
            },
            _ => (),
        }
    }
    
    // returns the final screen rect
    pub fn get_rect(&self, cx: &Cx) -> Rect {
        
        return match self {
            Area::Instance(inst) => {
                if inst.instance_count == 0 {
                    //panic!();
                    println!("get_rect called on instance_count ==0 area pointer, use mark/sweep correctly!");
                    return Rect::default()
                }
                let draw_list = &cx.draw_lists[inst.draw_list_id];
                if draw_list.redraw_id != inst.redraw_id {
                    return Rect::default();
                }
                let draw_call = &draw_list.draw_items[inst.draw_item_id].draw_call.as_ref().unwrap();
                
                if draw_call.instances.as_ref().unwrap().len() == 0 {
                    println!("No instances but everything else valid?");
                    return Rect::default()
                }
                if cx.draw_shaders.generation != draw_call.draw_shader.draw_shader_generation {
                    println!("Generation invalid get_rect {} {:?} {} {}",draw_list.debug_id, inst, cx.draw_shaders.generation, draw_call.draw_shader.draw_shader_generation);
                    return Rect::default()
                }
                let sh = &cx.draw_shaders[draw_call.draw_shader.draw_shader_id];
                // ok now we have to patch x/y/w/h into it
                let buf = draw_call.instances.as_ref().unwrap();
                if let Some(rect_pos) = sh.mapping.rect_pos {
                    let x = buf[inst.instance_offset + rect_pos + 0];
                    let y = buf[inst.instance_offset + rect_pos + 1];
                    if let Some(rect_size) = sh.mapping.rect_size {
                        let w = buf[inst.instance_offset + rect_size + 0];
                        let h = buf[inst.instance_offset + rect_size + 1];
                        return draw_call.draw_uniforms.clip_and_scroll_rect(x, y, w, h);
                    }
                }
                Rect::default()
            },
            Area::DrawList(list) => {
                let draw_list = &cx.draw_lists[list.draw_list_id];
                Rect {
                    pos: draw_list.rect.pos - draw_list.parent_scroll,
                    size: draw_list.rect.size
                }
            },
            _ => Rect::default(),
        }
    }
    
    pub fn abs_to_rel(&self, cx: &Cx, abs: Vec2) -> Vec2 {
        return match self {
            Area::Instance(inst) => {
                if inst.instance_count == 0 {
                    println!("abs_to_rel_scroll called on instance_count ==0 area pointer, use mark/sweep correctly!");
                    return abs
                }
                let draw_list = &cx.draw_lists[inst.draw_list_id];
                if draw_list.redraw_id != inst.redraw_id {
                    return abs;
                }
                let draw_call = &draw_list.draw_items[inst.draw_item_id].draw_call.as_ref().unwrap();
                if cx.draw_shaders.generation != draw_call.draw_shader.draw_shader_generation {
                    println!("Generation invalid abs_to_rel {} {:?} {} {}",draw_list.debug_id,inst, cx.draw_shaders.generation, draw_call.draw_shader.draw_shader_generation);
                    return abs;
                }
                
                let sh = &cx.draw_shaders[draw_call.draw_shader.draw_shader_id];
                // ok now we have to patch x/y/w/h into it
                if let Some(rect_pos) = sh.mapping.rect_pos {
                    let buf = draw_call.instances.as_ref().unwrap();
                    let x = buf[inst.instance_offset + rect_pos + 0];
                    let y = buf[inst.instance_offset + rect_pos + 1];
                    return Vec2 {
                        x: abs.x - x + draw_call.draw_uniforms.draw_scroll.x,
                        y: abs.y - y + draw_call.draw_uniforms.draw_scroll.y
                    }
                }
                abs
            },
            Area::DrawList(list) => {
                let draw_list = &cx.draw_lists[list.draw_list_id];
                Vec2 {
                    x: abs.x - draw_list.rect.pos.x + draw_list.parent_scroll.x + draw_list.unsnapped_scroll.x,
                    y: abs.y - draw_list.rect.pos.y - draw_list.parent_scroll.y + draw_list.unsnapped_scroll.y
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
                let draw_call = cxview.draw_items[inst.draw_item_id].draw_call.as_mut().unwrap();
                if cx.draw_shaders.generation != draw_call.draw_shader.draw_shader_generation {
                    return;
                }
                let sh = &cx.draw_shaders[draw_call.draw_shader.draw_shader_id]; // ok now we have to patch x/y/w/h into it
                let buf = draw_call.instances.as_mut().unwrap();
                if let Some(rect_pos) = sh.mapping.rect_pos {
                    buf[inst.instance_offset + rect_pos + 0] = rect.pos.x;
                    buf[inst.instance_offset + rect_pos + 1] = rect.pos.y;
                }
                if let Some(rect_size) = sh.mapping.rect_size {
                    buf[inst.instance_offset + rect_size + 0] = rect.size.x;
                    buf[inst.instance_offset + rect_size + 1] = rect.size.y;
                }
            },
            Area::DrawList(list) => {
                let draw_list = &mut cx.draw_lists[list.draw_list_id];
                draw_list.rect = rect.clone()
            },
            _ => ()
        }
    }
    
    pub fn get_read_ref<'a>(&self, cx: &'a Cx, id: LiveId, ty: ShaderTy) -> Option<DrawReadRef<'a >> {
        match self {
            Area::Instance(inst) => {
                let draw_list = &cx.draw_lists[inst.draw_list_id];
                let draw_call = &draw_list.draw_items[inst.draw_item_id].draw_call.as_ref().unwrap();
                if draw_list.redraw_id != inst.redraw_id {
                    println!("get_instance_read_ref called on invalid area pointer, use mark/sweep correctly!");
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
                            buffer: &draw_call.instances.as_ref().unwrap()[(inst.instance_offset + input.offset)..],
                        }
                    )
                }
                panic!("get_read_ref property not found!");
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
                let draw_call = draw_list.draw_items[inst.draw_item_id].draw_call.as_mut().unwrap();
                if cx.draw_shaders.generation != draw_call.draw_shader.draw_shader_generation {
                    return None;
                }
                let sh = &cx.draw_shaders[draw_call.draw_shader.draw_shader_id];
                
                if let Some(input) = sh.mapping.user_uniforms.inputs.iter().find( | input | input.id == id) {
                    if input.ty != ty {
                        panic!("get_write_ref {} wrong uniform type, expected {:?} got: {:?}!", name, input.ty, ty);
                    }
                    
                    cx.passes[draw_list.pass_id].paint_dirty = true;
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
                    
                    cx.passes[draw_list.pass_id].paint_dirty = true;
                    draw_call.instance_dirty = true;
                    if inst.instance_count == 0 {
                        return None
                    }
                    return Some(
                        DrawWriteRef {
                            repeat: inst.instance_count,
                            stride: sh.mapping.instances.total_slots,
                            buffer: &mut draw_call.instances.as_mut().unwrap()[(inst.instance_offset + input.offset)..]
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

