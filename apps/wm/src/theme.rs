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

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use makepad_widgets::{log, ScriptMod, ScriptVm, Vec4f};

pub const DEFAULT_THEME: &str = "tokyo-night";
/// The bundled Liquid Glass theme (see BUNDLED_MAKEOS_SPLASH).
pub const MAKEOS_THEME: &str = "makeos";

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
/// key set changes: `ensure_bundled_themes` reseeds the bundled themes when
/// the marker on disk is older, so new keys reach existing installs. Keys
/// the chrome generates at evaluation time (`material`, `desk`) need no
/// bump.
pub const THEME_FORMAT: &str = "(format 3)";

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
const OMARCHY_RAW: &str = "https://raw.githubusercontent.com/omacom/omarchy/quattro/themes";
const OMARCHY_API: &str = "https://api.github.com/repos/omacom/omarchy/contents/themes";

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
    if let Some(home) = std::env::var_os("MAKEPAD_HOME") {
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

/// The pictures in `dir`, sorted by lowercased name: rasters the async
/// decoder takes, and `.svg` (the bundled MakeOS scene) for the vector
/// engine.
pub fn backgrounds_in(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.extension()
                        .and_then(|e| e.to_str())
                        .map(|e| {
                            matches!(
                                e.to_ascii_lowercase().as_str(),
                                "jpg" | "jpeg" | "png" | "webp" | "bmp" | "gif" | "svg"
                            )
                        })
                        .unwrap_or(false)
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    files
}

/// Sorted background images of an installed theme.
pub fn theme_backgrounds(name: &str) -> Vec<PathBuf> {
    backgrounds_in(&themes_dir().join(name).join("backgrounds"))
}

/// Seed the bundled themes into `dir` (`themes_dir()` in production):
/// tokyo-night — omarchy's default, colours only; a later import adds its
/// pictures — and makeos, colours, material and its own vector wallpaper.
/// A theme.splash already on disk is left alone unless it predates the
/// current format marker or carries the retired `#x` colour form (the
/// runtime tokenizer never understood it), so new keys reach old installs
/// and a person's edits survive. The wallpaper is written only when
/// missing: a person may replace it with their own pictures, and a changed
/// bundled scene reaches an install only when its file is deleted. Never
/// touches the network.
pub fn ensure_bundled_themes_in(dir: &Path) {
    seed_theme(dir, DEFAULT_THEME, BUNDLED_TOKYO_NIGHT_SPLASH);
    seed_theme(dir, MAKEOS_THEME, BUNDLED_MAKEOS_SPLASH);
    let wallpaper = dir.join(MAKEOS_THEME).join("backgrounds").join("makeos.svg");
    if !wallpaper.exists() {
        if let Some(parent) = wallpaper.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&wallpaper, BUNDLED_MAKEOS_WALLPAPER);
    }
}

fn seed_theme(dir: &Path, name: &str, splash: &str) {
    let path = dir.join(name).join("theme.splash");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        if !existing.contains("#x") && existing.contains(THEME_FORMAT) {
            return;
        }
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, splash);
}

pub fn ensure_bundled_themes() {
    ensure_bundled_themes_in(&themes_dir());
}

