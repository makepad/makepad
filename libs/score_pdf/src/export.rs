//! Incremental PDF export preserving the original byte prefix.

use crate::display::{page_object_refs, Color, PathCommand, PathPaint};
use crate::geometry::{Point, Rect};
use crate::provenance::canonical_provenance_json;
use crate::sha256::sha256;
use crate::splice::{ErasePlan, OperatorEdit, PaintCommand, SpliceWarning};
use crate::RecognizedDocument;
use makepad_pdf_parse::{ObjRef, PdfDict, PdfDocument, PdfObj};
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct PdfExportOptions {
    pub include_provenance: bool,
    pub fail_on_signatures: bool,
}

impl Default for PdfExportOptions {
    fn default() -> Self {
        Self {
            include_provenance: true,
            fail_on_signatures: true,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExportValidation {
    pub original_prefix_identical: bool,
    pub incremental_revision: bool,
    pub reparsed: bool,
    pub page_count: usize,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ExportedPdf {
    pub bytes: Vec<u8>,
    pub original_len: usize,
    pub appended_len: usize,
    pub provenance_json: Vec<u8>,
    pub created_object_hashes: Vec<(u32, [u8; 32])>,
    pub validation: ExportValidation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PdfExportError {
    NoEdits,
    EncryptedDocument,
    SignedDocument,
    ReflowApprovalRequired,
    ScanPatchUnavailable,
    InvalidOriginal(String),
    Validation(String),
}

impl std::fmt::Display for PdfExportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoEdits => formatter.write_str("document has no applied edits"),
            Self::EncryptedDocument => {
                formatter.write_str("encrypted PDFs are not modified without a permission policy")
            }
            Self::SignedDocument => formatter.write_str(
                "PDF carries signature/certification evidence; export is fail-closed",
            ),
            Self::ReflowApprovalRequired => {
                formatter.write_str("system reflow requires explicit approval")
            }
            Self::ScanPatchUnavailable => {
                formatter.write_str("scan recognition/inpainting is not available")
            }
            Self::InvalidOriginal(message) => write!(formatter, "invalid original PDF: {message}"),
            Self::Validation(message) => write!(formatter, "export validation failed: {message}"),
        }
    }
}

impl std::error::Error for PdfExportError {}

#[derive(Clone)]
struct RevisionObject {
    reference: ObjRef,
    body: Vec<u8>,
}

pub fn export_pdf(
    document: &RecognizedDocument,
    options: &PdfExportOptions,
) -> Result<ExportedPdf, PdfExportError> {
    if document.edits.edits.is_empty() {
        return Err(PdfExportError::NoEdits);
    }
    if document
        .edits
        .edits
        .iter()
        .any(|edit| edit.plan.requires_explicit_approval)
    {
        return Err(PdfExportError::ReflowApprovalRequired);
    }
    if document.edits.edits.iter().any(|edit| {
        matches!(edit.plan.erase, ErasePlan::RasterPatchUnavailable)
    }) {
        return Err(PdfExportError::ScanPatchUnavailable);
    }
    if options.fail_on_signatures
        && (contains(&document.original, b"/ByteRange")
            || contains(&document.original, b"/DocMDP"))
    {
        return Err(PdfExportError::SignedDocument);
    }

    let compatible = crate::display::parser_compatible_bytes(&document.original);
    let parser_bytes = compatible.as_deref().unwrap_or(&document.original);
    let mut parser = PdfDocument::parse(parser_bytes)
        .map_err(|error| PdfExportError::InvalidOriginal(error.to_string()))?;
    if parser.trailer().get("Encrypt").is_some() {
        return Err(PdfExportError::EncryptedDocument);
    }
    let root = parser
        .trailer()
        .get_ref("Root")
        .ok_or_else(|| PdfExportError::InvalidOriginal("trailer has no /Root".to_string()))?;
    let original_size = parser.trailer().get_int("Size").unwrap_or(1).max(1) as u32;
    let previous_xref = last_startxref(&document.original).ok_or_else(|| {
        PdfExportError::InvalidOriginal("startxref was not found".to_string())
    })?;
    let page_refs = page_object_refs(&mut parser)
        .map_err(|error| PdfExportError::InvalidOriginal(error.to_string()))?;

    let mut next_object = original_size;
    let mut revisions = Vec::new();
    let mut created_hashes = Vec::new();
    let mut plans_by_page: BTreeMap<u32, Vec<_>> = BTreeMap::new();
    for edit in &document.edits.edits {
        plans_by_page
            .entry(edit.plan.page.0)
            .or_default()
            .push(&edit.plan);
    }

    for (page_index, plans) in plans_by_page {
        let page = document.pages.get(page_index as usize).ok_or_else(|| {
            PdfExportError::InvalidOriginal(format!("recognized page {page_index} is absent"))
        })?;
        let page_ref = *page_refs.get(page_index as usize).ok_or_else(|| {
            PdfExportError::InvalidOriginal(format!("PDF page {page_index} is absent"))
        })?;
        let page_object = parser
            .resolve_ref(page_ref)
            .map_err(|error| PdfExportError::InvalidOriginal(error.to_string()))?;
        let page_dict = page_object
            .as_dict()
            .cloned()
            .ok_or_else(|| PdfExportError::InvalidOriginal("page is not a dictionary".to_string()))?;
        let original_resources = inherited_resources(&mut parser, &page_dict)
            .map_err(PdfExportError::InvalidOriginal)?;

        let form_ref = allocate(&mut next_object);
        let resources_ref = allocate(&mut next_object);
        let content_ref = allocate(&mut next_object);
        let form_name = format!("MPOriginal{page_index}");
        let exact_edits = plans
            .iter()
            .flat_map(|plan| match &plan.erase {
                ErasePlan::OperatorRewrite { edits, .. } => edits.iter().collect::<Vec<_>>(),
                _ => Vec::new(),
            })
            .collect::<Vec<_>>();
        let original_content = rewrite_content_streams(&page.display, &exact_edits)
            .map_err(PdfExportError::InvalidOriginal)?;
        let form_dict = format!(
            "/Type /XObject /Subtype /Form /BBox [{} {} {} {}] /Resources {}",
            number(page.display.media_box.min_x),
            number(page.display.media_box.min_y),
            number(page.display.media_box.max_x),
            number(page.display.media_box.max_y),
            object_inline(&original_resources)
        );
        push_new_object(
            &mut revisions,
            &mut created_hashes,
            form_ref,
            stream_body(&form_dict, &original_content),
        );

        let merged_resources = merge_xobject_resources(
            &mut parser,
            &original_resources,
            &form_name,
            form_ref,
        )
        .map_err(PdfExportError::InvalidOriginal)?;
        push_new_object(
            &mut revisions,
            &mut created_hashes,
            resources_ref,
            object_body(&PdfObj::Dict(merged_resources)),
        );

        let patches = merge_rectangles(
            plans
                .iter()
                .filter(|plan| matches!(plan.erase, ErasePlan::ClippedOriginalForm { .. }))
                .map(|plan| plan.patch_bounds)
                .collect(),
        );
        let mut patch_content = Vec::new();
        patch_content.extend_from_slice(b"q\n");
        if !patches.is_empty() {
            write_rect_path(&mut patch_content, page.display.crop_box);
            for patch in &patches {
                write_rect_path(&mut patch_content, *patch);
            }
            patch_content.extend_from_slice(b"W* n\n");
        }
        patch_content.extend_from_slice(format!("/{form_name} Do\nQ\n").as_bytes());
        for plan in plans {
            let commands = match &plan.erase {
                ErasePlan::OperatorRewrite { replacement, .. }
                | ErasePlan::ClippedOriginalForm { replacement }
                | ErasePlan::OverlayOnly { replacement } => replacement,
                ErasePlan::RasterPatchUnavailable => return Err(PdfExportError::ScanPatchUnavailable),
            };
            write_paint_commands(&mut patch_content, commands);
        }
        push_new_object(
            &mut revisions,
            &mut created_hashes,
            content_ref,
            stream_body("", &patch_content),
        );

        let mut revised_page = page_dict;
        revised_page
            .map
            .insert("Contents".to_string(), PdfObj::Ref(content_ref));
        revised_page
            .map
            .insert("Resources".to_string(), PdfObj::Ref(resources_ref));
        revisions.push(RevisionObject {
            reference: page_ref,
            body: object_body(&PdfObj::Dict(revised_page)),
        });
    }

    let mut provenance_json = Vec::new();
    if options.include_provenance {
        provenance_json = canonical_provenance_json(document, &created_hashes);
        let embedded_ref = allocate(&mut next_object);
        let filespec_ref = allocate(&mut next_object);
        let names_ref = allocate(&mut next_object);
        let metadata_ref = allocate(&mut next_object);
        push_new_object(
            &mut revisions,
            &mut created_hashes,
            embedded_ref,
            stream_body("/Type /EmbeddedFile /Subtype /application#2Fjson", &provenance_json),
        );
        let mut filespec = PdfDict::new();
        filespec
            .map
            .insert("Type".to_string(), PdfObj::Name("Filespec".to_string()));
        filespec.map.insert(
            "F".to_string(),
            PdfObj::Str(b"makepad-score-edit.json".to_vec()),
        );
        filespec.map.insert(
            "UF".to_string(),
            PdfObj::Str(b"makepad-score-edit.json".to_vec()),
        );
        let mut ef = PdfDict::new();
        ef.map.insert("F".to_string(), PdfObj::Ref(embedded_ref));
        filespec.map.insert("EF".to_string(), PdfObj::Dict(ef));
        filespec.map.insert(
            "AFRelationship".to_string(),
            PdfObj::Name("Data".to_string()),
        );
        push_new_object(
            &mut revisions,
            &mut created_hashes,
            filespec_ref,
            object_body(&PdfObj::Dict(filespec)),
        );
        let mut embedded_names = PdfDict::new();
        embedded_names.map.insert(
            "Names".to_string(),
            PdfObj::Array(vec![
                PdfObj::Str(b"makepad-score-edit.json".to_vec()),
                PdfObj::Ref(filespec_ref),
            ]),
        );
        push_new_object(
            &mut revisions,
            &mut created_hashes,
            names_ref,
            object_body(&PdfObj::Dict(embedded_names)),
        );
        let xmp = xmp_summary(&document.original_sha256, document.edits.edits.len());
        push_new_object(
            &mut revisions,
            &mut created_hashes,
            metadata_ref,
            stream_body("/Type /Metadata /Subtype /XML", xmp.as_bytes()),
        );
        let catalog_object = parser
            .resolve_ref(root)
            .map_err(|error| PdfExportError::InvalidOriginal(error.to_string()))?;
        let mut catalog = catalog_object
            .as_dict()
            .cloned()
            .ok_or_else(|| PdfExportError::InvalidOriginal("catalog is not a dictionary".to_string()))?;
        let mut names = match catalog.get("Names") {
            Some(value) => parser
                .resolve(value)
                .ok()
                .and_then(|value| value.as_dict().cloned())
                .unwrap_or_default(),
            None => PdfDict::new(),
        };
        names
            .map
            .insert("EmbeddedFiles".to_string(), PdfObj::Ref(names_ref));
        catalog.map.insert("Names".to_string(), PdfObj::Dict(names));
        catalog
            .map
            .insert("Metadata".to_string(), PdfObj::Ref(metadata_ref));
        catalog
            .map
            .insert("AF".to_string(), PdfObj::Array(vec![PdfObj::Ref(filespec_ref)]));
        revisions.push(RevisionObject {
            reference: root,
            body: object_body(&PdfObj::Dict(catalog)),
        });
    }

    revisions.sort_by_key(|object| (object.reference.num, object.reference.gen));
    let mut bytes = document.original.to_vec();
    if !bytes.ends_with(b"\n") {
        bytes.push(b'\n');
    }
    let mut xref_entries = Vec::new();
    for object in &revisions {
        let offset = bytes.len();
        bytes.extend_from_slice(
            format!("{} {} obj\n", object.reference.num, object.reference.gen).as_bytes(),
        );
        bytes.extend_from_slice(&object.body);
        if !bytes.ends_with(b"\n") {
            bytes.push(b'\n');
        }
        bytes.extend_from_slice(b"endobj\n");
        xref_entries.push((object.reference, offset));
    }
    let xref_offset = bytes.len();
    bytes.extend_from_slice(b"xref\n");
    for (reference, offset) in &xref_entries {
        bytes.extend_from_slice(format!("{} 1\n", reference.num).as_bytes());
        bytes.extend_from_slice(format!("{:010} {:05} n \n", offset, reference.gen).as_bytes());
    }
    let size = next_object.max(original_size);
    bytes.extend_from_slice(
        format!(
            "trailer\n<< /Size {size} /Root {} {} R /Prev {previous_xref} >>\nstartxref\n{xref_offset}\n%%EOF\n",
            root.num, root.gen
        )
        .as_bytes(),
    );

    let original_prefix_identical = bytes.starts_with(&document.original);
    let compatible_export = crate::display::parser_compatible_bytes(&bytes);
    let exported_parser_bytes = compatible_export.as_deref().unwrap_or(&bytes);
    let (reparsed, page_count) = match PdfDocument::parse(exported_parser_bytes) {
        Ok(parsed) => (true, parsed.page_count()),
        Err(error) => return Err(PdfExportError::Validation(error.to_string())),
    };
    let mut warnings = Vec::new();
    if !options.fail_on_signatures
        && (contains(&document.original, b"/ByteRange")
            || contains(&document.original, b"/DocMDP"))
    {
        warnings.push("the incremental edit is not covered by the original signature".to_string());
    }
    if document
        .edits
        .edits
        .iter()
        .any(|edit| edit.plan.warnings.contains(&SpliceWarning::FontSubstituted))
    {
        warnings.push("patch uses OFL-compatible substitute geometry".to_string());
    }
    Ok(ExportedPdf {
        original_len: document.original.len(),
        appended_len: bytes.len() - document.original.len(),
        provenance_json,
        created_object_hashes: created_hashes,
        validation: ExportValidation {
            original_prefix_identical,
            incremental_revision: true,
            reparsed,
            page_count,
            warnings,
        },
        bytes,
    })
}

fn rewrite_content_streams(
    display: &crate::display::DisplayList,
    edits: &[&OperatorEdit],
) -> Result<Vec<u8>, String> {
    let mut pending: BTreeMap<(u32, u16, u16, u32, u32), &[u8]> = BTreeMap::new();
    for edit in edits {
        if !edit.source.form_chain.is_empty() {
            return Err("exact operator rewrite cannot target an operator inside a Form".to_string());
        }
        let key = (
            edit.source.object.num,
            edit.source.object.gen,
            edit.source.stream_index,
            edit.source.decoded_bytes.start,
            edit.source.decoded_bytes.end,
        );
        match pending.insert(key, &edit.replacement) {
            Some(existing) if existing != edit.replacement => {
                return Err("conflicting replacements target one PDF operator".to_string())
            }
            _ => {}
        }
    }

    let mut output = Vec::new();
    for stream in &display.content_streams {
        let mut stream_edits = pending
            .iter()
            .filter(|((object, generation, stream_index, _, _), _)| {
                *object == stream.object.num
                    && *generation == stream.object.gen
                    && *stream_index == stream.stream_index
            })
            .map(|((_, _, _, start, end), replacement)| (*start, *end, *replacement))
            .collect::<Vec<_>>();
        stream_edits.sort_by_key(|(start, end, _)| (*start, *end));
        let mut cursor = 0_usize;
        for (start, end, replacement) in stream_edits {
            let (start, end) = (start as usize, end as usize);
            if start < cursor || end < start || end > stream.decoded.len() {
                return Err("operator rewrite has an invalid or overlapping byte range".to_string());
            }
            output.extend_from_slice(&stream.decoded[cursor..start]);
            output.extend_from_slice(replacement);
            cursor = end;
            pending.remove(&(
                stream.object.num,
                stream.object.gen,
                stream.stream_index,
                start as u32,
                end as u32,
            ));
        }
        output.extend_from_slice(&stream.decoded[cursor..]);
        output.push(b'\n');
    }
    if !pending.is_empty() {
        return Err("operator rewrite source stream was not found on the page".to_string());
    }
    Ok(output)
}

fn allocate(next: &mut u32) -> ObjRef {
    let value = ObjRef { num: *next, gen: 0 };
    *next = next.saturating_add(1);
    value
}

fn push_new_object(
    revisions: &mut Vec<RevisionObject>,
    hashes: &mut Vec<(u32, [u8; 32])>,
    reference: ObjRef,
    body: Vec<u8>,
) {
    hashes.push((reference.num, sha256(&body)));
    revisions.push(RevisionObject { reference, body });
}

fn inherited_resources(
    parser: &mut PdfDocument<'_>,
    page: &PdfDict,
) -> Result<PdfObj, String> {
    if let Some(resources) = page.get("Resources") {
        return Ok(resources.clone());
    }
    let mut parent = page.get("Parent").cloned();
    for _ in 0..128 {
        let Some(parent_object) = parent else {
            break;
        };
        let resolved = parser.resolve(&parent_object).map_err(|error| error.to_string())?;
        let dict = resolved
            .as_dict()
            .ok_or_else(|| "page parent is not a dictionary".to_string())?;
        if let Some(resources) = dict.get("Resources") {
            return Ok(resources.clone());
        }
        parent = dict.get("Parent").cloned();
    }
    Ok(PdfObj::Dict(PdfDict::new()))
}

fn merge_xobject_resources(
    parser: &mut PdfDocument<'_>,
    resources: &PdfObj,
    name: &str,
    form: ObjRef,
) -> Result<PdfDict, String> {
    let resolved = parser.resolve(resources).map_err(|error| error.to_string())?;
    let mut resources = resolved.as_dict().cloned().unwrap_or_default();
    let mut xobjects = match resources.get("XObject") {
        Some(value) => parser
            .resolve(value)
            .map_err(|error| error.to_string())?
            .as_dict()
            .cloned()
            .unwrap_or_default(),
        None => PdfDict::new(),
    };
    xobjects.map.insert(name.to_string(), PdfObj::Ref(form));
    resources
        .map
        .insert("XObject".to_string(), PdfObj::Dict(xobjects));
    Ok(resources)
}

fn stream_body(extra_dict: &str, data: &[u8]) -> Vec<u8> {
    let mut output = format!("<< /Length {} {} >>\nstream\n", data.len(), extra_dict).into_bytes();
    output.extend_from_slice(data);
    if !output.ends_with(b"\n") {
        output.push(b'\n');
    }
    output.extend_from_slice(b"endstream");
    output
}

fn object_body(object: &PdfObj) -> Vec<u8> {
    let mut output = Vec::new();
    write_object(&mut output, object);
    output
}

fn object_inline(object: &PdfObj) -> String {
    String::from_utf8_lossy(&object_body(object)).into_owned()
}

fn write_object(output: &mut Vec<u8>, object: &PdfObj) {
    match object {
        PdfObj::Null => output.extend_from_slice(b"null"),
        PdfObj::Bool(value) => output.extend_from_slice(if *value { b"true" } else { b"false" }),
        PdfObj::Int(value) => output.extend_from_slice(value.to_string().as_bytes()),
        PdfObj::Real(value) => output.extend_from_slice(number(*value).as_bytes()),
        PdfObj::Name(value) => {
            output.push(b'/');
            write_name(output, value);
        }
        PdfObj::Str(value) => {
            output.push(b'<');
            for byte in value {
                output.extend_from_slice(format!("{byte:02X}").as_bytes());
            }
            output.push(b'>');
        }
        PdfObj::Array(values) => {
            output.push(b'[');
            for value in values {
                write_object(output, value);
                output.push(b' ');
            }
            output.push(b']');
        }
        PdfObj::Dict(dict) => {
            output.extend_from_slice(b"<<");
            let mut entries: Vec<_> = dict.map.iter().collect();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            for (key, value) in entries {
                output.extend_from_slice(b" /");
                write_name(output, key);
                output.push(b' ');
                write_object(output, value);
            }
            output.extend_from_slice(b" >>");
        }
        PdfObj::Ref(reference) => output.extend_from_slice(
            format!("{} {} R", reference.num, reference.gen).as_bytes(),
        ),
        PdfObj::Stream(stream) => {
            let mut dict = stream.dict.clone();
            dict.map
                .insert("Length".to_string(), PdfObj::Int(stream.data.len() as i64));
            write_object(output, &PdfObj::Dict(dict));
            output.extend_from_slice(b"\nstream\n");
            output.extend_from_slice(&stream.data);
            output.extend_from_slice(b"\nendstream");
        }
    }
}

fn write_name(output: &mut Vec<u8>, value: &str) {
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
            output.push(byte);
        } else {
            output.extend_from_slice(format!("#{byte:02X}").as_bytes());
        }
    }
}

