//! `memory`, `heap top|diff|path`, `top by=alloc|live`, `vms`, `mem timeline`.

use super::frames::Frames;
use super::{arr, f, mb, pct, s, sig, spark, Answer, Table};
use makepad_strict_json::Value;

pub fn memory(a: &mut Answer) {
    let Some(t) = a.take1("rss_mb") else { return };
    let split = a.take1("split").and_then(|x| x.get("split").cloned());
    let nodes = a.take("node");
    let findings = a.take1("findings");
    let domains = a.take("domain");
    a.take1("freed_by_domain_teardown_bytes");
    let node = |k: &str| nodes.iter().find(|n| s(n, "node") == k).map_or(0.0, |n| f(n, "mb"));
    let sp = |k: &str| split.as_ref().map_or(0.0, |x| f(x, k));
    let rss = f(&t, "rss_mb");
    let in_use = f(&t, "malloc_in_use_mb");
    let held = f(&t, "malloc_held_free_mb");
    let tracked = node("malloc/in use/program (tracked stacks, est.)");
    // who owns the memory: the program's malloc heap, makepad run's arenas and JIT code, the app
    // domains' arenas; resident estimates (an arena's touched pages, not what it reserved)
    let arena_res = sp("compiler_arena_touched_mb") + sp("compiler_arena_large_mb");
    let mut parts: Vec<(String, f64, String)> = vec![
        ("program heap (malloc, in use)".into(), in_use, if tracked > 0.0 { format!("{} tracked by `heap start`", mb(tracked)) } else { "the program's live allocations (`heap start` splits them by stack)".into() }),
        ("malloc: freed, held".into(), held, "freed by the program, kept by malloc".into()),
        ("makepad run: compiler arena".into(), arena_res, format!("touched pages; {} reserved", mb(sp("compiler_arena_committed_mb")))),
        ("makepad run: burst arena".into(), sp("burst_arena_committed_mb"), "committed".into()),
        ("makepad run: JIT code + data".into(), sp("jit_code_mb") + sp("jit_ro_data_mb"), String::new()),
    ];
    for d in &domains {
        let used = f(d, "arena_in_use_bytes") / 1048576.0;
        if used >= 1.0 {
            parts.push((format!("app domain {} arena ({})", f(d, "domain"), s(d, "package")), used, format!("in use; {} committed, {} threads", mb(f(d, "arena_committed_bytes") / 1048576.0), f(d, "threads"))));
        }
    }
    parts.retain(|p| p.1 >= 0.05);
    parts.sort_by(|x, y| y.1.partial_cmp(&x.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut head = format!("RSS {} (peak {}", mb(rss), mb(f(&t, "peak_rss_mb")));
    if f(&t, "footprint_mb") > 0.0 {
        head.push_str(&format!(", footprint {}", mb(f(&t, "footprint_mb"))));
    }
    head.push(')');
    if let Some(d) = t.get("rss_since_last_mb") {
        head.push_str(&format!(", {} since the last `memory`", super::num(Some(d)).map(|x| format!("{}{} MB", if x >= 0.0 { "+" } else { "" }, sig(x))).unwrap_or_default()));
    }
    let prog = in_use + held;
    let runtime = arena_res + sp("burst_arena_committed_mb") + sp("jit_code_mb") + sp("jit_ro_data_mb");
    if let Some(p) = parts.first() {
        head.push_str(&format!(". Largest part: {} {}.", p.0, mb(p.1)));
    }
    head.push_str(&format!(" Program malloc heap {} (in use + held free) vs makepad run's own {} (arenas + JIT code).", mb(prog), mb(runtime)));
    a.head = head;
    let mut tb = Table::new(&["part", "MB", "% of RSS", "note"]);
    for (n, v, note) in &parts {
        tb.row(vec![n.clone(), sig(*v), pct(v * 100.0 / rss.max(1.0)), note.clone()]);
    }
    a.line(&tb.md());
    // the OS view: resident pages by mapping kind (pages may be counted under several kinds)
    let mut os: Vec<(String, f64)> = nodes
        .iter()
        .filter_map(|n| {
            let k = s(n, "node");
            let rest = k.strip_prefix("regions/")?;
            Some((rest.replace("private/", "").replace(" (makepad run JIT code/data, burst arena, mapped files)", " (JIT, arenas, files)"), f(n, "mb")))
        })
        .collect();
    os.sort_by(|x, y| y.1.partial_cmp(&x.1).unwrap_or(std::cmp::Ordering::Equal));
    if !os.is_empty() {
        let shown: Vec<String> = os.iter().take(4).map(|(k, v)| format!("{} {}", k, mb(*v))).collect();
        a.line(&format!("OS mappings, resident (overlapping kinds; can exceed RSS): {}", shown.join(", ")));
    }
    let grew: Vec<String> = nodes.iter().filter(|n| n.get("since_last_mb").is_some() && !s(n, "node").contains("DefaultMallocZone")).take(4).map(|n| format!("{} {}{}", s(n, "node"), if f(n, "since_last_mb") >= 0.0 { "+" } else { "" }, sig(f(n, "since_last_mb")))).collect();
    if !grew.is_empty() {
        a.line(&format!("changed since the last `memory` (MB): {}", grew.join(", ")));
    }
    if let Some(fd) = findings {
        for x in arr(&fd, "findings").iter().filter_map(Value::as_str) {
            if x.starts_with("makepad run's own reservations") || x.contains("/DefaultMallocZone") || x.starts_with("regions") {
                continue;
            }
            a.line(&format!("- {}", x));
        }
    }
    a.more("heap top (program heap by stack)");
    a.more("heap start host=1 (makepad run's own heap)");
    a.more("mem timeline");
    a.more("memory compiler=1");
}

/// Sites by allocation stack: rows of one site (several stacks into the same program line) merged.
pub fn heap_top(a: &mut Answer) {
    let rows = a.take("id");
    let rest = a.take1("not_shown");
    let total = a.take1("live_growth_mb");
    if rows.is_empty() {
        return;
    }
    let diff = rows.iter().any(|r| r.get("live_growth_mb").is_some());
    let by = a.req.get("by").unwrap_or("live").to_string();
    let by = match (a.cmd.as_str(), by.as_str()) {
        ("top", "alloc") => "alloc".to_string(),
        ("top", "churn") => "freed".to_string(),
        ("top", _) => "live".to_string(),
        (_, b) => b.to_string(),
    };
    let secs = rest.as_ref().map_or(0.0, |r| f(r, "window_s"));
    // site -> (ids, live, alloc, freed, growth)
    struct Site {
        name: String,
        at: String,
        ids: Vec<String>,
        live: f64,
        alloc: f64,
        freed: f64,
        growth: f64,
        frames: Vec<Value>,
    }
    let mut sites: Vec<Site> = Vec::new();
    for r in &rows {
        let (name, at) = (s(r, "site").to_string(), s(r, "site_at").to_string());
        let (name, at) = if name.is_empty() {
            let fr = Frames::from(arr(r, "frames"));
            fr.f.first().map(|x| (x.name.clone(), x.at.clone())).unwrap_or_default()
        } else {
            (name, at)
        };
        let key_at = a.names.at(&at);
        let i = match sites.iter().position(|x| x.name == name && a_names_eq(&x.at, &key_at)) {
            Some(i) => i,
            None => {
                sites.push(Site { name, at: key_at, ids: Vec::new(), live: 0.0, alloc: 0.0, freed: 0.0, growth: 0.0, frames: arr(r, "frames").to_vec() });
                sites.len() - 1
            }
        };
        let x = &mut sites[i];
        x.ids.push(s(r, "id").to_string());
        x.live += f(r, "live_mb");
        x.alloc += f(r, "alloc_mb") + f(r, "allocated_mb");
        x.freed += f(r, "freed_mb");
        x.growth += f(r, "live_growth_mb");
    }
    let verdict = |x: &Site| -> String {
        if diff {
            return if x.growth > 0.0 { "grew".into() } else { "shrank".into() };
        }
        if x.live > 0.0 && x.freed < x.alloc * 0.02 {
            if secs > 0.0 { format!("grows ~{} MB/s, never freed", sig(x.live / secs)) } else { "kept, never freed".into() }
        } else if x.alloc > 0.0 && x.live < x.alloc * 0.05 {
            "churn: freed again".into()
        } else {
            "kept in part".into()
        }
    };
    let top = &sites[0];
    let ids = top.ids.join(",");
    a.head = if diff {
        format!("Heap grew {} between the snapshots; most: {} @ {} ({}) +{}.", total.as_ref().map(|t| mb(f(t, "live_growth_mb"))).unwrap_or_default(), super::short_fn(&top.name), top.at, ids, mb(top.growth))
    } else {
        match by.as_str() {
            "alloc" => format!("Most allocated: {} @ {} ({}) {} allocated in {} s, {} live -- {}.", super::short_fn(&top.name), top.at, ids, mb(top.alloc), sig(secs), mb(top.live), verdict(top)),
            "freed" => format!("Most freed (churn): {} @ {} ({}) {} freed in {} s.", super::short_fn(&top.name), top.at, ids, mb(top.freed), sig(secs)),
            _ => format!("Largest live site: {} @ {} ({}) {} live -- {}.", super::short_fn(&top.name), top.at, ids, mb(top.live), verdict(top)),
        }
    };
    let mut t = if diff { Table::new(&["site", "ids", "grew MB", "allocated MB"]) } else { Table::new(&["site", "ids", "live MB", "allocated MB", "freed MB", "verdict"]) };
    for x in &sites {
        let site = format!("{} @ {}", super::short_fn(&x.name), x.at);
        if diff {
            t.row(vec![site, x.ids.join(","), sig(x.growth), sig(x.alloc)]);
        } else {
            t.row(vec![site, x.ids.join(","), sig(x.live), sig(x.alloc), sig(x.freed), verdict(x)]);
        }
    }
    a.line(&t.md());
    if let Some(r) = &rest {
        a.line(&format!(
            "totals over {} s (sampled estimates): {} live, {} allocated, {} stacks{}",
            sig(f(r, "window_s")),
            mb(f(r, "total_live_mb")),
            mb(f(r, "total_alloc_mb")),
            f(r, "sites"),
            if f(r, "not_shown") > 0.0 { format!("; +{} smaller stacks not shown ({} live, {} allocated)", f(r, "not_shown"), mb(f(r, "not_shown_live_mb")), mb(f(r, "not_shown_alloc_mb"))) } else { String::new() }
        ));
    }
    // the top site's stack, folded
    let fr = Frames::from(&top.frames);
    if !fr.f.is_empty() {
        a.line(&format!("stack of {}:", top.ids[0]));
        a.body.push_str(&fr.compact(&mut a.names, "  "));
    }
    a.more(format!("heap path {}", top.ids[0]));
    if !diff {
        a.more(if by == "alloc" { "heap top by=live" } else { "heap top by=alloc" });
        a.more("heap snap a; later: heap diff a now");
    }
}

fn a_names_eq(x: &str, y: &str) -> bool {
    x == y
}

pub fn heap_path(a: &mut Answer) {
    let Some(r) = a.take1("frames") else { return };
    let fr = Frames::from(arr(&r, "frames"));
    a.head = format!("Allocation stack {}: {} live, {} allocated, {} freed.", s(&r, "id"), mb(f(&r, "live_mb")), mb(f(&r, "alloc_mb")), mb(f(&r, "freed_mb")));
    for (i, x) in fr.f.iter().enumerate() {
        let at = if x.at.is_empty() { x.module.clone() } else { a.names.at(&x.at) };
        let tag = if x.inlined { " (inlined)".to_string() } else if let Some(sl) = x.slot { format!(" [S{}]", sl) } else { String::new() };
        a.line(&format!("{}. {} @ {}{}", i, super::short_fn(&x.name), at, tag));
    }
}

pub fn vms(a: &mut Answer) {
    let rows = a.take("vm");
    if rows.is_empty() {
        a.head = "No live Splash VM.".into();
        return;
    }
    let mut v: Vec<&Value> = rows.iter().collect();
    v.sort_by(|x, y| f(y, "objects").partial_cmp(&f(x, "objects")).unwrap_or(std::cmp::Ordering::Equal));
    let b = v[0];
    a.head = format!("{} live Splash VM{}; largest heap: {} ({} on {}) {} objects, {} est.", rows.len(), if rows.len() == 1 { "" } else { "s" }, s(b, "vm"), s(b, "label"), s(b, "thread"), f(b, "objects"), mb(f(b, "heap_bytes_est") / 1048576.0));
    let mut t = Table::new(&["vm", "label", "thread", "objects", "strings", "arrays", "heap MB", "GC runs", "GC ms", "errors", "evals"]);
    for r in &v {
        t.row(vec![s(r, "vm").into(), s(r, "label").into(), s(r, "thread").into(), sig(f(r, "objects")), sig(f(r, "strings")), sig(f(r, "arrays")), sig(f(r, "heap_bytes_est") / 1048576.0), sig(f(r, "gc_runs")), sig(f(r, "gc_ms_total")), sig(f(r, "errors")), sig(f(r, "evals"))]);
    }
    a.line(&t.md());
}

pub fn mem_timeline(a: &mut Answer) {
    let Some(r) = a.take1("rss_mb_at_ms") else { return };
    let pts: Vec<(f64, f64)> = arr(&r, "rss_mb_at_ms").iter().filter_map(|p| p.as_arr()).filter_map(|p| Some((super::num(p.first())?, super::num(p.get(1))?))).collect();
    let peak = a.take1("peak");
    let marks = a.take1("marks");
    if pts.is_empty() {
        a.head = "No resident-size samples yet.".into();
        return;
    }
    let (t0, v0) = pts[0];
    let (t1, v1) = pts[pts.len() - 1];
    let vals: Vec<f64> = pts.iter().map(|p| p.1).collect();
    a.head = format!("RSS {} -> {} over {} s ({:+} MB)", mb(v0), mb(v1), sig((t1 - t0) / 1000.0), sig(v1 - v0));
    if let Some(p) = peak.as_ref().and_then(|p| p.get("peak").cloned()) {
        a.head.push_str(&format!(", peak {} at {} s (after {})", mb(f(&p, "rss_mb")), sig(f(&p, "at_ms") / 1000.0), s(&p, "after")));
    }
    a.head.push('.');
    let lo = vals.iter().cloned().fold(f64::MAX, f64::min);
    let hi = vals.iter().cloned().fold(f64::MIN, f64::max);
    a.line(&format!("RSS {}..{} MB, {}..{} s: {}", sig(lo), sig(hi), sig(t0 / 1000.0), sig(t1 / 1000.0), spark(&vals, 60)));
    if let Some(m) = marks {
        let ms: Vec<String> = arr(&m, "marks").iter().filter_map(|x| x.as_arr()).filter_map(|x| Some(format!("{} s {}", sig(super::num(x.first())? / 1000.0), x.get(1)?.as_str()?))).collect();
        if !ms.is_empty() {
            a.line(&format!("marks: {}", ms.join(", ")));
        }
    }
}

/// `heap start|stop|snap|status`: one line.
pub fn heap_state(a: &mut Answer) {
    if let Some(h) = a.take1("heap") {
        a.head = match s(&h, "heap") {
            "tracking" => format!("Heap tracking on: {}, 1 sample per {} KB allocated, stops by itself after {} s.", s(&h, "of"), f(&h, "rate_bytes") / 1024.0, f(&h, "ttl_s")),
            "stopped" => "Heap tracking stopped; its tables stay for `heap top` / `heap diff` until the next start.".into(),
            other => format!("Heap: {}.", other),
        };
        a.more("heap top");
        a.more("heap snap a; later: heap diff a now");
    } else if let Some(sn) = a.take1("heap_snap") {
        a.head = format!("Heap snapshot `{}` kept ({} stacks); compare: heap diff {} now.", s(&sn, "heap_snap"), f(&sn, "stacks"), s(&sn, "heap_snap"));
    }
}
