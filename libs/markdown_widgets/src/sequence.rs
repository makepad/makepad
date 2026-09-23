//! Editor.md/js-sequence-diagrams grammar adapter, plus Mermaid arrow semantics.
//! This builds the renderer's IR; original Markdown is never rewritten.
use mermaid_rs_renderer::{
    DiagramKind, Direction, Edge, EdgeDecoration, EdgeStyle, Graph, NodeShape,
    ir::{SequenceNote, SequenceNotePosition},
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Head {
    None,
    Filled,
    Open,
    Cross,
    Both,
}

#[derive(Debug)]
pub(crate) struct Sequence {
    pub graph: Graph,
    pub heads: Vec<Head>,
    pub title: Option<String>,
}

fn unquote(text: &str) -> Result<String, String> {
    let text = text.trim();
    let text = if text.starts_with('"') {
        text.strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .ok_or("Unclosed quoted name")?
    } else {
        if text.contains('"') {
            return Err("Unexpected quote in name".into());
        }
        text
    };
    if text.is_empty() {
        return Err("Missing participant name".into());
    }
    Ok(text.replace("\\n", "\n"))
}

/// Find punctuation outside a quoted actor name. Byte offsets remain valid for CJK.
fn outside(text: &str, predicate: impl Fn(char) -> bool) -> Option<usize> {
    let mut quoted = false;
    for (i, c) in text.char_indices() {
        if c == '"' {
            quoted = !quoted;
        } else if !quoted && predicate(c) {
            return Some(i);
        }
    }
    None
}

fn keyword<'a>(line: &'a str, word: &str) -> Option<&'a str> {
    let prefix = line.get(..word.len())?;
    let rest = &line[word.len()..];
    (prefix.eq_ignore_ascii_case(word)
        && (rest.is_empty() || rest.starts_with(char::is_whitespace)))
    .then(|| rest.trim())
}

fn message(line: &str) -> Result<(&str, String), String> {
    let colon = outside(line, |c| c == ':').ok_or("Expected ':' before the message")?;
    let text = line[colon + 1..].trim();
    if text.is_empty() {
        return Err("Missing message text".into());
    }
    Ok((&line[..colon], text.trim_matches('"').replace("\\n", "\n")))
}

fn actor(graph: &mut Graph, alias: &str, label: Option<String>) -> String {
    // IDs stay separate from display text, including quoted names with punctuation.
    let id = alias.to_owned();
    if !graph.nodes.contains_key(&id) {
        graph.sequence_participants.push(id.clone());
    }
    graph.ensure_node(&id, label, Some(NodeShape::ActorBox));
    id
}

