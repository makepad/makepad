//! The look: one signature theme, a near-black terminal lit by an ion-cyan /
//! signal-magenta pair. Every colour the window uses is named once in
//! `PALETTE`; drawing code reads it through `palette()`, and the layout reads
//! the same values as `theme.*` and `mod.look.*`, set by the script below
//! before the layout is evaluated.
//!
//! Roles, not hues: `primary`/`info` is the agent (talking, working,
//! selected), `human` is you (your prompt and everything that waits on you:
//! approvals, questions), `ok`/`error` are outcomes, `edited` a file that
//! changed, `lane` a sub-agent. Glow is kept for the one thing that is live.

use makepad_widgets::*;

/// Every colour the window uses, as 0xRRGGBB. The rest of the app reads
/// these (through `palette()` / `vec()`) and never spells a colour itself.
pub struct Palette {
    /// The window's ground.
    pub bg_app: u32,
    /// Rail and editor pane.
    pub container: u32,
    pub surface: u32,
    pub raised: u32,
    pub text: u32,
    /// Your words and active rows: the brightest ink.
    pub text_hi: u32,
    pub meta: u32,
    pub disabled: u32,
    /// The agent, and selection.
    pub primary: u32,
    /// Hairlines: the only borders.
    pub outline: u32,
    pub outline_hi: u32,
    pub warning: u32,
    pub error: u32,
    /// Finished, applied, added.
    pub ok: u32,
    /// Running, live.
    pub info: u32,
    /// You, and everything waiting on you.
    pub human: u32,
    /// Sub-agents and reasoning.
    pub lane: u32,
    pub add: u32,
    pub del: u32,
    pub edited: u32,
    pub read: u32,
    pub terminal_bg: u32,
    pub terminal_text: u32,
}

pub const PALETTE: Palette = Palette {
    bg_app: 0x05070a,
    container: 0x080b0f,
    surface: 0x0b0f15,
    raised: 0x111720,
    text: 0xc3cdd8,
    text_hi: 0xeaf2f8,
    meta: 0x6a7787,
    disabled: 0x3a4554,
    primary: 0x3ce6f2,
    outline: 0x18202a,
    outline_hi: 0x253140,
    warning: 0xffbe4d,
    error: 0xff6168,
    ok: 0xa8ef5a,
    info: 0x3ce6f2,
    human: 0xff4f9e,
    lane: 0xa58cff,
    add: 0x6cc24a,
    del: 0xe0505a,
    edited: 0xffbe4d,
    read: 0x34b9c4,
    terminal_bg: 0x040609,
    terminal_text: 0x93a1b2,
};

pub fn palette() -> &'static Palette {
    &PALETTE
}

/// A palette colour with an alpha, for drawing code.
pub const fn vec(rgb: u32, alpha: f32) -> Vec4f {
    vec4(((rgb >> 16) & 0xff) as f32 / 255.0, ((rgb >> 8) & 0xff) as f32 / 255.0, (rgb & 0xff) as f32 / 255.0, alpha)
}

const fn op(rgb: u32) -> Vec4f {
    vec(rgb, 1.0)
}

