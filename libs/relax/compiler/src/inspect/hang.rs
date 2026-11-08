//! `why-hung`: is the program stuck, and why (DESIGN.md §4.3). One answer from the threads' CPU
//! time over a short probe, the wait registry (who waits for whom), the wait-for graph's cycles and
//! the stacks of the threads involved. Needs no owner (only the names of JIT frames ask it, with a
//! timeout).

use super::{frame_json, json_str, thread_label, Captured, Out, Req};
use crate::waits::{Kind, Wait};
use makepad_cpu_budget::{Tokens, JIT};

/// These diagnostic replies need only string fields, including quoted text in a reason.
fn string_field(line: &str, key: &str) -> Option<String> {
    let prefix = format!("\"{key}\":\"");
    let mut chars = line[line.find(&prefix)? + prefix.len()..].chars();
    let mut value = String::new();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(value),
            '\\' => match chars.next()? {
                c @ ('"' | '\\') => value.push(c),
                c => { value.push('\\'); value.push(c); }
            },
            c => value.push(c),
        }
    }
    None
}

fn wm_pace(req: &Req, out: &mut Out) {
    super::host_query("hal", req, out);
}

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

/// The threads that cannot run again, whatever the others do: what is left once every thread that
/// can run has freed what waits for it. A wait with no known holder (a notify) counts as free; a
/// wait younger than `min_age_ns` too (a row lags its thread: a waiter just woken, one about to give
/// its token up). A token is free when its class has room, when a thread holding one is free (that
/// thread gives it back), and for the JIT's job when no other runs (it goes above the budget).
fn stuck_threads(waits: &[Wait], e: &[(u64, u64)], tokens: &Tokens, min_age_ns: u64, now: u64) -> Vec<u64> {
    let waiting: Vec<&Wait> = waits.iter().filter(|w| w.kind != Kind::None && now.saturating_sub(w.since_ns) >= min_age_ns).collect();
    let mut stuck: Vec<u64> = waiting.iter().map(|w| w.thread).collect();
    loop {
        let still: Vec<u64> = waiting.iter().filter(|w| stuck.contains(&w.thread)).filter(|w| match w.kind {
            Kind::Token => {
                let jit = tokens.waiting.iter().any(|t| t.thread == w.thread && t.client == JIT);
                let held = tokens.holders.iter().filter(|h| !h.lent);
                tokens.running >= tokens.limits[(w.object as usize).min(2)]
                    && (!jit || held.clone().any(|h| h.client == JIT))
                    && held.clone().all(|h| stuck.contains(&h.thread))
            }
            _ => e.iter().find(|x| x.0 == w.thread).is_some_and(|x| stuck.contains(&x.1)),
        }).map(|w| w.thread).collect();
        if still.len() == stuck.len() {
            return stuck;
        }
        stuck = still;
    }
}

/// A deadlock through the CPU budget, as a chain: who waits for what (one group per wait), the
/// threads waiting for a token, and who holds the tokens. None: no stuck thread waits for a token.
fn budget_chain(stuck: &[u64], waits: &[Wait], tokens: &Tokens, list: &[super::threads::Thread]) -> Option<String> {
    let mine: Vec<&Wait> = waits.iter().filter(|w| w.kind != Kind::None && stuck.contains(&w.thread)).collect();
    let label = |t: u64| super::thread_label(t, list);
    let token_waits: Vec<String> = mine.iter().filter(|w| w.kind == Kind::Token).map(|w| {
        format!("{} waits for a {} CPU token ({} of {} running)", label(w.thread), w.label, tokens.running, tokens.limits[(w.object as usize).min(2)])
    }).collect();
    if token_waits.is_empty() {
        return None;
    }
    let mut groups: Vec<(&Wait, Vec<String>)> = Vec::new();
    for w in mine.iter().filter(|w| w.kind != Kind::Token) {
        match groups.iter_mut().find(|g| (g.0.kind, g.0.object, g.0.holder) == (w.kind, w.object, w.holder)) {
            Some(g) => g.1.push(label(w.thread)),
            None => groups.push((w, vec![label(w.thread)])),
        }
    }
    // (from the outside in: requests wait for the owner, the owner for a build)
    groups.sort_by_key(|g| g.0.kind != Kind::Owner);
    let mut steps: Vec<String> = groups.iter().map(|(w, who)| format!("{} waits: {}", who.join(", "), super::wait_text(w, list))).collect();
    steps.push(token_waits.join("; "));
    let holders: Vec<String> = tokens.holders.iter().filter(|h| !h.lent).map(|h| format!("{} {:?}", label(h.thread), h.label)).collect();
    steps.push(format!("the tokens are held by {}", holders.join(", ")));
    Some(steps.join(" -> "))
}

