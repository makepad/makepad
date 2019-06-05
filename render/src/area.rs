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

pub struct InstanceReadRef<'a>{
    pub offset:usize,
    pub slots:usize,
    pub count:usize,
    pub buffer:&'a Vec<f32>
}

pub struct InstanceWriteRef<'a>{
    pub offset:usize,
    pub slots:usize,
    pub count:usize,
    pub buffer:&'a mut Vec<f32>
}

impl Area{
    pub fn is_empty(&self)->bool{
        if let Area::Empty = self{
            return true
        }
        false
    }

    pub fn is_valid(&self, cx:&Cx)->bool{
        return match self{
            Area::Instance(inst)=>{
                if inst.instance_count == 0{
                    return false
                }
                let draw_list = &cx.draw_lists[inst.draw_list_id];
                if draw_list.redraw_id != inst.redraw_id {
                    return false
                }
                return true
            },
            Area::DrawList(draw_list_area)=>{
                let draw_list = &cx.draw_lists[draw_list_area.draw_list_id];
                if draw_list.redraw_id != draw_list_area.redraw_id {
                    return false
                }
                return true
            },
            _=>false,
        }
    }
    
    pub fn get_scroll_pos(&self, cx:&Cx)->Vec2{
        return match self{
            Area::Instance(inst)=>{
                let draw_list = &cx.draw_lists[inst.draw_list_id];
                if draw_list.redraw_id != inst.redraw_id {
                    Vec2::zero()
                }
                else{
                    draw_list.get_scroll_pos()
                }
            },
            Area::DrawList(draw_list_area)=>{
                let draw_list = &cx.draw_lists[draw_list_area.draw_list_id];
                draw_list.get_scroll_pos()
            },
            _=>Vec2::zero(),
        }
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
                                }
                            }
                        }
                    }
                }
                Rect::zero()
            },
            Area::DrawList(draw_list_area)=>{
                let draw_list = &cx.draw_lists[draw_list_area.draw_list_id];
                draw_list.rect.clone()
            },
            _=>Rect::zero(),
        }
    }

    pub fn abs_to_rel(&self, cx:&Cx, abs:Vec2, no_scrolling:bool)->Vec2{
        return match self{
            Area::Instance(inst)=>{
                if inst.instance_count == 0{
                    println!("abs_to_rel_scroll called on instance_count ==0 area pointer, use mark/sweep correctly!");
                    return abs
                }
                let draw_list = &cx.draw_lists[inst.draw_list_id];
                if draw_list.redraw_id != inst.redraw_id {
                    return abs;
                }
                let draw_call = &draw_list.draw_calls[inst.draw_call_id];
                let csh = &cx.compiled_shaders[draw_call.shader_id];
                // ok now we have to patch x/y/w/h into it
                if let Some(ix) = csh.rect_instance_props.x{
                    let x = draw_call.instance[inst.instance_offset + ix];
                    if let Some(iy) = csh.rect_instance_props.y{
                        let y = draw_call.instance[inst.instance_offset + iy];
                        if no_scrolling{
                            return Vec2{
                                x:abs.x - x,
                                y:abs.y - y
                            }
                        }
                        else{
                            let scroll = draw_list.get_scroll_pos();
                            return Vec2{
                                x:abs.x - x + scroll.x,
                                y:abs.y - y + scroll.y
                            }
                        }
                    }
                }
                abs
            },
            Area::DrawList(draw_list_area)=>{
                let draw_list = &cx.draw_lists[draw_list_area.draw_list_id];
                Vec2{
                    x:abs.x - draw_list.rect.x,
                    y:abs.y - draw_list.rect.y
                }
            },
            _=>abs,
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

    pub fn get_instance_offset(&self, cx:&Cx, prop_name:&str)->usize{
        match self{
            Area::Instance(inst)=>{
                let draw_list = &cx.draw_lists[inst.draw_list_id];
                let draw_call = &draw_list.draw_calls[inst.draw_call_id];
                let csh = &cx.compiled_shaders[draw_call.shader_id];
                for prop in &csh.named_instance_props.props{
                    if prop.name == prop_name{
                        return prop.offset
                    }
                }
            }
            _=>(),
        }
        println!("get_instance_offset {} called on invalid prop", prop_name);
        0
    }

    pub fn get_uniform_offset(&self, cx:&Cx, prop_name:&str)->usize{
        match self{
            Area::Instance(inst)=>{
                let draw_list = &cx.draw_lists[inst.draw_list_id];
                let draw_call = &draw_list.draw_calls[inst.draw_call_id];
                let csh = &cx.compiled_shaders[draw_call.shader_id];
                for prop in &csh.named_uniform_props.props{
                    if prop.name == prop_name{
                        return prop.offset
                    }
                }
                let mut dbg = String::new();
                for prop in &csh.named_uniform_props.props{
                    dbg.push_str(&format!("name:{} offset:{}, ", prop.name, prop.offset));
                }
                println!("get_uniform_offset {} not found in [{}]", prop_name,dbg);
            }
            _=>(),
        }
        println!("get_uniform_offset {} called on invalid prop", prop_name);
        0
    }


    pub fn get_read_ref<'a>(&self, cx:&'a Cx)->Option<InstanceReadRef<'a>>{
        match self{
            Area::Instance(inst)=>{
                let draw_list = &cx.draw_lists[inst.draw_list_id];
                let draw_call = &draw_list.draw_calls[inst.draw_call_id];
                if draw_list.redraw_id != inst.redraw_id {
                    println!("get_read_ref alled on invalid area pointer, use mark/sweep correctly!");
                    return None;
                }
                let csh = &cx.compiled_shaders[draw_call.shader_id];
                return Some(
                    InstanceReadRef{
                        offset:inst.instance_offset, 
                        count:inst.instance_count, 
                        slots:csh.instance_slots,
                        buffer:&draw_call.instance
                    }
                )
            }
            _=>(),
        }
        return None;
    }

    pub fn get_write_ref<'a>(&self, cx:&'a mut Cx)->Option<InstanceWriteRef<'a>>{
        match self{
            Area::Instance(inst)=>{
                let draw_list = &mut cx.draw_lists[inst.draw_list_id];
                let draw_call = &mut draw_list.draw_calls[inst.draw_call_id];
                if draw_list.redraw_id != inst.redraw_id {
                    //println!("get_write_ref called on invalid area pointer, use mark/sweep correctly!");
                    return None;
                }
                let csh =&cx.compiled_shaders[draw_call.shader_id];
                cx.windows[draw_list.window_id].paint_dirty = true;
                draw_call.instance_dirty = true;
                return Some(
                    InstanceWriteRef{
                        offset:inst.instance_offset, 
                        count:inst.instance_count, 
                        slots:csh.instance_slots,
                        buffer:&mut draw_call.instance
                    }
                )
            }
            _=>(),
        }
        return None;
    }

    pub fn get_uniform_write_ref<'a>(&self, cx:&'a mut Cx)->Option<&'a mut Vec<f32>>{
        match self{
            Area::Instance(inst)=>{
                let draw_list = &mut cx.draw_lists[inst.draw_list_id];
                let draw_call = &mut draw_list.draw_calls[inst.draw_call_id];
                if draw_list.redraw_id != inst.redraw_id {
                    return None;
                }
                cx.windows[draw_list.window_id].paint_dirty = true;
                draw_call.uniforms_dirty = true;
                return Some(
                    &mut draw_call.uniforms
                )
            }
            _=>(),
        }
        return None;
    }

    pub fn write_float(&self, cx:&mut Cx, prop_name:&str, value:f32){
        let inst_offset = self.get_instance_offset(cx, prop_name);
        let write = self.get_write_ref(cx);
        if let Some(write) = write{
            for i in 0..write.count{
                write.buffer[write.offset + inst_offset + i * write.slots] = value;
            }
        }
    }

    pub fn read_float(&self, cx:&Cx, prop_name:&str)->f32{
        let inst_offset = self.get_instance_offset(cx, prop_name);
        let read = self.get_read_ref(cx);
        if let Some(read) = read{
            read.buffer[read.offset + inst_offset]
        }
        else{
            0.0
        }
    }

   pub fn write_vec2(&self, cx:&mut Cx, prop_name:&str, value:Vec2){
        let inst_offset = self.get_instance_offset(cx, prop_name);
        let write = self.get_write_ref(cx);
        if let Some(write) = write{
            for i in 0..write.count{
                write.buffer[write.offset + inst_offset + 0 + i * write.slots] = value.y;
                write.buffer[write.offset + inst_offset + 1 + i * write.slots] = value.x;
            }
        }
   }

    pub fn read_vec2(&self, cx:&Cx, prop_name:&str)->Vec2{
        let inst_offset = self.get_instance_offset(cx, prop_name);
        let read = self.get_read_ref(cx);
        if let Some(read) = read{
            Vec2{
                x:read.buffer[read.offset + inst_offset + 0],
                y:read.buffer[read.offset + inst_offset + 1]
            }
        }
        else{
            Vec2::zero()
        }
    }

   pub fn write_vec3(&self, cx:&mut Cx, prop_name:&str, value:Vec3){
        let inst_offset = self.get_instance_offset(cx, prop_name);
        let write = self.get_write_ref(cx);
        if let Some(write) = write{
            for i in 0..write.count{
                write.buffer[write.offset + inst_offset + 0 + i * write.slots] = value.y;
                write.buffer[write.offset + inst_offset + 1 + i * write.slots] = value.x;
                write.buffer[write.offset + inst_offset + 2 + i * write.slots] = value.z;
            }
        }
   }

    pub fn read_vec3(&self, cx:&Cx, prop_name:&str)->Vec3{
        let inst_offset = self.get_instance_offset(cx, prop_name);
        let read = self.get_read_ref(cx);
        if let Some(read) = read{
            Vec3{
                x:read.buffer[read.offset + inst_offset + 0],
                y:read.buffer[read.offset + inst_offset + 1],
                z:read.buffer[read.offset + inst_offset + 2]
            }
        }
        else{
            Vec3::zero()
        }
    }

   pub fn write_vec4(&self, cx:&mut Cx, prop_name:&str, value:Vec4){
        let inst_offset = self.get_instance_offset(cx, prop_name);
        let write = self.get_write_ref(cx);
        if let Some(write) = write{
            for i in 0..write.count{
                write.buffer[write.offset + inst_offset + 0 + i * write.slots] = value.x;
                write.buffer[write.offset + inst_offset + 1 + i * write.slots] = value.y;
                write.buffer[write.offset + inst_offset + 2 + i * write.slots] = value.z;
                write.buffer[write.offset + inst_offset + 3 + i * write.slots] = value.w;
            }
        }
   }

    pub fn read_vec4(&self, cx:&Cx, prop_name:&str)->Vec4{
        let inst_offset = self.get_instance_offset(cx, prop_name);
        let read = self.get_read_ref(cx);
        if let Some(read) = read{
            Vec4{
                x:read.buffer[read.offset + inst_offset + 0],
                y:read.buffer[read.offset + inst_offset + 1],
                z:read.buffer[read.offset + inst_offset + 2],
                w:read.buffer[read.offset + inst_offset + 3],
            }
        }
        else{
            Vec4::zero()
        }
    }

    pub fn write_color(&self, cx:&mut Cx, prop_name:&str, value:Color){
        let inst_offset = self.get_instance_offset(cx, prop_name);
        let write = self.get_write_ref(cx);
        if let Some(write) = write{
            for i in 0..write.count{
                write.buffer[write.offset + inst_offset + 0 + i * write.slots] = value.r;
                write.buffer[write.offset + inst_offset + 1 + i * write.slots] = value.g;
                write.buffer[write.offset + inst_offset + 2 + i * write.slots] = value.b;
                write.buffer[write.offset + inst_offset + 3 + i * write.slots] = value.a;
            }
        }
   }

    pub fn read_color(&self, cx:&Cx, prop_name:&str)->Color{
        let inst_offset = self.get_instance_offset(cx, prop_name);
        let read = self.get_read_ref(cx);
        if let Some(read) = read{
            Color{
                r:read.buffer[read.offset + inst_offset + 0],
                g:read.buffer[read.offset + inst_offset + 1],
                b:read.buffer[read.offset + inst_offset + 2],
                a:read.buffer[read.offset + inst_offset + 3],
            }
        }
        else{
            Color::zero()
        }
    }

    pub fn write_uniform_float(&self, cx:&mut Cx, prop_name:&str, v:f32){
        let uni_offset = self.get_uniform_offset(cx, prop_name);
        let write = self.get_uniform_write_ref(cx);
        if let Some(write) = write{
            while uni_offset >= write.len(){
                write.push(0.);
            }
            write[uni_offset] = v;
        }
    }
