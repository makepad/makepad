use crate::content::*;
use crate::filter;
use crate::lexer::*;
use crate::object::*;
use crate::*;

// ============================================================
// Lexer tests
// ============================================================

#[test]
fn test_lex_integer() {
    let mut lex = Lexer::new(b"42", 0);
    let obj = lex.read_object().unwrap();
    assert!(matches!(obj, PdfObj::Int(42)));
}

#[test]
fn test_lex_negative_integer() {
    let mut lex = Lexer::new(b"-7", 0);
    let obj = lex.read_object().unwrap();
    assert!(matches!(obj, PdfObj::Int(-7)));
}

#[test]
fn test_lex_real() {
    let mut lex = Lexer::new(b"3.14", 0);
    let obj = lex.read_object().unwrap();
    match obj {
        PdfObj::Real(v) => assert!((v - 3.14).abs() < 0.001),
        _ => panic!("expected Real"),
    }
}

#[test]
fn test_lex_name() {
    let mut lex = Lexer::new(b"/Helvetica", 0);
    let obj = lex.read_object().unwrap();
    assert!(matches!(obj, PdfObj::Name(ref n) if n == "Helvetica"));
}

#[test]
fn test_lex_name_with_hex_escape() {
    let mut lex = Lexer::new(b"/A#20B", 0);
    let obj = lex.read_object().unwrap();
    assert!(matches!(obj, PdfObj::Name(ref n) if n == "A B"));
}

#[test]
fn test_lex_literal_string() {
    let mut lex = Lexer::new(b"(Hello World)", 0);
    let obj = lex.read_object().unwrap();
    match obj {
        PdfObj::Str(s) => assert_eq!(s, b"Hello World"),
        _ => panic!("expected Str"),
    }
}

#[test]
fn test_lex_literal_string_nested_parens() {
    let mut lex = Lexer::new(b"(Hello (nested) World)", 0);
    let obj = lex.read_object().unwrap();
    match obj {
        PdfObj::Str(s) => assert_eq!(s, b"Hello (nested) World"),
        _ => panic!("expected Str"),
    }
}

#[test]
fn test_lex_literal_string_escapes() {
    let mut lex = Lexer::new(b"(line1\\nline2\\\\end)", 0);
    let obj = lex.read_object().unwrap();
    match obj {
        PdfObj::Str(s) => assert_eq!(s, b"line1\nline2\\end"),
        _ => panic!("expected Str"),
    }
}

#[test]
fn test_lex_hex_string() {
    let mut lex = Lexer::new(b"<48656C6C6F>", 0);
    let obj = lex.read_object().unwrap();
    match obj {
        PdfObj::Str(s) => assert_eq!(s, b"Hello"),
        _ => panic!("expected Str"),
    }
}

#[test]
fn test_lex_hex_string_odd_digits() {
    let mut lex = Lexer::new(b"<ABC>", 0);
    let obj = lex.read_object().unwrap();
    match obj {
        PdfObj::Str(s) => assert_eq!(s, vec![0xAB, 0xC0]),
        _ => panic!("expected Str"),
    }
}

#[test]
fn test_lex_bool_true() {
    let mut lex = Lexer::new(b"true", 0);
    let obj = lex.read_object().unwrap();
    assert!(matches!(obj, PdfObj::Bool(true)));
}

#[test]
fn test_lex_bool_false() {
    let mut lex = Lexer::new(b"false", 0);
    let obj = lex.read_object().unwrap();
    assert!(matches!(obj, PdfObj::Bool(false)));
}

#[test]
fn test_lex_null() {
    let mut lex = Lexer::new(b"null", 0);
    let obj = lex.read_object().unwrap();
    assert!(matches!(obj, PdfObj::Null));
}

#[test]
fn test_lex_array() {
    let mut lex = Lexer::new(b"[1 2.5 /Name (text)]", 0);
    let obj = lex.read_object().unwrap();
    match obj {
        PdfObj::Array(arr) => {
            assert_eq!(arr.len(), 4);
            assert!(matches!(arr[0], PdfObj::Int(1)));
            assert!(matches!(arr[2], PdfObj::Name(ref n) if n == "Name"));
        }
        _ => panic!("expected Array"),
    }
}

