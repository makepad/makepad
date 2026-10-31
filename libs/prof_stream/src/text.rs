//! Text renderings for agents (profproto.md §5): numbers at 2-3 significant digits, markdown
//! tables with units in the header, Unicode lane timelines with a legend, sparklines, and short
//! names with a legend of their full paths. Scope's panels show the same things drawn.

use crate::model::Profile;
use crate::role;

/// ns as milliseconds at 2-3 significant digits ("12.4", "0.31", "0.004" -> "4µs" style is
/// left to [`dur`]).
pub fn ms(ns: u64) -> String {
    sig(ns as f64 / 1e6)
}

/// A duration with its unit: ms, or µs under 0.1 ms.
pub fn dur(ns: u64) -> String {
    if ns < 100_000 {
        format!("{}µs", sig(ns as f64 / 1e3))
    } else {
        format!("{}ms", sig(ns as f64 / 1e6))
    }
}

/// 2-3 significant digits, no trailing zeros.
pub fn sig(v: f64) -> String {
    if v == 0.0 {
        return "0".into();
    }
    let a = v.abs();
    let s = if a >= 100.0 {
        format!("{:.0}", v)
    } else if a >= 10.0 {
        format!("{:.1}", v)
    } else if a >= 1.0 {
        format!("{:.2}", v)
    } else if a >= 0.1 {
        format!("{:.2}", v)
    } else if a >= 0.01 {
        format!("{:.3}", v)
    } else {
        format!("{:.4}", v)
    };
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

pub fn pct(part: u64, whole: u64) -> String {
    if whole == 0 {
        return "-".into();
    }
    let p = part as f64 * 100.0 / whole as f64;
    if p >= 10.0 {
        format!("{:.0}", p)
    } else {
        sig(p)
    }
}

/// A markdown table: header cells (units in them), rows of cells.
#[derive(Clone, Debug, Default)]
pub struct Table {
    pub head: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// per row, a query it leads to (Scope makes the row clickable); empty: none
    pub links: Vec<Option<String>>,
}

impl Table {
    pub fn new(head: &[&str]) -> Table {
        Table { head: head.iter().map(|s| s.to_string()).collect(), rows: Vec::new(), links: Vec::new() }
    }
    pub fn row(&mut self, cells: Vec<String>) {
        self.rows.push(cells);
    }
    /// A row that leads to `query` when clicked.
    pub fn row_link(&mut self, cells: Vec<String>, query: Option<String>) {
        while self.links.len() < self.rows.len() {
            self.links.push(None);
        }
        self.rows.push(cells);
        self.links.push(query);
    }
    pub fn render(&self) -> String {
        let n = self.head.len();
        let mut w = vec![0usize; n];
        for (i, h) in self.head.iter().enumerate() {
            w[i] = h.chars().count();
        }
        for r in &self.rows {
            for (i, c) in r.iter().enumerate().take(n) {
                w[i] = w[i].max(c.chars().count());
            }
        }
        let line = |cells: &[String]| {
            let mut s = String::from("|");
            for i in 0..n {
                let c = cells.get(i).map(String::as_str).unwrap_or("");
                s.push(' ');
                s.push_str(c);
                s.push_str(&" ".repeat(w[i] - c.chars().count()));
                s.push_str(" |");
            }
            s
        };
        let mut out = line(&self.head);
        out.push('\n');
        out.push('|');
        for wi in &w {
            out.push_str(&"-".repeat(wi + 2));
            out.push('|');
        }
        for r in &self.rows {
            out.push('\n');
            out.push_str(&line(r));
        }
        out
    }
}

/// A sparkline of `v` (one block char per value, scaled to the max); at most `width` chars
/// (values are max-pooled into buckets so spikes stay visible).
pub fn sparkline(v: &[u64], width: usize) -> String {
    const B: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    if v.is_empty() {
        return String::new();
    }
    let n = v.len().min(width.max(1));
    let mut pooled = vec![0u64; n];
    for (i, x) in v.iter().enumerate() {
        let k = i * n / v.len();
        pooled[k] = pooled[k].max(*x);
    }
    let max = *pooled.iter().max().unwrap_or(&1).max(&1);
    pooled.iter().map(|x| B[((*x as f64 / max as f64) * 7.0).round() as usize]).collect()
}

/// Shortens a Rust path for a table cell: its last two segments, generics dropped.
pub fn short_name(full: &str) -> String {
    let mut base = String::new();
    let mut depth = 0;
    for c in full.chars() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            _ if depth == 0 => base.push(c),
            _ => {}
        }
    }
    let parts: Vec<&str> = base.split("::").filter(|p| !p.is_empty()).collect();
    if parts.len() <= 2 {
        return base;
    }
    parts[parts.len() - 2..].join("::")
}

/// Names shortened with a legend: `name(full)` gives the cell text; [`Legend::render`] lists the
/// full paths that were shortened, once.
#[derive(Default)]
pub struct Legend {
    pub entries: Vec<(String, String)>,
}

