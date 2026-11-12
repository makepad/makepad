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
        color: [0.2, 0.3, 0.4],
        origin: [36.0, baseline],
        mode,
        glyphs,
    }
}

fn image(width: u32, height: u32, rect: Rect) -> PageImage {
    let mut rgb = Vec::new();
    for y in 0..height {
        for x in 0..width {
            rgb.extend_from_slice(&[(x * 6) as u8, (y * 8) as u8, 128]);
        }
    }
    PageImage {
        width_px: width,
        height_px: height,
        rgb,
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
    let font = writer.add_font(FontProgram {
        data: data.to_vec(),
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
    let expected = image(
        40,
        30,
        Rect {
            x: 0.0,
            y: 0.0,
            w: 40.0,
            h: 30.0,
        },
    );
    let rgba: Vec<u8> = expected
        .rgb
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
        content2.contains("3 Tr 1 0 0 1 38.5 692 Tm"),
        "invisible run must use mode 3 and preserve its first glyph offset"
    );
    assert!(
        content2.contains("0 Tr 1 0 0 1 38.5 720 Tm"),
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
