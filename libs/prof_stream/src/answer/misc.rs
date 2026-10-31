//! `help`, events (`panics`, `events`, `watch`), `locals`, `timeline frames|frame`, `hitch`.

use super::frames::{Frames, Kind};
use super::{arr, f, ms, s, short_fn, sig, spark, Answer, Table};
use makepad_strict_json::Value;

/// Which question goes to which command (the start of `help`).
const START: &str = "\
Ask by question (ids printed in answers are accepted by later commands: T<n> thread, S<n> fn slot,
H<n> heap site, P<n> panic, K<n> breakpoint, G<n> probe group, F<n> frame, V<n> Splash VM):
- slow / CPU: `why-slow`, then `prof [threads=T1] [s=5]`
- stuck / hang / deadlock: `why-hung`, `waits`, `stacks thread=T3`
- crash: `panics`, `locals panic=P1`, `resume P1 panic|retry|restart`
- memory: `memory`, then `heap top [by=live|alloc]` (after `heap start`), `heap path H7`, `mem timeline`
- JIT code: `versions <fn|S<n>>`, `tier <fn> o1|o3|auto`, `count <fn>`
- frames / hitches: `timeline start`, `timeline frames`, `frame F12`, `events on=hitch`
- threads: `threads`, `stacks`; Splash VMs: `vms`; GPU memory: `gpu top`
Every command: `as=json` gives the JSON lines (for programs).";

pub fn help(a: &mut Answer) {
    let rows = a.take("cmd");
    let one = a.req.pos.first().is_some();
    if one {
        if let Some(r) = rows.first() {
            a.head = format!("`{} {}`: {}", s(r, "cmd"), s(r, "args"), s(r, "answers"));
        }
        return;
    }
    a.head = format!("makepad inspect: {} commands; `help <cmd>` gives one's arguments and answer.", rows.len());
    a.line(START);
    let names: Vec<&str> = rows.iter().map(|r| s(r, "cmd")).collect();
    a.line(&format!("all commands: {}", names.join(", ")));
}

// ------------------------------------------------------------------ events

pub fn events(a: &mut Answer) {
    let evs = a.take("event");
    if evs.is_empty() {
        if a.cmd == "panics" {
            a.head = "No panic is parked.".into();
            a.more("events on=panic (past panics)");
        } else if a.head.is_empty() {
            a.head = "No events.".into();
        }
        return;
    }
    let hitches: Vec<&Value> = evs.iter().filter(|e| s(e, "event") == "hitch").collect();
    let others: Vec<&Value> = evs.iter().filter(|e| s(e, "event") != "hitch").collect();
    let mut heads = Vec::new();
    for e in &others {
        heads.push(event(a, e));
    }
    if !hitches.is_empty() {
        heads.push(hitch_table(a, &hitches));
    }
    a.head = heads.join(" ");
}

/// One event; its finding returned, its evidence written.
fn event(a: &mut Answer, e: &Value) -> String {
    let kind = s(e, "event");
    let at = super::num(e.get("at_ms")).map(|x| format!(" at {} s", sig(x / 1000.0))).unwrap_or_default();
    match kind {
        "panic" => {
            let id = s(e, "id");
            let fr = Frames::from(arr(e, "frames"));
            let site = fr.first_program().map(|p| format!(" in {} @ {}", short_fn(&p.name), a.names.at(&p.at))).unwrap_or_else(|| format!(" at {}", a.names.at(s(e, "location"))));
            let h = format!("{} panic on thread {}{}: {}{}.", id, s(e, "thread"), at, s(e, "message"), site);
            a.line(&format!("\n{} stack (innermost first):", id));
            a.body.push_str(&fr.compact(&mut a.names, "  "));
            if a.cmd == "panics" || e.get("parked").and_then(Value::as_bool).unwrap_or(false) {
                a.more(format!("locals panic={}", id));
                a.more(format!("resume {} panic|retry|restart|return <v>", id));
            }
            h
        }
        _ => {
            // other kinds: the scalar fields on one line
            let mut parts = Vec::new();
            if let Value::Obj(p) = e {
                for (k, v) in p {
                    if matches!(k.as_str(), "event" | "pid" | "app" | "verified" | "more" | "at_ms") {
                        continue;
                    }
                    match v {
                        Value::Str(x) => parts.push(format!("{}={}", k, a.names.text(x))),
                        Value::Int(_) | Value::F64(_) | Value::Bool(_) => parts.push(format!("{}={}", k, super::cell_of(v))),
                        _ => {}
                    }
                }
            }
            format!("{} event{}: {}.", kind, at, parts.join(" "))
        }
    }
}

