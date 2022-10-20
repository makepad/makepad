use crate::makepad_draw_2d::*;
use crate::frame::*;
use crate::widget::*;

live_design!{
    import makepad_draw_2d::shader::std::*;
    import makepad_widgets::frame::*;
    registry Widget::*;
    
    const SLIDE_WIDTH = 800
    
    Body = <Label> {
        label: {
            color: #f
            text_style: {
                font_size: 35
            }
        }
        text: ""
    }
    
    Slide = <Frame> {
        bg: {shape: Box, color: #2, radius: 10}
        walk: {width: (SLIDE_WIDTH), height: Fill}
        layout: {align: {x: 0.5, y: 0.5}, flow: Down, spacing: 40}
        title = <Label> {
            label: {
                color: #f
                text_style: {
                    font_size: 64
                }
            }
            text: "SlideTitle"
        }
    }
    
    SlidesView = {{SlidesView}} {
        slide_width: (SLIDE_WIDTH)
        goal_pos: 0.0
        anim_speed: 0.9
        frame: <ScrollX> {
            walk: {width: Fill, height: Fill}
            <Slide> {title = {text: "Portable SIMD"}, <Body> {text: "For Native and Wasm"}}
            <Slide> {title = {text: "Intro"}, <Body> {text: "Rik Arends\nBuilding Makepad\nRust livecoding IDE"}}
            
            <Slide> {title = {text: "Fractals Native"}, <Body> {text: "Native: All cores and SIMD"}}
            <Slide> {title = {text: "Fractals web"}, <Body> {text: "No SIMD on safari"}}
            <Slide> {title = {text: "Mandelbrot"}, <Body> {text: "Trivially parallel"}}
            <Slide> {title = {text: "Tile engine"}, <Body> {text: "Threads and queues\nchange core counter"}}
            <Slide> {title = {text: "Live updating"}, <Body> {text: "Core feature of makepad\nLive shaders+DSL"}}
            
            <Slide> {title = {text: "SIMD"}, <Body> {text: "Single instruction\nMultiple data\nLane bitsize"}}
            <Slide> {title = {text: "Lanes"}, <Body> {text: "128 bit (ARM/Wasm)\n256 bit: AMD/Intel\n512 bit intel"}}
            <Slide> {title = {text: "CPU Instructions"}, <Body> {text: "intel: MMX\nintel: SSE2\nintel: AVX\nARM: Neon"}}
            <Slide> {title = {text: "Intrinsics"}, <Body> {text: "CPU specific\nMany codepaths"}}
            <Slide> {title = {text: "Portable SIMD"}, <Body> {text: "Write once run everywhere"}}
            <Slide> {title = {text: "Mandelbrot"}, <Body> {text: "Show code"}}
            
            <Slide> {title = {text: "Web issues"}, <Body> {text: "Bindgen huge\nRust primitives\nStack frame\nSafari"}}
            <Slide> {title = {text: "Audio threads"}, <Body> {text: "Fun audio"}}
            
            <Slide> {title = {text: "Future for Makepad"}, <Body> {text: "Rust IDE\nLive coding\nDesigntool"}}
            <Slide> {title = {text: "Links"}, <Body> {text: "twitter: @makepad @rikarends\ngithub.com/makepad/makepad"}}
            /*
            Slide {title = {text: "Makepad JS"}}
            Slide {title = {text: "What was wrong"}, Body {text: "Not stable after 30+ mins"}}
            Slide {title = {text: "The real problem"}, Body {text: "No CPU perf left over\nNo VR + HiFPS"}}
            Slide {title = {text: "JS was wrong"}, Body {text: "GC limits complex use\nNot enough perf\nDynamic typing doesnt scale"}}
            Slide {title = {text: "The end?"}, Body {text: "Your life has no purpose"}}
            Slide {title = {text: "Hi Rust"}, Body {text: "Static typed\nCompiled native +wasm\nHigh and low level"}}
            Slide {title = {text: "A new chance"}, Body {text: "Eddy Bruël - code\nSebastian Michaelidis - design"}}
            Slide {title = {text: "Could this be it?"}, Body {text: "Run code native and on web"}}
            Slide {title = {text: "Makepad Rust"}, Body {text: "6 months first iteration"}}
            Slide {title = {text: "Chrome profile"}, Body {text: "Time left to be useful"}}
            Slide {title = {text: "What is it"}, Body {text: "UI Stack\nCode editor and IDE\nDesigntool"}}
            Slide {title = {text: "The idea"}, Body {text: "a UI DSL with live hybrid editing"}}
            Slide {title = {text: "Why"}, Body {text: "Having fun again as a programmer"}}
            Slide {title = {text: "Fun Audio"}}
            Slide {title = {text: "Future"}, Body {text: "OSS release in 3 months\nDesigntool will be commercial"}}
            Slide {title = {text: "Links"}, Body {text: "makepad.dev\ngithub.com/makepad/makepad\ntwitter: @rikarends @makepad"}}
            */
        }
    }
}


#[derive(Live, LiveHook)]
pub struct SlidesView {
    slide_width: f64,
    goal_pos: f64,
    current_pos: f64,
    anim_speed: f64,
    frame: FrameRef,
    #[rust] next_frame: NextFrame
}


impl SlidesView {
    
    fn next_frame(&mut self, cx: &mut Cx) {
        self.next_frame = cx.new_next_frame();
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        // lets handle mousedown, setfocus
        self.frame.handle_event(cx, event);
        match event {
            Event::Construct => {
                self.next_frame(cx);
            }
            Event::NextFrame(ne) if ne.set.contains(&self.next_frame) => {
                self.current_pos = self.current_pos * self.anim_speed + self.goal_pos * (1.0 - self.anim_speed);
                if (self.current_pos - self.goal_pos).abs()>0.00001 {
                    self.next_frame(cx);
                }
                self.frame.set_scroll_pos(cx, dvec2(self.current_pos * self.slide_width, 0.0));
                self.frame.redraw(cx);
            }
            _ => ()
        }
        match event.hits(cx, self.frame.area()) {
            Hit::KeyDown(KeyEvent {key_code: KeyCode::ArrowRight, ..}) => {
                self.goal_pos += 1.0;
                self.next_frame(cx);
            }
            Hit::KeyDown(KeyEvent {key_code: KeyCode::ArrowLeft, ..}) => {
                self.goal_pos -= 1.0;
                if self.goal_pos < 0.0 {
                    self.goal_pos = 0.0;
                }
                self.next_frame(cx);
            }
            Hit::FingerDown(_fe) => {
                cx.set_key_focus(self.frame.area());
            },
            _ => ()
        }
    }
    
    pub fn redraw(&mut self, cx: &mut Cx) {
        self.frame.redraw(cx);
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        while self.frame.draw(cx).is_not_done() {
        }
    }
}