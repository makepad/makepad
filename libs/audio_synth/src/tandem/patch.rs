//! A Tandem patch: the hybrid synth's `.fxp` patch read losslessly (the
//! 60-byte container, the `sub3` header, the XML document, embedded
//! wavetables, the compressed block storage), migrated from any revision
//! 0..31 to the current one, and written back in revision 31 layout.
//!
//! Values are kept raw: nothing is clamped on load (consumers clamp).

use super::binn;
use super::catalog::{self, catalog, gl, sc, Catalog, Owner, N_FX, N_GLOBAL, N_LFO, N_PARAMS};
use super::slots;
use super::xml::{self, Element};
use crate::kit::core::SlotValue;

/// Mirrors the reference: a type-dependent slot stored as an integer but of a float kind keeps
/// the integer's bits, so it reads as that bit pattern's float (old files wrote such values).
pub const QUIRK_INT_READ_FLOAT_SLOT_BITS: bool = true;

/// The revision this loader migrates to and writes.
pub const CURRENT_REVISION: i32 = 31;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Routing {
    /// Catalog index of the destination.
    pub dest: u16,
    pub source: u8,
    pub depth: f32,
    pub muted: bool,
    pub source_index: i32,
    /// For routings to global parameters: which scene's source.
    pub source_scene: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PortaOpts {
    pub const_rate: bool,
    pub gliss: bool,
    pub retrigger: bool,
    /// -1 log, 0 linear, 1 exp.
    pub curve: i8,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NonParam {
    pub mono_priority: [i32; 2],
    pub mono_envelope: [i32; 2],
    pub repeated_key: [i32; 2],
    /// 1 +18 dBFS, 2 0 dBFS, 3 off (scene only).
    pub hardclip_global: i32,
    pub hardclip_scene: [i32; 2],
    /// 0 retune everything, 1 retune MIDI only.
    pub tuning_mode: i32,
    pub detach_sustain: bool,
}

impl Default for NonParam {
    fn default() -> Self {
        NonParam { mono_priority: [1; 2], mono_envelope: [0; 2], repeated_key: [0; 2], hardclip_global: 1, hardclip_scene: [1; 2], tuning_mode: 1, detach_sustain: false }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct OscExtra {
    pub display_name: String,
    /// The wavetable script (decoded source), if any.
    pub script: Option<String>,
    pub script_nframes: i32,
    pub script_res_base: i32,
    pub extra: Vec<f32>,
}

impl Default for OscExtra {
    fn default() -> Self {
        OscExtra { display_name: String::new(), script: None, script_nframes: 10, script_res_base: 5, extra: Vec::new() }
    }
}

/// An embedded wavetable block as stored (lossless) plus its float view.
#[derive(Clone, Debug, PartialEq)]
pub struct Wavetable {
    pub samples_per_frame: usize,
    pub frames: usize,
    pub flags: u16,
    /// int16 data when flag 0x04, else empty.
    pub data_i16: Vec<i16>,
    /// float32 data when not int16, else empty.
    pub data_f32: Vec<f32>,
}

pub const WT_SAMPLE: u16 = 0x01;
pub const WT_LOOP: u16 = 0x02;
pub const WT_INT16: u16 = 0x04;
pub const WT_FULL16: u16 = 0x08;

impl Wavetable {
    /// The samples as floats (int16 / 16384, or / 32768 with full range).
    pub fn to_f32(&self) -> Vec<f32> {
        if self.flags & WT_INT16 != 0 {
            let scale = if self.flags & WT_FULL16 != 0 { 1.0 / 32768.0 } else { 1.0 / 16384.0 };
            self.data_i16.iter().map(|&s| s as f32 * scale).collect()
        } else {
            self.data_f32.clone()
        }
    }

    fn parse(b: &[u8]) -> Result<Wavetable, String> {
        if b.len() < 12 {
            return Err("wavetable block shorter than its header".into());
        }
        let n = u32::from_le_bytes([b[4], b[5], b[6], b[7]]) as usize;
        let frames = u16::from_le_bytes([b[8], b[9]]) as usize;
        let flags = u16::from_le_bytes([b[10], b[11]]);
        if n == 0 || n > 4096 || frames > 512 {
            return Err(format!("wavetable block: bad size {n} x {frames}"));
        }
        let width = if flags & WT_INT16 != 0 { 2 } else { 4 };
        let need = 12 + frames * n * width;
        if b.len() < need {
            return Err("wavetable block: payload too short".into());
        }
        let body = &b[12..need];
        let mut t = Wavetable { samples_per_frame: n, frames, flags, data_i16: Vec::new(), data_f32: Vec::new() };
        if width == 2 {
            t.data_i16 = body.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
        } else {
            t.data_f32 = body.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        }
        Ok(t)
    }

    fn write(&self) -> Vec<u8> {
        let mut out = vec![0u8; 4];
        out.extend_from_slice(&(self.samples_per_frame as u32).to_le_bytes());
        out.extend_from_slice(&(self.frames as u16).to_le_bytes());
        out.extend_from_slice(&self.flags.to_le_bytes());
        if self.flags & WT_INT16 != 0 {
            for s in &self.data_i16 {
                out.extend_from_slice(&s.to_le_bytes());
            }
        } else {
            for s in &self.data_f32 {
                out.extend_from_slice(&s.to_le_bytes());
            }
        }
        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StepSeqData {
    pub steps: [f32; 16],
    pub loop_start: i32,
    pub loop_end: i32,
    pub shuffle: f32,
    /// bits 0..15 retrigger both EGs, 16..31 filter EG, 32..47 amp EG.
    pub trigmask: u64,
}

impl Default for StepSeqData {
    fn default() -> Self {
        StepSeqData { steps: [0.0; 16], loop_start: 0, loop_end: 15, shuffle: 0.0, trigmask: 0 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MsegSegment {
    pub duration: f32,
    pub v0: f32,
    pub nv1: f32,
    pub cpduration: f32,
    pub cpv: f32,
    pub ty: i32,
    pub use_deform: bool,
    pub invert_deform: bool,
    pub retrigger_feg: bool,
    pub retrigger_aeg: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MsegData {
    pub segments: Vec<MsegSegment>,
    pub endpoint_mode: i32,
    pub edit_mode: i32,
    pub loop_mode: i32,
    /// `loopStartPoint` / `loopEndPoint` (point indices; -13214 = unset) as stored.
    pub loop_start_point: Option<i32>,
    pub loop_end_point: Option<i32>,
    /// Legacy `loopStart` / `loopEnd` (segment indices, -1 = unset) as stored.
    pub legacy_loop_start: Option<i32>,
    pub legacy_loop_end: Option<i32>,
    pub hsnap_default: f32,
    pub vsnap_default: f32,
    pub hsnap: f32,
    pub vsnap: f32,
    pub axis_width: f32,
    pub axis_start: f32,
}

pub const MSEG_UNSET: i32 = -13214;

#[derive(Clone, Debug, PartialEq)]
pub struct Macro {
    pub v: f32,
    pub bipolar: bool,
    pub label: String,
}

impl Default for Macro {
    fn default() -> Self {
        Macro { v: 0.0, bipolar: false, label: "-".into() }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LfoLabel {
    pub scene: u8,
    pub lfo: u8,
    pub idx: u8,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PatchTuning {
    pub scl_b64: String,
    pub kbm_b64: String,
    pub mapping_name: String,
}

/// The decoded block storage: per FX slot user data, per oscillator
/// wavetable snapshots, plus the raw binn tree for anything else.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlockStorage {
    pub fx: Vec<(usize, Vec<(String, Vec<u8>)>)>,
    pub tree: Option<binn::Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Patch {
    pub name: String,
    pub category: String,
    pub comment: String,
    pub author: String,
    pub license: String,
    pub tags: Vec<String>,
    /// The revision the file was written at (after load the state is current).
    pub revision: i32,
    pub values: Vec<SlotValue>,
    pub porta: [PortaOpts; 2],
    pub routings: Vec<Routing>,
    pub nonparam: NonParam,
    pub osc_extra: [[OscExtra; 3]; 2],
    pub tables: [[Option<Wavetable>; 3]; 2],
    pub stepseq: [[StepSeqData; N_LFO]; 2],
    pub msegs: [[Option<MsegData>; N_LFO]; 2],
    pub formulae: [[Option<String>; N_LFO]; 2],
    pub extra_lfo: [[i32; N_LFO]; 2],
    pub macros: [Macro; 8],
    pub lfo_labels: Vec<LfoLabel>,
    pub modwheel: [f32; 2],
    pub correct_comb: bool,
    pub tuning: Option<PatchTuning>,
    pub tempo_on_save: Option<f32>,
    pub blocks: BlockStorage,
    /// `<parameters>` children with no catalogue entry (names), kept as
    /// read and written back so a round trip loses nothing.
    pub unknown: Vec<String>,
    pub unknown_xml: Vec<Element>,
    pub warnings: Vec<String>,
    /// The revision the file was written at (`revision` is current after load).
    pub loaded_revision: i32,
    // Load-time scratch.
    abs_size: Option<usize>,
    deact_seen: Vec<bool>,
    raw_ints: Vec<Option<i32>>,
}

impl Default for Patch {
    fn default() -> Self {
        Patch::init()
    }
}

/// How a patch is read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LoadMode {
    #[default]
    Patch,
    /// Keep session state: no fx_bypass, no meta name, no mod wheel.
    Preset,
}

impl Patch {
    /// The init state every load starts from (section 3.2.1 rule 1 and 3.3.5).
    pub fn init() -> Patch {
        let c = catalog();
        let mut values: Vec<SlotValue> = c
            .params
            .iter()
            .map(|p| {
                let mut v = if p.int { SlotValue::int(p.default as i32) } else { SlotValue::float(p.default) };
                v.deactivated = p.can_deactivate && p.default_deactivated;
                v
            })
            .collect();
        for s in 0..2 {
            let sp = |i: usize| Catalog::scene_param(s, i);
            // Both EGs: attack -8, decay -2, sustain 1, release -5.
            for e in 0..2 {
                let b = sc::ENV + sc::ENV_STRIDE * e;
                values[sp(b)].v = -8.0;
                values[sp(b + 2)].v = -2.0;
                values[sp(b + 4)].v = 1.0;
                values[sp(b + 5)].v = -5.0;
            }
            for o in 0..3 {
                values[sp(sc::OSC + sc::OSC_STRIDE * o + 10)].v = 1.0;
            }
        }
        Patch {
            name: String::new(),
            category: String::new(),
            comment: String::new(),
            author: String::new(),
            license: String::new(),
            tags: Vec::new(),
            revision: CURRENT_REVISION,
            values,
            porta: [PortaOpts::default(); 2],
            routings: Vec::new(),
            nonparam: NonParam::default(),
            osc_extra: Default::default(),
            tables: Default::default(),
            stepseq: [[StepSeqData::default(); N_LFO]; 2],
            msegs: Default::default(),
            formulae: Default::default(),
            extra_lfo: [[0; N_LFO]; 2],
            macros: Default::default(),
            lfo_labels: Vec::new(),
            modwheel: [0.0; 2],
            correct_comb: true,
            tuning: None,
            tempo_on_save: None,
            blocks: BlockStorage::default(),
            unknown: Vec::new(),
            unknown_xml: Vec::new(),
            warnings: Vec::new(),
            loaded_revision: CURRENT_REVISION,
            abs_size: None,
            deact_seen: Vec::new(),
            raw_ints: Vec::new(),
        }
    }

    /// Set a parameter by storage name (`a_filter1_cutoff`), raw units;
    /// false if there is none. An oscillator or FX type set this way resets
    /// that unit's slots to the type's defaults (an interactive change).
    pub fn set(&mut self, name: &str, v: f32) -> bool {
        let c = catalog();
        let Some(i) = c.index(name) else { return false };
        let info = &c.params[i];
        self.values[i].v = if info.int { v.round() } else { v };
        self.values[i].int = info.int;
        if let Some(rest) = name.strip_prefix("a_").or_else(|| name.strip_prefix("b_")) {
            let s = if name.starts_with("a_") { 0 } else { 1 };
            for o in 0..3 {
                if rest == format!("osc{}_type", o + 1) {
                    let b = Catalog::scene_param(s, sc::OSC + sc::OSC_STRIDE * o + 3);
                    let (d, retrigger) = super::snapshots::osc(v.round().clamp(0.0, 11.0) as u8);
                    self.values[b..b + 7].copy_from_slice(&d);
                    if let Some(r) = retrigger {
                        self.values[b + 8].v = r as i32 as f32;
                    }
                    self.osc_extra[s][o].extra = crate::kit::oscs::default_extra(v.round().clamp(0.0, 11.0) as u8);
                }
            }
        }
        if i >= gl::FX && i < N_GLOBAL && (i - gl::FX) % gl::FX_STRIDE == 0 {
            let slot = (i - gl::FX) / gl::FX_STRIDE;
            let d = crate::kit::fxs::init_values(v.round().clamp(0.0, 31.0) as u8, matches!(slot, 4 | 5 | 12 | 13));
            self.values[i + 1..i + 13].copy_from_slice(&d);
        }
        true
    }

    /// A flag of a parameter by storage name.
    pub fn value_mut(&mut self, name: &str) -> Option<&mut SlotValue> {
        catalog().index(name).map(move |i| &mut self.values[i])
    }

    pub fn scene(&self, s: usize, i: usize) -> &SlotValue {
        &self.values[Catalog::scene_param(s, i)]
    }

    pub fn scene_mut(&mut self, s: usize, i: usize) -> &mut SlotValue {
        &mut self.values[Catalog::scene_param(s, i)]
    }

    pub fn global(&self, i: usize) -> &SlotValue {
        &self.values[i]
    }

    pub fn osc_type(&self, s: usize, o: usize) -> u8 {
        self.scene(s, sc::OSC + sc::OSC_STRIDE * o).i().clamp(0, 11) as u8
    }

    pub fn fx_type(&self, slot: usize) -> u8 {
        self.global(gl::FX + gl::FX_STRIDE * slot).i().clamp(0, 31) as u8
    }

    /// An oscillator's 7 slots.
    pub fn osc_slots(&self, s: usize, o: usize) -> [SlotValue; 7] {
        let b = Catalog::scene_param(s, sc::OSC + sc::OSC_STRIDE * o + 3);
        std::array::from_fn(|k| self.values[b + k])
    }

    pub fn fx_slots(&self, slot: usize) -> [SlotValue; 12] {
        let b = gl::FX + gl::FX_STRIDE * slot + 1;
        std::array::from_fn(|k| self.values[b + k])
    }

    // ------------------------------------------------------------- reading

    /// Read an `.fxp` file (or a bare payload: `sub3` or XML).
    pub fn from_fxp(bytes: &[u8], file_stem: &str, mode: LoadMode) -> Result<Patch, String> {
        let payload = if bytes.len() >= 60 && &bytes[0..4] == b"CcnK" {
            if &bytes[8..12] != b"FPCh" {
                return Err("not an opaque-chunk program (FPCh)".into());
            }
            if &bytes[16..20] != b"cjs3" {
                return Err("not one of this synth's patches (plugin id)".into());
            }
            let n = u32::from_be_bytes([bytes[56], bytes[57], bytes[58], bytes[59]]) as usize;
            let end = (60 + n).min(bytes.len());
            &bytes[60..end]
        } else {
            bytes
        };
        let mut p = Patch::from_payload(payload, mode)?;
        if p.name.is_empty() || mode == LoadMode::Preset {
            p.name = file_stem.to_string();
        }
        Ok(p)
    }

    /// Read a host-state / patch payload.
    pub fn from_payload(b: &[u8], mode: LoadMode) -> Result<Patch, String> {
        if b.len() >= 32 && &b[0..4] == b"sub3" {
            let le = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as usize;
            let x = le(4);
            if x >= 4 << 20 {
                return Err("XML size over 4 MiB".into());
            }
            let w: [usize; 6] = std::array::from_fn(|k| le(8 + 4 * k));
            let mut at = 32;
            if at + x > b.len() {
                return Err("truncated XML".into());
            }
            let text = xml::decode_text(&b[at..at + x]);
            at += x;
            let mut p = Patch::from_xml(&text, mode)?;
            for (k, &size) in w.iter().enumerate() {
                if size == 0 {
                    continue;
                }
                let (s, o) = (k / 3, k % 3);
                if at + size > b.len() {
                    p.warnings.push(format!("wavetable block {s}/{o} truncated"));
                    at = b.len();
                    continue;
                }
                match Wavetable::parse(&b[at..at + size]) {
                    Ok(t) => p.tables[s][o] = Some(t),
                    Err(e) => p.warnings.push(e),
                }
                at += size;
            }
            // Block storage.
            let rest = &b[at.min(b.len())..];
            if !rest.is_empty() {
                let block = if p.revision >= 29 {
                    match p.abs_size {
                        Some(sz) if sz <= rest.len() => Some(&rest[..sz]),
                        Some(_) => {
                            p.warnings.push("block storage truncated: skipped".into());
                            None
                        }
                        None => None,
                    }
                } else {
                    Some(rest)
                };
                if let Some(block) = block {
                    p.read_blocks(block);
                }
            }
            p.post_load();
            Ok(p)
        } else {
            let text = xml::decode_text(b);
            let mut p = Patch::from_xml(&text, mode)?;
            p.post_load();
            Ok(p)
        }
    }

    fn read_blocks(&mut self, block: &[u8]) {
        let data = match slots::zstd_decode(block, 256 << 20) {
            Ok(d) => d,
            Err(e) => {
                if self.revision >= 29 {
                    self.warnings.push(format!("block storage: {e}"));
                }
                return;
            }
        };
        match binn::parse(&data) {
            Ok(tree) => {
                for s in 0..N_FX {
                    if let Some(binn::Value::Object(items)) = tree.get(&format!("fx{s}")) {
                        let entries = items.iter().filter_map(|(k, v)| v.blob().map(|b| (k.clone(), b.to_vec()))).collect();
                        self.blocks.fx.push((s, entries));
                    }
                }
                self.blocks.tree = Some(tree);
            }
            Err(e) => self.warnings.push(format!("block storage: {e}")),
        }
    }

    /// Parse the XML document. `post_load` (migrations and slot kinds)
    /// runs after the binary parts are read.
    pub fn from_xml(text: &str, mode: LoadMode) -> Result<Patch, String> {
        let root = xml::parse(text).ok_or("no XML root element")?;
        if root.name != "patch" {
            return Err(format!("root element is <{}>, not <patch>", root.name));
        }
        let mut p = Patch::init();
        let rev = root.attr_i64("revision").unwrap_or(0) as i32;
        p.revision = rev;
        if rev > CURRENT_REVISION {
            p.warnings.push(format!("revision {rev} is newer than {CURRENT_REVISION}: loading what is known"));
        }
        if let Some(m) = root.child("meta") {
            if mode == LoadMode::Patch {
                p.name = m.attr("name").unwrap_or("").to_string();
                p.category = m.attr("category").unwrap_or("").to_string();
            }
            p.comment = m.attr("comment").unwrap_or("").to_string();
            p.author = m.attr("author").unwrap_or("").to_string();
            p.license = m.attr("license").unwrap_or("").to_string();
            if let Some(tags) = m.child("tags") {
                p.tags = tags.children_named("tag").filter_map(|t| t.attr("tag").map(str::to_string)).collect();
            }
        }
        p.abs_size = root.child("serialization").and_then(|s| s.attr_i64("absSize")).map(|v| v.max(0) as usize);
        let c = catalog();
        if let Some(params) = root.child("parameters") {
            p.read_params(params, mode, c);
        }
        p.read_nonparam(root.child("nonparamconfig"));
        if let Some(x) = root.child("extraoscdata") {
            for e in &x.children {
                let (Some(s), Some(o)) = (e.attr_i64("scene"), e.attr_i64("osc")) else { continue };
                if !(0..2).contains(&s) || !(0..3).contains(&o) {
                    continue;
                }
                let ex = &mut p.osc_extra[s as usize][o as usize];
                ex.display_name = e.attr("wavetable_display_name").unwrap_or("").to_string();
                let script = e.attr("wavetable_script").or(e.attr("wavetable_formula"));
                if let Some(b64) = script.filter(|s| !s.is_empty()) {
                    ex.script = makepad_base64::base64_decode(b64.as_bytes()).ok().map(|b| String::from_utf8_lossy(&b).into_owned());
                }
                ex.script_nframes = e.attr_i64("wavetable_script_nframes").or(e.attr_i64("wavetable_formula_nframes")).unwrap_or(10) as i32;
                ex.script_res_base = e.attr_i64("wavetable_script_res_base").or(e.attr_i64("wavetable_formula_res_base")).unwrap_or(5) as i32;
                let n = e.attr_i64("extra_n").unwrap_or(0).clamp(0, 64) as usize;
                ex.extra = (0..n).map(|k| e.attr_f64(&format!("extra_data_{k}")).unwrap_or(0.0) as f32).collect();
            }
        }
        if let Some(x) = root.child("stepsequences") {
            for e in x.children_named("sequence") {
                let Some((s, l)) = scene_lfo(e) else { continue };
                let q = &mut p.stepseq[s][l];
                for k in 0..16 {
                    q.steps[k] = e.attr_f64(&format!("s{k}")).unwrap_or(0.0) as f32;
                }
                q.loop_start = e.attr_i64("loop_start").unwrap_or(0) as i32;
                q.loop_end = e.attr_i64("loop_end").unwrap_or(15) as i32;
                q.shuffle = e.attr_f64("shuffle").unwrap_or(0.0) as f32;
                let mut mask = e.attr_i64("trigmask").unwrap_or(0) as u64 & 0xFFFF;
                for (w, key) in ["trigmask_0to15", "trigmask_16to31", "trigmask_32to47"].iter().enumerate() {
                    if let Some(v) = e.attr_i64(key) {
                        mask = (mask & !(0xFFFFu64 << (16 * w))) | ((v as u64 & 0xFFFF) << (16 * w));
                    }
                }
                q.trigmask = mask;
            }
        }
        if let Some(x) = root.child("msegs") {
            for e in x.children_named("mseg") {
                let Some((s, l)) = scene_lfo(e) else { continue };
                p.msegs[s][l] = Some(read_mseg(e, rev));
            }
        }
        if let Some(x) = root.child("formulae") {
            for e in x.children_named("formula") {
                let Some((s, l)) = scene_lfo(e) else { continue };
                if let Some(b64) = e.attr("formula") {
                    p.formulae[s][l] = makepad_base64::base64_decode(b64.as_bytes()).ok().map(|b| String::from_utf8_lossy(&b).into_owned());
                }
            }
        }
        if let Some(x) = root.child("extralfo") {
            for e in x.children_named("lfo") {
                let Some((s, l)) = scene_lfo(e) else { continue };
                p.extra_lfo[s][l] = e.attr_i64("extraAmplitude").unwrap_or(0) as i32;
            }
        }
        if let Some(x) = root.child("customcontroller") {
            for e in x.children_named("entry") {
                if e.attr_i64("scene").unwrap_or(0) != 0 {
                    continue;
                }
                let Some(i) = e.attr_i64("i").filter(|i| (0..8).contains(i)) else { continue };
                let m = &mut p.macros[i as usize];
                m.v = e.attr_f64("v").unwrap_or(0.0) as f32;
                m.bipolar = e.attr_i64("bipolar").unwrap_or(0) != 0;
                if rev >= 9 {
                    if let Some(l) = e.attr("label") {
                        m.label = l.to_string();
                    }
                }
            }
        }
        if let Some(x) = root.child("lfobanklabels") {
            for e in x.children_named("label") {
                let (Some(l), Some(i), Some(s)) = (e.attr_i64("lfo"), e.attr_i64("idx"), e.attr_i64("scene")) else { continue };
                if (0..12).contains(&l) && (0..16).contains(&i) && (0..2).contains(&s) {
                    p.lfo_labels.push(LfoLabel { scene: s as u8, lfo: l as u8, idx: i as u8, text: e.attr("v").unwrap_or("").to_string() });
                }
            }
        }
        if mode == LoadMode::Patch {
            if let Some(m) = root.child("modwheel") {
                p.modwheel = [m.attr_f64("s0").unwrap_or(0.0) as f32, m.attr_f64("s1").unwrap_or(0.0) as f32];
            }
        }
        p.correct_comb = rev >= 14;
        if let Some(v) = root.child("compatability").and_then(|c| c.child("correctlyTunedCombFilter")).and_then(|e| e.attr_i64("v")) {
            p.correct_comb = v != 0;
        }
        if let Some(t) = root.child("patchTuning") {
            p.tuning = Some(PatchTuning {
                scl_b64: t.attr("v").unwrap_or("").to_string(),
                kbm_b64: t.attr("m").unwrap_or("").to_string(),
                mapping_name: t.attr("mname").unwrap_or("").to_string(),
            });
        }
        p.tempo_on_save = root.child("tempoOnSave").and_then(|t| t.attr_f64("v")).map(|v| v as f32);
        if rev < 23 {
            p.tempo_on_save = None;
        } else if p.tempo_on_save.is_none() {
            p.tempo_on_save = Some(120.0);
        }
        Ok(p)
    }

    fn read_params(&mut self, params: &Element, mode: LoadMode, c: &Catalog) {
        let rev = self.revision;
        // Rule 1 exceptions: polylimit 16 (volume / fx_bypass keep the session's: here the init).
        self.values[gl::POLYLIMIT] = SlotValue::int(16);
        // Presence of `deactivated` per parameter, to apply the absent-default after the type is known.
        let mut deact_seen = vec![false; N_PARAMS];
        let mut extend_seen = vec![false; N_PARAMS];
        let mut raw_ints: Vec<Option<i32>> = vec![None; N_PARAMS];
        for e in &params.children {
            let Some(idx) = c.index(&e.name) else {
                self.unknown.push(e.name.clone());
                self.unknown_xml.push(e.clone());
                continue;
            };
            if mode == LoadMode::Preset && (idx == gl::FX_BYPASS || (rev < 17 && idx == gl::VOLUME)) {
                continue;
            }
            let info = &c.params[idx];
            let ty = e.attr_i64("type");
            let is_float = match ty {
                Some(2) => true,
                Some(_) => false,
                None => !info.int,
            };
            let v = &mut self.values[idx];
            if is_float {
                v.int = false;
                if let Some(x) = e.attr_f64("value") {
                    v.v = x as f32;
                }
            } else {
                v.int = true;
                if let Some(x) = e.attr_i64("value") {
                    v.v = x.clamp(i32::MIN as i64, i32::MAX as i64) as f32;
                    raw_ints[idx] = Some(x as i32);
                } else if let Some(x) = e.attr_f64("value") {
                    v.v = x as i32 as f32;
                }
            }
            if let Some(t) = e.attr_i64("temposync") {
                v.temposync = t != 0;
            }
            if let Some(t) = e.attr_i64("extend_range") {
                v.extend = t != 0;
                extend_seen[idx] = true;
            }
            if let Some(t) = e.attr_i64("absolute") {
                v.absolute = t != 0;
            }
            if let Some(t) = e.attr_i64("deactivated") {
                v.deactivated = t != 0;
                deact_seen[idx] = true;
            }
            if let Some(t) = e.attr_i64("deform_type") {
                v.deform = t as i32;
            }
            if idx == Catalog::scene_param(0, sc::PORTAMENTO) || idx == Catalog::scene_param(1, sc::PORTAMENTO) {
                let s = if idx == Catalog::scene_param(0, sc::PORTAMENTO) { 0 } else { 1 };
                let po = &mut self.porta[s];
                po.const_rate = e.attr_i64("porta_const_rate").unwrap_or(0) != 0;
                po.gliss = e.attr_i64("porta_gliss").unwrap_or(0) != 0;
                po.retrigger = e.attr_i64("porta_retrigger").unwrap_or(0) != 0;
                po.curve = match e.attr_i64("porta_curve") {
                    Some(-1) => -1,
                    Some(1) => 1,
                    _ => 0,
                };
            }
            // Routings: only on float parameters (or elements without a type).
            if ty.is_none() || ty == Some(2) {
                for r in e.children_named("modrouting") {
                    self.read_routing(r, idx, info, rev);
                }
            }
        }
        // Rule 6 / 7: absent attributes. Slot kinds come from the types (post_load).
        for (idx, info) in c.params.iter().enumerate() {
            if matches!(info.owner, Owner::OscSlot { .. } | Owner::FxSlot { .. }) {
                continue;
            }
            if info.can_deactivate && !deact_seen[idx] {
                self.values[idx].deactivated = info.default_deactivated;
            }
            if !extend_seen[idx] && info.short == "drift" && rev >= 16 {
                self.values[idx].extend = true;
            }
        }
        self.deact_seen = deact_seen;
        self.raw_ints = raw_ints.clone();
        // Rule 9: pitch bend ranges stored as float bit patterns.
        let pb_a = Catalog::scene_param(0, sc::PBRANGE_UP);
        if let Some(raw) = raw_ints[pb_a] {
            if raw as u32 & !0xFF != 0 {
                for s in 0..2 {
                    for k in [sc::PBRANGE_UP, sc::PBRANGE_DN] {
                        let i = Catalog::scene_param(s, k);
                        let bits = raw_ints[i].unwrap_or(self.values[i].v as i32) as u32;
                        self.values[i].v = (f32::from_bits(bits) as i32) as f32;
                    }
                }
            }
        }
    }

    fn read_routing(&mut self, r: &Element, idx: usize, info: &catalog::ParamInfo, rev: i32) {
        let Some(mut src) = r.attr_i64("source") else { return };
        if rev < 9 && src > 13 {
            src += 1;
        }
        if rev < 21 && idx == gl::VOLUME {
            return;
        }
        let index = r.attr_i64("source_index").unwrap_or(0);
        let scene = r.attr_i64("source_scene").unwrap_or(0);
        if src <= 0 || src >= 41 || index < 0 || !(0..=1).contains(&scene) {
            self.warnings.push(format!("dropped routing source {src} on {}", info.name));
            return;
        }
        let src = src as u8;
        let source_scene = match info.owner {
            Owner::Global | Owner::FxSlot { .. } if catalog::source_is_per_scene(src) => scene as u8,
            _ => 0,
        };
        self.routings.push(Routing {
            dest: idx as u16,
            source: src,
            depth: r.attr_f64("depth").unwrap_or(0.0) as f32,
            muted: r.attr_i64("muted").unwrap_or(0) != 0,
            source_index: index as i32,
            source_scene,
        });
    }

    fn read_nonparam(&mut self, e: Option<&Element>) {
        let rev = self.revision;
        let np = &mut self.nonparam;
        if let Some(e) = e {
            let v = |name: &str| e.child(name).and_then(|c| c.attr_i64("v"));
            for s in 0..2 {
                if let Some(x) = v(&format!("monoVoicePrority_{s}")) {
                    np.mono_priority[s] = x as i32;
                }
                if let Some(x) = v(&format!("monoVoiceEnvelope_{s}")) {
                    np.mono_envelope[s] = x as i32;
                }
                if let Some(x) = v(&format!("polyVoiceRepeatedKeyMode_{s}")) {
                    np.repeated_key[s] = x as i32;
                }
            }
            if let Some(h) = e.child("hardclipmodes") {
                let ok = |x: i64, scene: bool| (1..=if scene { 3 } else { 2 }).contains(&x) || (!scene && x == 3);
                if let Some(x) = h.attr_i64("global").filter(|x| ok(*x, false)) {
                    np.hardclip_global = x as i32;
                }
                for s in 0..2 {
                    if let Some(x) = h.attr_i64(&format!("sc{s}")).filter(|x| ok(*x, true)) {
                        np.hardclip_scene[s] = x as i32;
                    }
                }
            }
            if let Some(x) = v("tuningApplicationMode") {
                np.tuning_mode = x as i32;
            }
            if let Some(x) = v("detachSustainPedalFromVoicing") {
                np.detach_sustain = x != 0;
            }
        }
        if rev < 15 {
            np.mono_priority = [0; 2];
        }
        if rev < 20 {
            np.mono_envelope = [0; 2];
            np.repeated_key = [0; 2];
        }
        if rev <= 15 {
            np.tuning_mode = 0;
        }
    }

    /// Slot kinds and every revision migration (section 6), in order.
    fn post_load(&mut self) {
        let rev = self.revision;
        let deact_seen = std::mem::take(&mut self.deact_seen);
        // Oscillator and FX slot kinds: absent `deactivated` defaults (rule 6), int/float kind.
        for s in 0..2 {
            for o in 0..3 {
                let ty = self.osc_type(s, o);
                let defs = slots::osc_slot_defs(ty);
                let b = Catalog::scene_param(s, sc::OSC + sc::OSC_STRIDE * o + 3);
                for k in 0..7 {
                    if defs[k].can_deactivate && !deact_seen.get(b + k).copied().unwrap_or(false) {
                        self.values[b + k].deactivated = true;
                    }
                }
            }
        }
        for f in 0..N_FX {
            let ty = self.fx_type(f);
            let defs = slots::fx_slot_defs(ty);
            let b = gl::FX + gl::FX_STRIDE * f + 1;
            for k in 0..12 {
                if defs[k].can_deactivate && !deact_seen.get(b + k).copied().unwrap_or(false) {
                    self.values[b + k].deactivated = rev != 16;
                }
            }
        }
        for s in 0..2 {
            let sp = |i: usize| Catalog::scene_param(s, i);
            // rev < 1: attack shapes += 1.
            if rev < 1 {
                for e in 0..2 {
                    let i = sp(sc::ENV + sc::ENV_STRIDE * e + 1);
                    self.values[i].v = (self.values[i].v + 1.0).clamp(0.0, 2.0);
                }
            }
            // rev < 2: LFO sustain from decay.
            if rev < 2 {
                for l in 0..N_LFO {
                    let b = sp(sc::LFO + sc::LFO_STRIDE * l);
                    self.values[b + 11].v = if self.values[b + 10].v >= 5.0 { 1.0 } else { 0.0 };
                }
            }
            let stereo = self.values[sp(sc::FB_CONFIG)].i() == 5;
            if rev < 5 && stereo {
                self.values[sp(sc::PAN)].v = -1.0;
                self.values[sp(sc::WIDTH)].v = 1.0;
            }
            for u in 0..2 {
                let tb = sp(if u == 0 { sc::F1_TYPE } else { sc::F2_TYPE });
                let ri = sp(if u == 0 { sc::F1_RESO } else { sc::F2_RESO });
                let mut ty = self.values[tb].i();
                let mut sub = self.values[tb + 1].i();
                let mut reso = self.values[ri].v;
                migrate_filter(rev, &mut ty, &mut sub, &mut reso);
                self.values[tb].v = ty as f32;
                self.values[tb + 1].v = sub as f32;
                self.values[ri].v = reso;
            }
            if rev < 8 && stereo {
                let (p1, p2) = (self.values[sp(sc::PAN)].v, self.values[sp(sc::WIDTH)].v);
                self.values[sp(sc::PAN)].v = (p1 + p2) / 2.0;
                self.values[sp(sc::WIDTH)].v = (p2 - p1) / 2.0;
            }
            if rev <= 21 {
                self.values[sp(sc::MIX + 4 * 3)].deform = 0;
                self.values[sp(sc::MIX + 4 * 4)].deform = 0;
                self.values[sp(sc::VOLUME)].deactivated = false;
            }
            if rev <= 22 {
                for e in 0..2 {
                    self.values[sp(sc::ENV + sc::ENV_STRIDE * e + 5)].deform = 0;
                }
            }
            if rev <= 23 {
                for c in [3, 4] {
                    let v = &mut self.values[sp(sc::MIX + 4 * c)];
                    if v.deform > 4 {
                        v.deform += 2;
                    }
                }
            }
            if rev <= 25 {
                for l in 0..N_LFO {
                    let v = &mut self.values[sp(sc::LFO + sc::LFO_STRIDE * l + 3)];
                    if v.extend {
                        v.v = 2.0 * v.v - 1.0;
                    }
                }
            }
            // Subtype clamp (always).
            for tb in [sc::F1_TYPE, sc::F2_TYPE] {
                let ty = self.values[sp(tb)].i();
                let n = slots::filter_subtype_count(ty);
                let sv = &mut self.values[sp(tb + 1)];
                sv.v = (sv.i().clamp(0, (n - 1).max(0))) as f32;
            }
            // Per-oscillator migrations and the wavetable flag migration.
            for o in 0..3 {
                let ty = self.osc_type(s, o);
                let tb = sc::OSC + sc::OSC_STRIDE * o;
                let mut slots7 = self.osc_slots(s, o);
                let mut retrig = self.scene(s, tb + 11).on();
                let is_sample = self.tables[s][o].as_ref().is_some_and(|t| t.flags & WT_SAMPLE != 0);
                slots::osc_migrate(ty, rev, &mut slots7, &mut retrig, is_sample);
                if rev <= 30 && is_sample {
                    if ty == 7 {
                        if let Some(t) = &mut self.tables[s][o] {
                            t.flags &= !(WT_SAMPLE | WT_LOOP);
                        }
                    } else if ty == 2 {
                        // (The play-count deform is the oscillator's own patch rule.)
                        if slots7[slots::WT_VOICES_SLOT].v >= 7.0 {
                            if let Some(t) = &mut self.tables[s][o] {
                                t.flags |= WT_LOOP;
                            }
                        }
                    }
                }
                let b0 = Catalog::scene_param(s, tb + 3);
                for (k, (v, d)) in slots7.iter_mut().zip(slots::osc_slot_defs(ty).iter()).enumerate() {
                    if d.used() {
                        if QUIRK_INT_READ_FLOAT_SLOT_BITS && !d.int && v.int {
                            if let Some(raw) = self.raw_ints.get(b0 + k).copied().flatten() {
                                v.v = f32::from_bits(raw as u32);
                            }
                        }
                        v.int = d.int && v.v.fract() == 0.0;
                    }
                }
                let b = Catalog::scene_param(s, tb + 3);
                self.values[b..b + 7].copy_from_slice(&slots7);
                if ty == 10 {
                    // rev <= 27: the twist oscillator's pitch correction.
                    let pv = *self.scene(s, tb + 2);
                    self.scene_mut(s, tb + 2).v = crate::kit::oscs::twist::legacy_pitch_fix(rev, pv.v, pv.extend, pv.absolute);
                }
                self.scene_mut(s, tb + 11).v = retrig as i32 as f32;
            }
        }
        if rev < 10 {
            self.values[Catalog::CHARACTER].v = 0.0;
        }
        if rev <= 15 && self.values[gl::POLYLIMIT].i() == 8 {
            self.values[gl::POLYLIMIT].v = 16.0;
        }
        for f in 0..N_FX {
            let ty = self.fx_type(f);
            if ty == 0 {
                continue;
            }
            let mut p = self.fx_slots(f);
            // The slots take their kind from the type (int or float), as the reference assigns it after load.
            let b0 = gl::FX + gl::FX_STRIDE * f + 1;
            for (k, (v, d)) in p.iter_mut().zip(slots::fx_slot_defs(ty).iter()).enumerate() {
                if d.used() {
                    if QUIRK_INT_READ_FLOAT_SLOT_BITS && !d.int && v.int {
                        if let Some(raw) = self.raw_ints.get(b0 + k).copied().flatten() {
                            v.v = f32::from_bits(raw as u32);
                        }
                    }
                }
            }
            // FX slot values are clamped into the type's range after load (osc slots are not).
            slots::fx_clamp(ty, &mut p);
            for (v, d) in p.iter_mut().zip(slots::fx_slot_defs(ty).iter()) {
                if d.used() {
                    v.int = d.int && v.v.fract() == 0.0;
                }
            }
            slots::fx_migrate(ty, rev, &mut p);
            let b = gl::FX + gl::FX_STRIDE * f + 1;
            self.values[b..b + 12].copy_from_slice(&p);
        }
        if rev < 16 {
            for s in 0..2 {
                for l in 0..N_LFO {
                    if let Some(m) = &mut self.msegs[s][l] {
                        if m.edit_mode == 1 {
                            m.endpoint_mode = 1;
                        }
                    }
                }
            }
        }
        self.revision = CURRENT_REVISION.max(rev);
        self.loaded_revision = rev;
    }

    // ------------------------------------------------------------- writing

    /// The XML document (revision 31 layout).
    pub fn to_xml(&self) -> String {
        let c = catalog();
        let mut root = Element::new("patch").with("revision", CURRENT_REVISION.max(self.revision).to_string());
        let mut meta = Element::new("meta")
            .with("name", &self.name)
            .with("category", &self.category)
            .with("comment", &self.comment)
            .with("author", &self.author)
            .with("license", &self.license);
        let mut tags = Element::new("tags");
        for t in &self.tags {
            tags.children.push(Element::new("tag").with("tag", t));
        }
        meta.children.push(tags);
        root.children.push(meta);
        let mut params = Element::new("parameters");
        let mut dests: Vec<Vec<&Routing>> = vec![Vec::new(); N_PARAMS];
        for r in &self.routings {
            dests[r.dest as usize].push(r);
        }
        for (idx, info) in c.params.iter().enumerate() {
            let v = &self.values[idx];
            if let Owner::FxSlot { fx, .. } = info.owner {
                if self.fx_type(fx as usize) == 0 {
                    continue;
                }
            }
            if idx >= gl::FX && idx < N_GLOBAL && (idx - gl::FX) % gl::FX_STRIDE == 0 && v.i() == 0 {
                continue;
            }
            let mut e = Element::new(&info.name).with("type", if v.int { "0" } else { "2" }).with("value", fmt_value(v));
            let (ext, deact, deform, sync, abs) = self.flag_kinds(idx, info);
            let _ = (sync, abs);
            if v.temposync {
                e.set("temposync", "1");
            }
            if ext || v.extend {
                e.set("extend_range", if v.extend { "1" } else { "0" });
            }
            if v.absolute {
                e.set("absolute", "1");
            }
            if deact || v.deactivated {
                e.set("deactivated", if v.deactivated { "1" } else { "0" });
            }
            if deform || v.deform != 0 {
                e.set("deform_type", v.deform.to_string());
            }
            if info.short == "portamento" {
                let s = if idx < Catalog::scene_param(1, 0) { 0 } else { 1 };
                let po = &self.porta[s];
                e.set("porta_const_rate", (po.const_rate as i32).to_string());
                e.set("porta_gliss", (po.gliss as i32).to_string());
                e.set("porta_retrigger", (po.retrigger as i32).to_string());
                e.set("porta_curve", po.curve.to_string());
            }
            for r in &dests[idx] {
                let mut m = Element::new("modrouting")
                    .with("source", r.source.to_string())
                    .with("depth", fmt_f32(r.depth))
                    .with("muted", (r.muted as i32).to_string())
                    .with("source_index", r.source_index.to_string());
                if matches!(info.owner, Owner::Global | Owner::FxSlot { .. }) {
                    m.set("source_scene", r.source_scene.to_string());
                }
                e.children.push(m);
            }
            params.children.push(e);
        }
        params.children.extend(self.unknown_xml.iter().cloned());
        root.children.push(params);
        let np = &self.nonparam;
        let mut npe = Element::new("nonparamconfig");
        for s in 0..2 {
            npe.children.push(Element::new(&format!("monoVoicePrority_{s}")).with("v", np.mono_priority[s].to_string()));
        }
        for s in 0..2 {
            npe.children.push(Element::new(&format!("monoVoiceEnvelope_{s}")).with("v", np.mono_envelope[s].to_string()));
        }
        for s in 0..2 {
            npe.children.push(Element::new(&format!("polyVoiceRepeatedKeyMode_{s}")).with("v", np.repeated_key[s].to_string()));
        }
        npe.children.push(
            Element::new("hardclipmodes").with("global", np.hardclip_global.to_string()).with("sc0", np.hardclip_scene[0].to_string()).with("sc1", np.hardclip_scene[1].to_string()),
        );
        npe.children.push(Element::new("tuningApplicationMode").with("v", np.tuning_mode.to_string()));
        npe.children.push(Element::new("detachSustainPedalFromVoicing").with("v", (np.detach_sustain as i32).to_string()));
        root.children.push(npe);
        let mut eo = Element::new("extraoscdata");
        for s in 0..2 {
            for o in 0..3 {
                let x = &self.osc_extra[s][o];
                let mut e = Element::new(&format!("osc_extra_sc{s}_osc{o}")).with("scene", s.to_string()).with("osc", o.to_string()).with("wavetable_display_name", &x.display_name);
                if let Some(script) = &x.script {
                    let b64 = makepad_base64::base64_encode(script.as_bytes(), &makepad_base64::BASE64_STANDARD);
                    e.set("wavetable_script", String::from_utf8_lossy(&b64).into_owned());
                    e.set("wavetable_script_nframes", x.script_nframes.to_string());
                    e.set("wavetable_script_res_base", x.script_res_base.to_string());
                }
                e.set("extra_n", x.extra.len().to_string());
                for (k, v) in x.extra.iter().enumerate() {
                    e.set(&format!("extra_data_{k}"), fmt_f32(*v));
                }
                eo.children.push(e);
            }
        }
        root.children.push(eo);
        let mut ss = Element::new("stepsequences");
        let mut ms = Element::new("msegs");
        let mut fo = Element::new("formulae");
        let mut el = Element::new("extralfo");
        for s in 0..2 {
            for l in 0..N_LFO {
                let shape = self.scene(s, sc::LFO + sc::LFO_STRIDE * l).i();
                let q = &self.stepseq[s][l];
                if shape == 7 || *q != StepSeqData::default() {
                    let mut e = Element::new("sequence").with("scene", s.to_string()).with("i", l.to_string());
                    for k in 0..16 {
                        e.set(&format!("s{k}"), fmt_f32(q.steps[k]));
                    }
                    e.set("loop_start", q.loop_start.to_string());
                    e.set("loop_end", q.loop_end.to_string());
                    e.set("shuffle", fmt_f32(q.shuffle));
                    if l < 6 {
                        e.set("trigmask", (q.trigmask & 0xFFFF).to_string());
                        e.set("trigmask_0to15", (q.trigmask & 0xFFFF).to_string());
                        e.set("trigmask_16to31", ((q.trigmask >> 16) & 0xFFFF).to_string());
                        e.set("trigmask_32to47", ((q.trigmask >> 32) & 0xFFFF).to_string());
                    }
                    ss.children.push(e);
                }
                if let Some(m) = &self.msegs[s][l] {
                    ms.children.push(write_mseg(m, s, l));
                }
                if let Some(f) = &self.formulae[s][l] {
                    let b64 = makepad_base64::base64_encode(f.as_bytes(), &makepad_base64::BASE64_STANDARD);
                    fo.children.push(Element::new("formula").with("scene", s.to_string()).with("i", l.to_string()).with("formula", String::from_utf8_lossy(&b64).into_owned()).with("interpreter", "1001"));
                }
                el.children.push(Element::new("lfo").with("scene", s.to_string()).with("i", l.to_string()).with("extraAmplitude", self.extra_lfo[s][l].to_string()));
            }
        }
        root.children.extend([ss, ms, fo, el]);
        let mut cc = Element::new("customcontroller");
        for (i, m) in self.macros.iter().enumerate() {
            cc.children.push(Element::new("entry").with("i", i.to_string()).with("bipolar", (m.bipolar as i32).to_string()).with("v", fmt_f32(m.v)).with("label", &m.label));
        }
        root.children.push(cc);
        let mut lb = Element::new("lfobanklabels");
        for l in &self.lfo_labels {
            lb.children.push(Element::new("label").with("lfo", l.lfo.to_string()).with("idx", l.idx.to_string()).with("scene", l.scene.to_string()).with("v", &l.text));
        }
        root.children.push(lb);
        root.children.push(Element::new("modwheel").with("s0", fmt_f32(self.modwheel[0])).with("s1", fmt_f32(self.modwheel[1])));
        let mut comp = Element::new("compatability");
        comp.children.push(Element::new("correctlyTunedCombFilter").with("v", (self.correct_comb as i32).to_string()));
        root.children.push(comp);
        if let Some(t) = &self.tuning {
            root.children.push(Element::new("patchTuning").with("v", &t.scl_b64).with("m", &t.kbm_b64).with("mname", &t.mapping_name));
        }
        root.children.push(Element::new("tempoOnSave").with("v", fmt_f32(self.tempo_on_save.unwrap_or(120.0))));
        root.children.push(Element::new("dawExtraState").with("populated", "0"));
        xml::write(&root)
    }

    /// Which flags parameter `idx`'s kind carries: (extend, deactivate, deform, temposync, absolute).
    fn flag_kinds(&self, idx: usize, info: &catalog::ParamInfo) -> (bool, bool, bool, bool, bool) {
        match info.owner {
            Owner::OscSlot { scene, osc, slot } => {
                let d = slots::osc_slot_defs(self.osc_type(scene as usize, osc as usize))[slot as usize];
                (d.can_extend, d.can_deactivate, !d.deform.is_empty(), d.can_temposync, d.can_absolute)
            }
            Owner::FxSlot { fx, slot } => {
                let d = slots::fx_slot_defs(self.fx_type(fx as usize))[slot as usize];
                (d.can_extend, d.can_deactivate, !d.deform.is_empty(), d.can_temposync, d.can_absolute)
            }
            _ => {
                let _ = idx;
                (info.can_extend, info.can_deactivate, info.has_deform, info.can_temposync, info.can_absolute)
            }
        }
    }

    /// The `.fxp` file (container + `sub3` payload).
    pub fn to_fxp(&self) -> Vec<u8> {
        let payload = self.to_payload();
        let mut out = Vec::with_capacity(60 + payload.len());
        out.extend_from_slice(b"CcnK");
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(b"FPCh");
        out.extend_from_slice(&1u32.to_be_bytes());
        out.extend_from_slice(b"cjs3");
        out.extend_from_slice(&1u32.to_be_bytes());
        out.extend_from_slice(&1u32.to_be_bytes());
        let mut name = [0u8; 28];
        for (i, b) in self.name.bytes().take(28).enumerate() {
            name[i] = b;
        }
        out.extend_from_slice(&name);
        out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        out.extend_from_slice(&payload);
        out
    }

    /// The `sub3` payload. Block storage is written only when the patch
    /// carried one (kept as read, uncompressed decoding is not re-encoded).
    pub fn to_payload(&self) -> Vec<u8> {
        let xml = self.to_xml();
        let blocks: Vec<Vec<u8>> = (0..6).map(|k| self.tables[k / 3][k % 3].as_ref().map(|t| t.write()).unwrap_or_default()).collect();
        let mut out = Vec::new();
        out.extend_from_slice(b"sub3");
        out.extend_from_slice(&(xml.len() as u32).to_le_bytes());
        for b in &blocks {
            out.extend_from_slice(&(b.len() as u32).to_le_bytes());
        }
        out.extend_from_slice(xml.as_bytes());
        for b in &blocks {
            out.extend_from_slice(b);
        }
        out
    }
}

/// FILTERS.md section 9 / PATCH-FORMAT section 6 for one filter unit
/// (all but the subtype clamp, which runs after).
pub fn migrate_filter(rev: i32, ty: &mut i32, sub: &mut i32, reso: &mut f32) {
    if rev < 3 {
        match *ty {
            3 => *sub = 3,
            8 => *sub = 1,
            9 => {
                *ty = 8;
                *sub = 3;
            }
            _ => {}
        }
    }
    if rev == 3 && *ty == 9 {
        *ty = 8;
        *sub += 2;
    }
    if rev < 6 {
        match *ty {
            1 | 4 | 6 => {
                let r = *reso;
                let qi = 1.0 - 0.99 * (1.0 - (1.0 - r) * (1.0 - r)).clamp(0.0, 1.0);
                *reso = 1.0 - (1.0 - (1.0 - qi) / 1.05).max(0.0).sqrt();
            }
            2 | 5 => *reso *= 0.99 / 1.05,
            _ => {}
        }
    }
    if rev < 8 {
        match *ty {
            1 | 2 | 4 | 5 | 6 => *sub = if rev < 6 { 0 } else { 1 },
            7 => *sub = 1,
            _ => {}
        }
    }
    if rev < 15 {
        match *ty {
            11 => {
                let s = *sub;
                *ty = [11, 22, 20, 21][(s.rem_euclid(4)) as usize];
                *sub = if s < 4 { 0 } else { 1 };
            }
            6 if (3..=5).contains(sub) => {
                *ty = 23;
                *sub -= 3;
            }
            7 if (2..=3).contains(sub) => {
                *ty = 24;
                *sub -= 2;
            }
            8 if (2..=3).contains(sub) => {
                *ty = 25;
                *sub -= 2;
            }
            _ => {}
        }
    }
    if rev <= 26 {
        match (*ty, *sub) {
            (12, 3) => *sub = 4,
            (6, 2) => *sub = 4,
            (6, 1) => *sub = 3,
            _ => {}
        }
    }
    if *ty >= 36 {
        *ty = 0;
    }
}

fn scene_lfo(e: &Element) -> Option<(usize, usize)> {
    let s = e.attr_i64("scene")?;
    let l = e.attr_i64("i")?;
    ((0..2).contains(&s) && (0..12).contains(&l)).then_some((s as usize, l as usize))
}

fn read_mseg(e: &Element, rev: i32) -> MsegData {
    let n = e.attr_i64("activeSegments").unwrap_or(0).clamp(0, 256) as usize;
    let segs: Vec<&Element> = e.child("segments").map(|s| s.children_named("segment").collect()).unwrap_or_default();
    let segments = (0..n)
        .map(|k| {
            let Some(s) = segs.get(k) else { return MsegSegment { use_deform: true, ty: 1, ..Default::default() } };
            let f = |a: &str| s.attr_f64(a).unwrap_or(0.0) as f32;
            MsegSegment {
                duration: f("duration"),
                v0: f("v0"),
                nv1: f("nv1"),
                cpduration: f("cpduration"),
                cpv: f("cpv"),
                ty: s.attr_i64("type").unwrap_or(1) as i32,
                use_deform: s.attr_i64("useDeform").unwrap_or(1) != 0,
                invert_deform: s.attr_i64("invertDeform").unwrap_or(0) != 0,
                retrigger_feg: s.attr_i64("retriggerFEG").unwrap_or(0) != 0,
                retrigger_aeg: s.attr_i64("retriggerAEG").unwrap_or(0) != 0,
            }
        })
        .collect();
    let _ = rev;
    MsegData {
        segments,
        endpoint_mode: e.attr_i64("endpointMode").unwrap_or(2) as i32,
        edit_mode: e.attr_i64("editMode").unwrap_or(0) as i32,
        loop_mode: e.attr_i64("loopMode").unwrap_or(2) as i32,
        loop_start_point: e.attr_i64("loopStartPoint").map(|v| v as i32),
        loop_end_point: e.attr_i64("loopEndPoint").map(|v| v as i32),
        legacy_loop_start: e.attr_i64("loopStart").map(|v| v as i32),
        legacy_loop_end: e.attr_i64("loopEnd").map(|v| v as i32),
        hsnap_default: e.attr_f64("hSnapDefault").unwrap_or(0.125) as f32,
        vsnap_default: e.attr_f64("vSnapDefault").unwrap_or(0.25) as f32,
        hsnap: e.attr_f64("hSnap").unwrap_or(0.0) as f32,
        vsnap: e.attr_f64("vSnap").unwrap_or(0.0) as f32,
        axis_width: e.attr_f64("axisWidth").unwrap_or(-1.0) as f32,
        axis_start: e.attr_f64("axisStart").unwrap_or(-1.0) as f32,
    }
}

fn write_mseg(m: &MsegData, s: usize, l: usize) -> Element {
    let mut e = Element::new("mseg")
        .with("scene", s.to_string())
        .with("i", l.to_string())
        .with("activeSegments", m.segments.len().to_string())
        .with("endpointMode", m.endpoint_mode.to_string())
        .with("editMode", m.edit_mode.to_string())
        .with("loopMode", m.loop_mode.to_string())
        .with("hSnapDefault", fmt_f32(m.hsnap_default))
        .with("vSnapDefault", fmt_f32(m.vsnap_default))
        .with("hSnap", fmt_f32(m.hsnap))
        .with("vSnap", fmt_f32(m.vsnap))
        .with("axisWidth", fmt_f32(m.axis_width))
        .with("axisStart", fmt_f32(m.axis_start));
    if let Some(v) = m.loop_start_point {
        e.set("loopStartPoint", v.to_string());
    }
    if let Some(v) = m.loop_end_point {
        e.set("loopEndPoint", v.to_string());
    }
    if let Some(v) = m.legacy_loop_start {
        e.set("loopStart", v.to_string());
    }
    if let Some(v) = m.legacy_loop_end {
        e.set("loopEnd", v.to_string());
    }
    let mut segs = Element::new("segments");
    for g in &m.segments {
        segs.children.push(
            Element::new("segment")
                .with("duration", fmt_f32(g.duration))
                .with("v0", fmt_f32(g.v0))
                .with("nv1", fmt_f32(g.nv1))
                .with("cpduration", fmt_f32(g.cpduration))
                .with("cpv", fmt_f32(g.cpv))
                .with("type", g.ty.to_string())
                .with("useDeform", (g.use_deform as i32).to_string())
                .with("invertDeform", (g.invert_deform as i32).to_string())
                .with("retriggerFEG", (g.retrigger_feg as i32).to_string())
                .with("retriggerAEG", (g.retrigger_aeg as i32).to_string()),
        );
    }
    e.children.push(segs);
    e
}

/// A float as the shortest text that reads back to the same f32.
pub fn fmt_f32(v: f32) -> String {
    if v.is_nan() {
        "nan".into()
    } else if v.is_infinite() {
        (if v > 0.0 { "inf" } else { "-inf" }).into()
    } else {
        format!("{v}")
    }
}

fn fmt_value(v: &SlotValue) -> String {
    if v.int { (v.v as i32).to_string() } else { fmt_f32(v.v) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny(rev: i32, body: &str) -> String {
        format!("<?xml version=\"1.0\"?><patch revision=\"{rev}\"><meta name=\"T\" category=\"Leads\"/><parameters>{body}</parameters></patch>")
    }

    #[test]
    fn init_defaults() {
        let p = Patch::init();
        assert_eq!(p.scene(0, sc::VOLUME).v, 0.890899);
        assert!(!p.scene(0, sc::VOLUME).deactivated);
        assert!(!p.scene(0, sc::LOWCUT).deactivated);
        assert_eq!(p.scene(1, sc::MIX + 5).v, 1.0, "osc2 muted");
        assert_eq!(p.scene(0, sc::MIX).v, 1.0, "osc1 level");
        assert_eq!(p.scene(0, sc::ENV + 5).v, -5.0);
        assert_eq!(p.global(gl::POLYLIMIT).v, 16.0);
    }

    #[test]
    fn reads_values_flags_routings() {
        let x = tiny(
            20,
            r#"<a_filter1_cutoff type="2" value="12.5" extend_range="1"><modrouting source="6" depth="24" muted="0" source_index="0"/></a_filter1_cutoff>
               <a_lfo0_rate type="2" value="1" temposync="1"/><a_polymode type="0" value="1"/><a_portamento type="2" value="-2" porta_curve="1" porta_gliss="1"/>
               <fx1_type type="0" value="1"/><fx1_p0 type="2" value="-1.5"/><bogus type="2" value="1"/>"#,
        );
        let p = Patch::from_payload(x.as_bytes(), LoadMode::Patch).unwrap();
        assert_eq!(p.name, "T");
        assert_eq!(p.scene(0, sc::F1_CUTOFF).v, 12.5);
        assert!(p.scene(0, sc::F1_CUTOFF).extend);
        assert!(p.scene(0, sc::LFO + 1).temposync);
        assert_eq!(p.scene(0, sc::POLYMODE).i(), 1);
        assert_eq!(p.porta[0].curve, 1);
        assert!(p.porta[0].gliss);
        assert_eq!(p.routings.len(), 1);
        assert_eq!(p.routings[0].source, 6);
        assert_eq!(p.routings[0].depth, 24.0);
        assert_eq!(p.fx_type(0), 1);
        assert_eq!(p.unknown, vec!["bogus".to_string()]);
        // Round trip.
        let back = Patch::from_payload(p.to_payload().as_slice(), LoadMode::Patch).unwrap();
        assert_eq!(back.values, p.values);
        assert_eq!(back.routings, p.routings);
        assert_eq!(back.porta, p.porta);
    }

    #[test]
    fn migrations() {
        // rev 2 v14-era filter: type 9 -> comb (8) subtype 3; rev < 6 LP24 resonance scale; old macro shift.
        let x = tiny(
            2,
            r#"<a_filter1_type type="0" value="9"/><a_filter2_type type="0" value="2"/><a_filter2_resonance type="2" value="0.5"/>
               <a_pitch type="2" value="0"><modrouting source="14" depth="1"/></a_pitch>"#,
        );
        let p = Patch::from_payload(x.as_bytes(), LoadMode::Patch).unwrap();
        // rev < 3: type 9 -> comb+ subtype 3; rev < 15: comb subtype 2..3 -> comb- (25), subtype - 2.
        assert_eq!(p.scene(0, sc::F1_TYPE).i(), 25);
        assert_eq!(p.scene(0, sc::F1_SUBTYPE).i(), 1);
        assert!((p.scene(0, sc::F2_RESO).v - 0.5 * 0.99 / 1.05).abs() < 1e-6);
        assert_eq!(p.scene(0, sc::F2_SUBTYPE).i(), 0, "rev < 6: standard");
        assert_eq!(p.routings[0].source, 15);
        assert_eq!(p.values[Catalog::CHARACTER].i(), 0, "rev < 10: warm");
        let mut ty = 11;
        let mut sub = 6;
        let mut r = 0.0;
        migrate_filter(14, &mut ty, &mut sub, &mut r);
        assert_eq!((ty, sub), (20, 1));
    }

    #[test]
    fn container_and_wavetable_round_trip() {
        let mut p = Patch::init();
        p.name = "Wave".into();
        *p.scene_mut(0, sc::OSC) = SlotValue::int(2);
        p.tables[0][0] = Some(Wavetable { samples_per_frame: 4, frames: 2, flags: WT_INT16, data_i16: vec![0, 100, -100, 16383, 1, 2, 3, 4], data_f32: vec![] });
        let fxp = p.to_fxp();
        let q = Patch::from_fxp(&fxp, "file", LoadMode::Patch).unwrap();
        assert_eq!(q.tables[0][0], p.tables[0][0]);
        assert_eq!(q.name, "Wave");
        assert_eq!(q.tables[0][0].as_ref().unwrap().to_f32()[3], 16383.0 / 16384.0);
    }

    #[test]
    fn pitch_bend_float_quirk() {
        let bits = 12.0f32.to_bits();
        let x = tiny(10, &format!(r#"<a_pbrange_up type="0" value="{bits}"/><a_pbrange_dn type="0" value="{bits}"/><b_pbrange_up type="0" value="{}"/>"#, 2.0f32.to_bits()));
        let p = Patch::from_payload(x.as_bytes(), LoadMode::Patch).unwrap();
        assert_eq!(p.scene(0, sc::PBRANGE_UP).i(), 12);
        assert_eq!(p.scene(1, sc::PBRANGE_UP).i(), 2);
    }
}
