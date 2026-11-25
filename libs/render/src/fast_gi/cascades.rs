//! Probe cascades and the per-frame update schedule (UI thread, O(budget)).
//!
//! Each cascade is a camera-centred lattice (`grid` probes at `spacing`)
//! stored TOROIDALLY: world cell `c` lives in slot `c mod grid`, so moving
//! the window only re-targets the slots of the slab it newly covers. Those
//! slots are queued for tracing. Kept probes retain their history. Those
//! still needed refresh it when their voxel level or a coarser one moves.
//! A re-targeted slot reads as untraced once the scatter has drawn its info
//! clear; until then it still holds the probe of the cell it left. The scatter
//! draws after the producers, so a frame with clears pending issues no probes
//! ([`schedule_after_clears`]): no trace reads, and no probe keeps, that light.
//!
//! Schedule, per frame, `budget` probes: queued work first (new slab
//! probes, probes near an edit), then a round-robin refresh, both split
//! 2:1:1 between the fine and coarser cascades.
use std::collections::VecDeque;

/// Fraction of history retained on a warm refresh.
pub(crate) const REFRESH_HYSTERESIS: f32 = 0.85;
/// Fraction of history retained on the first issue near an edit.
pub(crate) const DIRTY_HYSTERESIS: f32 = 0.2;
/// Fraction of history retained on a cold transport issue.
const COLD_HYSTERESIS: f32 = 0.5;
/// Target remaining fraction of the initial transport error for a cold probe.
const COLD_EPSILON: f64 = 0.02;
/// Target remaining fraction of the initial transport error for a warm probe.
const WARM_EPSILON: f64 = 0.05;
/// Additional rotated ray sets averaged with the final cold transport estimate.
const NOISE_ISSUES: u16 = 6;
/// A first trace followed only by the rotated ray sets.
const DIRECT_ISSUES: u16 = 1 + NOISE_ISSUES;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind { Cold, Warm, NearEdit }

#[derive(Clone, Copy)]
struct IssueCounts {
    cold: u16,
    warm: u16,
    retrace: u16,
}

impl IssueCounts {
    fn new(config: super::GiConfig) -> Self {
        // Relight caps voxel albedo at 0.9 before multiplying bounce and feedback.
        let rho = 0.9 * f64::from(config.bounce) * f64::from(config.feedback);
        let transport = |history: f32, epsilon: f64, rho: f64| {
            let h = f64::from(history);
            (epsilon.ln() / (h + (1.0 - h) * rho).ln()).ceil() as u16
        };
        Self {
            cold: transport(COLD_HYSTERESIS, COLD_EPSILON, rho).saturating_add(DIRECT_ISSUES),
            warm: transport(REFRESH_HYSTERESIS, WARM_EPSILON, rho),
            retrace: transport(REFRESH_HYSTERESIS, WARM_EPSILON, 0.0),
        }
    }
}

#[derive(Clone, Copy)]
struct Probe {
    kind: Kind,
    remaining: u16,
    needed: bool,
    queued: bool,
    /// Last frame issued, or zero if this cell has never been traced.
    stamp: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BatchItem {
    pub cascade: usize,
    pub slot: usize,
    pub cell: [i32; 3],
    pub hysteresis: f32,
}

pub(crate) struct Cascade {
    pub grid: [usize; 3],
    pub spacing: f32,
    /// Min cell of the window; None until the first voxels arrived.
    pub origin: Option<[i32; 3]>,
    probes: Vec<Probe>,
    counts: IssueCounts,
    owed: usize,
    queue: VecDeque<u32>,
    cursor: usize,
    /// The refresh's step through the slots (see [`refresh_stride`]).
    stride: usize,
    frame: u32,
    #[cfg(test)]
    pub(super) refresh_visits: usize,
}

/// The round-robin refresh's step: slot `(step * stride) % count` visits every slot once per
/// `count` steps (stride and count coprime), spread through the volume so a refresh wave never
/// sweeps visibly through space.
fn refresh_stride(count: usize) -> usize {
    let mut s = (count * 5 / 8) | 1;
    while gcd(s, count) != 1 { s += 2; }
    s
}
fn gcd(a: usize, b: usize) -> usize { if b == 0 { a } else { gcd(b, a % b) } }

impl Cascade {
    pub fn new(config: super::GiConfig, level: usize) -> Self {
        let count = config.probe_count();
        let probe = Probe { kind: Kind::Cold, remaining: 0, needed: !config.settle, queued: false, stamp: 0 };
        Self {
            grid: config.grid, spacing: config.cascade_spacing(level), origin: None,
            probes: vec![probe; count], counts: IssueCounts::new(config), owed: 0,
            queue: VecDeque::with_capacity(count), cursor: 0, stride: refresh_stride(count), frame: 0,
            #[cfg(test)]
            refresh_visits: 0,
        }
    }

