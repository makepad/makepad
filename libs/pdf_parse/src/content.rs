use crate::lexer::*;
use crate::object::*;

/// A PDF content stream operation.
#[derive(Clone, Debug)]
pub enum PdfOp {
    // Graphics state
    SaveState,              // q
    RestoreState,           // Q
    ConcatMatrix([f64; 6]), // cm
    SetLineWidth(f64),      // w
    SetLineCap(i32),        // J
    SetLineJoin(i32),       // j
    SetMiterLimit(f64),     // M
    SetDash(Vec<f64>, f64), // d
    SetExtGState(String),   // gs

    // Color
    SetStrokeGray(f64),                // G
    SetFillGray(f64),                  // g
    SetStrokeRgb(f64, f64, f64),       // RG
    SetFillRgb(f64, f64, f64),         // rg
    SetStrokeCmyk(f64, f64, f64, f64), // K
    SetFillCmyk(f64, f64, f64, f64),   // k
    SetStrokeColorSpace(String),       // CS
    SetFillColorSpace(String),         // cs
    SetStrokeColor(Vec<f64>),          // SC / SCN
    SetFillColor(Vec<f64>),            // sc / scn

    // Path construction
    MoveTo(f64, f64),                      // m
    LineTo(f64, f64),                      // l
    CurveTo(f64, f64, f64, f64, f64, f64), // c
    CurveToV(f64, f64, f64, f64),          // v (initial point = current)
    CurveToY(f64, f64, f64, f64),          // y (final point = control)
    ClosePath,                             // h
    Rectangle(f64, f64, f64, f64),         // re

    // Path painting
    Stroke,                 // S
    CloseStroke,            // s
    Fill,                   // f / F
    FillEvenOdd,            // f*
    FillStroke,             // B
    FillStrokeEvenOdd,      // B*
    CloseFillStroke,        // b
    CloseFillStrokeEvenOdd, // b*
    EndPath,                // n (no-op path end, used with clipping)

    // Clipping
    Clip,        // W
    ClipEvenOdd, // W*

    // Text
    BeginText,                                  // BT
    EndText,                                    // ET
    SetFont(String, f64),                       // Tf (font_name, size)
    MoveText(f64, f64),                         // Td
    MoveTextSetLeading(f64, f64),               // TD
    SetTextMatrix([f64; 6]),                    // Tm
    NextLine,                                   // T*
    SetCharSpacing(f64),                        // Tc
    SetWordSpacing(f64),                        // Tw
    SetTextLeading(f64),                        // TL
    SetTextRenderMode(i32),                     // Tr
    SetTextRise(f64),                           // Ts
    SetHorizScaling(f64),                       // Tz
    ShowText(Vec<u8>),                          // Tj
    ShowTextArray(Vec<TextArrayItem>),          // TJ
    ShowTextNextLine(Vec<u8>),                  // '
    ShowTextNextLineSpacing(f64, f64, Vec<u8>), // "

    // XObject
    PaintXObject(String), // Do

    // Inline image (decoded separately)
    InlineImage { dict: PdfDict, data: Vec<u8> },

    // Marked content (we skip these but parse them to not break)
    BeginMarkedContent(String),      // BMC
    BeginMarkedContentProps(String), // BDC
    EndMarkedContent,                // EMC
}

/// Item in a TJ array: either a string or a position adjustment.
#[derive(Clone, Debug)]
pub enum TextArrayItem {
    Text(Vec<u8>),
    Adjustment(f64), // negative = advance right, positive = advance left
}

