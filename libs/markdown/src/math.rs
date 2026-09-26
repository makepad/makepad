//! TeX stays in the document. Hosts decide how to typeset it for a preview.
use crate::{escape, render::Renderer};
use pulldown_cmark::{html, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

#[derive(Clone, Debug, PartialEq)]
pub struct Formula {
    pub tex: String,
    pub display: bool,
}

pub fn normalize(tex: &str, fenced: bool) -> String {
    let tex = tex.trim();
    let tex = tex
        .strip_prefix("\\(")
        .and_then(|s| s.strip_suffix("\\)"))
        .unwrap_or(tex);
    // Editor.md's sample escapes underscores in math fences as Markdown.
    // This affects rendering only; the original source remains untouched.
    if fenced {
        tex.replace("\\_", "_")
    } else {
        tex.to_owned()
    }
}

/// Sanitize source HTML before substituting host-generated math markup.
/// Ordinary code fences and code spans never invoke the math renderer.
pub fn markdown_html(source: &str, mut render: impl FnMut(&Formula) -> String) -> String {
    struct MathRenderer<F>(F);
    impl<F: FnMut(&Formula) -> String> Renderer for MathRenderer<F> {
        fn math(&mut self, formula: &Formula) -> String {
            (self.0)(formula)
        }
    }
    markdown_html_with_renderer(source, &mut MathRenderer(&mut render))
}

pub fn markdown_html_with_renderer(source: &str, renderer: &mut dyn Renderer) -> String {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
        | Options::ENABLE_MATH;
    let prefix = format!("MAKEPADMATH{}X", blake3::hash(source.as_bytes()).to_hex());
    enum Extension {
        Math(Formula),
        Code(String, String),
    }
    let mut extensions = Vec::new();
    let mut token = |extension: Extension| {
        let marker = format!("{prefix}{}END", extensions.len());
        extensions.push((marker.clone(), extension));
        Event::Text(marker.into())
    };
    let mut metadata = false;
    let mut fence: Option<(String, String)> = None;
    let events = Parser::new_ext(source, options)
        .into_offset_iter()
        .filter_map(|(event, range)| match event {
            Event::Start(Tag::MetadataBlock(_)) => {
                metadata = true;
                None
            }
            Event::End(TagEnd::MetadataBlock(_)) => {
                metadata = false;
                None
            }
            _ if metadata => None,
            Event::Start(Tag::CodeBlock(kind)) => {
                let language = match kind {
                    CodeBlockKind::Fenced(lang) => {
                        lang.split_whitespace().next().unwrap_or("").to_owned()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                fence = Some((language, String::new()));
                None
            }
            Event::Text(text) if fence.is_some() => {
                fence.as_mut().unwrap().1.push_str(&text);
                None
            }
            Event::End(TagEnd::CodeBlock) if fence.is_some() => {
                let (language, source) = fence.take().unwrap();
                Some(token(
                    if matches!(language.as_str(), "math" | "katex" | "latex") {
                        Extension::Math(Formula {
                            tex: normalize(&source, true),
                            display: true,
                        })
                    } else {
                        Extension::Code(language, source)
                    },
                ))
            }
            Event::InlineMath(tex) => Some(token(Extension::Math(Formula {
                tex: normalize(&tex, false),
                display: false,
            }))),
            Event::DisplayMath(tex) => {
                let before = source[..range.start].rsplit('\n').next().unwrap_or("");
                let after = source[range.end..].split('\n').next().unwrap_or("");
                Some(token(Extension::Math(Formula {
                    tex: normalize(&tex, false),
                    display: before.trim().is_empty() && after.trim().is_empty(),
                })))
            }
            Event::TaskListMarker(checked) => {
                Some(Event::Text(if checked { "☑ " } else { "☐ " }.into()))
            }
            _ => Some(event),
        });
    let mut html = String::new();
    html::push_html(&mut html, events);
    let mut html = crate::markup::html_fragment_with_renderer(&html, renderer);
    for (marker, extension) in extensions {
        let rendered = match extension {
            Extension::Math(formula) => renderer.math(&formula),
            Extension::Code(language, source) => renderer.code(&language, &source),
        };
        html = html.replace(&marker, &rendered);
    }
    html
}

pub fn fallback(formula: &Formula) -> String {
    let text = format!("<code>{}</code>", escape(&formula.tex));
    if formula.display {
        format!("<div>{text}</div>")
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detects_math_before_markdown_escaping_and_keeps_inline_context() {
        let source = "$$E=mc^2$$\n\n中文$$x^2$$后文 $y_1$\n\n```math\n\\sum\\_{k=1}^n a\\_k\n```\n\n`$literal$`\n\n```rust\nlet x = \"$$not math$$\";\n```";
        let mut seen = Vec::new();
        let html = markdown_html(source, |f| {
            seen.push(f.clone());
            "<rmath></rmath>".into()
        });
        assert_eq!(seen.len(), 4);
        assert_eq!(
            seen.iter().map(|f| f.display).collect::<Vec<_>>(),
            [true, false, false, true]
        );
        assert_eq!(seen[3].tex, "\\sum_{k=1}^n a_k");
        assert!(html.contains("中文<rmath></rmath>后文"));
        assert!(html.contains("$literal$"));
        assert!(html.contains("$$not math$$"));
    }
    #[test]
    fn raw_html_cannot_inject_generated_math_widgets() {
        let html = markdown_html(
            "<rmath tex='spoof'>x</rmath>\n\n<script>attack()</script>\n\n$\\sqrt{x}$",
            |_| "<rmath tex='real'></rmath>".into(),
        );
        assert!(!html.contains("spoof") && !html.contains("attack"));
        assert_eq!(html.matches("<rmath").count(), 1);
    }
}
