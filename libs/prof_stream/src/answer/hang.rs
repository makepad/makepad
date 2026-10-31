//! `why-hung`, `threads`, `waits`, `owner`, `stacks`.

use super::frames::{meaning, Frames, Kind};
use super::{arr, f, pct, s, sig, Answer, Table};
use makepad_strict_json::Value;

fn cap(t: &str) -> String {
    let mut c = t.chars();
    match c.next() {
        Some(x) => x.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

pub fn why_hung(a: &mut Answer) {
    let Some(v) = a.take1("verdict") else { return };
    let found = arr(&v, "findings");
    let mut first = true;
    for x in found {
        let t = a.names.text(s(x, "why"));
        if first {
            a.head = format!("{}: {}", cap(s(x, "kind")), t);
            first = false;
        } else {
            a.line(&format!("- {}: {}", s(x, "kind"), t));
        }
    }
    if first {
        a.head = format!("{}: {}", cap(s(&v, "verdict")), a.names.text(s(&v, "why")));
    }
    if let Some(o) = v.get("owner") {
        let serving = s(o, "serving");
        let q = o.get("queued").map(|q| f(q, "total")).unwrap_or(0.0);
        if serving != "idle" || q > 0.0 {
            a.line(&format!("- owner: serving {}, {} queued", serving, q));
        }
    }
    let threads = a.take("frames");
    let mut ids = Vec::new();
    for t in &threads {
        let fr = Frames::from(arr(t, "frames"));
        let w = s(t, "waits");
        let state = if w.is_empty() { format!("{}, {}% CPU", s(t, "state"), f(t, "cpu_pct") as i64) } else { format!("waits {}", a.names.text(w)) };
        a.line(&format!("\n{} ({}): {}", s(t, "thread"), s(t, "id"), state));
        a.body.push_str(&fr.compact(&mut a.names, "  "));
        ids.push(s(t, "id").to_string());
    }
    if let Some(o) = a.take1("other_threads") {
        let list: Vec<&str> = arr(&o, "other_threads").iter().filter_map(Value::as_str).collect();
        a.line(&format!("\nother threads ({}): {}", list.len(), fold_others(&list)));
    }
    for id in ids.iter().take(2) {
        a.more(format!("stacks thread={}", id));
    }
    if found.iter().any(|x| s(x, "kind") == "parked panic") {
        a.more("panics");
        a.more("locals panic=P<n>");
    }
    a.more("waits");
}

/// "name T3 waiting 0%: __ulock_wait2" lines folded: the busy ones by name, the rest counted by
/// what their top frame means.
fn fold_others(list: &[&str]) -> String {
    let mut busy = Vec::new();
    let mut by: Vec<(String, usize)> = Vec::new();
    for l in list {
        let (who, top) = l.rsplit_once(": ").unwrap_or((l, ""));
        let pct_v: i64 = who.rsplit(' ').next().and_then(|p| p.trim_end_matches('%').parse().ok()).unwrap_or(0);
        if pct_v > 0 {
            busy.push(format!("{} {}%", who.split(' ').next().unwrap_or(""), pct_v));
            continue;
        }
        let m = meaning(top.trim()).map(str::to_string).unwrap_or_else(|| format!("in {}", super::short_fn(top.trim())));
        match by.iter_mut().find(|x| x.0 == m) {
            Some(x) => x.1 += 1,
            None => by.push((m, 1)),
        }
    }
    by.sort_by(|a, b| b.1.cmp(&a.1));
    let mut parts: Vec<String> = by.iter().map(|(m, n)| format!("{} {}", n, m)).collect();
    if !busy.is_empty() {
        parts.insert(0, format!("busy: {}", busy.join(", ")));
    } else {
        parts.push("none busy".into());
    }
    parts.join(", ")
}

fn is_runtime_role(r: &str) -> bool {
    matches!(r, "makepad run" | "compiler" | "inspect" | "owner")
}

pub fn threads(a: &mut Answer) {
    let rows = a.take("role");
    if rows.is_empty() {
        return;
    }
    let total: f64 = rows.iter().map(|r| f(r, "cpu_ms")).sum::<f64>().max(1.0);
    let prog: Vec<&Value> = rows.iter().filter(|r| !is_runtime_role(s(r, "role"))).collect();
    let rt: Vec<&Value> = rows.iter().filter(|r| is_runtime_role(s(r, "role"))).collect();
    let busiest = rows.iter().max_by(|x, y| f(x, "cpu_ms").partial_cmp(&f(y, "cpu_ms")).unwrap_or(std::cmp::Ordering::Equal));
    let waiting: Vec<&&Value> = prog.iter().filter(|r| !s(r, "waits").is_empty()).collect();
    let unnamed = prog.iter().filter(|r| s(r, "role") == "unnamed").count();
    let mut head = format!(
        "{} threads: {} of the program, {} of makepad run{}.",
        rows.len(),
        prog.len() - unnamed,
        rt.len(),
        if unnamed > 0 { format!(", {} unnamed (OS dispatch workers or unnamed program threads: `stacks thread=<id>` tells)", unnamed) } else { String::new() }
    );
    if let Some(b) = busiest {
        head.push_str(&format!(" Most CPU since start: {} ({}) {} s = {} of all thread CPU.", s(b, "name"), s(b, "id"), sig(f(b, "cpu_ms") / 1000.0), pct(f(b, "cpu_ms") * 100.0 / total)));
    }
    if !waiting.is_empty() {
        head.push_str(&format!(" {} program threads wait on a lock or event.", waiting.len()));
    }
    a.head = head;
    let mut t = Table::new(&["id", "name", "role", "state", "CPU s", "waits"]);
    // a pool (3+ threads named `<base>-<n>` / `<base><n>`, none waiting) is one row
    let base = |n: &str| n.trim_end_matches(|c: char| c.is_ascii_digit()).trim_end_matches(['-', '_', ' ']).to_string();
    let mut pools: Vec<(String, Vec<&Value>)> = Vec::new();
    for r in &prog {
        let n = s(r, "name");
        if n.is_empty() || !s(r, "waits").is_empty() || !n.ends_with(|c: char| c.is_ascii_digit()) {
            continue;
        }
        let b = base(n);
        match pools.iter_mut().find(|p| p.0 == b) {
            Some(p) => p.1.push(r),
            None => pools.push((b, vec![r])),
        }
    }
    pools.retain(|p| p.1.len() >= 3);
    let mut done: Vec<String> = Vec::new();
    for r in &prog {
        let n = s(r, "name");
        if let Some(p) = pools.iter().find(|p| p.1.iter().any(|x| s(x, "id") == s(r, "id"))) {
            if done.contains(&p.0) {
                continue;
            }
            done.push(p.0.clone());
            let ids: Vec<&str> = p.1.iter().map(|x| s(x, "id")).collect();
            let running = p.1.iter().filter(|x| s(x, "state") == "running").count();
            t.row(vec![format!("{}..{}", ids[0], ids[ids.len() - 1]), format!("{}* ({} threads)", p.0, ids.len()), s(r, "role").into(), format!("{} running", running), sig(p.1.iter().map(|x| f(x, "cpu_ms")).sum::<f64>() / 1000.0), String::new()]);
            continue;
        }
        let name = if n.is_empty() { "(unnamed)" } else { n };
        t.row(vec![s(r, "id").into(), name.into(), s(r, "role").into(), s(r, "state").into(), sig(f(r, "cpu_ms") / 1000.0), a.names.text(s(r, "waits"))]);
    }
    // makepad run's threads: one row per role, a busy one (5% or more of the CPU) on its own
    let mut roles: Vec<(String, usize, f64, Vec<String>)> = Vec::new();
    for r in &rt {
        let c = f(r, "cpu_ms");
        if c * 100.0 / total >= 5.0 || !s(r, "waits").is_empty() && s(r, "role") == "owner" {
            t.row(vec![s(r, "id").into(), s(r, "name").into(), s(r, "role").into(), s(r, "state").into(), sig(c / 1000.0), a.names.text(s(r, "waits"))]);
            continue;
        }
        let role = s(r, "role").to_string();
        match roles.iter_mut().find(|x| x.0 == role) {
            Some(x) => {
                x.1 += 1;
                x.2 += c;
                x.3.push(s(r, "id").to_string());
            }
            None => roles.push((role, 1, c, vec![s(r, "id").to_string()])),
        }
    }
    for (role, n, c, ids) in &roles {
        let span = if ids.len() > 3 { format!("{}..{}", ids[0], ids[ids.len() - 1]) } else { ids.join(",") };
        t.row(vec![span, format!("{} thread{}", n, if *n == 1 { "" } else { "s" }), role.clone(), String::new(), sig(c / 1000.0), String::new()]);
    }
    a.line(&t.md());
    a.more("stacks thread=<id>");
    a.more("prof s=3 (CPU now)");
    if !waiting.is_empty() {
        a.more("why-hung");
    }
}

pub fn waits(a: &mut Answer) {
    let rows = a.take("thread");
    let real: Vec<&Value> = rows.iter().filter(|r| !s(r, "waits").is_empty()).collect();
    if real.is_empty() {
        a.head = "No thread waits on anything the registry tracks (locks, the owner, queries, joins).".into();
    } else {
        a.head = format!("{} thread{} waiting (wait registry):", real.len(), if real.len() == 1 { "" } else { "s" });
        let mut t = Table::new(&["thread", "waits on"]);
        for r in &real {
            t.row(vec![s(r, "thread").into(), a.names.text(s(r, "waits"))]);
        }
        a.line(&t.md());
    }
    let own: Vec<String> = rows.iter().filter(|r| !s(r, "inspect").is_empty()).map(|r| format!("{}: {}", s(r, "thread"), s(r, "inspect"))).collect();
    if own.len() > 1 {
        a.line(&format!("inspect commands running: {}", own.join("; ")));
    }
    a.more("why-hung");
}

pub fn owner(a: &mut Answer) {
    let Some(v) = a.take1("serving") else { return };
    let q = v.get("queued");
    let total = q.map_or(0.0, |q| f(q, "total"));
    a.head = format!("Owner thread: {}; {} request{} queued.", s(&v, "serving"), total, if total == 1.0 { "" } else { "s" });
    if let Some(Value::Obj(p)) = q {
        let parts: Vec<String> = p.iter().filter(|(k, _)| k != "total").map(|(k, x)| format!("{} {}", k, super::num(Some(x)).unwrap_or(0.0))).collect();
        if total > 0.0 {
            a.line(&format!("queued by class: {}", parts.join(", ")));
        }
    }
}

pub fn stacks(a: &mut Answer) {
    let groups = a.take("frames");
    let picked = a.req.get("thread").or(a.req.pos.first().map(String::as_str)).is_some_and(|t| t != "all");
    let mut idle: Vec<(String, Vec<String>)> = Vec::new();
    let mut shown = 0;
    let n_threads: usize = groups.iter().map(|g| arr(g, "threads").len()).sum();
    for g in &groups {
        let fr = Frames::from(arr(g, "frames"));
        let names: Vec<String> = arr(g, "threads").iter().filter_map(Value::as_str).map(str::to_string).collect();
        // runtime threads with no program code on their stack fold into one line per activity
        if !picked && fr.f.iter().all(|x| x.kind != Kind::Program) {
            let what = fr.doing(&mut a.names);
            match idle.iter_mut().find(|x| x.0 == what) {
                Some(x) => x.1.extend(names),
                None => idle.push((what, names)),
            }
            continue;
        }
        shown += 1;
        let doing = fr.doing(&mut a.names);
        a.line(&format!("\n{} -- {}: {}", names.join(", "), s(g, "state"), doing));
        a.body.push_str(&fr.compact(&mut a.names, "  "));
    }
    a.head = if picked {
        format!("Stack{} of {}:", if n_threads == 1 { "" } else { "s" }, groups.iter().flat_map(|g| arr(g, "threads").iter().filter_map(Value::as_str)).collect::<Vec<_>>().join(", "))
    } else {
        format!("{} threads: {} stack{} with program code below; {} makepad run threads folded.", n_threads, shown, if shown == 1 { "" } else { "s" }, idle.iter().map(|x| x.1.len()).sum::<usize>())
    };
    if !idle.is_empty() {
        a.line("\nmakepad run threads (no program code):");
        for (what, names) in &idle {
            let list = if names.len() > 4 { format!("{}, +{} more", names[..3].join(", "), names.len() - 3) } else { names.join(", ") };
            a.line(&format!("- {}: {}", what, list));
        }
    }
    a.more("stacks thread=<id> depth=24");
    a.more("why-hung");
}
