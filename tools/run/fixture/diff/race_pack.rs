// diff_fast corpus program: the Sandbox race kit's wire pack (RaceKit::pack -> Race::to_wire().ser_bin(out),
// micro_serde's derived SerBin: per-field to_le_bytes + extend_from_slice / push, Vec<T> = u64 length then
// items, [u32; 6] arrays, Option<u32>), into one reused buffer that is cleared per instance and compared
// with the last sent bytes (HostSession::send_kit_instances). Relax O3 took a bus error in pack, writing
// at a page boundary, in a hosted race session.
// rustc --test -O and `makepad test` (all tiers) must print identical D lines.
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
trait SerBin { fn ser_bin(&self, s: &mut Vec<u8>); }
macro_rules! le { ($($t:ty),*) => { $(impl SerBin for $t { fn ser_bin(&self, s: &mut Vec<u8>) { s.extend_from_slice(&self.to_le_bytes()); } })* } }
le!(f32, u64, u32, i32);
impl SerBin for u8 { fn ser_bin(&self, s: &mut Vec<u8>) { s.push(*self); } }
impl SerBin for bool { fn ser_bin(&self, s: &mut Vec<u8>) { s.push(if *self { 1 } else { 0 }); } }
impl<T: SerBin> SerBin for Vec<T> {
    fn ser_bin(&self, s: &mut Vec<u8>) { let len = self.len() as u64; len.ser_bin(s); for item in self { item.ser_bin(s); } }
}
impl<T: SerBin> SerBin for Option<T> {
    fn ser_bin(&self, s: &mut Vec<u8>) { match self { None => s.push(0), Some(v) => { s.push(1); v.ser_bin(s); } } }
}
impl<T: SerBin> SerBin for [T] { fn ser_bin(&self, s: &mut Vec<u8>) { for item in self { item.ser_bin(s); } } }
impl<T: SerBin, const N: usize> SerBin for [T; N] { fn ser_bin(&self, s: &mut Vec<u8>) { self.as_slice().ser_bin(s) } }
macro_rules! wire { ($name:ident { $($f:ident: $t:ty),* $(,)? }) => {
    #[derive(Clone, Debug, PartialEq)] struct $name { $($f: $t),* }
    impl SerBin for $name { fn ser_bin(&self, s: &mut Vec<u8>) { $(self.$f.ser_bin(s);)* } }
} }
const MAX_SECTORS: usize = 6;
#[derive(Clone, Copy, Debug, Default, PartialEq)] struct V3 { x: f32, y: f32, z: f32 }
#[derive(Clone, Copy)] struct SpawnPoint { pos: V3, yaw: f32, owner: u64 }
#[derive(Clone, Copy)] struct Checkpoint { pos: V3, half: V3, #[allow(dead_code)] index: u32, entity: u64 }
#[derive(Clone, Copy, Default)]
struct LapTiming {
    started: bool, lap_start: u32, sector_start: u32, sector: u8, sectors: [u32; MAX_SECTORS], last_sectors: [u32; MAX_SECTORS],
    best_sectors: [u32; MAX_SECTORS], last_lap: u32, best_lap: u32, laps_timed: u32, gate_ms: u32, delta: i32, has_delta: bool,
    total: u32, gap_ahead: u32, gap_behind: u32, progress_ms: u32,
}
#[derive(Clone, Copy)]
struct Standing { entity: u64, lap: u32, checkpoint: u32, progress: u32, finished: bool, finish_tick: u64, score: i32, timing: LapTiming, waiting: bool, hold_end: u64 }
struct Race {
    spawns: Vec<SpawnPoint>, checkpoints: Vec<Checkpoint>, standings: Vec<Standing>, laps: u32, running: bool, winner: u64, circuit: bool,
    sectors: u32, hold_ticks: Option<u32>, hold_end: u64, release_tick: u64, fastest: (u64, u32),
}
wire!(TimingWire { started: bool, lap_start: u32, sector_start: u32, sector: u8, sectors: [u32; MAX_SECTORS], last_sectors: [u32; MAX_SECTORS],
    best_sectors: [u32; MAX_SECTORS], last_lap: u32, best_lap: u32, laps_timed: u32, gate_ms: u32, delta: i32, has_delta: bool, total: u32,
    gap_ahead: u32, gap_behind: u32, progress_ms: u32 });
wire!(StandingWire { entity: u64, lap: u32, checkpoint: u32, progress: u32, finished: bool, finish_tick: u64, score: i32, waiting: bool, hold_end: u64, timing: TimingWire });
wire!(GateWire { pos: [f32; 3], half: [f32; 3], entity: u64 });
wire!(SlotWire { pos: [f32; 3], yaw: f32, owner: u64 });
wire!(RaceWire { gates: Vec<GateWire>, slots: Vec<SlotWire>, hold_ticks: Option<u32>, laps: u32, running: bool, winner: u64, circuit: bool,
    sectors: u32, hold_end: u64, release_tick: u64, fastest_entity: u64, fastest_ms: u32, standings: Vec<StandingWire> });
