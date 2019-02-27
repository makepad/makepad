use crate::shader::*;
use crate::cx::*;
use crate::cxdrawing::*;
use crate::cxshaders::*;
use crate::cxfonts::*;
use crate::cxtextures::*;
use crate::cxturtle::*;
use crate::events::*;
use crate::animation::*;

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

    pub redraw_dirty:Option<Area>,
    pub redraw_area:Option<Area>,
    pub paint_dirty:bool,

    pub binary_deps:Vec<BinaryDep>,
    pub clear_color:Vec4,
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
            resources:CxResources{..Default::default()},
            animations:Vec::new(),
            binary_deps:Vec::new(),
            redraw_area:None,
            redraw_dirty:None,
            clear_color:vec4(0.3,0.3,0.3,1.0),
            paint_dirty:false
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
        self.redraw_dirty = Some(Area::zero())
    }

    // trigger a redraw on the UI
    pub fn redraw_none(&mut self){ 
        self.redraw_dirty = None
    }

    // trigger a redraw on the UI
    pub fn redraw(&mut self, area:&Area){ 
        self.redraw_dirty = Some(area.clone())
    }

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
    pub fn len(&self)->usize{
        self.len
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

            cx.event_loop(|cx, ev|{
                app.handle(cx, &ev);
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
            (*appcx.1).process_to_wasm(msg,|cx, ev|{
                (*appcx.0).handle(cx, &ev);
            })
        }
    };
}