use crate::makepad_platform::*;
use crate::debug_view::DebugView;
use crate::nav_control::NavControl;

live_register!{
    BareWindow: {{BareWindow}} {
        main_view:{}
    }
}

#[derive(Live)]
pub struct BareWindow {
    pass: Pass,
    depth_texture: Texture,
    debug_view: DebugView,
    nav_control: NavControl,
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
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event){
        self.debug_view.handle_event(cx,event);
        self.nav_control.handle_event(cx, event);
    }
    
    pub fn begin(&mut self, cx: &mut Cx2d) -> ViewRedrawing {
        if !cx.view_will_redraw(&self.main_view) {
            return ViewRedrawing::no()
        }
        cx.begin_pass(&self.pass);
        
        self.main_view.begin(cx, Walk::default(), Layout::flow_right()).assume_redrawing();
        
        ViewRedrawing::yes()
    }
    
    pub fn end(&mut self, cx: &mut Cx2d) {
        self.debug_view.draw(cx);
        self.main_view.end(cx);
        cx.end_pass(&self.pass);
    }
}

