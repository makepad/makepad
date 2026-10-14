use makepad_hal::buffers::{pack_drag_items, read_drag_items, read_items, read_pairs, read_strs, PipelineBlob};
use makepad_hal::cmds::{sample_cmds, CMD_TABLE};
use makepad_hal::events::{sample_events, EVENT_TABLE};
use makepad_hal::session::{Miss, Table, World};
use makepad_hal::*;
use std::time::{Duration, Instant};

fn open(hal: &Hal, key: u64) -> u32 {
    let info = hal.open(&OpenReq { backend: backend_id::HEADLESS, app_key: key, flags: 0, dpi_hint: 2.0, parent: 0 });
    assert_ne!(info.session, 0);
    assert_eq!(info.dpi, 2.0);
    info.session
}

fn pump(hal: &Hal, store: &mut EventStore, timeout_ns: u64) -> Vec<(u64, String)> {
    let mut buf = store.buf();
    let n = hal.pump(timeout_ns, &mut buf);
    store.reader(n, buf.arena_len).map(|(t, e)| (t, format!("{e:?}"))).collect()
}

#[test]
fn every_command_round_trips() {
    let mut w = CmdWriter::new(1);
    let expected = sample_cmds(&mut w);
    assert_eq!(expected.len(), CMD_TABLE.len());
    assert_eq!(w.cmds().len(), CMD_TABLE.len());
    // 94 commands: 56 core (including ClipRead, GpuBudget, WindowChrome and WindowDpiOverride, with
    // timer start/stop counted separately), 6 embedded, 4 domains, 10 media, 3 fonts, 8 direct display/input
    // (incl. DirectDisplayArrangement, DirectDisplayPacing, DirectTapToClick), 2 GL interop, 3 mobile system UI (MobileUi,
    // SelectionHandles, Haptic), 1 system back (BackUnhandled), 1 font touch (GlyphTouch, tag 150).
    // The media section (tags 120-129, backend/apple/media) has its own budget.
    let media = CMD_TABLE.iter().filter(|t| (120..130).contains(&t.0)).count();
    let fonts = CMD_TABLE.iter().filter(|t| (130..133).contains(&t.0)).count();
    assert_eq!(media, 10);
    assert_eq!(fonts, 3);
    assert!(media <= 15, "the media section is budgeted at 15 commands, it has {media}");
    assert_eq!(CMD_TABLE.len(), 56 + 6 + 4 + media + fonts + 8 + 2 + 4 + 1);
    for ((cmd, want), (tag, name, _)) in w.cmds().iter().zip(&expected).zip(CMD_TABLE) {
        assert_eq!(cmd.tag, *tag, "{name}");
        let got = cmd.decode(w.arena()).unwrap_or_else(|| panic!("{name} does not decode"));
        assert_eq!(&format!("{got:?}"), want, "{name}");
    }
    let tags: std::collections::HashSet<u16> = CMD_TABLE.iter().map(|t| t.0).collect();
    assert_eq!(tags.len(), CMD_TABLE.len(), "tags are unique");
    assert!(Cmd { tag: 9999, ..Default::default() }.decode(&[]).is_none());
}

#[test]
fn every_event_round_trips() {
    let mut q = EventQueue::new();
    q.set_time(77);
    let expected = sample_events(&mut q);
    assert_eq!(expected.len(), EVENT_TABLE.len());
    assert_eq!(EVENT_TABLE.len(), 92, "54 core (incl. Clipboard, GpuBudget, PayloadSize, MouseCancel, AppMessage, DeviceReset, GpuPressure), 5 parent/child (85-89), 13 media, 4 font, 4 direct display/input 1 GL interop and 9 mobile (LongPress, ImeState, ImeAction, ImeResize, PhysicalKeyboard, SelectionHandle, TouchCancel, SafeArea, ImeAnimate) and 2 system back (BackGesture, BackPressed) events");
    assert_eq!(EVENT_TABLE.iter().filter(|t| (120..133).contains(&t.0)).count(), 13);
    assert_eq!(EVENT_TABLE.iter().filter(|t| (100..104).contains(&t.0)).count(), 4);
    let tags: std::collections::HashSet<u16> = EVENT_TABLE.iter().map(|t| t.0).collect();
    assert_eq!(tags.len(), EVENT_TABLE.len(), "tags are unique");
    let mut store = EventStore::new(1, 128, 4096);
    let mut buf = store.buf();
    let n = q.drain_into(&mut buf) as usize;
    assert_eq!(n, EVENT_TABLE.len());
    let got: Vec<(u64, String)> = store.reader(n as u32, buf.arena_len).map(|(t, e)| (t, format!("{e:?}"))).collect();
    for (((t, g), want), (_, name, _)) in got.iter().zip(&expected).zip(EVENT_TABLE) {
        assert_eq!(g, want, "{name}");
        assert_eq!(*t, 77);
    }
}

