use brotli::Decompressor;
use makepad_fast_inflate::gzip_decompress_vec;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

const HEADER_LENGTH: usize = 66;
const BLOCK_DEFINITION_LENGTH: usize = 33;
const TILE_INDEX_ENTRY_LENGTH: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeoBounds {
    pub west: f64,
    pub south: f64,
    pub east: f64,
    pub north: f64,
}

impl GeoBounds {
    pub const EUROPE: Self = Self {
        west: -32.683_233,
        south: 29.635_548,
        east: 46.753_480,
        north: 81.472_990,
    };

    pub fn parse(value: &str) -> Result<Self, String> {
        let values = value
            .split(',')
            .map(str::trim)
            .map(str::parse::<f64>)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| format!("invalid --bbox '{value}': {err}"))?;
        if values.len() != 4 {
            return Err(format!(
                "invalid --bbox '{value}': expected west,south,east,north"
            ));
        }
        let bounds = Self {
            west: values[0],
            south: values[1],
            east: values[2],
            north: values[3],
        };
        if !(-180.0..=180.0).contains(&bounds.west)
            || !(-180.0..=180.0).contains(&bounds.east)
            || !(-85.051_128_78..=85.051_128_78).contains(&bounds.south)
            || !(-85.051_128_78..=85.051_128_78).contains(&bounds.north)
            || bounds.west >= bounds.east
            || bounds.south >= bounds.north
        {
            return Err(format!("invalid geographic bounds '{value}'"));
        }
        Ok(bounds)
    }

    pub fn as_csv(self) -> String {
        format!(
            "{:.7},{:.7},{:.7},{:.7}",
            self.west, self.south, self.east, self.north
        )
    }

    pub fn center(self) -> (f64, f64) {
        (
            (self.west + self.east) * 0.5,
            (self.south + self.north) * 0.5,
        )
    }

    pub fn tile_bounds(self, zoom: u8) -> TileBounds {
        let axis = 1_u32 << zoom;
        let x_min = longitude_to_tile_x(self.west, axis);
        let x_max = longitude_to_tile_x(self.east, axis);
        let y_min = latitude_to_tile_y(self.north, axis);
        let y_max = latitude_to_tile_y(self.south, axis);
        TileBounds {
            x_min,
            y_min,
            x_max,
            y_max,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileBounds {
    pub x_min: u32,
    pub y_min: u32,
    pub x_max: u32,
    pub y_max: u32,
}

impl TileBounds {
    pub fn contains(self, x: u32, y: u32) -> bool {
        x >= self.x_min && x <= self.x_max && y >= self.y_min && y <= self.y_max
    }

    pub fn intersects(self, other: Self) -> bool {
        self.x_min <= other.x_max
            && other.x_min <= self.x_max
            && self.y_min <= other.y_max
            && other.y_min <= self.y_max
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileCompression {
    Uncompressed,
    Gzip,
    Brotli,
    Zstd,
}

#[derive(Clone, Debug)]
pub struct VersaTilesHeader {
    pub min_zoom: u8,
    pub max_zoom: u8,
    pub compression: TileCompression,
    pub bounds: GeoBounds,
    pub metadata_offset: u64,
    pub metadata_length: u64,
    pub blocks_offset: u64,
    pub blocks_length: u64,
}

#[derive(Clone, Debug)]
pub struct BlockDefinition {
    pub zoom: u8,
    pub block_x: u32,
    pub block_y: u32,
    pub x_min: u32,
    pub y_min: u32,
    pub x_max: u32,
    pub y_max: u32,
    pub tiles_offset: u64,
    pub tiles_length: u64,
    pub index_offset: u64,
    pub index_length: u32,
}

impl BlockDefinition {
    pub fn bounds(&self) -> TileBounds {
        TileBounds {
            x_min: self.x_min,
            y_min: self.y_min,
            x_max: self.x_max,
            y_max: self.y_max,
        }
    }

    pub fn tile_count(&self) -> usize {
        (self.x_max - self.x_min + 1) as usize
            * (self.y_max - self.y_min + 1) as usize
    }

    pub fn sort_key(&self) -> (u8, u32, u32) {
        (self.zoom, self.block_y, self.block_x)
    }
}

pub struct VersaTilesReader {
    file: File,
    pub header: VersaTilesHeader,
    pub metadata_json: Vec<u8>,
    pub blocks: Vec<BlockDefinition>,
}

impl VersaTilesReader {
    pub fn open(path: &Path) -> Result<Self, String> {
        let mut file =
            File::open(path).map_err(|err| format!("open {}: {err}", path.display()))?;
        let mut header_bytes = [0_u8; HEADER_LENGTH];
        file.read_exact(&mut header_bytes)
            .map_err(|err| format!("read {} header: {err}", path.display()))?;
        let header = parse_header(&header_bytes)?;

        let metadata_blob =
            read_range(&mut file, header.metadata_offset, header.metadata_length)
                .map_err(|err| format!("read VersaTiles metadata: {err}"))?;
        let metadata_json = decompress_metadata(&metadata_blob, header.compression)?;

        let block_index_blob = read_range(&mut file, header.blocks_offset, header.blocks_length)
            .map_err(|err| format!("read VersaTiles block index: {err}"))?;
        let block_index = decompress_brotli(&block_index_blob)
            .map_err(|err| format!("decompress VersaTiles block index: {err}"))?;
        if block_index.len() % BLOCK_DEFINITION_LENGTH != 0 {
            return Err(format!(
                "VersaTiles block index has invalid length {}",
                block_index.len()
            ));
        }

        let mut blocks = block_index
            .chunks_exact(BLOCK_DEFINITION_LENGTH)
            .map(parse_block)
            .collect::<Result<Vec<_>, _>>()?;
        blocks.sort_unstable_by_key(BlockDefinition::sort_key);

        Ok(Self {
            file,
            header,
            metadata_json,
            blocks,
        })
    }

    pub fn open_tile_reader(&self, path: &Path) -> Result<CachedRangeReader, String> {
        CachedRangeReader::open(path)
            .map_err(|err| format!("open tile data {}: {err}", path.display()))
    }

    pub fn read_tile_index(&mut self, block: &BlockDefinition) -> Result<Vec<TileRange>, String> {
        let compressed =
            read_range(&mut self.file, block.index_offset, u64::from(block.index_length))
                .map_err(|err| {
                    format!(
                        "read tile index z{} block {}/{}: {err}",
                        block.zoom, block.block_x, block.block_y
                    )
                })?;
        let bytes = decompress_brotli(&compressed).map_err(|err| {
            format!(
                "decompress tile index z{} block {}/{}: {err}",
                block.zoom, block.block_x, block.block_y
            )
        })?;
        let expected = block.tile_count() * TILE_INDEX_ENTRY_LENGTH;
        if bytes.len() != expected {
            return Err(format!(
                "tile index z{} block {}/{} is {} bytes, expected {expected}",
                block.zoom,
                block.block_x,
                block.block_y,
                bytes.len()
            ));
        }

        let mut ranges = Vec::with_capacity(block.tile_count());
        for entry in bytes.chunks_exact(TILE_INDEX_ENTRY_LENGTH) {
            let relative_offset = read_u64(entry, 0);
            let length = read_u32(entry, 8);
            if relative_offset
                .checked_add(u64::from(length))
                .is_none_or(|end| end > block.tiles_length)
            {
                return Err(format!(
                    "tile range {}+{} exceeds z{} block {}/{} data length {}",
                    relative_offset,
                    length,
                    block.zoom,
                    block.block_x,
                    block.block_y,
                    block.tiles_length
                ));
            }
            ranges.push(TileRange {
                offset: block.tiles_offset + relative_offset,
                length,
            });
        }
        Ok(ranges)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TileRange {
    pub offset: u64,
    pub length: u32,
}

pub struct CachedRangeReader {
    file: File,
    file_len: u64,
    cache_offset: u64,
    cache: Vec<u8>,
}

impl CachedRangeReader {
    const CACHE_BYTES: usize = 8 * 1024 * 1024;

    fn open(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        let file_len = file.metadata()?.len();
        Ok(Self {
            file,
            file_len,
            cache_offset: 0,
            cache: Vec::new(),
        })
    }

    pub fn read(&mut self, offset: u64, length: u32) -> io::Result<Vec<u8>> {
        let length = length as usize;
        let end = offset
            .checked_add(length as u64)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "tile range overflow"))?;
        if end > self.file_len {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "tile range extends past source file",
            ));
        }

        let cache_end = self.cache_offset + self.cache.len() as u64;
        if offset >= self.cache_offset && end <= cache_end {
            let start = (offset - self.cache_offset) as usize;
            return Ok(self.cache[start..start + length].to_vec());
        }

        if length > Self::CACHE_BYTES {
            return read_range(&mut self.file, offset, length as u64);
        }

        let available = (self.file_len - offset).min(Self::CACHE_BYTES as u64) as usize;
        self.cache.resize(available, 0);
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read_exact(&mut self.cache)?;
        self.cache_offset = offset;
        Ok(self.cache[..length].to_vec())
    }
}

fn parse_header(bytes: &[u8; HEADER_LENGTH]) -> Result<VersaTilesHeader, String> {
    if &bytes[0..14] != b"versatiles_v02" {
        return Err("source is not a VersaTiles v02 archive".to_string());
    }
    if bytes[14] != 0x20 {
        return Err(format!(
            "source contains tile format 0x{:02x}, expected MVT/PBF (0x20)",
            bytes[14]
        ));
    }
    let compression = match bytes[15] {
        0 => TileCompression::Uncompressed,
        1 => TileCompression::Gzip,
        2 => TileCompression::Brotli,
        3 => TileCompression::Zstd,
        value => return Err(format!("unknown VersaTiles compression value {value}")),
    };
    if compression == TileCompression::Zstd {
        return Err("Zstd-compressed VersaTiles archives are not supported".to_string());
    }

    let min_zoom = bytes[16];
    let max_zoom = bytes[17];
    if min_zoom > max_zoom || max_zoom > 31 {
        return Err(format!(
            "invalid VersaTiles zoom range {min_zoom}..{max_zoom}"
        ));
    }

    let scale = 10_000_000.0;
    let bounds = GeoBounds {
        west: f64::from(read_i32(bytes, 18)) / scale,
        south: f64::from(read_i32(bytes, 22)) / scale,
        east: f64::from(read_i32(bytes, 26)) / scale,
        north: f64::from(read_i32(bytes, 30)) / scale,
    };
    let metadata_offset = read_u64(bytes, 34);
    let metadata_length = read_u64(bytes, 42);
    let blocks_offset = read_u64(bytes, 50);
    let blocks_length = read_u64(bytes, 58);

    Ok(VersaTilesHeader {
        min_zoom,
        max_zoom,
        compression,
        bounds,
        metadata_offset,
        metadata_length,
        blocks_offset,
        blocks_length,
    })
}

fn parse_block(bytes: &[u8]) -> Result<BlockDefinition, String> {
    let zoom = bytes[0];
    let block_x = read_u32(bytes, 1);
    let block_y = read_u32(bytes, 5);
    let local_x_min = u32::from(bytes[9]);
    let local_y_min = u32::from(bytes[10]);
    let local_x_max = u32::from(bytes[11]);
    let local_y_max = u32::from(bytes[12]);
    if local_x_min > local_x_max || local_y_min > local_y_max {
        return Err("VersaTiles block has inverted local bounds".to_string());
    }

    let tiles_offset = read_u64(bytes, 13);
    let tiles_length = read_u64(bytes, 21);
    let index_length = read_u32(bytes, 29);
    let x_offset = block_x
        .checked_mul(256)
        .ok_or_else(|| "VersaTiles block x offset overflow".to_string())?;
    let y_offset = block_y
        .checked_mul(256)
        .ok_or_else(|| "VersaTiles block y offset overflow".to_string())?;

    Ok(BlockDefinition {
        zoom,
        block_x,
        block_y,
        x_min: x_offset + local_x_min,
        y_min: y_offset + local_y_min,
        x_max: x_offset + local_x_max,
        y_max: y_offset + local_y_max,
        tiles_offset,
        tiles_length,
        index_offset: tiles_offset
            .checked_add(tiles_length)
            .ok_or_else(|| "VersaTiles block index offset overflow".to_string())?,
        index_length,
    })
}

fn decompress_metadata(bytes: &[u8], compression: TileCompression) -> Result<Vec<u8>, String> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    match compression {
        TileCompression::Uncompressed => Ok(bytes.to_vec()),
        TileCompression::Gzip => {
            gzip_decompress_vec(bytes).map_err(|err| format!("decompress gzip metadata: {err}"))
        }
        TileCompression::Brotli => {
            decompress_brotli(bytes).map_err(|err| format!("decompress Brotli metadata: {err}"))
        }
        TileCompression::Zstd => Err("Zstd metadata is not supported".to_string()),
    }
}

pub fn decompress_tile(bytes: &[u8], compression: TileCompression) -> Result<Vec<u8>, String> {
    match compression {
        TileCompression::Uncompressed => Ok(bytes.to_vec()),
        TileCompression::Gzip => {
            gzip_decompress_vec(bytes).map_err(|err| format!("decompress gzip tile: {err}"))
        }
        TileCompression::Brotli => {
            decompress_brotli(bytes).map_err(|err| format!("decompress Brotli tile: {err}"))
        }
        TileCompression::Zstd => Err("Zstd tile compression is not supported".to_string()),
    }
}

fn decompress_brotli(bytes: &[u8]) -> io::Result<Vec<u8>> {
    let mut decoder = Decompressor::new(bytes, 64 * 1024);
    let mut output = Vec::new();
    decoder.read_to_end(&mut output)?;
    Ok(output)
}

fn read_range(file: &mut File, offset: u64, length: u64) -> io::Result<Vec<u8>> {
    let length = usize::try_from(length)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "range is too large"))?;
    let mut bytes = vec![0; length];
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn longitude_to_tile_x(longitude: f64, axis: u32) -> u32 {
    let value = ((longitude + 180.0) / 360.0 * f64::from(axis)).floor();
    value.clamp(0.0, f64::from(axis - 1)) as u32
}