fn curl(url: &str) -> Option<Vec<u8>> {
    let out = std::process::Command::new("curl")
        .args(["-sfL", "--max-time", "60", url])
        .output()
        .ok()?;
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

/// MakeOS: the Liquid Glass look, bundled so it needs no network. A dark
/// navy ground with a cool-blue accent, white-on-dark type, the glass
/// material, rounded tiles, and its own vector wallpaper
/// (`resources/wallpapers/makeos.svg`, seeded next to it).
pub const BUNDLED_MAKEOS_SPLASH: &str = r##"// wm theme "makeos" — the Liquid Glass look, bundled. This splash file
// IS the theme. Edit freely. (format 3)
mod.wm_theme = {
    accent: #5b9dff
    selection: #1d2b4a
    muted: #34415f
    background: #0b1220
    dark_background: #080d18
    darker_background: #05070e
    lighter_background: #16203a
    foreground: #d6e2ff
    dark_foreground: #6b7a99
    light_foreground: #e6eeff
    bright_foreground: #ffffff
    cursor: #ffffff
    active_border: #7cc4ff
    active_border_end: #5b9dff
    active_border_angle: 45.0
    active_border_alpha: 0.900
    inactive_border: #ffffff
    inactive_border_alpha: 0.180
    light_mode: false
    material: {
        glass: 1.0
        corner_radius: 12.0
        control_radius: 8.0
        blur_level: 5.2
        lensing_effect: 0.94
        lensing_strength: 28.0
        lensing_width: 20.0
        diffraction_strength: 4.4
        tint_color: #f8fbff
        tint_alpha: 0.06
        border_color: #ffffff
        border_alpha: 0.55
        border_width: 1.0
        specular_strength: 0.22
        noise_strength: 0.004
        shadow_color: #000000
        shadow_alpha: 0.44
        shadow_radius: 13.0
        shadow_offset_y: 5.0
        fallback_color: #334156
    }
    desk: {
        gaps_in: 6.0
        gaps_out: 12.0
        border_size: 1.5
        corner_radius: 12.0
    }
    term: {
        color0: #0b1220
        color1: #ff6b81
        color2: #4fe3a3
        color3: #ffd166
        color4: #5b9dff
        color5: #c792ea
        color6: #5ee7f2
        color7: #d6e2ff
        color8: #34415f
        color9: #ff8a9b
        color10: #7dffc0
        color11: #ffe08a
        color12: #7cc4ff
        color13: #d9b3ff
        color14: #8ef3fb
        color15: #ffffff
        foreground: #d6e2ff
        background: #0b1220
        cursor: #ffffff
        selection: #1d2b4a
    }
}
"##;

/// The MakeOS wallpaper, written to the theme's `backgrounds/` on seeding.
pub const BUNDLED_MAKEOS_WALLPAPER: &str = include_str!("../resources/wallpapers/makeos.svg");

// ----------------------------------------------------------------------
// The shell token object — `default/themed/shell.toml.tpl`
// ----------------------------------------------------------------------

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

/// One `key: #rrggbb` of a theme.splash as a colour (alpha 1), top level:
/// the dotted path `scan_values` gives it, so a nested `term.background`
/// never stands in for `background`, and a trailing comment is no part
/// of the value.
pub fn scan_color(source: &str, key: &str) -> Option<Vec4f> {
    let values = scan_values(source);
    let rgb = parse_hex(values.get(key)?)?;
    Some(Vec4f {
        x: rgb.r as f32 / 255.0,
        y: rgb.g as f32 / 255.0,
        z: rgb.b as f32 / 255.0,
        w: 1.0,
    })
}

/// The palette colours the chrome bakes in at DSL time and re-pushes on a
/// live switch (`App::apply_theme_to_chrome`) — the wallpaper gradient,
/// the bar strip, the tile wash and tab strip, the module ground, the AI
/// pane. A key the theme lacks takes the bundled default's value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaletteColors {
    pub background: Vec4f,
    pub darker_background: Vec4f,
    pub lighter_background: Vec4f,
    pub foreground: Vec4f,
    pub dark_foreground: Vec4f,
    pub accent: Vec4f,
}

pub fn scan_palette(source: &str) -> PaletteColors {
    let pick = |key: &str| {
        scan_color(source, key)
            .or_else(|| scan_color(BUNDLED_TOKYO_NIGHT_SPLASH, key))
            .expect("the bundled theme names every palette key")
    };
    PaletteColors {
        background: pick("background"),
        darker_background: pick("darker_background"),
        lighter_background: pick("lighter_background"),
        foreground: pick("foreground"),
        dark_foreground: pick("dark_foreground"),
        accent: pick("accent"),
    }
}

impl Default for PaletteColors {
    fn default() -> Self {
        scan_palette("")
    }
}

/// True when the theme ships its own `shell: { ... }` block, in which case
/// it replaces the generated one wholesale (omarchy's rule for a theme
/// shipping `themes/<name>/shell.toml` instead of the generated file).
pub fn theme_defines_shell(source: &str) -> bool {
    source
        .lines()
        .any(|l| l.trim_start().starts_with("shell:") || l.trim_start().starts_with("shell :"))
}

