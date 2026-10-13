//! What the model has seen, kept on our side: the items of a conversation,
//! their token estimate, and the text forms used to seed a fresh session.
//!
//! The hub's chat session is append-only (the node's KV cache extends it),
//! so folding old turns into a summary means starting a new session whose
//! system text carries the summary and the most recent turns.

use super::event::{ContextSegment, SegmentKind};

/// Rough tokens for English/code text.
pub fn est_tokens(chars: usize) -> u32 {
    (chars as f32 / 3.5).ceil() as u32
}

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    /// A request; `refs` are the files the person attached, `context` the
    /// file text sent with it (counted, not saved: a resume sends the turn
    /// without it and the model reads the files again if it needs them).
    User { text: String, refs: Vec<String>, context: String },
    Assistant(String),
    Tool { name: String, args: String, result: String, ok: bool },
    /// Old turns folded into prose.
    Summary(String),
}

impl Item {
    pub fn chars(&self) -> usize {
        match self {
            Item::User { text, context, .. } => text.chars().count() + context.chars().count(),
            Item::Assistant(t) | Item::Summary(t) => t.chars().count(),
            // The call markup and the response wrapper cost a little too.
            Item::Tool { name, args, result, .. } => name.len() + args.len() + result.chars().count() + 60,
        }
    }

    pub fn kind(&self) -> SegmentKind {
        match self {
            Item::User { .. } => SegmentKind::User,
            Item::Assistant(_) => SegmentKind::Assistant,
            Item::Tool { .. } => SegmentKind::ToolResult,
            Item::Summary(_) => SegmentKind::Summary,
        }
    }

    /// The item as plain text for a seed or a summary request; long tool
    /// results are clipped to `clip` chars.
    pub fn render(&self, clip: usize) -> String {
        match self {
            Item::User { text, refs, .. } if refs.is_empty() => format!("User: {text}\n"),
            Item::User { text, refs, .. } => format!("User: {text}\n(attached: {})\n", refs.join(", ")),
            Item::Assistant(t) => format!("Assistant: {t}\n"),
            Item::Summary(t) => format!("Summary of earlier work: {t}\n"),
            Item::Tool { name, args, result, ok } => {
                let result = super::tools::truncate_middle(result, clip);
                let flag = if *ok { "" } else { " (failed)" };
                format!("Tool {name}({args}){flag}: {result}\n")
            }
        }
    }
}

/// Estimated tokens of the whole context: the system text plus every item.
pub fn total_tokens(system_chars: usize, items: &[Item]) -> u32 {
    est_tokens(system_chars + items.iter().map(Item::chars).sum::<usize>())
}

pub fn segments(system_chars: usize, notes_chars: usize, items: &[Item]) -> Vec<ContextSegment> {
    let mut out = vec![
        ContextSegment { kind: SegmentKind::System, tokens: est_tokens(system_chars.saturating_sub(notes_chars)) },
        ContextSegment { kind: SegmentKind::Project, tokens: est_tokens(notes_chars) },
    ];
    for kind in [SegmentKind::Summary, SegmentKind::User, SegmentKind::Assistant, SegmentKind::ToolResult] {
        let chars: usize = items.iter().filter(|i| i.kind() == kind).map(Item::chars).sum();
        out.push(ContextSegment { kind, tokens: est_tokens(chars) });
    }
    out
}

/// Index where the verbatim tail starts: whole items from the end while they
/// fit in `budget_chars` (at least the last item), never splitting a
/// Tool item from its turn (items are already whole units here).
pub fn tail_start(items: &[Item], budget_chars: usize) -> usize {
    let mut used = 0;
    let mut start = items.len();
    for (i, item) in items.iter().enumerate().rev() {
        // A tail item's tool result is clipped when rendered.
        let c = item.chars().min(1500);
        if used + c > budget_chars && start < items.len() {
            break;
        }
        used += c;
        start = i;
    }
    // Start the tail at a user message when one is close, so it reads as a turn.
    if let Some(off) = items[start..].iter().position(|i| matches!(i, Item::User { .. })) {
        if off < 4 && start + off < items.len() {
            start += off;
        }
    }
    start
}

