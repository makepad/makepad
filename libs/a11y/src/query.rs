//! The one node query grammar, shared by the remote `ui` commands, `makepad_test` selectors and the HAL
//! automation scripts.
//!
//! A query is space-separated terms, all of which must match:
//!
//! | term | matches |
//! |---|---|
//! | `role=button` | the role's short name (`role=button,link` = any of them) |
//! | `name=Save` / `name~=sav` | the name exactly / containing (case-insensitive) |
//! | `value=..` / `value~=..` | the value (or text content) exactly / containing |
//! | `text~=..` | name, value or description containing |
//! | `state=focused,!disabled` | every listed state set (`!` = not set); `enabled` = `!disabled` |
//! | `state=visible` | nonempty bounds and neither hidden nor offscreen |
//! | `action=press` | offers the action |
//! | `id=1a2` / `#1a2` | the node id |
//! | `class=Button` | the widget type |
//! | `path=a.b` | the widget path ends with these segments |
//! | `nth=2` | (selection) the third match in tree order |
//! | bare word | `text~=word` |
//!
//! Values with spaces are quoted: `name="Save as"`.

use crate::mirror::Tree;
use crate::*;

#[derive(Clone, Debug, PartialEq)]
enum Term {
    Roles(Vec<Role>),
    Name(String, bool),
    Value(String, bool),
    Text(String),
    State(States, States),
    Visible(bool),
    Toggle(Toggle, bool),
    Action(Actions),
    Id(NodeId),
    Class(String),
    Path(Vec<String>),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    terms: Vec<Term>,
    pub nth: Option<usize>,
}

/// Splits on spaces, keeping `key="a b"` together.
fn split_terms(s: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for c in s.chars() {
        if escaped { cur.push(c); escaped = false; continue; }
        match c {
            '\\' => escaped = true,
            '"' => quoted = !quoted,
            ' ' | '\t' if !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur))
                }
            }
            c => cur.push(c),
        }
    }
    if quoted || escaped {
        return Err("unterminated quote".into());
    }
    if !cur.is_empty() {
        out.push(cur)
    }
    Ok(out)
}

/// Quote a query value, including literal quotes and backslashes.
pub fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

impl Query {
    pub fn parse(s: &str) -> Result<Query, String> {
        let mut q = Query::default();
        for t in split_terms(s)? {
            if let Some(id) = t.strip_prefix('#') {
                q.terms.push(Term::Id(NodeId::parse_short(id).ok_or_else(|| format!("bad node id `{t}`"))?));
                continue;
            }
            let (key, val, contains) = if let Some((k, v)) = t.split_once("~=") {
                (k, v, true)
            } else if let Some((k, v)) = t.split_once('=') {
                (k, v, false)
            } else {
                q.terms.push(Term::Text(t.to_lowercase()));
                continue;
            };
            let term = match key {
                "role" => Term::Roles(
                    val.split(',')
                        .map(|r| {
                            Role::from_short(r).ok_or_else(|| {
                                let all: Vec<&str> = Role::ALL.iter().map(|r| r.short()).collect();
                                format!("unknown role `{r}`; roles: {}", all.join(" "))
                            })
                        })
                        .collect::<Result<_, _>>()?,
                ),
                "name" => Term::Name(if contains { val.to_lowercase() } else { val.to_string() }, contains),
                "value" => Term::Value(if contains { val.to_lowercase() } else { val.to_string() }, contains),
                "text" => Term::Text(val.to_lowercase()),
                "state" | "states" => {
                    let (mut on, mut off) = (States::empty(), States::empty());
                    for s in val.split(',') {
                        let (neg, s) = match s.strip_prefix('!') {
                            Some(s) => (true, s),
                            None => (false, s),
                        };
                        if s == "visible" {
                            q.terms.push(Term::Visible(!neg));
                            continue;
                        }
                        if matches!(s, "checked" | "mixed") {
                            q.terms.push(Term::Toggle(if s == "checked" { Toggle::On } else { Toggle::Mixed }, neg));
                            continue;
                        }
                        let (flag, neg) = if s == "enabled" { (States::DISABLED, !neg) } else {
                            (States::from_short(s).ok_or_else(|| format!("unknown state `{s}`; states: {}", States(u64::MAX).list().replace(',', " ")))?, neg)
                        };
                        if neg { off.insert(flag) } else { on.insert(flag) }
                    }
                    Term::State(on, off)
                }
                "action" | "actions" => {
                    let mut a = Actions::empty();
                    for s in val.split(',') {
                        a.insert(Actions::from_short(s).ok_or_else(|| format!("unknown action `{s}`"))?);
                    }
                    Term::Action(a)
                }
                "id" => Term::Id(NodeId::parse_short(val).ok_or_else(|| format!("bad node id `{val}`"))?),
                "class" | "type" => Term::Class(val.to_string()),
                "path" => Term::Path(val.split('.').map(str::to_string).collect()),
                "nth" => {
                    q.nth = Some(val.parse().map_err(|_| format!("nth wants a number, got `{val}`"))?);
                    continue;
                }
                _ => return Err(format!("unknown query key `{key}` (role name value text state action id class path nth)")),
            };
            q.terms.push(term);
        }
        Ok(q)
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty() && self.nth.is_none()
    }

