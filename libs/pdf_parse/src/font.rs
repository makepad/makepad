use crate::page::{FontEncoding, FontResource};

/// Get the width of a character code in a font (in PDF text space units, i.e. 1/1000 of text size).
pub fn char_width(font: &FontResource, char_code: u32) -> f64 {
    // Try font-specific widths first
    if char_code >= font.first_char && char_code <= font.last_char {
        let idx = (char_code - font.first_char) as usize;
        if idx < font.widths.len() {
            return font.widths[idx];
        }
    }

    // Fall back to base-14 metrics
    if let Some(w) = base14_char_width(&font.base_font, char_code) {
        return w;
    }

    font.default_width
}

/// Decode a byte sequence to a Unicode string using the font's encoding.
pub fn decode_text(font: &FontResource, bytes: &[u8]) -> String {
    // Try ToUnicode CMap first
    if let Some(ref cmap) = font.to_unicode {
        let mut result = String::new();
        // Check if this is a 2-byte encoding (CIDFont / Identity-H)
        let is_two_byte =
            matches!(font.encoding, FontEncoding::Identity) || font.subtype == "Type0";

        if is_two_byte && bytes.len() >= 2 {
            let mut i = 0;
            while i + 1 < bytes.len() {
                let code = ((bytes[i] as u32) << 8) | (bytes[i + 1] as u32);
                if let Some(s) = cmap.mappings.get(&code) {
                    result.push_str(s);
                } else if let Some(ch) = char::from_u32(code) {
                    result.push(ch);
                } else {
                    result.push('\u{FFFD}');
                }
                i += 2;
            }
            // Handle odd trailing byte
            if i < bytes.len() {
                let code = bytes[i] as u32;
                if let Some(s) = cmap.mappings.get(&code) {
                    result.push_str(s);
                }
            }
        } else {
            for &b in bytes {
                let code = b as u32;
                if let Some(s) = cmap.mappings.get(&code) {
                    result.push_str(s);
                } else {
                    result.push(winansi_to_char(b));
                }
            }
        }
        return result;
    }

    // Fall back to encoding-based decoding
    match &font.encoding {
        FontEncoding::WinAnsi | FontEncoding::Standard => {
            bytes.iter().map(|&b| winansi_to_char(b)).collect()
        }
        FontEncoding::MacRoman => bytes.iter().map(|&b| macroman_to_char(b)).collect(),
        FontEncoding::Identity => {
            // Two-byte identity encoding
            let mut result = String::new();
            let mut i = 0;
            while i + 1 < bytes.len() {
                let code = ((bytes[i] as u32) << 8) | (bytes[i + 1] as u32);
                if let Some(ch) = char::from_u32(code) {
                    result.push(ch);
                }
                i += 2;
            }
            result
        }
        FontEncoding::Custom(_) => {
            // For custom encodings, just use WinAnsi as a reasonable fallback
            bytes.iter().map(|&b| winansi_to_char(b)).collect()
        }
    }
}

/// Get the width of a char in glyph widths for a 2-byte encoded font.
pub fn char_width_2byte(font: &FontResource, hi: u8, lo: u8) -> f64 {
    let code = ((hi as u32) << 8) | (lo as u32);
    char_width(font, code)
}

/// WinAnsi encoding to Unicode.
fn winansi_to_char(b: u8) -> char {
    // WinAnsi is mostly ISO-8859-1 except for 0x80-0x9F range
    match b {
        0x80 => '\u{20AC}', // Euro sign
        0x82 => '\u{201A}', // Single low-9 quotation mark
        0x83 => '\u{0192}', // Latin small f with hook
        0x84 => '\u{201E}', // Double low-9 quotation mark
        0x85 => '\u{2026}', // Horizontal ellipsis
        0x86 => '\u{2020}', // Dagger
        0x87 => '\u{2021}', // Double dagger
        0x88 => '\u{02C6}', // Modifier letter circumflex accent
        0x89 => '\u{2030}', // Per mille sign
        0x8A => '\u{0160}', // Latin capital S with caron
        0x8B => '\u{2039}', // Single left-pointing angle quotation mark
        0x8C => '\u{0152}', // Latin capital ligature OE
        0x8E => '\u{017D}', // Latin capital Z with caron
        0x91 => '\u{2018}', // Left single quotation mark
        0x92 => '\u{2019}', // Right single quotation mark
        0x93 => '\u{201C}', // Left double quotation mark
        0x94 => '\u{201D}', // Right double quotation mark
        0x95 => '\u{2022}', // Bullet
        0x96 => '\u{2013}', // En dash
        0x97 => '\u{2014}', // Em dash
        0x98 => '\u{02DC}', // Small tilde
        0x99 => '\u{2122}', // Trade mark sign
        0x9A => '\u{0161}', // Latin small s with caron
        0x9B => '\u{203A}', // Single right-pointing angle quotation mark
        0x9C => '\u{0153}', // Latin small ligature oe
        0x9E => '\u{017E}', // Latin small z with caron
        0x9F => '\u{0178}', // Latin capital Y with diaeresis
        b => b as char,
    }
}

