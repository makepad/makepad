
use makepad_widgets::*;

live_design!{
    use link::widgets::*;
    use link::theme::*;
    use link::shaders::*;

    ICON_TRASH = dep("crate://self/resources/icon_trash.svg")

    TodoItem = {{TodoItem}} {
        width: Fill, height: Fit,
        spacing: 10.,
        flow: Right,
        align: {x: 0.0, y: 0.5},
        padding: { top: 10., bottom: 10., left: 10., right: 10. }

        checkbox = <CheckBox> {
            width: Fit, height: Fit,
            text: ""
        }

        label = <Label> {
            width: Fill, height: Fit,
            draw_text: {
                text_style: <THEME_FONT_REGULAR>{font_size: 12.0},
                color: #9
            }
            text: "Task description"
        }

        delete_button = <Button> {
            width: Fit, height: Fit
            icon_walk: {width: 15, height: 15},
            draw_icon: {
                svg_file: (ICON_TRASH),
                color: #9
            }
            text: ""
            draw_bg: {
                 fn pixel(self) -> vec4 {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                    return sdf.result;
                 }
            }
        }
    }

    TodoList = {{TodoList}} {
        width: Fill, height: Fit
        flow: Down
        list = <PortalList> {
            TodoItem = <TodoItem> {}
        }
    }

    App = {{App}} {
        ui: <Window> {
            window: {inner_size: vec2(400, 600)},
            body = <View> {
                width: Fill, height: Fill
                flow: Down,
                spacing: 10.,
                padding: 10.,
                show_bg: true,
                draw_bg: {
                    fn pixel(self) -> vec4 {
                        return #2;
                    }
                }

                input_view = <View> {
                    width: Fill, height: Fit,
                    flow: Right,
                    spacing: 10.,

                    task_input = <TextInput> {
                        width: Fill, height: Fit,
                        text: ""
                        empty_text: "Enter new task..."
                    }

                    add_button = <Button> {
                        width: Fit, height: Fit,
                        text: "Add"
                    }
                }

                todo_list = <TodoList> {
                    width: Fill, height: Fill
                }
            }
        }
    }
}


app_main!(App); 
 
#[derive(Live, LiveHook)]
pub struct App {
    #[live] ui: WidgetRef,
    #[rust] counter: usize,
 }
 
impl LiveRegister for App {
    fn live_register(cx: &mut Cx) { 
        crate::makepad_widgets::live_design(cx);
    }
}

impl MatchEvent for App{
    fn handle_startup(&mut self, _cx:&mut Cx){
    }
    
    fn handle_actions(&mut self, _cx: &mut Cx, actions:&Actions){
        if self.ui.button(id!(button_1)).clicked(&actions) {
            self.counter += 1;
            log!("HI");
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::XrUpdate(_e) = event{
            //log!("{:?}", e.now.left.trigger.analog);
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}



// This is our custom allocator!
use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicU64, Ordering},
};

pub struct TrackingHeapWrap{
    count: AtomicU64,
    total: AtomicU64,
}

impl TrackingHeapWrap {
    // A const initializer that starts the count at 0.
    pub const fn new() -> Self {
        Self{
            count: AtomicU64::new(0),
            total: AtomicU64::new(0)
        }
    }
        
    // Returns the current count.
    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }
        
    pub fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }
}

unsafe impl GlobalAlloc for TrackingHeapWrap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // Pass everything to System.
        let count = self.count.fetch_add(1, Ordering::Relaxed); 
        self.total.fetch_add(layout.size() as u64, Ordering::Relaxed);
        if layout.size() > 60000000{
            //panic!();
            
            println!("{count} {:?}",layout.size());
        }
        System.alloc(layout)
    }
            
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.count.fetch_sub(1, Ordering::Relaxed); 
        self.total.fetch_sub(layout.size() as u64, Ordering::Relaxed);
        System.dealloc(ptr, layout)
    }
}

// Register our custom allocator.
#[global_allocator]
static TRACKING_HEAP: TrackingHeapWrap = TrackingHeapWrap::new();