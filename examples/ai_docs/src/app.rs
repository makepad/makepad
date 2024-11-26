// tis is an example application with a horizontal gradient
use makepad_widgets::*;

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    App = {{App}} {
        ui: <Root>{
            main_window = <Window>{
                body = <ScrollXYView>{
                    flow: Down,
                    align:{
                        x:0.5, // align horizontal center, 0.0=left, 1.0=right
                        y:0.5 // align vertical center, 0.0=top, 1.0= right
                    }
                    // this shader syntax is NOT Rust code but comparable to GLSL. Do NOT write Rust code in these blocks
                    fn pixel(self)->vec4{
                        // make a vertical background shader
                        return mix(#ccc,#333,self.pos.y)
                    }
                    button1 = <Button> {
                        text: "Button 1"
                    }
                }
            }
        }
    }
}

// The following Rust code is the minimum required for an applicaiton
app_main!(App); 
 
// This is the application struct, you can add datafields to this if need be
#[derive(Live, LiveHook)]
pub struct App {
    #[live] ui: WidgetRef,
    #[rust] counter: usize,
 }
 
 // This registers the widget library, do not omit this
impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
    }
}

// The AppMain trait implementation, leave this in tact
impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

// THe following code can be changed depending on what components are in use
impl MatchEvent for App{
    fn handle_actions(&mut self, _cx: &mut Cx, actions:&Actions){
        if self.ui.button(id!(button1)).clicked(&actions){
           self.counter += 1;
        }
    }
}
