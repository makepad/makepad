//! Tandem's parameters in the kit's one schema (key, label, group, range,
//! default, curve, unit, choices, modulatable, one line for the AI), built
//! from the catalogue; the type-dependent slots (oscillator param0..6, FX
//! p0..p11) are described per type from the modules' own slot definitions.

use super::catalog::{catalog, Owner};
use crate::kit::core::{SlotDef, SlotUnit};
use crate::kit::schema::{Builder, Curve, Schema, Unit};
use crate::kit::{flt, fxs, oscs, shaper};
use std::sync::OnceLock;

/// How a Tandem node is written, for the AI.
pub const STRUCTURE: &str = "Tandem{patch: \"Category/Name\" a_filter1_cutoff: 12 ...}: plays a patch of the hybrid patch library (tandem_patches finds them) with any parameter set over it by storage name in raw units. Two scenes (a_ and b_, scenemode picks single/split/dual), each: 3 oscillators (a_osc1_type: 0 classic 1 sine 2 wavetable 3 S&H noise 4 audio in 5 FM3 6 FM2 7 window 8 modern 9 string 10 twist 11 alias; their param0..6 mean per type: ask tandem_params with osc_type), ring mods and noise into a mixer (level is amplitude, gain = v^3), a filter block (a_fb_config, 2 filters a_filter1_type 0..35 with subtypes, a waveshaper a_ws_type 0..42, feedback), the amp/filter envelopes env1/env2 (times log2 seconds), 6 voice + 6 scene LFOs (lfo0..lfo11, rate log2 Hz). 16 FX slots fxN_type (inserts, sends, globals; pN per type: ask with fx_type). Pitches and cutoffs are notes vs A440 (0 = 440 Hz, 12 = an octave).";

/// Group of a catalogue parameter (by its short name).
pub fn group_of(short: &str, owner: Owner) -> String {
    let scene = match owner {
        Owner::Scene(s) | Owner::OscSlot { scene: s, .. } => Some(if s == 0 { "a" } else { "b" }),
        _ => None,
    };
    let g = if let Owner::FxSlot { fx, .. } = owner {
        format!("fx{}", fx + 1)
    } else if short.starts_with("fx") && short.ends_with("_type") {
        format!("fx{}", short.trim_start_matches("fx").trim_end_matches("_type"))
    } else if short.starts_with("osc") {
        short[..4].to_string()
    } else if short.starts_with("lfo") {
        short.split('_').next().unwrap_or("lfo").to_string()
    } else if short.starts_with("env") {
        short[..4].to_string()
    } else if short.starts_with("filter") || short.starts_with("f2_") || short == "fb_config" || short == "feedback" || short == "f_balance" || short.starts_with("ws_") {
        "filters".into()
    } else if ["level_", "mute_", "solo_", "route_"].iter().any(|p| short.starts_with(p)) || short == "noisecol" {
        "mixer".into()
    } else if scene.is_some() {
        "scene".into()
    } else {
        "global".into()
    };
    match scene {
        Some(s) => format!("{s}_{g}"),
        None => g,
    }
}

fn unit_note(u: SlotUnit, about: &'static str) -> &'static str {
    if !about.is_empty() {
        return about;
    }
    u.about()
}

/// The whole schema (type-dependent slots as generic 0..1 entries; their
/// meaning per type is [`describe_osc`] / [`describe_fx`]).
pub fn schema() -> &'static Schema {
    static S: OnceLock<Schema> = OnceLock::new();
    S.get_or_init(|| {
        let mut b = Builder::new();
        // The LFO shape's line carries the formula shape's guide.
        let shape_about: &'static str = Box::leak(format!("0 sine 1 tri 2 square 3 saw 4 noise 5 S&H 6 envelope 7 step seq 8 MSEG 9 formula: {}", crate::kit::formula::GUIDE).into_boxed_str());
        for p in &catalog().params {
            let group = group_of(p.short, p.owner);
            b.group(&group, "");
            let (curve, unit) = p.unit.schema(p.int, false);
            let curve = if p.int && p.min == 0.0 && p.max == 1.0 { Curve::Bool } else { curve };
            let modulatable = !p.int && !matches!(p.owner, Owner::Global if p.short.starts_with("fx"));
            let label = p.short.rsplit('_').next().unwrap_or(p.short);
            let about = if p.short.starts_with("lfo") && p.short.ends_with("_shape") { shape_about } else { unit_note(p.unit, p.about) };
            b.add(p.name.clone(), label, p.min, p.max, p.default, curve, unit, &[], modulatable, about);
        }
        b.finish()
    })
}

