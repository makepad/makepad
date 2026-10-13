// A big value made by a call (returned in memory), passed by value into another constructor and
// wrapped in Some(..) into an existing place -- Score's audio callback:
// `engine = Some(PlaybackEngine::new(SamplerBackend::new(..)))` with 108 KB / 121 KB values and Arc
// fields moved in, on CoreAudio's 512 KB IO thread. Relax kept each step in its own slot plus a dead
// slot's size in a colour (351 KB frame): the call writes its result buffer only, so the result is
// built where it is copied to, and a slot nothing addresses takes no stack.
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
static DROPS: AtomicUsize = AtomicUsize::new(0);
struct Tracked(u32);
impl Drop for Tracked {
    fn drop(&mut self) {
        DROPS.fetch_add(1, Ordering::Relaxed);
    }
}
struct Backend {
    sound: Arc<Tracked>,
    peak: Arc<Tracked>,
    left: [f32; 8192],
    right: [f32; 8192],
    events: [(u8, u32); 64],
    n: u32,
}
impl Backend {
    #[inline(never)]
    fn new(sound: Arc<Tracked>, peak: Arc<Tracked>, n: u32) -> Self {
        let mut events = [(5u8, 0u32); 64];
        events[3].1 = check(n);
        Self { sound, peak, left: [0.25; 8192], right: [0.5; 8192], events, n }
    }
}
struct Engine {
    backend: Backend,
    pending: [u64; 2048],
    len: usize,
}
impl Engine {
    #[inline(never)]
    fn new(backend: Backend) -> Self {
        let mut pending = [0u64; 2048];
        pending[7] = backend.n as u64 * 3;
        Self { backend, pending, len: 1 }
    }
}
#[inline(never)]
fn check(n: u32) -> u32 {
    if n > 1000 {
        panic!("too big");
    }
    n + 1
}
fn hash(e: &Engine) -> u64 {
    let mut h = 0u64;
    for (k, x) in e.backend.left.iter().chain(e.backend.right.iter()).enumerate() {
        h = h.wrapping_mul(31).wrapping_add((x.to_bits() as u64) ^ k as u64);
    }
    for (a, b) in e.backend.events.iter() {
        h = h.wrapping_mul(31).wrapping_add(*a as u64 + *b as u64);
    }
    for p in e.pending.iter() {
        h = h.wrapping_mul(31).wrapping_add(*p);
    }
    h ^ (e.len as u64) ^ (e.backend.sound.0 as u64) << 8 ^ (e.backend.peak.0 as u64) << 16
}
#[inline(never)]
fn fill(slot: &mut Option<Engine>, n: u32) {
    let sound = Arc::new(Tracked(3));
    let peak = Arc::new(Tracked(4));
    if slot.is_none() {
        *slot = Some(Engine::new(Backend::new(sound.clone(), peak.clone(), n)));
    }
}
#[test]
fn big_through_calls() {
    let mut slot: Option<Engine> = None;
    fill(&mut slot, 7);
    let e = slot.as_ref().unwrap();
    println!("hash {} n {} arcs {} {}", hash(e), e.backend.n, Arc::strong_count(&e.backend.sound), Arc::strong_count(&e.backend.peak));
    drop(slot);
    println!("drops {}", DROPS.load(Ordering::Relaxed));
    // the constructor panics half way: the moved-in Arcs are dropped once, the slot stays None
    let mut slot2: Option<Engine> = None;
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fill(&mut slot2, 5000)));
    println!("unwound {} none {} drops {}", r.is_err(), slot2.is_none(), DROPS.load(Ordering::Relaxed));
}
