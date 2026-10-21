//! The glyphs laid out, per font file, while a trace host records them
//! (`makepad-web-trace`): which glyphs of which font files a program's
//! outlined type uses, so a web export ships only those. Off unless
//! switched on; the layout is the same either way.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

static RECORDING: AtomicBool = AtomicBool::new(false);

#[derive(Default)]
struct Record {
    /// A font file's hash by where its bytes are (fonts are kept for the
    /// program's life: one hash per file).
    hashes: HashMap<(usize, usize), u64>,
    glyphs: BTreeMap<u64, BTreeSet<u16>>,
}

fn record() -> &'static Mutex<Record> {
    static RECORD: OnceLock<Mutex<Record>> = OnceLock::new();
    RECORD.get_or_init(Default::default)
}

/// FNV-1a 64 over a font file's bytes (how a trace names the file).
fn fnv64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

/// Starts (or stops) recording; stopping keeps what was recorded.
pub fn record_glyphs(on: bool) {
    RECORDING.store(on, Ordering::Relaxed);
}

pub(crate) fn recording() -> bool {
    RECORDING.load(Ordering::Relaxed)
}

/// `glyphs` of the font file `data` were laid out.
pub(crate) fn note(data: &[u8], glyphs: impl Iterator<Item = u16>) {
    let mut record = record().lock().unwrap_or_else(PoisonError::into_inner);
    let at = (data.as_ptr() as usize, data.len());
    let hash = *record.hashes.entry(at).or_insert_with(|| fnv64(data));
    record.glyphs.entry(hash).or_default().extend(glyphs);
}

/// The glyphs recorded since the last call: per font file hash, the ids.
pub fn take_recorded_glyphs() -> Vec<(u64, BTreeSet<u16>)> {
    std::mem::take(&mut record().lock().unwrap_or_else(PoisonError::into_inner).glyphs).into_iter().collect()
}

/// The trace host's switch for outlined type's record.
#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn wasm_record_outline_glyphs(on: u32) {
    record_glyphs(on != 0);
}

/// The trace host's read of the record, as draw's `wasm_take_recorded_glyphs`
/// lays it out: `[words, fonts, (file hash, glyph count, glyph id...)...]`.
#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn wasm_take_outline_glyphs() -> u32 {
    let fonts = take_recorded_glyphs();
    let mut words = vec![0u64, fonts.len() as u64];
    for (hash, glyphs) in fonts {
        words.push(hash);
        words.push(glyphs.len() as u64);
        words.extend(glyphs.iter().map(|&id| id as u64));
    }
    words[0] = words.len() as u64;
    Box::leak(words.into_boxed_slice()).as_ptr() as usize as u32
}
