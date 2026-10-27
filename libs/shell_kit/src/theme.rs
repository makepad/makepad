//! The default shell theme: the bundled Tokyo Night `mod.wm_theme` source and the
//! `mod.wm_theme.shell` block resolved from a theme's palette, plus the colour and
//! border readers they need. The WM evaluates a user's theme the same way; an app
//! without the WM calls [`eval_default_theme`].

use std::fmt::Write as _;

/// The alpha omarchy's templates give every border stop: `rgba(...ee)`.
const BORDER_ALPHA: f64 = 0.933;

/// The focused border a theme gets when it names none. Omarchy's
/// `default/themed/hyprland.lua.tpl` reads
/// `active_border = {{ hypr_gradient hyprland_active_border accent }}`.
/// A theme that names its own gradient (hackerman, last-horizon,
/// solitude) still wins; this is what every other theme gets, and it is
/// hyprland's OWN default, which omarchy keeps in
/// `default/hypr/looknfeel.lua:3-4`:
/// `active_border_color = { colors = { "rgba(33ccffee)", "rgba(00ff99ee)" }, angle = 45 }`
/// — the cyan→green diagonal ring the reference desktop shows.
pub const DEFAULT_ACTIVE_BORDER: Gradient = Gradient {
    start: Stop {
        rgb: Rgb {
            r: 0x33,
            g: 0xcc,
            b: 0xff,
        },
        alpha: BORDER_ALPHA,
    },
    end: Stop {
        rgb: Rgb {
            r: 0x00,
            g: 0xff,
            b: 0x99,
        },
        alpha: BORDER_ALPHA,
    },
    angle: 45.0,
};

/// Hyprland's `rgba(595959aa)`, lifted from 0.667 to 0.85 alpha: omarchy
/// composites it over a lit wallpaper, our desk is darker and 0.667 sinks
/// the edge into the background. Themes tune it with `inactive_border` and
/// `inactive_border_alpha`.
pub const DEFAULT_INACTIVE_BORDER: Stop = Stop {
    rgb: Rgb {
        r: 0x59,
        g: 0x59,
        b: 0x59,
    },
    alpha: 0.85,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// Hex form for RUNTIME-evaluated splash sources. Plain `#hex`: the
    /// digit-then-`e` exponent hazard only exists in the proc-macro path
    /// (Rust's tokenizer); the runtime script tokenizer takes pure hex and
    /// does not know the `#` escape.
    pub fn splash_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

pub fn parse_hex(s: &str) -> Option<Rgb> {
    let s = s.trim().trim_start_matches("#").trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    Some(Rgb {
        r: u8::from_str_radix(&s[0..2], 16).ok()?,
        g: u8::from_str_radix(&s[2..4], 16).ok()?,
        b: u8::from_str_radix(&s[4..6], 16).ok()?,
    })
}

/// One color stop of a border: an rgb triple plus its own alpha.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stop {
    pub rgb: Rgb,
    pub alpha: f64,
}

/// A hyprland border value: two stops and an angle in degrees. A solid
/// color is the degenerate gradient (both stops equal).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gradient {
    pub start: Stop,
    pub end: Stop,
    pub angle: f64,
}

impl Gradient {
    pub fn solid(rgb: Rgb, alpha: f64) -> Self {
        let stop = Stop { rgb, alpha };
        Self {
            start: stop,
            end: stop,
            angle: 0.0,
        }
    }

    pub fn is_solid(&self) -> bool {
        self.start == self.end
    }
}

/// A hyprland color literal: `rgba(RRGGBBAA)`, `rgb(RRGGBB)`, `0xAARRGGBB`
/// or a plain `#rrggbb` / `#rrggbbaa`.
pub fn parse_hypr_color(s: &str) -> Option<Stop> {
    let s = s.trim();
    let hex = |body: &str, has_alpha: bool, alpha_first: bool| -> Option<Stop> {
        let want = if has_alpha { 8 } else { 6 };
        if body.len() != want || !body.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let (a, rgb) = if !has_alpha {
            (255u8, body)
        } else if alpha_first {
            (u8::from_str_radix(&body[0..2], 16).ok()?, &body[2..8])
        } else {
            (u8::from_str_radix(&body[6..8], 16).ok()?, &body[0..6])
        };
        Some(Stop {
            rgb: parse_hex(rgb)?,
            alpha: a as f64 / 255.0,
        })
    };
    if let Some(body) = s.strip_prefix("rgba(").and_then(|s| s.strip_suffix(')')) {
        return hex(body.trim(), true, false);
    }
    if let Some(body) = s.strip_prefix("rgb(").and_then(|s| s.strip_suffix(')')) {
        return hex(body.trim(), false, false);
    }
    if let Some(body) = s.strip_prefix("0x") {
        return hex(body.trim(), true, true);
    }
    let body = s.trim_start_matches('#');
    hex(body, body.len() == 8, false)
}

