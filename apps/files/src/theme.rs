//! Files chrome and disk-map colours come from the active system theme.

use makepad_widgets::*;

/// Every color the DSL reads, as `#rrggbbaa` strings.
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
    /// Row/tile selection from the system highlight role.
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
    /// It comes from the system error role.
    pub danger: String,
    /// The treemap's kind classes: video, image, audio, code, text, archive,
    /// other, each bound to a semantic system colour.
    pub kinds: [String; 7],
}

/// Semantic colours in the order of [`Palette::kinds`].
const KIND_COLOR_ROLES: [&str; 7] = [
    "color_info",            // video
    "color_success",         // image
    "color_warning",         // audio
    "color_info",            // code
    "color_text",            // text
    "color_tertiary",        // archive
    "color_bevel_outset_2",  // other
];

impl Palette {
    /// Read the owning VM on every application/style reload.
    pub fn for_vm(vm: &mut ScriptVm) -> Self {
        let mut hex = |role| {
            let color = theme_tokens::color(vm, role)
                .unwrap_or_else(|| panic!("missing system theme colour: {role}"));
            format!("#{:08x}", color.to_u32())
        };
        let accent = hex("color_focus");
        let bg = hex("color_bg_app");
        let bg_light = hex("color_fg_app");
        let fg = hex("color_text");
        let sel = hex("color_bg_highlight");
        let mut sel_soft = Self::vec4(&sel);
        sel_soft.w *= 77.0 / 255.0;
        Self {
            sel_soft: format!("#{:08x}", sel_soft.to_u32()),
            sel,
            hover: mix(&accent, &bg, 0.13),
            hover_soft: mix(&fg, &bg, 0.08),
            stripe: mix(&bg_light, &bg, 0.4),
            accent,
            bg,
            bg_dark: hex("color_bg_container"),
            bg_light,
            fg,
            fg_bright: hex("color_text_hover"),
            fg_dim: hex("color_text_disabled"),
            muted: hex("color_bevel_outset_2"),
            danger: hex("color_error"),
            kinds: KIND_COLOR_ROLES.map(&mut hex),
        }
    }

    pub fn for_cx(cx: &mut Cx) -> Self { cx.with_vm(Self::for_vm) }

    /// Colours the disk map paints with, matching this palette.
    pub fn map_palette(&self) -> makepad_diskmap::MapPalette {
        makepad_diskmap::MapPalette::from_hexes(
            &self.accent,
            &self.bg,
            &self.bg_dark,
            &self.fg,
            &self.fg_bright,
            &self.fg_dim,
            &self.muted,
            self.kinds.clone(),
        )
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
            w: hex.trim().trim_start_matches('#').get(6..8)
                .and_then(|a| u8::from_str_radix(a, 16).ok())
                .map_or(1.0, |a| a as f32 / 255.0),
        }
    }

    /// Bind the app's Splash roles to the installed system theme.
    pub fn publish(&self, vm: &mut ScriptVm) {
        vm.eval(script! {
            use mod.prelude.widgets.*
            mod.mpf = {
                accent: theme.color_focus
                bg: theme.color_bg_app
                bg_dark: theme.color_bg_container
                bg_light: theme.color_fg_app
                fg: theme.color_text
                fg_bright: theme.color_text_hover
                fg_dim: theme.color_text_disabled
                muted: theme.color_bevel_outset_2
                sel: theme.color_bg_highlight
                sel_soft: theme.color_bg_highlight * vec4(1.0, 1.0, 1.0, 0.3019608)
                hover: mix(theme.color_bg_app, theme.color_focus, 0.13)
                stripe: mix(theme.color_bg_app, theme.color_fg_app, 0.4)
            }
            true
        });
        for e in vm.take_errors() {
            log!("files theme: {}", e);
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

/// `a` over `b` at `t`, as a `#rrggbbaa` string.
fn mix(a: &str, b: &str, t: f64) -> String {
    let a = Palette::vec4(a);
    let b = Palette::vec4(b);
    let c = |x: f32, y: f32| ((x as f64 * t + y as f64 * (1.0 - t)) * 255.0).round().clamp(0.0, 255.0) as u8;
    format!("#{:02x}{:02x}{:02x}{:02x}", c(a.x, b.x), c(a.y, b.y), c(a.z, b.z), c(a.w, b.w))
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
            border_radius: uniform(theme.corner_radius)
            border_size: uniform(1.0)
            color: mod.theme.color_bg_app
            color_hover: uniform(mod.theme.color_bg_app)
            color_focus: uniform(mod.theme.color_bg_app)
            color_down: uniform(mod.theme.color_bg_app)
            color_empty: uniform(mod.theme.color_bg_app)
            color_disabled: uniform(mod.theme.color_bg_app)
            color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            border_color: uniform(mod.theme.color_bevel_outset_2)
            border_color_hover: uniform(mod.theme.color_bevel_outset_2)
            border_color_focus: uniform(mod.theme.color_focus)
            border_color_down: uniform(mod.theme.color_focus)
            border_color_empty: uniform(mod.theme.color_bevel_outset_2)
            border_color_disabled: uniform(mod.theme.color_bevel_outset_2)
            border_color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
        }
        draw_text +: {
            color: mod.theme.color_text_hover
            color_hover: uniform(mod.theme.color_text_hover)
            color_focus: uniform(mod.theme.color_text_hover)
            color_down: uniform(mod.theme.color_text_hover)
            color_disabled: uniform(mod.theme.color_text_disabled)
            color_empty: uniform(mod.theme.color_text_disabled)
            color_empty_hover: uniform(mod.theme.color_text_disabled)
            color_empty_focus: uniform(mod.theme.color_text_disabled)
            text_style: theme.font_regular{font_size: 9.5}
        }
        draw_cursor +: {color: uniform(mod.theme.color_focus)}
        draw_selection +: {
            border_radius: uniform(theme.textselection_corner_radius)
            color: uniform(mod.theme.color_bg_highlight)
            color_hover: uniform(mod.theme.color_bg_highlight)
            color_focus: uniform(mod.theme.color_bg_highlight)
            color_down: uniform(mod.theme.color_bg_highlight)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixes_toward_the_first_color() {
        assert_eq!(mix("#ffffff", "#000000", 0.0), "#000000ff");
        assert_eq!(mix("#ffffff", "#000000", 1.0), "#ffffffff");
        assert_eq!(mix("#ffffff", "#000000", 0.5), "#808080ff");
    }

    #[test]
    fn selection_follows_the_system_highlight() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(makepad_widgets::script_mod);
        cx.with_vm(|vm| {
            let p = Palette::for_vm(vm);
            assert_eq!(Palette::vec4(&p.sel), theme_tokens::color(vm, "color_bg_highlight").unwrap());
            p.publish(vm);
            let files = vm.module(id!(mpf));
            assert_eq!(vm.value(files, id!(sel)).as_color().map(Vec4f::from_u32), theme_tokens::color(vm, "color_bg_highlight"));
        });
    }

    #[test]
    fn every_treemap_kind_uses_its_system_role() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(makepad_widgets::script_mod);
        cx.with_vm(|vm| {
            let p = Palette::for_vm(vm);
            for (class, role) in KIND_COLOR_ROLES.iter().enumerate() {
                let expected = theme_tokens::color(vm, role).unwrap();
                assert_eq!(p.kind_color(class), expected);
            }
            assert_eq!(p.kind_color(99), p.kind_color(p.kinds.len() - 1));
        });
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
