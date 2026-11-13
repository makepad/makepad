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

use crate::editor::{follows_block, heading_anchor, slug, RowCache};
use crate::rowmap::flat_of;
use makepad_markdown_edit::{
    parse_options,
    pulldown_cmark::{Event as MdEvent, Parser, Tag},
    BlockKind, Index,
};
use makepad_pdf_write::{
    DocInfo, Link as PdfLink, LinkTarget, OutlineEntry, Page, PageImage, PdfWriter, Rect as PdfRect, RgbImage,
    RgbImageEncoder,
};
use makepad_widgets_core::image_cache::{decode_image_from_data, looks_like_svg, ImageBuffer};
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
    Idle(PdfWriter),
    /// A step on a worker hands back what it made and how long it took.
    Adding(TaskHandle<(PdfWriter, f64)>),
    Finishing(TaskHandle<(Arc<[u8]>, f64)>),
    Done,
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
    /// The pages' compression on workers (pixels to RGB, Flate), summed
    /// and the longest, and the times rendering waited for it.
    compress: f64,
    compress_max: f64,
    held: usize,
    /// The writer's time on its worker (adding pages, finishing).
    writer: f64,
    /// The link areas the pages hold.
    links: usize,
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
    /// Pages handed to the renderer, rendered, and added to the PDF.
    started: usize,
    rendered: usize,
    added: usize,
    /// Pages compressing, in page order, how many may at once, and the
    /// compressed ones waiting for the writer.
    compressing: VecDeque<TaskHandle<(RgbImage, f64)>>,
    ahead: usize,
    held: bool,
    compressed: Vec<RgbImage>,
    writer: Writer,
    times: Times,
}

