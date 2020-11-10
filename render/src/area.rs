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

pub struct InstanceReadRef<'a>{
    pub offset:usize,
    pub slots:usize,
    pub count:usize, 
    pub instances:&'a Vec<f32>
}

pub struct InstanceWriteRef<'a>{
    pub offset:usize,
    pub slots:usize,
    pub count:usize,
    pub instances:&'a mut Vec<f32>
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
    // returns the final screen rect
    pub fn get_rect(&self, cx:&Cx)->Rect{

        return match self{
            Area::Instance(inst)=>{
                if inst.instance_count == 0{
                    println!("get_rect called on instance_count ==0 area pointer, use mark/sweep correctly!");
                    return Rect::default()
                }
                let cxview = &cx.views[inst.view_id];
                if cxview.redraw_id != inst.redraw_id {
                    return Rect::default();
                }
                let draw_call = &cxview.draw_calls[inst.draw_call_id];
                let sh = &cx.shaders[draw_call.shader.shader_id];
                // ok now we have to patch x/y/w/h into it
                if let Some(ix) = sh.mapping.rect_instance_props.x{
                    let x = draw_call.instances[inst.instance_offset + ix];
                    if let Some(iy) = sh.mapping.rect_instance_props.y{
                        let y = draw_call.instances[inst.instance_offset + iy];
                        if let Some(iw) = sh.mapping.rect_instance_props.w{
                            let w = draw_call.instances[inst.instance_offset + iw];
                            if let Some(ih) = sh.mapping.rect_instance_props.h{
                                let h = draw_call.instances[inst.instance_offset + ih];
                                return draw_call.clip_and_scroll_rect(x,y,w,h);
                            }
                        }
                    }
                }
                Rect::default()
            },
            Area::View(view_area)=>{
                let cxview = &cx.views[view_area.view_id];
                Rect{
                    x:cxview.rect.x - cxview.parent_scroll.x,
                    y:cxview.rect.y - cxview.parent_scroll.y,
                    w:cxview.rect.w,
                    h:cxview.rect.h
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
                if let Some(ix) = sh.mapping.rect_instance_props.x{
                    let x = draw_call.instances[inst.instance_offset + ix];
                    if let Some(iy) = sh.mapping.rect_instance_props.y{
                        let y = draw_call.instances[inst.instance_offset + iy];
                        return Vec2{
                            x:abs.x - x + draw_call.draw_uniforms.draw_scroll_x,
                            y:abs.y - y + draw_call.draw_uniforms.draw_scroll_y
                        }
                    }
                }
                abs
            },
            Area::View(view_area)=>{
                let cxview = &cx.views[view_area.view_id];
                return Vec2{
                    x:abs.x - cxview.rect.x + cxview.parent_scroll.x + cxview.unsnapped_scroll.x,
                    y:abs.y - cxview.rect.y - cxview.parent_scroll.y + cxview.unsnapped_scroll.y
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
                
                if let Some(ix) = sh.mapping.rect_instance_props.x{
                    draw_call.instances[inst.instance_offset + ix] = rect.x;
                }
                if let Some(iy) = sh.mapping.rect_instance_props.y{
                    draw_call.instances[inst.instance_offset + iy] = rect.y;
                }
                if let Some(iw) = sh.mapping.rect_instance_props.w{
                    draw_call.instances[inst.instance_offset + iw] = rect.w;
                }
                if let Some(ih) = sh.mapping.rect_instance_props.h{
                    draw_call.instances[inst.instance_offset + ih] = rect.h;
                }
            },
            Area::View(view_area)=>{
                let cxview = &mut cx.views[view_area.view_id];
                cxview.rect = rect.clone()
            },
            _=>()
         }
    }

    pub fn get_instance_offset(&self, cx:&Cx, live_item_id:LiveItemId, ty:Ty)->Option<usize>{
        match self{
            Area::Instance(inst)=>{
                let cxview = &cx.views[inst.view_id];
                let draw_call = &cxview.draw_calls[inst.draw_call_id];
                let sh = &cx.shaders[draw_call.shader.shader_id];

                if let Some(prop_id) = sh.mapping.instance_props.prop_map.get(&live_item_id){
                    let prop = &sh.mapping.instance_props.props[*prop_id];
                    if prop.ty != ty{
                        return None;//panic!("Type wrong of live_id fetch in shader:{}, passed as arg: {}", prop.ty, ty);
                    }
                    return Some(prop.offset)
                }
            }
            _=>(),
        }
        None
    }

    pub fn get_user_uniform_offset(&self, cx:&Cx, live_item_id:LiveItemId, ty:Ty)->Option<usize>{
        match self{
            Area::Instance(inst)=>{
                let cxview = &cx.views[inst.view_id];
                let draw_call = &cxview.draw_calls[inst.draw_call_id];
                let sh = &cx.shaders[draw_call.shader.shader_id];
                if let Some(prop_id) = sh.mapping.user_uniform_props.prop_map.get(&live_item_id){
                    let prop = &sh.mapping.user_uniform_props.props[*prop_id];
                    if prop.ty != ty{
                        return None
                    }
                    return Some(prop.offset)
                }
            }
            _=>(),
        }
        None
    }



    pub fn get_read_ref<'a>(&self, cx:&'a Cx)->Option<InstanceReadRef<'a>>{
        match self{
            Area::Instance(inst)=>{
                let cxview = &cx.views[inst.view_id];
                let draw_call = &cxview.draw_calls[inst.draw_call_id];
                if cxview.redraw_id != inst.redraw_id {
                    println!("get_read_ref alled on invalid area pointer, use mark/sweep correctly!");
                    return None;
                }
                let sh = &cx.shaders[draw_call.shader.shader_id];
                return Some(
                    InstanceReadRef{
                        offset:inst.instance_offset, 
                        count:inst.instance_count, 
                        slots:sh.mapping.instance_props.total_slots,
                        instances:&draw_call.instances
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
                let cxview = &mut cx.views[inst.view_id];
                let draw_call = &mut cxview.draw_calls[inst.draw_call_id];
                if cxview.redraw_id != inst.redraw_id {
                    //println!("get_write_ref called on invalid area pointer, use mark/sweep correctly!");
                    return None;
                }
                let sh = &cx.shaders[draw_call.shader.shader_id];
                cx.passes[cxview.pass_id].paint_dirty = true;
                draw_call.instance_dirty = true;
                return Some(
                    InstanceWriteRef{
                        offset:inst.instance_offset, 
                        count:inst.instance_count, 
                        slots:sh.mapping.instance_props.total_slots,
                        instances:&mut draw_call.instances
                    }
                )
            }
            _=>(),
        }
        return None;
    }

    pub fn get_user_uniforms_write_ref<'a>(&self, cx:&'a mut Cx)->Option<&'a mut Vec<f32>>{
        match self{
            Area::Instance(inst)=>{
                let cxview = &mut cx.views[inst.view_id];
                let draw_call = &mut cxview.draw_calls[inst.draw_call_id];
                if cxview.redraw_id != inst.redraw_id {
                    return None;
                }
                cx.passes[cxview.pass_id].paint_dirty = true;
                draw_call.uniforms_dirty = true;
                return Some(
                    &mut draw_call.user_uniforms
                )
            }
            _=>(),
        }
        return None;
    }

    pub fn write_float(&self, cx:&mut Cx, live_item_id:LiveItemId, value:f32){
        if let Some(inst_offset) = self.get_instance_offset(cx, live_item_id, Ty::Float){
            let write = self.get_write_ref(cx);
            if let Some(write) = write{
                for i in 0..write.count{
                    write.instances[write.offset + inst_offset + i * write.slots] = value;
                }
            }
        }
    }

    pub fn read_float(&self, cx:&Cx, live_item_id:LiveItemId)->f32{
        if let Some(inst_offset) = self.get_instance_offset(cx, live_item_id, Ty::Float){
            let read = self.get_read_ref(cx);
            if let Some(read) = read{
                return read.instances[read.offset + inst_offset]
            }
        }
        0.0
    }

   pub fn write_vec2(&self, cx:&mut Cx, live_item_id:LiveItemId, value:Vec2){
        if let Some(inst_offset) = self.get_instance_offset(cx, live_item_id, Ty::Vec2){
            let write = self.get_write_ref(cx);
            if let Some(write) = write{
                for i in 0..write.count{
                    write.instances[write.offset + inst_offset + 0 + i * write.slots] = value.y;
                    write.instances[write.offset + inst_offset + 1 + i * write.slots] = value.x;
                }
            }
        }
   }

    pub fn read_vec2(&self, cx:&Cx, live_item_id:LiveItemId)->Vec2{
        if let Some(inst_offset) = self.get_instance_offset(cx, live_item_id, Ty::Vec2){
            let read = self.get_read_ref(cx);
            if let Some(read) = read{
                return Vec2{
                    x:read.instances[read.offset + inst_offset + 0],
                    y:read.instances[read.offset + inst_offset + 1]
                }
            }
        }
        Vec2::default()
    }

   pub fn write_vec3(&self, cx:&mut Cx, live_item_id:LiveItemId, value:Vec3){
        if let Some(inst_offset) = self.get_instance_offset(cx, live_item_id, Ty::Vec3){
            let write = self.get_write_ref(cx);
            if let Some(write) = write{
                for i in 0..write.count{
                    write.instances[write.offset + inst_offset + 0 + i * write.slots] = value.y;
                    write.instances[write.offset + inst_offset + 1 + i * write.slots] = value.x;
                    write.instances[write.offset + inst_offset + 2 + i * write.slots] = value.z;
                }
            }
        }
    }

    pub fn read_vec3(&self, cx:&Cx, live_item_id:LiveItemId)->Vec3{
        if let Some(inst_offset) = self.get_instance_offset(cx, live_item_id, Ty::Vec3){
            let read = self.get_read_ref(cx);
            if let Some(read) = read{
                return Vec3{
                    x:read.instances[read.offset + inst_offset + 0],
                    y:read.instances[read.offset + inst_offset + 1],
                    z:read.instances[read.offset + inst_offset + 2]
                }
            }
        }
        Vec3::default()
    }

   pub fn write_vec4(&self, cx:&mut Cx, live_item_id:LiveItemId, value:Vec4){
        if let Some(inst_offset) = self.get_instance_offset(cx, live_item_id, Ty::Vec4){
            let write = self.get_write_ref(cx);
            if let Some(write) = write{
                for i in 0..write.count{
                    write.instances[write.offset + inst_offset + 0 + i * write.slots] = value.x;
                    write.instances[write.offset + inst_offset + 1 + i * write.slots] = value.y;
                    write.instances[write.offset + inst_offset + 2 + i * write.slots] = value.z;
                    write.instances[write.offset + inst_offset + 3 + i * write.slots] = value.w;
                }
            }
        }
   }

    pub fn read_vec4(&self, cx:&Cx, live_item_id:LiveItemId)->Vec4{
        if let Some(inst_offset) = self.get_instance_offset(cx, live_item_id, Ty::Vec4){
            let read = self.get_read_ref(cx);
            if let Some(read) = read{
                return Vec4{
                    x:read.instances[read.offset + inst_offset + 0],
                    y:read.instances[read.offset + inst_offset + 1],
                    z:read.instances[read.offset + inst_offset + 2],
                    w:read.instances[read.offset + inst_offset + 3],
                }
            }
        }
        Vec4::default()
    }

    pub fn write_color(&self, cx:&mut Cx, live_item_id:LiveItemId, value:Color){
        if let Some(inst_offset) = self.get_instance_offset(cx, live_item_id, Ty::Vec4){
            let write = self.get_write_ref(cx);
            if let Some(write) = write{
                for i in 0..write.count{
                    write.instances[write.offset + inst_offset + 0 + i * write.slots] = value.r;
                    write.instances[write.offset + inst_offset + 1 + i * write.slots] = value.g;
                    write.instances[write.offset + inst_offset + 2 + i * write.slots] = value.b;
                    write.instances[write.offset + inst_offset + 3 + i * write.slots] = value.a;
                }
            }
        }
   }

    pub fn read_color(&self, cx:&Cx, live_item_id:LiveItemId)->Color{
        if let Some(inst_offset) = self.get_instance_offset(cx, live_item_id, Ty::Vec4){
            let read = self.get_read_ref(cx);
            if let Some(read) = read{
                return Color{
                    r:read.instances[read.offset + inst_offset + 0],
                    g:read.instances[read.offset + inst_offset + 1],
                    b:read.instances[read.offset + inst_offset + 2],
                    a:read.instances[read.offset + inst_offset + 3],
                }
            }
        }
        Color::default()
    }
    
    pub fn write_mat4(&self, cx:&mut Cx, live_item_id:LiveItemId, value:&Mat4){
        if let Some(inst_offset) = self.get_instance_offset(cx, live_item_id, Ty::Mat4){
            let write = self.get_write_ref(cx);
            if let Some(write) = write{
                for i in 0..write.count{
                    for j in 0..16{
                        write.instances[write.offset + inst_offset + j + i * write.slots] = value.v[j];
                    }
                }
            }
        }
   }

    pub fn read_mat4(&self, cx:&Cx, live_item_id:LiveItemId)->Mat4{
        if let Some(inst_offset) = self.get_instance_offset(cx, live_item_id, Ty::Mat4){
            let read = self.get_read_ref(cx);
            if let Some(read) = read{
                let mut ret = Mat4::default();
                for j in 0..16{
                    ret.v[j] = read.instances[read.offset + inst_offset + j];
                }
                return ret
            }
        }
        Mat4::default()
    }

    pub fn write_uniform_float(&self, cx:&mut Cx, live_item_id:LiveItemId, v:f32){
        if let Some(uni_offset) = self.get_user_uniform_offset(cx, live_item_id, Ty::Float){
            let write = self.get_user_uniforms_write_ref(cx);
            if let Some(write) = write{
                write[uni_offset] = v;
            }
        }
    }
    
    pub fn write_uniform_vec3(&self, cx:&mut Cx, live_item_id:LiveItemId, v:Vec3){
        if let Some(uni_offset) = self.get_user_uniform_offset(cx, live_item_id, Ty::Vec3){
            let write = self.get_user_uniforms_write_ref(cx);
            if let Some(write) = write{
                write[uni_offset+0] = v.x;
                write[uni_offset+1] = v.y;
                write[uni_offset+2] = v.z;
            }
        }
    } 

    pub fn write_texture_2d_id(&self, cx:&mut Cx, live_item_id:LiveItemId, texture_id: usize){
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
    }

    pub fn write_texture_2d(&self, cx:&mut Cx, live_item_id:LiveItemId, texture: Texture){
        self.write_texture_2d_id(cx, live_item_id, texture.texture_id);
    }
}