fn number(value: f64) -> String {
    if value.fract().abs() < 1e-9 {
        format!("{value:.0}")
    } else {
        let mut value = format!("{value:.6}");
        while value.ends_with('0') {
            value.pop();
        }
        value
    }
}

fn write_rect_path(output: &mut Vec<u8>, rect: Rect) {
    output.extend_from_slice(
        format!(
            "{} {} {} {} re\n",
            number(rect.min_x),
            number(rect.min_y),
            number(rect.width()),
            number(rect.height())
        )
        .as_bytes(),
    );
}

fn write_paint_commands(output: &mut Vec<u8>, commands: &[PaintCommand]) {
    for command in commands {
        match command {
            PaintCommand::StaffLine {
                start,
                end,
                thickness,
            }
            | PaintCommand::Stem {
                start,
                end,
                thickness,
            } => {
                output.extend_from_slice(
                    format!(
                        "q 0 G {} w {} {} m {} {} l S Q\n",
                        number(*thickness),
                        number(start.x),
                        number(start.y),
                        number(end.x),
                        number(end.y)
                    )
                    .as_bytes(),
                );
            }
            PaintCommand::Notehead {
                center,
                width,
                height,
                filled,
            } => write_ellipse(output, *center, *width, *height, *filled),
            PaintCommand::Beam { corners } => {
                output.extend_from_slice(b"q 0 g ");
                write_polygon(output, corners);
                output.extend_from_slice(b" f Q\n");
            }
            PaintCommand::Dot { center, diameter } => {
                write_ellipse(output, *center, *diameter, *diameter, true);
            }
            PaintCommand::AccidentalText { origin, name } => {
                write_accidental_geometry(output, *origin, name);
            }
            PaintCommand::Path {
                commands,
                paint,
                line_width,
            } => {
                output.extend_from_slice(format!("q 0 g 0 G {} w\n", number(*line_width)).as_bytes());
                write_path_commands(output, commands);
                output.extend_from_slice(match paint {
                    PathPaint::Stroke => b"S Q\n",
                    PathPaint::Fill | PathPaint::FillEvenOdd => b"f Q\n",
                    PathPaint::FillStroke | PathPaint::FillStrokeEvenOdd => b"B Q\n",
                    PathPaint::None => b"n Q\n",
                });
            }
        }
    }
}

