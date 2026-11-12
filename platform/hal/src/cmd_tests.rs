use crate::cmds::{sample_cmds, CMD_TABLE};
use crate::*;

fn open(hal: &Hal, key: u64) -> u32 {
    let info = hal.open(&OpenReq { backend: backend_id::HEADLESS, app_key: key, flags: 0, dpi_hint: 2.0, parent: 0 });
    assert_ne!(info.session, 0);
    assert_eq!(info.dpi, 2.0);
    info.session
}

#[test]
fn every_command_round_trips() {
    let mut w = CmdWriter::new(1);
    let expected = sample_cmds(&mut w);
    assert_eq!(expected.len(), CMD_TABLE.len());
    assert_eq!(w.cmds().len(), CMD_TABLE.len());
    // 59 core (including BufferReadbacks, PipelineWanted, ClipRead, GpuBudget, WindowChrome, WindowCaption and WindowDpiOverride, with
    // timer start/stop counted separately), 6 embedded, 4 domains, 9 media, 3 native layers, 2 accessibility, 3 fonts, 8 direct display/input
    // (incl. DirectDisplayArrangement, DirectDisplayPacing, DirectTapToClick, DirectDisplayVirtual tag 153), 2 GL interop, 3 mobile system UI (MobileUi,
    // SelectionHandles, Haptic), 1 system back (BackUnhandled), 1 font touch (GlyphTouch, tag 150), 3 tray (TraySet,
    // TrayRemove, WindowAtTray tag 154), 3 XR frame (XrFrameEyes 155, XrPoseLead 156, XrRefreshRate 157),
    // 2 accessibility (A11yUpdate 158, A11yActionDone 159), 1 XR haptic (XrHaptic 163), 4 hosted (WindowScreenOrigin 28,
    // HostedPresented 29, HostedNativeBridge 33, HostedNativeIn 34), 1 GPU profiler readout (GpuProfQuery 35),
    // 1 hosted focus for accessibility (WindowA11yActive 36), 1 refresh rate (FrameRateBoost 165), 1 working-set reclaim (GpuReclaim 166),
    // 1 PDF print request (PrintPdf 167).
    // The media section (tags 120-129, backend/apple/media) has its own budget.
    let media = CMD_TABLE.iter().filter(|t| (120..130).contains(&t.0)).count();
    let fonts = CMD_TABLE.iter().filter(|t| (130..133).contains(&t.0)).count();
    assert_eq!(media, 9);
    assert_eq!(fonts, 3);
    assert!(media <= 15, "the media section is budgeted at 15 commands, it has {media}");
    assert_eq!(CMD_TABLE.len(), 59 + 6 + 4 + media + fonts + 8 + 2 + 4 + 1 + 3 + 3 + 2 + 3 + 1 + 4 + 1 + 1 + 1 + 1 + 1 + 1);
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
    // SAFETY: `bytes` outlives the submit
    let bad = CmdBuf { cmds: unsafe { Slice::from_bytes(&bytes) }, ..w.buf() };
    hal_submit(&bad);
    assert_eq!(Hal::global().with_world(|w| w.session(info.session).unwrap().stats.bad_bufs), 1);
}
