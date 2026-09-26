//! Whole-document Markdown parsing for native readers. Layout, resource loading,
//! and navigation belong to the host; this module has no browser dependency.
use crate::{escape, math::Formula, render::Renderer};
use comrak::{
    nodes::{AstNode, NodeHtmlBlock, NodeValue},
    Arena, Options,
};
use html5ever::{
    buffer_queue::BufferQueue,
    tendril::StrTendril,
    tokenizer::{TagKind, Token, TokenSink, TokenSinkResult, Tokenizer},
};
use std::cell::RefCell;

#[derive(Clone, Copy, Debug)]
pub enum Dialect {
    CommonMark,
    Gfm,
    /// Extended Markdown: front matter, TOC, math, alerts, and footnotes.
    Extended,
    /// Compatibility name for [`Dialect::Extended`].
    Article,
}

fn enable_gfm(options: &mut Options<'_>) {
    options.extension.table = true;
    options.extension.strikethrough = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
}

#[allow(deprecated)]
pub fn options(dialect: Dialect) -> Options<'static> {
    let mut o = Options::default();
    // Raw HTML is sanitized after parsing, before host widget substitutions.
    o.render.r#unsafe = true;
    if !matches!(dialect, Dialect::CommonMark) {
        enable_gfm(&mut o);
        o.extension.tagfilter = true;
    }
    if matches!(dialect, Dialect::Extended | Dialect::Article) {
        // The native sanitizer removes active HTML, including its contents.
        // Escaping just the script tag first would expose nested image tags.
        o.extension.tagfilter = false;
        o.extension.footnotes = true;
        o.extension.alerts = true;
        o.extension.math_dollars = true;
        o.extension.math_code = true;
        o.extension.description_lists = true;
        o.extension.front_matter_delimiter = Some("---".into());
        o.extension.header_id_prefix = Some(String::new());
    }
    o
}

/// Unsanitized parser output, used for specification conformance checks only.
/// Applications must use `render` so source HTML cannot create native widgets.
pub fn specification_html(source: &str, dialect: Dialect) -> String {
    comrak::markdown_to_html(source, &options(dialect))
}

#[derive(Clone, Debug, Default)]
pub struct RenderedBlock {
    /// Original top-level node's 1-based source line (before substitutions).
    pub source_line: usize,
    pub html: String,
    pub text: String,
    pub anchors: Vec<String>,
}

#[derive(Default)]
struct TextSink(RefCell<(String, Vec<String>, usize)>);
impl TokenSink for TextSink {
    type Handle = ();
    fn process_token(&self, token: Token, _: u64) -> TokenSinkResult<()> {
        let mut state = self.0.borrow_mut();
        match token {
            Token::CharacterTokens(s) => state.0.push_str(&s),
            Token::TagToken(t) => {
                let name = t.name.as_ref();
                if t.kind == TagKind::StartTag {
                    for attr in &t.attrs {
                        if attr.name.local.as_ref() == "id" {
                            state.1.push(attr.value.to_string());
                        }
                    }
                    if name == "input"
                        && t.attrs.iter().any(|a| {
                            a.name.local.as_ref() == "type" && a.value.as_ref() == "checkbox"
                        })
                    {
                        state.0.push_str(
                            if t.attrs.iter().any(|a| a.name.local.as_ref() == "checked") {
                                "☑ "
                            } else {
                                "☐ "
                            },
                        );
                    }
                    if name == "img" {
                        if let Some(alt) = t.attrs.iter().find(|a| a.name.local.as_ref() == "alt") {
                            state.0.push_str(&alt.value);
                        }
                    }
                    if name == "pre" {
                        state.2 += 1;
                    }
                } else if name == "pre" {
                    state.2 = state.2.saturating_sub(1);
                }
                if name == "br"
                    || t.kind == TagKind::EndTag
                        && matches!(
                            name,
                            "p" | "div"
                                | "li"
                                | "tr"
                                | "blockquote"
                                | "pre"
                                | "h1"
                                | "h2"
                                | "h3"
                                | "h4"
                                | "h5"
                                | "h6"
                        )
                {
                    if !state.0.ends_with('\n') {
                        state.0.push('\n');
                    }
                } else if t.kind == TagKind::EndTag && matches!(name, "td" | "th") {
                    state.0.push('\t');
                }
            }
            _ => {}
        }
        TokenSinkResult::Continue
    }
}
pub fn text_and_anchors(html: &str) -> (String, Vec<String>) {
    let q = BufferQueue::default();
    q.push_back(StrTendril::from(html));
    let t = Tokenizer::new(TextSink::default(), Default::default());
    let _ = t.feed(&q);
    t.end();
    let (text, anchors, _) = t.sink.0.into_inner();
    (text.trim().to_owned(), anchors)
}