#[test]
fn test_lex_dict() {
    let mut lex = Lexer::new(b"<< /Type /Page /Count 3 >>", 0);
    let obj = lex.read_object().unwrap();
    match obj {
        PdfObj::Dict(d) => {
            assert_eq!(d.get_name("Type"), Some("Page"));
            assert_eq!(d.get_int("Count"), Some(3));
        }
        _ => panic!("expected Dict"),
    }
}

#[test]
fn test_lex_indirect_ref() {
    let mut lex = Lexer::new(b"10 0 R", 0);
    let obj = lex.read_object().unwrap();
    match obj {
        PdfObj::Ref(r) => {
            assert_eq!(r.num, 10);
            assert_eq!(r.gen, 0);
        }
        _ => panic!("expected Ref"),
    }
}

#[test]
fn test_lex_skip_comments() {
    let mut lex = Lexer::new(b"% this is a comment\n42", 0);
    let obj = lex.read_object().unwrap();
    assert!(matches!(obj, PdfObj::Int(42)));
}

// ============================================================
// Filter tests
// ============================================================

#[test]
fn test_ascii_hex_decode() {
    let data = b"48656C6C6F>";
    let decoded = filter::decode_ascii_hex(data).unwrap();
    assert_eq!(decoded, b"Hello");
}

#[test]
fn test_ascii_hex_decode_whitespace() {
    let data = b"48 65 6C 6C 6F>";
    let decoded = filter::decode_ascii_hex(data).unwrap();
    assert_eq!(decoded, b"Hello");
}

#[test]
fn test_ascii85_decode() {
    // "Hello world!" encoded in ASCII85
    let data = b"87cURD]j7BEbo80~>";
    let decoded = filter::decode_ascii85(data).unwrap();
    // The encoded string "87cURD]j7BEbo80" decodes to "Hello world!"
    assert_eq!(std::str::from_utf8(&decoded).unwrap(), "Hello world!");
}

#[test]
fn test_ascii85_decode_z() {
    // 'z' represents four zero bytes
    let data = b"z~>";
    let decoded = filter::decode_ascii85(data).unwrap();
    assert_eq!(decoded, vec![0, 0, 0, 0]);
}

#[test]
fn test_flate_decode_via_integration() {
    // FlateDecode is thoroughly tested via the integration tests
    // that parse generated PDFs with compressed content streams.
    // Here we just verify the error path.
    let bad_data = b"not valid zlib";
    let dict = crate::object::PdfDict::new();
    let mut dict_with_filter = dict;
    dict_with_filter.map.insert(
        "Filter".to_string(),
        PdfObj::Name("FlateDecode".to_string()),
    );
    assert!(filter::decode_stream(bad_data, &dict_with_filter).is_err());
}

// ============================================================
// Content stream parsing tests
// ============================================================

#[test]
fn test_parse_simple_content_stream() {
    let stream = b"BT /F1 12 Tf 72 720 Td (Hello World) Tj ET";
    let ops = parse_content_stream(stream).unwrap();

    assert!(matches!(ops[0], PdfOp::BeginText));
    assert!(
        matches!(ops[1], PdfOp::SetFont(ref name, size) if name == "F1" && (size - 12.0).abs() < 0.01)
    );
    assert!(
        matches!(ops[2], PdfOp::MoveText(x, y) if (x - 72.0).abs() < 0.01 && (y - 720.0).abs() < 0.01)
    );
    assert!(matches!(ops[3], PdfOp::ShowText(ref s) if s == b"Hello World"));
    assert!(matches!(ops[4], PdfOp::EndText));
}

