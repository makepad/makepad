//! The thumbnail view-regions contract.
//!
//! A thumbnail is still ONE mandatory picture. `views` is metadata about that
//! picture — which parts of it are a spectrogram, a waveform strip, a packed
//! sprite sheet — so a consumer reads the layout instead of measuring pixels
//! and guessing. These tests pin the two properties that make that safe to
//! rely on: the kind numbers are frozen and append-only, and the five fields a
//! thumbnail has always carried stay exactly where they were on the wire.

mod common;

use common::*;
use makepad_asset_data::*;

/// Frozen wire numbers. A kind may be APPENDED with the next free tag; an
/// existing one may never be renumbered, because every stored manifest in
/// every catalog already means the old number.
#[test]
fn view_kind_tags_are_frozen_and_append_only() {
    // The tags are not public API, so they are pinned through the only thing
    // that is: the bytes. One view per kind, encoded on its own, and the tag
    // is the first byte of the view block.
    let tag_of = |kind: ThumbnailViewKind| -> u8 {
        let mut thumb = thumbnail();
        thumb.views = vec![ThumbnailView::whole(kind, thumb.width, thumb.height)];
        let bytes = manifest_with(thumb).to_canonical_bytes().unwrap();
        let at = views_offset(&bytes);
        // count:u32 then the first view's kind tag.
        bytes[at + 4]
    };
    assert_eq!(tag_of(ThumbnailViewKind::Image), 0);
    assert_eq!(tag_of(ThumbnailViewKind::Fft), 1);
    assert_eq!(tag_of(ThumbnailViewKind::Wave), 2);
    assert_eq!(tag_of(ThumbnailViewKind::Anim), 3);
}

/// An unknown kind refuses rather than being taken for a known one: that is
/// what makes appending safe. A future `video` view read by today's decoder
/// is an error, never an `image`.
#[test]
fn an_unknown_kind_refuses() {
    let mut thumb = thumbnail();
    thumb.views = vec![ThumbnailView::whole(ThumbnailViewKind::Anim, 512, 512)];
    let mut bytes = manifest_with(thumb).to_canonical_bytes().unwrap();
    let at = views_offset(&bytes);
    bytes[at + 4] = 99;
    assert!(matches!(
        AssetManifest::from_canonical_bytes(&bytes),
        Err(AssetDataError::BadTag { what: "ThumbnailViewKind", found: 99 })
    ));
    // And so does an unknown LAYOUT tag, for the same reason.
    let mut bytes = manifest_with({
        let mut t = thumbnail();
        t.views = vec![ThumbnailView::whole(ThumbnailViewKind::Image, 512, 512)];
        t
    })
    .to_canonical_bytes()
    .unwrap();
    let at = views_offset(&bytes);
    bytes[at + 5] = 7;
    assert!(matches!(
        AssetManifest::from_canonical_bytes(&bytes),
        Err(AssetDataError::BadTag { what: "ThumbnailLayout", found: 7 })
    ));
}

/// The compatibility promise, proved rather than asserted: a reader that
/// knows only the five fields a thumbnail carried before views existed walks
/// them off a NEW manifest and gets exactly the values the producer wrote.
/// The views ride behind them, where such a reader simply never looks.
#[test]
fn an_old_shaped_reader_still_parses_a_new_thumbnail() {
    let mut thumb = thumbnail();
    thumb.views = vec![
        ThumbnailView::rect(ThumbnailViewKind::Fft, 0, 0, 512, 448),
        ThumbnailView::rect(ThumbnailViewKind::Wave, 0, 448, 512, 64),
    ];
    thumb.canonicalize();
    let bytes = manifest_with(thumb.clone()).to_canonical_bytes().unwrap();

    // Exactly the decode an old reader performed: blob, media, width, height,
    // byte_len, in that order, at those widths, big-endian.
    let at = find(&bytes, thumb.blob.as_bytes()).expect("thumbnail blob on the wire");
    let mut p = at + 32;
    let u8_ = |p: &mut usize| {
        let v = bytes[*p];
        *p += 1;
        v
    };
    let u32_ = |p: &mut usize| {
        let v = u32::from_be_bytes(bytes[*p..*p + 4].try_into().unwrap());
        *p += 4;
        v
    };
    let u64_ = |p: &mut usize| {
        let v = u64::from_be_bytes(bytes[*p..*p + 8].try_into().unwrap());
        *p += 8;
        v
    };
    assert_eq!(u8_(&mut p), 0, "media tag: Png");
    assert_eq!(u32_(&mut p), thumb.width);
    assert_eq!(u32_(&mut p), thumb.height);
    assert_eq!(u64_(&mut p), thumb.byte_len);
    // Everything the old reader knew is consumed, and the picture it would
    // fetch and draw is byte-identical to before. What it stopped at is the
    // views block: a count it never knew to read.
    assert_eq!(u32_(&mut p), 2, "and the new metadata sits behind it");

    // The same five fields, from a thumbnail with NO views, land at the same
    // offsets with the same values — views are additive, never a reshuffle.
    let plain = ThumbnailMeta::plain(thumb.blob, thumb.media, thumb.width, thumb.height,
        thumb.byte_len);
    let plain_bytes = manifest_with(plain).to_canonical_bytes().unwrap();
    let plain_at = find(&plain_bytes, thumb.blob.as_bytes()).unwrap();
    assert_eq!(
        &plain_bytes[plain_at..plain_at + 32 + 17],
        &bytes[at..at + 32 + 17],
        "the fixed head of a thumbnail is the same bytes either way"
    );
}