    pub fn matches(&self, n: &Node) -> bool {
        self.terms.iter().all(|t| match t {
            Term::Roles(r) => r.contains(&n.role),
            Term::Name(s, true) => n.name.to_lowercase().contains(s),
            Term::Name(s, false) => n.name == *s,
            Term::Value(s, c) => {
                let v = if n.value.is_empty() { n.text.as_ref().map(|t| t.content.as_str()).unwrap_or("") } else { &n.value };
                if *c { v.to_lowercase().contains(s) } else { v == s }
            }
            Term::Text(s) => {
                n.name.to_lowercase().contains(s) || n.value.to_lowercase().contains(s) || n.description.to_lowercase().contains(s)
                    || n.text.as_ref().map(|t| t.content.to_lowercase().contains(s)).unwrap_or(false)
            }
            Term::State(on, off) => (on.is_empty() || n.states.contains(*on)) && !n.states.intersects(*off),
            Term::Visible(expected) => (!n.bounds.is_empty() && !n.states.intersects(States::HIDDEN | States::OFFSCREEN)) == *expected,
            Term::Toggle(toggle, neg) => (n.toggle == *toggle) != *neg,
            Term::Action(a) => n.actions.contains(*a),
            Term::Id(id) => n.id == *id,
            Term::Class(c) => n.class == *c,
            Term::Path(segs) => {
                let p: Vec<&str> = n.path.split('.').collect();
                p.len() >= segs.len() && p[p.len() - segs.len()..].iter().zip(segs).all(|(a, b)| a == b)
            }
        })
    }

    /// Matching nodes in tree order (all of them; `nth` picks one).
    pub fn find<'a>(&self, tree: &'a Tree) -> Vec<&'a Node> {
        let mut out = Vec::new();
        tree.walk(|n, _| {
            if self.matches(n) {
                out.push(n)
            }
        });
        match self.nth {
            Some(i) => out.into_iter().nth(i).into_iter().collect(),
            None => out,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_match() {
        let n = Node {
            id: NodeId::widget(0x1a2),
            role: Role::Button,
            name: "Save as".into(),
            states: States::FOCUSABLE,
            actions: Actions::PRESS,
            path: "main.toolbar.save".into(),
            class: "Button".into(),
            ..Default::default()
        };
        for q in [
            "role=button",
            "role=link,button name=\"Save as\"",
            "name~=save",
            "state=focusable,enabled",
            "action=press",
            "#1a2",
            "id=1a2",
            "path=toolbar.save",
            "class=Button",
            "sav",
        ] {
            assert!(Query::parse(q).unwrap().matches(&n), "{q}");
        }
        for q in ["role=link", "name=Save", "state=focused", "state=!focusable", "path=save.toolbar", "state=disabled"] {
            assert!(!Query::parse(q).unwrap().matches(&n), "{q}");
        }
        assert!(Query::parse("role=nope").is_err());
        assert!(Query::parse("bogus=1").is_err());
        assert_eq!(Query::parse("nth=2").unwrap().nth, Some(2));
    }
}
