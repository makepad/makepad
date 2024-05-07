use crate::{
    makepad_widgets::*,
}; 
 
live_design!{ 
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    App = {{App}} {
        
        ui: <Window> {
            show_bg: true
            width: Fill,
            height: Fill
            body = {
                <SlidesView> {
                    //current_slide:2.0
                    <Slide> {
                        title = {text: "Hello RustNL!"},
                        <SlideBody> {text: ""}
                    }
                    <SlideChapter> {
                        title = {text: "Visual application design\nin Rust"},
                        <SlideBody> {text: "Rik Arends\n"}
                    }
                    <Slide> {
                        title = {text:"Makepad"},
                        <SlideBody> {text: "- Reimagining VB/Delphi\n- A new IDE and designtool"}
                    }
                    <Slide> {
                        title = {text:"Why"},
                        <SlideBody> {text: "- I love to have fun with code\n- Sound, Graphics, Robots"}
                    }
                    <Slide> {
                        title = {text: "Lets use the side screens!"},
                        <SlideBody> {text: "- Simple app on appleTV"}
                    }
                    <Slide> {
                        title = {text: "AI SDXL"},
                        <SlideBody> {text: "- Fun with GenAI\n- Server IO test"}
                    }
                    <Slide> {
                        title = {text: "Lets add a camera"},
                        <SlideBody> {text: "- RC Car with ESP32 and Rust"}
                    }
                    <Slide> {
                        title = {text: "Makepad Studio"},
                        <SlideBody> {text: "- Hybrid code and design"}
                    }
                    <Slide> {
                        title = {text: "UI Framework"},
                        <SlideBody> {text: "- Shader based DSL\n- High performance\n- Query based model"}
                    }
                    <Slide> {
                        title = {text: "IDE Extensions"},
                        <SlideBody> {text: "- WASM engine: Stitch\n- Eddy Bruël"}
                    }
                    <Slide> {
                        title = {text: "IDE Extensions"},
                        <SlideBody> {text: "- WASM engine: Stitch\n- Eddy Bruël"}
                    }                   
                    <Slide> {
                        title = {text: "Links"}, 
                        <SlideBody> {text: "- github.com/makepad/makepad\n- makepad.nl\n- twitter @rikarends"}
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

}

impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
