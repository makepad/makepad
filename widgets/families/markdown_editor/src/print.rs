//! The document's pages for print and PDF export, made from the final view's
//! rows as they read on paper (dark ink on white, no caret, links as text).
//!
//! A job works on a copy of the document taken when it starts, so editing
//! goes on meanwhile. It first reads and decodes the document's pictures on
//! workers (a page is measured with every picture in it, or the alt text of
//! one that did not load). It measures every row once at the page's content width
//! (a slice of each frame), cuts the stack into pages where no line, table
//! row or picture breaks (a heading goes with the text under it), then has
//! each page rendered off screen (`PageRender`), compressed on a worker of
//! its own (pages in parallel) and added to the PDF in order on another.
//! Pages carry PDF text over a picture without that text; blank paper is
//! left out of the pictures.

use crate::editor::{follows_block, heading_anchor, slug, RowCache};
use crate::pdf_structure::{PageStructure, StructureBuilder};
use crate::pdf_text::{PdfText, TextStats};
use crate::rowmap::flat_of;
use makepad_markdown_edit::{
    parse_options,
    pulldown_cmark::{Event as MdEvent, Parser, Tag},
    BlockKind, Index,
};
use makepad_pdf_write::{
    DocInfo, Link as PdfLink, LinkTarget, OutlineEntry, Page, PageImage, PdfWriter, Rect as PdfRect, RgbImage,
    RgbImageEncoder, StructId, Tagging, WritingMode,
};
use makepad_widgets_core::image_cache::{decode_image_from_data, looks_like_svg, ImageBuffer};
use makepad_widgets_core::makepad_draw::glyph_capture::CapturedText;
use makepad_widgets_core::makepad_platform::thread::{Class, SignalToUI, TaskHandle};
use makepad_widgets_core::*;
use makepad_widgets_rich_text::markdown::{picture_bytes, resolve_source, Markdown, MarkdownHost};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    ops::Range,
    path::PathBuf,
    sync::Arc,
};

/// How long a frame measures rows for.
const MEASURE_SLICE: f64 = 0.006;
/// Rendered pages compressing at once before the next is rendered (each
/// holds its pixels until it is done: 35 MB at A4, 300 dpi), and no more
/// than the pool has workers.
const PAGES_AHEAD: usize = 4;

/// Where a print job stands, for its host.
#[derive(Clone, Debug, PartialEq)]
pub enum PrintUpdate {
    /// `done` pages of `pages` are made (`pages` is 0 while the rows are
    /// measured).
    Progress { done: usize, pages: usize },
    /// The document as a PDF.
    Done(Arc<[u8]>),
    Failed(String),
}

enum Writer {
    Idle(Document),
    /// A step on a worker hands back what it made and how long it took.
    Adding(TaskHandle<(Document, f64)>),
    Finishing(TaskHandle<Result<(Arc<[u8]>, TextStats, f64), String>>),
    Done,
}

/// The writer, the shared font bytes its pages have used and the writer's ids of the structure
/// elements so far, moving together between workers.
struct Document {
    writer: PdfWriter,
    text: PdfText,
    ids: Vec<StructId>,
}

/// A page's compressed pictures and the text that belongs above them.
struct CompressedPage {
    segments: Vec<(RgbImage, Range<usize>)>,
    text: Option<CapturedText>,
    left_out: usize,
}

/// A picture read and decoded on a worker.
enum Decoded {
    Raster(ImageBuffer),
    /// An SVG draws in place, as the view draws it.
    Svg,
    Failed(String),
}

/// The document's pictures on their way, before the rows are measured.
enum Pictures {
    /// Their sources, from a parse of the copy on a worker.
    Finding(TaskHandle<Vec<String>>),
    /// Each read and decoded on a worker of its own.
    Loading(Vec<(String, TaskHandle<Decoded>)>),
    Ready,
}

/// The time each stage took, logged when the PDF is made.
#[derive(Default)]
struct Times {
    started: f64,
    /// When the pictures were ready, how many, and how many did not load.
    pictures: f64,
    picture_count: usize,
    pictures_failed: usize,
    measured: f64,
    paginated: f64,
    /// The longest a frame measured for, and the draws of page bands.
    measure_max: f64,
    band_total: f64,
    band_max: f64,
    bands: usize,
    /// Bands read back; the others were paper.
    read_bands: usize,
    /// The pages' compression on workers (pixels to RGB, Flate), summed
    /// and the longest, and the times rendering waited for it.
    compress: f64,
    compress_max: f64,
    held: usize,
    /// The writer's time on its worker (adding pages, finishing).
    writer: f64,
    /// The link areas the pages hold.
    links: usize,
    segments: usize,
    blank_rows: usize,
}

