use std::collections::HashMap;

pub use crate::shadergen::*;
pub use crate::cx_fonts::*;
pub use crate::cx_turtle::*;
pub use crate::cx_mouse_cursor::*;
pub use crate::math::*;
pub use crate::events::*;
pub use crate::shader::*;
pub use crate::colors::*;
pub use crate::elements::*;
pub use crate::animator::*;
pub use crate::area::*;
pub use crate::view::*;
pub use crate::style::*;

#[cfg(feature = "ogl")]
pub use crate::cx_ogl::*; 

#[cfg(feature = "mtl")]
pub use crate::cx_mtl::*; 

#[cfg(feature = "webgl")]
pub use crate::cx_webgl::*; 

#[cfg(any(feature = "webgl", feature = "ogl"))]
pub use crate::cx_gl::*; 

#[cfg(any(feature = "mtl", feature = "ogl"))]
pub use crate::cx_winit::*; 

#[derive(Clone)]
pub struct Cx{
    pub title:String,
    pub running:bool,

    pub fonts:Vec<Font>,
    pub textures_2d:Vec<Texture2D>,
    pub uniforms:Vec<f32>,

    pub draw_lists: Vec<DrawList>,
    pub draw_lists_free: Vec<usize>,
    pub instance_area_stack: Vec<Area>,
    pub draw_list_stack: Vec<usize>,
    pub current_draw_list_id: usize,

    pub compiled_shaders: Vec<CompiledShader>,
    pub shaders: Vec<Shader>,
    pub shader_map: HashMap<Shader, usize>,

    pub dirty_area:Area,
    pub redraw_area:Area,
    pub paint_dirty:bool,
    pub clear_color:Vec4,
    pub redraw_id: u64,

    pub turtles:Vec<Turtle>,
    pub align_list:Vec<Area>,
    pub target_size:Vec2,
    pub target_dpi_factor:f32,

    pub down_mouse_cursor:Option<MouseCursor>,
    pub hover_mouse_cursor:Option<MouseCursor>,
    pub captured_fingers:Vec<Area>,
    pub fingers_down:Vec<bool>,

    pub playing_anim_areas:Vec<AnimArea>,
    pub ended_anim_areas:Vec<AnimArea>,

    pub resources:CxResources,

    pub style:StyleSheet,

    pub binary_deps:Vec<BinaryDep>
 }

impl Default for Cx{
    fn default()->Self{
        let mut uniforms = Vec::<f32>::new();
        uniforms.resize(CX_UNI_SIZE, 0.0);
        let mut captured_fingers = Vec::new();
        let mut fingers_down = Vec::new();
        for _i in 0..10{
            captured_fingers.push(Area::Empty);
            fingers_down.push(false);
        }
        Self{
            title:"Hello World".to_string(),
            running: true,

            fonts:Vec::new(),
            textures_2d:Vec::new(),
            uniforms:Vec::new(),

            draw_lists:Vec::new(),
            draw_lists_free:Vec::new(),
            instance_area_stack:Vec::new(),
            draw_list_stack:Vec::new(),
            current_draw_list_id:0,

            compiled_shaders:Vec::new(),
            shaders:Vec::new(),
            shader_map:HashMap::new(),

            dirty_area:Area::All,
            redraw_area:Area::Empty,
            paint_dirty:true,
            clear_color:vec4(0.1,0.1,0.1,1.0),
            redraw_id:1,

            turtles:Vec::new(),
            align_list:Vec::new(),
            target_size:vec2(0.0,0.0),
            target_dpi_factor:0.0,

            down_mouse_cursor:None,
            hover_mouse_cursor:None,
            captured_fingers:captured_fingers,
            fingers_down:fingers_down,

            playing_anim_areas:Vec::new(),
            ended_anim_areas:Vec::new(),

            style: StyleSheet{..Default::default()},

            resources:CxResources{..Default::default()},

            binary_deps:Vec::new()
        }
    }
}

const CX_UNI_CAMERA_PROJECTION:usize = 0;
const CX_UNI_SIZE:usize = 16;

impl Cx{
    pub fn new_shader(&mut self)->Shader{
        let mut sh = Shader{..Default::default()};
        Shader::def_builtins(&mut sh);
        Shader::def_df(&mut sh);
        Cx::def_uniforms(&mut sh);
        DrawList::def_uniforms(&mut sh);
        sh
    }

    pub fn get_shader(&self, id:usize)->&CompiledShader{
        &self.compiled_shaders[id]
    }

    pub fn add_shader(&mut self, sh:Shader)->usize{
        let next_id = self.shaders.len();
        let store_id = self.shader_map.entry(sh.clone()).or_insert(next_id);
        if *store_id == next_id{
            self.shaders.push(sh);
        }
        *store_id
    }

    pub fn def_uniforms(sh: &mut Shader){
        sh.add_ast(shader_ast!({
            let camera_projection:mat4<UniformCx>;
        }));
    }