/*
    pub fn push_uniform_vec2f(&self, cx:&mut Cx,  x:f32, y:f32){
        let draw_list = &mut cx.draw_lists[self.draw_list_id];
        if draw_list.redraw_id != self.redraw_id {
            println!("uniform_vec2f called on invalid area pointer, use mark/sweep correctly!");
            return
        }
        let draw_call = &mut draw_list.draw_calls[self.draw_call_id]; 
        draw_call.uniforms.push(x);
        draw_call.uniforms.push(y);
    }

    pub fn push_uniform_vec3f(&mut self, cx:&mut Cx, x:f32, y:f32, z:f32){
        let draw_list = &mut cx.draw_lists[self.draw_list_id];
        if draw_list.redraw_id != self.redraw_id {
            println!("uniform_vec3f called on invalid area pointer, use mark/sweep correctly!");
            return
        }
        let draw_call = &mut draw_list.draw_calls[self.draw_call_id]; 
        draw_call.uniforms.push(x);
        draw_call.uniforms.push(y);
        draw_call.uniforms.push(z);
    }

    pub fn push_uniform_vec4f(&self, cx:&mut Cx, x:f32, y:f32, z:f32, w:f32){
        let draw_list = &mut cx.draw_lists[self.draw_list_id];
        if draw_list.redraw_id != self.redraw_id {
            println!("uniform_vec4f called on invalid area pointer, use mark/sweep correctly!");
            return
        }
        let draw_call = &mut draw_list.draw_calls[self.draw_call_id]; 
        draw_call.uniforms.push(x);
        draw_call.uniforms.push(y);
        draw_call.uniforms.push(z);
        draw_call.uniforms.push(w);
    }

    pub fn push_uniform_mat4(&self, cx:&mut Cx, v:&Mat4){
        let draw_list = &mut cx.draw_lists[self.draw_list_id];
        if draw_list.redraw_id != self.redraw_id {
            println!("uniform_mat4 called on invalid area pointer, use mark/sweep correctly!");
            return
        }
        let draw_call = &mut draw_list.draw_calls[self.draw_call_id]; 
        for i in 0..16{
            draw_call.uniforms.push(v.v[i]);
        }
    }    */
}

