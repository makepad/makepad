use makepad_component::*;
use makepad_platform::*;
use makepad_platform::platform::apple::core_audio::CoreAudio;
use std::sync::Arc;
use std::cell::Cell;

live_register!{
    use makepad_component::frame::Frame;
    use makepad_component::button::Button;
    App: {{App}} {
        scroll_view: {
            h_show: true,
            v_show: true,
            view: {
                layout: {
                    line_wrap: LineWrap::NewLine
                }
            }
        }
        frame: {
        }
    }
}
main_app!(App);

#[derive(Live, LiveHook)]
pub struct App {
    frame: Frame,
    frame_component_registry: FrameComponentRegistry,
    desktop_window: DesktopWindow,
    scroll_view: ScrollView,
    #[rust] offset: u64
}

impl App {
    pub fn live_register(cx: &mut Cx) {
        makepad_component::live_register(cx);
    }
    
    pub fn new_app(cx: &mut Cx) -> Self {
        Self::new_as_main_module(cx, &module_path!(), id!(App)).unwrap()
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) {
        
        self.desktop_window.handle_event(cx, event);
        self.scroll_view.handle_event(cx, event);
        
        match event {
            Event::Construct => {  

                // spawn 1000 buttons into the live structure
                let mut out = Vec::new();
                out.open();
                for i in 0..1 {
                    out.push_live(live_object!{
                        [id_num!(btn, i)]: Button {
                            label: (format!("This is makepad metal UI{}", i + self.offset))
                        }
                    });
                }
                out.close();
                self.frame.apply_clear(cx, &out);
                
                //cx.new_next_frame();
                println!("here!");
                cx.redraw_all();
            }
            Event::Draw(draw_event) => {
                self.draw(&mut Cx2d::new(cx, draw_event));
            }
            _ => ()
        }
        
        for item in self.frame.handle_event(cx, event) {
            if let ButtonAction::IsPressed = item.action.cast() {
                println!("Clicked on button {}", item.id);
            }
        }
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        if self.desktop_window.begin(cx, None).is_err() {
            return;
        }
        if self.scroll_view.begin(cx).is_ok() {
            //if let Some(button) = get_component!(id!(b1), Button, self.frame) {
            //    button.label = "Btn1 label override".to_string();
            // }
            //cx.profile_start(1);
            self.frame.draw(cx);
            //cx.profile_end(1);
            //cx.set_turtle_bounds(Vec2{x:10000.0,y:10000.0});
            self.scroll_view.end(cx);
        }
        
        self.desktop_window.end(cx);
        //cx.debug_draw_tree(false);
    }
}