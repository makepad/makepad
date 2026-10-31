//! The font service on the headless backend: sessions share one raster per glyph through the
//! host's atlas, the atlas outlives the sessions, and a small atlas gives slots back.

use makepad_font_core::geom::Size;
use makepad_font_core::rustybuzz::ttf_parser::Face;
use makepad_hal::font::wire::{self, atlas, glyph, kind};
use makepad_hal::font::{FontSource, GlyphEntry};
use makepad_hal::*;
use std::collections::HashMap;
use std::time::{Duration, Instant};

const PLEX: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../widgets/resources/IBMPlexSans-Text.ttf");
const EMOJI: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../widgets/resources/NotoColorEmoji.ttf");

fn glyph_ids(path: &str, text: &str) -> Vec<u32> {
    let bytes = std::fs::read(path).expect("font file");
    let face = Face::parse(&bytes, 0).expect("font parses");
    let mut ids: Vec<u32> = text.chars().filter_map(|c| face.glyph_index(c)).map(|g| g.0 as u32).collect();
    ids.sort();
    ids.dedup();
    ids
}

#[derive(Default)]
struct Seen {
    opened: Vec<(u32, u32, u64, u64, String)>,
    atlases: Vec<(u32, Id, u32, u32, u32)>,
    ready: HashMap<(u32, u32, u32), GlyphEntry>,
    upgrades: Vec<GlyphEntry>,
    evicted: Vec<(u32, u32)>,
    ready_events: usize,
}

struct Client<'a> {
    hal: &'a Hal,
    sid: u32,
    store: EventStore,
    seen: Seen,
}

