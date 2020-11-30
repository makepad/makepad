use crate::cx::*; 
use makepad_live_compiler::ty::Ty;

#[derive(Clone, Default, Hash, Ord, PartialOrd, Eq,Debug, PartialEq, Copy)]
pub struct InstanceArea{
    pub view_id:usize,
    pub draw_call_id:usize,
    pub instance_offset:usize,
    pub instance_count:usize,
    pub redraw_id:u64
}

#[derive(Clone, Default, Hash, Ord, PartialOrd, Eq,Debug, PartialEq, Copy)]
pub struct ViewArea{
    pub view_id:usize,
    pub redraw_id:u64 
}

#[derive(Clone, Debug, Hash, PartialEq, Ord, PartialOrd, Eq, Copy)]
pub enum Area{
    Empty,
    All,
    Instance(InstanceArea),
    View(ViewArea)
}

impl Default for Area{
    fn default()->Area{
        Area::Empty
    } 
}  

pub struct DrawReadRef<'a>{
    pub repeat: usize,
    pub stride: usize,
    pub buffer:&'a [f32]
}

pub struct DrawWriteRef<'a>{
    pub repeat: usize,
    pub stride: usize,
    pub buffer:&'a mut [f32]
}

impl Area{
    pub fn is_empty(&self)->bool{
        if let Area::Empty = self{
            return true
        }
        false
    }
    
    pub fn is_first_instance(&self)->bool{
        return match self{
            Area::Instance(inst)=>{
                inst.instance_offset == 0
            },
            _=>false,
        }
    }

    pub fn is_valid(&self, cx:&Cx)->bool{
        return match self{
            Area::Instance(inst)=>{
                if inst.instance_count == 0{
                    return false
                }
                let cxview = &cx.views[inst.view_id];
                if cxview.redraw_id != inst.redraw_id {
                    return false
                }
                return true
            },
            Area::View(view_area)=>{
                let cxview = &cx.views[view_area.view_id];
                if cxview.redraw_id != view_area.redraw_id {
                    return false
                }
                return true
            },
            _=>false,
        }
    }
    
    pub fn get_local_scroll_pos(&self, cx:&Cx)->Vec2{
        return match self{
            Area::Instance(inst)=>{
                let cxview = &cx.views[inst.view_id];
                if cxview.redraw_id != inst.redraw_id {
                    Vec2::default()
                }
                else{
                    cxview.unsnapped_scroll
                }
            },
            Area::View(view_area)=>{
                let cxview = &cx.views[view_area.view_id];
                cxview.unsnapped_scroll
            },
            _=>Vec2::default(),
        }
    }

    pub fn get_scroll_pos(&self, cx:&Cx)->Vec2{
        return match self{
            Area::Instance(inst)=>{
                let cxview = &cx.views[inst.view_id];
                if cxview.redraw_id != inst.redraw_id {
                    Vec2::default()
                }
                else{
                    let draw_call = &cxview.draw_calls[inst.draw_call_id];
                    Vec2{
                        x:draw_call.draw_uniforms.draw_scroll_x,
                        y:draw_call.draw_uniforms.draw_scroll_y
                    }
                }
            },
            Area::View(view_area)=>{
                let cxview = &cx.views[view_area.view_id];
                cxview.parent_scroll
            },
            _=>Vec2::default(),
        }
    }
    
    pub fn set_do_scroll(&self, cx:&mut Cx, hor:bool, ver:bool){
        return match self{
            Area::Instance(inst)=>{
                let cxview = &mut cx.views[inst.view_id];
                if cxview.redraw_id != inst.redraw_id {
                    return
                }
                else{
                    let draw_call = &mut cxview.draw_calls[inst.draw_call_id];
                    draw_call.do_h_scroll = hor;
                    draw_call.do_v_scroll = ver;
                }
            },
            Area::View(view_area)=>{
                let cxview = &mut cx.views[view_area.view_id];
                cxview.do_h_scroll = hor;
                cxview.do_v_scroll = ver;
            },
            _=>(),
        }
    }
    