/// Every scalar `key: value` line of a theme.splash, keyed by its dotted
/// path — `accent`, `term.color0`, `material.glass`. Values are the source
/// text, untouched (`#7aa2f7`, `12.0`, `true`); a trailing `// comment` is
/// dropped. Blocks nest by the emitted shape's `key: {` / `}` lines. A
/// one-line block (`material: { glass: 1.0, corner_radius: 12.0 }`) is
/// scanned too, its pairs comma-separated. Anything from `//` on is a
/// comment, so a value cannot contain `//` (no key does today), and a line
/// without a colon — the `mod.wm_theme = {` wrapper — is skipped and adds
/// no prefix.
pub fn scan_values(source: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut prefix: Vec<String> = Vec::new();
    for line in source.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        if line.starts_with('}') {
            prefix.pop();
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.split("//").next().unwrap_or("").trim().trim_end_matches(',').trim();
        if value == "{" {
            if !key.starts_with("mod.") {
                prefix.push(key.to_string());
            }
            continue;
        }
        if value.starts_with('{') && value.ends_with('}') {
            // A one-line block: `material: { glass: 1.0, corner_radius: 12.0 }`
            // — its pairs are comma-separated.
            let inner = &value[1..value.len() - 1];
            for pair in inner.split(',') {
                let Some((k, v)) = pair.split_once(':') else {
                    continue;
                };
                let (k, v) = (k.trim(), v.trim());
                if k.is_empty() || v.is_empty() {
                    continue;
                }
                let mut path = prefix.clone();
                path.push(key.to_string());
                path.push(k.to_string());
                out.insert(path.join("."), v.to_string());
            }
            continue;
        }
        if value.is_empty() {
            continue;
        }
        let full = if prefix.is_empty() {
            key.to_string()
        } else {
            format!("{}.{}", prefix.join("."), key)
        };
        out.insert(full, value.to_string());
    }
    out
}

/// A number as the VM wants it: `12` becomes `12.0` (an int literal does
/// not apply to an f32/f64 field); anything else passes through.
fn float_literal(s: &str) -> String {
    if s.parse::<i64>().is_ok() {
        format!("{}.0", s)
    } else {
        s.to_string()
    }
}

/// One generated block: the theme's own `<section>.<key>` values where it
/// names them, `defaults` elsewhere. Numbers get their decimal point.
fn filled_block(
    source: &str,
    target: &str,
    section: &str,
    numbers: &[(&str, &str)],
    colors: &[(&str, &str)],
) -> String {
    let v = scan_values(source);
    let mut s = String::new();
    let _ = writeln!(s, "{} = {{", target);
    for (key, default) in numbers {
        let value = v
            .get(&format!("{}.{}", section, key))
            .map(|x| float_literal(x))
            .unwrap_or_else(|| default.to_string());
        let _ = writeln!(s, "    {}: {}", key, value);
    }
    for (key, default) in colors {
        let value = v
            .get(&format!("{}.{}", section, key))
            .cloned()
            .unwrap_or_else(|| default.to_string());
        let _ = writeln!(s, "    {}: {}", key, value);
    }
    let _ = writeln!(s, "}}");
    s
}

/// `mod.wm_theme.material = { ... }`, complete: the theme's own material
/// keys where present, the flat defaults elsewhere — so a theme may say
/// just `material: { glass: 1.0 }`. Evaluated after every theme (the DSL
/// reads every key).
pub fn material_splash_block(source: &str) -> String {
    filled_block(
        source,
        "mod.wm_theme.material",
        "material",
        &[
            ("glass", "0.0"),
            ("corner_radius", "0.0"),
            ("control_radius", "0.0"),
            ("blur_level", "5.2"),
            ("lensing_effect", "0.94"),
            ("lensing_strength", "28.0"),
            ("lensing_width", "20.0"),
            ("diffraction_strength", "4.4"),
            ("tint_alpha", "0.06"),
            ("border_alpha", "0.55"),
            ("border_width", "1.0"),
            ("specular_strength", "0.22"),
            ("noise_strength", "0.004"),
            ("shadow_alpha", "0.44"),
            ("shadow_radius", "13.0"),
            ("shadow_offset_y", "5.0"),
        ],
        &[
            ("tint_color", "#f8fbff"),
            ("border_color", "#ffffff"),
            ("shadow_color", "#000000"),
            ("fallback_color", "#334156"),
        ],
    )
}

