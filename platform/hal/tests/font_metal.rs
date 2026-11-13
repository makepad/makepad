//! The font service on a real device (Metal, offscreen windows): glyphs drawn from the session's
//! lent atlas id show their own pixels before the atlas grows, after it moved to a taller texture
//! and after it went back to its base size.
#![cfg(target_vendor = "apple")]

use makepad_font_core::rustybuzz::ttf_parser::Face;
use makepad_hal::font::wire::{self, atlas, glyph, kind};
use makepad_hal::font::{FontSource, GlyphEntry};
use makepad_hal::*;
use std::collections::HashMap;
use std::time::{Duration, Instant};

const EMOJI: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../widgets/resources/NotoColorEmoji.ttf");
const DPXS: f32 = 96.0;
const WIN: Id = Id::new(1, 1);
const QUAD: Id = Id::new(1, 1);
const INDEX: Id = Id::new(2, 1);
const PIPE: Id = Id::new(1, 1);
const W: u32 = 900;
const H: u32 = 160;

/// Draws texel rect `t` of the atlas 1:1 at window rect `r`, nearest texels at their centres, so
/// every window pixel is exactly one atlas texel.
const MSL: &str = r#"
#include <metal_stdlib>
using namespace metal;
struct V { float4 pos [[position]]; float2 texel; };
vertex V vertex_main(uint vid [[vertex_id]], uint iid [[instance_id]], constant float2 *geom [[buffer(0)]], constant float4 *inst [[buffer(1)]]) {
    float4 r = inst[iid * 2];
    float4 t = inst[iid * 2 + 1];
    float2 g = geom[vid];
    float2 p = r.xy + g * r.zw;
    V o;
    o.pos = float4(p.x * 2.0 - 1.0, 1.0 - p.y * 2.0, 0.0, 1.0);
    o.texel = t.xy + g * t.zw;
    return o;
}
fragment float4 fragment_main(V i [[stage_in]], texture2d<float> atlas [[texture(0)]]) {
    constexpr sampler s(filter::nearest);
    float2 size = float2(atlas.get_width(), atlas.get_height());
    return atlas.sample(s, (floor(i.texel) + 0.5) / size);
}
"#;

fn emoji_ids(n: usize) -> Vec<u32> {
    let bytes = std::fs::read(EMOJI).expect("font file");
    let face = Face::parse(&bytes, 0).expect("font parses");
    let mut ids: Vec<u32> = (0x1F300u32..0x1FB00).filter_map(char::from_u32).filter_map(|c| face.glyph_index(c)).map(|g| g.0 as u32).collect();
    ids.sort();
    ids.dedup();
    ids.truncate(n);
    ids
}

#[derive(Default)]
struct Seen {
    atlas: Option<(Id, u32)>,
    ready: HashMap<u32, GlyphEntry>,
    evicted: Vec<u32>,
    readback: Option<Vec<u8>>,
    pipelines: u32,
    done: u64,
    partial: u64,
}

struct Device {
    hal: std::sync::Arc<Hal>,
    sid: u32,
    store: EventStore,
    w: CmdWriter,
    seen: Seen,
    frame: u64,
}

impl Device {
    fn pump(&mut self) {
        let mut buf = self.store.buf();
        let n = self.hal.pump(10_000_000, &mut buf);
        let len = buf.arena_len;
        for (_, e) in self.store.reader(n, len) {
            match e {
                HalEvent::AtlasInfo { kind: atlas::RASTER, texture, h, .. } => self.seen.atlas = Some((texture, h)),
                HalEvent::GlyphsReady { count, entries, .. } => {
                    for e in wire::read_entries(entries, count) {
                        self.seen.ready.insert(e.glyph, e);
                    }
                }
                HalEvent::AtlasEvicted { count, slots, .. } => self.seen.evicted.extend(wire::read_slots(slots, count).into_iter().map(|(s, _)| s)),
                HalEvent::Readback { req: 7, status, lease, .. } => {
                    // SAFETY: this process pumped the event and takes its lease once.
                    let pixels = unsafe { makepad_hal::lease::Held::from_payload(lease) }.unwrap();
                    assert_eq!(status, readback::OK);
                    self.seen.readback = Some(pixels.to_vec());
                }
                HalEvent::PipelineReady { .. } => self.seen.pipelines += 1,
                HalEvent::PipelineFailed { log, .. } => panic!("pipeline: {}", String::from_utf8_lossy(log)),
                HalEvent::FramePartial { frame, .. } => self.seen.partial = frame,
                HalEvent::FrameDone { frame, .. } => self.seen.done = self.seen.done.max(frame),
                _ => {}
            }
        }
    }

