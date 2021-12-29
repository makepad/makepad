pub use {
    std::{
        rc::Rc,
        cell::RefCell
    },
    makepad_shader_compiler::{
        ShaderTy
    },
    makepad_shader_compiler::makepad_live_compiler::{
        LiveId,
    },
    makepad_shader_compiler::makepad_math::{
        Vec2,
        Rect
    },
    crate::{
        cx::Cx
    }
};

#[derive(Clone, Default, Hash, Ord, PartialOrd, Eq, Debug, PartialEq, Copy)]
pub struct InstanceArea {
    pub view_id: usize,
    pub draw_item_id: usize,
    pub instance_offset: usize,
    pub instance_count: usize,
    pub redraw_id: u64
}

#[derive(Clone, Default, Hash, Ord, PartialOrd, Eq, Debug, PartialEq, Copy)]
pub struct ViewArea {
    pub view_id: usize,
    pub redraw_id: u64
}

#[derive(Clone, Debug, Hash, PartialEq, Ord, PartialOrd, Eq, Copy)]
pub enum Area {
    Empty,
    Instance(InstanceArea),
    View(ViewArea)
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
    
    pub fn view_id(&self) -> Option<usize> {
        return match self {
            Area::Instance(inst) => {
                Some(inst.view_id)
            },
            Area::View(view) => {
                Some(view.view_id)
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
                let cxview = &cx.views[inst.view_id];
                if cxview.redraw_id != inst.redraw_id {
                    return false
                }
                return true
            },
            Area::View(view_area) => {
                let cxview = &cx.views[view_area.view_id];
                if cxview.redraw_id != view_area.redraw_id {
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
                let cxview = &cx.views[inst.view_id];
                if cxview.redraw_id != inst.redraw_id {
                    Vec2::default()
                }
                else {
                    cxview.unsnapped_scroll
                }
            },
            Area::View(view_area) => {
                let cxview = &cx.views[view_area.view_id];
                cxview.unsnapped_scroll
            },
            _ => Vec2::default(),
        }
    }
    
    pub fn get_scroll_pos(&self, cx: &Cx) -> Vec2 {
        return match self {
            Area::Instance(inst) => {
                let cxview = &cx.views[inst.view_id];
                if cxview.redraw_id != inst.redraw_id {
                    Vec2::default()
                }
                else {
                    let draw_call = &cxview.draw_items[inst.draw_item_id].draw_call.as_ref().unwrap();
                    Vec2 {
                        x: draw_call.draw_uniforms.draw_scroll.x,
                        y: draw_call.draw_uniforms.draw_scroll.y
                    }
                }
            },
            Area::View(view_area) => {
                let cxview = &cx.views[view_area.view_id];
                cxview.parent_scroll
            },
            _ => Vec2::default(),
        }
    }
    
    pub fn set_no_scroll(&self, cx: &mut Cx, hor: bool, ver: bool) {
        return match self {
            Area::Instance(inst) => {
                let cxview = &mut cx.views[inst.view_id];
                if cxview.redraw_id != inst.redraw_id {
                    return
                }
                else {
                    let draw_call = cxview.draw_items[inst.draw_item_id].draw_call.as_mut().unwrap();
                    draw_call.no_h_scroll = hor;
                    draw_call.no_v_scroll = ver;
                }
            },
            Area::View(view_area) => {
                let cxview = &mut cx.views[view_area.view_id];
                cxview.no_h_scroll = hor;
                cxview.no_v_scroll = ver;
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
                let cxview = &cx.views[inst.view_id];
                if cxview.redraw_id != inst.redraw_id {
                    return Rect::default();
                }
                let draw_call = &cxview.draw_items[inst.draw_item_id].draw_call.as_ref().unwrap();
                
                if draw_call.instances.as_ref().unwrap().len() == 0 {
                    println!("No instances but everything else valid?");
                    return Rect::default()
                }
                if cx.draw_shader_generation != draw_call.draw_shader.draw_shader_generation {
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
                        return draw_call.clip_and_scroll_rect(x, y, w, h);
                    }
                }
                Rect::default()
            },
            Area::View(view_area) => {
                let cxview = &cx.views[view_area.view_id];
                Rect {
                    pos: cxview.rect.pos - cxview.parent_scroll,
                    size: cxview.rect.size
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
                let cxview = &cx.views[inst.view_id];
                if cxview.redraw_id != inst.redraw_id {
                    return abs;
                }
                let draw_call = &cxview.draw_items[inst.draw_item_id].draw_call.as_ref().unwrap();
                if cx.draw_shader_generation != draw_call.draw_shader.draw_shader_generation {
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
            Area::View(view_area) => {
                let cxview = &cx.views[view_area.view_id];
                Vec2 {
                    x: abs.x - cxview.rect.pos.x + cxview.parent_scroll.x + cxview.unsnapped_scroll.x,
                    y: abs.y - cxview.rect.pos.y - cxview.parent_scroll.y + cxview.unsnapped_scroll.y
                }
            },
            _ => abs,
        }
    }
    
    pub fn set_rect(&self, cx: &mut Cx, rect: &Rect) {
        match self {
            Area::Instance(inst) => {
                let cxview = &mut cx.views[inst.view_id];
                if cxview.redraw_id != inst.redraw_id {
                    //println!("set_rect called on invalid area pointer, use mark/sweep correctly!");
                    return;
                }
                let draw_call = cxview.draw_items[inst.draw_item_id].draw_call.as_mut().unwrap();
                if cx.draw_shader_generation != draw_call.draw_shader.draw_shader_generation {
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
            Area::View(view_area) => {
                let cxview = &mut cx.views[view_area.view_id];
                cxview.rect = rect.clone()
            },
            _ => ()
        }
    }
    
    pub fn get_read_ref<'a>(&self, cx: &'a Cx, id: LiveId, ty: ShaderTy) -> Option<DrawReadRef<'a >> {
        match self {
            Area::Instance(inst) => {
                let cxview = &cx.views[inst.view_id];
                let draw_call = &cxview.draw_items[inst.draw_item_id].draw_call.as_ref().unwrap();
                if cxview.redraw_id != inst.redraw_id {
                    println!("get_instance_read_ref called on invalid area pointer, use mark/sweep correctly!");
                    return None;
                }
                if cx.draw_shader_generation != draw_call.draw_shader.draw_shader_generation {
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
                let cxview = &mut cx.views[inst.view_id];
                if cxview.redraw_id != inst.redraw_id {
                    return None;
                }
                let draw_call = cxview.draw_items[inst.draw_item_id].draw_call.as_mut().unwrap();
                if cx.draw_shader_generation != draw_call.draw_shader.draw_shader_generation {
                    return None;
                }
                let sh = &cx.draw_shaders[draw_call.draw_shader.draw_shader_id];
                
                if let Some(input) = sh.mapping.user_uniforms.inputs.iter().find( | input | input.id == id) {
                    if input.ty != ty {
                        panic!("get_write_ref {} wrong uniform type, expected {:?} got: {:?}!", name, input.ty, ty);
                    }
                    
                    cx.passes[cxview.pass_id].paint_dirty = true;
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
                    
                    cx.passes[cxview.pass_id].paint_dirty = true;
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

