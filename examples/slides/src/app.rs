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
                        title = {text: "A long long time ago …"},
                        news_feed=<RunView> {
                            width:Fill,
                            height:Fill
                        }
                    }
                    <Slide> {
                        title = {text: "A long long time ago …"},
                        ironfish=<RunView> {
                            width:Fill,
                            height:Fill
                        }
                    }
                    
                    <SlideChapter> {
                        title = {text: "MAKEPAD.\nDESIGNING MODERN\nUIs FOR RUST."},
                        <SlideBody> {text: "Rik Arends\n"}
                    }
                    <Slide> {
                        title = {text: "A long long time ago …"},
                        <SlideBody> {text: "… in a galaxy nearby\n   Cloud9 IDE & ACE"}
                    }
                    <Slide> {
                        title = {text: "HTML as an IDE UI?\nMadness!"},
                        <SlideBody> {text: "- Integrating design and code was hard\n- Could not innovate editing\n- Too slow, too hard to control"}
                    }
                    <Slide> {
                        title = {text: "Let's start over!"},
                        <SlideBody> {text: "- JavaScript and WebGL for UI\n- Write shaders to style UI\n- A quick demo"}
                    }
                    <Slide> {
                        title = {text: "Maybe JavaScript\nwas the problem?"},
                        <SlideBody> {text: "- Great livecoding, but …\n- Chrome crashing tabs after 30 minutes\n- Too slow"}
                    }
                    <Slide> {
                        title = {text: "Rust appears"},
                        <SlideBody> {text: "- Let's try again: Native + Wasm\n- Makepad in Rust\n- Startup with Eddy and Sebastian"}
                    }
                    <Slide> {
                        title = {text: "Rust is fast: SIMD Mandelbrot"},
                        align: {x: 0.0, y: 0.5} flow: Down,
                        spacing: 10,
                        padding: 50
                        draw_bg: {color: #x1A, radius: 5.0}
                    }
                    
                    <Slide> {
                        title = {text: "Instanced rendering"},
                        align: {x: 0.0, y: 0.5} flow: Down,
                        spacing: 10,
                        padding: 50
                        draw_bg: {color: #x1A, radius: 5.0}
                    }
                    
                    <Slide> {
                        title = {text: "Our goal:\nUnify coding and UI design again."},
                        <SlideBody> {text: "As it was in Visual Basic.\nNow with modern design."}
                    }
                    
                    <Slide> {title = {text: "Ironfish Desktop"},}
                    
                    <Slide> {title = {text: "Ironfish Mobile"},}
                    
                    <Slide> {title = {text: "Multi modal"},}
                    
                    <Slide> {title = {text: "Visual design"},}
                    
                    <Slide> {
                        title = {text: "Our UI language: Live."},
                        <SlideBody> {text: "- Live editable\n- Design tool manipulates text\n- Inheritance structure\n- Rust-like module system"}
                    }
                    
                    <Slide> {
                        title = {text: "These slides are a Makepad app"},
                        <SlideBody> {text: "- Show source\n"}
                        <SlideBody> {text: "- Show Rust API\n"}
                    }
                    
                    <Slide> {
                        title = {text: "Future"},
                        <SlideBody> {text: "- Release of 0.4.0 soon\n- Windows, Linux, Mac, Web and Android\n- github.com/makepad/makepad\n- twitter: @rikarends @makepad"}
                    }
                    
                    <Slide> {
                        title = {text: "Build for Android"},
                        <SlideBody> {text: "- SDK installer\n- Cargo makepad android\n"}
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
        self.build_manager.run_app(live_id!(news_feed),"makepad-example-news-feed");
        self.build_manager.run_app(live_id!(ironfish),"makepad-example-ironfish");
    }    
}

impl App {
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        
        let apps = [id!(news_feed),id!(ironfish)];
        
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