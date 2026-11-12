//! Pages for print and export: an app lays its content out in points, this
//! renders each page off screen at print resolution and hands back its
//! pixels, and [`paginate`] decides where pages break.
//!
//! A page renders in horizontal bands sized so a band's readback stays within
//! half of the context's readback budget and at most 4 MB (A4 at 300 dpi: nine
//! bands), one band per frame, so the UI thread never waits on the GPU. A
//! refused readback is asked for again on a later frame, never dropped. The
//! host widget calls [`PageRender::begin_page`] in its draw (drawing the page
//! between it and [`PageRender::end_page`] when it answers a page) and
//! [`PageRender::handle_event`] with its events.

use crate::makepad_draw::*;
use std::{ops::Range, sync::Arc};

/// A page's paper, margins and resolution.
#[derive(Clone, Copy, Debug)]
pub struct PageFormat {
    /// The paper in points (1/72 inch), the unit a page's content is laid
    /// out in.
    pub size: DVec2,
    /// The margins in points; the content stands inside them.
    pub margin: Inset,
    /// Pixels per inch of the rendered page.
    pub dpi: f64,
}

impl PageFormat {
    /// A4 (210 x 297 mm) with 20 mm margins, at 300 dpi.
    pub fn a4() -> Self {
        let mm = 72.0 / 25.4;
        let margin = 20.0 * mm;
        Self {
            size: dvec2(210.0 * mm, 297.0 * mm),
            margin: Inset { left: margin, right: margin, top: margin, bottom: margin },
            dpi: 300.0,
        }
    }

    /// US Letter (8.5 x 11 in) with 0.75 in margins, at 300 dpi.
    pub fn letter() -> Self {
        let margin = 54.0;
        Self {
            size: dvec2(612.0, 792.0),
            margin: Inset { left: margin, right: margin, top: margin, bottom: margin },
            dpi: 300.0,
        }
    }

    /// Where the content stands on the page, in points.
    pub fn content(&self) -> Rect {
        let m = self.margin;
        Rect {
            pos: dvec2(m.left, m.top),
            size: dvec2(
                (self.size.x - m.left - m.right).max(1.0),
                (self.size.y - m.top - m.bottom).max(1.0),
            ),
        }
    }

    /// Pixels per point.
    pub fn scale(&self) -> f64 {
        self.dpi / 72.0
    }

    /// The page in pixels.
    pub fn pixels(&self) -> (usize, usize) {
        let scale = self.scale();
        (
            (self.size.x * scale).round().max(1.0) as usize,
            (self.size.y * scale).round().max(1.0) as usize,
        )
    }
}

/// One item of content stacked top to bottom on pages: its height and where
/// a break may not cut it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageItem {
    pub height: f64,
    /// Bands from the item's top a break must not cut: its lines, rows and
    /// pictures. An item without any is one band, cut only where it is
    /// taller than a page.
    pub bands: Vec<Range<f64>>,
    /// The item's last band goes to the page of the next item's first one
    /// (a heading stays with the text under it).
    pub keep_with_next: bool,
}

/// Bands closer than this do not count as overlapping.
const BAND_SLACK: f64 = 0.5;

/// The pages of `items` stacked top to bottom with no gaps, `page_height`
/// tall: each page's range of the stack, together covering all of it. A page
/// ends where the band it would cut starts, unless that band started the
/// page (it is taller than a page): then it is cut where the page ends.
/// No items make one empty page.
pub fn paginate(items: &[PageItem], page_height: f64) -> Vec<Range<f64>> {
    let page_height = page_height.max(1.0);
    let mut bands: Vec<Range<f64>> = Vec::new();
    let mut top = 0.0;
    // Where the last band of an item kept with the next one starts.
    let mut held: Option<f64> = None;
    for item in items {
        let first = bands.len();
        if item.bands.is_empty() {
            if item.height > 0.0 {
                bands.push(top..top + item.height);
            }
        } else {
            bands.extend(
                item.bands
                    .iter()
                    .filter(|band| band.end > band.start)
                    .map(|band| top + band.start..top + band.end),
            );
        }
        let own = &bands[first..];
        if let Some(from) = held {
            // An item that drew nothing passes the hold on.
            if let Some(end) = own.iter().min_by(|a, b| a.start.total_cmp(&b.start)).map(|band| band.end) {
                bands.push(from..end);
                held = None;
            }
        }
        if item.keep_with_next {
            if let Some(start) = bands[first..].iter().map(|band| band.start).max_by(f64::total_cmp) {
                held = Some(start);
            }
        }
        top += item.height;
    }
    let total = top;
    bands.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut merged: Vec<Range<f64>> = Vec::with_capacity(bands.len());
    for band in bands {
        match merged.last_mut() {
            Some(last) if band.start < last.end - BAND_SLACK => last.end = last.end.max(band.end),
            _ => merged.push(band),
        }
    }
    let mut pages = Vec::new();
    let mut start = 0.0;
    loop {
        let mut end = start + page_height;
        if end >= total - BAND_SLACK {
            pages.push(start..total.max(start));
            return pages;
        }
        let cut = merged.partition_point(|band| band.start < end);
        if let Some(band) = cut.checked_sub(1).map(|i| &merged[i]) {
            if band.end > end + BAND_SLACK && band.start > start + BAND_SLACK {
                end = band.start;
            }
        }
        pages.push(start..end);
        start = end;
    }
}

