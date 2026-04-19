pub use makepad_code_editor;
pub use makepad_widgets;

use makepad_ai::*;
use makepad_widgets::makepad_platform::makepad_micro_serde::*;
use makepad_widgets::*;
use streaming_markdown_kit::{
    SanitizeOptions, streaming_display_with_latex_autowrap,
    streaming_display_with_latex_autowrap_remend, wrap_bare_latex,
};

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.CodeView
    use mod.text.*
    use mod.res.*

    let MermaidSvgView = #(MermaidSvgView::register_widget(vm)) {
        width: Fill
        height: Fit
        // `theme.font_code{ ... }` is the **spread** constructor: it inherits
        // every TextStyle field from theme.font_code (line_spacing, kerning,
        // brightness, etc.) and lets us override font_family + font_size.
        //
        // DON'T write `text_style: { ... }` without the spread — that replaces
        // the entire TextStyle with a partially-initialised one and DrawText
        // silently renders nothing (observed empirically: replacing text_style
        // zeroes out the tofu that the broken font path would otherwise show).
        //
        // We still inline the CJK font stack because `let MermaidSvgView = ...`
        // is expanded before aichat's `mod.themes.dark.font_code` override
        // below — same eager-expansion problem as CodeView's font workaround
        // in the Assistant bubble.
        // Animated flow dot shader: SDF circle + halo.
        // CRITICAL: return `Pal.premul(...)` — without it, Makepad treats the
        // quad as a fully opaque rect and the SDF cut-out is ignored
        // (that's the "small square instead of a dot" symptom).
        // Per-edge color (incl. pulse alpha in `.w`) is written from Rust.
        draw_flow_dot +: {
            color: #xe2e8f0
            pixel: fn() {
                let r = length(self.pos - vec2(0.5, 0.5))
                // 0.32 solid core, 0.5 halo edge — leaves a visible circle
                // with a soft glow out to the quad corners.
                let core = 1.0 - smoothstep(0.30, 0.38, r)
                let halo = (1.0 - smoothstep(0.38, 0.50, r)) * 0.55
                let a = clamp(core + halo, 0.0, 1.0) * self.color.w
                return Pal.premul(vec4(self.color.xyz, a))
            }
        }
        draw_text +: {
            // Default slate-200 — the per-cmd colour from darkify_mermaid_svg
            // overrides this anyway; the DSL default only matters if some
            // `<text>` had no fill attribute to remap.
            color: #xe2e8f0
            text_style: theme.font_code{
                font_size: 12
                font_family: FontFamily{
                    latin := FontMember{res: crate_resource("self:resources/LiberationMono-Regular.ttf") asc: 0.0 desc: 0.0}
                    chinese := FontMember{res: crate_resource("self:resources/LXGWWenKaiMono-Regular.ttf") asc: 0.0 desc: 0.0}
                    emoji := FontMember{res: crate_resource("self:resources/NotoColorEmoji.ttf") asc: 0.0 desc: 0.0}
                }
            }
        }
    }

    // Override theme fonts. Two purposes:
    //   1. font_code — CJK-capable monospace (LXGW Mono) so `` `inline` ``
    //      and CodeView render Chinese correctly.
    //   2. font_regular/bold/italic/bold_italic — add a `symbols` fallback
    //      (NotoSans) so Unicode blocks outside IBM Plex Sans's repertoire
    //      (arrows U+2190-U+21FF, math operators, misc technical) render
    //      as glyphs instead of tofu. Observed trigger: `1→5` in prose.
    //
    // Note: Makepad's Markdown widget bakes `theme.font_*` at expansion
    // time, so these theme-level overrides are **necessary but not
    // sufficient** — per-instance overrides on each Markdown instance
    // (text_style_fixed / text_style_normal / …) are also applied below.
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
                    color: #3a5a8a
                    radius: 8.0
                }

                selectable := Markdown {
                    width: Fill
                    height: Fit
                    selectable: true
                    // BISECT (2026-04-19): temporarily false to test whether
                    // CodeView is the source of P2 long-code-block "content
                    // disappears" issue. Revert to true once bisect done.
                    use_code_block_widget: false
                    use_math_widget: true
                    body: ""
                    // Per-instance override for `` `inline code` ``. The
                    // Markdown widget bakes `theme.font_code` at expansion
                    // time, so a later `mod.themes.dark{...}` override
                    // doesn't reach it — same issue as CodeView's comment
                    // below. Without this override, CJK inside backticks
                    // renders as tofu (no glyph) because Liberation Mono
                    // is Latin-only.
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
                    code_block := View {
                        width: Fill
                        height: Fit
                        flow: Overlay
                        code_view := CodeView {
                            keep_cursor_at_end: false
                            editor +: {
                                height: Fit
                                draw_bg +: { color: #1a1a2e }
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
                    color: #2a2a3a
                    radius: 8.0
                }

                RubberView {
                    width: Fill
                    height: Fit
                    smoothing: 0.3
                    flow: Down

                    selectable := Markdown {
                        width: Fill
                        height: Fit
                        selectable: true
                        // BISECT (2026-04-19): temporarily false to test whether
                    // CodeView is the source of P2 long-code-block "content
                    // disappears" issue. Revert to true once bisect done.
                    use_code_block_widget: false
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
                        code_block := View {
                            width: Fill
                            height: Fit
                            flow: Overlay
                            code_view := CodeView {
                                keep_cursor_at_end: true
                                editor +: {
                                    height: Fit
                                    draw_bg +: { color: #1a1a2e }
                                    // Local font override: CodeView is defined in the
                                    // makepad-code-editor crate and bakes `theme.font_code`
                                    // at its own expansion time, so later `mod.themes.dark`
                                    // overrides don't reach it. We override per-instance.
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
                        // Markdown widget dispatches here when it encounters a
                        // ```mermaid fenced block (same pattern as splash_block
                        // for ```runsplash). The Markdown widget calls
                        // `mermaid_view.set_text(cx, src)` with the raw mermaid
                        // source; MermaidSvgView::set_text routes through
                        // rusty-mermaid → SVG → DrawSvg render.
                        mermaid_block := View {
                            width: Fill
                            height: Fit
                            flow: Down
                            margin: Inset{top: 4, bottom: 4}
                            mermaid_view := MermaidSvgView {
                                width: Fill
                                height: 480
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
                window.inner_size: vec2(900, 700)
                window.title: "AI Chat"
                body +: {
                    flow: Down
                    padding: Inset{left: 16 top: 16 right: 16 bottom: 16}
                    spacing: 12

                    View {
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 12
                        align: Align{y: 0.5}

                        Label {
                            text: "AI Chat"
                            draw_text.text_style.font_size: 18
                        }

                        View { width: Fill height: 1 }

                        Label {
                            text: "Backend:"
                            draw_text.text_style.font_size: 12
                        }

                        backend_dropdown := DropDown {
                            width: 170
                            labels: ["Claude Splash" "Claude (ACP)" "Claude (API)" "Gemini" "Gemini Splash" "OpenAI" "Moonshot"]
                        }
                    }

                    chat_list := ChatList {}

                    View {
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{y: 1.0}

                        input := TextInput {
                            width: Fill
                            height: Fit
                            empty_text: "Type a message... (Enter to send)"
                            // Per-instance font override — TextInput bakes
                            // `theme.font_regular` at DSL-expansion time, same
                            // issue as Markdown/CodeView. Without this the
                            // input box shows tofu for CJK and U+2192 arrows.
                            draw_text +: {
                                text_style: theme.font_regular{
                                    line_spacing: theme.font_wdgt_line_spacing
                                    font_size: theme.font_size_p
                                    font_family: FontFamily{
                                        latin := FontMember{res: crate_resource("self:resources/NotoSans-Regular.ttf") asc: 0.0 desc: 0.0}
                                        chinese := FontMember{res: crate_resource("self:resources/LXGWWenKaiMono-Regular.ttf") asc: 0.0 desc: 0.0}
                                        symbols := FontMember{res: crate_resource("self:resources/NotoSans-Regular.ttf") asc: 0.0 desc: 0.0}
                                        emoji := FontMember{res: crate_resource("self:resources/NotoColorEmoji.ttf") asc: 0.0 desc: 0.0}
                                    }
                                }
                            }
                        }

                        send_button := Button {
                            text: "Send"
                            width: 80
                        }

                        cancel_button := Button {
                            text: "Cancel"
                            width: 80
                            visible: false
                        }

                        clear_button := Button {
                            text: "Clear"
                            width: 80
                        }
                    }

                    View {
                        width: Fill
                        height: Fit

                        status_label := Label {
                            width: Fill
                            height: Fit
                            text: "Initializing..."
                            draw_text.text_style.font_size: 10
                            draw_text.color: #888
                        }
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
    is_streaming: false,
});

// Mermaid rendering is now fully handled by the Markdown widget's
// `mermaid_block` language hook (see widgets/src/markdown.rs). The source of
// each fenced `mermaid` block is routed to MermaidSvgView::set_text, which
// delegates to set_mermaid_src → rusty-mermaid → SVG → DrawSvg. No aichat-
// side extraction or cache is needed — MermaidSvgView dedupes by content
// hash internally.

/// Some LLMs, when asked "show me a markdown file demo with ... inside",
/// wrap their ENTIRE reply in a single ```markdown ... ``` fence. Because
/// CommonMark does not support fence nesting, pulldown-cmark then treats
/// the whole reply as ONE code block — collapsing markdown structure
/// (headings, lists, inner mermaid fences, math, …) into monospace text
/// and killing the streaming fade animation (CodeView's DrawText doesn't
/// carry the per-char fade shader the way our Markdown widget does).
///
/// Strategy is **aggressive**: as soon as the text starts with
/// ```markdown\n (or ```md\n), strip that opener even if the outer fence
/// hasn't closed yet. Otherwise we'd keep the whole streaming reply stuck
/// in code-block mode until the final token arrives. The trailing outer
/// ``` is stripped too when present (so `# …\n```mermaid\n…\n```\n````
/// becomes `# …\n```mermaid\n…\n````), which lets inner fenced blocks
/// render correctly while the outer wrapper is folded away.
fn unwrap_outer_markdown_fence(text: &str) -> &str {
    let trimmed_text = text.trim_start();
    // CommonMark allows fences of any length ≥ 3 — 3 backticks for plain
    // code, 4+ for wrappers that want to contain inner 3-backtick blocks.
    // LLMs use both (3 when they're not thinking about nesting, 4 when our
    // system prompt tells them to). Handle any length.
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

const CHAT_SAVE_PATH: &str = "aichat_history.json";

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

// MermaidSvgView: custom widget that renders raw SVG bytes using Makepad's
// native GPU-backed SvgDocument path (no resvg rasterisation, no PNG).
// Shapes go through DrawSvg; <text> labels go through DrawText via the
// makepad-svg `collect_text_cmds` sidecar.
use makepad_widgets::makepad_draw::svg::{
    collect_edges, collect_text_cmds, parse_svg, SvgDocument, SvgEdge, SvgTextAnchor, SvgTextCmd,
};

#[derive(Script, ScriptHook, Widget)]
pub struct MermaidSvgView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_svg: DrawSvg,
    #[live]
    draw_text: DrawText,
    /// GPU-shaded dot that rides each edge to visualise data flow. Using
    /// DrawColor (= DrawQuad + color instance) so we can set per-edge tint
    /// from Rust each frame without a custom shader struct.
    #[live]
    draw_flow_dot: DrawColor,
    #[rust]
    doc: SvgDocument,
    #[rust]
    content_w: f64,
    #[rust]
    content_h: f64,
    #[rust]
    last_src_hash: u64,
    /// Debounce: hash of the source we saw on the immediately previous call
    /// but haven't rendered yet. Rendering only fires when the SAME hash
    /// arrives twice in a row — during streaming the source changes every
    /// chunk, so we skip; once the stream pauses (fence closed or LLM
    /// catching breath), the next set_text with the same content triggers
    /// the actual mermaid layout + SVG generation. Cuts per-token cost of
    /// large diagrams from "render every chunk" to "render at most once
    /// per stream pause".
    #[rust]
    pending_src_hash: u64,
    /// Cache of text commands extracted from `self.doc`. `collect_text_cmds`
    /// walks the entire SVG tree, which is O(nodes). draw_walk runs per
    /// frame (because the flow-dot animation self-schedules NextFrame), so
    /// without this cache we walk the tree on every frame of every mermaid
    /// widget. Invalidate on `set_svg_str`.
    #[rust]
    cached_text_cmds: Vec<SvgTextCmd>,
    /// Cache of edge geometries for the flow-dot animation. Same rationale
    /// as `cached_text_cmds`.
    #[rust]
    cached_edges: Vec<SvgEdge>,
    // -- pan/zoom state --
    /// Current zoom factor. 1.0 = fit-to-widget, >1 zoomed in, <1 zoomed out.
    #[rust(1.0f64)]
    zoom: f64,
    /// Screen-space offset applied on top of rect.pos to shift the content.
    #[rust]
    pan: DVec2,
    /// Mouse-down absolute position when a drag starts.
    #[rust]
    drag_start_abs: Option<DVec2>,
    /// Pan snapshot at drag start, used to compute incremental pan.
    #[rust]
    drag_start_pan: DVec2,
    /// Cache of the last draw_walk rect so `handle_event`'s zoom-at-cursor
    /// math can reference the widget's on-screen bounds without recomputing.
    #[rust]
    last_rect: Rect,
    // -- flow-dot animation --
    /// Monotonic 0..1 time cursor; each edge's dot position is `points[
    /// (t + edge_idx * phase_offset).fract() * len]`. Wraps on each cycle.
    #[rust]
    anim_t: f32,
    /// Live request for the next frame; Makepad redelivers via
    /// `Event::NextFrame` which we match via `is_event`.
    #[rust]
    next_frame: NextFrame,
}

impl MermaidSvgView {
    /// Accept raw SVG and parse it directly. Used by tests / manual code.
    pub fn set_svg_str(&mut self, cx: &mut Cx, svg: &str) {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let mut hs = DefaultHasher::new();
        svg.hash(&mut hs);
        let h = hs.finish();
        if h == self.last_src_hash && !self.doc.root.is_empty() {
            return;
        }
        self.last_src_hash = h;
        self.doc = parse_svg(svg);
        // Rebuild per-doc caches exactly once per SVG swap. draw_walk runs
        // per frame (flow-dot animation self-schedules), so computing these
        // here instead of inside draw_walk cuts scroll/zoom cost from
        // O(nodes × frames) to O(nodes once).
        self.cached_text_cmds = collect_text_cmds(&self.doc);
        self.cached_edges = collect_edges(&self.doc);
        self.draw_svg.cache_valid = false;
        self.draw_svg.set_doc_bounds(&self.doc);
        if let Some(vb) = self.doc.viewbox.as_ref() {
            self.draw_svg.content_bounds = (vb.x, vb.y, vb.x + vb.width, vb.y + vb.height);
            self.content_w = vb.width as f64;
            self.content_h = vb.height as f64;
            self.draw_svg.content_size = dvec2(self.content_w, self.content_h);
        }
        self.redraw(cx);
    }

    /// Accept raw **mermaid source** (the body of a ```mermaid fenced block),
    /// run it through rusty-mermaid → SVG → `set_svg_str`. Called by the
    /// Markdown widget's `mermaid_block` template hook via `Widget::set_text`.
    pub fn set_mermaid_src(&mut self, cx: &mut Cx, src: &str) {
        use std::hash::{DefaultHasher, Hash, Hasher};
        // pulldown-cmark auto-closes unclosed fences at EOF, so during
        // streaming the still-open ```mermaid block may swallow our cursor
        // glyph (▋) + any trailing prose. Strip the cursor and bail if the
        // result looks too partial to parse. This keeps us from hammering
        // rusty-mermaid with garbage mid-stream.
        let cleaned: String = src.chars().filter(|c| *c != '▋').collect();
        let trimmed = cleaned.trim();
        if trimmed.is_empty() || trimmed.len() < 8 {
            return;
        }
        let mut hs = DefaultHasher::new();
        trimmed.hash(&mut hs);
        let h = hs.finish();

        // Already rendered this exact src — nothing to do.
        if h == self.last_src_hash && !self.doc.root.is_empty() {
            return;
        }

        // Streaming debounce: only render when the SAME new hash arrives
        // twice in a row. During an active stream the mermaid body grows
        // every chunk, so each call has a fresh hash and we skip. When the
        // stream pauses (fence closed, LLM paused, chunk coalesced) the
        // next identical call triggers the render. Big diagrams (15+
        // nodes) go from "dagre layout every chunk" to "render once per
        // stream pause".
        if h != self.pending_src_hash {
            self.pending_src_hash = h;
            return;
        }

        match streaming_markdown_kit::render_mermaid_to_svg(trimmed) {
            Ok(svg) => {
                self.set_svg_str(cx, &svg);
                self.last_src_hash = h;
            }
            Err(_) => {
                // Silent while streaming — failures here are expected for
                // mid-stream fragments. If the fence closes with a clean
                // final source, a subsequent call will succeed.
            }
        }
    }
}

impl Widget for MermaidSvgView {
    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        // The Markdown widget routes `mermaid_view.set_text(cx, src)` here
        // when it closes a ```mermaid fenced block. We interpret the value
        // as raw mermaid source and render it in-place.
        self.set_mermaid_src(cx, v);
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        // Frame tick for the flow-dot animation. `0.003` ≈ ~5 s for a full
        // cycle on a 60 Hz display, which matches the pace of "data
        // flowing" indicators in dashboards without feeling jittery.
        if self.next_frame.is_event(event).is_some() {
            self.anim_t = (self.anim_t + 0.003).rem_euclid(1.0);
            self.next_frame = cx.new_next_frame();
            self.redraw(cx);
        }
        match event.hits_with_capture_overload(cx, self.draw_svg.area(), true) {
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                // Double-click resets zoom+pan — the "I got lost, take me
                // home" gesture. Matches Preview.app / macOS convention.
                if fe.tap_count >= 2 {
                    self.zoom = 1.0;
                    self.pan = DVec2::default();
                    self.drag_start_abs = None;
                    self.redraw(cx);
                } else {
                    self.drag_start_abs = Some(fe.abs);
                    self.drag_start_pan = self.pan;
                    cx.set_cursor(MouseCursor::Grabbing);
                }
            }
            Hit::FingerMove(fe) => {
                if let Some(start) = self.drag_start_abs {
                    self.pan = self.drag_start_pan + (fe.abs - start);
                    self.redraw(cx);
                }
            }
            Hit::FingerUp(_) => {
                if self.drag_start_abs.is_some() {
                    self.drag_start_abs = None;
                    cx.set_cursor(MouseCursor::Grab);
                }
            }
            Hit::FingerHoverIn(_) => cx.set_cursor(MouseCursor::Grab),
            Hit::FingerScroll(fs) => {
                // Modifier-gated zoom: plain 2-finger scroll falls through
                // to the parent (so the chat list can scroll past this
                // widget). Only Cmd+scroll (macOS) / Ctrl+scroll (others)
                // zooms. KeyModifiers::is_primary() is platform-aware.
                //
                // Real trackpad pinch would need a new `Hit::FingerMagnify`
                // variant in Makepad's platform layer (NSEventTypeMagnify
                // handler is stubbed out in macos_app.rs today). Deferred to
                // an upstream PR.
                if !fs.modifiers.is_primary() {
                    return;
                }
                let dy = if fs.scroll.y.abs() > f64::EPSILON {
                    fs.scroll.y
                } else {
                    fs.scroll.x
                };
                let factor = (1.0 - dy * 0.005).clamp(0.5, 2.0);
                let old_zoom = self.zoom.max(0.01);
                let new_zoom = (old_zoom * factor).clamp(0.2, 8.0);
                let local = fs.abs - self.last_rect.pos - self.pan;
                let content_local = local / old_zoom;
                self.pan = fs.abs - self.last_rect.pos - content_local * new_zoom;
                self.zoom = new_zoom;
                self.redraw(cx);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.doc.root.is_empty() {
            return DrawStep::done();
        }
        let sw = self.draw_svg.content_size.x;
        let sh = self.draw_svg.content_size.y;
        if sw <= 0.0 || sh <= 0.0 {
            return DrawStep::done();
        }
        let walk = Walk {
            abs_pos: walk.abs_pos,
            margin: walk.margin,
            width: match walk.width {
                Size::Fit { .. } => Size::Fixed(sw),
                other => other,
            },
            height: match walk.height {
                Size::Fit { .. } => Size::Fixed(sh),
                other => other,
            },
            metrics: walk.metrics,
        };
        let rect = cx.walk_turtle(walk);
        self.last_rect = rect;

        // Effective draw rect = widget rect shifted by `pan` and scaled by
        // `zoom`. DrawSvg::render_to_rect fits content_bounds into this rect
        // with preserve_aspect, so enlarging it zooms the geometry; shifting
        // it translates. render_text_cmds uses the same effective rect so
        // labels track the shapes 1:1.
        let z = if self.zoom > 0.01 { self.zoom } else { 1.0 };
        let eff = Rect {
            pos: rect.pos + self.pan,
            size: rect.size * z,
        };

        // Use caches populated in set_svg_str — walking the SVG doc tree
        // every frame is what made scroll/zoom laggy on 15+ node diagrams.
        self.draw_svg.svg_doc = Some(std::mem::take(&mut self.doc));
        self.draw_svg.has_animations = false;
        self.draw_svg.render_to_rect(cx, &eff, 0.0);
        self.doc = self.draw_svg.svg_doc.take().unwrap_or_default();

        let text_cmds = std::mem::take(&mut self.cached_text_cmds);
        self.render_text_cmds(cx, &eff, &text_cmds);
        self.cached_text_cmds = text_cmds;

        let edges = std::mem::take(&mut self.cached_edges);
        self.render_flow_dots(cx, &eff, &edges);
        let has_edges = !edges.is_empty();
        self.cached_edges = edges;

        // Self-sustaining animation: if we have at least one edge, keep an
        // outstanding NextFrame pending so `handle_event` keeps ticking.
        if has_edges {
            self.next_frame = cx.new_next_frame();
        }
        DrawStep::done()
    }
}

impl MermaidSvgView {
    fn render_text_cmds(&mut self, cx: &mut Cx2d, rect: &Rect, cmds: &[SvgTextCmd]) {
        if cmds.is_empty() {
            return;
        }
        let (min_x, min_y, max_x, max_y) = self.draw_svg.content_bounds;
        let content_w = (max_x - min_x) as f64;
        let content_h = (max_y - min_y) as f64;
        if content_w <= 0.0 || content_h <= 0.0 {
            return;
        }
        // Match DrawSvg::compute_render_rect's preserve-aspect letterboxing so
        // labels track the shapes.
        let scale = (rect.size.x / content_w).min(rect.size.y / content_h);
        let render_w = content_w * scale;
        let render_h = content_h * scale;
        let origin_x = rect.pos.x + (rect.size.x - render_w) * 0.5;
        let origin_y = rect.pos.y + (rect.size.y - render_h) * 0.5;

        // SVG `font-size="14px"` means 14 pixels, but Makepad's
        // `text_style.font_size` is in points (pt). 1pt ≈ 1.333px, so without
        // conversion labels come out ~33% larger than the source intends.
        const PX_TO_PT: f64 = 0.75;

        for cmd in cmds {
            if cmd.text.trim().is_empty() {
                continue;
            }
            let world_font_size = (cmd.font_size as f64 * scale * PX_TO_PT).max(1.0);
            self.draw_text.text_style.font_size = world_font_size as f32;
            self.draw_text.color = vec4(
                cmd.color.0,
                cmd.color.1,
                cmd.color.2,
                cmd.color.3.max(0.0),
            );

            // Multi-line text: lay out lines in screen-pt space using the
            // ACTUAL rendered font size, not the SVG-coord font size. rusty-
            // mermaid emits `dy="16.8"` (1.2× font_size=14 in SVG px) which
            // if applied as world coords creates 1.6× line-height in our
            // rendered (PX_TO_PT-shrunk) font. 1.2× in screen-pt gets us the
            // proper typographic rhythm.
            let lines: Vec<&str> = cmd.text.split('\n').collect();
            let line_step_screen = world_font_size * 1.2;
            // Baseline of first line in screen coords = cmd.y (central).
            // Subsequent lines cascade down by line_step. Block top = first
            // line's center - line_step/2 * (n-1); we don't shift-up the
            // whole block because rusty-mermaid already chose cmd.y so that
            // the SVG dy cascade lands the block centred inside the rect.
            let base_cy = origin_y + (cmd.y as f64 - min_y as f64) * scale;
            let base_cx_screen = origin_x + (cmd.x as f64 - min_x as f64) * scale;

            for (line_i, line) in lines.iter().enumerate() {
                if line.is_empty() {
                    continue;
                }
                // CJK-aware width estimate (Latin ≈ 0.55 · fs, CJK ≈ 1.0 · fs).
                let est_width: f64 = line
                    .chars()
                    .map(|c| {
                        let advance = if (c as u32) >= 0x2E80 { 1.0 } else { 0.55 };
                        advance * world_font_size
                    })
                    .sum();
                let anchor_shift = match cmd.text_anchor {
                    SvgTextAnchor::Start => 0.0,
                    SvgTextAnchor::Middle => -0.5,
                    SvgTextAnchor::End => -1.0,
                } * est_width;

                let px = base_cx_screen + anchor_shift;
                // Line's vertical centre: cmd.y + i * line_step (matches SVG
                // dy cascade, but line_step is now in screen-pt not SVG px).
                let cy = base_cy + line_step_screen * line_i as f64;
                // DrawText::draw_abs places top-left at given pos; shift up
                // ~half the rendered font height. Using 0.7 (not 0.5) tracks
                // Makepad's cap-height+ascent rather than pure em centre.
                let py = cy - world_font_size * 0.7;
                self.draw_text.draw_abs(cx, dvec2(px, py), line);
            }
        }
    }

    /// One dot per edge, each riding its own polyline. Phase offsets are
    /// staggered (0.17 per edge) so the diagram looks like it's breathing
    /// rather than every dot leaving the gate together. Dot alpha breathes
    /// via a `|sin|` envelope for a gentle "pulse" feel.
    fn render_flow_dots(&mut self, cx: &mut Cx2d, rect: &Rect, edges: &[SvgEdge]) {
        if edges.is_empty() {
            return;
        }
        let (min_x, min_y, max_x, max_y) = self.draw_svg.content_bounds;
        let content_w = (max_x - min_x) as f64;
        let content_h = (max_y - min_y) as f64;
        if content_w <= 0.0 || content_h <= 0.0 {
            return;
        }
        let scale = (rect.size.x / content_w).min(rect.size.y / content_h);
        let render_w = content_w * scale;
        let render_h = content_h * scale;
        let origin_x = rect.pos.x + (rect.size.x - render_w) * 0.5;
        let origin_y = rect.pos.y + (rect.size.y - render_h) * 0.5;

        // Dot diameter in screen pixels. Fixed (not zoom-dependent) so the
        // pulse stays legible at any zoom level.
        let dot_size = 10.0_f64;
        // Global alpha pulse — 1.5 Hz breathing.
        let pulse = 0.55
            + 0.45
                * (self.anim_t * std::f32::consts::TAU * 1.5)
                    .sin()
                    .abs();

        for (i, edge) in edges.iter().enumerate() {
            if edge.points.len() < 2 {
                continue;
            }
            // Stagger each edge so the flow reads as many dots, not a single
            // wave. 0.17 is prime-ish to avoid synchronising on edge counts
            // of 2/3/4.
            let phase = (self.anim_t + i as f32 * 0.17).rem_euclid(1.0);
            let max_idx = edge.points.len() - 1;
            let float_idx = phase * max_idx as f32;
            let i0 = float_idx as usize;
            let i1 = (i0 + 1).min(max_idx);
            let frac = float_idx - i0 as f32;
            let p0 = edge.points[i0];
            let p1 = edge.points[i1];
            let wx = p0.0 + (p1.0 - p0.0) * frac;
            let wy = p0.1 + (p1.1 - p0.1) * frac;

            let sx = origin_x + (wx as f64 - min_x as f64) * scale;
            let sy = origin_y + (wy as f64 - min_y as f64) * scale;

            self.draw_flow_dot.color = vec4(
                edge.color.0,
                edge.color.1,
                edge.color.2,
                edge.color.3 * pulse,
            );
            let dot_rect = Rect {
                pos: dvec2(sx - dot_size * 0.5, sy - dot_size * 0.5),
                size: dvec2(dot_size, dot_size),
            };
            self.draw_flow_dot.draw_abs(cx, dot_rect);
        }
    }
}

// ChatList widget wrapping PortalList for chat message display. One
// PortalList item per ChatMessage. Mermaid blocks inside an assistant
// message are rendered IN-PLACE by the Markdown widget via its new
// `mermaid_block` template (mirrors the existing `splash_block` pattern).
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
                        let (item_widget, _) =
                            list.item_with_existed(cx, item_id, id!(Assistant));
                        let streaming_body;
                        let text: &str = if data.streaming_text.is_empty() {
                            "..."
                        } else {
                            let opts = SanitizeOptions {
                                trim_unclosed_fence: false,
                                ..SanitizeOptions::default()
                            };
                            // P2 bisect (2026-04-19) confirmed remend is NOT
                            // the source of content-disappear; CodeView is.
                            // Remend is validated and back on. The toggle is
                            // kept as a one-line diagnostic hook in case a
                            // future regression needs the same A/B test.
                            const USE_REMEND: bool = true;
                            streaming_body = if USE_REMEND {
                                streaming_display_with_latex_autowrap_remend(
                                    &data.streaming_text,
                                    opts,
                                )
                            } else {
                                streaming_display_with_latex_autowrap(
                                    &data.streaming_text,
                                    opts,
                                )
                            };
                            // Debug: record set_text calls so we can see what
                            // the buffer looks like when content disappears.
                            // Writes to aichat_stream.log next to the exe.
                            if std::env::var("AICHAT_STREAM_LOG").is_ok() {
                                use std::io::Write;
                                if let Ok(mut f) = std::fs::OpenOptions::new()
                                    .create(true).append(true).open("aichat_stream.log")
                                {
                                    let n = streaming_body.len();
                                    let mut ts = n.saturating_sub(120);
                                    while ts < n && !streaming_body.is_char_boundary(ts) {
                                        ts += 1;
                                    }
                                    let tail = &streaming_body[ts..];
                                    let _ = writeln!(
                                        f,
                                        "[set_text] remend={} len={} tail={:?}",
                                        USE_REMEND, n, tail,
                                    );
                                }
                            }
                            &streaming_body
                        };
                        let mut markdown = item_widget.markdown(cx, ids!(selectable));
                        // Unwrap outer ```markdown wrapper in streaming
                        // content too: some LLMs emit the wrapper as the
                        // very first tokens, so we'd otherwise render a
                        // growing code block for the whole stream.
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
                        // MathView can render them. Mermaid blocks are
                        // handled by the Markdown widget itself via the
                        // `mermaid_block` template — no aichat-side
                        // extraction needed.
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum BackendType {
    ClaudeSplash,
    ClaudeAcp,
    ClaudeApi,
    Gemini,
    GeminiSplash,
    OpenAi,
    Moonshot,
}

const ALL_BACKENDS: [BackendType; 7] = [
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

When you need to show a diagram, ALWAYS use a fenced code block with the `mermaid` language tag:

```mermaid
flowchart LR
A --> B
```

NEVER emit mermaid syntax outside such a block (no `mermaid:` prefixes, no indented pseudo-code). The renderer only recognises fenced `mermaid` blocks; anything else displays as literal text.

## Fence nesting — use 4+ backticks for OUTER wrappers

CommonMark closes a 3-backtick fence at the next 3-backtick sequence — there is no nesting of same-length fences. If you want to show MARKDOWN SOURCE that itself contains ```mermaid (or any other) fenced blocks, wrap the outer demo in at least **four backticks**:

````markdown
# Demo

```mermaid
flowchart LR
  A --> B
```

More prose here.
````

WRONG (first inner ```mermaid won't render, its closing ``` prematurely closes the outer):

```markdown
```mermaid
flowchart LR
  A --> B
```
```

This rule applies whenever you're quoting a whole markdown file or README as sample content. If you're answering directly with diagrams (not showing source), just use plain ```mermaid at top level — no outer wrapper needed.

# Architecture diagrams (system design / infra / services)

When the user asks for an ARCHITECTURE diagram (系统架构, 架构图, service map, infrastructure diagram, component diagram), do NOT draw a plain flowchart. Use mermaid's semantic features so the diagram reads like a proper system map:

## 1. Use `classDef` to colour by component type

Always declare this palette at the top of an architecture diagram (tuned for dark theme):

```
classDef frontend fill:#083344,stroke:#22d3ee,stroke-width:2px,color:#e2e8f0
classDef backend  fill:#064e3b,stroke:#34d399,stroke-width:2px,color:#e2e8f0
classDef db       fill:#4c1d95,stroke:#a78bfa,stroke-width:2px,color:#e2e8f0
classDef cloud    fill:#78350f,stroke:#fbbf24,stroke-width:2px,color:#e2e8f0
classDef security fill:#881337,stroke:#fb7185,stroke-width:2px,color:#e2e8f0
classDef bus      fill:#7c2d12,stroke:#fb923c,stroke-width:2px,color:#e2e8f0
classDef external fill:#1e293b,stroke:#94a3b8,stroke-width:2px,color:#e2e8f0
```

Then tag each node with `:::class`:
- `A[Web App]:::frontend`
- `B[API Gateway]:::backend`
- `C[(Postgres)]:::db`
- `D[S3]:::cloud`
- `E[Auth]:::security`
- `F[Kafka]:::bus`
- `G[Stripe API]:::external`

## 2. Use `subgraph` to group layers / regions

```
subgraph Edge["边缘层"]
  W[Web UI]:::frontend
  M[Mobile App]:::frontend
end
subgraph API["服务层"]
  G[Gateway]:::backend
  S[Service]:::backend
end
```

## 3. Use `<br/>` for two-line labels

`A["Auth Service<br/>JWT · OAuth2"]:::security` — the first line is the primary name, the second line is a short technology/role hint (≤3 words, separated by `·`).

**Label-length discipline** (the renderer does NOT auto-wrap — overflow is visible):
- Primary line: ≤ **2 words** in English, or ≤ **4 Han characters** in Chinese.
- Secondary line (after `<br/>`): ≤ **3 short tokens** separated by ` · ` (middle dot with spaces), e.g. `React · Vite`, `Go · Gin`, `JWT · OAuth2`.
- Do NOT cram a whole sentence into a node. If a component needs a full description, put it in prose below the diagram, not inside the node.
- Prefer `PostgreSQL<br/>Messages` over `"PostgreSQL Messages"` (splits into two short lines).

## 4. Prefer cylinder shape for databases + caches

`A[("Postgres<br/>messages")]:::db` — the `[(...)]` syntax renders a cylinder, which visually reads as a database / persistent store. Use it for `Postgres`, `Redis`, `MongoDB`, `S3`, `Milvus`, etc. Keep the inner label to primary-name + optional `<br/>`-subline; don't nest quotes.

## 4. Prefer TD/TB flow; keep edge labels short

`A -- "stream" --> B` — one or two words, no sentences.

## Full example (follow this shape for any architecture request):

```mermaid
flowchart TD
  classDef frontend fill:#083344,stroke:#22d3ee,stroke-width:2px,color:#e2e8f0
  classDef backend  fill:#064e3b,stroke:#34d399,stroke-width:2px,color:#e2e8f0
  classDef db       fill:#4c1d95,stroke:#a78bfa,stroke-width:2px,color:#e2e8f0
  classDef security fill:#881337,stroke:#fb7185,stroke-width:2px,color:#e2e8f0

  subgraph Edge["边缘层"]
    W["Web UI<br/>React · Vite"]:::frontend
  end
  subgraph API["服务层"]
    G["Gateway<br/>Kong · JWT"]:::security
    S["Chat Service<br/>Axum · Tokio"]:::backend
  end
  subgraph Data["数据层"]
    C[("Redis<br/>cache")]:::db
    P[("Postgres<br/>messages")]:::db
  end
  W -- "HTTPS" --> G
  G -- "gRPC" --> S
  S --> C
  S --> P
```

For SIMPLE flowcharts (a short if/else, a process walkthrough, etc.) — keep it plain without classDef/subgraph. Only reach for the full architecture template when the diagram is actually an architecture diagram."#
                  .to_string(),
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
    fn detect_available_backends() -> Vec<BackendType> {
        let mut available_backends = vec![];
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
                    },
                )))) as Box<dyn Agent>
            }),
            BackendType::Moonshot => Self::read_key("MOONSHOT_API_KEY").map(|key| {
                let model = std::env::var("MOONSHOT_MODEL")
                    .unwrap_or_else(|_| "kimi-k2.5".to_string());
                let base_url = std::env::var("MOONSHOT_BASE_URL").unwrap_or_else(|_| {
                    "https://api.moonshot.ai/v1/chat/completions".to_string()
                });
                Box::new(StatelessBackendAdapter::new(Box::new(OpenAiBackend::new(
                    BackendConfig::OpenAI {
                        api_key: key,
                        model,
                        base_url: Some(base_url),
                        reasoning_effort: None,
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
        self.ui.redraw(cx);
    }

    fn send_message(&mut self, cx: &mut Cx) {
        let input = self.ui.text_input(cx, ids!(input));
        let text = input.text();
        if text.trim().is_empty() {
            return;
        }
        if std::env::var("AICHAT_STREAM_LOG").is_ok() {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true).append(true).open("aichat_stream.log")
            {
                let head: String = text.chars().take(40).collect();
                let _ = writeln!(
                    f, "[send_message] user_input={:?}", head
                );
            }
        }

        let (agent, session_id) = match (&mut self.agent, self.session_id) {
            (Some(agent), Some(session_id)) => (agent, session_id),
            _ => return,
        };

        let items_len = {
            let mut data = CHAT_DATA.write().unwrap();
            data.messages.push(ChatMessage {
                role: ChatRole::User,
                text: text.clone(),
            });
            data.streaming_text.clear();
            data.is_streaming = true;
            data.messages.len() + 1
        };
        input.set_text(cx, "");

        // Inject history on first prompt for stateless backends
        if !self.history_injected && agent.is_stateless() {
            let data = CHAT_DATA.read().unwrap();
            let history: Vec<Message> = data.messages[..data.messages.len() - 1]
                .iter()
                .map(|m| match m.role {
                    ChatRole::User => Message::user(&m.text),
                    ChatRole::Assistant => Message::assistant(&m.text),
                })
                .collect();
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

    /// Open a URL in the system browser. Platform-dispatched via std::process::Command
    /// so we don't pull a new crate dependency. Returns Err if the process couldn't be
    /// spawned; caller may ignore.
    fn open_url_in_browser(url: &str) -> std::io::Result<()> {
        #[cfg(target_os = "macos")]
        let cmd = std::process::Command::new("open").arg(url).spawn();
        #[cfg(target_os = "linux")]
        let cmd = std::process::Command::new("xdg-open").arg(url).spawn();
        #[cfg(target_os = "windows")]
        let cmd = std::process::Command::new("cmd").args(["/C", "start", "", url]).spawn();
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        let cmd: std::io::Result<std::process::Child> = Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "unsupported platform",
        ));
        cmd.map(|_| ())
    }

    fn cancel_request(&mut self, cx: &mut Cx) {
        if let (Some(agent), Some(prompt_id)) = (&mut self.agent, self.current_prompt.take()) {
            agent.cancel_prompt(cx, prompt_id);

            let mut data = CHAT_DATA.write().unwrap();
            let text = std::mem::take(&mut data.streaming_text);
            if !text.is_empty() {
                data.messages.push(ChatMessage {
                    role: ChatRole::Assistant,
                    text,
                });
            }
            data.is_streaming = false;
            drop(data);

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
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        // Markdown link click — open the URL in the system browser only
        // when Cmd (macOS) / Super (Linux/Windows) was held. Plain clicks
        // are reserved for drag-selection inside the Markdown widget.
        for action in actions {
            if let Some(md_action) = action.downcast_ref::<makepad_widgets::markdown::MarkdownAction>() {
                if let makepad_widgets::markdown::MarkdownAction::LinkNavigated { url, modifiers } = md_action {
                    if modifiers.logo {
                        let _ = Self::open_url_in_browser(url);
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
        let default_backend = if self.available_backends.contains(&BackendType::ClaudeSplash) {
            Some(BackendType::ClaudeSplash)
        } else if self.available_backends.contains(&BackendType::GeminiSplash) {
            Some(BackendType::GeminiSplash)
        } else {
            self.available_backends.first().copied()
        };
        if let Some(backend) = default_backend {
            self.switch_backend(cx, backend);
            self.ui
                .drop_down(cx, ids!(backend_dropdown))
                .set_selected_item(cx, backend.to_index());
        }
        self.update_status(cx);
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        crate::makepad_code_editor::script_mod(vm);
        self::script_mod(vm)
    }

    fn after_new_from_script(_vm: &mut ScriptVm, app: &mut Self) {
        CHAT_DATA.write().unwrap().messages = ChatData::load_from_disk();
        app.available_backends = Self::detect_available_backends();
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
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
                        let item_id = {
                            let mut data = CHAT_DATA.write().unwrap();
                            if std::env::var("AICHAT_STREAM_LOG").is_ok() {
                                use std::io::Write;
                                if let Ok(mut f) = std::fs::OpenOptions::new()
                                    .create(true).append(true).open("aichat_stream.log")
                                {
                                    let dlen = text.len();
                                    let bhead: String = text.chars().take(40).collect();
                                    let _ = writeln!(
                                        f,
                                        "[delta] buf_before={} delta_len={} delta_head={:?}",
                                        data.streaming_text.len(), dlen, bhead,
                                    );
                                }
                            }
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
                    AgentEvent::TurnComplete { .. } => {
                        if std::env::var("AICHAT_STREAM_LOG").is_ok() {
                            use std::io::Write;
                            if let Ok(mut f) = std::fs::OpenOptions::new()
                                .create(true).append(true).open("aichat_stream.log")
                            {
                                let _ = writeln!(f, "[TurnComplete]");
                            }
                        }
                        let mut data = CHAT_DATA.write().unwrap();
                        let text = std::mem::take(&mut data.streaming_text);
                        if !text.is_empty() {
                            data.messages.push(ChatMessage {
                                role: ChatRole::Assistant,
                                text,
                            });
                        }
                        data.is_streaming = false;
                        data.save_to_disk();
                        drop(data);

                        self.current_prompt = None;
                        self.ui.view(cx, ids!(cancel_button)).set_visible(cx, false);
                        cx.redraw_all();
                    }
                    AgentEvent::PromptError { error, .. } => {
                        CHAT_DATA.write().unwrap().is_streaming = false;
                        self.current_prompt = None;
                        self.ui.view(cx, ids!(cancel_button)).set_visible(cx, false);
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