/// A heading as the PDF finds it: a bookmark, and where `#anchor` links go.
struct Heading {
    title: String,
    anchor: String,
    level: u8,
    page: usize,
    /// From the page's top, in points.
    y: f32,
}

pub(crate) struct PrintJob {
    title: String,
    index: Index,
    lines: Arc<Vec<String>>,
    /// The pictures, by their source as the rows read it: decoded, or
    /// `None` for one that did not load (drawn as its alt text).
    pictures: Pictures,
    loaded: HashMap<String, Option<Texture>>,
    render: PageRender,
    /// The rows measured so far, with their tops in the stack.
    items: Vec<PageItem>,
    tops: Vec<f64>,
    pages: Vec<Range<f64>>,
    /// The headings, in document order, once the rows are paginated.
    headings: Vec<Heading>,
    /// Each page's links, found in the first draw of its page.
    links: Vec<Option<Vec<PdfLink>>>,
    /// The document's structure elements, made as pages are first drawn, and each page's part.
    structure: StructureBuilder,
    page_structures: Vec<Option<PageStructure>>,
    /// Pages handed to the renderer, rendered, and added to the PDF.
    started: usize,
    rendered: usize,
    added: usize,
    /// Pages compressing, in page order, how many may at once, and the
    /// compressed ones waiting for the writer.
    compressing: VecDeque<TaskHandle<(CompressedPage, f64)>>,
    ahead: usize,
    held: bool,
    compressed: Vec<CompressedPage>,
    writer: Writer,
    times: Times,
}

impl PrintJob {
    /// A job over `lines`, whose relative pictures are read from `base_dir`.
    pub(crate) fn new(cx: &mut Cx, title: &str, index: Index, lines: Vec<String>, base_dir: Option<PathBuf>) -> Self {
        let mut writer = PdfWriter::new(DocInfo { title: title.to_string(), producer: "Makepad".to_string() });
        // The view lays every paragraph out left to right.
        writer.set_tagged(Tagging { lang: front_matter_lang(&lines), direction: WritingMode::LrTb });
        let mut render = PageRender::new(cx, PageFormat::a4());
        render.set_text_capture(true);
        let lines = Arc::new(lines);
        let copy = lines.clone();
        let find = move || {
            let sources = picture_sources(&copy, base_dir.as_deref());
            SignalToUI::set_ui_signal();
            sources
        };
        // Without a worker the rows are measured as they draw: a picture
        // that is still loading shows its alt text.
        let pictures = match cx.task_pool().submit(Class::Visible, find) {
            Ok(task) => Pictures::Finding(task),
            Err(error) => {
                log!("print: pictures not read ahead, no worker: {error}");
                Pictures::Ready
            }
        };
        Self {
            title: title.to_string(),
            index,
            lines,
            pictures,
            loaded: HashMap::new(),
            render,
            items: Vec::new(),
            tops: Vec::new(),
            pages: Vec::new(),
            headings: Vec::new(),
            links: Vec::new(),
            structure: StructureBuilder::default(),
            page_structures: Vec::new(),
            started: 0,
            rendered: 0,
            added: 0,
            compressing: VecDeque::new(),
            ahead: PAGES_AHEAD.min(cx.task_pool().worker_count()).max(1),
            held: false,
            compressed: Vec::new(),
            writer: Writer::Idle(Document { writer, text: PdfText::new(), ids: Vec::new() }),
            times: Times { started: Cx::time_now(), ..Times::default() },
        }
    }

    pub(crate) fn title(&self) -> &str {
        &self.title
    }

