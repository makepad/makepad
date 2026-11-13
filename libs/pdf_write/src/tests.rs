use super::*;
use makepad_pdf_parse::{extract_image, PdfDocument, PdfObj};

fn u16_at(data: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([data[at], data[at + 1]])
}

fn i16_at(data: &[u8], at: usize) -> i16 {
    u16_at(data, at) as i16
}

fn u32_at(data: &[u8], at: usize) -> usize {
    u32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]) as usize
}

fn table<'a>(data: &'a [u8], tag: &[u8; 4]) -> &'a [u8] {
    for index in 0..usize::from(u16_at(data, 4)) {
        let record = 12 + index * 16;
        if &data[record..record + 4] == tag {
            let offset = u32_at(data, record + 8);
            return &data[offset..offset + u32_at(data, record + 12)];
        }
    }
    panic!("fixture font must contain table {tag:?}");
}

fn glyph_id(data: &[u8], ch: char) -> u16 {
    let cmap = table(data, b"cmap");
    for index in 0..usize::from(u16_at(cmap, 2)) {
        let record = 4 + index * 8;
        let platform = u16_at(cmap, record);
        let encoding = u16_at(cmap, record + 2);
        let subtable = &cmap[u32_at(cmap, record + 4)..];
        if !(platform == 0 || (platform == 3 && encoding == 1)) || u16_at(subtable, 0) != 4 {
            continue;
        }
        let code = ch as u16;
        let segments = usize::from(u16_at(subtable, 6) / 2);
        for segment in 0..segments {
            let end = u16_at(subtable, 14 + segment * 2);
            let start = u16_at(subtable, 16 + segments * 2 + segment * 2);
            if code < start || code > end {
                continue;
            }
            let delta = u16_at(subtable, 16 + segments * 4 + segment * 2);
            let range_at = 16 + segments * 6 + segment * 2;
            let range = usize::from(u16_at(subtable, range_at));
            if range == 0 {
                return code.wrapping_add(delta);
            }
            let glyph = u16_at(subtable, range_at + range + usize::from(code - start) * 2);
            return if glyph == 0 {
                0
            } else {
                glyph.wrapping_add(delta)
            };
        }
    }
    panic!("fixture font must map {ch:?} through a Unicode format 4 cmap");
}

fn text_run(data: &[u8], font: FontId, mode: TextMode, baseline: f32) -> TextRun {
    let em = f32::from(u16_at(table(data, b"head"), 18));
    let metrics = table(data, b"hmtx");
    let metric_count = usize::from(u16_at(table(data, b"hhea"), 34));
    let mut x = 2.5;
    let mut glyphs = Vec::new();
    for ch in "Hello PDF".chars() {
        let id = glyph_id(data, ch);
        assert_ne!(id, 0, "fixture font must map every sample character");
        let advance = u16_at(metrics, usize::from(id).min(metric_count - 1) * 4);
        glyphs.push(Glyph {
            id,
            x,
            y: 0.0,
            advance,
            text: ch.to_string(),
        });
        x += f32::from(advance) * 12.0 / em;
        if ch == 'H' {
            x += 0.75;
        }
    }
    TextRun {
        font,
        size: 12.0,
        color: [0.2, 0.3, 0.4, 1.0],
        origin: [36.0, baseline],
        clip: None,
        mode,
        glyphs,
    }
}

fn pixels(width: u32, height: u32) -> Vec<u8> {
    let mut rgb = Vec::new();
    for y in 0..height {
        for x in 0..width {
            rgb.extend_from_slice(&[(x * 6) as u8, (y * 8) as u8, 128]);
        }
    }
    rgb
}

/// Compressed in uneven pieces, not at row ends, as a page's bands come.
fn image(width: u32, height: u32, rect: Rect) -> PageImage {
    let rgb = pixels(width, height);
    let mut encoder = RgbImageEncoder::new(width, height);
    for piece in rgb.chunks(rgb.len() / 3 + 7) {
        encoder.write(piece);
    }
    PageImage {
        image: encoder.finish(),
        rect,
    }
}

