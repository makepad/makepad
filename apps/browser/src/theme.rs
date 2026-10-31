//! Browser chrome and CEF page colours follow the active system theme.

use makepad_widgets::*;

/// System theme colours used by the browser chrome and generated pages.
#[derive(Clone, Debug)]
pub struct Palette {
    /// Tab strip background (the "frame").
    pub darker_background: String,
    /// Active tab + toolbar.
    pub background: String,
    pub foreground: String,
    pub dark_foreground: String,
    pub bright_foreground: String,
    pub muted: String,
}

impl Palette {
    /// Read the owning VM after the system stylesheet is installed.
    pub fn for_vm(vm: &mut ScriptVm) -> Self {
        let mut hex = |role| {
            let color = theme_tokens::color(vm, role)
                .unwrap_or_else(|| panic!("missing system theme colour: {role}"));
            format!("#{:08x}", color.to_u32())
        };
        Self {
            darker_background: hex("color_bg_container"),
            background: hex("color_bg_app"),
            foreground: hex("color_text"),
            dark_foreground: hex("color_text_disabled"),
            bright_foreground: hex("color_text_hover"),
            muted: hex("color_bevel_outset_2"),
        }
    }

    /// Bind the browser's Splash roles directly to the installed theme.
    pub fn splash_source(&self) -> String {
        String::from("mod.browser_theme = {\n\
            darker_background: mod.theme.color_bg_container\n\
            background: mod.theme.color_bg_app\n\
            dark_background: mod.theme.color_bg_container\n\
            lighter_background: mod.theme.color_fg_app\n\
            foreground: mod.theme.color_text\n\
            dark_foreground: mod.theme.color_text_disabled\n\
            bright_foreground: mod.theme.color_text_hover\n\
            muted: mod.theme.color_bevel_outset_2\n\
            selection: mod.theme.color_bg_highlight\n\
            accent: mod.theme.color_focus\n\
            }\ntrue\n")
    }

    /// Evaluate `mod.browser_theme` into the VM. Call after
    /// `makepad_widgets::script_mod(vm)` and before the chrome modules.
    pub fn apply(&self, vm: &mut ScriptVm) {
        let script_mod_id = ScriptMod::new(env!("CARGO_MANIFEST_DIR"), "browser_theme", "browser_theme.splash", 0, 0, &self.splash_source(), &[]);
        vm.eval(script_mod_id);
        for e in vm.take_errors() {
            log!("browser theme: {}", e);
        }
    }

    /// What CEF fills the page with before the site has composited anything,
    /// and behind area a reflow has not reached yet. The same colour as the
    /// ground the page quad sits on, so neither a cold tab nor a mid-resize
    /// page shows a black hole.
    pub fn page_background_argb(&self) -> u32 {
        parse_argb(&self.darker_background).expect("a system theme colour")
    }

    pub fn is_dark(&self) -> bool {
        let c = parse_hex(&self.background).expect("a system theme colour");
        0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z < 0.5
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
             <div class=n>browser</div><div class=s>Type a URL or search in the box above</div>\
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
    let rgba = match s.len() {
        6 => (u32::from_str_radix(s, 16).ok()? << 8) | 0xff,
        8 => u32::from_str_radix(s, 16).ok()?,
        _ => return None,
    };
    Some(Vec4f::from_u32(rgba))
}

/// A theme role as opaque ARGB (`0xFFRRGGBB`) for CEF's page background.
pub fn parse_argb(s: &str) -> Option<u32> {
    Some(0xff00_0000 | (parse_hex(s)?.to_u32() >> 8))
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
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(makepad_widgets::script_mod);
        let p = cx.with_vm(Palette::for_vm);
        assert!(is_new_tab_url(&p.new_tab_url()));
        assert!(!is_new_tab_url("https://makepad.nl"));
    }

    #[test]
    fn hex_parses() {
        let c = parse_hex("#8ab4f8").unwrap();
        assert!((c.x - 0x8a as f32 / 255.0).abs() < 1e-6);
        assert!(parse_hex("#12345").is_none());
    }

    #[test]
    fn argb_is_opaque() {
        // CEF paints the page on this before the site composites; a
        // transparent (or zero) colour is what makes a page open black.
        assert_eq!(parse_argb("#202124"), Some(0xff20_2124));
        assert_eq!(parse_argb("202124"), Some(0xff20_2124));
        assert_eq!(parse_argb("#nope"), None);
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(makepad_widgets::script_mod);
        cx.with_vm(|vm| {
            let p = Palette::for_vm(vm);
            let ground = theme_tokens::color(vm, "color_bg_container").unwrap();
            assert_eq!(p.page_background_argb(), 0xff00_0000 | (ground.to_u32() >> 8),
                "the CEF fill must match the themed ground the page sits on");
        });
    }
}
