// Below is an application with a vertically stacked set of buttons
use makepad_widgets::*;

// Below is an application with a vertically stacked set of buttons
live_design!{
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*; 
    App = {{App}} {
        ui: <Root>{
            main_window = <Window>{
                body = <ScrollXYView>{
                    flow: Down,
                    button1 = <Button> {
                        text: "Button 1"
                        draw_text:{color:#f00}
                    }
                    button2 = <Button> {
                        text: "Button 2"
                        draw_text:{color:#f00}
                    }
                }
            }
        }
    }
}
/*
// Below is an application with a horizontally stacked set of buttons and a slider
live_design!{
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*; 
    App = {{App}} {
        ui: <Root>{
            main_window = <Window>{
                body = <ScrollXYView>{
                    flow: Down // vertical stacked
                    <View>{ 
                        width: Fit // content sized
                        height: Fit
                        flow: Right, // horizontally stacked
                        spacing: 10.0 // spacing between items
                        button1 = <Button> {
                            text: "Button 1"
                            draw_text:{color:#f00}
                        }
                        button2 = <Button> {
                            text: "Button 2"
                            draw_text:{color:#f00}
                        }
                    }
                    <View>{ 
                        width: Fill // fill the parent container
                        height: 100 // fixed width
                        flow: Down, // vertical stacked
                        slider1 = <Slider> {
                            text: "Slider 1"
                            min: 0
                            max: 100
                            step: 0.1
                        }
                    }
                }
            }
        }
    }
}

// Below is an application with a vertical gradient background shader
live_design!{
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*; 
    App = {{App}} {
        ui: <Root>{
            main_window = <Window>{
                body = <ScrollXYView>{
                    flow: Down
                    // all containers can have a shader background with these options
                    show_bg: true,
                    draw_bg:{
                        // this shader syntax is NOT Rust code but comparable to GLSL
                        fn pixel(self)->vec4{
                            return mix(#f00,#0f0,self.pos.y)
                        }
                    }
                }
            }
        }
    }
}

// Below is an application using the SDF api to make an icon with 2 overlaid filled boxes
live_design!{
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*; 
    import makepad_widgets::theme_desktop_dark::*; 
    App = {{App}} {
        ui: <Root>{
            main_window = <Window>{
                body = <ScrollXYView>{
                    flow: Down
                    show_bg: true,
                    draw_bg:{
                        // this shader syntax is NOT Rust code but comparable to GLSL
                        fn pixel(self)->vec4{
                            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                            sdf.box(
                                0.,
                                0.,
                                self.rect_size.x,
                                self.rect_size.y,
                                2.
                            );
                            sdf.fill(#f3)
                            sdf.box(
                                10.,
                                10.,
                                50.,
                                50.,
                                3.
                            );
                            sdf.fill(#f00);
                            return sdf.result;
                        }
                    }
                }
            }
        }
    }
}*/

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
        if self.ui.button(id!(button1)).clicked(&actions) {
        }
    }
}