impl InstanceArea{
    pub fn into_area(self)->Area{
        Area::Instance(self)
    }

    pub fn push_slice(&self, cx:&mut Cx, data:&[f32]){
        let draw_list = &mut cx.draw_lists[self.draw_list_id];
        if draw_list.redraw_id != self.redraw_id {
            println!("push_data called on invalid area pointer, use mark/sweep correctly!");
            return
        }
        let draw_call = &mut draw_list.draw_calls[self.draw_call_id];
        //let csh = &cx.shaders.compiled_shaders[draw_call.shader_id];
        draw_call.instance.extend_from_slice(data);
    }

    pub fn push_float(&self, cx:&mut Cx, value:f32){
        let draw_list = &mut cx.draw_lists[self.draw_list_id];
        if draw_list.redraw_id != self.redraw_id {
            println!("push_float called on invalid area pointer, use mark/sweep correctly!");
            return
        }
        let draw_call = &mut draw_list.draw_calls[self.draw_call_id];
        //let csh = &cx.shaders.compiled_shaders[draw_call.shader_id];
        draw_call.instance.push(value);
    }

    pub fn push_vec2(&self, cx:&mut Cx, value:Vec2){
        let draw_list = &mut cx.draw_lists[self.draw_list_id];
        if draw_list.redraw_id != self.redraw_id {
            println!("push_vec2 called on invalid area pointer, use mark/sweep correctly!");
            return
        }
        let draw_call = &mut draw_list.draw_calls[self.draw_call_id];
        //let csh = &cx.shaders.compiled_shaders[draw_call.shader_id];
        draw_call.instance.push(value.x);
        draw_call.instance.push(value.y);
    }

