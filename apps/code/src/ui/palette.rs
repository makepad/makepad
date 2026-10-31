//! The command palette: a flat list of everything the window can do, ranked
//! by a fuzzy match on what the person types. Pure data; the view runs the
//! chosen `Cmd`.

use makepad_code_agent::event::{kv_label, short_tokens, ApprovalMode, ContextReport, SessionInfo};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Plan,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    NewSession,
    Resume(String),
    Approval(ApprovalMode),
    /// Open the context and KV menu (the status-bar chip opens the same one).
    ContextMenu,
    /// Switch the node to this context size, keeping the conversation.
    SetContext(u32),
    /// Switch the node's attention KV precision, keeping the conversation.
    SetKv(String),
    Toggle(Pane),
    Interrupt,
    CopyLastAnswer,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub title: String,
    pub hint: String,
    pub cmd: Cmd,
}

fn item(title: &str, hint: &str, cmd: Cmd) -> Item {
    Item { title: title.into(), hint: hint.into(), cmd }
}

/// Every command, with the saved sessions as "Resume: <title>" entries.
pub fn items(sessions: &[SessionInfo]) -> Vec<Item> {
    let mut out = vec![
        item("New chat", "Cmd+N", Cmd::NewSession),
        item("Interrupt", "Esc", Cmd::Interrupt),
        item("Copy last answer", "", Cmd::CopyLastAnswer),
        item("Approvals: ask for everything", "", Cmd::Approval(ApprovalMode::Ask)),
        item("Approvals: allowlist (reads and safe commands run)", "", Cmd::Approval(ApprovalMode::Allowlist)),
        item("Approvals: auto (nothing asks)", "", Cmd::Approval(ApprovalMode::Auto)),
        item("Context size...", "", Cmd::ContextMenu),
        item("Toggle plan panel", "", Cmd::Toggle(Pane::Plan)),
    ];
    for s in sessions {
        out.push(Item { title: format!("Resume: {}", s.title), hint: s.when.clone(), cmd: Cmd::Resume(s.id.clone()) });
    }
    out
}

/// The sizes the context menu offers: 16k, 32k, 64k and the largest the node
/// says fits, each with the VRAM the node reports for it, plus KV precision.
pub fn context_items(report: Option<&ContextReport>, current: u32) -> Vec<Item> {
    let mut sizes = vec![16384u32, 32768, 65536];
    let max_fit = report.map_or(0, |r| r.max_fit);
    if max_fit > 0 && !sizes.contains(&max_fit) {
        sizes.push(max_fit);
        sizes.sort_unstable();
    }
    let mut out: Vec<Item> = sizes
        .into_iter()
        .map(|tokens| {
            let option = report.and_then(|r| r.options.iter().find(|o| o.tokens == tokens));
            let mut title = format!("Context {}", short_tokens(tokens));
            if tokens == max_fit {
                title.push_str(" (max that fits)");
            }
            if tokens == current {
                title.push_str(" (current)");
            }
            let hint = match option {
                Some(o) if !o.fits => match o.vram_mb {
                    Some(mb) => format!("KV ~{:.1} GB, does not fit", mb as f32 / 1024.0),
                    None => "does not fit".to_string(),
                },
                Some(o) => o.vram_mb.map_or(String::new(), |mb| format!("KV ~{:.1} GB", mb as f32 / 1024.0)),
                None => String::new(),
            };
            Item { title, hint, cmd: Cmd::SetContext(tokens) }
        })
        .collect();
    for (kv, hint) in [
        ("q8", "half the memory per token, slightly lower quality"),
        ("f16", "reference quality, twice the memory per token"),
    ] {
        let mut title = format!("KV cache: {}", kv_label(kv));
        if report.and_then(|r| r.kv.as_deref()) == Some(kv) {
            title.push_str(" (current)");
        }
        out.push(Item { title, hint: hint.into(), cmd: Cmd::SetKv(kv.into()) });
    }
    out
}

/// Fuzzy score of `query` against `text`: every query character must appear
/// in order (case-insensitive); consecutive runs and word starts score more,
/// gaps cost a little. `None` when it does not match.
pub fn score(query: &str, text: &str) -> Option<i32> {
    let q: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).flat_map(|c| c.to_lowercase()).collect();
    if q.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = text.chars().flat_map(|c| c.to_lowercase()).collect();
    let mut at = 0usize;
    let mut total = 0i32;
    let mut prev: Option<usize> = None;
    for &c in &q {
        let found = (at..t.len()).find(|&i| t[i] == c)?;
        let mut s = 10;
        if prev.is_some_and(|p| p + 1 == found) {
            s += 12;
        }
        if found == 0 || !t[found - 1].is_alphanumeric() {
            s += 8;
        }
        if let Some(p) = prev {
            s -= ((found - p - 1) as i32).min(6);
        } else {
            s -= (found as i32).min(10);
        }
        total += s;
        prev = Some(found);
        at = found + 1;
    }
    Some(total)
}