impl Into<Area> for InstanceArea{
    fn into(self)->Area{
        Area::Instance(self)
    }
}

impl InstanceArea{
    
    pub fn push_slice(&self, cx:&mut Cx, data:&[f32]){
        let cxview = &mut cx.views[self.view_id];
        let draw_call = &mut cxview.draw_calls[self.draw_call_id];
        //let csh = &cx.shaders.compiled_shaders[draw_call.shader_id];
        draw_call.instances.extend_from_slice(data);
    }
    
    pub fn push_last_float(&self, cx:&mut Cx, animator:&Animator, live_item_id:LiveItemId)->f32{
        let ret = animator.last_float(cx, live_item_id);
        self.push_float(cx, ret);
        ret
    }

    pub fn push_float(&self, cx:&mut Cx, value:f32){
        let cxview = &mut cx.views[self.view_id];
        let draw_call = &mut cxview.draw_calls[self.draw_call_id];
        //let csh = &cx.shaders.compiled_shaders[draw_call.shader_id];
        draw_call.instances.push(value);
    }

    pub fn push_last_vec2(&self, cx:&mut Cx, animator:&Animator, live_item_id:LiveItemId)->Vec2{
        let ret =  animator.last_vec2(cx, live_item_id);
        self.push_vec2(cx, ret);
        ret
    }