/// Parse a content stream into a list of operations.
pub fn parse_content_stream(data: &[u8]) -> PdfResult<Vec<PdfOp>> {
    let mut ops = Vec::new();
    let mut operands: Vec<PdfObj> = Vec::new();
    let mut lex = Lexer::new(data, 0);

    loop {
        lex.skip_whitespace();
        if lex.is_eof() {
            break;
        }

        let b = lex.data[lex.pos];

        // Check for inline image
        if b == b'B'
            && lex.pos + 1 < data.len()
            && data[lex.pos + 1] == b'I'
            && (lex.pos + 2 >= data.len() || is_space_or_eol(data[lex.pos + 2]))
        {
            lex.pos += 2;
            if let Ok(inline_img) = parse_inline_image(&mut lex) {
                ops.push(inline_img);
            }
            operands.clear();
            continue;
        }

        // Try to read an operand (number, string, name, array, dict, bool)
        if b == b'('
            || b == b'<'
            || b == b'['
            || b == b'/'
            || b == b'+'
            || b == b'-'
            || b == b'.'
            || b.is_ascii_digit()
            || (b == b't' && lex.starts_with(b"true"))
            || (b == b'f' && lex.starts_with(b"false"))
            || (b == b'n' && lex.starts_with(b"null"))
        {
            match lex.read_object() {
                Ok(obj) => {
                    operands.push(obj);
                    continue;
                }
                Err(_) => {
                    // If we can't parse as object, try as operator keyword
                }
            }
        }

        // Must be an operator keyword
        let kw = match lex.read_keyword() {
            Ok(k) => k,
            Err(_) => {
                lex.pos += 1; // skip unknown byte
                continue;
            }
        };

        let op = build_op(&kw, &operands);
        if let Some(op) = op {
            ops.push(op);
        }
        operands.clear();
    }

    Ok(ops)
}

fn is_space_or_eol(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0 | 12)
}