/// Indexes into `items`, best match first; all of them for an empty query.
pub fn rank(items: &[Item], query: &str) -> Vec<usize> {
    let mut hits: Vec<(i32, usize)> =
        items.iter().enumerate().filter_map(|(i, it)| score(query, &it.title).map(|s| (s, i))).collect();
    if !query.trim().is_empty() {
        hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    }
    hits.into_iter().map(|(_, i)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsequence_matches_and_order_matters() {
        assert!(score("nws", "New session").is_some());
        assert!(score("swn", "New session").is_none());
        assert_eq!(score("", "anything"), Some(0));
    }

    #[test]
    fn tighter_and_word_start_matches_rank_first() {
        let all = items(&[]);
        let top = rank(&all, "auto");
        assert_eq!(all[top[0]].cmd, Cmd::Approval(ApprovalMode::Auto));
        let top = rank(&all, "new");
        assert_eq!(all[top[0]].cmd, Cmd::NewSession);
    }

    #[test]
    fn context_has_one_menu_entry_and_no_restart_option() {
        let all = items(&[]);
        assert_eq!(all.iter().filter(|i| i.cmd == Cmd::ContextMenu).count(), 1);
        assert!(all.iter().all(|i| !i.title.starts_with("Context size:")));
        let top = rank(&all, "context");
        assert_eq!(all[top[0]].cmd, Cmd::ContextMenu);
    }

    #[test]
    fn context_menu_lists_sizes_with_reported_vram() {
        use makepad_code_agent::event::ContextOption;
        let report = ContextReport {
            current: 32768,
            kv: Some("q8".into()),
            max_fit: 49152,
            options: vec![
                ContextOption { tokens: 16384, vram_mb: Some(18_400), fits: true },
                ContextOption { tokens: 32768, vram_mb: Some(19_800), fits: true },
                ContextOption { tokens: 65536, vram_mb: Some(22_000), fits: false },
                ContextOption { tokens: 49152, vram_mb: Some(21_000), fits: true },
            ],
        };
        let menu = context_items(Some(&report), 32768);
        let titles: Vec<&str> = menu.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, [
            "Context 16k", "Context 32k (current)", "Context 48k (max that fits)", "Context 64k",
            "KV cache: 8-bit (q8) (current)", "KV cache: 16-bit (f16)",
        ]);
        assert_eq!(menu[0].hint, "KV ~18.0 GB");
        assert_eq!(menu[3].hint, "KV ~21.5 GB, does not fit");
        assert_eq!(menu[2].cmd, Cmd::SetContext(49152));
        // Before the node reported: the three sizes have no memory estimates.
        let bare = context_items(None, 16384);
        assert_eq!(bare.len(), 5);
        assert!(bare[..3].iter().all(|i| i.hint.is_empty()));
    }

    #[test]
    fn context_menu_lists_kv_choices_and_marks_only_the_reported_precision() {
        for kv in [None, Some("q8"), Some("f16")] {
            let report = kv.map(|kv| ContextReport { kv: Some(kv.into()), ..Default::default() });
            let menu = context_items(report.as_ref(), 16384);
            let choices: Vec<_> = menu.iter().filter(|i| matches!(&i.cmd, Cmd::SetKv(_))).collect();
            assert_eq!(choices.len(), 2);
            assert_eq!(choices[0].cmd, Cmd::SetKv("q8".into()));
            assert_eq!(choices[1].cmd, Cmd::SetKv("f16".into()));
            assert_eq!(choices[0].title, if kv == Some("q8") { "KV cache: 8-bit (q8) (current)" } else { "KV cache: 8-bit (q8)" });
            assert_eq!(choices[1].title, if kv == Some("f16") { "KV cache: 16-bit (f16) (current)" } else { "KV cache: 16-bit (f16)" });
            assert_eq!(choices[0].hint, "half the memory per token, slightly lower quality");
            assert_eq!(choices[1].hint, "reference quality, twice the memory per token");
        }
    }

    #[test]
    fn sessions_become_resume_entries() {
        let s = SessionInfo { id: "abc".into(), title: "fix the parser".into(), when: "today".into(), turns: 3 };
        let all = items(&[s]);
        let top = rank(&all, "parser");
        assert_eq!(all[top[0]].cmd, Cmd::Resume("abc".into()));
        assert_eq!(rank(&all, "").len(), all.len());
    }
}