/// Absence is not a claim: a thumbnail with no views round-trips, and says
/// nothing at all about being a sheet — which is exactly what every revision
/// baked before this field existed means.
#[test]
fn no_views_means_no_claim() {
    let manifest = weapon_manifest();
    let thumb = manifest.thumbnail.clone().unwrap();
    assert!(thumb.views.is_empty());
    assert_eq!(thumb.animation(), None);
    assert_eq!(thumb.view(ThumbnailViewKind::Anim), None);
    let back = AssetManifest::from_canonical_bytes(&manifest.to_canonical_bytes().unwrap()).unwrap();
    assert_eq!(back, manifest);
}

/// Views round-trip through the canonical encoding, and a producer that found
/// them in any order ships identical bytes after `canonicalize` — or refuses,
/// rather than being silently reordered.
#[test]
fn views_round_trip_and_have_one_canonical_order() {
    let composite = audio_composite();
    let manifest = manifest_with(composite.clone());
    let bytes = manifest.to_canonical_bytes().unwrap();
    let back = AssetManifest::from_canonical_bytes(&bytes).unwrap();
    assert_eq!(back.thumbnail.as_ref().unwrap(), &composite);
    assert_eq!(back.to_canonical_bytes().unwrap(), bytes);

    let mut shuffled = manifest_with({
        let mut t = audio_composite();
        t.views.reverse();
        t
    });
    assert!(matches!(
        shuffled.to_canonical_bytes(),
        Err(AssetDataError::NotSorted { what: "thumbnail views" })
    ));
    shuffled.canonicalize();
    assert_eq!(shuffled.to_canonical_bytes().unwrap(), bytes);

    // The same region declared twice is a producer bug, not a set.
    let mut duped = manifest_with({
        let mut t = audio_composite();
        t.views.push(t.views[0]);
        t.canonicalize();
        t
    });
    duped.canonicalize();
    assert!(matches!(
        duped.to_canonical_bytes(),
        Err(AssetDataError::Duplicate { what: "thumbnail views" })
    ));
}

/// A view must fit the picture it claims to describe. A region that runs off
/// the edge is a producer that stamped a layout it did not write, and a
/// consumer that trusted it would read someone else's pixels.
#[test]
fn a_view_must_fit_inside_its_picture() {
    let refuse = |views: Vec<ThumbnailView>| {
        let mut t = thumbnail();
        t.views = views;
        t.canonicalize();
        manifest_with(t).to_canonical_bytes().unwrap_err()
    };
    // 512x512 picture.
    assert!(matches!(
        refuse(vec![ThumbnailView::rect(ThumbnailViewKind::Image, 0, 0, 513, 512)]),
        AssetDataError::Malformed { what: "thumbnail view rect bounds" }
    ));
    assert!(matches!(
        refuse(vec![ThumbnailView::rect(ThumbnailViewKind::Wave, 0, 0, 0, 10)]),
        AssetDataError::Malformed { what: "thumbnail view rect" }
    ));
    // A near-overflow origin must refuse, not wrap into range.
    assert!(matches!(
        refuse(vec![ThumbnailView::rect(ThumbnailViewKind::Image, u32::MAX, 0, 8, 8)]),
        AssetDataError::Malformed { what: "thumbnail view rect bounds" }
    ));
    // Cells: 4 columns of 128 is exactly 512 wide; a fifth column is not.
    let cells = |cols, count| ThumbnailCells { cols, cell_w: 128, cell_h: 128, first: 0, count };
    assert!(matches!(
        refuse(vec![ThumbnailView::cells(ThumbnailViewKind::Anim, cells(5, 5), 8.0)]),
        AssetDataError::Malformed { what: "thumbnail view cells bounds" }
    ));
    // 4x4 cells of 128 fill a 512x512 picture exactly, and are accepted.
    let mut ok = thumbnail();
    ok.views = vec![ThumbnailView::cells(ThumbnailViewKind::Anim, cells(4, 16), 8.0)];
    manifest_with(ok).to_canonical_bytes().expect("a layout that fits is accepted");
    // A seventeenth cell needs a fifth row.
    assert!(matches!(
        refuse(vec![ThumbnailView::cells(ThumbnailViewKind::Anim, cells(4, 17), 8.0)]),
        AssetDataError::Malformed { what: "thumbnail view cells bounds" }
    ));
    // An empty range describes nothing.
    assert!(matches!(
        refuse(vec![ThumbnailView::cells(ThumbnailViewKind::Anim, cells(4, 0), 8.0)]),
        AssetDataError::Malformed { what: "thumbnail view cells" }
    ));
}