/// `mod.wm_theme.desk = { ... }`, complete, omarchy's geometry by default.
pub fn desk_splash_block(source: &str) -> String {
    filled_block(
        source,
        "mod.wm_theme.desk",
        "desk",
        &[
            ("gaps_in", "5.0"),
            ("gaps_out", "10.0"),
            ("border_size", "2.0"),
            ("corner_radius", "0.0"),
        ],
        &[],
    )
}

// ----------------------------------------------------------------------
// Evaluation into the script VM — startup and every live switch
// ----------------------------------------------------------------------

/// A splash source as an eval body: leading blank and comment lines are
/// dropped (they shift the runtime parser's span tracking), and a trailing
/// `true` makes the final assignment a real statement — the eval body's
/// last statement is otherwise treated as its result expression and a
/// trailing `mod.x = {...}` never commits.
pub fn eval_body(source: &str) -> String {
    let mut lines = source.lines().peekable();
    while let Some(line) = lines.peek() {
        let t = line.trim();
        if t.is_empty() || t.starts_with("//") {
            lines.next();
        } else {
            break;
        }
    }
    let mut s = lines.collect::<Vec<_>>().join("\n");
    s.push_str("\ntrue\n");
    s
}

fn eval_block(vm: &mut ScriptVm, name: &str, code: &str) -> bool {
    let script_mod_id = ScriptMod {
        cargo_manifest_path: env!("CARGO_MANIFEST_DIR").to_string(),
        module_path: name.to_string(),
        file: format!("{}.splash", name),
        line: 0,
        column: 0,
        code: code.to_string(),
        values: vec![],
    };
    // Parse errors are dropped and runtime errors logged by the VM's own
    // core unless a sink is installed; without one `take_errors` sees
    // nothing and a broken theme reports success. Save and restore: hosts
    // (splash, tweaker) install sinks of their own.
    let prev = vm.bx.captured_errors.replace(Vec::new());
    let value = vm.eval(script_mod_id);
    let errors = vm.take_errors();
    vm.bx.captured_errors = prev;
    for e in &errors {
        log!("wm theme: {}", e);
    }
    !value.is_err() && errors.is_empty()
}