fn build_op(kw: &str, operands: &[PdfObj]) -> Option<PdfOp> {
    match kw {
        // Graphics state
        "q" => Some(PdfOp::SaveState),
        "Q" => Some(PdfOp::RestoreState),
        "cm" => {
            let m = get_matrix(operands)?;
            Some(PdfOp::ConcatMatrix(m))
        }
        "w" => Some(PdfOp::SetLineWidth(get_f64(operands, 0)?)),
        "J" => Some(PdfOp::SetLineCap(get_i32(operands, 0)?)),
        "j" => Some(PdfOp::SetLineJoin(get_i32(operands, 0)?)),
        "M" => Some(PdfOp::SetMiterLimit(get_f64(operands, 0)?)),
        "d" => {
            let arr = operands.first()?.as_array()?;
            let dash: Vec<f64> = arr.iter().filter_map(|o| o.as_f64()).collect();
            let phase = get_f64(operands, 1).unwrap_or(0.0);
            Some(PdfOp::SetDash(dash, phase))
        }
        "gs" => Some(PdfOp::SetExtGState(get_name(operands, 0)?)),

        // Color
        "G" => Some(PdfOp::SetStrokeGray(get_f64(operands, 0)?)),
        "g" => Some(PdfOp::SetFillGray(get_f64(operands, 0)?)),
        "RG" => Some(PdfOp::SetStrokeRgb(
            get_f64(operands, 0)?,
            get_f64(operands, 1)?,
            get_f64(operands, 2)?,
        )),
        "rg" => Some(PdfOp::SetFillRgb(
            get_f64(operands, 0)?,
            get_f64(operands, 1)?,
            get_f64(operands, 2)?,
        )),
        "K" => Some(PdfOp::SetStrokeCmyk(
            get_f64(operands, 0)?,
            get_f64(operands, 1)?,
            get_f64(operands, 2)?,
            get_f64(operands, 3)?,
        )),
        "k" => Some(PdfOp::SetFillCmyk(
            get_f64(operands, 0)?,
            get_f64(operands, 1)?,
            get_f64(operands, 2)?,
            get_f64(operands, 3)?,
        )),
        "CS" => Some(PdfOp::SetStrokeColorSpace(get_name(operands, 0)?)),
        "cs" => Some(PdfOp::SetFillColorSpace(get_name(operands, 0)?)),
        "SC" | "SCN" => {
            let vals: Vec<f64> = operands.iter().filter_map(|o| o.as_f64()).collect();
            Some(PdfOp::SetStrokeColor(vals))
        }
        "sc" | "scn" => {
            let vals: Vec<f64> = operands.iter().filter_map(|o| o.as_f64()).collect();
            Some(PdfOp::SetFillColor(vals))
        }

        // Path construction
        "m" => Some(PdfOp::MoveTo(get_f64(operands, 0)?, get_f64(operands, 1)?)),
        "l" => Some(PdfOp::LineTo(get_f64(operands, 0)?, get_f64(operands, 1)?)),
        "c" => Some(PdfOp::CurveTo(
            get_f64(operands, 0)?,
            get_f64(operands, 1)?,
            get_f64(operands, 2)?,
            get_f64(operands, 3)?,
            get_f64(operands, 4)?,
            get_f64(operands, 5)?,
        )),
        "v" => Some(PdfOp::CurveToV(
            get_f64(operands, 0)?,
            get_f64(operands, 1)?,
            get_f64(operands, 2)?,
            get_f64(operands, 3)?,
        )),
        "y" => Some(PdfOp::CurveToY(
            get_f64(operands, 0)?,
            get_f64(operands, 1)?,
            get_f64(operands, 2)?,
            get_f64(operands, 3)?,
        )),
        "h" => Some(PdfOp::ClosePath),
        "re" => Some(PdfOp::Rectangle(
            get_f64(operands, 0)?,
            get_f64(operands, 1)?,
            get_f64(operands, 2)?,
            get_f64(operands, 3)?,
        )),

        // Path painting
        "S" => Some(PdfOp::Stroke),
        "s" => Some(PdfOp::CloseStroke),
        "f" | "F" => Some(PdfOp::Fill),
        "f*" => Some(PdfOp::FillEvenOdd),
        "B" => Some(PdfOp::FillStroke),
        "B*" => Some(PdfOp::FillStrokeEvenOdd),
        "b" => Some(PdfOp::CloseFillStroke),
        "b*" => Some(PdfOp::CloseFillStrokeEvenOdd),
        "n" => Some(PdfOp::EndPath),

        // Clipping
        "W" => Some(PdfOp::Clip),
        "W*" => Some(PdfOp::ClipEvenOdd),

        // Text
        "BT" => Some(PdfOp::BeginText),
        "ET" => Some(PdfOp::EndText),
        "Tf" => {
            let name = get_name(operands, 0)?;
            let size = get_f64(operands, 1)?;
            Some(PdfOp::SetFont(name, size))
        }
        "Td" => Some(PdfOp::MoveText(
            get_f64(operands, 0)?,
            get_f64(operands, 1)?,
        )),
        "TD" => Some(PdfOp::MoveTextSetLeading(
            get_f64(operands, 0)?,
            get_f64(operands, 1)?,
        )),
        "Tm" => {
            let m = get_matrix(operands)?;
            Some(PdfOp::SetTextMatrix(m))
        }
        "T*" => Some(PdfOp::NextLine),
        "Tc" => Some(PdfOp::SetCharSpacing(get_f64(operands, 0)?)),
        "Tw" => Some(PdfOp::SetWordSpacing(get_f64(operands, 0)?)),
        "TL" => Some(PdfOp::SetTextLeading(get_f64(operands, 0)?)),
        "Tr" => Some(PdfOp::SetTextRenderMode(get_i32(operands, 0)?)),
        "Ts" => Some(PdfOp::SetTextRise(get_f64(operands, 0)?)),
        "Tz" => Some(PdfOp::SetHorizScaling(get_f64(operands, 0)?)),
        "Tj" => {
            let bytes = get_bytes(operands, 0)?;
            Some(PdfOp::ShowText(bytes))
        }
        "TJ" => {
            let arr = operands.first()?.as_array()?;
            let mut items = Vec::new();
            for item in arr {
                match item {
                    PdfObj::Str(s) => items.push(TextArrayItem::Text(s.clone())),
                    PdfObj::Int(n) => items.push(TextArrayItem::Adjustment(*n as f64)),
                    PdfObj::Real(n) => items.push(TextArrayItem::Adjustment(*n)),
                    _ => {}
                }
            }
            Some(PdfOp::ShowTextArray(items))
        }
        "'" => {
            let bytes = get_bytes(operands, 0)?;
            Some(PdfOp::ShowTextNextLine(bytes))
        }
        "\"" => {
            let aw = get_f64(operands, 0)?;
            let ac = get_f64(operands, 1)?;
            let bytes = get_bytes(operands, 2)?;
            Some(PdfOp::ShowTextNextLineSpacing(aw, ac, bytes))
        }

        // XObject
        "Do" => Some(PdfOp::PaintXObject(get_name(operands, 0)?)),

        // Marked content (skip but parse)
        "BMC" => {
            let tag = get_name(operands, 0).unwrap_or_default();
            Some(PdfOp::BeginMarkedContent(tag))
        }
        "BDC" => {
            let tag = get_name(operands, 0).unwrap_or_default();
            Some(PdfOp::BeginMarkedContentProps(tag))
        }
        "EMC" => Some(PdfOp::EndMarkedContent),

        // Unknown operator: ignore
        _ => None,
    }
}