#[test]
fn small_event_buffer_delivers_in_order_without_loss() {
    let mut q = EventQueue::new();
    for i in 0..10 {
        q.text(Id::new(1, 1), input::text::TYPED, format!("event number {i}").as_bytes());
    }
    let mut store = EventStore::new(1, 3, 40);
    let mut seen = Vec::new();
    while !q.is_empty() {
        let mut buf = store.buf();
        let n = q.drain_into(&mut buf);
        assert!(n > 0);
        seen.extend(store.reader(n, buf.arena_len).map(|(_, e)| format!("{e:?}")));
    }
    assert_eq!(seen.len(), 10);
    assert!(seen[9].contains(&format!("{:?}", b"event number 9".as_slice())));
}

#[test]
fn arrays_and_blobs_round_trip() {
    let mut w = CmdWriter::new(1);
    let items = [DragItem { kind: 1, str: 2 }, DragItem { kind: 3, str: 4 }];
    let r = w.put_items(&items);
    let back: Vec<DragItem> = read_items(&w.arena()[r.off as usize..(r.off + r.len) as usize], 2).collect();
    assert_eq!(back, items);
    let strs = w.put_strs(&["a.png", "b.txt"]);
    assert_eq!(read_strs(&w.arena()[strs.off as usize..(strs.off + strs.len) as usize], 2), ["a.png", "b.txt"]);
    let desc = PipelineDesc { shader: 9, name: 3, source: 4, instance_stride: 12, varyings: 5, blend: 1, color_formats: [1, 0, 0, 0], ..Default::default() };
    let blob = CmdWriter::pipeline_blob(desc, b"shader code", &[1, 2, 3], &[7, 8]);
    let p = PipelineBlob::parse(&blob).unwrap();
    assert_eq!((p.code, p.bindings.as_slice(), p.vertex_layout.as_slice()), (&b"shader code"[..], &[1, 2, 3][..], &[7, 8][..]));
    assert_eq!((p.desc.shader, p.desc.name, p.desc.instance_stride, p.desc.varyings), (9, 3, 12, 5));
    assert!(PipelineBlob::parse(&blob[..20]).is_none());
}

/// A texture the device cannot hold (no room, past its limits) is reported gone as an
/// evicted one, so the app makes it again when it next needs it instead of drawing with an id
/// that never got storage (149: a window maximised on a full GPU then showed only its outline).
#[test]
fn a_texture_the_device_cannot_hold_is_reported_evicted() {
    let hal = Hal::new();
    let s = open(&hal, 0);
    let mut store = EventStore::new(1, 128, 4096);
    let mut w = CmdWriter::new(s);
    w.texture_create(Id::new(1, 1), usage::SAMPLED | usage::RENDER_TARGET, format::BGRA8, 1200, 800, [1, 1], Attr::default());
    w.texture_create(Id::new(2, 1), usage::SAMPLED | usage::RENDER_TARGET, format::BGRA8, 40000, 800, [1, 1], Attr::default());
    hal.submit(&w.buf());
    let evs: Vec<String> = pump(&hal, &mut store, 0).into_iter().map(|e| e.1).collect();
    assert_eq!(evs.iter().filter(|e| e.contains("TextureEvicted")).count(), 1, "{evs:?}");
    assert!(evs.iter().any(|e| e.contains("TextureEvicted") && e.contains("index: 2")), "{evs:?}");
    assert_eq!(hal.with_world(|w| w.count(ledger::Kind::Texture)), 1);
    // made again at a size the device holds (the window restored): it is there
    w.clear();
    w.texture_create(Id::new(2, 2), usage::SAMPLED | usage::RENDER_TARGET, format::BGRA8, 1200, 800, [1, 1], Attr::default());
    hal.submit(&w.buf());
    assert!(pump(&hal, &mut store, 0).iter().all(|e| !e.1.contains("TextureEvicted")));
    assert_eq!(hal.with_world(|w| w.count(ledger::Kind::Texture)), 2);
}

