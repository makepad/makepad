//! `ab <fn> b=<variant> [a=<variant>]`: a live A/B experiment on one fn in the running app. Each
//! variant is a build of the fn's own slot installed in turn (ABAB... rounds), each round measured
//! the same way (samples with the fn on the stack, per second of wall: its inclusive cost rate),
//! and the answer is noise-aware: per-variant medians, the spread of the rounds, a permutation
//! test over the rounds, a verdict. Variants:
//! - `base`: an O3 build as the JIT makes it (inspect builds are never cached)
//! - `opt:<tok>,<tok>`: an O3 build with these MAKEPAD_RUN_OPT tokens (`-sroa`, `-latedce`, ...:
//!   the passes' own off switches), for this slot only
//! - `inline:<callee>` / `noinline:<callee>`: an O3 build that inlines every site of the callee /
//!   none (tokens `+inl@<slot>` / `-inl@<slot>`, read by the inliner)
//! - `o1`: O1 code (a pin, as `tier <fn> o1`)
//! The rate metric fits workloads that run at a fixed pace (frame-locked UI, playback); a
//! throughput loop that runs flat out keeps the fn's share constant: measure those `per=call`
//! (each round's variant built again with timed entry/exit probes: exact ns per call) or
//! `per=frame`.

use super::{short, Out, Req};
use std::collections::HashMap;

static OPTS: relax_session::sync::Mutex<Option<HashMap<u32, Vec<String>>>> = relax_session::sync::Mutex::new(None);

/// The experiment's extra option tokens for slot `id`'s inspect O3 build.
pub fn opts_for(id: u32) -> Option<Vec<String>> {
    OPTS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&id).cloned())
}

fn set_opts(id: u32, v: Option<Vec<String>>) {
    let _k = crate::zone_alloc::kept();
    let mut g = OPTS.lock().unwrap_or_else(|e| e.into_inner());
    let m = g.get_or_insert_with(HashMap::new);
    match v {
        Some(v) => {
            m.insert(id, v);
        }
        None => {
            m.remove(&id);
        }
    }
}

#[derive(Clone, Debug)]
enum Variant {
    Base,
    Opt(Vec<String>),
    O1,
}

/// Parses a variant; callee names resolve to slots on the owner.
fn parse(v: &str, up: *mut crate::jit::Unit) -> Result<(Variant, String), String> {
    if v == "base" || v == "o3" {
        return Ok((Variant::Base, "O3 as built".into()));
    }
    if v == "o1" {
        return Ok((Variant::O1, "O1 code".into()));
    }
    if let Some(t) = v.strip_prefix("opt:") {
        let toks: Vec<String> = t.split(',').filter(|x| !x.is_empty()).map(|x| x.to_string()).collect();
        if toks.is_empty() {
            return Err("opt: needs tokens, e.g. opt:-sroa,-latedce".into());
        }
        return Ok((Variant::Opt(toks.clone()), format!("O3 with {}", toks.join(","))));
    }
    for (pre, tok, what) in [("inline:", "+inl@", "inlining every site of"), ("noinline:", "-inl@", "inlining no site of")] {
        if let Some(callee) = v.strip_prefix(pre) {
            let q = callee.to_string();
            let ids = crate::owner::call_within(crate::owner::Class::Job, 10_000, move || {
                let u = unsafe { &*up };
                super::instrument::find_all(u, &q, 16).into_iter().map(|x| x.0).collect::<Vec<u32>>()
            })
            .ok_or("the owner did not answer in 10 s")?;
            if ids.is_empty() {
                return Err(format!("no fn matches `{}`", callee));
            }
            return Ok((Variant::Opt(ids.iter().map(|i| format!("{}{}", tok, i)).collect()), format!("O3 {} `{}` ({} slot(s))", what, callee, ids.len())));
        }
    }
    Err(format!("variant `{}`: base | o1 | opt:<tok>,.. | inline:<callee> | noinline:<callee>", v))
}

const PIN_BASE: u32 = 0x4000_0000;

