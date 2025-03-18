use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*
};

live_design!{
    link widgets;
    use link::shaders::*;
    
    DrawWebView= {{DrawWebView}} {
        texture image: texture2d
        opacity: 1.0
        image_scale: vec2(1.0, 1.0)
        image_pan: vec2(0.0, 0.0)
        
        fn get_color_scale_pan(self, scale: vec2, pan: vec2) -> vec4 {
            return sample2d(self.image, self.pos * scale + pan).xyzw;
        }
                                
        fn get_color(self) -> vec4 {
            return self.get_color_scale_pan(self.image_scale, self.image_pan)
        }
        
        fn pixel(self) -> vec4 {
            let color = self.get_color();
            return Pal::premul(vec4(color.xyz, color.w * self.opacity))
        }
    }
    
    pub WebViewBase = {{WebView}} {}
    pub WebView = <WebViewBase> {
        width: 100
        height: 100
        draw_bg: {}
    }
}

#[derive(Live, LiveHook, LiveRegister)]
#[repr(C)]
pub struct DrawWebView {
    #[deref] draw_super: DrawQuad,
    #[live] pub opacity: f32,
    #[live] image_scale: Vec2,
    #[live] image_pan: Vec2,
}

#[derive(Live, Widget, LiveHook)]
pub struct WebView {
    #[walk] walk: Walk,
    #[redraw] #[live] pub draw_bg: DrawWebView,
    #[live] url: ArcStringMut,
    #[rust] _texture: Option<Texture>,
    #[rust] _web_view_id: Option<LiveId>,
}

impl Widget for WebView {
    fn handle_event(&mut self, _cx:&mut Cx, _event:&Event, _scope:&mut Scope){
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_walk(cx, walk)
    }
}

impl WebView {
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) -> DrawStep {
        self.draw_bg.draw_walk(cx, walk);
        DrawStep::done()
    }
}

impl WebViewRef {
}