    pub fn set_transport(&mut self, config: super::GiConfig) {
        self.counts = IssueCounts::new(config);
        self.restart();
    }

    /// Apply the mask at its published origin and count newly required slots.
    pub(super) fn set_needed(&mut self, origin: [i32; 3], local: &[bool]) -> usize {
        let (gx, gy) = (self.grid[0], self.grid[1]);
        let mut prune = false;
        let mut armed = 0;
        for (i, &now) in local.iter().enumerate() {
            let (x, y, z) = (i % gx, (i / gx) % gy, i / (gx * gy));
            let slot = self.slot([origin[0] + x as i32, origin[1] + y as i32, origin[2] + z as i32]);
            let probe = &mut self.probes[slot];
            if probe.needed == now { continue; }
            probe.needed = now;
            if now {
                self.arm(slot, Kind::Warm, self.counts.cold);
                armed += 1;
            } else {
                self.owed -= usize::from(probe.remaining > 0);
                probe.remaining = 0;
                prune |= probe.queued;
                probe.queued = false;
            }
        }
        if prune { self.queue.retain(|&slot| self.probes[slot as usize].queued); }
        armed
    }

    /// Renew each required slot once, preserving pending first/near-edit traces.
    pub fn restart(&mut self) {
        if self.origin.is_some() {
            for slot in 0..self.count() { self.arm(slot, Kind::Warm, self.counts.cold); }
        }
    }

    /// Refresh the direct light of kept probes whose rays can see a moved voxel window.
    fn retrace_kept(&mut self) -> usize {
        let mut armed = 0;
        for probe in &mut self.probes {
            if !probe.needed || probe.stamp == 0 || probe.queued || probe.remaining >= self.counts.retrace { continue; }
            self.owed += usize::from(probe.remaining == 0);
            probe.kind = Kind::Warm;
            probe.remaining = self.counts.retrace;
            armed += 1;
        }
        armed
    }

