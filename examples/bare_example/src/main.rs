use makepad_render::*;
use makepad_widget::*;

live_body!{
    use makepad_render::drawcolor::DrawColor;
    use makepad_render::drawtext::DrawText;
    use makepad_widget::normalbutton::NormalButton;
    
    App: Component {
        rust_type: {{BareExampleApp}}
        draw_quad: DrawColor {
            color: #f00
            fn pixel(self) -> vec4 {
                return mix(#f00, #0f0, self.geom_pos.y)
            }
        }
        draw_text: DrawText{
        }
        normal_button: NormalButton{
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
    #[live()] draw_text: DrawText,
    #[live()] normal_button: NormalButton
}

impl BareExampleApp {
    pub fn live_register(cx: &mut Cx){
        makepad_widget::live_register(cx); 
    }
  
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
            self.draw_text.draw_text_abs(cx, Vec2{x:60.,y:60.}, "HELLO WORLD");
            self.normal_button.draw_normal_button(cx, "HELLO");
            self.main_view.end_view(cx);
        }
        self.pass.end_pass(cx);
        self.window.end_window(cx);
    }
}