#[test]
fn round_trip_raster_text_links_and_outline() {
    let data = include_bytes!("../../../widgets/resources/IBMPlexSans-Text.ttf");
    let head = table(data, b"head");
    let hhea = table(data, b"hhea");
    let os2 = table(data, b"OS/2");
    let mut writer = PdfWriter::new(DocInfo {
        title: "PDF \u{1F4C4}".into(),
        producer: "Writer (RGB) \\ text".into(),
    });
    writer.set_outline(vec![
        OutlineEntry {
            title: "Images".into(),
            level: 1,
            page: 0,
            y: 24.0,
        },
        OutlineEntry {
            title: "Text \u{1F4C4}".into(),
            level: 2,
            page: 1,
            y: 72.0,
        },
    ]);
    assert_eq!(
        writer.page_count(),
        0,
        "outline reservations must not add pages"
    );
    writer.add_page(Page {
        width: 595.276,
        height: 841.89,
        images: vec![image(
            40,
            30,
            Rect {
                x: 18.0,
                y: 24.0,
                w: 400.0,
                h: 300.0,
            },
        )],
        text: Vec::new(),
        links: vec![
            Link {
                rect: Rect {
                    x: 18.0,
                    y: 24.0,
                    w: 100.0,
                    h: 20.0,
                },
                target: LinkTarget::Uri("https://example.com/a(b)\\c/é".into()),
            },
            Link {
                rect: Rect {
                    x: 18.0,
                    y: 50.0,
                    w: 100.0,
                    h: 20.0,
                },
                target: LinkTarget::Page { index: 1, y: 72.0 },
            },
        ],
    });
    assert_eq!(
        writer.page_count(),
        1,
        "a forward link must not count its target as an added page"
    );
    let font = writer.add_font(FontMetrics {
        postscript_name: "IBMPlexSans-Text".into(),
        units_per_em: u16_at(head, 18),
        ascent: i16_at(hhea, 4),
        descent: i16_at(hhea, 6),
        cap_height: i16_at(os2, 88),
        bbox: [
            i16_at(head, 36),
            i16_at(head, 38),
            i16_at(head, 40),
            i16_at(head, 42),
        ],
    });
    let fill = text_run(data, font, TextMode::Fill, 72.0);
    let h_id = fill.glyphs[0].id;
    let h_advance = fill.glyphs[0].advance;
    writer.add_page(Page {
        width: 612.0,
        height: 792.0,
        images: vec![image(
            20,
            20,
            Rect {
                x: 36.0,
                y: 24.0,
                w: 120.0,
                h: 120.0,
            },
        )],
        // Put the invisible run first to guard render-mode reset for visible text.
        text: vec![text_run(data, font, TextMode::Invisible, 100.0), fill],
        links: Vec::new(),
    });
    assert_eq!(
        writer.page_count(),
        2,
        "writer must count exactly the two added pages"
    );
    writer.set_font_program(font, data.to_vec());
    let bytes = writer.finish();
    assert!(
        bytes.starts_with(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n"),
        "PDF header must include version and binary marker"
    );
    assert!(
        bytes.ends_with(b"%%EOF\n"),
        "PDF must end with its EOF marker"
    );
    let mut doc = PdfDocument::parse(&bytes).expect("written PDF must parse");
    assert_eq!(
        doc.page_count(),
        2,
        "parsed document must contain two pages"
    );
    let page1 = doc.page(0).expect("raster page must parse");
    let page2 = doc.page(1).expect("text page must parse");
    for (index, page, width, height) in [(0, &page1, 595.276, 841.89), (1, &page2, 612.0, 792.0)] {
        assert!(
            (page.width() - width).abs() < 0.01,
            "page {index} width must survive the round trip"
        );
        assert!(
            (page.height() - height).abs() < 0.01,
            "page {index} height must survive the round trip"
        );
    }
    let xobject = page1
        .xobjects
        .values()
        .find(|object| object.subtype == "Image")
        .expect("raster page must have an image XObject");
    let decoded = extract_image(&mut doc, xobject.obj_ref).expect("RGB image must decode");
    assert_eq!(
        (decoded.width, decoded.height),
        (40, 30),
        "image dimensions must be preserved"
    );
    let rgba: Vec<u8> = pixels(40, 30)
        .chunks_exact(3)
        .flat_map(|rgb| [rgb[0], rgb[1], rgb[2], 255])
        .collect();
    assert_eq!(
        decoded.rgba, rgba,
        "every decoded RGB pixel and opaque alpha must match"
    );
    assert!(
        page1.fonts.is_empty(),
        "raster-only page must not need font resources"
    );
    let parsed_font = page2
        .fonts
        .values()
        .next()
        .expect("text page must have a font resource");
    assert_eq!(
        parsed_font.subtype, "Type0",
        "glyph runs must use a Type0 font"
    );
    let cmap = parsed_font
        .to_unicode
        .as_ref()
        .expect("embedded font must have ToUnicode");
    assert_eq!(
        cmap.mappings.get(&u32::from(h_id)).map(String::as_str),
        Some("H"),
        "ToUnicode must map the H glyph id back to H"
    );
    let width = parsed_font
        .cid_widths
        .as_ref()
        .and_then(|widths| widths.get(&u32::from(h_id)))
        .expect("used glyph must have an explicit CID width");
    assert!(
        (*width - f64::from(h_advance) * 1000.0 / f64::from(u16_at(head, 18))).abs() < 0.001,
        "CID widths must be scaled to 1000/em"
    );
    let content1 = std::str::from_utf8(&page1.content_data).expect("page operators must be ASCII");
    assert!(
        content1.contains("q 400 0 0 300 18 517.89 cm /Im0 Do Q"),
        "image rectangle must flip from top-left to PDF coordinates"
    );
    let content2 = std::str::from_utf8(&page2.content_data).expect("text operators must be ASCII");
    assert!(
        content2.find(" Do Q").expect("page must draw its image")
            < content2.find("BT ").expect("page must draw text"),
        "images must be painted before text"
    );
    assert!(
        content2.contains("3 Tr /F0 12 Tf 1 0 0 1 38.5 692 Tm"),
        "invisible run must use mode 3 and preserve its first glyph offset"
    );
    assert!(
        content2.contains("0 Tr /F0 12 Tf 1 0 0 1 38.5 720 Tm"),
        "visible run must reset mode 0 and flip its baseline"
    );
    assert!(
        content2.contains("-62.5"),
        "TJ adjustment must preserve additional glyph spacing"
    );

    let root = doc
        .trailer()
        .get_ref("Root")
        .expect("trailer must reference the catalog");
    assert_eq!(root.num, 1, "catalog must have object number 1");
    let catalog = doc
        .resolve_ref(root)
        .expect("catalog reference must resolve");
    let catalog = catalog.as_dict().expect("catalog must be a dictionary");
    let pages_ref = catalog
        .get_ref("Pages")
        .expect("catalog must reference Pages");
    assert_eq!(pages_ref.num, 2, "Pages must have object number 2");
    let pages = doc
        .resolve_ref(pages_ref)
        .expect("Pages reference must resolve");
    let kids = pages
        .as_dict()
        .and_then(|dict| dict.get_array("Kids"))
        .expect("Pages must have Kids");
    let first_page = doc
        .resolve(&kids[0])
        .expect("first page reference must resolve");
    let first_page = first_page
        .as_dict()
        .expect("first page must be a dictionary");
    assert_eq!(
        first_page.get_ref("Parent"),
        Some(pages_ref),
        "page must point to the reserved Pages parent"
    );
    let annotations = first_page
        .get_array("Annots")
        .expect("page must contain annotations");
    assert_eq!(annotations.len(), 2, "page must retain both links");
    let uri_link = doc
        .resolve(&annotations[0])
        .expect("URI link reference must resolve");
    let uri_link = uri_link.as_dict().expect("URI link must be a dictionary");
    assert_eq!(
        uri_link
            .get_dict("A")
            .and_then(|action| action.get_str("URI")),
        Some(b"https://example.com/a(b)\\c/%C3%A9".as_slice()),
        "URI must percent-encode UTF-8 and escape literal delimiters"
    );
    let link_rect = uri_link
        .get("Rect")
        .and_then(PdfObj::as_number_array)
        .expect("link must have a numeric rectangle");
    for (actual, expected) in link_rect.iter().zip([18.0, 797.89, 118.0, 817.89]) {
        assert!(
            (actual - expected).abs() < 0.01,
            "annotation rectangle must flip both y edges"
        );
    }
    let page_link = doc
        .resolve(&annotations[1])
        .expect("forward link reference must resolve");
    let dest = page_link
        .as_dict()
        .and_then(|link| link.get_array("Dest"))
        .expect("forward link must have a destination");
    assert_eq!(
        dest[0], kids[1],
        "forward link must resolve to the actual second page object"
    );
    assert_eq!(
        dest[3].as_f64(),
        Some(720.0),
        "forward destination must use the target page height"
    );
    assert_eq!(
        catalog.get_name("PageMode"),
        Some("UseOutlines"),
        "catalog must open the bookmark view"
    );
    let outline_ref = catalog
        .get_ref("Outlines")
        .expect("catalog must reference the outline");
    let outline = doc
        .resolve_ref(outline_ref)
        .expect("outline reference must resolve");
    let outline = outline.as_dict().expect("outline must be a dictionary");
    assert_eq!(
        outline.get_int("Count"),
        Some(2),
        "outline root must count both visible entries"
    );
    let parent_ref = outline
        .get_ref("First")
        .expect("outline must have a first entry");
    let parent = doc
        .resolve_ref(parent_ref)
        .expect("parent bookmark must resolve");
    let parent = parent
        .as_dict()
        .expect("parent bookmark must be a dictionary");
    assert_eq!(
        parent.get_int("Count"),
        Some(1),
        "parent bookmark must count its child"
    );
    assert_eq!(
        parent.get_ref("Parent"),
        Some(outline_ref),
        "top-level bookmark must point to the outline root"
    );
    let child_ref = parent
        .get_ref("First")
        .expect("level 2 bookmark must be a child");
    assert_eq!(
        parent.get_ref("Last"),
        Some(child_ref),
        "only child must be both First and Last"
    );
    let child = doc
        .resolve_ref(child_ref)
        .expect("child bookmark must resolve");
    let child = child
        .as_dict()
        .expect("child bookmark must be a dictionary");
    assert_eq!(
        child.get_ref("Parent"),
        Some(parent_ref),
        "nested bookmark must reference its parent"
    );
    let title: Vec<u8> = std::iter::once(0xFEFFu16)
        .chain("Text \u{1F4C4}".encode_utf16())
        .flat_map(u16::to_be_bytes)
        .collect();
    assert_eq!(
        child.get_str("Title"),
        Some(title.as_slice()),
        "Unicode bookmark title must use UTF-16BE including surrogate pairs"
    );
    let info_ref = doc
        .trailer()
        .get_ref("Info")
        .expect("trailer must reference document info");
    let info = doc
        .resolve_ref(info_ref)
        .expect("document info must resolve");
    assert_eq!(
        info.as_dict().and_then(|dict| dict.get_str("Producer")),
        Some(b"Writer (RGB) \\ text".as_slice()),
        "ASCII metadata escapes must round trip"
    );

    let marker = bytes
        .windows(b"startxref\n".len())
        .rposition(|window| window == b"startxref\n")
        .expect("PDF must contain startxref");
    let tail = std::str::from_utf8(&bytes[marker + b"startxref\n".len()..])
        .expect("xref offset must be ASCII");
    let xref: usize = tail
        .lines()
        .next()
        .expect("startxref must have an offset")
        .parse()
        .expect("xref offset must be decimal");
    let mut lines = std::str::from_utf8(&bytes[xref..])
        .expect("xref and trailer must be ASCII")
        .lines();
    assert_eq!(
        lines.next(),
        Some("xref"),
        "startxref must point exactly at the classic xref table"
    );
    let size = doc
        .trailer()
        .get_int("Size")
        .expect("trailer must declare object count") as usize;
    assert_eq!(
        lines.next(),
        Some(format!("0 {size}").as_str()),
        "xref range must agree with trailer Size"
    );
    assert_eq!(
        lines.next(),
        Some("0000000000 65535 f "),
        "xref object zero must be free"
    );
    for object in 1..size {
        let line = lines
            .next()
            .expect("every reserved object must have an xref entry");
        assert_eq!(
            line.len(),
            19,
            "xref entries must have ten-digit offsets and fixed-width fields"
        );
        let offset: usize = line[..10].parse().expect("object offset must be decimal");
        assert!(
            bytes[offset..].starts_with(format!("{object} 0 obj\n").as_bytes()),
            "xref offset must address its own numbered object"
        );
        let resolved = doc
            .resolve_obj_num(object as u32)
            .expect("every written object must resolve");
        if let PdfObj::Stream(stream) = resolved {
            assert_eq!(
                stream.dict.get_name("Filter"),
                Some("FlateDecode"),
                "every stream must use FlateDecode"
            );
            assert_eq!(
                stream.dict.get_int("Length"),
                Some(stream.data.len() as i64),
                "stream Length must equal compressed byte length"
            );
            doc.decode_stream(&stream)
                .expect("every compressed stream must decode");
        }
    }
}

fn font_metrics(data: &[u8], name: &str) -> FontMetrics {
    let head = table(data, b"head");
    let hhea = table(data, b"hhea");
    FontMetrics {
        postscript_name: name.into(),
        units_per_em: u16_at(head, 18),
        ascent: i16_at(hhea, 4),
        descent: i16_at(hhea, 6),
        cap_height: i16_at(table(data, b"OS/2"), 88),
        bbox: [
            i16_at(head, 36),
            i16_at(head, 38),
            i16_at(head, 40),
            i16_at(head, 42),
        ],
    }
}

fn synthetic_metrics() -> FontMetrics {
    FontMetrics {
        postscript_name: "Synthetic".into(),
        units_per_em: 2048,
        ascent: 900,
        descent: -100,
        cap_height: 800,
        bbox: [-100, -100, 1000, 1000],
    }
}

fn positioned_glyph(id: u16, x: f32, text: &str) -> Glyph {
    Glyph {
        id,
        x,
        y: 0.0,
        advance: 600,
        text: text.into(),
    }
}

fn vector_run(font: FontId, baseline: f32, glyphs: Vec<Glyph>) -> TextRun {
    TextRun {
        font,
        size: 12.0,
        color: [0.0, 0.0, 0.0, 1.0],
        origin: [10.0, baseline],
        clip: None,
        mode: TextMode::Fill,
        glyphs,
    }
}

fn vector_page(text: Vec<TextRun>) -> Page {
    Page {
        width: 200.0,
        height: 200.0,
        images: Vec::new(),
        text,
        links: Vec::new(),
    }
}

fn actual_texts(content: &[u8]) -> Vec<String> {
    let content = std::str::from_utf8(content).expect("generated content must be ASCII");
    content
        .split("/ActualText <")
        .skip(1)
        .map(|suffix| {
            let hex = suffix
                .split('>')
                .next()
                .expect("ActualText must end its hex string");
            assert!(
                hex.starts_with("FEFF"),
                "ActualText must include a UTF-16BE BOM"
            );
            assert_eq!(
                hex.len() % 4,
                0,
                "ActualText must contain complete UTF-16 units"
            );
            let units: Vec<u16> = hex[4..]
                .as_bytes()
                .chunks_exact(4)
                .map(|unit| {
                    u16::from_str_radix(
                        std::str::from_utf8(unit).expect("UTF-16 hex must be ASCII"),
                        16,
                    )
                    .expect("ActualText must contain hexadecimal UTF-16 units")
                })
                .collect();
            String::from_utf16(&units).expect("ActualText must contain valid UTF-16")
        })
        .collect()
}

fn decoded_runs(page: &makepad_pdf_parse::PdfPage) -> Vec<String> {
    use makepad_pdf_parse::{decode_text, parse_content_stream, PdfOp, TextArrayItem};

    // The parser retains BDC's tag only; pair its operations with the raw ActualText values.
    let actual = actual_texts(&page.content_data);
    let mut actual = actual.iter();
    let mut inside_actual = false;
    let mut font = String::new();
    let mut runs: Vec<String> = Vec::new();
    for op in parse_content_stream(&page.content_data).expect("text operators must parse") {
        match op {
            PdfOp::BeginText => runs.push(String::new()),
            PdfOp::SetFont(name, _) => font = name,
            PdfOp::BeginMarkedContentProps(tag) => {
                assert_eq!(tag, "Span", "ActualText must use a Span tag");
                assert!(!inside_actual, "generated ActualText spans must not nest");
                runs.last_mut()
                    .expect("BDC must be inside the run")
                    .push_str(
                        actual
                            .next()
                            .expect("each BDC must have an ActualText value"),
                    );
                inside_actual = true;
            }
            PdfOp::EndMarkedContent => {
                assert!(inside_actual, "EMC must close an ActualText span");
                inside_actual = false;
            }
            PdfOp::ShowTextArray(items) if !inside_actual => {
                for item in items {
                    if let TextArrayItem::Text(bytes) = item {
                        runs.last_mut()
                            .expect("TJ must be inside text")
                            .push_str(&decode_text(
                                page.fonts.get(&font).expect("Tf must name a page font"),
                                &bytes,
                            ));
                    }
                }
            }
            PdfOp::EndText => assert!(!inside_actual, "ActualText must end before ET"),
            _ => {}
        }
    }
    assert!(
        actual.next().is_none(),
        "every ActualText value must have a BDC operator"
    );
    runs
}

fn page_font_objects(doc: &mut PdfDocument<'_>, index: usize) -> makepad_pdf_parse::PdfDict {
    let root = doc.trailer().get_ref("Root").expect("catalog must exist");
    let root = doc.resolve_ref(root).expect("catalog must resolve");
    let pages = root
        .as_dict()
        .and_then(|dict| dict.get_ref("Pages"))
        .expect("catalog must reference pages");
    let pages = doc.resolve_ref(pages).expect("page tree must resolve");
    let page = &pages
        .as_dict()
        .and_then(|dict| dict.get_array("Kids"))
        .expect("page tree must have children")[index];
    let page = doc.resolve(page).expect("page must resolve");
    page.as_dict()
        .and_then(|dict| dict.get_dict("Resources"))
        .and_then(|dict| dict.get_dict("Font"))
        .expect("page must have font resources")
        .clone()
}

#[test]
fn round_trip_vector_clusters_and_type3() {
    use makepad_pdf_parse::{parse_content_stream, PdfOp, TextArrayItem};

    let data = include_bytes!("../../../widgets/resources/NotoSans-Regular.ttf");
    let mut writer = PdfWriter::new(DocInfo {
        title: "Vector text".into(),
        producer: "pdf_write".into(),
    });
    let font = writer.add_font(font_metrics(data, "NotoSans-Regular"));
    let unused = writer.add_font(font_metrics(data, "UnusedFont"));
    let type3 = writer.add_type3_font(synthetic_metrics());
    let split = writer.add_type3_font(synthetic_metrics());
    let unused_type3 = writer.add_type3_font(synthetic_metrics());
    assert!(
        writer.used_glyphs(font).is_empty(),
        "registration must not use any glyphs"
    );

    let ligature = glyph_id(data, '\u{FB01}');
    let base = glyph_id(data, 'e');
    let mark = glyph_id(data, '\u{301}');
    let trailing = glyph_id(data, 'x');
    let rtl_ids = ['D', 'C', 'B', 'A'].map(|ch| glyph_id(data, ch));
    let mut expected_ids = vec![ligature, base, mark, trailing];
    expected_ids.extend(rtl_ids);
    assert!(
        expected_ids.iter().all(|id| *id != 0),
        "fixture must provide the requested glyphs"
    );
    expected_ids.sort_unstable();
    expected_ids.dedup();
    assert_eq!(
        expected_ids.len(),
        8,
        "sample clusters must use distinct glyph ids"
    );

    let mut accented = vector_run(
        font,
        40.0,
        vec![
            positioned_glyph(base, 0.0, "e\u{301}"),
            Glyph {
                y: -3.0,
                advance: 0,
                ..positioned_glyph(mark, 6.0, "")
            },
            positioned_glyph(trailing, 12.0, "x"),
        ],
    );
    accented.color[3] = 0.5;
    accented.clip = Some(Rect {
        x: 5.0,
        y: 30.0,
        w: 80.0,
        h: 20.0,
    });
    // The visual glyphs for the RTL word are supplied rightmost logical letter first.
    let mut rtl = vector_run(
        font,
        60.0,
        rtl_ids
            .iter()
            .enumerate()
            .map(|(index, &id)| {
                positioned_glyph(id, index as f32 * 8.0, if index == 0 { "שלום" } else { "" })
            })
            .collect(),
    );
    rtl.color[3] = 0.5;
    let mut colour = vector_run(
        type3,
        100.0,
        vec![
            positioned_glyph(12, 0.0, "\u{1F600}"),
            positioned_glyph(13, 12.0, "∑"),
            positioned_glyph(14, 24.0, "☀"),
            positioned_glyph(13, 36.0, "S"),
        ],
    );
    colour.color = [1.0, 0.0, 0.0, 0.25];
    let mut zero = vector_run(
        font,
        120.0,
        vec![
            positioned_glyph(ligature, 0.0, "fi"),
            positioned_glyph(ligature, 2.0, "fi"),
        ],
    );
    zero.size = 0.0;
    zero.clip = Some(Rect {
        x: 0.0,
        y: 0.0,
        w: 100.0,
        h: 150.0,
    });
    writer.add_page(vector_page(vec![
        vector_run(font, 20.0, vec![positioned_glyph(ligature, 0.0, "fi")]),
        accented,
        rtl,
        vector_run(font, 80.0, vec![positioned_glyph(ligature, 0.0, "fl")]),
        colour,
        zero,
        vector_run(unused, 140.0, Vec::new()),
    ]));
    assert_eq!(
        writer.used_glyphs(font),
        expected_ids,
        "deferred sfnt usage must be exact, sorted and unique"
    );
    assert_eq!(
        writer.used_glyphs(type3),
        [12, 13, 14],
        "Type 3 usage must deduplicate repeated ids"
    );
    assert!(
        writer.used_glyphs(unused).is_empty(),
        "an empty run must not use its font"
    );
    assert!(
        writer.used_glyphs(unused_type3).is_empty(),
        "an unused Type 3 font must need no procedures"
    );

    let split_text: String = (0..300)
        .map(|index| char::from_u32(0x400 + index).expect("sample must be Unicode"))
        .collect();
    let mut split_glyphs: Vec<Glyph> = split_text
        .chars()
        .enumerate()
        .map(|(index, ch)| {
            positioned_glyph(1299 - index as u16, index as f32 * 0.5, &ch.to_string())
        })
        .collect();
    // Switch back to the first sub-font, then to the second, without changing text objects.
    split_glyphs.push(positioned_glyph(
        1299,
        150.0,
        &split_text
            .chars()
            .next()
            .expect("sample must be nonempty")
            .to_string(),
    ));
    split_glyphs.push(positioned_glyph(
        1000,
        150.5,
        &split_text
            .chars()
            .last()
            .expect("sample must be nonempty")
            .to_string(),
    ));
    writer.add_page(vector_page(vec![vector_run(split, 20.0, split_glyphs)]));
    writer.add_page(vector_page(vec![
        vector_run(split, 20.0, vec![positioned_glyph(1000, 0.0, "last")]),
        vector_run(font, 40.0, vec![positioned_glyph(ligature, 0.0, "fi")]),
    ]));
    assert_eq!(
        writer.used_glyphs(split),
        (1000..1300).collect::<Vec<u16>>(),
        "300 Type 3 ids must remain sorted despite reverse first use"
    );

    // Programs and procedures arrive only after the final page.
    writer.set_font_program(font, data.to_vec());
    let rgba: Vec<u8> = (0..16)
        .flat_map(|index| [index * 11, 80, 200, index * 17])
        .collect();
    writer.set_type3_glyph(
        type3,
        12,
        Type3Glyph::Image {
            rgba: rgba.clone(),
            width_px: 4,
            height_px: 4,
            rect: [-20.0, -40.0, 780.0, 760.0],
        },
    );
    writer.set_type3_glyph(
        type3,
        13,
        Type3Glyph::Outline(vec![
            PathCommand::MoveTo(0.0, 0.0),
            PathCommand::QuadTo(300.0, 600.0, 600.0, 0.0),
            PathCommand::LineTo(600.0, -60.0),
            PathCommand::CurveTo(400.0, -60.0, 200.0, -60.0, 0.0, -60.0),
            PathCommand::Close,
        ]),
    );
    writer.set_type3_glyph(
        type3,
        14,
        Type3Glyph::Image {
            rgba: vec![10, 20, 30, 255],
            width_px: 1,
            height_px: 1,
            rect: [0.0, 0.0, 600.0, 600.0],
        },
    );
    for id in writer.used_glyphs(split) {
        writer.set_type3_glyph(
            split,
            id,
            Type3Glyph::Outline(vec![
                PathCommand::MoveTo(0.0, 0.0),
                PathCommand::LineTo(1.0, 1.0),
                PathCommand::Close,
            ]),
        );
    }
    let bytes = writer.finish();
    let mut doc = PdfDocument::parse(&bytes).expect("vector document must parse");
    let page = doc.page(0).expect("cluster page must parse");
    assert_eq!(
        decoded_runs(&page),
        ["fi", "e\u{301}x", "שלום", "fl", "😀∑☀S", "fifi"],
        "ligatures, marks, RTL, conflicts, emoji and zero-size runs must extract correctly"
    );
    assert_eq!(
        actual_texts(&page.content_data),
        ["e\u{301}", "שלום", "fl", "S"],
        "only multi-glyph clusters and conflicting single glyphs need ActualText"
    );
    assert_eq!(
        page.fonts.len(),
        2,
        "only the two fonts used on this page belong in its resources"
    );
    let cmap = page.fonts["F0"]
        .to_unicode
        .as_ref()
        .expect("sfnt must have ToUnicode");
    assert_eq!(
        cmap.mappings.get(&u32::from(ligature)).map(String::as_str),
        Some("fi"),
        "conflicts must preserve the first ligature mapping"
    );
    assert_eq!(
        cmap.mappings.get(&u32::from(base)).map(String::as_str),
        Some("e\u{301}"),
        "the first glyph in ActualText must retain a fallback mapping"
    );
    assert!(
        !cmap.mappings.contains_key(&u32::from(mark)),
        "continuation glyphs must not gain invented text"
    );
    let type3_cmap = page.fonts["F2_0"]
        .to_unicode
        .as_ref()
        .expect("Type 3 must have ToUnicode");
    assert_eq!(
        type3_cmap.mappings.get(&1).map(String::as_str),
        Some("∑"),
        "Type 3 conflicts must also preserve their first mapping"
    );

    let content = std::str::from_utf8(&page.content_data).expect("content must be ASCII");
    assert!(
        content.contains("q /GS0 gs 5 150 80 20 re W n BT"),
        "alpha and top-left clipping must share q/Q and precede BT"
    );
    assert!(
        content.contains("q 0 50 100 150 re W n BT"),
        "a clip-only run must also save the graphics state"
    );
    assert!(
        content.contains("1 0 0 1 16 163 Tm"),
        "a negative mark offset must move upward at its own pen x"
    );
    assert!(
        content.contains("EMC 1 0 0 1 22 160 Tm"),
        "text after ActualText must resume at its own baseline and x"
    );
    assert_eq!(
        content.matches("/GS0 gs").count(),
        2,
        "equal alphas must reuse their state"
    );
    assert_eq!(
        page.ext_gstate.len(),
        2,
        "only distinct nonopaque alphas need ExtGStates"
    );
    assert_eq!(
        page.ext_gstate["GS0"].ca_lower,
        Some(0.5),
        "half-opacity text must set nonstroking alpha"
    );
    assert_eq!(
        page.ext_gstate["GS1"].ca_lower,
        Some(0.25),
        "a second alpha must have its own state"
    );
    let ops = parse_content_stream(&page.content_data).expect("all page operators must parse");
    let matrices: Vec<_> = ops
        .iter()
        .filter_map(|op| match op {
            PdfOp::SetTextMatrix(matrix) => Some(*matrix),
            _ => None,
        })
        .collect();
    assert_eq!(
        matrices.len(),
        9,
        "equal-y glyphs must stay in one TJ, including RTL and zero-size runs"
    );
    let mut rtl_codes = Vec::new();
    let mut run_index = 0;
    for op in &ops {
        match op {
            PdfOp::BeginText => run_index += 1,
            PdfOp::ShowTextArray(items) if run_index == 3 => {
                for item in items {
                    if let TextArrayItem::Text(code) = item {
                        assert_eq!(code.len(), 2, "sfnt glyphs must use two-byte ids");
                        rtl_codes.push(u16::from_be_bytes([code[0], code[1]]));
                    }
                }
            }
            _ => {}
        }
    }
    assert_eq!(
        rtl_codes, rtl_ids,
        "ActualText must not reorder visual glyphs"
    );
    assert_eq!(
        ops.iter()
            .filter(|op| matches!(op, PdfOp::SaveState))
            .count(),
        4,
        "only alpha or clipped runs should save graphics state"
    );
    assert_eq!(
        ops.iter()
            .filter(|op| matches!(op, PdfOp::RestoreState))
            .count(),
        4,
        "each saved graphics state must be restored"
    );

    let fonts = page_font_objects(&mut doc, 0);
    let type3_object = doc
        .resolve(fonts.get("F2_0").expect("Type 3 resource must exist"))
        .expect("Type 3 font must resolve");
    let type3_dict = type3_object
        .as_dict()
        .expect("Type 3 font must be a dictionary");
    let matrix = type3_dict
        .get("FontMatrix")
        .and_then(PdfObj::as_number_array)
        .expect("Type 3 must have a font matrix");
    assert!(
        (matrix[0] * 2048.0 - 1.0).abs() < 1e-8 && matrix[0] == matrix[3],
        "Type 3 scale must preserve 1/upem rather than rounding it to millipoints"
    );
    assert_eq!(
        type3_dict.get("FontBBox").and_then(PdfObj::as_number_array),
        Some(vec![-100.0, -100.0, 1000.0, 1000.0]),
        "Type 3 FontBBox must remain in font units"
    );
    assert_eq!(
        page.fonts["F2_0"].widths, [600.0; 3],
        "Type 3 widths must use unscaled font advances"
    );
    let procedures = type3_dict
        .get_dict("CharProcs")
        .expect("Type 3 must have CharProcs");
    let outline = doc
        .resolve_stream(procedures.get("g13").expect("outline procedure must exist"))
        .expect("outline must decompress");
    let outline = std::str::from_utf8(&outline).expect("outline must be ASCII");
    assert!(
        outline.starts_with("600 0 0 -60 600 300 d1\n"),
        "outline d1 must bound the curve itself, not its off-curve control points"
    );
    assert!(
        outline.contains("200 400 400 400 600 0 c"),
        "quadratic controls must convert exactly to cubic controls at two thirds"
    );
    assert!(
        outline.ends_with("h\nf\n"),
        "outlines must close and fill using the run colour"
    );
    let image_proc = doc
        .resolve_stream(procedures.get("g12").expect("image procedure must exist"))
        .expect("image procedure must decompress");
    assert_eq!(
        std::str::from_utf8(&image_proc).expect("image procedure must be ASCII"),
        "600 0 d0\nq 800 0 0 800 -20 -40 cm /Im12 Do Q\n",
        "colour procedures must use d0 and the font-space image rectangle"
    );
    let images = type3_dict
        .get_dict("Resources")
        .and_then(|resources| resources.get_dict("XObject"))
        .expect("Type 3 images must be local resources");
    let image = doc
        .resolve(images.get("Im12").expect("emoji must have an image"))
        .expect("emoji image must resolve");
    let image = image.as_stream().expect("emoji must be an image stream");
    assert_eq!(
        image.dict.get_name("ColorSpace"),
        Some("DeviceRGB"),
        "emoji colour must be an RGB XObject"
    );
    assert_eq!(
        (image.dict.get_int("Width"), image.dict.get_int("Height")),
        (Some(4), Some(4)),
        "emoji pixel dimensions must survive"
    );
    assert_eq!(
        doc.decode_stream(image).expect("emoji RGB must decompress"),
        rgba.chunks_exact(4)
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2]])
            .collect::<Vec<_>>(),
        "emoji RGB bytes must survive compression"
    );
    let mask = doc
        .resolve(
            image
                .dict
                .get("SMask")
                .expect("partial alpha must have an SMask"),
        )
        .expect("SMask must resolve");
    let mask = mask.as_stream().expect("SMask must be a stream");
    assert_eq!(
        mask.dict.get_name("ColorSpace"),
        Some("DeviceGray"),
        "SMask must use grayscale"
    );
    assert_eq!(
        doc.decode_stream(mask).expect("SMask must decompress"),
        rgba.chunks_exact(4)
            .map(|pixel| pixel[3])
            .collect::<Vec<_>>(),
        "every emoji alpha byte must survive compression"
    );
    let opaque = doc
        .resolve(images.get("Im14").expect("opaque image must exist"))
        .expect("opaque image must resolve");
    assert!(
        opaque
            .as_dict()
            .expect("opaque image must have a dictionary")
            .get("SMask")
            .is_none(),
        "opaque images must omit redundant masks"
    );

    let split_page = doc.page(1).expect("split page must parse");
    let expected_split = format!(
        "{split_text}{}{}",
        split_text.chars().next().expect("first character"),
        split_text.chars().last().expect("last character")
    );
    assert_eq!(
        decoded_runs(&split_page),
        [expected_split],
        "all 300 Type 3 glyphs and revisits must decode"
    );
    assert_eq!(
        split_page.fonts.len(),
        2,
        "300 glyphs must produce exactly two sub-font resources"
    );
    for (name, count) in [("F3_0", 256), ("F3_1", 44)] {
        let font = &split_page.fonts[name];
        assert_eq!(font.subtype, "Type3", "each split resource must be Type 3");
        assert_eq!(
            (font.first_char, font.last_char),
            (0, count - 1),
            "each sub-font must use a dense byte range"
        );
        assert_eq!(
            font.to_unicode
                .as_ref()
                .expect("each sub-font needs ToUnicode")
                .mappings
                .len(),
            count as usize,
            "each sub-font must map every used code"
        );
    }
    let split_ops =
        parse_content_stream(&split_page.content_data).expect("split operators must parse");
    let selections: Vec<_> = split_ops
        .iter()
        .filter_map(|op| match op {
            PdfOp::SetFont(name, _) => Some(name.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        selections,
        ["F3_0", "F3_1", "F3_0", "F3_1"],
        "Tf must switch both ways inside the run"
    );
    assert_eq!(
        split_ops
            .iter()
            .filter(|op| matches!(op, PdfOp::BeginText))
            .count(),
        1,
        "a sub-font switch must stay inside one text object"
    );
    let mut code_count = 0;
    for op in &split_ops {
        if let PdfOp::ShowTextArray(items) = op {
            for item in items {
                if let TextArrayItem::Text(code) = item {
                    assert_eq!(code.len(), 1, "Type 3 glyph codes must be one byte");
                    if code_count < 300 {
                        assert_eq!(
                            code[0],
                            (code_count % 256) as u8,
                            "Type 3 codes must follow first use, not glyph id order"
                        );
                    }
                    code_count += 1;
                }
            }
        }
    }
    assert_eq!(
        code_count, 302,
        "font switches must not drop or repeat glyphs"
    );
    let last_page = doc.page(2).expect("reuse page must parse");
    assert_eq!(
        decoded_runs(&last_page),
        ["last", "fi"],
        "conflicts and stable mappings must persist across pages"
    );
    assert!(
        !last_page.fonts.contains_key("F3_0") && last_page.fonts.contains_key("F3_1"),
        "a page must only reference the sub-font it uses"
    );

    let mut font_objects = 0;
    let size = doc
        .trailer()
        .get_int("Size")
        .expect("trailer must have Size");
    for id in 1..size {
        let object = doc
            .resolve_obj_num(id as u32)
            .expect("every output object must resolve");
        assert_ne!(
            object,
            PdfObj::Null,
            "unused fonts must not leave even null reservations"
        );
        if object
            .as_dict()
            .is_some_and(|dict| dict.get_name("Type") == Some("Font"))
        {
            font_objects += 1;
        }
        if let PdfObj::Stream(stream) = &object {
            assert_eq!(
                stream.dict.get_name("Filter"),
                Some("FlateDecode"),
                "font, procedure and image streams must all be compressed"
            );
            doc.decode_stream(stream)
                .expect("every output stream must decompress");
        }
    }
    assert_eq!(
        font_objects, 5,
        "only one Type0, its descendant and three Type 3 objects must be written"
    );
}

#[test]
fn missing_font_data_names_the_font() {
    use std::panic::{catch_unwind, AssertUnwindSafe};

    for type3 in [false, true] {
        let name = if type3 {
            "MissingGlyph"
        } else {
            "MissingProgram"
        };
        let mut metrics = synthetic_metrics();
        metrics.postscript_name = name.into();
        let mut writer = PdfWriter::new(DocInfo {
            title: String::new(),
            producer: String::new(),
        });
        let font = if type3 {
            writer.add_type3_font(metrics)
        } else {
            writer.add_font(metrics)
        };
        writer.add_page(vector_page(vec![vector_run(
            font,
            20.0,
            vec![
                positioned_glyph(8, 0.0, "a"),
                positioned_glyph(7, 12.0, "b"),
            ],
        )]));
        if type3 {
            writer.set_type3_glyph(font, 8, Type3Glyph::Outline(Vec::new()));
        }
        let error = catch_unwind(AssertUnwindSafe(|| writer.finish()))
            .expect_err("finish must reject a missing used program or glyph");
        let message = error
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| error.downcast_ref::<&str>().copied())
            .expect("missing font data must report a text message");
        assert!(
            message.contains(name),
            "missing font data must identify its font: {message}"
        );
        if type3 {
            assert!(
                message.contains("glyph 7"),
                "missing Type 3 data must identify its used glyph: {message}"
            );
        }
    }
}
