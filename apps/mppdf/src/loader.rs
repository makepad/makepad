//! The off-thread document loader.
//!
//! The `PdfView` widget draws lazily already — its `PortalList` only walks
//! the ops of the pages that are actually on screen — but *parsing* is not
//! lazy: `PdfView::load_pdf_data` reads the xref, resolves every page and
//! decompresses every content stream in one synchronous call, on whatever
//! thread it is called from. For a 300-page document that is seconds of
//! frozen UI.
//!
//! So mppdf does not call it. A worker thread owns the file bytes, parses
//! page by page, and streams finished [`CachedPage`]s to the UI over a
//! `ToUISender` (which pokes the UI signal, so the viewer wakes up on
//! `Event::Signal` with no polling). The viewer hands each batch straight
//! to `PdfView::append_pages`. Page one is sent on its own, so it appears as
//! soon as it exists; the batches then grow so the tail of a long document
//! costs a handful of wakeups rather than one per page.
//!
//! Nothing in here parses anything itself: `PdfDocument` and
//! `parse_content_stream` are `makepad-pdf-parse`, exactly what the widget
//! calls — this only moves the calls off the UI thread.

use makepad_pdf_parse::{content::parse_content_stream, document::PdfDocument, PdfPage};
use makepad_widgets::makepad_platform::thread::ToUISender;
use makepad_widgets::CachedPage;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The first send is a single page (show something now), then 2, 4, 8...
const FIRST_BATCH: usize = 1;
const MAX_BATCH: usize = 16;

/// What the worker tells the viewer.
pub enum PdfLoadMsg {
    /// The document opened: page count known, nothing parsed yet.
    Opened { page_count: usize, open_ms: u64 },
    /// The next run of parsed pages, in document order.
    Pages { pages: Vec<CachedPage> },
    /// Every page is parsed.
    Done { total_ms: u64 },
    /// The file could not be read or is not a PDF we understand.
    Failed { message: String },
}

impl std::fmt::Debug for PdfLoadMsg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Opened {
                page_count,
                open_ms,
            } => write!(f, "Opened({page_count} pages, {open_ms}ms)"),
            Self::Pages { pages } => write!(f, "Pages({})", pages.len()),
            Self::Done { total_ms } => write!(f, "Done({total_ms}ms)"),
            Self::Failed { message } => write!(f, "Failed({message})"),
        }
    }
}

/// Read and parse `path` on a worker thread, streaming the result to `tx`.
///
/// Returns immediately. The thread stops early — without finishing the
/// document — as soon as the receiving end is gone, so opening another file
/// does not leave the old parse burning a core.
pub fn spawn_load(path: PathBuf, tx: ToUISender<PdfLoadMsg>) {
    std::thread::spawn(move || {
        let started = Instant::now();

        let data = match std::fs::read(&path) {
            Ok(data) => data,
            Err(error) => {
                let _ = tx.send(PdfLoadMsg::Failed {
                    message: format!("{}: {}", file_name(&path), error),
                });
                return;
            }
        };

        let mut doc = match PdfDocument::parse(&data) {
            Ok(doc) => doc,
            Err(error) => {
                let _ = tx.send(PdfLoadMsg::Failed {
                    message: format!("{}: not a readable PDF ({error:?})", file_name(&path)),
                });
                return;
            }
        };

        let page_count = doc.page_count();
        if tx
            .send(PdfLoadMsg::Opened {
                page_count,
                open_ms: started.elapsed().as_millis() as u64,
            })
            .is_err()
        {
            return;
        }

        let mut batch: Vec<CachedPage> = Vec::new();
        let mut batch_target = FIRST_BATCH;
        for index in 0..page_count {
            batch.push(parse_one(&mut doc, index));
            if batch.len() >= batch_target {
                let pages = std::mem::take(&mut batch);
                if tx.send(PdfLoadMsg::Pages { pages }).is_err() {
                    return;
                }
                batch_target = (batch_target * 2).min(MAX_BATCH);
            }
        }
        if !batch.is_empty() && tx.send(PdfLoadMsg::Pages { pages: batch }).is_err() {
            return;
        }

        let _ = tx.send(PdfLoadMsg::Done {
            total_ms: started.elapsed().as_millis() as u64,
        });
    });
}

/// One page, or an empty sheet of the same nominal size when the page or its
/// content stream is broken — a bad page must not take the document down.
fn parse_one(doc: &mut PdfDocument, index: usize) -> CachedPage {
    match doc.page(index) {
        Ok(page) => {
            let ops = parse_content_stream(&page.content_data).unwrap_or_default();
            CachedPage::new(page, ops)
        }
        Err(_) => CachedPage::new(PdfPage::default(), Vec::new()),
    }
}

/// The last path component, for messages.
pub fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_generated_document_parses_page_by_page() {
        // The same generator examples/pdf uses for its demo document: 25
        // pages of text, tables, charts and shapes.
        let data = makepad_pdf_parse::generate_test_pdf(25);
        let mut doc = PdfDocument::parse(&data).expect("generated PDF parses");
        assert_eq!(doc.page_count(), 25);
        let page = parse_one(&mut doc, 0);
        assert_eq!(page.size().x, 612.0);
        assert_eq!(page.size().y, 792.0);
    }

    #[test]
    fn a_broken_page_still_yields_a_sheet() {
        // A document with a plausible header but no usable body: every page
        // request fails, and the fallback sheet keeps its nominal size.
        let mut doc = match PdfDocument::parse(b"%PDF-1.4\n%%EOF\n") {
            Ok(doc) => doc,
            // Refusing the file outright is also fine — that path reports
            // Failed instead, and there is nothing to fall back to.
            Err(_) => return,
        };
        let page = parse_one(&mut doc, 0);
        assert_eq!(page.size().y, 792.0);
    }

    #[test]
    fn names_come_from_the_last_path_component() {
        assert_eq!(file_name(Path::new("/a/b/paper.pdf")), "paper.pdf");
        assert_eq!(file_name(Path::new("paper.pdf")), "paper.pdf");
    }
}