/// The text a fresh session carries in its system turn.
pub fn seed_text(summary: Option<&str>, tail: &[Item]) -> String {
    let mut out = String::new();
    if let Some(s) = summary {
        out.push_str("# Earlier in this session (summary)\n");
        out.push_str(s);
        out.push('\n');
    }
    if !tail.is_empty() {
        out.push_str("\n# Most recent turns (verbatim, tool results clipped)\n");
        for item in tail {
            out.push_str(&item.render(600));
        }
    }
    out
}

impl Item {
    pub fn user(text: impl Into<String>) -> Item {
        Item::User { text: text.into(), refs: Vec::new(), context: String::new() }
    }
}

/// One attached file's cap: bytes and lines.
const REF_FILE_BYTES: usize = 24 * 1024;
const REF_FILE_LINES: usize = 400;
/// Entries listed for an attached folder.
const REF_DIR_ENTRIES: usize = 200;

/// The attached files as context for a request, `<file path="..">..</file>`
/// blocks in order: each file capped at 24 KB / 400 lines (with a note of
/// what was left out), a folder as its listing, a missing file as a note;
/// the whole at most `budget` chars (later files become notes when it is
/// spent). Paths are relative to `root`; anything leaving it is refused.
pub fn refs_context(root: &std::path::Path, refs: &[String], budget: usize) -> String {
    let mut out = String::new();
    for rel in refs {
        let block = |body: &str| format!("<file path=\"{rel}\">\n{body}\n</file>\n");
        let path = std::path::Path::new(rel);
        if super::pictures::is_picture_ref(rel) {
            // A picture is shown to the model with the turn, not read as text.
            out.push_str(&block(&match super::pictures::load(rel) {
                Ok(_) => "(a picture: attached to this message for you to look at)".to_string(),
                Err(why) => format!("(a picture that could not be attached: {why})"),
            }));
            continue;
        }
        if path.is_absolute() || path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            out.push_str(&block("(outside the workspace: not attached)"));
            continue;
        }
        let abs = root.join(path);
        let body = if abs.is_dir() {
            let mut names: Vec<String> = std::fs::read_dir(&abs)
                .map(|rd| {
                    rd.flatten()
                        .map(|e| {
                            let name = e.file_name().to_string_lossy().to_string();
                            if e.path().is_dir() { format!("{name}/") } else { name }
                        })
                        .collect()
                })
                .unwrap_or_default();
            names.sort();
            let more = names.len().saturating_sub(REF_DIR_ENTRIES);
            names.truncate(REF_DIR_ENTRIES);
            let mut body = format!("(folder, {} entries)\n{}", names.len() + more, names.join("\n"));
            if more > 0 {
                body.push_str(&format!("\n(and {more} more)"));
            }
            body
        } else {
            match std::fs::read(&abs) {
                Err(_) => "(missing: no such file)".to_string(),
                Ok(bytes) => match String::from_utf8(bytes) {
                    Err(_) => "(binary file: not attached)".to_string(),
                    Ok(text) => clip_file(&text),
                },
            }
        };
        let piece = block(&body);
        if out.len() + piece.len() > budget {
            out.push_str(&block("(not attached: the context budget for attachments is spent; read_file it)"));
            continue;
        }
        out.push_str(&piece);
    }
    out
}

/// A file's text within the per-file caps, with a note of the rest.
fn clip_file(text: &str) -> String {
    let total = text.lines().count();
    let mut out = String::new();
    let mut shown = 0;
    for line in text.lines() {
        if shown >= REF_FILE_LINES || out.len() + line.len() + 1 > REF_FILE_BYTES {
            break;
        }
        out.push_str(line);
        out.push('\n');
        shown += 1;
    }
    if shown < total {
        out.push_str(&format!("(truncated, {} more lines)", total - shown));
    } else if out.ends_with('\n') {
        out.pop();
    }
    out
}