    pub fn uniform_camera_projection(&mut self, v:Mat4){
        //dump in uniforms
        self.uniforms.resize(CX_UNI_SIZE, 0.0);
        for i in 0..16{
            self.uniforms[CX_UNI_CAMERA_PROJECTION+i] = v.v[i];
        }
    }

    pub fn get_binary_dep(&self, name:&str)->Option<BinaryDep>{
        if let Some(dep) = self.binary_deps.iter().find(|v| v.name == name){
            return Some(dep.clone());
        }
        None
    }

    pub fn new_empty_texture_2d(&mut self)->&mut Texture2D{
        //let id = self.textures.len();
        let id = self.textures_2d.len();
        self.textures_2d.push(
            Texture2D{
                texture_id:id,
                ..Default::default()
            }
        );
        &mut self.textures_2d[id]
    }

    pub fn prepare_frame(&mut self){
        let camera_projection = Mat4::ortho(
                0.0, self.target_size.x, 0.0, self.target_size.y, -100.0, 100.0, 
                1.0,1.0
        );
        self.uniform_camera_projection(camera_projection);
        self.align_list.truncate(0);
    }

    pub fn check_ended_anim_areas(&mut self, time:f64){
        let mut i = 0;
        self.ended_anim_areas.truncate(0);
        loop{
            if i >= self.playing_anim_areas.len(){
                break
            }
            let anim_start_time =self.playing_anim_areas[i].start_time;
            let anim_total_time =self.playing_anim_areas[i].total_time;
            
            if time - anim_start_time >= anim_total_time{
                self.ended_anim_areas.push(self.playing_anim_areas.remove(i));
            }
            else{
                i = i + 1;
            }
        }
    }

    pub fn any_fingers_down(&mut self)->bool{
		for down in &self.fingers_down{
            if *down{
                return true
            }
		}
        return false
    }

