use crate::document::PdfDocument;
use crate::lexer::*;
use crate::object::*;

/// Represents a single PDF page with its properties and content.
#[derive(Clone, Debug, Default)]
pub struct PdfPage {
    /// MediaBox: [x0, y0, x1, y1] in PDF points (1/72 inch).
    pub media_box: [f64; 4],
    /// CropBox (defaults to MediaBox if not specified).
    pub crop_box: [f64; 4],
    /// Page rotation in degrees (0, 90, 180, 270).
    pub rotate: i32,
    /// Decoded content stream bytes (concatenated if multiple).
    pub content_data: Vec<u8>,
    /// Font resources: name → font dict info.
    pub fonts: std::collections::HashMap<String, FontResource>,
    /// XObject resources: name → xobject info.
    pub xobjects: std::collections::HashMap<String, XObjectResource>,
    /// ExtGState resources.
    pub ext_gstate: std::collections::HashMap<String, ExtGStateResource>,
}

/// Minimal font resource info extracted from the page's resources.
#[derive(Clone, Debug)]
pub struct FontResource {
    pub subtype: String,   // Type1, TrueType, Type0, Type3, etc.
    pub base_font: String, // e.g. "Helvetica", "TimesNewRoman"
    pub encoding: FontEncoding,
    pub widths: Vec<f64>, // character widths (indexed from first_char)
    pub first_char: u32,
    pub last_char: u32,
    pub to_unicode: Option<CMapData>,
    pub default_width: f64,
}

#[derive(Clone, Debug)]
pub enum FontEncoding {
    Standard,
    MacRoman,
    WinAnsi,
    Identity,
    Custom(std::collections::HashMap<u8, String>), // char code → glyph name
}

/// Parsed ToUnicode CMap: maps character codes to Unicode strings.
#[derive(Clone, Debug)]
pub struct CMapData {
    pub mappings: std::collections::HashMap<u32, String>,
}

#[derive(Clone, Debug)]
pub struct XObjectResource {
    pub subtype: String, // "Image" or "Form"
    pub obj_ref: ObjRef,
}

#[derive(Clone, Debug)]
pub struct ExtGStateResource {
    pub ca: Option<f64>,       // stroke alpha
    pub ca_lower: Option<f64>, // fill alpha (lowercase ca)
}

impl PdfPage {
    pub fn from_obj(doc: &mut PdfDocument, page_obj: &PdfObj) -> PdfResult<Self> {
        let dict = page_obj
            .as_dict()
            .ok_or_else(|| PdfError::new("page object is not a dict"))?;

        // MediaBox (required, may be inherited)
        let media_box = parse_rect(dict.get("MediaBox"))?;
        let crop_box = dict
            .get("CropBox")
            .and_then(|o| parse_rect(Some(o)).ok())
            .unwrap_or(media_box);
        let rotate = dict.get_int("Rotate").unwrap_or(0) as i32;

        // Content streams
        let content_data = extract_content_data(doc, dict)?;

        // Resources (may be inherited from parent Pages node)
        let resources_obj = dict.get("Resources");
        let resources = if let Some(r) = resources_obj {
            doc.resolve(r)?
        } else {
            PdfObj::Dict(PdfDict::new())
        };
        let resources_dict = resources.as_dict().cloned().unwrap_or_default();

        // Extract font resources
        let fonts = extract_fonts(doc, &resources_dict)?;

        // Extract XObject resources
        let xobjects = extract_xobjects(doc, &resources_dict)?;

        // Extract ExtGState resources
        let ext_gstate = extract_ext_gstate(doc, &resources_dict)?;

        Ok(PdfPage {
            media_box,
            crop_box,
            rotate,
            content_data,
            fonts,
            xobjects,
            ext_gstate,
        })
    }

    /// Page width in PDF points.
    pub fn width(&self) -> f64 {
        self.crop_box[2] - self.crop_box[0]
    }

    /// Page height in PDF points.
    pub fn height(&self) -> f64 {
        self.crop_box[3] - self.crop_box[1]
    }
}