    pub fn push_vec2(&self, cx:&mut Cx, value:Vec2){
        let cxview = &mut cx.views[self.view_id];
        let draw_call = &mut cxview.draw_calls[self.draw_call_id];
        //let csh = &cx.shaders.compiled_shaders[draw_call.shader_id];
        draw_call.instances.push(value.x);
        draw_call.instances.push(value.y);
    }

    pub fn push_last_vec3(&self, cx:&mut Cx, animator:&Animator, live_item_id:LiveItemId)->Vec3{
        let ret = animator.last_vec3(cx, live_item_id);
        self.push_vec3(cx, ret);
        ret
    }

    pub fn push_vec3(&self, cx:&mut Cx, value:Vec3){
        let cxview = &mut cx.views[self.view_id];
        let draw_call = &mut cxview.draw_calls[self.draw_call_id];
        draw_call.instances.push(value.x);
        draw_call.instances.push(value.y);
        draw_call.instances.push(value.z);
    }

    pub fn push_last_vec4(&self, cx:&mut Cx, animator:&Animator, live_item_id:LiveItemId)->Vec4{
        let ret = animator.last_vec4(cx, live_item_id);
        self.push_vec4(cx, ret);
        ret
    }

    pub fn push_vec4(&self, cx:&mut Cx, value:Vec4){
        let cxview = &mut cx.views[self.view_id];
        let draw_call = &mut cxview.draw_calls[self.draw_call_id];
        draw_call.instances.push(value.x);
        draw_call.instances.push(value.y);
        draw_call.instances.push(value.z);
        draw_call.instances.push(value.w);
    }