    /// The job's draw: measures rows, or draws the page being rendered.
    pub(crate) fn draw(&mut self, cx: &mut Cx2d, md: &mut Markdown, number: &mut DrawText) {
        if !matches!(self.pictures, Pictures::Ready) {
            // Nothing to draw yet, but the renderer keeps its place in this
            // draw, so the redraw once the pictures are in reaches the host.
            let _ = self.render.begin_page(cx);
            return;
        }
        let rows = self.index.row_count();
        if self.items.len() < rows || self.times.measured == 0.0 {
            self.measure(cx, md, rows);
            return;
        }
        // A few bands of the page each draw, each into a target of its own.
        while let Some(page) = self.render.begin_page(cx) {
            let start = Cx::time_now();
            // The page's first draw finds its links and its part of the structure.
            let mut builder = self.links.get(page).is_some_and(Option::is_none).then(|| std::mem::take(&mut self.structure));
            let found = self.draw_page(cx, md, page, builder.as_mut());
            if let Some(builder) = builder {
                self.structure = builder;
            }
            if let Some((links, structure)) = found {
                if let Some(slot) = self.links.get_mut(page) {
                    *slot = Some(links);
                }
                if let Some(slot) = self.page_structures.get_mut(page) {
                    *slot = Some(structure);
                }
            }
            // The page's number, centred in the margin under the text.
            let format = *self.render.format();
            let content = format.content();
            let foot = Walk::abs_rect(Rect {
                pos: dvec2(content.pos.x, content.pos.y + content.size.y),
                size: dvec2(content.size.x, format.margin.bottom),
            });
            number.draw_walk(cx, foot, Align { x: 0.5, y: 0.5 }, &format!("{} / {}", page + 1, self.pages.len()));
            self.render.end_page(cx);
            let took = Cx::time_now() - start;
            self.times.band_total += took;
            self.times.band_max = self.times.band_max.max(took);
            self.times.bands += 1;
        }
    }

    fn measure(&mut self, cx: &mut Cx2d, md: &mut Markdown, rows: usize) {
        if !self.render.begin_measure(cx) {
            return;
        }
        let content = self.render.format().content();
        let start = Cx::time_now();
        let until = start + MEASURE_SLICE;
        while self.items.len() < rows {
            let row = self.items.len();
            let (height, bands) = self.draw_row(cx, md, row, dvec2(0.0, 0.0), content.size.x, None, None);
            let keep_with_next = matches!(self.index.blocks()[self.index.row(row).block].kind, BlockKind::Heading(_));
            self.items.push(PageItem { height, bands, keep_with_next });
            if Cx::time_now() > until {
                break;
            }
        }
        self.render.end_measure(cx);
        self.times.measure_max = self.times.measure_max.max(Cx::time_now() - start);
        if self.items.len() < rows {
            self.render.redraw(cx);
            return;
        }
        self.times.measured = Cx::time_now();
        let mut top = 0.0;
        self.tops = self
            .items
            .iter()
            .map(|item| {
                let at = top;
                top += item.height;
                at
            })
            .collect();
        self.pages = paginate(&self.items, content.size.y);
        self.headings = self.find_headings(content.pos.y);
        self.links = (0..self.pages.len()).map(|_| None).collect();
        self.page_structures = (0..self.pages.len()).map(|_| None).collect();
        self.times.paginated = Cx::time_now();
        self.next_page(cx);
    }

    /// The headings' titles, anchors and places on the pages (`content_top`
    /// down the paper), once the rows are paginated.
    fn find_headings(&self, content_top: f64) -> Vec<Heading> {
        let index = &self.index;
        let mut headings = Vec::new();
        let mut last = None;
        for (row, top) in self.tops.iter().enumerate() {
            let block = index.row(row).block;
            let BlockKind::Heading(level) = &index.blocks()[block].kind else { continue };
            if last.replace(block) == Some(block) {
                continue;
            }
            let page = self.pages.partition_point(|window| window.end <= *top).min(self.pages.len().saturating_sub(1));
            let Some(window) = self.pages.get(page) else { break };
            let lines = &self.lines[index.block_lines(block)];
            headings.push(Heading {
                title: index.reading_text(&lines.join("\n")),
                anchor: lines.first().map(|line| heading_anchor(line)).unwrap_or_default(),
                level: *level,
                page,
                y: (content_top + top - window.start) as f32,
            });
        }
        headings
    }