#[test]
fn stale_ids_are_counted_and_ignored() {
    let hal = Hal::new();
    let s = open(&hal, 0);
    let mut w = CmdWriter::new(s);
    let t = Id::new(4, 2);
    w.texture_create(t, usage::SAMPLED, format::RGBA8, 16, 16, [1, 1], Attr::default());
    w.texture_destroy(t);
    w.texture_destroy(t); // stale: already gone
    w.texture_create(Id::new(4, 1), usage::SAMPLED, format::RGBA8, 16, 16, [1, 1], Attr::default()); // older generation than the freed one
    w.texture_update(Id::new(4, 2), 0, [0, 0, 1, 1], 0, &[0; 4]); // dead
    w.buffer_destroy(Id::new(99, 1)); // never existed
    hal.submit(&w.buf());
    let stats = hal.with_world(|w| w.session(s).unwrap().stats);
    assert_eq!(stats.stale_ids, 4);
    assert_eq!(hal.with_world(|w| w.count(ledger::Kind::Texture)), 0);
    // a newer generation of the slot is fine
    w.clear();
    w.texture_create(Id::new(4, 3), usage::SAMPLED, format::RGBA8, 16, 16, [1, 1], Attr::default());
    hal.submit(&w.buf());
    assert_eq!(hal.with_world(|w| w.count(ledger::Kind::Texture)), 1);
    // generic table behaviour
    let mut t: Table<u8> = Table::default();
    t.insert(1, Id::new(0, 5), 1, 7).unwrap();
    assert_eq!(t.get(1, Id::new(0, 4)).err(), Some(Miss::Stale));
    assert_eq!(t.insert(1, Id::new(0, 4), 1, 8).err(), Some(Miss::Stale));
}

#[test]
fn bad_arguments_are_counted() {
    let hal = Hal::new();
    let s = open(&hal, 0);
    let mut w = CmdWriter::new(s);
    w.texture_create(Id::new(0, 1), usage::SAMPLED, 999, 16, 16, [1, 1], Attr::default());
    w.texture_create(Id::new(1, 1), usage::SAMPLED, format::RGBA8, 0, 16, [1, 1], Attr::default());
    w.texture_create(Id::new(2, 1), usage::SAMPLED, format::RGBA8, 4, 4, [1, 1], Attr::default());
    w.texture_update(Id::new(2, 1), 0, [2, 2, 4, 4], 0, &[0; 64]); // outside
    w.texture_update(Id::new(2, 1), 0, [0, 0, 4, 4], 0, &[0; 10]); // wrong length
    w.texture_update(Id::new(2, 1), 0, [0, 0, 4, 4], 0, &[0; 64]); // fine
    w.push(Cmd { tag: 9999, ..Default::default() });
    hal.submit(&w.buf());
    let st = hal.with_world(|w| w.session(s).unwrap().stats);
    assert_eq!((st.bad_args, st.unknown_cmds, st.stale_ids), (4, 1, 0));
}

#[test]
fn data_formats_mips_cube_faces_and_blocks_are_validated() {
    let hal = Hal::new();
    let s = open(&hal, 0);
    let mut w = CmdWriter::new(s);
    // a cube (6 layers) of RG16F with 2 mips: level 0 is 8x8x4 bytes, level 1 is 4x4x4 bytes
    w.texture_create(Id::new(0, 1), usage::SAMPLED, format::RG16F, 8, 8, [6, 2], Attr::default());
    w.texture_update(Id::new(0, 1), 0, [0, 0, 8, 8], 5, &[0; 256]); // face 5, level 0
    w.texture_update(Id::new(0, 1), 1, [0, 0, 4, 4], 3, &[0; 64]); // face 3, level 1
    w.texture_update(Id::new(0, 1), 0, [0, 0, 8, 8], 6, &[0; 256]); // face 6 does not exist
    w.texture_update(Id::new(0, 1), 2, [0, 0, 2, 2], 0, &[0; 16]); // level 2 does not exist
    // a BC7 image of 9x5 texels is 3x2 blocks of 16 bytes
    w.texture_create(Id::new(1, 1), usage::SAMPLED, format::BC7, 9, 5, [1, 1], Attr::default());
    w.texture_update(Id::new(1, 1), 0, [0, 0, 9, 5], 0, &[0; 96]);
    w.texture_update(Id::new(1, 1), 0, [0, 0, 9, 5], 0, &[0; 45]); // texel count, not block bytes
    w.texture_create(Id::new(2, 1), usage::RENDER_TARGET, format::R32UINT, 4, 4, [1, 1], Attr::default());
    hal.submit(&w.buf());
    let st = hal.with_world(|w| w.session(s).unwrap().stats);
    assert_eq!((st.bad_args, st.stale_ids), (3, 0));
    assert_eq!(format::level_bytes(format::BC7, 9, 5), 96);
    assert_eq!(headless_bytes(format::BC7, 9, 5, 1, 1), 96);
    assert_eq!(headless_bytes(format::RG16F, 8, 8, 6, 2), 6 * (8 * 8 * 4 + 4 * 4 * 4));
}