fn math_on_own_line(node: &AstNode<'_>) -> bool {
    // Inspect inline siblings, not raw line prefixes: blockquote/list markers
    // are structure and should not turn a standalone formula into inline math.
    if !node
        .parent()
        .is_some_and(|p| matches!(p.data.borrow().value, NodeValue::Paragraph))
    {
        return false;
    }
    for backwards in [true, false] {
        let mut sibling = if backwards {
            node.previous_sibling()
        } else {
            node.next_sibling()
        };
        while let Some(current) = sibling {
            match &current.data.borrow().value {
                NodeValue::SoftBreak | NodeValue::LineBreak => break,
                NodeValue::Text(text) if text.trim().is_empty() => (),
                _ => return false,
            }
            sibling = if backwards {
                current.previous_sibling()
            } else {
                current.next_sibling()
            };
        }
    }
    true
}

/// Render once for the entire document: references, heading slugs, footnote
/// numbering and back-links are resolved before the reader virtualizes blocks.
pub fn render(source: &str, renderer: &mut dyn Renderer) -> Vec<RenderedBlock> {
    render_with_dialect(source, Dialect::Extended, renderer)
}

/// Render a selected dialect with the same passive HTML policy as [`render`].
/// Host code-block substitutions apply in all dialects; math fences, front
/// matter, and TOC markers are interpreted only in the extended dialect.
pub fn render_with_dialect(
    source: &str,
    dialect: Dialect,
    renderer: &mut dyn Renderer,
) -> Vec<RenderedBlock> {
    let extended = matches!(dialect, Dialect::Extended | Dialect::Article);
    // Keep blocked elements intact until the sanitizer can discard both the
    // tag and its contents. The GFM specification's legacy tag filter would
    // escape only the outer tag, exposing embedded resource tags instead.
    let options = if matches!(dialect, Dialect::Gfm) {
        let mut options = options(Dialect::CommonMark);
        enable_gfm(&mut options);
        options
    } else {
        options(dialect)
    };
    let arena = Arena::new();
    let root = comrak::parse_document(&arena, source, &options);
    let prefix = format!(
        "MAKEPADMARKDOWN{}",
        blake3::hash(source.as_bytes()).to_hex()
    );
    let separator = format!("<!--{prefix}BLOCK-->");
    let mut extensions: Vec<(String, String, String)> = Vec::new();
    let mut headings = Vec::new();
    let mut anchorizer = comrak::Anchorizer::new();
    for node in root.descendants() {
        if let NodeValue::Heading(heading) = &node.data.borrow().value {
            let title = node
                .descendants()
                .filter_map(|n| match &n.data.borrow().value {
                    NodeValue::Text(s) => Some(s.to_string()),
                    NodeValue::Code(c) => Some(c.literal.clone()),
                    _ => None,
                })
                .collect::<String>();
            headings.push((heading.level, title.clone(), anchorizer.anchorize(&title)));
        }
    }
    let toc = format!(
        "<nav><p><strong>Table of contents</strong></p><ul>{}</ul></nav>",
        headings
            .iter()
            .map(|(level, title, id)| format!(
                "<li>{}<a href=\"#{}\">{}</a></li>",
                "　".repeat(level.saturating_sub(1) as usize),
                escape(id),
                escape(title)
            ))
            .collect::<String>()
    );
    let nodes: Vec<_> = root.descendants().collect();
    for node in nodes {
        let replacement = match &node.data.borrow().value {
            NodeValue::CodeBlock(code) => {
                let language = code.info.split_whitespace().next().unwrap_or("");
                let html = if extended && matches!(language, "math" | "katex" | "latex") {
                    renderer.math(&Formula {
                        tex: crate::math::normalize(&code.literal, true),
                        display: true,
                    })
                } else {
                    renderer.code(language, &code.literal)
                };
                Some((html, code.literal.clone()))
            }
            NodeValue::Math(math) => Some((
                renderer.math(&Formula {
                    tex: crate::math::normalize(&math.literal, false),
                    // Editor.md accepts $$…$$ inside a sentence. Only a
                    // formula on its own line should create a display block.
                    display: math.display_math && math_on_own_line(node),
                }),
                math.literal.clone(),
            )),
            NodeValue::Paragraph
                if extended
                    && matches!(node.first_child().map(|n|n.data.borrow().value.clone()),Some(NodeValue::Text(ref s)) if matches!(s.as_ref(),"[TOC]"|"[TOCM]"))
                    && node.children().count() == 1 =>
            {
                Some((
                    toc.clone(),
                    headings
                        .iter()
                        .map(|(_, t, _)| t.as_str())
                        .collect::<Vec<_>>()
                        .join("\n"),
                ))
            }
            _ => None,
        };
        if let Some((html, text)) = replacement {
            let marker = format!("{prefix}EXT{}END", extensions.len());
            extensions.push((marker.clone(), html, text));
            for child in node.children().collect::<Vec<_>>() {
                child.detach();
            }
            let block = node.data.borrow().value.block();
            node.data.borrow_mut().value = if block {
                NodeValue::HtmlBlock(NodeHtmlBlock {
                    block_type: 7,
                    literal: format!("{marker}\n"),
                })
            } else {
                NodeValue::HtmlInline(marker)
            };
        }
    }
    let mut source_lines = vec![0];
    for child in root.children().collect::<Vec<_>>() {
        if matches!(child.data.borrow().value, NodeValue::FootnoteDefinition(_)) {
            continue;
        }
        source_lines.push(child.data.borrow().sourcepos.start.line);
        let split = arena.alloc(AstNode::from(NodeValue::HtmlBlock(NodeHtmlBlock {
            block_type: 7,
            literal: separator.clone(),
        })));
        child.insert_before(split);
    }
    let mut html = String::new();
    comrak::format_html(root, &options, &mut html).expect("String formatting");
    html.split(&separator)
        .enumerate()
        .filter_map(|(index, part)| {
            let mut safe = crate::markup::html_fragment_with_renderer(part, renderer);
            struct Plain {
                extended: bool,
            }
            impl Renderer for Plain {
                fn text(&mut self, text: &str) -> String {
                    if self.extended {
                        if cfg!(feature = "editormd") {
                            crate::render::editormd_emoji_text(text, self)
                        } else {
                            crate::render::emoji_text(text, self)
                        }
                    } else {
                        escape(text)
                    }
                }
            }
            let (mut text, anchors) = text_and_anchors(
                &crate::markup::html_fragment_with_renderer(part, &mut Plain { extended }),
            );
            for (marker, markup, plain) in &extensions {
                safe = safe.replace(marker, markup);
                text = text.replace(marker, plain);
            }
            if safe.trim().is_empty() {
                None
            } else {
                Some(RenderedBlock {
                    source_line: source_lines[index],
                    html: safe,
                    text,
                    anchors,
                })
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::{markdown_render, render::Renderer};
    struct Plain;
    impl Renderer for Plain {}
    #[test]
    fn double_dollar_math_keeps_inline_context() {
        #[derive(Default)]
        struct Math(Vec<bool>);
        impl Renderer for Math {
            fn math(&mut self, formula: &crate::math::Formula) -> String {
                self.0.push(formula.display);
                "FORMULA".into()
            }
        }
        let source = "$$E=mc^2$$\n\n行内的公式$$E=mc^2$$行内的公式，行内的$$E=mc^2$$公式。\n\n$$x > y$$\n\n```math\nx^2\n```\n\n> $$x^2$$\n\n- $$y^2$$\n\nText **$$z^2$$** after\n";
        let mut reader = Math::default();
        markdown_render::render(source, &mut reader);
        assert_eq!(
            reader.0,
            [true, false, false, true, true, true, true, false]
        );
    }

    #[test]
    fn native_document_resolves_global_structure() {
        let source = "# 重复标题\n\n[TOC]\n\n3. three\n4. four\n\n# 重复标题\n\nVisit www.example.com and a@example.com.\n\nNote[^a] and again[^a].\n\n> [!WARNING]\n> Watch **out**.\n\n[^a]: 中文 footnote.\n\n[go](#重复标题-1)\n";
        let blocks = markdown_render::render(source, &mut Plain);
        let html = blocks.iter().map(|b| b.html.as_str()).collect::<String>();
        assert!(html.contains("<ol start=\"3\">"), "{html}");
        assert!(html.contains("href=\"http://www.example.com\""), "{html}");
        assert!(html.contains("href=\"mailto:a@example.com\""), "{html}");
        assert!(html.contains("href=\"#重复标题-1\""), "{html}");
        assert!(
            blocks
                .iter()
                .flat_map(|b| &b.anchors)
                .any(|a| a == "重复标题-1"),
            "{html}"
        );
        assert!(
            html.contains("中文 footnote") && html.contains("Warning"),
            "{html}"
        );
        assert!(html.contains("href=\"#fn-a\""), "{html}");
    }

    #[test]
    fn native_document_does_not_activate_source_code_or_drop_hard_breaks() {
        let source = "a  \nb\n\nsoft\nbreak\n\n- [x] complete\n- [ ] incomplete\n\n```js\nconst text = ':smiley:';\n```\n\n<script>attack()</script>\n\n[bad](javascript:attack)";
        let blocks = markdown_render::render(source, &mut Plain);
        let html = blocks.iter().map(|b| b.html.as_str()).collect::<String>();
        assert!(html.contains("a<br>\nb"), "{html}");
        assert!(html.contains("soft\nbreak"), "{html}");
        assert!(html.contains("☑") && html.contains("☐"), "{html}");
        assert!(html.contains(":smiley:"));
        assert!(
            !html.contains("javascript:") && !html.contains("<script"),
            "{html}"
        );
    }
}