    pub fn new_aligned_instance(&mut self, shader_id:usize)->Area{
        let area = self.new_instance(shader_id);
        self.align_list.push(area.clone());
        area
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
        let sh = &self.compiled_shaders[shader_id];
        let draw_list = &mut self.draw_lists[self.current_draw_list_id];
        
        // find our drawcall in the filled draws
        for i in (0..draw_list.draw_calls_len).rev(){
            if draw_list.draw_calls[i].shader_id == sh.shader_id{
                // reuse this drawcmd.
                let dc = &mut draw_list.draw_calls[i];
                dc.current_instance_offset = dc.instance.len();
                let slot_align = dc.instance.len() % sh.instance_slots;
                if slot_align != 0{
                    panic!("Instance offset disaligned! shader: {} misalign: {} slots: {}", shader_id, slot_align, sh.instance_slots);
                }
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
                draw_list_id:self.current_draw_list_id,
                sub_list_id:0,
                shader_id:sh.shader_id,
                instance:Vec::new(),
                uniforms:Vec::new(),
                textures_2d:Vec::new(),
                current_instance_offset:0,
                need_uniforms_now:true,
                instance_dirty:true,
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
        dc.textures_2d.truncate(0);
        dc.instance_dirty = true;
        dc.need_uniforms_now = true;
        return dc.get_current_area();
    }


    // push instance so it can be written to again in pop_instance
    pub fn begin_instance(&mut self, area:Area, layout:&Layout){
        self.begin_turtle(layout);
        self.instance_area_stack.push(area.clone());
    }

    // pops instance patching the supplied geometry in the instancebuffer
    pub fn end_instance(&mut self)->Area{
        let area = self.instance_area_stack.pop().unwrap();
        let rect = self.end_turtle();
        area.set_rect(self, &rect);
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
    pub textures_2d:Vec<u32>,
    pub instance_dirty:bool,
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
const DL_UNI_SCROLL:usize = 0;
const DL_UNI_CLIP:usize = 2;
const DL_UNI_SIZE:usize = 6;

#[derive(Default,Clone)]
pub struct DrawList{
    pub draw_calls:Vec<DrawCall>,
    pub draw_calls_len: usize,
    pub uniforms:Vec<f32>, // cmdlist uniforms
    pub resources:DrawListResources,
    pub rect:Rect,
    pub clipped:bool
}

impl DrawList{
    pub fn initialize(&mut self, clipped:bool){
        self.clipped = clipped;
        self.uniforms.resize(DL_UNI_SIZE, 0.0);
    }

    pub fn set_clipping_uniforms(&mut self){
        if self.clipped{
            self.uniform_draw_list_clip(self.rect.x, self.rect.y, self.rect.x+self.rect.w, self.rect.y+self.rect.h);
        }
        else{
            self.uniform_draw_list_clip(-50000.0,-50000.0,50000.0,50000.0);
        }
    }

    pub fn def_uniforms(sh:&mut Shader){
        sh.add_ast(shader_ast!({
            let draw_list_scroll:vec2<UniformDl>;
            let draw_list_clip:vec4<UniformDl>;
        }));
    }

    pub fn set_scroll_x(&mut self, x:f32){
        self.uniforms[DL_UNI_SCROLL+0] = x;
    }

    pub fn set_scroll_y(&mut self, y:f32){
        self.uniforms[DL_UNI_SCROLL+1] = y;
    }

    pub fn get_scroll(&self)->Vec2{
        return vec2(self.uniforms[DL_UNI_SCROLL+0],self.uniforms[DL_UNI_SCROLL+1])
    }

    pub fn uniform_draw_list_clip(&mut self, min_x:f32, min_y:f32, max_x:f32, max_y:f32){
        
        self.uniforms[DL_UNI_CLIP+0] = min_x;
        self.uniforms[DL_UNI_CLIP+1] = min_y;
        self.uniforms[DL_UNI_CLIP+2] = max_x;
        self.uniforms[DL_UNI_CLIP+3] = max_y;
    }
}

pub trait Style{
    fn style(cx:&mut Cx) -> Self;
}

#[derive(Clone)]
pub struct BinaryDep{
    pub name:String,
    buffer: *const u8,
    pub parse:isize,
    pub length:isize
}

impl BinaryDep{
    pub fn new_from_wasm(name:String, wasm_ptr:u32)->BinaryDep{
        BinaryDep{
            name:name, 
            buffer:wasm_ptr as *const u8,
            parse:8,
            length:unsafe{(wasm_ptr as *const u64).read() as isize}
        }
    }

    pub fn new_from_vec(name:String, vec_ptr:&Vec<u8>)->BinaryDep{
        BinaryDep{
            name:name, 
            buffer:vec_ptr.as_ptr() as *const u8,
            parse:0,
            length:vec_ptr.len() as isize
        }
    }

    pub fn u8(&mut self)->Result<u8, String>{
        if self.parse + 1 > self.length{
            return Err(format!("Eof on u8 file {} offset {}", self.name, self.parse))
        }
        unsafe{
            let ret = self.buffer.offset(self.parse).read();
            self.parse += 1;
            Ok(ret)
        }
    }

    pub fn u16(&mut self)->Result<u16, String>{
        if self.parse+2 > self.length{
            return Err(format!("Eof on u16 file {} offset {}", self.name, self.parse))
        }
        unsafe{
            let ret = (self.buffer.offset(self.parse) as *const u16).read();
            self.parse += 2;
            Ok(ret)
        }
    }

    pub fn u32(&mut self)->Result<u32, String>{
        if self.parse+4 > self.length{
            return Err(format!("Eof on u32 file {} offset {}", self.name, self.parse))
        }
        unsafe{
            let ret = (self.buffer.offset(self.parse) as *const u32).read();
            self.parse += 4;
            Ok(ret)
        }
    }

    pub fn f32(&mut self)->Result<f32, String>{
        if self.parse+4 > self.length{
            return Err(format!("Eof on f32 file {} offset {}", self.name, self.parse))
        }
        unsafe{
            let ret = (self.buffer.offset(self.parse) as *const f32).read();
            self.parse += 4;
            Ok(ret)
        }
    }

    pub fn read(&mut self, out:&mut [u8])->Result<usize, String>{
        let len = out.len();
        if self.parse + len as isize > self.length{
             return Err(format!("Eof on read file {} len {} offset {}", self.name, out.len(), self.parse));
        };
        unsafe{
            for i in 0..len{
                out[i] = self.buffer.offset(self.parse + i as isize).read();
            };
            self.parse += len as isize;
        }
        Ok(len)
    }
}

#[macro_export]
macro_rules! log {
    ($cx:ident, $($arg:expr),+) => {
        $cx.log(&format!("[{}:{}] {}\n",file!(),line!(),&format!($($arg),+)))
    };
}

#[macro_export]
macro_rules! main_app {
    ($app:ident, $name:expr) => {
        //TODO do this with a macro to generate both entrypoints for App and Cx
        pub fn main() {
            let mut cx = Cx{
                title:$name.to_string(),
                ..Default::default()
            };

            let mut app = $app{
                ..Style::style(&mut cx)
            };

            cx.event_loop(|cx, mut event|{
                if let Event::Redraw = event{return app.draw_app(cx);}
                app.handle_app(cx, &mut event);
            });
        }

        #[export_name = "create_wasm_app"]
        pub extern "C" fn create_wasm_app()->u32{
            let mut cx = Box::new(
                Cx{
                    title:$name.to_string(),
                    ..Default::default()
                }
            );
            let app = Box::new(
                $app{
                    ..Style::style(&mut cx)
                }
            );
            Box::into_raw(Box::new((Box::into_raw(app),Box::into_raw(cx)))) as u32
        }

        #[export_name = "process_to_wasm"]
        pub unsafe extern "C" fn process_to_wasm(appcx:u32, msg:u32)->u32{
            let appcx = &*(appcx as *mut (*mut $app,*mut Cx));
            (*appcx.1).process_to_wasm(msg,|cx, mut event|{
                if let Event::Redraw = event{return (*appcx.0).draw_app(cx);}
                (*appcx.0).handle_app(cx, &mut event);
            })
        }
    };
}