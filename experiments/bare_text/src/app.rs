use makepad_widgets::*;
       
live_design!{
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*; 
    
    App = {{App}} {
        ui: <Root>{
            main_window = <Window>{
                show_bg: true
                width: Fill,
                height: Fill
                
                draw_bg: {
                    fn pixel(self) -> vec4 {
                        // test
                        return mix(#7, #3, self.pos.y);
                    }
                }
                
                body = <ScrollXYView>{
                    flow: Down,
                    spacing:10,
                    <Label>{
                        padding: 10
                        margin: 0
                        draw_text: {
                            color: #f
                        },
                        text: r#"Testing text 1234"#,
                        width: 200.0,
                    }
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
        // we could create a theme mapping here
        //cx.set_theme("makepad_widgets::theme_desktop_dark")
        /*makepad_draw::live_design(cx);
        makepad_widgets::base::live_design(cx);
        makepad_widgets::theme_desktop_dark::live_design(cx);
        makepad_widgets::label::live_design(cx);
        makepad_widgets::view::live_design(cx);
        makepad_widgets::button::live_design(cx);
        makepad_widgets::window::live_design(cx);
        makepad_widgets::scroll_bar::live_design(cx);
        makepad_widgets::scroll_bars::live_design(cx);
        makepad_widgets::root::live_design(cx);*/
        crate::makepad_widgets::live_design(cx);
    }
}

impl MatchEvent for App{
    fn handle_actions(&mut self, cx: &mut Cx, actions:&Actions){
        if self.ui.button(id!(button1)).clicked(&actions) {
            log!("Press button {}", self.counter); 
            self.counter += 1;
            let label = self.ui.label(id!(label1));
            label.set_text_and_redraw(cx,&format!("Counter: {}", self.counter));
            //log!("TOTAL : {}",TrackingHeap.total());
            
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
} 

/*

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
        self.count.fetch_add(1, Ordering::Relaxed); 
        self.total.fetch_add(layout.size() as u64, Ordering::Relaxed);
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
static TrackingHeap: TrackingHeapWrap = TrackingHeapWrap::new();*/