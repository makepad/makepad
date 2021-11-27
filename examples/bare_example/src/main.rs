use makepad_render::*;
use makepad_widget::*;

live_register!{
    use makepad_widget::frame::Frame;
    use makepad_widget::button::Button;
    App: {{App}} {
        scroll_view:{show_h:true, show_v:true,view:{layout:{line_wrap:LineWrap::NewLine}}} 
        frame: {
            b1: Button {label: "btn1"}
            b2: Button {label: "btn2"}
            frame1: Frame {
                b3: Button {label: "btn3"}
                children: [b3]
            }
        }
    }
}
main_app!(App);

#[derive(LiveComponent, LiveApply, LiveCast)]
pub struct App {
    #[live] frame: Frame,
    #[live] desktop_window: DesktopWindow,
    #[live] scroll_view: ScrollView,
    #[rust] offset:u64
}

impl App {
    pub fn live_register(cx: &mut Cx) {
        makepad_widget::live_register(cx);
    }
    
    pub fn new_app(cx: &mut Cx) -> Self {
        //println!("{}", get_local_doc!(cx, id!(App)).nodes.to_string(0,100));
        Self::new_from_doc(cx, get_local_doc!(cx, id!(App)))
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) {
        self.desktop_window.handle_desktop_window(cx, event);
        self.scroll_view.handle_scroll_view(cx, event);
        if let Event::Construct = event{
            // spawn 1000 buttons into the live structure
            cx.profile_start(0);
            let mut out = Vec::new();
            out.open();  
            for i in 0..10000{
                out.push_live(live_object!{ 
                    [id_num!(btn,i)]: Button{label: (format!("B{}",i+self.offset))},
                });
            }
            self.offset += 9999;
            out.close();
            // now apply it to frame to create i t
            self.frame.apply_clear(cx, &out);
            cx.profile_end(0);
        }
        for item in self.frame.handle_frame(cx, event) {
            if let ButtonAction::Pressed = item.action.cast() {
                println!("Clicked on button {}", item.id);
            }
        }
    }
    
    pub fn draw(&mut self, cx: &mut Cx) {
        if self.desktop_window.begin_desktop_window(cx, None).is_err() {
            return;
        }
        if self.scroll_view.begin_view(cx).is_ok(){
            
            if let Some(button) = get_component!(id!(b1), Button, self.frame) {
                button.label = "Btn1 label override".to_string();
            }
            //cx.profile_start(1);
            self.frame.draw_frame(cx);
            //cx.profile_end(1);
            //cx.set_turtle_bounds(Vec2{x:10000.0,y:10000.0});
            self.scroll_view.end_view(cx);
        }
        
        self.desktop_window.end_desktop_window(cx);
        cx.debug_draw_tree(false);
    }
}