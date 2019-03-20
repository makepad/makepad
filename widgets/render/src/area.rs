
use crate::math::*;
use crate::cx::*; 

#[derive(Clone, Default, Debug, PartialEq, Copy)]
pub struct InstanceArea{
    pub draw_list_id:usize,
    pub draw_call_id:usize,
    pub instance_offset:usize,
    pub instance_count:usize,
    pub redraw_id:u64
}

#[derive(Clone, Default, Debug, PartialEq, Copy)]
pub struct DrawListArea{
    pub draw_list_id:usize,
    pub redraw_id:u64
}

#[derive(Clone, Debug, PartialEq, Copy)]
pub enum Area{
    Empty,
    All,
    Instance(InstanceArea),
    DrawList(DrawListArea)
}

impl Default for Area{
    fn default()->Area{
        Area::Empty
    }
}

impl Area{
    pub fn is_empty(&self)->bool{
        if let Area::Empty = self{
            return true
        }
        false
    }

    pub fn get_rect(&self, cx:&Cx, no_scrolling:bool)->Rect{
        return match self{
            Area::Instance(inst)=>{
                if inst.instance_count == 0{
                    println!("get_rect called on instance_count ==0 area pointer, use mark/sweep correctly!");
                    return Rect::zero()
                }
                let draw_list = &cx.draw_lists[inst.draw_list_id];
                if draw_list.redraw_id != inst.redraw_id {
                    println!("get_rect called on invalid area pointer, use mark/sweep correctly!");
                    return Rect::zero();
                }
                let draw_call = &draw_list.draw_calls[inst.draw_call_id];
                let csh = &cx.compiled_shaders[draw_call.shader_id];
                // ok now we have to patch x/y/w/h into it
                if let Some(ix) = csh.rect_instance_props.x{
                    let x = draw_call.instance[inst.instance_offset + ix];
                    if let Some(iy) = csh.rect_instance_props.y{
                        let y = draw_call.instance[inst.instance_offset + iy];
                        if let Some(iw) = csh.rect_instance_props.w{
                            let w = draw_call.instance[inst.instance_offset + iw];
                            if let Some(ih) = csh.rect_instance_props.h{
                                let h = draw_call.instance[inst.instance_offset + ih];
                                if no_scrolling{
                                    return Rect{x:x,y:y,w:w,h:h}
                                }
                                else{
                                    return draw_list.clip_and_scroll_rect(x,y,w,h);
                                    //let scroll = draw_list.get_scroll();
                                    // also clip it 
                                    //return Rect{x:x - scroll.x,y:y - scroll.y,w:w,h:h}
                                }
                            }
                        }
                    }
                }
                Rect::zero()
            },
            Area::DrawList(draw_list_area)=>{
                let draw_list = &cx.draw_lists[draw_list_area.draw_list_id];
                //draw_list.get_scroll();
                //let mut rect = 
                draw_list.rect.clone()
            },
            _=>Rect::zero(),
        }
    }

    pub fn set_rect(&self, cx:&mut Cx, rect:&Rect){
         match self{
            Area::Instance(inst)=>{
                let draw_list = &mut cx.draw_lists[inst.draw_list_id];
                if draw_list.redraw_id != inst.redraw_id {
                    println!("set_rect called on invalid area pointer, use mark/sweep correctly!");
                    return;
                }
                let draw_call = &mut draw_list.draw_calls[inst.draw_call_id];
                let csh = &cx.compiled_shaders[draw_call.shader_id];        // ok now we have to patch x/y/w/h into it

                if let Some(ix) = csh.rect_instance_props.x{
                    draw_call.instance[inst.instance_offset + ix] = rect.x;
                }
                if let Some(iy) = csh.rect_instance_props.y{
                    draw_call.instance[inst.instance_offset + iy] = rect.y;
                }
                if let Some(iw) = csh.rect_instance_props.w{
                    draw_call.instance[inst.instance_offset + iw] = rect.w;
                }
                if let Some(ih) = csh.rect_instance_props.h{
                    draw_call.instance[inst.instance_offset + ih] = rect.h;
                }
            },
            Area::DrawList(draw_list_area)=>{
                let draw_list = &mut cx.draw_lists[draw_list_area.draw_list_id];
                draw_list.rect = rect.clone()
            },
            _=>()
         }
    }

