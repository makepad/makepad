use std::collections::HashMap;
use std::collections::BTreeMap;

pub use crate::shadergen::*;
pub use crate::cx_fonts::*;
pub use crate::cx_turtle::*;
pub use crate::cx_cursor::*;
pub use crate::math::*;
pub use crate::events::*;
pub use crate::shader::*;
pub use crate::colors::*;
pub use crate::elements::*;
pub use crate::animator::*;
pub use crate::area::*;
pub use crate::view::*;

#[cfg(feature = "ogl")]
pub use crate::cx_ogl::*; 

#[cfg(feature = "mtl")]
pub use crate::cx_mtl::*; 

#[cfg(feature = "webgl")]
pub use crate::cx_webgl::*; 

#[cfg(any(feature = "webgl", feature = "ogl"))]
pub use crate::cx_gl::*; 

#[cfg(any(feature = "ogl", feature="mtl"))]
pub use crate::cx_desktop::*; 

#[derive(Clone)]
pub struct Cx{
    pub title:String,
    pub running:bool,

    pub fonts:Vec<Font>,
    pub textures_2d:Vec<Texture2D>,
    pub uniforms:Vec<f32>,

    pub draw_lists: Vec<DrawList>,
    pub draw_lists_free: Vec<usize>,
    pub draw_list_stack: Vec<usize>,
    pub current_draw_list_id: usize,

    pub compiled_shaders: Vec<CompiledShader>,
    pub shaders: Vec<Shader>,
    pub shader_map: HashMap<Shader, usize>,

    pub redraw_areas:Vec<Area>,
    pub incr_areas:Vec<Area>,
    pub paint_dirty:bool,
    pub clear_color:Vec4,
    pub redraw_id: u64,
    pub event_id: u64,
    pub is_in_redraw_cycle:bool,

    pub last_key_focus:Area,
    pub key_focus:Area,

    pub debug_area:Area,

    pub turtles:Vec<Turtle>,
    pub align_list:Vec<Area>,
    pub target_size:Vec2,
    pub target_dpi_factor:f32,

    pub down_mouse_cursor:Option<MouseCursor>,
    pub hover_mouse_cursor:Option<MouseCursor>,
    pub captured_fingers:Vec<Area>,

    pub user_events:Vec<Event>,

    pub playing_anim_areas:Vec<AnimArea>,
    pub ended_anim_areas:Vec<AnimArea>,

    pub platform:CxPlatform,

    pub style_values:BTreeMap<String, StyleValue>,

    pub binary_deps:Vec<BinaryDep>
 }