fn hitch_table(a: &mut Answer, hs: &[&Value]) -> String {
    let mut v: Vec<&Value> = hs.to_vec();
    v.sort_by(|x, y| f(y, "frame_ms").partial_cmp(&f(x, "frame_ms")).unwrap_or(std::cmp::Ordering::Equal));
    let worst = v[0];
    let top = arr(worst, "time_by").first();
    let comp = worst.get("compile");
    let mut h = format!(
        "{} hitch{} over {} ms; slowest: hitch {}{} took {}",
        hs.len(),
        if hs.len() == 1 { "" } else { "es" },
        f(worst, "threshold_ms"),
        f(worst, "hitch"),
        super::num(worst.get("at_ms")).map(|x| format!(" at {} s", sig(x / 1000.0))).unwrap_or_default(),
        ms(f(worst, "frame_ms"))
    );
    if let Some(t) = top {
        h.push_str(&format!(", mostly {} ({})", s(t, "on"), ms(f(t, "ms"))));
    }
    if let Some(c) = comp {
        if f(c, "ms") > 0.0 {
            h.push_str(&format!("; lazy JIT compiles {} ({} compiles)", ms(f(c, "ms")), f(c, "n")));
        }
    }
    h.push('.');
    let mut t = Table::new(&["hitch", "at s", "frame ms", "compile ms", "compiles", "top 3 by time (ms)", "critical path ms"]);
    for x in &v {
        let by: Vec<String> = arr(x, "time_by").iter().take(3).map(|b| format!("{} {}", s(b, "on"), sig(f(b, "ms")))).collect();
        let cp: f64 = arr(x, "critical_path").iter().map(|p| f(p, "ms")).sum::<f64>() + f(x, "critical_path_other_ms");
        let c = x.get("compile");
        t.row(vec![
            format!("{}", f(x, "hitch")),
            sig(f(x, "at_ms") / 1000.0),
            sig(f(x, "frame_ms")),
            c.map_or(String::new(), |c| sig(f(c, "ms"))),
            c.map_or(String::new(), |c| sig(f(c, "n"))),
            by.join(", "),
            sig(cp),
        ]);
    }
    a.line(&t.md());
    let waits: Vec<String> = arr(worst, "waits").iter().take(3).map(|w| format!("{} {} ({}x, max {})", s(w, "wait"), ms(f(w, "ms")), f(w, "n"), ms(f(w, "max_ms")))).collect();
    if !waits.is_empty() {
        a.line(&format!("slowest hitch's waits: {}", waits.join(", ")));
    }
    let ph: Vec<String> = arr(worst, "phases").iter().take(4).filter_map(|p| p.as_arr()).filter_map(|p| Some(format!("{} {}", p.first()?.as_str()?, ms(super::num(p.get(1))?)))).collect();
    if !ph.is_empty() {
        a.line(&format!("slowest hitch's UI phases: {}", ph.join(", ")));
    }
    if f(worst, "records_dropped") > 0.0 {
        a.line(&format!("note: {} timeline records dropped in that frame (its numbers are lower bounds)", f(worst, "records_dropped")));
    }
    a.more("timeline start; then timeline frames");
    a.more("why-slow");
    h
}

// ------------------------------------------------------------------ locals

/// A Debug value on one line.
fn one_line(v: &str) -> String {
    let mut out = String::new();
    let mut last_space = false;
    for c in v.chars() {
        if c == '\n' || c == ' ' {
            if !last_space && !out.ends_with('[') && !out.ends_with('{') && !out.ends_with('(') {
                out.push(' ');
            }
            last_space = true;
            continue;
        }
        if (c == ']' || c == '}' || c == ')') && out.ends_with(' ') {
            out.pop();
            if out.ends_with(',') {
                out.pop();
            }
        }
        last_space = false;
        out.push(c);
    }
    if out.len() > 240 {
        let k = out.char_indices().map(|(i, _)| i).take_while(|&i| i <= 236).last().unwrap_or(0);
        out.truncate(k);
        out.push_str(" ..");
    }
    out
}

fn why_short(w: &str) -> String {
    if w.starts_with("optimised out") {
        "optimised out".into()
    } else if w.starts_with("not kept") {
        "not kept here".into()
    } else if let Some(r) = w.strip_prefix("in ") {
        format!("in {} at the call, value lost", r.split(' ').next().unwrap_or(""))
    } else if w.contains("shares storage") {
        "slot reused, not known".into()
    } else if w.contains("not Debug") {
        "not Debug (bytes)".into()
    } else {
        w.to_string()
    }
}

