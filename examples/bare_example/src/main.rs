use makepad_render::*;
use makepad_widget::*;

live_register!{
    App: {{BareExampleApp}} {
        /*draw_quad: {
            color: #f0f
            fn pixel(self) -> vec4 {
                return mix(self.color, #0f0, self.geom_pos.y)
            }
        }*/
        use makepad_widget::frame::Frame;
        use makepad_widget::button::Button;
        frame:{
            b1:Button{label:"hi"}
            b2:Button{label:"ho"}
            b3:Button{label:"ho"}
            frame1:Frame{children:[b3]}
            children:[b1, b2, b3, frame1]
        }
    }
}

main_app!(BareExampleApp);

#[derive(LiveComponent, LiveApply, LiveCast)]
pub struct BareExampleApp {
    #[live] desktop_window: DesktopWindow,
    #[live] frame: Frame
    //#[live] draw_quad: DrawColor,
    //#[live] draw_text: DrawText,
    //#[live] normal_button: NormalButton,
    //#[live] desktop_button: DesktopButton
}

impl BareExampleApp {
    pub fn live_register(cx: &mut Cx) { 
        makepad_widget::live_register(cx);
    }
    
    pub fn new_app(cx: &mut Cx) -> Self {
        println!("{}",  std::mem::size_of::<LiveNode>());
        println!("{}",  cx.live_registry.clone().borrow().module_path_str_id_to_doc(&module_path!(), id!(App)).unwrap().nodes.len()*40);
        Self::new_from_doc(
            cx,
            cx.live_registry.clone().borrow().module_path_str_id_to_doc(&module_path!(), id!(App)).unwrap()
        )
    }
    
    pub fn myui_button_clicked(&mut self, _cx: &mut Cx) {
    }
    
    pub fn handle_app(&mut self, cx: &mut Cx, event: &mut Event) {
        self.desktop_window.handle_desktop_window(cx, event);
        self.frame.handle_frame(cx, event);
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
        
        self.frame.draw_frame(cx);
        
        /*
        self.draw_quad.draw_quad_abs(cx, Rect {pos: Vec2 {x: 30., y: 30.}, size: Vec2 {x: 200., y: 200.}});
        self.draw_text.draw_text_abs(cx, Vec2 {x: 60., y: 60.}, "HELLO WORLD");
        
        self.normal_button.apply_draw(cx, live!{ 
            label: "DSL",
        });
        
        self.desktop_button.draw_desktop_button(cx, DesktopButtonType::WindowsMax );
        */
        self.desktop_window.end_desktop_window(cx);
    }
}

