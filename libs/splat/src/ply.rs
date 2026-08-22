use crate::{Splat, SplatError, SplatFileFormat, SplatScene};
use std::io::{BufRead, BufReader, Cursor, Seek};

const SH_C0: f32 = 0.2820948;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PlyFormat {
    Ascii,
    BinaryLittleEndian,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PlyScalarType {
    Char,
    UChar,
    Short,
    UShort,
    Int,
    UInt,
    Float,
    Double,
}

#[derive(Clone, Debug)]
struct PlyProperty {
    name: String,
    scalar_type: PlyScalarType,
}

#[derive(Clone, Debug)]
struct PlyHeader {
    format: PlyFormat,
    vertex_count: usize,
    properties: Vec<PlyProperty>,
    data_offset: usize,
}

pub fn load_ply_from_bytes(bytes: &[u8]) -> Result<SplatScene, SplatError> {
    let header = parse_header(bytes)?;

    let mut scene = SplatScene::empty(SplatFileFormat::Ply);
    scene.splats.reserve(header.vertex_count);

    match header.format {
        PlyFormat::Ascii => parse_ascii_vertices(bytes, &header, &mut scene.splats)?,
        PlyFormat::BinaryLittleEndian => parse_binary_vertices(bytes, &header, &mut scene.splats)?,
    }

    scene.recompute_bounds();
    Ok(scene)
}

fn parse_header(bytes: &[u8]) -> Result<PlyHeader, SplatError> {
    let mut reader = BufReader::new(Cursor::new(bytes));
    let mut line = String::new();

    reader.read_line(&mut line)?;
    if line.trim_end_matches(['\r', '\n']) != "ply" {
        return Err(SplatError::InvalidData(
            "missing PLY signature on first line".to_string(),
        ));
    }

    let mut format: Option<PlyFormat> = None;
    let mut vertex_count: Option<usize> = None;
    let mut properties = Vec::new();
    let mut in_vertex_element = false;

    loop {
        line.clear();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            return Err(SplatError::InvalidData(
                "unexpected EOF while reading PLY header".to_string(),
            ));
        }

        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed == "end_header" {
            break;
        }

        if trimmed.is_empty() || trimmed.starts_with("comment") {
            continue;
        }

        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }

        match parts[0] {
            "format" => {
                if parts.len() < 2 {
                    return Err(SplatError::InvalidData(
                        "format line missing encoding".to_string(),
                    ));
                }
                format = Some(match parts[1] {
                    "ascii" => PlyFormat::Ascii,
                    "binary_little_endian" => PlyFormat::BinaryLittleEndian,
                    other => {
                        return Err(SplatError::Unsupported(format!(
                            "unsupported PLY format '{other}'"
                        )))
                    }
                });
            }
            "element" => {
                if parts.len() < 3 {
                    return Err(SplatError::InvalidData(
                        "element line missing payload".to_string(),
                    ));
                }
                in_vertex_element = parts[1] == "vertex";
                if in_vertex_element {
                    vertex_count = Some(parts[2].parse::<usize>().map_err(|_| {
                        SplatError::InvalidData("invalid vertex count in header".to_string())
                    })?);
                    properties.clear();
                }
            }
            "property" if in_vertex_element => {
                if parts.len() < 3 {
                    return Err(SplatError::InvalidData(
                        "vertex property line missing payload".to_string(),
                    ));
                }
                if parts[1] == "list" {
                    return Err(SplatError::Unsupported(
                        "list vertex properties are not supported for splat PLY".to_string(),
                    ));
                }
                let scalar_type = parse_scalar_type(parts[1])?;
                properties.push(PlyProperty {
                    name: parts[2].to_string(),
                    scalar_type,
                });
            }
            _ => {}
        }
    }

    let format = format.ok_or_else(|| SplatError::MissingField("format".to_string()))?;
    let vertex_count =
        vertex_count.ok_or_else(|| SplatError::MissingField("element vertex".to_string()))?;

    let data_offset = reader.stream_position()? as usize;

    Ok(PlyHeader {
        format,
        vertex_count,
        properties,
        data_offset,
    })
}

fn parse_scalar_type(value: &str) -> Result<PlyScalarType, SplatError> {
    match value {
        "char" | "int8" => Ok(PlyScalarType::Char),
        "uchar" | "uint8" => Ok(PlyScalarType::UChar),
        "short" | "int16" => Ok(PlyScalarType::Short),
        "ushort" | "uint16" => Ok(PlyScalarType::UShort),
        "int" | "int32" => Ok(PlyScalarType::Int),
        "uint" | "uint32" => Ok(PlyScalarType::UInt),
        "float" | "float32" => Ok(PlyScalarType::Float),
        "double" | "float64" => Ok(PlyScalarType::Double),
        other => Err(SplatError::Unsupported(format!(
            "unsupported PLY scalar type '{other}'"
        ))),
    }
}

