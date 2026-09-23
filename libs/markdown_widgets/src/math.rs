//! Native TeX layout and glyph rasterization, shared by native and HTML views.
//! No browser, JavaScript, SVG admission or remote resource loading is involved.
use makepad_latex_math::{LayoutItem, MathStyle, layout, parse};
use makepad_markdown::math::Formula;
use std::{cell::RefCell, collections::VecDeque, sync::Arc};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Transform};

const FONT: &[u8] = include_bytes!("../../latex_math/fonts/NewCMMath-Regular.otf");
pub struct MathImage {
    pub png: Vec<u8>,
    pub width: f32,
    pub height: f32,
    pub descent: f32,
    pub id: String,
}

/// The host grants only this generated PNG to its HTML renderer.
pub fn html(
    formula: &Formula,
    font_size: f32,
    ink: u32,
    mut register: impl FnMut(&str, &[u8]) -> Result<String, String>,
) -> Result<String, String> {
    let image = render(formula, font_size, ink)?;
    let url = register(&format!("{}.png", image.id), &image.png)?;
    let tag = format!(
        "<img src=\"{}\" width=\"{}\" height=\"{}\" alt=\"{}\" style=\"max-width:100%;height:auto;vertical-align:-{}px\">",
        makepad_markdown::escape(&url),
        image.width,
        image.height,
        makepad_markdown::escape(&formula.tex),
        image.descent
    );
    Ok(if formula.display {
        format!("<div style=\"text-align:center;margin:16px 0\">{tag}</div>")
    } else {
        tag
    })
}

struct Outline {
    path: PathBuilder,
}
impl ttf_parser::OutlineBuilder for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        self.path.move_to(x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.path.line_to(x, y);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.path.quad_to(x1, y1, x, y);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.path.cubic_to(x1, y1, x2, y2, x, y);
    }
    fn close(&mut self) {
        self.path.close();
    }
}

/// Cache only bounded generated formula images, independent of host documents.
pub fn render(formula: &Formula, font_size: f32, ink: u32) -> Result<Arc<MathImage>, String> {
    if formula.tex.len() > 2048 || !font_size.is_finite() || !(8.0..=48.0).contains(&font_size) {
        return Err("Formula exceeds preview limits".into());
    }
    let mut depth = 0usize;
    for c in formula.tex.chars() {
        if c == '{' {
            depth += 1;
            if depth > 32 {
                return Err("Formula nesting exceeds preview limits".into());
            }
        }
        if c == '}' {
            depth = depth.saturating_sub(1);
        }
    }
    let id = format!(
        "math-{}",
        blake3::hash(format!("{}:{}:{font_size}:{ink}", formula.display, formula.tex).as_bytes())
            .to_hex()
    );
    type Cache = VecDeque<Arc<MathImage>>;
    thread_local! { static CACHE: RefCell<Cache> = const { RefCell::new(VecDeque::new()) }; }
    if let Some(image) = CACHE.with(|cache| cache.borrow().iter().find(|i| i.id == id).cloned()) {
        return Ok(image);
    }
    let (tex, style) = native_tex(formula);
    let math =
        layout(&parse(&tex), FONT, font_size * 1.21, style).ok_or("Unable to lay out formula")?;
    if !math.width.is_finite()
        || !math.height.is_finite()
        || math.width > 2044.0
        || math.height > 1020.0
        || math.items.len() > 2048
    {
        return Err("Formula exceeds preview dimensions".into());
    }
    let width = (math.width + 4.0).ceil().max(1.0);
    let height = (math.height + 4.0).ceil().max(1.0);
    let mut pixels = Pixmap::new((width * 2.0) as u32, (height * 2.0) as u32)
        .ok_or("Unable to allocate formula")?;
    let face = ttf_parser::Face::parse(FONT, 0).map_err(|_| "Invalid math font")?;
    let mut paint = Paint::default();
    paint.set_color_rgba8((ink >> 16) as u8, (ink >> 8) as u8, ink as u8, 255);
    for item in &math.items {
        match item {
            LayoutItem::Glyph(glyph) => {
                let mut outline = Outline {
                    path: PathBuilder::new(),
                };
                if face
                    .outline_glyph(ttf_parser::GlyphId(glyph.glyph_id), &mut outline)
                    .is_some()
                {
                    if let Some(path) = outline.path.finish() {
                        let s = glyph.size / face.units_per_em() as f32 * 2.0;
                        pixels.fill_path(
                            &path,
                            &paint,
                            FillRule::Winding,
                            Transform::from_row(
                                s,
                                0.0,
                                0.0,
                                -s,
                                (glyph.x + 2.0) * 2.0,
                                (math.ascent + glyph.y + 2.0) * 2.0,
                            ),
                            None,
                        );
                    }
                }
            }
            LayoutItem::Rule(rule) => fill_rect(
                &mut pixels,
                &paint,
                rule.x + 2.0,
                math.ascent + rule.y + 2.0,
                rule.width,
                rule.height,
            ),
            LayoutItem::Rect(rect) => fill_rect(
                &mut pixels,
                &paint,
                rect.x + 2.0,
                math.ascent + rect.y + 2.0,
                rect.width,
                rect.height,
            ),
        }
    }
    let image = Arc::new(MathImage {
        png: pixels.encode_png().map_err(|e| e.to_string())?,
        width,
        height,
        descent: math.descent + 2.0,
        id,
    });
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        while cache.len() >= 64
            || cache.iter().map(|i| i.png.len()).sum::<usize>() + image.png.len() > 8 * 1024 * 1024
        {
            if cache.pop_front().is_none() {
                break;
            }
        }
        cache.push_back(image.clone());
    });
    Ok(image)
}

