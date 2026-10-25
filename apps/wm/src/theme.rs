//! wm theming lives in SPLASH. A theme is a `theme.splash` file defining
//! `mod.wm_theme = { ... }`, evaluated into the script VM at startup and
//! on switch; the chrome styles itself from `mod.wm_theme.*` and the
//! terminal palette handed to child terminals comes from the same object.
//! Users edit the .splash file directly — it is the theme.
//!
//! Omarchy themes are an IMPORT SOURCE only: `--import-theme <name>`
//! downloads an omarchy theme (basecamp/omarchy), runs its color-derivation
//! cascade (ported from `bin/omarchy-theme-color`) once, and writes a
//! self-contained theme.splash + the background images. Nothing of
//! omarchy's format survives past the import.
//!
//! Installed layout: `~/.makepad/wm/themes/<name>/theme.splash`
//!                   `~/.makepad/wm/themes/<name>/backgrounds/*`

use makepad_shell_kit::theme::{
    parse_hex, parse_hypr_color, Gradient, Rgb, Stop,
    BUNDLED_TOKYO_NIGHT_SPLASH, DEFAULT_ACTIVE_BORDER, DEFAULT_INACTIVE_BORDER,
};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::PathBuf;

pub const DEFAULT_THEME: &str = "tokyo-night";

/// The omarchy theme catalog (basecamp/omarchy `themes/`), the import
/// menu's list. Importing any of them writes a self-contained theme.splash
/// here; nothing of omarchy's format survives the conversion.
pub const OMARCHY_THEMES: &[&str] = &[
    "tokyo-night",
    "catppuccin",
    "catppuccin-latte",
    "ethereal",
    "everforest",
    "flexoki-light",
    "gruvbox",
    "hackerman",
    "kanagawa",
    "last-horizon",
    "lumon",
    "lupine",
    "matte-black",
    "miasma",
    "nord",
    "osaka-jade",
    "retro-82",
    "ristretto",
    "rose-pine",
    "solitude",
    "vantablack",
    "white",
];

/// Stamped into every generated theme.splash. Bump it whenever the emitted
/// key set changes: `ensure_default_theme` reseeds the bundled theme when
/// the marker on disk is older, so new keys reach existing installs.
pub const THEME_FORMAT: &str = "(format 3)";


const OMARCHY_RAW: &str = "https://raw.githubusercontent.com/omacom/omarchy/quattro/themes";
const OMARCHY_API: &str = "https://api.github.com/repos/omacom/omarchy/contents/themes";








/// A hyprland border value: `rgba(33ccffee) rgba(00ff99ee) 45deg`, or any
/// single color. Extra stops beyond the second are ignored (we draw a two
/// stop gradient); the angle defaults to 0.
pub fn parse_hypr_gradient(s: &str) -> Option<Gradient> {
    let mut stops: Vec<Stop> = Vec::new();
    let mut angle = 0.0f64;
    for token in s.split_whitespace() {
        if let Some(deg) = token.strip_suffix("deg") {
            if let Ok(v) = deg.parse::<f64>() {
                angle = v;
            }
            continue;
        }
        if let Some(stop) = parse_hypr_color(token) {
            stops.push(stop);
        }
    }
    let start = *stops.first()?;
    let end = *stops.get(1).unwrap_or(&start);
    Some(Gradient { start, end, angle })
}

/// Linear per-channel blend, rounding half up (`mix_color`).
fn mix(a: Rgb, b: Rgb, amount: f64) -> Rgb {
    let amount = amount.clamp(0.0, 1.0);
    let ch = |x: u8, y: u8| ((x as f64 + (y as f64 - x as f64) * amount) + 0.5) as u8;
    Rgb {
        r: ch(a.r, b.r),
        g: ch(a.g, b.g),
        b: ch(a.b, b.b),
    }
}

const BLACK: Rgb = Rgb { r: 0, g: 0, b: 0 };
const WHITE: Rgb = Rgb {
    r: 255,
    g: 255,
    b: 255,
};

