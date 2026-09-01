use crate::{parse_xml, MusicXmlDocument, MusicXmlError, MusicXmlResult, XmlElement};
use makepad_zip_file::{
    CentralDirectoryFileHeader, EndOfCentralDirectory, ZipCentralDirectory, ZipMethod, ZipWriter,
    CENTRAL_DIR_FILE_HEADER_SIZE, END_OF_CENTRAL_DIRECTORY_SIZE,
};
use std::io::{Cursor, Read, Seek, SeekFrom};

pub const DEFAULT_ROOTFILE_PATH: &str = "score.musicxml";
const CONTAINER_PATH: &str = "META-INF/container.xml";
const MUSICXML_MEDIA_TYPE: &str = "application/vnd.recordare.musicxml+xml";
const MIMETYPE: &[u8] = b"application/vnd.recordare.musicxml";
const MAX_XML_SIZE: u64 = 256 * 1024 * 1024;

pub fn parse_mxl(bytes: &[u8]) -> MusicXmlResult<MusicXmlDocument> {
    parse_mxl_reader(&mut Cursor::new(bytes))
}

/// Parses MusicXML bytes, recognizing UTF-8 and BOM-marked UTF-16.
pub fn parse_musicxml_bytes(bytes: &[u8]) -> MusicXmlResult<MusicXmlDocument> {
    let (source, encoding) = decode_xml_bytes(bytes)?;
    let xml = parse_xml(&source)?;
    if has_xml_declaration(&source) {
        validate_declared_encoding(xml.declaration.encoding.as_deref(), encoding)?;
    }
    MusicXmlDocument::from_xml_document(xml)
}

pub fn parse_mxl_reader<R: Read + Seek>(reader: &mut R) -> MusicXmlResult<MusicXmlDocument> {
    let directory = read_central_directory(reader)?;
    let container_header = unique_header(&directory, CONTAINER_PATH)?;
    let container_bytes = extract_checked(container_header, reader)?;
    let (container_source, container_encoding) = decode_xml_bytes(&container_bytes)?;
    let container = parse_xml(&container_source)?;
    if has_xml_declaration(&container_source) {
        validate_declared_encoding(
            container.declaration.encoding.as_deref(),
            container_encoding,
        )?;
    }
    if container.root.name != "container" {
        return Err(MusicXmlError::InvalidMxl(format!(
            "{CONTAINER_PATH} has {:?} as its root element",
            container.root.name
        )));
    }
    let rootfiles = container
        .root
        .first_child("rootfiles")
        .ok_or_else(|| MusicXmlError::InvalidMxl("container has no rootfiles element".into()))?;
    let candidates: Vec<_> = rootfiles.children_named("rootfile").collect();
    let rootfile = candidates
        .iter()
        .copied()
        .find(|element| element.attr("media-type") == Some(MUSICXML_MEDIA_TYPE))
        .or_else(|| candidates.first().copied())
        .ok_or_else(|| MusicXmlError::InvalidMxl("container has no rootfile".into()))?;
    let path = rootfile
        .attr("full-path")
        .ok_or_else(|| MusicXmlError::InvalidMxl("rootfile has no full-path".into()))?;
    validate_member_path(path)?;
    let score_header = unique_header(&directory, path)?;
    let score_bytes = extract_checked(score_header, reader)?;
    parse_musicxml_bytes(&score_bytes)
}

pub fn write_mxl(document: &MusicXmlDocument) -> MusicXmlResult<Vec<u8>> {
    write_mxl_with_rootfile(document, DEFAULT_ROOTFILE_PATH)
}

pub fn write_mxl_with_rootfile(
    document: &MusicXmlDocument,
    rootfile_path: &str,
) -> MusicXmlResult<Vec<u8>> {
    validate_member_path(rootfile_path)?;
    if rootfile_path == CONTAINER_PATH || rootfile_path == "mimetype" {
        return Err(MusicXmlError::InvalidMxl(
            "rootfile path collides with a container member".into(),
        ));
    }
    let score = document.to_xml_string()?;
    let container = container_xml(rootfile_path)?;
    let mut writer = ZipWriter::new();
    writer
        .add("mimetype", MIMETYPE, ZipMethod::Store)
        .map_err(|error| MusicXmlError::ZipWrite(format!("{error:?}")))?;
    writer
        .add(CONTAINER_PATH, container.as_bytes(), ZipMethod::Deflate)
        .map_err(|error| MusicXmlError::ZipWrite(format!("{error:?}")))?;
    writer
        .add(rootfile_path, score.as_bytes(), ZipMethod::Deflate)
        .map_err(|error| MusicXmlError::ZipWrite(format!("{error:?}")))?;
    writer
        .finish()
        .map_err(|error| MusicXmlError::ZipWrite(format!("{error:?}")))
}

