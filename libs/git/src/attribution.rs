//! Commit trailers that identify an AI coauthor or Studio provenance.

use crate::Commit;

/// Commit trailers that identify an AI coauthor or Studio provenance.
/// `provider` is set only from a recognised `Co-Authored-By` name
/// (Claude/Codex/Grok/Astra); `Studio-Local-Checkpoint: true` is evidence of
/// Studio provenance and is not an AI provider on its own.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AiAttribution {
    pub evidence: Vec<String>,
    pub provider: Option<String>,
}

const AI_PROVIDERS: &[&str] = &["Claude", "Codex", "Grok", "Astra"];

/// Recognised AI / Studio trailers on `commit`. Uses `Commit::trailers()`
/// (final paragraph, `Key: value`, continuation lines) now that libs/git
/// exposes it.
pub fn ai_attribution(commit: &Commit) -> AiAttribution {
    let mut evidence = Vec::new();
    let mut provider = None;
    for (key, value) in commit.trailers() {
        if key.eq_ignore_ascii_case("Co-Authored-By") {
            if let Some(name) = provider_in(&value) {
                evidence.push(format!("{key}: {value}"));
                if provider.is_none() {
                    provider = Some(name);
                }
            }
        } else if key.eq_ignore_ascii_case("Studio-Local-Checkpoint")
            && value.eq_ignore_ascii_case("true")
        {
            evidence.push(format!("{key}: {value}"));
        }
    }
    AiAttribution { evidence, provider }
}

fn provider_in(value: &str) -> Option<String> {
    let lower = value.to_ascii_lowercase();
    for name in AI_PROVIDERS {
        let needle = name.to_ascii_lowercase();
        let mut start = 0;
        while let Some(rel) = lower[start..].find(&needle) {
            let at = start + rel;
            let before_ok = at == 0 || !lower.as_bytes()[at - 1].is_ascii_alphanumeric();
            let end = at + needle.len();
            let after_ok = end == lower.len() || !lower.as_bytes()[end].is_ascii_alphanumeric();
            if before_ok && after_ok {
                return Some(value[at..at + name.len()].to_string());
            }
            start = at + 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ObjectId, Signature};

    fn sig() -> Signature {
        Signature {
            name: "Test".into(),
            email: "test@example.org".into(),
            timestamp: 0,
            tz_offset: "+0000".into(),
        }
    }

    fn commit_message(message: &str) -> Commit {
        Commit {
            tree: ObjectId::ZERO,
            parents: vec![],
            author: sig(),
            committer: sig(),
            message: message.into(),
        }
    }

#[test]
    fn ai_attribution_recognises_coauthored_claude() {
        let a = ai_attribution(&commit_message(
            "Fix the list.\n\nCo-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>\n",
        ));
        assert_eq!(a.provider.as_deref(), Some("Claude"));
        assert!(
            a.evidence
                .iter()
                .any(|e| e.to_ascii_lowercase().contains("co-authored-by")
                    && e.to_ascii_lowercase().contains("claude")),
            "{:?}",
            a.evidence
        );
        for (name, message) in [
            (
                "Codex",
                "msg\n\nCo-Authored-By: Codex <noreply@openai.com>\n",
            ),
            ("Grok", "msg\n\nCo-Authored-By: Grok 4.6 <noreply@x.ai>\n"),
            ("Astra", "msg\n\nco-authored-by: Astra <noreply@x.ai>\n"),
        ] {
            let a = ai_attribution(&commit_message(message));
            assert_eq!(a.provider.as_deref(), Some(name), "{message}");
        }
    }

#[test]
    fn ai_attribution_studio_checkpoint_is_provenance_not_provider() {
        let a = ai_attribution(&commit_message(
            "checkpoint\n\nStudio-Local-Checkpoint: true\n",
        ));
        assert_eq!(a.provider, None);
        assert!(
            a.evidence
                .iter()
                .any(|e| e.contains("Studio-Local-Checkpoint")),
            "{:?}",
            a.evidence
        );
        let both = ai_attribution(&commit_message(
            "both\n\nCo-Authored-By: Claude <noreply@anthropic.com>\nStudio-Local-Checkpoint: true\n",
        ));
        assert_eq!(both.provider.as_deref(), Some("Claude"));
        assert_eq!(both.evidence.len(), 2);
    }

#[test]
    fn ai_attribution_ignores_ordinary_commits() {
        let a = ai_attribution(&commit_message(
            "Just a fix\n\nSigned-off-by: Human <h@example.com>\n",
        ));
        assert!(a.evidence.is_empty(), "{:?}", a.evidence);
        assert_eq!(a.provider, None);
        let none = ai_attribution(&commit_message("No trailers at all\n"));
        assert!(none.evidence.is_empty());
        assert_eq!(none.provider, None);
    }
}