    pub fn count(&self) -> usize { self.probes.len() }
    pub fn pending(&self) -> usize { self.queue.len() }
    pub fn owed(&self) -> usize { self.owed }
    #[cfg(test)]
    pub(super) fn remaining(&self, slot: usize) -> u16 { self.probes[slot].remaining }
    pub fn full_sweep_issued(&self) -> bool {
        self.origin.is_some() && self.pending() == 0 && self.probes.iter().all(|p| p.stamp != 0)
    }
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
    fn arm(&mut self, slot: usize, kind: Kind, cold_issues: u16) {
        let probe = &mut self.probes[slot];
        if !probe.needed { return; }
        if kind == Kind::Warm && probe.stamp != 0 && !probe.queued && probe.remaining > 0
            && matches!(probe.kind, Kind::Cold | Kind::NearEdit)
        {
            // Keep the first trace and restart transport and noise for the new inputs.
            probe.kind = Kind::Cold;
            probe.remaining = cold_issues.saturating_sub(1);
            return;
        }
        self.owed += usize::from(probe.remaining == 0);
        probe.kind = if probe.stamp == 0 { Kind::Cold }
            else if probe.queued && kind == Kind::Warm { probe.kind }
            else { kind };
        probe.remaining = match probe.kind {
            Kind::Warm => self.counts.warm,
            Kind::Cold | Kind::NearEdit => cold_issues,
        };
        if probe.kind != Kind::Warm && !probe.queued {
            self.queue.push_back(slot as u32);
            probe.queued = true;
        }
    }
    #[cfg(test)]
    pub fn scroll(&mut self, origin: [i32; 3]) -> Vec<usize> {
        self.scroll_with_count(origin, self.counts.cold)
    }
    /// Return all slots to clear and give each required new cell the selected countdown.
    fn scroll_with_count(&mut self, origin: [i32; 3], exposed_issues: u16) -> Vec<usize> {
        let old = self.origin.replace(origin);
        let mut exposed = Vec::new();
        if old == Some(origin) { return exposed; }
        for slot in 0..self.count() {
            let c = self.cell(slot).unwrap();
            let kept = old.is_some_and(|o| (0..3).all(|a| c[a] >= o[a] && c[a] < o[a] + self.grid[a] as i32));
            if !kept {
                self.probes[slot].stamp = 0;
                self.arm(slot, Kind::Cold, exposed_issues);
                exposed.push(slot);
            }
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
            self.arm(s, Kind::NearEdit, self.counts.cold); n += 1;
        }}}
        n
    }
    fn issue(&mut self, index: usize, slot: usize, out: &mut Vec<BatchItem>) {
        let cell = self.cell(slot).expect("only resident cascades issue probes");
        let probe = &mut self.probes[slot];
        let hysteresis = if probe.queued {
            probe.queued = false;
            if probe.kind == Kind::NearEdit { DIRTY_HYSTERESIS } else { 0.0 }
        } else if probe.kind == Kind::Warm {
            REFRESH_HYSTERESIS
        } else if probe.remaining > NOISE_ISSUES {
            COLD_HYSTERESIS
        } else {
            let samples = f32::from(1 + NOISE_ISSUES - probe.remaining);
            samples / (samples + 1.0)
        };
        probe.stamp = self.frame;
        probe.remaining = probe.remaining.saturating_sub(1);
        self.owed -= usize::from(probe.remaining == 0);
        out.push(BatchItem { cascade: index, slot, cell, hysteresis });
    }

    fn take_queued(&mut self, index: usize, n: usize, out: &mut Vec<BatchItem>) -> usize {
        let taken = n.min(self.queue.len());
        for _ in 0..taken {
            let slot = self.queue.pop_front().unwrap() as usize;
            self.issue(index, slot, out);
        }
        taken
    }

    fn take_refresh(&mut self, index: usize, n: usize, out: &mut Vec<BatchItem>) -> usize {
        let count = self.count();
        let mut taken = 0;
        for _ in 0..count {
            if taken == n || self.owed == 0 { break; }
            #[cfg(test)]
            { self.refresh_visits += 1; }
            let slot = (self.cursor * self.stride) % count;
            self.cursor = (self.cursor + 1) % count;
            let probe = &self.probes[slot];
            if probe.remaining == 0 || probe.queued || probe.stamp == self.frame { continue; }
            self.issue(index, slot, out);
            taken += 1;
        }
        taken
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct WindowCommit {
    pub scrolled: usize,
    /// Kept probes whose countdown was raised for a moved voxel window.
    pub retraced: usize,
    pub dirty: usize,
    pub new_cells: usize,
    pub first: bool,
    pub moved: bool,
    pub recentred: bool,
    pub edited: bool,
}

/// Adopt one accepted voxel publication. Probes trace their own voxel level and
/// all coarser levels, so a move refreshes kept probes of that level and finer.
pub(super) fn commit_windows(
    cascades: &mut [Cascade], committed: &mut [Option<[i32; 3]>], windows: &[Option<[i32; 3]>],
    update: &super::VoxelUpdate, config: super::GiConfig, clears: &mut Vec<(usize, usize)>,
) -> WindowCommit {
    let first = committed.iter().all(Option::is_none);
    let mut commit = WindowCommit {
        first, moved: !first && *committed != *windows, recentred: update.recentred, edited: !first && update.edited,
        ..Default::default()
    };
    let mut coarser_settled = false;
    let mut coarser_moved = false;
    for (level, cascade) in cascades.iter_mut().enumerate().rev() {
        let level_moved = committed[level].is_some() && committed[level] != windows[level];
        let origin = windows[level].map(|w| w.map(|v| v / 4));
        // The incoming mask can require a previously unused slot. Apply it before
        // scrolling so every required exposed slot gets this move's countdown.
        let mask = update.needed.get(level).and_then(Option::as_ref);
        if let (Some(mask_origin), Some(mask)) = (origin.or(cascade.origin), mask) {
            commit.new_cells += cascade.set_needed(mask_origin, mask);
        }
        let exposed = origin.map(|o| {
            let issues = if committed[level].is_some() && coarser_settled { DIRECT_ISSUES } else { cascade.counts.cold };
            cascade.scroll_with_count(o, issues)
        }).unwrap_or_default();
        if commit.edited {
            for (lo, hi) in &update.dirty {
                commit.dirty += cascade.invalidate([lo.x, lo.y, lo.z], [hi.x, hi.y, hi.z], config.home);
            }
            cascade.restart();
        } else if level_moved || coarser_moved {
            commit.retraced += cascade.retrace_kept();
        }
        coarser_moved |= level_moved;
        // A move can expose slots even when the mask requires none of them.
        coarser_settled = origin.is_some() && exposed.is_empty() && cascade.owed() == 0;
        commit.scrolled += exposed.len();
        clears.extend(exposed.into_iter().map(|slot| (level, slot)));
    }
    committed.copy_from_slice(windows);
    commit
}

/// Shares of the budget: the finest cascade covers what the eye sees.
fn shares(budget: usize, cascades: usize, rotate_remainder: bool, frame: u32) -> [usize; super::MAX_CASCADES] {
    let mut s = [0; super::MAX_CASCADES];
    let total = cascades + 1;
    for (i, share) in s[..cascades].iter_mut().enumerate() {
        *share = budget * if i == 0 { 2 } else { 1 } / total;
    }
    let remainder = budget - s.iter().sum::<usize>();
    if rotate_remainder {
        let first = (frame - 1) as usize % cascades;
        for offset in 0..remainder { s[(first + offset) % cascades] += 1; }
    } else {
        // Preserve ordinary batches, including a partial share after an edit.
        s[0] += remainder;
    }
    s
}

/// Largest batch whose 2:1:1 refresh shares fit without repeating a slot.
/// Queued work may borrow those shares, but never raises this ceiling.
pub(crate) fn batch_capacity(count: usize, cascades: usize) -> usize {
    count * (cascades + 1) / 2
}

/// This frame's updates, or none while a commit's slot clears are still to be
/// drawn. The producers run before the scatter that draws those clears, so a
/// probe issued now would gather the stale probes of the cells its exposed
/// neighbours left (light from a window width away) into its first, unweighted
/// ray set. The next frame schedules the same work against cleared slots.
pub(crate) fn schedule_after_clears(cascades: &mut [Cascade], budget: usize, favor_fine_remainder: bool, clears_pending: bool,
    out: &mut Vec<BatchItem>) {
    if clears_pending {
        out.clear();
        return;
    }
    schedule(cascades, budget, favor_fine_remainder, out);
}

/// This frame's updates, in the existing queue/stride order. Storage belongs
/// to the caller; no catch-up work or allocation on an ordinary frame.
pub(crate) fn schedule(cascades: &mut [Cascade], budget: usize, favor_fine_remainder: bool, out: &mut Vec<BatchItem>) {
    out.clear();
    let mut ready = [0; super::MAX_CASCADES];
    let mut nready = 0;
    for (i, cascade) in cascades.iter_mut().enumerate() {
        if cascade.owed > 0 {
            ready[nready] = i;
            nready += 1;
            cascade.frame = cascade.frame.wrapping_add(1).max(1);
        }
    }
    let ready = &ready[..nready];
    if ready.is_empty() || budget == 0 { return; }
    let frame = cascades[ready[0]].frame;
    // Full-field coverage favors the fine cascade for spare rows, as before.
    // Always rotate when a busy cascade would otherwise starve.
    let rotate_remainder = |busy: usize| !favor_fine_remainder || budget <= busy;
    let mut left = budget;
    loop {
        let mut busy = [0; super::MAX_CASCADES];
        let mut nbusy = 0;
        for &i in ready {
            if cascades[i].pending() > 0 {
                busy[nbusy] = i;
                nbusy += 1;
            }
        }
        if left == 0 || nbusy == 0 { break; }
        let s = shares(left, nbusy, rotate_remainder(nbusy), frame);
        let mut taken = 0;
        for (k, &i) in busy[..nbusy].iter().enumerate() {
            taken += cascades[i].take_queued(i, s[k], out);
        }
        if taken == 0 { break; }
        left -= taken;
    }
    // Look past already queued/stamped slots, retaining the same stride.
    // An edit must not leave a short frame followed by a full-frame burst.
    let mut exhausted = [false; super::MAX_CASCADES];
    while left > 0 {
        let mut refresh = [0; super::MAX_CASCADES];
        let mut nrefresh = 0;
        for &i in ready {
            if !exhausted[i] && cascades[i].owed > 0 { refresh[nrefresh] = i; nrefresh += 1; }
        }
        if nrefresh == 0 { break; }
        let s = shares(left, nrefresh, rotate_remainder(nrefresh), frame);
        let mut taken = 0;
        for (k, &i) in refresh[..nrefresh].iter().enumerate() {
            let n = cascades[i].take_refresh(i, s[k], out);
            if n < s[k] { exhausted[i] = true; }
            taken += n;
        }
        left -= taken;
    }
}
