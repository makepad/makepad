pub use makepad_audio_widgets;
pub use makepad_audio_widgets::makepad_widgets;
pub use makepad_widgets::makepad_platform;

use {
    crate::{
        makepad_widgets::*,
        makepad_platform::thread::*,
        makepad_platform::video::*,
        makepad_draw::*,
    },
};


live_design!{
    import makepad_widgets::frame::*;
    import makepad_draw::shader::std::*;
    //import crate::video_view::VideoView;
    registry Widget::*;
    
    VideoFrame = <Frame> {
        show_bg: true,
        walk: {width: Fill, height: Fill},
        draw_bg: {
            texture image: texture2d
            uniform image_size: vec2
            fn yuv_to_rgb(y: float, u: float, v: float) -> vec4 {
                return vec4(
                    y + 1.14075 * (v - 0.5),
                    y - 0.3455 * (u - 0.5) - 0.7169 * (v - 0.5),
                    y + 1.7790 * (u - 0.5),
                    1.0
                )
            }
            
            fn get_video_pixel(self) -> vec4 {
                let pix = self.pos * self.image_size;
                
                // fetch pixel
                let data = sample2d(self.image, self.pos).xyzw;
                
                if mod (pix.x, 2.0)>1.0 {
                    return yuv_to_rgb(data.x, data.y, data.w)
                }
                return yuv_to_rgb(data.z, data.y, data.w)
            }
            
            fn pixel(self) -> vec4 {
                return self.get_video_pixel();
            }
        }
    }
    
    App = {{App}} {
        mixer:{
            channel:[2.0,2.0,2.0,2.0]
        }
        window: {ui: {inner_view = {
            video_input1 = <VideoFrame> {
            }
        }}}
       
    }
}
app_main!(App); 


#[derive(Live, LiveHook)]
pub struct App {
    window: DesktopWindow,
    video_input1: Texture, 
    #[rust] video_recv: ToUIReceiver<VideoBuffer>,
}

impl App {
    pub fn live_design(cx: &mut Cx) {
        makepad_audio_widgets::live_design(cx);
    }
    
    pub fn start_inputs(&mut self, cx: &mut Cx) {
        let video_sender = self.video_recv.sender();
        cx.video_input(0, move | img | {
            let _ = video_sender.send(img.to_buffer());
        })
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        match event {
            Event::Signal => {
                if let Ok(mut vfb) = self.video_recv.try_recv_flush() {
                    self.video_input1.set_desc(cx, TextureDesc {
                        format: TextureFormat::ImageBGRA,
                        width: Some(vfb.format.width / 2),
                        height: Some(vfb.format.height)
                    });
                    self.video_input1.swap_image_u32(cx, &mut vfb.data);
                    let image_size = [vfb.format.width as f32, vfb.format.height as f32];
                    
                    for v in [
                        self.window.ui.get_frame(id!(video_input1)),
                    ] {
                        v.set_texture(0, &self.video_input1);
                        v.set_uniform(cx, id!(image_size), &image_size);
                        v.redraw(cx);
                    }
                }
            }
            Event::Draw(event) => {
                return self.draw(&mut Cx2d::new(cx, event));
            }
            Event::Construct => {
                self.start_inputs(cx);
            }
            Event::VideoInputs(devices) => {
                //log!("Got devices! {:?}", devices);
                cx.use_video_input(&devices.find_highest_at_res(0, 1920, 1080));
            }
            _ => ()
        }
        
        self.window.handle_event(cx, event);
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        if self.window.begin(cx).is_redrawing() {
            self.window.end(cx);
        }
    }
}