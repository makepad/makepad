//! The document's pages for print and PDF export, made from the final view's
//! rows as they read on paper (dark ink on white, no caret, links as text).
//!
//! A job works on a copy of the document taken when it starts, so editing
//! goes on meanwhile. It measures every row once at the page's content width
//! (a slice of each frame), cuts the stack into pages where no line, table
//! row or picture breaks (a heading goes with the text under it), then has
//! each page rendered off screen (`PageRender`), compressed on a worker of
//! its own (pages in parallel) and added to the PDF in order on another.

use crate::editor::{follows_block, RowCache};
use makepad_markdown_edit::{BlockKind, Index};
use makepad_pdf_write::{DocInfo, Page, PageImage, PdfWriter, Rect as PdfRect, RgbImage, RgbImageEncoder};
use makepad_widgets_core::makepad_platform::thread::{Class, SignalToUI, TaskHandle};
use makepad_widgets_core::*;
use makepad_widgets_rich_text::markdown::{Markdown, MarkdownHost};
use std::{collections::VecDeque, ops::Range, sync::Arc};

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

/// The time each stage took, logged when the PDF is made.
#[derive(Default)]
struct Times {
    started: f64,
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
}

pub(crate) struct PrintJob {
    title: String,
    index: Index,
    lines: Vec<String>,
    render: PageRender,
    /// The rows measured so far, with their tops in the stack.
    items: Vec<PageItem>,
    tops: Vec<f64>,
    pages: Vec<Range<f64>>,
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
    pub(crate) fn new(cx: &mut Cx, title: &str, index: Index, lines: Vec<String>) -> Self {
        let writer = PdfWriter::new(DocInfo { title: title.to_string(), producer: "Makepad".to_string() });
        Self {
            title: title.to_string(),
            index,
            lines,
            render: PageRender::new(cx, PageFormat::a4()),
            items: Vec::new(),
            tops: Vec::new(),
            pages: Vec::new(),
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
        let rows = self.index.row_count();
        if self.items.len() < rows || self.times.measured == 0.0 {
            self.measure(cx, md, rows);
            return;
        }
        let Some(page) = self.render.begin_page(cx) else { return };
        let start = Cx::time_now();
        self.draw_page(cx, md, page);
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
            let (height, bands) = self.draw_row(cx, md, row, dvec2(0.0, 0.0), content.size.x);
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
        self.times.paginated = Cx::time_now();
        self.next_page(cx);
    }

    /// Draws row `row` of the copy at `pos`, `width` across: its height and
    /// the bands a page break must not cut.
    fn draw_row(&self, cx: &mut Cx2d, md: &mut Markdown, row_id: usize, pos: DVec2, width: f64) -> (f64, Vec<Range<f64>>) {
        let index = &self.index;
        let row = index.row(row_id);
        let cache = RowCache::build(index, &self.lines, &row);
        let footnote = |label: &str| index.footnote_number(label);
        let mut host = MarkdownHost {
            footnote_number: Some(&footnote),
            list_start: cache.list_start,
            follows_block: follows_block(row_id, &row),
            ..Default::default()
        };
        let walk = Walk { abs_pos: Some(pos), width: Size::Fixed(width), height: Size::fit(), ..Walk::default() };
        md.draw_events(cx, walk, &cache.slice.text, cache.events.iter().cloned(), &mut host);
        (md.text_flow.area().rect(cx).size.y, md.text_flow.break_hints(cx))
    }

    fn draw_page(&self, cx: &mut Cx2d, md: &mut Markdown, page: usize) {
        let content = self.render.format().content();
        let window = self.pages[page].clone();
        // The page shows its share of the stack, clipped below where its
        // last line ends (the next page starts there).
        let clip = Rect { pos: content.pos, size: dvec2(content.size.x, window.end - window.start) };
        cx.begin_turtle(Walk::abs_rect(clip), Layout::flow_overlay());
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
                self.draw_row(cx, md, row, content.pos + dvec2(0.0, top - window.start), content.size.x);
            }
        }
        cx.end_turtle();
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

    /// Call with every event the host gets.
    pub(crate) fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> Option<PrintUpdate> {
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
                    "print: {} pages, {} rows, {} bytes in {:.0} ms: measure {:.0} ms (frame max {:.1} ms), paginate {:.2} ms, pages {:.0} ms ({} band draws, mean {:.1} ms, max {:.1} ms), compress {:.0} ms on up to {} workers (page max {:.0} ms, rendering held {} times), writer {:.0} ms",
                    self.pages.len(),
                    self.items.len(),
                    pdf.len(),
                    (now - t.started) * 1e3,
                    (t.measured - t.started) * 1e3,
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
                );
                return Some(PrintUpdate::Done(pdf));
            }
            Writer::Idle(_) | Writer::Done => {}
        }
        let Writer::Idle(_) = &self.writer else { return None };
        if !self.compressed.is_empty() {
            let Writer::Idle(mut writer) = std::mem::replace(&mut self.writer, Writer::Done) else { return None };
            let pages = std::mem::take(&mut self.compressed);
            self.added += pages.len();
            let format = *self.render.format();
            let job = move || {
                let start = Cx::time_now();
                for image in pages {
                    writer.add_page(Page {
                        width: format.size.x as f32,
                        height: format.size.y as f32,
                        images: vec![PageImage {
                            image,
                            rect: PdfRect { x: 0.0, y: 0.0, w: format.size.x as f32, h: format.size.y as f32 },
                        }],
                        text: Vec::new(),
                        links: Vec::new(),
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
            let Writer::Idle(writer) = std::mem::replace(&mut self.writer, Writer::Done) else { return None };
            let job = move || {
                let start = Cx::time_now();
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
