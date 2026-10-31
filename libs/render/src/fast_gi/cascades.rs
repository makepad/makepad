//! Probe cascades and the per-frame update schedule (UI thread, O(budget)).
//!
//! Each cascade is a camera-centred lattice (`grid` probes at `spacing`)
//! stored TOROIDALLY: world cell `c` lives in slot `c mod grid`, so moving
//! the window only re-targets the slots of the slab it newly covers. Those
//! slots are queued for tracing; every other probe keeps its value. The
//! shaders reject a slot whose stored probe does not belong to the cell
//! they expect, so a not-yet-traced slot can never show stale light.
//!
//! Schedule, per frame, `budget` probes: queued work first (new slab
//! probes, probes near an edit), then a round-robin refresh, both split
//! 2:1:1 between the fine and coarser cascades.
use std::collections::VecDeque;

/// History weight of an ordinary refresh (rotated ray set each time).
pub(crate) const REFRESH_HYSTERESIS: f32 = 0.85;
/// History weight right after an edit near the probe: show it quickly.
pub(crate) const DIRTY_HYSTERESIS: f32 = 0.2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BatchItem {
    pub cascade: usize,
    pub slot: usize,
    pub cell: [i32; 3],
    pub hysteresis: f32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Queued { No, New, Dirty }

pub(crate) struct Cascade {
    pub grid: [usize; 3],
    pub spacing: f32,
    /// Min cell of the window; None until the first voxels arrived.
    pub origin: Option<[i32; 3]>,
    queued: Vec<Queued>,
    queue: VecDeque<u32>,
    cursor: usize,
    /// Frame stamp of each slot's last scheduling: one update per frame.
    stamp: Vec<u32>,
    frame: u32,
}

/// Visits every slot once per `count` steps, spread through the volume so
/// a refresh wave never sweeps visibly through space.
fn refresh_slot(step: usize, count: usize) -> usize {
    let stride = (count * 5 / 8) | 1;
    let mut s = stride;
    while gcd(s, count) != 1 { s += 2; }
    (step * s) % count
}
fn gcd(a: usize, b: usize) -> usize { if b == 0 { a } else { gcd(b, a % b) } }

impl Cascade {
    pub fn new(grid: [usize; 3], spacing: f32) -> Self {
        let count = grid.iter().product();
        Self { grid, spacing, origin: None, queued: vec![Queued::No; count], queue: VecDeque::new(), cursor: 0, stamp: vec![0; count], frame: 0 }
    }
    pub fn count(&self) -> usize { self.queued.len() }
    pub fn pending(&self) -> usize { self.queue.len() }
    pub fn slot(&self, cell: [i32; 3]) -> usize {
        let m = [0, 1, 2].map(|a| cell[a].rem_euclid(self.grid[a] as i32) as usize);
        m[0] + self.grid[0] * (m[1] + self.grid[1] * m[2])
    }
    /// The world cell `slot` holds in the current window.
    pub fn cell(&self, slot: usize) -> Option<[i32; 3]> {
        let o = self.origin?;
        let m = [slot % self.grid[0], (slot / self.grid[0]) % self.grid[1], slot / (self.grid[0] * self.grid[1])];
        Some([0, 1, 2].map(|a| o[a] + (m[a] as i32 - o[a]).rem_euclid(self.grid[a] as i32)))
    }
    fn enqueue(&mut self, slot: usize, why: Queued) {
        match self.queued[slot] {
            Queued::No => { self.queued[slot] = why; self.queue.push_back(slot as u32); }
            Queued::New => {}
            Queued::Dirty => if why == Queued::New { self.queued[slot] = Queued::New; },
        }
    }
    /// Move the window; returns the slots that now hold another cell (their
    /// old probe must be cleared, the new one traced).
    pub fn scroll(&mut self, origin: [i32; 3]) -> Vec<usize> {
        let old = self.origin.replace(origin);
        let mut exposed = Vec::new();
        if old == Some(origin) { return exposed; }
        for slot in 0..self.count() {
            let c = self.cell(slot).unwrap();
            let kept = old.is_some_and(|o| (0..3).all(|a| c[a] >= o[a] && c[a] < o[a] + self.grid[a] as i32));
            if !kept { self.enqueue(slot, Queued::New); exposed.push(slot); }
        }
        exposed
    }
    /// Queue the probes near a changed world box; returns how many.
    pub fn invalidate(&mut self, lo: [f32; 3], hi: [f32; 3], home: [f32; 3]) -> usize {
        let Some(o) = self.origin else { return 0; };
        let pad = 1.5;
        let r = [0, 1, 2].map(|a| {
            let l = ((lo[a] / self.spacing - home[a] - pad).ceil() as i32).max(o[a]);
            let h = ((hi[a] / self.spacing - home[a] + pad).floor() as i32).min(o[a] + self.grid[a] as i32 - 1);
            (l, h)
        });
        let mut n = 0;
        for z in r[2].0..=r[2].1 { for y in r[1].0..=r[1].1 { for x in r[0].0..=r[0].1 {
            let s = self.slot([x, y, z]);
            self.enqueue(s, Queued::Dirty); n += 1;
        }}}
        n
    }
    fn take_queued(&mut self, index: usize, n: usize, out: &mut Vec<BatchItem>) -> usize {
        let mut taken = 0;
        while taken < n {
            let Some(slot) = self.queue.pop_front() else { break };
            let slot = slot as usize;
            let why = std::mem::replace(&mut self.queued[slot], Queued::No);
            let Some(cell) = self.cell(slot) else { continue };
            let hysteresis = if why == Queued::Dirty { DIRTY_HYSTERESIS } else { 0.0 };
            self.stamp[slot] = self.frame;
            out.push(BatchItem { cascade: index, slot, cell, hysteresis });
            taken += 1;
        }
        taken
    }
    fn take_refresh(&mut self, index: usize, n: usize, out: &mut Vec<BatchItem>) -> usize {
        let count = self.count();
        let mut taken = 0;
        for _ in 0..n.min(count) {
            let slot = refresh_slot(self.cursor, count);
            self.cursor = (self.cursor + 1) % count;
            if self.queued[slot] != Queued::No || self.stamp[slot] == self.frame { continue; }
            self.stamp[slot] = self.frame;
            let Some(cell) = self.cell(slot) else { break };
            out.push(BatchItem { cascade: index, slot, cell, hysteresis: REFRESH_HYSTERESIS });
            taken += 1;
        }
        taken
    }
}

/// Shares of the budget: the finest cascade covers what the eye sees.
fn shares(budget: usize, cascades: usize) -> Vec<usize> {
    let weights: Vec<usize> = (0..cascades).map(|i| if i == 0 { 2 } else { 1 }).collect();
    let total: usize = weights.iter().sum();
    let mut s: Vec<usize> = weights.iter().map(|w| budget * w / total).collect();
    let rest = budget - s.iter().sum::<usize>();
    if let Some(first) = s.first_mut() { *first += rest; }
    s
}

/// This frame's probe updates, at most `budget` of them.
pub(crate) fn schedule(cascades: &mut [Cascade], budget: usize) -> Vec<BatchItem> {
    let mut out = Vec::with_capacity(budget);
    let ready: Vec<usize> = (0..cascades.len()).filter(|&i| cascades[i].origin.is_some()).collect();
    if ready.is_empty() || budget == 0 { return out; }
    for &i in &ready { cascades[i].frame = cascades[i].frame.wrapping_add(1).max(1); }
    // Queued work first; a cascade with no queue lends its share to the
    // others' queues before anything is spent on refresh.
    let mut left = budget;
    loop {
        let busy: Vec<usize> = ready.iter().copied().filter(|&i| cascades[i].pending() > 0).collect();
        if left == 0 || busy.is_empty() { break; }
        let s = shares(left, busy.len());
        let mut taken = 0;
        for (k, &i) in busy.iter().enumerate() { taken += cascades[i].take_queued(i, s[k], &mut out); }
        if taken == 0 { break; }
        left -= taken;
    }
    let s = shares(left, ready.len());
    for (k, &i) in ready.iter().enumerate() { cascades[i].take_refresh(i, s[k], &mut out); }
    out
}
