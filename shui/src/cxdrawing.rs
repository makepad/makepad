use crate::shader::*;
use crate::cx::*;
use crate::cx_shared::*;
use crate::cxturtle::*;
use crate::area::*;
use crate::view::*;
use crate::cxshaders::*;

#[derive(Clone, Default)]
pub struct CxDrawing{
    pub draw_lists: Vec<DrawList>,
    pub draw_lists_free: Vec<usize>,
    pub instance_area_stack: Vec<Area>,
    pub view_stack: Vec<View>,
    pub draw_list_id: usize,
    pub frame_id: usize
}

impl CxDrawing{
    pub fn draw_list(&mut self)->&mut DrawList{
        &mut self.draw_lists[self.draw_list_id]
    }

    pub fn instance_aligned(&mut self, sh:&CompiledShader, turtle:&mut CxTurtle)->&mut DrawCall{
        let draw_list_id = self.draw_list_id;
        let dc = self.instance(sh);
        turtle.align_list.push(Area::Instance(InstanceArea{
            draw_list_id:draw_list_id,
            draw_call_id:dc.draw_call_id,
            instance_offset:dc.current_instance_offset,
            instance_count:1,
            instance_writer:0
        }));
        dc
    }

    pub fn instance(&mut self, sh:&CompiledShader)->&mut DrawCall{
        let draw_list = &mut self.draw_lists[self.draw_list_id];
        
        // find our drawcall in the filled draws
        for i in (0..draw_list.draw_calls_len).rev(){
            if draw_list.draw_calls[i].shader_id == sh.shader_id{
                // reuse this drawcmd.
                let dc = &mut draw_list.draw_calls[i];
                dc.current_instance_offset = dc.instance.len();
                dc.first = false;
                return dc
            }
        }

        // we need a new draw
        let id = draw_list.draw_calls_len;
        draw_list.draw_calls_len = draw_list.draw_calls_len + 1;
        
        // see if we need to add a new one
        if id >= draw_list.draw_calls.len(){
            draw_list.draw_calls.push(DrawCall{
                draw_call_id:draw_list.draw_calls.len(),
                draw_list_id:self.draw_list_id,
                sub_list_id:0,
                shader_id:sh.shader_id,
                instance:Vec::new(),
                uniforms:Vec::new(),
                textures:Vec::new(),
                current_instance_offset:0,
                first:true,
                update_frame_id:self.frame_id,
                resources:DrawCallResources{..Default::default()}
            });
            
            return &mut draw_list.draw_calls[id]
        }

        // reuse a draw
        let dc = &mut draw_list.draw_calls[id];
        dc.shader_id = sh.shader_id;
        // truncate buffers and set update frame
        dc.instance.truncate(0);
        dc.current_instance_offset = 0;
        dc.uniforms.truncate(0);
        dc.textures.truncate(0);
        dc.update_frame_id = self.frame_id;
        dc.first = true;
        dc
    }

    // push instance so it can be written to again in pop_instance
    pub fn push_instance(&mut self, draw_call_id:usize)->&mut DrawCall{
        let draw_list = &mut self.draw_lists[self.draw_list_id];
        let draw_call = &mut draw_list.draw_calls[draw_call_id];

        // store our current instance properties so we can update-patch it in pop instance
        self.instance_area_stack.push(Area::Instance(InstanceArea{
            draw_list_id: self.draw_list_id,
            draw_call_id:draw_call_id,
            instance_offset:draw_call.current_instance_offset,
            instance_count:1,
            instance_writer:0
        }));
        draw_call
    }

    // pops instance patching the supplied geometry in the instancebuffer
    pub fn pop_instance(&mut self, shaders:&CxShaders, geom:Rect)->Area{
        let area = self.instance_area_stack.pop().unwrap();
        area.set_rect_sep(self, shaders, &geom);
        area
    }
}

#[derive(Default,Clone)]
pub struct DrawCall{
    pub draw_call_id:usize,
    pub draw_list_id:usize,
    pub sub_list_id:usize, // if not 0, its a subnode
    pub shader_id:usize, // if shader_id changed, delete gl vao
    pub instance:Vec<f32>,
    pub current_instance_offset:usize, // offset of current instance
    pub uniforms:Vec<f32>,  // draw uniforms
    pub textures:Vec<u32>,
    pub update_frame_id: usize,
    pub resources:DrawCallResources,
    pub first:bool
}

