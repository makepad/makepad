use makepad_render::*;
use makepad_widget::*;

live_register!{
    App: {{App}} { // the {{Type}} makes a reference to the Rust type App 
        // the DSL has its own module namespaces
        use makepad_widget::frame::Frame;
        use makepad_widget::button::Button;
        // override the frame component with data:
        frame: {
            // this 'clones' the Button struct above and overrides label
            b1: Button {label: "btn1"}
            b2: Button {label: "btn2"}
            b3: Button {label: "btn3"}
            // you can use Frame itself as a dynamic component too
            frame1: Frame {
                children: [b3]
            }
            // this is just an array of identifiers resolved on our own scope by the Frame component
            children: [b1, b2, frame1]
        }
    }
}

main_app!(App);

#[derive(LiveComponent, LiveApply, LiveCast)]
// this is the main App struct, with hard-typed subcomponents (window and frame in this case)
pub struct App {
    #[live] desktop_window: DesktopWindow,
    #[live] frame: Frame
}

impl App {
    // at the App level this is needed to register the components on the modules we use
    pub fn live_register(cx: &mut Cx) {
        makepad_widget::live_register(cx);
    }
    
    // The main_app macro calls new_app as its main entrypoint
    pub fn new_app(cx: &mut Cx) -> Self {
        // this spawns up the App rust structure based on a lookup of the above live_register doc App id
        // to fill up its members with data
        Self::new_from_doc(cx, get_local_doc!(cx, id!(App)))
    }

    // called by the system to handle events
    pub fn handle_app(&mut self, cx: &mut Cx, event: &mut Event) {
        // let the desktop window (bunch of window chrome) handle itself:
        self.desktop_window.handle_desktop_window(cx, event);

        // handle frame returns an iterator over a set of dyn AnyAction traits you can hard-type-cast
        for item in self.frame.handle_frame(cx, event) {
            if let ButtonAction::Clicked = item.action.cast() {
                println!("Clicked on button {}", item.id);
            }
        }
    }
    
    // the system calls draw-app to render the application
    pub fn draw_app(&mut self, cx: &mut Cx) {
        // desktop window begin/end wraps the immediate drawflow to have it drawn into a desktop window
        if self.desktop_window.begin_desktop_window(cx, None).is_err() {
            return;
        }
        // this is how you access a concrete child component from the frame component
        // the macro makes it a bit less noisy: $frame.get_component($comp_id).map_or(None, |v| v.cast_mut::<$ty>())
        if let Some(button) = get_component!(id!(b1), Button, self.frame) {
            button.label = "Btn1 label override".to_string();
        }
        // draw_frame is our 'dynamic component' interpreter for the props defined in the live_register above
        self.frame.draw_frame(cx);
        
        self.desktop_window.end_desktop_window(cx);
    }
}