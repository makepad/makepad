use crate::shader::*;
use crate::cxdrawing::*;
use crate::cxshaders::*;
use crate::cxfonts::*;
use crate::cxtextures::*;
use crate::cxturtle::*;
use crate::events::*;

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
            redraw_area:Some(Area::zero()),
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

    pub fn prepare_frame(&mut self){
        let camera_projection = Mat4::ortho(
                0.0, self.turtle.target_size.x, 0.0, self.turtle.target_size.y, -100.0, 100.0, 
                1.0,1.0
        );
        self.uniform_camera_projection(camera_projection);
        self.turtle.align_list.truncate(0);
    }

    #[cfg(any(feature = "mtl", feature = "ogl"))]
    pub fn map_winit_event(&mut self, winit_event:winit::Event, glutin_window:&winit::Window)->Event{
        match winit_event{
            winit::Event::WindowEvent{ event, .. } => match event {
                winit::WindowEvent::CloseRequested =>{
                    self.running = false;
                    return Event::CloseRequested
                },
                winit::WindowEvent::Resized(logical_size) => {
                    
                    let dpi_factor = glutin_window.get_hidpi_factor();
                    let old_dpi_factor = self.turtle.target_dpi_factor as f32;
                    let old_size = self.turtle.target_size.clone();
                    self.turtle.target_dpi_factor = dpi_factor as f32;
                    self.turtle.target_size = vec2(logical_size.width as f32, logical_size.height as f32);
                    return Event::Resized(ResizedEvent{
                        old_size: old_size,
                        old_dpi_factor: old_dpi_factor,
                        new_size: self.turtle.target_size.clone(),
                        new_dpi_factor: self.turtle.target_dpi_factor
                    })
                },
                _ => ()
            },
            _ => ()
        }
        Event::None
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

    pub fn compute_animation(&mut self, area_name:&str, id_area:&Area, states:&Vec<AnimState>, tgt_area:&Area){
        // alright we need to compute an animation. 
        // ok so first we use the id area to fetch the animation
        // alright first we find area
        let anim_opt = self.animations.iter_mut().find(|v| v.area == *id_area);
        if anim_opt.is_none(){
            return
        }
        let anim = anim_opt.unwrap();
    
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