//! The palette. mpwm exports its active `theme.splash` as MPWM_THEME_SPLASH;
//! `mp_theme` line-scans it and retints the stock widgets, and this module
//! publishes the same colors as `mod.mpf.*` so mpfiles' own chrome — which is
//! all custom views — follows the desktop theme too. Standalone runs get
//! Tokyo Night, so the app is dark and square either way.

use makepad_widgets::*;
use std::sync::OnceLock;

/// Every color the DSL reads, as `#rrggbb` strings.
#[derive(Clone, Debug)]
pub struct Palette {
    pub accent: String,
    pub bg: String,
    pub bg_dark: String,
    pub bg_light: String,
    pub fg: String,
    pub fg_bright: String,
    pub fg_dim: String,
    pub muted: String,
    /// Row/tile selection: the accent lifted out of the background enough to
    /// read at a glance, which a raw theme `selection` often is not.
    pub sel: String,
    /// The same selection as a translucent overlay, for the grid — which
    /// paints its selection *over* the cells it has already drawn.
    pub sel_soft: String,
    /// Hover, one step below selection.
    pub hover: String,
    /// Zebra stripe for the list view.
    pub stripe: String,
    /// The popup card's hover: the foreground at 8% over the background,
    /// which is the omarchy menu's own rule.
    pub hover_soft: String,
    /// The one warning color in the app, for the row that cannot be undone.
    /// It comes from the theme's red, because a theme that has a red has an
    /// opinion about what danger looks like.
    pub danger: String,
    /// The treemap's kind classes, in the order [`KIND_COLOR_KEYS`] names
    /// them: video, image, audio, code, text, archive, other. They come from
    /// the theme's terminal palette, which is the only place a WM theme keeps
    /// a full spread of hues — the chrome palette is all one family by design.
    pub kinds: [String; 7],
}

/// The terminal-palette keys the treemap's kind colors come from, with the
/// Tokyo Night value each falls back to. Order matches [`Palette::kinds`].
/// The theme's red, when it has none of its own: Tokyo Night's.
const DANGER_FALLBACK: &str = "#f7768e";

const KIND_COLOR_KEYS: [(&str, &str); 7] = [
    ("term.color4", "#7aa2f7"),  // video   — blue
    ("term.color2", "#9ece6a"),  // image   — green
    ("term.color3", "#e0af68"),  // audio   — yellow
    ("term.color6", "#7dcfff"),  // code    — cyan
    ("term.color7", "#a9b1d6"),  // text    — foreground
    ("term.color5", "#bb9af7"),  // archive — magenta
    ("term.color8", "#414868"),  // other   — muted
];

impl Default for Palette {
    fn default() -> Self {
        Self::tokyo_night()
    }
}

impl Palette {
    /// The fallback theme, matching mpwm's default.
    pub fn tokyo_night() -> Self {
        Self::derive(
            "#7aa2f7", "#1a1b26", "#16161e", "#24283b", "#a9b1d6", "#c0caf5", "#565f89", "#414868",
            DANGER_FALLBACK,
            KIND_COLOR_KEYS.map(|(_, fallback)| fallback.to_string()),
        )
    }