#[test]
fn drag_items_in_events_carry_their_strings() {
    let payload = pack_drag_items(&[(input::drag_item::FILE, "/tmp/a.png"), (input::drag_item::STRING, "héllo")]);
    let mut q = EventQueue::new();
    q.drag_over(Id::new(1, 1), 3.0, 4.0, 0, 2, &payload);
    let mut store = EventStore::new(1, 8, 4096);
    let mut buf = store.buf();
    let n = q.drain_into(&mut buf);
    let evs: Vec<_> = store.reader(n, buf.arena_len).collect();
    match evs[0].1 {
        HalEvent::DragOver { count, items, .. } => {
            let got = read_drag_items(items, count);
            assert_eq!(got, vec![(input::drag_item::FILE, "/tmp/a.png".to_string()), (input::drag_item::STRING, "héllo".to_string())]);
        }
        ref e => panic!("{e:?}"),
    }
    // a truncated payload ends the list instead of reading out of bounds
    assert_eq!(read_drag_items(&payload[..payload.len() - 3], 2).len(), 1);
}

#[test]
fn instance_ranges_are_element_ranges() {
    let hal = Hal::new();
    let s = open(&hal, 0);
    let mut w = CmdWriter::new(s);
    w.str_def(1, b"padding that moves the arena off an 8 multiple");
    let r = w.put_pairs(&[[0, 4], [10, 2], [20, 1]]);
    assert_eq!(r.len, 3);
    assert_eq!(read_pairs(w.arena(), r).collect::<Vec<_>>(), vec![[0, 4], [10, 2], [20, 1]]);
    w.pipeline_create(Id::new(0, 1), dialect::NONE, Attr::default(), &CmdWriter::pipeline_blob(PipelineDesc::default(), &[], &[], &[]));
    w.frame_begin(1, FrameMeta::default());
    w.pass(Pass { color: [Id::NONE; 4], depth: Id::NONE, ..Default::default() });
    w.draw(Draw { pipeline: Id::new(0, 1), inst_ranges: r, ..Default::default() });
    w.frame_end(Id::new(0, 1), target::WINDOW);
    w.window_create(Id::new(0, 1), window_kind::NORMAL, Id::NONE, [0.0, 0.0, 10.0, 10.0], false);
    hal.submit(&w.buf());
    assert_eq!(hal.with_world(|w| w.session(s).unwrap().stats.bad_args), 0);
}

fn headless_bytes(f: u32, w: u32, h: u32, d: u32, m: u32) -> u64 {
    makepad_hal::backend::headless::texture_bytes(f, w, h, d, m)
}