    pub fn push_vec3(&self, cx:&mut Cx, value:Vec3){
        let draw_list = &mut cx.draw_lists[self.draw_list_id];
        if draw_list.redraw_id != self.redraw_id {
            println!("push_vec3 called on invalid area pointer, use mark/sweep correctly!");
            return
        }
        let draw_call = &mut draw_list.draw_calls[self.draw_call_id];
        draw_call.instance.push(value.x);
        draw_call.instance.push(value.y);
        draw_call.instance.push(value.z);
    }

    pub fn push_vec4(&self, cx:&mut Cx, value:Vec4){
        let draw_list = &mut cx.draw_lists[self.draw_list_id];
        if draw_list.redraw_id != self.redraw_id {
            println!("push_vec4 called on invalid area pointer, use mark/sweep correctly!");
            return
        }
        let draw_call = &mut draw_list.draw_calls[self.draw_call_id];
        draw_call.instance.push(value.x);
        draw_call.instance.push(value.y);
        draw_call.instance.push(value.z);
        draw_call.instance.push(value.w);
    }

    pub fn push_color(&self, cx:&mut Cx, value:Color){
        let draw_list = &mut cx.draw_lists[self.draw_list_id];
        if draw_list.redraw_id != self.redraw_id {
            println!("push_vec4 called on invalid area pointer, use mark/sweep correctly!");
            return
        }
        let draw_call = &mut draw_list.draw_calls[self.draw_call_id];
        draw_call.instance.push(value.r);
        draw_call.instance.push(value.g);
        draw_call.instance.push(value.b);
        draw_call.instance.push(value.a);
    }

