//! Passive mail content, rendered with the active OS theme. No sender CSS,
//! scripts, forms or image URLs reach the widget tree or start network requests.
//! Remote content is never loaded, including on demand. Links are navigation
//! actions only; they are never prefetched or expanded into previews.
use makepad_html::{parse_html, HtmlNode};
use makepad_widgets::{id, InternLiveId, LiveId};

fn escaped(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
pub fn plain(text: &str) -> String {
    text.replace("\r\n", "\n")
        .split("\n\n")
        .filter(|p| !p.trim().is_empty())
        .map(|p| {
            let quote = p.lines().all(|l| l.starts_with('>') || l.is_empty());
            let content = p
                .lines()
                .map(|l| {
                    escaped(if quote {
                        l.trim_start_matches('>').trim_start()
                    } else {
                        l
                    })
                })
                .collect::<Vec<_>>()
                .join("<br/>");
            if quote {
                format!("<blockquote>{content}</blockquote>")
            } else {
                format!("<p>{content}</p>")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
/// Apply the passive-content policy at the display boundary as well as at
/// ingestion. Cached markup and future source adapters cannot bypass it.
/// Run on the mail worker, before publishing the body to the UI.
pub fn preview(markup: &str) -> String {
    let mut chars = markup.chars();
    let bounded: String = chars.by_ref().take(100_000).collect();
    let mut result = sanitize(&bounded);
    if chars.next().is_some() {
        result.push_str("<p><em>This preview is shortened. The complete downloaded text is searchable.</em></p>");
    }
    result
}
pub fn sanitize(raw: &str) -> String {
    let doc = parse_html(raw, &mut None, InternLiveId::No);
    let tags = [
        (id!(p), "p"),
        (id!(div), "p"),
        (id!(strong), "strong"),
        (id!(b), "b"),
        (id!(em), "em"),
        (id!(i), "i"),
        (id!(u), "u"),
        (id!(ul), "ul"),
        (id!(ol), "ol"),
        (id!(li), "li"),
        (id!(blockquote), "blockquote"),
        (id!(pre), "pre"),
        (id!(code), "code"),
        (id!(h1), "h3"),
        (id!(h2), "h3"),
        (id!(h3), "h3"),
        (id!(tr), "p"),
        (id!(td), "span"),
        (id!(th), "strong"),
        (id!(a), "a"),
    ];
    let mut out = String::new();
    let mut hidden = None;
    let mut stack: Vec<(LiveId, &'static str)> = Vec::new();
    for (index, node) in doc.nodes.iter().enumerate() {
        match node {
            HtmlNode::OpenTag { lc, .. }
                if [
                    id!(head),
                    id!(style),
                    id!(script),
                    id!(iframe),
                    id!(object),
                    id!(svg),
                    id!(form),
                ]
                .contains(lc) =>
            {
                if hidden.is_none() {
                    hidden = Some(*lc);
                }
            }
            HtmlNode::CloseTag { lc, .. } if hidden == Some(*lc) => hidden = None,
            _ if hidden.is_some() => {}
            HtmlNode::Text { start, end, .. } => out.push_str(&escaped(&doc.decoded[*start..*end])),
            HtmlNode::OpenTag { lc, .. } if *lc == id!(br) => out.push_str("<br/>"),
            HtmlNode::OpenTag { lc, .. } if *lc == id!(hr) => out.push_str("<hr/>"),
            HtmlNode::OpenTag { lc, .. } => {
                if let Some((_, tag)) = tags.iter().find(|(id, _)| id == lc) {
                    out.push('<');
                    out.push_str(tag);
                    if *lc == id!(a) {
                        for attr in doc.nodes.iter().skip(index + 1) {
                            let HtmlNode::Attribute { lc, start, end, .. } = attr else {
                                break;
                            };
                            if *lc == id!(href) {
                                let href = doc.decoded[*start..*end].trim();
                                let lower = href.to_ascii_lowercase();
                                if ["https://", "http://", "mailto:"]
                                    .iter()
                                    .any(|s| lower.starts_with(s))
                                {
                                    out.push_str(" href=\"");
                                    out.push_str(&escaped(href));
                                    out.push('"');
                                }
                            }
                        }
                    }
                    out.push('>');
                    stack.push((*lc, *tag));
                }
            }
            HtmlNode::CloseTag { lc, .. } => {
                if let Some(at) = stack.iter().rposition(|(id, _)| id == lc) {
                    while stack.len() > at {
                        let (_, tag) = stack.pop().unwrap();
                        out.push_str("</");
                        out.push_str(tag);
                        out.push('>');
                    }
                    if *lc == id!(td) || *lc == id!(th) {
                        out.push(' ');
                    }
                }
            }
            _ => {}
        }
    }
    while let Some((_, tag)) = stack.pop() {
        out.push_str("</");
        out.push_str(tag);
        out.push('>');
    }
    out
}