/// Evaluate a theme into `vm`: `mod.wm_theme` from `source` (the bundled
/// default when the file does not evaluate), then the blocks the chrome
/// reads and a theme may leave out — `mod.wm_theme.shell` unless the theme
/// ships its own, and `.material` / `.desk` always, the theme's values
/// filled up with the defaults. Startup (`AppMain::script_mod`) and every
/// live switch (`App::set_theme`) go through here. True when `source`
/// itself evaluated; a source with no statements did not.
pub fn eval_into(vm: &mut ScriptVm, source: &str) -> bool {
    let body = eval_body(source);
    if body.trim() == "true" {
        log!("wm theme: no statements in the theme file; using the bundled default");
    }
    let ok = body.trim() != "true" && eval_block(vm, "wm_theme", &body);
    if !ok {
        eval_block(vm, "wm_theme_fallback", &eval_body(BUNDLED_TOKYO_NIGHT_SPLASH));
    }
    let effective = if ok { source } else { BUNDLED_TOKYO_NIGHT_SPLASH };
    if !theme_defines_shell(effective) {
        eval_block(vm, "wm_theme_shell", &eval_body(&shell_splash_block(effective)));
    }
    if !eval_block(vm, "wm_theme_material", &eval_body(&material_splash_block(effective))) {
        eval_block(
            vm,
            "wm_theme_material",
            &eval_body(&material_splash_block(BUNDLED_TOKYO_NIGHT_SPLASH)),
        );
    }
    if !eval_block(vm, "wm_theme_desk", &eval_body(&desk_splash_block(effective))) {
        eval_block(
            vm,
            "wm_theme_desk",
            &eval_body(&desk_splash_block(BUNDLED_TOKYO_NIGHT_SPLASH)),
        );
    }
    ok
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
    use makepad_widgets::makepad_script::{ScriptValue, ScriptVmBase, ScriptVmHost};

    /// A bare VM, built the way `platform/script/tests` build one.
    fn test_vm() -> ScriptVm<'static> {
        let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
        ScriptVm {
            host,
            bx: Box::new(ScriptVmBase::new()),
        }
    }

    /// The value of `expr` in `vm`, read back the way the chrome reads
    /// `mod.wm_theme.*`: a probe block whose result expression it is.
    fn read(vm: &mut ScriptVm, expr: &str) -> ScriptValue {
        vm.eval(ScriptMod {
            cargo_manifest_path: String::new(),
            module_path: "probe".to_string(),
            file: "probe.splash".to_string(),
            line: 0,
            column: 0,
            code: format!("({expr})\n"),
            values: vec![],
        })
    }

    /// A colour value as the `#rrggbb` a theme file spells it; anything
    /// else as its debug form, so a failed assertion says what came back.
    fn hex(v: ScriptValue) -> String {
        match v.as_color() {
            Some(c) => format!("#{:06x}", c >> 8),
            None => format!("{v:?}"),
        }
    }


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
        let g = parse_hypr_gradient("rgba(33ccffee) rgba(00ff99ee) 45deg").unwrap();
        assert_eq!(g.end.rgb.hex(), "#00ff99");
        assert_eq!(g.angle, 45.0);
        assert!(parse_hypr_gradient("rgba(595959aa)").unwrap().is_solid());
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
        // A theme that ships its own block keeps it.
        assert!(theme_defines_shell("mod.wm_theme = {\n    shell: {}\n}\n"));
        assert!(!theme_defines_shell(SHELL_SRC));
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
    fn light_mode_resolution() {
        let light = "mode = \"light\"\nbackground = \"#101010\"\nforeground = \"#000000\"\n";
        assert!(resolve_omarchy_colors("x", light).unwrap().light_mode);
        let lum = "background = \"#f0f0f0\"\nforeground = \"#000000\"\n";
        assert!(resolve_omarchy_colors("x", lum).unwrap().light_mode);
    }

    #[test]
    fn scan_values_reads_nested_keys_and_strips_comments() {
        let v = scan_values(
            "// c\nmod.wm_theme = {\n    accent: #7aa2f7 // the blue\n    material: {\n        glass: 1.0\n    }\n    term: {\n        color0: #1a1b26\n    }\n}\n",
        );
        assert_eq!(v.get("accent").map(String::as_str), Some("#7aa2f7"));
        assert_eq!(v.get("material.glass").map(String::as_str), Some("1.0"));
        assert_eq!(v.get("term.color0").map(String::as_str), Some("#1a1b26"));
        assert!(v.get("mod.wm_theme").is_none());
    }

    #[test]
    fn material_block_fills_in_the_flat_defaults() {
        let block = material_splash_block(BUNDLED_TOKYO_NIGHT_SPLASH);
        assert!(block.starts_with("mod.wm_theme.material = {\n"));
        assert!(block.contains("    glass: 0.0\n"));
        assert!(block.contains("    corner_radius: 0.0\n"));
        assert!(block.contains("    tint_color: #f8fbff\n"));
        assert!(block.contains("    fallback_color: #334156\n"));
        assert!(block.ends_with("}\n"));
        // A theme that names only some keys gets the rest from the defaults,
        // and an int gets its decimal point.
        let partial = "mod.wm_theme = {\n    material: {\n        glass: 1\n        corner_radius: 12.0\n        tint_color: #102030\n    }\n}\n";
        let block = material_splash_block(partial);
        assert!(block.contains("    glass: 1.0\n"));
        assert!(block.contains("    corner_radius: 12.0\n"));
        assert!(block.contains("    tint_color: #102030\n"));
        assert!(block.contains("    blur_level: 5.2\n"));
    }

    #[test]
    fn desk_block_defaults_to_omarchy_geometry() {
        let block = desk_splash_block(BUNDLED_TOKYO_NIGHT_SPLASH);
        assert_eq!(
            block,
            "mod.wm_theme.desk = {\n    gaps_in: 5.0\n    gaps_out: 10.0\n    border_size: 2.0\n    corner_radius: 0.0\n}\n"
        );
        let rounded = "mod.wm_theme = {\n    desk: {\n        corner_radius: 12\n        gaps_out: 12.0\n    }\n}\n";
        let block = desk_splash_block(rounded);
        assert!(block.contains("    corner_radius: 12.0\n"));
        assert!(block.contains("    gaps_out: 12.0\n"));
        assert!(block.contains("    gaps_in: 5.0\n"));
    }

    #[test]
    fn generated_defaults_match_the_token_structs() {
        use crate::shell::{DeskTokens, MaterialTokens};
        let m = MaterialTokens::default();
        let v = scan_values(&material_splash_block(""));
        assert_eq!(v.len(), 20, "16 numbers + 4 colours");
        let num = |k: &str| v[k].parse::<f64>().unwrap();
        let near = |a: f64, b: f32| (a as f32 - b).abs() < 1e-6;
        let col = |k: &str| {
            let rgb = parse_hex(&v[k]).unwrap();
            (rgb.r as f32 / 255.0, rgb.g as f32 / 255.0, rgb.b as f32 / 255.0)
        };
        assert_eq!(num("glass"), m.glass);
        assert_eq!(num("corner_radius"), m.corner_radius);
        assert_eq!(num("control_radius"), m.control_radius);
        assert_eq!(num("blur_level"), m.blur_level);
        assert_eq!(num("lensing_effect"), m.lensing_effect);
        assert_eq!(num("lensing_strength"), m.lensing_strength);
        assert_eq!(num("lensing_width"), m.lensing_width);
        assert_eq!(num("diffraction_strength"), m.diffraction_strength);
        assert!(near(num("tint_alpha"), m.tint_alpha));
        assert!(near(num("border_alpha"), m.border_alpha));
        assert_eq!(num("border_width"), m.border_width);
        assert!(near(num("specular_strength"), m.specular_strength));
        assert!(near(num("noise_strength"), m.noise_strength));
        assert!(near(num("shadow_alpha"), m.shadow_alpha));
        assert_eq!(num("shadow_radius"), m.shadow_radius);
        assert_eq!(num("shadow_offset_y"), m.shadow_offset_y);
        assert_eq!(col("tint_color"), (m.tint_color.x, m.tint_color.y, m.tint_color.z));
        assert_eq!(col("border_color"), (m.border_color.x, m.border_color.y, m.border_color.z));
        assert_eq!(col("shadow_color"), (m.shadow_color.x, m.shadow_color.y, m.shadow_color.z));
        assert_eq!(col("fallback_color"), (m.fallback_color.x, m.fallback_color.y, m.fallback_color.z));
        let d = DeskTokens::default();
        let v = scan_values(&desk_splash_block(""));
        assert_eq!(v.len(), 4);
        assert_eq!(v["gaps_in"].parse::<f64>().unwrap(), d.gaps_in);
        assert_eq!(v["gaps_out"].parse::<f64>().unwrap(), d.gaps_out);
        assert_eq!(v["border_size"].parse::<f64>().unwrap(), d.border_size);
        assert_eq!(v["corner_radius"].parse::<f64>().unwrap(), d.corner_radius);
    }

    #[test]
    fn scan_values_reads_the_bundled_shape_and_hand_edits() {
        let v = scan_values(BUNDLED_TOKYO_NIGHT_SPLASH);
        assert_eq!(v.get("background").map(String::as_str), Some("#1a1b26"));
        assert_eq!(v.get("term.background").map(String::as_str), Some("#1a1b26"));
        assert_eq!(v.get("active_border_angle").map(String::as_str), Some("45.0"));
        assert_eq!(v.len(), 39, "19 top-level keys + 20 term keys");
        // A one-line block, and whitespace before a trailing comma.
        let v = scan_values(
            "mod.wm_theme = {\n    material: { glass: 1.0, corner_radius: 12 }\n    accent: #7aa2f7 ,\n}\n",
        );
        assert_eq!(v.get("material.glass").map(String::as_str), Some("1.0"));
        assert_eq!(v.get("material.corner_radius").map(String::as_str), Some("12"));
        assert_eq!(v.get("accent").map(String::as_str), Some("#7aa2f7"));
        let block = material_splash_block("mod.wm_theme = {\n    material: { glass: 1 }\n}\n");
        assert!(block.contains("    glass: 1.0\n"));
        assert!(block.contains("    blur_level: 5.2\n"));
    }

    #[test]
    fn eval_body_drops_leading_comments_and_commits_the_last_statement() {
        let body = eval_body("// a\n\n// b\nmod.wm_theme = {\n    accent: #7aa2f7\n}\n");
        assert!(body.starts_with("mod.wm_theme = {"));
        assert!(body.ends_with("\ntrue\n"));
        assert_eq!(eval_body("x = 1\n"), "x = 1\ntrue\n");
    }

    #[test]
    fn the_format_marker_is_unchanged() {
        // material and desk are generated at evaluation time, so no installed
        // theme needs reseeding for them.
        assert_eq!(THEME_FORMAT, "(format 3)");
        assert!(BUNDLED_TOKYO_NIGHT_SPLASH.contains(THEME_FORMAT));
    }

    #[test]
    fn eval_into_defines_every_object_a_good_theme_needs() {
        let mut vm = test_vm();
        assert!(eval_into(&mut vm, BUNDLED_TOKYO_NIGHT_SPLASH));
        assert_eq!(hex(read(&mut vm, "mod.wm_theme.accent")), "#7aa2f7");
        assert_eq!(hex(read(&mut vm, "mod.wm_theme.shell.bar.background")), "#1a1b26");
        assert_eq!(read(&mut vm, "mod.wm_theme.material.glass").as_f64(), Some(0.0));
        assert_eq!(read(&mut vm, "mod.wm_theme.desk.gaps_out").as_f64(), Some(10.0));
    }

    #[test]
    fn eval_into_falls_back_when_the_theme_is_broken() {
        let mut vm = test_vm();
        assert!(!eval_into(&mut vm, "mod.wm_theme = {\n    accent: nothing_here.foo\n}\n"));
        assert_eq!(hex(read(&mut vm, "mod.wm_theme.accent")), "#7aa2f7");
        // No statements at all is not a theme either.
        let mut vm = test_vm();
        assert!(!eval_into(&mut vm, ""));
        assert_eq!(hex(read(&mut vm, "mod.wm_theme.accent")), "#7aa2f7");
    }

    #[test]
    fn eval_into_keeps_a_theme_that_ships_its_own_shell() {
        let mut vm = test_vm();
        let source = "mod.wm_theme = {\n    accent: #7aa2f7\n    shell: {\n        bar: {\n            background: #123456\n        }\n    }\n}\n";
        assert!(eval_into(&mut vm, source));
        assert_eq!(hex(read(&mut vm, "mod.wm_theme.shell.bar.background")), "#123456");
    }

    #[test]
    fn eval_into_can_run_again_without_growing_the_vm() {
        let mut vm = test_vm();
        assert!(eval_into(&mut vm, BUNDLED_TOKYO_NIGHT_SPLASH));
        let bodies = vm.bx.code.bodies.borrow().len();
        let red = BUNDLED_TOKYO_NIGHT_SPLASH.replace("accent: #7aa2f7", "accent: #ff0000");
        assert!(eval_into(&mut vm, &red));
        assert_eq!(vm.bx.code.bodies.borrow().len(), bodies);
        assert_eq!(hex(read(&mut vm, "mod.wm_theme.accent")), "#ff0000");
    }

    #[test]
    fn scan_palette_reads_the_theme_and_falls_back_to_the_bundled_default() {
        let p = scan_palette(BUNDLED_TOKYO_NIGHT_SPLASH);
        assert!((p.accent.x - 0x7a as f32 / 255.0).abs() < 0.01);
        assert!((p.background.z - 0x26 as f32 / 255.0).abs() < 0.01);
        assert_eq!(p.accent.w, 1.0);
        // A theme naming only an accent keeps the default's other colours.
        let q = scan_palette("mod.wm_theme = {\n    accent: #ff0000\n}\n");
        assert_eq!(q.accent.x, 1.0);
        assert_eq!(q.background, p.background);
        assert_eq!(PaletteColors::default(), p);
        // A nested block's key of the same name does not stand in for the
        // top-level one, and a trailing comment is no part of the value.
        let nested = "mod.wm_theme = {\n    term: {\n        background: #000000\n    }\n    background: #123456 // navy\n}\n";
        let n = scan_palette(nested);
        assert!((n.background.x - 0x12 as f32 / 255.0).abs() < 0.01);
        assert!((n.background.z - 0x56 as f32 / 255.0).abs() < 0.01);
    }

    #[test]
    fn bundled_makeos_is_a_complete_glass_theme() {
        let src = BUNDLED_MAKEOS_SPLASH;
        assert!(src.contains(THEME_FORMAT));
        assert!(!src.contains("#x"));
        assert!(scan_term_palette(src).is_some(), "the term block stays scannable");
        let v = scan_values(src);
        assert_eq!(v.get("material.glass").map(String::as_str), Some("1.0"));
        assert_eq!(v.get("material.corner_radius").map(String::as_str), Some("12.0"));
        assert_eq!(v.get("desk.corner_radius").map(String::as_str), Some("12.0"));
        let (active, inactive) = scan_borders(src);
        assert_eq!(active.start.rgb.hex(), "#7cc4ff");
        assert_eq!(active.end.rgb.hex(), "#5b9dff");
        assert!((inactive.alpha - 0.18).abs() < 0.001);
        assert!(material_splash_block(src).contains("    glass: 1.0\n"));
        assert!(desk_splash_block(src).contains("    gaps_out: 12.0\n"));
        assert!(!theme_defines_shell(src));
        let p = scan_palette(src);
        assert!((p.accent.x - 0x5b as f32 / 255.0).abs() < 0.01);
        assert!(BUNDLED_MAKEOS_WALLPAPER.starts_with("<svg"));
        let mut vm = test_vm();
        assert!(eval_into(&mut vm, BUNDLED_MAKEOS_SPLASH));
        assert_eq!(hex(read(&mut vm, "mod.wm_theme.accent")), "#5b9dff");
        assert_eq!(read(&mut vm, "mod.wm_theme.material.glass").as_f64(), Some(1.0));
        assert_eq!(read(&mut vm, "mod.wm_theme.desk.corner_radius").as_f64(), Some(12.0));
        assert_eq!(hex(read(&mut vm, "mod.wm_theme.shell.bar.active")), "#ff6b81");
    }

    #[test]
    fn seeding_writes_both_bundled_themes_and_respects_edits() {
        let dir = std::env::temp_dir().join(format!("wm-themes-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        ensure_bundled_themes_in(&dir);
        assert_eq!(
            std::fs::read_to_string(dir.join("tokyo-night/theme.splash")).unwrap(),
            BUNDLED_TOKYO_NIGHT_SPLASH
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("makeos/theme.splash")).unwrap(),
            BUNDLED_MAKEOS_SPLASH
        );
        assert_eq!(
            backgrounds_in(&dir.join("makeos/backgrounds")),
            vec![dir.join("makeos/backgrounds/makeos.svg")]
        );
        // A person's edit survives as long as it carries the current marker.
        let edited = BUNDLED_MAKEOS_SPLASH.replace("accent: #5b9dff", "accent: #ff00ff");
        std::fs::write(dir.join("makeos/theme.splash"), &edited).unwrap();
        ensure_bundled_themes_in(&dir);
        assert_eq!(std::fs::read_to_string(dir.join("makeos/theme.splash")).unwrap(), edited);
        // An older format is reseeded.
        std::fs::write(dir.join("makeos/theme.splash"), edited.replace(THEME_FORMAT, "(format 0)")).unwrap();
        ensure_bundled_themes_in(&dir);
        assert_eq!(
            std::fs::read_to_string(dir.join("makeos/theme.splash")).unwrap(),
            BUNDLED_MAKEOS_SPLASH
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn backgrounds_accept_vector_and_raster_pictures_sorted() {
        let dir = std::env::temp_dir().join(format!("wm-bg-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // `C.jpg` sorts first by byte order and third by lowercased name.
        // (Not `B.jpg`: beside `b.jpg` it is the same file on a
        // case-insensitive filesystem, and a tie elsewhere.)
        for f in ["b.jpg", "a.svg", "c.txt", "d.PNG", "C.jpg"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        let got: Vec<String> = backgrounds_in(&dir)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(got, vec!["a.svg", "b.jpg", "C.jpg", "d.PNG"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
