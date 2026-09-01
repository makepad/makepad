//! Lossless content-stream ledger and interpreted page-space display list.
//!
//! `makepad-pdf-parse` intentionally exposes cooked operations. This module
//! wraps it with local token-span tracking and direct page-tree traversal so
//! every decoded operator retains its source object, stream and byte range.

use crate::geometry::{bounds, Affine, Point, Rect};
use makepad_pdf_parse::font::{char_width, decode_codes};
use makepad_pdf_parse::page::{CMapData, FontEncoding, FontResource, XObjectResource};
use makepad_pdf_parse::{
    parse_content_stream, ObjRef, PdfDict, PdfDocument, PdfObj, PdfOp, PdfResult,
    TextArrayItem,
};
use std::collections::{HashMap, HashSet};
use std::ops::Range;

/// Compatibility shim for the checked-out parser, which does not yet resolve
/// indirect stream `/Length` values. Replacements are byte-for-byte equal in
/// size, so every xref offset and provenance byte range remains valid. The
/// caller continues to retain/export the untouched original bytes.
pub(crate) fn parser_compatible_bytes(data: &[u8]) -> Option<Vec<u8>> {
    let marker = b"/Length";
    let mut output = data.to_vec();
    let mut cursor = 0;
    let mut changed = false;
    while cursor + marker.len() < data.len() {
        let Some(relative) = data[cursor..]
            .windows(marker.len())
            .position(|window| window == marker)
        else {
            break;
        };
        let position = cursor + relative;
        cursor = position + marker.len();
        if data.get(cursor).is_some_and(|byte| !byte.is_ascii_whitespace()) {
            continue;
        }
        skip_ascii_space(data, &mut cursor);
        let reference_start = cursor;
        let Some(object_number) = parse_ascii_u32(data, &mut cursor) else {
            continue;
        };
        skip_ascii_space(data, &mut cursor);
        let Some(generation) = parse_ascii_u32(data, &mut cursor) else {
            continue;
        };
        skip_ascii_space(data, &mut cursor);
        if data.get(cursor) != Some(&b'R') {
            continue;
        }
        cursor += 1;
        let reference_end = cursor;
        let Some(length) = indirect_integer(data, object_number, generation) else {
            continue;
        };
        let digits = length.to_string();
        if digits.len() > reference_end - reference_start {
            continue;
        }
        output[reference_start..reference_end].fill(b' ');
        output[reference_start..reference_start + digits.len()].copy_from_slice(digits.as_bytes());
        changed = true;
    }
    changed.then_some(output)
}

fn skip_ascii_space(data: &[u8], cursor: &mut usize) {
    while data.get(*cursor).is_some_and(u8::is_ascii_whitespace) {
        *cursor += 1;
    }
}

fn parse_ascii_u32(data: &[u8], cursor: &mut usize) -> Option<u32> {
    let start = *cursor;
    while data.get(*cursor).is_some_and(u8::is_ascii_digit) {
        *cursor += 1;
    }
    (start != *cursor)
        .then(|| std::str::from_utf8(&data[start..*cursor]).ok()?.parse().ok())
        .flatten()
}

