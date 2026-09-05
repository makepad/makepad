//! The desk's palette. wm exports its active `theme.splash` as
//! MAKEPAD_WM_THEME_SPLASH; `makepad_wm_theme` line-scans it and retints the *stock*
//! widgets, and this module carries the same colours to the three places the
//! mixer paints itself:
//!
//!   * `mod.mpm.*` — read by `main.rs`'s own `script_mod!` (the window and
//!     the search page);
//!   * a one-line `let mp = {...}` preamble prepended to every surface layout
//!     — a `Splash` body runs in its OWN isolate VM, which registers a FRESH
//!     stock `mod.theme` and never sees `mod.mpm`, so a layout has to be
//!     handed its colours (see [`Palette::splash_preamble`]);
//!   * [`Palette::rgb3`], for the one colour `surface.rs` sets from Rust.
//!
//! It also applies the stock retint with these same values, because
//! `makepad_wm_theme::apply` is a no-op when the WM is not running and that would
//! otherwise leave the caption bar in the neutral stock theme above a black
//! desk. Standalone runs get Tokyo Night, so the surface is dark and square
//! either way.
//!
//! SAFETY: colours only. Nothing here can name an OSC address — see
//! `makepad_mixer::safety`.

use makepad_widgets::*;
use std::sync::OnceLock;

/// A palette entry: the name the DSL reads it by, the key in the WM's
/// theme.splash, and the Tokyo Night fallback.
///
/// The meter and lamp hues live in the theme's terminal block — omarchy's
/// base16 mapping is colour1 = red, 2 = green, 3 = yellow, 6 = cyan — because
/// a desktop theme has no "signal is clipping" role of its own.
const KEYS: &[(&str, &str, &str)] = &[
    ("bg", "background", "#1a1b26"),
    ("bg_dark", "darker_background", "#0e0e14"),
    ("bg_light", "lighter_background", "#24283b"),
    ("fg", "foreground", "#a9b1d6"),
    ("fg_bright", "bright_foreground", "#c0caf5"),
    ("fg_dim", "dark_foreground", "#565f89"),
    ("muted", "muted", "#414868"),
    ("accent", "accent", "#7aa2f7"),
    ("red", "term.color1", "#f7768e"),
    ("green", "term.color2", "#9ece6a"),
    ("yellow", "term.color3", "#e0af68"),
    ("cyan", "term.color6", "#449dab"),
];

/// Every colour the mixer paints with, as `#rrggbb` strings.
#[derive(Clone, Debug)]
pub struct Palette {
    /// `name` -> `#rrggbb`, in [`KEYS`] order.
    entries: Vec<(&'static str, String)>,
}

impl Palette {
    /// The palette for this process, read once.
    pub fn shared() -> &'static Palette {
        static PALETTE: OnceLock<Palette> = OnceLock::new();
        PALETTE.get_or_init(Palette::load)
    }

    /// The palette wm exported for this process, with Tokyo Night standing
    /// in for anything it does not name.
    pub fn load() -> Self {
        let wm = makepad_wm_theme::current();
        Palette {
            entries: KEYS
                .iter()
                .map(|(name, key, fallback)| {
                    let hex = match &wm {
                        Some(p) => p.hex(key, fallback),
                        None => fallback.to_string(),
                    };
                    (*name, hex)
                })
                .collect(),
        }
    }

