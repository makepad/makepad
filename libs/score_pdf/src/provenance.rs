//! Reversible edit log and deterministic provenance serialization.

use crate::sha256::hex;
use crate::confidence::Estimate;
use crate::splice::{ErasePlan, FontUseDecision, NoteEdit, SplicePlan};
use crate::RecognizedDocument;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct EditId(pub u64);

#[derive(Clone, Debug, PartialEq)]
pub struct AppliedEdit {
    pub id: EditId,
    pub plan: SplicePlan,
    pub inverse: NoteEdit,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditLog {
    pub edits: Vec<AppliedEdit>,
}

pub(crate) fn record_plan(document: &mut RecognizedDocument, plan: SplicePlan) -> EditId {
    let id = EditId(document.edits.edits.len() as u64 + 1);
    let inverse = NoteEdit {
        page: plan.page,
        note: plan.note,
        pitch: plan.before_pitch,
        duration: Some(plan.before_duration),
    };
    document.edits.edits.push(AppliedEdit { id, plan, inverse });
    id
}

pub fn canonical_provenance_json(
    document: &RecognizedDocument,
    created_objects: &[(u32, [u8; 32])],
) -> Vec<u8> {
    let mut output = String::new();
    output.push_str("{\"format\":\"makepad-score-pdf-edit\",\"version\":1,");
    output.push_str("\"original_sha256\":\"");
    output.push_str(&hex(&document.original_sha256));
    output.push_str("\",\"tool\":\"makepad-score-pdf/1.0.0\",\"created_objects\":[");
    for (index, (object, hash)) in created_objects.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        output.push_str(&format!(
            "{{\"object\":{},\"sha256\":\"{}\"}}",
            object,
            hex(hash)
        ));
    }
    output.push_str("],\"edits\":[");
    for (index, edit) in document.edits.edits.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        let plan = &edit.plan;
        output.push_str(&format!(
            "{{\"id\":{},\"page\":{},\"note\":{},\"scope\":\"{:?}\",",
            edit.id.0, plan.page.0, plan.note.0, plan.scope
        ));
        output.push_str(&format!(
            "\"patch\":[{:.6},{:.6},{:.6},{:.6}],",
            plan.patch_bounds.min_x,
            plan.patch_bounds.min_y,
            plan.patch_bounds.max_x,
            plan.patch_bounds.max_y
        ));
        output.push_str("\"before\":{");
        push_pitch(&mut output, plan.before_pitch);
        output.push_str(&format!(
            ",\"duration\":[{},{},{}]}},\"after\":{{",
            plan.before_duration.numerator,
            plan.before_duration.denominator,
            plan.before_duration.dots
        ));
        push_pitch(&mut output, plan.after_pitch);
        output.push_str(&format!(
            ",\"duration\":[{},{},{}]}},",
            plan.after_duration.numerator,
            plan.after_duration.denominator,
            plan.after_duration.dots
        ));
        output.push_str("\"primitives\":[");
        for (primitive_index, primitive) in plan.affected_primitives.iter().enumerate() {
            if primitive_index > 0 {
                output.push(',');
            }
            output.push_str(&primitive.0.to_string());
        }
        output.push_str("],\"sources\":[");
        if let Some(page) = document.pages.get(plan.page.0 as usize) {
            for (source_index, source) in plan
                .affected_primitives
                .iter()
                .filter_map(|primitive| page.display.primitive(*primitive))
                .map(|primitive| primitive.source())
                .enumerate()
            {
                if source_index > 0 {
                    output.push(',');
                }
                output.push_str(&format!(
                    "{{\"object\":[{},{}],\"stream\":{},\"bytes\":[{},{}],\"operator\":{},\"subpath\":{} ,\"forms\":[",
                    source.object.num,
                    source.object.gen,
                    source.stream_index,
                    source.decoded_bytes.start,
                    source.decoded_bytes.end,
                    source.operator_index,
                    source.subpath_index.map_or("null".to_string(), |value| value.to_string())
                ));
                for (hop_index, hop) in source.form_chain.iter().enumerate() {
                    if hop_index > 0 {
                        output.push(',');
                    }
                    output.push_str(&format!(
                        "{{\"name\":\"{}\",\"object\":[{},{}],\"operator\":{}}}",
                        escape(&hop.name), hop.object.num, hop.object.gen, hop.invocation_operator
                    ));
                }
                output.push_str("]}");
            }
        }
        output.push_str("],\"operator_edits\":[");
        if let ErasePlan::OperatorRewrite { edits, .. } = &plan.erase {
            for (operator_index, operator) in edits.iter().enumerate() {
                if operator_index > 0 {
                    output.push(',');
                }
                output.push_str(&format!(
                    "{{\"object\":[{},{}],\"stream\":{},\"bytes\":[{},{}],\"operator\":{},\"replacement_hex\":\"{}\"}}",
                    operator.source.object.num,
                    operator.source.object.gen,
                    operator.source.stream_index,
                    operator.source.decoded_bytes.start,
                    operator.source.decoded_bytes.end,
                    operator.source.operator_index,
                    hex_bytes(&operator.replacement)
                ));
            }
        }
        output.push_str("],\"style\":{");
        output.push_str(&format!(
            "\"staff_space\":{:.6},\"staff_line\":{:.6},\"stem\":{:.6},\"beam\":{:.6},\"notehead\":[{:.6},{:.6}],\"dot\":{:.6},\"curve\":{:.6},\"ink_gray\":{:.6}}},",
            plan.style.staff_space,
            plan.style.staff_line_thickness,
            plan.style.stem_thickness,
            plan.style.beam_thickness,
            plan.style.notehead_width,
            plan.style.notehead_height,
            plan.style.dot_diameter,
            plan.style.curve_thickness,
            plan.style.ink_gray
        ));
        output.push_str("\"onset_shifts\":[");
        for (shift_index, (note, shift)) in plan.onset_shifts.iter().enumerate() {
            if shift_index > 0 {
                output.push(',');
            }
            output.push_str(&format!("[{},{}]", note.0, shift));
        }
        output.push_str("],\"warnings\":[");
        for (warning_index, warning) in plan.warnings.iter().enumerate() {
            if warning_index > 0 {
                output.push(',');
            }
            output.push_str(&format!("\"{:?}\"", warning));
        }
        output.push_str("],\"confidence\":");
        if let Some(binding) = document
            .bindings
            .iter()
            .find(|binding| binding.page == plan.page && binding.semantic == plan.note)
        {
            push_confidence(&mut output, &binding.confidence);
        } else {
            output.push_str("null");
        }
        output.push(',');
        output.push_str("\"font\":");
        push_font(&mut output, &plan.font);
        output.push_str(",\"inverse\":{");
        push_pitch(&mut output, edit.inverse.pitch);
        output.push_str(&format!(
            ",\"duration\":[{},{},{}]}}}}",
            edit.inverse.duration.map_or(0, |value| value.numerator),
            edit.inverse.duration.map_or(1, |value| value.denominator),
            edit.inverse.duration.map_or(0, |value| value.dots)
        ));
    }
    output.push_str("]}");
    output.into_bytes()
}

