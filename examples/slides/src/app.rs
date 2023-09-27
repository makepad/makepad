use crate::{
    makepad_widgets::*,
    makepad_studio_core::build_manager::build_manager::*,
    makepad_studio_core::build_manager::run_view::*,
    makepad_studio_core::file_system::file_system::*,
};

live_design!{
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    import makepad_studio_core::build_manager::run_view::RunView;
    App = {{App}} {
        
        ui: <Window> {
            show_bg: true
            width: Fill,
            height: Fill
            body = {
                <SlidesView> {
                    <Slide> {
                        title = {text: "Hello Gosim"},
                        <SlideBody> {text: ""}
                    }
                    
                    <SlideChapter> {
                        title = {text: "LIVE APP BUILDING\nWITH MAKEPAD"},
                        <SlideBody> {text: "Rik Arends\n"}
                    }
                    <Slide> {
                        title = {text:"What we lost"},
                        <SlideBody> {text: "- Visual Basic\n- Delphi\n"}
                    }                    
                    <Slide> {
                        title = {text: "A long long time ago …"},
                        <SlideBody> {text: "Cloud9 IDE & ACE"}
                    }
                    <Slide> {
                        title = {text: "HTML as an IDE UI?\nMadness!"},
                        <SlideBody> {text: "- Integrating design and code was hard\n- Could not innovate editing: folding\n- Too slow, too hard to control"}
                    }
                    <Slide> {
                        title = {text: "Let's start over!"},
                        <SlideBody> {text: "- JavaScript and WebGL for UI\n- Write shaders to style UI"}
                    }
                    <Slide> {
                        title = {text: "Tried, and tried again."},
                        <SlideBody> {text: "5 times: 2013-2018"}
                    }
                    <Slide> {
                        title = {text: "Final result"},
                        <SlideBody> {text: "- makepad.github.io/makepad_js.html"}
                    }
                    <Slide> {
                        title = {text: "The good part"},
                        <SlideBody> {text: "- Live Coding"}
                    }
                    <Slide> {
                        title = {text: "The bad parts"},
                        <SlideBody> {text: "- Too slow\n- Cannot scale: no types\n- Chrome crashes"}
                    }
                    <Slide> {
                        title = {text: "Now what"},
                        <SlideBody> {text: "- 5 years of my life gone"}
                    }
                    <Slide> {
                        title = {text: "Rust appears"},
                        <SlideBody> {text: "- Let's try again: Native + Wasm\n- Makepad in Rust\n- Startup with Eddy and Sebastian"}
                    }
                    <Slide> {
                        title = {text: "Rust"},
                        <SlideBody> {text: "- Compiled: LLVM\n- Typed\n- Compiles everywhere"}
                    }
                    <Slide> {
                        title = {text: "Finally we have\nperformance"},
                    }                                                                                                  
                    <Slide> {
                        title = {text: "The slides are a\nMakepad app"},
                    }                    
                    <Slide> {
                        title = {text: "However"},
                        <SlideBody> {text: "- No more live coding?"}
                    }
                    <Slide> {
                        title = {text: "Goals"},
                        <SlideBody> {text: "- Make Rust programming fun:\n- Visual designer\n- Media APIs\n- Live coding"}
                    }
                    <Slide> {
                        title = {text: "Solution for\nlive coding"},
                        <SlideBody> {text: "- Makepad UI Language: A DSL"}
                    }
                    <Slide> {
                        title = {text: "Makepad Studio"},
                        <SlideBody> {text: "- github.com/makepad/makepad\n- cargo run -p makepad-studio --release"}
                    }
                    <Slide> {
                        title = {text: "A Makepad App"},
                        <SlideBody> {text: "- Platform layer: OS/GPU\n- UI DSL: Structure+Shaders\n- Rust Code: Logic"}
                    }
                    <Slide> {
                        title = {text: "Lets look at a\nMakepad app"},
                        <SlideBody> {text: "- Simple: UI and events\n- News Feed: Virtual viewport"}
                    }
                    <Slide> {
                        title = {text: "Platforms"},
                        <SlideBody> {text: "- Windows / Mac / Linux\n- iOS / Android\n- WebAssembly"}
                    }
                    <Slide> {
                        title = {text: "Build examples"}, 
                        <SlideBody> {text: "- Before: fast or multiplatform\n- Demo builds"}
                    }
                    <Slide> {
                        title = {text: "SDXL UI"},
                        <SlideBody> {text: "- AI Image gen"}
                    }
                    <Slide> {
                        title = {text: "Audio UI"},
                        <SlideBody> {text: "- Ironfish"}
                    }
                    <Slide> {
                        title = {text: "Live coding shaders"},
                        <SlideBody> {text: "- Ironfish piano\n - Designer accessible"}
                    }       
                    <Slide> {
                        title = {text: "Why"},
                        <SlideBody> {text: "- Fast compute needed\n - Run on web and native\n- Tooling UI"}
                    }       
                    <Slide> {
                        title = {text: "Inception"},
                    }
                    <Slide> {
                        title = {text: "Links"}, 
                        <SlideBody> {text: "- github.com/makepad/makepad\n- makepad.nl"}
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
    #[live] build_manager: BuildManager,
    #[rust] file_system: FileSystem,
}

impl LiveHook for App {
    fn before_live_design(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
        crate::makepad_code_editor::live_design(cx);
        crate::build_manager::build_manager::live_design(cx);
        crate::build_manager::run_list::live_design(cx);
        crate::build_manager::log_list::live_design(cx);
        crate::build_manager::run_view::live_design(cx);
        //cx.start_stdin_service();
    }
    
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        self.file_system.init(cx);
        self.build_manager.init(cx);
        self.build_manager.run_app(live_id!(fractal_zoom),"makepad-example-fractal-zoom");
        self.build_manager.run_app(live_id!(ironfish),"makepad-example-ironfish");
    }
}

impl App {
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        
        let apps = [id!(fractal_zoom),id!(ironfish)];
        
        if let Event::Draw(event) = event {
            //let dt = profile_start();
            let cx = &mut Cx2d::new(cx, event);
            while let Some(next) = self.ui.draw_widget(cx).hook_widget() {
                let run_view_id = next.id();
                if let Some(mut run_view) = next.as_run_view().borrow_mut() {
                    run_view.draw(cx, run_view_id, &mut self.build_manager);
                }
            }
            //profile_end!(dt);
            return
        }
        if let Event::Destruct = event {
            self.build_manager.clear_active_builds();
        }
        let _actions = self.ui.handle_widget_event(cx, event);
        
        //ok i want all runviews... how do i do that
        for app in apps{
            if let Some(mut run_view) = self.ui.run_view(app).borrow_mut(){
                run_view.pump_event_loop(cx, event, app[0], &mut self.build_manager);
                run_view.handle_event(cx, event, app[0], &mut self.build_manager);
            }
        }
        
        for _action in self.file_system.handle_event(cx, event, &self.ui) {}
                
        for action in self.build_manager.handle_event(cx, event, &mut self.file_system) {
            match action {
                BuildManagerAction::StdinToHost {run_view_id, msg} => {
                    if let Some(mut run_view) = self.ui.run_view(&[run_view_id]).borrow_mut() {
                        run_view.handle_stdin_to_host(cx, &msg, run_view_id, &mut self.build_manager);
                    }
                }
                _ => ()
            }
        }
        
    }
}