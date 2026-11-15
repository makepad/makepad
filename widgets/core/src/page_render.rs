//! Pages for print and export: an app lays its content out in points, this
//! renders each page off screen at print resolution and hands back its
//! pixels, and [`paginate`] decides where pages break.
//! Optional text capture hands back laid-out glyphs and leaves eligible text out of the pixels.
//!
//! A page renders in horizontal bands sized so a band's readback stays within
//! half of the context's readback budget and at most 4 MB (A4 at 300 dpi: nine
//! bands), one band per frame, so the UI thread never waits on the GPU. A
//! band's readback that waits for room holds its ticket; one refused at the
//! context's request limit is asked for again on a later frame, never dropped. The
//! host widget calls [`PageRender::begin_page`] in its draw (drawing the page
//! between it and [`PageRender::end_page`] when it answers a page) and
//! [`PageRender::handle_event`] with its events.
//!
//! A band nothing is drawn in (with text capture on: nothing but the text a writer
//! sets) is neither drawn again nor read back, and comes back as paper: the page's
//! first draw says, from the rects of the draws in its list, which bands hold
//! anything only the picture shows.

use crate::makepad_draw::*;
use std::{collections::VecDeque, ops::Range};

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

/// A rendered page: its pixels as the GPU gave them back, and any captured text.
pub struct RenderedPage {
    pub index: usize,
    pub width: usize,
    pub height: usize,
    /// Final glyph positions and clips when text capture was on, otherwise `None`.
    pub text: Option<CapturedText>,
    /// Bands top to bottom; the last can hold rows past the page's height.
    bands: Vec<BandPixels>,
}