    /*
    // moved into cxdrawing_turtle for borrowchecker reasons
    pub fn move_xy(&self, dx:f32, dy:f32, cd:&mut CxDrawing){
        return match self{
            Area::Instance(inst)=>{
                if inst.instance_count == 0{
                    return;
                }
                let draw_list = &mut cd.draw_lists[inst.draw_list_id];
                let draw_call = &mut draw_list.draw_calls[inst.draw_call_id];
                let csh = &cd.compiled_shaders[draw_call.shader_id];

                for i in 0..inst.instance_count{
                    if let Some(x) = csh.rect_instance_props.x{
                        draw_call.instance[inst.instance_offset + x + i * csh.instance_slots] += dx;
                    }
                    if let Some(y) = csh.rect_instance_props.y{
                        draw_call.instance[inst.instance_offset + y+ i * csh.instance_slots] += dy;
                    }
                }
            }
            Area::DrawList(area_draw_list)=>{
                let draw_list = &mut cd.draw_lists[area_draw_list.draw_list_id];
                draw_list.rect.x += dx;
                draw_list.rect.y += dy;
            },
            _=>(),
        }
    }
    */

    pub fn write_float(&self, cx:&mut Cx, prop_name:&str, value:f32){
        match self{
            Area::Instance(inst)=>{
                let draw_list = &mut cx.draw_lists[inst.draw_list_id];
                if draw_list.redraw_id != inst.redraw_id {
                    println!("write_float called on invalid area pointer, use mark/sweep correctly!");
                    return;
                }
                let draw_call = &mut draw_list.draw_calls[inst.draw_call_id];

                let csh = &cx.compiled_shaders[draw_call.shader_id];

                for prop in &csh.named_instance_props.props{
                    if prop.name == prop_name{
                        cx.paint_dirty = true;
                        draw_call.instance_dirty = true;
                        let mut off = inst.instance_offset + prop.offset;
                        for _i in 0..inst.instance_count{
                            draw_call.instance[off + 0] = value;
                            off += csh.instance_slots;
                        }
                        return
                    }
                }
            },
            _=>(),
        }
    }

    pub fn read_float(&self, cx:&Cx, prop_name:&str)->f32{
        match self{
            Area::Instance(inst)=>{
                let draw_list = &cx.draw_lists[inst.draw_list_id];
                let draw_call = &draw_list.draw_calls[inst.draw_call_id];
                if draw_list.redraw_id != inst.redraw_id {
                    println!("read_float called on invalid area pointer, use mark/sweep correctly!");
                    return 0.0;
                }
                let csh = &cx.compiled_shaders[draw_call.shader_id];

                for prop in &csh.named_instance_props.props{
                    if prop.name == prop_name{
                        return draw_call.instance[inst.instance_offset + prop.offset + 0]
                    }
                }
            }
            _=>(),
        }
        return 0.0;
    }

   pub fn write_vec2(&self, cx:&mut Cx, prop_name:&str, value:Vec2){
        match self{
            Area::Instance(inst)=>{
                let draw_list = &mut cx.draw_lists[inst.draw_list_id];
                if draw_list.redraw_id != inst.redraw_id {
                    println!("write_vec2 called on invalid area pointer, use mark/sweep correctly!");
                    return;
                }
                let draw_call = &mut draw_list.draw_calls[inst.draw_call_id];
                let csh = &cx.compiled_shaders[draw_call.shader_id];
                for prop in &csh.named_instance_props.props{
                    if prop.name == prop_name{
                        cx.paint_dirty = true;
                        draw_call.instance_dirty = true;
                        let mut off = inst.instance_offset + prop.offset;
                        for _i in 0..inst.instance_count{
                            draw_call.instance[off + 0] = value.x;
                            draw_call.instance[off + 1] = value.y;
                            off += csh.instance_slots;
                        }
                        return
                    }
                }
            }
            _=>(),
        }
    }

