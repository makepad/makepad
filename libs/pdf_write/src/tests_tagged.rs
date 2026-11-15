use super::*;
use crate::tests::{font_metrics, glyph_id, positioned_glyph, vector_run};
use makepad_pdf_parse::structure::{StructElement, StructKid, StructTree};
use makepad_pdf_parse::{PdfDocument, PdfObj};

fn children<'a>(element: &'a StructElement, kinds: &[&str]) -> Vec<&'a StructElement> {
    assert_eq!(element.kids.len(), kinds.len(), "{} child count", element.kind);
    element.kids.iter().zip(kinds)
        .map(|(kid, kind)| {
            let StructKid::Element(child) = kid else {
                panic!("{} must have only element children", element.kind);
            };
            assert_eq!(child.kind.as_str(), *kind, "{} child kind and order", element.kind);
            assert_eq!(child.standard.as_str(), *kind, "{} child standard type", element.kind);
            child
        })
        .collect()
}

fn leaf_text(element: &StructElement, expected: &str) {
    assert!(
        element.kids.iter().all(|kid| matches!(kid, StructKid::Content { .. })),
        "{} leaf must contain only content items", element.kind,
    );
    assert_eq!(element.text(), expected, "{} leaf text", element.kind);
}

fn name_attribute(element: &StructElement, owner: &str, key: &str, expected: &str) {
    assert_eq!(
        element.attribute(owner, key).and_then(PdfObj::as_name), Some(expected),
        "{} {owner} {key} attribute", element.kind,
    );
}

fn red_square(rect: Rect) -> PageImage {
    let mut encoder = RgbImageEncoder::new(1, 1);
    encoder.write(&[255, 0, 0]);
    PageImage { image: encoder.finish(), rect }
}