#[test]
fn session_close_frees_everything_and_keeps_adoptable_windows() {
    let hal = Hal::new();
    let host = hal.with_world(|w| w.host_window([0.0, 0.0, 800.0, 600.0]));
    let s = open(&hal, 42);
    let mut w = CmdWriter::new(s);
    w.window_create(Id::new(0, 1), window_kind::NORMAL, Id::NONE, [0.0, 0.0, 1.0, 1.0], true); // adopts the host window
    w.window_create(Id::new(1, 1), window_kind::NORMAL, Id::NONE, [0.0, 0.0, 100.0, 100.0], false);
    w.texture_create(Id::new(0, 1), usage::RENDER_TARGET, format::RGBA8, 64, 64, [1, 1], Attr::default());
    w.buffer_create(Id::new(0, 1), buffer_kind::VERTEX, 1024, Attr::default());
    w.timer_start(5, 1_000_000, true);
    hal.submit(&w.buf());
    assert_eq!(hal.with_world(|w| w.lent_to(host)), Some(s));
    // the adopted window carries the host's geometry
    let mut store = EventStore::new(s, 32, 1024);
    let evs = pump(&hal, &mut store, 0);
    assert!(evs.iter().any(|(_, e)| e.contains("Geom") && e.contains("800.0")), "{evs:?}");
    assert_eq!(hal.with_world(|w| (w.count(ledger::Kind::Window), w.count(ledger::Kind::Texture), w.count(ledger::Kind::Buffer))), (3, 1, 1));
    // a second open with the same app key = relaunch: the old session is closed
    let s2 = open(&hal, 42);
    assert_ne!(s, s2);
    assert!(hal.with_world(|w| w.session(s).is_none()));
    assert_eq!(hal.with_world(|w| (w.count(ledger::Kind::Texture), w.count(ledger::Kind::Buffer))), (0, 0));
    // the host window survived, lent to nobody
    assert!(hal.with_world(|w| w.host_window_alive(host)));
    assert_eq!(hal.with_world(|w| w.lent_to(host)), None);
    // and can be adopted again; detaching returns it as well
    let mut w2 = CmdWriter::new(s2);
    w2.window_create(Id::new(7, 1), window_kind::NORMAL, Id::NONE, [0.0; 4], true);
    hal.submit(&w2.buf());
    assert_eq!(hal.with_world(|w| w.lent_to(host)), Some(s2));
    w2.clear();
    w2.window_close(Id::new(7, 1));
    hal.submit(&w2.buf());
    assert_eq!(hal.with_world(|w| w.lent_to(host)), None);
    assert!(hal.with_world(|w| w.host_window_alive(host)));
    // nothing to adopt -> a normal window
    let mut w3 = World::new();
    let s3 = w3.open(0);
    assert!(w3.adopt_window(s3, Id::new(0, 1)).is_none());
}

#[test]
fn ledger_totals_and_idle() {
    let hal = Hal::new();
    let s = open(&hal, 0);
    let mut w = CmdWriter::new(s);
    w.str_def(1, b"vj/effects");
    w.str_def(2, b"effects.rs:10:5");
    w.str_def(3, b"main.rs:3:1");
    w.str_def(4, b"explicit-label");
    let a = Attr { owner: 1, site: 2, label: 0, kind: 0 };
    w.texture_create(Id::new(0, 1), usage::RENDER_TARGET, format::RGBA8, 100, 100, [1, 1], a);
    w.texture_create(Id::new(1, 1), usage::RENDER_TARGET, format::RGBA8, 10, 10, [1, 3], Attr { owner: 1, site: 2, label: 4, kind: 0 });
    w.texture_create(Id::new(2, 1), usage::RENDER_TARGET, format::DEPTH32F, 100, 100, [1, 1], Attr { owner: 0, site: 3, label: 0, kind: 0 });
    w.buffer_create(Id::new(0, 1), buffer_kind::INSTANCE, 5000, a);
    hal.submit(&w.buf());
    let r = hal.report(s, 30).unwrap();
    let mip_chain = 10 * 10 * 4 + 5 * 5 * 4 + 2 * 2 * 4;
    assert_eq!(r.total_bytes, 100 * 100 * 4 + mip_chain + 100 * 100 * 4 + 5000);
    let kind = |k: &str| r.by_kind.iter().find(|g| g.key == k).map(|g| (g.count, g.bytes));
    assert_eq!(kind("render target"), Some((2, 40_000 + mip_chain)));
    assert_eq!(kind("depth"), Some((1, 40_000)));
    assert_eq!(kind("instance buffers"), Some((1, 5000)));
    // owner = label, else scope, else site
    let owner = |k: &str| r.by_owner.iter().find(|g| g.key == k).map(|g| g.bytes);
    assert_eq!(owner("explicit-label"), Some(mip_chain));
    assert_eq!(owner("vj/effects"), Some(40_000 + 5000));
    assert_eq!(owner("main.rs:3:1"), Some(40_000));
    assert_eq!(r.by_site.iter().find(|g| g.key == "effects.rs:10:5").map(|g| g.count), Some(3));
    assert_eq!(r.idle.len(), 3, "never used textures are idle");
    // a frame that samples one texture makes it non-idle
    w.clear();
    w.frame_begin(5, FrameMeta::default());
    w.pass(Pass { color: [Id::new(0, 1), Id::NONE, Id::NONE, Id::NONE], depth: Id::NONE, ..Default::default() });
    w.frame_end(Id::new(0, 1), target::TEXTURE);
    hal.submit(&w.buf());
    let r = hal.report(s, 30).unwrap();
    assert_eq!(r.idle.len(), 2);
    assert_eq!(r.items.iter().find(|i| i.index == 0 && i.detail == "render target").unwrap().last_used_frame, 5);
    // destroying removes from the ledger
    w.clear();
    w.texture_destroy(Id::new(0, 1));
    hal.submit(&w.buf());
    assert_eq!(hal.report(s, 30).unwrap().total_bytes, r.total_bytes - 40_000);
    assert_eq!(hal.report_all(30).total_bytes, hal.report(s, 30).unwrap().total_bytes);
}

