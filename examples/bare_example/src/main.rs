use makepad_render::*;
use makepad_widget::*;

live_register!{
    App: {{App}} {
        use makepad_widget::frame::Frame;
        use makepad_widget::button::Button;
        frame: {
            b1: Button {label: "btn1"}
            b2: Button {label: "btn2"}
            frame1: Frame {
                b3: Button {label: "btn3"}
                children: [b3]
            }
            children: [b1,b2, frame1]
        }
    }
}
main_app!(App);

#[derive(LiveComponent, LiveApply, LiveCast)]
pub struct App {
    #[live] desktop_window: DesktopWindow,
    #[live] frame: Frame
}

impl App {
    pub fn live_register(cx: &mut Cx) {
        makepad_widget::live_register(cx);
    }
    
    pub fn new_app(cx: &mut Cx) -> Self {
        Self::new_from_doc(cx, get_local_doc!(cx, id!(App)))
    }

    pub fn handle_app(&mut self, cx: &mut Cx, event: &mut Event) {
        self.desktop_window.handle_desktop_window(cx, event);

        for item in self.frame.handle_frame(cx, event) {
            if let ButtonAction::Pressed = item.action.cast() {
                println!("Clicked on button {}", item.id);
                // mess with the frame structure
                self.frame.apply_live(cx, live!{
                    b2:{label:"HOOOO"},
                    frame1:{b3:{label:"whoop"}},
                    children:[b2,b1,frame1] 
                });
                cx.redraw_all()
            }
        }
    }
    
    pub fn draw_app(&mut self, cx: &mut Cx) {
        if self.desktop_window.begin_desktop_window(cx, None).is_err() {
            return;
        }

        if let Some(button) = get_component!(id!(b1), Button, self.frame) {
            button.label = "Btn1 label override".to_string();
        }
        self.frame.draw_frame(cx);
        
        self.desktop_window.end_desktop_window(cx);
    }
}