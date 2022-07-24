use crate::makepad_platform::*;

live_register!{
    BareWindow: {{BareWindow}} {
        main_view:{}
    }
}

#[derive(Live)]
pub struct BareWindow {
    pass: Pass,
    depth_texture: Texture,
    
    window: Window,
    main_view: View, // we have a root view otherwise is_overlay subviews can't attach topmost
}

impl LiveHook for BareWindow{
    fn after_new(&mut self, cx:&mut Cx){
        self.window.set_pass(cx, &self.pass);
        self.pass.set_depth_texture(cx, &self.depth_texture, PassClearDepth::ClearWith(1.0));
    }
}

impl BareWindow {
    pub fn begin(&mut self, cx: &mut Cx2d) -> ViewRedrawing {
        if !cx.view_will_redraw(&self.main_view) {
            return ViewRedrawing::No
        }
        cx.begin_pass(&self.pass);
        
        self.main_view.begin(cx, Walk::default(), Layout::flow_right()).assume_redrawing();
        
        ViewRedrawing::Yes
    }
    
    pub fn end(&mut self, cx: &mut Cx2d) {
        self.main_view.end(cx);
        cx.end_pass(&self.pass);
    }
}