    /// A link drawn over `rect` (page points) as the PDF holds it, kept to
    /// the part of the page the text shows (`clip`). None for one a reader
    /// cannot follow: a path relative to the document, or an anchor no
    /// heading has.
    fn pdf_link(&self, url: &str, rect: Rect, clip: Rect) -> Option<PdfLink> {
        let target = if let Some(anchor) = url.strip_prefix('#') {
            let anchor = slug(anchor);
            let heading = self.headings.iter().find(|heading| heading.anchor == anchor)?;
            LinkTarget::Page { index: heading.page, y: heading.y }
        } else if has_scheme(url) {
            LinkTarget::Uri(url.to_string())
        } else {
            return None;
        };
        let x0 = rect.pos.x.max(clip.pos.x);
        let y0 = rect.pos.y.max(clip.pos.y);
        let x1 = (rect.pos.x + rect.size.x).min(clip.pos.x + clip.size.x);
        let y1 = (rect.pos.y + rect.size.y).min(clip.pos.y + clip.size.y);
        (x1 > x0 && y1 > y0).then(|| PdfLink {
            rect: PdfRect { x: x0 as f32, y: y0 as f32, w: (x1 - x0) as f32, h: (y1 - y0) as f32 },
            target,
        })
    }

    /// Draws row `row` of the copy at `pos`, `width` across: its height and
    /// the bands a page break must not cut. With `links`, adds where each
    /// of its links drew (a rect per line) and its destination; with `tree`,
    /// its elements and where their lines drew on page `tree.2`.
    #[allow(clippy::too_many_arguments)]
    fn draw_row(
        &self,
        cx: &mut Cx2d,
        md: &mut Markdown,
        row_id: usize,
        pos: DVec2,
        width: f64,
        links: Option<&mut Vec<(String, Rect)>>,
        tree: Option<(&mut StructureBuilder, &mut PageStructure, usize)>,
    ) -> (f64, Vec<Range<f64>>) {
        let index = &self.index;
        let row = index.row(row_id);
        let cache = RowCache::build(index, &self.lines, &row);
        let footnote = |label: &str| index.footnote_number(label);
        let mut host = MarkdownHost {
            footnote_number: Some(&footnote),
            list_start: cache.list_start,
            follows_block: follows_block(row_id, &row),
            pictures: Some(&self.loaded),
            ..Default::default()
        };
        let walk = Walk { abs_pos: Some(pos), width: Size::Fixed(width), height: Size::fit(), ..Walk::default() };
        md.draw_events(cx, walk, &cache.slice.text, cache.events.iter().cloned(), &mut host);
        if let Some(links) = links {
            // Where the flow ended up, if it moved after its glyphs were placed.
            let shift = md.text_flow.drawn_shift(cx);
            for (event, range) in &cache.events {
                let MdEvent::Start(Tag::Link { dest_url, .. }) = event else { continue };
                let (start, end) = (flat_of(&host.pieces, range.start), flat_of(&host.pieces, range.end));
                for rect in md.text_flow.selection_rects(start, end) {
                    links.push((dest_url.to_string(), Rect { pos: rect.pos + shift, size: rect.size }));
                }
            }
        }
        if let Some((builder, structure, page)) = tree {
            let shift = md.text_flow.drawn_shift(cx);
            let (flow, pieces) = (&md.text_flow, &host.pieces);
            let rects = |start: usize, end: usize| -> Vec<Rect> {
                flow.selection_rects(flat_of(pieces, start), flat_of(pieces, end))
                    .into_iter()
                    .map(|rect| Rect { pos: rect.pos + shift, size: rect.size })
                    .collect()
            };
            builder.walk_row(structure, page, row_id, row.block, pos.x, &cache.events, &rects);
        }
        (md.text_flow.area().rect(cx).size.y, md.text_flow.break_hints(cx))
    }

