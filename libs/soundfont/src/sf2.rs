use crate::error::LoadError;
use crate::model::{
    cents_to_hz, select_zones, timecents_to_seconds, Envelope, LoopMode, Range, SampleRead,
    SampleSource, VoiceParameters, VoiceSource, Zone,
};

const GEN_COUNT: usize = 61;
const INSTRUMENT: u16 = 41;
const KEY_RANGE: u16 = 43;
const VEL_RANGE: u16 = 44;
const SAMPLE_ID: u16 = 53;

/// Bounds all allocations made while parsing untrusted SF2 data.
#[derive(Clone, Copy, Debug)]
pub struct ParseLimits {
    pub max_chunk_bytes: usize,
    pub max_records: usize,
    pub max_sample_points: usize,
}

impl Default for ParseLimits {
    fn default() -> Self {
        Self {
            max_chunk_bytes: 512 * 1024 * 1024,
            max_records: 1_000_000,
            max_sample_points: 64 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InfoEntry {
    pub id: [u8; 4],
    pub value: String,
}

/// The union-shaped two-byte amount stored by an SF2 generator record.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GeneratorAmount(pub u16);

impl GeneratorAmount {
    pub const fn signed(self) -> i16 {
        self.0 as i16
    }

    pub const fn range(self) -> Range {
        Range {
            low: (self.0 & 0xff) as u8,
            high: (self.0 >> 8) as u8,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Generator {
    pub operator: u16,
    pub amount: GeneratorAmount,
}

/// Raw SF2 modulator record. Unknown source/transform bit patterns are kept.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modulator {
    pub source: u16,
    pub destination: u16,
    pub amount: i16,
    pub amount_source: u16,
    pub transform: u16,
}

impl Modulator {
    fn identity(self) -> (u16, u16, u16, u16) {
        (self.source, self.destination, self.amount_source, self.transform)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PresetZone {
    pub generators: Vec<Generator>,
    pub modulators: Vec<Modulator>,
    pub instrument: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preset {
    pub name: String,
    pub program: u16,
    pub bank: u16,
    pub library: u32,
    pub genre: u32,
    pub morphology: u32,
    pub zones: Vec<PresetZone>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstrumentZone {
    pub generators: Vec<Generator>,
    pub modulators: Vec<Modulator>,
    pub sample: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instrument {
    pub name: String,
    pub zones: Vec<InstrumentZone>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleKind {
    Mono,
    Right,
    Left,
    Linked,
    Unknown(u16),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SampleHeader {
    pub name: String,
    pub start: u32,
    pub end: u32,
    pub loop_start: u32,
    pub loop_end: u32,
    pub sample_rate: u32,
    pub original_pitch: u8,
    pub pitch_correction: i8,
    pub link: u16,
    pub kind: SampleKind,
    pub is_rom: bool,
}

/// Parsed hierarchy, decoded PCM, and control-thread resolved zones.
#[derive(Clone, Debug)]
pub struct SoundFont {
    pub info: Vec<InfoEntry>,
    pub presets: Vec<Preset>,
    pub instruments: Vec<Instrument>,
    pub samples: Vec<SampleHeader>,
    pub zones: Vec<Zone>,
    pcm: Vec<f32>,
    pub sample_precision: u8,
}

impl SoundFont {
    pub fn pcm(&self) -> &[f32] {
        &self.pcm
    }

    /// Select bank 0 zones for `(program, key, velocity)`.
    pub fn select(&self, program: u16, key: u8, velocity: u8) -> Vec<VoiceParameters> {
        self.select_bank(0, program, key, velocity)
    }

    pub fn select_bank(
        &self,
        bank: u16,
        program: u16,
        key: u8,
        velocity: u8,
    ) -> Vec<VoiceParameters> {
        select_zones(&self.zones, program, bank, key, velocity)
    }

    fn read_one(&self, sample: usize, frame: i64) -> Option<f32> {
        let header = self.samples.get(sample)?;
        if header.is_rom {
            return None;
        }
        let absolute = i64::from(header.start).checked_add(frame)?;
        if absolute < 0 || absolute >= i64::from(header.end) {
            return None;
        }
        self.pcm.get(absolute as usize).copied()
    }
}

impl SampleSource for SoundFont {
    fn read_frame(&self, sample_id: u32, frame: i64) -> SampleRead {
        let index = sample_id as usize;
        let Some(sample) = self.samples.get(index) else {
            return SampleRead::Missing;
        };
        if sample.is_rom {
            return SampleRead::Missing;
        }
        let pair = sample.link as usize;
        let result = match sample.kind {
            SampleKind::Mono | SampleKind::Unknown(_) => {
                self.read_one(index, frame).map(|value| (value, value))
            }
            SampleKind::Left => self
                .read_one(index, frame)
                .zip(self.read_one(pair, frame)),
            SampleKind::Right => self
                .read_one(pair, frame)
                .zip(self.read_one(index, frame)),
            SampleKind::Linked => self
                .read_one(index, frame)
                .zip(self.read_one(pair, frame)),
        };
        match result {
            Some((left, right)) => SampleRead::Resident { left, right },
            None => SampleRead::Missing,
        }
    }
}

#[derive(Clone, Copy)]
struct Chunk<'a> {
    id: [u8; 4],
    data: &'a [u8],
    data_offset: usize,
}

#[derive(Clone, Copy, Debug)]
struct Bag {
    generator: usize,
    modulator: usize,
}

#[derive(Clone, Debug)]
struct PresetRecord {
    name: String,
    program: u16,
    bank: u16,
    bag: usize,
    library: u32,
    genre: u32,
    morphology: u32,
}

#[derive(Clone, Debug)]
struct InstrumentRecord {
    name: String,
    bag: usize,
}

#[derive(Clone, Copy)]
struct GeneratorSet {
    values: [Option<GeneratorAmount>; GEN_COUNT],
}

impl GeneratorSet {
    fn empty() -> Self {
        Self { values: [None; GEN_COUNT] }
    }

    fn insert_zone(&mut self, generators: &[Generator]) {
        for generator in generators {
            if let Some(slot) = self.values.get_mut(generator.operator as usize) {
                *slot = Some(generator.amount);
            }
        }
    }

    fn amount(self, operator: u16) -> Option<GeneratorAmount> {
        self.values.get(operator as usize).copied().flatten()
    }

    fn signed(self, operator: u16, default: i32) -> i32 {
        self.amount(operator)
            .map(|amount| i32::from(amount.signed()))
            .unwrap_or(default)
    }
}

pub fn parse_sf2(bytes: &[u8]) -> Result<SoundFont, LoadError> {
    parse_sf2_with_limits(bytes, ParseLimits::default())
}

pub fn parse_sf2_with_limits(bytes: &[u8], limits: ParseLimits) -> Result<SoundFont, LoadError> {
    if bytes.len() < 12 {
        return Err(LoadError::Truncated { offset: 0, needed: 12 });
    }
    if bytes.get(0..4) != Some(b"RIFF") {
        return Err(LoadError::InvalidRiff);
    }
    let riff_size = le_u32(bytes, 4)? as usize;
    let riff_end = 8usize
        .checked_add(riff_size)
        .ok_or(LoadError::InvalidRiff)?;
    if riff_end > bytes.len() {
        return Err(LoadError::Truncated {
            offset: bytes.len(),
            needed: riff_end - bytes.len(),
        });
    }
    if bytes.get(8..12) != Some(b"sfbk") {
        return Err(LoadError::InvalidForm);
    }

    let top = walk_chunks(&bytes[12..riff_end], 12, limits)?;
    let mut info_list = None;
    let mut sdta_list = None;
    let mut pdta_list = None;
    for chunk in top {
        if chunk.id != *b"LIST" || chunk.data.len() < 4 {
            continue;
        }
        let kind = chunk.data.get(0..4).unwrap_or_default();
        let content = &chunk.data[4..];
        let offset = chunk.data_offset + 4;
        match kind {
            b"INFO" => set_once(&mut info_list, (content, offset), *b"INFO")?,
            b"sdta" => set_once(&mut sdta_list, (content, offset), *b"sdta")?,
            b"pdta" => set_once(&mut pdta_list, (content, offset), *b"pdta")?,
            _ => {}
        }
    }

    let (sdta, sdta_offset) = sdta_list.ok_or(LoadError::MissingChunk("LIST sdta"))?;
    let (pdta, pdta_offset) = pdta_list.ok_or(LoadError::MissingChunk("LIST pdta"))?;
    let info = parse_info(info_list, limits)?;
    let (pcm, precision) = parse_sample_data(sdta, sdta_offset, limits)?;
    let hydra = parse_hydra(pdta, pdta_offset, limits)?;
    let presets = build_presets(&hydra)?;
    let instruments = build_instruments(&hydra)?;
    let samples = parse_sample_headers(hydra.shdr, limits)?;
    validate_samples(&samples, pcm.len())?;
    let zones = resolve_zones(&presets, &instruments, &samples, pcm.len())?;

    Ok(SoundFont {
        info,
        presets,
        instruments,
        samples,
        zones,
        pcm,
        sample_precision: precision,
    })
}

fn set_once<T>(slot: &mut Option<T>, value: T, id: [u8; 4]) -> Result<(), LoadError> {
    if slot.is_some() {
        Err(LoadError::DuplicateChunk(id))
    } else {
        *slot = Some(value);
        Ok(())
    }
}

fn walk_chunks<'a>(
    bytes: &'a [u8],
    base: usize,
    limits: ParseLimits,
) -> Result<Vec<Chunk<'a>>, LoadError> {
    let mut chunks = Vec::new();
    let mut position = 0usize;
    while position < bytes.len() {
        if bytes.len() - position < 8 {
            return Err(LoadError::Truncated {
                offset: base + position,
                needed: 8 - (bytes.len() - position),
            });
        }
        let mut id = [0; 4];
        id.copy_from_slice(&bytes[position..position + 4]);
        let size = le_u32(bytes, position + 4)? as usize;
        if size > limits.max_chunk_bytes {
            return Err(LoadError::LimitExceeded {
                what: "SF2 chunk",
                limit: limits.max_chunk_bytes,
            });
        }
        let data_start = position + 8;
        let data_end = data_start
            .checked_add(size)
            .ok_or(LoadError::InvalidChunkSize { chunk: id, size })?;
        if data_end > bytes.len() {
            return Err(LoadError::Truncated {
                offset: base + data_start,
                needed: data_end - bytes.len(),
            });
        }
        chunks.push(Chunk {
            id,
            data: &bytes[data_start..data_end],
            data_offset: base + data_start,
        });
        let padded_end = data_end
            .checked_add(size & 1)
            .ok_or(LoadError::InvalidChunkSize { chunk: id, size })?;
        if padded_end > bytes.len() {
            return Err(LoadError::Truncated {
                offset: base + data_end,
                needed: padded_end - bytes.len(),
            });
        }
        position = padded_end;
    }
    Ok(chunks)
}

fn parse_info(
    list: Option<(&[u8], usize)>,
    limits: ParseLimits,
) -> Result<Vec<InfoEntry>, LoadError> {
    let Some((bytes, base)) = list else {
        return Ok(Vec::new());
    };
    walk_chunks(bytes, base, limits)?
        .into_iter()
        .map(|chunk| {
            let value_bytes = chunk.data.strip_suffix(&[0]).unwrap_or(chunk.data);
            Ok(InfoEntry {
                id: chunk.id,
                value: String::from_utf8_lossy(value_bytes).into_owned(),
            })
        })
        .collect()
}

fn parse_sample_data(
    bytes: &[u8],
    base: usize,
    limits: ParseLimits,
) -> Result<(Vec<f32>, u8), LoadError> {
    let chunks = walk_chunks(bytes, base, limits)?;
    let mut smpl = None;
    let mut sm24 = None;
    for chunk in chunks {
        match chunk.id {
            id if id == *b"smpl" => set_once(&mut smpl, chunk.data, id)?,
            id if id == *b"sm24" => set_once(&mut sm24, chunk.data, id)?,
            _ => {}
        }
    }
    let words = smpl.ok_or(LoadError::MissingChunk("smpl"))?;
    if words.len() % 2 != 0 {
        return Err(LoadError::InvalidChunkSize {
            chunk: *b"smpl",
            size: words.len(),
        });
    }
    let points = words.len() / 2;
    if points > limits.max_sample_points {
        return Err(LoadError::LimitExceeded {
            what: "SF2 sample points",
            limit: limits.max_sample_points,
        });
    }
    if let Some(low) = sm24 {
        if low.len() < points {
            return Err(LoadError::InvalidChunkSize {
                chunk: *b"sm24",
                size: low.len(),
            });
        }
        let mut pcm = Vec::with_capacity(points);
        for (index, pair) in words.chunks_exact(2).enumerate() {
            let high = i16::from_le_bytes([pair[0], pair[1]]) as i32;
            let value = (high << 8) | i32::from(low[index]);
            pcm.push(value as f32 / 8_388_608.0);
        }
        Ok((pcm, 24))
    } else {
        let mut pcm = Vec::with_capacity(points);
        for pair in words.chunks_exact(2) {
            pcm.push(i16::from_le_bytes([pair[0], pair[1]]) as f32 / 32_768.0);
        }
        Ok((pcm, 16))
    }
}

struct Hydra<'a> {
    phdr: &'a [u8],
    pbag: &'a [u8],
    pmod: &'a [u8],
    pgen: &'a [u8],
    inst: &'a [u8],
    ibag: &'a [u8],
    imod: &'a [u8],
    igen: &'a [u8],
    shdr: &'a [u8],
    limits: ParseLimits,
}

fn parse_hydra<'a>(
    bytes: &'a [u8],
    base: usize,
    limits: ParseLimits,
) -> Result<Hydra<'a>, LoadError> {
    let mut found: [Option<&[u8]>; 9] = [None; 9];
    let names = [
        *b"phdr", *b"pbag", *b"pmod", *b"pgen", *b"inst", *b"ibag", *b"imod", *b"igen",
        *b"shdr",
    ];
    for chunk in walk_chunks(bytes, base, limits)? {
        if let Some(index) = names.iter().position(|name| *name == chunk.id) {
            set_once(&mut found[index], chunk.data, chunk.id)?;
        }
    }
    for (index, value) in found.iter().enumerate() {
        if value.is_none() {
            let name = match index {
                0 => "phdr",
                1 => "pbag",
                2 => "pmod",
                3 => "pgen",
                4 => "inst",
                5 => "ibag",
                6 => "imod",
                7 => "igen",
                _ => "shdr",
            };
            return Err(LoadError::MissingChunk(name));
        }
    }
    Ok(Hydra {
        phdr: found[0].unwrap_or_default(),
        pbag: found[1].unwrap_or_default(),
        pmod: found[2].unwrap_or_default(),
        pgen: found[3].unwrap_or_default(),
        inst: found[4].unwrap_or_default(),
        ibag: found[5].unwrap_or_default(),
        imod: found[6].unwrap_or_default(),
        igen: found[7].unwrap_or_default(),
        shdr: found[8].unwrap_or_default(),
        limits,
    })
}

fn parse_records<'a>(
    bytes: &'a [u8],
    size: usize,
    id: [u8; 4],
    limit: usize,
) -> Result<impl Iterator<Item = &'a [u8]>, LoadError> {
    if bytes.len() % size != 0 {
        return Err(LoadError::InvalidChunkSize { chunk: id, size: bytes.len() });
    }
    if bytes.len() / size > limit {
        return Err(LoadError::LimitExceeded { what: "SF2 records", limit });
    }
    Ok(bytes.chunks_exact(size))
}

fn parse_preset_records(hydra: &Hydra<'_>) -> Result<Vec<PresetRecord>, LoadError> {
    parse_records(hydra.phdr, 38, *b"phdr", hydra.limits.max_records)?
        .map(|record| {
            Ok(PresetRecord {
                name: fixed_name(&record[..20]),
                program: u16::from_le_bytes([record[20], record[21]]),
                bank: u16::from_le_bytes([record[22], record[23]]),
                bag: u16::from_le_bytes([record[24], record[25]]) as usize,
                library: u32::from_le_bytes([record[26], record[27], record[28], record[29]]),
                genre: u32::from_le_bytes([record[30], record[31], record[32], record[33]]),
                morphology: u32::from_le_bytes([record[34], record[35], record[36], record[37]]),
            })
        })
        .collect()
}

fn parse_instrument_records(hydra: &Hydra<'_>) -> Result<Vec<InstrumentRecord>, LoadError> {
    parse_records(hydra.inst, 22, *b"inst", hydra.limits.max_records)?
        .map(|record| {
            Ok(InstrumentRecord {
                name: fixed_name(&record[..20]),
                bag: u16::from_le_bytes([record[20], record[21]]) as usize,
            })
        })
        .collect()
}

fn parse_bags(bytes: &[u8], id: [u8; 4], limit: usize) -> Result<Vec<Bag>, LoadError> {
    parse_records(bytes, 4, id, limit)?
        .map(|record| {
            Ok(Bag {
                generator: u16::from_le_bytes([record[0], record[1]]) as usize,
                modulator: u16::from_le_bytes([record[2], record[3]]) as usize,
            })
        })
        .collect()
}

fn parse_generators(bytes: &[u8], id: [u8; 4], limit: usize) -> Result<Vec<Generator>, LoadError> {
    parse_records(bytes, 4, id, limit)?
        .map(|record| {
            let operator = u16::from_le_bytes([record[0], record[1]]);
            if operator >= GEN_COUNT as u16 {
                return Err(LoadError::InvalidGenerator { operator });
            }
            Ok(Generator {
                operator,
                amount: GeneratorAmount(u16::from_le_bytes([record[2], record[3]])),
            })
        })
        .collect()
}

fn parse_modulators(bytes: &[u8], id: [u8; 4], limit: usize) -> Result<Vec<Modulator>, LoadError> {
    parse_records(bytes, 10, id, limit)?
        .map(|record| {
            Ok(Modulator {
                source: u16::from_le_bytes([record[0], record[1]]),
                destination: u16::from_le_bytes([record[2], record[3]]),
                amount: i16::from_le_bytes([record[4], record[5]]),
                amount_source: u16::from_le_bytes([record[6], record[7]]),
                transform: u16::from_le_bytes([record[8], record[9]]),
            })
        })
        .collect()
}

fn build_presets(hydra: &Hydra<'_>) -> Result<Vec<Preset>, LoadError> {
    let records = parse_preset_records(hydra)?;
    if records.len() < 2 {
        return Err(LoadError::InvalidHierarchy("phdr has no terminal record"));
    }
    let bags = parse_bags(hydra.pbag, *b"pbag", hydra.limits.max_records)?;
    let generators = parse_generators(hydra.pgen, *b"pgen", hydra.limits.max_records)?;
    let modulators = parse_modulators(hydra.pmod, *b"pmod", hydra.limits.max_records)?;
    build_zone_owners(&records, &bags, &generators, &modulators, true)
}

fn build_instruments(hydra: &Hydra<'_>) -> Result<Vec<Instrument>, LoadError> {
    let records = parse_instrument_records(hydra)?;
    if records.len() < 2 {
        return Err(LoadError::InvalidHierarchy("inst has no terminal record"));
    }
    let bags = parse_bags(hydra.ibag, *b"ibag", hydra.limits.max_records)?;
    let generators = parse_generators(hydra.igen, *b"igen", hydra.limits.max_records)?;
    let modulators = parse_modulators(hydra.imod, *b"imod", hydra.limits.max_records)?;
    build_instrument_owners(&records, &bags, &generators, &modulators)
}

fn zone_slices(
    start: usize,
    end: usize,
    bags: &[Bag],
    generators: &[Generator],
    modulators: &[Modulator],
) -> Result<Vec<(Vec<Generator>, Vec<Modulator>)>, LoadError> {
    if start > end || end >= bags.len() {
        return Err(LoadError::InvalidHierarchy("bag index is out of range"));
    }
    let mut result = Vec::with_capacity(end - start);
    for index in start..end {
        let current = bags.get(index).ok_or(LoadError::InvalidHierarchy("missing bag"))?;
        let next = bags.get(index + 1).ok_or(LoadError::InvalidHierarchy("missing terminal bag"))?;
        if current.generator > next.generator
            || current.modulator > next.modulator
            || next.generator > generators.len()
            || next.modulator > modulators.len()
        {
            return Err(LoadError::InvalidHierarchy("non-monotonic bag indices"));
        }
        result.push((
            generators[current.generator..next.generator].to_vec(),
            modulators[current.modulator..next.modulator].to_vec(),
        ));
    }
    Ok(result)
}

fn build_zone_owners(
    records: &[PresetRecord],
    bags: &[Bag],
    generators: &[Generator],
    modulators: &[Modulator],
    _preset: bool,
) -> Result<Vec<Preset>, LoadError> {
    let mut result = Vec::with_capacity(records.len() - 1);
    for pair in records.windows(2) {
        let owner = &pair[0];
        let zones = zone_slices(owner.bag, pair[1].bag, bags, generators, modulators)?;
        let mut built = Vec::with_capacity(zones.len());
        for (zone_index, (generators, modulators)) in zones.into_iter().enumerate() {
            validate_zone_selector(&generators, INSTRUMENT, zone_index, "preset")?;
            if generators.iter().any(|generator| generator.operator == SAMPLE_ID) {
                return Err(LoadError::InvalidHierarchy("sampleID appears in a preset zone"));
            }
            let instrument = selector(&generators, INSTRUMENT);
            if instrument.is_none() && zone_index != 0 {
                return Err(LoadError::InvalidHierarchy("preset global zone is not first"));
            }
            built.push(PresetZone { generators, modulators, instrument });
        }
        result.push(Preset {
            name: owner.name.clone(),
            program: owner.program,
            bank: owner.bank,
            library: owner.library,
            genre: owner.genre,
            morphology: owner.morphology,
            zones: built,
        });
    }
    Ok(result)
}

fn build_instrument_owners(
    records: &[InstrumentRecord],
    bags: &[Bag],
    generators: &[Generator],
    modulators: &[Modulator],
) -> Result<Vec<Instrument>, LoadError> {
    let mut result = Vec::with_capacity(records.len() - 1);
    for pair in records.windows(2) {
        let owner = &pair[0];
        let zones = zone_slices(owner.bag, pair[1].bag, bags, generators, modulators)?;
        let mut built = Vec::with_capacity(zones.len());
        for (zone_index, (generators, modulators)) in zones.into_iter().enumerate() {
            validate_zone_selector(&generators, SAMPLE_ID, zone_index, "instrument")?;
            if generators.iter().any(|generator| generator.operator == INSTRUMENT) {
                return Err(LoadError::InvalidHierarchy("instrument selector appears in an instrument zone"));
            }
            let sample = selector(&generators, SAMPLE_ID);
            if sample.is_none() && zone_index != 0 {
                return Err(LoadError::InvalidHierarchy("instrument global zone is not first"));
            }
            built.push(InstrumentZone { generators, modulators, sample });
        }
        result.push(Instrument { name: owner.name.clone(), zones: built });
    }
    Ok(result)
}

fn selector(generators: &[Generator], operator: u16) -> Option<usize> {
    generators
        .iter()
        .rev()
        .find(|generator| generator.operator == operator)
        .map(|generator| generator.amount.0 as usize)
}

fn validate_zone_selector(
    generators: &[Generator],
    operator: u16,
    zone_index: usize,
    owner: &'static str,
) -> Result<(), LoadError> {
    let mut positions = generators
        .iter()
        .enumerate()
        .filter(|(_, generator)| generator.operator == operator)
        .map(|(index, _)| index);
    let first = positions.next();
    if positions.next().is_some() {
        return Err(LoadError::InvalidHierarchy("duplicate terminal selector generator"));
    }
    match first {
        None if zone_index == 0 => Ok(()),
        None if owner == "preset" => {
            Err(LoadError::InvalidHierarchy("preset global zone is not first"))
        }
        None => Err(LoadError::InvalidHierarchy("instrument global zone is not first")),
        Some(position) if position + 1 != generators.len() => {
            Err(LoadError::InvalidHierarchy("zone selector generator is not last"))
        }
        Some(_) => Ok(()),
    }
}

fn parse_sample_headers(bytes: &[u8], limits: ParseLimits) -> Result<Vec<SampleHeader>, LoadError> {
    let records: Vec<&[u8]> = parse_records(bytes, 46, *b"shdr", limits.max_records)?.collect();
    if records.len() < 2 {
        return Err(LoadError::InvalidHierarchy("shdr has no terminal record"));
    }
    records[..records.len() - 1]
        .iter()
        .map(|record| {
            let raw_type = u16::from_le_bytes([record[44], record[45]]);
            let kind = match raw_type & 0x7fff {
                1 => SampleKind::Mono,
                2 => SampleKind::Right,
                4 => SampleKind::Left,
                8 => SampleKind::Linked,
                other => SampleKind::Unknown(other),
            };
            Ok(SampleHeader {
                name: fixed_name(&record[..20]),
                start: read_record_u32(record, 20),
                end: read_record_u32(record, 24),
                loop_start: read_record_u32(record, 28),
                loop_end: read_record_u32(record, 32),
                sample_rate: read_record_u32(record, 36),
                original_pitch: record[40],
                pitch_correction: record[41] as i8,
                link: u16::from_le_bytes([record[42], record[43]]),
                kind,
                is_rom: raw_type & 0x8000 != 0,
            })
        })
        .collect()
}

fn validate_samples(samples: &[SampleHeader], pcm_len: usize) -> Result<(), LoadError> {
    for (index, sample) in samples.iter().enumerate() {
        if sample.sample_rate == 0 {
            return Err(LoadError::InvalidSample { sample: index, reason: "zero sample rate" });
        }
        if sample.start > sample.end {
            return Err(LoadError::InvalidSample { sample: index, reason: "start follows end" });
        }
        if !sample.is_rom && sample.end as usize > pcm_len {
            return Err(LoadError::InvalidSample { sample: index, reason: "sample exceeds smpl data" });
        }
        if matches!(sample.kind, SampleKind::Left | SampleKind::Right | SampleKind::Linked) {
            let Some(linked) = samples.get(sample.link as usize) else {
                return Err(LoadError::InvalidSample { sample: index, reason: "stereo link is out of range" });
            };
            if linked.link as usize != index {
                return Err(LoadError::InvalidSample { sample: index, reason: "stereo link is not reciprocal" });
            }
            let compatible = matches!(
                (sample.kind, linked.kind),
                (SampleKind::Left, SampleKind::Right)
                    | (SampleKind::Right, SampleKind::Left)
                    | (SampleKind::Linked, SampleKind::Linked)
            );
            if !compatible {
                return Err(LoadError::InvalidSample { sample: index, reason: "stereo link type mismatch" });
            }
        }
    }
    Ok(())
}

fn level_set(global: Option<&[Generator]>, local: &[Generator]) -> GeneratorSet {
    let mut result = GeneratorSet::empty();
    if let Some(global) = global {
        result.insert_zone(global);
    }
    result.insert_zone(local);
    result
}

fn generator_range(generators: &[Generator], operator: u16) -> Result<Range, LoadError> {
    let range = generators
        .iter()
        .rev()
        .find(|generator| generator.operator == operator)
        .map(|generator| generator.amount.range())
        .unwrap_or(Range::ALL);
    if range.low > range.high || range.high > 127 {
        Err(LoadError::InvalidHierarchy("invalid key/velocity range"))
    } else {
        Ok(range)
    }
}

fn combined_range(
    operator: u16,
    lists: &[Option<&[Generator]>],
) -> Result<Option<Range>, LoadError> {
    let mut result = Range::ALL;
    for generators in lists.iter().flatten() {
        let next = generator_range(generators, operator)?;
        let Some(intersection) = result.intersection(next) else {
            return Ok(None);
        };
        result = intersection;
    }
    Ok(Some(result))
}

fn merge_modulators(global: &[Modulator], local: &[Modulator]) -> Vec<Modulator> {
    let mut result = global.to_vec();
    for item in local {
        if let Some(existing) = result.iter_mut().find(|value| value.identity() == item.identity()) {
            *existing = *item;
        } else {
            result.push(*item);
        }
    }
    result
}

fn combine_modulators(instrument: &[Modulator], preset: &[Modulator]) -> Vec<Modulator> {
    let mut result = instrument.to_vec();
    for item in preset {
        if let Some(existing) = result.iter_mut().find(|value| value.identity() == item.identity()) {
            existing.amount = existing.amount.saturating_add(item.amount);
        } else {
            result.push(*item);
        }
    }
    result
}

fn resolve_zones(
    presets: &[Preset],
    instruments: &[Instrument],
    samples: &[SampleHeader],
    pcm_len: usize,
) -> Result<Vec<Zone>, LoadError> {
    let mut result = Vec::new();
    for preset in presets {
        let (preset_global, preset_locals) = split_preset_zones(&preset.zones)?;
        for preset_zone in preset_locals {
            let instrument_index = preset_zone.instrument.ok_or(LoadError::InvalidHierarchy(
                "preset local zone has no instrument",
            ))?;
            let instrument = instruments.get(instrument_index).ok_or(
                LoadError::InvalidHierarchy("preset references missing instrument"),
            )?;
            let (instrument_global, instrument_locals) = split_instrument_zones(&instrument.zones)?;
            let preset_set = level_set(
                preset_global.map(|zone| zone.generators.as_slice()),
                &preset_zone.generators,
            );
            let preset_modulators = merge_modulators(
                preset_global.map(|zone| zone.modulators.as_slice()).unwrap_or_default(),
                &preset_zone.modulators,
            );
            for instrument_zone in instrument_locals {
                let sample_index = instrument_zone.sample.ok_or(LoadError::InvalidHierarchy(
                    "instrument local zone has no sample",
                ))?;
                let sample = samples
                    .get(sample_index)
                    .ok_or(LoadError::InvalidHierarchy("instrument references missing sample"))?;
                let lists = [
                    preset_global.map(|zone| zone.generators.as_slice()),
                    Some(preset_zone.generators.as_slice()),
                    instrument_global.map(|zone| zone.generators.as_slice()),
                    Some(instrument_zone.generators.as_slice()),
                ];
                let Some(key_range) = combined_range(KEY_RANGE, &lists)? else { continue };
                let Some(velocity_range) = combined_range(VEL_RANGE, &lists)? else { continue };
                let instrument_set = level_set(
                    instrument_global.map(|zone| zone.generators.as_slice()),
                    &instrument_zone.generators,
                );
                let instrument_modulators = merge_modulators(
                    instrument_global.map(|zone| zone.modulators.as_slice()).unwrap_or_default(),
                    &instrument_zone.modulators,
                );
                let _resolved_modulators =
                    combine_modulators(&instrument_modulators, &preset_modulators);
                let parameters = resolve_parameters(
                    sample_index,
                    sample,
                    instrument_set,
                    preset_set,
                    pcm_len,
                )?;
                result.push(Zone {
                    program: preset.program,
                    bank: preset.bank,
                    key_range,
                    velocity_range,
                    parameters,
                    fixed_key: fixed_midi(instrument_set, 46),
                    fixed_velocity: fixed_midi(instrument_set, 47),
                    key_to_hold: combined_signed(instrument_set, preset_set, 39, 0) as i16,
                    key_to_decay: combined_signed(instrument_set, preset_set, 40, 0) as i16,
                });
            }
        }
    }
    Ok(result)
}

fn split_preset_zones(zones: &[PresetZone]) -> Result<(Option<&PresetZone>, &[PresetZone]), LoadError> {
    if zones.is_empty() {
        return Err(LoadError::InvalidHierarchy("preset has no zones"));
    }
    if zones[0].instrument.is_none() {
        if zones.len() == 1 {
            return Err(LoadError::InvalidHierarchy("preset has only a global zone"));
        }
        Ok((zones.first(), &zones[1..]))
    } else {
        Ok((None, zones))
    }
}

fn split_instrument_zones(
    zones: &[InstrumentZone],
) -> Result<(Option<&InstrumentZone>, &[InstrumentZone]), LoadError> {
    if zones.is_empty() {
        return Err(LoadError::InvalidHierarchy("instrument has no zones"));
    }
    if zones[0].sample.is_none() {
        if zones.len() == 1 {
            return Err(LoadError::InvalidHierarchy("instrument has only a global zone"));
        }
        Ok((zones.first(), &zones[1..]))
    } else {
        Ok((None, zones))
    }
}

fn generator_default(operator: u16) -> i32 {
    match operator {
        8 => 13_500,
        21 | 23 | 25 | 26 | 27 | 28 | 30 | 33 | 34 | 35 | 36 | 38 => -12_000,
        46 | 47 | 58 => -1,
        56 => 100,
        _ => 0,
    }
}

fn preset_may_modify(operator: u16) -> bool {
    matches!(
        operator,
        5..=11 | 13 | 15..=17 | 21..=40 | 48 | 51 | 52 | 56
    )
}

fn combined_signed(
    instrument: GeneratorSet,
    preset: GeneratorSet,
    operator: u16,
    default: i32,
) -> i32 {
    let base = instrument.signed(operator, default);
    if preset_may_modify(operator) {
        base.saturating_add(preset.signed(operator, 0))
    } else {
        base
    }
}

fn fixed_midi(set: GeneratorSet, operator: u16) -> Option<u8> {
    set.amount(operator)
        .map(|amount| amount.signed())
        .filter(|value| (0..=127).contains(value))
        .map(|value| value as u8)
}

fn resolve_parameters(
    sample_index: usize,
    sample: &SampleHeader,
    instrument: GeneratorSet,
    preset: GeneratorSet,
    pcm_len: usize,
) -> Result<VoiceParameters, LoadError> {
    let start_offset = i64::from(instrument.signed(0, 0))
        + i64::from(instrument.signed(4, 0)) * 32_768;
    let end_offset = i64::from(instrument.signed(1, 0))
        + i64::from(instrument.signed(12, 0)) * 32_768;
    let loop_start_offset = i64::from(instrument.signed(2, 0))
        + i64::from(instrument.signed(45, 0)) * 32_768;
    let loop_end_offset = i64::from(instrument.signed(3, 0))
        + i64::from(instrument.signed(50, 0)) * 32_768;
    let absolute_start = i64::from(sample.start) + start_offset;
    let absolute_end = i64::from(sample.end) + end_offset;
    let absolute_loop_start = i64::from(sample.loop_start) + loop_start_offset;
    let absolute_loop_end = i64::from(sample.loop_end) + loop_end_offset;
    if absolute_start < 0 || absolute_end <= absolute_start {
        return Err(LoadError::InvalidSample { sample: sample_index, reason: "generator-adjusted range is empty" });
    }
    if !sample.is_rom && absolute_end as usize > pcm_len {
        return Err(LoadError::InvalidSample { sample: sample_index, reason: "generator-adjusted range exceeds smpl" });
    }
    let mode = match instrument.signed(54, 0) & 3 {
        1 => LoopMode::Continuous,
        3 => LoopMode::UntilRelease,
        _ => LoopMode::NoLoop,
    };
    if mode != LoopMode::NoLoop
        && (absolute_loop_start < absolute_start
            || absolute_loop_end <= absolute_loop_start
            || absolute_loop_end > absolute_end)
    {
        return Err(LoadError::InvalidSample { sample: sample_index, reason: "invalid generator-adjusted loop" });
    }
    let base = i64::from(sample.start);
    let root = instrument.signed(58, generator_default(58));
    let root_key = if (0..=127).contains(&root) {
        root as f32
    } else if sample.original_pitch <= 127 {
        sample.original_pitch as f32
    } else {
        60.0
    };
    let time = |operator| {
        timecents_to_seconds(combined_signed(
            instrument,
            preset,
            operator,
            generator_default(operator),
        ))
    };
    let sustain_cb = combined_signed(instrument, preset, 37, 0).clamp(0, 1440);
    let attenuation_cb = combined_signed(instrument, preset, 48, 0).clamp(0, 1440);
    let cutoff_cents = combined_signed(instrument, preset, 8, generator_default(8));
    Ok(VoiceParameters {
        source: VoiceSource::Sample { sample_id: sample_index as u32 },
        key: 60,
        velocity: 127,
        root_key,
        tune_cents: combined_signed(instrument, preset, 51, 0) as f32 * 100.0
            + combined_signed(instrument, preset, 52, 0) as f32
            + sample.pitch_correction as f32,
        scale_tuning: combined_signed(instrument, preset, 56, generator_default(56)) as f32,
        sample_rate: sample.sample_rate,
        start_frame: absolute_start - base,
        end_frame: absolute_end - base,
        loop_start: absolute_loop_start - base,
        loop_end: absolute_loop_end - base,
        loop_mode: mode,
        release_on_note_off: true,
        envelope: Envelope {
            delay: time(33),
            attack: time(34),
            hold: time(35),
            decay: time(36),
            sustain: 10.0_f32.powf(-(sustain_cb as f32) / 200.0),
            release: time(38),
        },
        gain: 10.0_f32.powf(-(attenuation_cb as f32) / 200.0),
        pan: (combined_signed(instrument, preset, 17, 0) as f32 / 500.0).clamp(-1.0, 1.0),
        filter_cutoff_hz: cents_to_hz(cutoff_cents),
        filter_resonance_db: combined_signed(instrument, preset, 9, 0) as f32 / 10.0,
        exclusive_class: instrument.signed(57, 0).max(0) as u16,
    })
}

fn fixed_name(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|byte| *byte == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim_end().to_string()
}

fn read_record_u32(record: &[u8], offset: usize) -> u32 {
    let bytes = record.get(offset..offset + 4).unwrap_or(&[0, 0, 0, 0]);
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn le_u32(bytes: &[u8], offset: usize) -> Result<u32, LoadError> {
    let Some(slice) = bytes.get(offset..offset + 4) else {
        return Err(LoadError::Truncated { offset, needed: 4 });
    };
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}