#[test]
fn test_parse_graphics_ops() {
    let stream = b"q 1 0 0 rg 72 100 80 40 re f Q";
    let ops = parse_content_stream(stream).unwrap();

    assert!(matches!(ops[0], PdfOp::SaveState));
    assert!(
        matches!(ops[1], PdfOp::SetFillRgb(r, g, b) if (r - 1.0).abs() < 0.01 && g.abs() < 0.01 && b.abs() < 0.01)
    );
    assert!(matches!(ops[2], PdfOp::Rectangle(x, y, w, h) if (x - 72.0).abs() < 0.01));
    assert!(matches!(ops[3], PdfOp::Fill));
    assert!(matches!(ops[4], PdfOp::RestoreState));
}

#[test]
fn test_parse_path_ops() {
    let stream = b"0 0 m 100 0 l 100 100 l 0 100 l h f";
    let ops = parse_content_stream(stream).unwrap();

    assert!(matches!(ops[0], PdfOp::MoveTo(x, y) if x.abs() < 0.01 && y.abs() < 0.01));
    assert!(matches!(ops[1], PdfOp::LineTo(x, _) if (x - 100.0).abs() < 0.01));
    assert!(matches!(ops[4], PdfOp::ClosePath));
    assert!(matches!(ops[5], PdfOp::Fill));
}

#[test]
fn test_parse_curve() {
    let stream = b"72 50 m 200 90 400 10 540 50 c S";
    let ops = parse_content_stream(stream).unwrap();

    assert!(matches!(ops[0], PdfOp::MoveTo(..)));
    assert!(matches!(ops[1], PdfOp::CurveTo(..)));
    assert!(matches!(ops[2], PdfOp::Stroke));
}

#[test]
fn test_parse_text_array() {
    let stream = b"BT /F1 12 Tf [(Hello ) -100 (World)] TJ ET";
    let ops = parse_content_stream(stream).unwrap();

    if let PdfOp::ShowTextArray(ref items) = ops[2] {
        assert_eq!(items.len(), 3);
        assert!(matches!(items[0], TextArrayItem::Text(ref t) if t == b"Hello "));
        assert!(matches!(items[1], TextArrayItem::Adjustment(v) if (v + 100.0).abs() < 0.01));
        assert!(matches!(items[2], TextArrayItem::Text(ref t) if t == b"World"));
    } else {
        panic!("expected ShowTextArray");
    }
}

#[test]
fn test_parse_concat_matrix() {
    let stream = b"1 0 0 1 72 720 cm";
    let ops = parse_content_stream(stream).unwrap();

    if let PdfOp::ConcatMatrix(m) = &ops[0] {
        assert!((m[0] - 1.0).abs() < 0.01);
        assert!((m[4] - 72.0).abs() < 0.01);
        assert!((m[5] - 720.0).abs() < 0.01);
    } else {
        panic!("expected ConcatMatrix");
    }
}

#[test]
fn test_parse_color_ops() {
    let stream = b"0.5 G 0.3 0.6 0.9 rg 0.1 0.2 0.3 0.4 k";
    let ops = parse_content_stream(stream).unwrap();

    assert!(matches!(ops[0], PdfOp::SetStrokeGray(v) if (v - 0.5).abs() < 0.01));
    assert!(matches!(ops[1], PdfOp::SetFillRgb(r, g, b)
        if (r - 0.3).abs() < 0.01 && (g - 0.6).abs() < 0.01 && (b - 0.9).abs() < 0.01));
    assert!(matches!(ops[2], PdfOp::SetFillCmyk(c, m, y, k)
        if (c - 0.1).abs() < 0.01 && (m - 0.2).abs() < 0.01));
}

#[test]
fn test_parse_line_properties() {
    let stream = b"2 w 1 J 2 j 10 M";
    let ops = parse_content_stream(stream).unwrap();

    assert!(matches!(ops[0], PdfOp::SetLineWidth(v) if (v - 2.0).abs() < 0.01));
    assert!(matches!(ops[1], PdfOp::SetLineCap(1)));
    assert!(matches!(ops[2], PdfOp::SetLineJoin(2)));
    assert!(matches!(ops[3], PdfOp::SetMiterLimit(v) if (v - 10.0).abs() < 0.01));
}