fn write_path_commands(output: &mut Vec<u8>, commands: &[PathCommand]) {
    for command in commands {
        match command {
            PathCommand::Move(point) => output.extend_from_slice(
                format!("{} {} m\n", number(point.x), number(point.y)).as_bytes(),
            ),
            PathCommand::Line(point) => output.extend_from_slice(
                format!("{} {} l\n", number(point.x), number(point.y)).as_bytes(),
            ),
            PathCommand::Cubic(a, b, c) => output.extend_from_slice(
                format!(
                    "{} {} {} {} {} {} c\n",
                    number(a.x),
                    number(a.y),
                    number(b.x),
                    number(b.y),
                    number(c.x),
                    number(c.y)
                )
                .as_bytes(),
            ),
            PathCommand::Close => output.extend_from_slice(b"h\n"),
        }
    }
}

fn write_ellipse(output: &mut Vec<u8>, center: Point, width: f64, height: f64, filled: bool) {
    const KAPPA: f64 = 0.552_284_749_830_793_6;
    let rx = width * 0.5;
    let ry = height * 0.5;
    output.extend_from_slice(b"q 0 g 0 G ");
    output.extend_from_slice(
        format!("{} {} m\n", number(center.x + rx), number(center.y)).as_bytes(),
    );
    for (a, b, c) in [
        (
            Point::new(center.x + rx, center.y + ry * KAPPA),
            Point::new(center.x + rx * KAPPA, center.y + ry),
            Point::new(center.x, center.y + ry),
        ),
        (
            Point::new(center.x - rx * KAPPA, center.y + ry),
            Point::new(center.x - rx, center.y + ry * KAPPA),
            Point::new(center.x - rx, center.y),
        ),
        (
            Point::new(center.x - rx, center.y - ry * KAPPA),
            Point::new(center.x - rx * KAPPA, center.y - ry),
            Point::new(center.x, center.y - ry),
        ),
        (
            Point::new(center.x + rx * KAPPA, center.y - ry),
            Point::new(center.x + rx, center.y - ry * KAPPA),
            Point::new(center.x + rx, center.y),
        ),
    ] {
        output.extend_from_slice(
            format!(
                "{} {} {} {} {} {} c\n",
                number(a.x),
                number(a.y),
                number(b.x),
                number(b.y),
                number(c.x),
                number(c.y)
            )
            .as_bytes(),
        );
    }
    output.extend_from_slice(if filled { b"f Q\n" } else { b"1 g B Q\n" });
}

