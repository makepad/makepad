/// SVG unit/length parser and viewBox coordinate resolver.
use crate::document::ViewBox;

pub fn parse_length(s: &str) -> Option<f32> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // Try stripping known unit suffixes
    for (suffix, scale) in &[
        ("px", 1.0f32),
        ("pt", 1.333_333_3), // 1pt = 1.333px (96/72)
        ("pc", 16.0),        // 1pc = 16px
        ("mm", 3.779_528),   // 1mm = 3.7795px (96/25.4)
        ("cm", 37.795_28),   // 1cm = 37.795px
        ("in", 96.0),        // 1in = 96px
    ] {
        if let Some(num) = s.strip_suffix(suffix) {
            return num.trim().parse::<f32>().ok().map(|v| v * scale);
        }
    }
    // Percentage handling deferred (needs context)
    if s.ends_with('%') {
        return None; // caller must handle percentages
    }
    // Try bare number
    s.parse::<f32>().ok()
}

pub fn parse_length_or_percent(s: &str, reference: f32) -> Option<f32> {
    let s = s.trim();
    if let Some(pct) = s.strip_suffix('%') {
        return pct
            .trim()
            .parse::<f32>()
            .ok()
            .map(|v| v / 100.0 * reference);
    }
    parse_length(s)
}

pub fn parse_number(s: &str) -> Option<f32> {
    s.trim().parse::<f32>().ok()
}

pub fn parse_viewbox(s: &str) -> Option<ViewBox> {
    let parts: Vec<f32> = s
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .filter_map(|p| p.parse::<f32>().ok())
        .collect();
    if parts.len() == 4 {
        Some(ViewBox {
            x: parts[0],
            y: parts[1],
            width: parts[2],
            height: parts[3],
        })
    } else {
        None
    }
}

pub fn parse_points(s: &str) -> Vec<(f32, f32)> {
    let nums: Vec<f32> = s
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .filter_map(|p| p.parse::<f32>().ok())
        .collect();
    nums.chunks(2)
        .filter(|c| c.len() == 2)
        .map(|c| (c[0], c[1]))
        .collect()
}

/// Compute the transform from viewBox coordinates to a target (w, h) viewport.
/// Returns (scale_x, scale_y, translate_x, translate_y).
pub fn viewbox_transform(vb: &ViewBox, target_w: f32, target_h: f32) -> (f32, f32, f32, f32) {
    if vb.width <= 0.0 || vb.height <= 0.0 {
        return (1.0, 1.0, 0.0, 0.0);
    }
    // Default preserveAspectRatio: xMidYMid meet
    let sx = target_w / vb.width;
    let sy = target_h / vb.height;
    let s = sx.min(sy); // "meet" => use smaller scale
    let tx = (target_w - vb.width * s) * 0.5 - vb.x * s;
    let ty = (target_h - vb.height * s) * 0.5 - vb.y * s;
    (s, s, tx, ty)
}

pub fn parse_duration(s: &str) -> Option<f32> {
    let s = s.trim();
    if s == "indefinite" {
        return Some(f32::INFINITY);
    }
    if let Some(ms) = s.strip_suffix("ms") {
        return ms.trim().parse::<f32>().ok().map(|v| v / 1000.0);
    }
    if let Some(sec) = s.strip_suffix('s') {
        return sec.trim().parse::<f32>().ok();
    }
    // Try "min" suffix
    if let Some(min) = s.strip_suffix("min") {
        return min.trim().parse::<f32>().ok().map(|v| v * 60.0);
    }
    // Try "h" suffix
    if let Some(h) = s.strip_suffix('h') {
        return h.trim().parse::<f32>().ok().map(|v| v * 3600.0);
    }
    // Clock-value: hh:mm:ss or mm:ss
    if s.contains(':') {
        let parts: Vec<&str> = s.split(':').collect();
        match parts.len() {
            2 => {
                let min = parts[0].parse::<f32>().ok()?;
                let sec = parts[1].parse::<f32>().ok()?;
                return Some(min * 60.0 + sec);
            }
            3 => {
                let h = parts[0].parse::<f32>().ok()?;
                let min = parts[1].parse::<f32>().ok()?;
                let sec = parts[2].parse::<f32>().ok()?;
                return Some(h * 3600.0 + min * 60.0 + sec);
            }
            _ => return None,
        }
    }
    // Bare number = seconds
    s.parse::<f32>().ok()
}
