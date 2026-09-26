//! Host-owned image resources and presentation shared by native and HTML views.
use makepad_markdown::{
    escape,
    math::Formula,
    render::{ImageRequest, Renderer, image_link},
};
#[cfg(any(feature = "svg", feature = "syntax-highlighting"))]
use std::cell::LazyCell;
#[cfg(feature = "syntax-highlighting")]
use std::cell::RefCell;
use std::{collections::BTreeMap, sync::Arc};
#[cfg(feature = "syntax-highlighting")]
use syntect::{
    easy::HighlightLines, highlighting::ThemeSet, parsing::SyntaxSet, util::LinesWithEndings,
};

#[derive(Clone, Debug)]
pub struct PreviewImage {
    /// Content-addressed identity. Replacing pixels must also replace this ID;
    /// native widgets use it to retain textures across unchanged redraws.
    /// Prefer constructing resources with [`prepare_image`].
    pub id: String,
    pub png: Arc<Vec<u8>>,
    pub width: u32,
    pub height: u32,
}
pub type Images = Arc<BTreeMap<String, PreviewImage>>;

/// SVGs become inert pixels here. Nested images/files/network are never resolved.
pub fn prepare_image(bytes: &[u8]) -> Result<PreviewImage, String> {
    if bytes.len() > 8 * 1024 * 1024 {
        return Err("Image exceeds 8 MB".into());
    }
    let (width, height, png) = if bytes.starts_with(b"\x89PNG") || bytes.starts_with(b"\xff\xd8") {
        prepare_raster(bytes)?
    } else {
        prepare_svg(bytes)?
    };
    if png.len() > 8 * 1024 * 1024 {
        return Err("Decoded image exceeds 8 MB".into());
    }
    Ok(PreviewImage {
        id: format!("image-{}.png", blake3::hash(&png).to_hex()),
        png: Arc::new(png),
        width,
        height,
    })
}

#[cfg(feature = "svg")]
fn prepare_svg(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    if bytes.len() > 256 * 1024 {
        return Err("SVG exceeds 256 KB".into());
    }
    thread_local! {
        static FONTS: LazyCell<Arc<resvg::usvg::fontdb::Database>> = LazyCell::new(|| {
            let mut db = resvg::usvg::fontdb::Database::new();
            db.load_system_fonts();
            Arc::new(db)
        });
    }
    let options = resvg::usvg::Options {
        fontdb: FONTS.with(|fonts| Arc::clone(fonts)),
        image_href_resolver: resvg::usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..Default::default()
    };
    let tree = resvg::usvg::Tree::from_data(bytes, &options).map_err(|e| e.to_string())?;
    let size = tree.size().to_int_size();
    if size.width() > 2048 || size.height() > 2048 {
        return Err("SVG exceeds 2048 pixels".into());
    }
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size.width() * 2, size.height() * 2)
        .ok_or("Invalid image size")?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(2.0, 2.0),
        &mut pixmap.as_mut(),
    );
    Ok((
        size.width(),
        size.height(),
        pixmap.encode_png().map_err(|e| e.to_string())?,
    ))
}

#[cfg(not(feature = "svg"))]
fn prepare_svg(_: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    Err("SVG support is disabled; enable the svg feature".into())
}