fn fill_rect(pixels: &mut Pixmap, paint: &Paint, x: f32, y: f32, w: f32, h: f32) {
    if let Some(rect) = tiny_skia::Rect::from_xywh(x * 2.0, y * 2.0, w * 2.0, h * 2.0) {
        pixels.fill_rect(rect, paint, Transform::identity(), None);
    }
}

/// Adapt the style declaration and paired delimiter sizing in Editor.md's
/// sample to the native parser. Original TeX remains in the document.
/// Paired explicit sizes use the parser's content-sized delimiters.
fn native_tex(formula: &Formula) -> (String, MathStyle) {
    let mut tex = formula.tex.trim();
    let mut style = if formula.display {
        MathStyle::Display
    } else {
        MathStyle::Text
    };
    for (command, value) in [
        ("\\displaystyle", MathStyle::Display),
        ("\\textstyle", MathStyle::Text),
    ] {
        if let Some(rest) = tex.strip_prefix(command) {
            if !rest.starts_with(|c: char| c.is_ascii_alphabetic()) {
                tex = rest.trim_start();
                style = value;
            }
        }
    }
    let mut result = String::new();
    let mut chars = tex.chars().peekable();
    while let Some(c) = chars.next() {
        result.push(c);
        if c == '\\' {
            let mut command = String::new();
            while chars.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
                command.push(chars.next().unwrap());
            }
            result.push_str(match command.as_str() {
                "bigl" | "Bigl" | "biggl" | "Biggl" => "left",
                "bigr" | "Bigr" | "biggr" | "Biggr" => "right",
                _ => &command,
            });
        }
    }
    (result, style)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_math_has_real_ink_and_preserves_radicals_and_fraction_rules() {
        for tex in [
            "E=mc^2",
            "\\sqrt{3x-1}+(1+x)^2",
            "\\frac{1}{1+\\frac{x}{y}}",
            "\\sum_{i=0}^{n}x^i",
            "\\int_{-\\infty}^{\\infty}f(x)dx",
        ] {
            let image = render(
                &Formula {
                    tex: tex.into(),
                    display: true,
                },
                16.0,
                0x191919,
            )
            .unwrap();
            let pixels = Pixmap::decode_png(&image.png).unwrap();
            assert!(
                pixels.pixels().iter().filter(|p| p.alpha() > 64).count() > 40,
                "{tex}"
            );
            assert!(image.height > 12.0 && image.width > 10.0);
        }
        let plain = render(
            &Formula {
                tex: "x".into(),
                display: true,
            },
            16.0,
            0,
        )
        .unwrap();
        let radical = render(
            &Formula {
                tex: "\\sqrt{x}".into(),
                display: true,
            },
            16.0,
            0,
        )
        .unwrap();
        assert!(radical.width > plain.width && radical.height > plain.height);
    }

    #[test]
    fn editor_md_style_and_delimiters_are_not_literal_glyphs() {
        let formula = Formula {
            tex: r"\displaystyle \frac{1}{\Bigl(\sqrt{x}\Bigr)}".into(),
            display: true,
        };
        let (tex, _) = native_tex(&formula);
        assert_eq!(tex, r"\frac{1}{\left(\sqrt{x}\right)}");
        assert_eq!(
            render(&formula, 16.0, 0).unwrap().png,
            render(&Formula { tex, display: true }, 16.0, 0)
                .unwrap()
                .png
        );
    }
}