/// A cycling rate is bounded and positive: zero fps is a still declared as an
/// animation, and 10 000 fps is a producer bug that would spin a consumer's
/// frame clock.
#[test]
fn fps_is_bounded_and_positive() {
    let with_fps = |fps: f32| {
        let mut t = thumbnail();
        t.views = vec![ThumbnailView {
            kind: ThumbnailViewKind::Anim,
            layout: ThumbnailLayout::Cells(ThumbnailCells {
                cols: 4,
                cell_w: 128,
                cell_h: 128,
                first: 0,
                count: 4,
            }),
            fps: Some(fps),
        }];
        manifest_with(t).to_canonical_bytes()
    };
    assert!(with_fps(8.0).is_ok());
    assert!(with_fps(240.0).is_ok());
    assert!(matches!(
        with_fps(0.0),
        Err(AssetDataError::Malformed { what: "thumbnail view fps" })
    ));
    assert!(matches!(
        with_fps(-1.0),
        Err(AssetDataError::Malformed { what: "thumbnail view fps" })
    ));
    assert!(matches!(
        with_fps(1000.0),
        Err(AssetDataError::OverBudget { what: "thumbnail view fps", .. })
    ));
}

/// More regions than the budget refuses before allocation, on both sides.
#[test]
fn the_view_count_is_bounded() {
    let mut t = thumbnail();
    t.views = (0..9)
        .map(|i| ThumbnailView::rect(ThumbnailViewKind::Image, i * 8, 0, 8, 8))
        .collect();
    assert!(matches!(
        manifest_with(t).to_canonical_bytes(),
        Err(AssetDataError::OverBudget { what: "thumbnail views", .. })
    ));
}

/// The two questions consumers actually ask, answered from the manifest:
/// "is this a sheet, and what is its layout" and "where is the wave strip".
#[test]
fn the_accessors_answer_the_questions_consumers_used_to_guess() {
    let composite = audio_composite();
    assert_eq!(composite.animation(), None, "a spectrogram is not a sprite sheet");
    assert!(matches!(
        composite.view(ThumbnailViewKind::Wave).map(|v| v.layout),
        Some(ThumbnailLayout::Rect(ThumbnailRect { x: 0, y: 448, w: 512, h: 64 }))
    ));

    let mut sheet = thumbnail();
    let cells = ThumbnailCells { cols: 4, cell_w: 128, cell_h: 128, first: 0, count: 11 };
    sheet.views = vec![ThumbnailView::cells(ThumbnailViewKind::Anim, cells, 12.0)];
    let (found, fps) = sheet.animation().expect("a declared sheet says so");
    assert_eq!(found, cells);
    assert_eq!(fps, 12.0);
    // Eleven real frames in a 4x4 grid: the five padding cells the packer
    // wrote to clear the 256px floor are NOT frames, and nobody has to scan
    // pixels to find that out.
    assert_eq!(found.count, 11);
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// The audio composite the importer bakes: FFT over most of the picture, a
/// thin wave strip along the bottom edge, both declared.
fn audio_composite() -> ThumbnailMeta {
    let mut t = thumbnail();
    t.views = vec![
        ThumbnailView::rect(ThumbnailViewKind::Fft, 0, 0, 512, 448),
        ThumbnailView::rect(ThumbnailViewKind::Wave, 0, 448, 512, 64),
    ];
    t.canonicalize();
    t
}

fn manifest_with(thumbnail: ThumbnailMeta) -> AssetManifest {
    let mut m = weapon_manifest();
    m.thumbnail = Some(thumbnail);
    m
}

/// Byte offset of the views count inside an encoded manifest: the thumbnail
/// blob, then the 17 fixed bytes an old reader consumes.
fn views_offset(bytes: &[u8]) -> usize {
    let thumb = weapon_manifest().thumbnail.clone().unwrap();
    find(bytes, thumb.blob.as_bytes()).expect("thumbnail blob on the wire") + 32 + 17
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}