#[test]
fn frame_packet_with_two_passes_three_draws_yields_frame_done() {
    let hal = Hal::new();
    let s = open(&hal, 0);
    let mut w = CmdWriter::new(s);
    let win = Id::new(0, 1);
    w.window_create(win, window_kind::NORMAL, Id::NONE, [0.0, 0.0, 640.0, 480.0], false);
    let blob = CmdWriter::pipeline_blob(PipelineDesc { name: 1, ..Default::default() }, b"code", &[], &[]);
    w.pipeline_create(Id::new(0, 1), dialect::MSL, Attr::default(), &blob);
    w.texture_create(Id::new(0, 1), usage::RENDER_TARGET | usage::SAMPLED, format::RGBA8, 64, 64, [1, 1], Attr::default());
    w.buffer_create(Id::new(0, 1), buffer_kind::VERTEX, 256, Attr::default());
    let ranges = w.put_items(&[[0u32, 3]]);
    w.frame_begin(7, FrameMeta { input_ns: 1, ..Default::default() });
    w.pass(Pass { color: [Id::new(0, 1), Id::NONE, Id::NONE, Id::NONE], depth: Id::NONE, viewport: [0.0, 0.0, 64.0, 64.0], ..Default::default() });
    let draw = Draw { pipeline: Id::new(0, 1), vb: Id::new(0, 1), ib: Id::NONE, inst: Inst::inline(Slice::of(&[0.0f32; 8]), 2), inst_ranges: ranges, ubufs: [Id::NONE; 4], ..Default::default() };
    w.draw(draw);
    w.draw(draw);
    w.pass(Pass { color: [Id::NONE; 4], depth: Id::NONE, ..Default::default() });
    let tex = w.put_ids(&[Id::new(0, 1)]);
    w.draw(Draw { tex, ..draw });
    w.frame_end(win, target::WINDOW);
    hal.submit(&w.buf());
    let mut store = EventStore::new(s, 64, 4096);
    let evs = pump(&hal, &mut store, 0);
    let done: Vec<_> = evs.iter().filter(|(_, e)| e.starts_with("FrameDone")).collect();
    assert_eq!(done.len(), 1, "{evs:?}");
    assert!(done[0].1.contains("frame: 7"));
    assert!(evs.iter().any(|(_, e)| e.starts_with("PipelineReady")));
    assert!(evs.iter().any(|(_, e)| e.starts_with("Startup")));
    let st = hal.with_world(|w| w.session(s).unwrap().stats);
    assert_eq!((st.frames, st.passes, st.draws, st.skipped_draws, st.stale_ids), (1, 2, 3, 0, 0));
    assert!(hal.await_frame_done(s, 7, Duration::from_millis(10)));
    assert!(!hal.await_frame_done(s, 8, Duration::from_millis(10)));
    // a draw with a dead pipeline is skipped, counted, and the frame still completes
    w.clear();
    w.frame_begin(8, FrameMeta::default());
    w.pass(Pass { color: [Id::NONE; 4], depth: Id::NONE, ..Default::default() });
    w.draw(Draw { pipeline: Id::new(5, 1), ..draw });
    w.frame_end(win, target::WINDOW);
    hal.submit(&w.buf());
    assert!(hal.await_frame_done(s, 8, Duration::from_millis(10)));
    assert_eq!(hal.with_world(|w| w.session(s).unwrap().stats.skipped_draws), 1);
    // steady state: the writer does not grow
    let cap = w.capacities();
    for f in 9..20 {
        w.clear();
        w.frame_begin(f, FrameMeta::default());
        w.pass(Pass { color: [Id::NONE; 4], depth: Id::NONE, ..Default::default() });
        w.draw(Draw { pipeline: Id::new(0, 1), ..draw });
        w.frame_end(win, target::WINDOW);
        hal.submit(&w.buf());
    }
    assert_eq!(w.capacities(), cap);
}