/// Import-time intermediate: a resolved omarchy palette on its way into
/// splash. Also the Rust-side fallback when a theme.splash is unreadable.
#[derive(Clone, Debug)]
pub struct ImportedTheme {
    pub name: String,
    pub light_mode: bool,
    pub accent: Rgb,
    pub selection: Rgb,
    pub muted: Rgb,
    pub background: Rgb,
    pub dark_background: Rgb,
    pub darker_background: Rgb,
    pub lighter_background: Rgb,
    pub foreground: Rgb,
    pub dark_foreground: Rgb,
    pub light_foreground: Rgb,
    pub bright_foreground: Rgb,
    /// red yellow orange green cyan blue magenta brown
    pub hues: [Rgb; 8],
    /// bright: red yellow green cyan blue magenta
    pub bright: [Rgb; 6],
    pub cursor: Rgb,
    /// Focused window border: two stops + an angle (a solid color is the
    /// degenerate case where both stops match).
    pub active_border: Gradient,
    /// Unfocused window border: one stop, alpha included.
    pub inactive_border: Stop,
}

impl ImportedTheme {
    /// Terminal base16 per omarchy's alacritty mapping: normal black =
    /// background ... white = foreground; bright black = muted ... bright
    /// white = bright_foreground.
    pub fn terminal_base16(&self) -> [Rgb; 16] {
        let [red, yellow, _orange, green, cyan, blue, magenta, _brown] = self.hues;
        let [b_red, b_yellow, b_green, b_cyan, b_blue, b_magenta] = self.bright;
        [
            self.background,
            red,
            green,
            yellow,
            blue,
            magenta,
            cyan,
            self.foreground,
            self.muted,
            b_red,
            b_green,
            b_yellow,
            b_blue,
            b_magenta,
            b_cyan,
            self.bright_foreground,
        ]
    }
}

