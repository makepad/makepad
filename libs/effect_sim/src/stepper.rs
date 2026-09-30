//! The stepper (KERNELS.md §3.6): one box3d world and any number of
//! stateful kernel sims on one fixed, exact time grid, seeded, with
//! complete in-memory checkpoints and seek.
//!
//! **A step.** Going from step `s` to `s + 1`: kinematic bodies get their
//! pose at `(s + 1)·dt` from the host's [`Drive`]; the world steps; its
//! hits become contact records and its sleeps events (box3d clears its
//! event arrays every step, so the stepper keeps them for `retain` seconds
//! of sim time); then, at `s + 1`, bodies whose life ran out expire, the
//! spawns due run (a kernel writing `Body` records, or records in place;
//! a record with a `delay` waits in the pending queue), the sims due run
//! their update kernel, and the step's poses are recorded. State `s` is
//! the world after all of that.
//!
//! **Seek.** `seek(t)` makes steps `step(t) - 1 ..= step(t) + 1` available
//! (a view interpolates between `step(t)` and the step after, and gives the
//! pose one step earlier for motion vectors): a short way forward steps
//! from where it is, anything else restores the newest valid checkpoint at
//! or before `step(t) - 1` and steps forward. A checkpoint holds
//! everything the future depends on (§3.6.2): the world's snapshot image
//! and geometry registry, the step index, the spawn cursor and pending
//! queue, the retained contact and event history, every sim's state, the
//! stable id table (body id to document id and group, render fields,
//! expiry), and a version stamp. So a seek gives what playing up to the
//! same time gives, bit for bit, on the machine that renders (the contract
//! is same-machine reproducibility; no state crosses machines or builds).
//!
//! **Threads.** A stepper is one job: the host runs it on a worker (or in
//! its eval, for locked time). Kernels run inline on the caller.

