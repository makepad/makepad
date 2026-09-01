use super::ini::Ini;
use std::fmt;

pub type RawSection = Vec<(String, String)>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MapBounds {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Waypoint {
    pub number: u32,
    pub cell: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Terrain {
    pub cell: u16,
    pub name: String,
    pub trigger: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Overlay {
    pub cell: u16,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Smudge {
    pub cell: u16,
    pub name: String,
    pub referenced_cell: i32,
    pub data: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Unit {
    pub number: u32,
    pub owner: String,
    pub kind: String,
    pub health: i32,
    pub cell: u16,
    pub facing: i32,
    pub mission: String,
    pub trigger: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Infantry {
    pub number: u32,
    pub owner: String,
    pub kind: String,
    pub health: i32,
    pub cell: u16,
    pub sub_cell: i32,
    pub mission: String,
    pub facing: i32,
    pub trigger: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Structure {
    pub number: u32,
    pub owner: String,
    pub kind: String,
    pub health: i32,
    pub cell: u16,
    pub facing: i32,
    pub trigger: String,
    pub sellable: Option<bool>,
    pub repairable: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct TdMap {
    cells: Vec<(u8, u8)>,
    pub theater: String,
    pub bounds: MapBounds,
    pub basic: RawSection,
    pub waypoints: Vec<Waypoint>,
    pub terrain: Vec<Terrain>,
    pub overlay: Vec<Overlay>,
    pub smudges: Vec<Smudge>,
    pub units: Vec<Unit>,
    pub infantry: Vec<Infantry>,
    pub structures: Vec<Structure>,
    pub base: RawSection,
    pub triggers: RawSection,
    pub team_types: RawSection,
    pub briefing: RawSection,
    pub cell_triggers: RawSection,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MapError {
    InvalidBinSize,
    MissingField { section: &'static str, key: &'static str },
    InvalidNumber { section: &'static str, key: String },
    InvalidCell { section: &'static str, cell: i32 },
    InvalidRecord { section: &'static str, key: String },
    InvalidBase64 { section: &'static str },
    InvalidPackedChunk { section: &'static str },
    PackedSize { section: &'static str, expected: usize, actual: usize },
    Lcw { section: &'static str, error: super::lcw::LcwError },
}

impl fmt::Display for MapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBinSize => f.write_str("a map BIN must contain exactly 8192 bytes"),
            Self::MissingField { section, key } => write!(f, "missing [{section}] {key}"),
            Self::InvalidNumber { section, key } => write!(f, "invalid number in [{section}] {key}"),
            Self::InvalidCell { section, cell } => write!(f, "cell {cell} in [{section}] is outside the map"),
            Self::InvalidRecord { section, key } => write!(f, "invalid [{section}] record {key}"),
            Self::InvalidBase64 { section } => write!(f, "invalid base64 in [{section}]"),
            Self::InvalidPackedChunk { section } => write!(f, "invalid packed chunk in [{section}]"),
            Self::PackedSize {
                section,
                expected,
                actual,
            } => write!(f, "[{section}] decoded to {actual} bytes, expected {expected}"),
            Self::Lcw { section, error } => write!(f, "LCW error in [{section}]: {error}"),
        }
    }
}

impl std::error::Error for MapError {}

impl TdMap {
    pub fn parse(ini: &Ini, bin: &[u8]) -> Result<Self, MapError> {
        if bin.len() != 64 * 64 * 2 {
            return Err(MapError::InvalidBinSize);
        }
        let cells = bin.chunks_exact(2).map(|cell| (cell[0], cell[1])).collect();
        let theater = required(ini, "Map", "Theater")?.to_owned();
        let bounds = MapBounds {
            x: required_number(ini, "Map", "X")?,
            y: required_number(ini, "Map", "Y")?,
            width: required_number(ini, "Map", "Width")?,
            height: required_number(ini, "Map", "Height")?,
        };
        if bounds.x as usize + bounds.width as usize > 64
            || bounds.y as usize + bounds.height as usize > 64
        {
            return Err(MapError::InvalidRecord {
                section: "Map",
                key: "bounds".to_owned(),
            });
        }

        Ok(Self {
            cells,
            theater,
            bounds,
            basic: raw(ini, "Basic"),
            waypoints: parse_waypoints(ini)?,
            terrain: parse_terrain(ini)?,
            overlay: parse_overlay(ini)?,
            smudges: parse_smudges(ini)?,
            units: parse_units(ini)?,
            infantry: parse_infantry(ini)?,
            structures: parse_structures(ini)?,
            base: raw(ini, "Base"),
            triggers: raw(ini, "TRIGGERS"),
            team_types: raw(ini, "TEAMTYPES"),
            briefing: raw(ini, "Briefing"),
            cell_triggers: raw(ini, "CellTriggers"),
        })
    }

    pub fn cell(&self, x: usize, y: usize) -> (u8, u8) {
        if x >= 64 || y >= 64 {
            return (0xff, 0);
        }
        self.cells[y * 64 + x]
    }
}

#[derive(Clone, Debug)]
pub struct RaMap {
    cells: Vec<(u16, u8)>,
    overlay_ids: Vec<u8>,
    pub theater: String,
    pub bounds: MapBounds,
    pub basic: RawSection,
    pub waypoints: Vec<Waypoint>,
    pub terrain: Vec<Terrain>,
    pub smudges: Vec<Smudge>,
    pub units: Vec<Unit>,
    pub infantry: Vec<Infantry>,
    pub structures: Vec<Structure>,
    pub ships: Vec<Unit>,
    pub base: RawSection,
    pub team_types: RawSection,
    pub triggers: RawSection,
    pub briefing: RawSection,
}

impl RaMap {
    pub fn parse(ini: &Ini) -> Result<Self, MapError> {
        let theater = required(ini, "Map", "Theater")?.to_owned();
        let bounds = MapBounds {
            x: required_number(ini, "Map", "X")?,
            y: required_number(ini, "Map", "Y")?,
            width: required_number(ini, "Map", "Width")?,
            height: required_number(ini, "Map", "Height")?,
        };
        if bounds.x as usize + bounds.width as usize > 128
            || bounds.y as usize + bounds.height as usize > 128
        {
            return Err(MapError::InvalidRecord {
                section: "Map",
                key: "bounds".to_owned(),
            });
        }

        let map_pack = decode_ra_packed_section(ini, "MapPack", 49_152)?;
        let overlay_ids = decode_ra_packed_section(ini, "OverlayPack", 16_384)?;
        let mut cells = Vec::with_capacity(16_384);
        for cell in 0..16_384 {
            let template_at = cell * 2;
            let template = u16::from_le_bytes([map_pack[template_at], map_pack[template_at + 1]]);
            cells.push((template, map_pack[32_768 + cell]));
        }

        Ok(Self {
            cells,
            overlay_ids,
            theater,
            bounds,
            basic: raw(ini, "Basic"),
            waypoints: parse_waypoints(ini)?,
            terrain: parse_ra_terrain(ini)?,
            smudges: parse_ra_smudges(ini)?,
            units: parse_ra_units(ini, "UNITS")?,
            infantry: parse_ra_infantry(ini)?,
            structures: parse_ra_structures(ini)?,
            ships: parse_ra_units(ini, "SHIPS")?,
            base: raw(ini, "Base"),
            team_types: raw(ini, "TeamTypes"),
            triggers: raw(ini, "Trigs"),
            briefing: raw(ini, "Briefing"),
        })
    }

    pub fn cell(&self, x: usize, y: usize) -> (u16, u8) {
        if x >= 128 || y >= 128 {
            return (0xffff, 0);
        }
        self.cells[y * 128 + x]
    }

    pub fn overlay(&self, x: usize, y: usize) -> Option<u8> {
        if x >= 128 || y >= 128 {
            return None;
        }
        match self.overlay_ids[y * 128 + x] {
            0xff => None,
            overlay => Some(overlay),
        }
    }
}

pub fn decode_ra_packed_section(
    ini: &Ini,
    section: &'static str,
    expected_size: usize,
) -> Result<Vec<u8>, MapError> {
    let mut pieces = entries(ini, section)
        .iter()
        .map(|(key, value)| {
            key.parse::<u32>()
                .map(|number| (number, value.as_str()))
                .map_err(|_| MapError::InvalidNumber {
                    section,
                    key: key.to_owned(),
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    pieces.sort_by_key(|&(number, _)| number);
    if pieces.is_empty() {
        return Err(MapError::MissingField { section, key: "1" });
    }
    let encoded = pieces
        .iter()
        .flat_map(|(_, piece)| piece.bytes())
        .collect::<Vec<_>>();
    let packed = makepad_base64::base64_decode(&encoded)
        .map_err(|_| MapError::InvalidBase64 { section })?;
    let mut output = Vec::with_capacity(expected_size);
    let mut at = 0usize;
    while at < packed.len() {
        let header_end = at.checked_add(4).ok_or(MapError::InvalidPackedChunk { section })?;
        let header = packed
            .get(at..header_end)
            .ok_or(MapError::InvalidPackedChunk { section })?;
        at = header_end;
        let compressed_len = u16::from_le_bytes([header[0], header[1]]) as usize;
        let uncompressed_len = u16::from_le_bytes([header[2], header[3]]) as usize;
        if uncompressed_len != 8192 || compressed_len == 0 {
            return Err(MapError::InvalidPackedChunk { section });
        }
        let end = at
            .checked_add(compressed_len)
            .ok_or(MapError::InvalidPackedChunk { section })?;
        let compressed = packed
            .get(at..end)
            .ok_or(MapError::InvalidPackedChunk { section })?;
        at = end;
        let mut chunk = Vec::with_capacity(uncompressed_len);
        let decoded = super::lcw::decode(compressed, &mut chunk)
            .map_err(|error| MapError::Lcw { section, error })?;
        if decoded != uncompressed_len || chunk.len() != uncompressed_len {
            return Err(MapError::InvalidPackedChunk { section });
        }
        output.extend_from_slice(&chunk);
        if output.len() > expected_size {
            return Err(MapError::PackedSize {
                section,
                expected: expected_size,
                actual: output.len(),
            });
        }
    }
    if output.len() != expected_size {
        return Err(MapError::PackedSize {
            section,
            expected: expected_size,
            actual: output.len(),
        });
    }
    Ok(output)
}

fn parse_ra_terrain(ini: &Ini) -> Result<Vec<Terrain>, MapError> {
    entries(ini, "TERRAIN")
        .iter()
        .map(|(key, value)| {
            Ok(Terrain {
                cell: cell_number_ra("TERRAIN", key)?,
                name: value.trim().to_owned(),
                trigger: String::new(),
            })
        })
        .collect()
}

fn parse_ra_smudges(ini: &Ini) -> Result<Vec<Smudge>, MapError> {
    entries(ini, "SMUDGE")
        .iter()
        .map(|(key, value)| {
            let values = fields("SMUDGE", key, value, 3)?;
            Ok(Smudge {
                cell: cell_number_ra("SMUDGE", key)?,
                name: values[0].to_owned(),
                referenced_cell: integer("SMUDGE", key, values[1])?,
                data: integer("SMUDGE", key, values[2])?,
            })
        })
        .collect()
}

fn parse_ra_units(ini: &Ini, section: &'static str) -> Result<Vec<Unit>, MapError> {
    entries(ini, section)
        .iter()
        .map(|(key, value)| {
            let values = fields(section, key, value, 7)?;
            Ok(Unit {
                number: record_number(section, key)?,
                owner: values[0].to_owned(),
                kind: values[1].to_owned(),
                health: integer(section, key, values[2])?,
                cell: cell_number_ra(section, values[3])?,
                facing: integer(section, key, values[4])?,
                mission: values[5].to_owned(),
                trigger: values[6].to_owned(),
            })
        })
        .collect()
}

fn parse_ra_infantry(ini: &Ini) -> Result<Vec<Infantry>, MapError> {
    entries(ini, "INFANTRY")
        .iter()
        .map(|(key, value)| {
            let values = fields("INFANTRY", key, value, 8)?;
            Ok(Infantry {
                number: record_number("INFANTRY", key)?,
                owner: values[0].to_owned(),
                kind: values[1].to_owned(),
                health: integer("INFANTRY", key, values[2])?,
                cell: cell_number_ra("INFANTRY", values[3])?,
                sub_cell: integer("INFANTRY", key, values[4])?,
                mission: values[5].to_owned(),
                facing: integer("INFANTRY", key, values[6])?,
                trigger: values[7].to_owned(),
            })
        })
        .collect()
}

fn parse_ra_structures(ini: &Ini) -> Result<Vec<Structure>, MapError> {
    entries(ini, "STRUCTURES")
        .iter()
        .map(|(key, value)| {
            let values = fields("STRUCTURES", key, value, 8)?;
            Ok(Structure {
                number: record_number("STRUCTURES", key)?,
                owner: values[0].to_owned(),
                kind: values[1].to_owned(),
                health: integer("STRUCTURES", key, values[2])?,
                cell: cell_number_ra("STRUCTURES", values[3])?,
                facing: integer("STRUCTURES", key, values[4])?,
                trigger: values[5].to_owned(),
                sellable: Some(boolean("STRUCTURES", key, values[6])?),
                repairable: Some(boolean("STRUCTURES", key, values[7])?),
            })
        })
        .collect()
}

fn cell_number_ra(section: &'static str, value: &str) -> Result<u16, MapError> {
    let cell = value.parse::<i32>().map_err(|_| MapError::InvalidNumber {
        section,
        key: value.to_owned(),
    })?;
    if !(0..16_384).contains(&cell) {
        return Err(MapError::InvalidCell { section, cell });
    }
    Ok(cell as u16)
}

fn boolean(section: &'static str, key: &str, value: &str) -> Result<bool, MapError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "yes" | "true" | "1" => Ok(true),
        "no" | "false" | "0" => Ok(false),
        _ => Err(MapError::InvalidNumber {
            section,
            key: key.to_owned(),
        }),
    }
}

fn required<'a>(ini: &'a Ini, section: &'static str, key: &'static str) -> Result<&'a str, MapError> {
    ini.get(section, key)
        .ok_or(MapError::MissingField { section, key })
}

fn required_number<T: std::str::FromStr>(
    ini: &Ini,
    section: &'static str,
    key: &'static str,
) -> Result<T, MapError> {
    required(ini, section, key)?
        .parse()
        .map_err(|_| MapError::InvalidNumber {
            section,
            key: key.to_owned(),
        })
}

fn raw(ini: &Ini, section: &str) -> RawSection {
    ini.section(section).unwrap_or_default().to_vec()
}

fn entries<'a>(ini: &'a Ini, section: &str) -> &'a [(String, String)] {
    ini.section(section).unwrap_or_default()
}

fn record_number(section: &'static str, key: &str) -> Result<u32, MapError> {
    key.parse().map_err(|_| MapError::InvalidNumber {
        section,
        key: key.to_owned(),
    })
}

fn cell_number(section: &'static str, value: &str) -> Result<u16, MapError> {
    let cell = value.parse::<i32>().map_err(|_| MapError::InvalidNumber {
        section,
        key: value.to_owned(),
    })?;
    if !(0..4096).contains(&cell) {
        return Err(MapError::InvalidCell { section, cell });
    }
    Ok(cell as u16)
}

fn integer(section: &'static str, key: &str, value: &str) -> Result<i32, MapError> {
    value.parse().map_err(|_| MapError::InvalidNumber {
        section,
        key: key.to_owned(),
    })
}

fn fields<'a>(
    section: &'static str,
    key: &str,
    value: &'a str,
    count: usize,
) -> Result<Vec<&'a str>, MapError> {
    let fields = value.split(',').map(str::trim).collect::<Vec<_>>();
    if fields.len() != count {
        return Err(MapError::InvalidRecord {
            section,
            key: key.to_owned(),
        });
    }
    Ok(fields)
}

fn parse_waypoints(ini: &Ini) -> Result<Vec<Waypoint>, MapError> {
    entries(ini, "Waypoints")
        .iter()
        .map(|(key, value)| {
            Ok(Waypoint {
                number: record_number("Waypoints", key)?,
                cell: integer("Waypoints", key, value)?,
            })
        })
        .collect()
}

fn parse_terrain(ini: &Ini) -> Result<Vec<Terrain>, MapError> {
    entries(ini, "TERRAIN")
        .iter()
        .map(|(key, value)| {
            let values = fields("TERRAIN", key, value, 2)?;
            Ok(Terrain {
                cell: cell_number("TERRAIN", key)?,
                name: values[0].to_owned(),
                trigger: values[1].to_owned(),
            })
        })
        .collect()
}

fn parse_overlay(ini: &Ini) -> Result<Vec<Overlay>, MapError> {
    entries(ini, "OVERLAY")
        .iter()
        .map(|(key, value)| {
            Ok(Overlay {
                cell: cell_number("OVERLAY", key)?,
                name: value.trim().to_owned(),
            })
        })
        .collect()
}

fn parse_smudges(ini: &Ini) -> Result<Vec<Smudge>, MapError> {
    entries(ini, "SMUDGE")
        .iter()
        .map(|(key, value)| {
            let values = fields("SMUDGE", key, value, 3)?;
            Ok(Smudge {
                cell: cell_number("SMUDGE", key)?,
                name: values[0].to_owned(),
                referenced_cell: integer("SMUDGE", key, values[1])?,
                data: integer("SMUDGE", key, values[2])?,
            })
        })
        .collect()
}

fn parse_units(ini: &Ini) -> Result<Vec<Unit>, MapError> {
    entries(ini, "UNITS")
        .iter()
        .map(|(key, value)| {
            let v = fields("UNITS", key, value, 7)?;
            Ok(Unit {
                number: record_number("UNITS", key)?,
                owner: v[0].to_owned(),
                kind: v[1].to_owned(),
                health: integer("UNITS", key, v[2])?,
                cell: cell_number("UNITS", v[3])?,
                facing: integer("UNITS", key, v[4])?,
                mission: v[5].to_owned(),
                trigger: v[6].to_owned(),
            })
        })
        .collect()
}

fn parse_infantry(ini: &Ini) -> Result<Vec<Infantry>, MapError> {
    entries(ini, "INFANTRY")
        .iter()
        .map(|(key, value)| {
            let v = fields("INFANTRY", key, value, 8)?;
            Ok(Infantry {
                number: record_number("INFANTRY", key)?,
                owner: v[0].to_owned(),
                kind: v[1].to_owned(),
                health: integer("INFANTRY", key, v[2])?,
                cell: cell_number("INFANTRY", v[3])?,
                sub_cell: integer("INFANTRY", key, v[4])?,
                mission: v[5].to_owned(),
                facing: integer("INFANTRY", key, v[6])?,
                trigger: v[7].to_owned(),
            })
        })
        .collect()
}

fn parse_structures(ini: &Ini) -> Result<Vec<Structure>, MapError> {
    entries(ini, "STRUCTURES")
        .iter()
        .map(|(key, value)| {
            let v = fields("STRUCTURES", key, value, 6)?;
            Ok(Structure {
                number: record_number("STRUCTURES", key)?,
                owner: v[0].to_owned(),
                kind: v[1].to_owned(),
                health: integer("STRUCTURES", key, v[2])?,
                cell: cell_number("STRUCTURES", v[3])?,
                facing: integer("STRUCTURES", key, v[4])?,
                trigger: v[5].to_owned(),
                sellable: None,
                repairable: None,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cnc_import_ra_map_reads_synthetic_base64_chunks() {
        let map_pack = packed_fill(6, 0);
        let overlay_pack = packed_fill(2, 0xff);
        let map_encoded = String::from_utf8(makepad_base64::base64_encode(
            &map_pack,
            &makepad_base64::BASE64_STANDARD,
        ))
        .unwrap();
        let overlay_encoded = String::from_utf8(makepad_base64::base64_encode(
            &overlay_pack,
            &makepad_base64::BASE64_STANDARD,
        ))
        .unwrap();
        let text = format!(
            "[Basic]\nName=Fixture\n[Map]\nTheater=TEMPERATE\nX=1\nY=2\nWidth=64\nHeight=64\n[MapPack]\n1={map_encoded}\n[OverlayPack]\n1={overlay_encoded}\n"
        );
        let map = RaMap::parse(&Ini::parse(&text)).unwrap();
        assert_eq!(map.cell(0, 0), (0, 0));
        assert_eq!(map.cell(127, 127), (0, 0));
        assert_eq!(map.cell(128, 0), (0xffff, 0));
        assert_eq!(map.overlay(0, 0), None);
    }

    fn packed_fill(chunks: usize, value: u8) -> Vec<u8> {
        let mut packed = Vec::new();
        for _ in 0..chunks {
            let compressed = [0xfe, 0x00, 0x20, value, 0x80];
            packed.extend_from_slice(&(compressed.len() as u16).to_le_bytes());
            packed.extend_from_slice(&8192u16.to_le_bytes());
            packed.extend_from_slice(&compressed);
        }
        packed
    }
}