fn prepare_raster(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    use image::ImageDecoder;
    let format = image::guess_format(bytes).map_err(|e| e.to_string())?;
    if !matches!(format, image::ImageFormat::Jpeg | image::ImageFormat::Png) {
        return Err("Expected a JPEG or PNG image".into());
    }
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(128 * 1024 * 1024);
    limits.max_image_width = Some(12000);
    limits.max_image_height = Some(12000);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 24_000_000 {
        return Err("Image exceeds 24 megapixels".into());
    }
    let orientation = decoder.orientation().map_err(|e| e.to_string())?;
    let mut decoded = image::DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    decoded.apply_orientation(orientation);
    let resized = if decoded.width() > 2048 || decoded.height() > 2048 {
        decoded.resize(2048, 2048, image::imageops::FilterType::Lanczos3)
    } else {
        decoded
    };
    let mut output = std::io::Cursor::new(Vec::new());
    resized
        .write_to(&mut output, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok((resized.width(), resized.height(), output.into_inner()))
}

pub fn hex(text: &str) -> String {
    text.as_bytes().iter().map(|b| format!("{b:02x}")).collect()
}
pub fn unhex(text: &str) -> Option<String> {
    let bytes: Option<Vec<u8>> = text
        .as_bytes()
        .chunks(2)
        .map(|pair| {
            std::str::from_utf8(pair)
                .ok()
                .and_then(|s| u8::from_str_radix(s, 16).ok())
        })
        .collect();
    String::from_utf8(bytes?).ok()
}

pub type Tokens = Arc<Vec<(u32, String)>>;
#[cfg(feature = "syntax-highlighting")]
thread_local! {
    static SYNTAX: LazyCell<SyntaxSet> = LazyCell::new(SyntaxSet::load_defaults_newlines);
    static THEMES: LazyCell<ThemeSet> = LazyCell::new(ThemeSet::load_defaults);
    static CODE_CACHE: RefCell<BTreeMap<String, Tokens>> = const { RefCell::new(BTreeMap::new()) };
}
#[cfg(not(feature = "syntax-highlighting"))]
pub fn highlight(_: &str, source: &str) -> Tokens {
    Arc::new(vec![(0x24292e, source.to_owned())])
}

#[cfg(feature = "syntax-highlighting")]
pub fn highlight(language: &str, source: &str) -> Tokens {
    let key = blake3::hash(format!("{language}\0{source}").as_bytes())
        .to_hex()
        .to_string();
    if let Some(tokens) = CODE_CACHE.with(|cache| cache.borrow().get(&key).cloned()) {
        return tokens;
    }
    let language = match language {
        "js" => "JavaScript",
        "ts" => "TypeScript",
        "py" => "Python",
        "sh" | "bash" => "Bourne Again Shell (bash)",
        other => other,
    };
    let tokens = SYNTAX.with(|syntaxes| {
        THEMES.with(|themes| {
            let syntax = syntaxes
                .find_syntax_by_token(language)
                .unwrap_or_else(|| syntaxes.find_syntax_plain_text());
            let mut highlighter = HighlightLines::new(syntax, &themes.themes["InspiredGitHub"]);
            let mut tokens: Vec<(u32, String)> = Vec::new();
            for line in LinesWithEndings::from(source) {
                match highlighter.highlight_line(line, syntaxes) {
                    Ok(ranges) => {
                        for (style, text) in ranges {
                            let c = style.foreground;
                            let color = (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32;
                            if let Some(last) = tokens.last_mut().filter(|last| last.0 == color) {
                                last.1.push_str(text);
                            } else {
                                tokens.push((color, text.into()));
                            }
                        }
                    }
                    Err(_) => tokens.push((0x24292e, line.into())),
                }
            }
            Arc::new(tokens)
        })
    });
    // Bound cached text independently of the number of code blocks. Large
    // blocks can render without displacing the cache with an unbounded entry.
    if source.len() > 512 * 1024 {
        return tokens;
    }
    CODE_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.len() >= 64
            || cache
                .values()
                .flat_map(|tokens| tokens.iter())
                .map(|(_, text)| text.len())
                .sum::<usize>()
                + source.len()
                > 8 * 1024 * 1024
        {
            cache.clear();
        }
        cache.insert(key, tokens.clone());
    });
    tokens
}
pub fn expand_tabs(source: &str) -> String {
    // Expand tabs consistently before shaping code in native and HTML views.
    let mut expanded = String::new();
    let mut column = 0;
    for ch in source.chars() {
        match ch {
            '\t' => {
                let count = 4 - column % 4;
                expanded.extend(std::iter::repeat_n(' ', count));
                column += count;
            }
            '\n' => {
                expanded.push(ch);
                column = 0;
            }
            _ => {
                expanded.push(ch);
                column += 1;
            }
        }
    }
    expanded
}

pub fn code_html(language: &str, source: &str) -> String {
    let expanded = expand_tabs(source);
    let source = expanded.as_str();
    let mut html = String::from("<pre><code>");
    for (color, text) in highlight(language, source).iter() {
        html.push_str(&format!(
            "<span style=\"color:#{color:06x}\">{}</span>",
            escape(text)
        ));
    }
    html.push_str("</code></pre>");
    html
}

