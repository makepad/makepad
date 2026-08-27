//! The browser-chrome palette. Theming lives in splash: the chrome reads
//! `mod.mpb_theme.*` (tab strip, toolbar, omnibox, icon roles), which this
//! module evaluates into the VM before the UI modules.
//!
//! Under makepad-wm the roles come from the WM's theme.splash
//! (`MPWM_THEME_SPLASH`, line-scanned by `mp_theme`, the family bridge);
//! standalone runs get Chrome's own dark palette.

use makepad_widgets::*;

/// Chrome-dark roles, keyed like the mpwm theme so one mapping serves both.
#[derive(Clone, Debug)]
pub struct Palette {
    /// Tab strip background (the "frame").
    pub darker_background: String,
    /// Active tab + toolbar.
    pub background: String,
    /// Hovered inactive tab.
    pub dark_background: String,
    /// Button hover squares, omnibox focus fill.
    pub lighter_background: String,
    pub foreground: String,
    pub dark_foreground: String,
    pub bright_foreground: String,
    pub muted: String,
    pub selection: String,
    pub accent: String,
}

impl Palette {
    pub fn chrome_dark() -> Self {
        Self {
            darker_background: "#202124".into(),
            background: "#35363a".into(),
            dark_background: "#2b2c2f".into(),
            lighter_background: "#3c4043".into(),
            foreground: "#e8eaed".into(),
            dark_foreground: "#9aa0a6".into(),
            bright_foreground: "#ffffff".into(),
            muted: "#5f6368".into(),
            selection: "#264f78".into(),
            accent: "#8ab4f8".into(),
        }
    }

    /// The WM palette when mpwm exported one, else Chrome dark.
    pub fn current() -> Self {
        let fallback = Self::chrome_dark();
        let Some(p) = mp_theme::current() else {
            return fallback;
        };
        Self {
            darker_background: p.hex("darker_background", &fallback.darker_background),
            background: p.hex("background", &fallback.background),
            dark_background: p.hex("dark_background", &fallback.dark_background),
            lighter_background: p.hex("lighter_background", &fallback.lighter_background),
            foreground: p.hex("foreground", &fallback.foreground),
            dark_foreground: p.hex("dark_foreground", &fallback.dark_foreground),
            bright_foreground: p.hex("bright_foreground", &fallback.bright_foreground),
            muted: p.hex("muted", &fallback.muted),
            selection: p.hex("selection", &fallback.selection),
            accent: p.hex("accent", &fallback.accent),
        }
    }

    /// The `mod.mpb_theme = {...}` splash source. Runtime-evaluated, so plain
    /// `#hex` (the `#x` escape is a proc-macro-only hazard).
    pub fn splash_source(&self) -> String {
        format!(
            "mod.mpb_theme = {{\n\
             \x20   darker_background: {}\n\
             \x20   background: {}\n\
             \x20   dark_background: {}\n\
             \x20   lighter_background: {}\n\
             \x20   foreground: {}\n\
             \x20   dark_foreground: {}\n\
             \x20   bright_foreground: {}\n\
             \x20   muted: {}\n\
             \x20   selection: {}\n\
             \x20   accent: {}\n\
             }}\n\
             true\n",
            self.darker_background,
            self.background,
            self.dark_background,
            self.lighter_background,
            self.foreground,
            self.dark_foreground,
            self.bright_foreground,
            self.muted,
            self.selection,
            self.accent,
        )
    }

    /// Evaluate `mod.mpb_theme` into the VM. Call after
    /// `makepad_widgets::script_mod(vm)` and before the chrome modules.
    pub fn apply(&self, vm: &mut ScriptVm) {
        let script_mod_id = ScriptMod {
            cargo_manifest_path: env!("CARGO_MANIFEST_DIR").to_string(),
            module_path: "mpb_theme".to_string(),
            file: "mpb_theme.splash".to_string(),
            line: 0,
            column: 0,
            code: self.splash_source(),
            values: vec![],
        };
        vm.eval(script_mod_id);
        for e in vm.take_errors() {
            log!("mpbrowser theme: {}", e);
        }
    }

    /// The new-tab page: a data URL in the theme's colours, so a fresh tab
    /// never flashes white.
    pub fn new_tab_url(&self) -> String {
        let html = format!(
            "<!doctype html><html><head><meta charset=utf-8><title>New Tab</title>\
             <style>html,body{{margin:0;height:100%;background:{bg};color:{fg};\
             font:14px -apple-system,Helvetica,Arial,sans-serif}}\
             .c{{display:flex;height:100%;align-items:center;justify-content:center;\
             flex-direction:column;gap:10px}}.n{{font-size:28px;letter-spacing:1px;color:{fgb}}}\
             .s{{color:{fgd}}}</style></head><body><div class=c>\
             <div class=n>mpbrowser</div><div class=s>Type a URL or search in the box above</div>\
             </div></body></html>",
            bg = self.darker_background,
            fg = self.foreground,
            fgb = self.bright_foreground,
            fgd = self.dark_foreground,
        );
        format!("data:text/html;charset=utf-8,{}", percent_encode(&html))
    }
}

pub fn parse_hex(s: &str) -> Option<Vec4f> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(vec4(
        r as f32 / 255.0,
        g as f32 / 255.0,
        b as f32 / 255.0,
        1.0,
    ))
}

/// Minimal percent-encoding for a `data:` URL payload.
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// Is this new-tab-page URL (never shown in the omnibox)?
pub fn is_new_tab_url(url: &str) -> bool {
    url.starts_with("data:text/html;charset=utf-8,%3C%21doctype%20html%3E%3Chtml%3E%3Chead%3E%3Cmeta%20charset%3Dutf-8%3E%3Ctitle%3ENew%20Tab")
        || url == "about:blank"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_tab_url_is_recognised() {
        let p = Palette::chrome_dark();
        assert!(is_new_tab_url(&p.new_tab_url()));
        assert!(!is_new_tab_url("https://makepad.nl"));
    }

    #[test]
    fn hex_parses() {
        let c = parse_hex("#8ab4f8").unwrap();
        assert!((c.x - 0x8a as f32 / 255.0).abs() < 1e-6);
        assert!(parse_hex("#12345").is_none());
    }
}