/// Parse an omarchy `colors.toml` (flat, string values) and run the
/// derivation cascade from `omarchy-theme-color`. Import-time only.
pub fn resolve_omarchy_colors(name: &str, toml: &str) -> Option<ImportedTheme> {
    let mut raw: HashMap<String, String> = HashMap::new();
    for line in toml.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        raw.insert(
            key.trim().to_string(),
            value.trim().trim_matches('"').to_string(),
        );
    }

    // Legacy short-name + ANSI aliases; canonical name wins.
    let aliases: &[(&str, &[&str])] = &[
        ("background", &["bg", "color0"]),
        ("dark_background", &["dark_bg"]),
        ("darker_background", &["darker_bg"]),
        ("lighter_background", &["lighter_bg"]),
        ("foreground", &["fg", "color7"]),
        ("dark_foreground", &["dark_fg", "color8"]),
        ("light_foreground", &["light_fg"]),
        ("bright_foreground", &["bright_fg", "color15"]),
        ("red", &["color1"]),
        ("green", &["color2"]),
        ("yellow", &["color3"]),
        ("blue", &["color4"]),
        ("magenta", &["purple", "color5"]),
        ("cyan", &["color6"]),
        ("bright_red", &["color9"]),
        ("bright_green", &["color10"]),
        ("bright_yellow", &["color11"]),
        ("bright_blue", &["color12"]),
        ("bright_magenta", &["bright_purple", "color13"]),
        ("bright_cyan", &["color14"]),
    ];
    for (canon, alts) in aliases {
        if !raw.contains_key(*canon) {
            for alt in *alts {
                if let Some(v) = raw.get(*alt).cloned() {
                    raw.insert((*canon).to_string(), v);
                    break;
                }
            }
        }
    }

    let get = |raw: &HashMap<String, String>, key: &str| -> Option<Rgb> {
        raw.get(key).and_then(|v| parse_hex(v))
    };

    let background = get(&raw, "background")?;
    let foreground = get(&raw, "foreground")?;

    let light_foreground = get(&raw, "light_foreground").unwrap_or(foreground);
    let bright_foreground = get(&raw, "bright_foreground").unwrap_or(foreground);
    let cursor = bright_foreground;
    let lighter_background = get(&raw, "lighter_background").unwrap_or(background);
    let dark_foreground = get(&raw, "dark_foreground").unwrap_or(foreground);
    let muted = get(&raw, "muted").unwrap_or(dark_foreground);
    let selection = get(&raw, "selection")
        .or_else(|| get(&raw, "selection_background"))
        .or_else(|| get(&raw, "color8"))
        .unwrap_or(background);
    let yellow = get(&raw, "yellow").unwrap_or(foreground);
    let orange = get(&raw, "orange").unwrap_or(yellow);
    let brown = get(&raw, "brown").unwrap_or_else(|| mix(orange, BLACK, 0.5));
    let dark_background =
        get(&raw, "dark_background").unwrap_or_else(|| mix(background, BLACK, 0.25));
    let darker_background =
        get(&raw, "darker_background").unwrap_or_else(|| mix(background, BLACK, 0.5));

    let hue = |raw: &HashMap<String, String>, key: &str| -> Rgb {
        get(raw, key).unwrap_or(foreground)
    };
    let red = hue(&raw, "red");
    let green = hue(&raw, "green");
    let cyan = hue(&raw, "cyan");
    let blue = hue(&raw, "blue");
    let magenta = hue(&raw, "magenta");
    let bright_of = |raw: &HashMap<String, String>, key: &str, base: Rgb| -> Rgb {
        get(raw, key).unwrap_or_else(|| mix(base, WHITE, 0.2))
    };

    let accent = get(&raw, "accent").unwrap_or(blue);
    // Borders follow omarchy's default/themed/hyprland.lua.tpl:
    //   active   = hypr_gradient(hyprland_active_border, accent)
    //   inactive = hypr_gradient(hyprland_inactive_border, rgba(595959aa))
    // A theme that names its own gradient (hackerman, last-horizon,
    // solitude) wins. Everything else gets hyprland's own default, which
    // omarchy keeps in looknfeel.lua and the reference desktop shows: the
    // cyan→green 45° ring, NOT the theme accent solid.
    let active_border = raw
        .get("hyprland_active_border")
        .and_then(|v| parse_hypr_gradient(v))
        .unwrap_or(DEFAULT_ACTIVE_BORDER);
    let inactive_border = raw
        .get("hyprland_inactive_border")
        .and_then(|v| parse_hypr_gradient(v))
        .map(|g| g.start)
        .unwrap_or(DEFAULT_INACTIVE_BORDER);

    let light_mode = match raw.get("mode").map(|s| s.as_str()) {
        Some("light") => true,
        Some(_) => false,
        None => match raw.get("theme_type").map(|s| s.as_str()) {
            Some("light") => true,
            Some(_) => false,
            None => background.r as u32 + background.g as u32 + background.b as u32 > 382,
        },
    };

    Some(ImportedTheme {
        name: name.to_string(),
        light_mode,
        accent,
        selection,
        muted,
        background,
        dark_background,
        darker_background,
        lighter_background,
        foreground,
        dark_foreground,
        light_foreground,
        bright_foreground,
        hues: [red, yellow, orange, green, cyan, blue, magenta, brown],
        bright: [
            bright_of(&raw, "bright_red", red),
            bright_of(&raw, "bright_yellow", yellow),
            bright_of(&raw, "bright_green", green),
            bright_of(&raw, "bright_cyan", cyan),
            bright_of(&raw, "bright_blue", blue),
            bright_of(&raw, "bright_magenta", magenta),
        ],
        cursor,
        active_border,
        inactive_border,
    })
}

/// Emit the theme.splash for an imported theme. From here on THIS FILE is
/// the theme: wm evaluates it, the chrome reads `mod.wm_theme.*`, and
/// the `term` block is the palette handed to child terminals.
pub fn splash_source(theme: &ImportedTheme) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "// wm theme \"{}\" — imported from the omarchy theme of the same",
        theme.name
    );
    let _ = writeln!(
        s,
        "// name; this splash file IS the theme now. Edit freely. {}",
        THEME_FORMAT
    );
    let _ = writeln!(s, "mod.wm_theme = {{");
    {
        let mut kv = |k: &str, v: Rgb| {
            let _ = writeln!(s, "    {}: {}", k, v.splash_hex());
        };
        kv("accent", theme.accent);
        kv("selection", theme.selection);
        kv("muted", theme.muted);
        kv("background", theme.background);
        kv("dark_background", theme.dark_background);
        kv("darker_background", theme.darker_background);
        kv("lighter_background", theme.lighter_background);
        kv("foreground", theme.foreground);
        kv("dark_foreground", theme.dark_foreground);
        kv("light_foreground", theme.light_foreground);
        kv("bright_foreground", theme.bright_foreground);
        kv("cursor", theme.cursor);
        kv("active_border", theme.active_border.start.rgb);
        kv("active_border_end", theme.active_border.end.rgb);
    }
    // A float literal, always: the script VM types `45` as an int and
    // refuses to apply it to the shader's f32 angle.
    let _ = writeln!(s, "    active_border_angle: {:.1}", theme.active_border.angle);
    let _ = writeln!(
        s,
        "    active_border_alpha: {:.3}",
        theme.active_border.start.alpha
    );
    let _ = writeln!(
        s,
        "    inactive_border: {}",
        theme.inactive_border.rgb.splash_hex()
    );
    let _ = writeln!(
        s,
        "    inactive_border_alpha: {:.3}",
        theme.inactive_border.alpha
    );
    let _ = writeln!(s, "    light_mode: {}", theme.light_mode);
    let _ = writeln!(s, "    term: {{");
    for (i, c) in theme.terminal_base16().iter().enumerate() {
        let _ = writeln!(s, "        color{}: {}", i, c.splash_hex());
    }
    let _ = writeln!(s, "        foreground: {}", theme.foreground.splash_hex());
    let _ = writeln!(s, "        background: {}", theme.background.splash_hex());
    let _ = writeln!(s, "        cursor: {}", theme.cursor.splash_hex());
    let _ = writeln!(s, "        selection: {}", theme.selection.splash_hex());
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}");
    s
}

