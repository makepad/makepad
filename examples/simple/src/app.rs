
use makepad_widgets::*;

live_design!{
    use link::widgets::*;
    use link::theme::*;
    use link::shaders::*;
    App = {{App}} {
        ui: <Root>{
            main_window = <Window>{
                body = <ScrollXYView>{
                    flow: Down,
                    show_bg: true,
                    draw_bg: {

                        fn hsv_to_rgb(self, hsv: vec3) -> vec3 {
                            let k = vec4(1.0, 2.0 / 3.0, 1.0 / 3.0, 3.0);
                            let p = abs(fract(hsv.xxx + k.xyz) * 6.0 - k.www);
                            let rgb = hsv.z * mix(k.xxx, clamp(p - k.xxx, 0.0, 1.0), hsv.y);
                            return rgb;
                        }

                        fn pixel(self) -> vec4 {
                            let uv = self.pos * 6.0;
                            let t = self.time * 0.4;

                            let pattern_x = sin(uv.x + t * 1.2);
                            let pattern_y = cos(uv.y - t * 0.7);
                            let pattern_diag = sin(uv.x * 0.8 + uv.y * 1.2 + t * 0.5);
                            let pattern_dist = cos(length(uv * vec2(1.0, 1.5)) * 1.5 - t * 0.9);

                            let hue = fract(0.6 + 0.4 * (pattern_x * 1.1 + pattern_y * 0.9 + pattern_diag * 0.6 + pattern_dist * 0.4));
                            let sat = 0.7 + 0.3 * sin(uv.x * 2.5 - t * 1.5 + pattern_y * 3.14);
                            let val = 0.65 + 0.35 * cos(uv.y * 3.5 + t * 1.1 - pattern_x * 1.57);

                            let color_rgb = self.hsv_to_rgb(vec3(hue, clamp(sat, 0.5, 1.0), clamp(val, 0.4, 1.0)));

                            return vec4(color_rgb, 1.0);
                        }
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