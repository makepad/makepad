pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let scene_bg = Gradient{x1: 0 y1: 0 x2: 1 y2: 1
        Stop{offset: 0 color: #x09111c}
        Stop{offset: 0.55 color: #x101722}
        Stop{offset: 1 color: #x16111d}
    }
    let scene_cyan = RadGradient{cx: 0.18 cy: 0.22 r: 0.42
        Stop{offset: 0 color: #x54d5ff opacity: 0.55}
        Stop{offset: 0.5 color: #x2e7cff opacity: 0.16}
        Stop{offset: 1 color: #x2e7cff opacity: 0.0}
    }
    let scene_gold = RadGradient{cx: 0.82 cy: 0.18 r: 0.38
        Stop{offset: 0 color: #xffc86f opacity: 0.48}
        Stop{offset: 0.55 color: #xff8f3a opacity: 0.14}
        Stop{offset: 1 color: #xff8f3a opacity: 0.0}
    }
    let scene_violet = RadGradient{cx: 0.64 cy: 0.86 r: 0.44
        Stop{offset: 0 color: #xd79cff opacity: 0.45}
        Stop{offset: 0.55 color: #x7f67ff opacity: 0.12}
        Stop{offset: 1 color: #x7f67ff opacity: 0.0}
    }

    let Pill = RoundedView{
        width: Fit
        height: Fit
        padding: Inset{top: 8, bottom: 8, left: 14, right: 14}
        draw_bg.color: #FFFFFF14
        draw_bg.border_radius: 999.0
        label := Label{
            text: "Pill"
            draw_text.color: #FFFFFFCC
            draw_text.text_style.font_size: 10
        }
    }

    let MetricCard = GlassPanel{
        width: Fill
        height: Fit
        padding: Inset{top: 18, bottom: 18, left: 18, right: 18}
        flow: Down
        spacing: 10
        draw_bg +: {
            tint_color: #FFFFFF
            tint_alpha: 0.13
            border_color: #FFFFFF
            border_alpha: 0.24
            corner_radius: 22.0
            specular_strength: 0.44
            noise_strength: 0.026
            use_scene_blur: 1.0
            blur_amount: 0.85
        }
        overline := Label{
            text: "Metric"
            draw_text.color: #FFFFFF88
            draw_text.text_style.font_size: 10
        }
        value := Label{
            text: "00"
            draw_text.color: #FFFFFF
            draw_text.text_style: theme.font_bold{font_size: 30}
        }
        detail := Label{
            text: "detail"
            draw_text.color: #FFFFFFAA
            draw_text.text_style.font_size: 11
        }
    }

    let RecipeCard = GlassPanel{
        width: Fill
        height: Fit
        padding: Inset{top: 18, bottom: 18, left: 18, right: 18}
        flow: Down
        spacing: 12
        draw_bg +: {
            tint_color: #FFFFFF
            tint_alpha: 0.13
            border_color: #FFFFFF
            border_alpha: 0.24
            corner_radius: 24.0
            specular_strength: 0.46
            noise_strength: 0.024
            use_scene_blur: 1.0
            blur_amount: 0.72
        }
        recipe_pill := Pill{label.text: "Recipe"}
        title := Label{
            text: "Panel"
            draw_text.color: #FFFFFF
            draw_text.text_style: theme.font_bold{font_size: 18}
        }
        copy := Label{
            text: "Description"
            draw_text.color: #FFFFFFB8
            draw_text.text_style.font_size: 11
        }
        swatch := GlassPanel{
            width: Fill
            height: Fit
            padding: Inset{top: 18, bottom: 18, left: 16, right: 16}
            flow: Down
            spacing: 8
            draw_bg +: {
                tint_color: #FFFFFF
                tint_alpha: 0.16
                border_color: #FFFFFF
                border_alpha: 0.28
                corner_radius: 20.0
                specular_strength: 0.5
                noise_strength: 0.022
                use_scene_blur: 1.0
                blur_amount: 0.68
            }
            swatch_title := Label{
                text: "Preview"
                draw_text.color: #FFFFFF
                draw_text.text_style: theme.font_bold{font_size: 14}
            }
            swatch_copy := Label{
                text: "Notes"
                draw_text.color: #FFFFFFB4
                draw_text.text_style.font_size: 11
            }
        }
    }

    let CodeLine = GlassPanel{
        width: Fill
        height: Fit
        padding: Inset{top: 10, bottom: 10, left: 12, right: 12}
        flow: Right
        spacing: 10
        draw_bg +: {
            tint_color: #FFFFFF
            tint_alpha: 0.1
            border_color: #FFFFFF
            border_alpha: 0.18
            corner_radius: 16.0
            specular_strength: 0.34
            noise_strength: 0.022
            use_scene_blur: 1.0
            blur_amount: 0.55
        }
        bullet := RoundedView{
            width: 8
            height: 8
            draw_bg.color: #x54d5ff
            draw_bg.border_radius: 99.0
        }
        code := Label{
            width: Fill
            text: "setting: value"
            draw_text.color: #FFFFFFCC
            draw_text.text_style.font_size: 11
        }
    }

    let SceneVector = Vector{
        width: Fill
        height: 360
        viewbox: vec4(0 0 1200 360)

        Rect{x: 0 y: 0 w: 1200 h: 360 rx: 36 ry: 36 fill: scene_bg}
        Circle{cx: 215 cy: 88 r: 200 fill: scene_cyan}
        Circle{cx: 1030 cy: 92 r: 165 fill: scene_gold}
        Circle{cx: 760 cy: 320 r: 220 fill: scene_violet}

        Rect{x: 78 y: 62 w: 412 h: 212 rx: 28 ry: 28 fill: #FFFFFF08}
        Rect{x: 78 y: 62 w: 412 h: 212 rx: 28 ry: 28 fill: false stroke: #FFFFFF18 stroke_width: 1.2}

        Rect{x: 130 y: 112 w: 320 h: 18 rx: 9 ry: 9 fill: #FFFFFF18}
        Rect{x: 130 y: 148 w: 250 h: 10 rx: 5 ry: 5 fill: #FFFFFF12}
        Rect{x: 130 y: 170 w: 212 h: 10 rx: 5 ry: 5 fill: #FFFFFF10}

        Rect{x: 646 y: 56 w: 416 h: 126 rx: 26 ry: 26 fill: #FFFFFF09}
        Rect{x: 646 y: 56 w: 416 h: 126 rx: 26 ry: 26 fill: false stroke: #FFFFFF16 stroke_width: 1.2}
        Rect{x: 676 y: 88 w: 172 h: 44 rx: 22 ry: 22 fill: #x54d5ff22}
        Rect{x: 870 y: 88 w: 156 h: 44 rx: 22 ry: 22 fill: #xffc86f24}

        Rect{x: 532 y: 216 w: 262 h: 90 rx: 24 ry: 24 fill: #x7f67ff18}
        Rect{x: 532 y: 216 w: 262 h: 90 rx: 24 ry: 24 fill: false stroke: #xeadcff22 stroke_width: 1.0}

        Rect{x: 820 y: 206 w: 226 h: 112 rx: 28 ry: 28 fill: #x8affd11a}
        Rect{x: 820 y: 206 w: 226 h: 112 rx: 28 ry: 28 fill: false stroke: #xd7fff022 stroke_width: 1.0}
    }

    let HeroFeature = GlassPanel{
        width: Fill
        height: Fit
        padding: Inset{top: 14, bottom: 14, left: 16, right: 16}
        flow: Down
        spacing: 6
        draw_bg +: {
            tint_color: #FFFFFF
            tint_alpha: 0.11
            border_color: #FFFFFF
            border_alpha: 0.18
            corner_radius: 18.0
            specular_strength: 0.38
            noise_strength: 0.02
            use_scene_blur: 1.0
            blur_amount: 0.6
        }
        feature_title := Label{
            text: "Feature"
            draw_text.color: #FFFFFF
            draw_text.text_style: theme.font_bold{font_size: 13}
        }
        feature_copy := Label{
            text: "Description"
            draw_text.color: #FFFFFFAA
            draw_text.text_style.font_size: 11
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                pass.clear_color: #00000000
                window.inner_size: vec2(1380, 920)
                window.title: "Windows Blur + Glass Panel"
                window.transparent: false
                window.backdrop_intensity: 1.0
                body +: {
                    padding: Inset{top: 18, bottom: 18, left: 18, right: 18}

                    ScrollYView{
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 18

                        View{
                            width: Fill
                            height: Fit
                            flow: Overlay

                            hero_shell := GlassPanel{
                                width: Fill
                                height: Fit
                                padding: Inset{top: 14, bottom: 14, left: 14, right: 14}
                                draw_bg +: {
                                    tint_color: #FFFFFF
                                    tint_alpha: 0.08
                                    border_color: #FFFFFF
                                    border_alpha: 0.24
                                    corner_radius: 34.0
                                    specular_strength: 0.42
                                    noise_strength: 0.02
                                    use_scene_blur: 1.0
                                    blur_amount: 0.92
                                }
                                SceneVector{}
                            }

                            View{
                                width: Fill
                                height: Fit
                                padding: Inset{top: 32, bottom: 32, left: 34, right: 34}
                                flow: Right
                                spacing: 24

                                View{
                                    width: Fill
                                    height: Fit
                                    flow: Down
                                    spacing: 18

                                    View{
                                        width: Fill
                                        height: Fit
                                        flow: Right
                                        spacing: 10
                                        badge_a := Pill{label.text: "WINDOWS BLUR"}
                                        badge_b := Pill{label.text: "GLASS PANEL"}
                                        badge_c := Pill{label.text: "OS BACKDROP"}
                                    }

                                    Label{
                                        width: 720
                                        text: "Frosted system chrome, layered panels, and enough contrast to feel intentional."
                                        draw_text.color: #FFFFFF
                                        draw_text.text_style: theme.font_bold{font_size: 44}
                                    }

                                    Label{
                                        width: 660
                                        text: "This example is designed like a desktop scene instead of a docs page. The window can opt into Windows Acrylic at startup, while the UI demonstrates multiple glass recipes that still read cleanly on top of noisy content."
                                        draw_text.color: #FFFFFFCC
                                        draw_text.text_style.font_size: 14
                                    }

                                    View{
                                        width: Fill
                                        height: Fit
                                        flow: Right
                                        spacing: 12
                                        feature_a := HeroFeature{
                                            feature_title.text: "Window layer"
                                            feature_copy.text: "Transparent pass plus Acrylic backdrop on Windows."
                                        }
                                        feature_b := HeroFeature{
                                            feature_title.text: "Panel layer"
                                            feature_copy.text: "GlassPanel controls tint, edge, specular, and noise."
                                            draw_bg.tint_color: #x54d5ff
                                        }
                                        feature_c := HeroFeature{
                                            feature_title.text: "Fallback story"
                                            feature_copy.text: "Still looks composed on macOS or Linux without system blur."
                                            draw_bg.tint_color: #xffc86f
                                        }
                                    }
                                }

                                View{
                                    width: 320
                                    height: Fit
                                    flow: Down
                                    spacing: 14

                                    stat_a := MetricCard{
                                        overline.text: "Window mode"
                                        value.text: "Acrylic"
                                        detail.text: "Enabled at startup only when running on Windows."
                                    }

                                    stat_b := MetricCard{
                                        overline.text: "Hero blur"
                                        value.text: "0.92"
                                        detail.text: "The outer shell stays subtle so the wallpaper does the work."
                                        draw_bg.tint_color: #x54d5ff
                                    }

                                    stat_c := MetricCard{
                                        overline.text: "Opacity range"
                                        value.text: "8-16%"
                                        detail.text: "Low alpha keeps the surfaces crisp instead of cloudy."
                                        draw_bg.tint_color: #xffc86f
                                    }
                                }
                            }
                        }

                        View{
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 18

                            View{
                                width: Fill
                                height: Fit
                                flow: Down
                                spacing: 18

                                GlassPanel{
                                    width: Fill
                                    height: Fit
                                    padding: Inset{top: 22, bottom: 22, left: 22, right: 22}
                                    flow: Down
                                    spacing: 18
                                    draw_bg +: {
                                        tint_color: #FFFFFF
                                        tint_alpha: 0.12
                                        border_color: #FFFFFF
                                        border_alpha: 0.22
                                        corner_radius: 30.0
                                        specular_strength: 0.46
                                        noise_strength: 0.026
                                        use_scene_blur: 1.0
                                        blur_amount: 0.78
                                    }

                                    View{
                                        width: Fill
                                        height: Fit
                                        flow: Right
                                        spacing: 12

                                        View{
                                            width: Fill
                                            height: Fit
                                            flow: Down
                                            spacing: 8
                                            Label{
                                                text: "Glass recipes"
                                                draw_text.color: #FFFFFF
                                                draw_text.text_style: theme.font_bold{font_size: 22}
                                            }
                                            Label{
                                                width: 620
                                                text: "Three panel personalities. Each one uses the same GlassPanel primitive, but the tint, edge, and blur balance create very different moods."
                                                draw_text.color: #FFFFFFB8
                                                draw_text.text_style.font_size: 12
                                            }
                                        }

                                        Pill{label.text: "Reusable widget"}
                                    }

                                    View{
                                        width: Fill
                                        height: Fit
                                        flow: Right
                                        spacing: 14

                                        recipe_a := RecipeCard{
                                            recipe_pill.label.text: "Neutral"
                                            title.text: "System shell"
                                            copy.text: "The default material for big containers and content columns."
                                            swatch.swatch_title.text: "Soft edge"
                                            swatch.swatch_copy.text: "Neutral tint, stronger border, low cloudiness."
                                        }

                                        recipe_b := RecipeCard{
                                            recipe_pill.label.text: "Cool"
                                            title.text: "Signal surface"
                                            copy.text: "Useful for data cards, active views, and status-heavy UI."
                                            draw_bg.tint_color: #x54d5ff
                                            swatch.draw_bg.tint_color: #x8affd1
                                            swatch.draw_bg.border_color: #xd7fff0
                                            swatch.swatch_title.text: "Mint glass"
                                            swatch.swatch_copy.text: "Sharper highlights help the layer pop forward."
                                        }

                                        recipe_c := RecipeCard{
                                            recipe_pill.label.text: "Warm"
                                            title.text: "Accent action"
                                            copy.text: "A warmer recipe for priority controls or highlighted content."
                                            draw_bg.tint_color: #xffc86f
                                            swatch.draw_bg.tint_color: #xffc86f
                                            swatch.draw_bg.border_color: #xfff1d2
                                            swatch.swatch_title.text: "Amber glass"
                                            swatch.swatch_copy.text: "A touch more warmth keeps CTAs from disappearing."
                                        }
                                    }
                                }

                                GlassPanel{
                                    width: Fill
                                    height: Fit
                                    padding: Inset{top: 22, bottom: 22, left: 22, right: 22}
                                    flow: Down
                                    spacing: 16
                                    draw_bg +: {
                                        tint_color: #xd79cff
                                        tint_alpha: 0.12
                                        border_color: #xeadcff
                                        border_alpha: 0.24
                                        corner_radius: 30.0
                                        specular_strength: 0.42
                                        noise_strength: 0.024
                                        use_scene_blur: 1.0
                                        blur_amount: 0.74
                                    }

                                    Label{
                                        text: "Composition notes"
                                        draw_text.color: #FFFFFF
                                        draw_text.text_style: theme.font_bold{font_size: 20}
                                    }

                                    Label{
                                        width: Fill
                                        text: "Good glass needs structure around it. The wallpaper vector adds color variation behind the panels, the outer shell is broad and quiet, and the inner cards each have a distinct purpose so the blur reads as material instead of haze."
                                        draw_text.color: #FFFFFFBB
                                        draw_text.text_style.font_size: 12
                                    }

                                    View{
                                        width: Fill
                                        height: Fit
                                        flow: Right
                                        spacing: 12
                                        item_a := Pill{label.text: "Layer big before small"}
                                        item_b := Pill{label.text: "Keep alpha low"}
                                        item_c := Pill{label.text: "Use color sparingly"}
                                    }
                                }
                            }

                            View{
                                width: 350
                                height: Fit
                                flow: Down
                                spacing: 18

                                GlassPanel{
                                    width: Fill
                                    height: Fit
                                    padding: Inset{top: 20, bottom: 20, left: 20, right: 20}
                                    flow: Down
                                    spacing: 12
                                    draw_bg +: {
                                        tint_color: #FFFFFF
                                        tint_alpha: 0.11
                                        border_color: #FFFFFF
                                        border_alpha: 0.22
                                        corner_radius: 26.0
                                        specular_strength: 0.38
                                        noise_strength: 0.024
                                        use_scene_blur: 1.0
                                        blur_amount: 0.64
                                    }

                                    Label{
                                        text: "Implementation"
                                        draw_text.color: #FFFFFF
                                        draw_text.text_style: theme.font_bold{font_size: 18}
                                    }
                                    Label{
                                        width: Fill
                                        text: "These are the minimum settings that make the example convincing without locking it to a single operating system."
                                        draw_text.color: #FFFFFFBB
                                        draw_text.text_style.font_size: 11
                                    }

                                    line_a := CodeLine{code.text: "pass.clear_color: #00000000"}
                                    line_b := CodeLine{code.text: "GlassPanel.draw_bg.tint_alpha: 0.08..0.16"}
                                    line_c := CodeLine{code.text: "GlassPanel.draw_bg.use_scene_blur: 1.0"}
                                    line_d := CodeLine{code.text: "Windows startup: set WindowBackdrop::Acrylic"}
                                }

                                GlassPanel{
                                    width: Fill
                                    height: Fit
                                    padding: Inset{top: 20, bottom: 20, left: 20, right: 20}
                                    flow: Down
                                    spacing: 12
                                    draw_bg +: {
                                        tint_color: #x54d5ff
                                        tint_alpha: 0.1
                                        border_color: #xd8f1ff
                                        border_alpha: 0.22
                                        corner_radius: 26.0
                                        specular_strength: 0.4
                                        noise_strength: 0.022
                                        use_scene_blur: 1.0
                                        blur_amount: 0.62
                                    }

                                    Label{
                                        text: "What to extend"
                                        draw_text.color: #FFFFFF
                                        draw_text.text_style: theme.font_bold{font_size: 18}
                                    }
                                    Label{
                                        width: Fill
                                        text: "A natural next step is a live control strip for tint, border width, specular, and blur amount so the example becomes a real tuning playground."
                                        draw_text.color: #FFFFFFBB
                                        draw_text.text_style.font_size: 11
                                    }

                                    View{
                                        width: Fill
                                        height: Fit
                                        flow: Down
                                        spacing: 10
                                        p1 := Pill{label.text: "Backdrop selector"}
                                        p2 := Pill{label.text: "Tint sliders"}
                                        p3 := Pill{label.text: "Border + noise controls"}
                                    }
                                }
                            }
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

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        if !matches!(cx.os_type(), OsType::Windows) {
            return;
        }

        let window_id = CxWindowPool::id_zero();
        let visuals = WindowVisuals {
            transparent: true,
            backdrop: WindowBackdrop::Acrylic,
            backdrop_intensity: 1.0,
        }
        .normalized();

        if cx.windows[window_id].window_visuals() != visuals {
            cx.windows[window_id].transparent = visuals.transparent;
            cx.windows[window_id].backdrop = visuals.backdrop;
            cx.windows[window_id].backdrop_intensity = visuals.backdrop_intensity;

            if cx.windows[window_id].is_created {
                cx.push_unique_platform_op(CxOsOp::SetWindowVisuals(window_id, visuals));
            }
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
