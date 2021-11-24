use makepad_render::*;
use makepad_widget::*;

live_register!{
    App: {{BareExampleApp}} {
        use makepad_widget::frame::Frame;
        use makepad_widget::button::Button;
        
        frame: {
            b1: Button {label: "btn1"}
            b2: Button {label: "btn2"}
            b3: Button {label: "btn3"}
            frame1: Frame {
                children: [b3]
            }
            children: [b1, b2, frame1]
        }
    }
}

main_app!(BareExampleApp);

#[derive(LiveComponent, LiveApply, LiveCast)]
pub struct BareExampleApp {
    #[live] desktop_window: DesktopWindow,
    #[live] frame: Frame
}

impl BareExampleApp {
    pub fn live_register(cx: &mut Cx) {
        makepad_widget::live_register(cx);
    }
    
    pub fn new_app(cx: &mut Cx) -> Self {
        Self::new_from_doc(cx, cx.live_registry.clone().borrow().module_path_str_id_to_doc(&module_path!(), id!(App)).unwrap())
    }
    
    pub fn handle_app(&mut self, cx: &mut Cx, event: &mut Event) {
        self.desktop_window.handle_desktop_window(cx, event);
        for item in self.frame.handle_frame(cx, event) {
            if let ButtonAction::Clicked = item.action.cast() {
                println!("Clicked on button {}", item.id);
            }
        }
    }
    
    pub fn draw_app(&mut self, cx: &mut Cx) {
        if self.desktop_window.begin_desktop_window(cx, None).is_err() {
            return;
        }
        // hard access the button component
        if let Some(button) = get_component!(id!(b1), Button, self.frame) {
            button.label = "hard type".to_string();
        }
        self.frame.draw_frame(cx);
        
        self.desktop_window.end_desktop_window(cx);
    }
}