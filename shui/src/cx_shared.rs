use crate::shader::*;
use crate::cxdrawing::*;
use crate::cxshaders::*;
use crate::cxfonts::*;
use crate::cxtextures::*;
use crate::cxturtle::*;

#[derive(Clone)]
pub struct Cx{
    pub title:String,
    pub running:bool,

    pub turtle:CxTurtle,
    pub shaders:CxShaders,
    pub drawing:CxDrawing,
    pub fonts:CxFonts,
    pub textures:CxTextures,
    pub uniforms:Vec<f32>,
    pub resources:CxResources,

    pub animations:Vec<AnimArea>,
    pub redraw_area:Option<Area>,
    pub binary_deps:Vec<BinaryDep>,

    pub clear_color:Vec4,
    pub repaint:bool,
    pub cycle_time:f64, // time in seconds in f64
    pub cycle_id:u64
}

impl Default for Cx{
    fn default()->Self{
        let mut uniforms = Vec::<f32>::new();
        uniforms.resize(CX_UNI_SIZE, 0.0);
        Self{
            turtle:CxTurtle{..Default::default()},
            fonts:CxFonts{..Default::default()},
            drawing:CxDrawing{..Default::default()},
            shaders:CxShaders{..Default::default()},
            textures:CxTextures{..Default::default()},
            title:"Hello World".to_string(),
            running:true,
            cycle_time:0.0,
            cycle_id:0,
            uniforms:uniforms,
            resources:CxResources{..Default::default()},
            animations:Vec::new(),
            binary_deps:Vec::new(),
            redraw_area:Some(Area::zero()),
            clear_color:vec4(0.3,0.3,0.3,1.0),
            repaint:true
        }
    }
}

const CX_UNI_CAMERA_PROJECTION:usize = 0;
const CX_UNI_SIZE:usize = 16;

impl Cx{
    pub fn def_shader(sh:&mut Shader){
        Shader::def_builtins(sh);
        Shader::def_df(sh);
        Cx::def_uniforms(sh);
        DrawList::def_uniforms(sh);
    }

    pub fn def_uniforms(sh: &mut Shader){
        sh.add_ast(shader_ast!(||{
            let camera_projection:mat4<UniformCx>;
        }));
    }

