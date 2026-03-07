use makepad_half::f16;
use std::path::Path;

const EXR_MAGIC: u32 = 20000630;
const EXR_VERSION: u32 = 2;
const LONG_NAMES_FLAG: u32 = 1 << 10;
const MULTIPART_FLAG: u32 = 1 << 12;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    UnexpectedEof,
    InvalidMagic(u32),
    InvalidFormat(String),
    Unsupported(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(err) => write!(f, "{err}"),
            Error::UnexpectedEof => write!(f, "unexpected end of file"),
            Error::InvalidMagic(magic) => write!(f, "invalid OpenEXR magic {magic:#x}"),
            Error::InvalidFormat(msg) => write!(f, "{msg}"),
            Error::Unsupported(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Box2i {
    pub min_x: i32,
    pub min_y: i32,
    pub max_x: i32,
    pub max_y: i32,
}

impl Box2i {
    pub fn from_size(width: usize, height: usize) -> Self {
        Self {
            min_x: 0,
            min_y: 0,
            max_x: width.saturating_sub(1) as i32,
            max_y: height.saturating_sub(1) as i32,
        }
    }

    pub fn width(&self) -> Result<usize> {
        if self.max_x < self.min_x {
            return Err(Error::InvalidFormat(format!(
                "invalid data window x-range {}..{}",
                self.min_x, self.max_x
            )));
        }
        Ok((self.max_x - self.min_x + 1) as usize)
    }

    pub fn height(&self) -> Result<usize> {
        if self.max_y < self.min_y {
            return Err(Error::InvalidFormat(format!(
                "invalid data window y-range {}..{}",
                self.min_y, self.max_y
            )));
        }
        Ok((self.max_y - self.min_y + 1) as usize)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    None,
    Rle,
    Zips,
    Zip,
    Piz,
    Pxr24,
    B44,
    B44A,
    Dwaa,
    Dwab,
    Htj2k256,
    Htj2k32,
}

impl Compression {
    fn from_u8(value: u8) -> Result<Self> {
        Ok(match value {
            0 => Self::None,
            1 => Self::Rle,
            2 => Self::Zips,
            3 => Self::Zip,
            4 => Self::Piz,
            5 => Self::Pxr24,
            6 => Self::B44,
            7 => Self::B44A,
            8 => Self::Dwaa,
            9 => Self::Dwab,
            10 => Self::Htj2k256,
            11 => Self::Htj2k32,
            _ => {
                return Err(Error::Unsupported(format!(
                    "unsupported compression code {value}"
                )))
            }
        })
    }

    fn to_u8(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Rle => 1,
            Self::Zips => 2,
            Self::Zip => 3,
            Self::Piz => 4,
            Self::Pxr24 => 5,
            Self::B44 => 6,
            Self::B44A => 7,
            Self::Dwaa => 8,
            Self::Dwab => 9,
            Self::Htj2k256 => 10,
            Self::Htj2k32 => 11,
        }
    }

    fn lines_per_chunk(self) -> usize {
        match self {
            Self::None | Self::Rle | Self::Zips => 1,
            Self::Zip | Self::Piz | Self::Pxr24 => 16,
            Self::B44 | Self::B44A => 32,
            Self::Dwaa => 32,
            Self::Dwab => 256,
            Self::Htj2k256 | Self::Htj2k32 => 32,
        }
    }

    fn is_basic_supported(self) -> bool {
        matches!(self, Self::None | Self::Zips | Self::Zip | Self::Pxr24)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineOrder {
    IncreasingY,
    DecreasingY,
    RandomY,
}

impl LineOrder {
    fn from_u8(value: u8) -> Result<Self> {
        Ok(match value {
            0 => Self::IncreasingY,
            1 => Self::DecreasingY,
            2 => Self::RandomY,
            _ => {
                return Err(Error::Unsupported(format!(
                    "unsupported line order code {value}"
                )))
            }
        })
    }

    fn to_u8(self) -> u8 {
        match self {
            Self::IncreasingY => 0,
            Self::DecreasingY => 1,
            Self::RandomY => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelType {
    Uint,
    Half,
    Float,
}

impl PixelType {
    fn from_i32(value: i32) -> Result<Self> {
        Ok(match value {
            0 => Self::Uint,
            1 => Self::Half,
            2 => Self::Float,
            _ => {
                return Err(Error::Unsupported(format!(
                    "unsupported channel pixel type {value}"
                )))
            }
        })
    }

    fn to_i32(self) -> i32 {
        match self {
            Self::Uint => 0,
            Self::Half => 1,
            Self::Float => 2,
        }
    }

    fn bytes_per_sample(self) -> usize {
        match self {
            Self::Half => 2,
            Self::Uint | Self::Float => 4,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SampleBuffer {
    Half(Vec<f16>),
    Float(Vec<f32>),
    Uint(Vec<u32>),
}

impl SampleBuffer {
    fn new(pixel_type: PixelType, len: usize) -> Self {
        match pixel_type {
            PixelType::Half => Self::Half(vec![f16::from_bits(0); len]),
            PixelType::Float => Self::Float(vec![0.0; len]),
            PixelType::Uint => Self::Uint(vec![0; len]),
        }
    }

    pub fn pixel_type(&self) -> PixelType {
        match self {
            Self::Half(_) => PixelType::Half,
            Self::Float(_) => PixelType::Float,
            Self::Uint(_) => PixelType::Uint,
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Self::Half(values) => values.len(),
            Self::Float(values) => values.len(),
            Self::Uint(values) => values.len(),
        }
    }

    fn write_row_to(&self, row: usize, width: usize, out: &mut Vec<u8>) {
        let start = row * width;
        let end = start + width;
        match self {
            Self::Half(values) => {
                for sample in &values[start..end] {
                    out.extend_from_slice(&sample.to_bits().to_le_bytes());
                }
            }
            Self::Float(values) => {
                for sample in &values[start..end] {
                    out.extend_from_slice(&sample.to_le_bytes());
                }
            }
            Self::Uint(values) => {
                for sample in &values[start..end] {
                    out.extend_from_slice(&sample.to_le_bytes());
                }
            }
        }
    }

    fn read_row_from(&mut self, row: usize, width: usize, bytes: &[u8]) -> Result<()> {
        let start = row * width;
        let end = start + width;
        match self {
            Self::Half(values) => {
                if bytes.len() != width * 2 {
                    return Err(Error::InvalidFormat(format!(
                        "half row expected {} bytes, got {}",
                        width * 2,
                        bytes.len()
                    )));
                }
                for (slot, chunk) in values[start..end].iter_mut().zip(bytes.chunks_exact(2)) {
                    *slot = f16::from_bits(u16::from_le_bytes([chunk[0], chunk[1]]));
                }
            }
            Self::Float(values) => {
                if bytes.len() != width * 4 {
                    return Err(Error::InvalidFormat(format!(
                        "float row expected {} bytes, got {}",
                        width * 4,
                        bytes.len()
                    )));
                }
                for (slot, chunk) in values[start..end].iter_mut().zip(bytes.chunks_exact(4)) {
                    *slot = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                }
            }
            Self::Uint(values) => {
                if bytes.len() != width * 4 {
                    return Err(Error::InvalidFormat(format!(
                        "uint row expected {} bytes, got {}",
                        width * 4,
                        bytes.len()
                    )));
                }
                for (slot, chunk) in values[start..end].iter_mut().zip(bytes.chunks_exact(4)) {
                    *slot = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExrChannel {
    pub name: String,
    pub p_linear: bool,
    pub sampling: [i32; 2],
    pub samples: SampleBuffer,
}

impl ExrChannel {
    pub fn half(name: impl Into<String>, samples: Vec<f16>) -> Self {
        Self {
            name: name.into(),
            p_linear: false,
            sampling: [1, 1],
            samples: SampleBuffer::Half(samples),
        }
    }

    pub fn float(name: impl Into<String>, samples: Vec<f32>) -> Self {
        Self {
            name: name.into(),
            p_linear: false,
            sampling: [1, 1],
            samples: SampleBuffer::Float(samples),
        }
    }

    pub fn uint(name: impl Into<String>, samples: Vec<u32>) -> Self {
        Self {
            name: name.into(),
            p_linear: false,
            sampling: [1, 1],
            samples: SampleBuffer::Uint(samples),
        }
    }

    pub fn pixel_type(&self) -> PixelType {
        self.samples.pixel_type()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawAttribute {
    pub name: String,
    pub type_name: String,
    pub value: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExrPart {
    pub name: Option<String>,
    pub compression: Compression,
    pub display_window: Box2i,
    pub data_window: Box2i,
    pub line_order: LineOrder,
    pub pixel_aspect_ratio: f32,
    pub screen_window_center: [f32; 2],
    pub screen_window_width: f32,
    pub view: Option<String>,
    pub multi_view: Vec<String>,
    pub chunk_count: Option<i32>,
    pub channels: Vec<ExrChannel>,
    pub other_attributes: Vec<RawAttribute>,
}

impl ExrPart {
    pub fn new(
        name: Option<String>,
        width: usize,
        height: usize,
        compression: Compression,
        channels: Vec<ExrChannel>,
    ) -> Self {
        let data_window = Box2i::from_size(width, height);
        Self {
            name,
            compression,
            display_window: data_window,
            data_window,
            line_order: LineOrder::IncreasingY,
            pixel_aspect_ratio: 1.0,
            screen_window_center: [0.0, 0.0],
            screen_window_width: 1.0,
            view: None,
            multi_view: Vec::new(),
            chunk_count: None,
            channels,
            other_attributes: Vec::new(),
        }
    }

    fn empty() -> Self {
        Self {
            name: None,
            compression: Compression::None,
            display_window: Box2i::from_size(1, 1),
            data_window: Box2i::from_size(1, 1),
            line_order: LineOrder::IncreasingY,
            pixel_aspect_ratio: 1.0,
            screen_window_center: [0.0, 0.0],
            screen_window_width: 1.0,
            view: None,
            multi_view: Vec::new(),
            chunk_count: None,
            channels: Vec::new(),
            other_attributes: Vec::new(),
        }
    }

    pub fn width(&self) -> Result<usize> {
        self.data_window.width()
    }
    pub fn height(&self) -> Result<usize> {
        self.data_window.height()
    }

    fn pixel_count(&self) -> Result<usize> {
        let width = self.width()?;
        let height = self.height()?;
        width
            .checked_mul(height)
            .ok_or_else(|| Error::InvalidFormat("pixel count overflow".to_string()))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExrImage {
    pub parts: Vec<ExrPart>,
}

impl ExrImage {
    pub fn single(part: ExrPart) -> Self {
        Self { parts: vec![part] }
    }
}

pub fn read_file(path: impl AsRef<Path>) -> Result<ExrImage> {
    read_from_slice(&std::fs::read(path)?)
}

pub fn write_file(path: impl AsRef<Path>, image: &ExrImage) -> Result<()> {
    std::fs::write(path, write_to_vec(image)?)?;
    Ok(())
}

pub fn read_from_slice(bytes: &[u8]) -> Result<ExrImage> {
    let mut cursor = ByteCursor::new(bytes);
    let magic = cursor.read_u32()?;
    if magic != EXR_MAGIC {
        return Err(Error::InvalidMagic(magic));
    }

    let version = cursor.read_u32()?;
    let version_number = version & 0xff;
    if version_number != EXR_VERSION {
        return Err(Error::Unsupported(format!(
            "unsupported OpenEXR file version {version_number}"
        )));
    }

    let multipart = (version & MULTIPART_FLAG) != 0;
    let name_limit = if (version & LONG_NAMES_FLAG) != 0 {
        255
    } else {
        31
    };

    let mut parts = parse_headers(&mut cursor, multipart, name_limit)?;
    finalize_parts_for_read(&mut parts)?;

    let total_chunk_count = total_chunk_count(&parts, multipart)?;
    let mut chunk_offsets = Vec::with_capacity(total_chunk_count);
    for _ in 0..total_chunk_count {
        chunk_offsets.push(cursor.read_u64()?);
    }

    for offset in chunk_offsets {
        let offset = usize::try_from(offset)
            .map_err(|_| Error::InvalidFormat("chunk offset does not fit in usize".to_string()))?;
        read_chunk(bytes, offset, multipart, &mut parts)?;
    }

    Ok(ExrImage { parts })
}

pub fn write_to_vec(image: &ExrImage) -> Result<Vec<u8>> {
    if image.parts.is_empty() {
        return Err(Error::InvalidFormat(
            "cannot write an EXR with zero parts".to_string(),
        ));
    }

    let multipart = image.parts.len() > 1;
    let long_names = image
        .parts
        .iter()
        .flat_map(|part| {
            part.channels
                .iter()
                .map(|channel| channel.name.len())
                .chain(part.other_attributes.iter().map(|attr| attr.name.len()))
                .chain(
                    part.other_attributes
                        .iter()
                        .map(|attr| attr.type_name.len()),
                )
        })
        .any(|len| len > 31);

    let mut parts = image.parts.clone();
    finalize_parts_for_write(&mut parts, multipart)?;

    let mut version = EXR_VERSION;
    if multipart {
        version |= MULTIPART_FLAG;
    }
    if long_names {
        version |= LONG_NAMES_FLAG;
    }

    let mut out = Vec::new();
    out.extend_from_slice(&EXR_MAGIC.to_le_bytes());
    out.extend_from_slice(&version.to_le_bytes());

    for part in &parts {
        write_header(&mut out, part, multipart, long_names)?;
    }
    if multipart {
        out.push(0);
    }

    let total_chunk_count = total_chunk_count(&parts, multipart)?;
    let offset_table_start = out.len();
    out.resize(out.len() + total_chunk_count * 8, 0);

    let mut chunk_offsets = Vec::with_capacity(total_chunk_count);
    for (part_index, part) in parts.iter().enumerate() {
        let lines_per_chunk = part.compression.lines_per_chunk();
        let height = part.height()?;
        let min_y = part.data_window.min_y;
        for start_row in (0..height).step_by(lines_per_chunk) {
            chunk_offsets.push(out.len() as u64);
            if multipart {
                let part_index = u32::try_from(part_index).map_err(|_| {
                    Error::InvalidFormat("part index does not fit in u32".to_string())
                })?;
                out.extend_from_slice(&part_index.to_le_bytes());
            }

            let start_y = min_y + start_row as i32;
            out.extend_from_slice(&start_y.to_le_bytes());

            let lines = (height - start_row).min(lines_per_chunk);
            let raw = build_scanline_chunk(part, start_row, lines)?;
            let packed = match part.compression {
                Compression::None => raw.clone(),
                Compression::Zips | Compression::Zip => compress_zip_block(&raw)?,
                Compression::Pxr24 => compress_pxr24_block(part, &raw, lines)?,
                compression => {
                    return Err(Error::Unsupported(format!(
                        "writing compression {compression:?} is not implemented yet"
                    )))
                }
            };

            let payload = if packed.len() < raw.len() {
                &packed
            } else {
                &raw
            };
            let packed_size = u32::try_from(payload.len())
                .map_err(|_| Error::InvalidFormat("chunk payload too large".to_string()))?;
            out.extend_from_slice(&packed_size.to_le_bytes());
            out.extend_from_slice(payload);
        }
    }

    for (index, offset) in chunk_offsets.iter().enumerate() {
        let start = offset_table_start + index * 8;
        out[start..start + 8].copy_from_slice(&offset.to_le_bytes());
    }

    Ok(out)
}

fn parse_headers(
    cursor: &mut ByteCursor<'_>,
    multipart: bool,
    name_limit: usize,
) -> Result<Vec<ExrPart>> {
    let mut parts = Vec::new();
    loop {
        match parse_single_header(cursor, multipart, name_limit)? {
            Some(part) => parts.push(part),
            None => {
                if multipart {
                    break;
                }
                return Err(Error::InvalidFormat(
                    "single-part file contained an empty header".to_string(),
                ));
            }
        }
        if !multipart {
            break;
        }
    }

    if parts.is_empty() {
        return Err(Error::InvalidFormat(
            "EXR file did not contain any headers".to_string(),
        ));
    }
    Ok(parts)
}

fn parse_single_header(
    cursor: &mut ByteCursor<'_>,
    multipart: bool,
    name_limit: usize,
) -> Result<Option<ExrPart>> {
    let mut part = ExrPart::empty();
    let mut saw_attribute = false;

    loop {
        let attr_name = cursor.read_cstring(name_limit)?;
        if attr_name.is_empty() {
            if !saw_attribute {
                return if multipart {
                    Ok(None)
                } else {
                    Err(Error::InvalidFormat(
                        "missing required EXR header".to_string(),
                    ))
                };
            }
            break;
        }

        saw_attribute = true;
        let type_name = cursor.read_cstring(name_limit)?;
        let size = cursor.read_i32()?;
        if size < 0 {
            return Err(Error::InvalidFormat(format!(
                "attribute {attr_name} has negative payload size {size}"
            )));
        }
        let payload = cursor.read_bytes(size as usize)?;
        parse_attribute(&mut part, &attr_name, &type_name, payload, name_limit)?;
    }

    Ok(Some(part))
}

fn parse_attribute(
    part: &mut ExrPart,
    name: &str,
    type_name: &str,
    payload: &[u8],
    name_limit: usize,
) -> Result<()> {
    match (name, type_name) {
        ("channels", "chlist") => {
            part.channels = parse_chlist(payload, name_limit)?;
        }
        ("compression", "compression") => {
            if payload.len() != 1 {
                return Err(Error::InvalidFormat(
                    "compression attribute must be 1 byte".to_string(),
                ));
            }
            part.compression = Compression::from_u8(payload[0])?;
        }
        ("dataWindow", "box2i") => part.data_window = parse_box2i(payload)?,
        ("displayWindow", "box2i") => part.display_window = parse_box2i(payload)?,
        ("lineOrder", "lineOrder") => {
            if payload.len() != 1 {
                return Err(Error::InvalidFormat(
                    "lineOrder attribute must be 1 byte".to_string(),
                ));
            }
            part.line_order = LineOrder::from_u8(payload[0])?;
        }
        ("pixelAspectRatio", "float") => part.pixel_aspect_ratio = parse_f32_payload(payload)?,
        ("screenWindowCenter", "v2f") => part.screen_window_center = parse_v2f(payload)?,
        ("screenWindowWidth", "float") => part.screen_window_width = parse_f32_payload(payload)?,
        ("name", "string") | ("name", "text") => part.name = Some(parse_string_payload(payload)?),
        ("view", "string") | ("view", "text") => part.view = Some(parse_string_payload(payload)?),
        ("multiView", "stringvector") => part.multi_view = parse_stringvector(payload)?,
        ("chunkCount", "int") => part.chunk_count = Some(parse_i32_payload(payload)?),
        ("type", "string") | ("type", "text") => {
            let part_type = parse_string_payload(payload)?;
            if part_type != "scanlineimage" {
                return Err(Error::Unsupported(format!(
                    "unsupported EXR part type {part_type:?}"
                )));
            }
        }
        _ => {
            part.other_attributes.push(RawAttribute {
                name: name.to_string(),
                type_name: type_name.to_string(),
                value: payload.to_vec(),
            });
        }
    }
    Ok(())
}

fn parse_chlist(payload: &[u8], name_limit: usize) -> Result<Vec<ExrChannel>> {
    let mut cursor = ByteCursor::new(payload);
    let mut channels = Vec::new();

    loop {
        let name = cursor.read_cstring(name_limit)?;
        if name.is_empty() {
            break;
        }

        let pixel_type = PixelType::from_i32(cursor.read_i32()?)?;
        let p_linear = cursor.read_u8()? != 0;
        cursor.skip(3)?;
        let x_sampling = cursor.read_i32()?;
        let y_sampling = cursor.read_i32()?;
        channels.push(ExrChannel {
            name,
            p_linear,
            sampling: [x_sampling, y_sampling],
            samples: SampleBuffer::new(pixel_type, 0),
        });
    }

    Ok(channels)
}

fn parse_box2i(payload: &[u8]) -> Result<Box2i> {
    if payload.len() != 16 {
        return Err(Error::InvalidFormat(format!(
            "box2i payload must be 16 bytes, got {}",
            payload.len()
        )));
    }
    Ok(Box2i {
        min_x: i32::from_le_bytes(payload[0..4].try_into().unwrap()),
        min_y: i32::from_le_bytes(payload[4..8].try_into().unwrap()),
        max_x: i32::from_le_bytes(payload[8..12].try_into().unwrap()),
        max_y: i32::from_le_bytes(payload[12..16].try_into().unwrap()),
    })
}

fn parse_i32_payload(payload: &[u8]) -> Result<i32> {
    if payload.len() != 4 {
        return Err(Error::InvalidFormat(format!(
            "int payload must be 4 bytes, got {}",
            payload.len()
        )));
    }
    Ok(i32::from_le_bytes(payload.try_into().unwrap()))
}

fn parse_f32_payload(payload: &[u8]) -> Result<f32> {
    if payload.len() != 4 {
        return Err(Error::InvalidFormat(format!(
            "float payload must be 4 bytes, got {}",
            payload.len()
        )));
    }
    Ok(f32::from_le_bytes(payload.try_into().unwrap()))
}

fn parse_v2f(payload: &[u8]) -> Result<[f32; 2]> {
    if payload.len() != 8 {
        return Err(Error::InvalidFormat(format!(
            "v2f payload must be 8 bytes, got {}",
            payload.len()
        )));
    }
    Ok([
        f32::from_le_bytes(payload[0..4].try_into().unwrap()),
        f32::from_le_bytes(payload[4..8].try_into().unwrap()),
    ])
}

fn parse_string_payload(payload: &[u8]) -> Result<String> {
    String::from_utf8(payload.to_vec())
        .map_err(|_| Error::InvalidFormat("invalid UTF-8 string attribute".to_string()))
}

fn parse_stringvector(payload: &[u8]) -> Result<Vec<String>> {
    let mut cursor = ByteCursor::new(payload);
    let mut values = Vec::new();
    while !cursor.is_empty() {
        let len = cursor.read_i32()?;
        if len < 0 {
            return Err(Error::InvalidFormat(
                "stringvector entry had negative length".to_string(),
            ));
        }
        let bytes = cursor.read_bytes(len as usize)?;
        values.push(
            String::from_utf8(bytes.to_vec()).map_err(|_| {
                Error::InvalidFormat("invalid UTF-8 stringvector entry".to_string())
            })?,
        );
    }
    Ok(values)
}

fn finalize_parts_for_read(parts: &mut [ExrPart]) -> Result<()> {
    for part in parts {
        validate_part(part, false, false)?;
        let pixel_count = part.pixel_count()?;
        for channel in &mut part.channels {
            if channel.sampling != [1, 1] {
                return Err(Error::Unsupported(format!(
                    "channel {} uses unsupported sampling {:?}",
                    channel.name, channel.sampling
                )));
            }
            channel.samples = SampleBuffer::new(channel.pixel_type(), pixel_count);
        }
    }
    Ok(())
}

fn finalize_parts_for_write(parts: &mut [ExrPart], multipart: bool) -> Result<()> {
    for part in parts {
        validate_part(part, multipart, true)?;
        if !part.compression.is_basic_supported() {
            return Err(Error::Unsupported(format!(
                "compression {:?} is not implemented yet",
                part.compression
            )));
        }
        if part.line_order != LineOrder::IncreasingY {
            return Err(Error::Unsupported(format!(
                "writing line order {:?} is not implemented yet",
                part.line_order
            )));
        }
    }
    Ok(())
}

fn validate_part(part: &ExrPart, multipart: bool, require_samples: bool) -> Result<()> {
    let width = part.width()?;
    let height = part.height()?;
    let pixel_count = width
        .checked_mul(height)
        .ok_or_else(|| Error::InvalidFormat("pixel count overflow".to_string()))?;

    if part.channels.is_empty() {
        return Err(Error::InvalidFormat(
            "part must contain at least one channel".to_string(),
        ));
    }

    if multipart && part.name.as_deref().unwrap_or("").is_empty() {
        return Err(Error::InvalidFormat(
            "multipart EXR parts must have a non-empty name".to_string(),
        ));
    }

    for channel in &part.channels {
        if channel.sampling != [1, 1] {
            return Err(Error::Unsupported(format!(
                "channel {} uses unsupported sampling {:?}",
                channel.name, channel.sampling
            )));
        }
        if require_samples && channel.samples.len() != pixel_count {
            return Err(Error::InvalidFormat(format!(
                "channel {} expected {pixel_count} samples, found {}",
                channel.name,
                channel.samples.len()
            )));
        }
    }

    ensure_unique_channel_names(part)?;
    Ok(())
}

fn ensure_unique_channel_names(part: &ExrPart) -> Result<()> {
    let mut names: Vec<&str> = part
        .channels
        .iter()
        .map(|channel| channel.name.as_str())
        .collect();
    names.sort_unstable();
    for pair in names.windows(2) {
        if pair[0] == pair[1] {
            return Err(Error::InvalidFormat(format!(
                "duplicate channel name {:?}",
                pair[0]
            )));
        }
    }
    Ok(())
}

fn total_chunk_count(parts: &[ExrPart], multipart: bool) -> Result<usize> {
    if multipart {
        let mut total = 0usize;
        for part in parts {
            total = total
                .checked_add(chunk_count_for_part(part)?)
                .ok_or_else(|| Error::InvalidFormat("chunk count overflow".to_string()))?;
        }
        Ok(total)
    } else {
        chunk_count_for_part(&parts[0])
    }
}

fn chunk_count_for_part(part: &ExrPart) -> Result<usize> {
    if let Some(chunk_count) = part.chunk_count {
        if chunk_count < 0 {
            return Err(Error::InvalidFormat(
                "chunkCount attribute cannot be negative".to_string(),
            ));
        }
        return Ok(chunk_count as usize);
    }

    let height = part.height()?;
    Ok(height.div_ceil(part.compression.lines_per_chunk()))
}

fn read_chunk(bytes: &[u8], offset: usize, multipart: bool, parts: &mut [ExrPart]) -> Result<()> {
    let mut cursor = ByteCursor::new(bytes);
    cursor.set_pos(offset)?;

    let part_index = if multipart {
        usize::try_from(cursor.read_u32()?)
            .map_err(|_| Error::InvalidFormat("part index overflow".to_string()))?
    } else {
        0
    };
    let part = parts
        .get_mut(part_index)
        .ok_or_else(|| Error::InvalidFormat(format!("invalid part index {part_index}")))?;

    let y = cursor.read_i32()?;
    let packed_size = cursor.read_u32()? as usize;
    let packed = cursor.read_bytes(packed_size)?;
    let lines_per_chunk = part.compression.lines_per_chunk();

    if y < part.data_window.min_y || y > part.data_window.max_y {
        return Err(Error::InvalidFormat(format!(
            "chunk y-coordinate {y} is outside data window {}..{}",
            part.data_window.min_y, part.data_window.max_y
        )));
    }

    let start_row = (y - part.data_window.min_y) as usize;
    let lines = (part.height()? - start_row).min(lines_per_chunk);
    let expected_size = bytes_per_scanline(part)?
        .checked_mul(lines)
        .ok_or_else(|| Error::InvalidFormat("chunk byte size overflow".to_string()))?;

    let raw = match part.compression {
        Compression::None => {
            if packed_size != expected_size {
                return Err(Error::InvalidFormat(format!(
                    "uncompressed chunk expected {expected_size} bytes, found {packed_size}"
                )));
            }
            packed.to_vec()
        }
        Compression::Zips | Compression::Zip => {
            if packed_size < expected_size {
                decompress_zip_block(packed, expected_size)?
            } else if packed_size == expected_size {
                packed.to_vec()
            } else {
                return Err(Error::InvalidFormat(format!(
                    "chunk payload {packed_size} is larger than its uncompressed size {expected_size}"
                )));
            }
        }
        Compression::Pxr24 => {
            if packed_size < expected_size {
                decompress_pxr24_block(part, packed, lines, expected_size)?
            } else if packed_size == expected_size {
                packed.to_vec()
            } else {
                return Err(Error::InvalidFormat(format!(
                    "chunk payload {packed_size} is larger than its uncompressed size {expected_size}"
                )));
            }
        }
        compression => {
            return Err(Error::Unsupported(format!(
                "reading compression {compression:?} is not implemented yet"
            )))
        }
    };

    load_scanline_chunk(part, start_row, lines, &raw)
}

fn bytes_per_scanline(part: &ExrPart) -> Result<usize> {
    let width = part.width()?;
    part.channels.iter().try_fold(0usize, |acc, channel| {
        acc.checked_add(width * channel.pixel_type().bytes_per_sample())
            .ok_or_else(|| Error::InvalidFormat("scanline byte size overflow".to_string()))
    })
}

fn load_scanline_chunk(
    part: &mut ExrPart,
    start_row: usize,
    lines: usize,
    raw: &[u8],
) -> Result<()> {
    let width = part.width()?;
    let channel_order = sorted_channel_indices(part);
    let mut cursor = 0usize;

    for row in start_row..start_row + lines {
        for channel_index in channel_order.iter().copied() {
            let channel = &mut part.channels[channel_index];
            let row_bytes = width * channel.pixel_type().bytes_per_sample();
            let end = cursor + row_bytes;
            if end > raw.len() {
                return Err(Error::UnexpectedEof);
            }
            channel
                .samples
                .read_row_from(row, width, &raw[cursor..end])?;
            cursor = end;
        }
    }

    if cursor != raw.len() {
        return Err(Error::InvalidFormat(format!(
            "chunk payload had {} trailing bytes",
            raw.len() - cursor
        )));
    }
    Ok(())
}

fn build_scanline_chunk(part: &ExrPart, start_row: usize, lines: usize) -> Result<Vec<u8>> {
    let width = part.width()?;
    let mut raw = Vec::with_capacity(
        bytes_per_scanline(part)?
            .checked_mul(lines)
            .ok_or_else(|| Error::InvalidFormat("chunk byte size overflow".to_string()))?,
    );
    let channel_order = sorted_channel_indices(part);
    for row in start_row..start_row + lines {
        for channel_index in channel_order.iter().copied() {
            part.channels[channel_index]
                .samples
                .write_row_to(row, width, &mut raw);
        }
    }
    Ok(raw)
}

fn sorted_channel_indices(part: &ExrPart) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..part.channels.len()).collect();
    indices.sort_by(|&a, &b| part.channels[a].name.cmp(&part.channels[b].name));
    indices
}

fn write_header(
    out: &mut Vec<u8>,
    part: &ExrPart,
    multipart: bool,
    long_names: bool,
) -> Result<()> {
    let name_limit = if long_names { 255 } else { 31 };

    if multipart {
        write_attribute(
            out,
            "name",
            "string",
            &string_payload(part.name.as_deref().unwrap()),
        )?;
        write_attribute(out, "type", "string", &string_payload("scanlineimage"))?;
        write_attribute(
            out,
            "chunkCount",
            "int",
            &(chunk_count_for_part(part)? as i32).to_le_bytes(),
        )?;
        if let Some(view) = &part.view {
            write_attribute(out, "view", "string", &string_payload(view))?;
        }
    } else if !part.multi_view.is_empty() {
        write_attribute(
            out,
            "multiView",
            "stringvector",
            &stringvector_payload(&part.multi_view)?,
        )?;
    }

    write_attribute(
        out,
        "channels",
        "chlist",
        &chlist_payload(part, name_limit)?,
    )?;
    write_attribute(
        out,
        "compression",
        "compression",
        &[part.compression.to_u8()],
    )?;
    write_attribute(out, "dataWindow", "box2i", &box2i_payload(part.data_window))?;
    write_attribute(
        out,
        "displayWindow",
        "box2i",
        &box2i_payload(part.display_window),
    )?;
    write_attribute(out, "lineOrder", "lineOrder", &[part.line_order.to_u8()])?;
    write_attribute(
        out,
        "pixelAspectRatio",
        "float",
        &part.pixel_aspect_ratio.to_le_bytes(),
    )?;
    write_attribute(
        out,
        "screenWindowCenter",
        "v2f",
        &v2f_payload(part.screen_window_center),
    )?;
    write_attribute(
        out,
        "screenWindowWidth",
        "float",
        &part.screen_window_width.to_le_bytes(),
    )?;

    for attribute in &part.other_attributes {
        write_attribute(out, &attribute.name, &attribute.type_name, &attribute.value)?;
    }

    out.push(0);
    Ok(())
}

fn write_attribute(out: &mut Vec<u8>, name: &str, type_name: &str, payload: &[u8]) -> Result<()> {
    write_cstring(out, name)?;
    write_cstring(out, type_name)?;
    let size = i32::try_from(payload.len())
        .map_err(|_| Error::InvalidFormat("attribute payload too large".to_string()))?;
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(payload);
    Ok(())
}

fn write_cstring(out: &mut Vec<u8>, value: &str) -> Result<()> {
    if value.as_bytes().contains(&0) {
        return Err(Error::InvalidFormat(format!(
            "string {:?} contains an embedded NUL byte",
            value
        )));
    }
    out.extend_from_slice(value.as_bytes());
    out.push(0);
    Ok(())
}

fn string_payload(value: &str) -> Vec<u8> {
    value.as_bytes().to_vec()
}

fn stringvector_payload(values: &[String]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for value in values {
        let len = i32::try_from(value.len())
            .map_err(|_| Error::InvalidFormat("stringvector entry too large".to_string()))?;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(value.as_bytes());
    }
    Ok(out)
}

fn box2i_payload(value: Box2i) -> Vec<u8> {
    let mut out = Vec::with_capacity(16);
    out.extend_from_slice(&value.min_x.to_le_bytes());
    out.extend_from_slice(&value.min_y.to_le_bytes());
    out.extend_from_slice(&value.max_x.to_le_bytes());
    out.extend_from_slice(&value.max_y.to_le_bytes());
    out
}

fn v2f_payload(value: [f32; 2]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8);
    out.extend_from_slice(&value[0].to_le_bytes());
    out.extend_from_slice(&value[1].to_le_bytes());
    out
}

fn chlist_payload(part: &ExrPart, _name_limit: usize) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for channel_index in sorted_channel_indices(part) {
        let channel = &part.channels[channel_index];
        write_cstring(&mut out, &channel.name)?;
        out.extend_from_slice(&channel.pixel_type().to_i32().to_le_bytes());
        out.push(channel.p_linear as u8);
        out.extend_from_slice(&[0, 0, 0]);
        out.extend_from_slice(&channel.sampling[0].to_le_bytes());
        out.extend_from_slice(&channel.sampling[1].to_le_bytes());
    }
    out.push(0);
    Ok(out)
}

fn compress_zip_block(raw: &[u8]) -> Result<Vec<u8>> {
    let mut transformed = interleave_even_odd_bytes(raw);
    zip_predict_encode(&mut transformed);
    Ok(makepad_fast_inflate::zlib_compress(&transformed, 6))
}

fn decompress_zip_block(packed: &[u8], expected_size: usize) -> Result<Vec<u8>> {
    let mut transformed =
        makepad_fast_inflate::zlib_decompress_vec_with_hint(packed, expected_size)
            .map_err(|err| Error::InvalidFormat(format!("zlib inflate failed: {err}")))?;
    if transformed.len() != expected_size {
        return Err(Error::InvalidFormat(format!(
            "zip block inflated to {} bytes, expected {expected_size}",
            transformed.len()
        )));
    }
    zip_predict_decode(&mut transformed);
    Ok(deinterleave_even_odd_bytes(&transformed))
}

fn compress_pxr24_block(part: &ExrPart, raw: &[u8], lines: usize) -> Result<Vec<u8>> {
    if cfg!(target_endian = "big") {
        return Err(Error::Unsupported(
            "PXR24 is only implemented on little-endian targets".to_string(),
        ));
    }

    let width = part.width()?;
    let channel_order = sorted_channel_indices(part);
    let transformed_len = pxr24_transformed_size(part, lines)?;
    let mut transformed = vec![0u8; transformed_len];
    let mut read_cursor = 0usize;
    let mut write_cursor = 0usize;

    for _ in 0..lines {
        for channel_index in channel_order.iter().copied() {
            let channel = &part.channels[channel_index];
            let plane_len = width;
            match channel.pixel_type() {
                PixelType::Half => {
                    let (plane0, rest) = transformed[write_cursor..].split_at_mut(plane_len);
                    let plane1 = &mut rest[..plane_len];
                    let mut previous = 0u32;
                    for sample in 0..width {
                        let pixel = u16::from_le_bytes(
                            raw[read_cursor..read_cursor + 2].try_into().unwrap(),
                        ) as u32;
                        let diff = (pixel.wrapping_sub(previous) as u16).to_le_bytes();
                        plane0[sample] = diff[1];
                        plane1[sample] = diff[0];
                        previous = pixel;
                        read_cursor += 2;
                    }
                    write_cursor += plane_len * 2;
                }
                PixelType::Uint => {
                    let (plane0, rest) = transformed[write_cursor..].split_at_mut(plane_len);
                    let (plane1, rest) = rest.split_at_mut(plane_len);
                    let (plane2, plane3) = rest.split_at_mut(plane_len);
                    let mut previous = 0u32;
                    for sample in 0..width {
                        let pixel = u32::from_le_bytes(
                            raw[read_cursor..read_cursor + 4].try_into().unwrap(),
                        );
                        let diff = pixel.wrapping_sub(previous).to_le_bytes();
                        plane0[sample] = diff[3];
                        plane1[sample] = diff[2];
                        plane2[sample] = diff[1];
                        plane3[sample] = diff[0];
                        previous = pixel;
                        read_cursor += 4;
                    }
                    write_cursor += plane_len * 4;
                }
                PixelType::Float => {
                    let (plane0, rest) = transformed[write_cursor..].split_at_mut(plane_len);
                    let (plane1, plane2) = rest.split_at_mut(plane_len);
                    let mut previous = 0u32;
                    for sample in 0..width {
                        let pixel = f32_to_f24(f32::from_le_bytes(
                            raw[read_cursor..read_cursor + 4].try_into().unwrap(),
                        ));
                        let diff = pixel.wrapping_sub(previous).to_le_bytes();
                        plane0[sample] = diff[2];
                        plane1[sample] = diff[1];
                        plane2[sample] = diff[0];
                        previous = pixel;
                        read_cursor += 4;
                    }
                    write_cursor += plane_len * 3;
                }
            }
        }
    }

    Ok(makepad_fast_inflate::zlib_compress(&transformed, 4))
}

fn decompress_pxr24_block(
    part: &ExrPart,
    packed: &[u8],
    lines: usize,
    expected_size: usize,
) -> Result<Vec<u8>> {
    if cfg!(target_endian = "big") {
        return Err(Error::Unsupported(
            "PXR24 is only implemented on little-endian targets".to_string(),
        ));
    }

    let transformed_size = pxr24_transformed_size(part, lines)?;
    let transformed = makepad_fast_inflate::zlib_decompress_vec_with_hint(packed, transformed_size)
        .map_err(|err| Error::InvalidFormat(format!("zlib inflate failed: {err}")))?;
    if transformed.len() != transformed_size {
        return Err(Error::InvalidFormat(format!(
            "PXR24 block inflated to {} bytes, expected {transformed_size}",
            transformed.len()
        )));
    }

    let width = part.width()?;
    let channel_order = sorted_channel_indices(part);
    let mut read_cursor = 0usize;
    let mut out = Vec::with_capacity(expected_size);

    for _ in 0..lines {
        for channel_index in channel_order.iter().copied() {
            let channel = &part.channels[channel_index];
            let plane_len = width;
            match channel.pixel_type() {
                PixelType::Half => {
                    let plane0 = &transformed[read_cursor..read_cursor + plane_len];
                    let plane1 = &transformed[read_cursor + plane_len..read_cursor + plane_len * 2];
                    let mut accumulated = 0u32;
                    for sample in 0..width {
                        let diff = u16::from_le_bytes([plane1[sample], plane0[sample]]) as u32;
                        accumulated = accumulated.wrapping_add(diff);
                        out.extend_from_slice(&(accumulated as u16).to_le_bytes());
                    }
                    read_cursor += plane_len * 2;
                }
                PixelType::Uint => {
                    let plane0 = &transformed[read_cursor..read_cursor + plane_len];
                    let plane1 = &transformed[read_cursor + plane_len..read_cursor + plane_len * 2];
                    let plane2 =
                        &transformed[read_cursor + plane_len * 2..read_cursor + plane_len * 3];
                    let plane3 =
                        &transformed[read_cursor + plane_len * 3..read_cursor + plane_len * 4];
                    let mut accumulated = 0u32;
                    for sample in 0..width {
                        let diff = u32::from_le_bytes([
                            plane3[sample],
                            plane2[sample],
                            plane1[sample],
                            plane0[sample],
                        ]);
                        accumulated = accumulated.wrapping_add(diff);
                        out.extend_from_slice(&accumulated.to_le_bytes());
                    }
                    read_cursor += plane_len * 4;
                }
                PixelType::Float => {
                    let plane0 = &transformed[read_cursor..read_cursor + plane_len];
                    let plane1 = &transformed[read_cursor + plane_len..read_cursor + plane_len * 2];
                    let plane2 =
                        &transformed[read_cursor + plane_len * 2..read_cursor + plane_len * 3];
                    let mut accumulated = 0u32;
                    for sample in 0..width {
                        let diff =
                            u32::from_le_bytes([0, plane2[sample], plane1[sample], plane0[sample]]);
                        accumulated = accumulated.wrapping_add(diff);
                        out.extend_from_slice(&accumulated.to_le_bytes());
                    }
                    read_cursor += plane_len * 3;
                }
            }
        }
    }

    if out.len() != expected_size {
        return Err(Error::InvalidFormat(format!(
            "PXR24 block reconstructed {} bytes, expected {expected_size}",
            out.len()
        )));
    }

    Ok(out)
}

fn pxr24_transformed_size(part: &ExrPart, lines: usize) -> Result<usize> {
    let width = part.width()?;
    let bytes_per_pixel = part.channels.iter().try_fold(0usize, |acc, channel| {
        let encoded = match channel.pixel_type() {
            PixelType::Half => 2usize,
            PixelType::Float => 3usize,
            PixelType::Uint => 4usize,
        };
        acc.checked_add(encoded)
            .ok_or_else(|| Error::InvalidFormat("PXR24 byte size overflow".to_string()))
    })?;
    width
        .checked_mul(lines)
        .and_then(|count| count.checked_mul(bytes_per_pixel))
        .ok_or_else(|| Error::InvalidFormat("PXR24 byte size overflow".to_string()))
}

fn f32_to_f24(float: f32) -> u32 {
    let bits = float.to_bits();
    let sign = bits & 0x8000_0000;
    let exponent = bits & 0x7f80_0000;
    let mantissa = bits & 0x007f_ffff;

    let reduced = if exponent == 0x7f80_0000 {
        if mantissa != 0 {
            let mantissa = mantissa >> 8;
            (exponent >> 8) | mantissa | if mantissa == 0 { 1 } else { 0 }
        } else {
            exponent >> 8
        }
    } else {
        let rounded = ((exponent | mantissa) + (mantissa & 0x80)) >> 8;
        if rounded >= 0x007f_8000 {
            (exponent | mantissa) >> 8
        } else {
            rounded
        }
    };

    (sign >> 8) | reduced
}

fn interleave_even_odd_bytes(raw: &[u8]) -> Vec<u8> {
    let len = raw.len();
    let even_len = len.div_ceil(2);
    let mut out = Vec::with_capacity(len);
    out.resize(len, 0);

    for i in 0..even_len {
        out[i] = raw[i * 2];
    }
    for i in 0..(len / 2) {
        out[even_len + i] = raw[i * 2 + 1];
    }
    out
}

fn deinterleave_even_odd_bytes(raw: &[u8]) -> Vec<u8> {
    let len = raw.len();
    let even_len = len.div_ceil(2);
    let mut out = Vec::with_capacity(len);
    out.resize(len, 0);

    for i in 0..even_len {
        out[i * 2] = raw[i];
    }
    for i in 0..(len / 2) {
        out[i * 2 + 1] = raw[even_len + i];
    }
    out
}

fn zip_predict_encode(bytes: &mut [u8]) {
    if bytes.is_empty() {
        return;
    }
    let mut previous = bytes[0];
    for byte in &mut bytes[1..] {
        let current = *byte;
        *byte = current.wrapping_sub(previous).wrapping_add(128);
        previous = current;
    }
}

fn zip_predict_decode(bytes: &mut [u8]) {
    for index in 1..bytes.len() {
        let previous = bytes[index - 1];
        bytes[index] = previous.wrapping_add(bytes[index]).wrapping_sub(128);
    }
}

struct ByteCursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> ByteCursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn is_empty(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    fn set_pos(&mut self, pos: usize) -> Result<()> {
        if pos > self.bytes.len() {
            return Err(Error::UnexpectedEof);
        }
        self.pos = pos;
        Ok(())
    }

    fn skip(&mut self, len: usize) -> Result<()> {
        self.read_bytes(len)?;
        Ok(())
    }

    fn read_bytes(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(len).ok_or(Error::UnexpectedEof)?;
        if end > self.bytes.len() {
            return Err(Error::UnexpectedEof);
        }
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn read_u8(&mut self) -> Result<u8> {
        Ok(self.read_bytes(1)?[0])
    }

    fn read_u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.read_bytes(4)?.try_into().unwrap()))
    }

    fn read_i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.read_bytes(4)?.try_into().unwrap()))
    }

    fn read_u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.read_bytes(8)?.try_into().unwrap()))
    }

    fn read_cstring(&mut self, max_len: usize) -> Result<String> {
        let tail = &self.bytes[self.pos..];
        let nul = tail
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(|| Error::InvalidFormat("unterminated C string".to_string()))?;
        if nul > max_len {
            return Err(Error::InvalidFormat(format!(
                "string length {nul} exceeds limit {max_len}"
            )));
        }
        let bytes = self.read_bytes(nul + 1)?;
        String::from_utf8(bytes[..nul].to_vec())
            .map_err(|_| Error::InvalidFormat("invalid UTF-8 C string".to_string()))
    }
}
