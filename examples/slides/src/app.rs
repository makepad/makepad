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
                    <Slide> {
                        title = {text: "Hello conference"},
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

}

impl LiveHook for App {
    fn before_live_design(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
        //cx.start_stdin_service();
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ui.handle_event_no_scope(cx, event);
    }
}
