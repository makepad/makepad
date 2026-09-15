//! Colours the map paints with. Tokyo Night is the standalone default; a host
//! that already has a theme (the files app) can push its own hexes in.

use makepad_widgets::*;

/// The hues the disk map draws with.
#[derive(Clone, Debug)]
pub struct MapPalette {
    pub accent: String,
    pub bg: String,
    pub bg_dark: String,
    pub fg: String,
    pub fg_bright: String,
    pub fg_dim: String,
    pub muted: String,
    /// video, image, audio, code, text, archive, other
    pub kinds: [String; 7],
}

const KIND_FALLBACKS: [&str; 7] = [
    "#7aa2f7", // video   — blue
    "#9ece6a", // image   — green
    "#e0af68", // audio   — yellow
    "#7dcfff", // code    — cyan
    "#a9b1d6", // text    — foreground
    "#bb9af7", // archive — magenta
    "#414868", // other   — muted
];

impl Default for MapPalette {
    fn default() -> Self {
        Self::tokyo_night()
    }
}

impl MapPalette {
    /// The fallback theme, matching the files app's standalone look.
    pub fn tokyo_night() -> Self {
        Self {
            accent: "#7aa2f7".to_string(),
            bg: "#1a1b26".to_string(),
            bg_dark: "#16161e".to_string(),
            fg: "#a9b1d6".to_string(),
            fg_bright: "#c0caf5".to_string(),
            fg_dim: "#565f89".to_string(),
            muted: "#414868".to_string(),
            kinds: KIND_FALLBACKS.map(|s| s.to_string()),
        }
    }

    /// Build from the same hex strings the files app's `Palette` carries.
    pub fn from_hexes(
        accent: &str,
        bg: &str,
        bg_dark: &str,
        fg: &str,
        fg_bright: &str,
        fg_dim: &str,
        muted: &str,
        kinds: [String; 7],
    ) -> Self {
        Self {
            accent: accent.to_string(),
            bg: bg.to_string(),
            bg_dark: bg_dark.to_string(),
            fg: fg.to_string(),
            fg_bright: fg_bright.to_string(),
            fg_dim: fg_dim.to_string(),
            muted: muted.to_string(),
            kinds,
        }
    }

    /// The fill for one treemap kind class, by its index in [`MapPalette::kinds`].
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_treemap_kind_has_its_own_hue() {
        let p = MapPalette::tokyo_night();
        let mut seen: Vec<&str> = p.kinds.iter().map(String::as_str).collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), p.kinds.len(), "kind colors must be distinct");
        for class in 0..p.kinds.len() {
            assert_eq!(p.kind_color(class), MapPalette::vec4(&p.kinds[class]));
        }
        assert_eq!(p.kind_color(99), p.kind_color(p.kinds.len() - 1));
    }

    #[test]
    fn parses_hex() {
        assert_eq!(rgb("#7aa2f7"), (0x7a, 0xa2, 0xf7));
        assert_eq!(rgb("7aa2f7"), (0x7a, 0xa2, 0xf7));
        assert_eq!(rgb("bad"), (0, 0, 0));
        let v = MapPalette::vec4("#ff8000");
        assert!((v.x - 1.0).abs() < 0.001);
        assert!((v.z - 0.0).abs() < 0.001);
    }
}
