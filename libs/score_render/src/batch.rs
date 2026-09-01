use crate::{
    MusicFontRef, OverlayMetrics, OverlayState, PageId, PaintKind, PaintList, Primitive, Rect,
    TextFontRef, Transform,
};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PrimitivePipeline {
    Rule,
    Beam,
    Ribbon,
    Hairpin,
    Bracket,
    Line,
    Tuplet,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Pipeline {
    Paper,
    Primitive(PrimitivePipeline),
    MusicGlyph(MusicFontRef),
    Text(TextFontRef),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BatchKey {
    pub z: i16,
    pub pipeline: Pipeline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PlannedItemRef {
    pub page_slot: u16,
    pub paint_index: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstanceBatch {
    pub key: BatchKey,
    pub items: Vec<PlannedItemRef>,
}

#[derive(Clone, Debug)]
pub struct PageView {
    pub page: Arc<PaintList>,
    /// Maps page staff-space coordinates to physical screen pixels.
    pub transform: Transform,
}

#[derive(Clone, Debug)]
pub struct PlannedPage {
    pub page: Arc<PaintList>,
    pub transform: Transform,
    pub visible_items: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HighlightRole {
    Selection,
    Annotation,
    /// The note under the pointer — the one the app is sounding. A reading
    /// aid, so it is the lightest of the three.
    Hover,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OverlayCommand {
    /// Replay the exact source ink once, dilated by `halo_px`, as a single
    /// translucent wash. It is composited *under* the notation, so the ink it
    /// marks stays fully readable and no form-like box appears over the music.
    HighlightSource {
        source: PlannedItemRef,
        role: HighlightRole,
        halo_px: f32,
        /// An *area* wash (a bar's hit rect, a stem) must go behind the ink or
        /// it covers music it does not mark. A wash that hugs a glyph or a
        /// text run covers only its own ink, so it blends with it instead —
        /// which is also the only order that keeps the staff lines behind a
        /// notehead intact, since a glyph quad owns its depth range.
        under_ink: bool,
    },
    MeasureWash {
        page_slot: u16,
        rect_sp: Rect,
        corner_px: f32,
        opacity: f32,
    },
    PlaybackCursor {
        page_slot: u16,
        x_sp: f64,
        width_px: f32,
        /// Vertical extent in staff spaces, or `None` for the whole page.
        span_sp: Option<(f64, f64)>,
    },
}

impl OverlayCommand {
    /// True for anything that must be composited *behind* the notation.
    /// Only the playback cursor — a hairline, never a fill — sits on top.
    pub fn is_underlay(self) -> bool {
        match self {
            Self::HighlightSource { under_ink, .. } => under_ink,
            Self::MeasureWash { .. } => true,
            Self::PlaybackCursor { .. } => false,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct RenderPlan {
    pub pages: Vec<PlannedPage>,
    pub batches: Vec<InstanceBatch>,
    pub overlays: Vec<OverlayCommand>,
    pub culled_items: usize,
}

impl RenderPlan {
    pub fn visible_items(&self) -> usize {
        self.batches.iter().map(|batch| batch.items.len()).sum()
    }

    pub fn draw_calls(&self) -> usize {
        self.batches.len()
            + usize::from(!self.overlays.is_empty())
            + usize::from(!self.pages.is_empty())
    }

    pub fn memory_bytes(&self) -> usize {
        self.pages.len() * std::mem::size_of::<PlannedPage>()
            + self.batches.len() * std::mem::size_of::<InstanceBatch>()
            + self
                .batches
                .iter()
                .map(|batch| batch.items.len() * std::mem::size_of::<PlannedItemRef>())
                .sum::<usize>()
            + self.overlays.len() * std::mem::size_of::<OverlayCommand>()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlaybackPosition {
    pub page: PageId,
    pub x_sp: f64,
    /// Top and bottom of the system being played, in staff spaces.
    ///
    /// `None` means "the whole page", which is what a cursor with no system
    /// knowledge can honestly say — and what the cursor used to draw always,
    /// striking a full-page rule through systems nothing was playing in.
    pub system_span_sp: Option<(f64, f64)>,
}

#[derive(Clone, Debug, Default)]
pub struct RenderPlanner;

impl RenderPlanner {
    /// Culls each page before looking at item kinds, then groups visible
    /// instances across all pages by z/pipeline/font in deterministic order.
    pub fn plan(
        &self,
        views: &[PageView],
        screen_viewport_px: Rect,
        overlays: &OverlayState,
        overlay_metrics: OverlayMetrics,
    ) -> RenderPlan {
        let mut pages = Vec::new();
        let mut batches: BTreeMap<BatchKey, Vec<PlannedItemRef>> = BTreeMap::new();
        let mut total_items = 0usize;

        for view in views {
            total_items += view.page.items().len();
            let page_rect = view.transform.rect(Rect::from_xywh(
                0.0,
                0.0,
                view.page.page_size().x,
                view.page.page_size().y,
            ));
            if !page_rect.intersects(screen_viewport_px) || view.transform.scale <= 0.0 {
                continue;
            }
            // The page stays in the plan for as long as any of it is on
            // screen, even when none of its *items* are. A page's outer margin
            // carries no paint, so dropping the page for having no visible
            // items took its sheet of paper with it and made the whole page
            // pop out while a band of it was still inside the viewport.
            //
            // Item bounds are engraved bounds; the drawn ink can sit up to a
            // pixel outside them once a hairline is snapped to the device grid
            // and given its antialiasing fringe. Query one output pixel wide
            // so an item at the edge does not blink out a frame early.
            let margin = 1.0 / view.transform.scale;
            let page_viewport = view
                .transform
                .inverse_rect(screen_viewport_px)
                .expanded(margin);
            let visible = view.page.visible_indices(page_viewport);
            let page_slot = pages.len() as u16;
            for paint_index in visible.iter().copied() {
                let item = &view.page.items()[paint_index];
                let key = BatchKey {
                    z: item.z,
                    pipeline: pipeline_for(&item.kind),
                };
                batches.entry(key).or_default().push(PlannedItemRef {
                    page_slot,
                    paint_index: paint_index as u32,
                });
            }
            pages.push(PlannedPage {
                page: view.page.clone(),
                transform: view.transform,
                visible_items: visible.len(),
            });
        }

        let mut plan = RenderPlan {
            culled_items: total_items,
            pages,
            batches: batches
                .into_iter()
                .map(|(key, items)| InstanceBatch { key, items })
                .collect(),
            overlays: Vec::new(),
        };
        plan.culled_items = plan.culled_items.saturating_sub(plan.visible_items());
        append_overlays(&mut plan, overlays, overlay_metrics);
        plan
    }
}

fn pipeline_for(kind: &PaintKind) -> Pipeline {
    match kind {
        PaintKind::Glyph(glyph) => Pipeline::MusicGlyph(glyph.font),
        PaintKind::Text(text) => Pipeline::Text(text.font),
        PaintKind::Primitive(primitive) => Pipeline::Primitive(match primitive {
            Primitive::Rule { .. } => PrimitivePipeline::Rule,
            Primitive::Beam(_) => PrimitivePipeline::Beam,
            Primitive::Ribbon(_) => PrimitivePipeline::Ribbon,
            Primitive::Hairpin { .. } => PrimitivePipeline::Hairpin,
            Primitive::Bracket { .. } => PrimitivePipeline::Bracket,
            Primitive::Line { .. } => PrimitivePipeline::Line,
            Primitive::TupletBracket { .. } => PrimitivePipeline::Tuplet,
        }),
    }
}

fn append_overlays(plan: &mut RenderPlan, state: &OverlayState, metrics: OverlayMetrics) {
    let selected: std::collections::BTreeSet<_> = state.selected.iter().copied().collect();
    let annotated: std::collections::BTreeSet<_> = state.annotated.iter().copied().collect();
    let hovered: std::collections::BTreeSet<_> = state.hovered.iter().copied().collect();
    for (page_slot, page) in plan.pages.iter().enumerate() {
        for batch in &plan.batches {
            for source in &batch.items {
                if source.page_slot as usize != page_slot {
                    continue;
                }
                let item = &page.page.items()[source.paint_index as usize];
                let under_ink = matches!(item.kind, PaintKind::Primitive(_));
                if selected.contains(&item.id) {
                    plan.overlays.push(OverlayCommand::HighlightSource {
                        source: *source,
                        role: HighlightRole::Selection,
                        halo_px: metrics.selection_halo_px,
                        under_ink,
                    });
                }
                if annotated.contains(&item.id) {
                    plan.overlays.push(OverlayCommand::HighlightSource {
                        source: *source,
                        role: HighlightRole::Annotation,
                        halo_px: metrics.annotation_halo_px,
                        under_ink,
                    });
                }
                // A selected note is already marked; stacking hover on top of
                // it would only muddy the wash.
                if hovered.contains(&item.id) && !selected.contains(&item.id) {
                    plan.overlays.push(OverlayCommand::HighlightSource {
                        source: *source,
                        role: HighlightRole::Hover,
                        halo_px: metrics.hover_halo_px,
                        under_ink,
                    });
                }
            }
        }
        let mut washes = Vec::new();
        if let Some(transition) = state.playback_bar_transition {
            let (from, to) = transition.weights(state.presentation_time_s);
            if from > 0.0 {
                washes.push((transition.from, from));
            }
            washes.push((transition.to, to));
        } else if let Some(id) = state.playback_bar {
            washes.push((id, 1.0));
        }
        for (id, opacity) in washes {
            if let Some(item) = page.page.item(id) {
                plan.overlays.push(OverlayCommand::MeasureWash {
                    page_slot: page_slot as u16,
                    rect_sp: item.bounds,
                    corner_px: metrics.measure_corner_px,
                    opacity,
                });
            }
        }
        if let Some(cursor) = state.playback_cursor {
            if cursor.page == page.page.page_id() {
                plan.overlays.push(OverlayCommand::PlaybackCursor {
                    page_slot: page_slot as u16,
                    x_sp: cursor.x_sp,
                    width_px: metrics.playback_cursor_px,
                    span_sp: cursor.system_span_sp,
                });
            }
        }
    }
    plan.overlays.sort_by(|a, b| overlay_sort_key(*a).cmp(&overlay_sort_key(*b)));
}

fn overlay_sort_key(command: OverlayCommand) -> (u8, u16, u32) {
    match command {
        OverlayCommand::MeasureWash { page_slot, .. } => (0, page_slot, 0),
        OverlayCommand::HighlightSource { source, role, .. } => (
            match role {
                HighlightRole::Hover => 1,
                HighlightRole::Annotation => 2,
                HighlightRole::Selection => 3,
            },
            source.page_slot,
            source.paint_index,
        ),
        OverlayCommand::PlaybackCursor { page_slot, .. } => (0, page_slot, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        GlyphItem, Ink, InkRole, PageId, PaintItem, Point, Primitive, RuleKind, SemanticId,
        SmuflGlyph,
    };

    fn page() -> Arc<PaintList> {
        let mut items = Vec::new();
        for i in 0..2000 {
            let x = (i % 100) as f64;
            let y = (i / 100) as f64;
            items.push(if i % 2 == 0 {
                PaintItem {
                    id: SemanticId(i + 1),
                    bounds: Rect::from_xywh(x, y, 1.0, 1.0),
                    z: 2,
                    ink: Ink::role(InkRole::Primary),
                    kind: PaintKind::Glyph(GlyphItem {
                        font: MusicFontRef(0),
                        glyph: SmuflGlyph::new("noteheadBlack"),
                        origin: Point::new(x, y),
                        em_size: 4.0,
                    }),
                }
            } else {
                PaintItem::primitive(
                    SemanticId(i + 1),
                    0,
                    Ink::role(InkRole::Staff),
                    Primitive::Rule {
                        rect: Rect::from_xywh(x, y, 1.0, 0.13),
                        kind: RuleKind::Staff,
                        staff_group: Some((i / 10) as u32),
                    },
                )
            });
        }
        Arc::new(PaintList::new(PageId(0), 1, Point::new(100.0, 100.0), items).unwrap())
    }

    #[test]
    fn planner_culls_then_batches_by_pipeline() {
        let page = page();
        let plan = RenderPlanner.plan(
            &[PageView {
                page,
                transform: Transform::IDENTITY,
            }],
            Rect::from_xywh(0.0, 0.0, 9.5, 9.5),
            &OverlayState::default(),
            OverlayMetrics::default(),
        );
        assert_eq!(plan.batches.len(), 2);
        assert!(plan.visible_items() < 2000);
        assert!(plan.culled_items > 1500);
        for batch in &plan.batches {
            assert!(!batch.items.is_empty());
        }
    }

    /// A page's outer margin carries no ink. Panning it to the edge used to
    /// take the whole sheet out of the plan — paper included — while a band of
    /// it was still on screen, so the page popped out early.
    #[test]
    fn a_page_still_on_screen_keeps_its_paper_without_visible_items() {
        let page = page();
        // A viewport sitting entirely in the page's empty right-hand margin.
        let plan = RenderPlanner.plan(
            &[PageView {
                page,
                transform: Transform::IDENTITY,
            }],
            Rect::from_xywh(97.0, 40.0, 2.5, 10.0),
            &OverlayState::default(),
            OverlayMetrics::default(),
        );
        assert_eq!(plan.pages.len(), 1, "the page is still on screen");
        assert_eq!(plan.visible_items(), 0, "none of its items are");
        assert_eq!(plan.pages[0].visible_items, 0);
    }

    /// And a page genuinely off screen is still dropped.
    #[test]
    fn a_page_off_screen_is_dropped() {
        let page = page();
        let plan = RenderPlanner.plan(
            &[PageView {
                page,
                transform: Transform::IDENTITY,
            }],
            Rect::from_xywh(400.0, 400.0, 100.0, 100.0),
            &OverlayState::default(),
            OverlayMetrics::default(),
        );
        assert!(plan.pages.is_empty());
    }

    #[test]
    fn selection_replays_source_shape_as_halo() {
        let page = page();
        let plan = RenderPlanner.plan(
            &[PageView {
                page,
                transform: Transform::IDENTITY,
            }],
            Rect::from_xywh(0.0, 0.0, 100.0, 100.0),
            &OverlayState {
                selected: vec![SemanticId(1)],
                ..OverlayState::default()
            },
            OverlayMetrics::default(),
        );
        assert!(matches!(
            plan.overlays.as_slice(),
            [OverlayCommand::HighlightSource {
                role: HighlightRole::Selection,
                ..
            }]
        ));
    }

    /// Area washes have to end up behind the music. The playback hairline is
    /// never an underlay, and glyph-hugging washes blend with their own ink.
    #[test]
    fn area_washes_go_under_the_ink_and_the_cursor_never_does() {
        let page = page();
        let plan = RenderPlanner.plan(
            &[PageView {
                page,
                transform: Transform::IDENTITY,
            }],
            Rect::from_xywh(0.0, 0.0, 100.0, 100.0),
            &OverlayState {
                selected: vec![SemanticId(1)],
                annotated: vec![SemanticId(3)],
                hovered: Some(SemanticId(5)),
                playback_bar: Some(SemanticId(7)),
                playback_cursor: Some(crate::PlaybackPosition {
                    page: PageId(0),
                    x_sp: 3.0,
                    system_span_sp: Some((4.0, 16.0)),
                }),
                ..OverlayState::default()
            },
            OverlayMetrics::default(),
        );
        assert!(plan.overlays.len() > 1);
        // The cursor is drawn over the ink, and spans only the system in play.
        let cursor = plan
            .overlays
            .iter()
            .find(|overlay| matches!(overlay, OverlayCommand::PlaybackCursor { .. }))
            .expect("the playback cursor is planned");
        assert!(!cursor.is_underlay());
        let OverlayCommand::PlaybackCursor { span_sp, .. } = cursor else {
            unreachable!("just matched")
        };
        assert_eq!(*span_sp, Some((4.0, 16.0)));
        assert!(plan
            .overlays
            .iter()
            .any(|overlay| matches!(overlay, OverlayCommand::MeasureWash { .. })
                && overlay.is_underlay()));
        // Every area wash is behind the notation; the glyph-hugging ones blend.
        for overlay in &plan.overlays {
            if let OverlayCommand::HighlightSource { source, .. } = overlay {
                let item = &plan.pages[source.page_slot as usize].page.items()
                    [source.paint_index as usize];
                assert_eq!(
                    overlay.is_underlay(),
                    matches!(item.kind, PaintKind::Primitive(_))
                );
            }
        }
    }

    /// A note that is both hovered and selected must not get two washes.
    #[test]
    fn hover_defers_to_selection_on_the_same_element() {
        let page = page();
        let plan = RenderPlanner.plan(
            &[PageView {
                page,
                transform: Transform::IDENTITY,
            }],
            Rect::from_xywh(0.0, 0.0, 100.0, 100.0),
            &OverlayState {
                selected: vec![SemanticId(1)],
                hovered: Some(SemanticId(1)),
                ..OverlayState::default()
            },
            OverlayMetrics::default(),
        );
        assert_eq!(plan.overlays.len(), 1);
    }
}