impl<'a> Client<'a> {
    fn new(hal: &'a Hal, key: u64) -> Client<'a> {
        let info = hal.open(&OpenReq { backend: backend_id::HEADLESS, app_key: key, flags: 0, dpi_hint: 2.0, parent: 0 });
        assert_ne!(info.session, 0);
        Client { hal, sid: info.session, store: EventStore::new(info.session, 512, 1 << 20), seen: Seen::default() }
    }

    fn send(&self, f: impl FnOnce(&mut CmdWriter)) {
        let mut w = CmdWriter::new(self.sid);
        f(&mut w);
        self.hal.submit(&w.buf());
    }

    fn open_font(&self, font: u32, path: &str) {
        self.send(|w| w.open_font(font, 0, &[], FontSource::Path(path)));
    }

    fn request(&self, font: u32, kind: u32, glyphs: &[u32], dpxs: f32) {
        let list: Vec<(u32, f32)> = glyphs.iter().map(|g| (*g, dpxs)).collect();
        self.send(|w| w.request_glyphs(font, kind, &list));
    }

    fn pump(&mut self, timeout_ns: u64) -> u32 {
        let mut buf = self.store.buf();
        let n = self.hal.pump(timeout_ns, &mut buf);
        for (_, e) in self.store.reader(n, buf.arena_len) {
            match e {
                HalEvent::FontOpened { font, status, len, hash, path } => self.seen.opened.push((font, status, len, hash, String::from_utf8_lossy(path).into_owned())),
                HalEvent::AtlasInfo { kind, texture, w, h, generation } => self.seen.atlases.push((kind, texture, w, h, generation)),
                HalEvent::GlyphsReady { count, entries, .. } => {
                    self.seen.ready_events += 1;
                    for e in wire::read_entries(entries, count) {
                        if e.replaces != 0 {
                            self.seen.upgrades.push(e);
                        } else {
                            self.seen.ready.insert((e.glyph, e.dpxs_bits, e.kind), e);
                        }
                    }
                }
                HalEvent::AtlasEvicted { kind: _, generation, count, slots } => {
                    for (slot, gen) in wire::read_slots(slots, count) {
                        assert_eq!(gen, generation);
                        self.seen.evicted.push((slot, gen));
                    }
                }
                _ => {}
            }
        }
        n
    }

    /// Pumps until `done` holds; false on timeout.
    fn wait(&mut self, secs: u64, done: impl Fn(&Seen) -> bool) -> bool {
        let end = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < end {
            if done(&self.seen) {
                return true;
            }
            self.pump(20_000_000);
        }
        done(&self.seen)
    }

    fn entry(&self, glyph: u32, dpxs: f32, kind: u32) -> GlyphEntry {
        *self.seen.ready.get(&(glyph, dpxs.to_bits(), kind)).unwrap_or_else(|| panic!("no entry for glyph {glyph}"))
    }
}

#[test]
fn a_font_is_mapped_once_for_every_session() {
    let hal = Hal::new();
    let mut a = Client::new(&hal, 1);
    let mut b = Client::new(&hal, 2);
    a.open_font(7, PLEX);
    b.open_font(3, PLEX);
    assert!(a.wait(5, |s| !s.opened.is_empty()));
    assert!(b.wait(5, |s| !s.opened.is_empty()));
    let (fa, fb) = (&a.seen.opened[0], &b.seen.opened[0]);
    assert_eq!((fa.0, fb.0), (7, 3), "each answer names the app's own id");
    assert_eq!((fa.1, fb.1), (wire::font_status::OPENED, wire::font_status::OPENED));
    assert_eq!(fa.2, std::fs::metadata(PLEX).unwrap().len(), "the length of the file");
    assert_eq!((fa.2, fa.3), (fb.2, fb.3), "the same content");
    assert!(fa.4.ends_with("IBMPlexSans-Text.ttf"));
    let stats = hal.font_stats();
    assert_eq!((stats.opens, stats.maps, stats.shared_opens), (2, 1, 1));

    // not a file, not a font
    a.open_font(9, "/no/such/font.ttf");
    a.send(|w| w.open_font(10, 0, &[], FontSource::Bytes(b"not a font at all")));
    assert!(a.wait(5, |s| s.opened.len() == 3));
    assert_eq!(a.seen.opened[1].1, wire::font_status::MISSING);
    assert_eq!(a.seen.opened[2].1, wire::font_status::BAD);
}

#[test]
fn two_sessions_share_one_raster_per_glyph() {
    let hal = Hal::new();
    let mut a = Client::new(&hal, 1);
    let mut b = Client::new(&hal, 2);
    a.open_font(1, PLEX);
    b.open_font(1, PLEX);
    let mine = glyph_ids(PLEX, "Hello world");
    let yours = glyph_ids(PLEX, "world wide web");
    let both: std::collections::HashSet<u32> = mine.iter().chain(&yours).copied().collect();
    let dpxs = 16.0;
    a.request(1, kind::AUTO, &mine, dpxs);
    b.request(1, kind::AUTO, &yours, dpxs);
    assert!(a.wait(10, |s| mine.iter().all(|g| s.ready.contains_key(&(*g, dpxs.to_bits(), kind::AUTO)))));
    assert!(b.wait(10, |s| yours.iter().all(|g| s.ready.contains_key(&(*g, dpxs.to_bits(), kind::AUTO)))));

    // the atlas texture is the host's, lent to each session
    for c in [&a, &b] {
        let (k, alias, w, h, generation) = c.seen.atlases[0];
        assert_eq!((k, w, h), (atlas::RASTER, 2048, 2048));
        assert!(alias.index >= LENT_BASE && generation != 0);
        let lent = hal.with_world(|world| world.session(c.sid).unwrap().textures.get(c.sid, alias).map(|e| e.val.lent).unwrap());
        assert_eq!(lent.map(|l| l.child), Some(0), "an alias of a host texture");
    }
    // a glyph both asked for is one slot
    let shared = glyph_ids(PLEX, "o")[0];
    assert!(mine.contains(&shared) && yours.contains(&shared), "both asked for it");
    let (ea, eb) = (a.entry(shared, dpxs, kind::AUTO), b.entry(shared, dpxs, kind::AUTO));
    assert_eq!((ea.slot, ea.rect, ea.plane), (eb.slot, eb.rect, eb.plane));
    assert_eq!(ea.atlas_kind, glyph::GRAY);
    let stats = hal.font_stats();
    // rendered once per distinct glyph, whoever asked (a space has no image)
    let key = |g: &u32| (*g, dpxs.to_bits(), kind::AUTO);
    let with_image = both
        .iter()
        .filter(|g| [&a, &b].iter().any(|c| c.seen.ready.get(&key(g)).is_some_and(|e| e.atlas_kind != glyph::EMPTY)))
        .count();
    assert_eq!(stats.rasters as usize, with_image, "one raster per glyph image");
    assert!(stats.rasters < (mine.len() + yours.len()) as u64);
    assert_eq!(stats.raster_bytes, 2048 * 2048 * 4);
    // the ledger knows the atlas, under the host
    let report = hal.report(0, 0).unwrap();
    assert!(report.total_bytes >= 2048 * 2048 * 4);
}

#[test]
fn glyphs_outlive_the_session_that_asked() {
    let hal = Hal::new();
    let glyphs = glyph_ids(PLEX, "Atlas");
    let dpxs = 18.0;
    let first = {
        let mut a = Client::new(&hal, 1);
        a.open_font(1, PLEX);
        a.request(1, kind::AUTO, &glyphs, dpxs);
        assert!(a.wait(10, |s| glyphs.iter().all(|g| s.ready.contains_key(&(*g, dpxs.to_bits(), kind::AUTO)))));
        let e = glyphs.iter().map(|g| a.entry(*g, dpxs, kind::AUTO)).collect::<Vec<_>>();
        hal.close(a.sid);
        e
    };
    let rasters = hal.font_stats().rasters;
    assert!(rasters > 0);
    // the session is gone; its glyphs are not
    assert_eq!(hal.font_stats().raster_slots as u64, rasters);
    let mut b = Client::new(&hal, 1);
    b.open_font(5, PLEX);
    b.request(5, kind::AUTO, &glyphs, dpxs);
    assert!(b.wait(10, |s| glyphs.iter().all(|g| s.ready.contains_key(&(*g, dpxs.to_bits(), kind::AUTO)))));
    for (g, before) in glyphs.iter().zip(&first) {
        let now = b.entry(*g, dpxs, kind::AUTO);
        assert_eq!((now.slot, now.rect), (before.slot, before.rect), "the same slot as before the relaunch");
    }
    assert_eq!(hal.font_stats().rasters, rasters, "nothing was rendered again");
    assert_eq!(hal.font_stats().maps, 2, "the file was unmapped with its last session and mapped again");
}

#[test]
fn a_small_atlas_gives_slots_back_least_recently_used_first() {
    let hal = Hal::new();
    hal.with_world(|w| w.font_service().set_atlas_size(Size::new(256, 256)));
    let mut a = Client::new(&hal, 1);
    a.open_font(1, PLEX);
    let all: Vec<u32> = (1..400).collect();
    let dpxs = 40.0;
    // far more glyphs than the atlas holds, in batches
    for batch in all.chunks(40) {
        a.request(1, kind::SDF, batch, dpxs);
        assert!(a.wait(20, |s| batch.iter().all(|g| s.ready.contains_key(&(*g, dpxs.to_bits(), kind::SDF)))));
    }
    let stats = hal.font_stats();
    assert!(stats.evictions > 0, "the atlas was too small for everything");
    assert!(!a.seen.evicted.is_empty(), "the session was told");
    assert!(stats.raster_slots < all.len());
    assert!(a.seen.atlases.iter().all(|x| x.2 == 256));
    // the first glyphs were the oldest: they are among the evicted, and asking again works
    let evicted_slots: std::collections::HashSet<u32> = a.seen.evicted.iter().map(|(s, _)| *s).collect();
    // glyph ids from the start of the font: the first with an image went in first
    let first = *all.iter().find(|g| a.entry(**g, dpxs, kind::SDF).slot != 0).expect("a glyph with an image");
    let slot = a.entry(first, dpxs, kind::SDF).slot;
    assert!(evicted_slots.contains(&slot), "the least recently used slot went first");
    a.seen.ready.remove(&(first, dpxs.to_bits(), kind::SDF));
    a.request(1, kind::SDF, &[first], dpxs);
    assert!(a.wait(10, |s| s.ready.contains_key(&(first, dpxs.to_bits(), kind::SDF))));
    assert_ne!(a.entry(first, dpxs, kind::SDF).slot, slot, "a new slot, never the old id");
    // a slot never names two glyphs: every delivered slot id is unique
    let mut slots: Vec<u32> = a.seen.ready.values().filter(|e| e.atlas_kind != glyph::EMPTY).map(|e| e.slot).collect();
    slots.sort();
    let n = slots.len();
    slots.dedup();
    assert_eq!(slots.len(), n);
}

#[test]
fn colour_bitmaps_and_multichannel_upgrades() {
    let hal = Hal::new();
    let mut a = Client::new(&hal, 1);
    a.open_font(1, PLEX);
    a.open_font(2, EMOJI);
    let big = glyph_ids(PLEX, "g");
    let emoji = glyph_ids(EMOJI, "\u{1f600}");
    assert!(!emoji.is_empty());
    a.request(1, kind::AUTO, &big, 80.0);
    a.request(2, kind::AUTO, &emoji, 64.0);
    assert!(a.wait(20, |s| s.ready.contains_key(&(big[0], 80.0f32.to_bits(), kind::AUTO)) && s.ready.contains_key(&(emoji[0], 64.0f32.to_bits(), kind::AUTO))));
    let e = a.entry(emoji[0], 64.0, kind::AUTO);
    assert_eq!(e.atlas_kind, glyph::COLOR);
    assert_eq!(e.padding, 2);
    // the outline arrives as a plain field, then the multi-channel one replaces it
    let first = a.entry(big[0], 80.0, kind::AUTO);
    assert_eq!(first.atlas_kind, glyph::GRAY);
    assert_eq!(first.padding, 4);
    assert!(a.wait(20, |s| !s.upgrades.is_empty()), "the multi-channel field follows");
    let up = a.seen.upgrades[0];
    assert_eq!((up.atlas_kind, up.replaces), (glyph::MSDF, first.slot));
    assert_eq!((up.rect[2], up.rect[3]), (first.rect[2], first.rect[3]), "the same pixels, the other field");
    assert_eq!(hal.font_stats().upgrades, 1);
}

#[test]
fn slug_curves_share_one_texture() {
    let hal = Hal::new();
    let mut a = Client::new(&hal, 1);
    let mut b = Client::new(&hal, 2);
    a.open_font(1, PLEX);
    b.open_font(1, PLEX);
    let glyphs = glyph_ids(PLEX, "Slug");
    // curves do not depend on the size: the answer carries none
    a.request(1, kind::SLUG, &glyphs, 100.0);
    b.request(1, kind::SLUG, &glyphs, 250.0);
    assert!(a.wait(10, |s| glyphs.iter().all(|g| s.ready.contains_key(&(*g, 0, kind::SLUG)))));
    assert!(b.wait(10, |s| glyphs.iter().all(|g| s.ready.contains_key(&(*g, 0, kind::SLUG)))));
    for g in &glyphs {
        let (ea, eb) = (a.entry(*g, 0.0, kind::SLUG), b.entry(*g, 0.0, kind::SLUG));
        assert_eq!(ea.atlas_kind, glyph::SLUG);
        assert!(ea.rect[1] > 0, "curves");
        assert_eq!((ea.slot, ea.rect), (eb.slot, eb.rect), "the curves do not depend on the size");
        assert!(ea.scale > 0.0 && ea.size_h > 0.0);
    }
    let stats = hal.font_stats();
    assert_eq!(stats.rasters as usize, glyphs.len(), "one curve set per glyph");
    assert_eq!(stats.slug_glyphs, glyphs.len());
    assert!(a.seen.atlases.iter().any(|x| x.0 == atlas::SLUG));
}


/// The plane `plane` (0 = R .. 3 = A of BGRA bytes) of a rectangle's pixels, one byte per pixel.
fn plane_of(bytes: &[u8], plane: u32) -> Vec<u8> {
    let at = match plane { 0 => 2, 1 => 1, 2 => 0, _ => 3 };
    bytes.chunks(4).map(|p| p[at]).collect()
}

/// What a glyph looks like in an atlas big enough that nothing was ever taken back.
fn reference_plane(glyph: u32, dpxs: f32) -> (Vec<u8>, [u32; 4]) {
    let hal = Hal::new();
    let mut c = Client::new(&hal, 99);
    c.open_font(1, PLEX);
    c.request(1, kind::SDF, &[glyph], dpxs);
    assert!(c.wait(20, |s| s.ready.contains_key(&(glyph, dpxs.to_bits(), kind::SDF))));
    let e = c.entry(glyph, dpxs, kind::SDF);
    let bytes = hal.with_world(|w| w.font_service().raster_rect_bytes(e.rect)).expect("atlas");
    (plane_of(&bytes, e.plane), e.rect)
}

/// Small glyphs share rectangles across the four planes; after evictions and reclaims every
/// glyph the session still holds must show its own distance field, never another glyph's.
#[test]
fn held_small_glyphs_keep_their_own_pixels_through_evictions() {
    let hal = Hal::new();
    hal.with_world(|w| w.font_service().set_atlas_size(Size::new(128, 128)));
    let mut a = Client::new(&hal, 1);
    a.open_font(1, PLEX);
    let dpxs = 11.0;
    let all: Vec<u32> = (1..500).collect();
    let mut reference: HashMap<u32, Vec<u8>> = HashMap::new();
    let mut checked = 0;
    for batch in all.chunks(25) {
        a.request(1, kind::SDF, batch, dpxs);
        assert!(a.wait(30, |s| batch.iter().all(|g| s.ready.contains_key(&(*g, dpxs.to_bits(), kind::SDF)))));
        // let retirements finish and any further uploads settle
        for _ in 0..5 {
            a.pump(5_000_000);
        }
        let evicted: std::collections::HashSet<u32> = a.seen.evicted.iter().map(|(s, _)| *s).collect();
        let held: Vec<(u32, GlyphEntry)> = a
            .seen
            .ready
            .values()
            .filter(|e| e.atlas_kind == glyph::GRAY && e.slot != 0 && !evicted.contains(&e.slot))
            .map(|e| (e.glyph, *e))
            .collect();
        for (g, e) in held {
            let bytes = hal.with_world(|w| w.font_service().raster_rect_bytes(e.rect)).expect("atlas");
            let have = plane_of(&bytes, e.plane);
            let want = reference.entry(g).or_insert_with(|| reference_plane(g, dpxs).0);
            assert_eq!(&have, want, "glyph {g} (slot {}, rect {:?}, plane {}) shows other pixels", e.slot, e.rect, e.plane);
            checked += 1;
        }
    }
    assert!(hal.font_stats().evictions > 0, "the atlas was too small for everything");
    assert!(checked > 200);
}