fn parse_rect(obj: Option<&PdfObj>) -> PdfResult<[f64; 4]> {
    let obj = obj.ok_or_else(|| PdfError::new("missing rectangle"))?;
    let nums = obj
        .as_number_array()
        .ok_or_else(|| PdfError::new("rectangle must be a number array"))?;
    if nums.len() < 4 {
        return Err(PdfError::new("rectangle must have 4 elements"));
    }
    Ok([nums[0], nums[1], nums[2], nums[3]])
}

fn extract_content_data(doc: &mut PdfDocument, dict: &PdfDict) -> PdfResult<Vec<u8>> {
    let contents = match dict.get("Contents") {
        Some(c) => c.clone(),
        None => return Ok(Vec::new()),
    };

    let resolved = doc.resolve(&contents)?;
    match &resolved {
        PdfObj::Stream(s) => doc.decode_stream(s),
        PdfObj::Array(arr) => {
            let mut all_data = Vec::new();
            for item in arr {
                let resolved_item = doc.resolve(item)?;
                if let PdfObj::Stream(s) = &resolved_item {
                    let decoded = doc.decode_stream(s)?;
                    all_data.extend_from_slice(&decoded);
                    all_data.push(b'\n'); // separate content streams
                }
            }
            Ok(all_data)
        }
        PdfObj::Ref(_) => {
            // Already resolved above, shouldn't happen
            Ok(Vec::new())
        }
        _ => Ok(Vec::new()),
    }
}

fn extract_fonts(
    doc: &mut PdfDocument,
    resources: &PdfDict,
) -> PdfResult<std::collections::HashMap<String, FontResource>> {
    let mut fonts = std::collections::HashMap::new();
    let font_dict = match resources.get("Font") {
        Some(obj) => {
            let resolved = doc.resolve(obj)?;
            match resolved {
                PdfObj::Dict(d) => d,
                _ => return Ok(fonts),
            }
        }
        None => return Ok(fonts),
    };

    for (name, obj) in &font_dict.map {
        if let Ok(font) = parse_font_resource(doc, obj) {
            fonts.insert(name.clone(), font);
        }
    }
    Ok(fonts)
}

fn parse_font_resource(doc: &mut PdfDocument, obj: &PdfObj) -> PdfResult<FontResource> {
    let resolved = doc.resolve(obj)?;
    let dict = resolved
        .as_dict()
        .ok_or_else(|| PdfError::new("font resource is not a dict"))?;

    let subtype = dict.get_name("Subtype").unwrap_or("Type1").to_string();
    let base_font = dict.get_name("BaseFont").unwrap_or("Helvetica").to_string();

    let encoding = match dict.get("Encoding") {
        Some(PdfObj::Name(n)) => match n.as_str() {
            "MacRomanEncoding" => FontEncoding::MacRoman,
            "WinAnsiEncoding" => FontEncoding::WinAnsi,
            "StandardEncoding" => FontEncoding::Standard,
            "Identity-H" | "Identity-V" => FontEncoding::Identity,
            _ => FontEncoding::WinAnsi,
        },
        _ => FontEncoding::WinAnsi,
    };

    let first_char = dict.get_int("FirstChar").unwrap_or(0) as u32;
    let last_char = dict.get_int("LastChar").unwrap_or(255) as u32;

    let widths = if let Some(w_obj) = dict.get("Widths") {
        let resolved_w = doc.resolve(w_obj)?;
        match &resolved_w {
            PdfObj::Array(arr) => arr.iter().map(|o| o.as_f64().unwrap_or(0.0)).collect(),
            _ => Vec::new(),
        }
    } else {
        Vec::new()
    };

    let default_width = dict.get_f64("MissingWidth").unwrap_or(600.0);

    // Parse ToUnicode CMap if present
    let to_unicode = if let Some(tu_obj) = dict.get("ToUnicode") {
        match doc.resolve_stream(tu_obj) {
            Ok(cmap_data) => parse_to_unicode(&cmap_data).ok(),
            Err(_) => None,
        }
    } else {
        None
    };

    Ok(FontResource {
        subtype,
        base_font,
        encoding,
        widths,
        first_char,
        last_char,
        to_unicode,
        default_width,
    })
}

