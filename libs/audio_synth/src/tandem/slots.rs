//! Where the patch loader meets the type-dependent modules: oscillator and
//! FX slot kinds and migrations, filter subtype counts, the block-storage
//! decompressor.

use crate::kit::core::{SlotDef, SlotValue};
use crate::kit::{flt, fxs, oscs};

pub fn osc_slot_defs(ty: u8) -> [SlotDef; 7] {
    oscs::slot_defs(ty)
}

/// The oscillator's own stream migration, then its patch-level slot rules.
pub fn osc_migrate(ty: u8, rev: i32, slots: &mut [SlotValue; 7], retrigger: &mut bool, table_is_sample: bool) {
    oscs::migrate(ty, rev, slots, retrigger, table_is_sample);
    oscs::migrate_patch(ty, rev, slots, table_is_sample);
}

pub fn fx_slot_defs(ty: u8) -> [SlotDef; 12] {
    fxs::slot_defs(ty)
}

/// Clamp the used slots into the type's range (extended slots keep theirs).
pub fn fx_clamp(ty: u8, p: &mut [SlotValue; 12]) {
    fxs::clamp_values(ty, p);
}

pub fn fx_migrate(ty: u8, rev: i32, p: &mut [SlotValue; 12]) {
    fxs::migrate(ty, rev, p);
}

pub fn filter_subtype_count(ty: i32) -> i32 {
    if (0..36).contains(&ty) { flt::subtype_count(ty as u8) as i32 } else { 0 }
}

/// The Wavetable oscillator's unison-voices slot (its deform holds the
/// sample play count).
pub const WT_VOICES_SLOT: usize = 6;

pub fn zstd_decode(input: &[u8], max: usize) -> Result<Vec<u8>, String> {
    makepad_zstd_decode::decode(input, max)
}