fn parse_inline_image(lex: &mut Lexer) -> PdfResult<PdfOp> {
    // Parse key-value pairs until "ID"
    let mut dict = PdfDict::new();
    loop {
        lex.skip_whitespace();
        if lex.is_eof() {
            return Err(PdfError::new("unterminated inline image"));
        }

        // Check for "ID" keyword
        if lex.starts_with(b"ID") {
            lex.pos += 2;
            // Skip single whitespace after ID
            if lex.pos < lex.data.len() && is_space_or_eol(lex.data[lex.pos]) {
                lex.pos += 1;
            }
            break;
        }

        let key = lex.read_object()?;
        let key_name = match key {
            PdfObj::Name(n) => expand_inline_image_key(&n),
            _ => return Err(PdfError::new("inline image key must be a name")),
        };
        let value = lex.read_object()?;
        dict.map.insert(key_name, value);
    }

    // Read binary data until "EI" preceded by whitespace
    let start = lex.pos;
    let mut end = start;
    while end < lex.data.len().saturating_sub(2) {
        if is_space_or_eol(lex.data[end])
            && lex.data[end + 1] == b'E'
            && lex.data[end + 2] == b'I'
            && (end + 3 >= lex.data.len() || is_space_or_eol(lex.data[end + 3]))
        {
            break;
        }
        end += 1;
    }
    let data = lex.data[start..end].to_vec();
    lex.pos = end + 3; // skip whitespace + "EI"

    Ok(PdfOp::InlineImage { dict, data })
}

/// Expand abbreviated inline image keys to full names.
fn expand_inline_image_key(key: &str) -> String {
    match key {
        "BPC" => "BitsPerComponent".to_string(),
        "CS" => "ColorSpace".to_string(),
        "D" => "Decode".to_string(),
        "DP" => "DecodeParms".to_string(),
        "F" => "Filter".to_string(),
        "H" => "Height".to_string(),
        "IM" => "ImageMask".to_string(),
        "I" => "Interpolate".to_string(),
        "W" => "Width".to_string(),
        other => other.to_string(),
    }
}

// Helper functions for extracting operands

fn get_f64(operands: &[PdfObj], index: usize) -> Option<f64> {
    operands.get(index)?.as_f64()
}

fn get_i32(operands: &[PdfObj], index: usize) -> Option<i32> {
    operands.get(index)?.as_int().map(|n| n as i32)
}

fn get_name(operands: &[PdfObj], index: usize) -> Option<String> {
    operands.get(index)?.as_name().map(|s| s.to_string())
}

fn get_bytes(operands: &[PdfObj], index: usize) -> Option<Vec<u8>> {
    operands.get(index)?.as_str_bytes().map(|s| s.to_vec())
}

fn get_matrix(operands: &[PdfObj]) -> Option<[f64; 6]> {
    if operands.len() < 6 {
        return None;
    }
    Some([
        operands[0].as_f64()?,
        operands[1].as_f64()?,
        operands[2].as_f64()?,
        operands[3].as_f64()?,
        operands[4].as_f64()?,
        operands[5].as_f64()?,
    ])
}