impl Default for Cx{
    fn default()->Self{
        let mut uniforms = Vec::<f32>::new();
        uniforms.resize(CX_UNI_SIZE, 0.0);
        let mut captured_fingers = Vec::new();

        for _i in 0..10{
            captured_fingers.push(Area::Empty);
        }

        Self{
            title:"Hello World".to_string(),
            running: true,

            fonts:Vec::new(),
            textures_2d:Vec::new(),
            uniforms:Vec::new(),

            draw_lists:Vec::new(),
            draw_lists_free:Vec::new(),
            draw_list_stack:Vec::new(),
            current_draw_list_id:0,

            compiled_shaders:Vec::new(),
            shaders:Vec::new(),
            shader_map:HashMap::new(),

            redraw_areas:Vec::new(),
            incr_areas:Vec::new(),
            paint_dirty:false,
            clear_color:vec4(0.1,0.1,0.1,1.0),
            redraw_id:1,
            event_id:1,
            is_in_redraw_cycle:false,
            turtles:Vec::new(),
            align_list:Vec::new(),
            target_size:vec2(0.0,0.0),
            target_dpi_factor:0.0,
            
            last_key_focus:Area::Empty,
            key_focus:Area::Empty,

            debug_area:Area::Empty,

            down_mouse_cursor:None,
            hover_mouse_cursor:None,
            captured_fingers:captured_fingers,

            user_events:Vec::new(),

            style_values:BTreeMap::new(),

            playing_anim_areas:Vec::new(),
            ended_anim_areas:Vec::new(),

            platform:CxPlatform{..Default::default()},

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

    pub fn add_shader(&mut self, sh:Shader, name:&str)->usize{
        let next_id = self.shaders.len();
        let store_id = self.shader_map.entry(sh.clone()).or_insert(next_id);
        if *store_id == next_id{
            self.shaders.push(Shader{
                name:name.to_string(),
                ..sh
            });
        }
        *store_id
    }

    pub fn def_uniforms(sh: &mut Shader){
        sh.add_ast(shader_ast!({
            let camera_projection:mat4<UniformCx>;
        }));
    }

    pub fn redraw_area(&mut self, area:Area){
        // if we are redrawing all, clear the rest
        if area == Area::All{
            self.redraw_areas.truncate(0);
        }
        // check if we are already redrawing all
        else if self.redraw_areas.len() == 1 &&  self.redraw_areas[0] == Area::All{
            return;
        };
        // only add it if we dont have it already
        if let Some(_) = self.redraw_areas.iter().position(|a| *a == area){
            return;
        }
        self.redraw_areas.push(area);
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
            if  anim_start_time.is_nan() || time - anim_start_time >= anim_total_time{
                self.ended_anim_areas.push(self.playing_anim_areas.remove(i));
            }
            else{
                i = i + 1;
            }
        }
    }

    pub fn new_aligned_instance(&mut self, shader_id:usize)->AlignedInstance{
        let instance_area = self.new_instance(shader_id);
        let align_index = self.align_list.len();
        self.align_list.push(Area::Instance(instance_area.clone()));
        AlignedInstance{
            inst:instance_area,
            index:align_index
        }
    }

    pub fn update_aligned_instance_count(&mut self,align:&AlignedInstance){
        if let Area::Instance(instance) = &mut self.align_list[align.index]{
            instance.instance_count = align.inst.instance_count;
        }
    }
/*
    fn draw_call_to_area(dc:&DrawCall)->Area{
        Area::Instance(InstanceArea{
            draw_list_id:dc.draw_list_id,
            draw_call_id:dc.draw_call_id,
            redraw_id:dc.
            instance_offset:dc.current_instance_offset,
            instance_count:1
        })
    }*/

    pub fn new_instance(&mut self, shader_id:usize)->InstanceArea{
        if !self.is_in_redraw_cycle{
            panic!("calling get_instance outside of redraw cycle is not possible!");
        }

        let sh = &self.compiled_shaders[shader_id];
        let draw_list = &mut self.draw_lists[self.current_draw_list_id];
        
        // find our drawcall in the filled draws
        for i in (0..draw_list.draw_calls_len).rev(){
            let dc = &mut draw_list.draw_calls[i];
            if dc.sub_list_id == 0 && dc.shader_id == sh.shader_id{
                // reuse this drawcmd and add an instance
                dc.current_instance_offset = dc.instance.len();
                let slot_align = dc.instance.len() % sh.instance_slots;
                if slot_align != 0{
                    panic!("Instance offset disaligned! shader: {} misalign: {} slots: {}", shader_id, slot_align, sh.instance_slots);
                }
                dc.need_uniforms_now = false;
                
                return dc.get_current_instance_area();
            }
        }

        // we need a new draw
        let draw_call_id = draw_list.draw_calls_len;
        draw_list.draw_calls_len = draw_list.draw_calls_len + 1;
        
        // see if we need to add a new one
        if draw_call_id >= draw_list.draw_calls.len(){
            draw_list.draw_calls.push(DrawCall{
                draw_call_id:draw_call_id,
                draw_list_id:self.current_draw_list_id,
                redraw_id:self.redraw_id,
                sub_list_id:0,
                shader_id:sh.shader_id,
                instance:Vec::new(),
                uniforms:Vec::new(),
                textures_2d:Vec::new(),
                current_instance_offset:0,
                need_uniforms_now:true,
                instance_dirty:true,
                platform:DrawCallPlatform{..Default::default()}
            });
            let dc = &mut draw_list.draw_calls[draw_call_id];
            return dc.get_current_instance_area();
        }

        // reuse a draw
        let dc = &mut draw_list.draw_calls[draw_call_id];
        dc.shader_id = sh.shader_id;
        dc.sub_list_id = 0; // make sure its recognised as a draw call
        // truncate buffers and set update frame
        dc.redraw_id = self.redraw_id;
        dc.instance.truncate(0);
        dc.current_instance_offset = 0;
        dc.uniforms.truncate(0);
        dc.textures_2d.truncate(0);
        dc.instance_dirty = true;
        dc.need_uniforms_now = true;
        return dc.get_current_instance_area();
    }

    pub fn color(&self, name:&str)->Vec4{
        if let Some(StyleValue::Color(val)) = self.style_values.get(name){
            return *val;
        }
        panic!("Cannot find style color key {}", name);
    }

    pub fn font(&self, name:&str)->String{
        if let Some(StyleValue::Font(val)) = self.style_values.get(name){
            return val.clone();
        }
        panic!("Cannot find style font key {}", name);
    }

    pub fn size(&self, name:&str)->f64{
        if let Some(StyleValue::Size(val)) = self.style_values.get(name){
            return *val;
        }
        panic!("Cannot find style size key {}", name);
    }

    pub fn set_color(&mut self, name:&str, val:Vec4){
        self.style_values.insert(name.to_string(), StyleValue::Color(val));
    }

    pub fn set_font(&mut self, name:&str, val:&str){
        self.style_values.insert(name.to_string(), StyleValue::Font(val.to_string()));
    }

    pub fn set_size(&mut self, name:&str, val:f64){
        self.style_values.insert(name.to_string(), StyleValue::Size(val));
    }

    pub fn set_key_focus(&mut self, focus_area:Area){
        self.key_focus = focus_area;
    }


    // event handler wrappers

    pub fn call_event_handler<F>(&mut self, mut event_handler:F, event:&mut Event)
    where F: FnMut(&mut Cx, &mut Event)
    { 
        self.event_id += 1;
        event_handler(self, event);

        if self.last_key_focus != self.key_focus{
            let last_key_focus = self.last_key_focus;
            self.last_key_focus = self.key_focus;
            event_handler(self, &mut Event::KeyFocus(KeyFocusEvent{
                is_lost:false,
                last:last_key_focus,
                focus:self.key_focus
            }))
        }

        // check any user events and send them
        if self.user_events.len() > 0{
            let user_events = self.user_events.clone();
            self.user_events.truncate(0);
            for mut user_event in user_events{
                event_handler(self, &mut user_event);
            }
        }
    }

    pub fn call_draw_event<F, T>(&mut self, mut event_handler:F, root_view:&mut View<T>)
    where F: FnMut(&mut Cx, &mut Event), T: ScrollBarLike<T> + Clone + ElementLife
    { 
        //for i in 0..10{
        self.is_in_redraw_cycle = true;
        self.redraw_id += 1;
        root_view.begin_view(self, &Layout{..Default::default()});
        self.incr_areas = self.redraw_areas.clone();
        self.redraw_areas.truncate(0);
        self.call_event_handler(&mut event_handler, &mut Event::Draw);
        root_view.end_view(self);
        self.is_in_redraw_cycle = false;
        //}
    }

    pub fn call_animation_event<F>(&mut self, mut event_handler:F, time:f64)
    where F: FnMut(&mut Cx, &mut Event)
    { 
        self.call_event_handler(&mut event_handler, &mut Event::Animate(AnimateEvent{time:time}));
        self.check_ended_anim_areas(time);
        if self.ended_anim_areas.len() > 0{
            self.call_event_handler(&mut event_handler, &mut Event::AnimationEnded(AnimateEvent{time:time}));
        }
    }

    pub fn debug_draw_tree_recur(&mut self, draw_list_id: usize, depth:usize){
        if draw_list_id >= self.draw_lists.len(){
            println!("---------- Drawlist still empty ---------");
            return
        }
        let mut indent = String::new();
        for _i in 0..depth{
            indent.push_str("  ");
        }
        let draw_calls_len = self.draw_lists[draw_list_id].draw_calls_len;
        if draw_list_id == 0{
            println!("---------- Begin Debug draw tree for redraw_id: {} ---------", self.redraw_id)
        }
        println!("{}list {}: len:{} rect:{:?}", indent, draw_list_id, draw_calls_len, self.draw_lists[draw_list_id].rect);        
        indent.push_str("  ");
        for draw_call_id in 0..draw_calls_len{
            let sub_list_id = self.draw_lists[draw_list_id].draw_calls[draw_call_id].sub_list_id;
            if sub_list_id != 0{
                self.debug_draw_tree_recur(sub_list_id, depth + 1);
            }
            else{
                let draw_list = &mut self.draw_lists[draw_list_id];
                let draw_call = &mut draw_list.draw_calls[draw_call_id];
                let sh = &self.shaders[draw_call.shader_id];
                let shc = &self.compiled_shaders[draw_call.shader_id];
                let slots = shc.instance_slots;
                let instances = draw_call.instance.len() / slots;
                println!("{}call {}: {}({}) x:{}", indent, draw_call_id, sh.name, draw_call.shader_id, instances);        
                // lets dump the instance geometry
                for inst in 0..instances.min(1){
                    let mut out = String::new();
                    let mut off = 0;
                    for prop in &shc.named_instance_props.props{
                        match prop.slots{
                            1=>out.push_str(&format!("{}:{} ", prop.name, 
                                draw_call.instance[inst*slots + off])),
                            2=>out.push_str(&format!("{}:v2({},{}) ", prop.name, 
                                draw_call.instance[inst*slots+ off], 
                                draw_call.instance[inst*slots+1+ off])),
                            3=>out.push_str(&format!("{}:v3({},{},{}) ", prop.name, 
                                draw_call.instance[inst*slots+ off], 
                                draw_call.instance[inst*slots+1+ off], 
                                draw_call.instance[inst*slots+1+ off])),
                            4=>out.push_str(&format!("{}:v4({},{},{},{}) ", prop.name, 
                                draw_call.instance[inst*slots+ off], 
                                draw_call.instance[inst*slots+1+ off], 
                                draw_call.instance[inst*slots+2+ off], 
                                draw_call.instance[inst*slots+3+ off])),
                            _=>{}
                        }
                        off += prop.slots;
                    }
                    println!("  {}instance {}: {}", indent, inst, out);        
                }
            }
        }
        if draw_list_id == 0{
            println!("---------- End Debug draw tree for redraw_id: {} ---------", self.redraw_id)
        }
    }
}

#[derive(Clone)]
pub struct AlignedInstance{
    pub inst:InstanceArea,
    pub index:usize
}

#[derive(Clone)]
pub enum StyleValue{
    Color(Vec4),
    Font(String),
    Size(f64)
}

#[derive(Default,Clone)]
pub struct DrawCall{
    pub draw_call_id:usize,
    pub draw_list_id:usize,
    pub redraw_id:u64,
    pub sub_list_id:usize, // if not 0, its a subnode
    pub shader_id:usize, // if shader_id changed, delete gl vao
    pub instance:Vec<f32>,
    pub current_instance_offset:usize, // offset of current instance
    pub uniforms:Vec<f32>,  // draw uniforms
    pub textures_2d:Vec<u32>,
    pub instance_dirty:bool,
    pub platform:DrawCallPlatform,
    pub need_uniforms_now:bool
}

impl DrawCall{

