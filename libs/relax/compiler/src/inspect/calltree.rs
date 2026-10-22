//! `prof view=tree|callers|flame`: the sampled call tree across JIT and native frames, with the
//! JIT's inline levels expanded into frames of their own (an O3 body that inlined `push` into
//! `extend` shows `extend [O3] > push [inl]`) and every JIT frame marked with the tier of the code
//! it ran in. The same samples as `prof`; weights are CPU time or a hardware event (`ev=`).
//!
//! The tree is answer-first markdown (profproto.md §5): line 1 is the hottest path; rows are
//! indented by depth, cut below `min=` percent; `focus=<fn>` re-roots at every occurrence of a fn
//! (its callees, merged), `view=callers fn=<fn>` inverts (who calls it, merged). `save=1` writes
//! the expanded stacks as folded text (thread;outer;...;inner count) for flame graphs.

use super::{json_str, now_ms, short, Out, Req};
use std::collections::HashMap;

/// One frame of an expanded stack.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct XFrame {
    pub name: String,
    /// "O3", "O1", "baseline", ... for JIT code; "inl" for an inlined level; "native" for host code
    pub tier: String,
    /// file:line of the pc in this frame (JIT frames)
    pub at: String,
}

/// Expanded frames of each pc (innermost first, as stacks are): JIT pcs by the code tables on the
/// owner (inline stack from the pc map), host pcs by the symbol tables. `pcs`: (pc, is_leaf).
fn expand_pcs(pcs: &[u64], partial: &mut Vec<String>) -> HashMap<u64, Vec<XFrame>> {
    let mut out: HashMap<u64, Vec<XFrame>> = HashMap::new();
    let jit: Vec<u64> = pcs.iter().copied().filter(|&pc| crate::jit::in_jit_code(pc)).collect();
    let up = crate::jit::current_unit();
    if !jit.is_empty() && !up.is_null() {
        let r = crate::owner::call_within(crate::owner::Class::Job, 3000, move || {
            let u = unsafe { &*up };
            let mut res = super::codever::Resolver::new();
            jit.iter()
                .map(|&pc| {
                    let v = match super::codever::describe_pc(u, &mut res, pc) {
                        Some(c) => {
                            let tier = c.kind.tier_name().to_string();
                            let own = super::display_name(u, c.slot, 0);
                            match &c.inlined {
                                Some(i) if !i.stack.is_empty() => {
                                    // innermost first; the last level is the slot's own fn at the outermost call site
                                    let n = i.stack.len();
                                    // a level the code tables cannot name: its source position names it
                                    let unnamed = |name: &str, at: &str| if name == "(inlined fn)" { format!("(inlined code at {})", at.rsplit('/').next().unwrap_or(at).rsplitn(2, ':').last().unwrap_or(at)) } else { name.to_string() };
                                    i.stack.iter().enumerate().map(|(k, (name, _, at))| XFrame { name: if k + 1 == n { own.clone() } else { unnamed(name, at) }, tier: if k + 1 == n { tier.clone() } else { "inl".into() }, at: at.clone() }).collect()
                                }
                                Some(i) => vec![XFrame { name: i.name.clone(), tier: "inl".into(), at: c.at.clone() }, XFrame { name: own, tier, at: String::new() }],
                                None => vec![XFrame { name: own, tier, at: c.at.clone() }],
                            }
                        }
                        None => match super::codever::glue_name(u, pc) {
                            Some((name, _)) => vec![XFrame { name, tier: "glue".into(), at: String::new() }],
                            None => vec![XFrame { name: format!("jit {:#x}", pc), tier: "jit".into(), at: String::new() }],
                        },
                    };
                    (pc, v)
                })
                .collect::<Vec<_>>()
        });
        match r {
            Some(v) => out.extend(v),
            None => partial.push(format!("JIT frames unnamed: the owner was busy ({})", crate::owner::serving_note())),
        }
    }
    for &pc in pcs {
        out.entry(pc).or_insert_with(|| match super::symbols::host_symbol(pc) {
            Some((name, _module, _)) => vec![XFrame { name, tier: "native".into(), at: String::new() }],
            None => vec![XFrame { name: format!("{:#x}", pc), tier: "?".into(), at: String::new() }],
        });
    }
    out
}

/// Every sample as its expanded stack, outermost first, with its thread name and weight.
pub(crate) fn expanded(run: &super::prof::Run, partial: &mut Vec<String>) -> Vec<(String, Vec<XFrame>, u64)> {
    let tname = |id: u64| run.threads.iter().find(|t| t.os_id == id).map(|t| if t.name.is_empty() { format!("T{}", id) } else { format!("{} (T{})", t.name, id) }).unwrap_or_else(|| format!("T{}", id));
    expanded_raw(run, partial).into_iter().map(|(os, _, f, w)| (tname(os), f, w)).collect()
}

