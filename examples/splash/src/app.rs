use makepad_draw2::*;

app_main!(App); 
script_run!{
    use mod.std.*;
    #(App::script_api(vm)){
    }
}

impl App{
    fn run(vm:&mut ScriptVm)->Self{
        crate::makepad_draw2::script_run(vm);
        let r = App::script_run(vm, script_run);
        r
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[script] window: WindowHandle,
    #[script] pass: Pass,
    #[script] depth_texture: Texture,
    #[rust] smooth_throttle: f32,
    //#[script] draw_quad: DrawQuad,
    #[script] main_draw_list: DrawList2d,
}
 
impl MatchEvent for App{
    fn handle_timer(&mut self, cx:&mut Cx, _ev: &TimerEvent){
        let limiter: f32 = 0.7;
        for state in cx.game_input_states_mut(){
            match state{
                GameInputState::Gamepad(gp)=>{
                    let steer: f32 = gp.right_stick.x + gp.left_stick.x;
                    let throttle: f32 = ((gp.left_trigger * -1.0) + gp.right_trigger) * limiter;
                    let smooth = 0.95;
                    self.smooth_throttle = smooth * self.smooth_throttle + (1.0 - smooth) * throttle;
                    if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
                        let mut data = [0u8; 8];
                        data[0..4].copy_from_slice(&steer.to_le_bytes());
                        data[4..8].copy_from_slice(&throttle.to_le_bytes());
                        let _ = socket.send_to(&data, "10.0.0.134:5001");
                    }
                }
                GameInputState::Wheel(w)=>{
                    let steer: f32 = (w.steering / 0.12).max(-1.0).min(1.0);
                    w.steer_force = (steer*0.7).powf(3.0).max(-1.0).min(1.0);
                    let throttle: f32 = ((w.brake * -1.0) + w.throttle) * limiter;
                    if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
                        let mut data = [0u8; 8];
                        data[0..4].copy_from_slice(&steer.to_le_bytes());
                        data[4..8].copy_from_slice(&throttle.to_le_bytes());
                        let _ = socket.send_to(&data, "10.0.0.134:5001");
                    }
                    break;
                }
            }
        }
    }
    
    fn handle_startup(&mut self, cx:&mut Cx){
        self.window.set_pass(cx, &self.pass);
        self.depth_texture = Texture::new_with_format(cx, TextureFormat::DepthD32{
            size: TextureSize::Auto,
            initial: true,
        });
        self.pass.set_depth_texture(cx, &self.depth_texture, PassClearDepth::ClearWith(1.0));
        self.pass.set_window_clear_color(cx, vec4(0.0, 0.0, 1.0, 0.0));
        cx.start_interval(0.01);
    }

    fn handle_draw_2d(&mut self, cx: &mut Cx2d){
        if !cx.will_redraw(&mut self.main_draw_list, Walk::default()) {
            return
        }

        cx.begin_pass(&self.pass, None);
        self.main_draw_list.begin_always(cx);

        let size = cx.current_pass_size();
        cx.begin_root_turtle(size, Layout::flow_down());

        // draw things here

        cx.end_pass_sized_turtle();
        self.main_draw_list.end(cx);
        cx.end_pass(&self.pass);
    }
        
    fn handle_actions(&mut self, _cx: &mut Cx, _actions:&Actions){
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::GameInputConnected(ev) = event{
            println!("{:?}", ev);
        }
        let _ = self.match_event_with_draw_2d(cx, event);
    }
}
