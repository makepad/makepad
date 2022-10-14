use makepad_widgets;
use makepad_widgets::*;
use makepad_draw_2d::*;
use makepad_widgets::imgui::*;

// The live DSL area that can be hotloaded
live_register!{
    // import frame types
    import makepad_widgets::frame::*;
    // load the widget registry 
    registry Widget::*;

    App: {{App}} {
        frame: {
            layout: {padding: 30}
            walk: {width: Fill, height: Fill, flow: Down},
            bg: {
                shape: Solid
                // little gradient shader for the background
                fn pixel(self) -> vec4 {
                    return mix(#7, #3, self.geom_pos.y);
                }
            }
            // named button to click
            button1 = Button {
                text: "Click to count"
            }
            // label to show the counter
            label1 = Label {
                text: "Counter: 0"
            }
        }
    }
}
// define main function and eventloop entry point for both wasm and desktop
main_app!(App);

// main application struct
#[derive(Live, LiveHook)]
pub struct App {
    window: BareWindow,
    frame: Frame,
    #[rust] counter: usize
}

impl App {
    // register dependencies, in this case the makepad widgets library
    pub fn live_register(cx: &mut Cx) {
        makepad_widgets::live_register(cx);
    }
    
    // event message pump entry point
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) {

        // draw events need to be handled with a draw context
        if let Event::Draw(event) = event {
            return Cx2d::draw(cx, event, self, | cx, s | s.draw(cx));
        }

        // give the window time to do things
        self.window.handle_event(cx, event);
        
        // call handle event on the frame and return a framewrap with all the result actions
        let fw = self.frame.handle_event_wrap(cx, event);
        
        // the framewrap can be queried for components and events polled 
        if fw.button(path!(button1)).clicked() {
            self.counter += 1;
            // overwrite our UI structure with an updated value
            self.frame.apply_over(cx, live!{
                label1 = {text: (format!("Counter: {}", self.counter))}
            });
            // cause a redraw to happen
            self.frame.redraw(cx);
        }
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        // if the window is not dirty, don't redraw
        if self.window.begin(cx).not_redrawing() {
            return;
        }
        
        // iterate over any user-draw items in the frame
        while let Some(_) = self.frame.draw(cx).as_not_done() {
        };
        
        self.window.end(cx);
    }
}