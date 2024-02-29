use makepad_widgets::*;
  
live_design!{
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*; 
    
    MyHtml = {{MyHtml}}<Html>{
    }
    
    App = {{App}} {

        ui: <Window>{
            show_bg: true
            width: Fill,
            height: Fill
            
            draw_bg: {
                fn pixel(self) -> vec4 {
                    //return #000
                    return mix(#7, #3, self.pos.y);
                }
            }
            
            body = <View>{
                flow: Down,
                spacing: 20,
                align: {
                    x: 0.5,
                    y: 0.5
                },
                button1 = <Button> {
                    text: "Hello world"
                }
                input1 = <TextInput> {
                    width: 100, height: 30
                    text: "Click to count"
                }
                label1 = <Label> {
                    draw_text: {
                        color: #f
                    },
                    text: "Counter: 0"
                }
                <MyHtml>{
                    font_size: 12,
                    flow: RightWrap,
                    width:Fill,
                    height:Fit,
                    padding: 5,
                    line_spacing: 10,
                    Button = <Button> {
                        text: "Hello world"
                    }  
                    body:"this is <br/><li>one</li><br/><li>two</li><br/><code>let x = 1.0;</code><b>BOLD text</b>&nbsp;<i>italic</i><br/><sep/>Next line normal text button: <Button>Hi</Button><br/><block_quote>blockquote<br/><block_quote>blockquote</block_quote></block_quote><i>Bold italic</i>"
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
            log!("BUTTON CLICKED {}", self.counter); 
            self.counter += 1;
            let label = self.ui.label(id!(label1));
            label.set_text_and_redraw(cx,&format!("Counter: {}", self.counter));
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
} 



#[derive(Live, LiveHook, Widget)]
struct MyHtml{ 
    #[deref] html:Html
}

impl Widget for MyHtml{
    fn draw_walk(&mut self, cx:&mut Cx2d, _scope:&mut Scope, walk:Walk)->DrawStep{
        let tf = &mut self.html.text_flow;
        tf.begin(cx, walk); 
        let mut node = self.html.doc.walk();
        while !node.empty(){
            match Html::handle_open_tag(cx, tf, &mut node){
                Some(_)=>{
                    // handle tag here
                }
                _=>()
            }
            match Html::handle_close_tag(cx, tf, &mut  node){
                Some(_)=>{
                    // handle tag here
                }
                _=>()
            }
            Html::handle_text_node(cx, tf, &mut node);
            node = node.walk();
        }
        tf.end(cx);
        DrawStep::done()
    }
    
    fn handle_event(&mut self, cx:&mut Cx, event:&Event, scope:&mut Scope){
        self.html.handle_event(cx, event, scope)
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