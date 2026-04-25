pub use makepad_code_editor;
// Linking the kit activates its `script_mod!` block, which registers
// `mod.widgets.DiagramView`. Without this `pub use`, the DSL can't resolve
// the template below.
pub use makepad_diagram_kit;
pub use makepad_widgets;

use makepad_ai::*;
use makepad_widgets::makepad_platform::makepad_micro_serde::*;
use makepad_widgets::*;
use streaming_markdown_kit::{
    streaming_display_with_latex_autowrap_remend, wrap_bare_latex, SanitizeOptions,
};

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.widgets.CodeView
    use mod.widgets.DiagramView
    use mod.text.*
    use mod.res.*
    use mod.draw.*

    // Override theme fonts. Two purposes:
    //   1. font_code — CJK-capable monospace (LXGW Mono) so `` `inline` ``
    //      and CodeView render Chinese correctly.
    //   2. font_regular — add a symbols-capable latin (NotoSans) so Unicode
    //      blocks outside IBM Plex Sans's repertoire (arrows U+2190-U+21FF,
    //      math operators, misc technical) render as glyphs instead of tofu.
    //
    // Note: Makepad's Markdown widget bakes `theme.font_*` at expansion time,
    // so these theme-level overrides are necessary but not sufficient —
    // per-instance overrides on each Markdown instance are also applied below.
    mod.themes.dark = mod.themes.dark{
        font_code: TextStyle{
            font_size: theme.font_size_code
            font_family: FontFamily{
                latin := FontMember{res: crate_resource("self:resources/LiberationMono-Regular.ttf") asc: 0.0 desc: 0.0}
                chinese := FontMember{res: crate_resource("self:resources/LXGWWenKaiMono-Regular.ttf") asc: 0.0 desc: 0.0}
                symbols := FontMember{res: crate_resource("self:resources/NotoSans-Regular.ttf") asc: 0.0 desc: 0.0}
                emoji := FontMember{res: crate_resource("self:resources/NotoColorEmoji.ttf") asc: 0.0 desc: 0.0}
            }
            line_spacing: 1.35
        }
        font_regular: mod.themes.dark.font_regular{
            font_family: FontFamily{
                latin := FontMember{res: crate_resource("self:resources/NotoSans-Regular.ttf") asc: 0.0 desc: 0.0}
                chinese := FontMember{res: crate_resource("self:resources/LXGWWenKaiMono-Regular.ttf") asc: 0.0 desc: 0.0}
                symbols := FontMember{res: crate_resource("self:resources/NotoSans-Regular.ttf") asc: 0.0 desc: 0.0}
                emoji := FontMember{res: crate_resource("self:resources/NotoColorEmoji.ttf") asc: 0.0 desc: 0.0}
            }
        }
    }

    let ai_ink = #x06130F
    let ai_panel = #x06251D
    let ai_panel_deep = #x031510
    let ai_cream = #xF3E3C7
    let ai_cream_dim = #xCDBF9FAA
    let ai_cyan = #x72E4FF
    let ai_cyan_soft = #x72E4FF55
    let ai_gold = #xF6BE63
    let ai_gold_soft = #xF6BE6388

    let chat_scene_bg = Gradient{x1: 0 y1: 0 x2: 1 y2: 1
        Stop{offset: 0 color: #x071018 opacity: 0.56}
        Stop{offset: 0.44 color: #x121923 opacity: 0.52}
        Stop{offset: 0.72 color: #x18202B opacity: 0.48}
        Stop{offset: 1 color: #x201722 opacity: 0.52}
    }

    let chat_scene_cyan = RadGradient{cx: 0.14 cy: 0.16 r: 0.44
        Stop{offset: 0 color: #x72E4FF opacity: 0.70}
        Stop{offset: 0.44 color: #x2E84FF opacity: 0.20}
        Stop{offset: 1 color: #x2E84FF opacity: 0.0}
    }

    let chat_scene_gold = RadGradient{cx: 0.88 cy: 0.14 r: 0.36
        Stop{offset: 0 color: #xFFD18A opacity: 0.52}
        Stop{offset: 0.50 color: #xFF8F3A opacity: 0.15}
        Stop{offset: 1 color: #xFF8F3A opacity: 0.0}
    }

    let chat_scene_violet = RadGradient{cx: 0.64 cy: 0.88 r: 0.48
        Stop{offset: 0 color: #xDCA5FF opacity: 0.48}
        Stop{offset: 0.54 color: #x806DFF opacity: 0.14}
        Stop{offset: 1 color: #x806DFF opacity: 0.0}
    }

    let chat_scene_mint = RadGradient{cx: 0.28 cy: 0.76 r: 0.38
        Stop{offset: 0 color: #x8AFFD1 opacity: 0.42}
        Stop{offset: 0.48 color: #x2BD7B7 opacity: 0.12}
        Stop{offset: 1 color: #x2BD7B7 opacity: 0.0}
    }

    let ChatSceneVector = Vector{
        width: Fill
        height: Fill
        viewbox: vec4(0 0 1200 820)

        Rect{x: 0 y: 0 w: 1200 h: 820 fill: chat_scene_bg}
        Circle{cx: 160 cy: 112 r: 350 fill: chat_scene_cyan}
        Circle{cx: 1080 cy: 112 r: 290 fill: chat_scene_gold}
        Circle{cx: 768 cy: 790 r: 390 fill: chat_scene_violet}
        Circle{cx: 320 cy: 650 r: 300 fill: chat_scene_mint}

        Rect{x: 24 y: 28 w: 1152 h: 760 rx: 38 ry: 38 fill: #x07101822}
        Rect{x: 24 y: 28 w: 1152 h: 760 rx: 38 ry: 38 fill: false stroke: #xFFFFFF1A stroke_width: 1.2}
        Rect{x: 28 y: 32 w: 1144 h: 752 rx: 36 ry: 36 fill: false stroke: #x72E4FF20 stroke_width: 1.0}
        Rect{x: 42 y: 44 w: 1116 h: 724 rx: 32 ry: 32 fill: false stroke: #xFFD18A10 stroke_width: 0.8}

        Path{d: "M -80 190 C 170 72 330 120 520 70 S 905 20 1280 110" fill: false stroke: #x72E4FF22 stroke_width: 2.6 stroke_linecap: "round"}
        Path{d: "M -60 610 C 160 500 348 548 548 480 S 900 380 1260 475" fill: false stroke: #xDCA5FF1E stroke_width: 2.2 stroke_linecap: "round"}
        Path{d: "M 1120 -40 C 960 156 900 286 730 374 S 470 528 248 878" fill: false stroke: #xFFD18A1A stroke_width: 2.0 stroke_linecap: "round"}

        Rect{x: 92 y: 74 w: 320 h: 118 rx: 34 ry: 34 fill: #xFFFFFF05}
        Rect{x: 850 y: 84 w: 244 h: 88 rx: 30 ry: 30 fill: #xFFFFFF06}
        Rect{x: 470 y: 612 w: 330 h: 118 rx: 34 ry: 34 fill: #xFFFFFF05}
    }

    let ToolbarLabel = Label {
        draw_text.color: ai_cream_dim
        draw_text.text_style.font_size: 11
    }

    let PillButton = ButtonFlat {
        height: 34
        padding: Inset{left: 14 right: 14 top: 0 bottom: 0}
        draw_text +: {
            color: ai_cream
            text_style +: { font_size: 11 }
        }
        draw_bg +: {
            color: #x0B241CCC
            color_hover: #x12362ADD
            border_color: #x72E4FF30
            border_size: 1.0
            border_radius: 17.0
        }
    }

    let IconButton = ButtonFlat {
        width: 36
        height: 36
        padding: 0
        draw_text +: {
            color: ai_cream
            text_style +: { font_size: 15 }
        }
        draw_bg +: {
            color: #x0B241CAA
            color_hover: #x174335DD
            border_color: #x72E4FF26
            border_size: 1.0
            border_radius: 18.0
        }
    }

    let ChatList = #(ChatList::register_widget(vm)) {
        width: Fill
        height: Fill

        list := PortalList {
            width: Fill
            height: Fill
            flow: Down
            drag_scrolling: false
            auto_tail: true
            smooth_tail: true
            selectable: true

            User := RoundedView {
                width: Fill
                height: Fit
                margin: Inset{top: 4 bottom: 4 left: 50 right: 8}
                padding: Inset{left: 12 top: 8 right: 12 bottom: 8}
                flow: Overlay
                show_bg: true
                draw_bg +: {
                    color: #x0B2A22E6
                    radius: 12.0
                }

                selectable := Markdown {
                    width: Fill
                    height: Fit
                    selectable: true
                    use_code_block_widget: true
                    use_math_widget: true
                    body: ""
                    // Per-instance override for `` `inline code` ``. The
                    // Markdown widget bakes `theme.font_code` at expansion
                    // time, so a later `mod.themes.dark{...}` override
                    // doesn't reach it. Without this override, CJK inside
                    // backticks renders as tofu (no glyph) because Liberation
                    // Mono is Latin-only.
                    text_style_fixed: theme.font_code{
                        font_family: FontFamily{
                            latin := FontMember{res: crate_resource("self:resources/LiberationMono-Regular.ttf") asc: 0.0 desc: 0.0}
                            chinese := FontMember{res: crate_resource("self:resources/LXGWWenKaiMono-Regular.ttf") asc: 0.0 desc: 0.0}
                            symbols := FontMember{res: crate_resource("self:resources/NotoSans-Regular.ttf") asc: 0.0 desc: 0.0}
                            emoji := FontMember{res: crate_resource("self:resources/NotoColorEmoji.ttf") asc: 0.0 desc: 0.0}
                        }
                    }
                    // Prose font family with symbols fallback — fixes "tofu"
                    // for Unicode arrows / math / misc technical symbols
                    // (observed trigger: `1→5`, `≤`, `≥`, `α` in prose).
                    text_style_normal: theme.font_regular{
                        font_family: FontFamily{
                            latin := FontMember{res: crate_resource("self:resources/NotoSans-Regular.ttf") asc: 0.0 desc: 0.0}
                            chinese := FontMember{res: crate_resource("self:resources/LXGWWenKaiMono-Regular.ttf") asc: 0.0 desc: 0.0}
                            symbols := FontMember{res: crate_resource("self:resources/NotoSans-Regular.ttf") asc: 0.0 desc: 0.0}
                            emoji := FontMember{res: crate_resource("self:resources/NotoColorEmoji.ttf") asc: 0.0 desc: 0.0}
                        }
                    }
                    code_block := ScrollXView {
                        width: Fill
                        height: Fit
                        flow: Right
                        code_view := CodeView {
                            keep_cursor_at_end: false
                            editor +: {
                                height: Fit
                                draw_bg +: { color: #x031510EE }
                            }
                        }
                    }
                    splash_block := View {
                        width: Fill
                        height: Fit
                        splash_view := Splash {
                            width: Fill
                            height: Fit
                        }
                    }
                    // Diagram block — rendered by makepad-diagram-kit's
                    // DiagramView. The inner `diagram_view` id matches what
                    // the markdown widget's `ids!(diagram_view).set_text`
                    // dispatch expects.
                    diagram_block := ScrollXView {
                        width: Fill
                        height: Fit
                        flow: Right
                        diagram_view := DiagramView {
                            width: Fit
                            height: Fit
                        }
                    }
                    inline_math := MathView {
                        font_size: 13.0
                    }
                    display_math := MathView {
                        font_size: 15.0
                    }
                }

                View {
                    width: Fill
                    height: Fit
                    align: Align{x: 1.0}
                    delete_button := ButtonFlat {
                        width: Fit
                        height: Fit
                        padding: Inset{top: 2 bottom: 2 left: 6 right: 6}
                        margin: Inset{top: 2 right: 2}
                        text: "x"
                        draw_text +: {
                            color: #888
                            text_style +: { font_size: 9 }
                        }
                    }
                }
            }

            Assistant := RoundedView {
                width: Fill
                height: Fit
                margin: Inset{top: 4 bottom: 4 left: 8 right: 50}
                padding: Inset{left: 12 top: 8 right: 12 bottom: 8}
                flow: Overlay
                show_bg: true
                draw_bg +: {
                    color: #x0B2A22E6
                    radius: 12.0
                }

                RubberView {
                    width: Fill
                    height: Fit
                    smoothing: 0.3

                    selectable := Markdown {
                        width: Fill
                        height: Fit
                        selectable: true
                        use_code_block_widget: true
                        use_math_widget: true
                        body: ""
                        // Per-instance override — same as User's Markdown
                        // above. Fixes `` `中文` `` inline-code tofu.
                        text_style_fixed: theme.font_code{
                            font_family: FontFamily{
                                latin := FontMember{res: crate_resource("self:resources/LiberationMono-Regular.ttf") asc: 0.0 desc: 0.0}
                                chinese := FontMember{res: crate_resource("self:resources/LXGWWenKaiMono-Regular.ttf") asc: 0.0 desc: 0.0}
                                emoji := FontMember{res: crate_resource("self:resources/NotoColorEmoji.ttf") asc: 0.0 desc: 0.0}
                            }
                        }
                        draw_text +: {
                            get_color: fn() {
                                let fade_chars = 50.0
                                let dist_from_end = self.total_chars - self.char_index
                                let t = clamp(dist_from_end / fade_chars, 0.0, 1.0)
                                let alpha = pow(t, 0.5)
                                return vec4(self.color.rgb, self.color.a * alpha)
                            }
                        }
                        code_block := ScrollXView {
                            width: Fill
                            height: Fit
                            flow: Right
                            code_view := CodeView {
                                keep_cursor_at_end: true
                                editor +: {
                                    height: Fit
                                    draw_bg +: { color: #x031510EE }
                                    // Local font override: CodeView is defined in the
                                    // makepad-code-editor crate and bakes `theme.font_code`
                                    // at its own expansion time, so later `mod.themes.dark`
                                    // overrides don't reach it. Override per-instance.
                                    draw_text +: {
                                        text_style: theme.font_code{
                                            font_family: FontFamily{
                                                latin := FontMember{res: crate_resource("self:resources/LiberationMono-Regular.ttf") asc: 0.0 desc: 0.0}
                                                chinese := FontMember{res: crate_resource("self:resources/LXGWWenKaiMono-Regular.ttf") asc: 0.0 desc: 0.0}
                                                emoji := FontMember{res: crate_resource("self:resources/NotoColorEmoji.ttf") asc: 0.0 desc: 0.0}
                                            }
                                        }
                                    }
                                    draw_gutter +: {
                                        text_style: theme.font_code{
                                            font_family: FontFamily{
                                                latin := FontMember{res: crate_resource("self:resources/LiberationMono-Regular.ttf") asc: 0.0 desc: 0.0}
                                                chinese := FontMember{res: crate_resource("self:resources/LXGWWenKaiMono-Regular.ttf") asc: 0.0 desc: 0.0}
                                                emoji := FontMember{res: crate_resource("self:resources/NotoColorEmoji.ttf") asc: 0.0 desc: 0.0}
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        splash_block := SolidView{
                            flow: Overlay
                            new_batch: true
                            width: Fill
                            height: Fit
                            splash_view := Splash {
                                flow: Overlay
                                width: Fill
                                height: Fit
                            }
                        }
                        // Diagram block — see User-side comment.
                        diagram_block := ScrollXView{
                            flow: Right
                            new_batch: true
                            width: Fill
                            height: Fit
                            diagram_view := DiagramView {
                                width: Fit
                                height: Fit
                            }
                        }
                        inline_math := MathView {
                            font_size: 13.0
                        }
                        display_math := MathView {
                            font_size: 15.0
                        }
                    }
                }

                View {
                    width: Fill
                    height: Fit
                    align: Align{x: 1.0}
                    copy_button := ButtonFlat {
                        width: Fit
                        height: Fit
                        padding: Inset{top: 2 bottom: 2 left: 6 right: 6}
                        margin: Inset{top: 2 right: 2}
                        text: "copy"
                        draw_text +: {
                            color: #888
                            text_style +: { font_size: 9 }
                        }
                    }
                    delete_button := ButtonFlat {
                        width: Fit
                        height: Fit
                        padding: Inset{top: 2 bottom: 2 left: 6 right: 6}
                        margin: Inset{top: 2 right: 2}
                        text: "x"
                        draw_text +: {
                            color: #888
                            text_style +: { font_size: 9 }
                        }
                    }
                }
            }
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                show_caption_bar: false
                pass.clear_color: #00000000
                window.transparent: true
                window.macos: MacosWindowConfig{chrome: MacosWindowChrome.Borderless}
                window.inner_size: vec2(900, 700)
                window.title: " "
                body +: {
                    flow: Overlay
                    padding: 12
                    spacing: 0
                    draw_bg.color: #00000000

                    app_shell := GlassPanel {
                        width: Fill
                        height: Fill
                        new_batch: true
                        flow: Right
                        padding: Inset{left: 16 top: 16 right: 16 bottom: 16}
                        spacing: 0
                        draw_bg +: {
                            tint_color: ai_panel
                            tint_alpha: 0.90
                            border_color: ai_cyan
                            border_alpha: 0.58
                            border_width: 1.2
                            corner_radius: 30.0
                            specular_strength: 0.30
                            noise_strength: 0.010
                            use_scene_blur: 1.0
                            blur_amount: 0.10
                        }

                    sidebar := GlassPanel {
                        width: 298
                        height: Fill
                        new_batch: true
                        flow: Down
                        padding: Inset{left: 14 top: 14 right: 14 bottom: 14}
                        spacing: 10
                        draw_bg +: {
                            tint_color: #x03130F
                            tint_alpha: 1.0
                            border_color: #xEAD8B8
                            border_alpha: 0.0
                            border_width: 0.0
                            corner_radius: 0.0
                            specular_strength: 0.0
                            noise_strength: 0.004
                            use_scene_blur: 1.0
                            blur_amount: 0.0
                        }

                        sidebar_header := View {
                            width: Fill
                            height: Fit
                            flow: Down
                            spacing: 8
                            margin: Inset{top: 4 bottom: 18}

                            View {
                                width: Fill
                                height: Fit
                                flow: Right
                                spacing: 10
                                align: Align{y: 0.5}

                                Label {
                                    text: "AI"
                                    draw_text.color: ai_cyan
                                    draw_text.text_style.font_size: 14
                                }

                                Label {
                                    text: "AI Chat"
                                    draw_text.color: ai_cream
                                    draw_text.text_style.font_size: 15
                                }
                            }

                            Label {
                                text: "Diagram workspace"
                                draw_text.color: ai_cream_dim
                                draw_text.text_style.font_size: 11
                            }
                        }

                        nav_new := ButtonFlat {
                            width: Fill
                            height: 38
                            text: "+  新对话"
                            align: Align{x: 0.0 y: 0.5}
                            padding: Inset{left: 14 right: 12}
                            draw_text +: {
                                color: ai_cream
                                text_style +: { font_size: 12 }
                            }
                            draw_bg +: {
                                color: #x0B6B67AA
                                color_hover: #x108E88CC
                                border_color: #x72E4FF66
                                border_size: 1.0
                                border_radius: 10.0
                            }
                        }

                        nav_search := ButtonFlat {
                            width: Fill
                            height: 30
                            text: "⌕  搜索"
                            align: Align{x: 0.0 y: 0.5}
                            padding: Inset{left: 4 right: 4}
                            draw_text +: {
                                color: #xE4D4B6
                                text_style +: { font_size: 12 }
                            }
                            draw_bg +: {
                                color: #00000000
                                color_hover: #xEAD8B814
                                border_size: 0.0
                                border_radius: 8.0
                            }
                        }

                        nav_plugins := ButtonFlat {
                            width: Fill
                            height: 30
                            text: "⌘  插件"
                            align: Align{x: 0.0 y: 0.5}
                            padding: Inset{left: 4 right: 4}
                            draw_text +: {
                                color: #xE4D4B6
                                text_style +: { font_size: 12 }
                            }
                            draw_bg +: {
                                color: #00000000
                                color_hover: #xEAD8B814
                                border_size: 0.0
                                border_radius: 8.0
                            }
                        }

                        nav_automation := ButtonFlat {
                            width: Fill
                            height: 30
                            text: ">  自动化"
                            align: Align{x: 0.0 y: 0.5}
                            padding: Inset{left: 4 right: 4}
                            draw_text +: {
                                color: #xE4D4B6
                                text_style +: { font_size: 12 }
                            }
                            draw_bg +: {
                                color: #00000000
                                color_hover: #xEAD8B814
                                border_size: 0.0
                                border_radius: 8.0
                            }
                        }

                        nav_project := ButtonFlat {
                            width: Fill
                            height: 30
                            text: "#  项目"
                            align: Align{x: 0.0 y: 0.5}
                            padding: Inset{left: 4 right: 4}
                            draw_text +: {
                                color: #xE4D4B6
                                text_style +: { font_size: 12 }
                            }
                            draw_bg +: {
                                color: #00000000
                                color_hover: #xEAD8B814
                                border_size: 0.0
                                border_radius: 8.0
                            }
                        }

                        Label {
                            text: "对话"
                            margin: Inset{top: 28 bottom: 2 left: 0 right: 0}
                            draw_text.color: #xCDBF9FA0
                            draw_text.text_style.font_size: 12
                        }

                        Label {
                            text: "暂无聊天"
                            draw_text.color: #xCDBF9F55
                            draw_text.text_style.font_size: 12
                        }

                        View { width: Fill height: Fill }

                        settings_button := ButtonFlat {
                            width: Fill
                            height: 32
                            text: "*  设置"
                            align: Align{x: 0.0 y: 0.5}
                            padding: Inset{left: 4 right: 4}
                            draw_text +: {
                                color: #xF3E3C7
                                text_style +: { font_size: 12 }
                            }
                            draw_bg +: {
                                color: #00000000
                                color_hover: #xEAD8B814
                                border_size: 0.0
                                border_radius: 8.0
                            }
                        }
                    }

                    SolidView {
                        width: 1
                        height: Fill
                        draw_bg.color: #xEAD8B81E
                    }

                    main_area := GlassPanel {
                        width: Fill
                        height: Fill
                        new_batch: true
                        flow: Down
                        padding: Inset{left: 34 top: 18 right: 34 bottom: 22}
                        spacing: 12
                        draw_bg +: {
                            tint_color: #x061B16
                            tint_alpha: 0.96
                            border_color: #xEAD8B8
                            border_alpha: 0.0
                            border_width: 0.0
                            corner_radius: 0.0
                            specular_strength: 0.0
                            noise_strength: 0.004
                            use_scene_blur: 1.0
                            blur_amount: 0.0
                        }

                        top_bar := View {
                            width: Fill
                            height: 40
                            flow: Right
                            align: Align{y: 0.5}

                            Label {
                                text: "AI Chat"
                                draw_text.color: ai_cream
                                draw_text.text_style.font_size: 14
                            }

                            View { width: Fill height: 1 }

                            ToolbarLabel {
                                text: "Backend"
                                margin: Inset{right: 8}
                            }

                            backend_dropdown := DropDown {
                                width: 168
                                height: 34
                                labels: ["Claude Code" "Claude Splash" "Claude (ACP)" "Claude (API)" "Gemini" "Gemini Splash" "OpenAI" "Moonshot"]
                                draw_text +: {
                                    color: ai_cream
                                    text_style +: { font_size: 11 }
                                }
                                draw_bg +: {
                                    color: #x0B241CCC
                                    color_hover: #x12362ADD
                                    border_color: #x72E4FF30
                                    border_size: 1.0
                                    border_radius: 17.0
                                    arrow_color: ai_cream
                                }
                            }

                            ToolbarLabel {
                                text: "Glass"
                                margin: Inset{left: 16 right: 8}
                            }

                            opacity_slider := SliderMinimalFlat {
                                width: 170
                                height: 26
                                text: ""
                                min: 0.72
                                max: 0.98
                                step: 0.01
                                default: 0.90
                                precision: 2
                                label_walk: Walk{width: 0 height: 0}
                                text_input: TextInput{
                                    width: 0
                                    height: 0
                                    is_read_only: true
                                }
                                draw_bg +: {
                                    color: #x07130F
                                    color_hover: #x0E2118
                                    color_focus: #x11281D
                                    color_drag: #x153123
                                    color_2: #x07130F
                                    color_2_hover: #x0E2118
                                    color_2_focus: #x11281D
                                    color_2_drag: #x153123
                                    val_color: ai_gold
                                    val_color_hover: #xFFD98B
                                    val_color_focus: #xFFD98B
                                    val_color_drag: #xFFE2A3
                                    handle_color: ai_gold
                                    handle_color_hover: #xFFF0D2
                                    handle_color_focus: #xFFF0D2
                                    handle_color_drag: #xFFF0D2
                                    border_color: ai_cyan_soft
                                    border_color_2: #x00000066
                                    offset_y: 10.0
                                    handle_size: 16.0
                                }
                            }

                            opacity_value := Label {
                                width: 40
                                text: "90%"
                                margin: Inset{left: 6}
                                draw_text.color: ai_cream_dim
                                draw_text.text_style.font_size: 11
                            }
                        }

                        chat_shell := View {
                            width: Fill
                            height: Fill
                            flow: Overlay

                            empty_state := View {
                                width: Fill
                                height: Fill
                                flow: Down
                                align: Align{x: 0.5 y: 0.46}
                                spacing: 18

                                Label {
                                    text: "我们该做什么？"
                                    draw_text.color: #xF3E3C7
                                    draw_text.text_style.font_size: 27
                                }

                                Label {
                                    text: "输入自然语言，生成可交互的 Makepad diagram。"
                                    draw_text.color: #xCDBF9FAA
                                    draw_text.text_style.font_size: 12
                                }
                            }

                            chat_list := ChatList {}
                        }

                        composer_row := View {
                            width: Fill
                            height: Fit
                            align: Align{x: 0.5 y: 0.0}

                            composer := GlassPanel {
                                width: Fill{min: 620 max: 1040}
                                height: Fit
                                new_batch: true
                                flow: Down
                                padding: Inset{left: 18 top: 14 right: 14 bottom: 12}
                                spacing: 10
                                draw_bg +: {
                                    tint_color: ai_panel
                                    tint_alpha: 0.98
                                    border_color: ai_cyan
                                    border_alpha: 0.46
                                    border_width: 1.2
                                    corner_radius: 24.0
                                    specular_strength: 0.24
                                    noise_strength: 0.010
                                    use_scene_blur: 1.0
                                    blur_amount: 0.08
                                }

                                input := TextInput {
                                    width: Fill
                                    height: 56
                                    empty_text: "问 Codex 任何事。输入 @ 使用插件或提及文件"
                                    draw_bg +: {
                                        color: #00000000
                                        color_hover: #00000000
                                        color_focus: #00000000
                                        border_size: 0.0
                                        border_radius: 0.0
                                    }
                                    // Per-instance font override — TextInput bakes
                                    // `theme.font_regular` at DSL-expansion time, same
                                    // issue as Markdown/CodeView. Without this the
                                    // input box shows tofu for CJK and U+2192 arrows.
                                    draw_text +: {
                                        color: ai_cream
                                        color_empty: ai_cream_dim
                                        text_style: theme.font_regular{
                                            line_spacing: theme.font_wdgt_line_spacing
                                            font_size: 13
                                            font_family: FontFamily{
                                                latin := FontMember{res: crate_resource("self:resources/NotoSans-Regular.ttf") asc: 0.0 desc: 0.0}
                                                chinese := FontMember{res: crate_resource("self:resources/LXGWWenKaiMono-Regular.ttf") asc: 0.0 desc: 0.0}
                                                symbols := FontMember{res: crate_resource("self:resources/NotoSans-Regular.ttf") asc: 0.0 desc: 0.0}
                                                emoji := FontMember{res: crate_resource("self:resources/NotoColorEmoji.ttf") asc: 0.0 desc: 0.0}
                                            }
                                        }
                                    }
                                }

                                composer_actions := View {
                                    width: Fill
                                    height: Fit
                                    flow: Right
                                    align: Align{y: 0.5}
                                    spacing: 8

                                    attach_button := IconButton { text: "+" }

                                    mention_button := IconButton { text: "@" }

                                    tools_button := IconButton { text: "⌘" }

                                    Label {
                                        text: "默认权限"
                                        draw_text.color: ai_cream_dim
                                        draw_text.text_style.font_size: 11
                                    }

                                    View { width: Fill height: 1 }

                                    cancel_button := ButtonFlat {
                                        text: "Cancel"
                                        width: 72
                                        height: 32
                                        visible: false
                                        draw_text +: {
                                            color: #xF2F4F8
                                            text_style +: { font_size: 11 }
                                        }
                                        draw_bg +: {
                                            color: #x4B332FCC
                                            color_hover: #x64413ADD
                                            border_color: #xEAD8B818
                                            border_size: 1.0
                                            border_radius: 10.0
                                        }
                                    }

                                    clear_button := PillButton {
                                        text: "Clear"
                                        width: 72
                                    }

                                    send_button := ButtonFlat {
                                        text: "↑"
                                        width: 44
                                        height: 44
                                        padding: 0
                                        draw_text +: {
                                            color: ai_ink
                                            text_style +: { font_size: 20 }
                                        }
                                        draw_bg +: {
                                            color: ai_gold
                                            color_hover: #xFFD98B
                                            border_color: #xFFFFFF00
                                            border_size: 0.0
                                            border_radius: 22.0
                                        }
                                    }
                                }
                            }
                        }

                        status_label := Label {
                            width: Fill
                            height: Fit
                            text: "Initializing..."
                            margin: Inset{left: 92 right: 92 top: 0 bottom: 0}
                            draw_text.text_style.font_size: 10
                            draw_text.color: #xCDBF9F88
                        }
                    }
                    }

                    resize_grip := Vector{
                        width: 34
                        height: 34
                        margin: Inset{right: 18 bottom: 18}
                        align: Align{x: 1.0 y: 1.0}
                        viewbox: vec4(0 0 34 34)
                        Path{d: "M 18 28 L 28 18" fill: false stroke: #xEAD8B8AA stroke_width: 1.5 stroke_linecap: "round"}
                        Path{d: "M 12 28 L 28 12" fill: false stroke: #xF3E3C788 stroke_width: 1.2 stroke_linecap: "round"}
                        Path{d: "M 24 28 L 28 24" fill: false stroke: #x9F7E4BAA stroke_width: 1.5 stroke_linecap: "round"}
                    }
                }
            }
        }
    }
}

// Global chat state accessible to ChatList widget
pub static CHAT_DATA: std::sync::RwLock<ChatData> = std::sync::RwLock::new(ChatData {
    messages: Vec::new(),
    streaming_text: String::new(),
    thinking_text: String::new(),
    is_streaming: false,
});

const DEFAULT_GLASS_OPACITY: f64 = 0.90;
const MIN_GLASS_OPACITY: f64 = 0.72;
const MAX_GLASS_OPACITY: f64 = 0.98;

#[derive(Debug, Clone, Copy, PartialEq)]
struct GlassOpacity {
    app: f32,
    sidebar: f32,
    main: f32,
    composer: f32,
}

fn glass_opacity_values(opacity: f64) -> GlassOpacity {
    let opacity = opacity.clamp(MIN_GLASS_OPACITY, MAX_GLASS_OPACITY);
    GlassOpacity {
        app: opacity as f32,
        sidebar: (opacity + 0.06).min(0.98) as f32,
        main: (opacity + 0.04).min(0.97) as f32,
        composer: (opacity + 0.03).min(0.98) as f32,
    }
}

/// Some LLMs, when asked "show me a markdown file demo with ... inside",
/// wrap their ENTIRE reply in a single ```markdown ... ``` fence. Because
/// CommonMark does not support fence nesting, pulldown-cmark then treats
/// the whole reply as ONE code block — collapsing markdown structure
/// (headings, lists, inner fences, math, …) into monospace text and killing
/// the streaming fade animation.
///
/// Strategy is aggressive: as soon as the text starts with ```markdown\n (or
/// ```md\n), strip that opener even if the outer fence hasn't closed yet.
/// Otherwise we'd keep the whole streaming reply stuck in code-block mode
/// until the final token arrives. The trailing outer fence is stripped too
/// when present.
fn unwrap_outer_markdown_fence(text: &str) -> &str {
    let trimmed_text = text.trim_start();
    // CommonMark allows fences of any length ≥ 3 — 3 backticks for plain
    // code, 4+ for wrappers that want to contain inner 3-backtick blocks.
    // LLMs use both; handle any length.
    let bt_count = trimmed_text.bytes().take_while(|b| *b == b'`').count();
    if bt_count < 3 {
        return text;
    }
    let after_ticks = &trimmed_text[bt_count..];
    let body_start = after_ticks
        .strip_prefix("markdown\n")
        .or_else(|| after_ticks.strip_prefix("md\n"));
    let Some(body) = body_start else {
        return text;
    };
    // Try to strip a matching closing fence at the end: same length or
    // longer, optionally followed by trailing whitespace. If streaming is
    // mid-way and there's no close yet, return the opener-stripped body.
    let close_pat = "`".repeat(bt_count);
    let end_trimmed = body.trim_end();
    if let Some(without_close) = end_trimmed.strip_suffix(&close_pat) {
        return without_close.trim_end_matches('\n').trim_end();
    }
    body
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiagramFenceStatus {
    None,
    Valid,
    UnclosedNonDiagram,
    Invalid,
}

struct OpenReplyFence {
    count: usize,
    fence_char: char,
    info: String,
    body_start: usize,
}

fn scan_diagram_fence_status(text: &str) -> DiagramFenceStatus {
    let text = unwrap_outer_markdown_fence(text);
    let mut status = DiagramFenceStatus::None;
    let mut open: Option<OpenReplyFence> = None;
    let mut line_start = 0;
    let bytes = text.as_bytes();
    let mut i = 0;

    while i <= bytes.len() {
        let at_end = i == bytes.len();
        let is_newline = !at_end && bytes[i] == b'\n';
        if at_end || is_newline {
            let line = text.get(line_start..i).unwrap_or("");
            let next_line_start = if is_newline { i + 1 } else { i };

            match &open {
                Some(fence) => {
                    if reply_fence_closes(line, fence) {
                        if fence.info.eq_ignore_ascii_case("diagram") {
                            let body = text.get(fence.body_start..line_start).unwrap_or("");
                            if crate::makepad_diagram_kit::parse(body.trim()).is_err() {
                                return DiagramFenceStatus::Invalid;
                            }
                            status = DiagramFenceStatus::Valid;
                        }
                        open = None;
                    }
                }
                None => {
                    if let Some((count, fence_char, info)) = reply_fence_opens(line) {
                        open = Some(OpenReplyFence {
                            count,
                            fence_char,
                            info,
                            body_start: next_line_start,
                        });
                    }
                }
            }

            line_start = next_line_start;
            i += 1;
        } else {
            i += 1;
        }
    }

    match open {
        Some(fence) if fence.info.eq_ignore_ascii_case("diagram") => DiagramFenceStatus::Invalid,
        Some(_) => DiagramFenceStatus::UnclosedNonDiagram,
        None => status,
    }
}

fn reply_fence_opens(line: &str) -> Option<(usize, char, String)> {
    let trimmed = line.trim_start().trim_end_matches('\r');
    let first = trimmed.chars().next()?;
    if first != '`' && first != '~' {
        return None;
    }

    let count = trimmed.chars().take_while(|ch| *ch == first).count();
    if count < 3 {
        return None;
    }

    let info = trimmed[count..]
        .trim()
        .split_ascii_whitespace()
        .next()
        .unwrap_or("")
        .to_string();
    Some((count, first, info))
}

fn reply_fence_closes(line: &str, fence: &OpenReplyFence) -> bool {
    let trimmed = line.trim_start().trim_end_matches('\r');
    let count = trimmed
        .chars()
        .take_while(|ch| *ch == fence.fence_char)
        .count();
    count >= fence.count && trimmed[count..].trim().is_empty()
}

fn assistant_message_is_safe_to_store(text: &str) -> bool {
    scan_diagram_fence_status(text) != DiagramFenceStatus::Invalid
}

fn assistant_message_is_safe_for_history(text: &str) -> bool {
    // Stateless replay: only re-inject diagram-free messages. A rendered or
    // malformed diagram in history can confuse the next turn, so keep those
    // out of stateless replay even if they are safe to display.
    scan_diagram_fence_status(text) == DiagramFenceStatus::None
}

const CHAT_SAVE_PATH: &str = "aichat_history.json";
const MAX_STATELESS_HISTORY_MESSAGES: usize = 12;

fn stateless_history_messages(messages: &[ChatMessage]) -> Vec<Message> {
    let mut history = Vec::new();
    let mut index = 0;

    while index + 1 < messages.len() {
        let user = &messages[index];
        let assistant = &messages[index + 1];
        if user.role == ChatRole::User
            && assistant.role == ChatRole::Assistant
            && assistant_message_is_safe_for_history(&assistant.text)
        {
            history.push(Message::user(&user.text));
            history.push(Message::assistant(&assistant.text));
            index += 2;
        } else {
            index += 1;
        }
    }

    let start = history
        .len()
        .saturating_sub(MAX_STATELESS_HISTORY_MESSAGES);
    history.split_off(start)
}

#[derive(SerJson, DeJson)]
struct SavedMessage {
    role: String,
    content: String,
}

#[derive(SerJson, DeJson, Default)]
struct SavedHistory {
    messages: Vec<SavedMessage>,
}

#[derive(Clone)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub text: String,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ChatRole {
    User,
    Assistant,
}

pub struct ChatData {
    pub messages: Vec<ChatMessage>,
    pub streaming_text: String,
    pub thinking_text: String,
    pub is_streaming: bool,
}

impl ChatData {
    pub fn save_to_disk(&self) {
        let saved = SavedHistory {
            messages: self
                .messages
                .iter()
                .map(|m| SavedMessage {
                    role: match m.role {
                        ChatRole::User => "user".to_string(),
                        ChatRole::Assistant => "assistant".to_string(),
                    },
                    content: m.text.clone(),
                })
                .collect(),
        };
        let _ = std::fs::write(CHAT_SAVE_PATH, saved.serialize_json());
    }

    pub fn load_from_disk() -> Vec<ChatMessage> {
        std::fs::read_to_string(CHAT_SAVE_PATH)
            .ok()
            .and_then(|s| SavedHistory::deserialize_json(&s).ok())
            .map(|saved| {
                saved
                    .messages
                    .into_iter()
                    .map(|m| ChatMessage {
                        role: if m.role == "user" {
                            ChatRole::User
                        } else {
                            ChatRole::Assistant
                        },
                        text: m.content,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

// ChatList widget wrapping PortalList for chat message display.
#[derive(Script, ScriptHook, Widget)]
pub struct ChatList {
    #[deref]
    view: View,
    #[rust]
    animating_msg: Option<usize>,
}

impl Widget for ChatList {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let data = CHAT_DATA.read().unwrap();

        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.as_portal_list().borrow_mut() {
                let msg_count = data.messages.len();
                let items_len = msg_count + data.is_streaming as usize;
                list.set_item_range(cx, 0, items_len);

                while let Some(item_id) = list.next_visible_item(cx) {
                    if data.is_streaming && item_id == msg_count {
                        let just_started = self.animating_msg != Some(item_id);
                        if just_started {
                            self.animating_msg = Some(item_id);
                        }

                        let (item_widget, _) = list.item_with_existed(cx, item_id, id!(Assistant));
                        let streaming_body;
                        let text: &str = if data.streaming_text.is_empty() {
                            if data.thinking_text.is_empty() {
                                "..."
                            } else {
                                "Thinking..."
                            }
                        } else {
                            let opts = SanitizeOptions {
                                trim_unclosed_fence: false,
                                ..SanitizeOptions::default()
                            };
                            // Remend keeps fenced blocks, tables and math
                            // self-consistent mid-stream so the Markdown
                            // widget doesn't re-layout a half-closed block
                            // on every token.
                            streaming_body = streaming_display_with_latex_autowrap_remend(
                                &data.streaming_text,
                                opts,
                            );
                            &streaming_body
                        };
                        let mut markdown = item_widget.markdown(cx, ids!(selectable));
                        // Unwrap outer ```markdown wrapper in streaming
                        // content: some LLMs emit the wrapper as the very
                        // first tokens, so we'd otherwise render a growing
                        // code block for the whole stream.
                        markdown.set_text(cx, unwrap_outer_markdown_fence(text));
                        if just_started {
                            markdown.reset_all_streaming_animations();
                        } else {
                            markdown.start_streaming_animation();
                        }
                        item_widget.draw_all_unscoped(cx);
                        continue;
                    }

                    if let Some(msg) = data.messages.get(item_id) {
                        let is_animating = self.animating_msg == Some(item_id);
                        let template = match msg.role {
                            ChatRole::User => id!(User),
                            ChatRole::Assistant => id!(Assistant),
                        };
                        let item_widget = list.item(cx, item_id, template);
                        let mut markdown = item_widget.markdown(cx, ids!(selectable));
                        // wrap_bare_latex wraps `\cmd{…}` with `$…$` so
                        // MathView can render them.
                        let unwrapped = unwrap_outer_markdown_fence(&msg.text);
                        let rendered = wrap_bare_latex(unwrapped);
                        markdown.set_text(cx, &rendered);
                        if is_animating {
                            markdown.stop_streaming_animation();
                        }
                        item_widget.draw_all_unscoped(cx);
                        if is_animating && markdown.is_streaming_animation_done() {
                            self.animating_msg = None;
                        }
                    }
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);

        if let Event::Actions(actions) = event {
            let list = self.view.portal_list(cx, ids!(list));
            if list.any_items_with_actions(actions) {
                for (item_id, item) in list.items_with_actions(actions) {
                    let copy_btn = item.button(cx, ids!(copy_button));
                    if copy_btn.clicked(actions) {
                        let data = CHAT_DATA.read().unwrap();
                        if let Some(msg) = data.messages.get(item_id) {
                            cx.copy_to_clipboard(&msg.text);
                        }
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BackendType {
    ClaudeCode,
    ClaudeSplash,
    ClaudeAcp,
    ClaudeApi,
    Gemini,
    GeminiSplash,
    OpenAi,
    Moonshot,
}

const ALL_BACKENDS: [BackendType; 8] = [
    BackendType::ClaudeCode,
    BackendType::ClaudeSplash,
    BackendType::ClaudeAcp,
    BackendType::ClaudeApi,
    BackendType::Gemini,
    BackendType::GeminiSplash,
    BackendType::OpenAi,
    BackendType::Moonshot,
];

impl BackendType {
    fn to_index(self) -> usize {
        ALL_BACKENDS.iter().position(|&b| b == self).unwrap()
    }

    fn from_index(index: usize) -> Option<Self> {
        ALL_BACKENDS.get(index).copied()
    }

    fn status_label(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Active: Claude Code",
            Self::ClaudeSplash => "Active: Claude Splash (UI Agent via ACP)",
            Self::ClaudeAcp => "Active: Claude (ACP via Zed)",
            Self::ClaudeApi => "Active: Claude (API)",
            Self::Gemini => "Active: Gemini",
            Self::GeminiSplash => "Active: Gemini Splash (UI Agent)",
            Self::OpenAi => "Active: OpenAI",
            Self::Moonshot => "Active: Moonshot",
        }
    }

    fn system_prompt(self) -> String {
        match self {
            Self::ClaudeSplash | Self::GeminiSplash => {
                let splash_md_path =
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../splash.md");
                let splash_md = std::fs::read_to_string(&splash_md_path)
                    .unwrap_or_else(|_| include_str!("../../../splash.md").to_string());
                format!(
                    r#"You are an AI agent that can create on-demand UI using Makepad's Splash scripting language.

You can answer questions normally using markdown. But when it makes sense to show something visually — a layout, a UI mockup, a styled card, a button arrangement, an animation, or anything graphical — you should embed a ```runsplash code block in your markdown response. The content inside a ```runsplash block is live Splash script that will be rendered as real interactive UI inline in the chat.

IMPORTANT: `use mod.prelude.widgets.*` is automatically prepended to every runsplash block — do NOT include it yourself. All widget names (View, Label, Button, etc.) are already in scope.

The block content is Splash script. It gets evaluated and rendered as a live widget tree. Do NOT wrap it in Root{{}} or Window{{}} — the content is placed directly inside a container.

Here is the complete Splash scripting manual. Follow it exactly:

{splash_md}"#
                )
            }
            _ => r#"You are a helpful assistant. Be concise but thorough.

Formatting: respond in GitHub-flavoured Markdown. Wrap every mathematical expression in LaTeX delimiters — `$…$` for inline math (e.g. `$a^2 + b^2 = c^2$`) and `$$…$$` on their own lines for display math. NEVER write LaTeX commands (`\frac`, `\mathbb`, `\sum`, `\forall`, etc.) outside these delimiters, even when the math is short.

## Diagrams — use ```diagram fenced JSON for visual structure

When a user's question benefits from a visual diagram — hierarchy, layered stack, decision flow, state machine, data model, timeline, process lanes, set overlap, request/response interaction, 2-axis comparison, or tree of concepts — emit a fenced code block with the language tag `diagram` whose body is JSON matching the diagram-kit v1 spec. Do not narrate the JSON; just render it.

The thirteen supported types are `pyramid`, `quadrant`, `tree`, `layers`, `flowchart`, `architecture`, `sequence`, `state`, `er`, `timeline`, `swimlane`, `nested`, and `venn`. Use the specific type the user asks for; do NOT downgrade `state` to `flowchart`, `er` to `architecture`, `timeline` to `sequence`, or `swimlane` to `flowchart`.

Shared optional text fields where applicable: `tag` (short uppercase, like `ROOT` `CAT` `EXT` — appears as a small pill in the top-left) and `sublabel` (mono-font secondary line below the label). Top-level `accent_idx` (integer, 0-based) or `accent_path` (array of indices, tree only) applies to `pyramid`, `quadrant`, `tree`, `layers`, and `flowchart` only. `architecture`, `sequence`, `state`, `er`, `swimlane`, `nested`, and `venn` use role-driven emphasis instead. `timeline` uses `role:"major"` for its emphasized milestone.

### `pyramid` — ranked layers, narrow apex at top

```diagram
{"type":"pyramid","levels":[{"label":"Vision","tag":"APEX"},{"label":"Strategy"},{"label":"Tactics","sublabel":"weekly"}],"accent_idx":0}
```

### `quadrant` — 2-axis scatter with 4 labelled quadrants

```diagram
{"type":"quadrant","axes":{"x":{"min":0,"max":10,"low_label":"LOW EFFORT","high_label":"HIGH EFFORT"},"y":{"min":0,"max":10,"low_label":"LOW IMPACT","high_label":"HIGH IMPACT"}},"points":[{"x":2,"y":9,"label":"quick win"},{"x":9,"y":9,"label":"big bet"},{"x":2,"y":2,"label":"fill-in"}]}
```

### `tree` — parent → children hierarchy, root at top

```diagram
{"type":"tree","root":{"label":"Product","tag":"ROOT","children":[{"label":"Core","tag":"CAT","children":[{"label":"Parse","tag":"SUB"},{"label":"Layout","tag":"SUB"}]},{"label":"Bindings","tag":"CAT"}]},"accent_path":[0,1]}
```

### `layers` — stacked horizontal bands, top layer first in array

```diagram
{"type":"layers","layers":[{"label":"Application","tag":"L7"},{"label":"Transport","tag":"L4","sublabel":"TCP · UDP"},{"label":"Network","tag":"L3"},{"label":"Physical","tag":"L1"}],"accent_idx":1}
```

### `flowchart` — nodes + edges, vertical decision flow

Node shapes: `"rect"` (default), `"oval"` (start/end), `"diamond"` (decision — no tag renders on diamonds). Edge `role`: `"default"` (muted black), `"primary"` (accent orange — the main path), `"external"` (link blue — external/HTTP calls). Edge `label` is optional mono caption at midpoint.

```diagram
{"type":"flowchart","nodes":[{"id":"req","label":"Receive","tag":"IN","shape":"oval"},{"id":"auth","label":"Authorized?","shape":"diamond"},{"id":"serve","label":"Serve","tag":"OUT","shape":"rect"}],"edges":[{"from":"req","to":"auth"},{"from":"auth","to":"serve","label":"yes","role":"primary"}],"accent_idx":2}
```

### `architecture` — 2D layered system diagram with role-tagged nodes

For cloud / service / data-flow diagrams where each box plays a distinct architectural role. Nodes get a `role`: `"focal"` (THE highlighted component — tint fill, accent stroke), `"backend"` (compute — white fill, ink stroke), `"store"` (database / cache — light ink fill, muted stroke), `"external"` (client / 3rd-party — faded), `"input"` (user input source), `"optional"` (sidecar / observability), `"security"` (auth / encryption). Layout is left-to-right layered; `"orientation":"tb"` makes it top-down.

Edges reuse the flowchart `role` enum: `"default"`, `"primary"`, `"external"`.

```diagram
{"type":"architecture","nodes":[{"id":"client","label":"Reader","tag":"EXT","role":"external"},{"id":"cdn","label":"Cloudflare","tag":"EDGE","role":"backend","sublabel":"Pages · cache"},{"id":"app","label":"Astro Origin","tag":"ORIG","role":"focal","sublabel":"SSR + MDX"},{"id":"mdx","label":"MDX Bundle","tag":"BUN","role":"store"},{"id":"cms","label":"Content CMS","tag":"CMS","role":"store"}],"edges":[{"from":"client","to":"cdn","label":"HTTPS","role":"external"},{"from":"cdn","to":"app","label":"SSR","role":"primary"},{"from":"app","to":"mdx","label":"READ"},{"from":"app","to":"cms","label":"QUERY"}]}
```

### `sequence` — actor lifelines with top-to-bottom messages

For request / response timelines between actors. Actors render across the top with vertical lifelines; messages render in array order from top to bottom. Actor `role` is `"default"` or `"focal"` only. Message `role` reuses the flowchart/architecture edge roles: `"default"`, `"primary"`, `"external"`. Use `kind:"return"` for response / return messages; right-to-left messages are also rendered as dashed returns.

```diagram
{"type":"sequence","actors":[{"id":"user","label":"User","tag":"CLIENT"},{"id":"api","label":"API Gateway","tag":"MW","role":"focal"},{"id":"db","label":"Database","tag":"STORE"}],"messages":[{"from":"user","to":"api","label":"POST /login","role":"primary"},{"from":"api","to":"db","label":"SELECT user"},{"from":"db","to":"api","label":"row","kind":"return"},{"from":"api","to":"user","label":"200 OK","kind":"return","role":"primary"}]}
```

### `state` — finite state machine with start/end dots

For order status, auth lifecycle, job queues, and connection states. States use `kind`: `"state"` (default), `"start"` / `"initial"`, or `"end"` / `"final"`. Use state `role:"focal"` for the one state to emphasize. Transitions use `from`, `to`, optional `label`, and edge `role`.

```diagram
{"type":"state","orientation":"lr","states":[{"id":"draft","label":"Draft","kind":"start"},{"id":"pending","label":"Pending Payment"},{"id":"paid","label":"Paid"},{"id":"done","label":"Done","kind":"end","role":"focal"}],"transitions":[{"from":"draft","to":"pending","label":"submit"},{"from":"pending","to":"paid","label":"pay","role":"primary"},{"from":"paid","to":"done","label":"complete"}]}
```

### `er` — entity relationship / data model

For database schemas and domain models. Entities have `id`, `name`, optional `role:"focal"`, and `fields`. Fields use `name`, optional `type`, and `key`: `"pk"`, `"fk"`, or omitted. Relationships use `from`, `to`, `from_cardinality`, `to_cardinality`, optional `label`, and edge `role`.

```diagram
{"type":"er","entities":[{"id":"user","name":"User","role":"focal","fields":[{"name":"id","type":"uuid","key":"pk"},{"name":"email","type":"text"}]},{"id":"order","name":"Order","fields":[{"name":"id","type":"uuid","key":"pk"},{"name":"user_id","type":"uuid","key":"fk"},{"name":"total","type":"money"}]}],"relationships":[{"from":"user","to":"order","from_cardinality":"1","to_cardinality":"N","label":"places","role":"primary"}]}
```

### `timeline` — milestones on a horizontal date axis

For release history, incident timelines, project plans, and roadmaps. Events use ISO-ish `time`, `label`, optional `sublabel`, and optional `role:"major"` for the highlighted milestone. Add optional top-level `axis_label`.

```diagram
{"type":"timeline","axis_label":"2026 release","events":[{"time":"2026-01-10","label":"Kickoff"},{"time":"2026-02-20","label":"Beta","role":"major"},{"time":"2026-04-01","label":"Launch","sublabel":"public"}]}
```

### `swimlane` — cross-functional process lanes

For multi-team handoffs and ownership flows. Lanes have `id` and `label`; steps have `id`, `lane`, `label`, optional `sublabel`, and optional `role:"focal"`. Edges connect steps with optional `label` and edge `role`.

```diagram
{"type":"swimlane","lanes":[{"id":"pm","label":"Product"},{"id":"eng","label":"Engineering"},{"id":"ops","label":"Operations"}],"steps":[{"id":"brief","lane":"pm","label":"Write brief"},{"id":"build","lane":"eng","label":"Build","role":"focal"},{"id":"deploy","lane":"ops","label":"Deploy"},{"id":"announce","lane":"pm","label":"Announce"}],"edges":[{"from":"brief","to":"build","label":"handoff","role":"primary"},{"from":"build","to":"deploy"},{"from":"deploy","to":"announce"}]}
```

### `nested` — containment rings for scope hierarchy

For repo/crate/module scope, trust zones, folder nesting, and blast-radius boundaries. Levels are ordered outer-to-inner. Use `role:"focal"` on the one innermost or important level.

```diagram
{"type":"nested","levels":[{"label":"Repo","sublabel":"workspace"},{"label":"Crate","sublabel":"makepad-diagram-kit"},{"label":"Module","role":"focal"}]}
```

### `venn` — set overlap / sweet spot

Prefer 2 or 3 sets. Sets use `id`, `label`, optional `sublabel`, optional `radius`. Intersections use `sets` array, `label`, and optional `role:"focal"`.

```diagram
{"type":"venn","sets":[{"id":"desirable","label":"Desirable"},{"id":"feasible","label":"Feasible"},{"id":"viable","label":"Viable"}],"intersections":[{"sets":["desirable","feasible","viable"],"label":"Product","role":"focal"}]}
```

### Rules

- Editorial density: target 4 to 10 primary elements per diagram; if you need more, split into two diagrams.
- One accent max per diagram. For role-driven diagrams, use at most one `"focal"` node / actor / state / entity / step / level / intersection; otherwise omit emphasis.
- Labels stay short (≤ 2-3 words). Put detail in `sublabel`, not in the main label.
- Keep the JSON body under 200 KB; the parser rejects larger.
- The fence body must be strictly JSON — no comments, no trailing commas.

## Fence nesting — use 4+ backticks for OUTER wrappers

CommonMark closes a 3-backtick fence at the next 3-backtick sequence — there is no nesting of same-length fences. If you want to show MARKDOWN SOURCE that itself contains fenced blocks, wrap the outer demo in at least **four backticks**."#.to_string(),
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    agent: Option<Box<dyn Agent>>,
    #[rust]
    session_id: Option<SessionId>,
    #[rust]
    current_prompt: Option<PromptId>,
    #[rust]
    available_backends: Vec<BackendType>,
    #[rust]
    active_backend: Option<BackendType>,
    #[rust]
    history_injected: bool,
}

impl App {
    fn default_backend(available_backends: &[BackendType]) -> Option<BackendType> {
        if available_backends.contains(&BackendType::Moonshot) {
            Some(BackendType::Moonshot)
        } else if available_backends.contains(&BackendType::ClaudeSplash) {
            Some(BackendType::ClaudeSplash)
        } else if available_backends.contains(&BackendType::GeminiSplash) {
            Some(BackendType::GeminiSplash)
        } else {
            available_backends.first().copied()
        }
    }

    fn detect_available_backends() -> Vec<BackendType> {
        let mut available_backends = vec![];
        if ClaudeCodeCliAgent::is_available() {
            available_backends.push(BackendType::ClaudeCode);
        }
        if ClaudeAcpAgent::is_available() {
            available_backends.push(BackendType::ClaudeSplash);
            available_backends.push(BackendType::ClaudeAcp);
        }
        if Self::read_key_file("ANTHROPIC_API_KEY").is_some() {
            available_backends.push(BackendType::ClaudeApi);
        }
        if Self::read_key_file("GOOGLE_API_KEY").is_some() {
            available_backends.push(BackendType::Gemini);
            available_backends.push(BackendType::GeminiSplash);
        }
        if Self::read_key_file("OPENAI_API_KEY").is_some() {
            available_backends.push(BackendType::OpenAi);
        }
        if Self::read_key("MOONSHOT_API_KEY").is_some() {
            available_backends.push(BackendType::Moonshot);
        }
        available_backends
    }

    fn read_key_file(path: &str) -> Option<String> {
        std::fs::read_to_string(path)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// Read a key by trying env var first, then a file of the same name in CWD.
    fn read_key(name: &str) -> Option<String> {
        std::env::var(name)
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim().to_string())
            .or_else(|| Self::read_key_file(name))
    }

    fn create_agent(&self, backend: BackendType) -> Option<Box<dyn Agent>> {
        match backend {
            BackendType::ClaudeCode => ClaudeCodeCliAgent::is_available()
                .then(|| Box::new(ClaudeCodeCliAgent::new()) as Box<dyn Agent>),
            BackendType::ClaudeSplash | BackendType::ClaudeAcp => ClaudeAcpAgent::is_available()
                .then(|| Box::new(ClaudeAcpAgent::new()) as Box<dyn Agent>),
            BackendType::ClaudeApi => Self::read_key_file("ANTHROPIC_API_KEY").map(|key| {
                Box::new(StatelessBackendAdapter::new(Box::new(ClaudeBackend::new(
                    BackendConfig::Claude {
                        api_key: Some(key),
                        oauth_token: None,
                        model: "claude-sonnet-4-5-20250929".to_string(),
                    },
                )))) as Box<dyn Agent>
            }),
            BackendType::Gemini | BackendType::GeminiSplash => {
                Self::read_key_file("GOOGLE_API_KEY").map(|key| {
                    Box::new(StatelessBackendAdapter::new(Box::new(GeminiBackend::new(
                        BackendConfig::Gemini {
                            api_key: key,
                            model: "gemini-3-pro-preview".to_string(),
                        },
                    )))) as Box<dyn Agent>
                })
            }
            BackendType::OpenAi => Self::read_key_file("OPENAI_API_KEY").map(|key| {
                Box::new(StatelessBackendAdapter::new(Box::new(OpenAiBackend::new(
                    BackendConfig::OpenAI {
                        api_key: key,
                        model: "gpt-4o".to_string(),
                        base_url: None,
                        reasoning_effort: None,
                        thinking: None,
                    },
                )))) as Box<dyn Agent>
            }),
            BackendType::Moonshot => Self::read_key("MOONSHOT_API_KEY").map(|key| {
                let model =
                    std::env::var("MOONSHOT_MODEL").unwrap_or_else(|_| "kimi-k2.6".to_string());
                let base_url = std::env::var("MOONSHOT_BASE_URL")
                    .unwrap_or_else(|_| "https://api.moonshot.ai/v1/chat/completions".to_string());
                let thinking = std::env::var("MOONSHOT_THINKING")
                    .ok()
                    .filter(|mode| matches!(mode.as_str(), "enabled" | "disabled"))
                    .unwrap_or_else(|| "enabled".to_string());
                Box::new(StatelessBackendAdapter::new(Box::new(OpenAiBackend::new(
                    BackendConfig::OpenAI {
                        api_key: key,
                        model,
                        base_url: Some(base_url),
                        reasoning_effort: None,
                        thinking: Some(thinking),
                    },
                )))) as Box<dyn Agent>
            }),
        }
    }

    fn switch_backend(&mut self, cx: &mut Cx, backend: BackendType) {
        if self.active_backend == Some(backend) {
            return;
        }
        if let Some(agent) = self.create_agent(backend) {
            self.agent = Some(agent);
            self.active_backend = Some(backend);
            self.session_id = None;
            self.current_prompt = None;
            self.history_injected = false;

            let config = SessionConfig {
                system_prompt: Some(backend.system_prompt()),
                ..Default::default()
            };
            if let Some(agent) = &mut self.agent {
                self.session_id = Some(agent.create_session(cx, config));
            }
            self.update_status(cx);
        }
    }

    fn clear_chat(&mut self, cx: &mut Cx) {
        {
            let mut data = CHAT_DATA.write().unwrap();
            data.messages.clear();
            data.streaming_text.clear();
            data.thinking_text.clear();
            data.is_streaming = false;
            data.save_to_disk();
        }
        self.history_injected = false;

        if let Some(agent) = &mut self.agent {
            let backend = self.active_backend.unwrap_or(BackendType::Gemini);
            let config = SessionConfig {
                system_prompt: Some(backend.system_prompt()),
                ..Default::default()
            };
            self.session_id = Some(agent.create_session(cx, config));
        }
        self.update_empty_state_visibility(cx);
        self.ui.redraw(cx);
    }

    fn update_empty_state_visibility(&self, cx: &mut Cx) {
        let show_empty_state = {
            let data = CHAT_DATA.read().unwrap();
            data.messages.is_empty() && !data.is_streaming
        };
        self.ui
            .view(cx, ids!(empty_state))
            .set_visible(cx, show_empty_state);
    }

    fn send_message(&mut self, cx: &mut Cx) {
        let input = self.ui.text_input(cx, ids!(input));
        let text = input.text();
        if text.trim().is_empty() {
            return;
        }

        if self.agent.is_none() || self.session_id.is_none() {
            return;
        }

        let items_len = {
            let mut data = CHAT_DATA.write().unwrap();
            data.messages.push(ChatMessage {
                role: ChatRole::User,
                text: text.clone(),
            });
            data.streaming_text.clear();
            data.thinking_text.clear();
            data.is_streaming = true;
            data.messages.len() + 1
        };
        input.set_text(cx, "");
        self.update_empty_state_visibility(cx);

        let session_id = self.session_id.unwrap();
        let agent = self.agent.as_mut().unwrap();

        // Inject history on first prompt for stateless backends
        if !self.history_injected && agent.is_stateless() {
            let data = CHAT_DATA.read().unwrap();
            let history = stateless_history_messages(&data.messages[..data.messages.len() - 1]);
            drop(data);
            if !history.is_empty() {
                agent.inject_history(session_id, history);
            }
            self.history_injected = true;
        }

        // ACP doesn't support system prompts via the protocol, so for ClaudeSplash
        // we prepend the splash system prompt context to each user message.
        let prompt_text = if self.active_backend == Some(BackendType::ClaudeSplash) {
            let system = BackendType::ClaudeSplash.system_prompt();
            format!("<system>\n{system}\n</system>\n\n{text}")
        } else {
            text
        };
        self.current_prompt = Some(agent.send_prompt(cx, session_id, &prompt_text));
        self.ui.view(cx, ids!(cancel_button)).set_visible(cx, true);

        let chat_list = self.ui.widget(cx, ids!(chat_list));
        let list = chat_list.portal_list(cx, ids!(list));
        list.set_tail_range(true);
        list.set_first_id_and_scroll(items_len.saturating_sub(1), 0.0);
        self.ui.redraw(cx);
    }

    fn cancel_request(&mut self, cx: &mut Cx) {
        if let (Some(agent), Some(prompt_id)) = (&mut self.agent, self.current_prompt.take()) {
            agent.cancel_prompt(cx, prompt_id);

            let mut data = CHAT_DATA.write().unwrap();
            let text = std::mem::take(&mut data.streaming_text);
            data.thinking_text.clear();
            if !text.is_empty() {
                data.messages.push(ChatMessage {
                    role: ChatRole::Assistant,
                    text,
                });
            }
            data.is_streaming = false;
            drop(data);

            self.update_empty_state_visibility(cx);
            self.ui.view(cx, ids!(cancel_button)).set_visible(cx, false);
            self.ui.redraw(cx);
        }
    }

    fn update_status(&self, cx: &mut Cx) {
        let status = match self.active_backend {
            Some(b) => b.status_label(),
            None => "No backend selected",
        };
        self.ui.label(cx, ids!(status_label)).set_text(cx, status);
    }

    fn apply_glass_opacity(&self, cx: &mut Cx, opacity: f64) {
        let opacity = opacity.clamp(MIN_GLASS_OPACITY, MAX_GLASS_OPACITY);
        let glass = glass_opacity_values(opacity);

        let mut app_shell = self.ui.view(cx, ids!(app_shell));
        script_apply_eval!(cx, app_shell, {
            draw_bg +: { tint_alpha: #(glass.app) }
        });

        let mut sidebar = self.ui.view(cx, ids!(sidebar));
        script_apply_eval!(cx, sidebar, {
            draw_bg +: { tint_alpha: #(glass.sidebar) }
        });

        let mut main_area = self.ui.view(cx, ids!(main_area));
        script_apply_eval!(cx, main_area, {
            draw_bg +: { tint_alpha: #(glass.main) }
        });

        let mut composer = self.ui.view(cx, ids!(composer));
        script_apply_eval!(cx, composer, {
            draw_bg +: { tint_alpha: #(glass.composer) }
        });

        self.ui
            .label(cx, ids!(opacity_value))
            .set_text(cx, &format!("{:.0}%", opacity * 100.0));
        self.ui.redraw(cx);
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if let Some(opacity) = self.ui.slider(cx, ids!(opacity_slider)).slided(actions) {
            self.apply_glass_opacity(cx, opacity);
        }

        // Markdown link click — dispatch through robius-open for cross-platform
        // coverage (macOS/Linux/Windows/iOS/Android/WASM). Desktop requires a
        // modifier (Cmd on macOS, Cmd/Ctrl elsewhere) so plain clicks stay
        // available for drag-selection inside the Markdown widget; mobile &
        // web have no modifier concept, so a plain tap opens the URL.
        for action in actions {
            if let Some(widget_action) = action.as_widget_action() {
                if let makepad_widgets::markdown::MarkdownAction::LinkNavigated { url, modifiers } =
                    widget_action.cast()
                {
                    let should_open = {
                        #[cfg(any(
                            target_os = "ios",
                            target_os = "android",
                            target_arch = "wasm32"
                        ))]
                        {
                            let _ = modifiers;
                            true
                        }
                        #[cfg(not(any(
                            target_os = "ios",
                            target_os = "android",
                            target_arch = "wasm32"
                        )))]
                        {
                            modifiers.logo || modifiers.control
                        }
                    };
                    if should_open {
                        if let Err(e) = robius_open::Uri::new(&url).open() {
                            log::warn!("failed to open URL {}: {:?}", url, e);
                        }
                    }
                }
            }
        }
        if self.ui.button(cx, ids!(send_button)).clicked(actions) {
            self.send_message(cx);
        }
        if self.ui.button(cx, ids!(cancel_button)).clicked(actions) {
            self.cancel_request(cx);
        }
        if self.ui.button(cx, ids!(clear_button)).clicked(actions) {
            self.clear_chat(cx);
        }
        if self
            .ui
            .text_input(cx, ids!(input))
            .returned(actions)
            .is_some()
        {
            self.send_message(cx);
        }
        if self.ui.text_input(cx, ids!(input)).escaped(actions) {
            self.cancel_request(cx);
        }
        if let Some(index) = self
            .ui
            .drop_down(cx, ids!(backend_dropdown))
            .selected(actions)
        {
            if let Some(backend) = BackendType::from_index(index) {
                self.switch_backend(cx, backend);
            }
        }

        // Handle message deletion
        let chat_list = self.ui.widget(cx, ids!(chat_list));
        let list = chat_list.portal_list(cx, ids!(list));
        for (item_id, item) in list.items_with_actions(actions) {
            if item.button(cx, ids!(delete_button)).pressed(actions) {
                let mut data = CHAT_DATA.write().unwrap();
                if item_id < data.messages.len() {
                    data.messages.remove(item_id);
                    data.save_to_disk();
                }
                drop(data);
                self.ui.redraw(cx);
            }
        }
    }

    fn handle_startup(&mut self, cx: &mut Cx) {
        let default_backend = Self::default_backend(&self.available_backends);
        if let Some(backend) = default_backend {
            self.switch_backend(cx, backend);
            self.ui
                .drop_down(cx, ids!(backend_dropdown))
                .set_selected_item(cx, backend.to_index());
        }
        self.update_status(cx);
        self.update_empty_state_visibility(cx);
        self.ui
            .slider(cx, ids!(opacity_slider))
            .set_value(cx, DEFAULT_GLASS_OPACITY);
        self.apply_glass_opacity(cx, DEFAULT_GLASS_OPACITY);
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        crate::makepad_code_editor::script_mod(vm);
        crate::makepad_diagram_kit::script_mod(vm);
        self::script_mod(vm)
    }

    fn after_new_from_script(_vm: &mut ScriptVm, app: &mut Self) {
        CHAT_DATA.write().unwrap().messages = ChatData::load_from_disk();
        app.available_backends = Self::detect_available_backends();
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::WindowDragQuery(dq) = event {
            if Some(dq.window_id) == self.ui.window(cx, ids!(main_window)).window_id() {
                let size = self.ui.window(cx, ids!(main_window)).get_inner_size(cx);
                let top_drag_strip = dq.abs.y < 52.0 && dq.abs.x < size.x - 260.0;
                if top_drag_strip {
                    dq.response.set(WindowDragQueryResponse::Caption);
                    cx.set_cursor(MouseCursor::Default);
                }
            }
        }

        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());

        if let Some(agent) = &mut self.agent {
            for event in agent.handle_event(cx, event) {
                match event {
                    AgentEvent::SessionReady { .. } => {
                        self.update_status(cx);
                    }
                    AgentEvent::SessionError { error, .. } => {
                        self.ui
                            .label(cx, ids!(status_label))
                            .set_text(cx, &format!("Error: {}", error));
                    }
                    AgentEvent::TextDelta { text, .. } => {
                        log!("aichat UI text delta chars={}", text.chars().count());
                        let item_id = {
                            let mut data = CHAT_DATA.write().unwrap();
                            data.streaming_text.push_str(&text);
                            data.messages.len()
                        };
                        let chat_list = self.ui.widget(cx, ids!(chat_list));
                        let list = chat_list.portal_list(cx, ids!(list));
                        if let Some((_, item)) = list.get_item(item_id) {
                            item.widget(cx, ids!(splash_view)).redraw(cx);
                        }
                        cx.redraw_all();
                    }
                    AgentEvent::ThinkingDelta { text, .. } => {
                        log!("aichat UI thinking delta chars={}", text.chars().count());
                        {
                            let mut data = CHAT_DATA.write().unwrap();
                            data.thinking_text.push_str(&text);
                        }
                        self.ui
                            .label(cx, ids!(status_label))
                            .set_text(cx, "Thinking...");
                        cx.redraw_all();
                    }
                    AgentEvent::TurnComplete { .. } => {
                        let mut data = CHAT_DATA.write().unwrap();
                        let text = std::mem::take(&mut data.streaming_text);
                        log!(
                            "aichat UI turn complete content_chars={}",
                            text.chars().count()
                        );
                        data.thinking_text.clear();
                        if !text.is_empty() {
                            if assistant_message_is_safe_to_store(&text) {
                                data.messages.push(ChatMessage {
                                    role: ChatRole::Assistant,
                                    text,
                                });
                            } else {
                                self.ui.label(cx, ids!(status_label)).set_text(
                                    cx,
                                    "Error: incomplete diagram response discarded; retry",
                                );
                            }
                        }
                        data.is_streaming = false;
                        data.save_to_disk();
                        drop(data);

                        self.current_prompt = None;
                        self.ui.view(cx, ids!(cancel_button)).set_visible(cx, false);
                        self.update_empty_state_visibility(cx);
                        cx.redraw_all();
                    }
                    AgentEvent::PromptError { error, .. } => {
                        log!("aichat UI prompt error: {}", error);
                        {
                            let mut data = CHAT_DATA.write().unwrap();
                            data.messages.push(ChatMessage {
                                role: ChatRole::Assistant,
                                text: format!("Error: {error}"),
                            });
                            data.is_streaming = false;
                            data.thinking_text.clear();
                            data.save_to_disk();
                        }
                        self.current_prompt = None;
                        self.ui.view(cx, ids!(cancel_button)).set_visible(cx, false);
                        self.update_empty_state_visibility(cx);
                        self.ui
                            .label(cx, ids!(status_label))
                            .set_text(cx, &format!("Error: {}", error));
                        cx.redraw_all();
                    }
                    AgentEvent::ToolRequest { .. } => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        assistant_message_is_safe_for_history, assistant_message_is_safe_to_store,
        glass_opacity_values, Agent, App, BackendType, ClaudeCodeCliAgent, DEFAULT_GLASS_OPACITY,
        MAX_GLASS_OPACITY, MIN_GLASS_OPACITY,
    };

    #[test]
    fn aichat_glass_opacity_slider_contract() {
        assert!((DEFAULT_GLASS_OPACITY - 0.90).abs() < f64::EPSILON);
        let values = glass_opacity_values(DEFAULT_GLASS_OPACITY);
        assert!((0.85..=0.92).contains(&(values.app as f64)));
        assert!(values.sidebar >= values.app);
        assert!(values.main >= values.app);
        assert!(values.composer >= values.app);
    }

    #[test]
    fn aichat_liquid_glass_shell_contract() {
        let low = glass_opacity_values(0.0);
        let high = glass_opacity_values(2.0);
        assert!((low.app - MIN_GLASS_OPACITY as f32).abs() < f32::EPSILON);
        assert!((high.app - MAX_GLASS_OPACITY as f32).abs() < f32::EPSILON);
    }

    #[test]
    fn aichat_backend_type_includes_claude_code() {
        assert_eq!(BackendType::ClaudeCode.to_index(), 0);
        assert_eq!(BackendType::from_index(0), Some(BackendType::ClaudeCode));
        assert_eq!(
            BackendType::ClaudeCode.status_label(),
            "Active: Claude Code"
        );
        assert!(BackendType::ClaudeCode
            .system_prompt()
            .contains("The thirteen supported types"));
    }

    #[test]
    fn aichat_create_claude_code_agent() {
        let _agent: Box<dyn Agent> = Box::new(ClaudeCodeCliAgent::new());
    }

    #[test]
    fn aichat_defaults_to_moonshot_when_available() {
        let available = [
            BackendType::ClaudeCode,
            BackendType::ClaudeSplash,
            BackendType::Moonshot,
        ];
        assert_eq!(
            App::default_backend(&available),
            Some(BackendType::Moonshot)
        );
    }

    #[test]
    fn non_splash_prompt_documents_sequence_diagrams() {
        let prompt = BackendType::Moonshot.system_prompt();

        assert!(prompt.contains("### `sequence`"));
        assert!(prompt.contains(r#""type":"sequence""#));
        assert!(prompt.contains(r#"kind:"return""#));
        assert!(prompt.contains(r#""kind":"return""#));
        assert!(!prompt.contains("All five types"));
    }

    #[test]
    fn non_splash_prompt_documents_all_diagram_types() {
        let prompt = BackendType::Moonshot.system_prompt();

        assert!(prompt.contains("The thirteen supported types"));
        for ty in [
            "pyramid",
            "quadrant",
            "tree",
            "layers",
            "flowchart",
            "architecture",
            "sequence",
            "state",
            "er",
            "timeline",
            "swimlane",
            "nested",
            "venn",
        ] {
            assert!(
                prompt.contains(&format!(r#""type":"{ty}""#)),
                "prompt should document {ty}"
            );
        }
        assert!(prompt.contains(r#""kind":"end""#));
        assert!(prompt.contains(r#""key":"pk""#));
        assert!(prompt.contains(r#""role":"major""#));
        assert!(!prompt.contains("The seven supported types"));
        assert!(!prompt.contains("doesn't have a native timeline type"));
    }

    #[test]
    fn history_injection_allows_valid_diagram_assistant_messages() {
        let text = r#"```diagram
{"type":"state","orientation":"lr","states":[{"id":"draft","label":"Draft","kind":"start"},{"id":"done","label":"Done","kind":"end","role":"focal"}],"transitions":[{"from":"draft","to":"done","label":"submit"}]}
```"#;

        assert!(assistant_message_is_safe_to_store(text));
        assert!(!assistant_message_is_safe_for_history(text));
    }

    #[test]
    fn history_injection_rejects_incomplete_diagram_assistant_messages() {
        let text = r#"```diagram
{"type":"state","orientation":"lr","states":[{"id":"draft","label":"Draft","kind":"start"},{"id":"pending","label":"Pending Payment"},{"id":"paid","label":"
"#;

        assert!(!assistant_message_is_safe_to_store(text));
        assert!(!assistant_message_is_safe_for_history(text));
    }

    #[test]
    fn history_injection_rejects_invalid_closed_diagram_assistant_messages() {
        let text = r#"```diagram
{"type":"state","orientation":"lr","states":[{"id":"draft","label":"Draft"}],
```"#;

        assert!(!assistant_message_is_safe_to_store(text));
        assert!(!assistant_message_is_safe_for_history(text));
    }

    #[test]
    fn history_injection_allows_non_diagram_assistant_messages() {
        let text = "这里是普通解释，没有 diagram fence。";

        assert!(assistant_message_is_safe_to_store(text));
        assert!(assistant_message_is_safe_for_history(text));
    }

    // Regression: an unclosed *non-diagram* fence (e.g. response truncated
    // mid `rust`/`mermaid` block) was discarding the entire reply because
    // FenceScanError::Unclosed was treated the same as a malformed diagram.
    #[test]
    fn store_keeps_reply_with_unclosed_non_diagram_fence() {
        let text = "Here's a markdown demo:\n\n```rust\nfn main() {\n    println!(\"hi\";\n";
        assert!(assistant_message_is_safe_to_store(text));
        assert!(!assistant_message_is_safe_for_history(text));
    }

    #[test]
    fn store_rejects_bad_diagram_even_with_later_unclosed_non_diagram_fence() {
        let text = r#"```diagram
{"type":"state","orientation":"lr","states":[{"id":"draft","label":"Draft"}],
```

```rust
fn main() {
"#;

        assert!(!assistant_message_is_safe_to_store(text));
        assert!(!assistant_message_is_safe_for_history(text));
    }

    #[test]
    fn outer_markdown_wrapper_is_unwrapped_before_diagram_safety_scan() {
        let text = r#"```markdown
Here is a diagram:

```diagram
{"type":"state","orientation":"lr","states":[{"id":"draft","label":"Draft","kind":"start"},{"id":"done","label":"Done","kind":"end"}],"transitions":[{"from":"draft","to":"done","label":"submit"}]}
```
```"#;

        assert!(assistant_message_is_safe_to_store(text));
        assert!(!assistant_message_is_safe_for_history(text));
    }

    #[test]
    fn outer_markdown_wrapper_rejects_bad_inner_diagram() {
        let text = r#"```markdown
Here is a broken diagram:

```diagram
{"type":"state","orientation":"lr","states":[{"id":"draft","label":"Draft"}],
```
```"#;

        assert!(!assistant_message_is_safe_to_store(text));
        assert!(!assistant_message_is_safe_for_history(text));
    }
}
