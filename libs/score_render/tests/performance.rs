use makepad_score_render::*;
use std::{sync::Arc, time::Instant};

const ITEMS_PER_PAGE: usize = 2_480;

fn synthetic_page(page_number: u32) -> Arc<PaintList> {
    let base = page_number as u64 * 1_000_000;
    let notehead = SmuflGlyph::new("noteheadBlack");
    let mut items = Vec::with_capacity(ITEMS_PER_PAGE);
    for i in 0..2_048u64 {
        let x = 10.0 + (i % 64) as f64 * 2.5;
        let y = 10.0 + (i / 64) as f64 * 6.8;
        items.push(PaintItem {
            id: SemanticId(base + i + 1),
            bounds: Rect::from_xywh(x - 0.05, y - 0.45, 1.25, 0.9),
            z: 3,
            ink: Ink::role(InkRole::Primary),
            kind: PaintKind::Glyph(GlyphItem {
                font: MusicFontRef(0),
                glyph: notehead.clone(),
                origin: Point::new(x, y),
                em_size: 4.0,
            }),
        });
    }
    for i in 0..320u64 {
        let y = 8.0 + i as f64 * 0.72;
        items.push(PaintItem::primitive(
            SemanticId(base + 10_000 + i),
            0,
            Ink::role(InkRole::Staff),
            Primitive::Rule {
                rect: Rect::from_xywh(5.0, y, 170.0, 0.13),
                kind: RuleKind::Staff,
                staff_group: Some((i / 5) as u32),
            },
        ));
    }
    for i in 0..64u64 {
        let x = 8.0 + (i % 8) as f64 * 20.0;
        let y = 18.0 + (i / 8) as f64 * 28.0;
        items.push(PaintItem::primitive(
            SemanticId(base + 20_000 + i),
            1,
            Ink::role(InkRole::Primary),
            Primitive::Beam(Beam {
                start: Point::new(x, y),
                end: Point::new(x + 12.0, y + (i % 3) as f64 * 0.25),
                thickness: 0.50,
            }),
        ));
    }
    for i in 0..32u64 {
        let x = 8.0 + (i % 4) as f64 * 40.0;
        let y = 22.0 + (i / 4) as f64 * 28.0;
        items.push(PaintItem::primitive(
            SemanticId(base + 30_000 + i),
            2,
            Ink::role(InkRole::Primary),
            Primitive::Ribbon(Ribbon {
                curve: Cubic {
                    p0: Point::new(x, y),
                    p1: Point::new(x + 4.0, y - 2.0),
                    p2: Point::new(x + 12.0, y - 2.0),
                    p3: Point::new(x + 16.0, y),
                },
                endpoint_thickness: 0.10,
                midpoint_thickness: 0.22,
            }),
        ));
    }
    for i in 0..16u64 {
        let x = 8.0 + (i % 4) as f64 * 40.0;
        let y = 30.0 + (i / 4) as f64 * 52.0;
        items.push(PaintItem {
            id: SemanticId(base + 40_000 + i),
            bounds: Rect::from_xywh(x, y - 2.0, 18.0, 3.0),
            z: 4,
            ink: Ink::role(InkRole::Secondary),
            kind: PaintKind::Text(TextRun {
                font: TextFontRef(0),
                text: Arc::from("dolce e cantabile"),
                origin: Point::new(x, y),
                size: 2.4,
                letter_spacing: 0.0,
                direction: TextDirection::LeftToRight,
                language: Some(Arc::from("it")),
            }),
        });
    }
    assert_eq!(items.len(), ITEMS_PER_PAGE);
    Arc::new(
        PaintList::new(
            PageId(page_number),
            1,
            Point::new(180.0, 260.0),
            items,
        )
        .unwrap(),
    )
}

fn measure(page_count: usize) -> (usize, usize, u128, u128, usize) {
    let build_started = Instant::now();
    let pages: Vec<_> = (0..page_count as u32).map(synthetic_page).collect();
    let build_us = build_started.elapsed().as_micros();

    let scale = 0.12;
    let views: Vec<_> = pages
        .iter()
        .enumerate()
        .map(|(index, page)| PageView {
            page: page.clone(),
            transform: Transform {
                translation: Point::new(
                    (index % 10) as f64 * (180.0 * scale + 4.0),
                    (index / 10) as f64 * (260.0 * scale + 4.0),
                ),
                scale,
            },
        })
        .collect();
    let cull_started = Instant::now();
    let plan = RenderPlanner.plan(
        &views,
        Rect::from_xywh(0.0, 0.0, 400.0, 260.0),
        &OverlayState::default(),
        OverlayMetrics::default(),
    );
    let cull_us = cull_started.elapsed().as_micros();
    let items = pages.iter().map(|page| page.items().len()).sum();
    let memory = pages.iter().map(|page| page.memory_bytes()).sum::<usize>()
        + plan.memory_bytes();
    eprintln!(
        "score_render_perf pages={page_count} items={items} draw_calls={} cull_us={cull_us} build_us={build_us} memory_bytes={memory}",
        plan.draw_calls(),
    );
    assert_eq!(items, page_count * ITEMS_PER_PAGE);
    assert_eq!(plan.visible_items(), items);
    assert_eq!(plan.culled_items, 0);
    assert!(plan.draw_calls() <= 6);
    assert!(build_us < 10_000_000, "synthetic page build exceeded 10 seconds");
    assert!(cull_us < 2_000_000, "viewport planning exceeded 2 seconds");
    assert!(memory < 128 * 1024 * 1024, "retained data exceeded 128 MiB");
    (items, plan.draw_calls(), build_us, cull_us, memory)
}

#[test]
fn performance_1_10_50_pages() {
    let one = measure(1);
    let ten = measure(10);
    let fifty = measure(50);
    assert!(fifty.0 >= 100_000);
    assert_eq!(one.1, ten.1);
    assert_eq!(ten.1, fifty.1);
}
