
use makepad_widgets::*;

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
                
    App = {{App}} {
        ui: <Root>{
            main_window = <Window>{
                body = <View>{
                    flow: Down,
                    spacing: 10,
                    align: {
                        x: 0.5,
                        y: 0.5
                    },
                    show_bg: true,
                    draw_bg:{
                        fn noise(self, p: vec2) -> float {
                            let i = floor(p);
                            let f = fract(p);
                            let u = f * f * (3.0 - 2.0 * f);
                            let a = self.hash(i + vec2(0.0, 0.0));
                            let b = self.hash(i + vec2(1.0, 0.0));
                            let c = self.hash(i + vec2(0.0, 1.0));
                            let d = self.hash(i + vec2(1.0, 1.0));
                                                        
                            return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
                        }
                        
                        fn hash(self, p: vec2) -> float {
                            let p_sin = sin(p * vec2(12.9898, 78.233));
                            return fract(p_sin.x * p_sin.y * 43758.5453);
                        }
                        
                        fn fbm(self, p: vec2) -> float {
                            let total = 0.0;
                            let amplitude = 0.5;
                            let frequency = 2.0;
                            for i in 0..5 {
                                total += self.noise(p * frequency) * amplitude;
                                frequency *= 2.0;
                                amplitude *= 0.5;
                            }
                            return total;
                        }
                        
                        fn pixel(self) -> vec4 {
                            let uv = self.pos * 4.0; 
                                                        
                            let time_factor = self.time * 0.1;
                                                        
                            let motion = vec2(time_factor, time_factor * 0.5);
                                                        
                            let base_uv = uv + motion;
                            let noise1 = self.fbm(base_uv);
                                                        
                            let warp_uv = uv * 1.5 + vec2(time_factor * -0.5, time_factor);
                            let noise2 = self.fbm(warp_uv + noise1 * 0.5); 
                                                        
                            let combined_noise = (noise1 + noise2) * 0.5;
                                                        
                            let color_intensity = smoothstep(0.2, 0.8, combined_noise);
                                                        
                            let base_color = mix(#003, #159, color_intensity); 
                                                        
                            let highlight_intensity = pow(smoothstep(0.6, 0.8, combined_noise), 2.0);
                            let highlight_color = vec3(0.8, 0.9, 1.0); 
                                                        
                            let final_color = mix(base_color, highlight_color, highlight_intensity * 0.4); 
                                                                                       
                            return vec4(final_color, 1.0);
                        }
                    }
                    button_1 = <Button> {
                        text: "Click me  😊"
                        draw_text:{color:#fff, text_style:{font_size:28}}
                        show_bg: true,
                        draw_bg: {
                            fn mandelbrot(self, c: vec2)->float {
                                let z = vec2(0.0,0.0);
                                let max_iter = 30 + int(self.hover * 50.0);
                                for i in 0..max_iter {
                                    let x = (z.x * z.x - z.y * z.y) + c.x;
                                    let y = (2.0 * z.x * z.y) + c.y;
                                    if((x * x + y * y) > 4.0){
                                        return float(i) / float(max_iter);
                                    }
                                    z = vec2(x,y);
                                }
                                return 0.0;
                            }
                                                                                    
                            fn pixel(self) -> vec4 {
                                let uv = self.pos * 3.0 - vec2(2.0, 1.5); 
                                let i = self.mandelbrot(uv);
                                let base_color = mix(#f00, #00f, self.hover);
                                let color = mix(base_color, #fff, i);
                                return vec4(color, 1.0);
                            }
                        }
                    }
                    text_input = <TextInput> {
                        width: 100,
                        flow: RightWrap,
                        text: "Lorem ipsum"
                        draw_text:{color:#fff, text_style:{font_size:28}}
                    }
                    button_2 = <Button> {
                        text: "Click me 345 1234"
                        draw_text:{color:#fff, text_style:{font_size:28}}
                        show_bg: true,
                        draw_bg: {
                            fn julia(self, z: vec2, c: vec2)->float {
                                let max_iter = 20 + int(self.hover * 40.0);
                                for i in 0..max_iter {
                                    let x = (z.x * z.x - z.y * z.y) + c.x;
                                    let y = (2.0 * z.x * z.y) + c.y;
                                    if((x * x + y * y) > 4.0){
                                        return float(i) / float(max_iter);
                                    }
                                    z = vec2(x,y);
                                }
                                return 0.0;
                            }
                                                                                    
                            fn pixel(self) -> vec4 {
                                let uv = self.pos * 2.0 - vec2(1.0, 1.0); 
                                let julia_c = vec2(-0.8 + 0.6 * cos(self.time), 0.156 + 0.4 * sin(self.time));
                                let i = self.julia(uv, julia_c);
                                let base_color = mix(#0f0, #ff0, self.hover);
                                let color = mix(base_color, #000, i);
                                
                                return vec4(color, 1.0);
                            }
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