pub(crate) fn editor(source: &str) -> Result<Sequence, String> {
    let mut graph = Graph::new();
    graph.kind = DiagramKind::Sequence;
    graph.direction = Direction::LeftRight;
    let mut heads = Vec::new();
    let mut title = None;
    for (line_no, raw) in source.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parse_line = |graph: &mut Graph,
                          heads: &mut Vec<Head>,
                          title: &mut Option<String>|
         -> Result<(), String> {
            if let Some(rest) = keyword(line, "participant") {
                let rest = &rest[..outside(rest, |c| c == '#').unwrap_or(rest.len())];
                let rest = rest.trim();
                // The original lexer permits the entire declaration in quotes,
                // e.g. participant "Long display name as ID".
                let declaration = if rest.starts_with('"') && rest.ends_with('"') {
                    unquote(rest)?
                } else {
                    rest.to_owned()
                };
                let rest = declaration.as_str();
                // js-sequence uses "display name as alias", the reverse of Mermaid.
                let lower = rest.to_ascii_lowercase();
                let (alias, label) = if let Some(i) = lower.rfind(" as ") {
                    (unquote(&rest[i + 4..])?, Some(unquote(&rest[..i])?))
                } else {
                    (unquote(rest)?, None)
                };
                actor(graph, &alias, label);
                return Ok(());
            }
            if line
                .get(..5)
                .is_some_and(|p| p.eq_ignore_ascii_case("title"))
            {
                let rest = line[5..]
                    .trim()
                    .strip_prefix(':')
                    .unwrap_or(line[5..].trim())
                    .trim();
                if rest.is_empty() {
                    return Err("Missing title".into());
                }
                *title = Some(rest.replace("\\n", "\n"));
                return Ok(());
            }
            if let Some(rest) = keyword(line, "note") {
                let (targets, label) = message(rest)?;
                let (position, targets) = if let Some(rest) = keyword(targets, "left of") {
                    (SequenceNotePosition::LeftOf, rest)
                } else if let Some(rest) = keyword(targets, "right of") {
                    (SequenceNotePosition::RightOf, rest)
                } else if let Some(rest) = keyword(targets, "over") {
                    (SequenceNotePosition::Over, rest)
                } else {
                    return Err("Expected note left of, right of, or over".into());
                };
                let mut participants = Vec::new();
                let mut remaining = targets;
                loop {
                    let comma = outside(remaining, |c| c == ',');
                    let end = comma.unwrap_or(remaining.len());
                    let name = unquote(&remaining[..end])?;
                    participants.push(actor(graph, &name, None));
                    if let Some(i) = comma {
                        remaining = &remaining[i + 1..];
                    } else {
                        break;
                    }
                }
                if participants.len() > 2
                    || (position != SequenceNotePosition::Over && participants.len() != 1)
                    || (participants.len() == 2 && participants[0] == participants[1])
                {
                    return Err(
                        "A note needs one participant, or two distinct participants after 'over'"
                            .into(),
                    );
                }
                graph.sequence_notes.push(SequenceNote {
                    position,
                    participants,
                    label,
                    index: graph.edges.len(),
                });
                return Ok(());
            }
            let (signal, label) = message(line)?;
            let start = outside(signal, |c| c == '-').ok_or("Expected a sequence arrow")?;
            let token = ["-->>", "->>", "-->", "->", "--", "-"]
                .into_iter()
                .find(|t| signal[start..].starts_with(t))
                .unwrap();
            let from = unquote(&signal[..start])?;
            let to = unquote(&signal[start + token.len()..])?;
            if outside(&signal[start + token.len()..], |c| {
                matches!(c, '-' | '>' | ',')
            })
            .is_some()
            {
                return Err("Unexpected punctuation in participant; quote the name".into());
            }
            let from = actor(graph, &from, None);
            let to = actor(graph, &to, None);
            let head = if token.ends_with(">>") {
                Head::Open
            } else {
                Head::Filled
            };
            heads.push(head);
            graph.edges.push(Edge {
                from,
                to,
                label: Some(label),
                start_label: None,
                end_label: None,
                directed: true,
                arrow_start: false,
                arrow_end: true,
                arrow_start_kind: None,
                arrow_end_kind: None,
                start_decoration: None,
                end_decoration: None,
                style: if token.starts_with("--") {
                    EdgeStyle::Dotted
                } else {
                    EdgeStyle::Solid
                },
            });
            Ok(())
        };
        parse_line(&mut graph, &mut heads, &mut title)
            .map_err(|e| format!("Line {}: {e}", line_no + 1))?;
    }
    if graph.nodes.is_empty() {
        return Err("Sequence diagram has no participants".into());
    }
    Ok(Sequence {
        graph,
        heads,
        title,
    })
}

/// mmdr 0.3.1 loses arrowhead distinctions. Normalize supported arrows for its
/// layout parser, then restore their semantics in the IR and SVG markers.
pub(crate) fn mermaid(source: &str) -> Result<Sequence, String> {
    let mut normalized = String::new();
    let mut heads = Vec::new();
    let mut title = None;
    let mut frontmatter = false;
    let mut started = false;
    for (line_no, raw) in source.lines().enumerate() {
        let line = raw.trim();
        if line == "---" && !started {
            frontmatter = !frontmatter;
            continue;
        }
        if frontmatter {
            if let Some(value) = line.strip_prefix("title:") {
                title = Some(value.trim().trim_matches('"').into());
            }
            continue;
        }
        if line.is_empty() || line.starts_with("%%") {
            continue;
        }
        // Semicolon statements are supported; numeric/named entities stay intact.
        for line in statements(line) {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            started = true;
            let lower = line.to_ascii_lowercase();
            if lower == "sequencediagram" {
                normalized.push_str("sequenceDiagram\n");
                continue;
            }
            if let Some(value) = line.strip_prefix("title:") {
                title = Some(value.trim().into());
                continue;
            }
            let command = [
                "participant",
                "actor",
                "database",
                "boundary",
                "control",
                "entity",
                "note",
                "activate",
                "deactivate",
                "autonumber",
                "box",
                "alt",
                "opt",
                "loop",
                "par",
                "rect",
                "critical",
                "break",
                "else",
                "and",
                "option",
                "end",
            ]
            .iter()
            .any(|word| keyword(line, word).is_some());
            if command {
                normalized.push_str(line);
                normalized.push('\n');
                continue;
            }
            let (signal, label) =
                message(line).map_err(|e| format!("Line {}: {e}", line_no + 1))?;
            let start = outside(signal, |c| c == '-' || c == '<')
                .ok_or_else(|| format!("Line {}: unsupported sequence statement", line_no + 1))?;
            let token = [
                "<<-->>", "<<->>", "-->>", "->>", "-->", "->", "--)", "-)", "--x", "-x",
            ]
            .into_iter()
            .find(|t| signal[start..].starts_with(t))
            .ok_or_else(|| format!("Line {}: unsupported sequence arrow", line_no + 1))?;
            let head = if token.starts_with("<<") {
                Head::Both
            } else if token.ends_with(')') {
                Head::Open
            } else if token.ends_with('x') {
                Head::Cross
            } else if token.ends_with(">>") {
                Head::Filled
            } else {
                Head::None
            };
            let mut rest = signal[start + token.len()..].trim();
            let activation = if rest.starts_with(['+', '-']) {
                let a = &rest[..1];
                rest = rest[1..].trim();
                a
            } else {
                ""
            };
            if signal[..start].trim().is_empty()
                || rest.is_empty()
                || outside(rest, |c| matches!(c, '-' | '>' | '<' | ')')).is_some()
            {
                return Err(format!(
                    "Line {}: invalid sequence participant or unsupported arrow",
                    line_no + 1
                ));
            }
            let arrow = if token.contains("--") { "-->>" } else { "->>" };
            normalized.push_str(&format!(
                "{}{arrow}{activation}{rest}:{}\n",
                signal[..start].trim(),
                label.replace('\n', "<br/>")
            ));
            heads.push(head);
        }
    }
    // Mermaid creates undeclared participants implicitly, including when some
    // participants have explicit aliases. The upstream strict check rejects it.
    match mermaid_rs_renderer::validator::validate(&normalized) {
        Ok(()) | Err(mermaid_rs_renderer::ParseError::UnknownParticipant { .. }) => {}
        Err(error) => return Err(error.to_string()),
    }
    let mut graph = mermaid_rs_renderer::parse_mermaid(&normalized)
        .map_err(|e| e.to_string())?
        .graph;
    if graph.edges.len() != heads.len() {
        return Err("Sequence message could not be parsed completely".into());
    }
    for (edge, head) in graph.edges.iter_mut().zip(&heads) {
        edge.arrow_end = !matches!(head, Head::None | Head::Cross);
        edge.arrow_start = *head == Head::Both;
        if *head == Head::Cross {
            edge.end_decoration = Some(EdgeDecoration::Cross);
        }
    }
    if graph.nodes.is_empty() {
        return Err("Sequence diagram has no participants".into());
    }
    Ok(Sequence {
        graph,
        heads,
        title,
    })
}