use crate::desc::*;
use crate::layouts::*;
use crate::time::{SimTime, StepDt};
use crate::world::{bounding_radius, Kind, PhysWorld};
use makepad_box3d::id::BodyId;
use makepad_render_kernels::compute::kernel::Layout;
use makepad_render_kernels::compute::sched::InlineExecutor;
use makepad_render_kernels::pipeline::Count;
use makepad_render_kernels::Pipeline;
use std::collections::{BTreeSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The host's pose for kinematic bodies at a sim time: evaluated from the
/// document (pure and bounded), so the same time gives the same pose.
pub trait Drive {
    /// Position and rotation (x, y, z, w) of kinematic body `key` at `t`.
    fn pose(&mut self, key: &str, t: SimTime) -> Option<([f32; 3], [f32; 4])>;
}

/// No kinematic bodies (or keep them where they are).
pub struct NoDrive;

impl Drive for NoDrive {
    fn pose(&mut self, _key: &str, _t: SimTime) -> Option<([f32; 3], [f32; 4])> {
        None
    }
}

/// 64-bit FNV-1a over words.
#[derive(Clone, Copy)]
pub struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv {
    pub fn word(&mut self, w: u32) {
        for b in w.to_le_bytes() {
            self.0 ^= b as u64;
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    pub fn u64(&mut self, v: u64) {
        self.word(v as u32);
        self.word((v >> 32) as u32);
    }
    pub fn f32s(&mut self, v: &[f32]) {
        for x in v {
            self.word(x.to_bits());
        }
    }
    pub fn words(&mut self, v: &[u32]) {
        for x in v {
            self.word(*x);
        }
    }
    pub fn bytes(&mut self, v: &[u8]) {
        for b in v {
            self.0 ^= *b as u64;
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    pub fn finish(self) -> u64 {
        self.0
    }
}

fn mix32(a: u64, b: u64) -> u32 {
    let mut x = a ^ b.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (x ^ (x >> 31)) as u32
}

/// The version stamp of a description (§3.6.2 item 6): the document hash,
/// every kernel's program hash, the input revisions, the step grid, seed
/// and the world's precision. A checkpoint with another stamp is never
/// restored.
pub fn stamp_of(desc: &PhysicsDesc) -> u64 {
    let mut h = Fnv::default();
    h.u64(desc.doc_hash);
    h.u64(desc.revisions);
    h.u64(desc.seed);
    h.word(desc.dt.num());
    h.word(desc.dt.den());
    h.word(desc.substeps);
    h.word(std::mem::size_of::<makepad_box3d::math_functions::Pos>() as u32);
    for s in &desc.spawns {
        if let SpawnFrom::Kernel(k) = &s.from {
            h.u64(k.program);
        }
    }
    for s in &desc.sims {
        h.u64(s.init.program);
        h.u64(s.update.program);
    }
    h.finish()
}

#[derive(Clone, Debug)]
struct Entry {
    body: Option<BodyId>,
    group: u32,
    born: u64,
    /// The step it expires at (u64::MAX: never).
    expires: u64,
    color: [f32; 4],
    scale: [f32; 3],
    seed: u32,
    radius: f32,
}

/// The stable id table: a body's document id is its index here (spawn
/// order), for its whole life and across seeks.
#[derive(Clone, Debug, Default)]
struct Registry {
    entries: Vec<Entry>,
    groups: Vec<String>,
    /// Live ids, ascending.
    live: Vec<u32>,
    /// Kinematic bodies: (id, key).
    kinematic: Vec<(u32, String)>,
    /// box3d body index (`index1 - 1`) to stable id (box3d reuses indices
    /// of destroyed bodies; the entry is rewritten when it does).
    by_body: Vec<u32>,
}

impl Registry {
    fn group(&mut self, name: &str) -> u32 {
        match self.groups.iter().position(|g| g == name) {
            Some(i) => i as u32,
            None => {
                self.groups.push(name.to_string());
                (self.groups.len() - 1) as u32
            }
        }
    }
    fn id_of(&self, body: BodyId) -> Option<u32> {
        let id = *self.by_body.get((body.index1 - 1) as usize)?;
        (self.entries.get(id as usize)?.body == Some(body)).then_some(id)
    }
    fn find_group(&self, name: &str) -> Option<u32> {
        self.groups.iter().position(|g| g == name).map(|i| i as u32)
    }
    fn bytes(&self) -> usize {
        self.entries.len() * std::mem::size_of::<Entry>() + (self.live.len() + self.by_body.len()) * 4 + self.groups.iter().map(|g| g.len() + 24).sum::<usize>()
    }
}

/// A record that waits for its step (`delay`).
#[derive(Clone, Debug)]
struct Pending {
    step: u64,
    order: u64,
    group: u32,
    shape: Shape,
    fixed: bool,
    size: [f32; 3],
    record: BodyRecord,
}

/// Contacts and events kept past box3d's per-step arrays, step-ordered.
#[derive(Clone, Debug, Default)]
struct History {
    contacts: VecDeque<ContactRecord>,
    events: VecDeque<EventRecord>,
}

impl History {
    fn prune(&mut self, keep_from: u64) {
        while self.contacts.front().is_some_and(|c| c.step < keep_from) {
            self.contacts.pop_front();
        }
        while self.events.front().is_some_and(|e| e.step < keep_from) {
            self.events.pop_front();
        }
    }
    fn bytes(&self) -> usize {
        self.contacts.len() * std::mem::size_of::<ContactRecord>() + self.events.len() * std::mem::size_of::<EventRecord>()
    }
}

/// The poses of one step (a view interpolates between two of these).
#[derive(Clone, Debug, Default)]
struct Frame {
    step: u64,
    ids: Vec<u32>,
    poses: Vec<[f32; 7]>,
    speeds: Vec<f32>,
    sims: Vec<Arc<Vec<u32>>>,
}

impl Frame {
    fn pose_of(&self, id: u32) -> Option<(usize, [f32; 7])> {
        self.ids.binary_search(&id).ok().map(|k| (k, self.poses[k]))
    }
}

/// Everything the future depends on, at one step.
#[derive(Clone)]
struct Checkpoint {
    step: u64,
    stamp: u64,
    image: Arc<Vec<u8>>,
    geometry: Arc<Vec<u8>>,
    reg: Registry,
    cursor: usize,
    pending: Vec<Pending>,
    next_order: u64,
    history: History,
    sims: Vec<Arc<Vec<u32>>>,
    bytes: usize,
}

/// Checkpoints every 0.5 s of sim time within a byte budget; under
/// pressure every 2 s, then 5 s, then the oldest go; the initial state
/// always stays.
struct Ring {
    cps: Vec<Checkpoint>,
    intervals: [u64; 3],
    level: usize,
    budget: usize,
    bytes: usize,
}

impl Ring {
    fn new(dt: StepDt, budget: usize) -> Self {
        let every = |n: i64, d: u64| dt.steps_in(SimTime::from_ratio(n, d)).max(1);
        Self { cps: Vec::new(), intervals: [every(1, 2), every(2, 1), every(5, 1)], level: 0, budget, bytes: 0 }
    }
    fn wants(&self, step: u64) -> bool {
        step % self.intervals[self.level] == 0 && self.cps.binary_search_by_key(&step, |c| c.step).is_err()
    }
    fn insert(&mut self, cp: Checkpoint) -> usize {
        let at = self.cps.binary_search_by_key(&cp.step, |c| c.step).unwrap_or_else(|e| e);
        self.bytes += cp.bytes;
        self.cps.insert(at, cp);
        let mut dropped = 0;
        while self.bytes > self.budget && self.cps.len() > 1 {
            if self.level < 2 {
                self.level += 1;
                let every = self.intervals[self.level];
                let before = self.cps.len();
                let mut bytes = 0;
                self.cps.retain(|c| {
                    let keep = c.step == 0 || c.step % every == 0;
                    if keep {
                        bytes += c.bytes;
                    }
                    keep
                });
                self.bytes = bytes;
                dropped += before - self.cps.len();
            } else {
                // The oldest after the initial state.
                let k = if self.cps[0].step == 0 { 1 } else { 0 };
                if k >= self.cps.len() {
                    break;
                }
                self.bytes -= self.cps[k].bytes;
                self.cps.remove(k);
                dropped += 1;
            }
        }
        dropped
    }
}

/// Counters for the host's stats overlay.
#[derive(Clone, Debug, Default)]
pub struct StepperStats {
    pub steps: u64,
    /// Checkpoints restored by seeks.
    pub restores: u64,
    /// Checkpoints thrown away because their stamp was not the document's.
    pub stale_discarded: u64,
    /// Checkpoints thinned out under the byte budget.
    pub thinned: u64,
    pub checkpoints: usize,
    pub checkpoint_bytes: usize,
    pub bodies_live: usize,
    pub bodies_made: u64,
    /// Summed and worst step time (world step + collection + spawns +
    /// sims + pose extraction), ns.
    pub step_ns: u64,
    pub step_ns_max: u64,
    /// Pose extraction (the bridge's CPU copy) of the step times, ns.
    pub extract_ns: u64,
    /// Realtime: views that could not reach their time (the last state was
    /// shown instead).
    pub late: u64,
}

/// Where [`Stepper::advance_to`] got to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Progress {
    /// `t` is viewable.
    Ready,
    /// Out of budget at this head step; view the last state.
    Behind { head: u64 },
}

pub struct Stepper {
    desc: Arc<PhysicsDesc>,
    stamp: u64,
    world: PhysWorld,
    head: u64,
    reg: Registry,
    /// Scheduled spawns by (step, declaration index).
    schedule: Vec<(u64, usize)>,
    cursor: usize,
    pending: Vec<Pending>,
    next_order: u64,
    history: History,
    sims: Vec<Arc<Vec<u32>>>,
    recent: VecDeque<Frame>,
    spare: Vec<Frame>,
    keep: usize,
    ring: Ring,
    errors: BTreeSet<String>,
    stats: StepperStats,
    hits: Vec<([f32; 3], [f32; 3], f32, BodyId, BodyId, f32)>,
    slept: Vec<BodyId>,
}

impl Stepper {
    /// A stepper at its initial state (step 0: declared bodies, spawns at
    /// 0, sims initialised), with its first checkpoint.
    pub fn new(desc: PhysicsDesc, drive: &mut dyn Drive) -> Self {
        let desc = Arc::new(desc);
        let mut schedule: Vec<(u64, usize)> = desc.spawns.iter().enumerate().map(|(i, s)| (desc.dt.step_ceil(s.at), i)).collect();
        schedule.sort();
        // Frames kept for views behind the head: a tenth of a second (a
        // frame's shutter and a little scrub) plus the interpolation pair.
        let keep = desc.dt.steps_in(SimTime::from_ratio(1, 10)) as usize + 3;
        let mut st = Self {
            stamp: stamp_of(&desc),
            world: PhysWorld::new(&desc),
            head: 0,
            reg: Registry::default(),
            schedule,
            cursor: 0,
            pending: Vec::new(),
            next_order: 0,
            history: History::default(),
            sims: Vec::new(),
            recent: VecDeque::new(),
            spare: Vec::new(),
            keep,
            ring: Ring::new(desc.dt, desc.checkpoint_bytes),
            errors: BTreeSet::new(),
            stats: StepperStats::default(),
            hits: Vec::new(),
            slept: Vec::new(),
            desc,
        };
        st.init(drive);
        st
    }

    fn init(&mut self, drive: &mut dyn Drive) {
        let desc = self.desc.clone();
        for b in &desc.bodies {
            let group = if b.group.is_empty() { &b.name } else { &b.group };
            let g = self.reg.group(group);
            let mut rec = b.record;
            let kind = match &b.motion {
                Motion::Fixed => Kind::Fixed,
                Motion::Dynamic => Kind::Dynamic,
                Motion::Kinematic { key } => {
                    if let Some((p, q)) = drive.pose(key, SimTime::ZERO) {
                        rec.pos = p;
                        rec.rot = q;
                    }
                    Kind::Kinematic
                }
            };
            if let Err(e) = rec.validate(0) {
                self.errors.insert(format!("body `{}`: {}", b.name, e));
                continue;
            }
            let size = if rec.size.iter().any(|s| *s > 0.0) { rec.size } else { [1.0; 3] };
            if let Some(id) = self.make(g, &b.shape, kind, size, &rec, 0) {
                if let Motion::Kinematic { key } = &b.motion {
                    self.reg.kinematic.push((id, key.clone()));
                }
            }
        }
        for s in &desc.sims {
            let state = match self.run_call(&s.init, s.count as usize, 0, None) {
                Ok((words, _)) if words.len() >= s.count as usize * s.state.stride as usize => words,
                Ok(_) => {
                    self.errors.insert(format!("sim `{}`: init wrote no `{}` state", s.name, s.init.buffer));
                    vec![0; s.count as usize * s.state.stride as usize]
                }
                Err(e) => {
                    self.errors.insert(format!("sim `{}` init: {}", s.name, e));
                    vec![0; s.count as usize * s.state.stride as usize]
                }
            };
            self.sims.push(Arc::new(state));
        }
        self.spawns_at(0);
        self.record(0);
        self.maybe_checkpoint();
    }

    pub fn desc(&self) -> &PhysicsDesc {
        &self.desc
    }

    pub fn dt(&self) -> StepDt {
        self.desc.dt
    }

    pub fn stamp(&self) -> u64 {
        self.stamp
    }

    /// The step the world is at.
    pub fn head(&self) -> u64 {
        self.head
    }

    pub fn stats(&self) -> StepperStats {
        let mut s = self.stats.clone();
        s.checkpoints = self.ring.cps.len();
        s.checkpoint_bytes = self.ring.bytes;
        s.bodies_live = self.reg.live.len();
        s
    }

    /// Problems met so far (refused records, kernel errors), each once.
    pub fn errors(&self) -> impl Iterator<Item = &String> {
        self.errors.iter()
    }

    /// A new description (a document edit). The same stamp keeps
    /// everything; another one discards every checkpoint (none is ever
    /// restored under a stamp it was not made with) and restarts at the
    /// initial state.
    pub fn reload(&mut self, desc: PhysicsDesc, drive: &mut dyn Drive) -> bool {
        if stamp_of(&desc) == self.stamp {
            return false;
        }
        let stale = self.ring.cps.len() as u64;
        let mut stats = std::mem::take(&mut self.stats);
        *self = Stepper::new(desc, drive);
        stats.stale_discarded += stale;
        stats.steps += self.stats.steps;
        self.stats = stats;
        true
    }

    // -----------------------------------------------------------------
    // Bodies
    // -----------------------------------------------------------------

    fn make(&mut self, group: u32, shape: &Shape, kind: Kind, size: [f32; 3], rec: &BodyRecord, step: u64) -> Option<u32> {
        let id = self.reg.entries.len() as u32;
        let desc = self.desc.clone();
        match self.world.create(&desc, shape, kind, size, rec) {
            Ok(body) => {
                let k = (body.index1 - 1) as usize;
                if self.reg.by_body.len() <= k {
                    self.reg.by_body.resize(k + 1, u32::MAX);
                }
                self.reg.by_body[k] = id;
                let life = if rec.life > 0.0 { desc.dt.steps_in_f32(rec.life).max(1) } else { 0 };
                self.reg.entries.push(Entry {
                    body: Some(body),
                    group,
                    born: step,
                    expires: if life > 0 { step + life } else { u64::MAX },
                    color: if rec.color == [0.0; 4] { [1.0; 4] } else { rec.color },
                    scale: if rec.scale == [0.0; 3] { [1.0; 3] } else { rec.scale },
                    seed: mix32(desc.seed, id as u64),
                    radius: bounding_radius(shape, size),
                });
                self.reg.live.push(id);
                self.stats.bodies_made += 1;
                self.history.events.push_back(EventRecord { kind: EventKind::Spawn, id, time: desc.dt.time_of(step).as_secs() as f32, step, pos: rec.pos });
                Some(id)
            }
            Err(e) => {
                self.errors.insert(format!("group `{}`: {}", self.reg.groups[group as usize], e));
                None
            }
        }
    }

    fn spawn_records(&mut self, step: u64, group: u32, shape: &Shape, fixed: bool, size: [f32; 3], words: &[u32], n: usize, what: &str) {
        for i in 0..n {
            let Some(w) = words.get(i * BODY_WORDS..(i + 1) * BODY_WORDS) else { break };
            let rec = BodyRecord::from_words(w);
            if let Err(e) = rec.validate(i) {
                self.errors.insert(format!("{what}: {e}"));
                continue;
            }
            let size = std::array::from_fn(|k| if rec.size[k] > 0.0 { rec.size[k] } else { size[k] });
            if rec.delay > 0.0 {
                let at = step + self.desc.dt.steps_in_f32(rec.delay);
                let p = Pending { step: at, order: self.next_order, group, shape: shape.clone(), fixed, size, record: rec };
                self.next_order += 1;
                let k = self.pending.partition_point(|q| (q.step, q.order) < (p.step, p.order));
                self.pending.insert(k, p);
            } else {
                self.make(group, shape, if fixed { Kind::Fixed } else { Kind::Dynamic }, size, &rec, step);
            }
        }
    }

    fn spawns_at(&mut self, step: u64) {
        let desc = self.desc.clone();
        while self.cursor < self.schedule.len() && self.schedule[self.cursor].0 <= step {
            let idx = self.schedule[self.cursor].1;
            self.cursor += 1;
            let sp = &desc.spawns[idx];
            let group = self.reg.group(&sp.group);
            let what = format!("spawn {} into `{}`", idx, sp.group);
            match &sp.from {
                SpawnFrom::Records(recs) => {
                    let mut words = vec![0u32; recs.len() * BODY_WORDS];
                    for (i, r) in recs.iter().enumerate() {
                        r.to_words(&mut words[i * BODY_WORDS..(i + 1) * BODY_WORDS]);
                    }
                    self.spawn_records(step, group, &sp.shape, sp.fixed, sp.size, &words, recs.len(), &what);
                }
                SpawnFrom::Kernel(call) => match self.run_call(call, sp.count as usize, step, Some(idx as u64)) {
                    Ok((words, n)) => self.spawn_records(step, group, &sp.shape, sp.fixed, sp.size, &words, n, &what),
                    Err(e) => {
                        self.errors.insert(format!("{what}: {e}"));
                    }
                },
            }
        }
        while self.pending.first().is_some_and(|p| p.step <= step) {
            let p = self.pending.remove(0);
            self.make(p.group, &p.shape, if p.fixed { Kind::Fixed } else { Kind::Dynamic }, p.size, &p.record, step);
        }
    }

    fn expire_at(&mut self, step: u64) {
        let dt = self.desc.dt;
        let mut k = 0;
        while k < self.reg.live.len() {
            let id = self.reg.live[k];
            let e = &self.reg.entries[id as usize];
            if e.expires <= step {
                if let Some(body) = e.body {
                    let pose = self.world.pose(body);
                    self.world.destroy(body);
                    self.history.events.push_back(EventRecord { kind: EventKind::Expire, id, time: dt.time_of(step).as_secs() as f32, step, pos: [pose[0], pose[1], pose[2]] });
                }
                self.reg.entries[id as usize].body = None;
                self.reg.live.remove(k);
            } else {
                k += 1;
            }
        }
    }

    // -----------------------------------------------------------------
    // Kernels
    // -----------------------------------------------------------------

    /// Runs a kernel call at step `step`; returns its buffer's words and
    /// record count (what it emitted, or its elements).
    fn run_call(&mut self, call: &KernelCall, count: usize, step: u64, spawn: Option<u64>) -> Result<(Vec<u32>, usize), String> {
        let (words, n, _) = self.run_pipeline(call, count, step, spawn, None, None)?;
        Ok((words, n))
    }

    /// One pass: `call` over `count` elements at `step`, with its inputs
    /// bound from the running state and `prev` (a sim's previous state)
    /// when given; returns the call's buffer, its record count and the
    /// words of `also` (a second buffer, e.g. a sim's spawns) with its
    /// count.
    #[allow(clippy::type_complexity)]
    fn run_pipeline(&mut self, call: &KernelCall, count: usize, step: u64, salt: Option<u64>, prev: Option<(&str, Vec<u32>)>, also: Option<&str>) -> Result<(Vec<u32>, usize, Option<(Vec<u32>, usize)>), String> {
        let t = self.desc.dt.time_of(step);
        let mut p = Pipeline::new();
        {
            let job = p.pass(call.kernel.clone(), Count::Fixed(count));
            job.set_time(t.as_secs() as f32);
            job.set_seed(mix32(self.desc.seed, salt.unwrap_or(0x5eed)));
            for (n, v) in &call.params {
                job.set_param(n, *v);
            }
        }
        let mut counts = Vec::new();
        for (name, inp) in &call.inputs {
            let (mut words, n, stride) = self.input_words(inp, step);
            counts.push((format!("{name}_count"), n));
            if words.is_empty() {
                words = vec![0; stride];
            }
            p.set_buffer(name, words);
        }
        if let Some((name, words)) = prev {
            p.set_buffer(name, words);
        }
        if let Some(job) = p.job(0) {
            for (n, c) in &counts {
                job.set_param(n, *c as f32);
            }
        }
        p.run(&InlineExecutor, 1).map_err(|e| e.to_string())?;
        let stride = call.kernel.buffers().iter().find(|b| b.name == call.buffer).map(|b| b.stride as usize).unwrap_or(1).max(1);
        let n = p.emitted(&call.buffer);
        let words = p.take_buffer(&call.buffer).ok_or_else(|| format!("the kernel has no buffer `{}`", call.buffer))?;
        let n = n.unwrap_or(words.len() / stride);
        let extra = also.map(|name| {
            let n = p.emitted(name).unwrap_or(0);
            (p.take_buffer(name).unwrap_or_default(), n)
        });
        Ok((words, n, extra))
    }

    /// The words a kernel input reads at `step` (records, count, stride).
    fn input_words(&self, inp: &SimInput, step: u64) -> (Vec<u32>, usize, usize) {
        let t = self.desc.dt.time_of(step);
        match inp {
            SimInput::Bodies(g) => {
                let mut recs = Vec::new();
                self.live_rigid(g, step, &mut recs);
                let mut w = vec![0u32; recs.len() * RIGID_WORDS];
                for (i, r) in recs.iter().enumerate() {
                    r.to_words(&mut w[i * RIGID_WORDS..(i + 1) * RIGID_WORDS]);
                }
                (w, recs.len(), RIGID_WORDS)
            }
            SimInput::Contacts(g) => {
                let mut recs = Vec::new();
                self.contacts_in(g, step, t, &mut recs);
                let mut w = vec![0u32; recs.len() * CONTACT_WORDS];
                for (i, r) in recs.iter().enumerate() {
                    r.to_words(&mut w[i * CONTACT_WORDS..(i + 1) * CONTACT_WORDS]);
                }
                (w, recs.len(), CONTACT_WORDS)
            }
            SimInput::Events(g) => {
                let mut recs = Vec::new();
                self.events_in(g, step, t, &mut recs);
                let mut w = vec![0u32; recs.len() * EVENT_WORDS];
                for (i, r) in recs.iter().enumerate() {
                    r.to_words(&mut w[i * EVENT_WORDS..(i + 1) * EVENT_WORDS]);
                }
                (w, recs.len(), EVENT_WORDS)
            }
            SimInput::Sim(name) => match self.desc.sims.iter().position(|s| &s.name == name) {
                Some(k) => {
                    let st = self.desc.sims[k].state.stride as usize;
                    (self.sims[k].as_ref().clone(), self.desc.sims[k].count as usize, st)
                }
                None => (Vec::new(), 0, 1),
            },
        }
    }

    fn in_group(&self, g: Option<u32>, id: u32) -> bool {
        match g {
            None => true,
            Some(g) => self.reg.entries.get(id as usize).is_some_and(|e| e.group == g),
        }
    }

    /// `None` for every body (""), `Some(None)` for a group nothing joined.
    fn group_filter(&self, name: &str) -> Option<Option<u32>> {
        if name.is_empty() {
            Some(None)
        } else {
            self.reg.find_group(name).map(Some)
        }
    }

    fn contacts_in(&self, group: &str, step: u64, t: SimTime, out: &mut Vec<ContactRecord>) {
        let Some(g) = self.group_filter(group) else { return };
        let from = t.saturating_sub(self.desc.retain);
        let dt = self.desc.dt;
        for c in &self.history.contacts {
            if c.step > step {
                break;
            }
            if dt.time_of(c.step) > from && (self.in_group(g, c.a) || self.in_group(g, c.b)) {
                out.push(*c);
            }
        }
    }

    fn events_in(&self, group: &str, step: u64, t: SimTime, out: &mut Vec<EventRecord>) {
        let Some(g) = self.group_filter(group) else { return };
        let from = t.saturating_sub(self.desc.retain);
        let dt = self.desc.dt;
        for e in &self.history.events {
            if e.step > step {
                break;
            }
            if dt.time_of(e.step) > from && self.in_group(g, e.id) {
                out.push(*e);
            }
        }
    }

    fn rigid(&self, id: u32, pose: [f32; 7], prev: [f32; 7], speed: f32, t: SimTime) -> RigidRecord {
        let e = &self.reg.entries[id as usize];
        RigidRecord {
            pos: [pose[0], pose[1], pose[2]],
            rot: [pose[3], pose[4], pose[5], pose[6]],
            scale: e.scale,
            color: e.color,
            seed: e.seed,
            id,
            prev_pos: [prev[0], prev[1], prev[2]],
            prev_rot: [prev[3], prev[4], prev[5], prev[6]],
            age: (t.as_secs() - self.desc.dt.time_of(e.born).as_secs()) as f32,
            radius: e.radius,
            speed,
            group: e.group,
        }
    }

    /// Rigid records of the live world at `step` (a kernel's input; no
    /// interpolation), with the poses of the frame before as `prev`.
    fn live_rigid(&self, group: &str, step: u64, out: &mut Vec<RigidRecord>) {
        let Some(g) = self.group_filter(group) else { return };
        let t = self.desc.dt.time_of(step);
        let before = step.checked_sub(1).and_then(|s| self.frame(s));
        for &id in &self.reg.live {
            if !self.in_group(g, id) {
                continue;
            }
            let Some(body) = self.reg.entries[id as usize].body else { continue };
            let pose = self.world.pose(body);
            let prev = before.and_then(|f| f.pose_of(id)).map(|p| p.1).unwrap_or(pose);
            out.push(self.rigid(id, pose, prev, self.world.speed(body), t));
        }
    }

    fn sims_at(&mut self, step: u64) {
        let desc = self.desc.clone();
        for (k, s) in desc.sims.iter().enumerate() {
            if s.every == 0 || step % s.every as u64 != 0 {
                continue;
            }
            let prev = self.sims[k].as_ref().clone();
            let spawn_buf = s.spawn.as_ref().map(|x| x.0.as_str());
            match self.run_pipeline(&s.update, s.count as usize, step, Some(0x51_0000 + k as u64), Some((&s.prev, prev)), spawn_buf) {
                Ok((words, _, extra)) => {
                    if words.len() >= s.count as usize * s.state.stride as usize {
                        self.sims[k] = Arc::new(words);
                    } else {
                        self.errors.insert(format!("sim `{}`: update wrote no `{}` state", s.name, s.update.buffer));
                    }
                    if let (Some((words, n)), Some((_, group, shape))) = (extra, &s.spawn) {
                        let g = self.reg.group(group);
                        self.spawn_records(step, g, shape, false, [0.1; 3], &words, n, &format!("sim `{}` spawns", s.name));
                    }
                }
                Err(e) => {
                    // The state stays as it was (deterministic either way).
                    self.errors.insert(format!("sim `{}` update: {}", s.name, e));
                }
            }
        }
    }

    // -----------------------------------------------------------------
    // Stepping
    // -----------------------------------------------------------------

    fn step_once(&mut self, drive: &mut dyn Drive) {
        let t0 = Instant::now();
        let dt = self.desc.dt;
        let next = self.head + 1;
        let tn = dt.time_of(next);
        for (id, key) in &self.reg.kinematic {
            if let (Some(body), Some(pose)) = (self.reg.entries[*id as usize].body, drive.pose(key, tn)) {
                self.world.set_target(body, pose, dt.secs_f32());
            }
        }
        self.world.step(dt.secs_f32(), self.desc.substeps);
        self.hits.clear();
        self.world.hits(&mut self.hits);
        let time = tn.as_secs() as f32;
        for &(pos, normal, speed, a, b, m) in &self.hits {
            let (Some(a), Some(b)) = (self.reg.id_of(a), self.reg.id_of(b)) else { continue };
            self.history.contacts.push_back(ContactRecord { pos, normal, impulse: speed * m, speed, time, a, b, step: next });
        }
        self.slept.clear();
        self.world.slept(&mut self.slept);
        for k in 0..self.slept.len() {
            let body = self.slept[k];
            if let Some(id) = self.reg.id_of(body) {
                let p = self.world.pose(body);
                self.history.events.push_back(EventRecord { kind: EventKind::Sleep, id, time, step: next, pos: [p[0], p[1], p[2]] });
            }
        }
        self.head = next;
        self.expire_at(next);
        self.spawns_at(next);
        self.sims_at(next);
        // Keep what any view within the kept frames can still ask for.
        let window = dt.steps_in(self.desc.retain) + self.keep as u64 + 1;
        self.history.prune(next.saturating_sub(window));
        let te = Instant::now();
        self.record(next);
        let ns = t0.elapsed().as_nanos() as u64;
        self.stats.extract_ns += te.elapsed().as_nanos() as u64;
        self.stats.steps += 1;
        self.stats.step_ns += ns;
        self.stats.step_ns_max = self.stats.step_ns_max.max(ns);
        self.maybe_checkpoint();
    }

    fn record(&mut self, step: u64) {
        let mut f = self.spare.pop().unwrap_or_default();
        f.step = step;
        f.ids.clear();
        f.poses.clear();
        f.speeds.clear();
        for &id in &self.reg.live {
            if let Some(body) = self.reg.entries[id as usize].body {
                f.ids.push(id);
                f.poses.push(self.world.pose(body));
                f.speeds.push(self.world.speed(body));
            }
        }
        f.sims.clear();
        f.sims.extend(self.sims.iter().cloned());
        self.recent.push_back(f);
        while self.recent.len() > self.keep {
            let old = self.recent.pop_front().unwrap();
            self.spare.push(old);
        }
    }

    fn frame(&self, step: u64) -> Option<&Frame> {
        let first = self.recent.front()?.step;
        if step < first {
            return None;
        }
        self.recent.get((step - first) as usize)
    }

    fn maybe_checkpoint(&mut self) {
        if !self.ring.wants(self.head) {
            return;
        }
        let (image, geometry) = self.world.save();
        let history = self.history.clone();
        let bytes = image.len() + geometry.len() + self.reg.bytes() + history.bytes() + self.sims.iter().map(|s| s.len() * 4).sum::<usize>() + self.pending.len() * std::mem::size_of::<Pending>();
        let cp = Checkpoint {
            step: self.head,
            stamp: self.stamp,
            image: Arc::new(image),
            geometry: Arc::new(geometry),
            reg: self.reg.clone(),
            cursor: self.cursor,
            pending: self.pending.clone(),
            next_order: self.next_order,
            history,
            sims: self.sims.clone(),
            bytes,
        };
        let dropped = self.ring.insert(cp);
        if dropped > 0 {
            self.stats.thinned += dropped as u64;
        }
    }

    /// Restores the newest valid checkpoint at or before `step`.
    fn restore(&mut self, step: u64) -> bool {
        loop {
            let k = self.ring.cps.partition_point(|c| c.step <= step);
            if k == 0 {
                return false;
            }
            let cp = &self.ring.cps[k - 1];
            if cp.stamp != self.stamp {
                // Never restored under another document.
                self.ring.bytes -= cp.bytes;
                self.ring.cps.remove(k - 1);
                self.stats.stale_discarded += 1;
                continue;
            }
            let Some(world) = PhysWorld::restore(&self.desc, &cp.image, &cp.geometry) else {
                self.errors.insert(format!("checkpoint at step {} did not restore", cp.step));
                self.ring.bytes -= cp.bytes;
                self.ring.cps.remove(k - 1);
                continue;
            };
            let cp = cp.clone();
            self.world = world;
            self.head = cp.step;
            self.reg = cp.reg;
            self.cursor = cp.cursor;
            self.pending = cp.pending;
            self.next_order = cp.next_order;
            self.history = cp.history;
            self.sims = cp.sims;
            while let Some(f) = self.recent.pop_front() {
                self.spare.push(f);
            }
            self.record(self.head);
            self.stats.restores += 1;
            return true;
        }
    }

    /// Steps needed to view `t`: the frames `lo ..= hi`.
    fn need(&self, t: SimTime) -> (u64, u64) {
        let s = self.desc.dt.step_floor(t);
        (s.saturating_sub(1), s + 1)
    }

    fn have(&self, lo: u64, hi: u64) -> bool {
        self.recent.front().is_some_and(|f| f.step <= lo) && self.head >= hi
    }

    /// Prepares stepping towards `hi`: restores when the frames at `lo`
    /// are gone, or when a checkpoint lies well ahead of the head.
    fn plan(&mut self, lo: u64, hi: u64, drive: &mut dyn Drive) {
        let front = self.recent.front().map(|f| f.step).unwrap_or(u64::MAX);
        if front > lo {
            if !self.restore(lo) {
                // Nothing valid on record (every checkpoint failed to
                // restore): back to the initial state.
                self.rebuild(drive);
            }
            return;
        }
        if self.head < hi {
            let k = self.ring.cps.partition_point(|c| c.step <= lo);
            if k > 0 {
                let c = self.ring.cps[k - 1].step;
                // Restoring costs about as much as a few dozen steps.
                if c > self.head + 32 {
                    self.restore(lo);
                }
            }
        }
    }

    fn rebuild(&mut self, drive: &mut dyn Drive) {
        let mut stats = std::mem::take(&mut self.stats);
        let desc = (*self.desc).clone();
        *self = Stepper::new(desc, drive);
        stats.steps += self.stats.steps;
        self.stats = stats;
    }

    /// Makes `t` viewable, synchronously (locked time): restores the
    /// nearest valid checkpoint when it must and steps forward.
    pub fn seek(&mut self, t: SimTime, drive: &mut dyn Drive) {
        let (lo, hi) = self.need(t);
        if self.have(lo, hi) {
            return;
        }
        self.plan(lo, hi, drive);
        while self.head < hi {
            self.step_once(drive);
        }
    }

    /// Realtime: steps towards `t` until it is viewable or `budget` is
    /// spent (a view then shows the last state it has).
    pub fn advance_to(&mut self, t: SimTime, budget: Duration, drive: &mut dyn Drive) -> Progress {
        let (lo, hi) = self.need(t);
        if self.have(lo, hi) {
            return Progress::Ready;
        }
        let start = Instant::now();
        self.plan(lo, hi, drive);
        while self.head < hi {
            if start.elapsed() >= budget {
                return Progress::Behind { head: self.head };
            }
            self.step_once(drive);
        }
        Progress::Ready
    }

    /// The view at `t`; `None` until [`Self::seek`] (or a finished
    /// [`Self::advance_to`]) made it available.
    pub fn view(&self, t: SimTime) -> Option<View<'_>> {
        let (lo, hi) = self.need(t);
        if !self.have(lo, hi) {
            return None;
        }
        let dt = self.desc.dt;
        Some(View { st: self, t: if t.ticks() < 0 { SimTime::ZERO } else { t }, s: dt.step_floor(t), alpha: dt.alpha(t) })
    }

    /// The view at `t`, or at the newest state when `t` is not reachable
    /// yet (realtime: the last snapshot stays up; counted as late).
    pub fn view_or_latest(&mut self, t: SimTime) -> View<'_> {
        let (lo, hi) = self.need(t);
        if self.have(lo, hi) {
            let dt = self.desc.dt;
            return View { st: self, t: if t.ticks() < 0 { SimTime::ZERO } else { t }, s: dt.step_floor(t), alpha: dt.alpha(t) };
        }
        self.stats.late += 1;
        let s = self.head.saturating_sub(1);
        View { st: self, t: self.desc.dt.time_of(s), s, alpha: 0.0 }
    }

    /// A hash of the whole live state at the head: every body's pose and
    /// velocities, the id table, spawn cursor and queue, history and sims.
    pub fn state_hash(&self) -> u64 {
        let mut h = Fnv::default();
        h.u64(self.head);
        let mut w = Vec::new();
        for &id in &self.reg.live {
            h.word(id);
            if let Some(b) = self.reg.entries[id as usize].body {
                w.clear();
                self.world.motion_words(b, &mut w);
                h.words(&w);
            }
        }
        for e in &self.reg.entries {
            h.word(e.group);
            h.u64(e.born);
            h.u64(e.expires);
        }
        h.u64(self.cursor as u64);
        for p in &self.pending {
            h.u64(p.step);
            h.u64(p.order);
        }
        for c in &self.history.contacts {
            let mut cw = [0u32; CONTACT_WORDS];
            c.to_words(&mut cw);
            h.words(&cw);
        }
        for e in &self.history.events {
            let mut ew = [0u32; EVENT_WORDS];
            e.to_words(&mut ew);
            h.words(&ew);
        }
        for s in &self.sims {
            h.words(s);
        }
        h.finish()
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Spherical interpolation, shortest arc.
pub fn slerp(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let mut d = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    let b = if d < 0.0 {
        d = -d;
        [-b[0], -b[1], -b[2], -b[3]]
    } else {
        b
    };
    let (wa, wb) = if d > 0.9995 {
        (1.0 - t, t)
    } else {
        let th = d.clamp(-1.0, 1.0).acos();
        let s = th.sin();
        (((1.0 - t) * th).sin() / s, (t * th).sin() / s)
    };
    let q = [a[0] * wa + b[0] * wb, a[1] * wa + b[1] * wb, a[2] * wa + b[2] * wb, a[3] * wa + b[3] * wb];
    let l = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt().max(1e-12);
    [q[0] / l, q[1] / l, q[2] / l, q[3] / l]
}

fn interp(a: [f32; 7], b: [f32; 7], t: f32) -> [f32; 7] {
    if t == 0.0 {
        return a;
    }
    let q = slerp([a[3], a[4], a[5], a[6]], [b[3], b[4], b[5], b[6]], t);
    [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t), q[0], q[1], q[2], q[3]]
}

/// The sim as seen at one time: bodies interpolated between `step(t)` and
/// the step after, and the contacts, events and sim states of `step(t)`.
pub struct View<'a> {
    st: &'a Stepper,
    t: SimTime,
    s: u64,
    alpha: f32,
}

impl<'a> View<'a> {
    pub fn time(&self) -> SimTime {
        self.t
    }

    pub fn step(&self) -> u64 {
        self.s
    }

    pub fn alpha(&self) -> f32 {
        self.alpha
    }

    fn pose_at(&self, id: u32, s: u64, alpha: f32) -> Option<([f32; 7], f32)> {
        let a = self.st.frame(s)?;
        let (ka, pa) = a.pose_of(id)?;
        let sa = a.speeds[ka];
        match self.st.frame(s + 1).and_then(|b| b.pose_of(id).map(|(kb, pb)| (pb, b.speeds[kb]))) {
            Some((pb, sb)) if alpha > 0.0 => Some((interp(pa, pb, alpha), lerp(sa, sb, alpha))),
            _ => Some((pa, sa)),
        }
    }

    /// The bodies of `group` ("" for all) alive at this time, in id order.
    pub fn bodies(&self, group: &str, out: &mut Vec<RigidRecord>) {
        let st = self.st;
        let Some(g) = st.group_filter(group) else { return };
        let Some(a) = st.frame(self.s) else { return };
        let dt = st.desc.dt;
        let prev_t = self.t.saturating_sub(SimTime::from_ticks(dt.ticks()));
        let (ps, pal) = if prev_t.ticks() <= 0 { (0, 0.0) } else { (dt.step_floor(prev_t), dt.alpha(prev_t)) };
        for &id in &a.ids {
            if !st.in_group(g, id) {
                continue;
            }
            let Some((pose, speed)) = self.pose_at(id, self.s, self.alpha) else { continue };
            let prev = self.pose_at(id, ps, pal).map(|p| p.0).unwrap_or(pose);
            out.push(st.rigid(id, pose, prev, speed, self.t));
        }
    }

    /// The bodies of `group` written as `layout` records (a draw shader's
    /// reflected instance record, or [`rigid_layout`]); returns the count.
    pub fn write_bodies(&self, group: &str, layout: &Layout, out: &mut Vec<u32>) -> usize {
        let mut recs = Vec::new();
        self.bodies(group, &mut recs);
        let stride = layout.stride as usize;
        out.clear();
        out.resize(recs.len() * stride, 0);
        for (i, r) in recs.iter().enumerate() {
            r.write_as(layout, &mut out[i * stride..(i + 1) * stride]);
        }
        recs.len()
    }

    /// Contacts involving `group` within the retention window.
    pub fn contacts(&self, group: &str, out: &mut Vec<ContactRecord>) {
        self.st.contacts_in(group, self.s, self.t, out);
    }

    /// Events of `group` within the retention window.
    pub fn events(&self, group: &str, out: &mut Vec<EventRecord>) {
        self.st.events_in(group, self.s, self.t, out);
    }

    /// Contact records as kernel input words.
    pub fn contact_words(&self, group: &str) -> (Vec<u32>, usize) {
        let mut recs = Vec::new();
        self.contacts(group, &mut recs);
        let mut w = vec![0u32; recs.len() * CONTACT_WORDS];
        for (i, r) in recs.iter().enumerate() {
            r.to_words(&mut w[i * CONTACT_WORDS..(i + 1) * CONTACT_WORDS]);
        }
        (w, recs.len())
    }

    /// Event records as kernel input words.
    pub fn event_words(&self, group: &str) -> (Vec<u32>, usize) {
        let mut recs = Vec::new();
        self.events(group, &mut recs);
        let mut w = vec![0u32; recs.len() * EVENT_WORDS];
        for (i, r) in recs.iter().enumerate() {
            r.to_words(&mut w[i * EVENT_WORDS..(i + 1) * EVENT_WORDS]);
        }
        (w, recs.len())
    }

    /// A sim's state at `step(t)`.
    pub fn sim(&self, name: &str) -> Option<&'a [u32]> {
        let k = self.st.desc.sims.iter().position(|s| s.name == name)?;
        let f = self.st.frame(self.s)?;
        f.sims.get(k).map(|v| v.as_slice())
    }

    /// Groups known so far (declared bodies and spawns that ran).
    pub fn groups(&self) -> &'a [String] {
        &self.st.reg.groups
    }

    /// A hash of everything this view shows.
    pub fn hash(&self) -> u64 {
        let mut h = Fnv::default();
        h.u64(self.s);
        h.word(self.alpha.to_bits());
        let mut recs = Vec::new();
        self.bodies("", &mut recs);
        let mut w = [0u32; RIGID_WORDS];
        for r in &recs {
            r.to_words(&mut w);
            h.words(&w);
        }
        let (cw, _) = self.contact_words("");
        h.words(&cw);
        let (ew, _) = self.event_words("");
        h.words(&ew);
        for s in &self.st.desc.sims {
            if let Some(v) = self.sim(&s.name) {
                h.words(v);
            }
        }
        h.finish()
    }
}