    fn until(&mut self, what: &str, done: impl Fn(&Seen) -> bool) {
        let end = Instant::now() + Duration::from_secs(60);
        while !done(&self.seen) {
            assert!(Instant::now() < end, "timed out waiting for {what}");
            self.pump();
        }
    }

    fn submit(&mut self) {
        self.hal.submit(&self.w.buf());
        self.w.clear();
    }

    fn request(&mut self, glyphs: &[u32]) {
        let list: Vec<(u32, f32)> = glyphs.iter().map(|g| (*g, DPXS)).collect();
        self.w.request_glyphs(1, kind::AUTO, &list);
        self.submit();
        self.until("glyphs", |s| glyphs.iter().all(|g| s.ready.contains_key(g)));
    }

    /// Draws `glyphs` side by side from the session's atlas id and reads the window back.
    fn draw(&mut self, glyphs: &[GlyphEntry]) -> Vec<u8> {
        let (alias, _) = self.seen.atlas.expect("an atlas");
        let mut inst = Vec::new();
        let mut x = 4.0;
        for e in glyphs {
            let [tx, ty, tw, th] = e.rect.map(|v| v as f32);
            inst.extend_from_slice(&[x / W as f32, 4.0 / H as f32, tw / W as f32, th / H as f32, tx, ty, tw, th]);
            x += tw + 4.0;
        }
        // frames until the draw is drawn (pipeline variants for the window are built lazily)
        loop {
            self.frame += 1;
            let n = self.frame;
            let before = self.hal.with_world(|wd| wd.session(self.sid).unwrap().stats.skipped_draws);
            self.w.frame_begin(n, FrameMeta::default());
            self.w.pass(Pass { color: [Id::NONE; 4], depth: Id::NONE, load: [pass_load::CLEAR; 5], clear: [[0.0, 0.0, 0.0, 0.0]; 4], viewport: [0.0, 0.0, W as f32, H as f32], dpi: 1.0, ..Default::default() });
            let tex = self.w.put_ids(&[alias]);
            // SAFETY: `inst` lives, unchanged, until `submit` below has returned
            self.w.draw(Draw { pipeline: PIPE, vb: QUAD, ib: INDEX, index: [0, 6], inst: Inst::inline(unsafe { Slice::of(&inst[..]) }, glyphs.len() as u32), tex, scissor: [f32::NAN; 4], bounds: [f32::NAN; 4], ..Default::default() });
            self.w.frame_end(WIN, target::WINDOW);
            self.w.snapshot(WIN, 7);
            self.seen.readback = None;
            self.submit();
            self.until("the readback", |s| s.readback.is_some() && s.done >= n);
            if self.seen.partial != n && self.hal.with_world(|wd| wd.session(self.sid).unwrap().stats.skipped_draws) == before {
                return self.seen.readback.take().unwrap();
            }
            assert!(n < 200, "the draw stayed skipped");
        }
    }

    /// Each glyph's window pixels are its pixels in the host's atlas (RGBA against BGRA).
    fn assert_drawn(&self, glyphs: &[GlyphEntry], shot: &[u8], when: &str) {
        let mut x = 4;
        for e in glyphs {
            let want = self.hal.with_world(|w| w.font_service().raster_rect_bytes(e.rect)).expect("atlas");
            let (tw, th) = (e.rect[2] as usize, e.rect[3] as usize);
            let mut wrong = 0;
            for j in 0..th {
                for i in 0..tw {
                    let at = ((4 + j) * W as usize + x + i) * 4;
                    let have = [shot[at + 2], shot[at + 1], shot[at], shot[at + 3]];
                    let k = (j * tw + i) * 4;
                    if have != want[k..k + 4] {
                        wrong += 1;
                    }
                }
            }
            assert_eq!(wrong, 0, "{when}: glyph {} (rect {:?}) drew {wrong} of {} pixels wrong", e.glyph, e.rect, tw * th);
            assert!(want.chunks(4).any(|p| p[3] != 0), "glyph {} has ink", e.glyph);
            x += tw + 4;
        }
    }
}

/// For an app's session, then for the host's (`makepad run`'s boot renderer draws from the
/// atlas the same way, and its kept frames are replayed after the atlas moved).
#[test]
fn glyphs_drawn_from_the_lent_atlas_stay_right_across_growth_and_shrink() {
    for host in [false, true] {
        if !draw_across_growth_and_shrink(host) {
            eprintln!("SKIP: no Metal device");
            return;
        }
    }
}