    /// The palette for this process, read once.
    pub fn shared() -> &'static Palette {
        static PALETTE: OnceLock<Palette> = OnceLock::new();
        PALETTE.get_or_init(Palette::load)
    }

    /// The palette mpwm exported for this process, or Tokyo Night.
    pub fn load() -> Self {
        if crate::vfs::demo_requested() {
            return Self::tokyo_night();
        }
        let Some(p) = mp_theme::current() else {
            return Self::tokyo_night();
        };
        Self::derive(
            &p.hex("accent", "#7aa2f7"),
            &p.hex("background", "#1a1b26"),
            &p.hex("darker_background", "#16161e"),
            &p.hex("lighter_background", "#24283b"),
            &p.hex("foreground", "#a9b1d6"),
            &p.hex("bright_foreground", "#c0caf5"),
            &p.hex("dark_foreground", "#565f89"),
            &p.hex("muted", "#414868"),
            &p.hex("term.color1", DANGER_FALLBACK),
            KIND_COLOR_KEYS.map(|(key, fallback)| p.hex(key, fallback)),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn derive(
        accent: &str,
        bg: &str,
        bg_dark: &str,
        bg_light: &str,
        fg: &str,
        fg_bright: &str,
        fg_dim: &str,
        muted: &str,
        danger: &str,
        kinds: [String; 7],
    ) -> Self {
        Self {
            sel: mix(accent, bg, 0.34),
            sel_soft: format!("#{}4d", accent.trim().trim_start_matches('#')),
            hover: mix(accent, bg, 0.13),
            hover_soft: mix(fg, bg, 0.08),
            danger: danger.to_string(),
            stripe: mix(bg_light, bg, 0.4),
            accent: accent.to_string(),
            bg: bg.to_string(),
            bg_dark: bg_dark.to_string(),
            bg_light: bg_light.to_string(),
            fg: fg.to_string(),
            fg_bright: fg_bright.to_string(),
            fg_dim: fg_dim.to_string(),
            muted: muted.to_string(),
            kinds,
        }
    }

    /// The fill for one treemap kind class, by its index in [`Palette::kinds`].
    /// Out-of-range classes read as "other" rather than panicking: a map that
    /// paints an unknown file grey is right, one that crashes is not.
    pub fn kind_color(&self, class: usize) -> Vec4f {
        Self::vec4(&self.kinds[class.min(self.kinds.len() - 1)])
    }

    /// A color as a makepad `Vec4f`, for the handful of places Rust sets one.
    pub fn vec4(hex: &str) -> Vec4f {
        let (r, g, b) = rgb(hex);
        Vec4f {
            x: r as f32 / 255.0,
            y: g as f32 / 255.0,
            z: b as f32 / 255.0,
            w: 1.0,
        }
    }

    /// Publish the palette as `mod.mpf` so `script_mod!` can read it. Call
    /// after `makepad_widgets::script_mod` and before the app's own module.
    pub fn publish(&self, vm: &mut ScriptVm) {
        let code = format!(
            "mod.mpf = {{\n\
             accent: {accent}\n\
             bg: {bg}\n\
             bg_dark: {bg_dark}\n\
             bg_light: {bg_light}\n\
             fg: {fg}\n\
             fg_bright: {fg_bright}\n\
             fg_dim: {fg_dim}\n\
             muted: {muted}\n\
             sel: {sel}\n\
             sel_soft: {sel_soft}\n\
             hover: {hover}\n\
             stripe: {stripe}\n\
             }}\n\
             true\n",
            accent = self.accent,
            bg = self.bg,
            bg_dark = self.bg_dark,
            bg_light = self.bg_light,
            fg = self.fg,
            fg_bright = self.fg_bright,
            fg_dim = self.fg_dim,
            muted = self.muted,
            sel = self.sel,
            sel_soft = self.sel_soft,
            hover = self.hover,
            stripe = self.stripe,
        );
        vm.eval(ScriptMod {
            cargo_manifest_path: env!("CARGO_MANIFEST_DIR").to_string(),
            module_path: "mpfiles_palette".to_string(),
            file: "palette.splash".to_string(),
            line: 0,
            column: 0,
            code,
            values: vec![],
        });
        for e in vm.take_errors() {
            log!("mpfiles palette: {}", e);
        }
    }
}

/// `#rrggbb` (or `#rrggbbaa`) -> components. Unparseable input reads black,
/// which is visible rather than silently theme-shaped.
fn rgb(hex: &str) -> (u8, u8, u8) {
    let hex = hex.trim().trim_start_matches('#');
    if hex.len() < 6 {
        return (0, 0, 0);
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0);
    (byte(0), byte(2), byte(4))
}

/// `a` over `b` at `t`, as a `#rrggbb` string.
fn mix(a: &str, b: &str, t: f64) -> String {
    let (ar, ag, ab) = rgb(a);
    let (br, bg, bb) = rgb(b);
    let c = |x: u8, y: u8| (x as f64 * t + y as f64 * (1.0 - t)).round().clamp(0.0, 255.0) as u8;
    format!("#{:02x}{:02x}{:02x}", c(ar, br), c(ag, bg), c(ab, bb))
}

// The one text-field shape the whole app uses: flat, square, borderless,
// sitting inside whatever plate hosts it. It lives here, with the palette,
// because every other module's `use mod.widgets.*` resolves at the top of its
// own block — a widget defined inside the block that uses it is invisible to
// that block, so the shared field has to be registered first.
script_mod! {
    use mod.prelude.widgets.*

    // The one text field shape the whole app uses: flat, square, borderless,
    // sitting inside whatever plate hosts it. Published on `mod.widgets` so
    // the shell's path bar and dialogs get the same field as the inline
    // editors here — a `let` binding would be local to this block.
    mod.widgets.MpfInput = set_type_default() do TextInput{
        width: Fill
        height: Fill
        margin: 0.0
        padding: Inset{left: 6 right: 6 top: 3 bottom: 3}
        draw_bg +: {
            border_radius: uniform(0.0)
            border_size: uniform(1.0)
            color: mod.mpf.bg
            color_hover: uniform(mod.mpf.bg)
            color_focus: uniform(mod.mpf.bg)
            color_down: uniform(mod.mpf.bg)
            color_empty: uniform(mod.mpf.bg)
            color_disabled: uniform(mod.mpf.bg)
            color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            border_color: uniform(mod.mpf.muted)
            border_color_hover: uniform(mod.mpf.muted)
            border_color_focus: uniform(mod.mpf.accent)
            border_color_down: uniform(mod.mpf.accent)
            border_color_empty: uniform(mod.mpf.muted)
            border_color_disabled: uniform(mod.mpf.muted)
            border_color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
        }
        draw_text +: {
            color: mod.mpf.fg_bright
            color_hover: uniform(mod.mpf.fg_bright)
            color_focus: uniform(mod.mpf.fg_bright)
            color_down: uniform(mod.mpf.fg_bright)
            color_disabled: uniform(mod.mpf.fg_dim)
            color_empty: uniform(mod.mpf.fg_dim)
            color_empty_hover: uniform(mod.mpf.fg_dim)
            color_empty_focus: uniform(mod.mpf.fg_dim)
            text_style: theme.font_regular{font_size: 9.5}
        }
        draw_cursor +: {color: uniform(mod.mpf.accent)}
        draw_selection +: {
            border_radius: uniform(0.0)
            color: uniform(mod.mpf.sel)
            color_hover: uniform(mod.mpf.sel)
            color_focus: uniform(mod.mpf.sel)
            color_down: uniform(mod.mpf.sel)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixes_toward_the_first_color() {
        assert_eq!(mix("#ffffff", "#000000", 0.0), "#000000");
        assert_eq!(mix("#ffffff", "#000000", 1.0), "#ffffff");
        assert_eq!(mix("#ffffff", "#000000", 0.5), "#808080");
    }

    #[test]
    fn selection_sits_between_accent_and_background() {
        let p = Palette::tokyo_night();
        assert_ne!(p.sel, p.bg);
        assert_ne!(p.sel, p.accent);
        // Hover is the quieter of the two.
        assert_ne!(p.hover, p.sel);
    }

    #[test]
    fn every_treemap_kind_has_its_own_hue() {
        let p = Palette::tokyo_night();
        let mut seen: Vec<&str> = p.kinds.iter().map(String::as_str).collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), p.kinds.len(), "kind colors must be distinct");
        // Every class resolves, and an out-of-range one falls to "other".
        for class in 0..p.kinds.len() {
            assert_eq!(p.kind_color(class), Palette::vec4(&p.kinds[class]));
        }
        assert_eq!(p.kind_color(99), p.kind_color(p.kinds.len() - 1));
    }

    #[test]
    fn parses_hex() {
        assert_eq!(rgb("#7aa2f7"), (0x7a, 0xa2, 0xf7));
        assert_eq!(rgb("7aa2f7"), (0x7a, 0xa2, 0xf7));
        assert_eq!(rgb("bad"), (0, 0, 0));
        let v = Palette::vec4("#ff8000");
        assert!((v.x - 1.0).abs() < 0.001);
        assert!((v.z - 0.0).abs() < 0.001);
    }
}