/// `expanded` with each sample's thread os id and time: (os id, t, frames outermost first, weight).
pub(crate) fn expanded_raw(run: &super::prof::Run, partial: &mut Vec<String>) -> Vec<(u64, u64, Vec<XFrame>, u64)> {
    let mut pcs: Vec<u64> = Vec::new();
    for s in &run.samples {
        for (i, &pc) in s.frames.iter().enumerate() {
            pcs.push(if i == 0 { pc } else { pc.wrapping_sub(1) });
        }
    }
    pcs.sort_unstable();
    pcs.dedup();
    let names = expand_pcs(&pcs, partial);
    run.samples
        .iter()
        .map(|s| {
            let mut v: Vec<XFrame> = Vec::new();
            for (i, &pc) in s.frames.iter().enumerate().rev() {
                let look = if i == 0 { pc } else { pc.wrapping_sub(1) };
                if let Some(f) = names.get(&look) {
                    // the expansion is innermost first
                    v.extend(f.iter().rev().cloned());
                }
            }
            (s.os_id, s.t, v, s.weight.max(1))
        })
        .collect()
}

struct Node {
    name: String,
    tier: String,
    total: u64,
    selfw: u64,
    kids: HashMap<(String, String), usize>,
}

struct Tree {
    nodes: Vec<Node>,
}

impl Tree {
    fn new(root: &str) -> Tree {
        Tree { nodes: vec![Node { name: root.into(), tier: String::new(), total: 0, selfw: 0, kids: HashMap::new() }] }
    }
    fn add(&mut self, path: &[(String, String)], w: u64) {
        let mut cur = 0;
        self.nodes[0].total += w;
        for key in path {
            let next = match self.nodes[cur].kids.get(key) {
                Some(&k) => k,
                None => {
                    let k = self.nodes.len();
                    self.nodes.push(Node { name: key.0.clone(), tier: key.1.clone(), total: 0, selfw: 0, kids: HashMap::new() });
                    self.nodes[cur].kids.insert(key.clone(), k);
                    k
                }
            };
            self.nodes[next].total += w;
            cur = next;
        }
        self.nodes[cur].selfw += w;
    }
    fn kids_sorted(&self, k: usize) -> Vec<usize> {
        let mut v: Vec<usize> = self.nodes[k].kids.values().copied().collect();
        v.sort_by(|a, b| self.nodes[*b].total.cmp(&self.nodes[*a].total).then(self.nodes[*a].name.cmp(&self.nodes[*b].name)));
        v
    }
    /// The heaviest path from the root: fn names joined with " > ".
    fn hot_path(&self) -> (String, u64) {
        let mut cur = 0;
        let mut names = Vec::new();
        while let Some(&k) = self.kids_sorted(cur).first() {
            if self.nodes[k].total * 2 < self.nodes[cur].total && cur != 0 {
                break;
            }
            names.push(short(&super::asm_plain(&self.nodes[k].name), 50));
            cur = k;
        }
        // keep the last few levels: the interesting end
        let n = names.len();
        let shown = if n > 6 { format!("... > {}", names[n - 6..].join(" > ")) } else { names.join(" > ") };
        (shown, self.nodes[cur].total)
    }
}