/// What the model is asked to write when the window fills up.
pub const SUMMARY_REQUEST: &str = "Stop using tools. Write a summary of this session so far that you will read \
instead of the full history. Under 250 words, plain text: the user's goal; what is done (files changed, commands \
run, results); what is left; facts you must not forget (paths, names, decisions, errors). No preamble.";

/// A summary made without the model: the old items, clipped. The fallback when
/// the model cannot be asked (a failed route) and the form used by resume.
pub fn mechanical_summary(items: &[Item], budget_chars: usize) -> String {
    let mut out = String::new();
    for item in items {
        out.push_str(&item.render(200));
    }
    super::tools::truncate_middle(&out, budget_chars)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(n: usize) -> Item {
        Item::Tool { name: "read_file".into(), args: "path=a.rs".into(), result: "x".repeat(n), ok: true }
    }

    #[test]
    fn estimates_and_segments() {
        assert_eq!(est_tokens(35), 10);
        let items = vec![Item::user("hi"), tool(700), Item::Summary("s".into())];
        let seg = segments(350, 70, &items);
        assert_eq!(seg[0].tokens, 80); // system without notes
        assert_eq!(seg[1].tokens, 20); // project notes
        assert!(seg.iter().find(|s| s.kind == SegmentKind::ToolResult).unwrap().tokens > 190);
        assert_eq!(total_tokens(350, &items), est_tokens(350 + 2 + 778 + 1));
    }

    #[test]
    fn tail_keeps_recent_whole_items() {
        let items = vec![
            Item::user("a"),
            tool(1000),
            Item::Assistant("b".into()),
            Item::user("c"),
            tool(1000),
            Item::Assistant("d".into()),
        ];
        let s = tail_start(&items, 1200);
        assert!(s >= 3, "{s}");
        assert_eq!(tail_start(&items, 100000), 0);
        // Always at least the last item.
        assert!(tail_start(&items, 1) <= items.len() - 1);
    }

    #[test]
    fn attached_files_are_capped_listed_and_noted() {
        let root = std::env::temp_dir().join(format!("code-refs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("dir/sub")).unwrap();
        std::fs::write(root.join("small.rs"), "fn a() {}\n").unwrap();
        std::fs::write(root.join("big.rs"), (0..1000).map(|i| format!("line {i}\n")).collect::<String>()).unwrap();
        std::fs::write(root.join("dir/x.txt"), "x").unwrap();
        let refs: Vec<String> = ["small.rs", "big.rs", "dir", "gone.rs", "../etc/passwd"].iter().map(|s| s.to_string()).collect();
        let out = refs_context(&root, &refs, 100_000);
        assert!(out.starts_with("<file path=\"small.rs\">\nfn a() {}\n</file>\n"), "{out}");
        assert!(out.contains("line 399\n(truncated, 600 more lines)\n</file>"));
        assert!(!out.contains("line 400"));
        assert!(out.contains("<file path=\"dir\">\n(folder, 2 entries)\nsub/\nx.txt\n</file>"));
        assert!(out.contains("<file path=\"gone.rs\">\n(missing: no such file)"));
        assert!(out.contains("outside the workspace"));
        // A small budget: the first fits, the rest become notes.
        let tight = refs_context(&root, &refs[..2], 200);
        assert!(tight.contains("fn a() {}") && tight.contains("budget for attachments is spent"));
        let item = Item::User { text: "q".into(), refs: refs[..1].to_vec(), context: out.clone() };
        assert_eq!(item.chars(), 1 + out.chars().count());
        assert!(item.render(10).contains("(attached: small.rs)"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn seed_text_has_summary_and_tail() {
        let t = seed_text(Some("did X"), &[Item::user("next")]);
        assert!(t.contains("did X") && t.contains("User: next"));
        assert_eq!(seed_text(None, &[]), "");
    }
}
