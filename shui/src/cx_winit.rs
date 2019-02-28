use crate::cx_shared::*;
use crate::events::*;
use std::io::prelude::*;
use std::fs::File;
use std::io;
use crate::math::*;

#[derive(Clone, Default)]
pub struct CxWinit{
    pub last_x:f32,
    pub last_y:f32
}

impl Cx{
    pub fn map_winit_event(&mut self, winit_event:winit::Event, glutin_window:&winit::Window)->Event{
        //self.log(&format!("{:?}\n", winit_event));
        match winit_event{
            winit::Event::WindowEvent{ event, .. } => match event {
                winit::WindowEvent::MouseWheel{delta, ..}=>{
                    return Event::FingerScroll(FingerScrollEvent{
                        x:self.resources.winit.last_x,
                        y:self.resources.winit.last_y,
                        dx:match delta{
                            winit::MouseScrollDelta::LineDelta(dx,_dy)=>dx,
                            winit::MouseScrollDelta::PixelDelta(pp)=>pp.x as f32
                        },
                        dy:match delta{
                            winit::MouseScrollDelta::LineDelta(_dx,dy)=>dy,
                            winit::MouseScrollDelta::PixelDelta(pp)=>pp.y as f32
                        },
                    })
                },
                winit::WindowEvent::CursorMoved{position,..}=>{
                    self.resources.winit.last_x = position.x as f32;
                    self.resources.winit.last_y = position.y as f32;
                    return Event::FingerHover(FingerHoverEvent{
                        x:self.resources.winit.last_x,
                        y:self.resources.winit.last_y
                    })
                },
                winit::WindowEvent::MouseInput{state,button,..}=>{
                    match state{
                        winit::ElementState::Pressed=>{
                            return Event::FingerDown(FingerDownEvent{
                                x:self.resources.winit.last_x,
                                y:self.resources.winit.last_y,
                                button:match button{
                                    winit::MouseButton::Left=>MouseButton::Left,
                                    winit::MouseButton::Right=>MouseButton::Right,
                                    winit::MouseButton::Middle=>MouseButton::Middle,
                                    winit::MouseButton::Other(id)=>MouseButton::Other(id)
                                },
                                digit:0,
                                is_touch:false,
                            })
                        },
                        winit::ElementState::Released=>{
                            return Event::FingerUp(FingerUpEvent{
                                x:self.resources.winit.last_x,
                                y:self.resources.winit.last_y,
                                button:match button{
                                    winit::MouseButton::Left=>MouseButton::Left,
                                    winit::MouseButton::Right=>MouseButton::Right,
                                    winit::MouseButton::Middle=>MouseButton::Middle,
                                    winit::MouseButton::Other(id)=>MouseButton::Other(id)
                                },
                                digit:0,
                                is_touch:false,
                            })
                        }
                    }
                },
               
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

    pub fn load_binary_deps_from_file(&mut self){
        for i in 0..self.fonts.font_resources.len(){
            let resource_name = &self.fonts.font_resources[i].name;
            // lets turn a file into a binary dep
            let file_result = File::open(&resource_name);
            if let Ok(mut file) = file_result{
                let mut buffer = Vec::new();
                // read the whole file
                if file.read_to_end(&mut buffer).is_ok(){
                    // store it in a bindep
                    let mut bin_dep = BinaryDep::new_from_vec(resource_name.clone(), &buffer);
                    let _err = self.fonts.load_from_binary_dep(&mut bin_dep, &mut self.textures);

                    //     println!("Error loading font {} ", resource_name);
                    //};
                }
            }
            else{
                println!("Error loading font {} ", resource_name);
            }
        }
    }

    pub fn process_to_wasm<F>(&mut self, _msg:u32, mut _event_handler:F)->u32{
        0
    }

    pub fn log(&mut self, val:&str){
        let mut stdout = io::stdout();
        let _e = stdout.write(val.as_bytes());
        let _e = stdout.flush();
    }
}