pub struct NativeRenderer<'a> {
    pub images: &'a Images,
    pub size: f32,
    pub ink: u32,
}
impl Renderer for NativeRenderer<'_> {
    fn emoji(&mut self, text: &str) -> String {
        if cfg!(all(
            feature = "emoji",
            any(target_os = "macos", target_os = "ios")
        )) {
            format!("<remoji>{}:{}</remoji>", self.size, hex(text))
        } else {
            escape(text)
        }
    }
    fn math(&mut self, formula: &Formula) -> String {
        crate::math_view::formula_html(formula, self.size, self.ink)
    }
    fn code(&mut self, language: &str, source: &str) -> String {
        #[cfg(feature = "diagrams")]
        if crate::diagram::is_language(language) {
            return match crate::diagram::render(language, source) {
                Ok(_) => format!(
                    "<p><rdiagram>{}:{}</rdiagram></p>",
                    hex(language),
                    hex(source)
                ),
                Err(error) => crate::diagram::fallback(language, source, &error),
            };
        }
        format!(
            "<rcode>{}:{}:{}</rcode>",
            self.size * 0.85,
            hex(language),
            hex(&expand_tabs(source))
        )
    }
    fn image(&mut self, request: &ImageRequest<'_>) -> String {
        let Some(image) = self.images.get(request.url) else {
            return image_link(request);
        };
        let width = request.width.unwrap_or(image.width);
        format!(
            "<rimage>{}:{width}:{}</rimage>",
            image.id,
            image.height as f64 * width as f64 / image.width as f64
        )
    }
    fn text(&mut self, text: &str) -> String {
        #[cfg(feature = "editormd")]
        {
            makepad_markdown::render::editormd_emoji_text(text, self)
        }
        #[cfg(not(feature = "editormd"))]
        {
            makepad_markdown::render::emoji_text(text, self)
        }
    }
}

pub struct HtmlRenderer<'a, F: FnMut(&str, &[u8]) -> Result<String, String>> {
    pub images: &'a Images,
    pub size: f32,
    pub ink: u32,
    pub register: F,
    pub error: Option<String>,
    pub math_count: usize,
}
impl<F: FnMut(&str, &[u8]) -> Result<String, String>> Renderer for HtmlRenderer<'_, F> {
    fn emoji(&mut self, text: &str) -> String {
        match crate::emoji::render(text, self.size) {
            Ok(image) => match (self.register)(&image.id, &image.png) {
                Ok(url) => format!(
                    "<img src=\"{}\" alt=\"{}\" style=\"width:{}px;height:{}px;vertical-align:middle\">",
                    escape(&url),
                    escape(text),
                    image.width,
                    image.height
                ),
                Err(error) => {
                    self.error = Some(error);
                    escape(text)
                }
            },
            Err(_) => escape(text),
        }
    }
    fn math(&mut self, formula: &Formula) -> String {
        self.math_count += 1;
        #[cfg(feature = "math")]
        {
            crate::math::html(formula, self.size, self.ink, &mut self.register).unwrap_or_else(
                |e| {
                    self.error = Some(e);
                    makepad_markdown::math::fallback(formula)
                },
            )
        }
        #[cfg(not(feature = "math"))]
        {
            makepad_markdown::math::fallback(formula)
        }
    }
    fn code(&mut self, language: &str, source: &str) -> String {
        #[cfg(feature = "diagrams")]
        if crate::diagram::is_language(language) {
            return match crate::diagram::render(language, source) {
                Ok(image) => match (self.register)(&image.id, &image.png) {
                    Ok(url) => format!(
                        "<p><img class=\"diagram\" src=\"{}\" alt=\"Diagram\" style=\"width:{}px;max-width:100%;height:auto\"></p>",
                        escape(&url),
                        image.width
                    ),
                    Err(error) => {
                        self.error = Some(error.clone());
                        crate::diagram::fallback(language, source, &error)
                    }
                },
                Err(error) => crate::diagram::fallback(language, source, &error),
            };
        }
        code_html(language, source)
    }
    fn image(&mut self, request: &ImageRequest<'_>) -> String {
        let Some(image) = self.images.get(request.url) else {
            return image_link(request);
        };
        match (self.register)(&image.id, &image.png) {
            Ok(url) => format!(
                "<img src=\"{}\" alt=\"{}\" style=\"max-width:100%;width:{}px;height:auto;vertical-align:middle\">",
                escape(&url),
                escape(request.alt),
                request.width.unwrap_or(image.width)
            ),
            Err(e) => {
                self.error = Some(e);
                image_link(request)
            }
        }
    }
    fn text(&mut self, text: &str) -> String {
        #[cfg(feature = "editormd")]
        {
            makepad_markdown::render::editormd_emoji_text(text, self)
        }
        #[cfg(not(feature = "editormd"))]
        {
            makepad_markdown::render::emoji_text(text, self)
        }
    }
}