fn slot_line(key: &str, d: &SlotDef) -> String {
    let range = if !d.choices.is_empty() {
        d.choices.iter().enumerate().map(|(i, c)| format!("{i} {c}")).collect::<Vec<_>>().join(", ")
    } else {
        let (_, u) = d.unit.schema(d.int, false);
        format!("{}..{}{}", d.min, d.max, if u == Unit::None { String::new() } else { format!(" {}", u.name()) })
    };
    let mut flags = Vec::new();
    if d.can_extend {
        flags.push("extendable");
    }
    if d.can_deactivate {
        flags.push("can be off");
    }
    if d.can_temposync {
        flags.push("tempo-sync");
    }
    let note = unit_note(d.unit, d.about);
    let deform = if d.deform.is_empty() { String::new() } else { format!(" variants: {}", d.deform.join("/")) };
    format!("{key} {}: {range} ={}{}{}{}", d.name, d.default, if flags.is_empty() { String::new() } else { format!(" ({})", flags.join(", ")) }, if note.is_empty() { String::new() } else { format!(": {note}") }, deform)
}

/// What an oscillator type's param0..6 mean.
pub fn describe_osc(ty: u8) -> String {
    let mut out = format!("osc type {ty} {}:\n", oscs::TYPE_NAMES.get(ty as usize).copied().unwrap_or("?"));
    for (k, d) in oscs::slot_defs(ty).iter().enumerate().filter(|(_, d)| d.used()) {
        out.push_str("  ");
        out.push_str(&slot_line(&format!("param{k}"), d));
        out.push('\n');
    }
    out
}

/// What an FX type's p0..p11 mean.
pub fn describe_fx(ty: u8) -> String {
    let mut out = format!("fx type {ty} {}:\n", fxs::TYPE_NAMES.get(ty as usize).copied().unwrap_or("?"));
    for (k, d) in fxs::slot_defs(ty).iter().enumerate().filter(|(_, d)| d.used()) {
        out.push_str("  ");
        out.push_str(&slot_line(&format!("p{k}"), d));
        out.push('\n');
    }
    out
}

/// The type lists (oscillators, filters with subtypes, waveshapers, FX).
pub fn describe_types() -> String {
    let mut out = String::from("oscillators: ");
    out.push_str(&oscs::TYPE_NAMES.iter().enumerate().map(|(i, n)| format!("{i} {n}")).collect::<Vec<_>>().join(", "));
    out.push_str("\nfilters (type: subtypes): ");
    out.push_str(&(0..36u8).map(|t| {
        let subs = flt::subtype_names(t);
        if subs.is_empty() { format!("{t} {}", flt::TYPE_NAMES[t as usize]) } else { format!("{t} {} [{}]", flt::TYPE_NAMES[t as usize], subs.join("/")) }
    }).collect::<Vec<_>>().join(", "));
    out.push_str("\nwaveshapers: ");
    out.push_str(&shaper::SHAPER_NAMES.iter().enumerate().map(|(i, n)| format!("{i} {n}")).collect::<Vec<_>>().join(", "));
    out.push_str("\nfx: ");
    out.push_str(&fxs::TYPE_NAMES.iter().enumerate().map(|(i, n)| format!("{i} {n}")).collect::<Vec<_>>().join(", "));
    out.push_str("\nlfo shape 9 (formula): ");
    out.push_str(crate::kit::formula::GUIDE);
    out.push_str("\nmod sources: ");
    out.push_str(&super::catalog::SOURCE_NAMES.iter().enumerate().skip(1).map(|(i, n)| format!("{i} {n}")).collect::<Vec<_>>().join(" "));
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_covers_the_catalogue() {
        let s = schema();
        assert_eq!(s.params.len(), catalog().params.len());
        let line = s.param("a_filter1_cutoff").unwrap().line();
        assert!(line.starts_with("a_filter1_cutoff -60..70"), "{line}");
        assert_eq!(s.param("a_filter1_cutoff").unwrap().group, "a_filters");
        assert!(describe_osc(0).contains("param0"));
        assert!(describe_fx(1).contains("p0"));
        assert!(describe_types().contains("LP 12 dB"));
    }
}