pub fn locals(a: &mut Answer) {
    let levels = a.take("locals");
    let frames = a.take("frame");
    let stop = levels.first().map(|l| {
        for k in ["panic", "break", "thread"] {
            let v = s(l, k);
            if !v.is_empty() {
                return v.to_string();
            }
        }
        String::new()
    });
    if levels.is_empty() {
        let _ = frames;
        return;
    }
    // the levels as frames: their kind decides how much is shown
    let mut known_head: Vec<String> = Vec::new();
    let mut known_prog = 0usize;
    let mut folded = Vec::new();
    for l in &levels {
        let fn_name = s(l, "fn");
        let at = s(l, "at");
        let kind = super::frames::kind_of(at, fn_name);
        let ls = arr(l, "locals");
        let with_value: Vec<&Value> = ls.iter().filter(|x| x.get("value").is_some_and(|v| !v.is_null())).collect();
        let tag = format!("#{}{}", f(l, "frame"), if f(l, "inline_level") > 0.0 { format!(".{}", f(l, "inline_level")) } else { String::new() });
        if kind != Kind::Program && with_value.is_empty() {
            folded.push(tag);
            continue;
        }
        if ls.is_empty() {
            folded.push(tag);
            continue;
        }
        let at_s = a.names.at(at);
        a.line(&format!("\n{} {} @ {}{}", tag, short_fn(fn_name), at_s, if l.get("inlined_into").is_some() { " (inlined)" } else { "" }));
        let mut t = Table::new(&["local", "type", "value"]);
        for x in ls {
            let v = x.get("value").and_then(Value::as_str).map(one_line);
            let cellv = match &v {
                Some(v) => v.clone(),
                None => format!("({})", why_short(s(x, "why"))),
            };
            if let Some(v) = &v {
                let item = format!("{} = {}", s(x, "name"), if v.len() > 80 { format!("{}..", &v[..v.char_indices().map(|(i, _)| i).take_while(|&i| i <= 76).last().unwrap_or(0)]) } else { v.clone() });
                // the program's values first, then the nearest std frame's (a bounds check's index and len)
                if kind == Kind::Program {
                    known_head.insert(known_prog, item);
                    known_prog += 1;
                } else if s(x, "type") != "Arguments" {
                    known_head.push(item);
                }
            }
            t.row(vec![s(x, "name").into(), s(x, "type").into(), cellv]);
        }
        a.line(&t.md());
    }
    if !folded.is_empty() {
        a.line(&format!("frames without readable locals (std / runtime / closures): {}", folded.join(" ")));
    }
    let site = levels.iter().find(|l| super::frames::kind_of(s(l, "at"), s(l, "fn")) == Kind::Program && !s(l, "fn").starts_with("(inlined"));
    a.head = format!(
        "Locals at {}{}: {}.",
        stop.unwrap_or_default(),
        site.map(|l| format!(" ({} @ {})", short_fn(s(l, "fn")), a.names.at(s(l, "at")))).unwrap_or_default(),
        if known_head.is_empty() { "no value is known at the stop".to_string() } else { known_head.iter().take(5).cloned().collect::<Vec<_>>().join("; ") }
    );
    a.more("locals ... frame=<n> (one frame)");
    a.more("eval <expr> (in the stop's scope)");
}

// ------------------------------------------------------------------ timeline

pub fn frames(a: &mut Answer) {
    let Some(sum) = a.take1("frame_ms") else { return };
    let rows = a.take("critical_path_ms");
    let st = sum.get("frame_ms").cloned().unwrap_or(Value::Null);
    let series: Vec<f64> = arr(&sum, "series_ms").iter().filter_map(|x| super::num(Some(x))).collect();
    let n = f(&st, "n");
    let slow = rows.first();
    let over = series.iter().filter(|x| **x > 50.0).count();
    a.head = format!(
        "{} frames: median {}, p95 {}, max {}{}; {} over 50 ms.",
        n,
        ms(f(&st, "median")),
        ms(f(&st, "p95")),
        ms(f(&st, "max")),
        slow.map(|r| format!(" ({})", s(r, "frame"))).unwrap_or_default(),
        over
    );
    if !series.is_empty() {
        let lo = series.iter().cloned().fold(f64::MAX, f64::min);
        let hi = series.iter().cloned().fold(f64::MIN, f64::max);
        a.line(&format!("frame ms over time ({}..{} ms, {} frames, oldest left): {}", sig(lo), sig(hi), series.len(), spark(&series, 80)));
    }
    let mut t = Table::new(&["frame", "ms", "UI busy ms", "UI wait ms", "work ms (all threads)", "critical path busy ms", "parallelism", "GPU ms"]);
    for r in rows.iter().take(8) {
        t.row(vec![s(r, "frame").into(), sig(f(r, "ms")), sig(f(r, "ui_busy_ms")), sig(f(r, "ui_wait_ms")), sig(f(r, "work_ms")), sig(f(r, "critical_path_ms")), sig(f(r, "achieved_parallelism")), sig(f(r, "gpu_ms"))]);
    }
    a.line("slowest frames:");
    a.line(&t.md());
    if rows.len() > 8 {
        a.line(&format!("+{} more slow frames not shown (the next is {} ms)", rows.len() - 8, sig(f(&rows[8], "ms"))));
    }
    if let Some(r) = slow {
        a.more(format!("frame {}", s(r, "frame")));
    }
    a.more("timeline save (.mkpf: Scope, `file=<x.mkpf> pf ..`)");
}

