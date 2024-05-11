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
                        title = {text:"Rust"},
                        <SlideBody> {text: "- Fast, reliable\n- Focus on compiletime"}
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
                        <SlideBody> {text: "- Smile :)"}
                    }
                    <Slide> {
                        title = {text: "Team"},
                        <SlideBody> {text: "- Design: Sebastian Michailidis\n- Hard things: Eddy Bruël\n- Cheerleader: Rik Arends"}
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
                        title = {text: "Future"},
                        <SlideBody> {text: "- Tool assisted instancing"}
                    }
                    <Slide> {
                        title = {text: "IDE Extensions"},
                        <SlideBody> {text: "- WASM engine: Stitch\n- Eddy Bruël"}
                    }
                    <Slide> {                        
                        title = {text: "Embedded Rust"},
                        <SlideBody> {text: "- Lets drive"}
                    }
                    <Slide> {
                        title = {text: "Embedded and UI workshop!"},
                        <SlideBody> {text: "- Today! 14:00 - 17:30"}
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
