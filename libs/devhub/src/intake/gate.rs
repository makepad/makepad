//! Mechanical checks on a whitepaper before it crosses the intake boundary.

use super::item::{GateReason, Item};
use super::text;

pub const MAX_BYTES: usize = 6144;
pub const MAX_CODE_LINES: usize = 5;
pub const SECTIONS: [&str; 7] = [
    "1. Item",
    "2. Observed",
    "3. Reference",
    "4. Proposed change",
    "5. Test property",
    "6. Trust notes",
    "7. Not recommended",
];

fn item_line(item: &Item) -> String {
    format!("{} {} {}", item.tag(), item.kind.word(), item.class.word())
}

/// The skeleton the whitepaper agent fills: the title line, then each `## <section>` heading,
/// section 1 already holding the item line.
pub fn skeleton(item: &Item) -> String {
    let mut paper = format!("# Whitepaper {}\n", item.tag());
    for (i, section) in SECTIONS.iter().enumerate() {
        paper.push_str(&format!("\n## {section}\n"));
        if i == 0 {
            paper.push_str(&item_line(item));
            paper.push('\n');
        }
    }
    paper
}

/// The first rule the paper breaks, or GateReason::None.
pub fn shape(paper: &[u8], item: &Item) -> GateReason {
    if paper.iter().all(u8::is_ascii_whitespace) {
        return GateReason::Missing;
    }
    if paper.len() > MAX_BYTES {
        return GateReason::Size;
    }
    if paper.iter().any(|&b| b != b'\n' && !(0x20..=0x7e).contains(&b)) {
        return GateReason::Ascii;
    }
    let paper = std::str::from_utf8(paper).expect("ASCII was checked above");

    // Finish this pass before checking sections to preserve rule precedence.
    let mut in_code = false;
    let mut code_lines = 0;
    for line in paper.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("~~~") {
            return GateReason::CodeBlock;
        }
        if let Some(info) = trimmed.strip_prefix("```") {
            if in_code {
                if line.trim() != "```" {
                    return GateReason::CodeBlock;
                }
            } else if info.len() > 12
                || !info.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            {
                return GateReason::CodeBlock;
            }
            in_code = !in_code;
            code_lines = 0;
        } else if in_code {
            code_lines += 1;
            if code_lines > MAX_CODE_LINES {
                return GateReason::CodeBlock;
            }
        }
    }
    if in_code {
        return GateReason::CodeBlock;
    }

    let mut lines = paper.lines();
    if lines.next() != Some(format!("# Whitepaper {}", item.tag()).as_str()) {
        return GateReason::Sections;
    }
    let expected_item = item_line(item);
    let mut section_count = 0;
    let mut has_content = false;
    for line in lines {
        if !in_code && line.starts_with('#') {
            if section_count == SECTIONS.len()
                || (section_count > 0 && !has_content)
                || line.strip_prefix("## ") != SECTIONS.get(section_count).copied()
            {
                return GateReason::Sections;
            }
            section_count += 1;
            has_content = false;
        } else if section_count > 0 && !line.trim().is_empty() {
            if section_count == 1 && !has_content && line != expected_item {
                return GateReason::Sections;
            }
            has_content = true;
        }
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
        }
    }
    if section_count != SECTIONS.len() || !has_content {
        return GateReason::Sections;
    }

    for line in paper.lines() {
        for token in line.split(|c: char| {
            c.is_ascii_whitespace() || matches!(c, '(' | ')' | '<' | '>' | '[' | ']' | '"' | '\'' | '`')
        }) {
            let token = token.trim_end_matches(['.', ',', ';', ':', '!', '?']);
            if token.contains("://") {
                let Some(rest) = token.strip_prefix("https://github.com/makepad/") else {
                    return GateReason::Url;
                };
                if rest.is_empty()
                    || !rest.bytes().all(|b| b.is_ascii_alphanumeric() || b"._/#-".contains(&b))
                {
                    return GateReason::Url;
                }
            }
            // a host without a scheme (`example.org/x`): its first part has a dot and ends in a
            // name of letters, the way a domain does and a file path's first directory does not
            if !token.contains("://") {
                if let Some((host, _)) = token.split_once('/') {
                    let tld = host.rsplit('.').next().unwrap_or("");
                    if host.contains('.') && (2..=24).contains(&tld.len()) && tld.bytes().all(|b| b.is_ascii_alphabetic()) && !(host.eq_ignore_ascii_case("github.com") && token[host.len()..].starts_with("/makepad/")) {
                        return GateReason::Url;
                    }
                }
            }
            if ["www.", "data:", "javascript:", "file:", "mailto:"].iter().any(|prefix| {
                token.get(..prefix.len()).is_some_and(|start| start.eq_ignore_ascii_case(prefix))
            }) {
                return GateReason::Url;
            }
        }
    }
    if text::scan(paper).iter().any(|signal| signal.high()) {
        return GateReason::Scan;
    }
    GateReason::None
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::intake::item::{Class, ItemKind, PaperModel, Repo, Status};

    pub(crate) fn good_paper() -> (Item, String) {
        let item = Item {
            repo: Repo::Relax,
            number: 12,
            kind: ItemKind::Issue,
            author_id: 0,
            author_login: String::new(),
            updated: 0,
            head_sha: None,
            comments: 0,
            bytes: 0,
            class: Class::Issue,
            status: Status::Ready,
            prefilter: None,
            paper_model: PaperModel::None,
            paper_turns: 0,
            gate: GateReason::None,
            whitepaper_sha256: None,
        };
        let mut paper = skeleton(&item);
        for (section, sentence) in SECTIONS[1..].iter().zip([
            "A const fn panic differs at compile time.",
            "relax and rustc print different values.",
            "Preserve the const evaluation result.",
            "The const fn panic has the same diagnostic.",
            "The reproduction uses a local integer constant.",
            "Changing runtime panic behavior is unrelated.",
        ]) {
            paper = paper.replace(&format!("## {section}\n"), &format!("## {section}\n{sentence}\n"));
        }
        (item, paper)
    }

    #[test]
    fn bytes_and_code_blocks() {
        let (item, good) = good_paper();
        assert_eq!(shape(good.as_bytes(), &item), GateReason::None, "good paper");
        assert_eq!(shape(b"", &item), GateReason::Missing, "empty paper");
        assert_eq!(shape(b" \n\r\t", &item), GateReason::Missing, "whitespace precedes ASCII");
        assert_eq!(shape(&vec![b' '; MAX_BYTES + 1], &item), GateReason::Missing, "missing precedes size");
        let mut oversized = good.clone().into_bytes();
        oversized.resize(MAX_BYTES + 1, b' ');
        assert_eq!(shape(&oversized, &item), GateReason::Size, "one byte over size limit");
        oversized[0] = b'\t';
        assert_eq!(shape(&oversized, &item), GateReason::Size, "size precedes ASCII");
        for suffix in ["\t", "\r", "\u{e9}", "\0"] {
            assert_eq!(shape(format!("{good}{suffix}").as_bytes(), &item), GateReason::Ascii, "invalid byte {suffix:?}");
        }
        for block in [
            "```\na\nb\nc\nd\ne\nf\n```\n",
            "```\nx\n",
            "  ~~~\n",
            "```Rust\nx\n```\n",
            "```rust extra\nx\n```\n",
            "```abcdefghijklm\nx\n```\n",
            "```\nx\n```rust\n",
        ] {
            assert_eq!(shape(format!("{good}{block}").as_bytes(), &item), GateReason::CodeBlock, "invalid fence {block:?}");
        }
        for block in ["```\n```\n", "  ```rust12345678\na\nb\nc\nd\ne\n  ```  \n", "```\n## 8. Extra\n# Title\n```\n"] {
            assert_eq!(shape(format!("{good}{block}").as_bytes(), &item), GateReason::None, "valid fence {block:?}");
        }
        assert_eq!(shape(b"wrong title\n```\n", &item), GateReason::CodeBlock, "code precedes sections");
        assert_eq!(shape(b"\t\n```\n", &item), GateReason::Ascii, "ASCII precedes code");
        let mut exact = good.into_bytes();
        exact.resize(MAX_BYTES, b' ');
        assert_eq!(shape(&exact, &item), GateReason::None, "exact byte limit");
    }

    #[test]
    fn sections_and_identity() {
        let (item, good) = good_paper();
        assert_eq!(shape(skeleton(&item).as_bytes(), &item), GateReason::Sections, "skeleton has empty sections");
        let reordered = good
            .replace("## 2. Observed", "## temporary")
            .replace("## 3. Reference", "## 2. Observed")
            .replace("## temporary", "## 3. Reference");
        assert_eq!(shape(reordered.as_bytes(), &item), GateReason::Sections, "reordered sections");
        for (from, to, property) in [
            ("# Whitepaper relax#12", "# Whitepaper relax#13", "wrong title"),
            ("## 3. Reference\n", "", "missing heading"),
            ("## 3. Reference", "## 2. Observed\n## 3. Reference", "duplicate heading"),
            ("relax#12 issue issue", "relax#13 issue issue", "wrong item"),
            ("relax#12 issue issue", "relax#12 pr issue", "wrong kind"),
            ("relax#12 issue issue", "relax#12 issue c", "wrong class"),
            ("A const fn panic differs at compile time.", " ", "empty middle section"),
            ("Changing runtime panic behavior is unrelated.", " ", "empty last section"),
        ] {
            assert_eq!(shape(good.replacen(from, to, 1).as_bytes(), &item), GateReason::Sections, "{property}");
        }
        assert_eq!(shape(format!("{good}## 8. Extra\nx\n").as_bytes(), &item), GateReason::Sections, "extra heading");
        assert_eq!(shape(good.replace("## 3. Reference", "```\n## 3. Reference\n```").as_bytes(), &item), GateReason::Sections, "fenced heading does not supply a section");
        assert_eq!(shape(good.replace("## 1. Item\n", "## 1. Item\n \n").as_bytes(), &item), GateReason::None, "blank lines before item");
        assert_eq!(shape(format!("{good}# Extra\nhttp://github.com/makepad/x\n").as_bytes(), &item), GateReason::Sections, "sections precede URLs");
    }

    #[test]
    fn urls_and_scan() {
        let (item, good) = good_paper();
        for url in [
            "http://github.com/makepad/x",
            "https://github.com/makepad.example/x",
            "https://gist.github.com/x",
            "https://github.com/makepad/",
            "https://github.com/makepad/x?q=1",
            "https://github.com/makepad/x%20y",
            "www.example.com",
            "WWW.example.com",
            "DATA:x",
            "javascript:x",
            "file:x",
            "MAILTO:x",
        ] {
            assert_eq!(shape(format!("{good}{url}\n").as_bytes(), &item), GateReason::Url, "disallowed URL {url}");
        }
        let url = "https://github.com/makepad/relax/issues/12";
        for token in [url.to_string(), format!("([<\"'`{url}.,;:!?`'\">])"), "https://github.com/makepad/a_B.c-1/#D".into()] {
            assert_eq!(shape(format!("{good}{token}\n").as_bytes(), &item), GateReason::None, "allowed URL and delimiters {token}");
        }
        // Exact high-signal input from text.rs's role_tags rule_pair.
        assert_eq!(shape(format!("{good}<system>\n").as_bytes(), &item), GateReason::Scan, "high role-tag signal");
        assert_eq!(shape(format!("{good}```\n<system>\n```\n").as_bytes(), &item), GateReason::Scan, "code content is scanned");
        assert_eq!(shape(format!("{good}<system>\nwww.example.com\n").as_bytes(), &item), GateReason::Url, "URLs precede scanning");
        assert_eq!(shape(format!("{good}AGENTS.md\n").as_bytes(), &item), GateReason::None, "medium signals pass");
    }
}