/// Parse a ToUnicode CMap from decoded stream data.
fn parse_to_unicode(data: &[u8]) -> PdfResult<CMapData> {
    let text = String::from_utf8_lossy(data);
    let mut mappings = std::collections::HashMap::new();

    // Parse "beginbfchar" sections
    let mut chars = text.as_ref();
    while let Some(pos) = chars.find("beginbfchar") {
        let section = &chars[pos + 11..];
        let end = section.find("endbfchar").unwrap_or(section.len());
        let section = &section[..end];

        for line in section.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            // Format: <XXXX> <YYYY>
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                if let (Some(src), Some(dst)) =
                    (parse_hex_token(parts[0]), parse_hex_unicode(parts[1]))
                {
                    mappings.insert(src, dst);
                }
            }
        }

        chars = &chars[pos + 11 + end..];
    }

    // Parse "beginbfrange" sections
    chars = text.as_ref();
    while let Some(pos) = chars.find("beginbfrange") {
        let section = &chars[pos + 12..];
        let end = section.find("endbfrange").unwrap_or(section.len());
        let section = &section[..end];

        for line in section.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 {
                if let (Some(start), Some(end_val), Some(dst_start)) = (
                    parse_hex_token(parts[0]),
                    parse_hex_token(parts[1]),
                    parse_hex_unicode(parts[2]),
                ) {
                    let mut code_point = dst_start.chars().next().unwrap_or('\0') as u32;
                    for src_code in start..=end_val {
                        if let Some(ch) = char::from_u32(code_point) {
                            mappings.insert(src_code, ch.to_string());
                        }
                        code_point += 1;
                    }
                }
            }
        }

        chars = &chars[pos + 12 + end..];
    }

    Ok(CMapData { mappings })
}

fn parse_hex_token(s: &str) -> Option<u32> {
    let s = s.trim_start_matches('<').trim_end_matches('>');
    u32::from_str_radix(s, 16).ok()
}

fn parse_hex_unicode(s: &str) -> Option<String> {
    let s = s.trim_start_matches('<').trim_end_matches('>');
    // Could be multiple 4-hex-digit groups (for supplementary chars)
    let mut result = String::new();
    let mut i = 0;
    let bytes = s.as_bytes();
    while i + 4 <= bytes.len() {
        let hex_str = std::str::from_utf8(&bytes[i..i + 4]).ok()?;
        let code = u32::from_str_radix(hex_str, 16).ok()?;
        result.push(char::from_u32(code)?);
        i += 4;
    }
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

fn extract_xobjects(
    doc: &mut PdfDocument,
    resources: &PdfDict,
) -> PdfResult<std::collections::HashMap<String, XObjectResource>> {
    let mut xobjects = std::collections::HashMap::new();
    let xobj_dict = match resources.get("XObject") {
        Some(obj) => {
            let resolved = doc.resolve(obj)?;
            match resolved {
                PdfObj::Dict(d) => d,
                _ => return Ok(xobjects),
            }
        }
        None => return Ok(xobjects),
    };

    for (name, obj) in &xobj_dict.map {
        if let PdfObj::Ref(r) = obj {
            // Peek at the subtype without fully decoding
            if let Ok(resolved) = doc.resolve(obj) {
                let subtype = resolved
                    .as_dict()
                    .and_then(|d| d.get_name("Subtype"))
                    .unwrap_or("Image")
                    .to_string();
                xobjects.insert(
                    name.clone(),
                    XObjectResource {
                        subtype,
                        obj_ref: *r,
                    },
                );
            }
        }
    }
    Ok(xobjects)
}

fn extract_ext_gstate(
    doc: &mut PdfDocument,
    resources: &PdfDict,
) -> PdfResult<std::collections::HashMap<String, ExtGStateResource>> {
    let mut result = std::collections::HashMap::new();
    let gs_dict = match resources.get("ExtGState") {
        Some(obj) => {
            let resolved = doc.resolve(obj)?;
            match resolved {
                PdfObj::Dict(d) => d,
                _ => return Ok(result),
            }
        }
        None => return Ok(result),
    };

    for (name, obj) in &gs_dict.map {
        if let Ok(resolved) = doc.resolve(obj) {
            if let Some(d) = resolved.as_dict() {
                result.insert(
                    name.clone(),
                    ExtGStateResource {
                        ca: d.get_f64("CA"),
                        ca_lower: d.get_f64("ca"),
                    },
                );
            }
        }
    }
    Ok(result)
}