pub const CSS: &str = r#"
table{border-collapse:collapse;width:100%;margin:18px 0;font-size:.9em;table-layout:fixed}
th,td{border:1px solid #b8c2bd;padding:8px;overflow-wrap:anywhere}th{background:#edf2ef;font-weight:bold}
td[align=right],th[align=right]{text-align:right}td[align=center],th[align=center]{text-align:center}
pre{background:#f3f5f4;border:1px solid #dde3df;border-radius:4px;padding:12px;white-space:pre-wrap;overflow-wrap:anywhere;line-height:1.55;font-size:13px}
code{font-family:Menlo,Consolas,monospace}p code,li code,td code{background:#f0f2f1;padding:2px 3px}
img{max-width:100%;height:auto}pre code{padding:0;background:transparent}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(all(
        feature = "syntax-highlighting",
        feature = "editormd",
        any(target_os = "macos", target_os = "ios")
    ))]
    #[test]
    fn language_tokens_and_emoji_do_not_modify_literals() {
        for (language, source) in [
            ("javascript", "const x = \"hello\"; // comment\n"),
            ("html", "<div class=\"test\">中文</div>"),
            ("python", "def f():\n    return 42\n"),
            ("rust", "fn main() { println!(\"hello\"); }"),
        ] {
            let tokens = highlight(language, source);
            assert_eq!(
                tokens.iter().map(|(_, s)| s.as_str()).collect::<String>(),
                source
            );
            assert!(
                tokens
                    .iter()
                    .map(|(c, _)| c)
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    >= 3,
                "{language}"
            );
        }
        let source = ":smiley: :fa-star: :fa-gear: `:smiley:`\n\n```js\nconst s = ':smiley:';\n```";
        let images = Images::default();
        let mut renderer = NativeRenderer {
            images: &images,
            size: 14.,
            ink: 0x191919,
        };
        let html = makepad_markdown::markdown_render::render(source, &mut renderer)
            .iter()
            .map(|b| b.html.as_str())
            .collect::<String>();
        assert_eq!(html.matches("<remoji>").count(), 3);
        assert!(html.contains("<code>:smiley:</code>"));
    }
    #[cfg(feature = "svg")]
    #[test]
    fn svg_badge_becomes_pixels_and_nested_resources_are_denied() {
        let image = prepare_image(br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="20"><rect width="100" height="20" fill="#00cc00"/><image href="file:///private/test.png"/></svg>"##).unwrap();
        assert_eq!((image.width, image.height), (100, 20));
        assert!(image.png.starts_with(b"\x89PNG"));
        assert!(prepare_image(b"not an image").is_err());
    }
    #[test]
    fn native_containers_styled_links_and_aligned_cells_keep_their_contents() {
        let html = native_html(
            "<section id=\"notes\"><div><a href=\"#target\">first <strong>bold</strong> <code>code</code></a> <abbr title=\"Expanded\">abbr</abbr></div></section>",
        );
        assert!(
            html.contains("<strong><a href=\"#target\">bold</a></strong>"),
            "{html}"
        );
        assert!(
            html.contains("<code><a href=\"#target\">code</a></code>"),
            "{html}"
        );
        assert!(html.contains("abbr") && !html.contains("<section") && !html.contains("<abbr"));
        assert_eq!(
            native_html("<a href=\"next.md\"><rimage>image:20:20</rimage></a>"),
            "<rimage href=\"next.md\">image:20:20</rimage>"
        );
        let table =
            native_html("<table><tr><td align=\"right\"><strong>234</strong></td></tr></table>");
        assert!(table.contains(&format!("<rcell>1:{}</rcell>", hex("<strong>234</strong>"))));
    }
    #[cfg(feature = "math")]
    #[test]
    fn whole_markdown_document_uses_native_extension_widgets() {
        let images = Images::default();
        let mut renderer = NativeRenderer {
            images: &images,
            size: 14.0,
            ink: 0x191919,
        };
        let blocks = makepad_markdown::markdown_render::render(
            "# Start\n\n[TOC]\n\n:smiley: and $E=mc^2$.\n\n```rust\nfn main() {}\n```\n\nNote[^a].\n\n[^a]: Definition.\n",
            &mut renderer,
        );
        let html = blocks
            .iter()
            .map(|b| native_html(&b.html))
            .collect::<String>();
        assert!(
            html.contains("<rmath>") && html.contains("<rcode>") && html.contains("href=\"#fn-a\""),
            "{html}"
        );
        assert!(
            html.contains("Table of contents") && html.contains("Definition."),
            "{html}"
        );
        assert!(!html.contains("<nav") && !html.contains("<section"));
        assert!(
            blocks
                .iter()
                .map(|b| b.text.as_str())
                .collect::<String>()
                .contains("fn main() {}")
        );
    }
}

/// Isolate native table alignment in one child flow. The pinned TextFlow applies
/// cell alignment to both its text layouter and its turtle, shifting text twice.
pub fn native_html(html: &str) -> String {
    let mut html = native_structure(html);
    // HtmlLink draws a text run. Let image widgets own an enclosing image link
    // so the native reader does not turn its resource identifier into text.
    let mut offset = 0;
    while let Some(start) = html[offset..].find("<a ").map(|n| n + offset) {
        let Some(open_end) = html[start..].find('>').map(|n| n + start) else {
            break;
        };
        let Some(end) = html[open_end + 1..].find("</a>").map(|n| n + open_end + 1) else {
            break;
        };
        let inner = html[open_end + 1..end].trim();
        if let Some(payload) = inner
            .strip_prefix("<rimage>")
            .and_then(|s| s.strip_suffix("</rimage>"))
        {
            let replacement = format!("<rimage {}>{payload}</rimage>", &html[start + 3..open_end]);
            html.replace_range(start..end + 4, &replacement);
            offset = start + replacement.len();
        } else {
            offset = end + 4;
        }
    }
    for tag in ["td", "th"] {
        for (name, align) in [("", 0.0), ("left", 0.0), ("right", 1.0), ("center", 0.5)] {
            let open = if name.is_empty() {
                format!("<{tag}>")
            } else {
                format!("<{tag} align=\"{name}\">")
            };
            let close = format!("</{tag}>");
            let mut offset = 0;
            while let Some(start) = html[offset..].find(&open).map(|n| n + offset) {
                let content_start = start + open.len();
                let Some(end) = html[content_start..]
                    .find(&close)
                    .map(|n| n + content_start)
                else {
                    break;
                };
                let replacement = format!(
                    "<{tag}><rcell>{align}:{}</rcell>{close}",
                    hex(&html[content_start..end])
                );
                html.replace_range(start..end + close.len(), &replacement);
                offset = start + replacement.len();
            }
        }
    }
    html
}

/// Makepad's Html handles a deliberately small tag set. Unknown containers
/// otherwise swallow their entire subtree, and HtmlLink only reads its first
/// text node. Preserve containers' contents and split styled link labels into
/// native link runs, without flattening bold/code/superscript formatting.
fn native_structure(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    let mut link: Option<String> = None;
    while !rest.is_empty() {
        if !rest.starts_with('<') {
            let end = rest.find('<').unwrap_or(rest.len());
            let text = &rest[..end];
            if let Some(open) = &link {
                out.push_str(open);
                out.push_str(text);
                out.push_str("</a>");
            } else {
                out.push_str(text);
            }
            rest = &rest[end..];
            continue;
        }
        let Some(end) = rest.find('>') else {
            out.push_str(rest);
            break;
        };
        let tag = &rest[..=end];
        let closing = tag.starts_with("</");
        let name = tag
            .trim_start_matches('<')
            .trim_start_matches('/')
            .split([' ', '>'])
            .next()
            .unwrap_or("");
        if name == "a" {
            if closing {
                link = None;
            } else {
                link = Some(tag.to_owned());
            }
        } else if name.starts_with('r')
            && matches!(
                name,
                "rmath" | "rcode" | "rimage" | "remoji" | "rdiagram" | "rcell"
            )
            && !closing
        {
            let close = format!("</{name}>");
            if let Some(stop) = rest.find(&close) {
                if let Some(open) = &link {
                    out.push_str(&format!("<{name} {}>", &open[3..open.len() - 1]));
                    out.push_str(&rest[end + 1..stop + close.len()]);
                } else {
                    out.push_str(&rest[..stop + close.len()]);
                }
                rest = &rest[stop + close.len()..];
                continue;
            }
        } else {
            match name {
                "span" | "abbr" | "nav" | "section" | "article" | "figure" => {}
                "div" | "figcaption" => out.push_str("<br>"),
                "dl" => out.push_str(if closing {
                    "</blockquote>"
                } else {
                    "<blockquote>"
                }),
                "dt" => out.push_str(if closing {
                    "</strong></p>"
                } else {
                    "<p><strong>"
                }),
                "dd" => out.push_str(if closing { "</p>" } else { "<p>" }),
                "tfoot" => out.push_str(if closing { "</tbody>" } else { "<tbody>" }),
                "caption" => out.push_str(if closing {
                    "</strong></p>"
                } else {
                    "<p><strong>"
                }),
                _ => out.push_str(tag),
            }
        }
        rest = &rest[end + 1..];
    }
    out
}
