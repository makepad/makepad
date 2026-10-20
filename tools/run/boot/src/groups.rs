//! Errors as the halted view lists them: root causes first, follow-on errors after, and repeated
//! errors folded into one group (unresolved imports under one module, the same message at many
//! sites). `order` puts the members of a group next to each other, so the host's selection (an
//! index into the list) steps through a group's members in place.
use relax_base::diag::{Diagnostic, Severity, CODES};
use std::ops::Range;

/// Where an error usually sits in a cascade: a parse or macro failure takes items away, which
/// fails resolution, which fails imports and types downstream. Lower ranks come first.
fn rank(d: &Diagnostic) -> u8 {
    if d.severity != Severity::Error {
        return 9;
    }
    let code = d.code.as_str();
    match code.get(..2) {
        Some("RP") => 0,
        Some("RM") => 1,
        _ if code == "RR0001" => 6,
        Some("RR") => 2,
        Some("RT") => 3,
        Some("RB") => 4,
        _ => 5,
    }
}

/// The path of an unresolved import (`unresolved import `a::b::C``).
fn import_path(d: &Diagnostic) -> Option<&str> {
    if d.code != "RR0001" {
        return None;
    }
    let start = d.message.find('`')? + 1;
    let end = start + d.message[start..].find('`')?;
    Some(&d.message[start..end])
}

/// The module an import looks in (its path without the last name).
fn import_module(path: &str) -> &str {
    path.rsplit_once("::").map_or("", |(module, _)| module)
}

/// What folds diagnostics into one group.
#[derive(Clone, PartialEq, Eq)]
enum Key {
    /// unresolved imports under one module (a nested module folds into its parent)
    Imports(String),
    Same(String, String),
}

fn keys(diagnostics: &[Diagnostic]) -> Vec<Key> {
    let modules: Vec<Option<&str>> = diagnostics
        .iter()
        .map(|d| import_path(d).map(import_module))
        .collect();
    diagnostics
        .iter()
        .zip(&modules)
        .map(|(d, module)| match module {
            Some(module) => {
                // the outermost module of the list this one lies under; a bare crate root
                // (`makepad_widgets`, `crate`) folds nothing below it
                let parent = modules
                    .iter()
                    .flatten()
                    .filter(|m| {
                        m.contains("::")
                            && (module == *m
                                || module.strip_prefix(**m).is_some_and(|r| r.starts_with("::")))
                    })
                    .min_by_key(|m| m.len())
                    .copied()
                    .unwrap_or(module);
                Key::Imports(parent.to_string())
            }
            None => Key::Same(d.code.clone(), d.message.clone()),
        })
        .collect()
}

/// Root causes first, follow-on after (stable otherwise); a group's members next to each other,
/// at the place of its first member.
pub fn order(diagnostics: &mut Vec<Diagnostic>) {
    let keys = keys(diagnostics);
    let first: Vec<usize> = keys
        .iter()
        .map(|k| keys.iter().position(|o| o == k).unwrap())
        .collect();
    let group_rank: Vec<u8> = first
        .iter()
        .map(|&f| {
            (0..keys.len())
                .filter(|&i| first[i] == f)
                .map(|i| rank(&diagnostics[i]))
                .min()
                .unwrap()
        })
        .collect();
    let mut index: Vec<usize> = (0..diagnostics.len()).collect();
    index.sort_by_key(|&i| (group_rank[i], first[i], i));
    let mut taken: Vec<Option<Diagnostic>> = std::mem::take(diagnostics).into_iter().map(Some).collect();
    diagnostics.extend(index.into_iter().map(|i| taken[i].take().unwrap()));
}

pub struct Group {
    /// the members, indices into the (ordered) list
    pub members: Range<usize>,
    /// in the first class of the cascade present (shown as a root cause)
    pub root: bool,
    pub code: String,
    /// the code's title (`unresolved import`)
    pub title: &'static str,
    /// one line for the whole group
    pub summary: String,
}

/// The groups of an ordered list (`order`).
pub fn groups(diagnostics: &[Diagnostic]) -> Vec<Group> {
    let keys = keys(diagnostics);
    let mut out: Vec<Group> = Vec::new();
    let mut i = 0;
    while i < diagnostics.len() {
        let mut end = i + 1;
        while end < diagnostics.len() && keys[end] == keys[i] {
            end += 1;
        }
        let d = &diagnostics[i];
        let summary = match &keys[i] {
            Key::Imports(module) if end - i > 1 => {
                let names: Vec<&str> = diagnostics[i..end]
                    .iter()
                    .filter_map(import_path)
                    .map(|p| {
                        p.strip_prefix(module.as_str())
                            .and_then(|r| r.strip_prefix("::"))
                            .unwrap_or(p)
                    })
                    .collect();
                format!("unresolved imports `{module}::{{{}}}`", names.join(", "))
            }
            _ => d.message.clone(),
        };
        let title = CODES
            .iter()
            .find(|(c, _)| *c == d.code)
            .map_or("", |(_, t)| *t);
        out.push(Group {
            members: i..end,
            root: false,
            code: d.code.clone(),
            title,
            summary,
        });
        i = end;
    }
    let best = diagnostics.iter().map(rank).min().unwrap_or(0);
    for g in &mut out {
        g.root = diagnostics[g.members.clone()].iter().map(rank).min() == Some(best);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(phase: &'static str, message: &str) -> Diagnostic {
        Diagnostic::new(phase, "src/main.rs", 1, 1, message)
    }

    #[test]
    fn imports_fold_and_follow_the_root_cause() {
        let mut list = vec![
            d("resolve", "unresolved import `crate::gone::Alpha`"),
            d("resolve", "unresolved import `makepad_widgets::NoSuchWidget`"),
            d("resolve", "unresolved import `crate::gone::Beta`"),
            d("parse", "RP0001 expected `;`"),
            d("resolve", "unresolved import `crate::gone::deeper::Epsilon`"),
        ];
        list[3].code = "RP0001".into();
        order(&mut list);
        let groups = groups(&list);
        assert_eq!(list[0].code, "RP0001");
        assert_eq!(groups.len(), 3);
        assert!(groups[0].root && !groups[1].root && !groups[2].root);
        assert_eq!(groups[1].members, 1..4);
        assert_eq!(groups[1].summary, "unresolved imports `crate::gone::{Alpha, Beta, deeper::Epsilon}`");
        assert_eq!(groups[2].members, 4..5);
    }
}