// ----------------------------------------------------------------------
// Install / download
// ----------------------------------------------------------------------

pub fn makepad_home() -> PathBuf {
    // An empty MAKEPAD_HOME counts as unset: PathBuf::from("") would make
    // every path below it relative to the current folder.
    if let Some(home) = std::env::var_os("MAKEPAD_HOME").filter(|home| !home.is_empty()) {
        return PathBuf::from(home);
    }
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(crate::host::homeless_root)
        .join(".makepad")
}

pub fn themes_dir() -> PathBuf {
    makepad_home().join("wm/themes")
}

pub fn theme_splash_path(name: &str) -> PathBuf {
    themes_dir().join(name).join("theme.splash")
}

pub fn installed_themes() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(themes_dir()) {
        for entry in entries.flatten() {
            if entry.path().join("theme.splash").exists() {
                if let Some(name) = entry.file_name().to_str() {
                    out.push(name.to_string());
                }
            }
        }
    }
    out.sort();
    out
}

/// The theme's splash source (bundled default when nothing is installed).
pub fn load_theme_source(name: &str) -> String {
    std::fs::read_to_string(theme_splash_path(name))
        .unwrap_or_else(|_| BUNDLED_TOKYO_NIGHT_SPLASH.to_string())
}

/// Sorted background images of an installed theme.
pub fn theme_backgrounds(name: &str) -> Vec<PathBuf> {
    let dir = themes_dir().join(name).join("backgrounds");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    matches!(
                        p.extension().and_then(|e| e.to_str()),
                        Some("jpg" | "jpeg" | "png" | "webp" | "bmp" | "gif")
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

/// Seed the default theme locally without touching the network (a later
/// `--import-theme` upgrades it with the real backgrounds).
pub fn ensure_default_theme() {
    let path = theme_splash_path(DEFAULT_THEME);
    if let Ok(existing) = std::fs::read_to_string(&path) {
        // Reseed files generated with the retired `#x` color form (the
        // runtime tokenizer never understood it) and anything older than
        // the current key set.
        if !existing.contains("#x") && existing.contains(THEME_FORMAT) {
            return;
        }
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, BUNDLED_TOKYO_NIGHT_SPLASH);
}

fn curl(url: &str) -> Option<Vec<u8>> {
    let mut command = std::process::Command::new("curl");
    crate::host::no_console_window(&mut command);
    let out = command.args(["-sfL", "--max-time", "60", url]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(out.stdout)
}

/// Fetch a theme's wallpapers from the omarchy repo into its `backgrounds`
/// folder when that folder is empty — the bundled default theme ships its
/// colours but not its pictures, so a fresh install has nothing behind the
/// desk until this runs. Off the UI thread (the caller spawns it); the
/// pictures are applied when the desk notices them (`theme_backgrounds`).
/// Nothing when the folder already has pictures or the network is away.
pub fn fetch_backgrounds_if_missing(name: &str) -> usize {
    let dir = themes_dir().join(name).join("backgrounds");
    if !theme_backgrounds(name).is_empty() {
        return 0;
    }
    let _ = std::fs::create_dir_all(&dir);
    let listing_url = format!("{}/{}/backgrounds", OMARCHY_API, name);
    let Some(listing) = curl(&listing_url) else {
        return 0;
    };
    let listing = String::from_utf8_lossy(&listing);
    let mut fetched = 0usize;
    for part in listing.split("\"download_url\"") {
        let Some(url) = part.split('"').nth(1).filter(|u| u.starts_with("https://")) else {
            continue;
        };
        let Some(file) = url.rsplit('/').next() else {
            continue;
        };
        if let Some(bytes) = curl(url) {
            let _ = std::fs::write(dir.join(file), bytes);
            fetched += 1;
        }
    }
    fetched
}

/// Download an omarchy theme by name and CONVERT it: theme.splash plus the
/// background images. Accepts "Tokyo Night" or "tokyo-night".
pub fn import_omarchy_theme(name: &str) -> Result<String, String> {
    // omarchy-theme-set's normalization: lowercase, spaces to dashes.
    let slug = name.trim().to_lowercase().replace(' ', "-");
    if slug.is_empty() || slug.starts_with('.') || slug.contains('/') {
        return Err(format!("invalid theme name: {}", name));
    }

    let toml_url = format!("{}/{}/colors.toml", OMARCHY_RAW, slug);
    let toml_bytes =
        curl(&toml_url).ok_or_else(|| format!("theme '{}' not found at {}", slug, toml_url))?;
    let toml = String::from_utf8_lossy(&toml_bytes).to_string();
    let theme = resolve_omarchy_colors(&slug, &toml)
        .ok_or_else(|| format!("theme '{}' has an unparseable colors.toml", slug))?;

    let dir = themes_dir().join(&slug);
    std::fs::create_dir_all(dir.join("backgrounds")).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("theme.splash"), splash_source(&theme))
        .map_err(|e| e.to_string())?;

    // Backgrounds via the github contents API listing — the same fetch a
    // fresh install runs for the bundled theme on its own.
    let fetched = fetch_backgrounds_if_missing(&slug);

    Ok(format!(
        "imported '{}' to splash ({} backgrounds) at {}",
        slug,
        fetched,
        dir.display()
    ))
}






// ----------------------------------------------------------------------
// The shell token object — `default/themed/shell.toml.tpl`
// ----------------------------------------------------------------------


/// True when the theme ships its own `shell: { ... }` block, in which case
/// it replaces the generated one wholesale (omarchy's rule for a theme
/// shipping `themes/<name>/shell.toml` instead of the generated file).
pub fn theme_defines_shell(source: &str) -> bool {
    source
        .lines()
        .any(|l| l.trim_start().starts_with("shell:") || l.trim_start().starts_with("shell :"))
}


#[cfg(test)]
mod tests {
    use super::*;

    const TOKYO_TOML: &str = r##"mode = "dark"
accent = "#7aa2f7"
selection = "#292e42"
muted = "#414868"
background = "#1a1b26"
dark_background = "#13141c"
darker_background = "#0e0e14"
lighter_background = "#24283b"
foreground = "#a9b1d6"
dark_foreground = "#565f89"
light_foreground = "#b4bee6"
bright_foreground = "#c0caf5"
red = "#f7768e"
yellow = "#e0af68"
orange = "#eb927b"
green = "#9ece6a"
cyan = "#449dab"
blue = "#7aa2f7"
magenta = "#ad8ee6"
brown = "#75493d"
bright_red = "#ff7a93"
bright_yellow = "#ff9e64"
bright_green = "#b9f27c"
bright_cyan = "#0db9d7"
bright_blue = "#7da6ff"
bright_magenta = "#bb9af7"
"##;

    #[test]
    fn import_matches_bundled_splash() {
        let t = resolve_omarchy_colors("tokyo-night", TOKYO_TOML).unwrap();
        assert_eq!(splash_source(&t), BUNDLED_TOKYO_NIGHT_SPLASH);
    }

    #[test]
    fn cascade_derives_missing_values() {
        let minimal = "background = \"#102030\"\nforeground = \"#c0c0c0\"\nred = \"#ff0000\"\n";
        let t = resolve_omarchy_colors("x", minimal).unwrap();
        assert_eq!(t.dark_background.hex(), "#0c1824");
        assert_eq!(t.darker_background.hex(), "#081018");
        assert_eq!(t.bright[0].hex(), "#ff3333");
        assert_eq!(t.cursor.hex(), "#c0c0c0");
        assert_eq!(t.hues[2].hex(), "#c0c0c0");
        assert!(!t.light_mode);
    }

    #[test]
    fn ansi_alias_only_theme() {
        let legacy = "color0 = \"#111111\"\ncolor7 = \"#dddddd\"\ncolor1 = \"#aa0000\"\ncolor8 = \"#555555\"\n";
        let t = resolve_omarchy_colors("legacy", legacy).unwrap();
        assert_eq!(t.background.hex(), "#111111");
        assert_eq!(t.foreground.hex(), "#dddddd");
        assert_eq!(t.hues[0].hex(), "#aa0000");
        assert_eq!(t.muted.hex(), "#555555");
    }

    #[test]
    fn a_theme_without_a_gradient_gets_hyprlands_own() {
        // looknfeel.lua:3-4 — a theme that names no gradient keeps
        // hyprland's default cyan→green 45° ring, which is what the
        // reference desktop shows; only the accent-solid guess was ours.
        let t = resolve_omarchy_colors("tokyo-night", TOKYO_TOML).unwrap();
        assert!(!t.active_border.is_solid());
        assert_eq!(t.active_border.start.rgb.hex(), "#33ccff");
        assert_eq!(t.active_border.end.rgb.hex(), "#00ff99");
        assert_eq!(t.active_border.angle, 45.0);
        assert_eq!(t.inactive_border.rgb.hex(), "#595959");
    }

    #[test]
    fn theme_can_override_borders() {
        // hackerman's real colors.toml line.
        let toml = format!(
            "{}hyprland_active_border = \"rgba(26a269ee) rgba(2ec27eee) 45deg\"\nhyprland_inactive_border = \"rgba(584e51aa)\"\n",
            TOKYO_TOML
        );
        let t = resolve_omarchy_colors("x", &toml).unwrap();
        assert!(!t.active_border.is_solid());
        assert_eq!(t.active_border.start.rgb.hex(), "#26a269");
        assert_eq!(t.active_border.end.rgb.hex(), "#2ec27e");
        assert_eq!(t.active_border.angle, 45.0);
        assert_eq!(t.inactive_border.rgb.hex(), "#584e51");
        assert!((t.inactive_border.alpha - 0.667).abs() < 0.002);
        // last-horizon's: two stops, NO angle — a horizontal gradient.
        let toml = format!(
            "{}hyprland_active_border = \"rgba(8a8588ee) rgba(e2dddcee)\"\n",
            TOKYO_TOML
        );
        let t = resolve_omarchy_colors("x", &toml).unwrap();
        assert_eq!(t.active_border.angle, 0.0);
        assert_eq!(t.active_border.end.rgb.hex(), "#e2dddc");
        // solitude's inactive: an rgb() with no alpha at all.
        let toml = format!("{}hyprland_inactive_border = \"rgb(1e1e1e)\"\n", TOKYO_TOML);
        let t = resolve_omarchy_colors("x", &toml).unwrap();
        assert_eq!(t.inactive_border.rgb.hex(), "#1e1e1e");
        assert_eq!(t.inactive_border.alpha, 1.0);
    }

    #[test]
    fn hypr_gradient_forms() {
        let g = parse_hypr_gradient("rgba(33ccffee) rgba(00ff99ee) 45deg").unwrap();
        assert_eq!(g.end.rgb.hex(), "#00ff99");
        assert_eq!(g.angle, 45.0);
        assert!(parse_hypr_gradient("rgba(595959aa)").unwrap().is_solid());
    }

    #[test]
    fn a_theme_that_ships_its_own_shell_block_keeps_it() {
        assert!(theme_defines_shell("mod.wm_theme = {\n    shell: {}\n}\n"));
        assert!(!theme_defines_shell(BUNDLED_TOKYO_NIGHT_SPLASH));
    }

    #[test]
    fn light_mode_resolution() {
        let light = "mode = \"light\"\nbackground = \"#101010\"\nforeground = \"#000000\"\n";
        assert!(resolve_omarchy_colors("x", light).unwrap().light_mode);
        let lum = "background = \"#f0f0f0\"\nforeground = \"#000000\"\n";
        assert!(resolve_omarchy_colors("x", lum).unwrap().light_mode);
    }
}
