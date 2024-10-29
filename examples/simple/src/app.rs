
use makepad_widgets::*;

live_design!{
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    App = {{App}} {
        ui: <Root>{
            main_window = <Window>{
                body = <ScrollXYView>{
                    flow: Down,
                    spacing:10,
                    align: {
                        x: 0.5,
                        y: 0.5
                    },
                    button1 = <Button> {
                        text: "Show/hide password"
                        draw_text:{color:#f00}
                    }
                    input1 = <TextInput> {
                        width: 100
                        text: "Your password here"
                        draw_text: { text_style: { is_secret: true } },
                    }
                    label1 = <Label> {
                        draw_text: {
                            color: #f
                        },
                        text: "This is a label",
                        width: 200.0,
                    }
                }
            }
        }
    }
}

app_main!(App); 
#[derive(Live, LiveHook)]
pub struct App {
    #[live] ui: WidgetRef,
 }
 
impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
    }
}
impl MatchEvent for App{
    fn handle_actions(&mut self, cx: &mut Cx, actions:&Actions){
        if self.ui.button(id!(button1)).clicked(&actions) {
            let text_input = self.ui.text_input(id!(input1));
            let mut text_input = text_input.borrow_mut().unwrap();
            text_input.draw_text.text_style.is_secret = !text_input.draw_text.text_style.is_secret;
            text_input.redraw(cx);
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}