#[test]
fn readback_and_snapshot_say_no_raster() {
    let hal = Hal::new();
    let s = open(&hal, 0);
    let mut w = CmdWriter::new(s);
    w.window_create(Id::new(0, 1), window_kind::NORMAL, Id::NONE, [0.0, 0.0, 320.0, 200.0], false);
    w.texture_create(Id::new(0, 1), usage::RENDER_TARGET, format::RGBA8, 8, 8, [1, 1], Attr::default());
    w.texture_readback(Id::new(0, 1), 11, [0, 0, 8, 8]);
    w.texture_readback(Id::new(9, 1), 12, [0, 0, 8, 8]);
    w.snapshot(Id::new(0, 1), 13);
    hal.submit(&w.buf());
    let mut store = EventStore::new(s, 64, 1024);
    let evs: Vec<String> = pump(&hal, &mut store, 0).into_iter().map(|e| e.1).collect();
    let rb = |req: &str| evs.iter().find(|e| e.starts_with("Readback") && e.contains(req)).cloned().unwrap_or_default();
    assert!(rb("req: 11").contains("status: 2"), "{evs:?}");
    assert!(rb("req: 12").contains("status: 1"));
    assert!(rb("req: 13").contains("status: 2") && rb("req: 13").contains("320"));
}

#[test]
fn timers_follow_the_manual_clock() {
    let hal = Hal::new();
    let s = open(&hal, 0);
    hal.clock_set(1_000);
    let mut w = CmdWriter::new(s);
    w.timer_start(1, 500, true);
    w.timer_start(2, 700, false);
    hal.submit(&w.buf());
    let mut store = EventStore::new(s, 16, 256);
    assert!(pump(&hal, &mut store, 0).iter().all(|e| !e.1.starts_with("Timer")));
    hal.clock_advance(600);
    let e = pump(&hal, &mut store, 0);
    assert_eq!(e.iter().filter(|e| e.1.starts_with("Timer { timer: 1")).count(), 1);
    assert!(e.iter().all(|e| e.0 == 1_600 || e.1.starts_with("Startup")));
    hal.clock_advance(200);
    let e = pump(&hal, &mut store, 0);
    assert_eq!(e.iter().filter(|e| e.1.starts_with("Timer { timer: 2")).count(), 1);
    assert_eq!(hal.now(), 1_800);
    w.clear();
    w.timer_stop(1);
    hal.submit(&w.buf());
    hal.clock_advance(10_000);
    assert!(pump(&hal, &mut store, 0).is_empty());
}

#[test]
fn inject_delivers_with_its_time() {
    let hal = Hal::new();
    let s = open(&hal, 0);
    let mut store = EventStore::new(s, 16, 256);
    pump(&hal, &mut store, 0);
    hal.clock_set(555);
    let mut e = RawEvent::key_down(Id::new(1, 1), 65, false, 0);
    e.time_ns = 999;
    hal.inject(s, e);
    hal.inject(s, RawEvent::key_up(Id::new(1, 1), 65, 0));
    let got = pump(&hal, &mut store, 0);
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].0, 999);
    assert!(got[0].1.starts_with("KeyDown"));
}

#[test]
fn wake_from_another_thread_unblocks_pump() {
    let hal = Hal::new();
    let s = open(&hal, 0);
    let mut store = EventStore::new(s, 16, 256);
    pump(&hal, &mut store, 0);
    let h = hal.clone();
    let t = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        h.wake();
    });
    let t0 = Instant::now();
    let got = pump(&hal, &mut store, 20_000_000_000);
    let took = t0.elapsed();
    t.join().unwrap();
    assert!(took >= Duration::from_millis(80) && took < Duration::from_secs(10), "{took:?}");
    assert_eq!(got.len(), 1);
    assert!(got[0].1.starts_with("Wake"));
    // a wake that arrives before the pump is not lost
    hal.wake();
    let t0 = Instant::now();
    assert_eq!(pump(&hal, &mut store, 20_000_000_000).len(), 1);
    assert!(t0.elapsed() < Duration::from_secs(5));
    // plain timeout with nothing to deliver
    let t0 = Instant::now();
    assert!(pump(&hal, &mut store, 50_000_000).is_empty());
    assert!(t0.elapsed() >= Duration::from_millis(40));
}