    pub fn push_last_color(&self, cx:&mut Cx, animator:&Animator, live_item_id:LiveItemId)->Color{
        let ret = animator.last_color(cx, live_item_id);
        self.push_color(cx, ret);
        ret
    }

    pub fn push_color(&self, cx:&mut Cx, value:Color){
        let cxview = &mut cx.views[self.view_id];
        let draw_call = &mut cxview.draw_calls[self.draw_call_id];
        draw_call.instances.push(value.r);
        draw_call.instances.push(value.g);
        draw_call.instances.push(value.b);
        draw_call.instances.push(value.a);
    }

    pub fn set_do_scroll(&self, cx:&mut Cx, hor:bool, ver:bool){
        let cxview = &mut cx.views[self.view_id];
        let draw_call = &mut cxview.draw_calls[self.draw_call_id];
        draw_call.do_h_scroll = hor;
        draw_call.do_v_scroll = ver;
    }

    pub fn is_first_instance(&self)->bool{
        self.instance_offset == 0
    }

    pub fn get_user_uniform_offset(sh:&CxShader, live_item_id:LiveItemId, ty:Ty)->Option<usize>{
        if let Some(prop_id) = sh.mapping.user_uniform_props.prop_map.get(&live_item_id){
            let prop = &sh.mapping.user_uniform_props.props[*prop_id];
            if prop.ty != ty{
                return None
            }
            return Some(prop.offset)
        }
        None
    }
    