#[test]
fn test_parse_text_state_ops() {
    let stream = b"BT 1 Tc 2 Tw 14 TL 0 Tr 3 Ts 100 Tz ET";
    let ops = parse_content_stream(stream).unwrap();

    assert!(matches!(ops[1], PdfOp::SetCharSpacing(v) if (v - 1.0).abs() < 0.01));
    assert!(matches!(ops[2], PdfOp::SetWordSpacing(v) if (v - 2.0).abs() < 0.01));
    assert!(matches!(ops[3], PdfOp::SetTextLeading(v) if (v - 14.0).abs() < 0.01));
    assert!(matches!(ops[4], PdfOp::SetTextRenderMode(0)));
    assert!(matches!(ops[5], PdfOp::SetTextRise(v) if (v - 3.0).abs() < 0.01));
    assert!(matches!(ops[6], PdfOp::SetHorizScaling(v) if (v - 100.0).abs() < 0.01));
}

// ============================================================
// Integration tests
// ============================================================

#[test]
fn test_generate_and_parse_pdf() {
    let pdf_data = generate_test_pdf(5);

    // Verify header
    assert!(pdf_data.starts_with(b"%PDF-1.4"));

    // Parse it
    let mut doc = PdfDocument::parse(&pdf_data).unwrap();
    assert_eq!(doc.page_count(), 5);

    // Check first page
    let page = doc.page(0).unwrap();
    assert!((page.width() - 612.0).abs() < 0.01);
    assert!((page.height() - 792.0).abs() < 0.01);

    // Parse content stream
    let ops = parse_content_stream(&page.content_data).unwrap();
    assert!(!ops.is_empty());

    // Should contain text operations
    let has_text = ops.iter().any(|op| matches!(op, PdfOp::BeginText));
    assert!(has_text, "page content should contain text operations");

    // Should contain graphics operations
    let has_rect = ops.iter().any(|op| matches!(op, PdfOp::Rectangle(..)));
    assert!(has_rect, "page content should contain rectangles");

    // Should contain path operations
    let has_curve = ops.iter().any(|op| matches!(op, PdfOp::CurveTo(..)));
    assert!(has_curve, "page content should contain curves");
}

#[test]
fn test_parse_all_pages() {
    let pdf_data = generate_test_pdf(20);
    let mut doc = PdfDocument::parse(&pdf_data).unwrap();
    assert_eq!(doc.page_count(), 20);

    for i in 0..20 {
        let page = doc.page(i).unwrap();
        assert!(
            !page.content_data.is_empty(),
            "page {} should have content",
            i
        );
        let ops = parse_content_stream(&page.content_data).unwrap();
        assert!(!ops.is_empty(), "page {} should have ops", i);
    }
}

#[test]
fn test_font_resources() {
    let pdf_data = generate_test_pdf(1);
    let mut doc = PdfDocument::parse(&pdf_data).unwrap();
    let page = doc.page(0).unwrap();

    assert!(page.fonts.contains_key("F1"), "page should have font F1");
    let font = &page.fonts["F1"];
    assert_eq!(font.base_font, "Helvetica");
    assert_eq!(font.subtype, "Type1");
}

#[test]
fn test_decode_text_winansi() {
    use crate::page::*;
    let font = FontResource {
        subtype: "Type1".to_string(),
        base_font: "Helvetica".to_string(),
        encoding: FontEncoding::WinAnsi,
        widths: Vec::new(),
        first_char: 0,
        last_char: 255,
        to_unicode: None,
        default_width: 600.0,
        cid_widths: None,
    };
    let result = decode_text(&font, b"Hello");
    assert_eq!(result, "Hello");
}