fn write_polygon(output: &mut Vec<u8>, points: &[Point; 4]) {
    output.extend_from_slice(
        format!("{} {} m", number(points[0].x), number(points[0].y)).as_bytes(),
    );
    for point in &points[1..] {
        output.extend_from_slice(format!(" {} {} l", number(point.x), number(point.y)).as_bytes());
    }
    output.extend_from_slice(b" h");
}

fn write_accidental_geometry(output: &mut Vec<u8>, origin: Point, name: &str) {
    let size = 4.0;
    output.extend_from_slice(b"q 0 G 0.6 w\n");
    if name.contains("Sharp") {
        for x in [origin.x - size * 0.25, origin.x + size * 0.25] {
            output.extend_from_slice(
                format!("{} {} m {} {} l S\n", number(x), number(origin.y - size), number(x), number(origin.y + size)).as_bytes(),
            );
        }
        for y in [origin.y - size * 0.3, origin.y + size * 0.3] {
            output.extend_from_slice(
                format!("{} {} m {} {} l S\n", number(origin.x - size * 0.7), number(y), number(origin.x + size * 0.7), number(y + size * 0.2)).as_bytes(),
            );
        }
    } else {
        output.extend_from_slice(
            format!("{} {} m {} {} l S\n", number(origin.x), number(origin.y - size), number(origin.x), number(origin.y + size)).as_bytes(),
        );
    }
    output.extend_from_slice(b"Q\n");
}

