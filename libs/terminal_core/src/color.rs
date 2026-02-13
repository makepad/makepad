/// RGB color
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
}

/// 256-color palette
pub struct Palette {
    pub colors: [Rgb; 256],
}

impl Palette {
    pub fn default_palette() -> Self {
        let mut colors = [Rgb::default(); 256];

        // Standard colors (0-7)
        colors[0] = Rgb::new(0x1d, 0x1f, 0x21); // black
        colors[1] = Rgb::new(0xcc, 0x66, 0x66); // red
        colors[2] = Rgb::new(0xb5, 0xbd, 0x68); // green
        colors[3] = Rgb::new(0xf0, 0xc6, 0x74); // yellow
        colors[4] = Rgb::new(0x81, 0xa2, 0xbe); // blue
        colors[5] = Rgb::new(0xb2, 0x94, 0xbb); // magenta
        colors[6] = Rgb::new(0x8a, 0xbe, 0xb7); // cyan
        colors[7] = Rgb::new(0xc5, 0xc8, 0xc6); // white

        // Bright colors (8-15)
        colors[8] = Rgb::new(0x66, 0x66, 0x66); // bright black
        colors[9] = Rgb::new(0xd5, 0x4e, 0x53); // bright red
        colors[10] = Rgb::new(0xb9, 0xca, 0x4a); // bright green
        colors[11] = Rgb::new(0xe7, 0xc5, 0x47); // bright yellow
        colors[12] = Rgb::new(0x7a, 0xa6, 0xda); // bright blue
        colors[13] = Rgb::new(0xc3, 0x97, 0xd8); // bright magenta
        colors[14] = Rgb::new(0x70, 0xc0, 0xb1); // bright cyan
        colors[15] = Rgb::new(0xea, 0xea, 0xea); // bright white

        // 216-color cube (16-231): 6x6x6 RGB
        for r in 0..6u8 {
            for g in 0..6u8 {
                for b in 0..6u8 {
                    let idx = 16 + (r as usize) * 36 + (g as usize) * 6 + (b as usize);
                    colors[idx] = Rgb::new(
                        if r == 0 { 0 } else { r * 40 + 55 },
                        if g == 0 { 0 } else { g * 40 + 55 },
                        if b == 0 { 0 } else { b * 40 + 55 },
                    );
                }
            }
        }

        // Grayscale ramp (232-255): 24 shades
        for i in 0..24u8 {
            let v = i * 10 + 8;
            colors[232 + i as usize] = Rgb::new(v, v, v);
        }

        Self { colors }
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self::default_palette()
    }
}