fn push_confidence(output: &mut String, confidence: &crate::ElementConfidence) {
    output.push('{');
    push_estimate(output, "class", &confidence.class, &confidence.class.value);
    output.push(',');
    match &confidence.pitch {
        Some(estimate) => {
            let value = format!(
                "{:?}{}{}",
                estimate.value.step, estimate.value.alter.0, estimate.value.octave
            );
            push_estimate(output, "pitch", estimate, &value);
        }
        None => output.push_str("\"pitch\":null"),
    }
    output.push(',');
    match &confidence.duration {
        Some(estimate) => {
            let value = format!(
                "{}/{}/{}",
                estimate.value.numerator, estimate.value.denominator, estimate.value.dots
            );
            push_estimate(output, "duration", estimate, &value);
        }
        None => output.push_str("\"duration\":null"),
    }
    output.push(',');
    match &confidence.voice {
        Some(estimate) => {
            push_estimate(output, "voice", estimate, &estimate.value.to_string())
        }
        None => output.push_str("\"voice\":null"),
    }
    output.push_str(",\"attachments\":[");
    for (index, estimate) in confidence.attachments.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        push_estimate_value(output, estimate, &estimate.value);
    }
    output.push_str("]}");
}

fn push_estimate<T: std::fmt::Debug>(
    output: &mut String,
    name: &str,
    estimate: &Estimate<T>,
    value: &str,
) {
    output.push_str(&format!("\"{}\":", escape(name)));
    push_estimate_value(output, estimate, value);
}

fn push_estimate_value<T: std::fmt::Debug>(
    output: &mut String,
    estimate: &Estimate<T>,
    value: &str,
) {
    output.push_str(&format!(
        "{{\"value\":\"{}\",\"probability\":{:.6},\"margin\":{:.6},\"verification\":\"{:?}\",\"evidence\":[",
        escape(value), estimate.probability, estimate.runner_up_margin, estimate.verification
    ));
    for (index, evidence) in estimate.evidence.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        output.push_str(&format!("\"{}\"", escape(&format!("{evidence:?}"))));
    }
    output.push_str("]}");
}

fn hex_bytes(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

fn push_pitch(output: &mut String, pitch: Option<makepad_score::model::Pitch>) {
    output.push_str("\"pitch\":");
    match pitch {
        Some(pitch) => output.push_str(&format!(
            "{{\"step\":\"{:?}\",\"alter\":\"{}\",\"octave\":{}}}",
            pitch.step, pitch.alter.0, pitch.octave
        )),
        None => output.push_str("null"),
    }
}

fn push_font(output: &mut String, font: &FontUseDecision) {
    match font {
        FontUseDecision::OriginalVerified {
            font_resource,
            license,
        } => output.push_str(&format!(
            "{{\"decision\":\"original-verified\",\"font\":\"{}\",\"license\":\"{}\",\"source\":\"{}\"}}",
            escape(font_resource),
            escape(&license.spdx),
            escape(&license.source)
        )),
        FontUseDecision::OriginalViewOnly {
            font_resource,
            reason,
        } => output.push_str(&format!(
            "{{\"decision\":\"original-view-only\",\"font\":\"{}\",\"reason\":\"{:?}\"}}",
            escape(font_resource), reason
        )),
        FontUseDecision::FallbackOfl {
            family,
            license,
            visual_delta,
        } => output.push_str(&format!(
            "{{\"decision\":\"ofl-substitution\",\"font\":\"{}\",\"license\":\"{}\",\"visual_delta\":{:.6}}}",
            escape(family), escape(license), visual_delta
        )),
    }
}

fn escape(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_control() => {
                output.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => output.push(character),
        }
    }
    output
}