fn parse_ascii_vertices(
    bytes: &[u8],
    header: &PlyHeader,
    out: &mut Vec<Splat>,
) -> Result<(), SplatError> {
    let mut reader = BufReader::new(Cursor::new(&bytes[header.data_offset..]));
    let mut line = String::new();

    let indices = PropertyIndices::new(&header.properties);

    for _ in 0..header.vertex_count {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Err(SplatError::InvalidData(
                "unexpected EOF while reading ascii vertex payload".to_string(),
            ));
        }
        if line.trim().is_empty() {
            continue;
        }

        let mut values = Vec::with_capacity(header.properties.len());
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.len() < header.properties.len() {
            return Err(SplatError::InvalidData(
                "ascii PLY vertex has fewer properties than header declares".to_string(),
            ));
        }

        for (property, token) in header.properties.iter().zip(tokens.iter()) {
            values.push(parse_ascii_scalar(*token, property.scalar_type)?);
        }

        out.push(build_splat_from_values(&values, &indices));
    }

    Ok(())
}

fn parse_binary_vertices(
    bytes: &[u8],
    header: &PlyHeader,
    out: &mut Vec<Splat>,
) -> Result<(), SplatError> {
    let fields = BinaryFields::new(&header.properties);
    let stride = fields.stride;
    let payload = &bytes[header.data_offset.min(bytes.len())..];
    let needed = header
        .vertex_count
        .checked_mul(stride)
        .ok_or_else(|| SplatError::InvalidData("PLY vertex payload size overflow".to_string()))?;
    if payload.len() < needed {
        return Err(SplatError::InvalidData(format!(
            "binary PLY payload truncated: need {} bytes for {} vertices, have {}",
            needed,
            header.vertex_count,
            payload.len()
        )));
    }
    let payload = &payload[..needed];

    let start = out.len();
    out.resize(start + header.vertex_count, Splat::ZERO);
    let dst = &mut out[start..];
    if stride == 0 {
        return Ok(());
    }

    // Decode rows in parallel chunks; each chunk writes its own slice of
    // `dst`, so no synchronization beyond the scoped join.
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 16);
    let min_rows_per_thread = 65_536;
    let chunk_rows = (header.vertex_count / threads)
        .max(min_rows_per_thread)
        .max(1);
    std::thread::scope(|scope| {
        for (dst_chunk, src_chunk) in dst
            .chunks_mut(chunk_rows)
            .zip(payload.chunks(chunk_rows * stride))
        {
            let fields = &fields;
            scope.spawn(move || {
                for (splat, row) in dst_chunk.iter_mut().zip(src_chunk.chunks_exact(stride)) {
                    *splat = fields.read_splat(row);
                }
            });
        }
    });
    Ok(())
}

/// Byte offset + scalar type of the splat properties inside one binary
/// vertex row, plus the row stride.
#[derive(Clone, Debug)]
struct BinaryFields {
    stride: usize,
    x: Option<(usize, PlyScalarType)>,
    y: Option<(usize, PlyScalarType)>,
    z: Option<(usize, PlyScalarType)>,
    dc0: Option<(usize, PlyScalarType)>,
    dc1: Option<(usize, PlyScalarType)>,
    dc2: Option<(usize, PlyScalarType)>,
    opacity: Option<(usize, PlyScalarType)>,
    scale0: Option<(usize, PlyScalarType)>,
    scale1: Option<(usize, PlyScalarType)>,
    scale2: Option<(usize, PlyScalarType)>,
    rot0: Option<(usize, PlyScalarType)>,
    rot1: Option<(usize, PlyScalarType)>,
    rot2: Option<(usize, PlyScalarType)>,
    rot3: Option<(usize, PlyScalarType)>,
}

impl BinaryFields {
    fn new(properties: &[PlyProperty]) -> Self {
        let mut out = Self {
            stride: 0,
            x: None,
            y: None,
            z: None,
            dc0: None,
            dc1: None,
            dc2: None,
            opacity: None,
            scale0: None,
            scale1: None,
            scale2: None,
            rot0: None,
            rot1: None,
            rot2: None,
            rot3: None,
        };
        let mut offset = 0usize;
        for property in properties {
            let at = Some((offset, property.scalar_type));
            match property.name.as_str() {
                "x" => out.x = at,
                "y" => out.y = at,
                "z" => out.z = at,
                "f_dc_0" => out.dc0 = at,
                "f_dc_1" => out.dc1 = at,
                "f_dc_2" => out.dc2 = at,
                "opacity" => out.opacity = at,
                "scale_0" => out.scale0 = at,
                "scale_1" => out.scale1 = at,
                "scale_2" => out.scale2 = at,
                "rot_0" => out.rot0 = at,
                "rot_1" => out.rot1 = at,
                "rot_2" => out.rot2 = at,
                "rot_3" => out.rot3 = at,
                _ => {}
            }
            offset += scalar_size(property.scalar_type);
        }
        out.stride = offset;
        out
    }