/// A rendered page: its pixels as the GPU gave them back.
pub struct RenderedPage {
    pub index: usize,
    pub width: usize,
    pub height: usize,
    /// RGBA bands top to bottom: (pixels, width, rows); the last can hold
    /// rows past the page's height.
    bands: Vec<(Arc<[u8]>, usize, usize)>,
}

impl RenderedPage {
    /// The page as packed RGB rows, top to bottom (an image in a document),
    /// handed to `f` a band at a time, so a band's worth is all it holds.
    /// It touches every pixel: call it on a worker.
    pub fn rgb_bands(&self, mut f: impl FnMut(&[u8])) {
        if self.width == 0 {
            return;
        }
        let mut rgb = Vec::new();
        let mut rows = 0;
        for (pixels, width, band_rows) in &self.bands {
            let take = (*band_rows).min(self.height - rows);
            // Paper first, where a band narrower than the page (it never
            // is) leaves pixels uncovered.
            rgb.clear();
            rgb.resize(take * self.width * 3, 255);
            for (row, out) in rgb.chunks_exact_mut(self.width * 3).enumerate() {
                let line = &pixels[row * width * 4..(row + 1) * width * 4];
                for (out, pixel) in out.chunks_exact_mut(3).zip(line.chunks_exact(4)) {
                    out.copy_from_slice(&pixel[..3]);
                }
            }
            rows += take;
            if !rgb.is_empty() {
                f(&rgb);
            }
        }
        // Rows no band covers (there are none): paper.
        if rows < self.height {
            rgb.clear();
            rgb.resize((self.height - rows) * self.width * 3, 255);
            f(&rgb);
        }
    }
}

/// The most pixel bytes a band reads back (A4 at 300 dpi: nine bands of
/// 390 rows, 3.9 MB each).
const BAND_BYTES: usize = 4 << 20;

#[derive(Clone, Copy, Debug)]
enum Band {
    /// The band is drawn in the next draw.
    Draw,
    /// Drawn; its readback was refused and is asked for again.
    Ask,
    /// Its pixels are on their way.
    Wait(ReadbackTicket),
}

struct PageJob {
    index: usize,
    band: usize,
    state: Band,
    bands: Vec<(Arc<[u8]>, usize, usize)>,
    refused: usize,
}

/// Renders pages off screen and reads them back (see the module docs).
pub struct PageRender {
    format: PageFormat,
    pass: DrawPass,
    list: DrawList2d,
    color: Texture,
    _depth: Texture,
    band_rows: usize,
    band_count: usize,
    /// A place in the host's draw list: redrawing it brings the next band.
    host: Area,
    job: Option<PageJob>,
    failed: Option<ReadbackError>,
    retry: NextFrame,
}

