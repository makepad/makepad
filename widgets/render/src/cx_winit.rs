use crate::cx::*;

use std::io::prelude::*;
use std::fs::File;
use std::io;

#[derive(Clone, Default)]
pub struct CxWinit{
    pub last_mouse_pos:Vec2,
    pub is_cursor_in_window:bool
}


impl Cx{

    fn make_mouse_move_events(&self)->Vec<Event>{
        let mut out = Vec::new();
        for i in 0..self.fingers_down.len(){
            let down = self.fingers_down[i];
            if down{
                out.push(Event::FingerMove(FingerMoveEvent{
                    abs:self.resources.winit.last_mouse_pos,
                    digit:i,
                    rel:self.resources.winit.last_mouse_pos,
                    abs_start:vec2(0.,0.),
                    rel_start:vec2(0.,0.),
                    is_over:false,
                    is_touch:false
                }))
            }
        };
        return out;
    }

    pub fn set_winit_mouse_cursor(&mut self, window:&winit::Window, mouse_cursor:MouseCursor){
        let (hide, cursor) = match mouse_cursor{
                MouseCursor::Hidden=>(true,winit::MouseCursor::Default),
                MouseCursor::Default=>(false,winit::MouseCursor::Default),
                MouseCursor::Crosshair=>(false,winit::MouseCursor::Crosshair),
                MouseCursor::Hand=>(false,winit::MouseCursor::Hand),
                MouseCursor::Arrow=>(false,winit::MouseCursor::Arrow),
                MouseCursor::Move=>(false,winit::MouseCursor::Move),
                MouseCursor::Text=>(false,winit::MouseCursor::Text),
                MouseCursor::Wait=>(false,winit::MouseCursor::Wait),
                MouseCursor::Help=>(false,winit::MouseCursor::Help),
                MouseCursor::Progress=>(false,winit::MouseCursor::Progress),
                MouseCursor::NotAllowed=>(false,winit::MouseCursor::NotAllowed),
                MouseCursor::ContextMenu=>(false,winit::MouseCursor::ContextMenu),
                MouseCursor::Cell=>(false,winit::MouseCursor::Cell),
                MouseCursor::VerticalText=>(false,winit::MouseCursor::VerticalText),
                MouseCursor::Alias=>(false,winit::MouseCursor::Alias),
                MouseCursor::Copy=>(false,winit::MouseCursor::Copy),
                MouseCursor::NoDrop=>(false,winit::MouseCursor::NoDrop),
                MouseCursor::Grab=>(false,winit::MouseCursor::Grab),
                MouseCursor::Grabbing=>(false,winit::MouseCursor::Grabbing),
                MouseCursor::AllScroll=>(false,winit::MouseCursor::AllScroll),
                MouseCursor::ZoomIn=>(false,winit::MouseCursor::ZoomIn),
                MouseCursor::ZoomOut=>(false,winit::MouseCursor::ZoomOut),
                MouseCursor::NResize=>(false,winit::MouseCursor::NResize),
                MouseCursor::NeResize=>(false,winit::MouseCursor::NeResize),
                MouseCursor::EResize=>(false,winit::MouseCursor::EResize),
                MouseCursor::SeResize=>(false,winit::MouseCursor::SeResize),
                MouseCursor::SResize=>(false,winit::MouseCursor::SResize),
                MouseCursor::SwResize=>(false,winit::MouseCursor::SwResize),
                MouseCursor::WResize=>(false,winit::MouseCursor::WResize),
                MouseCursor::NwResize=>(false,winit::MouseCursor::NwResize),
                MouseCursor::NsResize=>(false,winit::MouseCursor::NsResize),
                MouseCursor::NeswResize=>(false,winit::MouseCursor::NeswResize),
                MouseCursor::EwResize=>(false,winit::MouseCursor::EwResize),
                MouseCursor::NwseResize=>(false,winit::MouseCursor::NwseResize),
                MouseCursor::ColResize=>(false,winit::MouseCursor::ColResize),
                MouseCursor::RowResize=>(false,winit::MouseCursor::RowResize),
        };
        window.set_cursor(cursor);
        window.hide_cursor(hide);
    }

