use makepad_widgets::*;

live_design!{
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    
    App = {{App}} {

        ui: <Window>{
            show_bg: true
            width: Fill,
            height: Fill
            body = {
                <SlidesView> {
                    goal_pos: 0.0
                    
                    <SlideChapter> {
                        title = {text: "MAKEPAD.\nDESIGNING MODERN\nUIs FOR RUST."},
                        <SlideBody> {text: "Rik Arends\n"}
                    }
                    <Slide> {
                        title = {text: "A long long time ago …"},
                        <SlideBody> {text: "… in a galaxy nearby\n   Cloud9 IDE & ACE"}
                    }
                    <Slide> {
                        title = {text: "HTML as an IDE UI?\nMadness!"},
                        <SlideBody> {text: "- Integrating design and code was hard\n- Could not innovate editing\n- Too slow, too hard to control"}
                    }
                    <Slide> {
                        title = {text: "Let's start over!"},
                        <SlideBody> {text: "- JavaScript and WebGL for UI\n- Write shaders to style UI\n- A quick demo"}
                    }
                    <Slide> {
                        title = {text: "Maybe JavaScript\nwas the problem?"},
                        <SlideBody> {text: "- Great livecoding, but …\n- Chrome crashing tabs after 30 minutes\n- Too slow"}
                    }
                    <Slide> {
                        title = {text: "Rust appears"},
                        <SlideBody> {text: "- Let's try again: Native + Wasm\n- Makepad in Rust\n- Startup with Eddy and Sebastian"}
                    }
                    <Slide> {title = {text: "Rust is fast: SIMD Mandelbrot"}, 
                        align: {x: 0.0, y: 0.5} flow: Down, spacing: 10, padding: 50
                        draw_bg: { color: #x1A, radius: 5.0 }

                    }
    
                    <Slide> {title = {text: "Instanced rendering"}, 
                        align: {x: 0.0, y: 0.5} flow: Down, spacing: 10, padding: 50
                        draw_bg: { color: #x1A, radius: 5.0 }

                    }
                    
                    <Slide> {
                        title = {text: "Our goal:\nUnify coding and UI design again."},
                        <SlideBody> {text: "As it was in Visual Basic.\nNow with modern design."}
                    }
    
                    <Slide> {title = {text: "Ironfish Desktop"}, 

                    }
                    
                    <Slide> {title = {text: "Ironfish Mobile"}, 

                    }
                    
                    <Slide> {title = {text: "Multi modal"}, 

                    }
                    
                    <Slide> {title = {text: "Visual design"}, 

                    }
                    
                    <Slide> {
                        title = {text: "Our UI language: Live."},
                        <SlideBody> {text: "- Live editable\n- Design tool manipulates text\n- Inheritance structure\n- Rust-like module system"}
                    }
                    
                    <Slide> {
                        title = {text: "These slides are a Makepad app"},
                        <SlideBody> {text: "- Show source\n"}
                        <SlideBody> {text: "- Show Rust API\n"}
                    }                
                    
                    <Slide> {
                        title = {text: "Future"},
                        <SlideBody> {text: "- Release of 0.4.0 soon\n- Windows, Linux, Mac, Web and Android\n- github.com/makepad/makepad\n- twitter: @rikarends @makepad"}
                    }                
                    
                    <Slide> {
                        title = {text: "Build for Android"},
                        <SlideBody> {text: "- SDK installer\n- Cargo makepad android\n"}
                    }                
                }
            }
        }
    }
}

// This app_main macro generates the code necessary to initialize and run your application.
//
// This code is almost always the same between different applications, so it is convenient to use a
// macro for it. The two main tasks that this code needs to carry out are: initializing both the
// main application struct (`App`) and the global context object (`Cx`), and setting up event
// handling. On desktop, this means creating and running our own event loop. On web, this means
// creating an event handler function that the browser event loop can call into.
app_main!(App);

// The main application struct.
//
// The #[derive(Live, LiveHook)] attribute implements a bunch of traits for this struct that enable
// it to interact with the Makepad runtime. Among other things, this enables the Makepad runtime to
// initialize the struct from a DSL object.
#[derive(Live)]
// This function is used to register any DSL code that you depend on.
// called automatically by the code we generated with the call to the macro `main_app` above.
pub struct App {
    // A chromeless window for our application. Used to contain our frame widget.
    // A frame widget. Used to contain our button and label.
    #[live] ui: WidgetRef,
    
    // The value for our counter.
    //
    // The #[rust] attribute here is used to indicate that this field should *not* be initialized
    // from a DSL object, even when a corresponding property exists.
    #[rust] counter: usize,
}

impl LiveHook for App {
    fn before_live_design(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
    }
}

impl App{
    async fn _do_network_request(_cx:CxRef, _ui:WidgetRef, _url:&str)->String{
        //let x = fetch(urL).await;
        //ui.get_label(id!(thing)).set_text(&mut *cx.borrow_mut(), x);
        "".to_string()
    }
}

impl AppMain for App{
    
    
    // This function is used to handle any incoming events from the host system. It is called
    // automatically by the code we generated with the call to the macro `main_app` above.
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Draw(event) = event {
            // This is a draw event, so create a draw context and use that to draw our application.
            return self.ui.draw_widget_all(&mut Cx2d::new(cx, event));
        }
        
        // Forward the event to the frame. In this case, handle_event returns a list of actions.
        // Actions are similar to events, except that events are always forwarded downward to child
        // widgets, while actions are always returned back upwards to parent widgets.
        let actions = self.ui.handle_widget_event(cx, event);
        
        // Get a reference to our button from the frame, and check if one of the actions returned by
        // the frame was a notification that the button was clicked.
        if self.ui.button(id!(button1)).clicked(&actions) {
            //cx.spawn_async(Self::do_network_request(cx.get_ref(), self.ui.clone()))
            // Increment the counter.
            self.counter += 1;
            
            // Get a reference to our label from the frame, update its text, and schedule a redraw
            // for it.
            let label = self.ui.label(id!(label1));
            label.set_text_and_redraw(cx,&format!("Counter: {}", self.counter));
        }
    }
}