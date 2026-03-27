use std::sync::Arc;

use super::*;
use crate::browser_scene::{
    MpBrowserFontResource, MpBrowserGlyphInstance, MpBrowserTextDecorations,
    MpBrowserTextMetrics, MpBrowserTextRun,
};
use crate::{dvec2, vec4, Cx, CxDraw, DrawEvent, Rect};

const TEST_FONT_BYTES: &[u8] = include_bytes!("../../../widgets/resources/NotoSans-Regular.ttf");

#[test]
fn canonical_glyph_key_ignores_non_raster_state() {
    let face_key = BrowserFontFaceKey {
        resource_key: 7,
        face_index: 0,
    };
    let settings = BrowserTextRasterSettings::default();
    let left = BrowserGlyphRequest::new(face_key, 42, 19.2, settings).key;
    let right = BrowserGlyphRequest::new(face_key, 42, 19.2, settings).key;
    assert_eq!(left, right);
}

#[test]
fn prepared_run_invalid_after_cache_reset() {
    let mut cache = BrowserTextCache::new();
    let run = PreparedBrowserTextRun {
        key: MpPreparedBrowserTextRunKey {
            retained_scene_id: 1,
            stable_text_run_id: 1,
        },
        reset_epoch: cache.reset_epoch,
        batches: vec![],
    };
    assert!(cache.is_prepared_run_valid(&run));
    cache.reset();
    assert!(!cache.is_prepared_run_valid(&run));
}

#[test]
fn retained_prepare_reuses_prepared_text_on_unchanged_frames() {
    let mut scene = MpBrowserScene::new_with_retained_scene_id(
        7,
        Rect {
            pos: dvec2(0.0, 0.0),
            size: dvec2(160.0, 60.0),
        },
    );
    scene.push_text_run(MpBrowserTextRun {
        stable_id: 11,
        local_rect: Rect {
            pos: dvec2(10.0, 10.0),
            size: dvec2(120.0, 24.0),
        },
        transform_id: 0,
        clip_chain_id: 0,
        color: vec4(1.0, 1.0, 1.0, 1.0),
        fonts: vec![MpBrowserFontResource {
            key: 1,
            bytes: Arc::from(TEST_FONT_BYTES),
            face_index: 0,
        }],
        glyphs: vec![MpBrowserGlyphInstance {
            glyph_id: 0,
            font_size_px: 16.0,
            origin: dvec2(0.0, 14.0),
            font_slot: 0,
        }],
        metrics: MpBrowserTextMetrics::default(),
        decorations: MpBrowserTextDecorations::default(),
    });

    let mut cx = Cx::new(Box::new(|_, _| {}));
    let draw_event = DrawEvent::default();
    {
        let _cx_draw = CxDraw::new(&mut cx, &draw_event);
    }
    let fonts_rc = cx
        .get_global::<Rc<RefCell<makepad_draw::text::fonts::Fonts>>>()
        .clone();
    let mut cache = BrowserTextCache::new();
    let mut prepared = MpPreparedBrowserScene::new(scene.retained_scene_id);

    cache.prepare_retained_scene_for_test(&mut cx, &scene, &mut prepared, 1.0, &fonts_rc);
    let first = cache.frame_stats();
    cache.prepare_retained_scene_for_test(&mut cx, &scene, &mut prepared, 1.0, &fonts_rc);
    let second = cache.frame_stats();

    assert_eq!(first.prepared_text_batch_hit_count, 0);
    assert_eq!(first.prepared_text_batch_miss_count, 1);
    assert_eq!(first.prepared_text_batch_rebuild_count, 1);
    assert!(first.glyph_residency_miss_count >= 1);
    assert_eq!(second.prepared_text_batch_hit_count, 1);
    assert_eq!(second.prepared_text_batch_miss_count, 0);
    assert_eq!(second.prepared_text_batch_rebuild_count, 0);
    assert_eq!(second.glyph_residency_miss_count, 0);
}

#[test]
fn msdf_promotion_invalidates_only_affected_prepared_run() {
    let mut scene = MpBrowserScene::new_with_retained_scene_id(
        8,
        Rect {
            pos: dvec2(0.0, 0.0),
            size: dvec2(220.0, 80.0),
        },
    );
    for (stable_id, font_size_px, x) in [(11, 48.0, 10.0), (12, 64.0, 110.0)] {
        scene.push_text_run(MpBrowserTextRun {
            stable_id,
            local_rect: Rect {
                pos: dvec2(x, 10.0),
                size: dvec2(80.0, 48.0),
            },
            transform_id: 0,
            clip_chain_id: 0,
            color: vec4(1.0, 1.0, 1.0, 1.0),
            fonts: vec![MpBrowserFontResource {
                key: 1,
                bytes: Arc::from(TEST_FONT_BYTES),
                face_index: 0,
            }],
            glyphs: vec![MpBrowserGlyphInstance {
                glyph_id: 0,
                font_size_px,
                origin: dvec2(0.0, 36.0),
                font_slot: 0,
            }],
            metrics: MpBrowserTextMetrics::default(),
            decorations: MpBrowserTextDecorations::default(),
        });
    }

    let mut cx = Cx::new(Box::new(|_, _| {}));
    let draw_event = DrawEvent::default();
    {
        let _cx_draw = CxDraw::new(&mut cx, &draw_event);
    }
    let fonts_rc = cx
        .get_global::<Rc<RefCell<makepad_draw::text::fonts::Fonts>>>()
        .clone();
    let mut cache = BrowserTextCache::new();
    let mut prepared = MpPreparedBrowserScene::new(scene.retained_scene_id);

    cache.prepare_retained_scene_for_test(&mut cx, &scene, &mut prepared, 1.0, &fonts_rc);
    let first = cache.frame_stats();
    assert_eq!(first.prepared_text_batch_miss_count, 2);
    assert_eq!(cache.pending_msdf.len(), 2);

    cache.queued_msdf_jobs.clear();
    let promoted_key = *cache.pending_msdf.keys().next().unwrap();
    let promoted_epoch = cache.pending_msdf[&promoted_key];
    let promoted_size = cache.glyphs[&promoted_key].atlas_image_bounds.size;
    assert!(cache.apply_completed_msdf_job(
        &mut cx,
        BrowserCompletedMsdfJob {
            key: promoted_key,
            pixels: vec![Bgra::default(); promoted_size.width * promoted_size.height],
            image_size: promoted_size,
            epoch: promoted_epoch,
        },
    ));
    assert_eq!(cache.glyphs[&promoted_key].page.format, BrowserGlyphFormat::Msdf);

    let invalid_count = prepared
        .text_runs
        .values()
        .filter(|run| !cache.is_prepared_run_valid(run))
        .count();
    assert_eq!(invalid_count, 1);
}

#[test]
fn shelf_allocator_wraps_to_next_row() {
    let mut allocator = ShelfAllocator::new(Size::new(16, 16));
    let first = allocator.allocate(Size::new(10, 4)).unwrap();
    let second = allocator.allocate(Size::new(8, 4)).unwrap();
    assert_eq!(first.origin, Point::new(0, 0));
    assert_eq!(second.origin, Point::new(0, 4));
}
