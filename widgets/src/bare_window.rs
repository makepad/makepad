use crate::{
    makepad_draw_2d::*,
    debug_view::DebugView,
    nav_control::NavControl,
};

live_design!{
    BareWindow= {{BareWindow}} {
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
    overlay: Overlay,
    main_view: View, // we have a root view otherwise is_overlay subviews can't attach topmost
}

impl LiveHook for BareWindow{
    fn after_new_before_apply(&mut self, cx:&mut Cx){
        self.window.set_pass(cx, &self.pass);
        self.pass.set_depth_texture(cx, &self.depth_texture, PassClearDepth::ClearWith(1.0));
    }
}

impl BareWindow {
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event){
        self.debug_view.handle_event(cx,event);
        self.nav_control.handle_event(cx, event, self.main_view.draw_list_id());
        self.overlay.handle_event(cx, event);
    }
    
    pub fn begin(&mut self, cx: &mut Cx2d) -> ViewRedrawing {
        if !cx.view_will_redraw(&self.main_view) {
            return ViewRedrawing::no()
        }
        cx.begin_pass(&self.pass);
        
        self.main_view.begin_always(cx);

        cx.begin_overlay_turtle(Layout::flow_down());
        
        self.overlay.begin(cx);
        
        ViewRedrawing::yes()
    }
    
    pub fn end(&mut self, cx: &mut Cx2d) {
        self.debug_view.draw(cx);
        // we need an overlay view here
        // however this overlay view works 
        self.overlay.end(cx);

        cx.end_overlay_turtle();

        self.main_view.end(cx);
        cx.end_pass(&self.pass);
        //cx.debug_draw_tree(false, self.main_view.draw_list_id());
    }
}

