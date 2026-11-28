//! Watch the decode worker alone while a call shows and returns remote pictures.
use makepad_call::driver::codec::TestCodecs;
use makepad_call::driver::{Call, CallConfig, CallEvent, Content, Join, Spawn, VideoCaps, VideoConfig, VideoSource, VoiceSource};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[global_allocator]
static ALLOCATOR: Counting = Counting;
struct Counting;

thread_local! {
    static DECODE: Cell<bool> = const { Cell::new(false) };
}
static WATCH: AtomicBool = AtomicBool::new(false);
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static FREES: AtomicUsize = AtomicUsize::new(0);

fn watched() -> bool {
    DECODE.with(Cell::get) && WATCH.load(Ordering::Relaxed)
}

// SAFETY: counting uses only const TLS and atomics. All allocations go to System unchanged.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if watched() {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if watched() {
            FREES.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if watched() {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            FREES.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, size) }
    }
}

fn link(to: SyncSender<(SocketAddr, Vec<u8>)>) -> SyncSender<Vec<u8>> {
    let (tx, rx) = sync_channel(256);
    std::thread::spawn(move || {
        let from = SocketAddr::from(([192, 0, 2, 1], 47100));
        while let Ok(d) = rx.recv() {
            if to.send((from, d)).is_err() {
                break;
            }
        }
    });
    tx
}

#[test]
fn the_decode_worker_reuses_pictures_after_warmup() {
    let panicked = Arc::new(AtomicBool::new(false));
    let failed = panicked.clone();
    let spawn: Spawn = Arc::new(move |name, run| {
        let failed = failed.clone();
        std::thread::Builder::new().name(name.into()).spawn(move || {
            DECODE.with(|w| w.set(name == "call decode"));
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)).is_err() {
                failed.store(true, Ordering::Release);
            }
        }).expect("start call worker");
    });
    let config = |id, video| CallConfig {
        inlet: None, call_id: [7; 16], my_pub: vec![id; 65], video, voice: VoiceSource::Feed,
        echo_cancel: makepad_call::driver::EchoCancelMode::Off,
        codecs: Arc::new(TestCodecs), spawn: spawn.clone(), signal: Arc::new(|| {}), clock: Instant::now(),
    };
    let caps = VideoCaps { width: 320, height: 180, fps: 30, content: Content::Camera };
    let mut a = Call::start(config(1, Some(VideoConfig { caps, source: VideoSource::Feed })));
    let mut b = Call::start(config(2, None));
    let (ab, ba) = (link(b.inlet()), link(a.inlet()));
    a.join(Join { peer: 2, their_pub: vec![2; 65], key_id: 0, send_secret: [1; 32], recv_secret: [2; 32], out: ab, stream_cap: None });
    b.join(Join { peer: 1, their_pub: vec![1; 65], key_id: 0, send_secret: [2; 32], recv_secret: [1; 32], out: ba, stream_cap: None });
    let poll = |a: &mut Call, b: &mut Call| {
        assert!(!panicked.load(Ordering::Acquire), "a call worker panicked");
        for event in a.poll().into_iter().chain(b.poll()) {
            if let CallEvent::Problem { what, .. } = event {
                panic!("call worker reported {what}");
            }
        }
    };
    let camera = a.video_in().unwrap();
    // Keep feeding until a picture is shown: the guard may skip a capture on a slow box.
    for i in 0..180 {
        if i == 60 {
            WATCH.store(true, Ordering::Relaxed);
        }
        loop {
            camera.push_nv12(2, 2, &[128; 6]);
            std::thread::sleep(Duration::from_millis(2));
            poll(&mut a, &mut b);
            if b.tiles().iter().any(|t| t.who == Some(1) && t.fresh) {
                break;
            }
        }
    }
    WATCH.store(false, Ordering::Relaxed);
    a.stop();
    b.stop();
    while !a.stopped() || !b.stopped() {
        poll(&mut a, &mut b);
        std::thread::sleep(Duration::from_millis(2));
    }
    let counts = (ALLOCS.load(Ordering::Relaxed), FREES.load(Ordering::Relaxed));
    eprintln!("120 shown pictures after 60 warm-up: {} allocations, {} frees", counts.0, counts.1);
    assert_eq!(counts, (0, 0), "the decode worker must neither allocate nor free over 120 shown pictures after warm-up");
}