fn statements(line: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut entity = false;
    for (i, c) in line.char_indices() {
        if c == '"' {
            quoted = !quoted;
        }
        if !quoted {
            if c == '#' {
                entity = true;
            } else if c == ';' {
                if !entity {
                    parts.push(&line[start..i]);
                    start = i + 1;
                }
                entity = false;
            } else if !c.is_alphanumeric() {
                entity = false;
            }
        }
    }
    parts.push(&line[start..]);
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn editor_dialect_keeps_arrows_aliases_notes_and_order() {
        let s=editor("Title: 中文\nparticipant 客户端 as C\nparticipant 服务端 as S\nC->S: 请求\nS-->C: 完成\nC->>S: 开放\\n箭头\nNote over C,S: 提示\nS-->>C: 结束").unwrap();
        assert_eq!(s.graph.sequence_participants, ["C", "S"]);
        assert_eq!(s.graph.nodes["C"].label, "客户端");
        assert_eq!(
            s.heads,
            [Head::Filled, Head::Filled, Head::Open, Head::Open]
        );
        assert_eq!(s.graph.edges[1].style, EdgeStyle::Dotted);
        assert_eq!(s.graph.edges[2].label.as_deref(), Some("开放\n箭头"));
        assert_eq!(s.graph.sequence_notes[0].index, 3);
        assert_eq!(s.title.as_deref(), Some("中文"));
    }
    #[test]
    fn quoted_participants_and_diagnostics() {
        let s =
            editor("\"A:B-C\"->\"D,E\": say: hello # message\nNote over \"A:B-C\",\"D,E\": yes")
                .unwrap();
        assert_eq!(s.graph.nodes.len(), 2);
        assert!(
            editor("A->B: ok\nNote over A,A: bad")
                .unwrap_err()
                .contains("Line 2")
        );
        assert!(editor("A->: hello").is_err());
        assert!(editor("A->B: hello\nunknown statement").is_err());
        let s = editor("participant \"中文 # Name as C\" # comment\nC->C: ok").unwrap();
        assert_eq!(s.graph.nodes["C"].label, "中文 # Name");
    }
    #[test]
    fn mermaid_arrows_frames_and_implicit_participants() {
        let s=mermaid("sequenceDiagram\nparticipant A as 客户端\nA->>+B: 请求\nalt 成功\nB-->>-A: 完成\nelse 重试\nA-)B: 异步\nend\nA->B: 无箭头\nA--xB: 失败\nB<<->>A: 双向").unwrap();
        assert_eq!(
            s.heads,
            [
                Head::Filled,
                Head::Filled,
                Head::Open,
                Head::None,
                Head::Cross,
                Head::Both
            ]
        );
        assert_eq!(s.graph.sequence_frames.len(), 1);
        assert_eq!(s.graph.sequence_activations.len(), 2);
        assert!(!s.graph.edges[3].arrow_end);
        assert!(s.graph.edges[5].arrow_start);
        assert!(mermaid("sequenceDiagram\nloop open\nA->>B: hello").is_err());
        assert!(mermaid("sequenceDiagram\nA->>B: hi\nnot supported").is_err());
    }
}