    /// Draws page `page`; with `builder` (its first draw), answers the links it holds and its
    /// part of the document's structure.
    fn draw_page(
        &self,
        cx: &mut Cx2d,
        md: &mut Markdown,
        page: usize,
        mut builder: Option<&mut StructureBuilder>,
    ) -> Option<(Vec<PdfLink>, PageStructure)> {
        let content = self.render.format().content();
        let window = self.pages[page].clone();
        // The page shows its share of the stack, clipped below where its
        // last line ends (the next page starts there).
        let clip = Rect { pos: content.pos, size: dvec2(content.size.x, window.end - window.start) };
        cx.begin_turtle(Walk::abs_rect(clip), Layout::flow_overlay());
        let mut found = builder.is_some().then(Vec::new);
        let mut structure = builder.as_deref().map(StructureBuilder::begin_page);
        let first = self
            .tops
            .iter()
            .zip(&self.items)
            .take_while(|(top, item)| **top + item.height <= window.start)
            .count();
        for row in first..self.items.len() {
            let top = self.tops[row];
            if top >= window.end {
                break;
            }
            if self.items[row].height > 0.0 {
                let pos = content.pos + dvec2(0.0, top - window.start);
                if let Some(structure) = &mut structure {
                    let (top, bottom) = (pos.y.max(clip.pos.y), (pos.y + self.items[row].height).min(clip.pos.y + clip.size.y));
                    structure.add_row(top, bottom);
                }
                let tree = match (&mut builder, &mut structure) {
                    (Some(builder), Some(structure)) => Some((&mut **builder, structure, page)),
                    _ => None,
                };
                self.draw_row(cx, md, row, pos, content.size.x, found.as_mut(), tree);
            }
        }
        cx.end_turtle();
        let links = found?.into_iter().filter_map(|(url, rect)| self.pdf_link(&url, rect, clip)).collect();
        Some((links, structure.unwrap_or_default()))
    }

    /// Renders the next page while few rendered ones are compressing.
    fn next_page(&mut self, cx: &mut Cx) {
        if self.render.busy() || self.started == self.pages.len() {
            return;
        }
        if self.compressing.len() < self.ahead {
            self.render.render_page(cx, self.started);
            self.started += 1;
            self.held = false;
        } else if !self.held {
            self.held = true;
            self.times.held += 1;
        }
    }

    /// Compresses a rendered page on a worker, a band at a time.
    fn compress(&mut self, cx: &mut Cx, page: RenderedPage, text: Option<CapturedText>) -> Result<(), String> {
        let job = move || {
            let start = Cx::time_now();
            let (segments, left_out) = compress_segments(&page);
            SignalToUI::set_ui_signal();
            (CompressedPage { segments, text, left_out }, Cx::time_now() - start)
        };
        let task = cx.task_pool().submit(Class::Visible, job).map_err(|error| format!("no worker to compress a page on: {error}"))?;
        self.compressing.push_back(task);
        Ok(())
    }

    /// Takes the pictures that are ready; once all are, the rows are
    /// measured (from the next draw).
    fn load_pictures(&mut self, cx: &mut Cx) {
        if let Pictures::Finding(task) = &mut self.pictures {
            let Some(found) = task.try_take() else { return };
            let sources = found.unwrap_or_else(|error| {
                log!("print: the pictures were not found: {error:?}");
                Vec::new()
            });
            let mut loading = Vec::new();
            for src in sources {
                let job = {
                    let src = src.clone();
                    move || {
                        let decoded = match picture_bytes(&src) {
                            None => Decoded::Failed("not readable".to_string()),
                            Some(bytes) if looks_like_svg(&bytes) => Decoded::Svg,
                            Some(bytes) => match decode_image_from_data(&bytes) {
                                Ok(mut image) => {
                                    image.prepare_for_texture();
                                    Decoded::Raster(image)
                                }
                                Err(error) => Decoded::Failed(format!("{error:?}")),
                            },
                        };
                        SignalToUI::set_ui_signal();
                        decoded
                    }
                };
                match cx.task_pool().submit(Class::Visible, job) {
                    Ok(task) => loading.push((src, task)),
                    Err(error) => log!("print: picture {src} not read ahead, no worker: {error}"),
                }
            }
            self.pictures = Pictures::Loading(loading);
        }
        if let Pictures::Loading(loading) = &mut self.pictures {
            let mut i = 0;
            while i < loading.len() {
                let Some(result) = loading[i].1.try_take() else {
                    i += 1;
                    continue;
                };
                let (src, task) = loading.swap_remove(i);
                drop(task);
                self.times.picture_count += 1;
                match result {
                    Ok(Decoded::Raster(image)) => {
                        let texture = image.into_new_texture(cx);
                        self.loaded.insert(src, Some(texture));
                    }
                    Ok(Decoded::Svg) => {}
                    Ok(Decoded::Failed(error)) => {
                        log!("print: picture {src} did not load ({error}): its alt text is printed");
                        self.times.pictures_failed += 1;
                        self.loaded.insert(src, None);
                    }
                    Err(error) => {
                        log!("print: picture {src} did not load ({error:?}): its alt text is printed");
                        self.times.pictures_failed += 1;
                        self.loaded.insert(src, None);
                    }
                }
            }
            if loading.is_empty() {
                self.pictures = Pictures::Ready;
                self.times.pictures = Cx::time_now();
                self.render.redraw(cx);
            }
        }
    }