#[test]
fn round_trip_tagged_structure() {
    let data = include_bytes!("../../../widgets/resources/IBMPlexSans-Text.ttf");
    let mut writer = PdfWriter::new(DocInfo {
        title: "Tagged structure".into(),
        producer: "PDF writer".into(),
    });
    writer.set_tagged(Tagging {
        lang: Some("en-US".into()),
        direction: WritingMode::LrTb,
    });
    let metrics = font_metrics(data, "IBMPlexSans-Text");
    let step = 600.0 * 12.0 / f32::from(metrics.units_per_em);
    let font = writer.add_font(metrics);
    let run = |text: &str, tag, baseline| {
        let glyphs = text.chars().enumerate().map(|(index, ch)| {
            let id = glyph_id(data, ch);
            assert_ne!(id, 0, "tagged fixture font must map {ch:?}");
            positioned_glyph(id, index as f32 * step, &ch.to_string())
        }).collect();
        let mut run = vector_run(font, baseline, glyphs);
        run.tag = tag;
        run
    };

    let document = writer.add_struct(None, StructKind::Document, StructAttrs::default());
    let title = writer.add_struct(Some(document), StructKind::H1, StructAttrs::default());
    let paragraph = writer.add_struct(Some(document), StructKind::P, StructAttrs {
        writing_mode: Some(WritingMode::LrTb),
        ..StructAttrs::default()
    });
    // Adjacent runs with one owner must share a single marked-content sequence.
    let first = run("First ", RunTag::Element(paragraph), 40.0);
    let mut half = run("half", RunTag::Element(paragraph), 40.0);
    half.origin[0] += 6.0 * step;
    writer.add_page(Page {
        width: 200.0,
        height: 200.0,
        images: vec![red_square(Rect { x: 150.0, y: 10.0, w: 10.0, h: 10.0 })],
        text: vec![
            run("Title", RunTag::Element(title), 20.0),
            first,
            half,
            run("1 / 2", RunTag::Artifact(ArtifactKind::Footer), 190.0),
        ],
        links: Vec::new(),
    });

    // Elements may also be added after an earlier page was written.
    let list = writer.add_struct(Some(document), StructKind::L, StructAttrs {
        list_numbering: Some(ListNumbering::Disc),
        ..StructAttrs::default()
    });
    let item = writer.add_struct(Some(list), StructKind::LI, StructAttrs::default());
    let label = writer.add_struct(Some(item), StructKind::Lbl, StructAttrs::default());
    let body = writer.add_struct(Some(item), StructKind::LBody, StructAttrs::default());
    let item_text = writer.add_struct(Some(body), StructKind::P, StructAttrs::default());
    let table = writer.add_struct(Some(document), StructKind::Table, StructAttrs::default());
    let head = writer.add_struct(Some(table), StructKind::THead, StructAttrs::default());
    let header_row = writer.add_struct(Some(head), StructKind::TR, StructAttrs::default());
    let header_a = writer.add_struct(Some(header_row), StructKind::TH, StructAttrs {
        scope_column: true,
        ..StructAttrs::default()
    });
    let header_b = writer.add_struct(Some(header_row), StructKind::TH, StructAttrs {
        scope_column: true,
        ..StructAttrs::default()
    });
    let table_body = writer.add_struct(Some(table), StructKind::TBody, StructAttrs::default());
    let row = writer.add_struct(Some(table_body), StructKind::TR, StructAttrs::default());
    let cell_a = writer.add_struct(Some(row), StructKind::TD, StructAttrs::default());
    let cell_b = writer.add_struct(Some(row), StructKind::TD, StructAttrs::default());
    let code = writer.add_struct(Some(document), StructKind::Code, StructAttrs {
        block: true,
        ..StructAttrs::default()
    });
    let reversed = writer.add_struct(Some(document), StructKind::P, StructAttrs::default());
    let rect = Rect { x: 120.0, y: 120.0, w: 20.0, h: 20.0 };
    writer.add_struct(Some(document), StructKind::Figure, StructAttrs {
        bbox: Some((1, rect)),
        alt: Some("A red square".into()),
        ..StructAttrs::default()
    });
    let mut code_run = run("fi", RunTag::Element(code), 110.0);
    code_run.glyphs[0].text = "fi".into();
    code_run.glyphs[1].text.clear();
    let mut item_run = run("Item", RunTag::Element(item_text), 40.0);
    item_run.origin[0] = 22.0;
    let mut header_b_run = run("B", RunTag::Element(header_b), 70.0);
    header_b_run.origin[0] = 60.0;
    let mut cell_b_run = run("2", RunTag::Element(cell_b), 90.0);
    cell_b_run.origin[0] = 60.0;
    // Two runs in visual order ("cb", "a") share one replacement in reading order.
    let mut reversed_first = run("cb", RunTag::Element(reversed), 130.0);
    reversed_first.replacement = Replacement::Start("abc".into());
    let mut reversed_second = run("a", RunTag::Element(reversed), 130.0);
    reversed_second.origin[0] = 40.0;
    reversed_second.replacement = Replacement::Continue;
    writer.add_page(Page {
        width: 200.0,
        height: 200.0,
        images: vec![red_square(rect)],
        text: vec![
            run("second half", RunTag::Element(paragraph), 20.0),
            run("•", RunTag::Element(label), 40.0),
            item_run,
            run("A", RunTag::Element(header_a), 70.0),
            header_b_run,
            run("1", RunTag::Element(cell_a), 90.0),
            cell_b_run,
            code_run,
            reversed_first,
            reversed_second,
            run("2 / 2", RunTag::Artifact(ArtifactKind::Footer), 190.0),
        ],
        links: Vec::new(),
    });
    writer.set_font_program(font, data.to_vec());
    let bytes = writer.finish();
    let mut doc = PdfDocument::parse(&bytes).expect("tagged PDF must parse");
    assert_eq!(doc.page_count(), 2, "tagged document page count");
    let tree = StructTree::read(&mut doc)
        .expect("tagged structure must parse")
        .expect("tagged document must have a structure tree");
    assert!(tree.marked, "catalog must mark the document tagged");
    assert_eq!(tree.lang.as_deref(), Some("en-US"), "catalog language");
    assert_eq!(tree.direction.as_deref(), Some("L2R"), "catalog reading direction");
    assert!(tree.problems.is_empty(), "structure problems: {:?}", tree.problems);
    assert!(tree.notes.is_empty(), "structure notes: {:?}", tree.notes);
    assert_eq!(tree.roots.len(), 1, "one top-level document element");
    let document = &tree.roots[0];
    assert_eq!(document.kind, "Document", "root structure kind");
    assert_eq!(document.standard, "Document", "root standard structure type");
    let sections = children(document, &["H1", "P", "L", "Table", "Code", "P", "Figure"]);
    leaf_text(sections[0], "Title");
    let paragraph = sections[1];
    leaf_text(paragraph, "First halfsecond half");
    assert_eq!(paragraph.page, Some(0), "paragraph default page is its first content page");
    assert_eq!(paragraph.kids.len(), 2, "paragraph has one content item per page");
    for (kid, (page, mcid, expected)) in paragraph.kids.iter().zip([
        (0, 1, "First half"), (1, 0, "second half"),
    ]) {
        let StructKid::Content { page: actual_page, mcid: actual_mcid, text } = kid else {
            panic!("paragraph kids must be content items");
        };
        assert_eq!(*actual_page, page, "paragraph content page order");
        assert_eq!(*actual_mcid, mcid, "paragraph MCID stream order and grouping");
        assert_eq!(text, expected, "paragraph content text on page {page}");
    }
    name_attribute(paragraph, "Layout", "WritingMode", "LrTb");

    let list = sections[2];
    name_attribute(list, "List", "ListNumbering", "Disc");
    let items = children(list, &["LI"]);
    let parts = children(items[0], &["Lbl", "LBody"]);
    leaf_text(parts[0], "•");
    let body = children(parts[1], &["P"]);
    leaf_text(body[0], "Item");

    let table = children(sections[3], &["THead", "TBody"]);
    let header_rows = children(table[0], &["TR"]);
    let headers = children(header_rows[0], &["TH", "TH"]);
    for (header, expected) in headers.iter().zip(["A", "B"]) {
        leaf_text(header, expected);
        name_attribute(header, "Table", "Scope", "Column");
    }
    let rows = children(table[1], &["TR"]);
    let cells = children(rows[0], &["TD", "TD"]);
    leaf_text(cells[0], "1");
    leaf_text(cells[1], "2");
    leaf_text(sections[4], "fi");
    name_attribute(sections[4], "Layout", "Placement", "Block");
    leaf_text(sections[5], "abc");
    assert_eq!(sections[5].kids.len(), 1, "runs sharing a replacement stay in one marked-content sequence");
    let figure = sections[6];
    leaf_text(figure, "");
    assert_eq!(figure.alt.as_deref(), Some("A red square"), "figure alternate description");
    assert_eq!(figure.page, Some(1), "empty figure takes its bounding box page");
    assert!(figure.kids.is_empty(), "figure has no content kids");
    assert_eq!(
        figure.attribute("Layout", "BBox").and_then(PdfObj::as_number_array),
        Some(vec![
            f64::from(rect.x),
            200.0 - f64::from(rect.y) - f64::from(rect.h),
            f64::from(rect.x) + f64::from(rect.w),
            200.0 - f64::from(rect.y),
        ]),
        "figure bounding box converts top-left points to PDF user space",
    );
    assert_eq!(tree.artifacts.len(), 2, "artifact collection has one entry per page");
    for (page, footer) in ["1 / 2", "2 / 2"].into_iter().enumerate() {
        assert!(tree.artifacts[page].iter().any(|text| text == footer), "page {page} footer artifact");
    }
    assert_eq!(tree.untagged.len(), 2, "untagged collection has one entry per page");
    for (page, text) in tree.untagged.iter().enumerate() {
        assert!(text.is_empty(), "page {page} must have no untagged text: {text:?}");
    }
}