    #[inline]
    fn get(row: &[u8], field: Option<(usize, PlyScalarType)>) -> Option<f32> {
        field.map(|(offset, ty)| read_scalar_at(row, offset, ty))
    }

    #[inline]
    fn read_splat(&self, row: &[u8]) -> Splat {
        build_splat(RawSplatFields {
            x: Self::get(row, self.x),
            y: Self::get(row, self.y),
            z: Self::get(row, self.z),
            dc0: Self::get(row, self.dc0),
            dc1: Self::get(row, self.dc1),
            dc2: Self::get(row, self.dc2),
            opacity: Self::get(row, self.opacity),
            scale0: Self::get(row, self.scale0),
            scale1: Self::get(row, self.scale1),
            scale2: Self::get(row, self.scale2),
            rot0: Self::get(row, self.rot0),
            rot1: Self::get(row, self.rot1),
            rot2: Self::get(row, self.rot2),
            rot3: Self::get(row, self.rot3),
        })
    }
}

fn scalar_size(ty: PlyScalarType) -> usize {
    match ty {
        PlyScalarType::Char | PlyScalarType::UChar => 1,
        PlyScalarType::Short | PlyScalarType::UShort => 2,
        PlyScalarType::Int | PlyScalarType::UInt | PlyScalarType::Float => 4,
        PlyScalarType::Double => 8,
    }
}

/// Little-endian scalar at `offset` inside a row whose layout was validated
/// against the header stride (so the slice indexing cannot go out of range).
#[inline]
fn read_scalar_at(row: &[u8], offset: usize, ty: PlyScalarType) -> f32 {
    macro_rules! le {
        ($t:ty) => {{
            let size = std::mem::size_of::<$t>();
            let mut bytes = [0u8; std::mem::size_of::<$t>()];
            bytes.copy_from_slice(&row[offset..offset + size]);
            <$t>::from_le_bytes(bytes) as f32
        }};
    }
    match ty {
        PlyScalarType::Char => row[offset] as i8 as f32,
        PlyScalarType::UChar => row[offset] as f32,
        PlyScalarType::Short => le!(i16),
        PlyScalarType::UShort => le!(u16),
        PlyScalarType::Int => le!(i32),
        PlyScalarType::UInt => le!(u32),
        PlyScalarType::Float => le!(f32),
        PlyScalarType::Double => le!(f64),
    }
}

fn parse_ascii_scalar(token: &str, scalar_type: PlyScalarType) -> Result<f32, SplatError> {
    match scalar_type {
        PlyScalarType::Char => token
            .parse::<i8>()
            .map(|v| v as f32)
            .map_err(|_| SplatError::InvalidData("invalid int8 ascii value".to_string())),
        PlyScalarType::UChar => token
            .parse::<u8>()
            .map(|v| v as f32)
            .map_err(|_| SplatError::InvalidData("invalid uint8 ascii value".to_string())),
        PlyScalarType::Short => token
            .parse::<i16>()
            .map(|v| v as f32)
            .map_err(|_| SplatError::InvalidData("invalid int16 ascii value".to_string())),
        PlyScalarType::UShort => token
            .parse::<u16>()
            .map(|v| v as f32)
            .map_err(|_| SplatError::InvalidData("invalid uint16 ascii value".to_string())),
        PlyScalarType::Int => token
            .parse::<i32>()
            .map(|v| v as f32)
            .map_err(|_| SplatError::InvalidData("invalid int32 ascii value".to_string())),
        PlyScalarType::UInt => token
            .parse::<u32>()
            .map(|v| v as f32)
            .map_err(|_| SplatError::InvalidData("invalid uint32 ascii value".to_string())),
        PlyScalarType::Float => token
            .parse::<f32>()
            .map_err(|_| SplatError::InvalidData("invalid float ascii value".to_string())),
        PlyScalarType::Double => token
            .parse::<f64>()
            .map(|v| v as f32)
            .map_err(|_| SplatError::InvalidData("invalid double ascii value".to_string())),
    }
}

#[derive(Clone, Debug)]
struct PropertyIndices {
    x: Option<usize>,
    y: Option<usize>,
    z: Option<usize>,
    dc0: Option<usize>,
    dc1: Option<usize>,
    dc2: Option<usize>,
    opacity: Option<usize>,
    scale0: Option<usize>,
    scale1: Option<usize>,
    scale2: Option<usize>,
    rot0: Option<usize>,
    rot1: Option<usize>,
    rot2: Option<usize>,
    rot3: Option<usize>,
}

