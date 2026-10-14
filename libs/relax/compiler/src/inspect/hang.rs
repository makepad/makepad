//! `why-hung`: is the program stuck, and why (DESIGN.md §4.3). One answer from the threads' CPU
//! time over a short probe, the wait registry (who waits for whom), the wait-for graph's cycles and
//! the stacks of the threads involved. Needs no owner (only the names of JIT frames ask it, with a
//! timeout).

use super::{frame_json, json_str, thread_label, Captured, Out, Req};
use crate::waits::{Kind, Wait};
use std::sync::atomic::Ordering::Relaxed;

/// Edges thread -> holder (handles) of the registry's waits whose holder is a thread handle.
fn edges(waits: &[Wait]) -> Vec<(u64, u64)> {
    let owner = crate::owner::owner_tid();
    waits
        .iter()
        .filter_map(|w| match w.kind {
            Kind::None | Kind::Futex => None,
            Kind::Owner => Some((w.thread, owner)),
            _ => (w.holder != 0).then_some((w.thread, w.holder)),
        })
        .filter(|e| e.0 != e.1 && e.1 != 0)
        .collect()
}

/// A cycle in the registry's wait-for graph (thread handles, in order), or empty. Each thread
/// waits for at most one other, so following the chain from each thread finds every cycle.
pub fn registry_cycle(waits: &[Wait]) -> Vec<u64> {
    cycle_in(&edges(waits))
}

fn cycle_in(e: &[(u64, u64)]) -> Vec<u64> {
    for &(start, _) in e {
        let mut path = vec![start];
        let mut cur = start;
        while let Some(&(_, next)) = e.iter().find(|x| x.0 == cur) {
            if let Some(k) = path.iter().position(|&p| p == next) {
                let mut c = path[k..].to_vec();
                // a canonical rotation: the same cycle reads the same every time
                let m = c.iter().enumerate().min_by_key(|x| x.1).map(|x| x.0).unwrap_or(0);
                c.rotate_left(m);
                return c;
            }
            if path.len() > 64 {
                break;
            }
            path.push(next);
            cur = next;
        }
    }
    Vec::new()
}

