pub use makepad_widgets;
pub use makepad_widgets::makepad_platform;
mod video_view;

use {
    crate::{
        makepad_widgets::*,
        //makepad_platform::video_capture::*,
        makepad_draw::*,
    },
};


live_design!{
    import makepad_widgets::frame::*;
    import makepad_draw::shader::std::*;
    //import crate::video_view::VideoView;
    registry Widget::*;
    App = {{App}} {
        ui: {
            video = <VideoView>{
                
            }
            walk: {width: Fill, height: Fill},
            draw_bg: {
                fn pixel(self) -> vec4 {
                    return Pal::premul(#3)
                }
            }
        }
    }
}
app_main!(App);

#[derive(Live, LiveHook)]
pub struct App {
    window: DesktopWindow,
    ui: FrameRef,
}

impl App {
    pub fn live_design(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
        crate::video_view::live_design(cx);
    }
    
    pub fn start_inputs(&mut self, cx: &mut Cx) {
        /*cx.audio_input(0, move | _device, _time, input_buffer | {
            input_buffer
        });
        
        cx.audio_output(0, move | _device, _time, _output_buffer | {
        });*/
        
        cx.video_input(0, move |img|{
            // alright we need to get this thing to a texture and see
            // what it looks like
            
            //println!("Videoframe: {}", img.data.len()); 
        })
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        // no UI as of yet
        match event {
            Event::Draw(event) => {
                return self.draw(&mut Cx2d::new(cx, event));
            }
            Event::Construct => {
                self.start_inputs(cx);
            }
            Event::MidiPorts(ports) => {
                cx.use_midi_inputs(&ports.all_inputs());
            }
            Event::AudioDevices(devices) => { 
                //cx.use_audio_inputs(&devices.default_input());
                //cx.use_audio_outputs(&devices.default_output());
            }
            Event::VideoInputs(devices)=>{
                println!("Got devices!");
                cx.use_video_input(&devices.find_highest(0));
            }
            _ => ()
        }
         
        self.ui.handle_event(cx, event);
        self.window.handle_event(cx, event);
    } 
    
    pub fn draw(&mut self, cx: &mut Cx2d) { 
        if self.window.begin(cx).is_not_redrawing() {
            return;
        }
        
        while self.ui.draw(cx).is_not_done() {};
        
        //self.ui.redraw(cx);
        self.window.end(cx);
    }
}