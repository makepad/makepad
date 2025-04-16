use makepad_widgets::*;
   
live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    
    App = {{App}} {

        ui: <Window>{ 
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
                align: {
                    x: 0.5,
                    y: 0.5
                },
                button1 = <Button> {
                    text: "Hello world 13241234312434214321234112343412412312343421"
                    draw_text:{color:#f00}
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
                <Html>{
                    // a = {
                    //     draw_text: {
                    //         // other blue hyperlink colors: #1a0dab, // #0969da  // #0c50d1, #x155EEF, // #0a84ff
                    //         // color: #1a0dab,
                    //     }
                    // }

                    Button = <Button> {
                        text: "Helloworld"
                    }  
                    body:" 
                    Normal <u>underlined html</u> <s>strike</s> text hello world<ol>
                        <li>one in the list!!!!! </li><li>two</li><li>three<ol><li>sub one</li><li>sub two</li><li>sub three<ol><li>sub sub one</li><li>sub sub two</li><li>sub sub three</li></ol></li></ol></li></ol>inline <code>let x = 1.0;</code> code <b>BOLD text</b>&nbsp;<i>italic</i><br/>
                    <sep/>
                    Testing a link: <a href=\"https://www.google.com\">Click to Google</a><br/>
                    Next line normal text button:<Button>Hi</Button><br/>lkjlkqjwerlkjqwelrkjqwelkrjqwlekjrqwelr<blockquote>block<b>quote</b><br/><blockquote>blockquote</blockquote><br/>
                    Next line <br/>
                    <sep/>
                    </blockquote><b><i>Bold italic</i><br/>
                    <sep/></br>
                    <pre>this is a preformatted code block</pre>
                    "
                }
                <Markdown>{
                    body:"
                    # MD H1 
                    ## H2 **Bold** *italic*
                    1. aitem
                    1. item
                      1. item  
                      1. test  
                    4. item               
                                          
                    > block
                    > next
                    >> hi
                    continuation
                    
                    [link](https://image)
                    ![image](https://link)
                    Normal
                    Next line
                    
                    ---
                    ~~single newline~~ becomes space
                    *hello*hello world
                    
                        inline code
                        more inline code
                    Double newline
                    `inline code` text after
                    ```
                    let x = 10
                    let y = 10
                    ```
                    *italic* **Bold** normal _italic_ __bold__ ***Bolditalic*** normal
                    123
                    "
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
            label.set_text(cx,&format!("Counter: {}", self.counter));
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