    pub fn read_vec2(&self, cx:&Cx, prop_name:&str)->Vec2{
        match self{
            Area::Instance(inst)=>{
                let draw_list = &cx.draw_lists[inst.draw_list_id];
                if draw_list.redraw_id != inst.redraw_id {
                    println!("read_vec2 called on invalid area pointer, use mark/sweep correctly!");
                    return vec2(0.0,0.0)
                }
                let draw_call = &draw_list.draw_calls[inst.draw_call_id];
                let csh = &cx.compiled_shaders[draw_call.shader_id];

                for prop in &csh.named_instance_props.props{
                    if prop.name == prop_name{
                        let off = inst.instance_offset + prop.offset;
                        return Vec2{
                            x:draw_call.instance[off + 0],
                            y:draw_call.instance[off + 1]
                        }
                    }
                }
            },
            _=>(),
        }
        return vec2(0.0,0.0);
    }

    pub fn write_vec3(&self, cx:&mut Cx, prop_name:&str, value:Vec3){
        match self{
            Area::Instance(inst)=>{
                let draw_list = &mut cx.draw_lists[inst.draw_list_id];
                if draw_list.redraw_id != inst.redraw_id {
                    println!("write_vec3 called on invalid area pointer, use mark/sweep correctly!");
                    return
                }
                let draw_call = &mut draw_list.draw_calls[inst.draw_call_id];
                let csh = &cx.compiled_shaders[draw_call.shader_id];

                for prop in &csh.named_instance_props.props{
                    if prop.name == prop_name{
                        cx.paint_dirty = true;
                        draw_call.instance_dirty = true;
                        let mut off = inst.instance_offset + prop.offset;
                        for _i in 0..inst.instance_count{
                            draw_call.instance[off + 0] = value.x;
                            draw_call.instance[off + 1] = value.y;
                            draw_call.instance[off + 2] = value.z;
                            off += csh.instance_slots;
                        }
                        return
                    }
                }
            },
            _=>(),
        }
    }

    pub fn read_vec3(&self, cx:&Cx, prop_name:&str)->Vec3{
        match self{
            Area::Instance(inst)=>{
                let draw_list = &cx.draw_lists[inst.draw_list_id];
                let draw_call = &draw_list.draw_calls[inst.draw_call_id];
                if draw_list.redraw_id != inst.redraw_id {
                    println!("read_vec3 called on invalid area pointer, use mark/sweep correctly!");
                    return vec3(0.,0.,0.)
                }
                let csh = &cx.compiled_shaders[draw_call.shader_id];

                for prop in &csh.named_instance_props.props{
                    if prop.name == prop_name{
                        let off = inst.instance_offset + prop.offset;
                        return Vec3{
                            x:draw_call.instance[off + 0],
                            y:draw_call.instance[off + 1],
                            z:draw_call.instance[off + 2]
                        }
                    }
                }
            }
            _=>(),
        }
        return vec3(0.0,0.0,0.0);
    }

    pub fn write_vec4(&self, cx:&mut Cx, prop_name:&str, value:Vec4){
        match self{
            Area::Instance(inst)=>{
                let draw_list = &mut cx.draw_lists[inst.draw_list_id];
                if draw_list.redraw_id != inst.redraw_id {
                    println!("write_vec4 called on invalid area pointer, use mark/sweep correctly!");
                    return
                }
                let draw_call = &mut draw_list.draw_calls[inst.draw_call_id];
                let csh = &cx.compiled_shaders[draw_call.shader_id];

                for prop in &csh.named_instance_props.props{
                    if prop.name == prop_name{
                        cx.paint_dirty = true;
                        draw_call.instance_dirty = true;
                        let mut off = inst.instance_offset + prop.offset;
                        for _i in 0..inst.instance_count{
                            draw_call.instance[off + 0] = value.x;
                            draw_call.instance[off + 1] = value.y;
                            draw_call.instance[off + 2] = value.z;
                            draw_call.instance[off + 3] = value.w;
                            off += csh.instance_slots;
                        }
                        return
                    }
                }
            },
            _=>(),
        }
    }

