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
                            title.text: "Rapid High Performance Rust Apps"
                            TalkBody{text: "- Makepad + AI"}
                            TalkBody{text: "- Visual development loop"}
                            TalkSmall{text: "Left / right arrows"}
                        }

                        thesis := TalkSlide{
                            title.text: "The Core Claim"
                            TalkBody{text: "- Build"}
                            TalkBody{text: "- Run"}
                            TalkBody{text: "- See"}
                            TalkBody{text: "- Click"}
                            TalkBody{text: "- Iterate"}
                        }

                        old_loop := TalkSlide{
                            title.text: "Traditional AI Coding Loop"
                            TalkBody{text: "1. Ask for code"}
                            TalkBody{text: "2. Paste it into an editor"}
                            TalkBody{text: "3. Run it yourself"}
                            TalkBody{text: "4. Describe the failure back to the AI"}
                            TalkBody{text: "5. Repeat"}
                        }

                        agentic_loop := TalkSlide{
                            title.text: "Agentic AI Coding"
                            TalkBody{text: "- Edits files"}
                            TalkBody{text: "- Runs commands"}
                            TalkBody{text: "- Reads errors"}
                            TalkBody{text: "- Iterates autonomously"}
                        }

                        studio_loop := TalkSlide{
                            title.text: "Makepad Studio Differentiator"
                            TalkBody{text: "1. AI edits the app"}
                            TalkBody{text: "2. Runs it inside Studio"}
                            TalkBody{text: "3. Sees the live UI"}
                            TalkBody{text: "4. Clicks and types"}
                            TalkBody{text: "5. Fixes visual behavior"}
                        }

                        rust := TalkSlide{
                            title.text: "Why Rust Works for AI"
                            TalkBody{text: "- Strong types"}
                            TalkBody{text: "- Ownership checks"}
                            TalkBody{text: "- Precise compiler errors"}
                            TalkBody{text: "- Native performance"}
                        }

                        makepad := TalkSlide{
                            title.text: "Why Makepad Matters"
                            TalkBody{text: "- High performance UI"}
                            TalkBody{text: "- Custom rendering"}
                            TalkBody{text: "- Studio automation"}
                            TalkBody{text: "- Visual feedback for AI"}
                        }

                        makepad_monorepo := TalkSlide{
                            title.text: "Makepad as an AI Target"
                            TalkBody{text: "- Monorepo"}
                            TalkBody{text: "- Self-contained stack"}
                            TalkBody{text: "- Low dependency surface"}
                            TalkBody{text: "- Easy to inspect"}
                            TalkBody{text: "- Easy to modify"}
                            TalkBody{text: "- Adapt the stack itself"}
                        }

                        targets := TalkSlide{
                            title.text: "One Codebase, Many Targets"
                            TalkBody{text: "Desktop: Windows, macOS, Linux"}
                            TalkBody{text: "Web"}
                            TalkBody{text: "Mobile: Android and iOS"}
                            TalkBody{text: "XR: Quest"}
                        }

                        demo_setup := TalkChapter{
                            title.text: "The Demos"
                            TalkBody{text: "- Not just the final app"}
                            TalkBody{text: "- The automated loop"}
                        }

                        demo_simple := TalkSlide{
                            title.text: "Demo 1: Simple App Generation"
                            TalkBody{text: "- Input"}
                            TalkBody{text: "- Buttons"}
                            TalkBody{text: "- List state"}
                            TalkBody{text: "- AI types and verifies"}
                        }

                        demo_aichat := TalkSlide{
                            title.text: "Demo 2: Streaming Splash UIs"
                            TalkBody{text: "- aichat"}
                            TalkBody{text: "- Streaming generation"}
                            TalkBody{text: "- Live Splash UI"}
                            TalkBody{text: "- Render while chatting"}
                        }

                        demo_splash_3d := TalkSlide{
                            title.text: "Demo 3: Realtime 3D in Splash"
                            TalkBody{text: "- Prompt to model"}
                            TalkBody{text: "- Live Splash render"}
                            TalkBody{text: "- Realtime updates"}
                            TalkBody{text: "- Shape, detail, material"}
                        }

                        demo_3d := TalkSlide{
                            title.text: "Demo 4: AI-Generated 3D"
                            TalkBody{text: "- Scene"}
                            TalkBody{text: "- Camera"}
                            TalkBody{text: "- Lighting"}
                            TalkBody{text: "- Materials"}
                            TalkBody{text: "- Visual verification"}
                        }

                        demo_maps := TalkSlide{
                            title.text: "Demo 5: Map Rendering"
                            TalkBody{text: "- Map data"}
                            TalkBody{text: "- Pan and zoom"}
                            TalkBody{text: "- Markers and labels"}
                            TalkBody{text: "- Rendering density"}
                        }

                        demo_vectors := TalkSlide{
                            title.text: "Demo 6: Vector Engine"
                            TalkBody{text: "- Paths"}
                            TalkBody{text: "- Fills and strokes"}
                            TalkBody{text: "- Gradients"}
                            TalkBody{text: "- Animated variants"}
                        }

                        demo_xr := TalkSlide{
                            title.text: "Demo 7: Mixed Reality"
                            TalkBody{text: "- Quest target"}
                            TalkBody{text: "- Spatial UI"}
                            TalkBody{text: "- World scanning"}
                            TalkBody{text: "- 3D scene inspection"}
                        }

                        honest := TalkSlide{
                            title.text: "Honest Framing"
                            TalkBody{text: "- Goals still matter"}
                            TalkBody{text: "- AI still makes mistakes"}
                            TalkBody{text: "- Taste still matters"}
                            TalkBody{text: "- Architecture still matters"}
                        }

                        close := TalkChapter{
                            title.text: "Build. Run. See. Click. Improve."
                            TalkBody{text: "- Rust: correctness + performance"}
                            TalkBody{text: "- Makepad: visual runtime"}
                            TalkBody{text: "- AI: full app loop"}
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
