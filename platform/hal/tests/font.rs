//! The font service on the headless backend: sessions share one raster per glyph through the
//! host's atlas, the atlas outlives the sessions, a full atlas grows and later shrinks back, and
//! one that can't grow gives slots back.

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

    /// The slots of `glyphs` this session holds.
    fn slots(&self, glyphs: &[u32], dpxs: f32, kind: u32) -> Vec<(u32, u32)> {
        glyphs.iter().filter_map(|g| self.seen.ready.get(&(*g, dpxs.to_bits(), kind))).filter(|e| e.slot != 0).map(|e| (e.slot, e.slot_gen)).collect()
    }

    /// Tells the host these glyphs were drawn, as a client's touch does: what it drew before and
    /// not since is then the least recently used.
    fn touch(&self, glyphs: &[u32], dpxs: f32, kind: u32) {
        let slots = self.slots(glyphs, dpxs, kind);
        self.send(|w| w.touch_glyphs(&slots));
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
    // a device whose largest texture is this atlas: it can't grow
    hal.with_world(|w| w.max_texture = 256);
    a.open_font(1, PLEX);
    let all: Vec<u32> = (1..400).collect();
    let dpxs = 40.0;
    // far more glyphs than the atlas holds, in batches, each drawn in its turn
    for batch in all.chunks(40) {
        a.request(1, kind::SDF, batch, dpxs);
        assert!(a.wait(20, |s| batch.iter().all(|g| s.ready.contains_key(&(*g, dpxs.to_bits(), kind::SDF)))));
        a.touch(batch, dpxs, kind::SDF);
    }
    let stats = hal.font_stats();
    assert!(stats.evictions > 0, "the atlas was too small for everything");
    assert_eq!(stats.no_room, 0, "what was drawn before made room");
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
    hal.with_world(|w| w.max_texture = 128);
    a.open_font(1, PLEX);
    let dpxs = 11.0;
    let all: Vec<u32> = (1..500).collect();
    let mut reference: HashMap<u32, Vec<u8>> = HashMap::new();
    let mut checked = 0;
    for batch in all.chunks(25) {
        a.request(1, kind::SDF, batch, dpxs);
        assert!(a.wait(30, |s| batch.iter().all(|g| s.ready.contains_key(&(*g, dpxs.to_bits(), kind::SDF)))));
        a.touch(batch, dpxs, kind::SDF);
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

/// Big colour emoji, about 140x132 texels each: a 2048x2048 atlas holds some 200 of them.
const EMOJI_DPXS: f32 = 96.0;

/// The first `n` colour emoji of the emoji font.
fn emoji_ids(n: usize) -> Vec<u32> {
    let text: String = (0x1F300u32..0x1FB00).filter_map(char::from_u32).collect();
    let ids = glyph_ids(EMOJI, &text);
    assert!(ids.len() >= n, "the font has {} emoji there", ids.len());
    ids[..n].to_vec()
}

/// Asks for `glyphs` 50 at a time, as a scrolling grid does, and waits for every answer.
fn request_all(c: &mut Client, font: u32, glyphs: &[u32], dpxs: f32) {
    for batch in glyphs.chunks(50) {
        c.request(font, kind::AUTO, batch, dpxs);
        assert!(c.wait(60, |s| batch.iter().all(|g| s.ready.contains_key(&(*g, dpxs.to_bits(), kind::AUTO)))));
    }
}

/// Each glyph's pixels in an atlas tall enough that it never had to grow.
fn reference_pixels(glyphs: &[u32], dpxs: f32) -> HashMap<u32, Vec<u8>> {
    let hal = Hal::new();
    hal.with_world(|w| w.font_service().set_atlas_size(Size::new(2048, 8192)));
    let mut c = Client::new(&hal, 99);
    c.open_font(1, EMOJI);
    request_all(&mut c, 1, glyphs, dpxs);
    assert_eq!(hal.font_stats().growths, 0);
    glyphs.iter().map(|g| (*g, hal.with_world(|w| w.font_service().raster_rect_bytes(c.entry(*g, dpxs, kind::AUTO).rect)).expect("atlas"))).collect()
}

/// Every glyph the session holds shows its own pixels in the host's atlas.
fn assert_own_pixels(hal: &Hal, c: &Client, reference: &HashMap<u32, Vec<u8>>) {
    let evicted: std::collections::HashSet<u32> = c.seen.evicted.iter().map(|(s, _)| *s).collect();
    let mut checked = 0;
    for e in c.seen.ready.values().filter(|e| e.atlas_kind == glyph::COLOR && !evicted.contains(&e.slot)) {
        let have = hal.with_world(|w| w.font_service().raster_rect_bytes(e.rect)).expect("atlas");
        assert!(have == reference[&e.glyph], "glyph {} (slot {}, rect {:?}) shows other pixels", e.glyph, e.slot, e.rect);
        checked += 1;
    }
    assert!(checked > 0);
}

/// The host texture a session's atlas id stands for, and its size and uploaded bytes.
fn atlas_texture(hal: &Hal, c: &Client) -> (Id, u32, u32, u64) {
    let alias = c.seen.atlases.iter().rev().find(|x| x.0 == atlas::RASTER).expect("an atlas").1;
    hal.with_world(|w| {
        let lent = w.session(c.sid).unwrap().textures.get(c.sid, alias).unwrap().val.clone();
        let tex = lent.lent.expect("lent by the host").tex;
        let host = w.session(0).unwrap().textures.get(0, tex).expect("the host texture is alive").val.clone();
        assert_eq!((lent.w, lent.h), (host.w, host.h), "the session's id has the texture's size");
        (tex, host.w, host.h, host.uploaded)
    })
}

/// A dense emoji grid overflows the atlas: it moves to a texture twice as tall instead of taking
/// slots back, then twice as tall again, and every glyph keeps its slot and its pixels (a draw
/// recorded before a growth keeps its texels, and the shader divides them by the new size).
#[test]
fn a_full_atlas_grows_and_keeps_every_glyph_where_it_was() {
    let emoji = emoji_ids(700);
    let reference = reference_pixels(&emoji, EMOJI_DPXS);
    let hal = Hal::new();
    let mut a = Client::new(&hal, 1);
    a.open_font(1, EMOJI);
    // the atlas height each glyph was delivered at
    let mut delivered_at: HashMap<u32, u32> = HashMap::new();
    for batch in emoji.chunks(50) {
        request_all(&mut a, 1, batch, EMOJI_DPXS);
        let height = a.seen.atlases.last().unwrap().3;
        delivered_at.extend(batch.iter().map(|g| (*g, height)));
    }
    let stats = hal.font_stats();
    assert_eq!(stats.growths, 2, "twice as tall each time");
    assert_eq!((stats.evictions, a.seen.evicted.len(), stats.no_room), (0, 0, 0), "no slot was taken back");
    assert_eq!(stats.raster_bytes, 2048 * 8192 * 4);
    let heights: Vec<u32> = a.seen.atlases.iter().map(|x| x.3).collect();
    assert_eq!(heights, vec![2048, 4096, 8192], "64 MiB on a GPU of unknown size");
    let (first, last) = (a.seen.atlases[0], *a.seen.atlases.last().unwrap());
    assert_eq!((last.1, last.4), (first.1, first.4), "the same id and generation: every entry stays valid");
    let (_, w, h, uploaded) = atlas_texture(&hal, &a);
    assert_eq!((w, h), (2048, 8192));
    assert!(uploaded >= (2048 + 4096) * 2048 * 4, "what each old texture held went to the new one");
    // the entries delivered before each growth still show their own pixels
    for height in [2048, 4096] {
        assert!(delivered_at.values().any(|h| *h == height), "glyphs delivered while it was {height} tall");
    }
    assert!(a.seen.ready.values().any(|e| e.rect[1] + e.rect[3] > 4096), "some went into the last rows");
    assert_own_pixels(&hal, &a, &reference);
}

/// A grown atlas goes back to its base size after five minutes without a new glyph, once no
/// session holds one in its extra rows. The rest keep their slots and pixels, one taken back
/// comes back when asked, and the host's own textures keep their ids through every move.
#[test]
fn a_grown_atlas_goes_back_to_its_base_size_once_idle() {
    let emoji = emoji_ids(300);
    let reference = reference_pixels(&emoji, EMOJI_DPXS);
    let hal = Hal::new();
    let mut a = Client::new(&hal, 1);
    // the host's own textures, at low ids like `makepad run`'s boot renderer uses
    let host_ids: Vec<Id> = (1..=8).map(|i| Id::new(i, 1)).collect();
    let mut host = CmdWriter::new(0);
    for id in &host_ids {
        host.texture_create(*id, usage::SAMPLED, format::RGBA8, 64, 32, [1, 1], Attr::default());
    }
    hal.submit(&host.buf());
    a.open_font(1, EMOJI);
    request_all(&mut a, 1, &emoji, EMOJI_DPXS);
    assert_eq!(hal.font_stats().growths, 1);
    let held = a.seen.ready.clone();
    let past: Vec<u32> = held.values().filter(|e| e.rect[1] + e.rect[3] > 2048).map(|e| e.glyph).collect();
    assert!(!past.is_empty());
    // idle long enough, but the session still holds glyphs in the extra rows
    hal.with_world(|w| w.clock.manual = Some(w.now() + 310_000_000_000));
    a.pump(5_000_000);
    assert_eq!(hal.font_stats().shrinks, 0, "it waits while they're held");
    // the session lets go of them, and another idle period goes by
    let slots = a.slots(&past, EMOJI_DPXS, kind::AUTO);
    a.send(|w| w.release_glyphs(&slots));
    for g in &past {
        a.seen.ready.remove(&(*g, EMOJI_DPXS.to_bits(), kind::AUTO));
    }
    hal.with_world(|w| w.clock.manual = Some(w.now() + 290_000_000_000));
    a.pump(5_000_000);
    assert_eq!(hal.font_stats().shrinks, 0, "not idle long enough yet");
    hal.with_world(|w| w.clock.manual = Some(w.now() + 20_000_000_000));
    assert!(a.wait(5, |s| s.atlases.last().is_some_and(|x| x.3 == 2048)));
    let stats = hal.font_stats();
    assert_eq!((stats.shrinks, stats.raster_bytes), (1, 2048 * 2048 * 4));
    assert_eq!(stats.raster_slots, held.len() - past.len(), "only the extra rows' slots are taken back");
    assert!(a.seen.evicted.is_empty(), "nobody held them, so nobody was told");
    assert_eq!(a.seen.atlases.last().unwrap().4, a.seen.atlases[0].4, "the same generation");
    let (tex, _, h, _) = atlas_texture(&hal, &a);
    assert_eq!(h, 2048);
    assert!(tex.index >= LENT_BASE, "the atlas takes no id the host's own commands can");
    assert_own_pixels(&hal, &a, &reference);
    // ask again for one that was taken back: the atlas grows again for it
    let gone = past[0];
    a.request(1, kind::AUTO, &[gone], EMOJI_DPXS);
    assert!(a.wait(20, |s| s.ready.contains_key(&(gone, EMOJI_DPXS.to_bits(), kind::AUTO))));
    let e = a.entry(gone, EMOJI_DPXS, kind::AUTO);
    let have = hal.with_world(|w| w.font_service().raster_rect_bytes(e.rect)).expect("atlas");
    assert!(have == reference[&gone]);
    hal.with_world(|w| {
        let host = w.session(0).unwrap();
        for id in &host_ids {
            let t = host.textures.get(0, *id).expect("the host's texture is still there");
            assert_eq!((t.val.w, t.val.h), (64, 32));
        }
    });
}

/// The app going to the background takes a grown atlas back to its base size at once, through
/// the `Lifecycle` event the shell sends, giving back every glyph, and it doesn't grow again until
/// the app is back. The glyphs come back when asked.
#[test]
fn a_grown_atlas_goes_back_to_its_base_size_in_the_background() {
    let emoji = emoji_ids(300);
    let reference = reference_pixels(&emoji, EMOJI_DPXS);
    let hal = Hal::new();
    let mut a = Client::new(&hal, 1);
    let sid = a.sid;
    let send_lifecycle = |hal: &Hal, kind: u32| {
        hal.with_world(|w| {
            let mut q = EventQueue::new();
            q.lifecycle(kind);
            q.drain_with(|e, p| w.window_event(sid, e, p));
        })
    };
    let forget = |a: &mut Client, glyphs: &[u32]| {
        for g in glyphs {
            a.seen.ready.remove(&(*g, EMOJI_DPXS.to_bits(), kind::AUTO));
        }
    };
    a.open_font(1, EMOJI);
    request_all(&mut a, 1, &emoji[..20], EMOJI_DPXS);
    // in the background before it grew: nothing to give back, now or once it grows later
    send_lifecycle(&hal, lifecycle::BACKGROUND);
    send_lifecycle(&hal, lifecycle::FOREGROUND);
    request_all(&mut a, 1, &emoji, EMOJI_DPXS);
    a.pump(5_000_000);
    assert_eq!((hal.font_stats().growths, hal.font_stats().shrinks), (1, 0));
    let held: Vec<GlyphEntry> = a.seen.ready.values().filter(|e| e.atlas_kind == glyph::COLOR).copied().collect();
    send_lifecycle(&hal, lifecycle::BACKGROUND);
    assert!(a.wait(5, |s| s.atlases.last().is_some_and(|x| x.3 == 2048)));
    assert_eq!(hal.font_stats().shrinks, 1);
    let evicted: std::collections::HashSet<u32> = a.seen.evicted.iter().map(|(s, _)| *s).collect();
    assert!(held.iter().all(|e| evicted.contains(&e.slot)), "every glyph was given back");
    assert_eq!(hal.font_stats().raster_slots, 0);
    // what's asked for while in the background doesn't grow it again
    forget(&mut a, &emoji);
    request_all(&mut a, 1, &emoji, EMOJI_DPXS);
    assert_eq!(hal.font_stats().growths, 1);
    // back from the background: what it shows comes back
    send_lifecycle(&hal, lifecycle::FOREGROUND);
    forget(&mut a, &emoji);
    request_all(&mut a, 1, &emoji, EMOJI_DPXS);
    assert!(emoji.iter().all(|g| a.entry(*g, EMOJI_DPXS, kind::AUTO).atlas_kind == glyph::COLOR));
    assert_own_pixels(&hal, &a, &reference);
}

/// The atlas grows no taller than the device's largest texture; past that it takes back the
/// least recently drawn slots, as a scrolling grid draws each batch in its turn.
#[test]
fn a_full_grown_atlas_takes_slots_back() {
    let emoji = emoji_ids(500);
    let hal = Hal::new();
    let mut a = Client::new(&hal, 1);
    hal.with_world(|w| w.max_texture = 4096);
    a.open_font(1, EMOJI);
    for batch in emoji.chunks(50) {
        request_all(&mut a, 1, batch, EMOJI_DPXS);
        a.touch(batch, EMOJI_DPXS, kind::AUTO);
    }
    let stats = hal.font_stats();
    assert_eq!(stats.growths, 1);
    assert!(stats.evictions > 0 && !a.seen.evicted.is_empty(), "it took slots back once it was full-grown");
    assert_eq!(stats.no_room, 0, "the batches drawn before made room");
    assert_eq!(a.seen.atlases.last().unwrap().3, 4096);
    assert_eq!(stats.raster_bytes, 2048 * 4096 * 4);
}

/// When every glyph left in a full-grown atlas is on screen (the session drew all of them at
/// its last touch), taking one back would only bring it back in place of another. So a new glyph
/// is answered `NO_ROOM` and nothing is taken; once the screen moves on, there's room again.
#[test]
fn glyphs_on_screen_are_not_taken_back_for_more_than_the_atlas_holds() {
    let emoji = emoji_ids(300);
    let reference = reference_pixels(&emoji, EMOJI_DPXS);
    let hal = Hal::new();
    let mut a = Client::new(&hal, 1);
    hal.with_world(|w| w.max_texture = 2048);
    a.open_font(1, EMOJI);
    // one screen of emoji, every one drawn at each touch
    for (i, batch) in emoji.chunks(50).enumerate() {
        request_all(&mut a, 1, batch, EMOJI_DPXS);
        a.touch(&emoji[..(i + 1) * 50], EMOJI_DPXS, kind::AUTO);
    }
    let stats = hal.font_stats();
    let no_room: Vec<u32> = a.seen.ready.values().filter(|e| e.atlas_kind == glyph::NO_ROOM).map(|e| e.glyph).collect();
    assert!(!no_room.is_empty() && stats.no_room == no_room.len() as u64, "what didn't fit was answered NO_ROOM");
    assert_eq!((stats.evictions, a.seen.evicted.len(), stats.growths), (0, 0, 0), "nothing on screen was taken back");
    assert_own_pixels(&hal, &a, &reference);
    // the screen moves on: the first batch isn't drawn any more, so those glyphs make room
    a.touch(&emoji[50..], EMOJI_DPXS, kind::AUTO);
    for g in &no_room {
        a.seen.ready.remove(&(*g, EMOJI_DPXS.to_bits(), kind::AUTO));
    }
    request_all(&mut a, 1, &no_room[..no_room.len().min(20)], EMOJI_DPXS);
    assert!(no_room[..no_room.len().min(20)].iter().all(|g| a.entry(*g, EMOJI_DPXS, kind::AUTO).atlas_kind == glyph::COLOR));
    let first: std::collections::HashSet<u32> = a.slots(&emoji[..50], EMOJI_DPXS, kind::AUTO).into_iter().map(|s| s.0).collect();
    assert!(!a.seen.evicted.is_empty() && a.seen.evicted.iter().all(|(s, _)| first.contains(s)), "only what left the screen was taken back");
    assert_own_pixels(&hal, &a, &reference);
}

/// A device that refuses the taller texture isn't asked again at every overflow: once per idle
/// period, or once the app has been in the background.
#[test]
fn a_refused_growth_is_not_tried_again_at_once() {
    let emoji = emoji_ids(300);
    let hal = Hal::new();
    // a tall narrow atlas whose double is past the headless device's largest texture, though
    // the device claims it takes it
    hal.with_world(|w| w.font_service().set_atlas_size(Size::new(256, 16384)));
    let mut a = Client::new(&hal, 1);
    hal.with_world(|w| w.max_texture = 65536);
    let refused = |hal: &Hal| hal.with_world(|w| w.session(0).unwrap().stats.bad_args);
    a.open_font(1, EMOJI);
    let grid = |a: &mut Client, glyphs: &[u32]| {
        for batch in glyphs.chunks(25) {
            for g in batch {
                a.seen.ready.remove(&(*g, EMOJI_DPXS.to_bits(), kind::AUTO));
            }
            request_all(a, 1, batch, EMOJI_DPXS);
            a.touch(batch, EMOJI_DPXS, kind::AUTO);
        }
    };
    grid(&mut a, &emoji[..200]);
    let stats = hal.font_stats();
    assert!(stats.evictions > 1 && stats.growths == 0, "it took slots back instead");
    assert_eq!(refused(&hal), 1, "one refused texture for many overflows");
    // an idle period later it asks once more
    hal.with_world(|w| w.clock.manual = Some(w.now() + 310_000_000_000));
    grid(&mut a, &emoji[200..]);
    assert_eq!(refused(&hal), 2);
    // and once the app is back from the background
    hal.with_world(|w| {
        w.font_service().app_backgrounded();
        w.font_service().app_foregrounded();
    });
    grid(&mut a, &emoji[..100]);
    assert_eq!((refused(&hal), hal.font_stats().growths), (3, 0));
}

/// Slots no session holds any more go before the atlas grows: a grid that was scrolled past and
/// let go of makes room for the next one without taking more memory.
#[test]
fn let_go_slots_make_room_before_the_atlas_grows() {
    let emoji = emoji_ids(400);
    let hal = Hal::new();
    let mut a = Client::new(&hal, 1);
    a.open_font(1, EMOJI);
    let (first, second) = emoji.split_at(200);
    request_all(&mut a, 1, first, EMOJI_DPXS);
    let slots: Vec<(u32, u32)> = first.iter().map(|g| a.entry(*g, EMOJI_DPXS, kind::AUTO)).map(|e| (e.slot, e.slot_gen)).collect();
    a.send(|w| w.release_glyphs(&slots));
    request_all(&mut a, 1, second, EMOJI_DPXS);
    let stats = hal.font_stats();
    assert_eq!(stats.growths, 0);
    assert!(stats.evictions > 0, "the let-go slots were taken back");
    assert!(a.seen.evicted.is_empty(), "nobody held them, so nobody was told");
}
