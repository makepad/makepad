//! What a copy of rendered Markdown puts on the clipboard: the Markdown it
//! cuts, the text a reader sees of it and its HTML, beside each other so a
//! paste target takes the richest it understands.

use crate::index::Index;
use pulldown_cmark::{html, Event, Tag, TagEnd};

/// A copy's flavours.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flavours {
    /// The text as it reads: no markers, a table's cells apart by tabs and
    /// its rows by lines (as a spreadsheet takes them).
    pub text: String,
    /// The Markdown source itself.
    pub markdown: String,
    /// The Markdown as HTML.
    pub html: String,
}

impl Index {
    /// The flavours of Markdown cut from this document. Its links resolve as
    /// they do in the document.
    pub fn flavours(&self, markdown: String) -> Flavours {
        let events = self.parse_fragment(&markdown);
        let text = text_of(&events);
        let mut out = String::new();
        html::push_html(&mut out, events.into_iter());
        Flavours { text, html: out, markdown }
    }

    /// The text a reader sees of Markdown cut from this document (its text
    /// flavour alone): a heading's title for a PDF bookmark, say.
    pub fn reading_text(&self, markdown: &str) -> String {
        text_of(&self.parse_fragment(markdown))
    }
}

/// The text a reader sees of the events: a block's words, paragraphs and
/// headings apart by a blank line, list items and table rows by a line, a
/// row's cells by a tab.
fn text_of(events: &[Event<'_>]) -> String {
    let mut out = String::new();
    for event in events {
        match event {
            Event::Text(text) | Event::Code(text) | Event::InlineMath(text) | Event::DisplayMath(text) => {
                out.push_str(text)
            }
            Event::SoftBreak => out.push(' '),
            Event::HardBreak => out.push('\n'),
            Event::FootnoteReference(label) => {
                out.push('[');
                out.push_str(label);
                out.push(']');
            }
            Event::TaskListMarker(done) => out.push_str(if *done { "[x] " } else { "[ ] " }),
            Event::Start(Tag::Item) | Event::Start(Tag::TableRow) | Event::Start(Tag::TableHead) => line(&mut out),
            Event::End(TagEnd::TableCell) => out.push('\t'),
            Event::End(TagEnd::TableRow) | Event::End(TagEnd::TableHead) => {
                if out.ends_with('\t') {
                    out.pop();
                }
                out.push('\n');
            }
            Event::End(TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::Table)
            | Event::Rule => {
                line(&mut out);
                if !out.is_empty() {
                    out.push('\n');
                }
            }
            _ => {}
        }
    }
    out.truncate(out.trim_end_matches('\n').len());
    out
}

/// Ends the line being written, if one is.
fn line(out: &mut String) {
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use crate::Index;

    /// A copy's three flavours: the text reads without markers (a table's
    /// cells apart by tabs), the HTML renders the Markdown with the
    /// document's reference links resolved, the Markdown is kept as cut.
    #[test]
    fn a_copy_reads_as_text_markdown_and_html() {
        let lines: Vec<String> = ["Intro", "", "[home]: https://example.com/a?b&c"].map(String::from).to_vec();
        let index = Index::new(&lines);
        let cut = "Some *very* `code` [link][home]\nwraps\n\n- [x] done\n- next\n";
        let f = index.flavours(cut.into());
        assert_eq!(f.text, "Some very code link wraps\n\n[x] done\nnext", "the text reads without markers");
        assert!(f.html.contains("<em>very</em> <code>code</code>"), "inline marks render: {}", f.html);
        assert!(f.html.contains("<a href=\"https://example.com/a?b&amp;c\">link</a>"), "the reference resolves: {}", f.html);
        assert_eq!(f.markdown, cut, "the Markdown is kept");
        assert_eq!(index.reading_text("## A *b* [c][home] ##"), "A b c", "a heading reads as its title");
        let t = index.flavours("| a | b |\n|---|--:|\n| 1 | **2** |\n".into());
        assert_eq!(t.text, "a\tb\n1\t2", "a table reads as cells by tabs, rows by lines");
        assert!(t.html.contains("<td style=\"text-align: right\"><strong>2</strong></td>"), "a table renders: {}", t.html);
    }
}