impl DrawCall{

    pub fn get_current_area(&self)->Area{
        Area::Instance(InstanceArea{
            draw_list_id:self.draw_list_id,
            draw_call_id:self.draw_call_id,
            instance_offset:self.current_instance_offset,
            instance_count:1,
            instance_writer:0
        })
    }

    pub fn float(&mut self, _name: &str, v:f32){
        self.instance.push(v);
    }

    pub fn rect(&mut self, _name: &str, rect:Rect){
        self.instance.push(rect.x);
        self.instance.push(rect.y);
        self.instance.push(rect.w);
        self.instance.push(rect.h);
    }

    pub fn vec2f(&mut self, _name: &str, x:f32, y:f32){
        self.instance.push(x);
        self.instance.push(y);
    }

    pub fn vec3f(&mut self, _name: &str, x:f32, y:f32, z:f32){
        self.instance.push(x);
        self.instance.push(y);
        self.instance.push(z);
    }

    pub fn vec4f(&mut self, _name: &str, x:f32, y:f32, z:f32, w:f32){
        self.instance.push(x);
        self.instance.push(y);
        self.instance.push(z);
        self.instance.push(w);
    }

    pub fn vec2(&mut self, _name: &str, v:&Vec2){
        self.instance.push(v.x);
        self.instance.push(v.y);
    }

    pub fn vec3(&mut self, _name: &str, v:&Vec3){
        self.instance.push(v.x);
        self.instance.push(v.y);
        self.instance.push(v.z);
    }

    pub fn vec4(&mut self, _name: &str, v:&Vec4){
        self.instance.push(v.x);
        self.instance.push(v.y);
        self.instance.push(v.z);
        self.instance.push(v.w);
    }

    pub fn texture(&mut self, _name: &str, texture_id: usize){
        // how do we store these?
        self.textures.push(texture_id as u32);
    }

    pub fn ufloat(&mut self, _name: &str, v:f32){
        self.uniforms.push(v);
    }

    pub fn uvec2f(&mut self, _name: &str, x:f32, y:f32){
        self.uniforms.push(x);
        self.uniforms.push(y);
    }

    pub fn uvec3f(&mut self, _name: &str, x:f32, y:f32, z:f32){
        self.uniforms.push(x);
        self.uniforms.push(y);
        self.uniforms.push(z);
    }

    pub fn uvec4f(&mut self, _name: &str, x:f32, y:f32, z:f32, w:f32){
        self.uniforms.push(x);
        self.uniforms.push(y);
        self.uniforms.push(z);
        self.uniforms.push(w);
    }

    pub fn uvec2(&mut self, _name: &str, v:&Vec2){
        self.uniforms.push(v.x);
        self.uniforms.push(v.y);
    }

    pub fn uvec3(&mut self, _name: &str, v:&Vec3){
        self.uniforms.push(v.x);
        self.uniforms.push(v.y);
        self.uniforms.push(v.z);
    }

    pub fn uvec4(&mut self, _name: &str, v:&Vec4){
        self.uniforms.push(v.x);
        self.uniforms.push(v.y);
        self.uniforms.push(v.z);
        self.uniforms.push(v.w);
    }

    pub fn umat4(&mut self, _name: &str, v:&Mat4){
        for i in 0..16{
            self.uniforms.push(v.v[i]);
        }
    }
}

// CX and DL uniforms
const DL_UNI_PROP2:usize = 0;
const DL_UNI_SIZE:usize = 1;

#[derive(Default,Clone)]
pub struct DrawList{
    pub draw_calls:Vec<DrawCall>,
    pub draw_calls_len: usize,
    pub uniforms:Vec<f32>, // cmdlist uniforms
    pub resources:DrawListResources,
    pub rect:Rect
}

impl DrawList{
    pub fn initialize(&mut self){
        self.uniforms.resize(DL_UNI_SIZE, 0.0);
    }
    
    pub fn def_uniforms(_sh:&mut Shader){
        //sh.dl_uniform("prop2", Kind::Float);
    }

    pub fn uniform_prop2(&mut self, v:f32){
        self.uniforms[DL_UNI_PROP2] = v;
    }
}

pub trait Style{
    fn style(cx:&mut Cx) -> Self;
}
