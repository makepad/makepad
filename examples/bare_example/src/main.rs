use makepad_render::*;
use makepad_widget::*;

live_register!{
    use makepad_render::drawcolor::DrawColor;
    use makepad_render::drawtext::DrawText;
    use makepad_widget::normalbutton::NormalButton;
    use makepad_widget::desktopbutton::DesktopButton;
    App: {
        draw_quad: DrawColor {
            color: #f00
            fn pixel(self) -> vec4 {
                return mix(#f00, #0f0, self.geom_pos.y)
            }
        }
        
        draw_text: DrawText {
        }
        
        normal_button: NormalButton {
        }
         
        desktop_button: DesktopButton{
            
        }
    }
}

main_app!(BareExampleApp);

#[derive(LiveComponent, LiveApply)]
pub struct BareExampleApp {
    #[hide(Window::new(cx))] window: Window,
    #[hide] pass: Pass,
    #[hide(Texture::new(cx))] color_texture: Texture,
    #[hide(View::new())] main_view: View,
    #[live] draw_quad: DrawColor,
    #[live] draw_text: DrawText,
    #[live] normal_button: NormalButton,
    #[live] desktop_button: DesktopButton
}

impl BareExampleApp {
    pub fn live_register(cx: &mut Cx) {
        makepad_widget::live_register(cx);
    }
    
    pub fn new_app(cx: &mut Cx) -> Self {
        //println!("{}",  cx.live_registry.clone().borrow().module_path_id_to_doc(&module_path!(), id!(App)).unwrap().nodes.len()*48);
        Self::new_from_doc(
            cx,
            cx.live_registry.clone().borrow().module_path_id_to_doc(&module_path!(), id!(App)).unwrap()
        )
    }
    
    pub fn myui_button_clicked(&mut self, _cx: &mut Cx) {
    }
    
    pub fn handle_app(&mut self, cx: &mut Cx, event: &mut Event) {
        self.normal_button.handle_normal_button(cx, event);
        
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
            self.draw_quad.draw_quad_abs(cx, Rect {pos: Vec2 {x: 30., y: 30.}, size: Vec2 {x: 200., y: 200.}});
            self.draw_text.draw_text_abs(cx, Vec2 {x: 60., y: 60.}, "HELLO WORLD");
            /*
            self.normal_button.apply_draw(cx, live!{
                label: "DSL",
            });*/
            
            self.desktop_button.draw_desktop_button(cx, DesktopButtonType::WindowsMax );
            self.main_view.end_view(cx);
        }
        
        self.pass.end_pass(cx);
        self.window.end_window(cx);
    }
}

