pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let TalkSlide = Slide{
        padding: 64
        spacing: 18
        draw_bg +: {
            color: theme.color_bg_app
            radius: 0.0
        }
        title := H1{
            text: "SlideTitle"
            draw_text +: {
                color: theme.color_text
                text_style +: {font_size: 40}
            }
        }
    }

    let TalkChapter = SlideChapter{
        padding: 72
        spacing: 22
        draw_bg +: {
            color: theme.color_makepad
            radius: 0.0
        }
        title := H1{
            text: "SlideTitle"
            draw_text +: {
                color: theme.color_text
                text_style +: {font_size: 48}
            }
        }
    }

    let TalkBody = H2{
        width: Fill
        draw_text +: {
            color: theme.color_text
            text_style +: {font_size: 25}
        }
    }

    let TalkSmall = H3{
        width: Fill
        draw_text +: {
            color: theme.color_label_inner_inactive
            text_style +: {font_size: 18}
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1280, 720)
                pass.clear_color: theme.color_bg_app
                body +: {
                    width: Fill
                    height: Fill

                    slides := SlidesView{
                        width: Fill
                        height: Fill
                        anim_speed: 0.86

                        intro := TalkChapter{
                            title.text: "AI-Accelerated\nApplication Development\nfor Rust"
                            TalkBody{text: "- Makepad + AI"}
                            TalkBody{text: "- Visual development loop"}
                        }

                        old_loop := TalkSlide{
                            title.text: "'ChatGPT' Coding"
                            TalkBody{text: "- Ask for code"}
                            TalkBody{text: "- Paste it into an editor"}
                            TalkBody{text: "- Run it yourself"}
                            TalkBody{text: "- Describe the failure back to the AI"}
                            TalkBody{text: "- Repeat"}
                        }

                        agentic_loop := TalkSlide{
                            title.text: "Agentic AI Coding"
                            TalkBody{text: "- Edits files"}
                            TalkBody{text: "- Runs commands"}
                            TalkBody{text: "- Reads errors"}
                            TalkBody{text: "- Iterates autonomously"}
                        }

                        studio_loop := TalkSlide{
                            title.text: "Agentic AI UI"
                            TalkBody{text: "- AI edits the app"}
                            TalkBody{text: "- Runs it inside Studio"}
                            TalkBody{text: "- Sees the live UI"}
                            TalkBody{text: "- Clicks and types"}
                            TalkBody{text: "- Fixes visual behavior"}
                        }

                        rust := TalkSlide{
                            title.text: "Why Rust for AI"
                            TalkBody{text: "- Strong types"}
                            TalkBody{text: "- Ownership checks"}
                            TalkBody{text: "- Precise compiler errors"}
                            TalkBody{text: "- Native performance"}
                        }

                        makepad_monorepo := TalkSlide{
                            title.text: "Makepad for AI"
                            TalkBody{text: "- High performance UI"}
                            TalkBody{text: "- Self-contained stack"}
                            TalkBody{text: "- Live updating DSL"}
                            TalkBody{text: "- Visual feedback for AI"}
                        }

                        targets := TalkSlide{
                            title.text: "Platforms"
                            TalkBody{text: "Desktop: Windows, macOS, Linux"}
                            TalkBody{text: "Web"}
                            TalkBody{text: "Mobile: Android and iOS"}
                            TalkBody{text: "XR: Quest"}
                        }

                        demo_setup := TalkChapter{
                            title.text: "Examples"
                        }

                        demo_robrix := TalkSlide{
                            title.text: "Robrix"
                            TalkBody{text: "- Matrix client"}
                            TalkBody{text: "- Built on Makepad"}
                        }

                        demo_ui_stack := TalkSlide{
                            title.text: "Makepad UI Stack"
                            TalkBody{text: "- Glass style"}
                            TalkBody{text: "- Vector, Markdown, UI"}
                        }

                        demo_aichat := TalkSlide{
                            title.text: "Streaming Splash"
                            TalkBody{text: "- Splash: Our HTML/CSS/JS"}
                            TalkBody{text: "- Streaming generation"}
                            TalkBody{text: "- A2App: Logic"}
                            TalkBody{text: "- AI: Custom context"}
                        }
                        
                        demo_interact := TalkSlide{
                            title.text: "AI UI Control"
                            TalkBody{text: "- Studio Bridge"}
                            TalkBody{text: "- See"}
                            TalkBody{text: "- Manipulate"}
                            TalkBody{text: "- Test"}
                        }
                        
                        demo_splash_3d := TalkSlide{
                            title.text: "CAD Engine"
                            TalkBody{text: "- AI-generated CAD engine"}
                            TalkBody{text: "- Origin: OpenSCAD"}
                            TalkBody{text: "- Splash API"}
                            TalkBody{text: "- AI: Testable->Doable"}
                        }

                        demo_maps := TalkSlide{
                            title.text: "Map Rendering"
                            TalkBody{text: "- AI vector: 4 engines in 3 days"}
                            TalkBody{text: "- Optimisation hints"}
                            TalkBody{text: "- AI: Hard to converge"}
                        }

                        demo_xr := TalkSlide{
                            title.text: "Quest XR"
                            TalkBody{text: "- Quest target"}
                            TalkBody{text: "- World scanning"}
                            TalkBody{text: "- AI: 10 algos per day"}
                        }

                        local_ai := TalkSlide{
                            title.text: "Cloud AI is expensive"
                            TalkBody{text: "- Local AI is cheap"}
                            TalkBody{text: "- Manage cloud locally"}
                        }

                        honest := TalkSlide{
                            title.text: "Visual UI with AI"
                            TalkBody{text: "- Get out of its way"}
                            TalkBody{text: "- See->Fix"}
                            TalkBody{text: "- Automate everything"}
                            TalkBody{text: "- Its all about what"}
                            TalkBody{text: "- Algos are easy now"}
                            TalkBody{text: "- UX Detail is still hard"}
                        }

                        close := TalkChapter{
                            title.text: "Thanks!"
                            TalkBody{text: "- github.com/makepad/makepad"}
                            TalkBody{text: "- Try it out!"}
                        }
                    }
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