/// Fallback terminal palette values, read from a theme.splash source by
/// the WM when it hands a palette to a child terminal. This is a plain
/// line scan over the generated shape — themes edited into fancier splash
/// still work for the CHROME (full evaluation); only the term block needs
/// to keep `colorN: #hex` lines scannable.
pub fn scan_term_palette(splash_src: &str) -> Option<TermPalette> {
    let mut base16 = [Rgb { r: 0, g: 0, b: 0 }; 16];
    let mut seen = 0u32;
    let mut foreground = None;
    let mut background = None;
    let mut cursor = None;
    let mut in_term = false;
    for line in splash_src.lines() {
        let line = line.trim();
        if line.starts_with("term:") {
            in_term = true;
            continue;
        }
        if !in_term {
            continue;
        }
        if line.starts_with('}') {
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim().trim_end_matches(',');
        if let Some(idx) = key.strip_prefix("color") {
            if let (Ok(i), Some(rgb)) = (idx.parse::<usize>(), parse_hex(value)) {
                if i < 16 {
                    base16[i] = rgb;
                    seen |= 1 << i;
                }
            }
        } else if key == "foreground" {
            foreground = parse_hex(value);
        } else if key == "background" {
            background = parse_hex(value);
        } else if key == "cursor" {
            cursor = parse_hex(value);
        }
    }
    if seen != 0xffff {
        return None;
    }
    Some(TermPalette {
        base16,
        foreground: foreground?,
        background: background?,
        cursor,
    })
}

/// Read the border keys out of a theme.splash source: `active_border`,
/// `active_border_end`, `active_border_angle`, `active_border_alpha`,
/// `inactive_border`, `inactive_border_alpha`. A line scan over the shape
/// `splash_source` emits; anything missing falls back to the omarchy
/// default, so hand-written and older themes still draw.
pub fn scan_borders(splash_src: &str) -> (Gradient, Stop) {
    let mut active = DEFAULT_ACTIVE_BORDER;
    let mut inactive = DEFAULT_INACTIVE_BORDER;
    let mut start: Option<Stop> = None;
    let mut end: Option<Stop> = None;
    let mut active_alpha: Option<f64> = None;
    let mut inactive_alpha: Option<f64> = None;
    for line in splash_src.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().trim_end_matches(',');
        match key.trim() {
            "active_border" => start = parse_hypr_color(value),
            "active_border_end" => end = parse_hypr_color(value),
            "active_border_angle" => {
                if let Ok(v) = value.parse::<f64>() {
                    active.angle = v;
                }
            }
            "active_border_alpha" => active_alpha = value.parse::<f64>().ok(),
            "inactive_border" => {
                if let Some(stop) = parse_hypr_color(value) {
                    inactive = stop;
                }
            }
            "inactive_border_alpha" => inactive_alpha = value.parse::<f64>().ok(),
            _ => {}
        }
    }
    if let Some(start) = start {
        active.start = start;
        // One stop only: a solid border, not a gradient into the default.
        active.end = end.unwrap_or(start);
    } else if let Some(end) = end {
        active.end = end;
    }
    if let Some(alpha) = active_alpha {
        active.start.alpha = alpha;
        active.end.alpha = alpha;
    }
    if let Some(alpha) = inactive_alpha {
        inactive.alpha = alpha;
    }
    (active, inactive)
}

#[derive(Clone, Debug)]
pub struct TermPalette {
    pub base16: [Rgb; 16],
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Option<Rgb>,
}

