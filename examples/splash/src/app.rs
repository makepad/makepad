use makepad_draw2::*;

app_main!(App); 
script_run!{
    use mod.std;
    std.log("Script started!");
    #(App::script_api(vm)){
        // lets set some properties
        
    }
}

impl App{
    fn run(vm:&mut ScriptVm)->Self{
        crate::makepad_draw2::script_run(vm);
        App::script_run(vm, script_run)
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[script] window: WindowHandle,
    #[script] pass: Pass,
    #[script] depth_texture: Texture,
    #[script] main_draw_list: DrawList2d,
}
 
impl MatchEvent for App{
    fn handle_startup(&mut self, cx:&mut Cx){
        self.window.set_pass(cx, &self.pass);
        self.depth_texture = Texture::new_with_format(cx, TextureFormat::DepthD32{
            size: TextureSize::Auto,
            initial: true,
        });
        self.pass.set_depth_texture(cx, &self.depth_texture, PassClearDepth::ClearWith(1.0));
        self.pass.set_window_clear_color(cx, vec4(0.0, 0.0, 0.0, 1.0));
    }

    fn handle_draw_2d(&mut self, cx: &mut Cx2d){
        if self.main_draw_list.begin(cx, Walk::default()).is_not_redrawing() {
            return;
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
        let _ = self.match_event_with_draw_2d(cx, event);
    }
}
