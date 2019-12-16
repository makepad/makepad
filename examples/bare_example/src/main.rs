use makepad_render::*;

struct App {
    window: Window,
    pass: Pass,
    color_texture: Texture,
    main_view: View,
    quad: Quad
}

main_app!(App);

impl App {
    pub fn proto(cx: &mut Cx) -> Self {
        Self {
            window: Window::proto(cx),
            pass: Pass::default(),
            color_texture: Texture::default(),
            quad: Quad::proto(cx),
            main_view: View::proto(cx),
        }
    }
    
    fn handle_app(&mut self, _cx: &mut Cx, event: &mut Event) {
        match event {
            Event::Construct => {
            },
            _ => ()
        }
    }
    
    fn draw_app(&mut self, cx: &mut Cx) {
        self.window.begin_window(cx);
        self.pass.begin_pass(cx);
        self.pass.add_color_texture(cx, &mut self.color_texture, ClearColor::ClearWith(color256(128, 0, 0)));
        if self.main_view.begin_view(cx, Layout::default()).is_ok() {
            
            self.quad.draw_quad_abs(cx, Rect{x:100.,y:100.,w:100.,h:100.});
            
            self.main_view.end_view(cx);
        }
        self.pass.end_pass(cx);
        self.window.end_window(cx);
    }
}
