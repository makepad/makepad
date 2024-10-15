use crate::{
    makepad_widgets::*,
    makepad_widgets::slides_view::*,
    makepad_widgets::makepad_micro_serde::*,
}; 
use std::fs::File;
use std::io::Write;

live_design!{ 
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    App = {{App}} {
        
        ui: <Window> {
            show_bg: true
            width: Fill,
            height: Fill
            body = {
                slides_view = <SlidesView> {
                    //current_slide:2.0
                    <Slide> {
                        title = {text: "Welcome!"},
                        <SlideBody> {text: ""}
                    }
                    <SlideChapter> {
                        title = {text: "Visual application development\nin Rust/AI"},
                        <SlideBody> {text: "Rik Arends\n"}
                    }
                    <Slide> {
                        title = {text:"Our mission"},
                        <SlideBody> {text: "- Reimagining VB/Delphi for Rust\n- Integrated design workflow\n- All platforms: Web Desktop Mobile"}
                    }
                    <Slide> {
                        title = {text:"Since last year"},
                        <SlideBody> {text: "- Visual designer WIP\n- AI integration\n- External projects: \n     - Moxin: LMStudio\n     - Robrix: Matrix client  "}
                    }
                    <Slide> {
                        title = {text:"Makepad Framework"},
                        <SlideBody> {text: "- Our flutter/swiftUI\n- Designed for live coding / editing\n- DSL: Live\n- Rust: Compiled\n- It works! "}
                    }
                    <Slide> {
                        title = {text: "Makepad Studio"},
                        <SlideBody> {text: "- Code editor (vscode)\n- Build system\n- Visual designer (figma)  "}
                    }
                    <Slide> {
                        title = {text: "But this is not 2023"},
                        <SlideBody> {text: "- AI is automating software development\n"}
                    }
                    <Slide> {
                        title = {text: "What can an AI do"},
                        <SlideBody> {text: "- Help write Rust\n- Understand our APIs\n- Write DSL"}
                    }
                    <Slide> {
                        title = {text: "Demo basic workflow"},
                        <SlideBody> {text: ""}
                    }
                    <Slide> {
                        title = {text: "How does it work?"},
                        <SlideBody> {text: "- In trainingset\n- Context "}
                    }
                    <Slide> {
                        title = {text: "Future: Vision models"},
                        <SlideBody> {text: "- Sketch generation\n- See your application\n- Correct designs\n- Automated UI testing "}
                    }
                    <Slide> {
                        title = {text: "Future: Language models"},
                        <SlideBody> {text: "- Automatic localisation\n- Generating Rust code iteratively "}
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

#[derive(Live)]
pub struct App {
    #[live] ui: WidgetRef,

}

impl LiveHook for App{
    fn after_new_from_doc(&mut self, cx:&mut Cx){
        if let Ok(contents) = std::fs::read_to_string("makepad_slides_state.ron") {
            match AppStateRon::deserialize_ron(&contents) {
                Ok(state)=>{
                    self.ui.slides_view(id!(slides_view)).set_current_slide(cx, state.slide);
                }
                _=>()
            }
        }
    }
}

impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
    }
}

#[derive(SerRon, DeRon)]
struct AppStateRon{
    slide:usize
}

impl MatchEvent for App {
    fn handle_actions(&mut self, _cx:&mut Cx, actions:&Actions){
        let slides_view = self.ui.slides_view(id!(slides_view));
        if let Some(slide) = slides_view.flipped(&actions){
            let saved = AppStateRon{slide}.serialize_ron();
            let mut f = File::create("makepad_slides_state.ron").expect("Unable to create file");
            f.write_all(saved.as_bytes()).expect("Unable to write data");
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
