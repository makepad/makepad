use crate::makepad_platform::*;

live_register!{
    BareWindow: {{BareWindow}} {
        clear_color: #1e1e1e
        main_view:{},
    }
}

#[derive(Live)]
pub struct BareWindow {
    pass: Pass,
    color_texture: Texture,
    depth_texture: Texture,
    
    window: Window,
    main_view: View2, // we have a root view otherwise is_overlay subviews can't attach topmost
    clear_color: Vec4,
}

impl LiveHook for BareWindow{
    fn after_new(&mut self, cx:&mut Cx){
        self.window.set_pass(cx, &self.pass);
        self.pass.set_depth_texture(cx, &self.depth_texture, PassClearDepth::ClearWith(1.0));
    }
}

impl BareWindow {
    
    pub fn begin(&mut self, cx: &mut Cx2da) -> ViewRedraw {
        if !cx.view_will_redraw(&self.main_view) {
            return Err(())
        }
        cx.begin_pass(&self.pass);
        
        self.main_view.begin(cx).unwrap();
        
        Ok(())
    }
    
    pub fn end(&mut self, cx: &mut Cx2da) {
        self.main_view.end(cx);
        cx.end_pass(&self.pass);
    }
}