impl Legend {
    pub fn name(&mut self, full: &str) -> String {
        let s = short_name(full);
        if s != full && !self.entries.iter().any(|e| e.1 == full) {
            // two different full names with the same short form get a mark
            let taken = self.entries.iter().filter(|e| e.0 == s || e.0.starts_with(&format!("{s}#"))).count();
            let s2 = if taken > 0 { format!("{s}#{}", taken + 1) } else { s.clone() };
            self.entries.push((s2.clone(), full.to_string()));
            return s2;
        }
        self.entries.iter().find(|e| e.1 == full).map_or(s, |e| e.0.clone())
    }
    pub fn render(&self) -> String {
        if self.entries.is_empty() {
            return String::new();
        }
        let v: Vec<String> = self.entries.iter().map(|(s, f)| format!("{s} = {f}")).collect();
        format!("names: {}", v.join("; "))
    }
}

/// A Unicode timeline of [t0, t1): one row per lane, `width` columns; each column shows the
/// span that took most of that slice (a letter from the legend), `·` waiting, space idle. A ms
/// grid row on top. Lanes with nothing in the window are left out (and counted).
pub fn timeline(p: &Profile, lanes: &[usize], t0: u64, t1: u64, width: usize) -> String {
    let width = width.clamp(20, 200);
    let span = (t1 - t0).max(1);
    let mut letters: Vec<(u32, bool)> = Vec::new();
    const LETTERS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rows: Vec<(String, String)> = Vec::new();
    let mut empty = 0;
    for &li in lanes {
        let l = &p.lanes[li];
        let mut cells = vec![(0u64, 0u32, false); width];
        let mut any = false;
        for d in 0..l.rows.len() {
            for s in l.segs_in(d, t0, t1) {
                let a = s.t0.max(t0) - t0;
                let b = s.t1.min(t1) - t0;
                let c0 = (a as u128 * width as u128 / span as u128) as usize;
                let c1 = ((b as u128 * width as u128).div_ceil(span as u128) as usize).min(width);
                for (c, cell) in cells.iter_mut().enumerate().take(c1.max(c0 + 1).min(width)).skip(c0) {
                    let cs = t0 + (c as u128 * span as u128 / width as u128) as u64;
                    let ce = t0 + ((c + 1) as u128 * span as u128 / width as u128) as u64;
                    let cover = s.t1.min(ce).saturating_sub(s.t0.max(cs));
                    // deeper (inner) spans win over their parents when they cover at least half
                    if cover > 0 && (cover * 2 >= ce - cs || cover >= cell.0 || d > 0 && cover * 2 > cell.0) {
                        *cell = (cover, s.name, s.wait);
                        any = true;
                    }
                }
            }
        }
        if !any {
            empty += 1;
            continue;
        }
        let mut row = String::with_capacity(width);
        for (cover, name, wait) in &cells {
            if *cover == 0 {
                row.push(' ');
            } else if *wait {
                row.push('·');
            } else {
                let k = match letters.iter().position(|x| *x == (*name, false)) {
                    Some(k) => k,
                    None => {
                        letters.push((*name, false));
                        letters.len() - 1
                    }
                };
                row.push(LETTERS.get(k).copied().unwrap_or(b'?') as char);
            }
        }
        let nm = p.lane_name(li);
        let tag = if l.thread.role == role::GPU { format!("gpu {nm}") } else { nm };
        rows.push((tag, row));
    }
    let lw = rows.iter().map(|r| r.0.chars().count()).max().unwrap_or(4).min(18);
    let mut out = String::new();
    // grid: a tick every ~10 columns with its ms offset
    let mut grid = vec![' '; width];
    let mut labels = String::new();
    let step = nice_step(span as f64 / 1e6 / (width as f64 / 12.0));
    let mut ms_at = 0.0;
    while ms_at <= span as f64 / 1e6 {
        let c = ((ms_at * 1e6) / span as f64 * width as f64) as usize;
        if c < width {
            grid[c] = '|';
            let lab = sig(ms_at);
            let pad = c.saturating_sub(labels.chars().count());
            if pad > 0 || labels.is_empty() {
                labels.push_str(&" ".repeat(pad));
                labels.push_str(&lab);
            }
        }
        ms_at += step;
    }
    out.push_str(&format!("{:lw$} {}  ms\n", "", labels, lw = lw));
    out.push_str(&format!("{:lw$} {}\n", "", grid.iter().collect::<String>(), lw = lw));
    for (n, r) in &rows {
        let n: String = n.chars().take(lw).collect();
        out.push_str(&format!("{:lw$} {}\n", n, r, lw = lw));
    }
    let mut lg = Legend::default();
    let leg: Vec<String> = letters.iter().enumerate().map(|(k, (n, _))| format!("{}={}", LETTERS.get(k).copied().unwrap_or(b'?') as char, lg.name(p.s(*n)))).collect();
    out.push_str(&format!("legend: {}  ·=wait  (space)=idle", leg.join(" ")));
    if empty > 0 {
        out.push_str(&format!("\n+{} lanes idle in this window", empty));
    }
    out
}

fn nice_step(raw: f64) -> f64 {
    let raw = raw.max(1e-6);
    let mag = 10f64.powf(raw.log10().floor());
    for m in [1.0, 2.0, 5.0, 10.0] {
        if m * mag >= raw {
            return m * mag;
        }
    }
    10.0 * mag
}