    pub fn read_vec4(&self, cx:&Cx, prop_name:&str)->Vec4{
        match self{
            Area::Instance(inst)=>{
                let draw_list = &cx.draw_lists[inst.draw_list_id];
                if draw_list.redraw_id != inst.redraw_id {
                    println!("read_vec4 called on invalid area pointer, use mark/sweep correctly!");
                    return vec4(0.,0.,0.,0.)
                }
                let draw_call = &draw_list.draw_calls[inst.draw_call_id];
                let csh = &cx.compiled_shaders[draw_call.shader_id];

                for prop in &csh.named_instance_props.props{
                    if prop.name == prop_name{
                        let off = inst.instance_offset + prop.offset;
                        return Vec4{
                            x:draw_call.instance[off + 0],
                            y:draw_call.instance[off + 1],
                            z:draw_call.instance[off + 2],
                            w:draw_call.instance[off + 3]
                        }
                    }
                }
            },
            _=>(),
        }
        return vec4(0.0,0.0,0.0,0.0);
    }

    pub fn push_data(&self, cx:&mut Cx, data:&[f32]){
        match self{
            Area::Instance(inst)=>{
                let draw_list = &mut cx.draw_lists[inst.draw_list_id];
                if draw_list.redraw_id != inst.redraw_id {
                    println!("push_data called on invalid area pointer, use mark/sweep correctly!");
                    return
                }
                let draw_call = &mut draw_list.draw_calls[inst.draw_call_id];
                //let csh = &cx.shaders.compiled_shaders[draw_call.shader_id];
                draw_call.instance.extend_from_slice(data);
            },
            _=>(),
        }
    }

    pub fn push_float(&self, cx:&mut Cx, _name:&str, value:f32){
        match self{
            Area::Instance(inst)=>{
                let draw_list = &mut cx.draw_lists[inst.draw_list_id];
                if draw_list.redraw_id != inst.redraw_id {
                    println!("push_float called on invalid area pointer, use mark/sweep correctly!");
                    return
                }
                let draw_call = &mut draw_list.draw_calls[inst.draw_call_id];
                //let csh = &cx.shaders.compiled_shaders[draw_call.shader_id];
                draw_call.instance.push(value);
            },
            _=>(),
        }
    }


    pub fn push_vec2(&self, cx:&mut Cx, _name:&str, value:Vec2){
        match self{
            Area::Instance(inst)=>{
                let draw_list = &mut cx.draw_lists[inst.draw_list_id];
                if draw_list.redraw_id != inst.redraw_id {
                    println!("push_vec2 called on invalid area pointer, use mark/sweep correctly!");
                    return
                }
                let draw_call = &mut draw_list.draw_calls[inst.draw_call_id];
                //let csh = &cx.shaders.compiled_shaders[draw_call.shader_id];
                draw_call.instance.push(value.x);
                draw_call.instance.push(value.y);
            },
            _=>(),
        }
    }


    pub fn push_vec3(&self, cx:&mut Cx, _name:&str, value:Vec3){
        match self{
            Area::Instance(inst)=>{
                let draw_list = &mut cx.draw_lists[inst.draw_list_id];
                if draw_list.redraw_id != inst.redraw_id {
                    println!("push_vec3 called on invalid area pointer, use mark/sweep correctly!");
                    return
                }
                let draw_call = &mut draw_list.draw_calls[inst.draw_call_id];
                draw_call.instance.push(value.x);
                draw_call.instance.push(value.y);
                draw_call.instance.push(value.z);
            },
            _=>(),
        }
    }


    pub fn push_vec4(&self, cx:&mut Cx, _name:&str, value:Vec4){
        match self{
            Area::Instance(inst)=>{
                let draw_list = &mut cx.draw_lists[inst.draw_list_id];
                if draw_list.redraw_id != inst.redraw_id {
                    println!("push_vec4 called on invalid area pointer, use mark/sweep correctly!");
                    return
                }
                let draw_call = &mut draw_list.draw_calls[inst.draw_call_id];
                draw_call.instance.push(value.x);
                draw_call.instance.push(value.y);
                draw_call.instance.push(value.z);
                draw_call.instance.push(value.w);
            },
            _=>(),
        }
    }