    /// Call with every event the host gets.
    pub(crate) fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> Option<PrintUpdate> {
        if !matches!(self.pictures, Pictures::Ready) {
            self.load_pictures(cx);
            return None;
        }
        let mut update = None;
        match self.render.handle_event(cx, event) {
            Some(Ok(mut page)) => {
                self.times.read_bands += page.read_bands();
                let text = page.text.take();
                if let Err(error) = self.compress(cx, page, text) {
                    return Some(PrintUpdate::Failed(error));
                }
                self.rendered += 1;
                update = Some(PrintUpdate::Progress { done: self.rendered, pages: self.pages.len() });
            }
            Some(Err(error)) => return Some(PrintUpdate::Failed(format!("the pages could not be read back: {error}"))),
            None => {}
        }
        if let Some(done) = self.write(cx) {
            return Some(done);
        }
        self.next_page(cx);
        update
    }

    /// Moves the PDF along: takes the pages compressed (in page order) and
    /// a finished step, hands the writer the pages waiting, and finishes
    /// it after the last.
    fn write(&mut self, cx: &mut Cx) -> Option<PrintUpdate> {
        while let Some(task) = self.compressing.front_mut() {
            let Some(result) = task.try_take() else { break };
            self.compressing.pop_front();
            match result {
                Ok((page, took)) => {
                    self.times.compress += took;
                    self.times.compress_max = self.times.compress_max.max(took);
                    self.times.segments += page.segments.len();
                    self.times.blank_rows += page.left_out;
                    self.compressed.push(page);
                }
                Err(error) => return Some(PrintUpdate::Failed(format!("a page could not be compressed: {error:?}"))),
            }
        }
        match &mut self.writer {
            Writer::Adding(task) => match task.try_take()? {
                Ok((writer, took)) => {
                    self.times.writer += took;
                    self.writer = Writer::Idle(writer);
                }
                Err(error) => return Some(PrintUpdate::Failed(format!("the PDF writer stopped: {error:?}"))),
            },
            Writer::Finishing(task) => {
                let (pdf, text) = match task.try_take()? {
                    Ok(Ok((pdf, text, took))) => {
                        self.times.writer += took;
                        (pdf, text)
                    }
                    Ok(Err(error)) => return Some(PrintUpdate::Failed(error)),
                    Err(error) => return Some(PrintUpdate::Failed(format!("the PDF writer stopped: {error:?}"))),
                };
                self.writer = Writer::Done;
                let now = Cx::time_now();
                let t = &self.times;
                log!(
                    "print: {} pages, {} rows, {} bytes ({} per page, {:.0} ms per page) in {:.0} ms: pictures {} ({} not loaded) {:.0} ms, measure {:.0} ms (frame max {:.1} ms), paginate {:.2} ms, pages {:.0} ms ({} band draws, mean {:.1} ms, max {:.1} ms, {} of {} bands read back), compress {:.0} ms on up to {} workers (page max {:.0} ms, rendering held {} times), writer {:.0} ms, text {} runs, {} glyphs ({} invisible, {} clipped), fonts {} + {} Type 3 built in {:.0} ms ({:.1} KB), images {} ({} blank rows left out); {} links, {} bookmarks, structure {} elements, lines {} tagged, {} artifacts, {} unmarked, {} right-to-left stretches, {} formula rows ({} glyphs as shapes)",
                    self.pages.len(),
                    self.items.len(),
                    pdf.len(),
                    pdf.len() / self.pages.len().max(1),
                    (now - t.started) * 1e3 / self.pages.len().max(1) as f64,
                    (now - t.started) * 1e3,
                    t.picture_count,
                    t.pictures_failed,
                    (t.pictures - t.started) * 1e3,
                    (t.measured - t.pictures) * 1e3,
                    t.measure_max * 1e3,
                    (t.paginated - t.measured) * 1e3,
                    (now - t.paginated) * 1e3,
                    t.bands,
                    t.band_total * 1e3 / t.bands.max(1) as f64,
                    t.band_max * 1e3,
                    t.read_bands,
                    self.pages.len() * self.render.bands_per_page(),
                    t.compress * 1e3,
                    self.ahead,
                    t.compress_max * 1e3,
                    t.held,
                    t.writer * 1e3,
                    text.runs,
                    text.glyphs,
                    text.invisible,
                    text.clipped,
                    text.sfnt_fonts,
                    text.type3_fonts,
                    text.font_time * 1e3,
                    text.font_bytes as f64 / 1024.0,
                    t.segments,
                    t.blank_rows,
                    t.links,
                    self.headings.len(),
                    self.structure.element_count(),
                    text.tagged_lines,
                    text.artifact_lines,
                    text.untagged_lines,
                    text.rtl_stretches,
                    text.pictured_rows,
                    text.shape_glyphs,
                );
                return Some(PrintUpdate::Done(pdf));
            }
            Writer::Idle(_) | Writer::Done => {}
        }
        let Writer::Idle(_) = &self.writer else { return None };
        if !self.compressed.is_empty() {
            let Writer::Idle(mut document) = std::mem::replace(&mut self.writer, Writer::Done) else { return None };
            let pages = std::mem::take(&mut self.compressed);
            let first = self.added;
            self.added += pages.len();
            let links: Vec<Vec<PdfLink>> =
                (first..self.added).map(|page| self.links.get_mut(page).and_then(Option::take).unwrap_or_default()).collect();
            self.times.links += links.iter().map(Vec::len).sum::<usize>();
            let structures: Vec<Option<PageStructure>> =
                (first..self.added).map(|page| self.page_structures.get_mut(page).and_then(Option::take)).collect();
            let format = *self.render.format();
            let job = move || {
                let start = Cx::time_now();
                let (_, rows) = format.pixels();
                let page_w = format.size.x as f32;
                let page_h = format.size.y as f32;
                for ((page, links), structure) in pages.into_iter().zip(links).zip(structures) {
                    let tags = structure.map(|structure| structure.resolve(&mut document.writer, &mut document.ids));
                    let text = page
                        .text
                        .as_ref()
                        .map(|captured| document.text.page_runs(&mut document.writer, captured, tags.as_ref()))
                        .unwrap_or_default();
                    let images = page.segments.into_iter().map(|(image, range)| PageImage {
                        image,
                        rect: PdfRect {
                            x: 0.0, y: range.start as f32 * page_h / rows as f32, w: page_w,
                            h: (range.end - range.start) as f32 * page_h / rows as f32,
                        },
                    }).collect();
                    document.writer.add_page(Page {
                        width: format.size.x as f32,
                        height: format.size.y as f32,
                        images,
                        text,
                        links,
                    });
                }
                SignalToUI::set_ui_signal();
                (document, Cx::time_now() - start)
            };
            match cx.task_pool().submit(Class::Visible, job) {
                Ok(task) => self.writer = Writer::Adding(task),
                Err(error) => return Some(PrintUpdate::Failed(format!("no worker to write the PDF on: {error}"))),
            }
        } else if self.added == self.pages.len() && !self.pages.is_empty() {
            let Writer::Idle(Document { mut writer, text, .. }) = std::mem::replace(&mut self.writer, Writer::Done) else { return None };
            let outline: Vec<OutlineEntry> = self
                .headings
                .iter()
                .filter(|heading| !heading.title.is_empty())
                .map(|heading| OutlineEntry { title: heading.title.clone(), level: heading.level, page: heading.page, y: heading.y })
                .collect();
            let job = move || {
                let start = Cx::time_now();
                let result = text.finish(&mut writer).map(|stats| {
                    writer.set_outline(outline);
                    let pdf: Arc<[u8]> = writer.finish().into();
                    (pdf, stats, Cx::time_now() - start)
                });
                SignalToUI::set_ui_signal();
                result
            };
            match cx.task_pool().submit(Class::Visible, job) {
                Ok(task) => self.writer = Writer::Finishing(task),
                Err(error) => return Some(PrintUpdate::Failed(format!("no worker to write the PDF on: {error}"))),
            }
        }
        None
    }
}

