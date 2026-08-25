//! Vector export of a sheet. Paper millimetres at true scale: a 1:100 plan
//! of a 10 m wall is 100 mm of SVG, ready to print.

use crate::model::{Sheet, SheetItem};

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

fn rgb(c: [f32; 4]) -> String {
    let r = (c[0].clamp(0.0, 1.0) * 255.0).round() as u8;
    let g = (c[1].clamp(0.0, 1.0) * 255.0).round() as u8;
    let b = (c[2].clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("rgb({r},{g},{b})")
}

fn opacity(c: [f32; 4]) -> f32 {
    c[3].clamp(0.0, 1.0)
}

/// Paper y-up, origin bottom-left → SVG y-down, origin top-left.
fn yf(h: f32, y: f32) -> f32 {
    h - y
}

fn stroke_attrs(s: &crate::model::Stroke) -> String {
    let mut a = format!(
        r#"fill="none" stroke="{}" stroke-width="{}" stroke-linecap="round" stroke-linejoin="round""#,
        rgb(s.color),
        s.width_mm.max(0.05)
    );
    if s.dash[0] > 0.0 {
        a.push_str(&format!(
            r#" stroke-dasharray="{},{}""#,
            s.dash[0],
            if s.dash[1] > 0.0 { s.dash[1] } else { s.dash[0] * 0.6 }
        ));
    }
    if opacity(s.color) < 0.999 {
        a.push_str(&format!(r#" stroke-opacity="{}""#, opacity(s.color)));
    }
    a
}

fn poly_d(pts: &[[f32; 2]], closed: bool, h: f32) -> String {
    if pts.is_empty() {
        return String::new();
    }
    let mut d = format!("M{:.4},{:.4}", pts[0][0], yf(h, pts[0][1]));
    for p in pts.iter().skip(1) {
        d.push_str(&format!(" L{:.4},{:.4}", p[0], yf(h, p[1])));
    }
    if closed {
        d.push('Z');
    }
    d
}

/// SVG document whose `width`/`height` are paper millimetres — 1:100 is
/// 1 m → 10 mm on the page, no extra scale transform.
pub fn sheet_to_svg(sheet: &Sheet) -> String {
    let (w, h) = (sheet.size_mm[0], sheet.size_mm[1]);
    let mut out = String::with_capacity(8 * 1024 + sheet.items.len() * 128);
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" version=\"1.1\" width=\"{w}mm\" height=\"{h}mm\" viewBox=\"0 0 {w} {h}\" fill=\"none\">\n"
    ));
    out.push_str(&format!("<title>{}</title>\n", xml_escape(&sheet.name)));
    out.push_str(&format!(
        "<rect width=\"{w}\" height=\"{h}\" fill=\"#f4f2ee\"/>\n"
    ));
    for item in &sheet.items {
        match item {
            SheetItem::Fill { points, color, stroke } => {
                let d = poly_d(points, true, h);
                if d.is_empty() {
                    continue;
                }
                out.push_str(&format!(
                    r#"<path d="{d}" fill="{}" fill-opacity="{}" fill-rule="evenodd""#,
                    rgb(*color),
                    opacity(*color)
                ));
                if let Some(s) = stroke {
                    out.push(' ');
                    out.push_str(&stroke_attrs(s));
                } else {
                    out.push_str(r#" stroke="none""#);
                }
                out.push_str("/>\n");
            }
            SheetItem::Path {
                points,
                closed,
                stroke,
            } => {
                let d = poly_d(points, *closed, h);
                if d.is_empty() {
                    continue;
                }
                out.push_str(&format!(
                    r#"<path d="{d}" {}/>"#,
                    stroke_attrs(stroke)
                ));
                out.push('\n');
            }
            SheetItem::Arc {
                center,
                radius,
                start_deg,
                end_deg,
                stroke,
            } => {
                let steps = 24.max((end_deg - start_deg).abs() as i32 / 6) as i32;
                let mut pts = Vec::with_capacity(steps as usize + 1);
                for i in 0..=steps {
                    let t = i as f32 / steps as f32;
                    let a = (start_deg + (end_deg - start_deg) * t).to_radians();
                    pts.push([center[0] + a.cos() * radius, center[1] + a.sin() * radius]);
                }
                let d = poly_d(&pts, false, h);
                out.push_str(&format!(
                    r#"<path d="{d}" {}/>"#,
                    stroke_attrs(stroke)
                ));
                out.push('\n');
            }
            SheetItem::Text {
                pos,
                text,
                height_mm,
                angle_deg,
                color,
            } => {
                let rot = if *angle_deg != 0.0 {
                    format!(
                        r#" transform="rotate({} {} {})""#,
                        -angle_deg,
                        pos[0],
                        yf(h, pos[1])
                    )
                } else {
                    String::new()
                };
                out.push_str(&format!(
                    r#"<text x="{:.4}" y="{:.4}" font-size="{:.3}" font-family="IBM Plex Sans, Helvetica, sans-serif" fill="{}" fill-opacity="{}"{rot}>{}</text>"#,
                    pos[0],
                    yf(h, pos[1]),
                    height_mm,
                    rgb(*color),
                    opacity(*color),
                    xml_escape(text)
                ));
                out.push('\n');
            }
            SheetItem::Hatch { points, color, .. } => {
                let d = poly_d(points, true, h);
                if d.is_empty() {
                    continue;
                }
                out.push_str(&format!(
                    r#"<path d="{d}" fill="none" stroke="{}" stroke-width="0.2"/>"#,
                    rgb(*color)
                ));
                out.push('\n');
            }
        }
    }
    out.push_str("</svg>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::*;
    use crate::sheets::plan::{plan_sheet, PlanSettings};

    #[test]
    fn svg_export_is_true_scale_paper_mm() {
        let scene = Scene::from_model(crate::model::demo::demo_house(), &mut |_| {});
        let sheet = plan_sheet(
            &scene,
            0,
            SheetId::from_index(0),
            &PlanSettings::default(),
            &scene.units,
        )
        .expect("demo house plan");
        let svg = sheet_to_svg(&sheet);
        assert!(svg.contains(r#"width="420mm""#), "{svg:.200}");
        assert!(svg.contains(r#"height="297mm""#));
        assert!(svg.contains("viewBox=\"0 0 420 297\""));
        assert!(svg.contains("<path"), "no linework in svg");
        assert!(svg.contains("</svg>"));
    }
}
