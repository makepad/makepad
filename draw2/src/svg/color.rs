/// SVG color parser: named colors, hex (#rgb, #rrggbb, #rrggbbaa), rgb(), rgba(), hsl()

pub fn parse_color(s: &str) -> Option<(f32, f32, f32, f32)> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("none") || s.eq_ignore_ascii_case("transparent") {
        return None;
    }
    if s.eq_ignore_ascii_case("currentColor") || s.eq_ignore_ascii_case("inherit") {
        return Some((0.0, 0.0, 0.0, 1.0)); // fallback to black
    }
    if let Some(hex) = s.strip_prefix('#') {
        return parse_hex(hex);
    }
    if let Some(inner) = strip_func(s, "rgb") {
        return parse_rgb_func(inner);
    }
    if let Some(inner) = strip_func(s, "rgba") {
        return parse_rgba_func(inner);
    }
    if let Some(inner) = strip_func(s, "hsl") {
        return parse_hsl_func(inner);
    }
    if let Some(inner) = strip_func(s, "hsla") {
        return parse_hsla_func(inner);
    }
    named_color(s)
}

fn strip_func<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    let s_lc = s.to_ascii_lowercase();
    if s_lc.starts_with(name) {
        let rest = &s[name.len()..];
        let rest = rest.trim_start();
        if let Some(inner) = rest.strip_prefix('(') {
            if let Some(inner) = inner.strip_suffix(')') {
                return Some(inner);
            }
        }
    }
    None
}

fn parse_hex(hex: &str) -> Option<(f32, f32, f32, f32)> {
    match hex.len() {
        3 => {
            let r = u8_from_hex_char(hex.as_bytes()[0])?;
            let g = u8_from_hex_char(hex.as_bytes()[1])?;
            let b = u8_from_hex_char(hex.as_bytes()[2])?;
            Some((
                (r * 17) as f32 / 255.0,
                (g * 17) as f32 / 255.0,
                (b * 17) as f32 / 255.0,
                1.0,
            ))
        }
        4 => {
            let r = u8_from_hex_char(hex.as_bytes()[0])?;
            let g = u8_from_hex_char(hex.as_bytes()[1])?;
            let b = u8_from_hex_char(hex.as_bytes()[2])?;
            let a = u8_from_hex_char(hex.as_bytes()[3])?;
            Some((
                (r * 17) as f32 / 255.0,
                (g * 17) as f32 / 255.0,
                (b * 17) as f32 / 255.0,
                (a * 17) as f32 / 255.0,
            ))
        }
        6 => {
            let r = u8_from_hex2(&hex[0..2])?;
            let g = u8_from_hex2(&hex[2..4])?;
            let b = u8_from_hex2(&hex[4..6])?;
            Some((r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0))
        }
        8 => {
            let r = u8_from_hex2(&hex[0..2])?;
            let g = u8_from_hex2(&hex[2..4])?;
            let b = u8_from_hex2(&hex[4..6])?;
            let a = u8_from_hex2(&hex[6..8])?;
            Some((
                r as f32 / 255.0,
                g as f32 / 255.0,
                b as f32 / 255.0,
                a as f32 / 255.0,
            ))
        }
        _ => None,
    }
}