fn latitude_to_tile_y(latitude: f64, axis: u32) -> u32 {
    let latitude = latitude.clamp(-85.051_128_78, 85.051_128_78);
    let radians = latitude.to_radians();
    let value =
        (1.0 - (radians.tan().asinh() / std::f64::consts::PI)) * 0.5 * f64::from(axis);
    value.floor().clamp(0.0, f64::from(axis - 1)) as u32
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn read_i32(bytes: &[u8], offset: usize) -> i32 {
    i32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_be_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn europe_bounds_cover_amsterdam_at_each_zoom() {
        for zoom in 0..=14 {
            let europe = GeoBounds::EUROPE.tile_bounds(zoom);
            let amsterdam = GeoBounds {
                west: 4.89,
                south: 52.36,
                east: 4.90,
                north: 52.38,
            }
            .tile_bounds(zoom);
            assert!(europe.contains(amsterdam.x_min, amsterdam.y_min));
            assert!(europe.contains(amsterdam.x_max, amsterdam.y_max));
        }
    }

    #[test]
    fn parses_v02_header() {
        let mut bytes = [0_u8; HEADER_LENGTH];
        bytes[0..14].copy_from_slice(b"versatiles_v02");
        bytes[14] = 0x20;
        bytes[15] = 1;
        bytes[16] = 0;
        bytes[17] = 14;
        bytes[18..22].copy_from_slice(&(-1_800_000_000_i32).to_be_bytes());
        bytes[22..26].copy_from_slice(&(-850_511_287_i32).to_be_bytes());
        bytes[26..30].copy_from_slice(&(1_800_000_000_i32).to_be_bytes());
        bytes[30..34].copy_from_slice(&(850_511_287_i32).to_be_bytes());
        bytes[34..42].copy_from_slice(&66_u64.to_be_bytes());
        bytes[42..50].copy_from_slice(&100_u64.to_be_bytes());
        bytes[50..58].copy_from_slice(&166_u64.to_be_bytes());
        bytes[58..66].copy_from_slice(&200_u64.to_be_bytes());

        let header = parse_header(&bytes).unwrap();
        assert_eq!(header.min_zoom, 0);
        assert_eq!(header.max_zoom, 14);
        assert_eq!(header.metadata_offset, 66);
        assert_eq!(header.metadata_length, 100);
        assert_eq!(header.blocks_offset, 166);
        assert_eq!(header.blocks_length, 200);
    }
}
