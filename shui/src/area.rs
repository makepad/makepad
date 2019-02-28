
use crate::math::*;
use crate::cx_shared::*;
use crate::cxdrawing::*;
use crate::cxshaders::*;

#[derive(Clone, Default, Debug, PartialEq)]
pub struct InstanceArea{
    pub draw_list_id:usize,
    pub draw_call_id:usize,
    pub instance_offset:usize,
    pub instance_count:usize,
    pub instance_writer:usize
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct DrawListArea{
    pub draw_list_id:usize
}

#[derive(Clone, Debug, PartialEq)]
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

    pub fn get_rect_sep(&self, drawing:&CxDrawing, shaders:&CxShaders)->Rect{
        return match self{
            Area::Instance(inst)=>{
                if inst.instance_count == 0{
                    return Rect::zero()
                }
                let draw_list = &drawing.draw_lists[inst.draw_list_id];
                let draw_call = &draw_list.draw_calls[inst.draw_call_id];
                let csh = &shaders.compiled_shaders[draw_call.shader_id];
                // ok now we have to patch x/y/w/h into it
                if let Some(ix) = csh.rect_instance_props.x{
                    let x = draw_call.instance[inst.instance_offset + ix];
                    if let Some(iy) = csh.rect_instance_props.y{
                        let y = draw_call.instance[inst.instance_offset + iy];
                        if let Some(iw) = csh.rect_instance_props.w{
                            let w = draw_call.instance[inst.instance_offset + iw];
                            if let Some(ih) = csh.rect_instance_props.h{
                                let h = draw_call.instance[inst.instance_offset + ih];
                                return Rect{x:x,y:y,w:w,h:h}
                            }
                        }
                    }
                }
                Rect::zero()
            },
            Area::DrawList(draw_list_area)=>{
                let draw_list = &drawing.draw_lists[draw_list_area.draw_list_id];
                draw_list.rect.clone()
            },
            _=>Rect::zero(),
        }
    }

    pub fn get_rect(&self, cx:&Cx)->Rect{
        self.get_rect_sep(&cx.drawing, &cx.shaders)
    }

    pub fn set_rect_sep(&self, drawing:&mut CxDrawing, shaders:&CxShaders, rect:&Rect){
         match self{
            Area::Instance(inst)=>{
                let draw_list = &mut drawing.draw_lists[inst.draw_list_id];
                let draw_call = &mut draw_list.draw_calls[inst.draw_call_id];
                let csh = &shaders.compiled_shaders[draw_call.shader_id];        // ok now we have to patch x/y/w/h into it

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
                let draw_list = &mut drawing.draw_lists[draw_list_area.draw_list_id];
                draw_list.rect = rect.clone()
            },
            _=>()
         }
    }

    pub fn move_xy(&self, dx:f32, dy:f32, drawing:&mut CxDrawing, shaders:&CxShaders){
        return match self{
            Area::Instance(inst)=>{
                if inst.instance_count == 0{
                    return;
                }
                let draw_list = &mut drawing.draw_lists[inst.draw_list_id];
                let draw_call = &mut draw_list.draw_calls[inst.draw_call_id];
                let csh = &shaders.compiled_shaders[draw_call.shader_id];

                for i in 0..inst.instance_count{
                    if let Some(x) = csh.rect_instance_props.x{
                        draw_call.instance[inst.instance_offset + x + i * csh.instance_slots] += dx;
                    }
                    if let Some(y) = csh.rect_instance_props.y{
                        draw_call.instance[inst.instance_offset + y+ i * csh.instance_slots] += dy;
                    }
                }
            }
            _=>(),
        }
    }

    pub fn write_float(&self, cx:&mut Cx, prop_name:&str, value:f32){
        match self{
            Area::Instance(inst)=>{
                let draw_list = &mut cx.drawing.draw_lists[inst.draw_list_id];
                let draw_call = &mut draw_list.draw_calls[inst.draw_call_id];
                let csh = &cx.shaders.compiled_shaders[draw_call.shader_id];

                for prop in &csh.named_instance_props.props{
                    if prop.name == prop_name{
                        cx.paint_dirty = true;
                        draw_call.instance[inst.instance_offset + prop.offset] = value;
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
                let draw_list = &cx.drawing.draw_lists[inst.draw_list_id];
                let draw_call = &draw_list.draw_calls[inst.draw_call_id];
                let csh = &cx.shaders.compiled_shaders[draw_call.shader_id];

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
                let draw_list = &mut cx.drawing.draw_lists[inst.draw_list_id];
                let draw_call = &mut draw_list.draw_calls[inst.draw_call_id];
                let csh = &cx.shaders.compiled_shaders[draw_call.shader_id];

                for prop in &csh.named_instance_props.props{
                    if prop.name == prop_name{
                        cx.paint_dirty = true;
                        let off = inst.instance_offset + prop.offset;
                        draw_call.instance[off + 0] = value.x;
                        draw_call.instance[off + 1] = value.y;
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
                let draw_list = &cx.drawing.draw_lists[inst.draw_list_id];
                let draw_call = &draw_list.draw_calls[inst.draw_call_id];
                let csh = &cx.shaders.compiled_shaders[draw_call.shader_id];

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
                let draw_list = &mut cx.drawing.draw_lists[inst.draw_list_id];
                let draw_call = &mut draw_list.draw_calls[inst.draw_call_id];
                let csh = &cx.shaders.compiled_shaders[draw_call.shader_id];

                for prop in &csh.named_instance_props.props{
                    if prop.name == prop_name{
                        cx.paint_dirty = true;
                        let off = inst.instance_offset + prop.offset;
                        draw_call.instance[off + 0] = value.x;
                        draw_call.instance[off + 1] = value.y;
                        draw_call.instance[off + 2] = value.z;
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
                let draw_list = &cx.drawing.draw_lists[inst.draw_list_id];
                let draw_call = &draw_list.draw_calls[inst.draw_call_id];
                let csh = &cx.shaders.compiled_shaders[draw_call.shader_id];

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
                let draw_list = &mut cx.drawing.draw_lists[inst.draw_list_id];
                let draw_call = &mut draw_list.draw_calls[inst.draw_call_id];
                let csh = &cx.shaders.compiled_shaders[draw_call.shader_id];

                for prop in &csh.named_instance_props.props{
                    if prop.name == prop_name{
                        cx.paint_dirty = true;
                        let off = inst.instance_offset + prop.offset;
                        draw_call.instance[off + 0] = value.x;
                        draw_call.instance[off + 1] = value.y;
                        draw_call.instance[off + 2] = value.z;
                        draw_call.instance[off + 3] = value.w;
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
                let draw_list = &cx.drawing.draw_lists[inst.draw_list_id];
                let draw_call = &draw_list.draw_calls[inst.draw_call_id];
                let csh = &cx.shaders.compiled_shaders[draw_call.shader_id];

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

    pub fn set_rect(&self, cx:&mut Cx, rect:&Rect){
        self.set_rect_sep(&mut cx.drawing, &cx.shaders, rect)
    }

    pub fn contains(&self, x:f32, y:f32, cx:&Cx)->bool{
        let rect = self.get_rect(cx);

        return x >= rect.x && x <= rect.x + rect.w &&
            y >= rect.y && y <= rect.y + rect.h;
    }
}