const P: &Palette = &PALETTE;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // ---- the stock roles, so stock widgets sit in the same ink -----------

    mod.theme.color_bg_app = #(op(P.bg_app))
    mod.theme.color_fg_app = #(op(P.surface))
    mod.theme.color_bg_container = #(op(P.container))
    mod.theme.color_surface_container = #(op(P.surface))
    mod.theme.color_surface_container_highest = #(op(P.raised))
    mod.theme.color_text = #(op(P.text))
    mod.theme.color_text_meta = #(op(P.meta))
    mod.theme.color_text_disabled = #(op(P.disabled))
    mod.theme.color_primary = #(op(P.primary))
    mod.theme.color_outline_variant = #(op(P.outline))
    mod.theme.color_warning = #(op(P.warning))
    mod.theme.color_error = #(op(P.error))
    mod.theme.color_text_hl = #(op(P.text_hi))
    mod.theme.color_text_cursor = #(op(P.primary))
    mod.theme.color_text_placeholder = #(op(P.disabled))
    mod.theme.color_text_placeholder_hover = #(op(P.disabled))
    mod.theme.color_selection_focus = #(vec(P.primary, 0.28))
    mod.theme.color_selection_hover = #(vec(P.primary, 0.18))
    mod.theme.color_label_inner = #(op(P.text))
    mod.theme.color_label_inner_hover = #(op(P.text_hi))
    mod.theme.color_label_inner_down = #(op(P.text_hi))
    mod.theme.color_label_inner_focus = #(op(P.text_hi))
    mod.theme.color_label_inner_disabled = #(op(P.disabled))
    mod.theme.color_label_outer = #(op(P.meta))

    // ---- sizes: a 4-point rhythm, type in points --------------------------

    mod.theme.code_size_title = 9.5
    mod.theme.code_size_body = 9.0
    mod.theme.code_size_text = 9.0
    mod.theme.code_size_small = 8.5
    mod.theme.code_size_caption = 7.5
    mod.theme.code_row = 20
    mod.theme.code_gutter = 22
    mod.theme.code_page_x = 20

    // ---- the look's own tokens --------------------------------------------

    mod.look = {
        bg: #(op(P.bg_app))
        rail: #(op(P.container))
        surface: #(op(P.surface))
        raised: #(op(P.raised))
        hair: #(op(P.outline))
        hair_hi: #(op(P.outline_hi))
        text_hi: #(op(P.text_hi))
        text: #(op(P.text))
        meta: #(op(P.meta))
        dim: #(op(P.disabled))
        agent: #(op(P.primary))
        human: #(op(P.human))
        good: #(op(P.ok))
        bad: #(op(P.error))
        change: #(op(P.edited))
        lane: #(op(P.lane))
        terminal_text: #(op(P.terminal_text))

        // Washes: a signal colour laid thin over the ground.
        human_wash: #(vec(P.human, 0.035))
        human_field: #(vec(P.human, 0.055))
        lane_wash: #(vec(P.lane, 0.055))
        lane_text: #(vec(P.lane, 0.66))
        agent_wash: #(vec(P.primary, 0.06))
        agent_select: #(vec(P.primary, 0.26))
        // Panes over the textured ground, and wells sunk into it.
        glass: #(vec(P.container, 0.72))
        well: #(vec(P.terminal_bg, 0.72))

        // One typeface for everything: JetBrains Mono (in the tree, OFL),
        // with the CJK and emoji faces behind it, loaded only on a miss.
        mono: TextStyle{
            font_size: 9.0
            line_spacing: 1.32
            font_family: FontFamily{
                latin := FontMember{res: crate_resource("makepad_widgets:resources/jetbrains_mono_variable.ttf") asc: 0.0 desc: 0.0 weight: 400.0}
                cjk := FontMember{res: crate_resource("makepad_widgets:resources/LXGWWenKaiRegular.ttf") asc: 0.0 desc: 0.0 lazy: 1.0}
                emoji := FontMember{res: crate_resource("makepad_widgets:resources/NotoColorEmoji.ttf") asc: 0.0 desc: 0.0 lazy: 2.0}
            }
        }
        mono_bold: TextStyle{
            font_size: 9.0
            line_spacing: 1.32
            font_family: FontFamily{
                latin := FontMember{res: crate_resource("makepad_widgets:resources/jetbrains_mono_variable.ttf") asc: 0.0 desc: 0.0 weight: 700.0}
                cjk := FontMember{res: crate_resource("makepad_widgets:resources/LXGWWenKaiBold.ttf") asc: 0.0 desc: 0.0 lazy: 1.0}
                emoji := FontMember{res: crate_resource("makepad_widgets:resources/NotoColorEmoji.ttf") asc: 0.0 desc: 0.0 lazy: 2.0}
            }
        }
    }
}
