//! Integration tests against real-world PDFs from the stillhq.com-pdfdb corpus.
//! Run `bash tests/download_test_pdfs.sh` first to fetch the corpus.

use makepad_pdf_parse::*;
use std::path::Path;

fn pdfdb_dir() -> Option<std::path::PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pdfdb");
    if dir.is_dir() {
        Some(dir)
    } else {
        None
    }
}

fn collect_pdfs(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut pdfs = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pdfs.extend(collect_pdfs(&path));
            } else if path.extension().and_then(|e| e.to_str()) == Some("pdf") {
                pdfs.push(path);
            }
        }
    }
    pdfs.sort();
    pdfs
}

/// Try to parse every PDF in the corpus. Track how many succeed.
/// We don't require 100% because the corpus has intentionally broken files,
/// but a high success rate validates the parser against real-world content.
#[test]
fn test_parse_real_pdfs() {
    let dir = match pdfdb_dir() {
        Some(d) => d,
        None => {
            eprintln!("Skipping real PDF test — run tests/download_test_pdfs.sh first");
            return;
        }
    };

    let pdfs = collect_pdfs(&dir);
    if pdfs.is_empty() {
        eprintln!("No PDF files found in {:?}", dir);
        return;
    }

    let mut parsed = 0;
    let mut pages_ok = 0;
    let mut content_ok = 0;
    let mut failed = Vec::new();

    for path in &pdfs {
        let data = match std::fs::read(path) {
            Ok(d) => d,
            Err(_) => continue,
        };

        match PdfDocument::parse(&data) {
            Ok(mut doc) => {
                parsed += 1;
                let pc = doc.page_count();

                // Try to parse first page content
                if pc > 0 {
                    match doc.page(0) {
                        Ok(page) => {
                            pages_ok += 1;
                            if !page.content_data.is_empty() {
                                match parse_content_stream(&page.content_data) {
                                    Ok(ops) => {
                                        if !ops.is_empty() {
                                            content_ok += 1;
                                        }
                                    }
                                    Err(_) => {}
                                }
                            }
                        }
                        Err(_) => {}
                    }
                }
            }
            Err(e) => {
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                failed.push((name, e.msg));
            }
        }
    }

    let total = pdfs.len();
    let parse_rate = parsed as f64 / total as f64 * 100.0;

    eprintln!("\n=== Real PDF corpus results ===");
    eprintln!("Total files:     {}", total);
    eprintln!("Parsed OK:       {} ({:.1}%)", parsed, parse_rate);
    eprintln!("Page 0 OK:       {}", pages_ok);
    eprintln!("Content ops OK:  {}", content_ok);
    eprintln!("Parse failures:  {}", failed.len());

    if !failed.is_empty() {
        eprintln!("\nFirst 10 failures:");
        for (name, msg) in failed.iter().take(10) {
            eprintln!("  {} — {}", name, msg);
        }
    }

    // We expect at least 50% parse rate on real-world PDFs
    assert!(
        parse_rate > 50.0,
        "parse rate too low: {:.1}% ({}/{})",
        parse_rate,
        parsed,
        total
    );
}

/// Parse all pages of a subset of real PDFs to stress-test page iteration.
#[test]
fn test_parse_all_pages_real() {
    let dir = match pdfdb_dir() {
        Some(d) => d,
        None => return,
    };

    let pdfs = collect_pdfs(&dir);
    let mut total_pages = 0;
    let mut total_ops = 0;

    // Test first 50 parseable PDFs
    let mut count = 0;
    for path in &pdfs {
        if count >= 50 {
            break;
        }
        let data = match std::fs::read(path) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let mut doc = match PdfDocument::parse(&data) {
            Ok(d) => d,
            Err(_) => continue,
        };
        count += 1;

        for i in 0..doc.page_count() {
            if let Ok(page) = doc.page(i) {
                total_pages += 1;
                if let Ok(ops) = parse_content_stream(&page.content_data) {
                    total_ops += ops.len();
                }
            }
        }
    }

    eprintln!("\n=== All-pages stress test ===");
    eprintln!("PDFs tested:  {}", count);
    eprintln!("Total pages:  {}", total_pages);
    eprintln!("Total ops:    {}", total_ops);
}
