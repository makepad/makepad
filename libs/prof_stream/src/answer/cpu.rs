//! `prof` / `top by=time` and `why-slow`.

use super::{arr, f, ms, pct, s, short_fn, Answer, Table};
use makepad_strict_json::Value;

/// A fn row's name as read: an inlined copy names the fn whose code it is in.
fn fn_cell(r: &Value) -> String {
    let into = s(r, "inlined_into");
    if into.is_empty() {
        short_fn(s(r, "fn"))
    } else {
        format!("{} (inlined in {})", short_fn(s(r, "fn")), short_fn(into))
    }
}

fn is_jit(r: &Value) -> bool {
    s(r, "in").starts_with("jit")
}

pub fn prof(a: &mut Answer) {
    let win = a.take1("window_s");
    let secs = win.as_ref().map_or(0.0, |w| f(w, "window_s"));
    let threads = a.take1("threads");
    let act = a.take1("activity_pct");
    let rows = a.take("self_pct");
    let rest = a.take1("fns_not_shown");
    let incl = a.take1("inclusive_pct_over_10");
    let wall = (secs * 1000.0).max(1.0);
    // the hottest program fn: its own rows plus its inlined copies' rows
    let mut phys: Vec<(String, f64, String, String)> = Vec::new();
    for r in rows.iter().filter(|r| is_jit(r)) {
        let into = s(r, "inlined_into");
        let name = if into.is_empty() { s(r, "fn") } else { into }.to_string();
        let slot = if into.is_empty() { super::num(r.get("slot")).map(|x| format!("S{}", x as i64)).unwrap_or_default() } else { String::new() };
        match phys.iter_mut().find(|p| p.0 == name) {
            Some(p) => {
                p.1 += f(r, "self_pct");
                if p.3.is_empty() {
                    p.3 = slot;
                }
            }
            None => phys.push((name, f(r, "self_pct"), s(r, "thread").to_string(), slot)),
        }
    }
    phys.sort_by(|x, y| y.1.partial_cmp(&x.1).unwrap_or(std::cmp::Ordering::Equal));
    let tl: &[Value] = threads.as_ref().map_or(&[], |t| arr(t, "threads"));
    let busiest = tl.first();
    let mut head = String::new();
    if let Some((name, p, th, slot)) = phys.first() {
        head = format!("Hottest program fn: {}{} -- {} of CPU samples (self, incl. its inlined code)", short_fn(name), if slot.is_empty() { String::new() } else { format!(" ({})", slot) }, pct(*p));
        if !th.is_empty() {
            head.push_str(&format!(" on {}", th));
        }
        head.push('.');
    }
    if let Some(b) = busiest {
        head.push_str(&format!(" Busiest thread: {} ({}) {} CPU in {} = {} of a core.", s(b, "thread"), s(b, "id"), ms(f(b, "cpu_ms")), ms(wall), pct(f(b, "cpu_ms") * 100.0 / wall)));
    }
    if head.is_empty() {
        head = "No CPU samples in the window: every picked thread was idle.".into();
    }
    a.head = head.trim().to_string();
    if !tl.is_empty() {
        let mut t = Table::new(&["thread", "id", "CPU ms", "% of a core", "% of samples"]);
        // threads under 1% of a core with no samples are counted, not listed
        let (mut n_small, mut ms_small) = (0.0, 0.0);
        for (i, x) in tl.iter().enumerate() {
            if i > 0 && f(x, "cpu_ms") * 100.0 / wall < 1.0 && f(x, "pct_of_samples") < 1.0 {
                n_small += 1.0;
                ms_small += f(x, "cpu_ms");
                continue;
            }
            let name = if s(x, "thread").is_empty() { "(unnamed)" } else { s(x, "thread") };
            t.row(vec![name.to_string(), s(x, "id").to_string(), super::sig(f(x, "cpu_ms")), pct(f(x, "cpu_ms") * 100.0 / wall), pct(f(x, "pct_of_samples"))]);
        }
        a.line(&t.md());
        if let Some(th) = &threads {
            n_small += f(th, "other_threads");
            ms_small += f(th, "other_cpu_ms");
        }
        if n_small > 0.0 {
            a.line(&format!("+{} more threads under 1% of a core, {} CPU together", n_small, ms(ms_small)));
        }
    }
    if let Some(o) = a.take1("off_cpu").and_then(|x| x.get("off_cpu").cloned()) {
        let st = a.names.text(s(&o, "stack"));
        a.head.push_str(&format!(" Off CPU {}% of the time: {} ({} of {} stack captures).", f(&o, "pct"), meaning_of(&st), f(&o, "seen"), f(&o, "of")));
    }
    if let Some(Value::Obj(p)) = act.as_ref().and_then(|x| x.get("activity_pct")) {
        let parts: Vec<String> = p.iter().map(|(k, v)| format!("{} {}", k, pct(super::num(Some(v)).unwrap_or(0.0)))).collect();
        a.line(&format!("samples by activity: {}", parts.join(", ")));
    }
    if !rows.is_empty() {
        let mut t = Table::new(&["self %", "total %", "fn", "slot", "code", "hottest line", "thread"]);
        for r in &rows {
            let slot = super::num(r.get("slot")).map(|x| format!("S{}", x as i64)).unwrap_or_default();
            let line = s(r, "hottest_line");
            let line = if line.is_empty() { String::new() } else { format!("{} ({}%)", a.names.at(line), f(r, "line_pct") as i64) };
            let code = s(r, "in").trim_start_matches("jit/").to_string();
            t.row(vec![pct(f(r, "self_pct")), pct(f(r, "total_pct")), fn_cell(r), slot, code, line, s(r, "tid").to_string()]);
        }
        a.line(&t.md());
        if let Some(r) = &rest {
            if f(r, "fns_not_shown") > 0.0 {
                a.line(&format!("+{} more fns, {} of samples self together", f(r, "fns_not_shown"), pct(f(r, "not_shown_self_pct"))));
            }
        }
    }
    // inclusive: the program's own fns only (the runtime's frames below main are in every sample)
    if let Some(i) = &incl {
        let prog: Vec<String> = arr(i, "inclusive_pct_over_10")
            .iter()
            .filter_map(|e| {
                let e = e.as_arr()?;
                let p = super::num(e.first())?;
                let n = e.get(1)?.as_str()?;
                let (name, module) = n.split_once(" [").unwrap_or((n, ""));
                (module.starts_with("jit") && super::frames::kind_of("", name) == super::frames::Kind::Program && !name.starts_with("jit ") && !module.contains("inlined")).then(|| format!("{} {}", short_fn(name), pct(p)))
            })
            .collect();
        if !prog.is_empty() {
            a.line(&format!("program fns by inclusive time: {}", prog.join(", ")));
        }
    }
    if let Some(sv) = a.take1("saved") {
        a.line(&format!("folded stacks saved: {}", s(&sv, "saved")));
    }
    if let Some((_, _, _, slot)) = phys.first() {
        if !slot.is_empty() {
            a.more(format!("versions {}", slot));
            a.more(format!("count {} (exact calls; changes code)", slot));
        }
    }
    if let Some(b) = busiest {
        a.more(format!("prof threads={} s=5", s(b, "id")));
        a.more(format!("stacks thread={}", s(b, "id")));
    }
    a.more("prof save=1 (flame graph)");
}

pub fn why_slow(a: &mut Answer) {
    let rows = a.take("finding");
    let list: Vec<String> = rows.iter().map(|r| {
        let t = a.names.text(s(r, "finding"));
        match t.split_once("): ") {
            Some((h, st)) if h.contains("off CPU") => format!("{}): {}", h, meaning_of(st)),
            _ => t,
        }
    }).collect();
    if let Some(first) = list.first() {
        a.head = format!("{}.", first.trim_end_matches('.'));
    }
    for (i, l) in list.iter().enumerate().skip(1) {
        a.line(&format!("{}. {}", i + 1, l));
    }
    a.more("prof threads=main s=5");
    a.more("why-hung");
    a.more("stacks thread=main");
}

/// "__semwait_signal in std::sys::time::sleep from main (..)" -> "sleeping in std::sys::time::sleep from main (..)".
pub fn meaning_of(stack: &str) -> String {
    let (top, rest) = stack.split_once(' ').unwrap_or((stack, ""));
    match super::frames::meaning(top) {
        Some(m) => format!("{} ({}) {}", m, top, rest).trim().to_string(),
        None => stack.to_string(),
    }
}
