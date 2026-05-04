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
                            TalkBody{text: "Makepad + AI as a complete visual development loop"}
                            TalkSmall{text: "Use left and right arrow keys to navigate"}
                        }

                        thesis := TalkSlide{
                            title.text: "The Core Claim"
                            TalkBody{text: "AI should not only write code. It should build, run, see, click, and iterate."}
                            TalkBody{text: "Rust gives strong compiler feedback. Makepad Studio gives visual runtime feedback."}
                        }

                        old_loop := TalkSlide{
                            title.text: "Traditional AI Coding Loop"
                            TalkBody{text: "1. Ask for code"}
                            TalkBody{text: "2. Paste it into an editor"}
                            TalkBody{text: "3. Run it yourself"}
                            TalkBody{text: "4. Describe the failure back to the AI"}
                            TalkBody{text: "5. Repeat"}
                        }

                        studio_loop := TalkSlide{
                            title.text: "Makepad Studio Loop"
                            TalkBody{text: "1. AI edits the app"}
                            TalkBody{text: "2. AI launches it through Studio"}
                            TalkBody{text: "3. AI reads compiler and runtime feedback"}
                            TalkBody{text: "4. AI inspects screenshots and widget trees"}
                            TalkBody{text: "5. AI clicks, types, fixes, and repeats"}
                        }

                        rust := TalkSlide{
                            title.text: "Why Rust Works for AI"
                            TalkBody{text: "Wrong types, missing imports, ownership mistakes, and many runtime bugs become concrete compiler errors."}
                            TalkBody{text: "For AI, a strict compiler is not friction. It is a steering system."}
                        }

                        makepad := TalkSlide{
                            title.text: "Why Makepad Matters"
                            TalkBody{text: "Makepad is a high performance Rust UI and rendering framework with Studio automation."}
                            TalkBody{text: "The AI can validate visual software in the same loop where it writes the code."}
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
                            TalkBody{text: "The demo is not only the finished app."}
                            TalkBody{text: "The demo is the automated iteration loop."}
                        }

                        demo_simple := TalkSlide{
                            title.text: "Demo 1: Simple App Generation"
                            TalkBody{text: "Generate a compact productivity UI with input, buttons, list state, and status text."}
                            TalkBody{text: "Then let the AI run it, type into it, press return, and verify the UI changed."}
                        }

                        demo_cad := TalkSlide{
                            title.text: "Demo 2: AI-Generated CAD"
                            TalkBody{text: "Generate a CAD-style canvas with points, lines, rectangles, selection, and transforms."}
                            TalkBody{text: "The AI has to reason about coordinates, hit testing, drawing, input, and state."}
                        }

                        demo_3d := TalkSlide{
                            title.text: "Demo 3: AI-Generated 3D"
                            TalkBody{text: "Generate a 3D scene with camera movement, lighting, materials, and visible interaction."}
                            TalkBody{text: "Compile success is only the first checkpoint. Visual correctness matters."}
                        }

                        demo_maps := TalkSlide{
                            title.text: "Demo 4: Map Rendering"
                            TalkBody{text: "Render map data with pan, zoom, markers, labels, and density control."}
                            TalkBody{text: "Maps combine large data, rendering performance, and direct interaction."}
                        }

                        demo_vectors := TalkSlide{
                            title.text: "Demo 5: Vector Engine"
                            TalkBody{text: "Generate paths, fills, strokes, gradients, icons, and animated variants."}
                            TalkBody{text: "The AI can inspect screenshots and refine the output instead of guessing."}
                        }

                        demo_xr := TalkSlide{
                            title.text: "Demo 6: Mixed Reality"
                            TalkBody{text: "Target Quest and spatial interfaces with world scanning or 3D scene inspection."}
                            TalkBody{text: "The same loop applies: generate, run, inspect, interact, and fix."}
                        }

                        honest := TalkSlide{
                            title.text: "Honest Framing"
                            TalkBody{text: "The AI still needs good goals. It still makes mistakes. Human taste and architecture still matter."}
                            TalkBody{text: "The shift is that AI can now participate in the full visual application loop."}
                        }

                        close := TalkChapter{
                            title.text: "Build. Run. See. Click. Improve."
                            TalkBody{text: "Rust gives correctness and performance."}
                            TalkBody{text: "Makepad gives a cross-platform visual runtime that AI can control."}
                            TalkSmall{text: "AI-generated applications no longer need to stop at snippets."}
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
