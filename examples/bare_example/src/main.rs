use makepad_render::*;

live_body!{
    use makepad_render::drawcolor::DrawColor;
    use makepad_render::drawtext::DrawText;

    App: Component {
        rust_type: {{BareExampleApp}}
        
        draw_quad: DrawColor {
            color: #f00;
            fn pixel(self) -> vec4 {
                return mix(#f00, #0f0, self.geom_pos.y);
            }
        }

        draw_text: DrawText{
        }
    }
}

main_app!(BareExampleApp);

#[derive(Live, LiveUpdateHooks)]
pub struct BareExampleApp {
    #[hidden(Window::new(cx))] window: Window,
    #[hidden()] pass: Pass,
    #[hidden(Texture::new(cx))] color_texture: Texture,
    #[hidden(View::new())] main_view: View,
    #[live()] draw_quad: DrawColor,
    #[live()] draw_text: DrawText
}

impl BareExampleApp {
    pub fn new(cx: &mut Cx) -> Self {
        let mut new = Self::live_new(cx);
        new.live_update(cx, cx.live_ptr_from_id(&module_path!(), id!(App)));
        new
    }
    
    pub fn myui_button_clicked(&mut self, _cx: &mut Cx) {
    }
    
    pub fn handle_app(&mut self, _cx: &mut Cx, event: &mut Event) {
        
        match event {
            Event::Construct => {
                
            },
            Event::FingerMove(_fm) => {
                //self.count = fm.abs.x * 0.01;
            },
            _ => ()
        }
    }
    
    pub fn draw_app(&mut self, cx: &mut Cx) {
        
        self.window.begin_window(cx);
        self.pass.begin_pass(cx);
        self.pass.add_color_texture(cx, self.color_texture, ClearColor::ClearWith(Vec4::color("000")));
        if self.main_view.begin_view(cx, Layout::default()).is_ok() {
            self.draw_quad.draw_quad_abs(cx, Rect {pos: Vec2 {x: 30., y: 30.}, size: Vec2 {x: 100., y: 100.}});
            /*
        while let Some(custom) = self.live.draw_live(cx){
            match custom.id_path{
            }
        }*/
            /*
            self.quad.counter = 0.;
            self.quad.begin_many(cx);
            
            self.quad.counter = 0.;
            self.quad.some = 0.;
            
            for i in 0..1000 {  
                let v = 0.5 * (i as f32);
                self.quad.counter += 0.01; //= (i as f32).sin();
                let x = 400. + (v + self.count).sin() * 400.;
                let y = 400. + (v * 1.12 + self.count * 18.).cos() * 400.;
                self.quad.draw_quad_abs(cx, Rect {pos: vec2(x, y), size: vec2(10., 10.0)});
            }
            self.quad.end_many(cx);
            self.count += 0.001;
            self.main_view.redraw_view(cx);*/
            self.main_view.end_view(cx);
        }
        self.pass.end_pass(cx);
        self.window.end_window(cx);
    }
}

