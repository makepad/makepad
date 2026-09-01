use makepad_musicxml::{parse_musicxml, MusicXmlDocument};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CandidateSource {
    Fenced { language: String },
    ConversationalText,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MusicXmlCandidate {
    pub source: CandidateSource,
    pub xml: String,
    /// False when a score root was found but its matching closing tag was not.
    pub complete: bool,
    pub byte_offset: usize,
}

impl MusicXmlCandidate {
    pub fn parse(&self) -> Result<MusicXmlDocument, String> {
        if !self.complete {
            return Err("MusicXML candidate is truncated (missing score closing tag)".to_string());
        }
        parse_musicxml(&self.xml).map_err(|error| error.to_string())
    }
}

/// Finds every plausible MusicXML payload instead of trusting the first code
/// fence. This lets the engine evaluate a draft and a later corrected block,
/// while retaining truncated candidates as explicit repair evidence.
pub fn extract_musicxml_candidates(reply: &str) -> Vec<MusicXmlCandidate> {
    let mut located = Vec::new();
    collect_fenced(reply, &mut located);
    collect_roots(reply, 0, CandidateSource::ConversationalText, &mut located);
    located.sort_by_key(|candidate| candidate.byte_offset);

    let mut seen = BTreeSet::new();
    located
        .into_iter()
        .filter(|candidate| seen.insert(candidate.xml.clone()))
        .collect()
}

fn collect_fenced(reply: &str, output: &mut Vec<MusicXmlCandidate>) {
    let mut cursor = 0usize;
    while let Some(relative_open) = reply[cursor..].find("```") {
        let open = cursor + relative_open;
        let header_start = open + 3;
        let Some(relative_newline) = reply[header_start..].find('\n') else {
            break;
        };
        let content_start = header_start + relative_newline + 1;
        let language = reply[header_start..header_start + relative_newline]
            .trim()
            .to_ascii_lowercase();
        let (content_end, next_cursor) = match reply[content_start..].find("```") {
            Some(relative_close) => {
                let close = content_start + relative_close;
                (close, close + 3)
            }
            None => (reply.len(), reply.len()),
        };
        let content = &reply[content_start..content_end];
        if language.contains("xml")
            || content.contains("<score-partwise")
            || content.contains("<score-timewise")
        {
            collect_roots(
                content,
                content_start,
                CandidateSource::Fenced {
                    language: language.clone(),
                },
                output,
            );
        }
        if next_cursor <= cursor {
            break;
        }
        cursor = next_cursor;
    }
}

fn collect_roots(
    text: &str,
    base_offset: usize,
    source: CandidateSource,
    output: &mut Vec<MusicXmlCandidate>,
) {
    let mut cursor = 0usize;
    while cursor < text.len() {
        let partwise = text[cursor..]
            .find("<score-partwise")
            .map(|offset| (cursor + offset, "score-partwise"));
        let timewise = text[cursor..]
            .find("<score-timewise")
            .map(|offset| (cursor + offset, "score-timewise"));
        let Some((start, root)) = earliest(partwise, timewise) else {
            break;
        };
        let close_tag = format!("</{root}>");
        let after_start = start + root.len() + 1;
        if let Some(relative_close) = text[after_start..].find(&close_tag) {
            let end = after_start + relative_close + close_tag.len();
            output.push(MusicXmlCandidate {
                source: source.clone(),
                xml: text[start..end].trim().to_string(),
                complete: true,
                byte_offset: base_offset + start,
            });
            cursor = end;
        } else {
            output.push(MusicXmlCandidate {
                source: source.clone(),
                xml: text[start..].trim().to_string(),
                complete: false,
                byte_offset: base_offset + start,
            });
            break;
        }
    }
}

fn earliest(
    left: Option<(usize, &'static str)>,
    right: Option<(usize, &'static str)>,
) -> Option<(usize, &'static str)> {
    match (left, right) {
        (Some(left), Some(right)) => Some(if left.0 <= right.0 { left } else { right }),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}
