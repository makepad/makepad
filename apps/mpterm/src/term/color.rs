//! Terminal colors: RGB, the 256-entry palette, and X11/XParseColor-style
//! color specification parsing. Port of ghostty `src/terminal/color.zig`
//! (trimmed: the full X11 rgb.txt name table is reduced to the names that
//! show up in practice from theming tools).

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Perceived luminance in [0,1], linearized. Used for contrast decisions
    /// (e.g. minimum-contrast cursor/text policies), same formula as ghostty.
    pub fn luminance(self) -> f32 {
        fn linearize(c: u8) -> f32 {
            let v = c as f32 / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * linearize(self.r) + 0.7152 * linearize(self.g) + 0.0722 * linearize(self.b)
    }
}

pub type Palette = [Rgb; 256];

/// The default 256-color palette: standard 16 (xterm defaults), the 6x6x6
/// color cube, and the 24-step grayscale ramp. Entries 0..16 are typically
/// overridden by the app theme; 16..256 are canonical.
pub fn default_palette() -> Palette {
    let mut palette = [Rgb::default(); 256];

    // Standard colors, xterm defaults (matches ghostty's color.zig).
    const BASE16: [Rgb; 16] = [
        Rgb::new(0x00, 0x00, 0x00), // black
        Rgb::new(0xcd, 0x00, 0x00), // red
        Rgb::new(0x00, 0xcd, 0x00), // green
        Rgb::new(0xcd, 0xcd, 0x00), // yellow
        Rgb::new(0x00, 0x00, 0xee), // blue
        Rgb::new(0xcd, 0x00, 0xcd), // magenta
        Rgb::new(0x00, 0xcd, 0xcd), // cyan
        Rgb::new(0xe5, 0xe5, 0xe5), // white
        Rgb::new(0x7f, 0x7f, 0x7f), // bright black
        Rgb::new(0xff, 0x00, 0x00), // bright red
        Rgb::new(0x00, 0xff, 0x00), // bright green
        Rgb::new(0xff, 0xff, 0x00), // bright yellow
        Rgb::new(0x5c, 0x5c, 0xff), // bright blue
        Rgb::new(0xff, 0x00, 0xff), // bright magenta
        Rgb::new(0x00, 0xff, 0xff), // bright cyan
        Rgb::new(0xff, 0xff, 0xff), // bright white
    ];
    palette[..16].copy_from_slice(&BASE16);

    // 6x6x6 cube.
    for r in 0..6usize {
        for g in 0..6usize {
            for b in 0..6usize {
                let idx = 16 + r * 36 + g * 6 + b;
                let c = |v: usize| -> u8 {
                    if v == 0 {
                        0
                    } else {
                        (v * 40 + 55) as u8
                    }
                };
                palette[idx] = Rgb::new(c(r), c(g), c(b));
            }
        }
    }

    // Grayscale ramp.
    for i in 0..24usize {
        let v = (i * 10 + 8) as u8;
        palette[232 + i] = Rgb::new(v, v, v);
    }

    palette
}

