
use makepad_widgets::*;

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    
    App = {{App}} {
        ui: <Window> {
            show_bg: true
            width: Fill,
            height: Fill

            body = <ScrollXYView> {
                flow: Down,
                spacing: 20,
                align: {
                    x: 0.5,
                    y: 0.5
                },
                draw_bg: {
                    fn pixel(self) -> vec4 {
                        return mix(#7, #3, self.pos.y);
                    }
                }
                button1 = <Button> {
                    text: "Click me!"
                    draw_text:{
                        color:#fff,
                        text_style: { font_size: 14 }
                    }
                }
                label1 = <Label> {
                    draw_text: {
                        color: #f
                        text_style: { font_size: 14 }
                    },
                    text: "Counter: 0"
                }
            }
        }
    }
}  

app_main!(App); 
 
#[derive(Live, LiveHook)]
pub struct App {
    #[live] ui: WidgetRef,
    #[rust] counter: usize,
 }
 
impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Actions(actions) = event {
            if self.ui.button(id!(button1)).clicked(&actions) {
                log!("BUTTON CLICKED {}", self.counter); 
                self.counter += 1;
                let label = self.ui.label(id!(label1));
                label.set_text(cx,&format!("Counter: {}", self.counter));
            }
        }

        match event.hits(cx, self.ui.area()) {
            Hit::FingerDown(fe) => {
                log!("FingerDown: button {:?}", fe.device.mouse_button());
            },
            Hit::FingerUp(fe) => {
                log!("FingerUp: button {:?}", fe.device.mouse_button());
            },
            _ => ()
        }

        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
