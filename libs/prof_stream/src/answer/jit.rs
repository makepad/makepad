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

/// `probe read P<n>`: the latest emitted values first, one table per emit (its expression's
/// elements are the columns), then the counts / hists / samples (generic rows).
pub fn probe_read(a: &mut Answer) {
    let emits = a.take("emit");
    let hidden = a.take1("emits_not_shown");
    let totals = a.take1("emitted");
    let id = a.req.pos.get(1).cloned().unwrap_or_default();
    let hits = a.measured.split('(').nth(1).and_then(|x| x.split_whitespace().next()).unwrap_or("?").to_string();
    if emits.is_empty() {
        a.head = format!("{} ran {} times and emitted nothing (std::relax::probe::emit sends values; count / hist / sample aggregate)", id, hits);
    } else {
        a.head = format!("{} ran {} times; its latest {} emitted value{} (oldest first):", id, hits, emits.len(), if emits.len() == 1 { "" } else { "s" });
    }
    // one table per emit line
    let mut lines: Vec<f64> = Vec::new();
    for e in &emits {
        let l = f(e, "line");
        if !lines.contains(&l) {
            lines.push(l);
        }
    }
    for l in lines {
        let rows: Vec<&Value> = emits.iter().filter(|e| f(e, "line") == l).collect();
        let names: Option<Vec<String>> = rows.iter().find_map(|e| match e.get("fields") {
            Some(Value::Obj(kv)) => Some(kv.iter().map(|(k, _)| k.clone()).collect()),
            _ => None,
        });
        if lines_len_gt1(&emits) {
            a.line(&format!("emit at probe line {}:", l));
        }
        let mut head: Vec<String> = vec!["ago s".into()];
        match &names {
            Some(n) => head.extend(n.iter().cloned()),
            None => head.push("value".into()),
        }
        let hr: Vec<&str> = head.iter().map(String::as_str).collect();
        let mut t = super::Table::new(&hr);
        for e in rows {
            let mut cells = vec![super::sig(f(e, "ago_ms") / 1000.0)];
            match (&names, e.get("fields")) {
                (Some(n), Some(Value::Obj(kv))) => {
                    for k in n {
                        cells.push(kv.iter().find(|x| &x.0 == k).map(|x| super::cell_of(&x.1)).unwrap_or_default());
                    }
                }
                _ => {
                    cells.push(s(e, "emit").to_string());
                    for _ in 1..hr.len().saturating_sub(1) {
                        cells.push(String::new());
                    }
                }
            }
            t.row(cells);
        }
        a.line(&t.md());
    }
    if let Some(h) = hidden {
        a.line(&format!("+{} older emitted values kept (n= shows more, up to {})", f(&h, "emits_not_shown"), f(&h, "kept")));
    }
    if let Some(t) = totals {
        if f(&t, "emit_dropped") > 0.0 {
            a.line(&format!("{} emits sent as events, {} not sent (over 20/s; all are kept here)", f(&t, "emitted"), f(&t, "emit_dropped")));
        }
    }
    a.more(format!("probe read {} n=50", id));
    a.more(format!("probe rm {}", id));
}

fn lines_len_gt1(emits: &[Value]) -> bool {
    emits.iter().any(|e| f(e, "line") != emits.first().map_or(0.0, |x| f(x, "line")))
}