fn merge_rectangles(mut values: Vec<Rect>) -> Vec<Rect> {
    values.sort_by(|left, right| left.min_x.total_cmp(&right.min_x));
    let mut output: Vec<Rect> = Vec::new();
    for value in values {
        if let Some(existing) = output
            .iter_mut()
            .find(|existing| existing.expand(0.5).intersects(value))
        {
            *existing = existing.union(value);
        } else {
            output.push(value);
        }
    }
    output
}

fn last_startxref(bytes: &[u8]) -> Option<usize> {
    let marker = b"startxref";
    let position = bytes.windows(marker.len()).rposition(|window| window == marker)?;
    let mut cursor = position + marker.len();
    while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    }
    let start = cursor;
    while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
        cursor += 1;
    }
    std::str::from_utf8(&bytes[start..cursor]).ok()?.parse().ok()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|window| window == needle)
}

fn xmp_summary(original_hash: &[u8; 32], edit_count: usize) -> String {
    format!(
        "<?xpacket begin='\u{feff}'?>\n<x:xmpmeta xmlns:x='adobe:ns:meta/'><rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'><rdf:Description xmlns:mp='https://makepad.dev/ns/score-pdf/1.0/' mp:OriginalSHA256='{}' mp:EditCount='{}'/></rdf:RDF></x:xmpmeta>\n<?xpacket end='w'?>",
        crate::sha256::hex(original_hash), edit_count
    )
}

