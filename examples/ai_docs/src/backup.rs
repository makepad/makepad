// Makepad applications generally include makepad_widgets with a wildcard for all the component apis
use makepad_widgets::*;

// Live design contains the makepad UI DSL code, which is its own syntax. It is not Rust code but something different
// Live design is 'deserialised' into Rust structures, the App below
live_design!{
    import makepad_widgets::base::*;
    // and the theme_desktop_dark contains all the styled components
    import makepad_widgets::theme_desktop_dark::*; 
    
    // App={{App}} binds this data to the App component below. Fields match up
    App = {{App}} {
        // Always leave ui/main_window/body in place
        ui: <Root>{
            main_window = <Window>{
                
                // the Window contains a body field and ScrollXYView is a container with 2 scrollbars
                body = <ScrollXYView>{
                    
                    // each container has width, height, flow, spacing, align, padding and margin
                    // Here is a vertically stacked container with spacing, center alignment and fill width/height:
                    // These containers are an example only, remove when building an application
                    <View>{
                        flow: Down,
                        // The spacing sets the space between the components, depending on the flow direction
                        spacing:10,
                        // Align sets the alignment. x:0.0 is left,  x:1.0 is right, y:0.0 is left, y:1.0 is right and x:0.5 y:0.5 is centered horizontally and vertically
                        align: {
                            x: 0.5,
                            y: 0.5
                        },
                        // the width and height field options are width:100 as a fixed width
                        // width: Fill to fill the entire available space in the parent container
                        // and width: Fit to set the container to be content sized
                        width: Fill // these values are the defaults, and can be omitted
                        height: Fill// these values are the defaults, and can be omitted
                    }
                    
                    // This is a horizontally stacked container with no spacing, left alignment and fixed width/height
                    <View>{ 
                        flow: Right,
                        width: 100 
                        height: 100
                        // now follow the child components.// Component id is placed before the <Tag> 
                        // Button component
                        button1 = <Button> {
                            // the label of the button or other items is generally the text: field
                            text: "Hello world "
                            // setting the color of the text using a CSS hex color is done like so
                            draw_text:{color:#f00}
                        }
                        // Text input component
                        input1 = <TextInput> {
                            // here we set a text input to fixed with
                            width: 100
                            text: "Click to count"
                        }
                        // Basic label component
                        label1 = <Label> {
                            draw_text: {
                                color: #f
                            },
                            text: r#"Lorem ipsum dolor sit amet"#,
                            width: 200.0,
                        }
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
    fn handle_actions(&mut self, cx: &mut Cx, actions:&Actions){
        if self.ui.button(id!(button1)).clicked(&actions) {
        }
    }
}
