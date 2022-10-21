use makepad_draw_2d::*;
use makepad_widgets;
use makepad_widgets::*;

// The live_design macro generates a function that registers a DSL code block with the global
// context object (`Cx`).
//
// DSL code blocks are used in Makepad to facilitate live design. A DSL code block defines
// structured data that describes the styling of the UI. The Makepad runtime automatically
// initializes widgets from their corresponding DSL objects. Moreover, external programs (such
// as a code editor) can notify the Makepad runtime that a DSL code block has been changed, allowing
// the runtime to automatically update the affected widgets.
live_design! {
    import makepad_widgets::button::Button;
    import makepad_widgets::label::Label;

    // The `{{App}}` syntax is used to inherit a DSL object from a Rust struct. This tells the
    // Makepad runtime that our DSL object corresponds to a Rust struct named `App`. Whenever an
    // instance of `App` is initialized, the Makepad runtime will obtain its initial values from
    // this DSL object.
    App = {{App}} {
        // The `ui` field on the struct `App` defines a frame widget. Frames are used as containers
        // for other widgets. Since the `ui` property on the DSL object `App` corresponds with the
        // `ui` field on the Rust struct `App`, the latter will be initialized from the DSL object
        // here below.
        ui: {
            // The `layout` property determines how child widgets are laid out within a frame. In
            // this case, child widgets flow downward, with 20 pixels of spacing in between them,
            // and centered horizontally with respect to the entire frame.
            //
            // Because the child widgets flow downward, vertical alignment works somewhat
            // differently. In this case, children are centered vertically with respect to the
            // remainder of the frame after the previous children have been drawn.
            layout: {
                flow: Down,
                spacing: 20,
                align: {
                    x: 0.5,
                    y: 0.5
                }
            },
            // The `walk` property determines how the frame widget itself is laid out. In this
            // case, the frame widget takes up the entire window.
            walk: {
                width: Fill,
                height: Fill
            },
            bg: {
                shape: Solid

                // The `fn pixel(self) -> vec4` syntax is used to define a property named `pixel`,
                // the value of which is a shader. We use our own custom DSL to define shaders. It's
                // syntax is *mostly* compatible with GLSL, although there are some differences as
                // well.
                fn pixel(self) -> vec4 {
                    // Within a shader, the `self.geom_pos` syntax is used to access the `geom_pos`
                    // attribute of the shader. In this case, the `geom_pos` attribute is built in,
                    // and ranges from 0 to 1. over x and y of the rendered rectangle
                    return mix(#7, #3, self.geom_pos.y);
                }
            }

            // The `name:` syntax is used to define fields, i.e. properties for which there are
            // corresponding struct fields. In contrast, the `name =` syntax is used to define
            // instance properties, i.e. properties for which there are no corresponding struct
            // fields. Note that fields and instance properties use different namespaces, so you
            // can have both a field and an instance property with the same name.
            //
            // Widgets can hook into the Makepad runtime with custom code and determine for
            // themselves how they want to handle instance properties. In the case of frame widgets,
            // they simply iterate over their instance properties, and use them to instantiate their
            // child widgets.

            // A button to increment the counter.
            //
            // The `<Button>` syntax is used to inherit a DSL object from another DSL object. This
            // tells the Makepad runtime our DSL object has the same properties as the DSL object
            // named `Button`, except for the properties defined here below, which override any
            // existing values.
            button1 = <Button> {
                text: "Click to count"
            }

            // A label to display the counter.
            label1 = <Label> {
                label: {
                    color: #f
                },
                text: "Counter: 0"
            }
        }
    }
}

// This main_app macro generates the code necessary to initialize and run your application.
//
// This code is almost always the same between different applications, so it is convenient to use a
// macro for it. The two main tasks that this code needs to carry out are: initializing both the
// main application struct (`App`) and the global context object (`Cx`), and setting up event
// handling. On desktop, this means creating and running our own event loop from a fn main(). On web, this means
// creating an event handler function that the browser event loop can call into.
main_app!(App);

// The main application struct.
//
// The #[derive(Live, LiveHook)] attribute implements a bunch of traits for this struct that enable
// it to interact with the Makepad runtime. Among other things, this enables the Makepad runtime to
// initialize the struct from a DSL object.
#[derive(Live, LiveHook)]
pub struct App {
    // A chromeless window for our application. Used to contain our frame widget.
    window: BareWindow,
    // A frame widget. Used to contain our button and label.
    ui: FrameRef,
    
    // The value for our counter.
    //
    // The #[rust] attribute here is used to indicate that this field should *not* be initialized
    // from a DSL object, even when a corresponding property exists. 
    #[rust]
    counter: usize,
}

impl App {
    // This function is used to register any DSL code blocks that you defined in this file. It is
    // called automatically by the code we generated with the call to the macro `main_app` above.
    // In this function you have to register any dependency crates live design code
    pub fn live_design(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
    }

    // This function is used to handle any incoming events from the host system. It is called
    // automatically by the code we generated with the call to the macro `main_app` above.
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Draw(event) = event {
            // This is a draw event, so create a draw context and use that to draw our application.
            let mut draw_cx = Cx2d::new(cx, event);
            return self.draw(&mut draw_cx);
        }

        // Forward the event to the window.
        self.window.handle_event(cx, event);

        // Forward the event to the frame. In this case, handle_event returns a list of actions.
        // Actions are similar to events, except that events are always forwarded downward to child
        // widgets, while actions are always returned back upwards to parent widgets.
        let actions = self.ui.handle_event(cx, event);

        // Get a reference to our button from the frame, and check if one of the actions returned by
        // the frame was a notification that the button was clicked.
        if self.ui.get_button(id!(button1)).clicked(&actions) {
            // Increment the counter.
            self.counter += 1;

            // Get a reference to our label from the frame, update its text, and schedule a redraw
            // for it.
            let label = self.ui.get_label(id!(label1));
            label.set_text(&format!("Counter: {}", self.counter));
            label.redraw(cx);
        }
    }

    // This is the immediate mode draw flow, as called above in response to the Draw event
    pub fn draw(&mut self, cx: &mut Cx2d) {
        // Indicate that we want to begin drawing to the window.
        if self.window.begin(cx).not_redrawing() {
            return;
        }
        
        // Draw the frame to the window.
        let _ = self.ui.draw(cx);

        // Indicate that we finished drawing to the window.
        self.window.end(cx);
    }
}