/// The document's language from its front matter's `lang:` line, if it says one.
fn front_matter_lang(lines: &[String]) -> Option<String> {
    if lines.first().map(|line| line.trim_end()) != Some("---") {
        return None;
    }
    lines[1..]
        .iter()
        .take_while(|line| !matches!(line.trim_end(), "---" | "..."))
        .find_map(|line| line.strip_prefix("lang:"))
        .map(|value| value.trim().trim_matches(|c| c == '"' || c == '\'').to_string())
        .filter(|value| !value.is_empty())
}

/// Compresses only spans with ink, keeping short blank gaps inside each image.
fn compress_segments(page: &RenderedPage) -> (Vec<(RgbImage, Range<usize>)>, usize) {
    if page.is_paper() {
        return (Vec::new(), page.height);
    }
    let mut ranges: Vec<Range<usize>> = Vec::new();
    let mut row_index = 0;
    let mut start = None;
    let mut end = 0;
    // An encoder needs its final height up front. Scan band by band for the
    // ranges, then feed those ranges in a second pass without keeping RGB pages.
    page.rgb_bands(|rgb| {
        for row in rgb.chunks_exact(page.width * 3) {
            let mut chunks = row.chunks_exact(8);
            let blank = chunks.by_ref().all(|chunk| u64::from_ne_bytes(chunk.try_into().unwrap()) == u64::MAX)
                && chunks.remainder().iter().all(|&byte| byte == 255);
            if !blank {
                start.get_or_insert(row_index);
                end = row_index + 1;
            } else if start.is_some() && row_index + 1 - end >= 64 {
                ranges.push(start.take().unwrap()..end);
            }
            row_index += 1;
        }
    });
    if let Some(start) = start {
        ranges.push(start..end);
    }
    // Each image keeps a few blank rows around its ink, so its edges lie on paper: a viewer
    // filtering an edge row with what lies past it (or repeating it) then mixes white with white,
    // as it does inside the full page. Gaps are at least 64 rows, so margins never meet.
    const MARGIN: usize = 8;
    for range in &mut ranges {
        range.start = range.start.saturating_sub(MARGIN);
        range.end = (range.end + MARGIN).min(page.height);
    }
    let left_out = page.height - ranges.iter().map(|range| range.end - range.start).sum::<usize>();
    let mut segments = Vec::with_capacity(ranges.len());
    if ranges.is_empty() {
        return (segments, left_out);
    }
    let mut current = 0;
    let mut encoder: Option<RgbImageEncoder> = None;
    row_index = 0;
    page.rgb_bands(|rgb| {
        for row in rgb.chunks_exact(page.width * 3) {
            if let Some(range) = ranges.get(current) {
                if row_index == range.start {
                    encoder = Some(RgbImageEncoder::new(page.width as u32, (range.end - range.start) as u32));
                }
                if let Some(encoder) = &mut encoder {
                    encoder.write(row);
                }
                if row_index + 1 == range.end {
                    segments.push((encoder.take().unwrap().finish(), range.clone()));
                    current += 1;
                }
            }
            row_index += 1;
        }
    });
    (segments, left_out)
}

/// The URL names its scheme (`https:`, `mailto:`), so a reader can follow
/// it; a path relative to the document cannot be followed (`C:` is a drive).
fn has_scheme(url: &str) -> bool {
    url.split_once(':').is_some_and(|(scheme, _)| {
        scheme.len() > 1
            && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    })
}

/// The sources of the pictures in `lines`, as the rows read them (relative
/// ones from `base_dir`), each once; not URLs, which load as the view loads
/// them, nor SVG files, which draw in place.
fn picture_sources(lines: &[String], base_dir: Option<&std::path::Path>) -> Vec<String> {
    let text = lines.join("\n");
    let mut seen = HashSet::new();
    let mut sources = Vec::new();
    for event in Parser::new_ext(&text, parse_options()) {
        let MdEvent::Start(Tag::Image { dest_url, .. }) = event else { continue };
        let src = resolve_source(&dest_url, base_dir).into_owned();
        let url = src.contains("://") && !src.starts_with("file://");
        let svg = src.rsplit('.').next().is_some_and(|ext| ext.eq_ignore_ascii_case("svg"));
        if !src.is_empty() && !url && !svg && seen.insert(src.clone()) {
            sources.push(src);
        }
    }
    sources
}
