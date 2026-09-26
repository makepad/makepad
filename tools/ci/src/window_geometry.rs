//! Resolve display geometry before Makepad creates a window; no platform API.
use crate::process::Result;
use makepad_toml_parser::{parse_toml, Toml};
use std::{path::Path, process::Command, sync::OnceLock};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry { pub x: f64, pub y: f64, pub width: f64, pub height: f64 }
const FALLBACK: Geometry = Geometry { x: 0.0, y: 0.0, width: 960.0, height: 1000.0 };
static GEOMETRY: OnceLock<Geometry> = OnceLock::new();
pub fn geometry() -> Geometry { GEOMETRY.get().copied().unwrap_or(FALLBACK) }

pub fn configured(text: &str) -> Result<Option<Geometry>> {
    let doc = parse_toml(text).map_err(|e| e.to_string())?;
    let Some(value) = doc.root.get("window") else { return Ok(None); };
    let Toml::Array(values) = value else { return Err("window must be [x, y, w, h]".into()); };
    if values.len() != 4 { return Err("window must contain four numbers [x, y, w, h]".into()); }
    let n: Vec<_> = values.iter().map(|v| v.as_num().ok_or("window must contain numbers"))
        .collect::<std::result::Result<_, _>>()?;
    if n.iter().any(|v| !v.is_finite()) || n[2] <= 0.0 || n[3] <= 0.0 {
        return Err("window requires finite coordinates and positive width/height".into());
    }
    Ok(Some(Geometry { x: n[0], y: n[1], width: n[2], height: n[3] }))
}
fn dimensions(text: &str) -> Option<(f64, f64)> {
    let mut parts = text.split_whitespace();
    let w: f64 = parts.next()?.parse().ok()?;
    if !matches!(parts.next()?, "x" | "×") { return None; }
    let h: f64 = parts.next()?.parse().ok()?;
    (w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0).then_some((w, h))
}
pub fn parse_display(text: &str) -> Option<Geometry> {
    #[derive(Default)]
    struct Display { main: bool, ui: Option<(f64, f64)>, resolution: Option<(f64, f64)> }
    let mut displays = Vec::new();
    let mut current = Display::default();
    for line in text.lines() {
        let line = line.trim();
        // system_profiler separates each display with a heading ending in ':'.
        if line.ends_with(':') {
            if current.ui.is_some() || current.resolution.is_some() {
                displays.push(current);
            }
            current = Display::default();
        } else if let Some(value) = line.strip_prefix("UI Looks like:") {
            current.ui = dimensions(value);
        } else if let Some(value) = line.strip_prefix("Resolution:") {
            current.resolution = dimensions(value).map(|(w, h)| {
                if value.contains("Retina") { (w / 2.0, h / 2.0) } else { (w, h) }
            });
        } else if let Some(value) = line.strip_prefix("Main Display:") {
            current.main = value.trim() == "Yes";
        }
    }
    displays.push(current);
    let display = displays.iter().find(|d| d.main)?;
    let (w, h) = display.ui.or(display.resolution)?;
    Some(Geometry { x: 0.0, y: 0.0, width: w / 2.0, height: h })
}
pub fn initialize(base: &Path) {
    GEOMETRY.get_or_init(|| {
        match std::fs::read_to_string(base.join("local/ci/ci.toml")) {
            Ok(text) => match configured(&text) {
                Ok(Some(window)) => return window,
                Ok(None) => {}
                Err(e) => eprintln!("ci window config: {e}"),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => eprintln!("ci window config: {e}"),
        }
        Command::new("/usr/sbin/system_profiler").arg("SPDisplaysDataType")
            .env("LC_ALL", "C").env("LANG", "C").output().ok()
            .filter(|output| output.status.success())
            .and_then(|output| parse_display(&String::from_utf8_lossy(&output.stdout)))
            .unwrap_or(FALLBACK)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn main_display_points_retina_and_config_override() {
        let text = "Graphics/Displays:\n    Other:\n      Resolution: 3840 x 2160\n      Main Display: No\n    Built-In:\n      Resolution: 3024 x 1964 Retina\n      UI Looks like: 1512 x 982 @ 60.00Hz\n      Main Display: Yes\n";
        let expected = Geometry { x: 0.0, y: 0.0, width: 756.0, height: 982.0 };
        assert_eq!(parse_display(text), Some(expected));
        assert_eq!(parse_display(&text.replace("      UI Looks like: 1512 x 982 @ 60.00Hz\n", "")), Some(expected));
        assert_eq!(parse_display("Display:\nResolution: 1920 x 1080\nMain Display: Yes"),
            Some(Geometry { width: 960.0, height: 1080.0, ..expected }));
        assert_eq!(parse_display("no display"), None);
        assert_eq!(configured("window = [-10, 20, 800, 900]").unwrap(),
            Some(Geometry { x: -10.0, y: 20.0, width: 800.0, height: 900.0 }));
        assert!(configured("window = [0, 0, 0, 100]").is_err());
        assert!(configured("window = [0, 0, 100]").is_err());
        assert_eq!(configured("model = 'test'").unwrap(), None);
    }
}
