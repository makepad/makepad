//! Native GPU inspection. Reports come from the HAL ledger, without a Cx mailbox or callback.

use crate::ledger::{Group, Report, ReportItem};
use makepad_remote::{json_str, Out, Req};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub const COMMANDS: &[(&str, &str, &str)] = &[
    (
        "gpu",
        "",
        "GPU bytes by session, kind and owner, reconciled against the device",
    ),
    (
        "gpu top",
        "[by=bytes|owner|site|churn] [n=15]",
        "largest allocations or allocating owners/sites",
    ),
    (
        "gpu idle",
        "[frames=1800] [n=15]",
        "textures unused for this many submitted frames, or never used",
    ),
    ("gpu snap", "<name>", "keep a named HAL ledger snapshot"),
    (
        "hal pump",
        "",
        "each session's pump turns and wake causes, totals and per second since the last call",
    ),
    (
        "gpu diff",
        "<a> <b|now>",
        "byte growth by owner and new/freed handles",
    ),
    (
        "gpu path",
        "<session>:<kind>:<index>:<generation>",
        "allocation owner and creation site",
    ),
];

static SNAPS: Mutex<Vec<(String, Arc<Report>)>> = Mutex::new(Vec::new());

fn key(item: &ReportItem) -> String {
    format!(
        "{}:{}:{}:{}",
        item.session,
        item.kind.name(),
        item.index,
        item.gen
    )
}

fn item(item: &ReportItem, out: &mut Out) {
    out.line(&format!(
        "{{\"id\":{},\"session\":{},\"kind\":{},\"bytes\":{},\"owner\":{},\"site\":{},\"created_frame\":{},\"last_used_frame\":{}}}",
        json_str(&key(item)), item.session, json_str(item.detail), item.bytes, json_str(&item.owner),
        json_str(&item.site), item.created_frame, item.last_used_frame,
    ));
}

fn groups(groups: &[Group], n: usize, out: &mut Out) {
    for g in groups.iter().take(n) {
        out.line(&format!(
            "{{\"name\":{},\"bytes\":{},\"count\":{}}}",
            json_str(&g.key),
            g.bytes,
            g.count
        ));
    }
}

fn fresh(req: &Req) -> Report {
    let idle = req
        .get("frames")
        .and_then(|n| n.parse().ok())
        .unwrap_or(1800);
    crate::Hal::global().report_all(idle)
}

/// True when this is a GPU or HAL command. The endpoint thread reads the native ledger directly.
pub fn dispatch(req: &Req, out: &mut Out) -> bool {
    if crate::framecap::dispatch(req, out) {
        return true;
    }
    if req.cmd == "hal" {
        match req.pos.first().map(String::as_str) {
            Some("pump") => pump(out),
            _ => out.err("hal pump"),
        }
        return true;
    }
    if req.cmd != "gpu" {
        return false;
    }
    if crate::gpu_prof::dispatch(req, out) {
        return true;
    }
    let sub = req.pos.first().map(String::as_str).unwrap_or("");
    if sub == "diff" {
        diff(req, out);
        return true;
    }
    let mut snap = fresh(req);
    let n = req.get("n").and_then(|n| n.parse().ok()).unwrap_or(15);
    match sub {
        "" => {
            let device = snap
                .device_allocated
                .map(|b| b.to_string())
                .unwrap_or_else(|| "null".into());
            out.line(&format!(
                "{{\"ledger_bytes\":{},\"device_bytes\":{},\"items\":{}}}",
                snap.total_bytes,
                device,
                snap.items.len()
            ));
            for session in &snap.by_session {
                out.line(&format!(
                    "{{\"session\":{},\"frame\":{},\"ledger_bytes\":{},\"items\":{}}}",
                    session.session, session.frame, session.bytes, session.count
                ));
            }
            groups(&snap.by_kind, usize::MAX, out);
            groups(&snap.by_owner, usize::MAX, out);
        }
        "top" => match req.get("by").unwrap_or("bytes") {
            "bytes" => {
                for i in snap.items.iter().take(n) {
                    item(i, out);
                }
            }
            "owner" => groups(&snap.by_owner, n, out),
            "site" => groups(&snap.by_site, n, out),
            "churn" => {
                let mut sites: HashMap<&str, (u64, u64)> = HashMap::new();
                for (site, count, bytes) in &snap.churn {
                    let totals = sites.entry(site).or_default();
                    totals.0 += count;
                    totals.1 += bytes;
                }
                let mut sites: Vec<_> = sites.into_iter().collect();
                sites.sort_by_key(|(_, (_, bytes))| std::cmp::Reverse(*bytes));
                for (site, (count, bytes)) in sites.into_iter().take(n) {
                    out.line(&format!(
                        "{{\"site\":{},\"handles_created\":{},\"bytes_created\":{}}}",
                        json_str(site),
                        count,
                        bytes
                    ));
                }
            }
            _ => out.err("gpu top: by=bytes|owner|site|churn"),
        },
        "idle" => {
            snap.idle.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.session.cmp(&b.session)).then(a.index.cmp(&b.index)));
            for i in snap.idle.iter().take(n) {
                item(i, out);
            }
        }
        "path" => match req
            .pos
            .get(1)
            .and_then(|id| snap.items.iter().find(|i| key(i) == *id))
        {
            Some(i) => item(i, out),
            None => out.err("gpu path: allocation not found (use an id from gpu top)"),
        },
        "snap" => {
            let Some(name) = req.pos.get(1) else {
                out.err("gpu snap <name>");
                return true;
            };
            let mut snaps = SNAPS.lock().unwrap_or_else(|e| e.into_inner());
            snaps.retain(|(n, _)| n != name);
            out.line(&format!(
                "{{\"snap\":{},\"ledger_bytes\":{}}}",
                json_str(name),
                snap.total_bytes
            ));
            snaps.push((name.clone(), Arc::new(snap)));
        }
        _ => out.err("gpu | gpu top | gpu idle | gpu snap | gpu diff | gpu path | gpu prof start|stop|status|frames|frame|lanes|top|util|diff"),
    }
    true
}