#[allow(dead_code)]
fn _color_is_retained(color: Color) -> Color {
    color
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::{ContentStreamRecord, DisplayList, PageIndex, SourceSpan};
    use std::collections::HashMap;

    #[test]
    fn startxref_uses_last_revision() {
        assert_eq!(last_startxref(b"startxref\n12\n%%EOF\nstartxref\n98\n%%EOF"), Some(98));
    }

    #[test]
    fn overlapping_clips_are_merged() {
        let merged = merge_rectangles(vec![
            Rect::new(0.0, 0.0, 10.0, 10.0),
            Rect::new(9.0, 5.0, 20.0, 12.0),
        ]);
        assert_eq!(merged, vec![Rect::new(0.0, 0.0, 20.0, 12.0)]);
    }

    #[test]
    fn exact_rewrite_changes_only_the_provenance_range() {
        let object = ObjRef { num: 7, gen: 0 };
        let display = DisplayList {
            page: PageIndex(0),
            page_object: ObjRef { num: 3, gen: 0 },
            media_box: Rect::new(0.0, 0.0, 100.0, 100.0),
            crop_box: Rect::new(0.0, 0.0, 100.0, 100.0),
            rotation: 0,
            content_streams: vec![ContentStreamRecord {
                object,
                stream_index: 0,
                decoded: b"q\nS\nQ".to_vec(),
            }],
            operators: Vec::new(),
            primitives: Vec::new(),
            fonts: HashMap::new(),
        };
        let edit = OperatorEdit {
            source: SourceSpan {
                object,
                stream_index: 0,
                decoded_bytes: 2..3,
                operator_index: 1,
                subpath_index: Some(0),
                form_chain: Vec::new(),
            },
            replacement: b"n".to_vec(),
        };
        assert_eq!(
            rewrite_content_streams(&display, &[&edit]).unwrap(),
            b"q\nn\nQ\n"
        );
    }
}
