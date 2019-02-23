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
    pub buffers:CxBuffers,
    pub animations:Vec<Animation>,
    pub redraw_area:Option<Area>,
    pub repaint:bool
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
            uniforms:uniforms,
            buffers:CxBuffers{..Default::default()},
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
    pub fn redraw_all(){ 

    }

    pub fn animate(area:&Area, duration:f64){
    }
}

#[derive(Clone)]
pub struct Animation{
    pub area:Area,
    pub start:f64,
    pub duration:f64
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