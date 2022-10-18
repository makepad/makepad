use makepad_widgets;
use makepad_widgets::*;
use makepad_draw_2d::*;

// The live DSL area that can be hotloaded
live_design!{
    // import frame types
    import makepad_widgets::frame::*;
    // load the widget registry
    registry Widget::*;
    
    App = {{App}} {
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
            button1 = <Button> {
                //walk: {margin: {left: 100, top: 100}}
                text: "Click to count"
            }
            // label to show the counter
            label1 = <Label> {
                //walk: {margin: {left: 114, top: 20}}
                label: {color: #f},
                text: "Counter: 0"
            }
        }
    }
}
// define main function and eventloop entry point for both wasm and desktop
main_app!(App);

// main application struct
#[derive(Live)]
pub struct App {
    window: BareWindow,
    ui: FrameRef,
    #[rust] counter: usize
}

impl LiveHook for App{
    fn before_apply(&mut self, _cx: &mut Cx, _apply_from: ApplyFrom, _index: usize, _nodes: &[LiveNode])->Option<usize>{
        //_nodes.debug_print(0,100);
        None
    }
}

impl App {
    // register dependencies, in this case the makepad widgets library
    pub fn live_design(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
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
        let actions = ui.handle_event(cx, event);
        
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