/// Installs variant `v` for slot `id` (O3 builds on a thread of their own, as the tier-up does).
fn install(up: *mut crate::jit::Unit, id: u32, v: &Variant, round: u32) -> Result<(), String> {
    let owner = super::patches::Owner::Pin(PIN_BASE + id);
    // whatever the last round installed goes first: an O1 pin comes off
    let upv = up as usize;
    let _ = crate::owner::call_within(crate::owner::Class::Program, 30_000, move || unsafe { super::patches::remove_owner(upv as *mut crate::jit::Unit, owner) });
    match v {
        Variant::O1 => {
            let r = crate::owner::call_within(crate::owner::Class::Program, 30_000, move || unsafe { super::patches::add(upv as *mut crate::jit::Unit, id, owner, super::patches::Edit::Pin) }).ok_or("the owner did not answer in 30 s")?;
            r.map(|_| ())
        }
        Variant::Base | Variant::Opt(_) => {
            set_opts(id, match v {
                Variant::Opt(t) => Some(t.clone()),
                _ => None,
            });
            let (tx, rx) = std::sync::mpsc::channel::<bool>();
            let spawned = {
                let _k = crate::zone_alloc::kept();
                std::thread::Builder::new().name(format!("run-inspect-ab{}", round)).spawn(move || {
                    super::heap::exclude_this_thread();
                    let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { crate::jit::compile_tier1_inspect(upv as *mut crate::jit::Unit, id, None) })).unwrap_or(false);
                    let _ = tx.send(ok);
                })
            };
            if spawned.is_err() {
                set_opts(id, None);
                return Err("cannot start the build thread".into());
            }
            let ok = rx.recv_timeout(std::time::Duration::from_secs(120)).unwrap_or(false);
            set_opts(id, None);
            if ok {
                Ok(())
            } else {
                Err("the O3 build was not installed (O3 off, not verified, or failed: MAKEPAD_RUN_TIER1_TRACE=1 says why)".into())
            }
        }
    }
}

/// The installed code range of slot `id` and its tier label.
fn code_of(up: *mut crate::jit::Unit, id: u32) -> Option<(u64, u64, String)> {
    let upv = up as usize;
    crate::owner::call_within(crate::owner::Class::Job, 10_000, move || {
        let u = unsafe { &*(upv as *const crate::jit::Unit) };
        let f = &u.fns[id as usize];
        (f.compiled && f.code_len > 0).then(|| (f.code, f.code + f.code_len as u64, super::codever::label(super::codever::kind_at(u, f.code))))
    })
    .flatten()
}

/// One round: weighted samples with a pc in [lo, hi) anywhere on the stack (incl) or innermost
/// (self), per second.
fn measure(lo: u64, hi: u64, secs: f64, hz: u64, ev: super::prof::Ev, self_only: bool, per_frame: bool, out: &Out) -> (f64, f64) {
    let gone = out.gone();
    let f0 = super::timeline::FRAMES.load(std::sync::atomic::Ordering::Relaxed);
    let run = super::prof::collect_ev(secs, hz, ev, &|_| true, &*gone);
    let frames = super::timeline::FRAMES.load(std::sync::atomic::Ordering::Relaxed).saturating_sub(f0);
    let mut w = 0u64;
    for s in &run.samples {
        let hit = if self_only { s.frames.first().is_some_and(|&pc| pc >= lo && pc < hi) } else { s.frames.iter().enumerate().any(|(i, &pc)| { let p = if i == 0 { pc } else { pc.wrapping_sub(1) }; p >= lo && p < hi }) };
        if hit {
            w += s.weight.max(1);
        }
    }
    // per second of wall, or per UI frame
    let per = if per_frame { w as f64 / frames.max(1) as f64 } else { w as f64 / run.secs.max(0.01) };
    // CPU samples: ms of CPU
    let v = if ev == super::prof::Ev::Cpu { per * 1000.0 / hz as f64 } else { per };
    (v, run.secs)
}

fn median(v: &[f64]) -> f64 {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    if s.is_empty() {
        0.0
    } else if s.len() % 2 == 1 {
        s[s.len() / 2]
    } else {
        (s[s.len() / 2 - 1] + s[s.len() / 2]) / 2.0
    }
}

/// Two-sided permutation test on the difference of means (exact up to 12+12 rounds' subsets,
/// else 20k random relabelings with a fixed seed).
fn perm_p(a: &[f64], b: &[f64]) -> f64 {
    let all: Vec<f64> = a.iter().chain(b).copied().collect();
    let n = all.len();
    let na = a.len();
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
    let obs = (mean(a) - mean(b)).abs();
    let total: f64 = all.iter().sum();
    let mut hits = 0u64;
    let mut tries = 0u64;
    if n <= 20 {
        for mask in 0u32..(1u32 << n) {
            if mask.count_ones() as usize != na {
                continue;
            }
            let sa: f64 = (0..n).filter(|k| mask & (1 << k) != 0).map(|k| all[k]).sum();
            let d = (sa / na as f64 - (total - sa) / (n - na) as f64).abs();
            tries += 1;
            if d >= obs - 1e-12 {
                hits += 1;
            }
        }
    } else {
        let mut x = 0x9e3779b97f4a7c15u64;
        let mut idx: Vec<usize> = (0..n).collect();
        for _ in 0..20_000 {
            for k in (1..n).rev() {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                idx.swap(k, (x % (k as u64 + 1)) as usize);
            }
            let sa: f64 = idx[..na].iter().map(|&k| all[k]).sum();
            let d = (sa / na as f64 - (total - sa) / (n - na) as f64).abs();
            tries += 1;
            if d >= obs - 1e-12 {
                hits += 1;
            }
        }
    }
    hits as f64 / tries.max(1) as f64
}

