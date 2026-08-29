//! The palette. mpwm exports its active `theme.splash` as MPWM_THEME_SPLASH;
//! `mp_theme` line-scans it and retints the stock widgets, and this module
//! publishes the colors mppdf's own chrome uses as `mod.mpp.*` (for the DSL)
//! and as `Vec4f` (for the bits the widgets draw themselves). Standalone
//! runs get Tokyo Night, so the viewer is dark and square either way. Flat
//! fills only: no gradients, no rounded corners.

use makepad_widgets::*;
use std::sync::OnceLock;

/// Every color mppdf paints, as `#rrggbb` strings.
#[derive(Clone, Debug)]
pub struct Palette {
    /// The window and the toolbar's own ground.
    pub background: String,
    /// The gutter the pages float on, and the thumbnail strip.
    pub darker_background: String,
    /// Control plates (the page field, a pressed toggle).
    pub lighter_background: String,
    /// Toolbar text and the status line.
    pub foreground: String,
    /// Separators, inactive glyphs, the page-of-count denominator.
    pub dark_foreground: String,
    /// The active page indicator: the thumbnail's frame and the fit toggle
    /// that is on.
    pub accent: String,
}

impl Default for Palette {
    fn default() -> Self {
        Self::tokyo_night()
    }
}

impl Palette {
    /// The fallback theme, matching mpwm's default.
    pub fn tokyo_night() -> Self {
        Self {
            background: "#1a1b26".into(),
            darker_background: "#0e0e14".into(),
            lighter_background: "#24283b".into(),
            foreground: "#a9b1d6".into(),
            dark_foreground: "#565f89".into(),
            accent: "#7aa2f7".into(),
        }
    }

    /// The palette for this process, read once.
    pub fn shared() -> &'static Palette {
        static PALETTE: OnceLock<Palette> = OnceLock::new();
        PALETTE.get_or_init(Palette::load)
    }

    /// The palette mpwm exported for this process, or Tokyo Night.
    pub fn load() -> Self {
        let Some(p) = mp_theme::current() else {
            return Self::tokyo_night();
        };
        Self {
            background: p.hex("background", "#1a1b26"),
            darker_background: p.hex("darker_background", "#0e0e14"),
            lighter_background: p.hex("lighter_background", "#24283b"),
            foreground: p.hex("foreground", "#a9b1d6"),
            dark_foreground: p.hex("dark_foreground", "#565f89"),
            accent: p.hex("accent", "#7aa2f7"),
        }
    }

    pub fn bg_vec4(&self) -> Vec4f {
        vec4_of(&self.background)
    }
    pub fn bg_dark_vec4(&self) -> Vec4f {
        vec4_of(&self.darker_background)
    }
    pub fn accent_vec4(&self) -> Vec4f {
        vec4_of(&self.accent)
    }
    pub fn dim_vec4(&self) -> Vec4f {
        vec4_of(&self.dark_foreground)
    }

    /// Publish the palette as `mod.mpp` so `script_mod!` can read it. Call
    /// after `mp_theme::apply` and before the app's own modules.
    pub fn publish(&self, vm: &mut ScriptVm) {
        let code = format!(
            "mod.mpp = {{\n\
             bg: {background}\n\
             bg_dark: {darker_background}\n\
             bg_light: {lighter_background}\n\
             fg: {foreground}\n\
             dim: {dark_foreground}\n\
             accent: {accent}\n\
             }}\n\
             true\n",
            background = self.background,
            darker_background = self.darker_background,
            lighter_background = self.lighter_background,
            foreground = self.foreground,
            dark_foreground = self.dark_foreground,
            accent = self.accent,
        );
        vm.eval(ScriptMod {
            cargo_manifest_path: env!("CARGO_MANIFEST_DIR").to_string(),
            module_path: "mppdf_palette".to_string(),
            file: "palette.splash".to_string(),
            line: 0,
            column: 0,
            code,
            values: vec![],
        });
        for e in vm.take_errors() {
            log!("mppdf palette: {}", e);
        }
    }
}

/// `#rrggbb` (or `#rrggbbaa`) as a makepad `Vec4f`. Unparseable input reads
/// black, which is visible rather than silently theme-shaped.
pub fn vec4_of(hex: &str) -> Vec4f {
    let hex = hex.trim().trim_start_matches('#');
    if hex.len() < 6 {
        return Vec4f {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        };
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0) as f32 / 255.0;
    Vec4f {
        x: byte(0),
        y: byte(2),
        z: byte(4),
        w: if hex.len() >= 8 { byte(6) } else { 1.0 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_into_components() {
        let v = vec4_of("#ff8000");
        assert!((v.x - 1.0).abs() < 0.005);
        assert!((v.y - 0.502).abs() < 0.005);
        assert!((v.z - 0.0).abs() < 0.005);
        assert_eq!(v.w, 1.0);
        assert_eq!(vec4_of("7aa2f7").x, vec4_of("#7aa2f7").x);
        assert!((vec4_of("#00000080").w - 0.502).abs() < 0.005);
        // Garbage reads black rather than something theme-shaped.
        assert_eq!(vec4_of("nope").x, 0.0);
    }

    #[test]
    fn the_six_chrome_colors_are_distinct() {
        let p = Palette::tokyo_night();
        let all = [
            &p.background,
            &p.darker_background,
            &p.lighter_background,
            &p.foreground,
            &p.dark_foreground,
            &p.accent,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in all.iter().skip(i + 1) {
                assert_ne!(a, b, "chrome colors must not collapse");
            }
        }
    }
}
