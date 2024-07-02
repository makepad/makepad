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
                        title = {text: "Welcome!"},
                        <SlideBody> {text: ""}
                    }
                    <SlideChapter> {
                        title = {text: "Visual application development\nin Rust"},
                        <SlideBody> {text: "Rik Arends\n"}
                    }
                    <Slide> {
                        title = {text:"Our mission"},
                        <SlideBody> {text: "- Reimagining VB/Delphi for Rust\n- Integrated design workflow\n- All platforms: Web Desktop Mobile"}
                    }
                    <Slide> {
                        title = {text:"What is Rust"},
                        <SlideBody> {text: "- Originated at mozilla\n- Open Source\n- Systems language\n- Memory safe\n- No GC\n- Can compile fast\n- Large package ecosystem  "}
                    }
                    <Slide> {
                        title = {text:"Makepad Framework"},
                        <SlideBody> {text: "- Our flutter/swiftUI\n- Designed for live coding / editing\n- DSL: Live\n- Rust: Compiled "}
                    }
                    <Slide> {
                        title = {text: "Makepad Studio"},
                        <SlideBody> {text: "- Code editor (vscode)\n- Build system\n- Visual designer (figma)  "}
                    }
                    <Slide> {
                        title = {text: "Examples"},
                        <SlideBody> {text: "- Makepad studio\n- Fractals\n- Ironfish\n- AI "}
                    }
                    <Slide> {
                        title = {text: "Build to"},
                        <SlideBody> {text: "- Web\n- Mobile\n- Desktop "}
                    }
                    <Slide> {
                        title = {text: "Architecture"},
                        <SlideBody> {text: "- DSL for UI structure\n- Shaders for styling\n- Fast GPU instancing, low CPU overhead\n- Scalable: Rust for all application code"}
                    }
                    <Slide> {
                        title = {text: "Example code"},
                        <SlideBody> {text: "- DSL at the top\n- Rust app below "}
                    }
                    <Slide> {
                        title = {text: "More apps"},
                        <SlideBody> {text: "- Moxin, LM Studio-like\n- Robrix, matrix client "}
                    }
                    <Slide> {
                        title = {text: "Challenges"},
                        <SlideBody> {text: "- Many mobile APIs (Robius)\n- App store packaging (Robius)\n- Font rendering\n- Visual design tool "}
                    }
                    <Slide> {
                        title = {text: "Extensions / micro apps"},
                        <SlideBody> {text: "- WASM engine: Stitch\n- Fastest interpreter: iOS possible\n- Eddy Bruël"}
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