#[test]
fn test_char_width_base14() {
    use crate::page::*;
    let font = FontResource {
        subtype: "Type1".to_string(),
        base_font: "Helvetica".to_string(),
        encoding: FontEncoding::WinAnsi,
        widths: Vec::new(),
        first_char: 0,
        last_char: 255,
        to_unicode: None,
        default_width: 600.0,
        cid_widths: None,
    };
    let w = char_width(&font, b'A' as u32);
    assert!(w > 0.0, "char width should be positive");
    assert!(w < 1000.0, "char width should be reasonable");

    // Courier should be monospaced
    let courier = FontResource {
        base_font: "Courier".to_string(),
        ..font
    };
    let w1 = char_width(&courier, b'i' as u32);
    let w2 = char_width(&courier, b'W' as u32);
    assert!((w1 - w2).abs() < 0.01, "Courier should be monospaced");
}

#[test]
fn test_page_inherits_media_box_and_resources() {
    fn push_obj(pdf: &mut Vec<u8>, offsets: &mut Vec<usize>, num: usize, body: &str) {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", num).as_bytes());
        pdf.extend_from_slice(body.as_bytes());
        pdf.extend_from_slice(b"\nendobj\n");
    }

    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = vec![0usize];

    push_obj(
        &mut pdf,
        &mut offsets,
        1,
        "<< /Type /Catalog /Pages 2 0 R >>",
    );
    push_obj(
        &mut pdf,
        &mut offsets,
        2,
        "<< /Type /Pages /Count 1 /Kids [3 0 R] /MediaBox [0 0 300 200] /Resources << /Font << /F1 4 0 R >> >> >>",
    );
    push_obj(
        &mut pdf,
        &mut offsets,
        3,
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R >>",
    );
    push_obj(
        &mut pdf,
        &mut offsets,
        4,
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    );

    let content = "BT /F1 12 Tf 10 10 Td (Hello) Tj ET";
    push_obj(
        &mut pdf,
        &mut offsets,
        5,
        &format!(
            "<< /Length {} >>\nstream\n{}\nendstream",
            content.len(),
            content
        ),
    );

    let xref_offset = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n", offsets.len()).as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets.iter().skip(1) {
        pdf.extend_from_slice(format!("{:010} 00000 n \n", offset).as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
            offsets.len(),
            xref_offset
        )
        .as_bytes(),
    );

    let mut doc = PdfDocument::parse(&pdf).unwrap();
    assert_eq!(doc.page_count(), 1);

    let page = doc.page(0).unwrap();
    assert_eq!(page.media_box, [0.0, 0.0, 300.0, 200.0]);
    assert_eq!(page.crop_box, [0.0, 0.0, 300.0, 200.0]);
    assert!(page.fonts.contains_key("F1"));
    assert!(!page.content_data.is_empty());
}

// ============================================================
// Stress test
// ============================================================

#[test]
fn test_stress_large_pdf() {
    let pdf_data = generate_test_pdf(50);
    assert!(pdf_data.len() > 10000, "large PDF should be substantial");

    let mut doc = PdfDocument::parse(&pdf_data).unwrap();
    assert_eq!(doc.page_count(), 50);

    // Parse every page's content
    let mut total_ops = 0;
    for i in 0..50 {
        let page = doc.page(i).unwrap();
        let ops = parse_content_stream(&page.content_data).unwrap();
        total_ops += ops.len();
    }
    assert!(
        total_ops > 1000,
        "50-page PDF should have many ops, got {}",
        total_ops
    );
}

// ============================================================
// Object tests
// ============================================================

#[test]
fn test_pdf_dict_helpers() {
    let mut dict = PdfDict::new();
    dict.map
        .insert("Name".to_string(), PdfObj::Name("Test".to_string()));
    dict.map.insert("Count".to_string(), PdfObj::Int(42));
    dict.map.insert("Scale".to_string(), PdfObj::Real(1.5));
    dict.map
        .insert("Data".to_string(), PdfObj::Str(b"hello".to_vec()));

    assert_eq!(dict.get_name("Name"), Some("Test"));
    assert_eq!(dict.get_int("Count"), Some(42));
    assert_eq!(dict.get_f64("Scale"), Some(1.5));
    assert_eq!(dict.get_f64("Count"), Some(42.0)); // Int coerced to f64
    assert_eq!(dict.get_str("Data"), Some(b"hello".as_slice()));
    assert_eq!(dict.get("Missing"), None);
}

