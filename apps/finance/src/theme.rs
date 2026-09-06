//! The palette, in one place.
//!
//! Published into `mod.finance.*` so the DSL reads `mod.finance.accent`
//! rather than a hex literal repeated forty times — change a colour here
//! and every screen moves together.
//!
//! Surfaces and controls follow the active widget theme. Chart series and
//! financial/status semantics keep their dedicated colors.
//!
use makepad_widgets::*;

pub fn install(vm: &mut ScriptVm) {
    script_eval!(vm, {
        mod.finance = {
            // Surfaces, darkest to lightest: the page, a card, and a
            // control on that card. Each step is a few percent lighter,
            // which is the whole elevation system — there are no shadows.
            bg: mod.theme.color_bg_app,
            panel: mod.theme.color_bg_container,
            raised: mod.theme.color_outset,
            line: mod.theme.color_bevel_inset_2,
            line_soft: mod.theme.color_bevel_outset_1,

            // Text.
            fg: mod.theme.color_text,
            fg_dim: mod.theme.color_text_disabled,
            fg_faint: mod.theme.color_text_disabled,

            // One accent, used for selection, the active tab and the
            // primary action. Anything else that wants attention has to
            // earn it with weight or size instead.
            accent: mod.theme.color_focus,
            accent_soft: mod.theme.color_bg_highlight,

            // Money. Nothing else may use these two.
            up: #x3fb950,
            down: #xf85149,

            // Chart series, in fixed order, never cycled. These are the
            // dataviz reference palette's dark steps: the set passes the
            // colour-blindness separation and contrast checks as a whole,
            // which a hand-picked set of "nice" hues does not — the blue
            // and violet I first chose were 2.4 ΔE apart to a deuteranope,
            // which is to say identical.
            c0: #x3987e5,
            c1: #xd95926,
            c2: #x199e70,
            c3: #xc98500,
            c4: #xd55181,
            c5: #x008300,
            c6: #x9085e9,
            c7: #xe66767,

            // Status, reserved: these four never stand in for a series.
            good: #x0ca30c,
            warning: #xfab219,
            serious: #xec835a,
            critical: #xd03b3b,

            // The warm tint behind a row that needs attention.
            warn: #x3a2d16,

            // Register surfaces: the alternate row is a hair lighter than
            // the page, never a different colour.
            zebra: mod.theme.color_bg_odd,
            select: mod.theme.color_bg_highlight,
        }
    });
}

/// Chart colours by index, for series the Rust side hands out.
pub const SERIES: [u32; 8] = [
    0x3987e5, 0xd95926, 0x199e70, 0xc98500, 0xd55181, 0x008300, 0x9085e9, 0xe66767,
];

/// Status colours, reserved. Money in and money out are STATUS, not series
/// — which is why they are never drawn from [`SERIES`].
pub const GOOD: u32 = 0x0ca30c;
pub const CRITICAL: u32 = 0xd03b3b;
pub const WARNING: u32 = 0xfab219;

/// A category's colour: its own if it has one, else one picked from the
/// series by id so it stays the same colour on every screen and across
/// runs.
pub fn category_color(id: i64, stored: u32) -> Vec4f {
    let rgb = if stored != 0 { stored } else { SERIES[(id.unsigned_abs() as usize) % SERIES.len()] };
    Vec4f {
        x: ((rgb >> 16) & 0xff) as f32 / 255.0,
        y: ((rgb >> 8) & 0xff) as f32 / 255.0,
        z: (rgb & 0xff) as f32 / 255.0,
        w: 1.0,
    }
}

pub fn rgb(value: u32) -> Vec4f {
    category_color(0, value)
}
