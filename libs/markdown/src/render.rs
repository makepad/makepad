//! Trusted host substitutions, applied only after sanitizing source markup.
use crate::{escape, math::Formula};

/// Resource requested only by the explicit [`editormd_emoji_text`] adapter.
pub const EDITORMD_LOGO: &str =
    "https://pandao.github.io/editor.md/images/logos/editormd-logo-180x180.png";

/// Navigation is dispatched by the host. Relative references are retained so
/// an imported document can resolve them against its explicitly chosen folder.
pub fn valid_link(value: &str) -> bool {
    if value.len() > 4096 || value.chars().any(|c| c.is_control()) {
        return false;
    }
    if value.starts_with('#') || value.is_empty() {
        return true;
    }
    let Ok(base) = url::Url::parse("https://markdown.invalid/") else {
        return false;
    };
    let Ok(url) = base.join(value) else {
        return false;
    };
    matches!(url.scheme(), "http" | "https" | "mailto")
        && url.username().is_empty()
        && url.password().is_none()
}

pub fn valid_image(value: &str) -> bool {
    if let Some(id) = value.strip_prefix("asset:") {
        return valid_resource_id(id);
    }
    if !valid_link(value) || value.starts_with('#') || value.is_empty() {
        return false;
    }
    url::Url::parse("https://markdown.invalid/")
        .and_then(|base| base.join(value))
        .is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
}

/// An opaque host resource identifier, without paths or executable URI syntax.
/// This validates an identifier only; the host chooses whether it exists and
/// whether the current document may access it.
pub fn valid_resource_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 100
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A passive image reference. This crate never fetches the resource itself.
pub struct ImageRequest<'a> {
    pub url: &'a str,
    pub alt: &'a str,
    pub width: Option<u32>,
}

/// Trusted substitutions inserted after source HTML is sanitized.
///
/// Implementations must escape untrusted text in their returned markup. Raw
/// source cannot create custom native widget tags; a renderer may create them
/// deliberately after resolving resources through the host's own policy.
pub trait Renderer {
    fn math(&mut self, formula: &Formula) -> String {
        crate::math::fallback(formula)
    }
    fn code(&mut self, _language: &str, source: &str) -> String {
        format!("<pre><code>{}</code></pre>", escape(source))
    }
    fn image(&mut self, image: &ImageRequest<'_>) -> String {
        image_link(image)
    }
    fn text(&mut self, text: &str) -> String {
        escape(text)
    }
    fn emoji(&mut self, emoji: &str) -> String {
        escape(emoji)
    }
}

pub fn image_link(image: &ImageRequest<'_>) -> String {
    format!(
        "<a href=\"{}\">[{}]</a>",
        escape(image.url),
        escape(if image.alt.is_empty() {
            "Image"
        } else {
            image.alt
        })
    )
}

/// Shortcodes are interpreted in prose only, never in code, TeX or attributes.
/// The `emoji` feature enables standard shortcodes and Unicode emoji detection.
/// Without that feature this returns escaped text.
pub fn emoji_text(text: &str, renderer: &mut dyn Renderer) -> String {
    emoji_text_with_compat(text, renderer, false)
}

/// Opt into Editor.md's Font Awesome aliases and logo-image shortcodes, in
/// addition to standard emoji when the `emoji` feature is enabled. A logo is
/// an ordinary image request; resource loading still belongs to the host.
pub fn editormd_emoji_text(text: &str, renderer: &mut dyn Renderer) -> String {
    emoji_text_with_compat(text, renderer, true)
}

fn prose(text: &str, renderer: &mut dyn Renderer) -> String {
    #[cfg(feature = "emoji")]
    {
        use unicode_segmentation::UnicodeSegmentation;
        text.graphemes(true)
            .map(|grapheme| {
                if emojis::get(grapheme).is_some() {
                    renderer.emoji(grapheme)
                } else {
                    escape(grapheme)
                }
            })
            .collect()
    }
    #[cfg(not(feature = "emoji"))]
    {
        let _ = renderer;
        escape(text)
    }
}

fn standard_shortcode(name: &str, renderer: &mut dyn Renderer) -> Option<String> {
    #[cfg(feature = "emoji")]
    {
        emojis::get_by_shortcode(name).map(|emoji| renderer.emoji(emoji.as_str()))
    }
    #[cfg(not(feature = "emoji"))]
    {
        let _ = (name, renderer);
        None
    }
}

fn emoji_text_with_compat(text: &str, renderer: &mut dyn Renderer, editormd: bool) -> String {
    let mut output = String::new();
    let mut rest = text;
    while let Some(start) = rest.find(':') {
        output.push_str(&prose(&rest[..start], renderer));
        rest = &rest[start..];
        let Some(end) = rest[1..].find(':').map(|n| n + 1) else {
            break;
        };
        let name = &rest[1..end];
        let replacement = match name {
            "fa-star" if editormd => Some(renderer.emoji("★")),
            "fa-gear" | "fa-cog" if editormd => Some(renderer.emoji("⚙")),
            "editormd-logo" | "editormd-logo-3x" | "editormd-logo-5x" if editormd => {
                Some(renderer.image(&ImageRequest {
                    url: EDITORMD_LOGO,
                    alt: "Editor.md",
                    width: Some(match name {
                        "editormd-logo-3x" => 48,
                        "editormd-logo-5x" => 80,
                        _ => 16,
                    }),
                }))
            }
            _ => standard_shortcode(name, renderer),
        };
        if let Some(replacement) = replacement {
            output.push_str(&replacement);
            rest = &rest[end + 1..];
        } else {
            output.push(':');
            rest = &rest[1..];
        }
    }
    output.push_str(&prose(rest, renderer));
    output
}

/// Decode an anchor without treating a literal plus as a form-encoded space.
pub fn decode_fragment(fragment: &str) -> String {
    url::form_urlencoded::parse(format!("id={}", fragment.replace('+', "%2B")).as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Collector(Vec<String>);
    impl Renderer for Collector {
        fn image(&mut self, image: &ImageRequest<'_>) -> String {
            if valid_image(image.url)
                && !self.0.iter().any(|url| url == image.url)
                && self.0.len() < 128
            {
                self.0.push(image.url.into());
            }
            String::new()
        }
        fn text(&mut self, text: &str) -> String {
            editormd_emoji_text(text, self)
        }
    }
    #[test]
    fn resources_are_collected_from_markup_and_emoji_but_not_code_or_scripts() {
        let source = "![logo](https://example.org/a.png)\n\n<img src='https://example.org/b.svg'>\n\n:editormd-logo: :smiley: 😃\n\n```html\n<img src='https://example.org/code.png'>\n```\n\n<script><img src='https://example.org/script.png'></script>";
        let mut collector = Collector::default();
        let blocks = crate::markdown_render::render(source, &mut collector);
        assert_eq!(
            collector.0,
            [
                "https://example.org/a.png",
                "https://example.org/b.svg",
                EDITORMD_LOGO
            ]
        );
        assert!(blocks.iter().all(|block| !block.html.contains("<img")));
    }
}
