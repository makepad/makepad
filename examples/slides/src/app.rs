use crate::{
    makepad_widgets::*,
    makepad_widgets::slides_view::*,
    makepad_widgets::makepad_micro_serde::*,
}; 
use std::fs::File;
use std::io::Write;

live_design!{ 
    use link::widgets::*;
    use link::theme::*;
    use link::shaders::*;
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
                        title = {text: "Visual application development\nin Rust, with a bit of AI"},
                        <SlideBody> {text: "Rik Arends\n"}
                    }
                    <Slide> {
                        title = {text:"Our mission"},
                        <SlideBody> {text: "AI Accelerated app design for Rust"}
                    }
                    <Slide> {
                        title = {text:"Projects using Makepad"},
                        <SlideBody> {text: "- Moxin: LMStudio\n- Robrix: Matrix client  "}
                    }
                    <Slide> {
                        title = {text:"What is Makepad"},
                        <SlideBody> {text: "- Makepad Studio\n- Makepad Framework"}
                    }
                    <Slide> {
                        title = {text:"Makepad Framework"},
                        <SlideBody> {text: "- Our Flutter/React\n- Designed for livecoding\n    - DSL: Live\n    - Rust: Compiled "}
                    }
                    <Slide> {
                        title = {text: "Makepad Studio"},
                        <SlideBody> {text: "- Our IDE\n    - Code editor\n    - Build system\n    - Visual designer"}
                    }
                    <Slide> {
                        title = {text: "But this is not the year 2005"},
                        <SlideBody> {text: "- AI is automating software development\n"}
                    }
                    <Slide> {
                        title = {text: "What can AI do"},
                        <SlideBody> {text: "- Generate UIs\n- Write code\n- Understand APIs "}
                    }
                    <Slide> {
                        title = {text: "Basic workflow"},
                        <SlideBody> {text: ""}
                    }
                    /*
                    <Slide> {
                        title = {text: ""},
                        <SlideBody> {text: "- Generate app\n- Modify in designer\n- Write shader code\n- Writes Rust code"}
                    }*/
                    <Slide> {
                        title = {text: "How does the AI work?"},
                        <SlideBody> {text: "- Makepad codebase in training set\n- Custom context "}
                    }
                    <Slide> {
                        title = {text: "Workflow"},
                        <SlideBody> {text: "- Generate App\n- Change UI in designer\n- Change code in editor\n- Iterate with AI "}
                    }
                    <Slide> {
                        title = {text: "Real examples"},
                        <SlideBody> {text: "- Ironfish\n- Studio"}
                    }
                    <Slide> {
                        title = {text: "The Future"},
                        <SlideBody> {text: "- Vision models\n- Language models "}
                    }
                    <Slide> {
                        title = {text: "Vision models"},
                        <SlideBody> {text: "- Sketch generation\n- See your application\n- Correct designs\n- Automated UI testing "}
                    }
                    <Slide> {
                        title = {text: "Language models"},
                        <SlideBody> {text: "- Automatic localisation\n- Generating Rust code\n     - Types\n     - Clean error messages"}
                    }
                    <Slide> {
                        title = {text: "Thank you"}, 
                        <SlideBody> {text: "- Repo\n        https://github.com/makepad/makepad\n- Website\n        www.makepad.nl\n- Twitter\n        @rikarends\n        @ejpbruel\n- Questions?"}
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
