use makepad_render::*;
use makepad_widget::*;

live_register!{
    use makepad_render::drawcolor::DrawColor;
    use makepad_render::drawtext::DrawText;
    use makepad_widget::normalbutton::NormalButton;
    use makepad_widget::desktopwindow::DesktopWindow;
    App: {
        draw_quad: DrawColor {
            color: #f00
            fn pixel(self) -> vec4 {
                return mix(#f00, #0f0, self.geom_pos.y)
            }
        }
        desktop_window: DesktopWindow{}
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
    #[live] desktop_window: DesktopWindow,
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
        println!("{}",  cx.live_registry.clone().borrow().module_path_id_to_doc(&module_path!(), id!(App)).unwrap().nodes.len()*48);
        Self::new_from_doc(
            cx,
            cx.live_registry.clone().borrow().module_path_id_to_doc(&module_path!(), id!(App)).unwrap()
        )
    }
    
    pub fn myui_button_clicked(&mut self, _cx: &mut Cx) {
    }
    
    pub fn handle_app(&mut self, cx: &mut Cx, event: &mut Event) {
        self.desktop_window.handle_desktop_window(cx, event);
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
        if self.desktop_window.begin_desktop_window(cx, None).is_err(){
            return;
        }
        
        self.draw_quad.draw_quad_abs(cx, Rect {pos: Vec2 {x: 30., y: 30.}, size: Vec2 {x: 200., y: 200.}});
        self.draw_text.draw_text_abs(cx, Vec2 {x: 60., y: 60.}, "HELLO WORLD");
        
        self.normal_button.apply_draw(cx, live!{ 
            label: "DSL",
        });
        
        self.desktop_button.draw_desktop_button(cx, DesktopButtonType::WindowsMax );
        
        self.desktop_window.end_desktop_window(cx);
    }
}

