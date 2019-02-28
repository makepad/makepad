use crate::shader::*;
use crate::cx::*;
use crate::cx_shared::*;
use crate::area::*;
use crate::view::*;
use crate::cxturtle::*;
use crate::cxshaders::*;
use crate::animation::*;

#[derive(Clone, Default)]
pub struct CxDrawing{
    pub draw_lists: Vec<DrawList>,
    pub draw_lists_free: Vec<usize>,
    pub instance_area_stack: Vec<Area>,
    pub view_stack: Vec<View>,
    pub current_draw_list_id: usize,
    pub compiled_shaders: Vec<CompiledShader>,
    pub shaders: Vec<Shader>,
    pub dirty_area:Area,
    pub redraw_area:Area,
    pub paint_dirty:bool,
    pub clear_color:Vec4,
    pub frame_id: u64,
    pub animations:Vec<AnimArea>,
    pub ended_animations:Vec<AnimArea>,
}

impl Cx{

    pub fn new_aligned_instance(&mut self, shader_id:usize)->Area{
        //let sh = &self.shaders.compiled_shaders[shader_id];
        //let draw_list_id = self.drawing.current_draw_list_id;
        let area = self.new_instance(shader_id);
        self.turtle.align_list.push(area.clone());
        area/*
        Area::Instance(InstanceArea{
            draw_list_id:draw_list_id,
            draw_call_id:dc.draw_call_id,
            instance_offset:dc.current_instance_offset,
            instance_count:1,
            instance_writer:0
        }));
        dc*/
    }

    fn draw_call_to_area(dc:&DrawCall)->Area{
        Area::Instance(InstanceArea{
            draw_list_id:dc.draw_list_id,
            draw_call_id:dc.draw_call_id,
            instance_offset:dc.current_instance_offset,
            instance_count:1,
            instance_writer:0
        })
    }

    pub fn new_instance(&mut self, shader_id:usize)->Area{
        let sh = &self.drawing.compiled_shaders[shader_id];
        let draw_list = &mut self.drawing.draw_lists[self.drawing.current_draw_list_id];
        
        // find our drawcall in the filled draws
        for i in (0..draw_list.draw_calls_len).rev(){
            if draw_list.draw_calls[i].shader_id == sh.shader_id{
                // reuse this drawcmd.
                let dc = &mut draw_list.draw_calls[i];
                dc.current_instance_offset = dc.instance.len();
                dc.need_uniforms_now = false;
                return dc.get_current_area();
            }
        }

        // we need a new draw
        let id = draw_list.draw_calls_len;
        draw_list.draw_calls_len = draw_list.draw_calls_len + 1;
        
        // see if we need to add a new one
        if id >= draw_list.draw_calls.len(){
            draw_list.draw_calls.push(DrawCall{
                draw_call_id:draw_list.draw_calls.len(),
                draw_list_id:self.drawing.current_draw_list_id,
                sub_list_id:0,
                shader_id:sh.shader_id,
                instance:Vec::new(),
                uniforms:Vec::new(),
                textures:Vec::new(),
                current_instance_offset:0,
                need_uniforms_now:true,
                update_frame_id:self.drawing.frame_id,
                resources:DrawCallResources{..Default::default()}
            });
            let dc = &mut draw_list.draw_calls[id];
            return dc.get_current_area();
        }

        // reuse a draw
        let dc = &mut draw_list.draw_calls[id];
        dc.shader_id = sh.shader_id;
        // truncate buffers and set update frame
        dc.instance.truncate(0);
        dc.current_instance_offset = 0;
        dc.uniforms.truncate(0);
        dc.textures.truncate(0);
        dc.update_frame_id = self.drawing.frame_id;
        dc.need_uniforms_now = true;
        return dc.get_current_area();
    }


    // push instance so it can be written to again in pop_instance
    pub fn begin_instance(&mut self, area:&Area, layout:&Layout){
        self.turtle.begin(layout);
        self.drawing.instance_area_stack.push(area.clone());
    }

    // pops instance patching the supplied geometry in the instancebuffer
    pub fn end_instance(&mut self)->Area{
        let area = self.drawing.instance_area_stack.pop().unwrap();
        let rect = self.turtle.end(&mut self.drawing);
        area.set_rect(&mut self.drawing, &rect);
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
    pub update_frame_id: u64,
    pub resources:DrawCallResources,
    pub need_uniforms_now:bool
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
