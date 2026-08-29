//! Dump a PDF's fonts and first text ops — the diagnosis instrument for
//! text-positioning bugs. `cargo run -p makepad-pdf-parse --example pdfdump
//! --release -- <file.pdf> [page] [max_ops]`.

use makepad_pdf_parse::content::parse_content_stream;
use makepad_pdf_parse::document::PdfDocument;
use makepad_pdf_parse::PdfOp;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: pdfdump <file.pdf> [page] [max]");
    let page_no: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let max_ops: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(160);

    let data = std::fs::read(&path).expect("read pdf");
    let mut doc = PdfDocument::parse(&data).expect("parse pdf");
    println!("pages: {}", doc.page_count());
    let page = doc.page(page_no).expect("page");
    println!(
        "page {}: media {:?} crop {:?} rotate {} content {} bytes",
        page_no,
        page.media_box,
        page.crop_box,
        page.rotate,
        page.content_data.len()
    );
    for (name, f) in &page.fonts {
        println!(
            "font {}: subtype={} base={} enc={:?} first={} last={} widths={} dw={} cidw={:?} tounicode={}",
            name,
            f.subtype,
            f.base_font,
            f.encoding,
            f.first_char,
            f.last_char,
            f.widths.len(),
            f.default_width,
            f.cid_widths.as_ref().map(|w| w.len()),
            f.to_unicode.as_ref().map(|t| t.mappings.len()).unwrap_or(0)
        );
    }
    let ops = parse_content_stream(&page.content_data).expect("content");
    println!("ops: {}", ops.len());
    let mut shown = 0usize;
    for op in &ops {
        let line = match op {
            PdfOp::BeginText => "BT".to_string(),
            PdfOp::EndText => "ET".to_string(),
            PdfOp::SetFont(n, s) => format!("Tf /{} {}", n, s),
            PdfOp::SetTextMatrix(m) => format!("Tm {:?}", m),
            PdfOp::MoveText(x, y) => format!("Td {} {}", x, y),
            PdfOp::MoveTextSetLeading(x, y) => format!("TD {} {}", x, y),
            PdfOp::NextLine => "T*".to_string(),
            PdfOp::SetTextLeading(v) => format!("TL {}", v),
            PdfOp::ConcatMatrix(m) => format!("cm {:?}", m),
            PdfOp::SaveState => "q".to_string(),
            PdfOp::RestoreState => "Q".to_string(),
            PdfOp::ShowText(b) => format!("Tj {:?}", String::from_utf8_lossy(b)),
            PdfOp::ShowTextArray(items) => {
                let mut s = String::from("TJ [");
                for it in items {
                    match it {
                        makepad_pdf_parse::TextArrayItem::Text(b) => {
                            s.push_str(&format!("{:?} ", String::from_utf8_lossy(b)))
                        }
                        makepad_pdf_parse::TextArrayItem::Adjustment(a) => {
                            s.push_str(&format!("{} ", a))
                        }
                    }
                }
                s.push(']');
                s
            }
            _ => continue,
        };
        println!("  {}", line);
        shown += 1;
        if shown >= max_ops {
            println!("  … (truncated)");
            break;
        }
    }
}