impl PrintJob {
    /// A job over `lines`, whose relative pictures are read from `base_dir`.
    pub(crate) fn new(cx: &mut Cx, title: &str, index: Index, lines: Vec<String>, base_dir: Option<PathBuf>) -> Self {
        let writer = PdfWriter::new(DocInfo { title: title.to_string(), producer: "Makepad".to_string() });
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
            render: PageRender::new(cx, PageFormat::a4()),
            items: Vec::new(),
            tops: Vec::new(),
            pages: Vec::new(),
            headings: Vec::new(),
            links: Vec::new(),
            started: 0,
            rendered: 0,
            added: 0,
            compressing: VecDeque::new(),
            ahead: PAGES_AHEAD.min(cx.task_pool().worker_count()).max(1),
            held: false,
            compressed: Vec::new(),
            writer: Writer::Idle(writer),
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
        let Some(page) = self.render.begin_page(cx) else { return };
        let start = Cx::time_now();
        let record = self.links.get(page).is_some_and(Option::is_none);
        if let (Some(found), Some(slot)) = (self.draw_page(cx, md, page, record), self.links.get_mut(page)) {
            *slot = Some(found);
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

    fn measure(&mut self, cx: &mut Cx2d, md: &mut Markdown, rows: usize) {
        if !self.render.begin_measure(cx) {
            return;
        }
        let content = self.render.format().content();
        let start = Cx::time_now();
        let until = start + MEASURE_SLICE;
        while self.items.len() < rows {
            let row = self.items.len();
            let (height, bands) = self.draw_row(cx, md, row, dvec2(0.0, 0.0), content.size.x, None);
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
    /// of its links drew (a rect per line) and its destination.
    fn draw_row(
        &self,
        cx: &mut Cx2d,
        md: &mut Markdown,
        row_id: usize,
        pos: DVec2,
        width: f64,
        links: Option<&mut Vec<(String, Rect)>>,
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
        (md.text_flow.area().rect(cx).size.y, md.text_flow.break_hints(cx))
    }

    /// Draws page `page`; with `record`, answers the links it holds.
    fn draw_page(&self, cx: &mut Cx2d, md: &mut Markdown, page: usize, record: bool) -> Option<Vec<PdfLink>> {
        let content = self.render.format().content();
        let window = self.pages[page].clone();
        // The page shows its share of the stack, clipped below where its
        // last line ends (the next page starts there).
        let clip = Rect { pos: content.pos, size: dvec2(content.size.x, window.end - window.start) };
        cx.begin_turtle(Walk::abs_rect(clip), Layout::flow_overlay());
        let mut found = record.then(Vec::new);
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
                self.draw_row(cx, md, row, pos, content.size.x, found.as_mut());
            }
        }
        cx.end_turtle();
        found.map(|found| found.into_iter().filter_map(|(url, rect)| self.pdf_link(&url, rect, clip)).collect())
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
    fn compress(&mut self, cx: &mut Cx, page: RenderedPage) -> Result<(), String> {
        let job = move || {
            let start = Cx::time_now();
            let mut encoder = RgbImageEncoder::new(page.width as u32, page.height as u32);
            page.rgb_bands(|rgb| encoder.write(rgb));
            let image = encoder.finish();
            SignalToUI::set_ui_signal();
            (image, Cx::time_now() - start)
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
                                Ok(image) => Decoded::Raster(image),
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
            Some(Ok(page)) => {
                if let Err(error) = self.compress(cx, page) {
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
                Ok((image, took)) => {
                    self.times.compress += took;
                    self.times.compress_max = self.times.compress_max.max(took);
                    self.compressed.push(image);
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
                let pdf = match task.try_take()? {
                    Ok((pdf, took)) => {
                        self.times.writer += took;
                        pdf
                    }
                    Err(error) => return Some(PrintUpdate::Failed(format!("the PDF writer stopped: {error:?}"))),
                };
                self.writer = Writer::Done;
                let now = Cx::time_now();
                let t = &self.times;
                log!(
                    "print: {} pages, {} rows, {} bytes in {:.0} ms: pictures {} ({} not loaded) {:.0} ms, measure {:.0} ms (frame max {:.1} ms), paginate {:.2} ms, pages {:.0} ms ({} band draws, mean {:.1} ms, max {:.1} ms), compress {:.0} ms on up to {} workers (page max {:.0} ms, rendering held {} times), writer {:.0} ms; {} links, {} bookmarks",
                    self.pages.len(),
                    self.items.len(),
                    pdf.len(),
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
                    t.compress * 1e3,
                    self.ahead,
                    t.compress_max * 1e3,
                    t.held,
                    t.writer * 1e3,
                    t.links,
                    self.headings.len(),
                );
                return Some(PrintUpdate::Done(pdf));
            }
            Writer::Idle(_) | Writer::Done => {}
        }
        let Writer::Idle(_) = &self.writer else { return None };
        if !self.compressed.is_empty() {
            let Writer::Idle(mut writer) = std::mem::replace(&mut self.writer, Writer::Done) else { return None };
            let pages = std::mem::take(&mut self.compressed);
            let first = self.added;
            self.added += pages.len();
            let links: Vec<Vec<PdfLink>> =
                (first..self.added).map(|page| self.links.get_mut(page).and_then(Option::take).unwrap_or_default()).collect();
            self.times.links += links.iter().map(Vec::len).sum::<usize>();
            let format = *self.render.format();
            let job = move || {
                let start = Cx::time_now();
                for (image, links) in pages.into_iter().zip(links) {
                    writer.add_page(Page {
                        width: format.size.x as f32,
                        height: format.size.y as f32,
                        images: vec![PageImage {
                            image,
                            rect: PdfRect { x: 0.0, y: 0.0, w: format.size.x as f32, h: format.size.y as f32 },
                        }],
                        text: Vec::new(),
                        links,
                    });
                }
                SignalToUI::set_ui_signal();
                (writer, Cx::time_now() - start)
            };
            match cx.task_pool().submit(Class::Visible, job) {
                Ok(task) => self.writer = Writer::Adding(task),
                Err(error) => return Some(PrintUpdate::Failed(format!("no worker to write the PDF on: {error}"))),
            }
        } else if self.added == self.pages.len() && !self.pages.is_empty() {
            let Writer::Idle(mut writer) = std::mem::replace(&mut self.writer, Writer::Done) else { return None };
            let outline: Vec<OutlineEntry> = self
                .headings
                .iter()
                .filter(|heading| !heading.title.is_empty())
                .map(|heading| OutlineEntry { title: heading.title.clone(), level: heading.level, page: heading.page, y: heading.y })
                .collect();
            let job = move || {
                let start = Cx::time_now();
                writer.set_outline(outline);
                let pdf: Arc<[u8]> = writer.finish().into();
                SignalToUI::set_ui_signal();
                (pdf, Cx::time_now() - start)
            };
            match cx.task_pool().submit(Class::Visible, job) {
                Ok(task) => self.writer = Writer::Finishing(task),
                Err(error) => return Some(PrintUpdate::Failed(format!("no worker to write the PDF on: {error}"))),
            }
        }
        None
    }
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