    // returns the final screen rect
    pub fn get_rect(&self, cx:&Cx)->Rect{

        return match self{
            Area::Instance(inst)=>{
                if inst.instance_count == 0{
                    //panic!();
                    println!("get_rect called on instance_count ==0 area pointer, use mark/sweep correctly!");
                    return Rect::default()
                }
                let cxview = &cx.views[inst.view_id];
                if cxview.redraw_id != inst.redraw_id {
                    return Rect::default();
                }
                let draw_call = &cxview.draw_calls[inst.draw_call_id];
                if draw_call.in_many_instances{
                    panic!("get_rect called whilst in many instances");
                    //return Rect::default();
                }
                if draw_call.instances.len() == 0{
                    println!("No instances but everything else valid?");
                    return Rect::default()
                }
                let sh = &cx.shaders[draw_call.shader.shader_id];
                // ok now we have to patch x/y/w/h into it
                if let Some(rect_pos) = sh.mapping.rect_instance_props.rect_pos{
                    let x = draw_call.instances[inst.instance_offset + rect_pos + 0];
                    let y = draw_call.instances[inst.instance_offset + rect_pos + 1];
                    if let Some(rect_size) = sh.mapping.rect_instance_props.rect_size{
                        let w = draw_call.instances[inst.instance_offset + rect_size + 0];
                        let h = draw_call.instances[inst.instance_offset + rect_size + 1];
                        return draw_call.clip_and_scroll_rect(x,y,w,h);
                    }
                }
                Rect::default()
            },
            Area::View(view_area)=>{
                let cxview = &cx.views[view_area.view_id];
                Rect{
                    pos: cxview.rect.pos - cxview.parent_scroll,
                    size: cxview.rect.size
                }
            },
            _=>Rect::default(),
        }
    }

    pub fn abs_to_rel(&self, cx:&Cx, abs:Vec2)->Vec2{
        return match self{
            Area::Instance(inst)=>{
                if inst.instance_count == 0{
                    println!("abs_to_rel_scroll called on instance_count ==0 area pointer, use mark/sweep correctly!");
                    return abs
                }
                let cxview = &cx.views[inst.view_id];
                if cxview.redraw_id != inst.redraw_id {
                    return abs;
                }
                let draw_call = &cxview.draw_calls[inst.draw_call_id];
                let sh = &cx.shaders[draw_call.shader.shader_id];
                // ok now we have to patch x/y/w/h into it
                if let Some(rect_pos) = sh.mapping.rect_instance_props.rect_pos{
                    let x = draw_call.instances[inst.instance_offset + rect_pos + 0];
                    let y = draw_call.instances[inst.instance_offset + rect_pos + 1];
                    return Vec2{
                        x:abs.x - x + draw_call.draw_uniforms.draw_scroll_x,
                        y:abs.y - y + draw_call.draw_uniforms.draw_scroll_y
                    }
                }
                abs
            },
            Area::View(view_area)=>{
                let cxview = &cx.views[view_area.view_id];
                Vec2{
                    x:abs.x - cxview.rect.pos.x + cxview.parent_scroll.x + cxview.unsnapped_scroll.x,
                    y:abs.y - cxview.rect.pos.y - cxview.parent_scroll.y + cxview.unsnapped_scroll.y
                }
            },
            _=>abs,
        }
    }

    pub fn set_rect(&self, cx:&mut Cx, rect:&Rect){
         match self{
            Area::Instance(inst)=>{
                let cxview = &mut cx.views[inst.view_id];
                if cxview.redraw_id != inst.redraw_id {
                    println!("set_rect called on invalid area pointer, use mark/sweep correctly!");
                    return;
                }
                let draw_call = &mut cxview.draw_calls[inst.draw_call_id];
                let sh = &cx.shaders[draw_call.shader.shader_id];        // ok now we have to patch x/y/w/h into it
                
                if let Some(rect_pos) = sh.mapping.rect_instance_props.rect_pos{
                    draw_call.instances[inst.instance_offset + rect_pos + 0] = rect.pos.x;
                    draw_call.instances[inst.instance_offset + rect_pos + 1] = rect.pos.y;
                }
                if let Some(rect_size) = sh.mapping.rect_instance_props.rect_size{
                    draw_call.instances[inst.instance_offset + rect_size + 0] = rect.size.x;
                    draw_call.instances[inst.instance_offset + rect_size + 1] = rect.size.y;
                }
            },
            Area::View(view_area)=>{
                let cxview = &mut cx.views[view_area.view_id];
                cxview.rect = rect.clone()
            },
            _=>()
         }
    }
    