    pub fn write_uniform_float(&self, cx:&mut Cx, live_item_id:LiveItemId, v:f32){
        let cxview = &mut cx.views[self.view_id];
        let draw_call = &mut cxview.draw_calls[self.draw_call_id];
        let sh = &cx.shaders[draw_call.shader.shader_id];

        if let Some(uni_offset) = Self::get_user_uniform_offset(sh, live_item_id, Ty::Float){
            draw_call.user_uniforms[uni_offset] = v;
        }
    }
    
    pub fn get_texture_offset(sh:&CxShader, live_item_id:LiveItemId)->Option<usize>{
        for (index, prop) in sh.mapping.textures.iter().enumerate(){
            if prop.live_item_id == live_item_id{
                return Some(index)
            }
        }
        None
    }
    
    pub fn write_texture_2d_id(&self, cx:&mut Cx, live_item_id:LiveItemId, texture_id: usize){
        let cxview = &mut cx.views[self.view_id];
        let draw_call = &mut cxview.draw_calls[self.draw_call_id];
        let sh = &cx.shaders[draw_call.shader.shader_id];

        if let Some(tex_offset) = Self::get_texture_offset(sh, live_item_id){
            draw_call.textures_2d[tex_offset] = texture_id as u32;
        }
    }

    pub fn write_texture_2d(&self, cx:&mut Cx, live_item_id:LiveItemId, texture: Texture){
        self.write_texture_2d_id(cx, live_item_id, texture.texture_id);
    }
}