    pub fn get_current_instance_area(&self)->InstanceArea{
        InstanceArea{
            draw_list_id:self.draw_list_id,
            draw_call_id:self.draw_call_id,
            redraw_id:self.redraw_id,
            instance_offset:self.current_instance_offset,
            instance_count:1
        }
    }
}

// CX and DL uniforms
const DL_UNI_SCROLL:usize = 0;
const DL_UNI_CLIP:usize = 2;
const DL_UNI_SIZE:usize = 6;

#[derive(Default,Clone)]
pub struct DrawList{
    pub nesting_draw_list_id:usize, // the id of the parent we nest in, codeflow wise
    pub redraw_id:u64,
    pub draw_calls:Vec<DrawCall>,
    pub draw_calls_len: usize,
    pub uniforms:Vec<f32>, // cmdlist uniforms
    pub platform:DrawListPlatform,
    pub rect:Rect,
    pub clipped:bool
}

impl DrawList{
    pub fn initialize(&mut self, clipped:bool, redraw_id:u64){
        self.clipped = clipped;
        self.redraw_id = redraw_id;
        self.uniforms.resize(DL_UNI_SIZE, 0.0);
    }

    pub fn set_clipping_uniforms(&mut self){
        if self.clipped{
            //println!("SET CLIPPING {} {} {} {} {}", self.draw_list_id, self.rect.x, self.rect.y, self.rect.x+self.rect.w, self.rect.y+self.rect.h);
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

    pub fn clip_and_scroll_rect(&self, x:f32, y:f32, w:f32, h:f32)->Rect{
        let mut x1 = x - self.uniforms[DL_UNI_SCROLL+0];
        let mut y1 = y - self.uniforms[DL_UNI_SCROLL+1];
        let mut x2 = x1 + w;
        let mut y2 = y1 + h; 
        let min_x = self.uniforms[DL_UNI_CLIP+0];
        let min_y = self.uniforms[DL_UNI_CLIP+1];
        let max_x = self.uniforms[DL_UNI_CLIP+2];
        let max_y = self.uniforms[DL_UNI_CLIP+3];
        x1 = min_x.max(x1).min(max_x);
        y1 = min_y.max(y1).min(max_y);
        x2 = min_x.max(x2).min(max_x);
        y2 = min_y.max(y2).min(max_y);
        return Rect{x:x1, y:y1, w:x2-x1, h:y2-y1};
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
    pub vec_obj:Vec<u8>,
    pub parse:isize
}

impl BinaryDep{
    pub fn new_from_vec(name:String, vec_obj:Vec<u8>)->BinaryDep{
        BinaryDep{
            name:name, 
            vec_obj:vec_obj,
            parse:0
        }
    }

    pub fn u8(&mut self)->Result<u8, String>{
        if self.parse + 1 > self.vec_obj.len() as isize{
            return Err(format!("Eof on u8 file {} offset {}", self.name, self.parse))
        }
        unsafe{
            let ret = (self.vec_obj.as_ptr().offset(self.parse) as *const u8).read();
            self.parse += 1;
            Ok(ret)
        }
    }

    pub fn u16(&mut self)->Result<u16, String>{
        if self.parse+2 > self.vec_obj.len() as isize{
            return Err(format!("Eof on u16 file {} offset {}", self.name, self.parse))
        }
        unsafe{
            let ret = (self.vec_obj.as_ptr().offset(self.parse) as *const u16).read();
            self.parse += 2;
            Ok(ret)
        }
    }

    pub fn u32(&mut self)->Result<u32, String>{
        if self.parse+4 > self.vec_obj.len() as isize{
            return Err(format!("Eof on u32 file {} offset {}", self.name, self.parse))
        }
        unsafe{
            let ret = (self.vec_obj.as_ptr().offset(self.parse) as *const u32).read();
            self.parse += 4;
            Ok(ret)
        }
    }

    pub fn f32(&mut self)->Result<f32, String>{
        if self.parse+4 > self.vec_obj.len() as isize{
            return Err(format!("Eof on f32 file {} offset {}", self.name, self.parse))
        }
        unsafe{
            let ret = (self.vec_obj.as_ptr().offset(self.parse) as *const f32).read();
            self.parse += 4;
            Ok(ret)
        }
    }

    pub fn read(&mut self, out:&mut [u8])->Result<usize, String>{
        let len = out.len();
        if self.parse + len as isize > self.vec_obj.len() as isize{
             return Err(format!("Eof on read file {} len {} offset {}", self.name, out.len(), self.parse));
        };
        //unsafe{
            for i in 0..len{
                out[i] = self.vec_obj[self.parse as usize + i];
            };
            self.parse += len as isize;
        //}
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
                if let Event::Draw = event{return app.draw_app(cx);}
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
        pub unsafe extern "C" fn process_to_wasm(appcx:u32, msg_bytes:u32)->u32{
            let appcx = &*(appcx as *mut (*mut $app,*mut Cx));
            (*appcx.1).process_to_wasm(msg_bytes,|cx, mut event|{
                if let Event::Draw = event{return (*appcx.0).draw_app(cx);}
                (*appcx.0).handle_app(cx, &mut event);
            })
        }
    };
}