impl MusicXmlDocument {
    pub fn from_xml_bytes(bytes: &[u8]) -> MusicXmlResult<Self> {
        parse_musicxml_bytes(bytes)
    }

    pub fn from_mxl_bytes(bytes: &[u8]) -> MusicXmlResult<Self> {
        parse_mxl(bytes)
    }

    pub fn to_mxl_bytes(&self) -> MusicXmlResult<Vec<u8>> {
        write_mxl(self)
    }

    pub fn to_mxl_bytes_with_rootfile(&self, path: &str) -> MusicXmlResult<Vec<u8>> {
        write_mxl_with_rootfile(self, path)
    }
}

fn container_xml(rootfile_path: &str) -> Result<String, crate::XmlError> {
    let mut rootfile = XmlElement::new("rootfile")
        .with_attribute("full-path", rootfile_path)
        .with_attribute("media-type", MUSICXML_MEDIA_TYPE);
    // Keep this explicitly empty; the generic writer emits the canonical />.
    rootfile.children.clear();
    let mut rootfiles = XmlElement::new("rootfiles");
    rootfiles.push_element(rootfile);
    let mut container = XmlElement::new("container")
        .with_attribute("version", "1.0")
        .with_attribute(
            "xmlns",
            "urn:oasis:names:tc:opendocument:xmlns:container",
        );
    container.push_element(rootfiles);
    crate::write_xml(&crate::XmlDocument {
        declaration: crate::XmlDeclaration::default(),
        doctype: None,
        before_root: Vec::new(),
        root: container,
        after_root: Vec::new(),
    })
}

fn unique_header<'a>(
    directory: &'a ZipCentralDirectory,
    name: &str,
) -> MusicXmlResult<&'a CentralDirectoryFileHeader> {
    let mut matches = directory
        .file_headers
        .iter()
        .filter(|header| header.file_name == name);
    let header = matches.next().ok_or_else(|| {
        MusicXmlError::InvalidMxl(format!("ZIP member {name:?} was not found"))
    })?;
    if matches.next().is_some() {
        return Err(MusicXmlError::InvalidMxl(format!(
            "ZIP member {name:?} occurs more than once"
        )));
    }
    Ok(header)
}

fn extract_checked<R: Read + Seek>(
    header: &CentralDirectoryFileHeader,
    reader: &mut R,
) -> MusicXmlResult<Vec<u8>> {
    if header.uncompressed_size as u64 > MAX_XML_SIZE {
        return Err(MusicXmlError::InvalidMxl(format!(
            "member {:?} exceeds the {} byte XML limit",
            header.file_name, MAX_XML_SIZE
        )));
    }
    if header.general_purpose_bit_flag & 1 != 0 {
        return Err(MusicXmlError::InvalidMxl(format!(
            "encrypted member {:?} is unsupported",
            header.file_name
        )));
    }
    let bytes = header
        .extract(reader)
        .map_err(|error| MusicXmlError::ZipRead(format!("{error:?}")))?;
    if crc32(&bytes) != header.crc32 {
        return Err(MusicXmlError::InvalidMxl(format!(
            "CRC mismatch for member {:?}",
            header.file_name
        )));
    }
    Ok(bytes)
}