impl Race {
    fn to_wire(&self) -> RaceWire {
        RaceWire {
            gates: self.checkpoints.iter().map(|c| GateWire { pos: [c.pos.x, c.pos.y, c.pos.z], half: [c.half.x, c.half.y, c.half.z], entity: c.entity }).collect(),
            slots: self.spawns.iter().map(|s| SlotWire { pos: [s.pos.x, s.pos.y, s.pos.z], yaw: s.yaw, owner: s.owner }).collect(),
            hold_ticks: self.hold_ticks, laps: self.laps, running: self.running, winner: self.winner, circuit: self.circuit, sectors: self.sectors,
            hold_end: self.hold_end, release_tick: self.release_tick, fastest_entity: self.fastest.0, fastest_ms: self.fastest.1,
            standings: self.standings.iter().map(|s| StandingWire {
                entity: s.entity, lap: s.lap, checkpoint: s.checkpoint, progress: s.progress, finished: s.finished, finish_tick: s.finish_tick,
                score: s.score, waiting: s.waiting, hold_end: s.hold_end,
                timing: { let t = &s.timing; TimingWire { started: t.started, lap_start: t.lap_start, sector_start: t.sector_start, sector: t.sector,
                    sectors: t.sectors, last_sectors: t.last_sectors, best_sectors: t.best_sectors, last_lap: t.last_lap, best_lap: t.best_lap,
                    laps_timed: t.laps_timed, gate_ms: t.gate_ms, delta: t.delta, has_delta: t.has_delta, total: t.total, gap_ahead: t.gap_ahead,
                    gap_behind: t.gap_behind, progress_ms: t.progress_ms } },
            }).collect(),
        }
    }
}
struct Kit { instances: Vec<Race>, ids: Vec<u32> }
impl Kit {
    fn get(&self, id: u32) -> Option<&Race> { self.ids.binary_search(&id).ok().map(|i| &self.instances[i]) }
    #[inline(never)]
    fn pack(&self, i: u32, out: &mut Vec<u8>) { if let Some(race) = self.get(i) { race.to_wire().ser_bin(out); } }
}
fn race(seed: u32, gates: usize, racers: usize) -> Race {
    let f = |k: u32| (seed.wrapping_mul(2654435761).wrapping_add(k.wrapping_mul(40503)) % 1000) as f32 * 0.25;
    Race {
        spawns: (0..racers.max(4)).map(|k| SpawnPoint { pos: V3 { x: f(k as u32), y: 1.0, z: -f(k as u32 + 7) }, yaw: f(k as u32 + 3), owner: if k < racers { 1000 + k as u64 } else { 0 } }).collect(),
        checkpoints: (0..gates).map(|k| Checkpoint { pos: V3 { x: f(k as u32), y: 0.5, z: f(k as u32 + 1) }, half: V3 { x: 4.0, y: 2.0, z: 0.5 }, index: k as u32, entity: 500 + k as u64 }).collect(),
        standings: (0..racers).map(|k| {
            let mut t = LapTiming::default();
            t.started = k % 2 == 0; t.lap_start = seed * 16 + k as u32; t.sector = (k % MAX_SECTORS) as u8;
            for s in 0..MAX_SECTORS { t.sectors[s] = seed + s as u32; t.last_sectors[s] = seed * 2 + s as u32; t.best_sectors[s] = seed * 3 + k as u32; }
            t.delta = k as i32 - 3; t.has_delta = k % 3 == 0; t.gap_ahead = seed ^ k as u32; t.progress_ms = seed * 7;
            Standing { entity: 1000 + k as u64, lap: seed % 4, checkpoint: k as u32, progress: seed + k as u32, finished: k == 0 && seed % 5 == 0,
                finish_tick: seed as u64 * 3, score: -(k as i32), timing: t, waiting: seed % 2 == 1, hold_end: 180 + seed as u64 }
        }).collect(),
        laps: 3, running: seed % 7 != 0, winner: if seed % 5 == 0 { 1000 } else { 0 }, circuit: seed % 2 == 0, sectors: 3,
        hold_ticks: if seed % 3 == 0 { None } else { Some(seed % 200) }, hold_end: 180 + seed as u64, release_tick: seed as u64, fastest: (1000 + (seed % 4) as u64, seed * 11),
    }
}
struct Host { kit_buf: Vec<u8>, sent: Vec<(u32, Vec<u8>)> }
impl Host {
    #[inline(never)]
    fn send(&mut self, kit: &Kit) -> (usize, u64) {
        let mut buf = std::mem::take(&mut self.kit_buf);
        let mut changed = 0; let mut sum = 0u64;
        let mut each = |i: u32| {
            buf.clear();
            kit.pack(i, &mut buf);
            for (k, b) in buf.iter().enumerate() { sum = sum.wrapping_mul(31).wrapping_add(*b as u64 ^ k as u64); }
            match self.sent.iter_mut().find(|(id, _)| *id == i) {
                Some((_, s)) if *s == buf => {}
                Some((_, s)) => { *s = buf.clone(); changed += 1; }
                None => { self.sent.push((i, buf.clone())); changed += 1; }
            }
        };
        for &i in &kit.ids { each(i); }
        self.kit_buf = buf;
        (changed, sum)
    }
}
fn work() {
    let mut host = Host { kit_buf: Vec::new(), sent: Vec::new() };
    let mut total = 0u64;
    for tick in 0..400u32 {
        let n = 1 + (tick as usize / 50) % 3;
        let kit = Kit {
            instances: (0..n).map(|r| race(tick / 3 + r as u32, 8 + (r * 13 + tick as usize / 20) % 40, 1 + (tick as usize / 10 + r) % 12)).collect(),
            ids: (1..=n as u32).collect(),
        };
        let (c, s) = host.send(&kit);
        total = total.wrapping_add(s ^ c as u64);
        if tick % 50 == 0 { d!("tick {} kits {} changed {} sum {:x} cap>=len {}", tick, n, c, s, host.kit_buf.capacity() >= host.kit_buf.len()); }
    }
    let lens: Vec<usize> = host.sent.iter().map(|(_, b)| b.len()).collect();
    d!("total {:x} lens {:?}", total, lens);
}
fn rounds(f: fn()) {
    let ms: u64 = 4 * std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }
