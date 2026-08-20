//! The ONE thumbnail path: declared views in, the right picture out.
//!
//! # The law
//!
//! A thumbnail carries PROPER METADATA declaring what the picture IS —
//! [`ThumbnailView`]s naming the kind of every region, the cell grid and
//! range of an animation, its rate. Consumers obey the declaration and
//! NOTHING else: no code anywhere may look at a picture's width, height or
//! aspect ratio to decide what it means. (Dimension reads for LAYOUT —
//! aspect-fitting a texture into a card — are fine; for SEMANTICS, never.
//! A 1024-square Flux render and a 64-tile sprite sheet are dimensionally
//! identical, and every host that ever guessed got one of them wrong.)
//!
//! This module is the single place thumbnails are interpreted and cut, and
//! [`AssetThumb`] is the single widget that draws them — the Library grid
//! card, the list row, the LOAD/import row, the History tile and the queue
//! chip are all instances of it. An app that wants a thumbnail drawn some
//! new place instantiates `AssetThumb` and hands it a [`ThumbMedia`]; it
//! does not write a new reader.
//!
//! The pipeline is split so each stage runs where it belongs:
//!
//! 1. [`plan_views`] — declaration → [`ThumbPlan`]. Pure, no Cx, testable.
//! 2. [`cut_plan_bgra`] — decoded pixels + plan → [`ThumbPixels`]. Pure,
//!    worker-thread safe: an app's IO pool cuts cells off the UI thread.
//! 3. [`ThumbMedia::from_pixels`] — pixels → textures, once, on the UI
//!    thread. Uploaded a single time; a cycling card re-binds, never
//!    re-uploads.
//! 4. [`AssetThumb`] — draws the still, or the declared-rate frame against
//!    the caller's shared clock so a wall of sprites animates in step.

use makepad_asset_data::{
    ThumbnailCells, ThumbnailLayout, ThumbnailRect, ThumbnailView, ThumbnailViewKind,
};
use makepad_widgets::image_cache::ImageCacheImpl;
use makepad_widgets::*;
use std::rc::Rc;

/// Cycle rate for a declared animation whose producer did not declare one.
/// A FALLBACK for incomplete declarations — never a reason to animate
/// something that declared no animation at all.
pub const THUMB_FALLBACK_FPS: f32 = 7.0;

/// What the declaration says to draw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ThumbPlan {
    /// No declaration, or nothing usable: the whole picture, still.
    Whole,
    /// A declared region, still.
    Rect(ThumbnailRect),
    /// Declared cells cycling at a declared rate.
    Cells(ThumbnailCells, f32),
}

/// Interpret a thumbnail's declared views. THE precedence, everywhere:
///
/// 1. An `Anim` view with at least two cells cycles them at its rate.
/// 2. Otherwise the best STILL region: `Fft` (an audio card is its
///    spectrogram), then `Image`, then `Wave`; a single-cell `Anim` view is
///    a still of that cell.
/// 3. No views at all — every revision baked before the views contract —
///    means "one picture, take it as it comes": the whole image, STILL.
///    Absence is never a claim, and never an invitation to guess.
pub fn plan_views(views: &[ThumbnailView]) -> ThumbPlan {
    // 1. A real animation wins.
    for view in views {
        if view.kind == ThumbnailViewKind::Anim {
            if let ThumbnailLayout::Cells(cells) = view.layout {
                if cells.count >= 2 {
                    return ThumbPlan::Cells(cells, view.fps.unwrap_or(THUMB_FALLBACK_FPS));
                }
            }
        }
    }
    // 2. The best still region, by kind.
    for kind in [
        ThumbnailViewKind::Fft,
        ThumbnailViewKind::Image,
        ThumbnailViewKind::Wave,
        ThumbnailViewKind::Anim,
    ] {
        if let Some(view) = views.iter().find(|view| view.kind == kind) {
            return ThumbPlan::Rect(view_rect(view.layout));
        }
    }
    ThumbPlan::Whole
}

/// The still region a view's layout names: a rect is itself; a cell grid's
/// still is its FIRST cell.
fn view_rect(layout: ThumbnailLayout) -> ThumbnailRect {
    match layout {
        ThumbnailLayout::Rect(rect) => rect,
        ThumbnailLayout::Cells(cells) => ThumbnailRect {
            x: (cells.first % cells.cols.max(1)) * cells.cell_w,
            y: (cells.first / cells.cols.max(1)) * cells.cell_h,
            w: cells.cell_w,
            h: cells.cell_h,
        },
    }
}

/// Cut pixels, ready to become textures. BGRA words, row-major.
#[derive(Clone, Debug, PartialEq)]
pub enum ThumbPixels {
    Still {
        width: usize,
        height: usize,
        bgra: Vec<u32>,
    },
    Frames {
        width: usize,
        height: usize,
        frames: Vec<Vec<u32>>,
        fps: f32,
    },
}

/// Execute a plan against decoded pixels. Total: a declaration that does not
/// fit the picture it arrived with (a stale stamp, a truncated decode)
/// degrades to the whole still image rather than panicking or half-cutting.
pub fn cut_plan_bgra(width: usize, height: usize, bgra: &[u32], plan: &ThumbPlan) -> ThumbPixels {
    let whole = |bgra: &[u32]| ThumbPixels::Still {
        width,
        height,
        bgra: bgra[..(width * height).min(bgra.len())].to_vec(),
    };
    if bgra.len() < width * height || width == 0 || height == 0 {
        // The decode did not deliver what the header promised; hand back
        // what exists rather than reading past it.
        return ThumbPixels::Still {
            width,
            height,
            bgra: bgra.to_vec(),
        };
    }
    match plan {
        ThumbPlan::Whole => whole(bgra),
        ThumbPlan::Rect(rect) => match copy_rect(width, height, bgra, rect) {
            Some((w, h, pixels)) => ThumbPixels::Still {
                width: w,
                height: h,
                bgra: pixels,
            },
            None => whole(bgra),
        },
        ThumbPlan::Cells(cells, fps) => {
            let mut frames = Vec::new();
            let cols = cells.cols.max(1);
            for index in cells.first..cells.first.saturating_add(cells.count) {
                let rect = ThumbnailRect {
                    x: (index % cols) * cells.cell_w,
                    y: (index / cols) * cells.cell_h,
                    w: cells.cell_w,
                    h: cells.cell_h,
                };
                match copy_rect(width, height, bgra, &rect) {
                    Some((_, _, pixels)) => frames.push(pixels),
                    // A range past the picture is a stale declaration: keep
                    // the frames that exist.
                    None => break,
                }
            }
            match frames.len() {
                0 => whole(bgra),
                1 => ThumbPixels::Still {
                    width: cells.cell_w as usize,
                    height: cells.cell_h as usize,
                    bgra: frames.remove(0),
                },
                _ => ThumbPixels::Frames {
                    width: cells.cell_w as usize,
                    height: cells.cell_h as usize,
                    frames,
                    fps: *fps,
                },
            }
        }
    }
}

/// Interpret + cut in one call: what an IO worker runs per decoded thumbnail.
pub fn thumb_pixels_from_bgra(
    width: usize,
    height: usize,
    bgra: &[u32],
    views: &[ThumbnailView],
) -> ThumbPixels {
    cut_plan_bgra(width, height, bgra, &plan_views(views))
}

fn copy_rect(
    width: usize,
    height: usize,
    bgra: &[u32],
    rect: &ThumbnailRect,
) -> Option<(usize, usize, Vec<u32>)> {
    let (x, y, w, h) = (
        rect.x as usize,
        rect.y as usize,
        rect.w as usize,
        rect.h as usize,
    );
    if w == 0 || h == 0 || x.checked_add(w)? > width || y.checked_add(h)? > height {
        return None;
    }
    let mut out = Vec::with_capacity(w * h);
    for row in y..y + h {
        let start = row * width + x;
        out.extend_from_slice(&bgra[start..start + w]);
    }
    Some((w, h, out))
}

// ---------------------------------------------------------------------------
// Textures + the widget
// ---------------------------------------------------------------------------

/// A thumbnail's textures, uploaded once. Cheap to clone (frames are shared),
/// so a host caches these by asset key and hands clones to every card that
/// shows the asset.
#[derive(Clone)]
pub enum ThumbMedia {
    Still(Texture),
    Anim { frames: Rc<Vec<Texture>>, fps: f32 },
}

impl ThumbMedia {
    /// Upload cut pixels. The UI-thread half of the pipeline.
    pub fn from_pixels(cx: &mut Cx, pixels: ThumbPixels) -> Option<Self> {
        let texture = |cx: &mut Cx, width: usize, height: usize, data: Vec<u32>| {
            Texture::new_with_format(
                cx,
                TextureFormat::VecBGRAu8_32 {
                    width,
                    height,
                    data: Some(data),
                    updated: TextureUpdated::Full,
                },
            )
        };
        match pixels {
            ThumbPixels::Still {
                width,
                height,
                bgra,
            } => {
                if width == 0 || height == 0 || bgra.is_empty() {
                    return None;
                }
                Some(Self::Still(texture(cx, width, height, bgra)))
            }
            ThumbPixels::Frames {
                width,
                height,
                frames,
                fps,
            } => {
                let frames: Vec<Texture> = frames
                    .into_iter()
                    .map(|data| texture(cx, width, height, data))
                    .collect();
                match frames.len() {
                    0 => None,
                    1 => Some(Self::Still(frames.into_iter().next().unwrap())),
                    _ => Some(Self::Anim {
                        frames: Rc::new(frames),
                        fps: if fps > 0.0 { fps } else { THUMB_FALLBACK_FPS },
                    }),
                }
            }
        }
    }

    /// A ready still texture (a badge, a placeholder, a render).
    pub fn still(texture: Texture) -> Self {
        Self::Still(texture)
    }

    /// Pre-cut frames a host already owns as textures.
    pub fn anim(frames: Vec<Texture>, fps: f32) -> Self {
        match frames.len() {
            1 => Self::Still(frames.into_iter().next().unwrap()),
            _ => Self::Anim {
                frames: Rc::new(frames),
                fps: if fps > 0.0 { fps } else { THUMB_FALLBACK_FPS },
            },
        }
    }

    pub fn is_animated(&self) -> bool {
        matches!(self, Self::Anim { .. })
    }

    /// The still face: an animation's first frame.
    pub fn first(&self) -> &Texture {
        match self {
            Self::Still(texture) => texture,
            Self::Anim { frames, .. } => &frames[0],
        }
    }

    /// The frame to show at time `now` — ONE clock for a whole wall of
    /// cards, each cycling at its own declared rate, in step.
    pub fn frame_at(&self, now: f64) -> &Texture {
        match self {
            Self::Still(texture) => texture,
            Self::Anim { frames, fps } => {
                let index = ((now * *fps as f64) as usize) % frames.len();
                &frames[index]
            }
        }
    }
}

/// THE thumbnail widget: an aspect-fitted picture that draws whatever its
/// [`ThumbMedia`] declares — still, or cycling at the declared rate. Hosts
/// re-bind media per draw (list items are recycled); a host with cycling
/// cards on screen keeps its own frame pump running, as it does for any
/// animation.
#[derive(Script, ScriptHook, Widget)]
pub struct AssetThumb {
    #[deref]
    image: Image,
    #[rust]
    media: Option<ThumbMedia>,
}

impl Widget for AssetThumb {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let texture = self
            .media
            .as_ref()
            .map(|media| media.frame_at(cx.time()).clone());
        ImageCacheImpl::set_texture(&mut self.image, texture, 0);
        self.image.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.image.handle_event(cx, event, scope);
    }
}

impl AssetThumb {
    /// Bind what this card shows. `None` is the honest empty well.
    pub fn set_media(&mut self, cx: &mut Cx, media: Option<ThumbMedia>) {
        self.media = media;
        self.image.redraw(cx);
    }

    pub fn is_animated(&self) -> bool {
        self.media.as_ref().is_some_and(ThumbMedia::is_animated)
    }
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.AssetThumbBase = #(AssetThumb::register_widget(vm))
    // ImageFit.Smallest shrinks the Image's own walk to the aspect-fitted
    // size — it does NOT center the result. A card wraps the thumb in a
    // fixed-size aligning box; the box centers portrait/square/strip
    // pictures, with no stretch and no crop.
    mod.widgets.AssetThumb = set_type_default() do mod.widgets.AssetThumbBase{
        width: Fill
        height: Fill
        fit: ImageFit.Smallest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(kind: ThumbnailViewKind, layout: ThumbnailLayout, fps: Option<f32>) -> ThumbnailView {
        ThumbnailView { kind, layout, fps }
    }

    fn rect(x: u32, y: u32, w: u32, h: u32) -> ThumbnailLayout {
        ThumbnailLayout::Rect(ThumbnailRect { x, y, w, h })
    }

    fn cells(cols: u32, cell_w: u32, cell_h: u32, first: u32, count: u32) -> ThumbnailCells {
        ThumbnailCells {
            cols,
            cell_w,
            cell_h,
            first,
            count,
        }
    }

    /// A test picture where every pixel is its own index — any cut can be
    /// verified to the pixel.
    fn indexed(width: usize, height: usize) -> Vec<u32> {
        (0..width * height).map(|i| i as u32).collect()
    }

    /// The audio composite: an FFT half and a wave half, declared as RECT
    /// views. It draws as the STATIC spectrogram region — never cycles, no
    /// matter its dimensions. This is the exact card that used to be
    /// guessed into an animation.
    #[test]
    fn audio_composite_is_the_static_fft_region() {
        let views = [
            view(ThumbnailViewKind::Fft, rect(0, 0, 8, 4), None),
            view(ThumbnailViewKind::Wave, rect(0, 4, 8, 4), None),
        ];
        assert_eq!(
            plan_views(&views),
            ThumbPlan::Rect(ThumbnailRect { x: 0, y: 0, w: 8, h: 4 })
        );
        let cut = thumb_pixels_from_bgra(8, 8, &indexed(8, 8), &views);
        match cut {
            ThumbPixels::Still {
                width,
                height,
                bgra,
            } => {
                assert_eq!((width, height), (8, 4));
                // The TOP half — the fft rows — to the pixel.
                assert_eq!(bgra, indexed(8, 4));
            }
            other => panic!("audio composite must be a still, got {other:?}"),
        }
    }

    /// A billboard's declared anim view cycles exactly its declared cells at
    /// its declared rate.
    #[test]
    fn declared_anim_cycles_its_declared_cells() {
        let views = [view(
            ThumbnailViewKind::Anim,
            ThumbnailLayout::Cells(cells(2, 4, 4, 0, 3)),
            Some(9.0),
        )];
        let cut = thumb_pixels_from_bgra(8, 8, &indexed(8, 8), &views);
        match cut {
            ThumbPixels::Frames {
                width,
                height,
                frames,
                fps,
            } => {
                assert_eq!((width, height), (4, 4));
                assert_eq!(frames.len(), 3, "count is the producer's, not the grid's");
                assert_eq!(fps, 9.0);
                // Cell 1 sits at (4,0) in a 2-column grid of 4x4 cells.
                let expected: Vec<u32> = (0..4)
                    .flat_map(|row| (0..4).map(move |col| (row * 8 + 4 + col) as u32))
                    .collect();
                assert_eq!(frames[1], expected);
            }
            other => panic!("declared anim must cycle, got {other:?}"),
        }
    }

    /// THE LAW: no declaration means the whole picture, still. A wide strip
    /// (the shape a legacy guesser would chop into tiles) stays one image.
    #[test]
    fn undeclared_pictures_are_never_guessed_into_sheets() {
        let cut = thumb_pixels_from_bgra(64, 4, &indexed(64, 4), &[]);
        assert_eq!(
            cut,
            ThumbPixels::Still {
                width: 64,
                height: 4,
                bgra: indexed(64, 4)
            },
            "a view-less picture is one still picture, whatever its aspect"
        );
    }

    /// An anim view that declared no fps still cycles — at the documented
    /// fallback, not at zero.
    #[test]
    fn anim_without_fps_uses_the_fallback_rate() {
        let views = [view(
            ThumbnailViewKind::Anim,
            ThumbnailLayout::Cells(cells(2, 4, 4, 0, 2)),
            None,
        )];
        match plan_views(&views) {
            ThumbPlan::Cells(_, fps) => assert_eq!(fps, THUMB_FALLBACK_FPS),
            other => panic!("expected cells, got {other:?}"),
        }
    }

    /// A single-cell anim view is a STILL of that cell — one frame cannot
    /// cycle, and the padding cells a packer added are not frames.
    #[test]
    fn single_cell_anim_is_a_still_of_that_cell() {
        let views = [view(
            ThumbnailViewKind::Anim,
            ThumbnailLayout::Cells(cells(2, 4, 4, 1, 1)),
            Some(9.0),
        )];
        match plan_views(&views) {
            ThumbPlan::Rect(r) => assert_eq!(r, ThumbnailRect { x: 4, y: 0, w: 4, h: 4 }),
            other => panic!("expected the cell as a still, got {other:?}"),
        }
    }

    /// A declaration that does not fit its picture degrades to the whole
    /// still image — stale stamps must not panic or half-draw.
    #[test]
    fn stale_declarations_degrade_to_the_whole_picture() {
        let views = [view(
            ThumbnailViewKind::Anim,
            ThumbnailLayout::Cells(cells(4, 64, 64, 0, 16)),
            Some(9.0),
        )];
        let cut = thumb_pixels_from_bgra(8, 8, &indexed(8, 8), &views);
        assert_eq!(
            cut,
            ThumbPixels::Still {
                width: 8,
                height: 8,
                bgra: indexed(8, 8)
            }
        );
        // A rect view past the edge degrades the same way.
        let views = [view(ThumbnailViewKind::Fft, rect(6, 0, 8, 4), None)];
        match thumb_pixels_from_bgra(8, 8, &indexed(8, 8), &views) {
            ThumbPixels::Still { width, height, .. } => assert_eq!((width, height), (8, 8)),
            other => panic!("expected whole-image fallback, got {other:?}"),
        }
    }

    /// An anim view whose range walks off the sheet keeps the frames that
    /// exist: the declaration's count is honoured up to the picture's edge.
    #[test]
    fn anim_range_past_the_sheet_keeps_the_real_frames() {
        let views = [view(
            ThumbnailViewKind::Anim,
            ThumbnailLayout::Cells(cells(2, 4, 4, 0, 9)),
            Some(9.0),
        )];
        match thumb_pixels_from_bgra(8, 8, &indexed(8, 8), &views) {
            ThumbPixels::Frames { frames, .. } => assert_eq!(frames.len(), 4),
            other => panic!("expected the four real frames, got {other:?}"),
        }
    }
}