/// `prof view=tree|callers|flame [focus=<fn>] [fn=<fn>] [min=1] [n=60]` on a finished run.
pub(crate) fn report(run: &super::prof::Run, req: &Req, out: &mut Out) {
    let mut partial: Vec<String> = run.notes.clone();
    let stacks = expanded(run, &mut partial);
    let total: u64 = stacks.iter().map(|s| s.2).sum::<u64>().max(1);
    let view = req.get("view").unwrap_or("tree");
    let min = req.get("min").and_then(|v| v.parse::<f64>().ok()).unwrap_or(1.0);
    let n = req.num("n", 60) as usize;
    let unit = match run.ev {
        super::prof::Ev::Cpu => "CPU",
        e => e.name(),
    };
    let matches = |f: &XFrame, q: &str| f.name == q || f.name.ends_with(&format!("::{}", q)) || super::asm_plain(&f.name).ends_with(q);
    let mut tree = Tree::new("all");
    match view {
        "callers" => {
            let Some(q) = req.get("fn").or(req.get("focus")) else {
                return out.err("view=callers needs fn=<fn>");
            };
            // inverted: the fn at the root, its callers below (each sample once, at its innermost occurrence)
            for (th, st, w) in &stacks {
                if let Some(k) = st.iter().rposition(|f| matches(f, q)) {
                    let mut p: Vec<(String, String)> = st[..=k].iter().rev().map(|f| (f.name.clone(), f.tier.clone())).collect();
                    p.push((th.clone(), "thread".into()));
                    tree.add(&p, *w);
                }
            }
        }
        _ => {
            let focus = req.get("focus");
            for (th, st, w) in &stacks {
                match focus {
                    Some(q) => {
                        // every occurrence's callees merged under one root (outermost occurrence)
                        if let Some(k) = st.iter().position(|f| matches(f, q)) {
                            let p: Vec<(String, String)> = st[k..].iter().map(|f| (f.name.clone(), f.tier.clone())).collect();
                            tree.add(&p, *w);
                        }
                    }
                    None => {
                        let mut p = vec![(th.clone(), "thread".to_string())];
                        p.extend(st.iter().map(|f| (f.name.clone(), f.tier.clone())));
                        tree.add(&p, *w);
                    }
                }
            }
        }
    }
    if view == "flame" || req.get("save") == Some("1") {
        let mut folded: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
        for (th, st, w) in &stacks {
            let mut parts = vec![th.replace(';', ":")];
            parts.extend(st.iter().map(|f| format!("{} [{}]", f.name.replace(';', ":"), f.tier)));
            *folded.entry(parts.join(";")).or_default() += w;
        }
        let path = std::env::temp_dir().join(format!("makepad-run-inspect-{}-prof-{}.folded", std::process::id(), now_ms()));
        let text: String = folded.iter().map(|(k, v)| format!("{} {}\n", k, v)).collect();
        match std::fs::write(&path, text) {
            Ok(()) => out.always(&format!("folded stacks (inline levels expanded, [tier] per frame): {} ({} distinct stacks)", path.display(), folded.len())),
            Err(e) => out.err(&format!("cannot write {}: {}", path.display(), e)),
        }
        if view == "flame" {
            return;
        }
    }
    let root_total = tree.nodes[0].total;
    if root_total == 0 {
        return out.err(&format!("no samples{}; try: prof view=tree s=5 threads=all", req.get("focus").or(req.get("fn")).map_or(String::new(), |q| format!(" with `{}` on the stack", q))));
    }
    let (hp, hw) = tree.hot_path();
    out.always(&format!(
        "{} {} samples over {:.1} s{}; hottest path ({}%): {}",
        count(root_total),
        unit,
        run.secs,
        if root_total != total { format!(" ({}% of all)", pct(root_total, total)) } else { String::new() },
        pct(hw, root_total),
        hp
    ));
    out.line("");
    out.line(&format!("| total % | self % | fn | tier |"));
    out.line("|---:|---:|---|---|");
    let mut rows = 0usize;
    let mut cut = 0u64;
    let mut cut_n = 0usize;
    let mut stack: Vec<(usize, usize)> = tree.kids_sorted(0).into_iter().rev().map(|k| (k, 0)).collect();
    let mut legend: Vec<String> = Vec::new();
    while let Some((k0, d)) = stack.pop() {
        let nd0 = &tree.nodes[k0];
        if (nd0.total as f64) * 100.0 < min * root_total as f64 || rows >= n || d > 48 {
            cut += nd0.selfw;
            cut_n += 1;
            continue;
        }
        rows += 1;
        // a chain of single callees with all of the time (startup frames, wrappers): one row
        let mut k = k0;
        let mut chain = 0usize;
        while tree.nodes[k].selfw == 0 && tree.nodes[k].kids.len() == 1 {
            let c = *tree.nodes[k].kids.values().next().unwrap();
            if tree.nodes[c].total != tree.nodes[k].total || tree.nodes[c].tier != tree.nodes[k].tier {
                break;
            }
            k = c;
            chain += 1;
        }
        let nd = &tree.nodes[k];
        let name = if chain > 0 { format!("{} > ({} frames) > {}", short(&super::asm_plain(&nd0.name), 40), chain.saturating_sub(1), super::asm_plain(&nd.name)) } else { super::asm_plain(&nd.name) };
        let shown = if name.len() > 70 {
            legend.push(format!("[{}] {}", legend.len() + 1, nd.name));
            format!("{} [{}]", short(&name, 60), legend.len())
        } else {
            name
        };
        out.line(&format!("| {} | {} | {}{} | {} |", pct(nd.total, root_total), if nd.selfw > 0 { pct(nd.selfw, root_total) } else { String::new() }, "· ".repeat(d), shown.replace('|', "/"), nd.tier));
        for c in tree.kids_sorted(k).into_iter().rev() {
            stack.push((c, d + 1));
        }
    }
    if cut_n > 0 {
        out.line(&format!("+{} nodes below {}% or past n={} ({}% self time in them)", cut_n, min, n, pct(cut, root_total)));
    }
    for l in legend.iter().take(12) {
        out.line(l);
    }
    out.always("tiers: O3/O2/O1/baseline = JIT code of that tier, inl = a level the JIT inlined (its own frame here), native = compiled-in code (makepad run, provided crates, system)");
    for p in &partial {
        out.always(&format!("note: {}", p));
    }
    out.always(&format!("note: {}", super::reliability::line(true)));
    out.always("more: prof view=tree focus=<fn>  prof view=callers fn=<fn>  prof view=flame  asm <fn> vs=rustc");
    let _ = json_str;
}

fn pct(a: u64, t: u64) -> String {
    let p = a as f64 * 100.0 / t.max(1) as f64;
    if p >= 10.0 {
        format!("{:.0}", p)
    } else {
        format!("{:.1}", p)
    }
}

fn count(c: u64) -> String {
    if c >= 1_000_000 {
        format!("{:.1}M", c as f64 / 1e6)
    } else if c >= 10_000 {
        format!("{:.0}k", c as f64 / 1e3)
    } else {
        c.to_string()
    }
}