    pub fn uniform_camera_projection(&mut self, v:Mat4){
        //dump in uniforms
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

    pub fn prepare_frame(&mut self){
        let camera_projection = Mat4::ortho(
                0.0, self.turtle.target_size.x, 0.0, self.turtle.target_size.y, -100.0, 100.0, 
                1.0,1.0
        );
        self.uniform_camera_projection(camera_projection);
        self.turtle.align_list.truncate(0);
    }

    // trigger a redraw on the UI
    pub fn redraw_all(&mut self){ 
        self.redraw_area = Some(Area::zero())
    }

    // trigger a redraw on the UI
    pub fn redraw_none(&mut self){ 
        self.redraw_area = None
    }

    // trigger a redraw on the UI
    pub fn redraw(&mut self, area:&Area){ 
        self.redraw_area = Some(area.clone())
    }

    pub fn start_animation(&mut self, state_name:&str, area:&Area, states:&Vec<AnimState>){

        let anim_state_opt = states.iter().find(|v| v.state_name == state_name);

        if anim_state_opt.is_none(){
            println!("Starting animation state {} does not exist in states", state_name);
            return;
        }
        let anim_state = anim_state_opt.unwrap();

        // alright first we find area
        if let Some(anim) = self.animations.iter_mut().find(|v| v.area == *area){
            //do we queue or replace
            match anim_state.anim_start{
                AnimStart::Interrupt=>{
                    anim.start = std::f64::NAN;
                    anim.current_state = state_name.to_string();
                    anim.next_state = "".to_string();
                },
                AnimStart::Queue=>{
                    anim.next_state = state_name.to_string();
                    // lets add up the durations of both states
                    let prev_anim_state = states.iter().find(|v| v.state_name == anim.current_state).unwrap();
                    anim.duration = anim_state.duration + prev_anim_state.duration;
                }
            }
        }
        else{
            self.animations.push(AnimArea{
                area:area.clone(),
                start:self.cycle_time,
                duration:anim_state.duration,
                current_state:state_name.to_string(),
                next_state:"".to_string()
            })
        }
    }

    pub fn compute_animation(&mut self, _area_name:&str, id_area:&Area, _states:&Vec<AnimState>, _tgt_area:&Area){
        // alright we need to compute an animation. 
        // ok so first we use the id area to fetch the animation
        // alright first we find area
        let anim_opt = self.animations.iter_mut().find(|v| v.area == *id_area);
        if anim_opt.is_none(){
            return
        }
        let _anim = anim_opt.unwrap();
    
        // so if the start is NAN we are the first time called.

        // ok so, if we are interrupting 

    }
}
#[derive(Clone,Debug)]
pub enum AnimData{
    NotSet,
    Vec4(Vec4),
    Float(f32)
}

#[derive(Clone,Debug)]
pub struct AnimValue{
    pub area_name:String,
    pub value_name:String,
    pub new_data:AnimData,
    pub old_data:AnimData
}

impl AnimValue{
    pub fn color(area_name:&str, value_name:&str, value:&str)->AnimValue{
        AnimValue{
            area_name:area_name.to_string(),
            value_name:value_name.to_string(),
            new_data:AnimData::Vec4(color(value)),
            old_data:AnimData::NotSet
        }
    }
    pub fn vec4f(area_name:&str, value_name:&str, x:f32, y:f32, z:f32, w:f32)->AnimValue{
        AnimValue{
            area_name:area_name.to_string(),
            value_name:value_name.to_string(),
            new_data:AnimData::Vec4(vec4(x,y,z,w)),
            old_data:AnimData::NotSet
        }
    }
    pub fn vec4(area_name:&str, value_name:&str, v:Vec4)->AnimValue{
        AnimValue{
            area_name:area_name.to_string(),
            value_name:value_name.to_string(),
            new_data:AnimData::Vec4(v),
            old_data:AnimData::NotSet
        }
    }
   pub fn float(area_name:&str, value_name:&str, v:f32)->AnimValue{
        AnimValue{
            area_name:area_name.to_string(),
            value_name:value_name.to_string(),
            new_data:AnimData::Float(v),
            old_data:AnimData::NotSet
        }
    }

}

#[derive(Clone,Debug)]
pub struct AnimKey{
    pub time:f64,
    pub values:Vec<AnimValue>
}

impl AnimKey{
    pub fn new( time:f64, values:Vec<AnimValue>)->AnimKey{
        AnimKey{
            time:time,
            values:values
        }
    }
}

#[derive(Clone,Debug)]
pub enum AnimStart{
    Queue,
    Interrupt
}

impl Default for AnimStart{
    fn default()->AnimStart{
        AnimStart::Queue
    }
}


#[derive(Clone,Debug)]
pub struct AnimState{
    pub state_name:String,
    pub duration:f64,
    pub anim_start:AnimStart,
    pub keys:Vec<AnimKey>
}

impl AnimState{
    pub fn new(name:&str, duration:f64, anim_start:AnimStart, keys:Vec<AnimKey>)->AnimState{
        AnimState{
            state_name:name.to_string(),
            duration:duration,
            anim_start:anim_start,
            keys:keys
        }
    }
}

#[derive(Clone,Debug,PartialEq)]
pub struct AnimArea{
    pub area:Area,
    pub start:f64,
    pub duration:f64,
    pub current_state:String,
    pub next_state:String
}

pub struct Elements<T>{
    pub elements:Vec<T>,
    pub len:usize
}

impl<T> Elements<T>
where T:Clone
{
    pub fn new()->Elements<T>{
        Elements::<T>{
            elements:Vec::new(),
            len:0
        }
    }

    pub fn reset(&mut self){
        self.len = 0;
    }

    pub fn add(&mut self, clone:&T)->&mut T{
        if self.len >= self.elements.len(){
            self.elements.push(clone.clone());
            self.len += 1;
            self.elements.last_mut().unwrap()

        }
        else{
            let last = self.len;
            self.len += 1;
            &mut self.elements[last]
        }
    }
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

            cx.event_loop(|cx, ev|{
                app.handle(cx, &ev);
            });
        }

        #[export_name = "init_wasm"]
        pub extern "C" fn init_wasm()->u32{
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

        #[export_name = "to_wasm"]
        pub unsafe extern "C" fn to_wasm(appcx:u32, msg:u32)->u32{
            let appcx = &*(appcx as *mut (*mut $app,*mut Cx));
            (*appcx.1).to_wasm(msg,|cx, ev|{
                (*appcx.0).handle(cx, &ev);
            })
        }
    };
}