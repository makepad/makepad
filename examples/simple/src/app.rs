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
                    fn pixel(self) -> vec4 {    // < --- Apply error: examples/simple/src/app.rs:21:20 - property: pixel target class not found
                        // 获取几何位置
                        let st = vec2(
                            self.geom_pos.x,
                            self.geom_pos.y
                        );
                        // 计算颜色，基于 x 和 y 位置及时间
                        let color = vec3(st.x, st.y, abs(sin(self.time)));
                        return vec4(color, 1.0);
                    }
                }
                
                body = <ScrollXYView>{
                    flow: Down,
                    spacing:10,
                    align: {
                        x: 0.5,
                        y: 0.5
                    },
                    <Label> {
                        draw_text: {
                            text_style: {
                                font: {path: dep("crate://makepad-widgets/resources/GoNotoKurrent-Bold.ttf")}
                                font_size: 9.5
                            },
                        }
                        text: "https://test"
                    }
                    button1 = <Button> {
                        text: "Hello world "
                        draw_text:{color:#f00}
                    }
                    input1 = <TextInput> {
                        width: 100
                        text: "Click to count 获取几何位置"
                    }
                    label1 = <Label> {
                        draw_text: {
                            color: #f
                        },
                        text: r#"Lorem ipsum dolor sit amet, consectetur adipiscing elit. Praesent tristique condimentum tristique. Donec sapien arcu, molestie vitae neque pretium, ultrices luctus diam. Aenean a eros ac lectus sollicitudin eleifend non in tellus. Nullam sapien velit, sodales et tincidunt vestibulum, sollicitudin et purus. Praesent elementum risus rhoncus enim consectetur pulvinar. Quisque rutrum leo quis odio mattis blandit. Etiam sit amet nibh felis. Vivamus maximus hendrerit turpis, vitae efficitur risus faucibus in. Vestibulum lorem dui, consectetur consectetur magna nec, hendrerit bibendum magna. Mauris faucibus rhoncus turpis luctus porta. Aenean interdum auctor sapien ac hendrerit.

                        Aliquam erat volutpat. Praesent velit felis, iaculis at interdum sed, pellentesque nec tortor. Nulla mauris augue, sollicitudin non nisi ac, consequat dapibus lorem. Maecenas mollis, nulla id tincidunt finibus, neque enim ultricies libero, vel accumsan metus libero vel mauris. Vivamus et suscipit nisl, vel lacinia massa. Sed et bibendum lectus, nec pellentesque tortor. Cras non est ut eros venenatis volutpat quis quis risus. Suspendisse convallis vestibulum orci. Etiam sit amet nisl eleifend, semper nibh sit amet, tincidunt leo. Sed ut tristique nunc. Nulla dictum hendrerit augue.
                        
                        Vivamus ac porttitor sem. In auctor posuere velit ac molestie. Suspendisse ornare ex quis eros porttitor tincidunt. Praesent tincidunt purus tellus, vel malesuada dui condimentum at. Morbi pellentesque, velit euismod tristique rhoncus, metus mi tincidunt lacus, at faucibus tortor nunc ut nibh. Etiam efficitur est diam, ut commodo enim bibendum at. Suspendisse accumsan gravida nisi, sit amet sodales lectus maximus eu."#,
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
