use crate::error::SfzError;
use crate::model::{
    select_zones, Envelope, LoopMode, Range, VoiceParameters, VoiceSource, Zone,
};

/// Bounds allocation while parsing an SFZ string.
#[derive(Clone, Copy, Debug)]
pub struct SfzLimits {
    pub max_text_bytes: usize,
    pub max_opcodes: usize,
    pub max_regions: usize,
}

impl Default for SfzLimits {
    fn default() -> Self {
        Self { max_text_bytes: 4 * 1024 * 1024, max_opcodes: 200_000, max_regions: 65_536 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SampleMetadata {
    pub sample_rate: u32,
    pub frames: u32,
    pub loop_start: Option<u32>,
    /// Exclusive loop end.
    pub loop_end: Option<u32>,
}

impl Default for SampleMetadata {
    fn default() -> Self {
        Self { sample_rate: 44_100, frames: u32::MAX, loop_start: None, loop_end: None }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SfzSample {
    pub path: String,
    pub metadata: SampleMetadata,
    loop_start_override: Option<u32>,
    loop_end_override: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct SfzInstrument {
    pub samples: Vec<SfzSample>,
    pub zones: Vec<Zone>,
    /// Unknown opcodes encountered during loading, de-duplicated in first-seen order.
    pub ignored_opcodes: Vec<String>,
}

impl SfzInstrument {
    pub fn select(&self, key: u8, velocity: u8) -> Vec<VoiceParameters> {
        select_zones(&self.zones, 0, 0, key, velocity)
    }

    /// Supply metadata after the referenced sample has been decoded. This is a
    /// control-thread operation; the SFZ parser itself performs no I/O.
    pub fn set_sample_metadata(&mut self, sample_id: usize, metadata: SampleMetadata) -> bool {
        let Some(sample) = self.samples.get_mut(sample_id) else {
            return false;
        };
        sample.metadata = metadata;
        let loop_start = sample.loop_start_override.or(metadata.loop_start).unwrap_or(0);
        let loop_end = sample.loop_end_override.or(metadata.loop_end).unwrap_or(metadata.frames);
        for zone in &mut self.zones {
            if zone.parameters.source == (VoiceSource::Sample { sample_id: sample_id as u32 }) {
                zone.parameters.sample_rate = metadata.sample_rate;
                zone.parameters.end_frame = i64::from(metadata.frames);
                zone.parameters.loop_start = i64::from(loop_start);
                zone.parameters.loop_end = i64::from(loop_end);
            }
        }
        true
    }
}

#[derive(Clone, Debug)]
struct RegionParameters {
    sample: Option<String>,
    key_range: Range,
    velocity_range: Range,
    root_key: u8,
    tune: i16,
    transpose: i16,
    loop_mode: LoopMode,
    release_on_note_off: bool,
    loop_start: Option<u32>,
    /// Stored exclusive even though the SFZ opcode is inclusive.
    loop_end: Option<u32>,
    volume_db: f32,
    pan: f32,
    envelope: Envelope,
}

impl Default for RegionParameters {
    fn default() -> Self {
        Self {
            sample: None,
            key_range: Range::ALL,
            velocity_range: Range::ALL,
            root_key: 60,
            tune: 0,
            transpose: 0,
            loop_mode: LoopMode::NoLoop,
            release_on_note_off: true,
            loop_start: None,
            loop_end: None,
            volume_db: 0.0,
            pan: 0.0,
            envelope: Envelope::default(),
        }
    }
}

#[derive(Clone, Debug)]
enum Token {
    Header { name: String, line: usize },
    Opcode { name: String, value: String, line: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScopeKind {
    Global,
    Group,
    Region,
}

/// Parse the deliberately small, portable SFZ subset used by the score app.
///
/// Supported headers are `<global>`, `<group>`, and `<region>`. Supported
/// opcodes are `sample`, `key`, `lokey`, `hikey`, `lovel`, `hivel`,
/// `pitch_keycenter`, `tune`, `transpose`, `loop_mode` (`no_loop`,
/// `one_shot`, `loop_continuous`, `loop_sustain`), `loop_start`, `loop_end`
/// (converted from SFZ's inclusive end to an exclusive end), `volume`, `pan`,
/// and `ampeg_delay`, `ampeg_attack`, `ampeg_hold`, `ampeg_decay`,
/// `ampeg_sustain`, `ampeg_release`. Quoted sample paths may contain spaces.
/// `//` and `/* ... */` comments are accepted. Unknown opcodes are retained in
/// [`SfzInstrument::ignored_opcodes`] and otherwise ignored.
pub fn parse_sfz(text: &str) -> Result<SfzInstrument, SfzError> {
    parse_sfz_with_limits(text, SfzLimits::default())
}

pub fn parse_sfz_with_limits(text: &str, limits: SfzLimits) -> Result<SfzInstrument, SfzError> {
    if text.len() > limits.max_text_bytes {
        return Err(SfzError::LimitExceeded { what: "SFZ text", limit: limits.max_text_bytes });
    }
    let clean = remove_comments(text)?;
    let tokens = tokenize(&clean, limits.max_opcodes)?;
    let mut global = RegionParameters::default();
    let mut group = global.clone();
    let mut region: Option<RegionParameters> = None;
    let mut scope = ScopeKind::Global;
    let mut instrument = SfzInstrument { samples: Vec::new(), zones: Vec::new(), ignored_opcodes: Vec::new() };

    for token in tokens {
        match token {
            Token::Header { name, line } => {
                if let Some(done) = region.take() {
                    finish_region(done, &mut instrument, limits)?;
                }
                match name.as_str() {
                    "global" => {
                        global = RegionParameters::default();
                        group = global.clone();
                        scope = ScopeKind::Global;
                    }
                    "group" => {
                        group = global.clone();
                        scope = ScopeKind::Group;
                    }
                    "region" => {
                        region = Some(group.clone());
                        scope = ScopeKind::Region;
                    }
                    _ => return Err(SfzError::InvalidHeader { line, header: name }),
                }
            }
            Token::Opcode { name, value, line } => {
                let target = match scope {
                    ScopeKind::Global => &mut global,
                    ScopeKind::Group => &mut group,
                    ScopeKind::Region => {
                        if region.is_none() {
                            region = Some(group.clone());
                        }
                        region.as_mut().ok_or_else(|| SfzError::InvalidHeader {
                            line,
                            header: "region".to_string(),
                        })?
                    }
                };
                if !apply_opcode(target, &name, &value, line)?
                    && !instrument.ignored_opcodes.iter().any(|opcode| opcode == &name)
                {
                    instrument.ignored_opcodes.push(name);
                }
                if scope == ScopeKind::Global {
                    group = global.clone();
                }
            }
        }
    }
    if let Some(done) = region {
        finish_region(done, &mut instrument, limits)?;
    }
    Ok(instrument)
}

fn finish_region(
    region: RegionParameters,
    instrument: &mut SfzInstrument,
    limits: SfzLimits,
) -> Result<(), SfzError> {
    if instrument.zones.len() >= limits.max_regions {
        return Err(SfzError::LimitExceeded { what: "SFZ regions", limit: limits.max_regions });
    }
    let region_number = instrument.zones.len();
    let path = region.sample.clone().ok_or(SfzError::MissingSample { region: region_number })?;
    let sample_id = instrument.samples.len();
    let metadata = SampleMetadata::default();
    let loop_start = region.loop_start.or(metadata.loop_start).unwrap_or(0);
    let loop_end = region.loop_end.or(metadata.loop_end).unwrap_or(metadata.frames);
    instrument.samples.push(SfzSample {
        path,
        metadata,
        loop_start_override: region.loop_start,
        loop_end_override: region.loop_end,
    });
    instrument.zones.push(Zone {
        program: 0,
        bank: 0,
        key_range: region.key_range,
        velocity_range: region.velocity_range,
        parameters: VoiceParameters {
            source: VoiceSource::Sample { sample_id: sample_id as u32 },
            key: 60,
            velocity: 127,
            root_key: region.root_key as f32,
            tune_cents: region.tune as f32 + region.transpose as f32 * 100.0,
            scale_tuning: 100.0,
            sample_rate: metadata.sample_rate,
            start_frame: 0,
            end_frame: i64::from(metadata.frames),
            loop_start: i64::from(loop_start),
            loop_end: i64::from(loop_end),
            loop_mode: region.loop_mode,
            release_on_note_off: region.release_on_note_off,
            envelope: region.envelope,
            gain: 10.0_f32.powf(region.volume_db / 20.0),
            pan: (region.pan / 100.0).clamp(-1.0, 1.0),
            filter_cutoff_hz: 20_000.0,
            filter_resonance_db: 0.0,
            exclusive_class: 0,
        },
        fixed_key: None,
        fixed_velocity: None,
        key_to_hold: 0,
        key_to_decay: 0,
    });
    Ok(())
}

fn apply_opcode(
    target: &mut RegionParameters,
    name: &str,
    value: &str,
    line: usize,
) -> Result<bool, SfzError> {
    match name {
        "sample" => target.sample = Some(value.replace('\\', "/")),
        "key" => {
            let key = parse_key(value).ok_or_else(|| invalid(line, name, value))?;
            target.key_range = Range { low: key, high: key };
            target.root_key = key;
        }
        "lokey" => target.key_range.low = parse_key(value).ok_or_else(|| invalid(line, name, value))?,
        "hikey" => target.key_range.high = parse_key(value).ok_or_else(|| invalid(line, name, value))?,
        "lovel" => target.velocity_range.low = parse_u8_127(value).ok_or_else(|| invalid(line, name, value))?,
        "hivel" => target.velocity_range.high = parse_u8_127(value).ok_or_else(|| invalid(line, name, value))?,
        "pitch_keycenter" => target.root_key = parse_key(value).ok_or_else(|| invalid(line, name, value))?,
        "tune" => target.tune = value.parse().map_err(|_| invalid(line, name, value))?,
        "transpose" => target.transpose = value.parse().map_err(|_| invalid(line, name, value))?,
        "loop_mode" => {
            target.loop_mode = match value {
                "no_loop" => {
                    target.release_on_note_off = true;
                    LoopMode::NoLoop
                }
                "one_shot" => {
                    target.release_on_note_off = false;
                    LoopMode::NoLoop
                }
                "loop_continuous" => {
                    target.release_on_note_off = true;
                    LoopMode::Continuous
                }
                "loop_sustain" => {
                    target.release_on_note_off = true;
                    LoopMode::UntilRelease
                }
                _ => return Err(invalid(line, name, value)),
            }
        }
        "loop_start" => target.loop_start = Some(value.parse().map_err(|_| invalid(line, name, value))?),
        "loop_end" => {
            let inclusive: u32 = value.parse().map_err(|_| invalid(line, name, value))?;
            target.loop_end = Some(inclusive.saturating_add(1));
        }
        "volume" => target.volume_db = value.parse().map_err(|_| invalid(line, name, value))?,
        "pan" => target.pan = value.parse().map_err(|_| invalid(line, name, value))?,
        "ampeg_delay" => target.envelope.delay = parse_nonnegative(value, line, name)?,
        "ampeg_attack" => target.envelope.attack = parse_nonnegative(value, line, name)?,
        "ampeg_hold" => target.envelope.hold = parse_nonnegative(value, line, name)?,
        "ampeg_decay" => target.envelope.decay = parse_nonnegative(value, line, name)?,
        "ampeg_sustain" => {
            let percent: f32 = value.parse().map_err(|_| invalid(line, name, value))?;
            if !(0.0..=100.0).contains(&percent) {
                return Err(invalid(line, name, value));
            }
            target.envelope.sustain = percent / 100.0;
        }
        "ampeg_release" => target.envelope.release = parse_nonnegative(value, line, name)?,
        _ => return Ok(false),
    }
    if target.key_range.low > target.key_range.high
        || target.velocity_range.low > target.velocity_range.high
        || matches!((target.loop_start, target.loop_end), (Some(start), Some(end)) if end <= start)
    {
        return Err(invalid(line, name, value));
    }
    Ok(true)
}

fn parse_nonnegative(value: &str, line: usize, opcode: &str) -> Result<f32, SfzError> {
    let parsed: f32 = value.parse().map_err(|_| invalid(line, opcode, value))?;
    if parsed.is_finite() && parsed >= 0.0 {
        Ok(parsed)
    } else {
        Err(invalid(line, opcode, value))
    }
}

fn parse_u8_127(value: &str) -> Option<u8> {
    value.parse::<u8>().ok().filter(|number| *number <= 127)
}

fn parse_key(value: &str) -> Option<u8> {
    if let Some(number) = parse_u8_127(value) {
        return Some(number);
    }
    let bytes = value.as_bytes();
    if bytes.len() < 2 {
        return None;
    }
    let base = match bytes[0].to_ascii_lowercase() {
        b'c' => 0,
        b'd' => 2,
        b'e' => 4,
        b'f' => 5,
        b'g' => 7,
        b'a' => 9,
        b'b' => 11,
        _ => return None,
    };
    let (accidental, octave_start) = match bytes.get(1).copied() {
        Some(b'#') => (1, 2),
        Some(b'b') | Some(b'B') => (-1, 2),
        _ => (0, 1),
    };
    let octave: i16 = value.get(octave_start..)?.parse().ok()?;
    let midi = (octave + 1) * 12 + base + accidental;
    u8::try_from(midi).ok().filter(|number| *number <= 127)
}

fn invalid(line: usize, opcode: &str, value: &str) -> SfzError {
    SfzError::InvalidValue { line, opcode: opcode.to_string(), value: value.to_string() }
}

fn remove_comments(text: &str) -> Result<String, SfzError> {
    let bytes = text.as_bytes();
    let mut result = Vec::with_capacity(text.len());
    let mut index = 0;
    let mut quote = false;
    let mut block = false;
    while index < bytes.len() {
        if block {
            if bytes.get(index..index + 2) == Some(b"*/") {
                block = false;
                index += 2;
            } else {
                if bytes[index] == b'\n' {
                    result.push(b'\n');
                }
                index += 1;
            }
            continue;
        }
        if !quote && bytes.get(index..index + 2) == Some(b"/*") {
            block = true;
            index += 2;
            continue;
        }
        if !quote && bytes.get(index..index + 2) == Some(b"//") {
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        let byte = bytes[index];
        if byte == b'"' {
            quote = !quote;
        }
        result.push(byte);
        index += 1;
    }
    if block {
        Err(SfzError::UnterminatedComment)
    } else {
        match String::from_utf8(result) {
            Ok(result) => Ok(result),
            Err(_) => Ok(text.to_string()),
        }
    }
}

fn tokenize(text: &str, max_opcodes: usize) -> Result<Vec<Token>, SfzError> {
    let bytes = text.as_bytes();
    let mut result = Vec::new();
    let mut index = 0usize;
    let mut line = 1usize;
    while index < bytes.len() {
        while let Some(byte) = bytes.get(index) {
            if !byte.is_ascii_whitespace() {
                break;
            }
            if *byte == b'\n' {
                line += 1;
            }
            index += 1;
        }
        let Some(byte) = bytes.get(index).copied() else { break };
        if byte == b'<' {
            let start_line = line;
            let start = index + 1;
            let Some(relative_end) = bytes[start..].iter().position(|value| *value == b'>') else {
                return Err(SfzError::InvalidHeader { line, header: String::new() });
            };
            let end = start + relative_end;
            let name = text[start..end].trim().to_ascii_lowercase();
            result.push(Token::Header { name, line: start_line });
            index = end + 1;
            continue;
        }
        let name_start = index;
        while let Some(value) = bytes.get(index) {
            if value.is_ascii_whitespace() || *value == b'=' || *value == b'<' {
                break;
            }
            index += 1;
        }
        let name = text[name_start..index].to_ascii_lowercase();
        while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
            if bytes[index] == b'\n' {
                line += 1;
            }
            index += 1;
        }
        if bytes.get(index) != Some(&b'=') {
            if index == name_start {
                index += 1;
            }
            continue;
        }
        index += 1;
        while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
            if bytes[index] == b'\n' {
                line += 1;
            }
            index += 1;
        }
        let value_line = line;
        let value;
        if bytes.get(index) == Some(&b'"') {
            index += 1;
            let start = index;
            let Some(relative_end) = bytes[start..].iter().position(|byte| *byte == b'"') else {
                return Err(SfzError::UnterminatedQuote { line: value_line });
            };
            let end = start + relative_end;
            value = text[start..end].to_string();
            line += bytes[start..end].iter().filter(|byte| **byte == b'\n').count();
            index = end + 1;
        } else {
            let start = index;
            while let Some(byte) = bytes.get(index) {
                if byte.is_ascii_whitespace() || *byte == b'<' {
                    break;
                }
                index += 1;
            }
            value = text[start..index].to_string();
        }
        if result.len() >= max_opcodes {
            return Err(SfzError::LimitExceeded { what: "SFZ opcodes", limit: max_opcodes });
        }
        result.push(Token::Opcode { name, value, line: value_line });
    }
    Ok(result)
}