impl PageRender {
    pub fn new(cx: &mut Cx, format: PageFormat) -> Self {
        let (width, height) = format.pixels();
        // A readback reserves its rows padded to 4 KiB; a band takes at most
        // half the context's budget, so it is never refused for good, and at
        // most BAND_BYTES: its pixels are copied on the UI thread as they
        // come back, so a smaller band keeps each frame's share small.
        let padded_row = (width * 4 + 4095) & !4095;
        let budget_rows = (TEXTURE_READBACK_MAX_BYTES / 2 / padded_row).min(BAND_BYTES / (width * 4).max(1)).max(1);
        let band_count = height.div_ceil(budget_rows);
        let band_rows = height.div_ceil(band_count);
        let size = TextureSize::Fixed { width, height: band_rows };
        let color = Texture::new_with_format(cx, TextureFormat::RenderBGRAu8 { size: size.clone(), initial: true }).label(cx, "page_render");
        // A depth target, as a window has: the page draws with the pipeline
        // each shader is built with (its colour layout over a depth target),
        // where a colour-only pass has the GPU backend compile another variant
        // of each on the UI thread (140 ms at a first print on Vulkan).
        let depth = Texture::new_with_format(cx, TextureFormat::DepthD32 { size, initial: true }).label(cx, "page_render depth");
        let pass = DrawPass::new_with_name(cx, "page_render");
        pass.set_size(cx, dvec2(width as f64, band_rows as f64) / format.scale());
        pass.set_color_texture(cx, &color, DrawPassClearColor::ClearWith(vec4(1.0, 1.0, 1.0, 1.0)));
        pass.set_depth_texture(cx, &depth, DrawPassClearDepth::ClearWith(1.0));
        Self {
            format,
            pass,
            list: DrawList2d::new(cx),
            color,
            _depth: depth,
            band_rows,
            band_count,
            host: Area::Empty,
            job: None,
            failed: None,
            retry: NextFrame::default(),
        }
    }

    pub fn format(&self) -> &PageFormat {
        &self.format
    }

    /// How many bands (draws and readbacks) a page takes.
    pub fn bands_per_page(&self) -> usize {
        self.band_count
    }

    /// A page is being made.
    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// Asks the host for a draw (the next band, or the next measuring step).
    pub fn redraw(&self, cx: &mut Cx) {
        cx.redraw_area(self.host);
    }

    /// Starts making page `index`, dropping a page in progress. The host
    /// draws it from its next draw on, a band at a time.
    pub fn render_page(&mut self, cx: &mut Cx, index: usize) {
        self.job = Some(PageJob { index, band: 0, state: Band::Draw, bands: Vec::new(), refused: 0 });
        self.failed = None;
        self.redraw(cx);
    }

    /// Stops the page in progress.
    pub fn cancel(&mut self) {
        self.job = None;
        self.failed = None;
    }

    /// Lays content out off screen at the page's resolution without showing
    /// it, for a host that measures what it will put on pages. Draw between
    /// this and [`Self::end_measure`] with walks that place themselves
    /// (`abs_pos`), the page's width across. False (draw nothing) while a
    /// page is being made: its target holds that page.
    pub fn begin_measure(&mut self, cx: &mut Cx2d) -> bool {
        cx.add_aligned_rect_area(&mut self.host, Rect::default());
        if self.job.is_some() {
            return false;
        }
        cx.begin_pass(&self.pass, Some(self.format.scale()));
        self.list.begin_always(cx);
        cx.begin_root_turtle(dvec2(self.format.size.x, 1.0e6), Layout::flow_overlay());
        true
    }

    pub fn end_measure(&mut self, cx: &mut Cx2d) {
        cx.end_pass_sized_turtle();
        self.list.end(cx);
        cx.end_pass(&self.pass);
    }

    /// Call in the host's draw. Answers the page to draw now: draw it in
    /// points, (0, 0) the paper's top left, then call [`Self::end_page`].
    /// None: nothing to draw this time.
    pub fn begin_page(&mut self, cx: &mut Cx2d) -> Option<usize> {
        cx.add_aligned_rect_area(&mut self.host, Rect::default());
        let job = self.job.as_ref()?;
        match job.state {
            Band::Draw => {}
            // Keep the pass attached while its pixels are owed.
            Band::Ask | Band::Wait(_) => {
                cx.make_child_pass(&self.pass);
                return None;
            }
        }
        let scale = self.format.scale();
        let (width, _) = self.format.pixels();
        let top = (job.band * self.band_rows) as f64 / scale;
        self.pass.set_size(cx, dvec2(width as f64 / scale, self.band_rows as f64 / scale));
        cx.set_pass_shift_scale(&self.pass, dvec2(0.0, top), dvec2(1.0, 1.0));
        cx.make_child_pass(&self.pass);
        cx.begin_pass(&self.pass, Some(scale));
        self.list.begin_always(cx);
        cx.begin_root_turtle(self.format.size, Layout::flow_overlay());
        Some(job.index)
    }

    pub fn end_page(&mut self, cx: &mut Cx2d) {
        cx.end_pass_sized_turtle();
        self.list.end(cx);
        cx.end_pass(&self.pass);
        cx.repaint_pass(self.pass.draw_pass_id());
        if let Some(job) = &mut self.job {
            job.state = Band::Ask;
        }
        self.ask(cx);
    }