impl TermPalette {
    /// The MAKEPAD_TERMINAL_COLORS env value handed to child terminals.
    pub fn env_value(&self) -> String {
        let mut s = String::new();
        for (i, c) in self.base16.iter().enumerate() {
            let _ = write!(s, "color{}={};", i, c.hex());
        }
        let _ = write!(s, "foreground={};", self.foreground.hex());
        let _ = write!(s, "background={};", self.background.hex());
        if let Some(c) = self.cursor {
            let _ = write!(s, "cursor={};", c.hex());
        }
        s
    }
}

/// Omarchy's default ("Tokyo Night", seeded by install/user/theme.sh),
/// pre-imported to splash so first launch needs no network.
pub const BUNDLED_TOKYO_NIGHT_SPLASH: &str = r##"// wm theme "tokyo-night" — imported from the omarchy theme of the same
// name; this splash file IS the theme now. Edit freely. (format 3)
mod.wm_theme = {
    accent: #7aa2f7
    selection: #292e42
    muted: #414868
    background: #1a1b26
    dark_background: #13141c
    darker_background: #0e0e14
    lighter_background: #24283b
    foreground: #a9b1d6
    dark_foreground: #565f89
    light_foreground: #b4bee6
    bright_foreground: #c0caf5
    cursor: #c0caf5
    active_border: #33ccff
    active_border_end: #00ff99
    active_border_angle: 45.0
    active_border_alpha: 0.933
    inactive_border: #595959
    inactive_border_alpha: 0.850
    light_mode: false
    term: {
        color0: #1a1b26
        color1: #f7768e
        color2: #9ece6a
        color3: #e0af68
        color4: #7aa2f7
        color5: #ad8ee6
        color6: #449dab
        color7: #a9b1d6
        color8: #414868
        color9: #ff7a93
        color10: #b9f27c
        color11: #ff9e64
        color12: #7da6ff
        color13: #bb9af7
        color14: #0db9d7
        color15: #c0caf5
        foreground: #a9b1d6
        background: #1a1b26
        cursor: #c0caf5
        selection: #292e42
    }
}
"##;

/// Read one `key: #rrggbb` out of a theme.splash source (top level).
fn scan_rgb(source: &str, key: &str) -> Option<Rgb> {
    for line in source.lines() {
        let line = line.trim();
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        if k.trim() == key {
            return parse_hex(v.trim().trim_end_matches(','));
        }
    }
    None
}