/// (tests) The chain of a deadlock through the CPU budget now, of waits at least `min_age_ns` old.
#[cfg(test)]
pub(crate) fn budget_deadlock(min_age_ns: u64) -> Option<String> {
    let waits = crate::waits::snapshot();
    let tokens = makepad_cpu_budget::tokens();
    let stuck = stuck_threads(&waits, &edges(&waits), &tokens, min_age_ns, crate::clock::wall_ns());
    let mut list = super::threads::list();
    super::name_handles(&mut list);
    budget_chain(&stuck, &waits, &tokens, &list)
}

pub fn why_hung(req: &Req, out: &mut Out) {
    let probe = req.num("probe_ms", 200).clamp(20, 5000);
    let depth = req.num("depth", 12) as usize;
    let before = super::threads::list();
    // the main thread sampled during the probe: where a busy main spends it
    let is_main = |t: &super::threads::Thread| super::is_program_main(t);
    let mut main_tops: Vec<Vec<u64>> = Vec::new();
    // main's JIT waits and the first compiles over the probe: a main that waits on its lazy
    // compiles (they run on the owner) shows little CPU of its own and is not idle
    let (jit_wait0, compiled0) = (crate::postframe::ui_wait_ns(), crate::jit::COMPILED_FNS.load(std::sync::atomic::Ordering::Relaxed));
    let t0 = std::time::Instant::now();
    while t0.elapsed().as_millis() < probe as u128 {
        match super::capture_all(&is_main) {
            Ok(mut c) => {
                if let Some(c) = c.pop() {
                    main_tops.push(c.frames.iter().copied().take(depth).collect());
                }
            }
            Err(e) => return out.err(&e),
        }
        crate::run_threads::sleep(std::time::Duration::from_millis((probe / 10).max(5)));
    }
    let caps = match super::capture_all(&|_| true) {
        Ok(c) => c,
        Err(e) => return out.err(&e),
    };
    let elapsed_ns = t0.elapsed().as_nanos().max(1) as u64;
    let jit_duty = crate::postframe::ui_wait_ns().saturating_sub(jit_wait0) * 100 / elapsed_ns;
    let compiled = crate::jit::COMPILED_FNS.load(std::sync::atomic::Ordering::Relaxed).saturating_sub(compiled0);
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
    let tokens = makepad_cpu_budget::tokens();
    // (a second's age: what a row lags by, and a hang is older)
    let stuck = stuck_threads(&waits, &e, &tokens, 1_000_000_000, crate::clock::wall_ns());
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
        // where: the program's innermost frame and the std call it is in (`Mutex::lock in post_ledger at ..`)
        let fr = &syms[i];
        let prog = fr.iter().position(super::is_program_frame);
        let top = match prog {
            Some(p) => {
                let callee = fr[..p].iter().rev().find(|f| f.module.starts_with("jit") && !f.name.starts_with("(inlined") && !f.name.starts_with("std::sys::") && !f.name.starts_with("std::sync::raw"));
                let at = if fr[p].at.is_empty() { String::new() } else { format!(" at {}", fr[p].at) };
                match callee {
                    Some(c) => format!("{} called from {}{}", c.name, fr[p].name, at),
                    None => format!("{}{}", fr[p].name, at),
                }
            }
            None => fr.iter().find(|f| f.module.starts_with("jit")).or(fr.first()).map(|f| f.name.clone()).unwrap_or_default(),
        };
        match w {
            Some(w) => format!("{} waits: {} in {}", thread_label(c.t.handle, &list).replace("?", &c.t.name), w, super::short(&top, 240)),
            None => format!("{} ({}, {}% CPU) in {}", if c.t.name.is_empty() { "thread" } else { &c.t.name }, c.t.state, cpu(c), super::short(&top, 100)),
        }
    };
    // every finding, the most decisive first: a wait cycle stops its threads for good, a parked
    // panic stops its thread until decided, then what main does; threads waiting long are listed
    // too (a park or condvar wait with no holder is waiting for a notify, not deadlocked)
    let mut findings: Vec<(&'static str, String)> = Vec::new();
    if !cycle.is_empty() {
        let steps: Vec<String> = cycle.iter().map(|h| match idx_of(*h) {
            Some(i) => describe(i),
            None => thread_label(*h, &list),
        }).collect();
        findings.push(("deadlock", format!("wait cycle: {} -> back to the first", steps.join(" -> "))));
    }
    let through_budget = budget_chain(&stuck, &waits, &tokens, &list);
    if let Some(steps) = &through_budget {
        findings.push(("deadlock", steps.clone()));
    }
    // a forked child that has not reached its exec (Relax std's fork for a pre_exec spawn,
    // forkexec.rs): the thread that spawned it waits on the child's CLOEXEC pipe meanwhile
    #[cfg(unix)]
    for f in crate::forkexec::in_flight().iter().filter(|f| f.age_ns >= 2_000_000_000) {
        let by = caps.iter().find(|c| c.t.handle == f.thread).map(|c| c.t.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| thread_label(f.thread, &list));
        let what = match f.state {
            crate::forkexec::STATE_FORKING => "the fork itself has not returned (fork() blocks in the kernel or in the C library's fork handlers)".to_string(),
            crate::forkexec::STATE_UNBUILT => format!("it reached `{}`, which was not built before the fork, and ended (exit 127)", crate::jit::slot_name(f.slot).unwrap_or_default()),
            _ => "it runs its pre_exec closures or blocks in them (a closure waits, or reached a lock another thread held at the fork)".to_string(),
        };
        let pid = if f.pid == 0 { "(no pid yet)".to_string() } else { f.pid.to_string() };
        findings.push(("fork without exec", format!("forked child {} of thread {} has not reached exec after {:.1} s: {} -- the spawn in that thread waits for it", pid, by, f.age_ns as f64 / 1e9, what)));
    }
    // compile jobs paused (an allocator swap, a plain fork, a unit's retirement): the pause waits
    // for every job counted running, named here with how long it has been
    let pauser = crate::jit::paused_for();
    if pauser != 0 {
        let now = crate::clock::wall_ns();
        for (t, since) in crate::jit::job_rows::snapshot().into_iter().filter(|j| j.0 != pauser) {
            findings.push(("paused for a job", format!("compile jobs are paused for thread {}, which waits for the compile job thread {} has run for {:.1} s", thread_label(pauser, &list), thread_label(t, &list), now.saturating_sub(since) as f64 / 1e9)));
        }
    }
    let parked = super::events::parked_now();
    for (id, os_id, thread, message, location) in &parked {
        let name = caps.iter().find(|c| c.t.os_id == *os_id).map(|c| c.t.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| thread.clone());
        findings.push(("parked panic", format!("P{} parked on thread {} ({}): {} at {} -- see `panics`, decide with `resume P{} panic`", id, name, super::tid_label(*os_id), super::short(message, 300), location, id)));
    }
    // a program paused for a report or an inspection: its threads are suspended where they were,
    // so main's frame (0% CPU anywhere) is no wait of its own
    if crate::inspect::instrument::paused() {
        let by = if parked.is_empty() { "by `pause`" } else { "for the parked report above" };
        findings.push(("paused", format!("the program is paused {by}: its threads are suspended where they were -- `resume`, or decide what is parked (`panics`)")));
    }
    // threads held for good at a layout patch: the app runs on without them, a restart is offered
    for line in crate::stale::held_now() {
        findings.push(("held", line));
    }
    // a boot window that has not presented: the app's main is not called before it does
    if let Some((_, note)) = crate::run::boot_window_note() {
        findings.push(("boot window", note));
    }
    if let Some(m) = main_i {
        let busy = cpu(&caps[m]);
        // a short owner request now (a lazy compile's round trip), not a long one
        let owner_round_trip = wait_of(caps[m].t.handle).map_or(true, |w| w.kind == Kind::Owner && crate::clock::wall_ns().saturating_sub(w.since_ns) < 1_000_000_000);
        if cycle.is_empty() && jit_duty >= 50 && owner_round_trip {
            findings.push(("compiling", format!("main is not stuck: it waited on its lazy compiles {}% of the {} ms probe ({} first compiles finished in it; main {}% CPU, the compiles run on the owner) -- see `jit`", jit_duty.min(100), probe, compiled, busy)));
        } else if chain.len() > 1 {
            let steps: Vec<String> = chain.iter().map(|&i| describe(i)).collect();
            findings.push(("blocked", format!("main is blocked: {}", steps.join(" -> "))));
        } else if wait_of(caps[m].t.handle).is_some() {
            findings.push(("blocked", format!("main is blocked: {}", describe(m))));
        } else if busy >= 50 {
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
            findings.push(("busy", format!("main runs ({}% CPU over {} ms): {}", busy, probe, hot.join("; "))));
        } else {
            // A lost frame or wake explains idle before outstanding network work does.
            let loop_reply = super::program_reply("loop").unwrap_or_default();
            for line in loop_reply.lines() {
                let why = string_field(line, "why").unwrap_or_default();
                match string_field(line, "verdict").as_deref() {
                    Some("STUCK") => {
                        let kind = match string_field(line, "kind").as_deref() {
                            Some("frame owed") => "frame owed",
                            Some("undelivered signal") => "undelivered signal",
                            _ => continue,
                        };
                        let mut why = format!("{why} -- see `loop`");
                        if kind == "frame owed" {
                            let (bytes, _) = makepad_remote::cmd::answer(wm_pace, "hal pace");
                            let pace = String::from_utf8_lossy(&bytes);
                            if let Some(first) = pace.lines().next() {
                                let detail = if string_field(first, "verdict").as_deref() == Some("OWED") {
                                    string_field(first, "why").unwrap_or_default()
                                } else { super::short(first, 300) };
                                why.push_str(&format!("; wm: {detail}"));
                            }
                        }
                        findings.push((kind, why));
                    }
                    Some("WAITING") => findings.push(("gpu wait", why)),
                    _ => {}
                }
            }
            // an idle main may still leave the app waiting: an answer its loop never read, a
            // request the server has not answered (the program's `net` ledger)
            let net = super::program_reply("net").unwrap_or_default();
            for l in net.lines().filter(|l| l.starts_with("runtime ")) {
                if l.contains(": STUCK:") {
                    findings.push(("unread network answer", format!("main is idle but the app has not read its network events: {} -- see `net`", super::short(l, 300))));
                } else if l.contains(": WAITING:") {
                    findings.push(("network wait", format!("the app waits for the network: {} -- see `net`", super::short(l, 300))));
                }
            }
            let top = syms[m].first().map(|f| f.name.clone()).unwrap_or_default();
            findings.push(("idle", format!("main is not stuck: {}% CPU, waiting for events or sleeping in {}", busy, super::short(&top, 100))));
        }
    } else {
        findings.push(("unknown", "the main thread was not found".into()));
    }
    let in_cycle = |h: u64| cycle.contains(&h) || through_budget.is_some() && stuck.contains(&h);
    for w in waits.iter().filter(|w| w.kind != Kind::None && !in_cycle(w.thread) && crate::clock::wall_ns().saturating_sub(w.since_ns) >= 5_000_000_000) {
        let Some(i) = idx_of(w.thread) else { continue };
        if Some(i) == main_i {
            continue;
        }
        let no_holder = w.kind == Kind::Futex && super::holder_of_tag(w.holder, &caps).is_none();
        let note = if no_holder && w.label != "mutex" { " (no holder: it waits for a notify -- a channel, condvar or park -- not for a lock)" } else { "" };
        findings.push(("long wait", format!("{}{}", describe(i), note)));
    }
    let (verdict, detail) = findings.first().cloned().unwrap_or(("unknown", String::new()));
    let found: Vec<String> = findings.iter().map(|(k, d)| format!("{{\"kind\":\"{}\",\"why\":{}}}", k, json_str(d))).collect();
    let owner = format!("{{\"serving\":{},\"queued\":{}}}", json_str(&crate::owner::serving_note()), crate::owner::queued_note());
    out.line(&format!("{{\"verdict\":\"{}\",\"why\":{},\"findings\":[{}],\"owner\":{}}}", verdict, json_str(&detail), found.join(","), owner));
    // stacks of the threads involved, one line for the others
    let mut involved: Vec<usize> = chain.clone();
    // the owner too: what every compile waits behind
    if let Some(i) = idx_of(crate::owner::owner_tid()).filter(|i| !involved.contains(i)) {
        involved.push(i);
    }
    let token_waiters = waits.iter().filter(|w| w.kind == Kind::Token && stuck.contains(&w.thread)).map(|w| w.thread);
    for h in cycle.iter().copied().chain(token_waiters) {
        if let Some(i) = idx_of(h) {
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
        let mut w = wait_of(c.t.handle).map(|w| format!(",\"waits\":{}", json_str(&super::wait_text(w, &list)))).unwrap_or_default();
        // a walk cut short within the frames shown: where and why it ends there
        if let Some(stops) = c.stops().filter(|_| c.frames.len() <= depth) {
            w.push_str(&format!(",\"stops\":{}", json_str(&stops)));
        }
        out.line(&format!("{{\"thread\":{},\"id\":\"{}\",\"state\":\"{}\",\"cpu_pct\":{}{},\"frames\":[{}]}}", json_str(&c.t.name), super::tid_label(c.t.os_id), c.t.state, cpu(c), w, fr.join(",")));
    }
    let mut others: Vec<String> = Vec::new();
    for (i, c) in caps.iter().enumerate() {
        if involved.contains(&i) {
            continue;
        }
        let top = syms[i].iter().find(|f| f.module.starts_with("jit")).or(syms[i].first()).map(|f| super::short(&f.name, 60)).unwrap_or_default();
        others.push(format!("{} {} {} {}%: {}", c.t.name, super::tid_label(c.t.os_id), c.t.state, cpu(c), top));
    }
    others.sort();
    let shown: Vec<String> = others.iter().take(40).map(|s| json_str(s)).collect();
    out.line(&format!("{{\"other_threads\":[{}],\"more\":\"stacks thread=<id>\"}}", shown.join(",")));
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_cpu_budget::{Holder, Waiting, APP, BACKGROUND, VISIBLE};

    fn wait(thread: u64, kind: Kind, object: u64, holder: u64, label: &'static str) -> Wait {
        Wait { thread, kind, object, holder, since_ns: 0, label }
    }
    /// `holders` hold the budget's tokens (the first is a compile job), `BUILD` waits for one.
    fn tokens(holders: &[u64], lent: bool) -> Tokens {
        let holder = |(i, &thread): (usize, &u64)| Holder { thread, class: VISIBLE, client: if i == 0 { JIT } else { APP }, label: "job".into(), lent, above: false, held_ns: 0 };
        let waiting = vec![Waiting { thread: BUILD, class: BACKGROUND, client: JIT }];
        Tokens { running: if lent { 0 } else { 2 }, limits: [3, 2, 1], holders: holders.iter().enumerate().map(holder).collect(), waiting }
    }
    const OWNER: u64 = 9;
    const BUILD: u64 = 8;
    const NOW: u64 = 5_000_000_000;

    /// A hang as the registry shows it: jobs wait for a build (one on the owner, which pumps
    /// for it), the build for a token, and those jobs hold all of them. `why-hung` must not say idle.
    #[test]
    fn a_deadlock_through_the_budget_reads_as_a_chain() {
        let waits = [
            wait(1, Kind::Build, 1234, BUILD, "some_fn"),
            wait(2, Kind::Owner, 2, OWNER, "program"),
            wait(OWNER, Kind::Pump, 0, BUILD, "pump"),
            wait(BUILD, Kind::Token, BACKGROUND as u64, 0, "background"),
        ];
        let e = [(1, BUILD), (2, OWNER), (OWNER, BUILD)];
        let stuck = stuck_threads(&waits, &e, &tokens(&[1, 2], false), 1_000_000_000, NOW);
        assert_eq!(stuck.len(), 4, "every thread of the knot: {stuck:?}");
        let chain = budget_chain(&stuck, &waits, &tokens(&[1, 2], false), &[]).expect("it goes through a token");
        for part in ["build #1234 some_fn held by thread 0x8", "waits for a background CPU token (2 of 1 running)", "the tokens are held by thread 0x1 \"job\", thread 0x2 \"job\""] {
            assert!(chain.contains(part), "{part:?} in {chain}");
        }
        // one holder runs: it gives its token back, so nothing is stuck
        let free = stuck_threads(&waits[2..], &e[2..], &tokens(&[1, 2], false), 1_000_000_000, NOW);
        assert!(free.is_empty(), "{free:?}");
        // the holders sleep with their tokens lent: the class has room
        assert!(stuck_threads(&waits, &e, &tokens(&[1, 2], true), 1_000_000_000, NOW).is_empty());
        // no compile job holds a token: the build goes above the budget
        let mut app = tokens(&[1, 2], false);
        app.holders[0].client = APP;
        assert!(stuck_threads(&waits, &e, &app, 1_000_000_000, NOW).is_empty());
        // an app job's wait for a token has no such way
        app.waiting[0].client = APP;
        assert_eq!(stuck_threads(&waits, &e, &app, 1_000_000_000, NOW).len(), 4);
        // a wait for nobody known is no claim, and neither is a row younger than the age
        let row = [wait(1, Kind::Build, 1, 0, "f")];
        assert!(stuck_threads(&row, &[], &tokens(&[], false), 0, NOW).is_empty());
        assert!(stuck_threads(&waits, &e, &tokens(&[1, 2], false), NOW + 1, NOW).is_empty());
    }
}
