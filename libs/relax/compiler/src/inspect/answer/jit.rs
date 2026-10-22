//! `versions` and `probes`.

use super::{arr, f, s, short_fn, Answer, Table};
use makepad_strict_json::Value;

pub fn versions(a: &mut Answer) {
    let slots = a.take("kind");
    let vers = a.take("version");
    let into = a.take("inlined_into");
    let inl = a.take1("inliners");
    if slots.is_empty() {
        return;
    }
    let mut head = Vec::new();
    for sl in &slots {
        let id = super::num(sl.get("slot")).unwrap_or(-1.0) as i64;
        let mine: Vec<&Value> = vers.iter().filter_map(|v| v.get("version")).filter(|v| super::num(v.get("slot")).map(|x| x as i64) == Some(id)).collect();
        let inst = mine.iter().find(|v| s(v, "state") == "installed");
        let name = short_fn(s(sl, "fn"));
        let mut h = match inst {
            Some(v) => {
                let th: Vec<&str> = arr(v, "threads").iter().filter_map(Value::as_str).collect();
                format!("{} (S{}) runs {} {} code{}", name, id, s(v, "tier"), s(v, "origin"), if th.is_empty() { String::new() } else { format!(" on {}", th.join(", ")) })
            }
            None if !sl.get("compiled").and_then(Value::as_bool).unwrap_or(true) => format!("{} (S{}) is not compiled yet (a lazy stub: compiled on its next call)", name, id),
            None => format!("{} (S{}) has no installed code", name, id),
        };
        let tiers: Vec<String> = mine.iter().map(|v| format!("{} {}", s(v, "tier"), s(v, "state"))).collect();
        h.push_str(&format!("; {} code version{}: {}", mine.len(), if mine.len() == 1 { "" } else { "s" }, tiers.join(", ")));
        if let Some(v) = inst {
            let il: Vec<String> = arr(v, "inlined").iter().filter_map(Value::as_str).map(short_fn).collect();
            if !il.is_empty() {
                h.push_str(&format!("; inlined into its installed code: {}", il.join(", ")));
                if f(v, "inlined_more") > 0.0 {
                    h.push_str(&format!(" +{} more", f(v, "inlined_more")));
                }
            }
        }
        if let Some(p) = sl.get("pinned").and_then(Value::as_str) {
            h.push_str(&format!("; pinned {}", p));
        }
        head.push(h);
        let mut t = Table::new(&["tier", "origin", "state", "code", "threads in it", "inlined fns"]);
        for v in &mine {
            let th: Vec<&str> = arr(v, "threads").iter().filter_map(Value::as_str).collect();
            let il: Vec<String> = arr(v, "inlined").iter().filter_map(Value::as_str).map(short_fn).collect();
            t.row(vec![s(v, "tier").into(), s(v, "origin").into(), s(v, "state").into(), s(v, "code").into(), th.join(", "), il.join(", ")]);
        }
        if slots.len() > 1 {
            a.line(&format!("S{} {}:", id, name));
        }
        a.line(&t.md());
        if let Some(l) = sl.get("line") {
            a.line(&format!("matched line {}", super::num(Some(l)).unwrap_or(0.0)));
        }
        a.more(format!("count S{}", id));
        a.more(format!("tier S{} o1|o3|auto", id));
    }
    a.head = format!("{}.", head.join(". "));
    let n = inl.as_ref().map_or(0.0, |x| f(x, "inliners"));
    if n > 0.0 {
        let list: Vec<String> = into.iter().filter_map(|x| x.get("inlined_into")).map(|v| format!("{} (S{}, {} {})", short_fn(s(v, "fn")), super::num(v.get("slot")).unwrap_or(0.0), s(v, "tier"), s(v, "state"))).collect();
        a.line(&format!("its code is inlined into {} other slot{}: {}", n, if n == 1.0 { "" } else { "s" }, list.join(", ")));
    } else {
        a.line("no other slot holds an inlined copy of it.");
    }
    if let Some(i) = inl {
        let _ = i;
    }
}

pub fn probes(a: &mut Answer) {
    let state = a.take1("paused");
    let rows: Vec<Value> = a.lines.iter().zip(a.used.iter()).filter(|(_, u)| !**u).map(|(l, _)| l.clone()).collect();
    let paused = state.as_ref().and_then(|x| x.get("paused")).and_then(Value::as_bool).unwrap_or(false);
    if rows.is_empty() {
        a.head = format!("No probes, counts or breakpoints installed{}.", if paused { "; the program is paused" } else { "; not paused" });
    } else {
        a.head = format!("{} probe / count / breakpoint record{}{}.", rows.len(), if rows.len() == 1 { "" } else { "s" }, if paused { "; the program is paused" } else { "" });
    }
    if let Some(st) = state {
        if let Some(r) = st.get("rearm").and_then(Value::as_str) {
            a.more(r.to_string());
        }
    }
}