/// `mod.wm_theme.shell = { ... }`, resolved from the theme's own palette
/// exactly the way `bin/omarchy-theme-set-templates` fills
/// `default/themed/shell.toml.tpl`:
///
/// * `{{ background }}` / `{{ foreground }}` / `{{ accent }}` / `{{ red }}`
///   come from the palette,
/// * `hyprland.active-border` is `shell_gradient hyprland_active_border
///   accent` — the theme's border gradient, or its accent when it names
///   none,
/// * `hyprland.active-border-foreground` is the same gradient with
///   `foreground` as the fallback (which is what a theme with no gradient
///   gets on the menu, launcher and tooltip),
/// * every size and alpha is the `.tpl` default.
///
/// Sizes are px and alphas 0..1, like the `.tpl`; the shell reads this and
/// nothing else at runtime.
pub fn shell_splash_block(source: &str) -> String {
    let foreground = scan_rgb(source, "foreground").unwrap_or(Rgb {
        r: 0xca,
        g: 0xcc,
        b: 0xcc,
    });
    let background = scan_rgb(source, "background").unwrap_or(Rgb {
        r: 0x10,
        g: 0x13,
        b: 0x15,
    });
    let accent = scan_rgb(source, "accent").unwrap_or(foreground);
    // `{{ red }}`: the palette's red, which the terminal block carries as
    // color1 (`urgent` in Color.qml's vocabulary).
    let red = scan_term_palette(source)
        .map(|p| p.base16[1])
        .unwrap_or(Rgb {
            r: 0xa5,
            g: 0x55,
            b: 0x55,
        });

    let (active, _inactive) = scan_borders(source);
    // A theme that names no gradient got `accent, solid` from the import,
    // so the `-foreground` variant of the token falls back to foreground.
    let named_gradient = !(active.is_solid() && active.start.rgb == accent);
    let (fg_border_start, fg_border_end, fg_border_angle) = if named_gradient {
        (active.start.rgb, active.end.rgb, active.angle)
    } else {
        (foreground, foreground, 0.0)
    };
    let (ac_border_start, ac_border_end, ac_border_angle) =
        (active.start.rgb, active.end.rgb, active.angle);

    let h = |c: Rgb| c.splash_hex();
    let mut s = String::new();
    let _ = writeln!(s, "mod.wm_theme.shell = {{");
    let _ = writeln!(s, "    corner_radius: 0.0");
    let _ = writeln!(s, "    bar: {{");
    let _ = writeln!(s, "        background: {}", h(background));
    let _ = writeln!(s, "        background_alpha: 1.0");
    let _ = writeln!(s, "        text: {}", h(foreground));
    let _ = writeln!(s, "        active: {}", h(red));
    let _ = writeln!(s, "        size_horizontal: 26.0");
    let _ = writeln!(s, "        size_vertical: 28.0");
    let _ = writeln!(s, "        icon_slot: 27.0");
    let _ = writeln!(s, "        icon_canvas: 16.0");
    let _ = writeln!(s, "        icon_font: 13.0");
    let _ = writeln!(s, "        status_slot: 21.0");
    let _ = writeln!(s, "    }}");

    /// One surface section: the card colors plus its border, which may be
    /// the accent-fallback gradient or the foreground-fallback one.
    #[allow(clippy::too_many_arguments)]
    fn surface(
        s: &mut String,
        name: &str,
        bg_alpha: &str,
        border_width: &str,
        background: Rgb,
        foreground: Rgb,
        border: (Rgb, Rgb, f64),
    ) {
        let _ = writeln!(s, "    {}: {{", name);
        let _ = writeln!(s, "        background: {}", background.splash_hex());
        let _ = writeln!(s, "        background_alpha: {}", bg_alpha);
        let _ = writeln!(s, "        text: {}", foreground.splash_hex());
        let _ = writeln!(s, "        border: {}", border.0.splash_hex());
        let _ = writeln!(s, "        border_end: {}", border.1.splash_hex());
        let _ = writeln!(s, "        border_angle: {:.1}", border.2);
        let _ = writeln!(s, "        border_alpha: 1.0");
        let _ = writeln!(s, "        border_width: {}", border_width);
    }
    let accent_border = (ac_border_start, ac_border_end, ac_border_angle);
    let fg_border = (fg_border_start, fg_border_end, fg_border_angle);

    surface(
        &mut s, "popups", "1.0", "2.0", background, foreground, accent_border,
    );
    let _ = writeln!(s, "    }}");
    surface(
        &mut s, "tooltip", "0.97", "1.0", background, foreground, fg_border,
    );
    let _ = writeln!(s, "    }}");
    surface(
        &mut s,
        "notifications",
        "1.0",
        "2.0",
        background,
        foreground,
        accent_border,
    );
    let _ = writeln!(s, "        countdown: {}", h(accent));
    let _ = writeln!(s, "    }}");

    for (name, card_alpha) in [("menu", "1.0"), ("launcher", "0.95")] {
        surface(
            &mut s, name, card_alpha, "2.0", background, foreground, fg_border,
        );
        let _ = writeln!(s, "        scrim: {}", h(background));
        let _ = writeln!(s, "        scrim_alpha: 0.5");
        let _ = writeln!(s, "        selected_background: {}", h(foreground));
        let _ = writeln!(s, "        selected_background_alpha: 0.08");
        let _ = writeln!(s, "        selected_text: {}", h(accent));
        let _ = writeln!(s, "        selected_border: {}", h(fg_border_start));
        let _ = writeln!(s, "        selected_border_alpha: 0.25");
        let _ = writeln!(s, "    }}");
    }

    let _ = writeln!(s, "    controls: {{");
    for state in ["normal", "hover", "focus", "selected"] {
        let (fill, border_a, width) = match state {
            "normal" => ("0.04", "0.4", "1.0"),
            "hover" | "focus" => ("0.08", "0.25", "1.0"),
            _ => ("0.18", "1.0", "0.0"),
        };
        let _ = writeln!(s, "        {}_color: {}", state, h(foreground));
        let _ = writeln!(s, "        {}_fill_alpha: {}", state, fill);
        let _ = writeln!(s, "        {}_border: {}", state, h(foreground));
        let _ = writeln!(s, "        {}_border_width: {}", state, width);
        let _ = writeln!(s, "        {}_border_alpha: {}", state, border_a);
    }
    let _ = writeln!(s, "        pressed_fill_alpha: 0.22");
    let _ = writeln!(s, "        selection_fill_alpha: 0.35");
    let _ = writeln!(s, "    }}");

    let _ = writeln!(s, "    spacing: {{");
    for (k, v) in [
        ("xxs", 2.0),
        ("xs", 3.0),
        ("sm", 4.0),
        ("md", 6.0),
        ("lg", 8.0),
        ("xl", 10.0),
        ("xxl", 12.0),
        ("xxxl", 14.0),
        ("huge", 18.0),
        ("control_gap", 8.0),
        ("control_padding_x", 10.0),
        ("control_padding_y", 6.0),
        ("input_padding_y", 7.0),
        ("control_height", 28.0),
        ("popup_row_height", 28.0),
        ("dropdown_width", 240.0),
        ("searchable_dropdown_width", 260.0),
        ("number_field_width", 120.0),
        ("searchable_popup_min_height", 220.0),
        ("row_gap", 8.0),
        ("row_padding_x", 12.0),
        ("label_gap", 4.0),
        ("panel_gap", 14.0),
        ("panel_padding", 18.0),
        ("popup_padding", 14.0),
        ("gaps_out", 5.0),
    ] {
        let _ = writeln!(s, "        {}: {:.1}", k, v);
    }
    let _ = writeln!(s, "    }}");

    let _ = writeln!(s, "    font: {{");
    for (k, v) in [
        ("base_size", 12.0),
        ("caption", 10.0),
        ("body_small", 11.0),
        ("body", 12.0),
        ("subtitle", 13.0),
        ("title", 14.0),
        ("heading", 16.0),
        ("display", 24.0),
        ("display_large", 28.0),
        ("icon_small", 11.0),
        ("icon", 14.0),
        ("icon_large", 18.0),
    ] {
        let _ = writeln!(s, "        {}: {:.1}", k, v);
    }
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_term_palette_from_splash() {
        let p = scan_term_palette(BUNDLED_TOKYO_NIGHT_SPLASH).unwrap();
        assert_eq!(p.base16[0].hex(), "#1a1b26");
        assert_eq!(p.base16[14].hex(), "#0db9d7");
        assert_eq!(p.foreground.hex(), "#a9b1d6");
        assert_eq!(p.cursor.unwrap().hex(), "#c0caf5");
        let env = p.env_value();
        assert!(env.contains("color9=#ff7a93;"));
        assert!(env.contains("background=#1a1b26;"));
    }

    #[test]
    fn scan_borders_reads_the_emitted_shape() {
        let (active, inactive) = scan_borders(BUNDLED_TOKYO_NIGHT_SPLASH);
        assert_eq!(active.start.rgb.hex(), "#33ccff");
        assert_eq!(active.end.rgb.hex(), "#00ff99");
        assert!(!active.is_solid());
        assert_eq!(active.angle, 45.0);
        assert!((active.start.alpha - 0.933).abs() < 0.001);
        assert_eq!(inactive.rgb.hex(), "#595959");
        assert!((inactive.alpha - 0.85).abs() < 0.001);
        // A theme that only names one stop draws a SOLID border.
        let solid = scan_borders("mod.wm_theme = {\n    active_border: #7aa2f7\n}\n");
        assert!(solid.0.is_solid());
        assert_eq!(solid.0.start.rgb.hex(), "#7aa2f7");
        // Nothing at all: the omarchy default.
        assert_eq!(scan_borders("").0, DEFAULT_ACTIVE_BORDER);
        assert_eq!(scan_borders("").1, DEFAULT_INACTIVE_BORDER);
    }

    /// A theme.splash the shell-token tests own, so they do not move when
    /// the bundled theme's border defaults do.
    const SHELL_SRC: &str = r##"mod.wm_theme = {
    accent: #7aa2f7
    background: #1a1b26
    foreground: #a9b1d6
    active_border: #7aa2f7
    active_border_end: #7aa2f7
    active_border_angle: 0.0
    term: {
        color0: #1a1b26
        color1: #f7768e
        color2: #9ece6a
        color3: #e0af68
        color4: #7aa2f7
        color5: #ad8ee6
        color6: #449dab
        color7: #a9b1d6
        color8: #414868
        color9: #ff7a93
        color10: #b9f27c
        color11: #ff9e64
        color12: #7da6ff
        color13: #bb9af7
        color14: #0db9d7
        color15: #c0caf5
        foreground: #a9b1d6
        background: #1a1b26
        cursor: #c0caf5
        selection: #292e42
    }
}
"##;

    #[test]
    fn the_shell_block_resolves_the_tpl_contract() {
        let block = shell_splash_block(SHELL_SRC);
        assert!(block.starts_with("mod.wm_theme.shell = {"));
        // [bar]: background at α 1.0, text = foreground, active = red.
        assert!(block.contains("        background: #1a1b26\n"));
        assert!(block.contains("        text: #a9b1d6\n"));
        assert!(block.contains("        active: #f7768e\n"));
        assert!(block.contains("        size_horizontal: 26.0\n"));
        // [launcher] is the translucent card, [menu] the opaque one.
        assert!(block.contains("    launcher: {\n"));
        assert!(block.contains("        background_alpha: 0.95\n"));
        assert!(block.contains("        scrim_alpha: 0.5\n"));
        assert!(block.contains("        selected_background_alpha: 0.08\n"));
        // [tooltip] keeps its legacy 0.97.
        assert!(block.contains("        background_alpha: 0.97\n"));
        // [controls]: the shared state alphas.
        assert!(block.contains("        normal_fill_alpha: 0.04\n"));
        assert!(block.contains("        hover_fill_alpha: 0.08\n"));
        assert!(block.contains("        selected_fill_alpha: 0.18\n"));
        assert!(block.contains("        pressed_fill_alpha: 0.22\n"));
        // The spacing and type scales.
        assert!(block.contains("        control_height: 28.0\n"));
        assert!(block.contains("        panel_padding: 18.0\n"));
        assert!(block.contains("        display_large: 28.0\n"));
    }

    #[test]
    fn the_shell_border_falls_back_the_way_shell_gradient_does() {
        // A theme that names no gradient: popups take the accent, and the
        // `-foreground` surfaces (menu, launcher, tooltip) take foreground.
        let block = shell_splash_block(SHELL_SRC);
        let section = |name: &str| -> String {
            let start = block.find(&format!("    {}: {{\n", name)).unwrap();
            let end = block[start..].find("\n    }").unwrap();
            block[start..start + end].to_string()
        };
        assert!(section("popups").contains("border: #7aa2f7"));
        assert!(section("menu").contains("border: #a9b1d6"));
        assert!(section("tooltip").contains("border: #a9b1d6"));
        // A theme with a real gradient hands both stops and the angle to
        // every surface that references the hyprland token.
        let themed = SHELL_SRC.replace(
            "    active_border_end: #7aa2f7",
            "    active_border_end: #2ec27e",
        );
        let block = shell_splash_block(&themed);
        assert!(block.contains("        border_end: #2ec27e\n"));
    }

    #[test]
    fn hypr_color_forms() {
        assert_eq!(
            parse_hypr_color("rgba(33ccffee)").unwrap().rgb.hex(),
            "#33ccff"
        );
        assert!((parse_hypr_color("rgba(33ccffee)").unwrap().alpha - 0.933).abs() < 0.002);
        assert_eq!(parse_hypr_color("rgb(00ff99)").unwrap().alpha, 1.0);
        assert_eq!(parse_hypr_color("0xff112233").unwrap().rgb.hex(), "#112233");
        assert_eq!(parse_hypr_color("#abcdef").unwrap().rgb.hex(), "#abcdef");
        assert!(parse_hypr_color("nope").is_none());
    }

    #[test]
    fn the_default_theme_resolves_a_shell_block() {
        let block = shell_splash_block(BUNDLED_TOKYO_NIGHT_SPLASH);
        assert!(block.starts_with("mod.wm_theme.shell = {"));
    }
}