    pub fn need_uniforms_now(&self, cx:&mut Cx)->bool{
        let draw_list = &mut cx.draw_lists[self.draw_list_id];
        if draw_list.redraw_id != self.redraw_id {
            println!("need_uniforms_now called on invalid area pointer, use mark/sweep correctly!");
            return false
        }
        let draw_call = &mut draw_list.draw_calls[self.draw_call_id];
        //let csh = &cx.shaders.compiled_shaders[draw_call.shader_id];
        return draw_call.need_uniforms_now()
    }

   pub fn push_uniform_texture_2d(&self, cx:&mut Cx,texture_id: usize){
        let draw_list = &mut cx.draw_lists[self.draw_list_id];
        if draw_list.redraw_id != self.redraw_id {
            println!("uniform_texture_2d called on invalid area pointer, use mark/sweep correctly!");
            return
        }
        let draw_call = &mut draw_list.draw_calls[self.draw_call_id]; 
        draw_call.textures_2d.push(texture_id as u32);
    }

    pub fn push_uniform_float(&self, cx:&mut Cx, v:f32){
        let draw_list = &mut cx.draw_lists[self.draw_list_id];
        if draw_list.redraw_id != self.redraw_id {
            println!("uniform_float called on invalid area pointer, use mark/sweep correctly!");
            return
        }
        let draw_call = &mut draw_list.draw_calls[self.draw_call_id]; 
        draw_call.uniforms.push(v);
    }

    pub fn push_uniform_vec2f(&self, cx:&mut Cx,  x:f32, y:f32){
        let draw_list = &mut cx.draw_lists[self.draw_list_id];
        if draw_list.redraw_id != self.redraw_id {
            println!("uniform_vec2f called on invalid area pointer, use mark/sweep correctly!");
            return
        }
        let draw_call = &mut draw_list.draw_calls[self.draw_call_id]; 
        let left = draw_call.uniforms.len()&3;
        if left > 2{ // align buffer
            for _ in 0..(4-left){
                draw_call.uniforms.push(0.0);
            }
        }
        draw_call.uniforms.push(x);
        draw_call.uniforms.push(y);
    }

    pub fn push_uniform_vec3f(&mut self, cx:&mut Cx, x:f32, y:f32, z:f32){
        let draw_list = &mut cx.draw_lists[self.draw_list_id];
        if draw_list.redraw_id != self.redraw_id {
            println!("uniform_vec3f called on invalid area pointer, use mark/sweep correctly!");
            return
        }
        let draw_call = &mut draw_list.draw_calls[self.draw_call_id]; 
        let left = draw_call.uniforms.len()&3;
        if left > 1{ // align buffer
            for _ in 0..(4-left){
                draw_call.uniforms.push(0.0);
            }
        }
        draw_call.uniforms.push(x);
        draw_call.uniforms.push(y);
        draw_call.uniforms.push(z);
    }

    pub fn push_uniform_vec4f(&self, cx:&mut Cx, x:f32, y:f32, z:f32, w:f32){
        let draw_list = &mut cx.draw_lists[self.draw_list_id];
        if draw_list.redraw_id != self.redraw_id {
            println!("uniform_vec4f called on invalid area pointer, use mark/sweep correctly!");
            return
        }
        let draw_call = &mut draw_list.draw_calls[self.draw_call_id]; 
        let left = draw_call.uniforms.len()&3;
        if left > 0{ // align buffer
            for _ in 0..(4-left){
                draw_call.uniforms.push(0.0);
            }
        }
        draw_call.uniforms.push(x);
        draw_call.uniforms.push(y);
        draw_call.uniforms.push(z);
        draw_call.uniforms.push(w);
    }

    pub fn push_uniform_mat4(&self, cx:&mut Cx, v:&Mat4){
        let draw_list = &mut cx.draw_lists[self.draw_list_id];
        if draw_list.redraw_id != self.redraw_id {
            println!("uniform_mat4 called on invalid area pointer, use mark/sweep correctly!");
            return
        }
        let draw_call = &mut draw_list.draw_calls[self.draw_call_id]; 
        for i in 0..16{
            draw_call.uniforms.push(v.v[i]);
        }
    }
}
