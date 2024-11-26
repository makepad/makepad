use makepad_widgets::*;

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    
    VideoFrame = <Image> {
        height: All,
        width: All,
        width_scale: 2.0,
        fit: Biggest,
        draw_bg: {
            uniform image_size: vec2
            uniform is_rgb: 0.0
            fn yuv_to_rgb(y: float, u: float, v: float) -> vec4 {
                return vec4(
                    y + 1.14075 * (v - 0.5),
                    y - 0.3455 * (u - 0.5) - 0.7169 * (v - 0.5),
                    y + 1.7790 * (u - 0.5),
                    1.0
                )
            }
            
            fn get_video_pixel(self, pos:vec2) -> vec4 {
                let pix = self.pos * self.image_size;
                
                // fetch pixel
                let data = sample2d(self.image, pos).xyzw;
                if self.is_rgb > 0.5 {
                    return vec4(data.xyz, 1.0);
                }
                if mod (pix.x, 2.0)>1.0 {
                    return yuv_to_rgb(data.x, data.y, data.w)
                }
                return yuv_to_rgb(data.z, data.y, data.w)
            }
            
            fn pixel(self) -> vec4 {
                return self.get_video_pixel(self.pos);
            }
        }
    }
    
    App = {{App}} {
        ui: <Window> {
            body={
                video_input0 = <VideoFrame>{}
            }
        }
    }
}
app_main!(App);

#[derive(Live, LiveHook)]
pub struct App {
    #[live] ui: WidgetRef,
    #[rust([Texture::new(cx)])] video_input: [Texture; 1],
    #[rust] video_recv: ToUIReceiver<(usize, VideoBuffer)>,
}

impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
    }
}

impl MatchEvent for App{
    fn handle_signal(&mut self, cx:&mut Cx){
        while let Ok((id, mut vfb)) = self.video_recv.try_recv() {
            let (current_w, current_h) = self.video_input[id].get_format(cx).vec_width_height().unwrap();
            if current_w != vfb.format.width / 2 || current_h != vfb.format.height {
                self.video_input[id] = Texture::new_with_format(cx, TextureFormat::VecBGRAu8_32{
                    data: vec![],
                    width: vfb.format.width / 2,
                    height: vfb.format.height
                });
            }
            if let Some(buf) = vfb.as_vec_u32() {
                self.video_input[id].swap_vec_u32(cx, buf);
            }
            let image_size = [vfb.format.width as f32, vfb.format.height as f32];
            let v = self.ui.view(id!(video_input0));
            v.as_image().set_texture(cx, Some(self.video_input[id].clone()));
            v.set_uniform(cx, id!(image_size), &image_size);
            v.set_uniform(cx, id!(is_rgb), &[0.0]);
            v.redraw(cx);
        }
    }
    
    fn handle_startup(&mut self, cx:&mut Cx){
        let video_sender = self.video_recv.sender();
        cx.video_input(0, move | img | {
            let _ = video_sender.send((0, img.to_buffer()));
        });
    }
    
    fn handle_video_inputs(&mut self, cx:&mut Cx, devices:&VideoInputsEvent){
        log!("{:?}", devices);
        let input = devices.find_highest_at_res(devices.find_device("Logitech BRIO"), 1920, 1080, 31.0);
        cx.use_video_input(&input);
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