    /// One colour by its DSL name. Unknown names read magenta rather than
    /// silently theme-shaped black.
    pub fn get(&self, name: &str) -> &str {
        self.entries
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.as_str())
            .unwrap_or("#ff00ff")
    }

    /// One colour as linear-ish rgb components, for the handful of places
    /// Rust sets a colour directly.
    pub fn rgb3(&self, name: &str) -> [f32; 3] {
        let hex = self.get(name).trim_start_matches('#');
        let nib = |i: usize| -> f32 {
            match hex.as_bytes().get(i).copied().unwrap_or(b'0') {
                c @ b'0'..=b'9' => (c - b'0') as f32,
                c @ b'a'..=b'f' => (c - b'a' + 10) as f32,
                c @ b'A'..=b'F' => (c - b'A' + 10) as f32,
                _ => 0.0,
            }
        };
        let byte = |i: usize| (nib(i * 2) * 16.0 + nib(i * 2 + 1)) / 255.0;
        [byte(0), byte(1), byte(2)]
    }

    /// The palette as ONE line of splash source, to be prepended to a layout
    /// body before it is handed to the `Splash` widget. One line so a script
    /// error's reported line is the layout's own line plus exactly one.
    pub fn splash_preamble(&self) -> String {
        let body: Vec<String> = self
            .entries
            .iter()
            .map(|(name, hex)| format!("{name}: {hex}"))
            .collect();
        format!("let mp = {{{}}}\n", body.join(", "))
    }

    /// Publish `mod.mpm.*` for the app's own `script_mod!`, and retint the
    /// stock widgets (the caption bar, the spinner) with the same palette.
    /// Call once, after `makepad_widgets::script_mod` and `makepad_wm_theme::apply`,
    /// and before this crate's own `script_mod`.
    pub fn install(&self, vm: &mut ScriptVm) {
        let mut code = String::from("mod.mpm = {\n");
        for (name, hex) in &self.entries {
            code.push_str(&format!("    {name}: {hex}\n"));
        }
        code.push_str("}\n");

        // `makepad_wm_theme::apply` only retints when the WM exported a palette;
        // standalone that leaves the window chrome in the stock theme, which
        // reads as a grey band above a Tokyo Night desk. Same keys, our
        // fallbacks.
        let c = |name: &str| self.get(name).to_string();
        code.push_str(&format!(
            "mod.theme.color_b = {bg_dark}\n\
             mod.theme.color_b_h = {bg_dark}00\n\
             mod.theme.color_w = {fg_bright}\n\
             mod.theme.color_w_h = {fg_bright}00\n\
             mod.theme.color_bg_app = {bg}\n\
             mod.theme.color_fg_app = {bg_light}\n\
             mod.theme.color_bg_container = {bg_dark}\n\
             mod.theme.color_text = {fg}\n\
             mod.theme.color_text_hover = {fg_bright}\n\
             mod.theme.color_text_muted = {fg_dim}\n\
             mod.theme.color_focus = {accent}\n\
             mod.theme.color_outset_active = {accent}\n\
             mod.theme.color_ctrl_default = {bg_light}\n\
             mod.theme.color_ctrl_hover = {muted}\n\
             mod.theme.color_ctrl_active = {accent}\n\
             mod.theme.color_ctrl_selected = {accent}\n\
             mod.theme.color_app_caption_bar = {bg_dark}\n\
             mod.theme.corner_radius = 0.0\n\
             true\n",
            bg = c("bg"),
            bg_dark = c("bg_dark"),
            bg_light = c("bg_light"),
            fg = c("fg"),
            fg_bright = c("fg_bright"),
            fg_dim = c("fg_dim"),
            muted = c("muted"),
            accent = c("accent"),
        ));

        vm.eval(ScriptMod {
            cargo_manifest_path: env!("CARGO_MANIFEST_DIR").to_string(),
            module_path: "mixer_theme".to_string(),
            file: "mixer_theme.splash".to_string(),
            line: 0,
            column: 0,
            code,
            values: vec![],
        });
        for e in vm.take_errors() {
            log!("mixer theme: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standalone_falls_back_to_tokyo_night() {
        let p = Palette::load();
        // The WM is not running under `cargo test`, so every key is a
        // fallback — and the fallbacks ARE wm's bundled tokyo-night.
        assert_eq!(p.get("bg"), "#1a1b26");
        assert_eq!(p.get("bg_dark"), "#0e0e14");
        assert_eq!(p.get("accent"), "#7aa2f7");
        assert_eq!(p.get("red"), "#f7768e");
        assert_eq!(p.get("green"), "#9ece6a");
        assert_eq!(p.get("yellow"), "#e0af68");
        assert_eq!(p.get("cyan"), "#449dab");
        // Nothing reads as neutral grey: every channel pair differs.
        for (name, hex) in &p.entries {
            let [r, g, b] = p.rgb3(name);
            assert!(
                (r - g).abs() > 0.001 || (g - b).abs() > 0.001,
                "{name} = {hex} is a neutral grey"
            );
        }
    }

    #[test]
    fn a_wm_palette_wins_over_every_fallback() {
        // Both the top-level roles and the terminal hues come from the WM's
        // own theme.splash when it is running.
        let src = "mod.wm_theme = {\n    accent: #ff8800\n    background: #101010\n    term: {\n        color1: #123456\n    }\n}\n";
        let wm = makepad_wm_theme::scan(src);
        assert_eq!(wm.hex("accent", "#7aa2f7"), "#ff8800");
        assert_eq!(wm.hex("term.color1", "#f7768e"), "#123456");
        // ...and a key the theme omits still resolves.
        assert_eq!(wm.hex("term.color6", "#449dab"), "#449dab");
    }

    #[test]
    fn the_layout_preamble_is_exactly_one_line() {
        let p = Palette::load();
        let pre = p.splash_preamble();
        assert_eq!(pre.lines().count(), 1, "preamble must stay one line: {pre}");
        assert!(pre.ends_with('\n'));
        assert!(pre.starts_with("let mp = {"));
        // Every colour a layout can ask for is bound.
        for (name, hex) in &p.entries {
            assert!(pre.contains(&format!("{name}: {hex}")), "missing {name}");
        }
    }

    #[test]
    fn parses_hex() {
        let p = Palette::load();
        let [r, g, b] = p.rgb3("bg_dark"); // #0e0e14
        assert!((r - 14.0 / 255.0).abs() < 1e-6);
        assert!((g - 14.0 / 255.0).abs() < 1e-6);
        assert!((b - 20.0 / 255.0).abs() < 1e-6);
        assert_eq!(p.get("nope"), "#ff00ff");
    }
}