impl PropertyIndices {
    fn new(properties: &[PlyProperty]) -> Self {
        let mut out = Self {
            x: None,
            y: None,
            z: None,
            dc0: None,
            dc1: None,
            dc2: None,
            opacity: None,
            scale0: None,
            scale1: None,
            scale2: None,
            rot0: None,
            rot1: None,
            rot2: None,
            rot3: None,
        };

        for (index, property) in properties.iter().enumerate() {
            match property.name.as_str() {
                "x" => out.x = Some(index),
                "y" => out.y = Some(index),
                "z" => out.z = Some(index),
                "f_dc_0" => out.dc0 = Some(index),
                "f_dc_1" => out.dc1 = Some(index),
                "f_dc_2" => out.dc2 = Some(index),
                "opacity" => out.opacity = Some(index),
                "scale_0" => out.scale0 = Some(index),
                "scale_1" => out.scale1 = Some(index),
                "scale_2" => out.scale2 = Some(index),
                "rot_0" => out.rot0 = Some(index),
                "rot_1" => out.rot1 = Some(index),
                "rot_2" => out.rot2 = Some(index),
                "rot_3" => out.rot3 = Some(index),
                _ => {}
            }
        }

        out
    }
}

/// The splat-relevant scalars of one vertex, `None` when the file lacks the
/// property (defaults applied in `build_splat`).
#[derive(Clone, Copy, Debug, Default)]
struct RawSplatFields {
    x: Option<f32>,
    y: Option<f32>,
    z: Option<f32>,
    dc0: Option<f32>,
    dc1: Option<f32>,
    dc2: Option<f32>,
    opacity: Option<f32>,
    scale0: Option<f32>,
    scale1: Option<f32>,
    scale2: Option<f32>,
    rot0: Option<f32>,
    rot1: Option<f32>,
    rot2: Option<f32>,
    rot3: Option<f32>,
}

fn build_splat_from_values(values: &[f32], idx: &PropertyIndices) -> Splat {
    let get = |i: Option<usize>| i.and_then(|i| values.get(i)).copied();
    build_splat(RawSplatFields {
        x: get(idx.x),
        y: get(idx.y),
        z: get(idx.z),
        dc0: get(idx.dc0),
        dc1: get(idx.dc1),
        dc2: get(idx.dc2),
        opacity: get(idx.opacity),
        scale0: get(idx.scale0),
        scale1: get(idx.scale1),
        scale2: get(idx.scale2),
        rot0: get(idx.rot0),
        rot1: get(idx.rot1),
        rot2: get(idx.rot2),
        rot3: get(idx.rot3),
    })
}

#[inline]
fn build_splat(f: RawSplatFields) -> Splat {
    let x = f.x.unwrap_or(0.0);
    let y = f.y.unwrap_or(0.0);
    let z = f.z.unwrap_or(0.0);

    let dc0 = f.dc0.unwrap_or(0.0);
    let dc1 = f.dc1.unwrap_or(0.0);
    let dc2 = f.dc2.unwrap_or(0.0);

    let opacity = f.opacity.unwrap_or(1.0);

    let s0 = f.scale0.unwrap_or(-7.0).exp();
    let s1 = f.scale1.unwrap_or(-7.0).exp();
    let s2 = f.scale2.unwrap_or(-7.0).exp();

    let rotation = if let (Some(w), Some(x), Some(y), Some(z)) = (f.rot0, f.rot1, f.rot2, f.rot3) {
        normalize_quaternion([x, y, z, w])
    } else {
        [0.0, 0.0, 0.0, 1.0]
    };

    let color = [
        (0.5 + SH_C0 * dc0).clamp(0.0, 1.0),
        (0.5 + SH_C0 * dc1).clamp(0.0, 1.0),
        (0.5 + SH_C0 * dc2).clamp(0.0, 1.0),
        sigmoid(opacity).clamp(0.0, 1.0),
    ];

    Splat {
        position: [x, y, z],
        scale: [s0, s1, s2],
        rotation,
        color,
    }
}

fn normalize_quaternion(q: [f32; 4]) -> [f32; 4] {
    let len2 = q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3];
    if len2 <= f32::EPSILON {
        return [0.0, 0.0, 0.0, 1.0];
    }
    let inv_len = len2.sqrt().recip();
    [
        q[0] * inv_len,
        q[1] * inv_len,
        q[2] * inv_len,
        q[3] * inv_len,
    ]
}

fn sigmoid(v: f32) -> f32 {
    if v >= 0.0 {
        1.0 / (1.0 + (-v).exp())
    } else {
        let exp_v = v.exp();
        exp_v / (1.0 + exp_v)
    }
}