/// Parse an XParseColor-style specification, as used by OSC 4/10/11/12:
/// `#rgb`, `#rrggbb`, `#rrrgggbbb`, `#rrrrggggbbbb`, `rgb:r/g/b` with 1-4
/// hex digits per channel, `rgbi:r/g/b` with float channels, and a small
/// set of color names.
pub fn parse_color_spec(spec: &str) -> Option<Rgb> {
    let spec = spec.trim();

    if let Some(hex) = spec.strip_prefix('#') {
        let n = hex.len();
        if n % 3 != 0 || n == 0 || n > 12 {
            return None;
        }
        let d = n / 3;
        let chan = |s: &str| -> Option<u8> {
            let v = u16::from_str_radix(s, 16).ok()?;
            // X11 #-syntax: value is the MOST significant bits.
            Some(match d {
                1 => (v << 4 | v) as u8,
                2 => v as u8,
                3 => (v >> 4) as u8,
                4 => (v >> 8) as u8,
                _ => return None,
            })
        };
        return Some(Rgb::new(
            chan(&hex[0..d])?,
            chan(&hex[d..2 * d])?,
            chan(&hex[2 * d..3 * d])?,
        ));
    }

    if let Some(rest) = spec.strip_prefix("rgb:") {
        let mut it = rest.split('/');
        let chan = |s: &str| -> Option<u8> {
            if s.is_empty() || s.len() > 4 {
                return None;
            }
            let v = u16::from_str_radix(s, 16).ok()? as u32;
            let max = (1u32 << (s.len() * 4)) - 1;
            // Scale to 8 bits.
            Some(((v * 255 + max / 2) / max) as u8)
        };
        let r = chan(it.next()?)?;
        let g = chan(it.next()?)?;
        let b = chan(it.next()?)?;
        if it.next().is_some() {
            return None;
        }
        return Some(Rgb::new(r, g, b));
    }

    if let Some(rest) = spec.strip_prefix("rgbi:") {
        let mut it = rest.split('/');
        let chan = |s: &str| -> Option<u8> {
            let v: f32 = s.parse().ok()?;
            if !(0.0..=1.0).contains(&v) {
                return None;
            }
            Some((v * 255.0 + 0.5) as u8)
        };
        let r = chan(it.next()?)?;
        let g = chan(it.next()?)?;
        let b = chan(it.next()?)?;
        if it.next().is_some() {
            return None;
        }
        return Some(Rgb::new(r, g, b));
    }

    // Minimal name table.
    Some(match spec.to_ascii_lowercase().as_str() {
        "black" => Rgb::new(0, 0, 0),
        "white" => Rgb::new(255, 255, 255),
        "red" => Rgb::new(255, 0, 0),
        "green" => Rgb::new(0, 255, 0),
        "blue" => Rgb::new(0, 0, 255),
        "yellow" => Rgb::new(255, 255, 0),
        "magenta" => Rgb::new(255, 0, 255),
        "cyan" => Rgb::new(0, 255, 255),
        "gray" | "grey" => Rgb::new(190, 190, 190),
        _ => return None,
    })
}

/// Encode a color as the `rgb:rrrr/gggg/bbbb` 16-bit-per-channel reply form
/// used when answering OSC 4/10/11/12 queries (`?`), matching xterm/ghostty.
pub fn encode_color_reply(color: Rgb) -> String {
    format!(
        "rgb:{:04x}/{:04x}/{:04x}",
        color.r as u16 * 257,
        color.g as u16 * 257,
        color.b as u16 * 257
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_forms() {
        assert_eq!(parse_color_spec("#abc"), Some(Rgb::new(0xaa, 0xbb, 0xcc)));
        assert_eq!(
            parse_color_spec("#1a2b3c"),
            Some(Rgb::new(0x1a, 0x2b, 0x3c))
        );
        assert_eq!(
            parse_color_spec("#111222333"),
            Some(Rgb::new(0x11, 0x22, 0x33))
        );
        assert_eq!(
            parse_color_spec("#11112222ffff"),
            Some(Rgb::new(0x11, 0x22, 0xff))
        );
        assert_eq!(parse_color_spec("#12345"), None);
    }

    #[test]
    fn rgb_forms() {
        assert_eq!(parse_color_spec("rgb:f/f/f"), Some(Rgb::new(255, 255, 255)));
        assert_eq!(
            parse_color_spec("rgb:12/34/56"),
            Some(Rgb::new(0x12, 0x34, 0x56))
        );
        assert_eq!(
            parse_color_spec("rgb:ffff/0000/8080"),
            Some(Rgb::new(255, 0, 0x80))
        );
        assert_eq!(parse_color_spec("rgbi:1.0/0.0/0.5"), Some(Rgb::new(255, 0, 128)));
    }

    #[test]
    fn reply_roundtrip() {
        assert_eq!(
            encode_color_reply(Rgb::new(0x11, 0x22, 0x33)),
            "rgb:1111/2222/3333"
        );
    }

    #[test]
    fn cube_and_gray() {
        let p = default_palette();
        assert_eq!(p[16], Rgb::new(0, 0, 0));
        assert_eq!(p[231], Rgb::new(255, 255, 255));
        assert_eq!(p[232], Rgb::new(8, 8, 8));
        assert_eq!(p[255], Rgb::new(238, 238, 238));
    }
}