/// `ab <fn|@slot> b=<variant> [a=base] [rounds=5] [s=1] [settle_ms=300] [metric=incl|self] [per=s|frame|call] [ev=cpu|cycles|instructions] [keep=a|b]`
pub fn cmd_ab(req: &Req, out: &mut Out) {
    let Some(q) = req.pos.first().cloned() else {
        return out.err("usage: ab <fn|@slot> b=<base|o1|opt:<tok>,..|inline:<callee>|noinline:<callee>> [a=base] [rounds=5] [s=1] [metric=incl|self] [ev=cpu|cycles|instructions] [keep=a|b]");
    };
    let up = crate::jit::current_unit();
    if up.is_null() {
        return out.err("no running unit");
    }
    let rounds = req.num("rounds", 5).clamp(2, 10) as usize;
    let secs = req.get("s").and_then(|v| v.parse::<f64>().ok()).unwrap_or(1.0).clamp(0.2, 10.0);
    let settle = req.num("settle_ms", 300).min(5000);
    let hz = req.num("hz", 1000).clamp(50, 4000);
    let self_only = req.get("metric") == Some("self");
    let per_frame = req.get("per") == Some("frame");
    let per_call = req.get("per") == Some("call");
    let ev = match super::prof::Ev::parse(req.get("ev").unwrap_or("cpu")) {
        Ok(e) => e,
        Err(e) => return out.err(&e),
    };
    let qq = q.clone();
    let upv = up as usize;
    let found = crate::owner::call_within(crate::owner::Class::Job, 10_000, move || {
        let u = unsafe { &*(upv as *const crate::jit::Unit) };
        match qq.strip_prefix('@').and_then(|s| s.parse::<u32>().ok()) {
            Some(id) if (id as usize) < u.fns.len() => vec![(id, super::display_name(u, id, 0))],
            _ => super::instrument::find_all(u, &qq, 4).into_iter().filter(|x| u.fns[x.0 as usize].compiled).map(|x| (x.0, super::display_name(u, x.0, 0))).collect(),
        }
    });
    let Some(found) = found else {
        return out.err("the owner did not answer in 10 s");
    };
    let Some((id, name)) = found.first().cloned() else {
        return out.err(&format!("no compiled fn matches `{}` (ab needs code of its own; `versions {}` says where it runs)", q, q));
    };
    let (va, la) = match parse(req.get("a").unwrap_or("base"), up) {
        Ok(x) => x,
        Err(e) => return out.err(&e),
    };
    let (vb, lb) = match parse(req.get("b").unwrap_or(""), up) {
        Ok(x) => x,
        Err(e) => return out.err(&e),
    };
    let mut a = Vec::new();
    let mut b = Vec::new();
    let mut sizes = [0u64; 2];
    let mut tiers = [String::new(), String::new()];
    let mut notes = Vec::new();
    for r in 0..rounds * 2 {
        if out.cancelled() {
            break;
        }
        let k = r % 2;
        let v = if k == 0 { &va } else { &vb };
        if let Err(e) = install(up, id, v, r as u32) {
            let _ = install(up, id, &Variant::Base, 99);
            return out.err(&format!("variant {} ({}): {}", if k == 0 { "A" } else { "B" }, if k == 0 { &la } else { &lb }, e));
        }
        let Some((lo, hi, tier)) = code_of(up, id) else {
            return out.err("the fn has no installed code after the build");
        };
        sizes[k] = hi - lo;
        tiers[k] = tier;
        let (val, took) = if per_call {
            // exact calls and inclusive time per call: the variant's build again with timed entry /
            // exit probes (the variant's tokens stay set for that build), the same probes in A and B
            set_opts(id, match v {
                Variant::Opt(t) => Some(t.clone()),
                _ => None,
            });
            let g = super::instrument::count_slot(id);
            set_opts(id, None);
            let (group, sites) = match g {
                Ok(x) => x,
                Err(e) => {
                    let _ = install(up, id, &Variant::Base, 99);
                    return out.err(&format!("per=call: the timed count build failed: {}", e));
                }
            };
            std::thread::sleep(std::time::Duration::from_millis(settle));
            let (h0, n0) = super::instrument::site_totals(&sites);
            let t0 = std::time::Instant::now();
            let gone = out.gone();
            while t0.elapsed().as_secs_f64() < secs && !gone() {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            let (h1, n1) = super::instrument::site_totals(&sites);
            let _ = super::instrument::remove_group(group);
            let calls = h1.saturating_sub(h0);
            (if calls > 0 { n1.saturating_sub(n0) as f64 / calls as f64 } else { 0.0 }, t0.elapsed().as_secs_f64())
        } else {
            std::thread::sleep(std::time::Duration::from_millis(settle));
            measure(lo, hi, secs, hz, ev, self_only, per_frame, out)
        };
        if took < secs * 0.8 {
            notes.push(format!("round {} measured {:.2} s of {:.2}", r + 1, took, secs));
        }
        if k == 0 {
            a.push(val);
        } else {
            b.push(val);
        }
    }
    // leave the winner (keep=), else A
    let keep_b = req.get("keep") == Some("b");
    let _ = install(up, id, if keep_b { &vb } else { &va }, 100);
    if a.len() < 2 || b.len() < 2 {
        return out.err("too few rounds measured (the client went away?)");
    }
    let (ma, mb) = (median(&a), median(&b));
    let spread = |v: &[f64], m: f64| v.iter().map(|x| ((x - m) / m.max(1e-9)).abs()).fold(0.0, f64::max) * 100.0;
    let (sa, sb) = (spread(&a, ma), spread(&b, mb));
    let d = (mb - ma) / ma.max(1e-9) * 100.0;
    let p = perm_p(&a, &b);
    let noise = sa.max(sb);
    let unit = if per_call { "ns per call (incl.)".to_string() } else { format!("{}/{}", match ev {
        super::prof::Ev::Cpu => "ms CPU",
        super::prof::Ev::Cycles => "cycles",
        super::prof::Ev::Instructions => "instr",
        super::prof::Ev::BranchMisses => "branch misses",
        super::prof::Ev::CacheMisses => "cache misses",
    }, if per_frame { "frame" } else { "s" }) };
    let verdict = if ma <= 0.0 && mb <= 0.0 {
        "no samples in the fn: is it running? (ab measures the fn's cost rate while the app runs)".to_string()
    } else if p < 0.05 && d.abs() > noise / 2.0 {
        format!("B is {} by {:.1}% (real: p={:.3}, beyond the rounds' spread ±{:.1}%)", if d < 0.0 { "cheaper" } else { "costlier" }, d.abs(), p, noise)
    } else {
        format!("no difference beyond noise: {:+.1}% with spread ±{:.1}%, p={:.2}", d, noise, p)
    };
    out.always(&format!("`{}`: {}", short(&name, 90), verdict));
    out.line("");
    out.line(&format!("| variant | what | code | median {} | rounds | spread % |", unit));
    out.line("|---|---|---|---:|---|---:|");
    let fmt = |v: &[f64]| v.iter().map(|x| fmt_num(*x)).collect::<Vec<_>>().join(" ");
    out.line(&format!("| A | {} | {} {} B | {} | {} | {:.1} |", la, tiers[0], sizes[0], fmt_num(ma), fmt(&a), sa));
    out.line(&format!("| B | {} | {} {} B | {} | {} | {:.1} |", lb, tiers[1], sizes[1], fmt_num(mb), fmt(&b), sb));
    out.always(&format!(
        "note: {} rounds per variant, ABAB order, {:.1} s each after {} ms settle, metric {} ({}), permutation test over rounds; now installed: {}",
        rounds,
        secs,
        settle,
        if self_only { "self (pc in the fn's code)" } else { "inclusive (the fn's code anywhere on the stack)" },
        ev.name(),
        if keep_b { "B" } else { "A" }
    ));
    if per_call {
        out.always("note: per=call: each round counts the variant's build again with timed entry/exit probes (exact calls, inclusive ns per call); the probes cost the same in A and B");
    } else if !per_frame {
        out.always("note: a rate per second of wall: right for fixed-pace work (playback, a frame-locked UI); per=frame divides by UI frames instead; a flat-out loop keeps the fn's share constant -- compare those with count <fn> time=1");
    }
    for n in notes {
        out.always(&format!("note: {}", n));
    }
    out.always(&format!("note: {}", super::reliability::line(true)));
    out.always(&format!("more: asm @{} vs=rustc  remarks @{}  versions @{}", id, id, id));
}

fn fmt_num(x: f64) -> String {
    if x >= 1e9 {
        format!("{:.3}G", x / 1e9)
    } else if x >= 1e6 {
        format!("{:.3}M", x / 1e6)
    } else if x >= 1e3 {
        format!("{:.2}k", x / 1e3)
    } else {
        format!("{:.2}", x)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn perm_test_separates_and_ties() {
        let p = super::perm_p(&[10.0, 10.1, 9.9, 10.0, 10.05], &[9.0, 9.1, 8.9, 9.05, 9.0]);
        assert!(p < 0.02, "{}", p);
        let q = super::perm_p(&[10.0, 9.0, 10.0, 9.0], &[9.5, 9.6, 9.4, 9.5]);
        assert!(q > 0.3, "{}", q);
    }
}