/// A band as read back (RGBA pixels, width, rows), or paper: nothing in it
/// that only the picture shows, so it was not read back.
enum BandPixels {
    Read(HostBuf, usize, usize),
    Paper(usize),
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
        for band in &self.bands {
            let band_rows = match band {
                BandPixels::Read(_, _, rows) | BandPixels::Paper(rows) => *rows,
            };
            let take = band_rows.min(self.height - rows);
            // Paper first, where a band narrower than the page (it never
            // is) leaves pixels uncovered.
            rgb.clear();
            rgb.resize(take * self.width * 3, 255);
            if let BandPixels::Read(pixels, width, _) = band {
                for (row, out) in rgb.chunks_exact_mut(self.width * 3).enumerate() {
                    let line = &pixels[row * width * 4..(row + 1) * width * 4];
                    for (out, pixel) in out.chunks_exact_mut(3).zip(line.chunks_exact(4)) {
                        out.copy_from_slice(&pixel[..3]);
                    }
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

    /// Nothing on the page is only in the picture: every band is paper.
    pub fn is_paper(&self) -> bool {
        self.bands.iter().all(|band| matches!(band, BandPixels::Paper(_)))
    }

    /// How many bands were read back.
    pub fn read_bands(&self) -> usize {
        self.bands.iter().filter(|band| matches!(band, BandPixels::Read(..))).count()
    }
}

/// The most pixel bytes a band reads back (A4 at 300 dpi: nine bands of
/// 390 rows, 3.9 MB each).
const BAND_BYTES: usize = 4 << 20;

/// How far past its rect a draw may paint, in points, for the bands it counts
/// on: anti-aliasing and a glyph's padding. A shader that paints farther
/// outside its rect (a wide shadow) is cut at a paper band's edge.
const BLEED: f64 = 4.0;

/// Bands drawn and read back at once, each into its own target: three
/// readbacks of at most [`BAND_BYTES`] come back in a frame.
const BANDS_IN_FLIGHT: usize = 3;

/// A band's target: its pass, list and textures, and the band it holds.
struct Slot {
    pass: DrawPass,
    list: DrawList2d,
    color: Texture,
    _depth: Texture,
    state: SlotState,
}

#[derive(Clone, Copy, Debug)]
enum SlotState {
    Free,
    /// Band drawn; its readback is to be asked for (again, once refused).
    Ask(usize),
    /// The band's pixels are on their way.
    Wait(usize, ReadbackTicket),
}

struct PageJob {
    index: usize,
    /// The bands still to draw, in order; a band whose readback must be
    /// made again comes back to the front.
    todo: VecDeque<usize>,
    /// Each band once it is in.
    bands: Vec<Option<BandPixels>>,
    /// Per band, whether it holds a draw only the picture shows; found by the
    /// page's first draw.
    raster: Option<Vec<bool>>,
    /// The band being drawn and its slot, between `begin_page` and `end_page`.
    drawing: Option<(usize, usize)>,
    refused: usize,
    capture_text: bool,
    stock_shader: Option<DrawShaderId>,
    text: Option<CapturedText>,
}

/// Renders pages off screen and reads them back (see the module docs).
pub struct PageRender {
    format: PageFormat,
    capture_text: bool,
    slots: Vec<Slot>,
    band_rows: usize,
    band_count: usize,
    /// A place in the host's draw list: redrawing it brings the next bands.
    host: Area,
    job: Option<PageJob>,
    failed: Option<ReadbackError>,
    retry: NextFrame,
}

impl PageRender {
    pub fn new(cx: &mut Cx, format: PageFormat) -> Self {
        let (width, height) = format.pixels();
        // A readback reserves its rows padded to 4 KiB; a band takes at most
        // half the context's budget, so it is admitted beside other readbacks
        // rather than waiting for all of them to be taken, and at most
        // BAND_BYTES: its pixels are copied on the UI thread as they
        // come back, so a smaller band keeps each frame's share small.
        let padded_row = (width * 4 + 4095) & !4095;
        let budget_rows = (TEXTURE_READBACK_MAX_BYTES / 2 / padded_row).min(BAND_BYTES / (width * 4).max(1)).max(1);
        let band_count = height.div_ceil(budget_rows);
        let band_rows = height.div_ceil(band_count);
        let slots = (0..BANDS_IN_FLIGHT.min(band_count))
            .map(|_| {
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
                Slot { pass, list: DrawList2d::new(cx), color, _depth: depth, state: SlotState::Free }
            })
            .collect();
        Self {
            format,
            capture_text: false,
            slots,
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

    /// With capture on, stock `DrawText` in embeddable faces is left out of the
    /// next page's pixels and returned in `RenderedPage::text` for a writer to set
    /// above the picture. Other shaders and faces stay in the pixels and are also
    /// returned, marked `in_pixels`. Anything drawn over text ends up under it in
    /// such a document. Glyph-only draws stay in the pixels and are not returned.
    /// Capture is off by default.
    pub fn set_text_capture(&mut self, on: bool) {
        self.capture_text = on;
    }

    /// How many bands (draws and readbacks) a page takes at most.
    pub fn bands_per_page(&self) -> usize {
        self.band_count
    }

    /// A page is being made.
    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// Asks the host for a draw (the next bands, or the next measuring step).
    pub fn redraw(&self, cx: &mut Cx) {
        cx.redraw_area(self.host);
    }

    /// Starts making page `index`, dropping a page in progress. The host
    /// draws it from its next draw on, a few bands at a time.
    pub fn render_page(&mut self, cx: &mut Cx, index: usize) {
        let stock_shader = if self.capture_text { stock_text_shader(cx) } else { None };
        self.job = Some(PageJob {
            index,
            todo: (0..self.band_count).collect(),
            bands: (0..self.band_count).map(|_| None).collect(),
            raster: None,
            drawing: None,
            refused: 0,
            capture_text: self.capture_text,
            stock_shader,
            text: None,
        });
        for slot in &mut self.slots {
            if let SlotState::Wait(_, ticket) = slot.state {
                cx.cancel_texture_readback(ticket);
            }
            slot.state = SlotState::Free;
        }
        self.failed = None;
        self.redraw(cx);
    }

    /// Stops the page in progress.
    pub fn cancel(&mut self) {
        self.job = None;
        for slot in &mut self.slots {
            slot.state = SlotState::Free;
        }
        self.failed = None;
    }

    /// Lays content out off screen at the page's resolution without showing
    /// it, for a host that measures what it will put on pages. Draw between
    /// this and [`Self::end_measure`] with walks that place themselves
    /// (`abs_pos`), the page's width across. False (draw nothing) while a
    /// page is being made: its targets hold that page.
    pub fn begin_measure(&mut self, cx: &mut Cx2d) -> bool {
        cx.add_aligned_rect_area(&mut self.host, Rect::default());
        if self.job.is_some() {
            return false;
        }
        let slot = &mut self.slots[0];
        cx.begin_pass(&slot.pass, Some(self.format.scale()));
        slot.list.begin_always(cx);
        cx.begin_root_turtle(dvec2(self.format.size.x, 1.0e6), Layout::flow_overlay());
        true
    }

    pub fn end_measure(&mut self, cx: &mut Cx2d) {
        cx.end_pass_sized_turtle();
        let slot = &mut self.slots[0];
        slot.list.end(cx);
        cx.end_pass(&slot.pass);
    }

    /// Call in the host's draw, again after each [`Self::end_page`] until it
    /// answers `None`: a page to draw now, a band of it into a target of its
    /// own (a few each draw). Draw it in points, (0, 0) the paper's top left,
    /// then call [`Self::end_page`]. A page's first band is drawn alone: its
    /// draw says which of the others hold anything the picture shows.
    pub fn begin_page(&mut self, cx: &mut Cx2d) -> Option<usize> {
        cx.add_aligned_rect_area(&mut self.host, Rect::default());
        let job = self.job.as_mut()?;
        // Keep each pass attached while its pixels are owed.
        for slot in &self.slots {
            if !matches!(slot.state, SlotState::Free) {
                cx.make_child_pass(&slot.pass);
            }
        }
        let free = self.slots.iter().position(|slot| matches!(slot.state, SlotState::Free))?;
        let band = job.todo.pop_front()?;
        job.drawing = Some((band, free));
        let scale = self.format.scale();
        let (width, _) = self.format.pixels();
        let top = (band * self.band_rows) as f64 / scale;
        let slot = &mut self.slots[free];
        slot.pass.set_size(cx, dvec2(width as f64 / scale, self.band_rows as f64 / scale));
        cx.set_pass_shift_scale(&slot.pass, dvec2(0.0, top), dvec2(1.0, 1.0));
        cx.make_child_pass(&slot.pass);
        cx.begin_pass(&slot.pass, Some(scale));
        slot.list.begin_always(cx);
        cx.begin_root_turtle(self.format.size, Layout::flow_overlay());
        if job.capture_text {
            cx.glyph_capture = Some(GlyphCapture::new(band == 0, job.stock_shader));
        }
        Some(job.index)
    }

    pub fn end_page(&mut self, cx: &mut Cx2d) {
        cx.end_pass_sized_turtle();
        let capture = cx.glyph_capture.take();
        let Some(job) = &mut self.job else { return };
        let Some((band, slot)) = job.drawing.take() else { return };
        if let Some(capture) = capture.filter(GlyphCapture::records) {
            job.text = capture.finish(cx);
        }
        let slot = &mut self.slots[slot];
        slot.list.end(cx);
        cx.end_pass(&slot.pass);
        if job.raster.is_none() {
            let raster = raster_bands(cx, slot.list.id(), self.band_rows, self.band_count, self.format.scale());
            for (raster, pixels) in raster.iter().zip(&mut job.bands) {
                if !raster {
                    *pixels = Some(BandPixels::Paper(self.band_rows));
                }
            }
            job.todo.retain(|band| raster[*band]);
            job.raster = Some(raster);
        }
        if job.bands[band].is_some() {
            // Paper: neither painted nor read back.
            if job.bands.iter().all(Option::is_some) {
                self.retry = cx.new_next_frame();
            }
            return;
        }
        cx.repaint_pass(slot.pass.draw_pass_id());
        slot.state = SlotState::Ask(band);
        self.ask(cx);
    }

    /// Asks for each drawn band's pixels.
    fn ask(&mut self, cx: &mut Cx) {
        let Some(job) = &mut self.job else { return };
        for slot in &mut self.slots {
            let SlotState::Ask(band) = slot.state else { continue };
            match slot.color.read_back(cx, ReadbackRequest::default()) {
                Ok(ticket) => {
                    slot.state = SlotState::Wait(band, ticket);
                    job.refused = 0;
                }
                Err(ReadbackError::Backpressure) => {
                    // The per-context limit of outstanding readback tickets is reached:
                    // ask again each frame until results are taken.
                    if job.refused == 0 {
                        log!("page render: per-context readback request limit reached; band waits, asking again each frame");
                    }
                    job.refused += 1;
                    self.retry = cx.new_next_frame();
                }
                Err(ReadbackError::NotRendered) => {
                    slot.state = SlotState::Free;
                    job.todo.push_front(band);
                    cx.redraw_area(self.host);
                }
                Err(error) => {
                    self.failed = Some(error);
                    return;
                }
            }
        }
    }

    /// Call with every event the host gets: takes finished bands and
    /// answers the page once its last band is in, or why it could not be
    /// made (the backend reads no pixels back).
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> Option<Result<RenderedPage, ReadbackError>> {
        if let Some(error) = self.failed.take() {
            self.cancel();
            return Some(Err(error));
        }
        if self.retry.is_event(event).is_some() {
            self.ask(cx);
        }
        let job = self.job.as_mut()?;
        let tickets: Vec<ReadbackTicket> = self
            .slots
            .iter()
            .filter_map(|slot| match slot.state {
                SlotState::Wait(_, ticket) => Some(ticket),
                _ => None,
            })
            .collect();
        let mut freed = false;
        for result in cx.try_take_texture_readbacks_for(&tickets) {
            let Some(slot) = self
                .slots
                .iter_mut()
                .find(|slot| matches!(slot.state, SlotState::Wait(_, ticket) if ticket == result.ticket))
            else {
                continue;
            };
            let SlotState::Wait(band, _) = slot.state else { continue };
            slot.state = SlotState::Free;
            freed = true;
            match result.data {
                Ok(pixels) => job.bands[band] = Some(BandPixels::Read(pixels, result.width, result.height)),
                // The target changed under the readback: draw the band again.
                Err(error @ (ReadbackError::Cancelled | ReadbackError::AllocationChanged | ReadbackError::NotRendered)) => {
                    log!("page render: band {band} of page {} again ({error:?})", job.index);
                    job.todo.push_front(band);
                }
                Err(error) => {
                    self.cancel();
                    return Some(Err(error));
                }
            }
        }
        if job.bands.iter().all(Option::is_some) {
            let job = self.job.take()?;
            let (width, height) = self.format.pixels();
            let bands = job.bands.into_iter().flatten().collect();
            return Some(Ok(RenderedPage { index: job.index, width, height, text: job.text, bands }));
        }
        if freed && !job.todo.is_empty() {
            cx.redraw_area(self.host);
        }
        None
    }
}

/// Per band of `band_rows` pixel rows, whether a draw in `list` (or a list in
/// it) paints there: its instances' rects in points, clipped, `scale` pixels a
/// point, widened by [`BLEED`]. Text the capture took out left no instances.
/// A draw it cannot place (a shader without a rect, instances it does not hold,
/// a list that moves or transforms its content, a native layer) counts on
/// every band. Runs on the UI thread once per page: one pass over the
/// instances left in the list.
fn raster_bands(cx: &Cx, list: DrawListId, band_rows: usize, count: usize, scale: f64) -> Vec<bool> {
    let everywhere = vec![true; count];
    let mut raster = vec![false; count];
    let band_points = band_rows as f64 / scale;
    let mut lists = vec![list];
    while let Some(list) = lists.pop() {
        let list = &cx.draw_lists[list];
        let uniforms = &list.draw_list_uniforms;
        if uniforms.view_shift != vec2(0.0, 0.0) || uniforms.view_transform != Mat4f::identity() {
            return everywhere;
        }
        let clip = uniforms.view_clip;
        for order in 0..list.draw_item_order_len() {
            let Some(item) = list.draw_item_id_at_order_index(order) else { continue };
            let item = &list.draw_items[item];
            let call = match &item.kind {
                CxDrawKind::DrawCall(call) => call,
                CxDrawKind::SubList(sub) => {
                    lists.push(*sub);
                    continue;
                }
                CxDrawKind::Empty => continue,
                CxDrawKind::NativeLayer { .. } => return everywhere,
            };
            if item.retained_instances.is_some() {
                return everywhere;
            }
            let Some(instances) = item.instances.as_ref().filter(|instances| !instances.is_empty()) else { continue };
            let mapping = &cx.draw_shaders[call.draw_shader_id.index].mapping;
            let stride = mapping.instances.total_slots;
            let (Some(pos), Some(size)) = (mapping.rect_pos, mapping.rect_size) else { return everywhere };
            if stride == 0 || mapping.instances.stride_bytes != stride * 4 {
                return everywhere;
            }
            for instance in instances.chunks_exact(stride) {
                let (mut x0, mut y0) = (instance[pos] as f64, instance[pos + 1] as f64);
                let (mut x1, mut y1) = (x0 + instance[size] as f64, y0 + instance[size + 1] as f64);
                if let Some(draw_clip) = mapping.draw_clip {
                    x0 = x0.max(instance[draw_clip] as f64);
                    y0 = y0.max(instance[draw_clip + 1] as f64);
                    x1 = x1.min(instance[draw_clip + 2] as f64);
                    y1 = y1.min(instance[draw_clip + 3] as f64);
                }
                x0 = x0.max(clip.x as f64);
                y0 = y0.max(clip.y as f64);
                x1 = x1.min(clip.z as f64);
                y1 = y1.min(clip.w as f64);
                if !(x1 > x0 && y1 > y0) {
                    continue;
                }
                let first = ((y0 - BLEED) / band_points).floor().max(0.0) as usize;
                let last = (((y1 + BLEED) / band_points).ceil().max(0.0) as usize).min(count);
                for band in first..last {
                    raster[band] = true;
                }
            }
        }
    }
    raster
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