/// MacRoman encoding to Unicode (simplified).
fn macroman_to_char(b: u8) -> char {
    if b < 0x80 {
        return b as char;
    }
    // Simplified: just map high bytes to replacement char if we don't have a full table
    static MACROMAN_HIGH: [u16; 128] = [
        0x00C4, 0x00C5, 0x00C7, 0x00C9, 0x00D1, 0x00D6, 0x00DC, 0x00E1, 0x00E0, 0x00E2, 0x00E4,
        0x00E3, 0x00E5, 0x00E7, 0x00E9, 0x00E8, 0x00EA, 0x00EB, 0x00ED, 0x00EC, 0x00EE, 0x00EF,
        0x00F1, 0x00F3, 0x00F2, 0x00F4, 0x00F6, 0x00F5, 0x00FA, 0x00F9, 0x00FB, 0x00FC, 0x2020,
        0x00B0, 0x00A2, 0x00A3, 0x00A7, 0x2022, 0x00B6, 0x00DF, 0x00AE, 0x00A9, 0x2122, 0x00B4,
        0x00A8, 0x2260, 0x00C6, 0x00D8, 0x221E, 0x00B1, 0x2264, 0x2265, 0x00A5, 0x00B5, 0x2202,
        0x2211, 0x220F, 0x03C0, 0x222B, 0x00AA, 0x00BA, 0x03A9, 0x00E6, 0x00F8, 0x00BF, 0x00A1,
        0x00AC, 0x221A, 0x0192, 0x2248, 0x2206, 0x00AB, 0x00BB, 0x2026, 0x00A0, 0x00C0, 0x00C3,
        0x00D5, 0x0152, 0x0153, 0x2013, 0x2014, 0x201C, 0x201D, 0x2018, 0x2019, 0x00F7, 0x25CA,
        0x00FF, 0x0178, 0x2044, 0x20AC, 0x2039, 0x203A, 0xFB01, 0xFB02, 0x2021, 0x00B7, 0x201A,
        0x201E, 0x2030, 0x00C2, 0x00CA, 0x00C1, 0x00CB, 0x00C8, 0x00CD, 0x00CE, 0x00CF, 0x00CC,
        0x00D3, 0x00D4, 0xF8FF, 0x00D2, 0x00DA, 0x00DB, 0x00D9, 0x0131, 0x02C6, 0x02DC, 0x00AF,
        0x02D8, 0x02D9, 0x02DA, 0x00B8, 0x02DD, 0x02DB, 0x02C7,
    ];
    char::from_u32(MACROMAN_HIGH[(b - 0x80) as usize] as u32).unwrap_or('\u{FFFD}')
}

/// Base-14 font approximate character widths (in 1/1000 units).
/// Returns width for common ASCII chars in standard PDF fonts.
fn base14_char_width(base_font: &str, char_code: u32) -> Option<f64> {
    // Simplified: use average widths for base-14 font families
    let is_mono = base_font.contains("Courier");
    let is_narrow = base_font.contains("Narrow") || base_font.contains("Condensed");

    if is_mono {
        return Some(600.0);
    }

    if char_code > 255 {
        return None;
    }

    let scale = if is_narrow { 0.82 } else { 1.0 };

    // Approximate widths for Helvetica-like proportional fonts
    let w = match char_code as u8 {
        b' ' => 278.0,
        b'!' => 278.0,
        b'"' => 355.0,
        b'#' => 556.0,
        b'$' => 556.0,
        b'%' => 889.0,
        b'&' => 667.0,
        b'\'' => 191.0,
        b'(' | b')' => 333.0,
        b'*' => 389.0,
        b'+' => 584.0,
        b',' => 278.0,
        b'-' => 333.0,
        b'.' => 278.0,
        b'/' => 278.0,
        b'0'..=b'9' => 556.0,
        b':' | b';' => 278.0,
        b'<' | b'=' | b'>' => 584.0,
        b'?' => 556.0,
        b'@' => 1015.0,
        b'A' | b'V' => 667.0,
        b'B' | b'E' | b'F' | b'P' => 667.0,
        b'C' | b'G' | b'O' | b'Q' => 722.0,
        b'D' | b'H' | b'N' | b'U' => 722.0,
        b'I' => 278.0,
        b'J' | b'S' => 556.0,
        b'K' | b'R' | b'X' | b'Y' | b'Z' => 667.0,
        b'L' => 556.0,
        b'M' | b'W' => 833.0,
        b'T' => 611.0,
        b'[' | b']' => 278.0,
        b'\\' => 278.0,
        b'^' => 469.0,
        b'_' => 556.0,
        b'`' => 333.0,
        b'a' | b'e' | b'o' => 556.0,
        b'b' | b'd' | b'p' | b'q' => 556.0,
        b'c' => 500.0,
        b'f' => 278.0,
        b'g' => 556.0,
        b'h' | b'n' | b'u' => 556.0,
        b'i' | b'l' => 222.0,
        b'j' => 222.0,
        b'k' => 500.0,
        b'm' => 833.0,
        b'r' => 333.0,
        b's' => 500.0,
        b't' => 278.0,
        b'v' | b'x' | b'y' | b'z' => 500.0,
        b'w' => 722.0,
        b'{' | b'}' => 334.0,
        b'|' => 260.0,
        b'~' => 584.0,
        _ => 556.0, // default for other chars
    };

    // Adjust for Times-like (serif) fonts which tend to be slightly narrower
    let is_serif = base_font.contains("Times") || base_font.contains("Serif");
    let serif_scale = if is_serif { 0.92 } else { 1.0 };

    Some(w * scale * serif_scale)
}
