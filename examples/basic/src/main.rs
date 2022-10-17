use makepad_widgets;
use makepad_widgets::*;
use makepad_draw_2d::*;

// The live_register macro generates a function that registers a DSL code block with the global
// context object (`Cx`).
//
// DSL code blocks are used in Makepad to facilitate live coding. A DSL code block defines
// structured data that describes the styling of the UI. The Makepad runtime automatically
// initializes UI components from their corresponding DSL definitions. Moreover, external programs
// (such as a code editor) can notify the Makepad runtime that a DSL code block has been changed,
// allowing the runtime to automatically update the affected UI components.
live_register! {
    // import frame types
    import makepad_widgets::frame::*;
    // load the widget registry
    registry Widget::*;

    // The App: {{App}} syntax is used to inherit a DSL object from a Rust struct. This tells the
    // Makepad runtime that whenever a Rust struct named `App` is initialized, it should obtain its
    // initial values from the DSL object named `App`. 
    App: {{App}} {
        ui: {
            layout: {flow: Down, spacing: 20, align:{x:0.5,y:0.5}}
            walk: {width: Fill, height: Fill},
            bg: {
                shape: Solid
                // little gradient shader for the background
                fn pixel(self) -> vec4 {
                    return mix(#7, #3, self.geom_pos.y);
                }
            }
            // named button to click
            button1 = Button {
                //walk: {margin: {left: 100, top: 100}}
                text: "Click to count"
            }
            // label to show the counter
            label1 = Label {
                //walk: {margin: {left: 114, top: 20}}
                label: {color: #f},
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
// handling. On desktop, this means creating and running our own event loop. On web, this means
// creating an event handler function that the browser event loop can call into.
main_app!(App);

// main application struct
#[derive(Live, LiveHook)]
pub struct App {
    window: BareWindow,
    ui: FrameRef,
    #[rust] counter: usize
}

impl App {
    // register dependencies, in this case the makepad widgets library
    pub fn live_register(cx: &mut Cx) {
        makepad_widgets::live_register(cx);
    }
    
    // event message pump entry point
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        let ui = self.ui.clone();
        
        // draw events need to be handled with a draw context
        if let Event::Draw(event) = event {
            return Cx2d::draw(cx, event, self, | cx, s | s.draw(cx));
        }
        
        // give the window time to do things
        self.window.handle_event(cx, event);
        
        // call handle event on the frame and return an actions vec
        let actions = ui.handle_event_vec(cx, event);
        
        // the framewrap can be queried for components and events polled
        if ui.get_button(id!(button1)).clicked(&actions) {
            self.counter += 1;
            
            // overwrite our UI structure with an updated value
            let label = ui.get_label(id!(label1));
            
            label.set_text(&format!("Counter: {}", self.counter));
            // cause a redraw to happen
            label.redraw(cx);
        }
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        // if the window is not dirty, don't redraw
        if self.window.begin(cx).not_redrawing() {
            return;
        }
        
        // iterate over any user-draw items in the frame
        while let Some(_) = self.ui.draw(cx).into_not_done() {
        };
        
        self.window.end(cx);
    }
}