fn u8_from_hex_char(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn u8_from_hex2(s: &str) -> Option<u8> {
    let hi = u8_from_hex_char(s.as_bytes()[0])?;
    let lo = u8_from_hex_char(s.as_bytes()[1])?;
    Some(hi * 16 + lo)
}

fn split_components(s: &str) -> Vec<&str> {
    // Split on comma or whitespace
    s.split(|c: char| c == ',' || c == '/')
        .flat_map(|p| p.split_whitespace())
        .filter(|p| !p.is_empty())
        .collect()
}

fn parse_color_component(s: &str) -> Option<f32> {
    let s = s.trim();
    if let Some(pct) = s.strip_suffix('%') {
        pct.trim()
            .parse::<f32>()
            .ok()
            .map(|v| (v / 100.0).clamp(0.0, 1.0))
    } else {
        s.parse::<f32>().ok().map(|v| (v / 255.0).clamp(0.0, 1.0))
    }
}

fn parse_alpha_component(s: &str) -> Option<f32> {
    let s = s.trim();
    if let Some(pct) = s.strip_suffix('%') {
        pct.trim()
            .parse::<f32>()
            .ok()
            .map(|v| (v / 100.0).clamp(0.0, 1.0))
    } else {
        s.parse::<f32>().ok().map(|v| v.clamp(0.0, 1.0))
    }
}

fn parse_rgb_func(s: &str) -> Option<(f32, f32, f32, f32)> {
    let parts = split_components(s);
    if parts.len() < 3 {
        return None;
    }
    let r = parse_color_component(parts[0])?;
    let g = parse_color_component(parts[1])?;
    let b = parse_color_component(parts[2])?;
    let a = if parts.len() >= 4 {
        parse_alpha_component(parts[3])?
    } else {
        1.0
    };
    Some((r, g, b, a))
}

fn parse_rgba_func(s: &str) -> Option<(f32, f32, f32, f32)> {
    let parts = split_components(s);
    if parts.len() < 4 {
        return None;
    }
    let r = parse_color_component(parts[0])?;
    let g = parse_color_component(parts[1])?;
    let b = parse_color_component(parts[2])?;
    let a = parse_alpha_component(parts[3])?;
    Some((r, g, b, a))
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    if s == 0.0 {
        return (l, l, l);
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let hue_to_rgb = |t: f32| -> f32 {
        let mut t = t;
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            return p + (q - p) * 6.0 * t;
        }
        if t < 1.0 / 2.0 {
            return q;
        }
        if t < 2.0 / 3.0 {
            return p + (q - p) * (2.0 / 3.0 - t) * 6.0;
        }
        p
    };
    (
        hue_to_rgb(h + 1.0 / 3.0),
        hue_to_rgb(h),
        hue_to_rgb(h - 1.0 / 3.0),
    )
}

fn parse_hsl_func(s: &str) -> Option<(f32, f32, f32, f32)> {
    let parts = split_components(s);
    if parts.len() < 3 {
        return None;
    }
    let h = parts[0].trim_end_matches("deg").parse::<f32>().ok()? / 360.0;
    let s_val = parts[1].trim_end_matches('%').parse::<f32>().ok()? / 100.0;
    let l = parts[2].trim_end_matches('%').parse::<f32>().ok()? / 100.0;
    let (r, g, b) = hsl_to_rgb(h, s_val, l);
    let a = if parts.len() >= 4 {
        parse_alpha_component(parts[3])?
    } else {
        1.0
    };
    Some((r, g, b, a))
}

fn parse_hsla_func(s: &str) -> Option<(f32, f32, f32, f32)> {
    let parts = split_components(s);
    if parts.len() < 4 {
        return None;
    }
    let h = parts[0].trim_end_matches("deg").parse::<f32>().ok()? / 360.0;
    let s_val = parts[1].trim_end_matches('%').parse::<f32>().ok()? / 100.0;
    let l = parts[2].trim_end_matches('%').parse::<f32>().ok()? / 100.0;
    let (r, g, b) = hsl_to_rgb(h, s_val, l);
    let a = parse_alpha_component(parts[3])?;
    Some((r, g, b, a))
}

pub fn named_color(name: &str) -> Option<(f32, f32, f32, f32)> {
    let hex = match name.to_ascii_lowercase().as_str() {
        "aliceblue" => 0xF0F8FF,
        "antiquewhite" => 0xFAEBD7,
        "aqua" => 0x00FFFF,
        "aquamarine" => 0x7FFFD4,
        "azure" => 0xF0FFFF,
        "beige" => 0xF5F5DC,
        "bisque" => 0xFFE4C4,
        "black" => 0x000000,
        "blanchedalmond" => 0xFFEBCD,
        "blue" => 0x0000FF,
        "blueviolet" => 0x8A2BE2,
        "brown" => 0xA52A2A,
        "burlywood" => 0xDEB887,
        "cadetblue" => 0x5F9EA0,
        "chartreuse" => 0x7FFF00,
        "chocolate" => 0xD2691E,
        "coral" => 0xFF7F50,
        "cornflowerblue" => 0x6495ED,
        "cornsilk" => 0xFFF8DC,
        "crimson" => 0xDC143C,
        "cyan" => 0x00FFFF,
        "darkblue" => 0x00008B,
        "darkcyan" => 0x008B8B,
        "darkgoldenrod" => 0xB8860B,
        "darkgray" | "darkgrey" => 0xA9A9A9,
        "darkgreen" => 0x006400,
        "darkkhaki" => 0xBDB76B,
        "darkmagenta" => 0x8B008B,
        "darkolivegreen" => 0x556B2F,
        "darkorange" => 0xFF8C00,
        "darkorchid" => 0x9932CC,
        "darkred" => 0x8B0000,
        "darksalmon" => 0xE9967A,
        "darkseagreen" => 0x8FBC8F,
        "darkslateblue" => 0x483D8B,
        "darkslategray" | "darkslategrey" => 0x2F4F4F,
        "darkturquoise" => 0x00CED1,
        "darkviolet" => 0x9400D3,
        "deeppink" => 0xFF1493,
        "deepskyblue" => 0x00BFFF,
        "dimgray" | "dimgrey" => 0x696969,
        "dodgerblue" => 0x1E90FF,
        "firebrick" => 0xB22222,
        "floralwhite" => 0xFFFAF0,
        "forestgreen" => 0x228B22,
        "fuchsia" => 0xFF00FF,
        "gainsboro" => 0xDCDCDC,
        "ghostwhite" => 0xF8F8FF,
        "gold" => 0xFFD700,
        "goldenrod" => 0xDAA520,
        "gray" | "grey" => 0x808080,
        "green" => 0x008000,
        "greenyellow" => 0xADFF2F,
        "honeydew" => 0xF0FFF0,
        "hotpink" => 0xFF69B4,
        "indianred" => 0xCD5C5C,
        "indigo" => 0x4B0082,
        "ivory" => 0xFFFFF0,
        "khaki" => 0xF0E68C,
        "lavender" => 0xE6E6FA,
        "lavenderblush" => 0xFFF0F5,
        "lawngreen" => 0x7CFC00,
        "lemonchiffon" => 0xFFFACD,
        "lightblue" => 0xADD8E6,
        "lightcoral" => 0xF08080,
        "lightcyan" => 0xE0FFFF,
        "lightgoldenrodyellow" => 0xFAFAD2,
        "lightgray" | "lightgrey" => 0xD3D3D3,
        "lightgreen" => 0x90EE90,
        "lightpink" => 0xFFB6C1,
        "lightsalmon" => 0xFFA07A,
        "lightseagreen" => 0x20B2AA,
        "lightskyblue" => 0x87CEFA,
        "lightslategray" | "lightslategrey" => 0x778899,
        "lightsteelblue" => 0xB0C4DE,
        "lightyellow" => 0xFFFFE0,
        "lime" => 0x00FF00,
        "limegreen" => 0x32CD32,
        "linen" => 0xFAF0E6,
        "magenta" => 0xFF00FF,
        "maroon" => 0x800000,
        "mediumaquamarine" => 0x66CDAA,
        "mediumblue" => 0x0000CD,
        "mediumorchid" => 0xBA55D3,
        "mediumpurple" => 0x9370DB,
        "mediumseagreen" => 0x3CB371,
        "mediumslateblue" => 0x7B68EE,
        "mediumspringgreen" => 0x00FA9A,
        "mediumturquoise" => 0x48D1CC,
        "mediumvioletred" => 0xC71585,
        "midnightblue" => 0x191970,
        "mintcream" => 0xF5FFFA,
        "mistyrose" => 0xFFE4E1,
        "moccasin" => 0xFFE4B5,
        "navajowhite" => 0xFFDEAD,
        "navy" => 0x000080,
        "oldlace" => 0xFDF5E6,
        "olive" => 0x808000,
        "olivedrab" => 0x6B8E23,
        "orange" => 0xFFA500,
        "orangered" => 0xFF4500,
        "orchid" => 0xDA70D6,
        "palegoldenrod" => 0xEEE8AA,
        "palegreen" => 0x98FB98,
        "paleturquoise" => 0xAFEEEE,
        "palevioletred" => 0xDB7093,
        "papayawhip" => 0xFFEFD5,
        "peachpuff" => 0xFFDAB9,
        "peru" => 0xCD853F,
        "pink" => 0xFFC0CB,
        "plum" => 0xDDA0DD,
        "powderblue" => 0xB0E0E6,
        "purple" => 0x800080,
        "rebeccapurple" => 0x663399,
        "red" => 0xFF0000,
        "rosybrown" => 0xBC8F8F,
        "royalblue" => 0x4169E1,
        "saddlebrown" => 0x8B4513,
        "salmon" => 0xFA8072,
        "sandybrown" => 0xF4A460,
        "seagreen" => 0x2E8B57,
        "seashell" => 0xFFF5EE,
        "sienna" => 0xA0522D,
        "silver" => 0xC0C0C0,
        "skyblue" => 0x87CEEB,
        "slateblue" => 0x6A5ACD,
        "slategray" | "slategrey" => 0x708090,
        "snow" => 0xFFFAFA,
        "springgreen" => 0x00FF7F,
        "steelblue" => 0x4682B4,
        "tan" => 0xD2B48C,
        "teal" => 0x008080,
        "thistle" => 0xD8BFD8,
        "tomato" => 0xFF6347,
        "turquoise" => 0x40E0D0,
        "violet" => 0xEE82EE,
        "wheat" => 0xF5DEB3,
        "white" => 0xFFFFFF,
        "whitesmoke" => 0xF5F5F5,
        "yellow" => 0xFFFF00,
        "yellowgreen" => 0x9ACD32,
        _ => return None,
    };
    let r = ((hex >> 16) & 0xFF) as f32 / 255.0;
    let g = ((hex >> 8) & 0xFF) as f32 / 255.0;
    let b = (hex & 0xFF) as f32 / 255.0;
    Some((r, g, b, 1.0))
}