pub fn why_hung(req: &Req, out: &mut Out) {
    let probe = req.num("probe_ms", 200).clamp(20, 5000);
    let depth = req.num("depth", 12) as usize;
    let before = super::threads::list();
    // the main thread sampled during the probe: where a busy main spends it
    let main_h = crate::jit::MAIN_TID.load(Relaxed);
    let pid = std::process::id() as u64;
    let is_main = move |t: &super::threads::Thread| (t.handle != 0 && t.handle == main_h) || t.os_id == pid;
    let mut main_tops: Vec<Vec<u64>> = Vec::new();
    let t0 = std::time::Instant::now();
    while t0.elapsed().as_millis() < probe as u128 {
        if let Some(c) = super::capture_all(&is_main).pop() {
            main_tops.push(c.frames.iter().copied().take(depth).collect());
        }
        std::thread::sleep(std::time::Duration::from_millis((probe / 10).max(5)));
    }
    let caps = super::capture_all(&|_| true);
    let elapsed_ns = t0.elapsed().as_nanos().max(1) as u64;
    let waits = crate::waits::snapshot();
    let list: Vec<super::threads::Thread> = caps.iter().map(|c| c.t.clone()).collect();
    let cpu = |c: &Captured| -> u64 {
        let b = before.iter().find(|t| t.os_id == c.t.os_id).map_or(c.t.cpu_ns, |t| t.cpu_ns);
        (c.t.cpu_ns.saturating_sub(b)) * 100 / elapsed_ns
    };
    // the graph with mutex holders resolved by their stack pages
    let mut e = edges(&waits);
    for w in waits.iter().filter(|w| w.kind == Kind::Futex && w.holder != 0) {
        if let Some(i) = super::holder_of_tag(w.holder, &caps) {
            if caps[i].t.handle != 0 && caps[i].t.handle != w.thread {
                e.push((w.thread, caps[i].t.handle));
            }
        }
    }
    let cycle = cycle_in(&e);
    let mut partial = Vec::new();
    let all_frames: Vec<Vec<u64>> = caps.iter().map(|c| c.frames.iter().copied().take(depth + 1).collect()).collect();
    let syms = super::symbolise_within(&all_frames, &mut partial, 300);
    let main_i = caps.iter().position(|c| is_main(&c.t));
    let idx_of = |h: u64| caps.iter().position(|c| c.t.handle == h && h != 0);
    let wait_of = |h: u64| waits.iter().find(|w| w.thread == h && w.kind != Kind::None);
    out.measured(&format!("{} threads, CPU over a {} ms probe, wait registry, stacks", caps.len(), probe), "os capture + registry (no owner)", &partial);
    // the chain from main
    let mut chain: Vec<usize> = Vec::new();
    if let Some(m) = main_i {
        chain.push(m);
        let mut cur = caps[m].t.handle;
        while let Some(&(_, next)) = e.iter().find(|x| x.0 == cur) {
            match idx_of(next) {
                Some(i) if !chain.contains(&i) => {
                    chain.push(i);
                    cur = next;
                }
                _ => break,
            }
        }
    }
    let describe = |i: usize| -> String {
        let c = &caps[i];
        let w = wait_of(c.t.handle).map(|w| super::wait_text(w, &list));
        let top = syms[i].iter().find(|f| f.module.starts_with("jit")).or(syms[i].first()).map(|f| f.name.clone()).unwrap_or_default();
        match w {
            Some(w) => format!("{} waits: {}", thread_label(c.t.handle, &list).replace("?", &c.t.name), w),
            None => format!("{} ({}, {}% CPU) in {}", if c.t.name.is_empty() { "thread" } else { &c.t.name }, c.t.state, cpu(c), super::short(&top, 100)),
        }
    };
    let verdict;
    let detail;
    // a panic parked waiting for a decision stops its thread for good: that is the answer, whatever
    // the other threads are doing meanwhile (main idles waiting for the frame the parked thread
    // never finishes)
    let parked = super::events::parked_now();
    if let Some((id, os_id, thread, message, location)) = parked.first() {
        verdict = "parked panic";
        let name = caps.iter().find(|c| c.t.os_id == *os_id).map(|c| c.t.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| thread.clone());
        let more = if parked.len() > 1 { format!(" (+{} more parked)", parked.len() - 1) } else { String::new() };
        detail = format!("P{} parked on thread {} (T{}): {} at {}{} -- see `panics`, decide with `resume P{} panic`", id, name, os_id, super::short(message, 300), location, more, id);
    } else if !cycle.is_empty() {
        let names: Vec<String> = cycle.iter().map(|h| thread_label(*h, &list)).collect();
        verdict = "deadlock";
        detail = format!("wait cycle: {} -> {}", names.join(" -> "), names[0]);
    } else if let Some(m) = main_i {
        let busy = cpu(&caps[m]);
        if chain.len() > 1 {
            verdict = "blocked";
            let steps: Vec<String> = chain.iter().map(|&i| describe(i)).collect();
            detail = steps.join(" -> ");
        } else if wait_of(caps[m].t.handle).is_some() {
            verdict = "blocked";
            detail = describe(m);
        } else if busy >= 50 {
            verdict = "busy";
            // the innermost JIT frame most often seen while main ran
            let mut count: Vec<(String, usize)> = Vec::new();
            let mut p2 = Vec::new();
            let tops = super::symbolise_within(&main_tops, &mut p2, 300);
            for s in &tops {
                if let Some(f) = s.iter().find(|f| f.module.starts_with("jit")).or(s.first()) {
                    let k = if f.at.is_empty() { f.name.clone() } else { format!("{} at {}", f.name, f.at) };
                    match count.iter_mut().find(|x| x.0 == k) {
                        Some(x) => x.1 += 1,
                        None => count.push((k, 1)),
                    }
                }
            }
            count.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            let hot: Vec<String> = count.iter().take(3).map(|(k, n)| format!("{} ({} of {} samples)", super::short(k, 160), n, tops.len())).collect();
            detail = format!("main runs ({}% CPU over {} ms): {}", busy, probe, hot.join("; "));
        } else {
            let top = syms[m].first().map(|f| f.name.clone()).unwrap_or_default();
            verdict = "idle";
            detail = format!("main is not in a JIT-visible wait and uses {}% CPU: waiting for events or sleeping in {}", busy, super::short(&top, 100));
        }
    } else {
        verdict = "unknown";
        detail = "the main thread was not found".into();
    }
    let owner = format!("{{\"serving\":{},\"queued\":{}}}", json_str(&crate::owner::serving_note()), crate::owner::queued_note());
    out.line(&format!("{{\"verdict\":\"{}\",\"why\":{},\"owner\":{}}}", verdict, json_str(&detail), owner));
    // stacks of the threads involved, one line for the others
    let mut involved: Vec<usize> = chain.clone();
    for h in &cycle {
        if let Some(i) = idx_of(*h) {
            if !involved.contains(&i) {
                involved.push(i);
            }
        }
    }
    for (i, c) in caps.iter().enumerate() {
        if !involved.contains(&i) && (cpu(c) >= 50 || parked.iter().any(|p| p.1 == c.t.os_id)) {
            involved.push(i);
        }
    }
    for &i in &involved {
        let c = &caps[i];
        let fr: Vec<String> = syms[i].iter().take(depth).map(frame_json).collect();
        let w = wait_of(c.t.handle).map(|w| format!(",\"waits\":{}", json_str(&super::wait_text(w, &list)))).unwrap_or_default();
        out.line(&format!("{{\"thread\":{},\"id\":\"T{}\",\"state\":\"{}\",\"cpu_pct\":{}{},\"frames\":[{}]}}", json_str(&c.t.name), c.t.os_id, c.t.state, cpu(c), w, fr.join(",")));
    }
    let mut others: Vec<String> = Vec::new();
    for (i, c) in caps.iter().enumerate() {
        if involved.contains(&i) {
            continue;
        }
        let top = syms[i].iter().find(|f| f.module.starts_with("jit")).or(syms[i].first()).map(|f| super::short(&f.name, 60)).unwrap_or_default();
        others.push(format!("{} T{} {} {}%: {}", c.t.name, c.t.os_id, c.t.state, cpu(c), top));
    }
    others.sort();
    let shown: Vec<String> = others.iter().take(40).map(|s| json_str(s)).collect();
    out.line(&format!("{{\"other_threads\":[{}],\"more\":\"stacks thread=<id>\"}}", shown.join(",")));
}