fn read_central_directory<R: Read + Seek>(reader: &mut R) -> MusicXmlResult<ZipCentralDirectory> {
    let length = reader
        .seek(SeekFrom::End(0))
        .map_err(|error| MusicXmlError::ZipRead(error.to_string()))?;
    if length < END_OF_CENTRAL_DIRECTORY_SIZE as u64 {
        return Err(MusicXmlError::InvalidMxl("ZIP is too short".into()));
    }
    let search_length = length.min((u16::MAX as u64) + END_OF_CENTRAL_DIRECTORY_SIZE as u64);
    reader
        .seek(SeekFrom::End(-(search_length as i64)))
        .map_err(|error| MusicXmlError::ZipRead(error.to_string()))?;
    let mut tail = vec![0; search_length as usize];
    reader
        .read_exact(&mut tail)
        .map_err(|error| MusicXmlError::ZipRead(error.to_string()))?;
    let signature = [0x50, 0x4b, 0x05, 0x06];
    let relative = (0..=tail.len() - END_OF_CENTRAL_DIRECTORY_SIZE)
        .rev()
        .find(|index| {
            tail[*index..].starts_with(&signature)
                && tail
                    .get(*index + 20..*index + 22)
                    .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]) as usize)
                    == Some(tail.len() - *index - END_OF_CENTRAL_DIRECTORY_SIZE)
        })
        .ok_or_else(|| MusicXmlError::InvalidMxl("end of central directory not found".into()))?;
    let eocd_offset = length - search_length + relative as u64;
    reader
        .seek(SeekFrom::Start(eocd_offset))
        .map_err(|error| MusicXmlError::ZipRead(error.to_string()))?;
    let eocd = EndOfCentralDirectory::from_stream(reader)
        .map_err(|error| MusicXmlError::ZipRead(format!("{error:?}")))?;
    if eocd.number_of_disk != 0
        || eocd.number_of_start_central_directory_disk != 0
        || eocd.total_entries_this_disk != eocd.total_entries_all_disk
    {
        return Err(MusicXmlError::InvalidMxl(
            "multi-disk ZIP containers are unsupported".into(),
        ));
    }
    let central_end = eocd.central_directory_offset as u64
        + eocd.size_of_the_central_directory as u64;
    if central_end > eocd_offset
        || eocd.total_entries_all_disk as usize * CENTRAL_DIR_FILE_HEADER_SIZE
            > eocd.size_of_the_central_directory as usize
    {
        return Err(MusicXmlError::InvalidMxl(
            "central directory has invalid bounds".into(),
        ));
    }
    reader
        .seek(SeekFrom::Start(eocd.central_directory_offset as u64))
        .map_err(|error| MusicXmlError::ZipRead(error.to_string()))?;
    let mut file_headers = Vec::with_capacity(eocd.total_entries_all_disk as usize);
    for _ in 0..eocd.total_entries_all_disk {
        file_headers.push(
            CentralDirectoryFileHeader::from_stream(reader)
                .map_err(|error| MusicXmlError::ZipRead(format!("{error:?}")))?,
        );
    }
    Ok(ZipCentralDirectory { eocd, file_headers })
}

fn validate_member_path(path: &str) -> MusicXmlResult<()> {
    let invalid = path.is_empty()
        || path.starts_with('/')
        || path.starts_with('\\')
        || path.contains('\\')
        || path.contains('\0')
        || path.chars().nth(1) == Some(':')
        || path
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..");
    if invalid {
        return Err(MusicXmlError::InvalidMxl(format!(
            "unsafe rootfile path {path:?}"
        )));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ByteEncoding {
    Utf8,
    Utf16,
}

fn decode_xml_bytes(bytes: &[u8]) -> MusicXmlResult<(String, ByteEncoding)> {
    if let Some(bytes) = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        return String::from_utf8(bytes.to_vec())
            .map(|text| (text, ByteEncoding::Utf8))
            .map_err(|_| MusicXmlError::UnsupportedEncoding("invalid UTF-8".into()));
    }
    if let Some(bytes) = bytes.strip_prefix(&[0xff, 0xfe]) {
        return decode_utf16(bytes, u16::from_le_bytes);
    }
    if let Some(bytes) = bytes.strip_prefix(&[0xfe, 0xff]) {
        return decode_utf16(bytes, u16::from_be_bytes);
    }
    String::from_utf8(bytes.to_vec())
        .map(|text| (text, ByteEncoding::Utf8))
        .map_err(|_| MusicXmlError::UnsupportedEncoding("non-UTF-8 XML without a BOM".into()))
}

fn decode_utf16(
    bytes: &[u8],
    convert: fn([u8; 2]) -> u16,
) -> MusicXmlResult<(String, ByteEncoding)> {
    if bytes.len() % 2 != 0 {
        return Err(MusicXmlError::UnsupportedEncoding(
            "odd-length UTF-16 XML".into(),
        ));
    }
    let units = bytes
        .chunks_exact(2)
        .map(|pair| convert([pair[0], pair[1]]));
    String::from_utf16(&units.collect::<Vec<_>>())
        .map(|text| (text, ByteEncoding::Utf16))
        .map_err(|_| MusicXmlError::UnsupportedEncoding("invalid UTF-16 XML".into()))
}

fn validate_declared_encoding(
    declared: Option<&str>,
    actual: ByteEncoding,
) -> MusicXmlResult<()> {
    let Some(declared) = declared else {
        return Ok(());
    };
    let normalized = declared.to_ascii_lowercase().replace('_', "-");
    let valid = match actual {
        ByteEncoding::Utf8 => matches!(normalized.as_str(), "utf-8" | "utf8" | "us-ascii"),
        ByteEncoding::Utf16 => matches!(
            normalized.as_str(),
            "utf-16" | "utf16" | "utf-16le" | "utf-16be"
        ),
    };
    if valid {
        Ok(())
    } else {
        Err(MusicXmlError::UnsupportedEncoding(declared.to_string()))
    }
}

fn has_xml_declaration(source: &str) -> bool {
    source
        .strip_prefix('\u{feff}')
        .unwrap_or(source)
        .strip_prefix("<?xml")
        .and_then(|tail| tail.chars().next())
        .is_some_and(char::is_whitespace)
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}
