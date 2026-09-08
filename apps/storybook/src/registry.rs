//! The story registry.
//!
//! A story is a record: where it sits in the navigator (category and
//! component), the name of its DSL template under `mod.stories`, the date it
//! was added, a note in Markdown, and the controls the controls panel may
//! drive on its subject. The records are plain constants in the story files;
//! `all()` walks them in the order the files register, which is the order the
//! navigator shows.
use crate::makepad_widgets::*;

/// The date the catalogue was started. Every story added on or after this
/// date counts as new until the user moves the baseline in the settings.
pub const DEFAULT_BASELINE: &str = "2026-09-05";

pub struct Story {
    /// `category/component/name`, lowercase, unique. The search key, the
    /// settings key, the test key and the remote-op key.
    pub key: &'static str,
    /// The navigator's top-level folder.
    pub category: &'static str,
    /// The navigator's second-level folder, normally the widget's DSL name.
    pub component: &'static str,
    /// Other declarations this page shows besides its own `component`.
    ///
    /// A page routinely renders more than the one widget it is filed under:
    /// the segmented control lives on the button-group page and the chip
    /// group on the chip page. Only the coverage count reads this, and
    /// without it those widgets report as undocumented while standing on
    /// their own catalogue page. It has to be written down rather than
    /// found: a story template's children do not exist until the page is
    /// built, so there is nothing to walk until something builds it.
    pub also: &'static [&'static str],
    /// The row in the navigator.
    pub name: &'static str,
    /// The template's name under `mod.stories`.
    pub dsl: &'static str,
    /// ISO date by which the WIDGET this story documents was in the
    /// library — not the date the story was written. A page written today
    /// about a widget that shipped last year carries the widget's date, so
    /// the NEW marker answers "what was added", never "what was documented".
    /// The two catalogue pages under Overview document no widget and carry a
    /// date before any baseline.
    pub added: &'static str,
    /// Free-form tags the search also matches ("ported", "layout", ...).
    pub tags: &'static [&'static str],
    /// The note shown in the docs panel, in Markdown.
    pub doc: &'static str,
    /// Id of the widget inside the story the docs and controls address;
    /// empty means the story's root.
    pub subject: &'static str,
    /// The cargo feature the story's widget family needs, if any. Such a
    /// story is listed greyed with the feature name when the feature is off.
    pub feature: Option<&'static str>,
    /// Live controls for the subject.
    pub controls: &'static [Control],
    /// A story's own action handler, run every actions pass with the story
    /// root, for demos that react to their own buttons.
    pub on_actions: Option<fn(&mut Cx, &WidgetRef, &Actions)>,
}

pub struct Control {
    pub label: &'static str,
    /// Id path of the widget the control writes to, relative to the story
    /// root; empty means the subject.
    pub target: &'static str,
    pub kind: ControlKind,
}

pub enum ControlKind {
    Bool { prop: &'static str, default: bool },
    Number { prop: &'static str, min: f64, max: f64, step: f64, default: f64 },
    Choice { prop: &'static str, options: &'static [&'static str], default: usize },
    Text { prop: &'static str, default: &'static str },
    Color { prop: &'static str, default: u32 },
    Disabled { default: bool },
}

/// Every story, in navigator order.
pub fn all() -> impl Iterator<Item = &'static Story> {
    crate::stories::tables().iter().flat_map(|table| table.iter())
}

pub fn find(key: &str) -> Option<&'static Story> {
    all().find(|story| story.key == key)
}

/// New means added on or after the baseline. Dates are ISO, so the string
/// order is the date order.
/// True when the widget this story documents arrived on or after the
/// baseline. A new story about an old widget is not new.
pub fn is_new(story: &Story, baseline: &str) -> bool {
    story.added >= baseline
}

/// The story a search query lands on: the first whose key, name, component,
/// category or tags contain the query, case-insensitively.
pub fn matches(story: &Story, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let q = query.to_lowercase();
    story.key.contains(&q)
        || story.name.to_lowercase().contains(&q)
        || story.component.to_lowercase().contains(&q)
        || story.category.to_lowercase().contains(&q)
        || story.tags.iter().any(|t| t.to_lowercase().contains(&q))
}

pub fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            _ => c.is_ascii_digit(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn keys_are_unique_lowercase_and_three_segments() {
        let mut seen = HashSet::new();
        for story in all() {
            assert!(seen.insert(story.key), "duplicate story key {}", story.key);
            assert_eq!(story.key, story.key.to_lowercase(), "{} is not lowercase", story.key);
            assert_eq!(
                story.key.split('/').count(),
                3,
                "{} is not category/component/name",
                story.key
            );
        }
    }

    #[test]
    fn templates_are_unique() {
        let mut seen = HashSet::new();
        for story in all() {
            assert!(seen.insert(story.dsl), "two stories share the template {}", story.dsl);
        }
    }

    #[test]
    fn added_dates_are_iso() {
        for story in all() {
            assert!(is_iso_date(story.added), "{} has a bad date {}", story.key, story.added);
        }
    }

    /// The two tags that make a claim about age must agree with the date,
    /// in both directions: a story tagged "new" is about a widget this
    /// programme added, and one tagged "ported" is about a widget the
    /// library already had, whatever day its page was written.
    #[test]
    fn the_age_tags_agree_with_the_marker() {
        for story in all() {
            if story.tags.contains(&"new") {
                assert!(
                    is_new(story, DEFAULT_BASELINE),
                    "{} is tagged new but its widget predates the baseline",
                    story.key
                );
            }
            if story.tags.contains(&"ported") {
                assert!(
                    !is_new(story, DEFAULT_BASELINE),
                    "{} is tagged ported but is marked new",
                    story.key
                );
            }
        }
    }

    #[test]
    fn baseline_marks_on_or_after() {
        let s = Story {
            key: "a/b/c",
            category: "A",
            component: "B",
            also: &[],
            name: "C",
            dsl: "X",
            added: "2026-09-05",
            tags: &[],
            doc: "",
            subject: "",
            feature: None,
            controls: &[],
            on_actions: None,
        };
        assert!(is_new(&s, "2026-09-05"));
        assert!(is_new(&s, "2026-01-01"));
        assert!(!is_new(&s, "2026-09-06"));
    }
}