pub fn frame(a: &mut Answer) {
    let ph = a.take1("ui_phases_exclusive_ms");
    let cp = a.take1("critical_path_busy_ms");
    let w = a.take1("ui_waits");
    let g = a.take1("gpu");
    let th = a.take1("threads");
    let Some(cp) = cp else { return };
    let frame_ms = th.as_ref().map_or(0.0, |t| f(t, "frame_ms"));
    let steps = arr(&cp, "steps");
    let top = steps.iter().max_by(|x, y| f(x, "ms").partial_cmp(&f(y, "ms")).unwrap_or(std::cmp::Ordering::Equal));
    let title = a.measured.trim_start_matches("frame ").to_string();
    a.head = format!(
        "Frame {}: critical path {} busy{}.",
        title,
        ms(f(&cp, "critical_path_busy_ms")),
        top.map(|t| format!(", longest piece {} on {} ({})", s(t, "what"), s(t, "thread"), ms(f(t, "ms")))).unwrap_or_default()
    );
    if let Some(t) = &th {
        let lanes = arr(t, "threads");
        if !lanes.is_empty() && lanes.iter().any(|l| !s(l, "lane").is_empty()) {
            a.line(&format!("lanes over the frame (0..{}; █ busy, ▄ partly, ░ waiting):", ms(frame_ms)));
            for l in lanes {
                let name = s(l, "name");
                a.line(&format!("{:<5} {:<14} |{}| busy {} wait {}", s(l, "thread"), if name.len() > 14 { &name[..14] } else { name }, s(l, "lane"), ms(f(l, "busy_ms")), ms(f(l, "wait_ms"))));
            }
        }
    }
    if let Some(p) = ph {
        let parts: Vec<String> = arr(&p, "ui_phases_exclusive_ms").iter().filter_map(|x| x.as_arr()).filter_map(|x| Some(format!("{} {}", x.first()?.as_str()?, ms(super::num(x.get(1))?)))).collect();
        if !parts.is_empty() {
            a.line(&format!("UI phases (exclusive): {}", parts.join(", ")));
        }
    }
    if !steps.is_empty() {
        let st: Vec<String> = steps.iter().map(|x| format!("{} {} {}", s(x, "thread"), s(x, "what"), ms(f(x, "ms")))).collect();
        a.line(&format!("critical path, oldest first: {}", st.join(" -> ")));
    }
    if let Some(w) = w {
        let ws: Vec<String> = arr(&w, "ui_waits").iter().map(|x| format!("{} {}{}", s(x, "what"), ms(f(x, "ms")), if s(x, "ended_by").is_empty() { String::new() } else { format!(" (ended by {})", s(x, "ended_by")) })).collect();
        if !ws.is_empty() {
            a.line(&format!("UI waits: {}", ws.join(", ")));
        }
    }
    if let Some(g) = g.and_then(|g| g.get("gpu").cloned()) {
        a.line(&format!("GPU busy {}", ms(f(&g, "busy_ms"))));
    }
}

pub fn hitch(a: &mut Answer) {
    let Some(h) = a.take1("hitch_ms") else { return };
    let t = f(&h, "hitch_ms");
    a.head = if t == 0.0 {
        "Hitch detection is off (`hitch ms=50` turns it on).".into()
    } else {
        format!("{} hitch{} so far (frames over {} ms).", f(&h, "hitches"), if f(&h, "hitches") == 1.0 { "" } else { "es" }, t)
    };
    a.more("events on=hitch");
}