#[test]
fn the_four_ops_work_through_the_global_hal() {
    let info = hal_open(&OpenReq { backend: backend_id::HEADLESS, app_key: 0xfeed, flags: 0, dpi_hint: 0.0, parent: 0 });
    assert_ne!(info.session, 0);
    assert_eq!(hal_open(&OpenReq { backend: 77, ..Default::default() }).session, 0);
    let mut w = CmdWriter::new(info.session);
    w.window_create(Id::new(0, 1), window_kind::NORMAL, Id::NONE, [0.0, 0.0, 10.0, 10.0], false);
    w.frame_begin(1, FrameMeta::default());
    w.frame_end(Id::new(0, 1), target::WINDOW);
    hal_submit(&w.buf());
    hal_wake();
    let mut store = EventStore::new(info.session, 16, 256);
    let mut buf = store.buf();
    let n = hal_pump(0, &mut buf);
    let evs: Vec<String> = store.reader(n, buf.arena_len).map(|e| format!("{:?}", e.1)).collect();
    assert!(evs.iter().any(|e| e.starts_with("FrameDone")) && evs.iter().any(|e| e.starts_with("Wake")), "{evs:?}");
    // a corrupt batch (misaligned element slice) is rejected whole and counted
    let bytes = [0u8; 13];
    let bad = CmdBuf { session: info.session, cmds: Slice::from_bytes(&bytes), ..Default::default() };
    hal_submit(&bad);
    assert_eq!(Hal::global().with_world(|w| w.session(info.session).unwrap().stats.bad_bufs), 1);
}

#[test]
fn clock_is_monotonic_and_absolute() {
    let a = clock_ns();
    let b = clock_ns();
    assert!(a > 0 && b >= a);
}

#[test]
fn relaunch_keeps_the_backend_and_frees_only_the_old_session() {
    let hal = Hal::new();
    let host = hal.with_world(|w| w.host_window([0.0, 0.0, 800.0, 600.0]));
    let a = open(&hal, 7);
    assert_eq!(hal.backend_inits(), 1);
    let mut w = CmdWriter::new(a);
    w.window_create(Id::new(0, 1), window_kind::NORMAL, Id::NONE, [0.0; 4], true);
    w.texture_create(Id::new(3, 1), usage::RENDER_TARGET, format::RGBA8, 32, 32, [1, 1], Attr::default());
    hal.submit(&w.buf());
    assert_eq!(hal.with_world(|w| w.lent_to(host)), Some(a));
    assert_eq!(hal.report(a, 30).unwrap().total_bytes, 32 * 32 * 4);
    // another app is not touched by A's relaunch
    let b = open(&hal, 8);
    let mut wb = CmdWriter::new(b);
    wb.texture_create(Id::new(0, 1), usage::SAMPLED, format::RGBA8, 4, 4, [1, 1], Attr::default());
    hal.submit(&wb.buf());
    // relaunch A
    let a2 = open(&hal, 7);
    assert_ne!(a, a2);
    assert_eq!(hal.backend_inits(), 1, "same backend instance");
    assert!(hal.report(a, 30).is_none(), "old session gone");
    assert_eq!(hal.report_all(30).total_bytes, 4 * 4 * 4, "A's texture freed, B's kept");
    assert_eq!(hal.with_world(|w| w.lent_to(host)), None);
    assert!(hal.with_world(|w| w.host_window_alive(host)));
    // a different backend id on a Hal that has one is refused
    assert_eq!(hal.open(&OpenReq { backend: 5, ..Default::default() }).session, 0);
    assert_eq!(hal.backend_inits(), 1);
    // explicit close frees too
    assert!(hal.close(b));
    assert_eq!(hal.report_all(30).total_bytes, 0);
}

/// A draw names any number of sampled textures (no fixed slot count): the ids round-trip
/// through the arena, and a slot past the range reads as none.
#[test]
fn draw_texture_ids_have_no_slot_limit() {
    use makepad_hal::buffers::{id_at, read_ids};
    let mut w = CmdWriter::new(1);
    let ids: Vec<Id> = (0..13).map(|i| Id::new(i, i + 1)).collect();
    w.put_strs(&["odd"]); // leaves the arena unaligned
    let r = w.put_ids(&ids);
    assert_eq!(r.len, 13);
    assert_eq!(read_ids(w.arena(), r).collect::<Vec<_>>(), ids);
    assert_eq!(id_at(w.arena(), r, 12), Id::new(12, 13));
    assert_eq!(id_at(w.arena(), r, 13), Id::NONE);
}
