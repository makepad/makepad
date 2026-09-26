//! Passive HTML fragments with trusted host substitutions.
use crate::escape;
use html5ever::tokenizer::{
    states::RawKind, BufferQueue, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer,
};
use std::cell::RefCell;

pub fn markdown_html(source: &str) -> String {
    crate::math::markdown_html(source, crate::math::fallback)
}

#[derive(Default)]
struct Fragment {
    html: String,
    stack: Vec<String>,
    suppressed: Option<String>,
}
struct Sanitizer<'a>(
    RefCell<Fragment>,
    RefCell<&'a mut dyn crate::render::Renderer>,
);
fn allowed_tag(name: &str) -> bool {
    matches!(
        name,
        "p" | "br"
            | "hr"
            | "div"
            | "span"
            | "section"
            | "article"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "blockquote"
            | "ul"
            | "ol"
            | "li"
            | "pre"
            | "code"
            | "strong"
            | "b"
            | "em"
            | "i"
            | "u"
            | "s"
            | "del"
            | "strike"
            | "sub"
            | "sup"
            | "abbr"
            | "a"
            | "table"
            | "thead"
            | "tbody"
            | "tfoot"
            | "tr"
            | "td"
            | "th"
            | "caption"
            | "details"
            | "summary"
            | "figure"
            | "figcaption"
            | "dl"
            | "dt"
            | "dd"
            | "nav"
    )
}
impl TokenSink for Sanitizer<'_> {
    type Handle = ();
    fn process_token(&self, token: Token, _: u64) -> TokenSinkResult<()> {
        let mut fragment = self.0.borrow_mut();
        match token {
            Token::TagToken(tag) => {
                let name = tag.name.as_ref();
                if let Some(suppressed) = &fragment.suppressed {
                    if tag.kind == TagKind::EndTag && suppressed == name {
                        fragment.suppressed = None;
                    }
                    return TokenSinkResult::Continue;
                }
                if tag.kind == TagKind::StartTag
                    && matches!(name, "script" | "style" | "title" | "textarea" | "xmp")
                {
                    fragment.suppressed = Some(name.into());
                    return TokenSinkResult::RawData(RawKind::Rawtext);
                }
                if tag.kind == TagKind::StartTag && name == "img" {
                    let attribute = |key: &str| {
                        tag.attrs
                            .iter()
                            .find(|a| a.name.local.as_ref() == key)
                            .map(|a| a.value.as_ref())
                    };
                    let label = attribute("alt")
                        .filter(|s| !s.is_empty())
                        .unwrap_or("Image");
                    if let Some(url) =
                        attribute("src").filter(|url| crate::render::valid_image(url))
                    {
                        fragment.html.push_str(&self.1.borrow_mut().image(
                            &crate::render::ImageRequest {
                                url,
                                alt: label,
                                width: None,
                            },
                        ));
                    } else {
                        fragment.html.push_str(&format!("[{}]", escape(label)));
                    }
                } else if tag.kind == TagKind::StartTag
                    && name == "input"
                    && tag
                        .attrs
                        .iter()
                        .any(|a| a.name.local.as_ref() == "type" && a.value.as_ref() == "checkbox")
                {
                    fragment.html.push_str(
                        if tag.attrs.iter().any(|a| a.name.local.as_ref() == "checked") {
                            "☑ "
                        } else {
                            "☐ "
                        },
                    );
                } else if allowed_tag(name) {
                    if tag.kind == TagKind::EndTag {
                        if let Some(index) = fragment.stack.iter().rposition(|n| n == name) {
                            while fragment.stack.len() > index {
                                let close = fragment.stack.pop().unwrap();
                                fragment.html.push_str(&format!("</{close}>"));
                            }
                        }
                    } else if fragment.stack.len() < 48 {
                        fragment.html.push('<');
                        fragment.html.push_str(name);
                        for attr in &tag.attrs {
                            let key = attr.name.local.as_ref();
                            let value = attr.value.as_ref();
                            let allowed = match (name, key) {
                                ("a", "href") => crate::render::valid_link(value),
                                (_, "id") => {
                                    value.len() <= 256 && !value.chars().any(char::is_control)
                                }
                                (_, "title") => value.len() <= 512,
                                ("td" | "th", "align") => {
                                    matches!(value, "left" | "center" | "right")
                                }
                                ("ol", "start") => {
                                    value.parse::<u32>().is_ok_and(|n| n <= 999_999_999)
                                }
                                ("td" | "th", "colspan" | "rowspan") => {
                                    value.parse::<u16>().is_ok_and(|n| n > 0 && n <= 1000)
                                }
                                ("details", "open") => true,
                                _ => false,
                            };
                            if allowed {
                                fragment
                                    .html
                                    .push_str(&format!(" {key}=\"{}\"", escape(value)));
                            } else if matches!(name, "td" | "th") && key == "style" {
                                let align = value
                                    .trim()
                                    .trim_end_matches(';')
                                    .strip_prefix("text-align:")
                                    .map(str::trim);
                                if let Some(align @ ("left" | "center" | "right")) = align {
                                    fragment.html.push_str(&format!(" align=\"{align}\""));
                                }
                            }
                        }
                        fragment.html.push('>');
                        if !matches!(name, "hr" | "br") {
                            fragment.stack.push(name.into());
                        }
                    }
                }
            }
            Token::CharacterTokens(text) if fragment.suppressed.is_none() => {
                let rendered = if fragment
                    .stack
                    .iter()
                    .any(|tag| matches!(tag.as_str(), "pre" | "code"))
                {
                    escape(&text)
                } else {
                    self.1.borrow_mut().text(&text)
                };
                fragment.html.push_str(&rendered);
            }
            Token::NullCharacterToken if fragment.suppressed.is_none() => fragment.html.push('�'),
            _ => (),
        }
        TokenSinkResult::Continue
    }
}

/// Sanitize passive HTML structure. Scripts, styles, event handlers, and image
/// loading are excluded; images become links unless the host supplies a renderer.
pub fn html_fragment(source: &str) -> String {
    struct Inert;
    impl crate::render::Renderer for Inert {}
    html_fragment_with_renderer(source, &mut Inert)
}

pub fn html_fragment_with_renderer(
    source: &str,
    renderer: &mut dyn crate::render::Renderer,
) -> String {
    let input = BufferQueue::default();
    input.push_back(source.into());
    let tokenizer = Tokenizer::new(
        Sanitizer(RefCell::new(Fragment::default()), RefCell::new(renderer)),
        Default::default(),
    );
    let _ = tokenizer.feed(&input);
    tokenizer.end();
    let mut fragment = tokenizer.sink.0.into_inner();
    while let Some(name) = fragment.stack.pop() {
        fragment.html.push_str(&format!("</{name}>"));
    }
    fragment.html
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_html_renders_inert_content() {
        let source = "<!doctype html><html><head><style>p {color:red}</style></head><body><h1>中文</h1><p onclick='run()'>hello <b>world</b></p><script>attack()</script><img src='file:///etc/passwd'><a href='jav&#97;script:run()'>bad</a><table><tr><td>cell</td></tr></table></body></html>";
        let html = html_fragment(source);
        for unsafe_text in [
            "<script",
            "attack()",
            "onclick",
            "file:",
            "javascript:",
            "<style",
            "color:red",
        ] {
            assert!(!html.contains(unsafe_text), "{html}");
        }
        assert!(html.contains("<b>world</b>"));
        assert!(html.contains("<td>cell</td>"));
    }
}