    pub fn get_read_ref<'a>(&self, cx:&'a Cx, live_item_id:LiveItemId, ty:Ty)->Option<DrawReadRef<'a>>{
        match self{
            Area::Instance(inst)=>{
                let cxview = &cx.views[inst.view_id];
                let draw_call = &cxview.draw_calls[inst.draw_call_id];
                if cxview.redraw_id != inst.redraw_id {
                    println!("get_instance_read_ref called on invalid area pointer, use mark/sweep correctly!");
                    return None;
                }
                let sh = &cx.shaders[draw_call.shader.shader_id];
                if let Some(prop_id) = sh.mapping.user_uniform_props.prop_map.get(&live_item_id){
                    let prop = &sh.mapping.user_uniform_props.props[*prop_id];
                    if prop.ty != ty{
                        panic!("get_read_ref wrong uniform type, expected {:?} got: {:?}!",  prop.ty, ty);
                    }
                    return Some(
                        DrawReadRef{
                            repeat: 1,
                            stride: 0,
                            buffer: &draw_call.user_uniforms[prop.offset..]
                        }
                    )
                }
                if let Some(prop_id) = sh.mapping.instance_props.prop_map.get(&live_item_id){
                    let prop = &sh.mapping.instance_props.props[*prop_id];
                    if prop.ty != ty{
                        panic!("get_read_ref wrong instance type, expected {:?} got: {:?}!", prop.ty, ty);
                    }
                    if inst.instance_count == 0{
                        return None
                    }
                    return Some(
                        DrawReadRef{
                            repeat: inst.instance_count,
                            stride: sh.mapping.instance_props.total_slots,
                            buffer: &draw_call.instances[(inst.instance_offset + prop.offset)..],
                        }
                    )
                }
                panic!("get_read_ref property not found!");
            }
            _=>(),
        }
        None
    } 
    
    pub fn get_write_ref<'a>(&self, cx:&'a mut Cx, live_item_id:LiveItemId, ty:Ty, name:&str)->Option<DrawWriteRef<'a>>{
        match self{
            Area::Instance(inst)=>{
                let cxview = &mut cx.views[inst.view_id];
                if cxview.redraw_id != inst.redraw_id {
                    return None;
                }
                let draw_call = &mut cxview.draw_calls[inst.draw_call_id];
                let sh = &cx.shaders[draw_call.shader.shader_id];
                if let Some(prop_id) = sh.mapping.user_uniform_props.prop_map.get(&live_item_id){
                    let prop = &sh.mapping.user_uniform_props.props[*prop_id];
                    if prop.ty != ty{
                        panic!("get_write_ref {} wrong uniform type, expected {:?} got: {:?}!", name, prop.ty, ty);
                    }

                    cx.passes[cxview.pass_id].paint_dirty = true;
                    draw_call.uniforms_dirty = true;

                    return Some(
                        DrawWriteRef{
                            repeat: 1,
                            stride: 0,
                            buffer: &mut draw_call.user_uniforms[prop.offset..]
                        }                        
                    )
                }
                if let Some(prop_id) = sh.mapping.instance_props.prop_map.get(&live_item_id){
                    let prop = &sh.mapping.instance_props.props[*prop_id];
                    if prop.ty != ty{
                        panic!("get_write_ref {} wrong instance type, expected {:?} got: {:?}!", name, prop.ty, ty);
                    }

                    cx.passes[cxview.pass_id].paint_dirty = true;
                    draw_call.instance_dirty = true;
                    if inst.instance_count == 0{
                        return None
                    }
                    return Some(
                        DrawWriteRef{
                            repeat:inst.instance_count,
                            stride:sh.mapping.instance_props.total_slots,
                            buffer: &mut draw_call.instances[(inst.instance_offset + prop.offset)..]
                        }
                    )
                }
                panic!("get_write_ref {} property not found!", name);
            }
            _=>(),
        }
        None
    }

    pub fn write_texture_2d_id(&self, cx:&mut Cx, live_item_id:LiveItemId, name:&str, texture_id: usize){
         match self{
            Area::Instance(inst)=>{
                let cxview = &mut cx.views[inst.view_id];
                let draw_call = &mut cxview.draw_calls[inst.draw_call_id];
                let sh = &cx.shaders[draw_call.shader.shader_id];
                for (index, prop) in sh.mapping.textures.iter().enumerate(){
                    if prop.live_item_id == live_item_id{
                        draw_call.textures_2d[index] = texture_id as u32;
                        return
                    }
                }
            }
            _=>(),
        }
        panic!("Cannot find texture2D prop {}", name)
    }
}

impl Into<Area> for InstanceArea{
    fn into(self)->Area{
        Area::Instance(self)
    }
}
