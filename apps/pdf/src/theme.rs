//! Pdf chrome colours come from the active system theme.

use makepad_widgets::*;

/// Every color pdf paints, as `#rrggbbaa` strings.
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

impl Palette {
    /// Read the owning VM again on each application/style reload.
    pub fn for_vm(vm: &mut ScriptVm) -> Self {
        let mut hex = |role| {
            let color = theme_tokens::color(vm, role)
                .unwrap_or_else(|| panic!("missing system theme colour: {role}"));
            format!("#{:08x}", color.to_u32())
        };
        Self {
            background: hex("color_bg_app"),
            darker_background: hex("color_bg_container"),
            lighter_background: hex("color_fg_app"),
            foreground: hex("color_text"),
            dark_foreground: hex("color_text_disabled"),
            accent: hex("color_focus"),
        }
    }

    pub fn for_cx(cx: &mut Cx) -> Self { cx.with_vm(Self::for_vm) }

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

    /// Bind `mod.mpp` to the system theme before the app's modules.
    pub fn publish(&self, vm: &mut ScriptVm) {
        vm.eval(script! {
            use mod.prelude.widgets.*
            mod.mpp = {
                bg: theme.color_bg_app
                bg_dark: theme.color_bg_container
                bg_light: theme.color_fg_app
                fg: theme.color_text
                dim: theme.color_text_disabled
                accent: theme.color_focus
            }
            true
        });
        for e in vm.take_errors() {
            log!("pdf theme: {}", e);
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
    fn chrome_colors_follow_the_system_theme() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(makepad_widgets::script_mod);
        cx.with_vm(|vm| {
            let p = Palette::for_vm(vm);
            p.publish(vm);
            assert_eq!(vec4_of(&p.background), theme_tokens::color(vm, "color_bg_app").unwrap());
            assert_eq!(vec4_of(&p.darker_background), theme_tokens::color(vm, "color_bg_container").unwrap());
            assert_eq!(vec4_of(&p.lighter_background), theme_tokens::color(vm, "color_fg_app").unwrap());
            assert_eq!(vec4_of(&p.foreground), theme_tokens::color(vm, "color_text").unwrap());
            assert_eq!(vec4_of(&p.dark_foreground), theme_tokens::color(vm, "color_text_disabled").unwrap());
            assert_eq!(vec4_of(&p.accent), theme_tokens::color(vm, "color_focus").unwrap());
            let chrome = vm.module(id!(mpp));
            assert_eq!(vm.value(chrome, id!(bg)).as_color().map(Vec4f::from_u32), theme_tokens::color(vm, "color_bg_app"));
            assert_eq!(vm.value(chrome, id!(bg_dark)).as_color().map(Vec4f::from_u32), theme_tokens::color(vm, "color_bg_container"));
            assert_eq!(vm.value(chrome, id!(bg_light)).as_color().map(Vec4f::from_u32), theme_tokens::color(vm, "color_fg_app"));
            assert_eq!(vm.value(chrome, id!(fg)).as_color().map(Vec4f::from_u32), theme_tokens::color(vm, "color_text"));
            assert_eq!(vm.value(chrome, id!(dim)).as_color().map(Vec4f::from_u32), theme_tokens::color(vm, "color_text_disabled"));
            assert_eq!(vm.value(chrome, id!(accent)).as_color().map(Vec4f::from_u32), theme_tokens::color(vm, "color_focus"));
        });
    }
}