    /// Asks for the drawn band's pixels.
    fn ask(&mut self, cx: &mut Cx) {
        let Some(job) = &mut self.job else { return };
        if !matches!(job.state, Band::Ask) {
            return;
        }
        match self.color.read_back(cx, ReadbackRequest::default()) {
            Ok(ticket) => {
                job.state = Band::Wait(ticket);
                job.refused = 0;
            }
            Err(ReadbackError::Backpressure) => {
                // Other readbacks hold the budget until they are taken.
                if job.refused == 0 {
                    log!("page render: readback budget in use; asking again each frame");
                }
                job.refused += 1;
                self.retry = cx.new_next_frame();
            }
            Err(ReadbackError::NotRendered) => {
                job.state = Band::Draw;
                cx.redraw_area(self.host);
            }
            Err(error) => self.failed = Some(error),
        }
    }

    /// Call with every event the host gets: takes a finished band and
    /// answers the page once its last band is in, or why it could not be
    /// made (the backend reads no pixels back).
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> Option<Result<RenderedPage, ReadbackError>> {
        if let Some(error) = self.failed.take() {
            self.job = None;
            return Some(Err(error));
        }
        if self.retry.is_event(event).is_some() {
            self.ask(cx);
        }
        let job = self.job.as_mut()?;
        let Band::Wait(ticket) = job.state else { return None };
        let result = cx.try_take_texture_readbacks_for(&[ticket]).pop()?;
        match result.data {
            Ok(pixels) => {
                job.bands.push((pixels, result.width, result.height));
                job.band += 1;
                if job.band < self.band_count {
                    job.state = Band::Draw;
                    cx.redraw_area(self.host);
                    return None;
                }
                let job = self.job.take()?;
                let (width, height) = self.format.pixels();
                Some(Ok(RenderedPage { index: job.index, width, height, bands: job.bands }))
            }
            // The target changed under the readback: draw the band again.
            Err(error @ (ReadbackError::Cancelled | ReadbackError::AllocationChanged | ReadbackError::NotRendered)) => {
                log!("page render: band {} of page {} again ({error:?})", job.band, job.index);
                job.state = Band::Draw;
                cx.redraw_area(self.host);
                None
            }
            Err(error) => {
                self.job = None;
                Some(Err(error))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lines 10 tall every 12, as one item.
    fn lines(count: usize) -> PageItem {
        PageItem {
            height: count as f64 * 12.0,
            bands: (0..count).map(|i| i as f64 * 12.0..i as f64 * 12.0 + 10.0).collect(),
            keep_with_next: false,
        }
    }

    #[test]
    fn pages_break_between_bands_and_cover_the_stack() {
        let check = |pages: &[Range<f64>], total: f64| {
            assert_eq!(pages.first().map(|p| p.start), Some(0.0), "pages start at the top");
            assert_eq!(pages.last().map(|p| p.end), Some(total), "pages end at the bottom");
            assert!(pages.windows(2).all(|w| w[0].end == w[1].start), "pages follow each other: {pages:?}");
        };

        // 10 lines on pages 50 tall: four lines a page (a fifth would be cut).
        let pages = paginate(&[lines(10)], 50.0);
        check(&pages, 120.0);
        assert_eq!(pages, vec![0.0..48.0, 48.0..96.0, 96.0..120.0], "breaks at line tops");

        // A band taller than a page is cut where the page ends.
        let tall = PageItem { height: 130.0, bands: vec![0.0..130.0], keep_with_next: false };
        let pages = paginate(&[tall, lines(1)], 50.0);
        check(&pages, 142.0);
        assert_eq!(pages[0], 0.0..50.0, "a band taller than a page is cut");

        // A heading at a page's foot goes over with the first line under it.
        let heading = PageItem { height: 20.0, bands: vec![2.0..18.0], keep_with_next: true };
        let pages = paginate(&[lines(3), heading, lines(3)], 60.0);
        check(&pages, 92.0);
        assert_eq!(pages[0].end, 38.0, "the heading starts the next page: {pages:?}");

        // An item without bands is not cut where it fits.
        let picture = PageItem { height: 30.0, bands: Vec::new(), keep_with_next: false };
        let pages = paginate(&[lines(3), picture], 50.0);
        check(&pages, 66.0);
        assert_eq!(pages[0].end, 36.0, "the picture moves to the next page");

        assert_eq!(paginate(&[], 50.0), vec![0.0..0.0], "no items: one empty page");
    }
}
