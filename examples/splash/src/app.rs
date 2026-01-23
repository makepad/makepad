use makepad_draw2::*;

app_main!(App); 
script_mod!{
    use mod.pod.*
    use mod.math.*
    use mod.sdf.*
    mod.res.load_all()
    #(App::script_component(vm)){
        draw_quad: mod.shaders.DrawQuad{ 
            pixel: ||{
                let sdf = Sdf2d.viewport(self.pos*self.rect_size)
                sdf.circle(40 40 35)
                sdf.fill(mix(#0f0 #f00 self.pos.y))
                sdf.result
            }
        }
        draw_text: mod.shaders.DrawText{
        }
    }
}

impl App{
    fn run(vm:&mut ScriptVm)->Self{
        crate::makepad_draw2::script_mod(vm);
        App::from_script_mod(vm, self::script_mod)
    }
}

#[derive(Script)]
pub struct App {
    #[new] window: WindowHandle,
    #[new] pass: DrawPass,
    #[new] depth_texture: Texture,
    #[live] draw_quad: DrawQuad,
    #[live] draw_text: DrawText,
    #[new] main_draw_list: DrawList2d,
}

impl ScriptHook for App{
    fn on_before_apply(&mut self, _vm:&mut ScriptVm, _apply:&mut ApplyScope, _value:ScriptValue){
    }
}

impl MatchEvent for App{
    
    fn handle_startup(&mut self, cx:&mut Cx){
        self.window.set_pass(cx, &self.pass);
        self.depth_texture = Texture::new_with_format(cx, TextureFormat::DepthD32{
            size: TextureSize::Auto,
            initial: true,
        });
        self.pass.set_depth_texture(cx, &self.depth_texture, DrawPassClearDepth::ClearWith(1.0));
        self.pass.set_window_clear_color(cx, vec4(0.0, 0.0, 1.0, 0.0));
    }

    fn handle_draw_2d(&mut self, cx: &mut Cx2d){
        if !cx.will_redraw(&mut self.main_draw_list, Walk::default()) {
            return
        }

        cx.begin_pass(&self.pass, None);
        self.main_draw_list.begin_always(cx);

        let size = cx.current_pass_size();
        cx.begin_root_turtle(size, Layout::flow_down());
        
        self.draw_quad.draw_abs(cx, rect(10.,10.,200.,100.));
        self.draw_text.draw_abs(cx, dvec2(10., 10.), "TEST");
        
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