#[test]
fn test_pdf_obj_number_array() {
    let arr = PdfObj::Array(vec![
        PdfObj::Int(0),
        PdfObj::Int(0),
        PdfObj::Real(612.0),
        PdfObj::Real(792.0),
    ]);
    let nums = arr.as_number_array().unwrap();
    assert_eq!(nums, vec![0.0, 0.0, 612.0, 792.0]);
}

// ============================================================
// XRef parsing tests
// ============================================================

#[test]
fn test_find_startxref() {
    let pdf = b"%PDF-1.4\nsome content\nstartxref\n42\n%%EOF\n";
    let offset = crate::parser::find_startxref(pdf).unwrap();
    assert_eq!(offset, 42);
}

#[test]
fn test_rfind() {
    let data = b"hello world hello";
    assert_eq!(Lexer::rfind(data, b"hello"), Some(12));
    assert_eq!(Lexer::rfind(data, b"world"), Some(6));
    assert_eq!(Lexer::rfind(data, b"missing"), None);
}

// ============================================================
// Text-space matrix math (the "giant garble" regression class):
// Td / T* / advances / TJ adjustments compose THROUGH the text
// matrix. With `Tf 1` + `Tm [s 0 0 s x y]` (the InDesign/Chrome
// export style) raw e/f addition collapses every run of a line
// onto one point.
// ============================================================

#[test]
fn test_tm_pre_translate_scales_through_matrix() {
    // Tm [20 0 0 20 152.126 737.8154]; Td 0.533 0 must move 10.66pt.
    let mut lm = [20.0, 0.0, 0.0, 20.0, 152.126, 737.8154];
    crate::content::tm_pre_translate(&mut lm, 0.533, 0.0);
    assert!((lm[4] - (152.126 + 0.533 * 20.0)).abs() < 1e-9);
    assert!((lm[5] - 737.8154).abs() < 1e-9);
    // T* with TL 9.342 moves the line 186.84pt down in text space.
    crate::content::tm_pre_translate(&mut lm, 0.0, -9.342);
    assert!((lm[5] - (737.8154 - 9.342 * 20.0)).abs() < 1e-9);
}

#[test]
fn test_tm_advance_follows_baseline() {
    let mut tm = [20.0, 0.0, 0.0, 20.0, 100.0, 500.0];
    crate::content::tm_advance(&mut tm, 0.7); // 0.7 text units = 14pt
    assert!((tm[4] - 114.0).abs() < 1e-9);
    assert!((tm[5] - 500.0).abs() < 1e-9);
    // A rotated baseline advances along its own direction.
    let (s, c) = (std::f64::consts::FRAC_PI_2.sin(), std::f64::consts::FRAC_PI_2.cos());
    let mut rot = [c, s, -s, c, 0.0, 0.0]; // 90°: baseline points +y
    crate::content::tm_advance(&mut rot, 2.0);
    assert!(rot[4].abs() < 1e-9);
    assert!((rot[5] - 2.0).abs() < 1e-9);
}

#[test]
fn test_tj_adjustment_advance() {
    // -1000 thousandths at size 10 = +10 text units to the right.
    let a = crate::content::tj_adjustment_advance(-1000.0, 10.0, 100.0);
    assert!((a - 10.0).abs() < 1e-9);
    // Tz halves it.
    let a = crate::content::tj_adjustment_advance(-1000.0, 10.0, 50.0);
    assert!((a - 5.0).abs() < 1e-9);
}

#[test]
fn test_mat_scales() {
    let m = [3.0, 4.0, -8.0, 6.0, 9.0, 9.0];
    assert!((crate::content::mat_x_scale(&m) - 5.0).abs() < 1e-9);
    assert!((crate::content::mat_y_scale(&m) - 10.0).abs() < 1e-9);
}

// ============================================================
// Encoding /Differences + glyph names + CID widths
// ============================================================

