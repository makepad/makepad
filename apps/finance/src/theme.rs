//! The palette, in one place.
//!
//! Published into `mod.finance.*` so the DSL reads `mod.finance.accent`
//! rather than a hex literal repeated forty times — change a colour here
//! and every screen moves together.
//!
//! It is a dark palette because a ledger is a wall of numbers, and dark
//! rows with one bright accent let the numbers carry the contrast instead
//! of fighting the background for it.
//!
//! Three rules, taken from what the best-looking money apps actually do:
//!
//! * **A cool near-black, never `#000`, and elevation by tone.** Three
//!   surfaces each a few percent lighter than the last, separated by
//!   hairlines rather than shadows. Drop shadows on cards are the single
//!   clearest "designed in 2018" tell.
//! * **One saturated accent.** Indigo, and nothing else competes with it.
//!   Restricting the palette is what reads as expensive; a different bright
//!   colour per spending category reads as a 2015 budgeting app.
//! * **Money colour is reserved and redundant.** Good/critical are status,
//!   never a chart series, and they never carry meaning alone — the sign is
//!   always there too, because roughly one man in twelve cannot tell the
//!   two hues apart.

use makepad_widgets::*;

pub fn install(vm: &mut ScriptVm) {
    script_eval!(vm, {
        mod.finance = {
            // Surfaces, darkest to lightest: the page, a card, and a
            // control on that card. Each step is a few percent lighter,
            // which is the whole elevation system — there are no shadows.
            bg: #x0c0d12,
            panel: #x14161d,
            raised: #x1b1e27,
            line: #x272a35,
            line_soft: #x1e212a,

            // Text.
            fg: #xf2f4f8,
            fg_dim: #xa2a8b8,
            fg_faint: #x6f7585,

            // One accent, used for selection, the active tab and the
            // primary action. Anything else that wants attention has to
            // earn it with weight or size instead.
            accent: #x5e6ad2,
            accent_soft: #x272a52,

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
            zebra: #x101219,
            select: #x5e6ad233,
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