fn indirect_integer(data: &[u8], object_number: u32, generation: u32) -> Option<u64> {
    let marker = format!("{object_number} {generation} obj");
    let mut search_start = 0;
    while let Some(relative) = data[search_start..]
        .windows(marker.len())
        .position(|window| window == marker.as_bytes())
    {
        let position = search_start + relative;
        search_start = position + marker.len();
        if position > 0
            && data[position - 1].is_ascii_graphic()
            && !matches!(data[position - 1], b'<' | b'>' | b'[' | b']' | b'(' | b')')
        {
            continue;
        }
        let mut cursor = position + marker.len();
        skip_ascii_space(data, &mut cursor);
        let start = cursor;
        while data.get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
        }
        if start != cursor {
            if let Ok(value) = std::str::from_utf8(&data[start..cursor]) {
                if let Ok(value) = value.parse() {
                    return Some(value);
                }
            }
        }
    }
    None
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PrimitiveId(pub u64);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PageIndex(pub u32);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FormHop {
    pub name: String,
    pub object: ObjRef,
    pub invocation_operator: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceSpan {
    pub object: ObjRef,
    pub stream_index: u16,
    pub decoded_bytes: Range<u32>,
    pub operator_index: u32,
    pub subpath_index: Option<u16>,
    pub form_chain: Vec<FormHop>,
}

#[derive(Clone, Debug)]
pub enum RetainedOperator {
    Parsed(PdfOp),
    Raw { keyword: String },
}

#[derive(Clone, Debug)]
pub struct OperatorRecord {
    pub source: SourceSpan,
    /// Exact decoded bytes from the beginning of the first operand through
    /// the operator. Unknown operators are retained here unchanged.
    pub raw: Vec<u8>,
    pub operation: RetainedOperator,
}

#[derive(Clone, Debug)]
pub struct ContentStreamRecord {
    pub object: ObjRef,
    pub stream_index: u16,
    pub decoded: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Color {
    Gray(f64),
    Rgb(f64, f64, f64),
    Cmyk(f64, f64, f64, f64),
    Components([f64; 4], u8),
}

impl Default for Color {
    fn default() -> Self {
        Self::Gray(0.0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathPaint {
    Stroke,
    Fill,
    FillEvenOdd,
    FillStroke,
    FillStrokeEvenOdd,
    None,
}

impl PathPaint {
    pub const fn is_stroked(self) -> bool {
        matches!(self, Self::Stroke | Self::FillStroke | Self::FillStrokeEvenOdd)
    }

    pub const fn is_filled(self) -> bool {
        matches!(
            self,
            Self::Fill | Self::FillEvenOdd | Self::FillStroke | Self::FillStrokeEvenOdd
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClipRule {
    NonZero,
    EvenOdd,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PathCommand {
    Move(Point),
    Line(Point),
    Cubic(Point, Point, Point),
    Close,
}

#[derive(Clone, Debug)]
pub struct PdfPath {
    pub commands: Vec<PathCommand>,
    pub bounds: Rect,
    pub paint: PathPaint,
    pub clip: Option<ClipRule>,
    pub line_width: f64,
    pub stroke_color: Color,
    pub fill_color: Color,
    pub command_sources: Vec<SourceSpan>,
    pub paint_source: SourceSpan,
}

impl PdfPath {
    pub fn points(&self) -> Vec<Point> {
        let mut points = Vec::new();
        for command in &self.commands {
            match command {
                PathCommand::Move(point) | PathCommand::Line(point) => points.push(*point),
                PathCommand::Cubic(a, b, c) => points.extend([*a, *b, *c]),
                PathCommand::Close => {}
            }
        }
        points
    }

    pub fn endpoints(&self) -> Option<(Point, Point)> {
        let mut first = None;
        let mut last = None;
        for command in &self.commands {
            let point = match command {
                PathCommand::Move(point) | PathCommand::Line(point) => Some(*point),
                PathCommand::Cubic(_, _, point) => Some(*point),
                PathCommand::Close => None,
            };
            if let Some(point) = point {
                first.get_or_insert(point);
                last = Some(point);
            }
        }
        first.zip(last)
    }
}

#[derive(Clone, Debug)]
pub struct PdfGlyph {
    pub font_resource: String,
    pub font_base_name: String,
    pub code: u32,
    pub raw_name: Option<String>,
    pub unicode: Option<String>,
    pub origin: Point,
    pub text_render_matrix: Affine,
    pub bounds: Rect,
    pub advance: f64,
    /// A `TJ` numeric adjustment that reproduces this glyph's text advance
    /// without painting it. Present only when the text scale is invertible.
    pub invisible_advance_1000: Option<f64>,
    pub source: SourceSpan,
}

#[derive(Clone, Debug)]
pub struct PdfImageRef {
    pub name: Option<String>,
    pub object: Option<ObjRef>,
    pub pixel_size: Option<(u32, u32)>,
    pub bounds: Rect,
    pub encoded_bytes: usize,
    pub source: SourceSpan,
}

#[derive(Clone, Debug)]
pub struct FormInvocation {
    pub name: String,
    pub object: ObjRef,
    pub matrix: Affine,
    pub bounds: Option<Rect>,
    pub source: SourceSpan,
    pub expanded: bool,
}

#[derive(Clone, Debug)]
pub enum DisplayPrimitive {
    Glyph(PdfGlyph),
    Path(PdfPath),
    Image(PdfImageRef),
    Form(FormInvocation),
    Transform { matrix: Affine, source: SourceSpan },
    State { operation: PdfOp, source: SourceSpan },
    Raw { bytes: Vec<u8>, source: SourceSpan },
}

impl DisplayPrimitive {
    pub fn source(&self) -> &SourceSpan {
        match self {
            Self::Glyph(value) => &value.source,
            Self::Path(value) => &value.paint_source,
            Self::Image(value) => &value.source,
            Self::Form(value) => &value.source,
            Self::Transform { source, .. }
            | Self::State { source, .. }
            | Self::Raw { source, .. } => source,
        }
    }

    pub fn bounds(&self) -> Option<Rect> {
        match self {
            Self::Glyph(value) => Some(value.bounds),
            Self::Path(value) => Some(value.bounds),
            Self::Image(value) => Some(value.bounds),
            Self::Form(value) => value.bounds,
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct DisplayList {
    pub page: PageIndex,
    pub page_object: ObjRef,
    pub media_box: Rect,
    pub crop_box: Rect,
    pub rotation: i32,
    pub content_streams: Vec<ContentStreamRecord>,
    pub operators: Vec<OperatorRecord>,
    pub primitives: Vec<(PrimitiveId, DisplayPrimitive)>,
    pub fonts: HashMap<String, FontResource>,
}

impl DisplayList {
    pub fn primitive(&self, id: PrimitiveId) -> Option<&DisplayPrimitive> {
        self.primitives
            .get(id.0.checked_sub(1)? as usize)
            .map(|(_, primitive)| primitive)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DisplayListOptions {
    pub max_form_depth: usize,
    pub max_operators_per_page: usize,
}

impl Default for DisplayListOptions {
    fn default() -> Self {
        Self {
            max_form_depth: 12,
            max_operators_per_page: 2_000_000,
        }
    }
}

pub(crate) fn page_object_refs(doc: &mut PdfDocument<'_>) -> PdfResult<Vec<ObjRef>> {
    let root = doc
        .trailer()
        .get_ref("Root")
        .ok_or_else(|| makepad_pdf_parse::PdfError::new("trailer missing /Root"))?;
    let catalog = doc.resolve_ref(root)?;
    let pages = catalog
        .as_dict()
        .and_then(|dict| dict.get("Pages"))
        .cloned()
        .ok_or_else(|| makepad_pdf_parse::PdfError::new("catalog missing /Pages"))?;
    let mut output = Vec::new();
    collect_page_refs(doc, &pages, &mut output, 0)?;
    Ok(output)
}

fn collect_page_refs(
    doc: &mut PdfDocument<'_>,
    object: &PdfObj,
    output: &mut Vec<ObjRef>,
    depth: usize,
) -> PdfResult<()> {
    if depth > 128 {
        return Err(makepad_pdf_parse::PdfError::new(
            "page tree exceeded 128 levels",
        ));
    }
    let reference = object.as_ref();
    let resolved = doc.resolve(object)?;
    let dict = resolved
        .as_dict()
        .ok_or_else(|| makepad_pdf_parse::PdfError::new("page tree node is not a dictionary"))?;
    match dict.get_name("Type").unwrap_or("") {
        "Pages" => {
            let children = dict
                .get_array("Kids")
                .ok_or_else(|| makepad_pdf_parse::PdfError::new("/Pages missing /Kids"))?
                .to_vec();
            for child in &children {
                collect_page_refs(doc, child, output, depth + 1)?;
            }
        }
        "Page" | "" => {
            let reference = reference.ok_or_else(|| {
                makepad_pdf_parse::PdfError::new("direct page objects have no stable provenance")
            })?;
            output.push(reference);
        }
        other => {
            return Err(makepad_pdf_parse::PdfError::new(format!(
                "unexpected page-tree node type {other}"
            )))
        }
    }
    Ok(())
}

pub(crate) fn build_display_list(
    doc: &mut PdfDocument<'_>,
    page_index: usize,
    options: DisplayListOptions,
) -> PdfResult<DisplayList> {
    let refs = page_object_refs(doc)?;
    let page_ref = *refs
        .get(page_index)
        .ok_or_else(|| makepad_pdf_parse::PdfError::new("page index out of range"))?;
    let mut page = doc.page(page_index)?;
    enrich_to_unicode_maps(doc, page_ref, &mut page.fonts)?;
    let page_object = doc.resolve_ref(page_ref)?;
    let page_dict = page_object
        .as_dict()
        .ok_or_else(|| makepad_pdf_parse::PdfError::new("page is not a dictionary"))?;
    let streams = content_streams(doc, page_dict, page_ref)?;
    let raw_media_box = array_rect(page.media_box);
    let raw_crop_box = array_rect(page.crop_box);
    let page_transform = page_rotation_transform(page.rotate, raw_crop_box);
    let media_box = transformed_rect(page_transform, raw_media_box);
    let crop_box = transformed_rect(page_transform, raw_crop_box);
    let mut output = DisplayList {
        page: PageIndex(page_index as u32),
        page_object: page_ref,
        media_box,
        crop_box,
        rotation: page.rotate,
        content_streams: streams,
        operators: Vec::new(),
        primitives: Vec::new(),
        fonts: page.fonts.clone(),
    };
    let resources = ResourceContext {
        fonts: page.fonts,
        xobjects: page.xobjects,
    };
    let mut interpreter = Interpreter::new(doc, &mut output, options, resources, page_transform);
    let top_streams = interpreter.output.content_streams.clone();
    for stream in &top_streams {
        interpreter.interpret_stream(
            &stream.decoded,
            stream.object,
            stream.stream_index,
            Vec::new(),
            0,
        )?;
    }
    Ok(output)
}

/// `pdf_parse` handles the common line-oriented ToUnicode form, but real
/// generators also put the destinations of a `bfrange` in an array.  Keep
/// that compatibility here until the lower-level crate exposes the complete
/// CMap parser; this crate deliberately does not need to modify it.
fn enrich_to_unicode_maps(
    doc: &mut PdfDocument<'_>,
    page_ref: ObjRef,
    fonts: &mut HashMap<String, FontResource>,
) -> PdfResult<()> {
    let Some(resources_object) = inherited_resources(doc, page_ref)? else {
        return Ok(());
    };
    let resources = doc.resolve(&resources_object)?;
    let Some(fonts_object) = resources.as_dict().and_then(|dict| dict.get("Font")).cloned()
    else {
        return Ok(());
    };
    let font_dictionary = doc.resolve(&fonts_object)?;
    let Some(font_dictionary) = font_dictionary.as_dict() else {
        return Ok(());
    };
    let entries = font_dictionary.map.clone();
    for (resource_name, font_object) in entries {
        let Some(font) = fonts.get_mut(&resource_name) else {
            continue;
        };
        let Ok(resolved_font) = doc.resolve(&font_object) else {
            continue;
        };
        let Some(to_unicode) = resolved_font
            .as_dict()
            .and_then(|dict| dict.get("ToUnicode"))
            .cloned()
        else {
            continue;
        };
        let Ok(decoded) = doc.resolve_stream(&to_unicode) else {
            continue;
        };
        let mappings = parse_complete_to_unicode(&decoded);
        if mappings.len()
            > font
                .to_unicode
                .as_ref()
                .map_or(0, |existing| existing.mappings.len())
        {
            font.to_unicode = Some(CMapData { mappings });
        }
    }
    Ok(())
}

fn inherited_resources(
    doc: &mut PdfDocument<'_>,
    page_ref: ObjRef,
) -> PdfResult<Option<PdfObj>> {
    let mut current = PdfObj::Ref(page_ref);
    for _ in 0..128 {
        let resolved = doc.resolve(&current)?;
        let Some(dictionary) = resolved.as_dict() else {
            return Ok(None);
        };
        if let Some(resources) = dictionary.get("Resources") {
            return Ok(Some(resources.clone()));
        }
        let Some(parent) = dictionary.get("Parent") else {
            return Ok(None);
        };
        current = parent.clone();
    }
    Err(makepad_pdf_parse::PdfError::new(
        "page inheritance exceeded 128 levels",
    ))
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum CMapToken {
    Hex(Vec<u8>),
    Word(String),
    ArrayStart,
    ArrayEnd,
}

fn parse_complete_to_unicode(data: &[u8]) -> HashMap<u32, String> {
    let tokens = cmap_tokens(data);
    let mut mappings = HashMap::new();
    let mut cursor = 0;
    while cursor < tokens.len() {
        match tokens.get(cursor) {
            Some(CMapToken::Word(word)) if word == "beginbfchar" => {
                cursor += 1;
                while !matches!(tokens.get(cursor), Some(CMapToken::Word(word)) if word == "endbfchar")
                {
                    let Some(CMapToken::Hex(source)) = tokens.get(cursor) else {
                        cursor += 1;
                        continue;
                    };
                    let Some(CMapToken::Hex(destination)) = tokens.get(cursor + 1) else {
                        cursor += 1;
                        continue;
                    };
                    if let (Some(code), Some(unicode)) =
                        (hex_code(source), utf16be_string(destination))
                    {
                        mappings.insert(code, unicode);
                    }
                    cursor += 2;
                }
            }
            Some(CMapToken::Word(word)) if word == "beginbfrange" => {
                cursor += 1;
                while !matches!(tokens.get(cursor), Some(CMapToken::Word(word)) if word == "endbfrange")
                {
                    let (Some(CMapToken::Hex(first)), Some(CMapToken::Hex(last))) =
                        (tokens.get(cursor), tokens.get(cursor + 1))
                    else {
                        cursor += 1;
                        continue;
                    };
                    let (Some(first), Some(last)) = (hex_code(first), hex_code(last)) else {
                        cursor += 2;
                        continue;
                    };
                    cursor += 2;
                    match tokens.get(cursor) {
                        Some(CMapToken::Hex(destination)) => {
                            if let Some(mut value) = utf16be_scalar(destination) {
                                for code in first..=last {
                                    if let Some(character) = char::from_u32(value) {
                                        mappings.insert(code, character.to_string());
                                    }
                                    value += 1;
                                }
                            }
                            cursor += 1;
                        }
                        Some(CMapToken::ArrayStart) => {
                            cursor += 1;
                            for code in first..=last {
                                let Some(CMapToken::Hex(destination)) = tokens.get(cursor) else {
                                    break;
                                };
                                if let Some(unicode) = utf16be_string(destination) {
                                    mappings.insert(code, unicode);
                                }
                                cursor += 1;
                            }
                            while !matches!(tokens.get(cursor), None | Some(CMapToken::ArrayEnd)) {
                                cursor += 1;
                            }
                            cursor += usize::from(cursor < tokens.len());
                        }
                        _ => {}
                    }
                }
            }
            _ => cursor += 1,
        }
    }
    mappings
}

fn cmap_tokens(data: &[u8]) -> Vec<CMapToken> {
    let mut output = Vec::new();
    let mut cursor = 0;
    while cursor < data.len() {
        match data[cursor] {
            b'%' => {
                while cursor < data.len() && !matches!(data[cursor], b'\r' | b'\n') {
                    cursor += 1;
                }
            }
            byte if byte.is_ascii_whitespace() => cursor += 1,
            b'[' => {
                output.push(CMapToken::ArrayStart);
                cursor += 1;
            }
            b']' => {
                output.push(CMapToken::ArrayEnd);
                cursor += 1;
            }
            b'<' if data.get(cursor + 1) != Some(&b'<') => {
                cursor += 1;
                let mut nibbles = Vec::new();
                while cursor < data.len() && data[cursor] != b'>' {
                    if data[cursor].is_ascii_hexdigit() {
                        nibbles.push(data[cursor]);
                    }
                    cursor += 1;
                }
                cursor += usize::from(cursor < data.len());
                if nibbles.len() % 2 == 1 {
                    nibbles.push(b'0');
                }
                let bytes = nibbles
                    .chunks_exact(2)
                    .filter_map(|pair| {
                        let text = std::str::from_utf8(pair).ok()?;
                        u8::from_str_radix(text, 16).ok()
                    })
                    .collect();
                output.push(CMapToken::Hex(bytes));
            }
            b'<' => cursor += 2,
            b'>' if data.get(cursor + 1) == Some(&b'>') => cursor += 2,
            _ => {
                let start = cursor;
                while cursor < data.len()
                    && !data[cursor].is_ascii_whitespace()
                    && !matches!(data[cursor], b'[' | b']' | b'<' | b'>')
                {
                    cursor += 1;
                }
                if start != cursor {
                    output.push(CMapToken::Word(
                        String::from_utf8_lossy(&data[start..cursor]).into_owned(),
                    ));
                } else {
                    cursor += 1;
                }
            }
        }
    }
    output
}

fn hex_code(bytes: &[u8]) -> Option<u32> {
    (bytes.len() <= 4).then(|| {
        bytes
            .iter()
            .fold(0_u32, |value, byte| (value << 8) | u32::from(*byte))
    })
}

fn utf16be_scalar(bytes: &[u8]) -> Option<u32> {
    let string = utf16be_string(bytes)?;
    let mut chars = string.chars();
    let first = chars.next()?;
    chars.next().is_none().then_some(first as u32)
}

fn utf16be_string(bytes: &[u8]) -> Option<String> {
    if bytes.len() % 2 != 0 {
        return None;
    }
    let units = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_be_bytes([pair[0], pair[1]]));
    char::decode_utf16(units).collect::<Result<String, _>>().ok()
}

fn array_rect(values: [f64; 4]) -> Rect {
    Rect::new(values[0], values[1], values[2], values[3])
}

fn content_streams(
    doc: &mut PdfDocument<'_>,
    page: &PdfDict,
    page_ref: ObjRef,
) -> PdfResult<Vec<ContentStreamRecord>> {
    let Some(contents) = page.get("Contents") else {
        return Ok(Vec::new());
    };
    let resolved = doc.resolve(contents)?;
    let objects = match resolved {
        PdfObj::Array(values) => values,
        value => vec![match contents {
            PdfObj::Ref(reference) => PdfObj::Ref(*reference),
            _ => value,
        }],
    };
    let mut output = Vec::new();
    for (index, item) in objects.iter().enumerate() {
        let object_ref = item.as_ref().unwrap_or(page_ref);
        let resolved = doc.resolve(item)?;
        if let PdfObj::Stream(stream) = resolved {
            output.push(ContentStreamRecord {
                object: object_ref,
                stream_index: index as u16,
                decoded: doc.decode_stream(&stream)?,
            });
        }
    }
    Ok(output)
}

#[derive(Clone)]
struct ResourceContext {
    fonts: HashMap<String, FontResource>,
    xobjects: HashMap<String, XObjectResource>,
}

#[derive(Clone, Debug)]
struct TextState {
    font: Option<String>,
    font_size: f64,
    matrix: Affine,
    line_matrix: Affine,
    char_spacing: f64,
    word_spacing: f64,
    leading: f64,
    rise: f64,
    horizontal_scale: f64,
}

impl Default for TextState {
    fn default() -> Self {
        Self {
            font: None,
            font_size: 0.0,
            matrix: Affine::IDENTITY,
            line_matrix: Affine::IDENTITY,
            char_spacing: 0.0,
            word_spacing: 0.0,
            leading: 0.0,
            rise: 0.0,
            horizontal_scale: 1.0,
        }
    }
}

#[derive(Clone, Debug)]
struct GraphicsState {
    ctm: Affine,
    line_width: f64,
    stroke_color: Color,
    fill_color: Color,
    text: TextState,
}

impl Default for GraphicsState {
    fn default() -> Self {
        Self {
            ctm: Affine::IDENTITY,
            line_width: 1.0,
            stroke_color: Color::default(),
            fill_color: Color::default(),
            text: TextState::default(),
        }
    }
}

#[derive(Clone, Debug, Default)]
struct BuildingSubpath {
    commands: Vec<PathCommand>,
    sources: Vec<SourceSpan>,
}

#[derive(Clone, Debug, Default)]
struct BuildingPath {
    subpaths: Vec<BuildingSubpath>,
    clip: Option<ClipRule>,
}

impl BuildingPath {
    fn current_mut(&mut self) -> &mut BuildingSubpath {
        if self.subpaths.is_empty() {
            self.subpaths.push(BuildingSubpath::default());
        }
        self.subpaths.last_mut().expect("subpath was inserted")
    }

    fn move_to(&mut self, point: Point, source: SourceSpan) {
        self.subpaths.push(BuildingSubpath {
            commands: vec![PathCommand::Move(point)],
            sources: vec![source],
        });
    }

    fn current_point(&self) -> Option<Point> {
        self.subpaths.last()?.commands.iter().rev().find_map(|command| match command {
            PathCommand::Move(point) | PathCommand::Line(point) => Some(*point),
            PathCommand::Cubic(_, _, point) => Some(*point),
            PathCommand::Close => None,
        })
    }
}

struct Interpreter<'a, 'b> {
    doc: &'a mut PdfDocument<'b>,
    output: &'a mut DisplayList,
    options: DisplayListOptions,
    resources: ResourceContext,
    state: GraphicsState,
    stack: Vec<GraphicsState>,
    path: BuildingPath,
    active_forms: HashSet<ObjRef>,
}

impl<'a, 'b> Interpreter<'a, 'b> {
    fn new(
        doc: &'a mut PdfDocument<'b>,
        output: &'a mut DisplayList,
        options: DisplayListOptions,
        resources: ResourceContext,
        page_transform: Affine,
    ) -> Self {
        Self {
            doc,
            output,
            options,
            resources,
            state: GraphicsState {
                ctm: page_transform,
                ..GraphicsState::default()
            },
            stack: Vec::new(),
            path: BuildingPath::default(),
            active_forms: HashSet::new(),
        }
    }

    fn push(&mut self, primitive: DisplayPrimitive) -> PrimitiveId {
        let id = PrimitiveId(self.output.primitives.len() as u64 + 1);
        self.output.primitives.push((id, primitive));
        id
    }

    fn interpret_stream(
        &mut self,
        data: &[u8],
        object: ObjRef,
        stream_index: u16,
        form_chain: Vec<FormHop>,
        depth: usize,
    ) -> PdfResult<()> {
        let ranges = operator_ranges(data);
        if self.output.operators.len() + ranges.len() > self.options.max_operators_per_page {
            return Err(makepad_pdf_parse::PdfError::new(
                "page exceeded configured operator limit",
            ));
        }
        for range in ranges {
            let raw = data[range.clone()].to_vec();
            let parsed = parse_content_stream(&raw)?;
            let operation = if parsed.len() == 1 {
                RetainedOperator::Parsed(parsed[0].clone())
            } else {
                RetainedOperator::Raw {
                    keyword: last_keyword(&raw),
                }
            };
            let source = SourceSpan {
                object,
                stream_index,
                decoded_bytes: range.start as u32..range.end as u32,
                operator_index: self.output.operators.len() as u32,
                subpath_index: None,
                form_chain: form_chain.clone(),
            };
            self.output.operators.push(OperatorRecord {
                source: source.clone(),
                raw: raw.clone(),
                operation: operation.clone(),
            });
            match operation {
                RetainedOperator::Parsed(op) => {
                    self.interpret_operation(op, source, &form_chain, depth)?
                }
                RetainedOperator::Raw { .. } => {
                    self.push(DisplayPrimitive::Raw { bytes: raw, source });
                }
            }
        }
        Ok(())
    }

    fn interpret_operation(
        &mut self,
        operation: PdfOp,
        source: SourceSpan,
        form_chain: &[FormHop],
        depth: usize,
    ) -> PdfResult<()> {
        match operation {
            PdfOp::SaveState => {
                self.stack.push(self.state.clone());
                self.push_state(PdfOp::SaveState, source);
            }
            PdfOp::RestoreState => {
                if let Some(state) = self.stack.pop() {
                    self.state = state;
                }
                self.push_state(PdfOp::RestoreState, source);
            }
            PdfOp::ConcatMatrix(values) => {
                let matrix = Affine::new(values);
                self.state.ctm = self.state.ctm.then(matrix);
                self.push(DisplayPrimitive::Transform { matrix, source });
            }
            PdfOp::SetLineWidth(value) => {
                self.state.line_width = value;
                self.push_state(PdfOp::SetLineWidth(value), source);
            }
            PdfOp::SetStrokeGray(value) => {
                self.state.stroke_color = Color::Gray(value);
                self.push_state(PdfOp::SetStrokeGray(value), source);
            }
            PdfOp::SetFillGray(value) => {
                self.state.fill_color = Color::Gray(value);
                self.push_state(PdfOp::SetFillGray(value), source);
            }
            PdfOp::SetStrokeRgb(r, g, b) => {
                self.state.stroke_color = Color::Rgb(r, g, b);
                self.push_state(PdfOp::SetStrokeRgb(r, g, b), source);
            }
            PdfOp::SetFillRgb(r, g, b) => {
                self.state.fill_color = Color::Rgb(r, g, b);
                self.push_state(PdfOp::SetFillRgb(r, g, b), source);
            }
            PdfOp::SetStrokeCmyk(c, m, y, k) => {
                self.state.stroke_color = Color::Cmyk(c, m, y, k);
                self.push_state(PdfOp::SetStrokeCmyk(c, m, y, k), source);
            }
            PdfOp::SetFillCmyk(c, m, y, k) => {
                self.state.fill_color = Color::Cmyk(c, m, y, k);
                self.push_state(PdfOp::SetFillCmyk(c, m, y, k), source);
            }
            PdfOp::SetStrokeColor(values) => {
                self.state.stroke_color = component_color(&values);
                self.push_state(PdfOp::SetStrokeColor(values), source);
            }
            PdfOp::SetFillColor(values) => {
                self.state.fill_color = component_color(&values);
                self.push_state(PdfOp::SetFillColor(values), source);
            }
            PdfOp::MoveTo(x, y) => {
                self.path
                    .move_to(self.state.ctm.transform_point(Point::new(x, y)), source);
            }
            PdfOp::LineTo(x, y) => {
                let point = self.state.ctm.transform_point(Point::new(x, y));
                let current = self.path.current_mut();
                current.commands.push(PathCommand::Line(point));
                current.sources.push(source);
            }
            PdfOp::CurveTo(x1, y1, x2, y2, x3, y3) => {
                let command = PathCommand::Cubic(
                    self.state.ctm.transform_point(Point::new(x1, y1)),
                    self.state.ctm.transform_point(Point::new(x2, y2)),
                    self.state.ctm.transform_point(Point::new(x3, y3)),
                );
                let current = self.path.current_mut();
                current.commands.push(command);
                current.sources.push(source);
            }
            PdfOp::CurveToV(x2, y2, x3, y3) => {
                let first = self.path.current_point().unwrap_or_default();
                let command = PathCommand::Cubic(
                    first,
                    self.state.ctm.transform_point(Point::new(x2, y2)),
                    self.state.ctm.transform_point(Point::new(x3, y3)),
                );
                let current = self.path.current_mut();
                current.commands.push(command);
                current.sources.push(source);
            }
            PdfOp::CurveToY(x1, y1, x3, y3) => {
                let final_point = self.state.ctm.transform_point(Point::new(x3, y3));
                let command = PathCommand::Cubic(
                    self.state.ctm.transform_point(Point::new(x1, y1)),
                    final_point,
                    final_point,
                );
                let current = self.path.current_mut();
                current.commands.push(command);
                current.sources.push(source);
            }
            PdfOp::ClosePath => {
                let current = self.path.current_mut();
                current.commands.push(PathCommand::Close);
                current.sources.push(source);
            }
            PdfOp::Rectangle(x, y, width, height) => {
                let points = [
                    Point::new(x, y),
                    Point::new(x + width, y),
                    Point::new(x + width, y + height),
                    Point::new(x, y + height),
                ]
                .map(|point| self.state.ctm.transform_point(point));
                self.path.subpaths.push(BuildingSubpath {
                    commands: vec![
                        PathCommand::Move(points[0]),
                        PathCommand::Line(points[1]),
                        PathCommand::Line(points[2]),
                        PathCommand::Line(points[3]),
                        PathCommand::Close,
                    ],
                    sources: vec![source.clone(); 5],
                });
            }
            PdfOp::Clip => {
                self.path.clip = Some(ClipRule::NonZero);
                self.push_state(PdfOp::Clip, source);
            }
            PdfOp::ClipEvenOdd => {
                self.path.clip = Some(ClipRule::EvenOdd);
                self.push_state(PdfOp::ClipEvenOdd, source);
            }
            PdfOp::Stroke => self.paint_path(PathPaint::Stroke, source),
            PdfOp::CloseStroke => {
                self.close_current_path(source.clone());
                self.paint_path(PathPaint::Stroke, source);
            }
            PdfOp::Fill => self.paint_path(PathPaint::Fill, source),
            PdfOp::FillEvenOdd => self.paint_path(PathPaint::FillEvenOdd, source),
            PdfOp::FillStroke => self.paint_path(PathPaint::FillStroke, source),
            PdfOp::FillStrokeEvenOdd => self.paint_path(PathPaint::FillStrokeEvenOdd, source),
            PdfOp::CloseFillStroke => {
                self.close_current_path(source.clone());
                self.paint_path(PathPaint::FillStroke, source);
            }
            PdfOp::CloseFillStrokeEvenOdd => {
                self.close_current_path(source.clone());
                self.paint_path(PathPaint::FillStrokeEvenOdd, source);
            }
            PdfOp::EndPath => self.paint_path(PathPaint::None, source),
            PdfOp::BeginText => {
                self.state.text.matrix = Affine::IDENTITY;
                self.state.text.line_matrix = Affine::IDENTITY;
                self.push_state(PdfOp::BeginText, source);
            }
            PdfOp::EndText => self.push_state(PdfOp::EndText, source),
            PdfOp::SetFont(name, size) => {
                self.state.text.font = Some(name.clone());
                self.state.text.font_size = size;
                self.push_state(PdfOp::SetFont(name, size), source);
            }
            PdfOp::SetTextMatrix(values) => {
                let matrix = Affine::new(values);
                self.state.text.matrix = matrix;
                self.state.text.line_matrix = matrix;
                self.push_state(PdfOp::SetTextMatrix(values), source);
            }
            PdfOp::MoveText(x, y) => {
                self.move_text(x, y);
                self.push_state(PdfOp::MoveText(x, y), source);
            }
            PdfOp::MoveTextSetLeading(x, y) => {
                self.state.text.leading = -y;
                self.move_text(x, y);
                self.push_state(PdfOp::MoveTextSetLeading(x, y), source);
            }
            PdfOp::NextLine => {
                self.move_text(0.0, -self.state.text.leading);
                self.push_state(PdfOp::NextLine, source);
            }
            PdfOp::SetCharSpacing(value) => {
                self.state.text.char_spacing = value;
                self.push_state(PdfOp::SetCharSpacing(value), source);
            }
            PdfOp::SetWordSpacing(value) => {
                self.state.text.word_spacing = value;
                self.push_state(PdfOp::SetWordSpacing(value), source);
            }
            PdfOp::SetTextLeading(value) => {
                self.state.text.leading = value;
                self.push_state(PdfOp::SetTextLeading(value), source);
            }
            PdfOp::SetTextRise(value) => {
                self.state.text.rise = value;
                self.push_state(PdfOp::SetTextRise(value), source);
            }
            PdfOp::SetHorizScaling(value) => {
                self.state.text.horizontal_scale = value / 100.0;
                self.push_state(PdfOp::SetHorizScaling(value), source);
            }
            PdfOp::ShowText(bytes) => self.show_text(&bytes, source),
            PdfOp::ShowTextArray(items) => {
                for item in items {
                    match item {
                        TextArrayItem::Text(bytes) => self.show_text(&bytes, source.clone()),
                        TextArrayItem::Adjustment(value) => {
                            let advance = -value / 1000.0
                                * self.state.text.font_size
                                * self.state.text.horizontal_scale;
                            self.advance_text(advance);
                        }
                    }
                }
            }
            PdfOp::ShowTextNextLine(bytes) => {
                self.move_text(0.0, -self.state.text.leading);
                self.show_text(&bytes, source);
            }
            PdfOp::ShowTextNextLineSpacing(word, character, bytes) => {
                self.state.text.word_spacing = word;
                self.state.text.char_spacing = character;
                self.move_text(0.0, -self.state.text.leading);
                self.show_text(&bytes, source);
            }
            PdfOp::PaintXObject(name) => {
                self.paint_xobject(name, source, form_chain, depth)?;
            }
            PdfOp::InlineImage { dict, data } => {
                let width = dict.get_int("Width").or_else(|| dict.get_int("W"));
                let height = dict.get_int("Height").or_else(|| dict.get_int("H"));
                self.push(DisplayPrimitive::Image(PdfImageRef {
                    name: None,
                    object: None,
                    pixel_size: width.zip(height).and_then(|(width, height)| {
                        Some((u32::try_from(width).ok()?, u32::try_from(height).ok()?))
                    }),
                    bounds: self.state.ctm.unit_square_bounds(),
                    encoded_bytes: data.len(),
                    source,
                }));
            }
            other => self.push_state(other, source),
        }
        Ok(())
    }

    fn push_state(&mut self, operation: PdfOp, source: SourceSpan) {
        self.push(DisplayPrimitive::State { operation, source });
    }

    fn close_current_path(&mut self, source: SourceSpan) {
        let current = self.path.current_mut();
        current.commands.push(PathCommand::Close);
        current.sources.push(source);
    }

    fn paint_path(&mut self, paint: PathPaint, source: SourceSpan) {
        let path = std::mem::take(&mut self.path);
        for (index, subpath) in path.subpaths.into_iter().enumerate() {
            if subpath.commands.is_empty() {
                continue;
            }
            let mut paint_source = source.clone();
            paint_source.subpath_index = Some(index as u16);
            let points = command_points(&subpath.commands);
            let Some(path_bounds) = bounds(&points) else {
                continue;
            };
            self.push(DisplayPrimitive::Path(PdfPath {
                commands: subpath.commands,
                bounds: path_bounds,
                paint,
                clip: path.clip,
                line_width: self.state.line_width * self.state.ctm.scale_magnitude(),
                stroke_color: self.state.stroke_color,
                fill_color: self.state.fill_color,
                command_sources: subpath.sources,
                paint_source,
            }));
        }
    }

    fn move_text(&mut self, x: f64, y: f64) {
        let translation = Affine::new([1.0, 0.0, 0.0, 1.0, x, y]);
        self.state.text.line_matrix = self.state.text.line_matrix.then(translation);
        self.state.text.matrix = self.state.text.line_matrix;
    }

    fn advance_text(&mut self, advance: f64) {
        self.state.text.matrix = self
            .state
            .text
            .matrix
            .then(Affine::new([1.0, 0.0, 0.0, 1.0, advance, 0.0]));
    }

    fn show_text(&mut self, bytes: &[u8], source: SourceSpan) {
        let Some(font_name) = self.state.text.font.clone() else {
            return;
        };
        let Some(font) = self.resources.fonts.get(&font_name).cloned() else {
            return;
        };
        for (code, unicode) in decode_codes(&font, bytes) {
            let word = if code == 32 {
                self.state.text.word_spacing
            } else {
                0.0
            };
            let advance = (char_width(&font, code) / 1000.0 * self.state.text.font_size
                + self.state.text.char_spacing
                + word)
                * self.state.text.horizontal_scale;
            let text_scale = self.state.text.font_size * self.state.text.horizontal_scale;
            let invisible_advance_1000 = (text_scale.abs() > 1e-12)
                .then_some(-advance / text_scale * 1000.0);
            let text_render = self.state.ctm.then(self.state.text.matrix);
            let origin = text_render.transform_point(Point::new(0.0, self.state.text.rise));
            let glyph_matrix = text_render.then(Affine::new([
                self.state.text.font_size * self.state.text.horizontal_scale,
                0.0,
                0.0,
                self.state.text.font_size,
                0.0,
                self.state.text.rise,
            ]));
            let glyph_bounds = bounds(&[
                glyph_matrix.transform_point(Point::new(0.0, -0.25)),
                glyph_matrix.transform_point(Point::new(advance.max(0.1) / self.state.text.font_size.max(0.1), -0.25)),
                glyph_matrix.transform_point(Point::new(0.0, 0.85)),
                glyph_matrix.transform_point(Point::new(advance.max(0.1) / self.state.text.font_size.max(0.1), 0.85)),
            ])
            .unwrap_or(Rect::new(origin.x, origin.y, origin.x, origin.y));
            let raw_name = raw_glyph_name(&font, code);
            self.push(DisplayPrimitive::Glyph(PdfGlyph {
                font_resource: font_name.clone(),
                font_base_name: font.base_font.clone(),
                code,
                raw_name,
                unicode: (!unicode.is_empty()).then_some(unicode),
                origin,
                text_render_matrix: glyph_matrix,
                bounds: glyph_bounds,
                advance,
                invisible_advance_1000,
                source: source.clone(),
            }));
            self.advance_text(advance);
        }
    }

    fn paint_xobject(
        &mut self,
        name: String,
        source: SourceSpan,
        form_chain: &[FormHop],
        depth: usize,
    ) -> PdfResult<()> {
        let Some(resource) = self.resources.xobjects.get(&name).cloned() else {
            self.push(DisplayPrimitive::Raw {
                bytes: format!("/{name} Do").into_bytes(),
                source,
            });
            return Ok(());
        };
        let resolved = self.doc.resolve_ref(resource.obj_ref)?;
        let Some(stream) = resolved.as_stream() else {
            return Ok(());
        };
        if resource.subtype == "Image" {
            let width = stream.dict.get_int("Width");
            let height = stream.dict.get_int("Height");
            self.push(DisplayPrimitive::Image(PdfImageRef {
                name: Some(name),
                object: Some(resource.obj_ref),
                pixel_size: width.zip(height).and_then(|(width, height)| {
                    Some((u32::try_from(width).ok()?, u32::try_from(height).ok()?))
                }),
                bounds: self.state.ctm.unit_square_bounds(),
                encoded_bytes: stream.data.len(),
                source,
            }));
            return Ok(());
        }
        if resource.subtype != "Form" {
            return Ok(());
        }
        let matrix = stream
            .dict
            .get_array("Matrix")
            .and_then(matrix_from_array)
            .unwrap_or(Affine::IDENTITY);
        let form_bounds = stream
            .dict
            .get_array("BBox")
            .and_then(rect_from_array)
            .map(|rect| transformed_rect(self.state.ctm.then(matrix), rect));
        let can_expand = depth < self.options.max_form_depth
            && !self.active_forms.contains(&resource.obj_ref);
        self.push(DisplayPrimitive::Form(FormInvocation {
            name: name.clone(),
            object: resource.obj_ref,
            matrix,
            bounds: form_bounds,
            source: source.clone(),
            expanded: can_expand,
        }));
        if !can_expand {
            return Ok(());
        }
        let decoded = self.doc.decode_stream(stream)?;
        let old_state = self.state.clone();
        let old_resources = self.resources.clone();
        self.state.ctm = self.state.ctm.then(matrix);
        self.resources.xobjects = form_xobjects(self.doc, &stream.dict, &old_resources.xobjects)?;
        self.active_forms.insert(resource.obj_ref);
        let mut chain = form_chain.to_vec();
        chain.push(FormHop {
            name,
            object: resource.obj_ref,
            invocation_operator: source.operator_index,
        });
        let result = self.interpret_stream(&decoded, resource.obj_ref, 0, chain, depth + 1);
        self.active_forms.remove(&resource.obj_ref);
        self.resources = old_resources;
        self.state = old_state;
        result
    }
}

fn form_xobjects(
    doc: &mut PdfDocument<'_>,
    form: &PdfDict,
    inherited: &HashMap<String, XObjectResource>,
) -> PdfResult<HashMap<String, XObjectResource>> {
    let Some(resources) = form.get("Resources") else {
        return Ok(inherited.clone());
    };
    let resources = doc.resolve(resources)?;
    let Some(xobjects) = resources
        .as_dict()
        .and_then(|dict| dict.get("XObject"))
        .cloned()
    else {
        return Ok(HashMap::new());
    };
    let xobjects = doc.resolve(&xobjects)?;
    let Some(dict) = xobjects.as_dict() else {
        return Ok(HashMap::new());
    };
    let mut output = HashMap::new();
    for (name, value) in &dict.map {
        let Some(object) = value.as_ref() else {
            continue;
        };
        let resolved = doc.resolve(value)?;
        let subtype = resolved
            .as_stream()
            .and_then(|stream| stream.dict.get_name("Subtype"))
            .unwrap_or("")
            .to_string();
        output.insert(
            name.clone(),
            XObjectResource {
                subtype,
                obj_ref: object,
            },
        );
    }
    Ok(output)
}

fn raw_glyph_name(font: &FontResource, code: u32) -> Option<String> {
    match &font.encoding {
        FontEncoding::Custom(_, names) => names.get(&(code as u8)).cloned(),
        _ => None,
    }
}

fn component_color(values: &[f64]) -> Color {
    let mut components = [0.0; 4];
    let len = values.len().min(4);
    components[..len].copy_from_slice(&values[..len]);
    Color::Components(components, len as u8)
}

fn command_points(commands: &[PathCommand]) -> Vec<Point> {
    let mut output = Vec::new();
    for command in commands {
        match command {
            PathCommand::Move(point) | PathCommand::Line(point) => output.push(*point),
            PathCommand::Cubic(first, second, third) => output.extend([*first, *second, *third]),
            PathCommand::Close => {}
        }
    }
    output
}

fn matrix_from_array(values: &[PdfObj]) -> Option<Affine> {
    if values.len() < 6 {
        return None;
    }
    let mut output = [0.0; 6];
    for (target, source) in output.iter_mut().zip(values) {
        *target = source.as_f64()?;
    }
    Some(Affine::new(output))
}

fn rect_from_array(values: &[PdfObj]) -> Option<Rect> {
    if values.len() < 4 {
        return None;
    }
    Some(Rect::new(
        values[0].as_f64()?,
        values[1].as_f64()?,
        values[2].as_f64()?,
        values[3].as_f64()?,
    ))
}

fn transformed_rect(matrix: Affine, rect: Rect) -> Rect {
    bounds(&[
        matrix.transform_point(Point::new(rect.min_x, rect.min_y)),
        matrix.transform_point(Point::new(rect.max_x, rect.min_y)),
        matrix.transform_point(Point::new(rect.min_x, rect.max_y)),
        matrix.transform_point(Point::new(rect.max_x, rect.max_y)),
    ])
    .unwrap_or_default()
}

fn page_rotation_transform(rotation: i32, crop: Rect) -> Affine {
    match rotation.rem_euclid(360) {
        90 => Affine::new([0.0, -1.0, 1.0, 0.0, -crop.min_y, crop.max_x]),
        180 => Affine::new([-1.0, 0.0, 0.0, -1.0, crop.max_x, crop.max_y]),
        270 => Affine::new([0.0, 1.0, -1.0, 0.0, crop.max_y, -crop.min_x]),
        _ => Affine::IDENTITY,
    }
}

/// Split a decoded content stream at top-level operator boundaries. PDF
/// objects (strings, hex strings, arrays and dictionaries) are skipped as a
/// unit, so binary/text payloads cannot masquerade as operators.
fn operator_ranges(data: &[u8]) -> Vec<Range<usize>> {
    let mut output = Vec::new();
    let mut cursor = 0;
    let mut command_start = None;
    while cursor < data.len() {
        skip_space_and_comments(data, &mut cursor);
        if cursor >= data.len() {
            break;
        }
        let start = cursor;
        command_start.get_or_insert(start);
        let first = data[cursor];
        if first == b'(' {
            skip_literal_string(data, &mut cursor);
            continue;
        }
        if first == b'[' {
            skip_balanced(data, &mut cursor, b'[', b']');
            continue;
        }
        if first == b'<' {
            if data.get(cursor + 1) == Some(&b'<') {
                skip_dictionary(data, &mut cursor);
            } else {
                cursor += 1;
                while cursor < data.len() && data[cursor] != b'>' {
                    cursor += 1;
                }
                cursor = (cursor + 1).min(data.len());
            }
            continue;
        }
        if first == b'/' {
            cursor += 1;
            while cursor < data.len() && !is_delimiter(data[cursor]) {
                cursor += 1;
            }
            continue;
        }
        while cursor < data.len() && !is_delimiter(data[cursor]) {
            cursor += 1;
        }
        if cursor == start {
            cursor += 1;
            continue;
        }
        let token = &data[start..cursor];
        if is_operand_keyword(token) || is_number(token) {
            continue;
        }
        if token == b"BI" {
            cursor = inline_image_end(data, cursor);
        }
        output.push(command_start.take().unwrap_or(start)..cursor);
    }
    if let Some(start) = command_start {
        output.push(start..data.len());
    }
    output
}

fn skip_space_and_comments(data: &[u8], cursor: &mut usize) {
    loop {
        while *cursor < data.len() && data[*cursor].is_ascii_whitespace() {
            *cursor += 1;
        }
        if data.get(*cursor) != Some(&b'%') {
            break;
        }
        while *cursor < data.len() && !matches!(data[*cursor], b'\n' | b'\r') {
            *cursor += 1;
        }
    }
}

fn skip_literal_string(data: &[u8], cursor: &mut usize) {
    let mut depth = 0_usize;
    while *cursor < data.len() {
        match data[*cursor] {
            b'\\' => *cursor = (*cursor + 2).min(data.len()),
            b'(' => {
                depth += 1;
                *cursor += 1;
            }
            b')' => {
                *cursor += 1;
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    break;
                }
            }
            _ => *cursor += 1,
        }
    }
}

fn skip_balanced(data: &[u8], cursor: &mut usize, open: u8, close: u8) {
    let mut depth = 0_usize;
    while *cursor < data.len() {
        match data[*cursor] {
            b'(' => skip_literal_string(data, cursor),
            byte if byte == open => {
                depth += 1;
                *cursor += 1;
            }
            byte if byte == close => {
                *cursor += 1;
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    break;
                }
            }
            _ => *cursor += 1,
        }
    }
}

fn skip_dictionary(data: &[u8], cursor: &mut usize) {
    let mut depth = 0_usize;
    while *cursor + 1 < data.len() {
        if data[*cursor] == b'(' {
            skip_literal_string(data, cursor);
        } else if &data[*cursor..*cursor + 2] == b"<<" {
            depth += 1;
            *cursor += 2;
        } else if &data[*cursor..*cursor + 2] == b">>" {
            depth = depth.saturating_sub(1);
            *cursor += 2;
            if depth == 0 {
                break;
            }
        } else {
            *cursor += 1;
        }
    }
}

fn inline_image_end(data: &[u8], after_bi: usize) -> usize {
    let mut cursor = after_bi;
    while cursor + 1 < data.len() {
        if data[cursor] == b'E'
            && data[cursor + 1] == b'I'
            && (cursor == 0 || data[cursor - 1].is_ascii_whitespace())
            && data
                .get(cursor + 2)
                .is_none_or(|value| value.is_ascii_whitespace())
        {
            return cursor + 2;
        }
        cursor += 1;
    }
    data.len()
}

fn is_delimiter(value: u8) -> bool {
    value.is_ascii_whitespace()
        || matches!(value, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

fn is_operand_keyword(value: &[u8]) -> bool {
    matches!(value, b"true" | b"false" | b"null")
}

fn is_number(value: &[u8]) -> bool {
    !value.is_empty()
        && value
            .iter()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'+' | b'-' | b'.'))
        && value.iter().any(u8::is_ascii_digit)
}

fn last_keyword(raw: &[u8]) -> String {
    raw.rsplit(|byte| byte.is_ascii_whitespace())
        .find(|token| !token.is_empty())
        .map(|token| String::from_utf8_lossy(token).into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizer_retains_unknown_and_nested_operands() {
        let data = b"q\n[(not an op) 20] TJ\n1 2 madeUp\nQ";
        let ranges = operator_ranges(data);
        assert_eq!(ranges.len(), 4);
        assert_eq!(&data[ranges[1].clone()], b"[(not an op) 20] TJ");
        assert_eq!(&data[ranges[2].clone()], b"1 2 madeUp");
    }

    #[test]
    fn indirect_lengths_are_materialized_without_moving_offsets() {
        let data = b"1 0 obj << /Length 2 0 R >> stream\nabc\nendstream\nendobj\n2 0 obj\n3\nendobj";
        let compatible = parser_compatible_bytes(data).unwrap();
        assert_eq!(compatible.len(), data.len());
        assert!(compatible.windows(b"/Length 3    ".len()).any(|window| window == b"/Length 3    "));
    }

    #[test]
    fn indirect_length_object_numbers_match_whole_tokens() {
        let data = b"293 0 obj\n<<>>\nendobj\n3 0 obj\n46\nendobj\n1 0 obj << /Length 3 0 R >> stream\nabc\nendstream\nendobj";
        let compatible = parser_compatible_bytes(data).unwrap();
        assert!(compatible
            .windows(b"/Length 46   ".len())
            .any(|window| window == b"/Length 46   "));
    }

    #[test]
    fn cmap_parser_accepts_array_bfranges_and_surrogates() {
        let cmap = br#"
            2 beginbfchar
            <0001> <E0A4>
            <0002> <D834DD1E>
            endbfchar
            1 beginbfrange
            <0003> <0004> [<E050> <E062>]
            endbfrange
        "#;
        let parsed = parse_complete_to_unicode(cmap);
        assert_eq!(parsed.get(&1).map(String::as_str), Some("\u{e0a4}"));
        assert_eq!(parsed.get(&2).map(String::as_str), Some("𝄞"));
        assert_eq!(parsed.get(&3).map(String::as_str), Some("\u{e050}"));
        assert_eq!(parsed.get(&4).map(String::as_str), Some("\u{e062}"));
    }
}