#[test]
fn test_glyph_name_component_ligature() {
    use crate::font::glyph_name_to_string;
    assert_eq!(glyph_name_to_string("fi"), "\u{FB01}");
    // Component names decompose: f_i = f + i, f_f_l = f + f + l.
    assert_eq!(glyph_name_to_string("f_i"), "fi");
    assert_eq!(glyph_name_to_string("f_f_l"), "ffl");
    assert_eq!(glyph_name_to_string("odieresis"), "ö");
    assert_eq!(glyph_name_to_string("uni20AC"), "€");
    assert_eq!(glyph_name_to_string("a"), "a");
    assert_eq!(glyph_name_to_string("bogusname"), "");
}

#[test]
fn test_differences_decode_over_winansi() {
    use crate::page::{FontEncoding, FontResource};
    let mut map = std::collections::HashMap::new();
    map.insert(31u8, "f_i".to_string());
    let font = FontResource {
        subtype: "Type1".into(),
        base_font: "Test".into(),
        encoding: FontEncoding::Custom(crate::page::BaseEncoding::WinAnsi, map),
        widths: vec![],
        first_char: 31,
        last_char: 255,
        to_unicode: None,
        default_width: 500.0,
        cid_widths: None,
    };
    // 0x1F -> fi ligature via Differences; 0xF6 -> ö via the WinAnsi base.
    let s = crate::font::decode_text(&font, &[0x1F, b'n', 0xF6]);
    assert_eq!(s, "finö");
    let codes = crate::font::decode_codes(&font, &[0x1F, 0xF6]);
    assert_eq!(codes.len(), 2);
    assert_eq!(codes[0], (0x1F, "fi".to_string()));
    assert_eq!(codes[1], (0xF6, "ö".to_string()));
}

#[test]
fn test_cid_widths_take_priority() {
    use crate::page::{FontEncoding, FontResource};
    let mut cid = std::collections::HashMap::new();
    cid.insert(5u32, 812.0);
    let font = FontResource {
        subtype: "Type0".into(),
        base_font: "Test".into(),
        encoding: FontEncoding::Identity,
        widths: vec![],
        first_char: 0,
        last_char: 255,
        to_unicode: None,
        default_width: 1000.0,
        cid_widths: Some(cid),
    };
    assert_eq!(crate::font::char_width(&font, 5), 812.0);
    assert_eq!(crate::font::char_width(&font, 6), 1000.0); // DW fallback
}

// ============================================================
// The acceptance file: local/retourformulier-techpunt-ned.pdf
// (checked into local/; the test skips silently when absent so
// the suite stays green on trees without local data).
// ============================================================