/// False when there's no Metal device.
fn draw_across_growth_and_shrink(host: bool) -> bool {
    let hal = Hal::new();
    let req = OpenReq { backend: backend_id::METAL, flags: open_flags::OFFSCREEN_WINDOWS, dpi_hint: 1.0, ..Default::default() };
    let info = if host { hal.open_host(&req) } else { hal.open(&req) };
    if info.backend != backend_id::METAL {
        return false;
    }
    let sid = info.session;
    assert_eq!(sid == 0, host);
    let mut d = Device { hal: hal.clone(), sid, store: EventStore::new(sid, 512, 1 << 24), w: CmdWriter::new(sid), seen: Seen::default(), frame: 0 };
    d.w.window_create(WIN, window_kind::NORMAL, Id::NONE, [0.0, 0.0, W as f32, H as f32], false);
    let quad: Vec<u8> = [0.0f32, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0].iter().flat_map(|f| f.to_le_bytes()).collect();
    d.w.buffer_create(QUAD, buffer_kind::VERTEX, quad.len() as u64, Attr::default());
    d.w.buffer_update(QUAD, 0, &quad);
    let idx: Vec<u8> = [0u16, 1, 2, 0, 2, 3].iter().flat_map(|i| i.to_le_bytes()).collect();
    d.w.buffer_create(INDEX, buffer_kind::INDEX, idx.len() as u64, Attr::default());
    d.w.buffer_update(INDEX, 0, &idx);
    let none = pipeline_binding::NONE;
    let blob = CmdWriter::pipeline_blob(PipelineDesc { outputs: 1, instance_stride: 8, color_formats: [format::BGRA8, 0, 0, 0], ..Default::default() }, MSL.as_bytes(), &[none, none, none, none, none, 1], &[8]);
    d.w.pipeline_create(PIPE, dialect::MSL, Attr::default(), &blob);
    d.w.open_font(1, 0, &[], FontSource::Path(EMOJI));
    d.submit();
    d.until("the pipeline", |s| s.pipelines == 1);

    let emoji = emoji_ids(300);
    d.request(&emoji[..5]);
    assert_eq!(d.seen.atlas.map(|a| a.1), Some(2048));
    let first: Vec<GlyphEntry> = emoji[..5].iter().map(|g| d.seen.ready[g]).collect();
    assert!(first.iter().all(|e| e.atlas_kind == glyph::COLOR));
    let shot = d.draw(&first);
    d.assert_drawn(&first, &shot, "before the atlas grew");

    // far more than 2048x2048 holds: the atlas moves to a texture twice as tall
    for batch in emoji[5..].chunks(50) {
        d.request(batch);
    }
    assert_eq!(d.seen.atlas.map(|a| a.1), Some(4096));
    assert!(d.seen.evicted.is_empty(), "it grew instead");
    assert_eq!(hal.font_stats().growths, 1);
    let grown: Vec<GlyphEntry> = d.seen.ready.values().filter(|e| e.rect[1] + e.rect[3] > 2048).take(3).copied().collect();
    assert_eq!(grown.len(), 3, "glyphs in the new rows");
    let mut both: Vec<GlyphEntry> = first[..2].to_vec();
    both.extend(&grown);
    for e in &both[..2] {
        assert_eq!((d.seen.ready[&e.glyph].slot, d.seen.ready[&e.glyph].rect), (e.slot, e.rect), "the old glyphs kept their slots");
    }
    let shot = d.draw(&both);
    d.assert_drawn(&both, &shot, "after the atlas grew");

    // in the background it goes back to its base size and gives every glyph back
    hal.with_world(|w| w.font_service().app_backgrounded());
    d.until("the smaller atlas", |s| s.atlas.is_some_and(|a| a.1 == 2048));
    assert_eq!(hal.font_stats().shrinks, 1);
    assert!(both.iter().all(|e| d.seen.evicted.contains(&e.slot)), "taken back");
    // back from the background: they come back into the smaller texture
    hal.with_world(|w| w.font_service().app_foregrounded());
    let again: Vec<u32> = both.iter().map(|e| e.glyph).collect();
    for g in &again {
        d.seen.ready.remove(g);
    }
    d.request(&again);
    let back: Vec<GlyphEntry> = again.iter().map(|g| d.seen.ready[g]).collect();
    let shot = d.draw(&back);
    d.assert_drawn(&back, &shot, "after the atlas shrank");
    true
}
