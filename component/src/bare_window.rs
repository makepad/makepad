use crate::makepad_platform::*;

live_register!{
    BareWindow: {{BareWindow}} {
        clear_color: #1e1e1e
        main_view:{},
    }
}

#[derive(Live, LiveHook)]
pub struct BareWindow {
    pass: Pass,
    color_texture: Texture,
    depth_texture: Texture,
    
    window: Window,
    main_view: View, // we have a root view otherwise is_overlay subviews can't attach topmost
    clear_color: Vec4,
}

impl BareWindow {
    /*
    pub fn begin(&mut self, cx: &mut Cx) -> ViewRedraw {
        
        if !self.main_view.view_will_redraw(cx) {
            return Err(())
        }

        self.window.begin(cx);
        
        self.pass.begin(cx);
        self.pass.add_color_texture(cx, &self.color_texture, PassClearColor::ClearWith(self.clear_color));
        self.pass.set_depth_texture(cx, &self.depth_texture, PassClearDepth::ClearWith(1.0));
        
        self.main_view.begin(cx).unwrap();
        
        Ok(())
    }
    
    pub fn end(&mut self, cx: &mut Cx) {
        self.main_view.end(cx);
        self.pass.end(cx);
        self.window.end(cx);
    }*/
}