    pub fn map_winit_event(&mut self, winit_event:winit::Event, glutin_window:&winit::Window)->Vec<Event>{
        //self.log(&format!("{:?}\n", winit_event));

        match winit_event{
            winit::Event::DeviceEvent{ event, .. } => match event {
                winit::DeviceEvent::MouseMotion{delta,..}=>{
                    if self.resources.winit.is_cursor_in_window{
                        return vec![Event::None]
                    }
                    self.resources.winit.last_mouse_pos.x += delta.0 as f32;//position.x as f32;
                    self.resources.winit.last_mouse_pos.y += delta.1 as f32;//position.y as f32;
                    
                    return self.make_mouse_move_events();/*vec![Event::FingerHover(FingerHoverEvent{
                        x:self.resources.winit.last_x,
                        y:self.resources.winit.last_y,
                        handled:false,
                        hover_state:HoverState::Over
                    })]*/

                },
                _=>()
            },
            winit::Event::WindowEvent{ event, .. } => match event {
                winit::WindowEvent::MouseWheel{delta, ..}=>{
                    return vec![Event::FingerScroll(FingerScrollEvent{
                        abs:self.resources.winit.last_mouse_pos,
                        rel:self.resources.winit.last_mouse_pos,
                        handled:false,
                        scroll:vec2(
                            match delta{
                                winit::MouseScrollDelta::LineDelta(dx,_dy)=>-dx*32.0,
                                winit::MouseScrollDelta::PixelDelta(pp)=>pp.x as f32
                            },
                            match delta{
                                winit::MouseScrollDelta::LineDelta(_dx,dy)=>-dy*32.0,
                                winit::MouseScrollDelta::PixelDelta(pp)=>pp.y as f32
                            }
                        ), 
                    })]
                },
                winit::WindowEvent::CursorMoved{position,..}=>{
                    self.resources.winit.is_cursor_in_window = true;
                    self.resources.winit.last_mouse_pos = vec2(position.x as f32, position.y as f32);
                    self.hover_mouse_cursor = None;
                    let mut events = self.make_mouse_move_events();
                    events.push(Event::FingerHover(FingerHoverEvent{
                        abs:self.resources.winit.last_mouse_pos,
                        rel:self.resources.winit.last_mouse_pos,
                        handled:false,
                        hover_state:HoverState::Over
                    }));
                    return events;
                },
                winit::WindowEvent::CursorEntered{..}=>{
                    self.resources.winit.is_cursor_in_window = true;
                },
                winit::WindowEvent::Focused(state)=>{
                    return vec![Event::AppFocus(state)]
                },
                winit::WindowEvent::CursorLeft{..}=>{
                    self.resources.winit.is_cursor_in_window = false;
                    self.hover_mouse_cursor = None;
                   // fire a hover out on our last known mouse position
                    return vec![Event::FingerHover(FingerHoverEvent{
                        abs:self.resources.winit.last_mouse_pos,
                        rel:self.resources.winit.last_mouse_pos,
                        handled:false,
                        hover_state:HoverState::Out
                    })]

                },
                winit::WindowEvent::MouseInput{state,button,..}=>{
                    match state{
                        winit::ElementState::Pressed=>{
                            let mut digit = match button{// this makes sure that single touch mode doesnt allow multiple mousedowns
                                winit::MouseButton::Left=>0,
                                winit::MouseButton::Right=>1,
                                winit::MouseButton::Middle=>2,
                                winit::MouseButton::Other(id)=>id as usize
                            };
                            if digit >= self.captured_fingers.len(){
                                digit = 0;
                            };
                            self.fingers_down[digit] = true;
                            return vec![Event::FingerDown(FingerDownEvent{
                                abs:self.resources.winit.last_mouse_pos,
                                rel:self.resources.winit.last_mouse_pos,
                                handled:false,
                                digit:digit,
                                is_touch:false,
                            })]
                        },
                        winit::ElementState::Released=>{

                            let mut digit = match button{// this makes sure that single touch mode doesnt allow multiple mousedowns
                                winit::MouseButton::Left=>0,
                                winit::MouseButton::Right=>1,
                                winit::MouseButton::Middle=>2,
                                winit::MouseButton::Other(id)=>id as usize
                            };
                            if digit >= self.captured_fingers.len(){
                                digit = 0;
                            };
                            self.fingers_down[digit] = false;
                            if !self.any_fingers_down(){
                                self.down_mouse_cursor = None;
                            }
                            return vec![Event::FingerUp(FingerUpEvent{
                                abs:self.resources.winit.last_mouse_pos,
                                rel:self.resources.winit.last_mouse_pos,
                                abs_start:vec2(0.,0.),
                                rel_start:vec2(0.,0.),
                                digit:digit,
                                is_over:false,
                                is_touch:false,
                            })]
                        }
                    }
                },
               
                winit::WindowEvent::CloseRequested =>{
                    self.running = false;
                    return vec![Event::CloseRequested]
                },
                winit::WindowEvent::Resized(logical_size) => {
                    let dpi_factor = glutin_window.get_hidpi_factor();
                    let old_dpi_factor = self.target_dpi_factor as f32;
                    let old_size = self.target_size.clone();
                    self.target_dpi_factor = dpi_factor as f32;
                    self.target_size = vec2(logical_size.width as f32, logical_size.height as f32);
                    return vec![Event::Resized(ResizedEvent{
                        old_size: old_size,
                        old_dpi_factor: old_dpi_factor,
                        new_size: self.target_size.clone(),
                        new_dpi_factor: self.target_dpi_factor
                    })]
                },
                _ => ()
            },
            _ => ()
        }
        vec![Event::None]
    }

    pub fn load_binary_deps_from_file(&mut self){
        let len = self.fonts.len();
        for i in 0..len{
            let resource_name = &self.fonts[i].name.clone();
            // lets turn a file into a binary dep
            let file_result = File::open(&resource_name);
            if let Ok(mut file) = file_result{
                let mut buffer = Vec::new();
                // read the whole file
                if file.read_to_end(&mut buffer).is_ok(){
                    // store it in a bindep
                    let mut bin_dep = BinaryDep::new_from_vec(resource_name.clone(), &buffer);
                    let _err = self.load_font_from_binary_dep(&mut bin_dep);

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