#[test]
fn test_retourformulier_fonts_and_text() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../local/retourformulier-techpunt-ned.pdf");
    let Ok(data) = std::fs::read(&path) else {
        return;
    };
    let mut doc = PdfDocument::parse(&data).expect("parse");
    assert_eq!(doc.page_count(), 1);
    let page = doc.page(0).expect("page");
    let font = page.fonts.get("T1_0").expect("font T1_0");
    // The Encoding is a dict with Differences [31 /f_i]; before the fix it
    // collapsed to plain WinAnsi and code 31 rendered as a control char.
    match &font.encoding {
        crate::page::FontEncoding::Custom(_, map) => {
            assert_eq!(map.get(&31).map(String::as_str), Some("f_i"));
        }
        other => panic!("expected Differences encoding, got {:?}", other),
    }
    assert_eq!(crate::font::decode_text(font, &[0x1F]), "fi");

    // The title line: Tm [20 0 0 20 152.126 737.8154] with per-run
    // Td kerning. Simulated through the fixed math, the runs of
    // "Retourformulier" must be strictly increasing in x and the second
    // line must sit 9.342*20 = 186.84pt lower.
    let ops = parse_content_stream(&page.content_data).expect("ops");
    let mut lm = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    let mut tm = lm;
    let mut fs = 0.0f64;
    let mut runs: Vec<(String, f64, f64)> = Vec::new();
    for op in &ops {
        match op {
            PdfOp::SetFont(_, s) => fs = *s,
            PdfOp::SetTextMatrix(m) => {
                tm = *m;
                lm = *m;
            }
            PdfOp::MoveText(x, y) => {
                crate::content::tm_pre_translate(&mut lm, *x, *y);
                tm = lm;
            }
            PdfOp::ShowText(bytes) => {
                let text = crate::font::decode_text(font, bytes);
                runs.push((text, tm[4], tm[5]));
                let mut adv = 0.0;
                for &b in bytes.iter() {
                    adv += crate::font::char_width(font, b as u32) * fs / 1000.0;
                }
                crate::content::tm_advance(&mut tm, adv);
            }
            _ => {}
        }
    }
    let title: Vec<&(String, f64, f64)> = runs
        .iter()
        .skip_while(|(t, _, _)| t != "R")
        .take(4)
        .collect();
    assert_eq!(title.len(), 4);
    let joined: String = title.iter().map(|(t, _, _)| t.as_str()).collect();
    assert_eq!(joined, "Retourformulier");
    // Strictly increasing x, each run starting past the previous one.
    for w in title.windows(2) {
        assert!(
            w[1].1 > w[0].1 + 1.0,
            "runs must not overlap: {:?} then {:?}",
            w[0],
            w[1]
        );
    }
    // "Retour" run 2 starts at 152.126 + 0.533*20 = 162.786.
    assert!((title[1].1 - 162.786).abs() < 0.01, "got {}", title[1].1);
    // Second title line ("R" of "Retourartikelen") sits 186.84pt lower.
    let second_r = runs
        .iter()
        .filter(|(t, _, _)| t == "R")
        .nth(1)
        .expect("second R run");
    assert!((second_r.2 - (737.8154 - 9.342 * 20.0)).abs() < 0.01);
}

// ============================================================
// Codex-review regressions
// ============================================================

#[test]
fn test_differences_macroman_base_fallback() {
    use crate::page::{BaseEncoding, FontEncoding, FontResource};
    let mut map = std::collections::HashMap::new();
    map.insert(31u8, "f_i".to_string());
    let font = FontResource {
        subtype: "Type1".into(),
        base_font: "Test".into(),
        encoding: FontEncoding::Custom(BaseEncoding::MacRoman, map),
        widths: vec![],
        first_char: 31,
        last_char: 255,
        to_unicode: None,
        default_width: 500.0,
        cid_widths: None,
    };
    // 0x80 is Ä in MacRoman (€ in WinAnsi) — the base must win.
    assert_eq!(crate::font::decode_text(&font, &[0x80]), "Ä");
}

#[test]
fn test_glyph_name_hex_forms_safe() {
    use crate::font::glyph_name_to_string;
    // Multi-group uniXXXXYYYY decodes all groups.
    assert_eq!(glyph_name_to_string("uni00660069"), "fi");
    // Non-hex / non-ASCII suffixes must not panic and yield nothing
    // (a single trailing char falls to the one-char rule only for
    // one-char names, not uni-prefixed ones).
    assert_eq!(glyph_name_to_string("uniZZZZ"), "");
    assert_eq!(glyph_name_to_string("uniéééé"), "");
    assert_eq!(glyph_name_to_string("minus"), "\u{2212}");
    assert_eq!(glyph_name_to_string("hyphen"), "-");
}

#[test]
fn test_type0_without_w_uses_dw() {
    use crate::page::{FontEncoding, FontResource};
    let font = FontResource {
        subtype: "Type0".into(),
        base_font: "Test".into(),
        encoding: FontEncoding::Identity,
        widths: vec![],
        first_char: 0,
        last_char: 255,
        to_unicode: None,
        default_width: 725.0,
        cid_widths: Some(std::collections::HashMap::new()),
    };
    // CID 65 ('A' under Identity) must take DW, not a base-14 estimate.
    assert_eq!(crate::font::char_width(&font, 65), 725.0);
}