    pub fn need_uniforms_now(&self, cx:&mut Cx)->bool{
        match self{
            Area::Instance(inst)=>{
                let draw_list = &mut cx.draw_lists[inst.draw_list_id];
                if draw_list.redraw_id != inst.redraw_id {
                    println!("need_uniforms_now called on invalid area pointer, use mark/sweep correctly!");
                    return false
                }
                let draw_call = &mut draw_list.draw_calls[inst.draw_call_id];
                //let csh = &cx.shaders.compiled_shaders[draw_call.shader_id];
                return draw_call.need_uniforms_now
            },
            _=>(),
        }
        return false
    }

   pub fn uniform_texture_2d(&self, cx:&mut Cx, _name: &str, texture_id: usize){
        match self{
            Area::Instance(inst)=>{
                let draw_list = &mut cx.draw_lists[inst.draw_list_id];
                if draw_list.redraw_id != inst.redraw_id {
                    println!("uniform_texture_2d called on invalid area pointer, use mark/sweep correctly!");
                    return
                }
                let draw_call = &mut draw_list.draw_calls[inst.draw_call_id]; 
                draw_call.textures_2d.push(texture_id as u32);
            },
            _=>()
        }
    }

    pub fn uniform_float(&self, cx:&mut Cx, _name: &str, v:f32){
        match self{
            Area::Instance(inst)=>{
                let draw_list = &mut cx.draw_lists[inst.draw_list_id];
                if draw_list.redraw_id != inst.redraw_id {
                    println!("uniform_float called on invalid area pointer, use mark/sweep correctly!");
                    return
                }
                let draw_call = &mut draw_list.draw_calls[inst.draw_call_id]; 
                draw_call.uniforms.push(v);
            },
            _=>()
         }
    }

    pub fn uniform_vec2f(&self, cx:&mut Cx, _name: &str, x:f32, y:f32){
        match self{
            Area::Instance(inst)=>{
                let draw_list = &mut cx.draw_lists[inst.draw_list_id];
                if draw_list.redraw_id != inst.redraw_id {
                    println!("uniform_vec2f called on invalid area pointer, use mark/sweep correctly!");
                    return
                }
                let draw_call = &mut draw_list.draw_calls[inst.draw_call_id]; 
                draw_call.uniforms.push(x);
                draw_call.uniforms.push(y);
            },
            _=>()
         }
    }

    pub fn uniform_vec3f(&mut self, cx:&mut Cx, _name: &str, x:f32, y:f32, z:f32){
        match self{
            Area::Instance(inst)=>{
                let draw_list = &mut cx.draw_lists[inst.draw_list_id];
                if draw_list.redraw_id != inst.redraw_id {
                    println!("uniform_vec3f called on invalid area pointer, use mark/sweep correctly!");
                    return
                }
                let draw_call = &mut draw_list.draw_calls[inst.draw_call_id]; 
                draw_call.uniforms.push(x);
                draw_call.uniforms.push(y);
                draw_call.uniforms.push(z);
            },
            _=>()
        }
    }

    pub fn uniform_vec4f(&self, cx:&mut Cx, _name: &str, x:f32, y:f32, z:f32, w:f32){
        match self{
            Area::Instance(inst)=>{
                let draw_list = &mut cx.draw_lists[inst.draw_list_id];
                if draw_list.redraw_id != inst.redraw_id {
                    println!("uniform_vec4f called on invalid area pointer, use mark/sweep correctly!");
                    return
                }
                let draw_call = &mut draw_list.draw_calls[inst.draw_call_id]; 
                draw_call.uniforms.push(x);
                draw_call.uniforms.push(y);
                draw_call.uniforms.push(z);
                draw_call.uniforms.push(w);
            },
            _=>()
        }
    }

    pub fn uniform_mat4(&self, cx:&mut Cx, _name: &str, v:&Mat4){
        match self{
            Area::Instance(inst)=>{
                let draw_list = &mut cx.draw_lists[inst.draw_list_id];
                if draw_list.redraw_id != inst.redraw_id {
                    println!("uniform_mat4 called on invalid area pointer, use mark/sweep correctly!");
                    return
                }
                let draw_call = &mut draw_list.draw_calls[inst.draw_call_id]; 
                for i in 0..16{
                    draw_call.uniforms.push(v.v[i]);
                }
            },
            _=>()
        }
    }
}