fn diff(req: &Req, out: &mut Out) {
    let (Some(a), Some(b)) = (req.pos.get(1), req.pos.get(2)) else {
        out.err("gpu diff <a> <b|now>");
        return;
    };
    let lookup = |name: &str| {
        SNAPS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, s)| s.clone())
    };
    let Some(a) = lookup(a) else {
        out.err("first snapshot not found");
        return;
    };
    let b = if b == "now" {
        Some(Arc::new(fresh(req)))
    } else {
        lookup(b)
    };
    let Some(b) = b else {
        out.err("second snapshot not found");
        return;
    };
    let mut growth: HashMap<&str, i128> = HashMap::new();
    for g in &a.by_owner {
        *growth.entry(&g.key).or_default() -= g.bytes as i128;
    }
    for g in &b.by_owner {
        *growth.entry(&g.key).or_default() += g.bytes as i128;
    }
    let mut growth: Vec<_> = growth.into_iter().filter(|(_, b)| *b != 0).collect();
    growth.sort_by_key(|(_, b)| std::cmp::Reverse(*b));
    for (owner, bytes) in growth {
        out.line(&format!(
            "{{\"owner\":{},\"growth_bytes\":{}}}",
            json_str(owner),
            bytes
        ));
    }
    let before: HashMap<_, _> = a.items.iter().map(|i| (key(i), i)).collect();
    let after: HashMap<_, _> = b.items.iter().map(|i| (key(i), i)).collect();
    for (id, i) in &after {
        if !before.contains_key(id) {
            out.line(&format!(
                "{{\"new\":{},\"bytes\":{}}}",
                json_str(id),
                i.bytes
            ));
        }
    }
    for (id, i) in &before {
        if !after.contains_key(id) {
            out.line(&format!(
                "{{\"freed\":{},\"bytes\":{}}}",
                json_str(id),
                i.bytes
            ));
        }
    }
}

/// The last `hal pump` answer, for the rates of the next one.
static PUMP_LAST: Mutex<Option<(std::time::Instant, Vec<(u32, crate::PumpCounts)>)>> = Mutex::new(None);

fn pump(out: &mut Out) {
    let now = std::time::Instant::now();
    let counts = crate::Hal::global().pump_counts();
    let mut last = PUMP_LAST.lock().unwrap_or_else(|e| e.into_inner());
    let before = last.take();
    for (sid, c) in &counts {
        let rates = before.as_ref().and_then(|(t, prev)| {
            let secs = now.duration_since(*t).as_secs_f64();
            let p = prev.iter().find(|(id, _)| id == sid).map(|(_, p)| *p).unwrap_or_default();
            (secs > 0.0).then(|| format!(
                ",\"secs\":{:.2},\"turns_per_s\":{:.1},\"calls_per_s\":{:.1},\"event_per_s\":{:.1},\"wake_per_s\":{:.1},\"all_per_s\":{:.1},\"timeout_per_s\":{:.1}",
                secs,
                (c.turns - p.turns) as f64 / secs,
                (c.calls - p.calls) as f64 / secs,
                (c.by_event - p.by_event) as f64 / secs,
                (c.by_wake - p.by_wake) as f64 / secs,
                (c.by_all - p.by_all) as f64 / secs,
                (c.by_timeout - p.by_timeout) as f64 / secs,
            ))
        });
        out.line(&format!(
            "{{\"session\":{},\"calls\":{},\"turns\":{},\"delivered\":{},\"by_event\":{},\"by_wake\":{},\"by_all\":{},\"by_timeout\":{}{}}}",
            sid, c.calls, c.turns, c.delivered, c.by_event, c.by_wake, c.by_all, c.by_timeout,
            rates.unwrap_or_default(),
        ));
    }
    *last = Some((now, counts));
}
