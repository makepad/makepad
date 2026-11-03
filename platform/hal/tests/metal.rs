//! Native Metal coverage (offscreen windows: no window shows).
#![cfg(target_vendor = "apple")]

use makepad_hal::*;
use std::time::{Duration, Instant};

/// An upload from a temporary (`&vec![..]`: freed at the end of its statement, its block handed
/// out again before the submit) lands the bytes of the call: the writer copied them at the call.
#[test]
fn uploads_from_freed_temporaries_land_the_bytes_of_the_call() {
    let hal = Hal::new();
    let info = hal.open(&OpenReq { backend: backend_id::METAL, flags: open_flags::OFFSCREEN_WINDOWS, ..Default::default() });
    assert_eq!(info.backend, backend_id::METAL, "Metal HAL initialization failed");
    let sid = info.session;
    let (buffer, texture) = (Id::new(9, 1), Id::new(9, 1));
    let want_buffer: Vec<u8> = (0..4096u32).map(|i| (i * 7 + 3) as u8).collect();
    let want_texels: Vec<u8> = (0..16 * 16 * 4u32).map(|i| (i * 13 + 5) as u8).collect();
    let mut w = CmdWriter::new(sid);
    w.buffer_create(buffer, buffer_kind::INSTANCE, 4096, Attr::default());
    w.texture_create(texture, usage::SAMPLED | usage::RENDER_TARGET, format::RGBA8, 16, 16, [1, 1], Attr::default());
    w.buffer_update(buffer, 0, &want_buffer.clone());
    w.texture_update(texture, 0, [0, 0, 16, 16], 0, &want_texels.clone());
    let scribble = [vec![0xeeu8; 4096], vec![0xee; 1024]];
    w.buffer_readback(buffer, 1, 0, 4096);
    w.texture_readback(texture, 2, [0, 0, 16, 16]);
    hal.submit(&w.buf());
    drop(scribble);
    let mut got: [Vec<u8>; 2] = Default::default();
    let mut store = EventStore::new(sid, 64, 1 << 16);
    let deadline = Instant::now() + Duration::from_secs(30);
    while got.iter().any(|g| g.is_empty()) {
        assert!(Instant::now() < deadline, "Metal readback deadline");
        let mut buf = store.buf();
        let n = hal.pump(10_000_000, &mut buf);
        for (_, e) in store.reader(n, buf.arena_len) {
            if let HalEvent::Readback { req: req @ 1..=2, status, pixels, .. } = e {
                assert_eq!(status, readback::OK);
                got[req as usize - 1] = pixels.to_vec();
            }
        }
    }
    assert!(got[0] == want_buffer, "the buffer holds other bytes than its upload's");
    assert!(got[1] == want_texels, "the texture holds other bytes than its upload's");
    assert_eq!(hal.with_world(|w| w.session(sid).unwrap().stats.bad_args), 0);
    assert!(hal.close(sid));
}
